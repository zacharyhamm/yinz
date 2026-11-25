//! Structured counterpart of the ANSI serializer. Both consume the same composed chunks.
use super::*;
use crate::panes::terminal_character::{AnsiStyledUnderline, NamedColor};
use std::collections::BTreeSet;
use yinz_runtime::native_media::{
    MediaBudget, MediaFile, MediaLease, MediaRoot, MediaUsage, Refusal, PORTABLE_FRAME_MEDIA_BYTES,
    SHARED_MEDIA_MIN_BYTES,
};
use yinz_types::render_negotiation as caps;
use yinz_types::{data::PaletteColor, structured_render::*};

#[derive(Default)]
pub struct NativeRenderState {
    pub agreement: caps::Agreement,
    pub negotiated: bool,
    root: Option<MediaRoot>,
    budget: Option<MediaBudget>,
    pub seq: u64,
    pub acknowledged: u64,
    /// Media the client may still be using, each released by the ACK of its frame.
    media: Vec<(u64, Retained)>,
    /// Panes whose image this viewer's budget refused, retried after a releasing ACK.
    deferred_panes: BTreeSet<PaneId>,
    /// Set by the first refusal of an overload episode; cleared by the next staged image.
    overloaded: bool,
    /// Chunked transfers not yet completed or abandoned, in creation order.
    transfers: Vec<PendingTransfer>,
    /// The last wire id assigned. Ids are taken at a transfer's first chunk, so first
    /// chunks carry strictly increasing ids on this attachment.
    last_transfer_id: u64,
    /// The last frame that carried chunk bytes, or ran out of allowance for them. New
    /// chunk bytes wait until the client acknowledges it, unless a whole transfer fits
    /// what remains of the frame's allowance and the whole-transfer allowance.
    chunk_frame: Option<u64>,
    /// Chunk bytes per frame the client has not acknowledged, in frame order.
    unacked_chunks: Vec<(u64, usize)>,
    /// An unstarted transfer returned its lease without an ACK-keyed entry. The next
    /// accepted ACK retries deferred panes as if it had released media.
    released_unretained: bool,
    /// Media bytes one frame may carry to a chunked viewer; `None` is the production value.
    frame_media: Option<usize>,
    /// What remains of the current frame's media allowance.
    frame_media_left: usize,
    cursor: CursorState,
    /// Wire hyperlink ID `index + 1` stands for this link identity on the client.
    links: Vec<LinkKey>,
    /// IDs allocated or retargeted this frame: the only entries a delta frame carries.
    fresh_links: Vec<u8>,
    /// The ID each client cell shows, row-major over the client's terminal.
    viewport_links: Vec<u8>,
    size: Option<Size>,
    presentation: Option<Presentation>,
}

/// A hyperlink as native viewers distinguish it: the program's explicit OSC 8 `id=`, if any,
/// and the URI.
#[derive(Debug, PartialEq, Eq)]
struct LinkKey {
    id: Option<String>,
    uri: String,
}

impl LinkKey {
    fn matches(&self, identity: &crate::panes::LinkIdentity<'_>) -> bool {
        self.id.as_deref() == identity.id && self.uri == identity.uri
    }
}

/// One unit of a chunk in a native frame: a cell, or a group of cells forming one cluster.
struct NativeUnit {
    start: usize,
    end: usize,
    column: usize,
}

/// The blanks written where a unit does not fit before the viewport's right edge.
struct EdgeFill {
    column: usize,
    count: usize,
    /// The first member of the unit that did not fit, whose background the blanks take.
    clipped: usize,
}

/// How one chunk lands in a native frame. The hyperlink release pre-pass and emission share it,
/// so the shadow releases exactly the cells the frame writes.
struct NativeLayout<'a> {
    /// The chunk's cells with width-0 entries folded into the glyph before them.
    cells: Cow<'a, [TerminalCharacter]>,
    /// The units that fit, in order; clipping is per whole unit.
    units: Vec<NativeUnit>,
    fill: Option<EdgeFill>,
}

impl NativeLayout<'_> {
    /// Every client cell the frame writes for this chunk, as (start column, width) spans in
    /// the chunk's logical columns.
    fn spans(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.units
            .iter()
            .flat_map(|unit| {
                let mut column = unit.column;
                self.cells[unit.start..unit.end].iter().map(move |cell| {
                    let span = (column, cell.width());
                    column += cell.width();
                    span
                })
            })
            .chain(self.fill.iter().map(|fill| (fill.column, fill.count)))
    }
}

