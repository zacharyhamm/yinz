use super::*;

#[test]
fn native_disabled_styled_underlines_keep_default_color() {
    let mut palette = [None; 256];
    palette[1] = Some(AnsiCode::RgbCode((10, 20, 30)));
    let styles = CharacterStyles {
        underline_color: Some(AnsiCode::NamedColor(NamedColor::Red)),
        ..DEFAULT_STYLES
    };
    let cell = wire_cell(&TerminalCharacter::new('x'), styles, Some(&palette), false);
    assert_eq!(WireColor::unpack(cell.underline_color), WireColor::Default);
}

#[test]
fn shared_uploads_survive_until_the_frame_is_acknowledged() {
    let directory = tempfile::tempdir().unwrap();
    let root = MediaRoot::open(directory.path()).unwrap();
    let budget = MediaBudget::viewer();
    let first = root
        .write_image(&[1], budget.try_reserve(1).unwrap())
        .unwrap();
    let second = root
        .write_image(&[2], budget.try_reserve(1).unwrap())
        .unwrap();
    let first_path = first.path();
    let second_path = second.path();
    let mut state = NativeRenderState {
        seq: 2,
        media: vec![(1, Retained::File(first)), (2, Retained::File(second))],
        ..Default::default()
    };
    assert!(state.acknowledge(1));
    assert!(!first_path.exists());
    assert!(second_path.exists());
    assert!(!state.acknowledge(3));
    assert!(second_path.exists());
    assert!(state.acknowledge(2));
    assert!(!second_path.exists());
    assert_eq!(budget.usage(), MediaUsage::default());
}

mod budget {
    use super::*;
    use crate::panes::kitty_graphics::{
        parser::{DecodedImage, KittyFormat},
        store::{InternalImageId, ScaledImageKey},
    };
    use yinz_types::{
        internal_ids::TerminalId,
        render_negotiation as caps,
        structured_render::{decode, GeometryRecord, GraphicsMedium, GraphicsRecord, PaneRect},
    };

    const IMAGE: usize = 256 * 256 * 4;

    /// An image shown at a placement uid, with its square side in pixels.
    type Shown = (InternalImageId, u64, u32);

    fn pane(id: u32) -> PaneId {
        PaneId::Terminal(TerminalId::new(id))
    }

    fn shared(root: &MediaRoot, budget: MediaBudget) -> NativeRenderState {
        NativeRenderState::negotiated(
            caps::Agreement {
                media: caps::INLINE | caps::SHARED,
                ..Default::default()
            },
            Some(root.clone()),
            Some(budget),
        )
    }

    fn image(side: u32, shade: u8) -> DecodedImage {
        DecodedImage {
            bytes: vec![shade; (side * side * 4) as usize],
            width: side,
            height: side,
            format: KittyFormat::Rgba32,
        }
    }

    fn chunk(internal_image_id: InternalImageId, placement_uid: u64, side: u32) -> KittyImageChunk {
        KittyImageChunk {
            cell_x: placement_uid as usize * 4,
            cell_y: 1,
            internal_image_id,
            source_px_x: 0,
            source_px_y: 0,
            source_px_width: side as usize,
            source_px_height: side as usize,
            cell_offset_x: 0,
            cell_offset_y: 0,
            z_index: 0,
            dest_cells: (3, 2),
            variant_key: ScaledImageKey {
                source: crate::panes::sixel::PixelRect::new(0, 0, side as usize, side as usize),
                size: (side as usize, side as usize),
            },
            scaled_px: None,
            placement_uid,
        }
    }

    struct Scene {
        output: Output,
        states: HashMap<ClientId, NativeRenderState>,
        _dir: tempfile::TempDir,
        root: MediaRoot,
        /// What each viewer's client has seen of its transfers, checked on every frame.
        wire: HashMap<ClientId, Wire>,
    }

    #[derive(Debug, Default)]
    struct Frame {
        shared: Vec<std::path::PathBuf>,
        inline: usize,
        placements: usize,
        cells: bool,
        seq: u64,
        cleared: bool,
        /// (transfer id, offset, length) of every chunk.
        chunks: Vec<(u64, u32, usize)>,
        cancels: Vec<u64>,
        /// Wire ids named by transfer residencies.
        transfers: Vec<u64>,
        /// Inline residency and chunk bytes, the part bounded by the frame allowance.
        media: usize,
        /// Concatenated chunk data per transfer id.
        data: HashMap<u64, Vec<u8>>,
    }

    impl Frame {
        fn first_chunks(&self) -> Vec<u64> {
            self.chunks
                .iter()
                .filter(|(_, offset, _)| *offset == 0)
                .map(|(id, _, _)| *id)
                .collect()
        }
    }

    /// The wire rules a client enforces: first chunks carry strictly increasing ids,
    /// continuations and cancels name a started incomplete transfer, offsets follow
    /// each other, and a transfer residency names a transfer its frame completed.
    #[derive(Default)]
    struct Wire {
        highest: u64,
        incomplete: HashMap<u64, (u32, u32)>,
    }

