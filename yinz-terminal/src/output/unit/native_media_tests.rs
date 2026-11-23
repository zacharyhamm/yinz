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
        media: vec![(1, first), (2, second)],
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
    }

    #[derive(Debug, Default)]
    struct Frame {
        shared: Vec<std::path::PathBuf>,
        inline: usize,
        placements: usize,
        cells: bool,
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
            }
        }

        fn viewer(&mut self, id: ClientId, budget: MediaBudget) {
            let state = shared(&self.root, budget);
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
            let ids: Vec<_> = self.states.keys().copied().collect();
            for id in ids {
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
                    let mut frame = Frame {
                        cells: view.rows().any(|(_, cells)| cells[0].character() == 'Z'),
                        ..Default::default()
                    };
                    for record in view.graphics() {
                        match record {
                            GraphicsRecord::Residency {
                                medium: GraphicsMedium::SharedFile,
                                data,
                                ..
                            } => frame.shared.push(String::from_utf8(data).unwrap().into()),
                            GraphicsRecord::Residency { .. } => frame.inline += 1,
                            GraphicsRecord::Placement { .. } => frame.placements += 1,
                            _ => {},
                        }
                    }
                    (id, frame)
                })
                .collect()
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
        // A portable viewer without a shared root keeps today's inline delivery.
        let mut portable = NativeRenderState::negotiated(
            Default::default(),
            None,
            Some(MediaBudget::new(4, 2 * IMAGE)),
        );
        assert_eq!(portable.stage_image(&vec![0; 4 * IMAGE]), Staging::Inline);
        assert_eq!(portable.media_usage(), MediaUsage::default());
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
}