fn native_layout(chunk: &CharacterChunk, cols: usize, dx: usize) -> NativeLayout<'_> {
    let (cells, grouped) = chunk_units(&chunk.terminal_characters);
    let styles_at = |cell: &TerminalCharacter, column: usize| {
        adjust_styles_for_custom_bg_fg(
            adjust_styles_for_possible_selection(
                chunk.selection_and_colors(),
                chunk.application_styles(*cell.styles),
                chunk.y,
                column,
            ),
            chunk.pane_default_fg,
            chunk.pane_default_bg,
        )
    };
    let column_starts: Vec<usize> = if grouped {
        cells
            .iter()
            .scan(chunk.x, |column, cell| {
                let start = *column;
                *column += cell.width();
                Some(start)
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut units = Vec::new();
    let mut fill = None;
    let mut column = chunk.x;
    let mut index = 0;
    while index < cells.len() {
        // A width-0 entry with no glyph before it in the chunk has nothing to join.
        if cells[index].width() == 0 {
            index += 1;
            continue;
        }
        let end = if grouped {
            grapheme_join::group_end(&cells, index, |previous, next| {
                styles_at(&cells[previous], column_starts[previous])
                    == styles_at(&cells[next], column_starts[next])
            })
            .end
        } else {
            index + 1
        };
        let columns: usize = cells[index..end].iter().map(|cell| cell.width()).sum();
        if column + columns + dx > cols {
            if column + dx < cols {
                fill = Some(EdgeFill {
                    column,
                    count: cols - dx - column,
                    clipped: index,
                });
            }
            break;
        }
        units.push(NativeUnit {
            start: index,
            end,
            column,
        });
        column += columns;
        index = end;
    }
    NativeLayout { cells, units, fill }
}

/// Media retained for the client until the ACK of the frame that last referenced it.
enum Retained {
    /// A staged file the client reads, unlinked on release.
    File(#[expect(dead_code, reason = "held so its drop unlinks the file")] MediaFile),
    /// The client's assembly of a transfer the frame completed, cancelled or cleared.
    Lease(MediaLease),
}

/// One image the server sends in chunks, with its copy of the pixels until the last one.
struct PendingTransfer {
    key: HostImageKey,
    raster_revision: u64,
    width: u32,
    height: u32,
    /// Assigned immediately before the first chunk; `None` means nothing reached the wire.
    wire_id: Option<u64>,
    rgba: Vec<u8>,
    sent: usize,
    lease: MediaLease,
    /// Placements whose image this transfer withholds until it completes.
    waiting: BTreeSet<(PaneId, u64)>,
}

/// The image a transfer carries, as the host state will record it once complete.
#[derive(Clone, Copy, Debug)]
pub struct TransferImage {
    pub key: HostImageKey,
    pub raster_revision: u64,
    pub width: u32,
    pub height: u32,
}

/// What `advance_transfer` emitted for an image this frame.
struct Completed {
    wire_id: u64,
    width: u32,
    height: u32,
    total: u32,
}

/// How a frame carries one image residency.
#[derive(Debug, PartialEq, Eq)]
pub enum Staging {
    /// A file staged under this viewer's budget, released by the consuming frame's ACK.
    Shared(std::path::PathBuf),
    /// Bytes in this frame: a viewer without chunked media and without a shared root, a
    /// small image within the frame's allowance, or a failed write without chunking.
    Inline,
    /// Chunks under this viewer's budget, paced by its ACKs. The lease is released by
    /// the ACK of the frame that completes, cancels or clears the transfer.
    Transfer,
    /// The budget is exhausted; the image waits for a releasing ACK.
    Deferred,
    /// The image exceeds the shared limit and is never sent.
    Dropped,
}

/// Why a frame did not carry an image, for its other placements in the same frame.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Withheld {
    Deferred,
    Dropped,
    /// A transfer is still in progress.
    Waiting,
}

impl NativeRenderState {
    pub fn negotiated(
        agreement: caps::Agreement,
        root: Option<MediaRoot>,
        budget: Option<MediaBudget>,
    ) -> Self {
        Self {
            agreement,
            root,
            budget,
            negotiated: true,
            frame_media_left: PORTABLE_FRAME_MEDIA_BYTES,
            ..Default::default()
        }
    }

    pub fn force_repaint(&mut self) {
        self.size = None;
    }

    /// The wire ID for a hyperlink identity, allocating one when the identity has none. When
    /// all IDs are taken, an ID no client cell shows and no row of this frame has emitted
    /// (`used`) is retargeted; with none free the cell is sent without a link. New and
    /// retargeted IDs are fresh: the frame carries their entries.
    fn link_id(
        &mut self,
        identity: &crate::panes::LinkIdentity<'_>,
        used: &[bool; MAX_LINK_ID as usize + 1],
    ) -> u8 {
        if let Some(index) = self.links.iter().position(|key| key.matches(identity)) {
            return (index + 1) as u8;
        }
        let key = LinkKey {
            id: identity.id.map(str::to_owned),
            uri: identity.uri.to_owned(),
        };
        let index = if self.links.len() < MAX_LINK_ID as usize {
            self.links.push(key);
            self.links.len() - 1
        } else {
            let Some(index) = (0..self.links.len()).find(|index| !used[index + 1]) else {
                return LINK_NONE;
            };
            self.links[index] = key;
            index
        };
        let id = (index + 1) as u8;
        if !self.fresh_links.contains(&id) {
            self.fresh_links.push(id);
        }
        id
    }

    fn chunked(&self) -> bool {
        self.agreement.media & caps::CHUNKED != 0
    }

    /// Keep only what the client can hold through `high`: frames above it never reach the
    /// client. Started transfers keep their leases until ACK(`high`) or removal, because
    /// the client may hold a partial assembly; unstarted ones are dropped at once.
    pub fn retain_prefix(&mut self, high: u64) {
        self.seq = self.seq.min(high);
        let media = std::mem::take(&mut self.media);
        self.media = media
            .into_iter()
            .filter_map(|(seq, retained)| match retained {
                Retained::File(_) if seq > high => None,
                Retained::Lease(lease) if seq > high => Some((high, Retained::Lease(lease))),
                retained => Some((seq, retained)),
            })
            .collect();
        for transfer in std::mem::take(&mut self.transfers) {
            if transfer.wire_id.is_some() {
                self.media.push((high, Retained::Lease(transfer.lease)));
            }
        }
        self.unacked_chunks.retain(|(seq, _)| *seq <= high);
        self.chunk_frame = None;
        self.deferred_panes.clear();
        self.released_unretained = false;
    }

    /// The viewer's retained-media budget. Chunked transfer buffers and assembled images
    /// reserve from it under the same deferral rule as staged files.
    pub fn media_budget(&self) -> Option<&MediaBudget> {
        self.budget.as_ref()
    }

    pub fn media_usage(&self) -> MediaUsage {
        self.budget
            .as_ref()
            .map(MediaBudget::usage)
            .unwrap_or_default()
    }

    /// Start a new frame's media allowance.
    fn begin_frame(&mut self) {
        self.frame_media_left = self.frame_allowance();
    }

    fn frame_allowance(&self) -> usize {
        self.frame_media.unwrap_or(PORTABLE_FRAME_MEDIA_BYTES)
    }

    /// Un-ACKed chunk bytes outside the chunk window's frame, which whole transfers sent
    /// past a closed window may raise to one frame allowance. With the window's own frame,
    /// at most two allowances of chunk bytes wait in the writer queue.
    fn unacked_whole_bytes(&self) -> usize {
        self.unacked_chunks
            .iter()
            .filter(|(seq, _)| Some(*seq) != self.chunk_frame)
            .map(|(_, len)| len)
            .sum()
    }

    /// Reserve `len` bytes, or report the refusal once per overload episode and return
    /// how the image is withheld.
    fn reserve(&mut self, len: usize) -> std::result::Result<MediaLease, Staging> {
        let budget = self.budget.get_or_insert_with(MediaBudget::viewer);
        match budget.try_reserve(len) {
            Ok(lease) => Ok(lease),
            Err(refusal) => {
                if !self.overloaded {
                    self.overloaded = true;
                    let action = match refusal {
                        Refusal::TooLarge => "dropping",
                        Refusal::Entries | Refusal::Bytes => "deferring",
                    };
                    log::warn!(
                        "native viewer media budget {refusal}: {action} a {len} byte image (retained {:?} of {:?})",
                        budget.usage(),
                        budget.limit()
                    );
                }
                Err(match refusal {
                    Refusal::TooLarge => Staging::Dropped,
                    Refusal::Entries | Refusal::Bytes => Staging::Deferred,
                })
            },
        }
    }

    /// Stage a shared file, or return `None` when this viewer has no shared root or the
    /// write failed.
    fn stage_file(&mut self, rgba: &[u8]) -> Option<Staging> {
        let root = self.root.clone()?;
        let lease = match self.reserve(rgba.len()) {
            Ok(lease) => lease,
            Err(staging) => return Some(staging),
        };
        // A failed write is an environment fault, not overload.
        let file = root
            .write_image(rgba, lease)
            .context("stage shared native image")
            .to_log()
            .ok()?;
        self.overloaded = false;
        let path = file.path();
        self.media.push((self.seq, Retained::File(file)));
        Some(Staging::Shared(path))
    }

    /// Choose how this frame carries `rgba`, and keep a large image available until the
    /// recipient consumes it. A chunked image becomes a pending transfer for `image.key`.
    pub fn stage_image(&mut self, image: TransferImage, rgba: &[u8]) -> Staging {
        if !self.chunked() {
            if self.budget.is_none() || rgba.len() < SHARED_MEDIA_MIN_BYTES {
                return Staging::Inline;
            }
            // Without chunking, a failed write keeps the inline fallback.
            return self.stage_file(rgba).unwrap_or(Staging::Inline);
        }
        if rgba.len() < SHARED_MEDIA_MIN_BYTES && rgba.len() <= self.frame_media_left {
            self.frame_media_left -= rgba.len();
            return Staging::Inline;
        }
        if let Some(staging) = self.stage_file(rgba) {
            return staging;
        }
        let lease = match self.reserve(rgba.len()) {
            Ok(lease) => lease,
            Err(staging) => return staging,
        };
        self.overloaded = false;
        self.transfers.push(PendingTransfer {
            key: image.key,
            raster_revision: image.raster_revision,
            width: image.width,
            height: image.height,
            wire_id: None,
            rgba: rgba.to_vec(),
            sent: 0,
            lease,
            waiting: BTreeSet::new(),
        });
        Staging::Transfer
    }

    fn has_transfer(&self, key: &HostImageKey) -> bool {
        self.transfers.iter().any(|transfer| &transfer.key == key)
    }

    /// Record a placement that waits for the transfer of `key`.
    fn wait_for_transfer(&mut self, key: &HostImageKey, placement: (PaneId, u64)) {
        if let Some(transfer) = self.transfers.iter_mut().find(|t| &t.key == key) {
            transfer.waiting.insert(placement);
        }
    }

    /// Emit as much of the transfer for `key` as the chunk window and this frame's
    /// allowance permit. Returns the completed image when this frame carried its last byte.
    fn advance_transfer(
        &mut self,
        key: &HostImageKey,
        out: &mut Vec<GraphicsRecord>,
    ) -> Option<Completed> {
        let index = self.transfers.iter().position(|t| &t.key == key)?;
        let window_open = self
            .chunk_frame
            .is_none_or(|frame| frame <= self.acknowledged || frame == self.seq);
        // An unstarted transfer that fits what remains of this frame's allowance starts
        // and completes here without waiting for the window, while un-ACKed whole
        // transfers stay within one more allowance. A replacement then arrives in the
        // frame that deletes the image it replaces, instead of leaving the position blank
        // for a round trip. Past that allowance it waits for the window's ACK, so a viewer
        // that falls behind its tab's other recipients cannot fill its writer queue.
        let transfer = &self.transfers[index];
        let len = transfer.rgba.len();
        let whole = transfer.sent == 0
            && len <= self.frame_media_left
            && self.unacked_whole_bytes() + len <= self.frame_allowance();
        if !window_open && !whole {
            return None;
        }
        if self.frame_media_left == 0 {
            // The ACK of this frame retries the waiting placements.
            self.chunk_frame = Some(self.seq);
            return None;
        }
        let next_id = self.last_transfer_id + 1;
        let transfer = &mut self.transfers[index];
        let wire_id = *transfer.wire_id.get_or_insert(next_id);
        if wire_id == next_id {
            self.last_transfer_id = next_id;
        }
        let total = transfer.rgba.len();
        let len = (total - transfer.sent).min(self.frame_media_left);
        out.push(GraphicsRecord::TransferChunk {
            transfer_id: wire_id,
            total: total as u32,
            offset: transfer.sent as u32,
            data: transfer.rgba[transfer.sent..transfer.sent + len].to_vec(),
        });
        transfer.sent += len;
        self.frame_media_left -= len;
        match self.unacked_chunks.last_mut() {
            Some((seq, bytes)) if *seq == self.seq => *bytes += len,
            _ => self.unacked_chunks.push((self.seq, len)),
        }
        if window_open {
            // A whole transfer sent past a closed window leaves the window as it was.
            self.chunk_frame = Some(self.seq);
        }
        if transfer.sent < total {
            return None;
        }
        // The client now holds the whole image; only its lease remains until ACK.
        let transfer = self.transfers.remove(index);
        self.media.push((self.seq, Retained::Lease(transfer.lease)));
        Some(Completed {
            wire_id,
            width: transfer.width,
            height: transfer.height,
            total: total as u32,
        })
    }

    /// Abandon one transfer. An unstarted one leaves nothing on the wire and is dropped
    /// with its lease. A started one is cancelled unless this frame clears, which discards
    /// it at the client, and its lease waits for this frame's ACK.
    fn abandon(&mut self, transfer: PendingTransfer, cleared: bool, out: &mut Vec<GraphicsRecord>) {
        let Some(transfer_id) = transfer.wire_id else {
            // The lease drops here, so no ACK will report its release; the next one must
            // still retry images this lease deferred.
            self.released_unretained = true;
            return;
        };
        if !cleared {
            out.push(GraphicsRecord::TransferCancel { transfer_id });
        }
        self.media.push((self.seq, Retained::Lease(transfer.lease)));
    }

    fn abandon_where(
        &mut self,
        cleared: bool,
        out: &mut Vec<GraphicsRecord>,
        mut abandoned: impl FnMut(&PendingTransfer) -> bool,
    ) {
        let (gone, kept) = std::mem::take(&mut self.transfers)
            .into_iter()
            .partition::<Vec<_>, _>(|transfer| abandoned(transfer));
        self.transfers = kept;
        for transfer in gone {
            self.abandon(transfer, cleared, out);
        }
    }

    /// Abandon every pending transfer on a frame that clears the client's images, or
    /// when this viewer no longer shows Kitty graphics. Runs whether or not the frame
    /// flattens Kitty placements.
    pub(super) fn prune_frame_transfers(
        &mut self,
        cleared: bool,
        kitty: bool,
        out: &mut Vec<GraphicsRecord>,
    ) {
        if cleared || !kitty {
            self.abandon_where(cleared, out, |_| true);
        }
    }

    /// Route an accepted ACK to the owner's active or retired stream. Returns the panes to
    /// re-render when it released media that an active stream's deferred images need.
    pub fn acknowledge_recipient(
        active: &mut HashMap<ClientId, Self>,
        retired: &mut HashMap<ClientId, Self>,
        owner: ClientId,
        seq: u64,
    ) -> BTreeSet<PaneId> {
        if let Some(state) = active.get_mut(&owner) {
            let (released, opened) = state.acknowledge_frames(seq);
            let mut panes = BTreeSet::new();
            if released || opened {
                panes = std::mem::take(&mut state.deferred_panes);
            }
            if opened {
                panes.extend(
                    state
                        .transfers
                        .iter()
                        .flat_map(|transfer| transfer.waiting.iter().map(|(pane, _)| *pane)),
                );
            }
            return panes;
        } else if let Some(state) = retired.get_mut(&owner) {
            state.acknowledge(seq);
        }
        BTreeSet::new()
    }

    /// Release media consumed through `seq`. Duplicate, stale and future ACKs release
    /// nothing. Returns whether any media was released or the chunk window opened.
    pub fn acknowledge(&mut self, seq: u64) -> bool {
        let (released, opened) = self.acknowledge_frames(seq);
        released || opened
    }

    /// Returns whether media was released (by this ACK, or by an unstarted transfer since
    /// the last accepted one), and whether pending transfers may continue.
    fn acknowledge_frames(&mut self, seq: u64) -> (bool, bool) {
        if seq > self.seq {
            return (false, false);
        }
        let previous = self.acknowledged;
        self.acknowledged = self.acknowledged.max(seq);
        let opened = !self.transfers.is_empty()
            && self
                .chunk_frame
                .is_some_and(|frame| previous < frame && frame <= self.acknowledged);
        self.unacked_chunks
            .retain(|(frame, _)| *frame > self.acknowledged);
        let retained = self.media.len();
        self.media.retain(|(frame, _)| *frame > self.acknowledged);
        let released = self.media.len() < retained;
        (
            std::mem::take(&mut self.released_unretained) || released,
            opened,
        )
    }
}

impl Output {
    pub fn set_native_geometry(&mut self, client: ClientId, geometry: GeometryRecord) {
        self.native_geometry.insert(client, geometry);
    }

    /// Record a screen overlay (toast, menu, palette, modal) that paints `rect` for `client`.
    /// Call in paint order, from the layout that produced the overlay's chunks, in the same
    /// tab coordinates as the panes. Native clients that negotiated geometry receive these
    /// after the panes as `PANE_POPUP` rects whose insets are zero, so an overlay's border
    /// belongs to the overlay rather than to the pane beneath it. ANSI output ignores them.
    pub fn add_native_popup(&mut self, client: ClientId, rect: CellRect, focused: bool) {
        if rect.is_empty() {
            return;
        }
        let clamp = |value: usize| value.min(u16::MAX as usize) as u16;
        self.native_popups
            .entry(client)
            .or_default()
            .push(PaneRect {
                x: clamp(rect.x),
                y: clamp(rect.y),
                cols: clamp(rect.cols),
                rows: clamp(rect.rows),
                left: 0,
                top: 0,
                right: 0,
                bottom: 0,
                flags: PANE_POPUP | if focused { PANE_FOCUSED } else { 0 },
            });
    }

    /// Drain just native clients, leaving the ordinary ANSI clients for `serialize`.
    pub fn serialize_native(
        &mut self,
        clients: &mut HashMap<ClientId, NativeRenderState>,
        sizes: &HashMap<ClientId, Size>,
        presentations: &HashMap<ClientId, Presentation>,
    ) -> HashMap<ClientId, Vec<u8>> {
        let mut frames = HashMap::new();
        for (client, state) in clients {
            let Some(chunks) = self.client_character_chunks.remove(client) else {
                continue;
            };
            let size = sizes.get(client).copied().unwrap_or(Size {
                cols: 120,
                rows: 40,
            });
            let kitty = state.agreement.features & caps::KITTY != 0
                && self
                    .kitty_host_capabilities
                    .borrow()
                    .get(client)
                    .copied()
                    .unwrap_or(true);
            let presentation = presentations.get(client).copied();
            let (dx, dy) = presentation.map_or((0, 0), Presentation::offset);
            state.seq += 1;
            state.begin_frame();
            let mut builder = FrameBuilder::new(
                size.cols.min(u16::MAX as usize) as u16,
                size.rows.min(u16::MAX as usize) as u16,
                state.seq,
            );
            let mut cleared = self.clients_with_cleared_host_display.remove(client)
                || state.size != Some(size)
                || state.presentation != presentation;
            if cleared {
                // Full repaints reset the receiver's cells and hyperlink table together, so
                // every ID this frame references is fresh and the frame carries the complete
                // mapping.
                state.links.clear();
            }
            state.fresh_links.clear();
            builder.set_full_repaint(cleared);
            builder.set_clear(cleared);
            state.size = Some(size);
            state.presentation = presentation;
            for instructions in [
                self.pre_vte_instructions.remove(client),
                self.post_vte_instructions.get(client).cloned(),
            ] {
                for instruction in instructions.into_iter().flatten() {
                    let mut parser = vte::Parser::new();
                    let mut control = CursorControl {
                        cursor: &mut state.cursor,
                        clear: false,
                    };
                    parser.advance(&mut control, instruction.as_bytes());
                    if control.clear {
                        cleared = true;
                        builder.set_clear(true);
                    }
                    builder
                        .sideband_mut()
                        .extend_from_slice(instruction.as_bytes());
                }
            }
            if cleared {
                state.viewport_links = vec![LINK_NONE; size.cols * size.rows];
            }
            let layouts: Vec<Option<NativeLayout<'_>>> = chunks
                .iter()
                .map(|chunk| {
                    (chunk.x + dx < size.cols && chunk.y + dy < size.rows)
                        .then(|| native_layout(chunk, size.cols, dx))
                })
                .collect();
            // Release every overwritten cell before assigning IDs, including cells later
            // in this frame. The spans are exactly the cells emission writes below.
            for (chunk, layout) in chunks.iter().zip(&layouts) {
                let Some(layout) = layout else { continue };
                for (column, width) in layout.spans() {
                    let start = (chunk.y + dy) * size.cols + column + dx;
                    state.viewport_links[start..start + width].fill(LINK_NONE);
                }
            }
            let mut used_links = [false; MAX_LINK_ID as usize + 1];
            for id in &state.viewport_links {
                used_links[*id as usize] = true;
            }
            if cleared && let Some(presentation) = presentation {
                paint_native_margins(&mut builder, presentation);
            }
            let handler = self
                .client_link_handlers
                .get(client)
                .filter(|_| state.agreement.features & caps::LINKS != 0)
                .map(|handler| handler.borrow());
            let mut member_text = String::new();
            let mut unit_text = String::new();
            for (chunk, layout) in chunks.iter().zip(&layouts) {
                let Some(layout) = layout else { continue };
                let mut cells = Vec::new();
                for unit in &layout.units {
                    let members = &layout.cells[unit.start..unit.end];
                    let columns: usize = members.iter().map(|cell| cell.width()).sum();
                    let strip_vs16 = (members.len() > 1 || members[0].has_attached())
                        && grapheme_join::prepare_unit_into(members, columns, &mut unit_text).0;
                    let mut col = unit.column;
                    for (offset, character) in members.iter().enumerate() {
                        let width = character.width();
                        let selected = adjust_styles_for_possible_selection(
                            chunk.selection_and_colors(),
                            chunk.application_styles(*character.styles),
                            chunk.y,
                            col,
                        );
                        let default_background =
                            uses_default_background(chunk, character, &selected, col);
                        let styles = adjust_styles_for_custom_bg_fg(
                            selected,
                            chunk.pane_default_fg,
                            chunk.pane_default_bg,
                        );
                        let mut cell =
                            wire_cell(character, styles, chunk.changed_colors.as_ref(), true);
                        // A theme's default background arrives as a concrete color; mark it
                        // so a see-through native background still treats it as default.
                        if default_background && WireColor::unpack(cell.bg) != WireColor::Default {
                            cell.attrs |= ATTR_THEME_BACKGROUND;
                        }
                        if character.has_attached() {
                            member_text.clear();
                            character.push_text_to(&mut member_text);
                            // A narrow cell keeps its text presentation, as on ANSI hosts.
                            if strip_vs16 {
                                member_text.retain(|c| c != '\u{FE0F}');
                            }
                            let mut chars = member_text.chars();
                            cell.ch = match (chars.next(), chars.next()) {
                                (Some(only), None) => only as u32,
                                // The frame's cluster record is bounded; past its limits the
                                // cell falls back to its first code point.
                                (Some(first), Some(_)) => {
                                    builder.cluster(&member_text).unwrap_or(first as u32)
                                },
                                (None, _) => ' ' as u32,
                            };
                        }
                        if !kitty && character.character == PLACEHOLDER {
                            cell.ch = ' ' as u32;
                        }
                        if offset > 0 {
                            cell.attrs |= ATTR_GROUP_CONTINUE;
                        }
                        if let Some(identity) = handler
                            .as_ref()
                            .and_then(|handler| handler.identity(character.styles.link_anchor))
                            .filter(|identity| identity.uri.len() <= MAX_LINK_URI_LEN)
                        {
                            cell.link = state.link_id(&identity, &used_links);
                        }
                        // Pin IDs used by any emitted row until the next frame, even if a
                        // later overlapping chunk replaces that row's cells.
                        used_links[cell.link as usize] = true;
                        let start = (chunk.y + dy) * size.cols + col + dx;
                        state.viewport_links[start..start + width].fill(cell.link);
                        cells.push(cell);
                        // Wide continuation cells never continue the group themselves.
                        cells.extend((1..width).map(|_| WireCell {
                            ch: ' ' as u32,
                            width: 1,
                            attrs: cell.attrs & !ATTR_GROUP_CONTINUE,
                            ..cell
                        }));
                        col += width;
                    }
                }
                if let Some(fill) = &layout.fill {
                    // Blank the visible columns the unit that did not fit would have started
                    // in, with only its background: the receiver rewrites only the cells in
                    // row records, so these would otherwise keep an earlier frame's text,
                    // clusters and links. Their shadow cells were released above.
                    let character = &layout.cells[fill.clipped];
                    let selected = adjust_styles_for_possible_selection(
                        chunk.selection_and_colors(),
                        chunk.application_styles(*character.styles),
                        chunk.y,
                        fill.column,
                    );
                    let default_background =
                        uses_default_background(chunk, character, &selected, fill.column);
                    let styles = adjust_styles_for_custom_bg_fg(
                        selected,
                        chunk.pane_default_fg,
                        chunk.pane_default_bg,
                    );
                    let mut blank = WireCell {
                        bg: wire_color(substitute_changed_color(
                            styles.background,
                            chunk.changed_colors.as_ref(),
                        )),
                        ..WireCell::BLANK
                    };
                    if default_background && WireColor::unpack(blank.bg) != WireColor::Default {
                        blank.attrs |= ATTR_THEME_BACKGROUND;
                    }
                    cells.extend(std::iter::repeat_n(blank, fill.count));
                }
                builder.push_row((chunk.y + dy) as u16, (chunk.x + dx) as u16, &cells);
            }
            builder.push_links(&LinkRecord {
                entries: state
                    .fresh_links
                    .iter()
                    .map(|id| LinkEntry {
                        id: *id,
                        uri: state.links[*id as usize - 1].uri.clone(),
                    })
                    .collect(),
            });
            let popups = self.native_popups.remove(client).unwrap_or_default();
            // A client without tab geometry this frame keeps the absent-geometry meaning, so
            // popups alone never stand in for the scene.
            if let Some(mut geometry) = self
                .native_geometry
                .remove(client)
                .filter(|_| state.agreement.features & caps::GEOMETRY != 0)
            {
                geometry.panes.extend(popups);
                for pane in &mut geometry.panes {
                    pane.x = pane.x.saturating_add(dx.min(u16::MAX as usize) as u16);
                    pane.y = pane.y.saturating_add(dy.min(u16::MAX as usize) as u16);
                }
                builder.push_geometry(&geometry);
            }
            let mut cursor = state.cursor;
            cursor.x = cursor.x.saturating_add(dx.min(u16::MAX as usize) as u16);
            cursor.y = cursor.y.saturating_add(dy.min(u16::MAX as usize) as u16);
            // A recipient smaller than its tab cannot show a cursor past its grid, as with a
            // cropped watcher. Only the frame hides it: the state keeps the position and
            // shape, so the next frame inside the grid shows the cursor again.
            if usize::from(cursor.x) >= size.cols || usize::from(cursor.y) >= size.rows {
                cursor.visible = false;
            }
            builder.set_cursor(cursor);
            self.post_vte_instructions.remove(client);
            // Sixel already has cropped, occluded slices at this stage.
            if let Some(chunks) = self
                .sixel_chunks
                .remove(client)
                .filter(|_| state.agreement.features & caps::SIXEL != 0)
            {
                for chunk in chunks {
                    if let Some(payload) = self.sixel_image_store.borrow_mut().serialize_image(
                        chunk.sixel_image_id,
                        chunk.sixel_image_pixel_x,
                        chunk.sixel_image_pixel_y,
                        chunk.sixel_image_pixel_width,
                        chunk.sixel_image_pixel_height,
                    ) {
                        builder.extend_graphics(&[GraphicsRecord::SixelChunk {
                            cell_x: (chunk.cell_x + dx) as u32,
                            cell_y: (chunk.cell_y + dy) as u32,
                            pixel_x: 0,
                            pixel_y: 0,
                            pixel_width: chunk.sixel_image_pixel_width as u32,
                            pixel_height: chunk.sixel_image_pixel_height as u32,
                            payload: payload.into_bytes(),
                        }]);
                    }
                }
            }
            let chunks_by_pane = self.client_kitty_chunks.remove(client).unwrap_or_default();
            let rendered_panes = self
                .client_rendered_kitty_panes
                .remove(client)
                .unwrap_or_default();
            let updated_placements = self
                .client_updated_kitty_placements
                .remove(client)
                .unwrap_or_default();
            let visible = self.client_kitty_visible_panes.remove(client);
            let mut records = Vec::new();
            // The frame's clear is final here, and the client discards incomplete transfers
            // on exactly the frames whose header clears.
            state.prune_frame_transfers(cleared, kitty, &mut records);
            if kitty {
                flatten_kitty_frame(
                    KittyFrameInput {
                        chunks_by_pane,
                        rendered_panes,
                        updated_placements,
                        visible_panes: visible.as_ref(),
                        kitty_image_store: &mut self.kitty_image_store.borrow_mut(),
                        host_state: self
                            .kitty_host_state
                            .borrow_mut()
                            .entry(*client)
                            .or_default(),
                        host_display_cleared: cleared,
                    },
                    &mut records,
                    state,
                )
                .context("flatten native graphics")
                .non_fatal();
            }
            for record in &mut records {
                if let GraphicsRecord::Placement { cell_x, cell_y, .. } = record {
                    *cell_x = cell_x.saturating_add(dx as u32);
                    *cell_y = cell_y.saturating_add(dy as u32);
                }
            }
            builder.extend_graphics(&records);
            frames.insert(*client, builder.finish());
        }
        frames
    }
}

// Gutters use explicit cells because native clients do not interpret ANSI erases.
fn paint_native_margins(builder: &mut FrameBuilder, presentation: Presentation) {
    let (dx, dy) = presentation.offset();
    let bg = match presentation.background {
        PaletteColor::Rgb((r, g, b)) => WireColor::Rgb(r, g, b),
        PaletteColor::EightBit(n) => WireColor::Indexed(n),
    }
    .pack();
    let cell = WireCell {
        ch: ' ' as u32,
        bg,
        ..WireCell::default()
    };
    for y in 0..presentation.terminal.rows {
        let spans = if y < dy || y >= dy + presentation.content.rows {
            [(0, presentation.terminal.cols), (0, 0)]
        } else {
            [
                (0, dx),
                (
                    dx + presentation.content.cols,
                    presentation
                        .terminal
                        .cols
                        .saturating_sub(dx + presentation.content.cols),
                ),
            ]
        };
        for (x, width) in spans {
            if width > 0 {
                builder.push_row(y as u16, x as u16, &vec![cell; width]);
            }
        }
    }
}

struct CursorControl<'a> {
    cursor: &'a mut CursorState,
    clear: bool,
}
impl vte::Perform for CursorControl<'_> {
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let args: Vec<u16> = params.iter().map(|p| p[0]).collect();
        match (action, intermediates) {
            ('H' | 'f', []) => {
                self.cursor.y = args.first().copied().unwrap_or(1).max(1) - 1;
                self.cursor.x = args.get(1).copied().unwrap_or(1).max(1) - 1;
            },
            ('h', [b'?']) if args.contains(&25) => self.cursor.visible = true,
            ('l', [b'?']) if args.contains(&25) => self.cursor.visible = false,
            ('J', []) if args.first() == Some(&2) => self.clear = true,
            ('q', [b' ']) => {
                let style = args.first().copied().unwrap_or(0);
                self.cursor.shape = match style {
                    0 => CURSOR_SHAPE_DEFAULT,
                    3 | 4 => CURSOR_SHAPE_UNDERLINE,
                    5 | 6 => CURSOR_SHAPE_BEAM,
                    _ => CURSOR_SHAPE_BLOCK,
                } | if style % 2 == 1 { CURSOR_BLINKING } else { 0 };
            },
            _ => {},
        }
    }
}