    impl Wire {
        fn check(&mut self, cleared: bool, records: &[GraphicsRecord]) {
            if cleared {
                self.incomplete.clear();
            }
            let mut completed = HashSet::new();
            for record in records {
                match record {
                    GraphicsRecord::TransferChunk {
                        transfer_id,
                        total,
                        offset,
                        data,
                    } => {
                        assert!(!data.is_empty(), "empty chunk of {transfer_id}");
                        let received = match self.incomplete.get(transfer_id) {
                            Some(&(declared, received)) => {
                                assert_eq!(declared, *total);
                                assert_eq!(
                                    received, *offset,
                                    "chunk of {transfer_id} out of order"
                                );
                                received
                            },
                            None => {
                                assert!(
                                    *transfer_id > self.highest,
                                    "first chunk of {transfer_id} after {}",
                                    self.highest
                                );
                                assert_eq!(*offset, 0);
                                self.highest = *transfer_id;
                                0
                            },
                        };
                        let received = received + data.len() as u32;
                        assert!(received <= *total);
                        if received == *total {
                            self.incomplete.remove(transfer_id);
                            completed.insert(*transfer_id);
                        } else {
                            self.incomplete.insert(*transfer_id, (*total, received));
                        }
                    },
                    GraphicsRecord::TransferCancel { transfer_id } => {
                        assert!(
                            self.incomplete.remove(transfer_id).is_some(),
                            "cancel of unstarted or complete transfer {transfer_id}"
                        );
                    },
                    GraphicsRecord::Residency {
                        medium: GraphicsMedium::Transfer,
                        data,
                        ..
                    } => {
                        let id = u64::from_le_bytes(data.as_slice().try_into().unwrap());
                        assert!(completed.remove(&id), "residency of incomplete {id}");
                    },
                    GraphicsRecord::Residency {
                        medium: GraphicsMedium::SharedFile,
                        ..
                    } => {},
                    _ => {},
                }
            }
            assert!(completed.is_empty(), "unconsumed transfers {completed:?}");
        }
    }

    impl Scene {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = MediaRoot::open(dir.path()).unwrap();
            Self {
                output: Output::default(),
                states: HashMap::new(),
                _dir: dir,
                root,
                wire: HashMap::new(),
            }
        }

        fn viewer(&mut self, id: ClientId, budget: MediaBudget) {
            let state = shared(&self.root, budget);
            self.add(id, state);
        }

        /// A forwarded viewer: chunked media, no shared root, `allowance` bytes per frame.
        fn portable(&mut self, id: ClientId, budget: MediaBudget, allowance: usize) {
            let mut state = NativeRenderState::negotiated(
                caps::Agreement {
                    media: caps::INLINE | caps::CHUNKED,
                    ..Default::default()
                },
                None,
                Some(budget),
            );
            state.frame_media = Some(allowance);
            self.add(id, state);
        }

        fn add(&mut self, id: ClientId, state: NativeRenderState) {
            self.states.insert(id, state);
            self.output.add_clients(
                &HashSet::from([id]),
                Rc::new(RefCell::new(LinkHandler::new())),
                None,
            );
            self.output
                .kitty_host_capabilities
                .borrow_mut()
                .insert(id, true);
        }

        fn store(&self, side: u32, shade: u8) -> InternalImageId {
            self.output
                .kitty_image_store
                .borrow_mut()
                .store_image(image(side, shade))
                .unwrap()
        }

        fn replace(&self, old: InternalImageId, side: u32, shade: u8) -> InternalImageId {
            self.output
                .kitty_image_store
                .borrow_mut()
                .replace_image(old, image(side, shade))
                .unwrap()
        }

        /// One render of `pane` showing `placements` to every viewer.
        fn render(&mut self, pane_id: PaneId, placements: &[Shown]) -> HashMap<ClientId, Frame> {
            self.render_panes(&[(pane_id, placements)])
        }

        /// One frame in which each listed pane renders its placements.
        fn render_panes(&mut self, panes: &[(PaneId, &[Shown])]) -> HashMap<ClientId, Frame> {
            self.render_with(panes, |_, _| {})
        }

        /// Like `render_panes`, with `extra` adding more to each viewer's frame.
        fn render_with(
            &mut self,
            panes: &[(PaneId, &[Shown])],
            mut extra: impl FnMut(&mut Output, ClientId),
        ) -> HashMap<ClientId, Frame> {
            let ids: Vec<_> = self.states.keys().copied().collect();
            for id in ids {
                extra(&mut self.output, id);
                self.output.client_character_chunks.insert(
                    id,
                    vec![CharacterChunk::new(vec![TerminalCharacter::new('Z')], 0, 0)],
                );
                self.output.set_native_geometry(
                    id,
                    GeometryRecord {
                        panes: vec![PaneRect {
                            cols: 80,
                            rows: 24,
                            ..Default::default()
                        }],
                    },
                );
                for &(pane_id, placements) in panes {
                    self.output.add_kitty_image_chunks_to_client(
                        id,
                        pane_id,
                        placements
                            .iter()
                            .map(|&(image, uid, side)| chunk(image, uid, side))
                            .collect(),
                        None,
                    );
                }
            }
            self.output
                .serialize_native(&mut self.states, &HashMap::new(), &HashMap::new())
                .into_iter()
                .map(|(id, bytes)| {
                    assert!(bytes.len() <= yinz_ipc_frame_limit());
                    let view = decode(&bytes).unwrap();
                    let header = view.header();
                    let mut frame = Frame {
                        cells: view.rows().any(|(_, cells)| cells[0].character() == 'Z'),
                        seq: header.seq,
                        cleared: header.full_repaint() || header.clear(),
                        ..Default::default()
                    };
                    let records = view.graphics();
                    self.wire
                        .entry(id)
                        .or_default()
                        .check(frame.cleared, &records);
                    for record in records {
                        match record {
                            GraphicsRecord::Residency {
                                medium: GraphicsMedium::SharedFile,
                                data,
                                ..
                            } => frame.shared.push(String::from_utf8(data).unwrap().into()),
                            GraphicsRecord::Residency {
                                medium: GraphicsMedium::Transfer,
                                data,
                                ..
                            } => frame
                                .transfers
                                .push(u64::from_le_bytes(data.as_slice().try_into().unwrap())),
                            GraphicsRecord::Residency { data, .. } => {
                                frame.inline += 1;
                                frame.media += data.len();
                            },
                            GraphicsRecord::Placement { .. } => frame.placements += 1,
                            GraphicsRecord::TransferChunk {
                                transfer_id,
                                offset,
                                data,
                                ..
                            } => {
                                frame.chunks.push((transfer_id, offset, data.len()));
                                frame.media += data.len();
                                frame.data.entry(transfer_id).or_default().extend(data);
                            },
                            GraphicsRecord::TransferCancel { transfer_id } => {
                                frame.cancels.push(transfer_id)
                            },
                            _ => {},
                        }
                    }
                    (id, frame)
                })
                .collect()
        }

