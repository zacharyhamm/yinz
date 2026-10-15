# Performance candidates and Kitty graphics bugs

This records a September 2026 survey of hot paths for explicit SIMD through the
[`fearless_simd`](https://crates.io/crates/fearless_simd) crate (1.0.0), and what else the survey turned up.
It was taken at `main` commit `e6e3f18bf` on an AMD Threadripper PRO 5975WX (AVX2).
Timings come from `cargo x bench` (`yinz-server/benches/multi_pane_render.rs`), perf_event IP-sampling and gdb
stack-sampling profiles of that bench (samply could not run at `perf_event_paranoid=2`), and throwaway
microbenchmarks that copied the relevant loops. Most runs were short Criterion runs on a machine shared with other
sessions (load averages of about 9–46), so absolute times vary up to about 2x between runs; relative results were
consistent. Since then, candidate 1 (without the inlining), candidate 2 (as one parser batch per Kitty APC
rather than option A), the Kitty `capture_run` change, the translucency runs, `base64-simd` for Kitty payloads,
and shrinking rows that enter the scrollback have landed; see [Implemented](#implemented).

## Verdict on SIMD

No candidate justifies adding `fearless_simd`. Each loop that SIMD could speed up is too small a share of runtime,
gets most of its gain from a scalar restructure, or is already matched by a crate in `Cargo.lock` (`memchr`,
`base64-simd`) or by `simdutf8` (in the local cargo registry, not yet in `Cargo.lock`).

Text output dominates runtime, and its cost is per character rather than per byte scan:

- With 16 panes, ingest (vte parse and grid update) is 82–94% of a frame; grid update alone is 55–86%.
- Grid update works on the 16-byte `TerminalCharacter`, whose styles are `Rc`s. The cost is `Rc` clones,
  `HyperlinkTracker`, `VecDeque` pushes, and malloc/free. There is no primitive slice for vector code to work on.
- Ingest costs about 28 ns per byte. A byte scan in front of the parser costs about 0.25 ns per byte, under 2% of a frame.

Revisit if cells move to a struct-of-arrays layout or CJK throughput becomes a goal.

| Path | Gain only `fearless_simd` would add |
| --- | --- |
| Text ingest and render (all five bench workloads, 1 and 16 panes) | About 0% |
| Kitty direct-transmission capture | None over `memchr3` + `memchr`; about 0.4 ns/B over a scalar bulk loop |
| vte UTF-8 validation | Same as `simdutf8`; either way vte must be patched |
| `scale_rgba` | 1.4–5 ms per scaled placement, over a scalar rewrite that is already about 4x faster |
| `blit_rgba` | About 1.5–2 ms per 1-megapixel animation frame over a scalar per-row-clip rewrite (2.1–2.6 ms to 0.63 ms; about 3.5–4.2 ms over today's code); no in-tree client sends `a=f` or `a=c` |

### Rejected SIMD candidates

| Candidate | Location | Why rejected |
| --- | --- | --- |
| Ground-state ESC scan | `yinz-panes/src/panes/terminal_pane.rs:257-269`, `plugin_pane.rs:294-305` | `position(\|b\| *b == 0x1b)` is scalar (about 1 byte per cycle) but only 0.4–1.2% of a frame. A `memchr` A/B test was noise, ±1–3% with the sign flipping between runs. |
| Kitty APC capture | `yinz-terminal-graphics/src/kitty_graphics/interceptor.rs:74-168` | Real cost during `t=d` uploads, but about 85% of the fix is scalar and `memchr` matches a SIMD kernel. See [Kitty direct-transmission capture](#kitty-direct-transmission-capture). |
| vte ground-state UTF-8 validation | vte 0.15 `src/lib.rs:609` (external) | Ingest −9 to −13% on `unicode_wide/16` and −6 to −8% on pure CJK, but 0% or slightly slower on colored output (`git log`, `ls`, `cargo tree`), whose runs average 5–28 bytes. Needs `[patch.crates-io]`, and `simdutf8::compat::from_utf8` is a drop-in that returns the same `Utf8Error`. A hand-written validator would feed `from_utf8_unchecked`, so it is soundness-critical. |
| Bilinear scaling | `yinz-terminal-graphics/src/kitty_graphics/grid_state.rs:1992` (`scale_rgba`) | Runs per placement with `c`/`r` or `U=1`, not per frame. A bit-exact scalar rewrite gets about 4x; `f64x4` adds 1.55–1.6x on top. `U=1` clients tend to request identity or near-identity sizes; exact identity wants a memcpy shortcut, which is 50x faster than SIMD. |
| Alpha blending | `grid_state.rs:1920` (`blit_rgba`) | Runs once per `a=f`/`a=c` frame. There is no playback clock, and no in-tree client sends `a=f` or `a=c` (its only callers are `transmit_frame` and `compose_frames`). Overwrite mode gets 3.7x from a per-row copy without SIMD. |
| Base64 decode | `yinz-terminal-graphics/src/kitty_graphics/parser.rs:749-761` (`run_payload_pipeline`) | Once per transmission, about 5% of image ingest. `base64-simd` is already locked and 3.5x faster. A `pshufb`-style `fearless_simd` kernel runs 5x slower than today's code on the SSE2 level (the compile-time x86_64 baseline, which `Level::new()` selects only on CPUs without SSE4.2), so it must be gated to SSE4.2, AVX2, or NEON. |
| Base64 encode | `yinz-terminal-graphics/src/kitty_graphics/store.rs:213-238` (`base64_for`) | Once per image or scaled variant, then cached in the store (later clients pay a `String` clone). The encode takes about 3.3–3.6 ms per 1080p image, and `base64-simd`, a byte-identical drop-in, would save about 2.7 ms of it. |
| RGB to RGBA expansion | `yinz-terminal-graphics/src/kitty_graphics/store.rs:542-554` (`normalize_to_rgba`), `parser.rs:868-891` (`decode_png`) | Once per transmission. The per-pixel `extend_from_slice` + `push` is the cost; a scalar `u32` rewrite gets 90–97% of the gain. SIMD's margin is 0.06–0.3 ms per 1080p image. |
| `Grid::print`, `Row::width`, `CharacterStyles::eq`, `serialize_chunks`, `pane_contents` | `yinz-terminal/` | Arrays of structs holding `Rc`s and padding, with no safe primitive-slice view, or per-character callbacks from vte. |
| Sixel decode and encode | `yinz-terminal-graphics/src/sixel.rs` | Decode (`SixelGrid::handle_byte`) receives one byte at a time from vte's DCS `put()` and feeds the external `sixel-tokenizer` 0.1.0 and `sixel-image` 0.2.1 per-byte state machines, which store pixels as `Vec<Vec<Pixel>>`. Encode (`SixelImageStore::serialize_image` → `SixelSerializer::serialize_range`) allocates a `HashMap` per 6-pixel column and a `BTreeMap` of per-color strings, and its output is cached per rect. Both are algorithmic costs inside third-party crates; SIMD would mean vendoring and rewriting them. |

If `fearless_simd` is adopted later, note the following, checked against the 1.0.0 source:

- `Level::new()` reads a cached `LazyLock<Level>`, so there's no need to cache it again.
- `u8x32::from_slice` and `store_slice` panic unless the slice length equals the lane count.
- Kernels using `swizzle_dyn` fall back to scalar on the SSE2 level. Gate them to SSE4.2, AVX2, or NEON.
- `[profile.dev]` builds workspace crates at opt-level 0, where a generic kernel measured only 1.4x faster than
  equally unoptimized scalar code (0.11 GB/s, against 26 GB/s in an opt-level-3 crate) and can be slower than the
  opt-level-3 dependency code it would replace. Put kernels in a small native-only crate with
  `[profile.dev.package.<crate>] opt-level = 3`.

## Implemented

Measured in October 2026 with `cargo bench -p yinz-server --bench multi_pane_render` against the
previous commit (the last three rows against the branch's `head` baseline, on a quiet machine), on the same shared machine (re-running outliers; single runs swung up to ±40%).

| Change | Where | Measured |
| --- | --- | --- |
| Candidate 1, partly: skip the style diff for a cell whose styles `Rc` is the previous cell's, when the chunk has no selection; and return early from `update_and_return_diff` for an unchanged style even with `changed_colors` (an OSC 4 palette re-sent a reset on every reset cell) | `serialize_chunks`, `RcCharacterStyles::ptr_eq`, `update_and_return_diff` | Landed with the commit before the baseline, so not in these numbers |
| Candidate 2: in ground state, copy everything up to the next `ESC _` into one batch (keeping a trailing ESC back for the next read), and hand the batch to vte only when a Kitty APC starts or the read ends. This replaced option A, which passed whole only a read with no `ESC _` and no trailing ESC | `KittyApcInterceptor::split`, driving `TerminalPane::handle_pty_bytes` and `PluginPane::handle_plugin_bytes` | A/B in one tree against one vte call per escape, on a quiet machine: `ingest/sgr_churn` and `ingest/mixed_kitty_sgr` −12 to −16%, `short_lines` −5 to −7%, `cursor_moves` −3 to −9%, `alt_screen_redraw` −2 to −7%, `plain_text` −1 to −5%; `blank_lines` and `unicode_wide` within noise. Option A on its own measured `ingest/sgr_churn` −9 to −14% |
| Kitty `capture_run` with `memchr3` + `memchr`, matching `push_capture_byte`'s overflow | `interceptor.rs`, called from `TerminalPane::handle_pty_bytes` | `kitty_graphics/ingest` −70%, `kitty_graphics/frame` −64% |
| Resolve and blend translucent cells once per run of equal styles, instead of allocating a styles `Rc` per covered cell twice | `Composition::add` and `finish` in `yinz-terminal/src/output/translucency.rs` | `panel/opacity_75/1` −40%, `panel/opacity_75/16` −13%, `panel_text` −45 to −55% |
| `base64-simd` for Kitty payloads both ways (decode checked against the `base64` crate by property test); share the cached encoding as `Rc<String>`; write transmissions straight into the frame with their size reserved; fill RGB→RGBA from an opaque buffer | `store.rs`, `parser.rs`, `emit_kitty_transmit` and `serialize_kitty_frame` | `kitty_graphics/ingest` −66%, `frame` −68%, `render_only` −73% |
| Shrink short lines on every path into the scrollback, so no row keeps spare capacity there | `bounded_push` in `grid.rs` | Memory, not speed. Pre-sizing new rows to the pane width did not pay: for a scrolling newline (`add_canonical_line`) it made `ingest/short_lines` 5–11% slower, and for a wrap (`line_wrap`) it gained nothing and cost `sgr_churn` and `unicode_wide` about 1% (interleaved A/B, three rounds). Both were removed |

Both ingest changes have property tests comparing them with the byte-at-a-time path:
`bulk_and_intercepted_ingest_leave_identical_panes` in `yinz-panes` (against `TerminalPane::ingest_per_byte`, as well
as across random read splits) and `bulk_capture_matches_per_byte_capture` in `yinz-terminal-graphics`. Both pane
kinds ingest through the same driver, `KittyApcInterceptor::split`.

## Non-SIMD performance candidates

Ranked by measured share of the text path. Shares are from the bench profile, which has no LTO; release fat LTO may shrink them.

| Rank | Candidate | Location | Measured share | Idea |
| --- | --- | --- | --- | --- |
| 1 | Style fast path when serializing | `yinz-terminal/src/output/mod.rs:202-275` (`serialize_chunks`; `write_changed_styles` at `:111`) and `yinz-terminal/src/panes/terminal_character.rs:322` (`update_and_return_diff`) | `CharacterStyles::eq` (`terminal_character.rs:232`) is 24–26% of a 1-pane frame | Skip `adjust_styles_*` and the style diff when the cell's style `Rc` is pointer-equal to the previous cell's and there are no selections, `changed_colors`, or pane defaults. Inline the call chain and drop the per-cell `with_context`. |
| 2 | One vte call per escape sequence | `yinz-panes/src/panes/terminal_pane.rs:240-330` (`handle_pty_bytes`) | 7–16% of SGR-heavy ingest; 3–6% for colored and TUI output | See [below](#one-vte-call-per-escape-sequence). |
| 3 | Per-character overhead in `Grid::print` | `yinz-terminal/src/panes/grid.rs` | 10–26% self time | Two `Rc` clones per character (`set_preceding_character` and the cell), plus a store-forwarding stall on the 16-byte cell. |
| 4 | `HyperlinkTracker::update` | `yinz-terminal/src/panes/hyperlink_tracker.rs` | 7–8% | Called per character. |
| 5 | Scrollback eviction | `bounded_push`, `Row::width` in `yinz-terminal/src/panes/grid.rs` | 4–11% | Reuse cached row widths. Partly bound by cache misses, since dropping the row touches the same lines. |
| 6 | Zero-width characters | `diacritic_value` (`yinz-types/src/kitty_placeholders.rs:78`), called from `Grid::record_placeholder_diacritic` (`yinz-terminal/src/panes/grid.rs:2703`) | About 5% of 16-pane `unicode_wide` frames; 11% of 1-pane `unicode_wide` ingest | Binary search on every zero-width character. Check first whether the previous cell is a placeholder. |
| 7 | SGR output formatting | `write_changed_styles` and `serialize_chunks` in `yinz-terminal/src/output/mod.rs`; `impl Display for CharacterStyles` in `yinz-terminal/src/panes/terminal_character.rs:568` | 3–12% of 1-pane frames | SGR goes through `core::fmt`, and each cell adds `anyhow` context. |
| 8 | Allocation churn | Row/`VecDeque` churn in `yinz-terminal/src/panes/grid.rs`; `CharacterChunk` in `yinz-terminal/src/output/mod.rs:1821` | 6–17% (all malloc/free) | Malloc/free from Row and `VecDeque` creation and destruction on scroll, scrollback eviction, and `CharacterChunk` Vec growth. `CharacterChunk` (1096 bytes) also holds its 256-entry `changed_colors` palette inline (about 1 KB), which costs two memcpys per chunk; `Rc` or borrow it. |
| 9 | `Grid::pane_contents` every render | `yinz-terminal/src/panes/grid.rs`, gathered in the tiled, floating, and overlay panes' `render` | Not in the bench | Runs for every pane of the rendered tab on every render, changed or not, even though no builtin plugin subscribes to `PaneRenderReport`. |

### One vte call per escape sequence

In ground state every ESC makes the interceptor return `Swallow`, which sets `capture_started` and flushes
`forwarded` through `advance_until_terminated`. Each escape sequence therefore gets its own vte call, its own
per-byte interceptor steps, and a copy into `forwarded`.

- **Option A, pre-scan (best measured).** After the `forward_paused` check, if the interceptor is in ground state,
  `bytes.last() != Some(&0x1b)`, and `memchr::memmem::find(bytes, b"\x1b_")` is `None`, pass `bytes` straight to
  `advance_until_terminated`, queue `bytes[consumed..]` in `pending_pty_input`, and return. Otherwise run the
  existing loop. Measured ingest: `sgr_churn` −11 to −16%, `cursor_moves` −4 to −7%, `alt_screen` −2 to −5%,
  `plain_text` −1 to −2%. A scalar `windows(2)` pre-scan made plain text about 3% slower.
- **Option B, no pre-scan.** Extend the ground-state run past `ESC x` when `x` is neither `_` nor ESC, since the
  interceptor forwards that pair and returns to ground anyway. Measured −8 to −10% on `sgr_churn`.

Option A landed first and was then replaced by a version of option B that scans once per run: the ground-state
run extends to the next `ESC _` (keeping a trailing ESC back), and the batch goes to vte only when a Kitty APC
starts or the read ends. See [Implemented](#implemented).

Why this is equivalent: `Grid::terminated()` looks only at `pending_forwarded_queries`, the leftover bytes after a
pause are the same, a partial UTF-8 sequence cut off by an ESC produces the same U+FFFD, and a trailing ESC falls
back to the loop so the interceptor's `Esc` state carries across reads.

Test with a property test in `yinz-panes`' `unit/` directory: feed random mixed streams at random split points
through the old and new paths, and compare the grid snapshot and `pending_pty_input`. Measure with
`cargo x bench -- ingest/`.

### Kitty direct-transmission capture

Inside `ESC _ G … ESC \`, every payload byte goes through `KittyApcInterceptor::advance`: a state match, an enum
return, a `Vec::push` with an overflow check, and a match in the caller. During a direct (`t=d`) upload this is
60–88% of the screen thread's ingest, about 35 ms for a 1080p RGBA image and about 10 ms for a yazi-sized
preview. Text never takes this path. Local `kitten icat` should avoid it too (inferred from the code, not
observed), because Yinz answers the `t=f`, `t=t`, and `t=s` probes.

| Variant (capture only, fat LTO) | ns/B |
| --- | --- |
| Current per-byte loop | 3.1–3.4 |
| Scalar bulk run: `position(matches!(b, 0x1b \| 0x9c \| 0x18 \| 0x1a))` + `extend_from_slice` | 0.47–0.51 |
| `memchr3(0x1b, 0x9c, 0x18)`, then `memchr(0x1a)` on the prefix | 0.049–0.074 |
| `fearless_simd` `u8x32` | 0.047–0.072 |

Add `is_capturing()` and a `capture_run(&mut self, bytes: &[u8]) -> usize` next to `is_in_ground_state` in
`interceptor.rs`. `capture_run` consumes payload bytes up to the first byte `advance_capturing` treats specially and
appends them in one `extend_from_slice`. To match `push_capture_byte`, which overflows on byte `MAX_CAPTURE_LEN + 1`,
set `overflowed` and clear the buffer exactly when `buffer.len() + run > MAX_CAPTURE_LEN`, and append nothing while
`overflowed` is already set (`push_capture_byte` ignores bytes after an overflow). Call it from
`TerminalPane::handle_pty_bytes` and `PluginPane::handle_plugin_bytes` when `is_capturing()`, setting
`capture_started = true` in the terminal pane. `forwarded` is always empty while capturing, so the skipped flushes
were no-ops.

Also replace `std::mem::take(&mut self.buffer)` at `interceptor.rs:119` and `:143` with
`std::mem::replace(&mut self.buffer, Vec::with_capacity(4096))`, so the buffer doesn't regrow from zero on every
chunk. This matters little once `capture_run` lands, because `extend_from_slice` then allocates each chunk once.

No bench covers this. Add a `kitty_direct` workload to `multi_pane_render.rs` (`a=T,f=32,m=1` in 4096-byte base64
chunks, 16 KiB reads) and a property test in `src/kitty_graphics/unit/` comparing per-byte and bulk driving across
random split points, plus `MAX_CAPTURE_LEN` boundary cases. Expected with a `memchr3` + `memchr` scan (add memchr,
locked at 2.7.6, as a direct dependency): about 38 ms to about 6 ms (about 11 ms with a scalar `position` scan) of
capture, parse, and base64-decode screen-thread CPU for a 1080p upload. Upload latency barely changes; the pty moves
200–300 MB/s.

### Image path

Cold and bursty, but cheap to fix:

- A scalar `scale_rgba` rewrite (`grid_state.rs:1992`): hoist the column table, round with `as u32` instead of
  calling libm `round`, preallocate the output, and return early for identity sizes.
- `crop_rgba` (`grid_state.rs:1965`): borrow the source when the rectangle covers the whole image.
- Cache the scaled variant in `place()`, keyed by `(c, r)`, source rectangle, and cell size; the current
  `(cols, rows)` key alone would reuse wrong pixels (see
  [Scaled variants overwrite each other](#scaled-variants-overwrite-each-other)).
- Expand RGB to RGBA with `u32` stores.
- Optionally, use `base64-simd` for both base64 directions.

## Kitty graphics bugs

Each was checked by writing tests against the code at `main`; the tests were not committed.

### Frame switches leave the host showing the old frame (confirmed)

`a=a,c=N` reaches `KittyGrid::animate` (`yinz-terminal-graphics/src/kitty_graphics/grid_state.rs:445-466`), which
calls `KittyImageStore::set_current_frame` (`store.rs:337-354`) and then `retransmit_placements_of`
(`grid_state.rs:568-575`). `set_current_frame` clears only the store's `scaled_variants` and `base64_cache`.
`retransmit_placements_of` gives each placement a fresh `placement_uid`, and the comment at `grid_state.rs:458-462`
says that is the signal to retransmit. It only forces a re-placement.

`HostKittyState.transmitted` (`yinz-terminal/src/output/mod.rs:399`) is keyed by `(InternalImageId, variant)`, and
neither changes on a frame switch. `serialize_kitty_frame` (`output/mod.rs:476`, `transmitted` lookup at `:570`)
therefore finds the old host copy and places it with `a=p,i=<old host id>`. The only removals are the
`host_display_cleared` wipe (`mod.rs:487-490`, set only by a tab's `\e[2J` redraw) and the freed-image sweep
(`mod.rs:491-503`), which fires only when the image leaves the store.

- **Trigger:** a program under a Kitty-capable host transmits and displays an image, adds a frame with `a=f`, then
  sends `a=a,i=<id>,c=2`. The host keeps showing the first frame. `a=c` onto the current frame (`compose_frames`,
  `grid_state.rs:474-541`) has the same problem.
- **Scaled placements:** `set_current_frame` drops `scaled_variants` and nothing rebuilds them. A host that already
  holds the variant keeps the stale scaled copy, and a client without it never gets the placement drawn
  (`output/mod.rs:574-583` hits `continue`).
- **Related, untested:** `a=f` with `r=` equal to the current frame edits the displayed raster, but `transmit_frame`
  calls neither `retransmit_placements_of` nor `render_full_viewport`, and `write_frame` leaves `scaled_variants`.
- **Repro tests:** `kitty_switching_the_current_frame_sends_the_host_the_new_frame` in
  `yinz-terminal/src/output/unit/output_tests.rs`, and the end-to-end
  `anim_making_a_frame_current_changes_what_the_host_displays` in
  `yinz-terminal/src/panes/unit/grid_tests/kitty_animation.rs`. Both failed: after `a=a,c=2` the second render
  sent no `a=t` and the host displayed the root frame.
- **Fix sketch:** give each stored image a displayed-raster generation, bumped by `set_current_frame`, by
  `write_frame` and `compose_frames`/`transmit_frame` when they touch the current frame, and by `remove_frames` when
  it resets `current_frame`. Record the generation each host copy was sent at, either in `HostImageKey` or as
  `(host_id, generation)` in `transmitted`. On a mismatch, send `a=d,q=2,d=I,i=<old>`, drop the entry, and
  retransmit. Rebuild scaled variants from `displayed_rgba()` after a frame change, as the cell-resize path does
  (`grid_state.rs:1640`, `:1698`). Make `transmit_frame` re-uid and re-render like `animate`, and correct the
  comment. A throwaway version (about 15 lines) passed both repro tests and all 187 `kitty_` tests in `yinz-terminal`.

### Scaled variants overwrite each other (confirmed)

`KittyGrid::place` (`grid_state.rs:674-690`) crops the source rectangle (`x,y,w,h`), scales it, and stores it with
`add_scaled_variant(internal, dest_cells, …)`, keyed only by `(cols, rows)` (`store.rs:33`, `:265`). It also sets
`variant_key: dest_cells` (`grid_state.rs:750`). Two scaled placements of one image with the same cell box but
different crops share one slot, and the last write wins. `HostKittyState.transmitted` uses the same key
(`output/mod.rs:387`, `:566-595`) and is not cleared when a variant's pixels change, so the host keeps whichever crop
it received first. A cell resize (`character_cell_size_possibly_changed`, `grid_state.rs:1586-1660`) rebuilds every
placement into the shared key and recreates the collision.

- **Trigger:** `a=p,i=1,p=1,x=0,w=1,c=1,r=1` followed by `a=p,i=1,p=2,x=1,w=1,c=1,r=1`. Sprite sheets, tiled
  images, and viewers that pan or zoom by changing `x,y,w,h` at a fixed `c`/`r` do this. Placed in one frame, both
  show the last crop. Placed after a render, the second gets the first crop's host image. Re-cropping one placement
  sends nothing (`output/mod.rs:606-611` hits `continue`). Unscaled crops are unaffected.
- **Related, untested:** a `U=1` virtual placement (`grid_state.rs:994`) uses the same `(columns, rows)` key with a
  raster of a different size, so it can collide with a normal placement's variant and send `s,v` that don't match
  the payload.
- **Repro tests:** `scaled_placements_of_different_source_rectangles_keep_their_own_pixels` in
  `yinz-terminal-graphics/src/kitty_graphics/unit/grid_state_spec_tests.rs`, and
  `scaled_placements_of_different_crops_placed_in_one_frame_show_their_own_crops`,
  `a_second_scaled_crop_placed_after_a_render_is_not_served_the_first_crops_host_image`, and
  `re_placing_a_scaled_placement_with_a_new_crop_updates_what_the_host_shows` in
  `yinz-terminal/src/output/unit/output_kitty_spec_tests.rs`. All four failed.
- **Fix sketch:** make the source rectangle part of the variant key everywhere: `scaled_variants`, `base64_cache`,
  `KittyPlacement::variant_key`, and `KittyImageChunk::variant_key`, built from `source_rect` in `place()` and the
  resize pass and from `source` plus a virtual marker in `place_virtual`. `HostImageKey` then distinguishes crops,
  and a re-crop gets a new host image id, so the placement is re-emitted. Free variants when their last placement
  goes away, or extra keys grow the store until quota eviction.

### `scale_rgba` length precondition (not a bug)

`scale_rgba` (`grid_state.rs:1992`) indexes `src` from `src_w` and `src_h` without checking its length, and
`crop_rgba` (`:1965`) truncates rather than clamps, so a short crop would panic. No client input reaches that:
`place` rejects or clamps out-of-bounds rectangles (`grid_state.rs:591-608`), image dimensions never change under an
internal id, the parser rejects short raw payloads and truncates long ones (`parser.rs:806-839`), PNG output always
matches its reported size, and animation frames start from full-size canvases. As optional hardening, add a
`debug_assert!` or an early return for `src.len() < src_w * src_h * 4`, and correct `crop_rgba`'s comment, which
says it clamps.

## Placeholder comments in library sources

`yinz-tile-utils/src/lib.rs` and `yinz-tile-markdown/src/lib.rs` each begin with ten committed lines reading
`// test comment line N - this is just harmless placeholder text for a test change`, followed by five
`// test comment N` lines.