fn wire_color(code: Option<AnsiCode>) -> u32 {
    match code {
        Some(AnsiCode::NamedColor(named)) => WireColor::Named(named_color_index(named)).pack(),
        Some(AnsiCode::ColorIndex(index)) => WireColor::Indexed(index).pack(),
        Some(AnsiCode::RgbCode((r, g, b))) => WireColor::Rgb(r, g, b).pack(),
        _ => WireColor::Default.pack(),
    }
}

fn named_color_index(named: NamedColor) -> u8 {
    match named {
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        NamedColor::BrightWhite => 15,
    }
}

fn substitute_changed_color(
    code: Option<AnsiCode>,
    changed_colors: Option<&[Option<AnsiCode>; 256]>,
) -> Option<AnsiCode> {
    match (code, changed_colors) {
        (Some(AnsiCode::ColorIndex(index)), Some(changed_colors)) => {
            changed_colors[index as usize].or(code)
        },
        (Some(AnsiCode::NamedColor(named)), Some(changed_colors)) => {
            changed_colors[named_color_index(named) as usize].or(code)
        },
        _ => code,
    }
}

fn wire_attrs(styles: &CharacterStyles, styled_underlines: bool) -> u16 {
    let mut attrs = 0u16;
    for (code, bit) in [
        (styles.bold, ATTR_BOLD),
        (styles.dim, ATTR_DIM),
        (styles.italic, ATTR_ITALIC),
        (styles.reverse, ATTR_REVERSE),
        (styles.hidden, ATTR_HIDDEN),
        (styles.strike, ATTR_STRIKE),
        (styles.slow_blink, ATTR_SLOW_BLINK),
        (styles.fast_blink, ATTR_FAST_BLINK),
    ] {
        if matches!(code, Some(AnsiCode::On)) {
            attrs |= bit;
        }
    }
    let underline = match styles.underline {
        Some(AnsiCode::Underline(None)) => UNDERLINE_STRAIGHT,
        Some(AnsiCode::Underline(Some(styled))) if styled_underlines => match styled {
            AnsiStyledUnderline::Double => UNDERLINE_DOUBLE,
            AnsiStyledUnderline::Undercurl => UNDERLINE_CURLY,
            AnsiStyledUnderline::Underdotted => UNDERLINE_DOTTED,
            AnsiStyledUnderline::Underdashed => UNDERLINE_DASHED,
        },
        Some(AnsiCode::Underline(Some(_))) => UNDERLINE_STRAIGHT,
        _ => UNDERLINE_NONE,
    };
    attrs |= underline << UNDERLINE_SHIFT;
    attrs
}