        fn render_one(&mut self, viewer: ClientId, pane_id: PaneId, placements: &[Shown]) -> Frame {
            self.render(pane_id, placements).remove(&viewer).unwrap()
        }

        fn ack_last(&mut self, id: ClientId) -> BTreeSet<PaneId> {
            let seq = self.states[&id].seq;
            self.ack(id, seq)
        }

        fn ack(&mut self, id: ClientId, seq: u64) -> BTreeSet<PaneId> {
            NativeRenderState::acknowledge_recipient(&mut self.states, &mut HashMap::new(), id, seq)
        }
    }

    // Mirrors yinz_ipc::MAX_IPC_FRAME_LEN, which this crate does not depend on.
    fn yinz_ipc_frame_limit() -> usize {
        64 * 1024 * 1024
    }

    #[test]
    fn render_negotiation_withheld_acks_defer_repeated_replacement_within_budget() {
        let mut scene = Scene::new();
        let viewer = ClientId::new(1);
        let budget = MediaBudget::new(32, 3 * IMAGE);
        scene.viewer(viewer, budget.clone());
        let mut image = scene.store(256, 1);
        let mut staged = Vec::new();
        for shade in 2..=6u8 {
            let frame = scene
                .render(pane(1), &[(image, 1, 256)])
                .remove(&viewer)
                .unwrap();
            assert!(frame.cells && frame.inline == 0, "{frame:?}");
            let usage = budget.usage();
            assert!(usage.entries <= 3 && usage.bytes <= 3 * IMAGE, "{usage:?}");
            if staged.len() < 3 {
                assert_eq!((frame.shared.len(), frame.placements), (1, 1), "{frame:?}");
                staged.extend(frame.shared);
            } else {
                // Deferred: the cells still arrive, the image waits for an ACK.
                assert_eq!((frame.shared.len(), frame.placements), (0, 0), "{frame:?}");
            }
            image = scene.replace(image, 256, shade);
        }
        assert!(staged.iter().all(|path| path.is_file()));
        assert_eq!(
            budget.usage(),
            MediaUsage {
                entries: 3,
                bytes: 3 * IMAGE
            }
        );
        let state = &scene.states[&viewer];
        assert_eq!(state.deferred_panes, BTreeSet::from([pane(1)]));
        let seq = state.seq;
        assert_eq!(scene.ack(viewer, seq), BTreeSet::from([pane(1)]));
        assert!(staged.iter().all(|path| !path.exists()));
        assert_eq!(budget.usage(), MediaUsage::default());
        // Re-rendering the returned pane now carries the latest image as a shared file.
        let frame = scene
            .render(pane(1), &[(image, 1, 256)])
            .remove(&viewer)
            .unwrap();
        assert_eq!(
            (frame.shared.len(), frame.placements, frame.inline),
            (1, 1, 0)
        );
        assert!(scene.states[&viewer].deferred_panes.is_empty());
    }

    #[test]
    fn render_negotiation_large_replacement_within_one_round_trip_is_deferred_not_inline() {
        // PR1 scenario 1: replaced twice before the first ACK arrives.
        let mut scene = Scene::new();
        let viewer = ClientId::new(1);
        let budget = MediaBudget::new(32, 3 * 1024 * 1024 / 2);
        scene.viewer(viewer, budget.clone());
        let first = scene.store(512, 1);
        let frame = scene
            .render(pane(1), &[(first, 1, 512)])
            .remove(&viewer)
            .unwrap();
        assert_eq!(frame.shared.len(), 1);
        let second = scene.replace(first, 512, 2);
        let frame = scene
            .render(pane(1), &[(second, 1, 512)])
            .remove(&viewer)
            .unwrap();
        assert_eq!((frame.shared.len(), frame.inline), (0, 0), "{frame:?}");
        let third = scene.replace(second, 512, 3);
        let frame = scene
            .render(pane(1), &[(third, 1, 512)])
            .remove(&viewer)
            .unwrap();
        assert_eq!((frame.shared.len(), frame.inline), (0, 0), "{frame:?}");
        assert_eq!(budget.usage().entries, 1);
        let seq = scene.states[&viewer].seq;
        assert_eq!(scene.ack(viewer, seq), BTreeSet::from([pane(1)]));
        let frame = scene
            .render(pane(1), &[(third, 1, 512)])
            .remove(&viewer)
            .unwrap();
        assert_eq!(
            (frame.shared.len(), frame.inline, frame.placements),
            (1, 0, 1)
        );
    }

    #[test]
    fn render_negotiation_viewers_stage_one_image_from_their_own_budgets() {
        // PR1 scenarios 2 and 3: each viewer pays for its own copy, and one stalled
        // viewer at its limit cannot change another viewer's staging.
        let mut scene = Scene::new();
        let viewers = [1, 2, 3].map(ClientId::new);
        let budgets = viewers.map(|_| MediaBudget::new(32, 2 * IMAGE));
        for (viewer, budget) in viewers.iter().zip(&budgets) {
            scene.viewer(*viewer, budget.clone());
        }
        let first = scene.store(256, 1);
        let frames = scene.render(pane(1), &[(first, 1, 256)]);
        let mut paths = HashSet::new();
        for viewer in viewers {
            assert_eq!(frames[&viewer].shared.len(), 1);
            paths.insert(frames[&viewer].shared[0].clone());
        }
        assert_eq!(paths.len(), 3);
        let mut image = first;
        // Viewer 1 never ACKs; viewers 2 and 3 ACK every frame.
        for shade in 2..=5u8 {
            image = scene.replace(image, 256, shade);
            let frames = scene.render(pane(1), &[(image, 1, 256)]);
            assert_eq!(frames[&viewers[0]].shared.len(), usize::from(shade == 2));
            for viewer in &viewers[1..] {
                assert_eq!(frames[viewer].shared.len(), 1, "{shade}");
                let seq = scene.states[viewer].seq;
                scene.ack(*viewer, seq);
                assert_eq!(
                    budgets[viewer.get() as usize - 1].usage(),
                    MediaUsage::default()
                );
            }
            assert_eq!(budgets[0].usage().bytes, 2 * IMAGE);
        }
    }

    #[test]
    fn render_negotiation_ack_classification_releases_only_consumed_media() {
        let mut scene = Scene::new();
        let viewer = ClientId::new(1);
        let budget = MediaBudget::new(1, 4 * IMAGE);
        scene.viewer(viewer, budget.clone());
        let first = scene.store(256, 1);
        let second = scene.store(256, 2);
        let frame = scene
            .render(pane(1), &[(first, 1, 256)])
            .remove(&viewer)
            .unwrap();
        let path = frame.shared[0].clone();
        let staged_at = scene.states[&viewer].seq;
        let frame = scene
            .render(pane(2), &[(second, 2, 256)])
            .remove(&viewer)
            .unwrap();
        assert!(frame.shared.is_empty());
        let high = scene.states[&viewer].seq;
        // Future, stale and unknown ACKs release nothing and return no panes.
        for (owner, seq) in [
            (viewer, high + 1),
            (viewer, staged_at - 1),
            (ClientId::new(9), high),
        ] {
            assert!(scene.ack(owner, seq).is_empty());
            assert!(path.is_file());
        }
        assert_eq!(scene.ack(viewer, staged_at), BTreeSet::from([pane(2)]));
        assert!(!path.exists());
        // A duplicate releases nothing more.
        assert!(scene.ack(viewer, staged_at).is_empty());
        assert!(scene.ack(viewer, high).is_empty());
        assert_eq!(budget.usage(), MediaUsage::default());
    }

    #[test]
    fn render_negotiation_image_above_the_shared_limit_is_dropped_not_inline() {
        let mut scene = Scene::new();
        let viewer = ClientId::new(1);
        let budget = MediaBudget::new(4, 2 * IMAGE);
        scene.viewer(viewer, budget.clone());
        let large = scene.store(512, 1);
        let frame = scene
            .render(pane(1), &[(large, 1, 512)])
            .remove(&viewer)
            .unwrap();
        assert_eq!(
            (frame.shared.len(), frame.inline, frame.placements),
            (0, 0, 0)
        );
        assert!(frame.cells);
        assert!(scene.states[&viewer].deferred_panes.is_empty());
        assert_eq!(budget.usage(), MediaUsage::default());
        // A portable viewer from before chunking keeps inline delivery, unbudgeted.
        let mut portable = NativeRenderState::negotiated(
            Default::default(),
            None,
            Some(MediaBudget::new(4, 2 * IMAGE)),
        );
        assert_eq!(
            portable.stage_image(transfer_image(1), &vec![0; 4 * IMAGE]),
            Staging::Inline
        );
        assert_eq!(portable.media_usage(), MediaUsage::default());
        // A chunked viewer sends what its budget can hold as a transfer and drops the rest.
        let mut chunked = NativeRenderState::negotiated(
            caps::Agreement {
                media: caps::INLINE | caps::CHUNKED,
                ..Default::default()
            },
            None,
            Some(MediaBudget::new(4, 2 * IMAGE)),
        );
        assert_eq!(
            chunked.stage_image(transfer_image(1), &vec![0; 4 * IMAGE]),
            Staging::Dropped
        );
        assert_eq!(
            chunked.stage_image(transfer_image(2), &vec![0; 2 * IMAGE]),
            Staging::Transfer
        );
        assert_eq!(
            chunked.media_usage(),
            MediaUsage {
                entries: 1,
                bytes: 2 * IMAGE
            }
        );
    }

    fn transfer_image(id: InternalImageId) -> TransferImage {
        TransferImage {
            key: (id, None),
            raster_revision: 0,
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn render_negotiation_deferral_drops_a_stale_placement_at_the_key() {
        let mut scene = Scene::new();
        let viewer = ClientId::new(1);
        scene.viewer(viewer, MediaBudget::new(1, 4 * IMAGE));
        let first = scene.store(256, 1);
        let second = scene.store(256, 2);
        scene.render(pane(1), &[(first, 1, 256)]);
        // The same placement now shows a different image that the budget defers.
        scene.render(pane(1), &[(second, 1, 256)]);
        let state = &scene.states[&viewer];
        assert_eq!(state.deferred_panes, BTreeSet::from([pane(1)]));
        assert!(scene.output.kitty_host_state.borrow()[&viewer]
            .live_placements
            .is_empty());
        // The same deferred image in a second pane of one frame retries with that pane too.
        scene.render_panes(&[
            (pane(1), &[(second, 1, 256)]),
            (pane(2), &[(second, 2, 256)]),
        ]);
        assert_eq!(
            scene.states[&viewer].deferred_panes,
            BTreeSet::from([pane(1), pane(2)])
        );
    }

    #[test]
    fn render_negotiation_seal_and_removal_interleavings_release_exactly_once() {
        let mut scene = Scene::new();
        let viewer = ClientId::new(1);
        let budget = MediaBudget::new(32, 4 * IMAGE);
        scene.viewer(viewer, budget.clone());
        let images: Vec<_> = (1..=4u8).map(|shade| scene.store(256, shade)).collect();
        let mut paths = Vec::new();
        for (uid, image) in images.iter().enumerate() {
            let frame = scene
                .render(pane(1), &[(*image, uid as u64, 256)])
                .remove(&viewer)
                .unwrap();
            paths.push((scene.states[&viewer].seq, frame.shared[0].clone()));
        }
        // The fifth image defers, then a seal trims the prefix to the second frame.
        let fifth = scene.store(256, 5);
        scene.render(pane(2), &[(fifth, 9, 256)]);
        let high = paths[1].0;
        let mut prefix = scene.states.remove(&viewer).unwrap();
        prefix.retain_prefix(high);
        assert!(prefix.deferred_panes.is_empty());
        assert!(paths[..2].iter().all(|(_, path)| path.is_file()));
        assert!(paths[2..].iter().all(|(_, path)| !path.exists()));
        let mut retired = HashMap::from([(viewer, prefix)]);
        let mut active = HashMap::new();
        // Above the sealed high mark: nothing. Inside it: exactly the covered file.
        for (seq, released) in [(high + 1, 0), (paths[0].0, 1), (paths[0].0, 1), (high, 2)] {
            assert!(NativeRenderState::acknowledge_recipient(
                &mut active,
                &mut retired,
                viewer,
                seq
            )
            .is_empty());
            assert_eq!(
                paths[..2].iter().filter(|(_, path)| !path.exists()).count(),
                released
            );
        }
        assert_eq!(budget.usage(), MediaUsage::default());
        // Removing a viewer with deferred images returns its whole budget.
        let other = ClientId::new(2);
        let other_budget = MediaBudget::new(1, 4 * IMAGE);
        scene.viewer(other, other_budget.clone());
        scene.render(pane(1), &[(images[0], 1, 256), (images[1], 2, 256)]);
        assert_eq!(other_budget.usage().entries, 1);
        assert!(!scene.states[&other].deferred_panes.is_empty());
        retired.remove(&viewer);
        scene.states.remove(&other);
        assert_eq!(other_budget.usage(), MediaUsage::default());
        // An ACK for a removed client is a no-op.
        assert!(scene.ack(other, 1).is_empty());
    }

    // Chunked portable delivery: transfers paced by ACKs under each viewer's budget.

    /// A 256 KiB image needs three frames at this allowance.
    const ALLOWANCE: usize = 100_000;

    fn portable_scene(budget: MediaBudget, allowance: usize) -> (Scene, ClientId) {
        let mut scene = Scene::new();
        let viewer = ClientId::new(1);
        scene.portable(viewer, budget, allowance);
        (scene, viewer)
    }

    fn assert_portable(frame: &Frame, allowance: usize) {
        assert!(frame.shared.is_empty(), "portable frames carry no paths");
        assert!(frame.media <= allowance, "{} > {allowance}", frame.media);
    }

    #[test]
    fn render_negotiation_portable_large_image_splits_into_acked_frames() {
        let budget = MediaBudget::viewer();
        let (mut scene, viewer) = portable_scene(budget.clone(), ALLOWANCE);
        let image = scene.store(256, 7);
        let mut assembled: Vec<u8> = Vec::new();
        let mut frames = Vec::new();
        loop {
            let frame = scene.render_one(viewer, pane(1), &[(image, 1, 256)]);
            assert_portable(&frame, ALLOWANCE);
            assert_eq!(frame.inline, 0);
            assert_eq!(frame.chunks.len(), 1, "{frame:?}");
            assembled.extend(frame.data.values().flatten());
            if let [id] = frame.transfers.as_slice() {
                assert_eq!(frame.placements, 1);
                assert_eq!(frame.chunks[0].0, *id);
                frames.push(frame);
                break;
            }
            assert_eq!(frame.placements, 0);
            frames.push(frame);
            // Without an ACK, a re-render carries no further chunk.
            let stalled = scene.render_one(viewer, pane(1), &[(image, 1, 256)]);
            assert!(stalled.chunks.is_empty() && stalled.transfers.is_empty());
            assert_eq!(scene.ack_last(viewer), BTreeSet::from([pane(1)]));
        }
        assert_eq!(frames.len(), IMAGE.div_ceil(ALLOWANCE));
        assert_eq!(assembled, vec![7; IMAGE]);
        // The completing frame's ACK returns the lease.
        assert_eq!(budget.usage().entries, 1);
        assert!(scene.ack_last(viewer).is_empty());
        assert_eq!(budget.usage(), MediaUsage::default());
    }

    #[test]
    fn render_negotiation_portable_100_mib_image_stays_within_the_frame_limit() {
        use yinz_runtime::native_media::{MAX_SHARED_IMAGE_BYTES, PORTABLE_FRAME_MEDIA_BYTES};
        let budget = MediaBudget::viewer();
        let (mut scene, viewer) = portable_scene(budget.clone(), PORTABLE_FRAME_MEDIA_BYTES);
        scene.states.get_mut(&viewer).unwrap().frame_media = None;
        let image = scene.store(5120, 3);
        let mut received = 0;
        let mut frames = 0;
        loop {
            let frame = scene.render_one(viewer, pane(1), &[(image, 1, 5120)]);
            assert_portable(&frame, PORTABLE_FRAME_MEDIA_BYTES);
            frames += 1;
            received += frame.media;
            assert_eq!(
                budget.usage(),
                MediaUsage {
                    entries: 1,
                    bytes: MAX_SHARED_IMAGE_BYTES
                }
            );
            if !frame.transfers.is_empty() {
                assert_eq!(frame.placements, 1);
                break;
            }
            scene.ack_last(viewer);
        }
        assert_eq!(received, MAX_SHARED_IMAGE_BYTES);
        assert_eq!(frames, 13);
        scene.ack_last(viewer);
        assert_eq!(budget.usage(), MediaUsage::default());
    }

    #[test]
    fn render_negotiation_portable_threshold_images_are_never_inline() {
        let budget = MediaBudget::viewer();
        let (mut scene, viewer) = portable_scene(budget.clone(), 3 * 64 * 64 * 4);
        let large = scene.store(256, 1);
        let frame = scene.render_one(viewer, pane(1), &[(large, 1, 256)]);
        assert_eq!((frame.inline, frame.chunks.len()), (0, 1));
        scene.ack_last(viewer);
        // Small images ride inline while they fit the frame's allowance; the rest is chunked.
        let small: Vec<_> = (0..4u8).map(|shade| scene.store(64, 10 + shade)).collect();
        let shown: Vec<Shown> = small
            .iter()
            .enumerate()
            .map(|(uid, image)| (*image, 10 + uid as u64, 64))
            .collect();
        let frame = scene.render_one(viewer, pane(2), &shown);
        assert_eq!(frame.inline, 3, "{frame:?}");
        assert_eq!(frame.chunks.len(), 0, "the allowance is spent: {frame:?}");
        assert_eq!(frame.placements, 3);
        assert_eq!(frame.media, 3 * 64 * 64 * 4);
        // The fourth waits as a transfer for the ACK of the frame that spent the allowance.
        assert_eq!(scene.ack_last(viewer), BTreeSet::from([pane(1), pane(2)]));
        let frame = scene.render_one(viewer, pane(2), &shown);
        assert_eq!(
            (frame.inline, frame.chunks.len(), frame.transfers.len()),
            (0, 1, 1)
        );
    }

    #[test]
    fn render_negotiation_portable_non_acking_reader_gets_one_chunk_frame() {
        let budget = MediaBudget::viewer();
        let (mut scene, viewer) = portable_scene(budget.clone(), ALLOWANCE);
        let image = scene.store(256, 1);
        let other = scene.store(256, 2);
        let mut chunk_frames = 0;
        for _ in 0..6 {
            let frames =
                scene.render_panes(&[(pane(1), &[(image, 1, 256)]), (pane(2), &[(other, 2, 256)])]);
            let frame = &frames[&viewer];
            assert!(frame.cells);
            chunk_frames += usize::from(!frame.chunks.is_empty());
        }
        assert_eq!(chunk_frames, 1);
        let usage = budget.usage();
        assert_eq!(usage.entries, 2, "both transfers hold leases while waiting");
        // A stale, duplicate or future ACK opens nothing.
        let seq = scene.states[&viewer].seq;
        assert!(scene.ack(viewer, seq + 1).is_empty());
        assert!(scene.ack(viewer, 0).is_empty());
    }

    #[test]
    fn render_negotiation_portable_withheld_acks_defer_replacement_within_budget() {
        let budget = MediaBudget::new(32, IMAGE);
        let (mut scene, viewer) = portable_scene(budget.clone(), PORTABLE_FRAME_MEDIA_BYTES_TEST);
        let mut image = scene.store(256, 1);
        let frame = scene.render_one(viewer, pane(1), &[(image, 1, 256)]);
        assert_eq!((frame.transfers.len(), frame.placements), (1, 1));
        for shade in 2..=5u8 {
            image = scene.replace(image, 256, shade);
            let frame = scene.render_one(viewer, pane(1), &[(image, 1, 256)]);
            assert!(frame.cells);
            assert_eq!(
                (
                    frame.inline,
                    frame.chunks.len(),
                    frame.transfers.len(),
                    frame.placements
                ),
                (0, 0, 0, 0),
                "deferred, never inline: {frame:?}"
            );
            let usage = budget.usage();
            assert!(usage.entries <= 1 && usage.bytes <= IMAGE, "{usage:?}");
        }
        assert_eq!(
            scene.states[&viewer].deferred_panes,
            BTreeSet::from([pane(1)])
        );
        // The completing frame's ACK releases its lease and retries the deferred pane.
        let completed = scene.states[&viewer].seq - 4;
        assert_eq!(scene.ack(viewer, completed), BTreeSet::from([pane(1)]));
        assert_eq!(budget.usage(), MediaUsage::default());
        let frame = scene.render_one(viewer, pane(1), &[(image, 1, 256)]);
        assert_eq!((frame.transfers.len(), frame.placements), (1, 1));
        assert_eq!(frame.data.values().next().unwrap()[0], 5);
    }

    /// Large enough for a 256 KiB image in one frame.
    const PORTABLE_FRAME_MEDIA_BYTES_TEST: usize = 2 * IMAGE;

    #[test]
    fn render_negotiation_portable_viewers_keep_separate_budgets() {
        let mut scene = Scene::new();
        let viewers = [ClientId::new(1), ClientId::new(2)];
        let budgets = viewers.map(|_| MediaBudget::new(32, IMAGE));
        for (viewer, budget) in viewers.iter().zip(&budgets) {
            scene.portable(*viewer, budget.clone(), PORTABLE_FRAME_MEDIA_BYTES_TEST);
        }
        let mut image = scene.store(256, 1);
        for shade in 2..=4u8 {
            let frames = scene.render(pane(1), &[(image, 1, 256)]);
            // Viewer 2 ACKs every frame; viewer 1 stalls after its first.
            assert_eq!(frames[&viewers[1]].transfers.len(), 1, "{shade}");
            assert_eq!(frames[&viewers[0]].transfers.len(), usize::from(shade == 2));
            scene.ack_last(viewers[1]);
            assert_eq!(budgets[1].usage(), MediaUsage::default());
            assert_eq!(budgets[0].usage().bytes, IMAGE);
            image = scene.replace(image, 256, shade);
        }
    }

    #[test]
    fn render_negotiation_portable_replacement_mid_transfer_cancels_and_releases_on_ack() {
        let budget = MediaBudget::viewer();
        let (mut scene, viewer) = portable_scene(budget.clone(), ALLOWANCE);
        let first = scene.store(256, 1);
        let frame = scene.render_one(viewer, pane(1), &[(first, 1, 256)]);
        let [started] = frame.first_chunks()[..] else {
            panic!("expected one started transfer: {frame:?}")
        };
        scene.ack_last(viewer);
        let second = scene.replace(first, 256, 2);
        let frame = scene.render_one(viewer, pane(1), &[(second, 1, 256)]);
        assert_eq!(frame.cancels, [started]);
        let [replacement] = frame.first_chunks()[..] else {
            panic!("expected the replacement to start: {frame:?}")
        };
        assert!(replacement > started);
        // The cancelled lease waits for this frame's ACK; the replacement keeps its own.
        assert_eq!(budget.usage().entries, 2);
        scene.ack_last(viewer);
        assert_eq!(budget.usage().entries, 1);
    }

    #[test]
    fn render_negotiation_portable_cleared_frame_abandons_without_cancel() {
        let budget = MediaBudget::viewer();
        let (mut scene, viewer) = portable_scene(budget.clone(), ALLOWANCE);
        let image = scene.store(256, 1);
        let frame = scene.render_one(viewer, pane(1), &[(image, 1, 256)]);
        assert_eq!(frame.first_chunks().len(), 1);
        // A waiting, unstarted second transfer.
        let other = scene.store(256, 2);
        scene.render_one(viewer, pane(2), &[(other, 2, 256)]);
        assert_eq!(budget.usage().entries, 2);
        scene.states.get_mut(&viewer).unwrap().force_repaint();
        let frame = scene.render_panes(&[]).remove(&viewer).unwrap();
        assert!(frame.cleared);
        assert!(
            frame.cancels.is_empty() && frame.chunks.is_empty(),
            "{frame:?}"
        );
        // The unstarted lease is gone; the started one waits for the clearing frame's ACK.
        assert_eq!(budget.usage().entries, 1);
        scene.ack_last(viewer);
        assert_eq!(budget.usage(), MediaUsage::default());
        assert!(scene.states[&viewer].transfers.is_empty());
    }

    #[test]
    fn render_negotiation_portable_seal_and_removal_release_leases_once() {
        let budget = MediaBudget::viewer();
        let (mut scene, viewer) = portable_scene(budget.clone(), ALLOWANCE);
        let started = scene.store(256, 1);
        scene.render_one(viewer, pane(1), &[(started, 1, 256)]);
        let high = scene.states[&viewer].seq;
        let unstarted = scene.store(256, 2);
        scene.render_one(viewer, pane(2), &[(unstarted, 2, 256)]);
        assert_eq!(budget.usage().entries, 2);
        let mut prefix = scene.states.remove(&viewer).unwrap();
        prefix.retain_prefix(high);
        assert!(prefix.transfers.is_empty());
        assert_eq!(
            budget.usage().entries,
            1,
            "the unstarted lease is released at once"
        );
        let mut retired = HashMap::from([(viewer, prefix)]);
        let mut active = HashMap::new();
        for (seq, entries) in [(high + 1, 1), (high - 1, 1), (high, 0), (high, 0)] {
            assert!(NativeRenderState::acknowledge_recipient(
                &mut active,
                &mut retired,
                viewer,
                seq
            )
            .is_empty());
            assert_eq!(budget.usage().entries, entries, "ACK {seq}");
        }
        // Removing a viewer mid-transfer returns its whole budget.
        let other = ClientId::new(2);
        let other_budget = MediaBudget::viewer();
        scene.portable(other, other_budget.clone(), ALLOWANCE);
        scene.render_one(other, pane(1), &[(started, 1, 256)]);
        scene.render_one(other, pane(2), &[(unstarted, 2, 256)]);
        assert_eq!(other_budget.usage().entries, 2);
        scene.states.remove(&other);
        assert_eq!(other_budget.usage(), MediaUsage::default());
    }

    #[test]
    fn render_negotiation_portable_waiting_transfers_start_in_increasing_id_order() {
        // Trigger A: a later-sorted pane stages first while the window is closed.
        let budget = MediaBudget::viewer();
        let (mut scene, viewer) = portable_scene(budget.clone(), 3 * IMAGE);
        let opener = scene.store(256, 1);
        let frame = scene.render_one(viewer, pane(3), &[(opener, 3, 256)]);
        assert_eq!(frame.transfers.len(), 1);
        let late = scene.store(256, 2);
        let early = scene.store(256, 3);
        assert!(scene
            .render_one(viewer, pane(2), &[(late, 2, 256)])
            .chunks
            .is_empty());
        assert!(scene
            .render_one(viewer, pane(1), &[(early, 1, 256)])
            .chunks
            .is_empty());
        let retried = scene.ack(viewer, frame.seq);
        assert_eq!(retried, BTreeSet::from([pane(1), pane(2)]));
        let frame = scene
            .render_panes(&[(pane(2), &[(late, 2, 256)]), (pane(1), &[(early, 1, 256)])])
            .remove(&viewer)
            .unwrap();
        // Pane 1 sorts first, so its transfer takes the lower id; the wire checker in
        // `render_with` asserts the order across every frame.
        let started = frame.first_chunks();
        assert_eq!(started.len(), 2, "{frame:?}");
        assert!(started[0] < started[1]);
        assert_eq!(frame.data[&started[0]][0], 3);
        assert_eq!(frame.transfers, started);
    }

    #[test]
    fn render_negotiation_portable_unstarted_replacement_cancels_silently() {
        // Trigger B: a transfer that never reached the wire is replaced.
        let budget = MediaBudget::viewer();
        let (mut scene, viewer) = portable_scene(budget.clone(), 2 * IMAGE);
        let opener = scene.store(256, 1);
        let opened = scene.render_one(viewer, pane(2), &[(opener, 2, 256)]);
        let [opener_id] = opened.transfers[..] else {
            panic!("expected the opener to complete")
        };
        let waiting = scene.store(256, 2);
        scene.render_one(viewer, pane(1), &[(waiting, 1, 256)]);
        assert_eq!(budget.usage().entries, 2);
        let replacement = scene.replace(waiting, 256, 3);
        let frame = scene.render_one(viewer, pane(1), &[(replacement, 1, 256)]);
        assert!(
            frame.cancels.is_empty() && frame.chunks.is_empty(),
            "{frame:?}"
        );
        // The unstarted lease went at once; the replacement holds its own.
        assert_eq!(budget.usage().entries, 2);
        assert_eq!(scene.ack(viewer, opened.seq), BTreeSet::from([pane(1)]));
        assert_eq!(budget.usage().entries, 1);
        let frame = scene.render_one(viewer, pane(1), &[(replacement, 1, 256)]);
        assert_eq!(frame.first_chunks(), [opener_id + 1]);
        assert_eq!(frame.data[&(opener_id + 1)][0], 3);
    }

    #[test]
    fn render_negotiation_portable_kitty_disable_cancels_pending_transfers() {
        for cleared in [false, true] {
            let budget = MediaBudget::viewer();
            let (mut scene, viewer) = portable_scene(budget.clone(), ALLOWANCE);
            let started = scene.store(256, 1);
            let frame = scene.render_one(viewer, pane(1), &[(started, 1, 256)]);
            let [started_id] = frame.first_chunks()[..] else {
                panic!("expected a started transfer")
            };
            let unstarted = scene.store(256, 2);
            scene.render_one(viewer, pane(2), &[(unstarted, 2, 256)]);
            assert_eq!(budget.usage().entries, 2);
            scene
                .output
                .kitty_host_capabilities
                .borrow_mut()
                .insert(viewer, false);
            if cleared {
                scene.states.get_mut(&viewer).unwrap().force_repaint();
            }
            let frame = scene.render_one(viewer, pane(1), &[(started, 1, 256)]);
            assert_eq!(frame.cleared, cleared);
            let expected: &[u64] = if cleared { &[] } else { &[started_id] };
            assert_eq!(frame.cancels, expected, "cleared={cleared}");
            assert!(frame.chunks.is_empty() && frame.placements == 0);
            assert_eq!(budget.usage().entries, 1, "cleared={cleared}");
            scene.ack_last(viewer);
            assert_eq!(budget.usage(), MediaUsage::default(), "cleared={cleared}");
        }
    }

    #[test]
    fn render_negotiation_portable_overlay_update_keeps_pending_transfer() {
        let budget = MediaBudget::viewer();
        let (mut scene, viewer) = portable_scene(budget.clone(), ALLOWANCE);
        let image = scene.store(256, 1);
        let badge = scene.store(8, 9);
        let frame = scene.render_one(viewer, pane(1), &[(image, 1, 256)]);
        let [id] = frame.first_chunks()[..] else {
            panic!("expected a started transfer")
        };
        // An overlay placement of the same pane updates without the pane re-rendering.
        let cell = SizeInPixels {
            width: 8,
            height: 16,
        };
        let frame = scene
            .render_with(&[], |output, client| {
                output.add_overlay_kitty_image(client, pane(1), chunk(badge, 7, 8), cell)
            })
            .remove(&viewer)
            .unwrap();
        assert!(frame.cancels.is_empty(), "{frame:?}");
        assert_eq!(frame.inline, 1);
        assert_eq!(budget.usage().entries, 1);
        // The transfer continues and completes after the ACKs.
        loop {
            let retried = scene.ack_last(viewer);
            assert!(retried.contains(&pane(1)), "{retried:?}");
            let frame = scene.render_one(viewer, pane(1), &[(image, 1, 256)]);
            assert!(frame.cancels.is_empty());
            assert!(frame.chunks.iter().all(|(chunk, _, _)| *chunk == id));
            if frame.transfers == [id] {
                break;
            }
        }
        // A real re-render without the image abandons a pending transfer.
        let next = scene.store(256, 2);
        scene.ack_last(viewer);
        let frame = scene.render_one(viewer, pane(1), &[(next, 1, 256)]);
        let [pending] = frame.first_chunks()[..] else {
            panic!("expected the next transfer to start")
        };
        let frame = scene.render_one(viewer, pane(1), &[]);
        assert_eq!(frame.cancels, [pending]);
    }
}