#[inline]
fn wire_cell(
    t_character: &TerminalCharacter,
    styles: CharacterStyles,
    chunk_changed_colors: Option<&[Option<AnsiCode>; 256]>,
    styled_underlines: bool,
) -> WireCell {
    WireCell {
        ch: t_character.character as u32,
        fg: wire_color(substitute_changed_color(
            styles.foreground,
            chunk_changed_colors,
        )),
        bg: wire_color(substitute_changed_color(
            styles.background,
            chunk_changed_colors,
        )),
        underline_color: if styled_underlines {
            wire_color(substitute_changed_color(
                styles.underline_color,
                chunk_changed_colors,
            ))
        } else {
            WireColor::Default.pack()
        },
        attrs: wire_attrs(&styles, styled_underlines),
        width: t_character.width().min(u8::MAX as usize) as u8,
        link: LINK_NONE,
    }
}

fn flatten_kitty_frame(
    kitty_input: KittyFrameInput,
    out: &mut Vec<GraphicsRecord>,
    state: &mut NativeRenderState,
) -> Result<()> {
    let KittyFrameInput {
        mut chunks_by_pane,
        rendered_panes,
        updated_placements,
        visible_panes,
        kitty_image_store,
        host_state,
        host_display_cleared,
    } = kitty_input;
    if host_display_cleared {
        host_state.live_placements.clear();
    }
    let mut freed: Vec<(HostImageKey, u32)> = host_state
        .transmitted
        .iter()
        // A full repaint may erase placements without freeing host image data. Delete our
        // old host IDs before forgetting them and transmitting replacement IDs.
        .filter(|(image_key, record)| {
            host_display_cleared
                || kitty_image_store.raster_revision(image_key.0) != Some(record.raster_revision)
                || image_key.1.is_some_and(|variant| {
                    kitty_image_store
                        .scaled_variant(image_key.0, variant)
                        .is_none()
                })
        })
        .map(|(image_key, record)| (*image_key, record.host_image_id))
        .collect();
    freed.sort_by_key(|(_, host_id)| *host_id);
    for (image_key, host_id) in freed {
        out.push(GraphicsRecord::DeleteImage { id: host_id as u64 });
        host_state.transmitted.remove(&image_key);
        host_state
            .live_placements
            .retain(|_, record| record.image_key != image_key);
    }
    // A transfer of a replaced raster or a vanished variant can never complete.
    state.abandon_where(false, out, |transfer| {
        kitty_image_store.raster_revision(transfer.key.0) != Some(transfer.raster_revision)
            || transfer.key.1.is_some_and(|variant| {
                kitty_image_store
                    .scaled_variant(transfer.key.0, variant)
                    .is_none()
            })
    });
    if let Some(visible_panes) = visible_panes {
        let mut to_delete: Vec<(KittyChunkKey, u32, u32)> = host_state
            .live_placements
            .iter()
            .filter(|(key, _)| !visible_panes.contains(&key.pane_id))
            .map(|(key, record)| (key.clone(), record.host_image_id, record.host_placement_id))
            .collect();
        to_delete.sort_by_key(|(key, _, _)| {
            (pane_sort_key(key.pane_id), key.placement_uid, key.sub_index)
        });
        for (key, host_image_id, host_placement_id) in to_delete {
            out.push(GraphicsRecord::DeletePlacement {
                image_id: host_image_id as u64,
                placement_id: host_placement_id as u64,
            });
            host_state.live_placements.remove(&key);
        }
    }
    // Overlay placements update independently of their owning pane, even when fully covered.
    let mut rendered: Vec<PaneId> = rendered_panes
        .iter()
        .copied()
        .chain(chunks_by_pane.keys().copied())
        .chain(updated_placements.iter().map(|(pane, _)| *pane))
        .collect();
    rendered.sort_unstable();
    rendered.dedup();
    rendered.sort_by_key(|pane_id| pane_sort_key(*pane_id));
    // Images this frame could not carry, and why, so their other placements skip a second
    // attempt but still retry with their pane.
    let mut withheld: HashMap<HostImageKey, Withheld> = HashMap::new();
    // Images each processed pane still shows, to tell a hidden waiting placement from one
    // shown under another placement id.
    let mut referenced: HashSet<(PaneId, HostImageKey)> = HashSet::new();
    for pane_id in rendered {
        let chunks = chunks_by_pane.remove(&pane_id).unwrap_or_default();
        let mut by_uid: BTreeMap<u64, Vec<KittyImageChunk>> = BTreeMap::new();
        for chunk in chunks {
            by_uid.entry(chunk.placement_uid).or_default().push(chunk);
        }
        let mut current: BTreeMap<KittyChunkKey, KittyImageChunk> = BTreeMap::new();
        for (placement_uid, mut group) in by_uid {
            group.sort_by_key(|chunk| (chunk.cell_y, chunk.cell_x));
            for (sub_index, chunk) in group.into_iter().enumerate() {
                current.insert(
                    KittyChunkKey {
                        pane_id,
                        placement_uid,
                        sub_index: sub_index as u32,
                    },
                    chunk,
                );
            }
        }
        let mut stale: Vec<(KittyChunkKey, u32, u32)> = host_state
            .live_placements
            .iter()
            .filter(|(key, _)| {
                key.pane_id == pane_id
                    && (rendered_panes.contains(&pane_id)
                        || updated_placements.contains(&(pane_id, key.placement_uid)))
                    && !current.contains_key(key)
            })
            .map(|(key, record)| (key.clone(), record.host_image_id, record.host_placement_id))
            .collect();
        stale.sort_by_key(|(key, _, _)| (key.placement_uid, key.sub_index));
        for (key, host_image_id, host_placement_id) in stale {
            out.push(GraphicsRecord::DeletePlacement {
                image_id: host_image_id as u64,
                placement_id: host_placement_id as u64,
            });
            host_state.live_placements.remove(&key);
        }
        for (key, chunk) in current {
            let variant = if chunk.scaled_px.is_some() {
                Some(chunk.variant_key)
            } else {
                None
            };
            let image_key = (chunk.internal_image_id, variant);
            referenced.insert((pane_id, image_key));
            let host_image_id = match host_state.transmitted.get(&image_key) {
                Some(record) => record.host_image_id,
                None => {
                    if let Some(&reason) = withheld.get(&image_key) {
                        match reason {
                            Withheld::Deferred => {
                                state.deferred_panes.insert(pane_id);
                            },
                            Withheld::Waiting => {
                                state.wait_for_transfer(&image_key, (pane_id, key.placement_uid))
                            },
                            Withheld::Dropped => {},
                        }
                        withhold_placement(host_state, &key, out);
                        continue;
                    }
                    let mut carried = None;
                    if !state.has_transfer(&image_key) {
                        if let Some(variant) = variant {
                            kitty_image_store
                                .ensure_scaled_variant(chunk.internal_image_id, variant);
                        }
                        let Some(image) = kitty_image_store.get(chunk.internal_image_id) else {
                            continue;
                        };
                        let (width, height, rgba) = if let Some(variant) = variant {
                            let Some(rgba) =
                                kitty_image_store.scaled_variant(chunk.internal_image_id, variant)
                            else {
                                continue;
                            };
                            (variant.size.0 as u32, variant.size.1 as u32, rgba)
                        } else {
                            (image.width, image.height, image.displayed_rgba())
                        };
                        let transfer = TransferImage {
                            key: image_key,
                            raster_revision: kitty_image_store
                                .raster_revision(chunk.internal_image_id)
                                .context("staged kitty image is missing")?,
                            width,
                            height,
                        };
                        let (medium, data) = match state.stage_image(transfer, rgba) {
                            Staging::Shared(path) => (
                                GraphicsMedium::SharedFile,
                                path.to_string_lossy().into_owned().into_bytes(),
                            ),
                            Staging::Inline => (GraphicsMedium::Inline, rgba.to_vec()),
                            Staging::Transfer => (GraphicsMedium::Transfer, Vec::new()),
                            staging @ (Staging::Deferred | Staging::Dropped) => {
                                // Everything else in the frame proceeds in order; only this
                                // image waits. Nothing is recorded as transmitted, so the
                                // next render of the pane tries again.
                                let reason = if staging == Staging::Deferred {
                                    state.deferred_panes.insert(pane_id);
                                    Withheld::Deferred
                                } else {
                                    Withheld::Dropped
                                };
                                withheld.insert(image_key, reason);
                                withhold_placement(host_state, &key, out);
                                continue;
                            },
                        };
                        if medium != GraphicsMedium::Transfer {
                            carried = Some((medium, data, width, height, rgba.len() as u32));
                        }
                    }
                    let (medium, data, width, height, byte_len) = match carried {
                        Some(carried) => carried,
                        None => match state.advance_transfer(&image_key, out) {
                            // The residency names the transfer right after its last chunk.
                            Some(done) => (
                                GraphicsMedium::Transfer,
                                done.wire_id.to_le_bytes().to_vec(),
                                done.width,
                                done.height,
                                done.total,
                            ),
                            None => {
                                // Like a deferral, without a warning: the ACK that opens the
                                // chunk window renders this pane again.
                                state.wait_for_transfer(&image_key, (pane_id, key.placement_uid));
                                withheld.insert(image_key, Withheld::Waiting);
                                withhold_placement(host_state, &key, out);
                                continue;
                            },
                        },
                    };
                    let host_image_id = host_state.next_host_image_id;
                    host_state.next_host_image_id += 1;
                    out.push(GraphicsRecord::Residency {
                        id: host_image_id as u64,
                        width,
                        height,
                        format: GRAPHICS_FORMAT_RGBA8,
                        medium,
                        byte_len,
                        data,
                    });
                    host_state.transmitted.insert(
                        image_key,
                        HostImageRecord {
                            host_image_id,
                            raster_revision: kitty_image_store
                                .raster_revision(chunk.internal_image_id)
                                .context("transmitted kitty image is missing")?,
                        },
                    );
                    host_image_id
                },
            };
            let existing = host_state.live_placements.get(&key).map(|record| {
                (
                    record.host_placement_id,
                    record.geometry,
                    record.host_image_id,
                )
            });
            let host_placement_id = match existing {
                Some((_, geometry, existing_host_image_id))
                    if geometry == chunk && existing_host_image_id == host_image_id =>
                {
                    continue;
                },
                Some((host_placement_id, _, existing_host_image_id)) => {
                    if existing_host_image_id != host_image_id {
                        out.push(GraphicsRecord::DeletePlacement {
                            image_id: existing_host_image_id as u64,
                            placement_id: host_placement_id as u64,
                        });
                    }
                    host_placement_id
                },
                None => {
                    let host_placement_id = host_state.next_host_placement_id;
                    host_state.next_host_placement_id += 1;
                    host_placement_id
                },
            };
            out.push(GraphicsRecord::Placement {
                image_id: host_image_id as u64,
                placement_id: host_placement_id as u64,
                cell_x: chunk.cell_x as u32,
                cell_y: chunk.cell_y as u32,
                offset_x: chunk.cell_offset_x,
                offset_y: chunk.cell_offset_y,
                source_x: chunk.source_px_x as u32,
                source_y: chunk.source_px_y as u32,
                source_width: chunk.source_px_width as u32,
                source_height: chunk.source_px_height as u32,
                z: chunk.z_index,
            });
            host_state.live_placements.insert(
                key,
                HostPlacementRecord {
                    host_image_id,
                    host_placement_id,
                    image_key,
                    geometry: chunk,
                },
            );
        }
    }
    // A waiting placement is gone when its pane is hidden, or when the pane or placement
    // re-rendered (the stale-placement rule above) without showing the image. A transfer
    // that no placement waits for any longer is abandoned. An overlay update of another
    // placement leaves a pane's waiting placements alone.
    for transfer in &mut state.transfers {
        let image_key = transfer.key;
        transfer.waiting.retain(|&(pane, uid)| {
            let shown = visible_panes.is_none_or(|visible| visible.contains(&pane));
            let rerendered =
                rendered_panes.contains(&pane) || updated_placements.contains(&(pane, uid));
            shown && (!rerendered || referenced.contains(&(pane, image_key)))
        });
    }
    state.abandon_where(false, out, |transfer| transfer.waiting.is_empty());
    Ok(())
}

/// Take down any placement shown at `key` whose image this frame withholds, so a stale
/// image is not left at the new geometry.
fn withhold_placement(
    host_state: &mut HostKittyState,
    key: &KittyChunkKey,
    out: &mut Vec<GraphicsRecord>,
) {
    if let Some(record) = host_state.live_placements.remove(key) {
        out.push(GraphicsRecord::DeletePlacement {
            image_id: record.host_image_id as u64,
            placement_id: record.host_placement_id as u64,
        });
    }
}

#[cfg(test)]
#[path = "unit/native_media_tests.rs"]
mod tests;
