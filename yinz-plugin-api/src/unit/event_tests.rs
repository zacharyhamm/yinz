use super::*;

#[test]
#[cfg(target_pointer_width = "64")]
fn tab_position_overflow_is_rejected_without_dropping_session_tabs() {
    let tab = TabInfo {
        group: Some(yinz_types::tab_bar::TabGroupInfo {
            id: 7,
            name: "Work".into(),
        }),
        position: TabPosition::new(usize::MAX),
        ..Default::default()
    };
    assert!(ProtobufTabInfo::try_from(tab.clone()).is_err());
    let session = SessionInfo {
        tabs: vec![tab],
        ..Default::default()
    };
    assert!(ProtobufSessionManifest::try_from(session).is_err());
}

#[test]
#[cfg(target_pointer_width = "64")]
fn tab_id_overflow_is_rejected_at_the_protobuf_boundary() {
    let tab = TabInfo {
        tab_id: TabId::new(usize::MAX),
        ..Default::default()
    };
    assert!(ProtobufTabInfo::try_from(tab).is_err());
    assert!(
        ProtobufClientTabHistory::try_from((ClientId::new(1), vec![TabId::new(usize::MAX)]))
            .is_err()
    );
}

#[test]
fn web_request_stream_chunk_round_trips_through_protobuf() {
    // a data chunk and the terminator carrying an error - the two shapes a plugin sees
    for original in [
        Event::WebRequestStreamChunk {
            stream_id: "stream-1".to_owned(),
            chunk: b"data: {\"n\":1}\n\n".to_vec(),
            done: false,
            error: None,
        },
        Event::WebRequestStreamChunk {
            stream_id: "stream-1".to_owned(),
            chunk: vec![],
            done: true,
            error: Some("cancelled".to_owned()),
        },
    ] {
        let protobuf: ProtobufEvent = original.clone().try_into().expect("encode");
        let decoded: Event = protobuf.try_into().expect("decode");
        assert_eq!(decoded, original);
    }
}

/// The two flags that qualify a reading of a pane: whether the user has scrolled it
/// back, and how long ago its process last wrote anything. A plugin cannot tell work
/// from a repaint without them, so they have to survive the plugin wire boundary.
#[test]
fn pane_contents_carries_scroll_and_output_age_across_protobuf() {
    for original in [
        PaneContents {
            viewport: vec!["waiting for you".to_owned()],
            last_output_ms_ago: Some(yinz_types::units::ElapsedMillis::new(4_200)),
            is_scrolled: true,
            ..Default::default()
        },
        PaneContents {
            viewport: vec!["nothing written yet".to_owned()],
            last_output_ms_ago: None,
            is_scrolled: false,
            ..Default::default()
        },
    ] {
        let protobuf: ProtobufPaneContents = original.clone().try_into().expect("encode");
        let decoded: PaneContents = protobuf.try_into().expect("decode");
        assert_eq!(decoded, original);
    }
}

#[test]
fn web_request_stream_chunk_event_type_round_trips() {
    let protobuf: ProtobufEventType = EventType::WebRequestStreamChunk.try_into().unwrap();
    let decoded: EventType = protobuf.try_into().unwrap();
    assert_eq!(decoded, EventType::WebRequestStreamChunk);
}

mod props {
    use super::*;
    use hegel::generators;
    use std::path::PathBuf;
    use {
        crate::test_generators::bytes, crate::test_generators::context,
        crate::test_generators::pane_id, crate::test_generators::paths_with_metadata,
        crate::test_generators::strings,
    };
    use {
        yinz_types::data::CopyDestination, yinz_types::data::HostTerminalThemeMode,
        yinz_types::data::PermissionStatus,
    };

    /// Never draws `actions`: they are deliberately absent from the wire, so an item with
    /// actions could not survive a round trip and the property would be asserting the wrong
    /// thing. What has to survive is identity - `id` - and everything the plugin draws with.
    fn any_context_menu_item(tc: &hegel::TestCase) -> ContextMenuItem {
        ContextMenuItem {
            actions: vec![],
            ..crate::test_generators::context_menu_items(tc)
        }
    }

    fn any_context_menu_context(tc: &hegel::TestCase) -> ContextMenuContext {
        let item_count = tc.draw(generators::integers::<usize>().max_value(3));
        ContextMenuContext {
            request_id: ContextMenuRequestId::new(tc.draw(generators::integers::<u64>())),
            client_id: ClientId::new(tc.draw(generators::integers::<u32>())),
            pane_id: pane_id(tc),
            tab_position: TabPosition::new(tc.draw(generators::integers::<u16>()) as usize),
            click: (
                tc.draw(generators::integers::<u16>()) as usize,
                tc.draw(generators::integers::<u16>()) as usize,
            ),
            click_in_pane: (
                tc.draw(generators::integers::<u16>()) as usize,
                tc.draw(generators::integers::<u16>()) as usize,
            ),
            on_frame: tc.draw(generators::booleans()),
            pane_title: strings(tc),
            pane_command: tc.draw(generators::optional(generators::text())),
            pane_exited: tc.draw(generators::booleans()),
            pane_exit_status: tc.draw(generators::optional(generators::integers::<i32>())),
            pane_is_held: tc.draw(generators::booleans()),
            pane_is_floating: tc.draw(generators::booleans()),
            selected_text: tc.draw(generators::optional(generators::text())),
            word_under_cursor: tc.draw(generators::optional(generators::text())),
            line_under_cursor: tc.draw(generators::optional(generators::text())),
            link: tc.draw(generators::booleans()).then(|| ContextMenuLink {
                plugin_id: yinz_types::internal_ids::PluginId::new(
                    tc.draw(generators::integers::<u32>()),
                ),
                pattern: strings(tc),
                matched: strings(tc),
                context: context(tc),
            }),
            items: (0..item_count).map(|_| any_context_menu_item(tc)).collect(),
        }
    }

    /// Every `Event` variant whose payload is built from plain scalars,
    /// collections and small enums.
    ///
    /// `ModeUpdate`, `TabUpdate`, `PaneUpdate`, `Key`, `Mouse`,
    /// `SessionUpdate`, `ListClients`, `UserAction`, `ActionComplete`,
    /// `PaneRenderReport`, `InitialKeybinds`, `AvailableLayoutInfo`,
    /// `HintText` and `InterceptedKeyPress` are not here: they carry
    /// `ModeInfo`, `Action` and friends, which need generators of their
    /// own. Their example tests stay.
    ///
    /// `Timer` draws a finite f64 on purpose - a NaN payload would fail the
    /// round-trip assertion on `NaN != NaN` rather than on anything the
    /// encoding did wrong.
    #[hegel::composite]
    fn any_event(tc: &hegel::TestCase) -> Event {
        match tc.draw(generators::integers::<u8>().max_value(34)) {
            34 => Event::AgentEvent(yinz_types::agent::AgentEvent {
                pane_id: yinz_types::internal_ids::TerminalId::new(
                    tc.draw(generators::integers::<u32>()),
                ),
                kind: strings(tc),
                hook: strings(tc),
                session_id: tc.draw(generators::optional(generators::text().max_size(8))),
                state: tc.draw(generators::optional(generators::sampled_from(vec![
                    AgentState::Busy,
                    AgentState::Waiting,
                    AgentState::Idle,
                    AgentState::Exited,
                ]))),
                detail: tc.draw(generators::optional(generators::text().max_size(8))),
                prompt: tc.draw(generators::optional(generators::text().max_size(8))),
                last_message: tc.draw(generators::optional(generators::text().max_size(8))),
                tool_name: tc.draw(generators::optional(generators::text().max_size(8))),
                cwd: tc.draw(generators::optional(generators::text().max_size(8))),
                raw: strings(tc),
            }),
            33 => Event::Hook {
                point: tc.draw(generators::sampled_from(HookPoint::ALL.to_vec())),
                payload: match tc.draw(generators::integers::<u8>().max_value(2)) {
                    0 => yinz_types::data::HookPayload::Agent {
                        pane_id: yinz_types::internal_ids::TerminalId::new(
                            tc.draw(generators::integers::<u32>()),
                        ),
                        agent: crate::test_generators::agent_infos(tc),
                        previous_state: tc.draw(generators::optional(generators::sampled_from(
                            vec![
                                AgentState::Busy,
                                AgentState::Waiting,
                                AgentState::Idle,
                                AgentState::Exited,
                            ],
                        ))),
                        command: tc.draw(generators::optional(generators::text().max_size(8))),
                    },
                    1 => yinz_types::data::HookPayload::PaneExit {
                        pane_id: yinz_types::internal_ids::TerminalId::new(
                            tc.draw(generators::integers::<u32>()),
                        ),
                        exit_code: tc.draw(generators::optional(generators::integers::<i32>())),
                        cwd: tc
                            .draw(generators::optional(generators::text().max_size(8)))
                            .map(PathBuf::from),
                        command: tc.draw(generators::optional(generators::text().max_size(8))),
                        held: tc.draw(generators::booleans()),
                    },
                    _ => yinz_types::data::HookPayload::SessionStart {
                        session_name: strings(tc).into(),
                        client_id: ClientId::new(tc.draw(generators::integers::<u32>())),
                    },
                },
            },
            31 => Event::ContextMenuRequested(Box::new(any_context_menu_context(tc))),
            32 => Event::ContextMenuItemSelected {
                label: strings(tc),
                item_context: context(tc),
                context: Box::new(any_context_menu_context(tc)),
            },
            // The protobuf field is a float, not a double (see
            // `TimerPayload(f32)` in assets/prost/api.event.rs), so the
            // conversion is lossy for any f64 that is not exactly
            // representable in 32 bits - `Timer(16777217.0)` comes back as
            // `Timer(16777216.0)`. Drawing through f32 keeps this property
            // about the encoding rather than about that rounding.
            0 => Event::Timer(yinz_types::units::FractionalSeconds::new(
                tc.draw(
                    generators::floats::<f32>()
                        .min_value(-1.0e9)
                        .max_value(1.0e9),
                ) as f64,
            )),
            1 => Event::Visible(tc.draw(generators::booleans())),
            2 => Event::InputReceived,
            3 => Event::SystemClipboardFailure,
            4 => Event::ConfigWasWrittenToDisk,
            5 => Event::BeforeClose,
            6 => Event::CopyToClipboard(tc.draw(generators::sampled_from(vec![
                CopyDestination::Command,
                CopyDestination::Primary,
                CopyDestination::System,
            ]))),
            7 => Event::CustomMessage(strings(tc), strings(tc)),
            8 => Event::FileSystemCreate(paths_with_metadata(tc)),
            9 => Event::FileSystemRead(paths_with_metadata(tc)),
            10 => Event::FileSystemUpdate(paths_with_metadata(tc)),
            11 => Event::FileSystemDelete(paths_with_metadata(tc)),
            12 => Event::PermissionRequestResult(tc.draw(generators::sampled_from(vec![
                PermissionStatus::Granted,
                PermissionStatus::Denied,
            ]))),
            13 => Event::RunCommandResult(
                tc.draw(generators::optional(generators::integers::<i32>())),
                bytes(tc),
                bytes(tc),
                context(tc),
            ),
            14 => Event::WebRequestResult(
                tc.draw(generators::integers::<u16>()),
                context(tc),
                bytes(tc),
                context(tc),
            ),
            15 => Event::WebRequestStreamChunk {
                stream_id: strings(tc),
                chunk: bytes(tc),
                done: tc.draw(generators::booleans()),
                error: tc.draw(generators::optional(generators::text())),
            },
            16 => Event::CommandPaneOpened(
                yinz_types::internal_ids::TerminalId::new(tc.draw(generators::integers::<u32>())),
                context(tc),
            ),
            17 => Event::CommandPaneExited(
                yinz_types::internal_ids::TerminalId::new(tc.draw(generators::integers::<u32>())),
                tc.draw(generators::optional(generators::integers::<i32>())),
                context(tc),
            ),
            18 => Event::PaneClosed(pane_id(tc)),
            19 => Event::EditPaneOpened(
                yinz_types::internal_ids::TerminalId::new(tc.draw(generators::integers::<u32>())),
                context(tc),
            ),
            20 => Event::EditPaneExited(
                yinz_types::internal_ids::TerminalId::new(tc.draw(generators::integers::<u32>())),
                tc.draw(generators::optional(generators::integers::<i32>())),
                context(tc),
            ),
            21 => Event::CommandPaneReRun(
                yinz_types::internal_ids::TerminalId::new(tc.draw(generators::integers::<u32>())),
                context(tc),
            ),
            22 => Event::FailedToWriteConfigToDisk {
                file_path: tc.draw(generators::optional(generators::text())),
                reason: tc.draw(generators::optional(generators::text())),
            },
            23 => Event::HostFolderChanged(PathBuf::from(strings(tc))),
            24 => {
                Event::FailedToChangeHostFolder(tc.draw(generators::optional(generators::text())))
            },
            25 => Event::PastedText(strings(tc)),
            26 => Event::PluginConfigurationChanged(context(tc)),
            27 => Event::PluginConfigurationChanged(context(tc)),
            28 | 29 => Event::PluginConfigurationChanged(context(tc)),
            _ => Event::HostTerminalThemeChanged(tc.draw(generators::sampled_from(vec![
                HostTerminalThemeMode::Dark,
                HostTerminalThemeMode::Light,
            ]))),
        }
    }

    #[hegel::test]
    fn every_event_survives_a_protobuf_round_trip(tc: hegel::TestCase) {
        use prost::Message;
        let event = tc.draw(any_event());
        let protobuf: ProtobufEvent = event
            .clone()
            .try_into()
            .unwrap_or_else(|e| panic!("{:?} failed to encode: {:?}", event, e));
        let bytes = protobuf.encode_to_vec();
        let decoded_protobuf: ProtobufEvent = Message::decode(bytes.as_slice())
            .unwrap_or_else(|e| panic!("{:?} failed to decode: {:?}", event, e));
        let round_tripped: Event = decoded_protobuf
            .try_into()
            .unwrap_or_else(|e| panic!("{:?} failed to convert back: {:?}", event, e));
        assert_eq!(
            round_tripped, event,
            "protobuf round trip changed the event"
        );
    }
}

#[test]
fn serialize_mode_update_event() {
    use prost::Message;
    let mode_update_event = Event::ModeUpdate(Default::default());
    let protobuf_event: ProtobufEvent = mode_update_event.clone().try_into().unwrap();
    let serialized_protobuf_event = protobuf_event.encode_to_vec();
    let deserialized_protobuf_event: ProtobufEvent =
        Message::decode(serialized_protobuf_event.as_slice()).unwrap();
    let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
    assert_eq!(
        mode_update_event, deserialized_event,
        "Event properly serialized/deserialized without change"
    );
}

#[test]
fn serialize_hook_event() {
    use prost::Message;
    let payloads = [
        yinz_types::data::HookPayload::AgentExit(BTreeMap::from([
            ("agent_pane_id".into(), "7".into()),
            ("agent_exit_code".into(), "17".into()),
            ("agent_command".into(), "claude --resume sid".into()),
        ])),
        yinz_types::data::HookPayload::Agent {
            pane_id: yinz_types::internal_ids::TerminalId::new(7),
            agent: AgentInfo {
                kind: "claude".to_owned(),
                state: AgentState::Waiting,
                source: AgentSource::Reported,
                detail: Some("y/n".to_owned()),
                waiting_on: None,
                wait_needs_a_person: true,
                cwd: Some("/work".to_owned()),
                session_id: Some("s1".to_owned()),
                run_id: None,
            },
            previous_state: Some(AgentState::Busy),
            command: Some("claude".to_owned()),
        },
        yinz_types::data::HookPayload::PaneExit {
            pane_id: yinz_types::internal_ids::TerminalId::new(7),
            exit_code: Some(1),
            cwd: Some(PathBuf::from("/work")),
            command: Some("cargo test".to_owned()),
            held: true,
        },
        yinz_types::data::HookPayload::SessionStart {
            session_name: "s".to_owned().into(),
            client_id: ClientId::new(1),
        },
    ];
    for (point, payload) in HookPoint::ALL.iter().zip(payloads.iter().cycle()) {
        let hook_event = Event::Hook {
            point: *point,
            payload: payload.clone(),
        };
        let protobuf_event: ProtobufEvent = hook_event.clone().try_into().unwrap();
        let serialized_protobuf_event = protobuf_event.encode_to_vec();
        let deserialized_protobuf_event: ProtobufEvent =
            Message::decode(serialized_protobuf_event.as_slice()).unwrap();
        let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
        assert_eq!(hook_event, deserialized_event, "{point}");
    }
}

#[test]
fn serialize_mode_update_event_with_non_default_values() {
    use prost::Message;
    use {
        yinz_types::data::BareKey, yinz_types::data::Palette, yinz_types::data::PaletteColor,
        yinz_types::data::ThemeHue,
    };
    let mode_update_event = Event::ModeUpdate(ModeInfo {
        tab_groups: Some(false),
        mode: InputMode::Locked,
        keybinds: vec![
            (
                InputMode::Locked,
                vec![(
                    KeyWithModifier::new(BareKey::Char('b')).with_alt_modifier(),
                    vec![Action::SwitchToMode {
                        input_mode: InputMode::Normal,
                    }],
                )],
            ),
            (
                InputMode::Tab,
                vec![(
                    KeyWithModifier::new(BareKey::Up).with_alt_modifier(),
                    vec![Action::SwitchToMode {
                        input_mode: InputMode::Pane,
                    }],
                )],
            ),
            (
                InputMode::Pane,
                vec![
                    (
                        KeyWithModifier::new(BareKey::Char('b')).with_ctrl_modifier(),
                        vec![
                            Action::SwitchToMode {
                                input_mode: InputMode::Tmux,
                            },
                            Action::Write {
                                key_with_modifier: None,
                                bytes: vec![10],
                                is_kitty_keyboard_protocol: false,
                            },
                        ],
                    ),
                    (
                        KeyWithModifier::new(BareKey::Char('a')),
                        vec![Action::WriteChars {
                            chars: "foo".to_owned(),
                        }],
                    ),
                ],
            ),
        ],
        style: Style {
            agent_theme_mapping: Default::default(),
            terminal: None,
            colors: Palette {
                source: yinz_types::data::PaletteSource::Default,
                theme_hue: ThemeHue::Light,
                fg: PaletteColor::Rgb((1, 1, 1)),
                bg: PaletteColor::Rgb((200, 200, 200)),
                black: PaletteColor::EightBit(1),
                red: PaletteColor::EightBit(2),
                green: PaletteColor::EightBit(2),
                yellow: PaletteColor::EightBit(2),
                blue: PaletteColor::EightBit(2),
                magenta: PaletteColor::EightBit(2),
                cyan: PaletteColor::EightBit(2),
                white: PaletteColor::EightBit(2),
                orange: PaletteColor::EightBit(2),
                gray: PaletteColor::EightBit(2),
                purple: PaletteColor::EightBit(2),
                gold: PaletteColor::EightBit(2),
                silver: PaletteColor::EightBit(2),
                pink: PaletteColor::EightBit(2),
                brown: PaletteColor::Rgb((222, 221, 220)),
            }
            .into(),
            rounded_corners: true,
            hide_session_name: false,
        },
        capabilities: PluginCapabilities {
            arrow_fonts: false,
            kitty_graphics: true,
        },
        session_name: Some("my awesome test session".to_owned().into()),
        base_mode: Some(InputMode::Locked),
        editor: Some(PathBuf::from("my_awesome_editor")),
        shell: Some(PathBuf::from("my_awesome_shell")),
        currently_marking_pane_group: Some(false),
        pane_frame_style: Some(yinz_types::input::options::PaneFrameStyle::Titles),
        session_dimmed: Some(true),
        session_ancestry: vec!["work".to_owned().into(), "prod".to_owned().into()],
        host_fullscreen: Some(true),
        nested_ascend_keys: vec![
            KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Up),
        ],
        session_ascended: Some(true),
        nested_descend_keys: vec![
            KeyWithModifier::new(BareKey::Char('o')).with_ctrl_modifier(),
            KeyWithModifier::new(BareKey::Down),
        ],
    });
    let protobuf_event: ProtobufEvent = mode_update_event.clone().try_into().unwrap();
    let serialized_protobuf_event = protobuf_event.encode_to_vec();
    let deserialized_protobuf_event: ProtobufEvent =
        Message::decode(serialized_protobuf_event.as_slice()).unwrap();
    let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
    assert_eq!(
        mode_update_event, deserialized_event,
        "Event properly serialized/deserialized without change"
    );
}

#[test]
fn serialize_tab_update_event() {
    use prost::Message;
    let tab_update_event = Event::TabUpdate(Default::default());
    let protobuf_event: ProtobufEvent = tab_update_event.clone().try_into().unwrap();
    let serialized_protobuf_event = protobuf_event.encode_to_vec();
    let deserialized_protobuf_event: ProtobufEvent =
        Message::decode(serialized_protobuf_event.as_slice()).unwrap();
    let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
    assert_eq!(
        tab_update_event, deserialized_event,
        "Event properly serialized/deserialized without change"
    );
}

#[test]
fn serialize_tab_update_event_with_non_default_values() {
    use prost::Message;
    let tab_update_event = Event::TabUpdate(vec![
        TabInfo {
            group: Some(yinz_types::tab_bar::TabGroupInfo {
                id: 7,
                name: "Work".into(),
            }),
            position: TabPosition::new(0),
            name: "First tab".to_owned(),
            active: true,
            panes_to_hide: 2,
            is_fullscreen_active: true,
            is_sync_panes_active: false,
            are_floating_panes_visible: true,
            other_focused_clients: vec![ClientId::new(2), ClientId::new(3), ClientId::new(4)],
            other_focused_client_slots: vec![ColorSlot::new(2), None, ColorSlot::new(10)],
            active_swap_layout_name: Some("my cool swap layout".to_owned()),
            is_swap_layout_dirty: false,
            viewport_rows: yinz_types::position::RowCount::new(10),
            viewport_columns: yinz_types::position::ColumnCount::new(10),
            display_area_rows: yinz_types::position::RowCount::new(10),
            display_area_columns: yinz_types::position::ColumnCount::new(10),
            selectable_tiled_panes_count: 10,
            selectable_floating_panes_count: 10,
            tab_id: TabId::new(0),
            has_bell_notification: false,
            is_flashing_bell: false,
            // deliberately larger than the display area, and different on each axis: the two pairs
            // are separate rectangles and a round trip that swapped them would still pass if they
            // agreed
        },
        TabInfo {
            group: Some(yinz_types::tab_bar::TabGroupInfo {
                id: 7,
                name: "Work".into(),
            }),
            position: TabPosition::new(1),
            name: "Secondtab".to_owned(),
            active: false,
            panes_to_hide: 5,
            is_fullscreen_active: false,
            is_sync_panes_active: true,
            are_floating_panes_visible: true,
            other_focused_clients: vec![ClientId::new(1), ClientId::new(5), ClientId::new(111)],
            other_focused_client_slots: vec![ColorSlot::new(2), None, ColorSlot::new(10)],
            active_swap_layout_name: None,
            is_swap_layout_dirty: true,
            viewport_rows: yinz_types::position::RowCount::new(10),
            viewport_columns: yinz_types::position::ColumnCount::new(10),
            display_area_rows: yinz_types::position::RowCount::new(10),
            display_area_columns: yinz_types::position::ColumnCount::new(10),
            selectable_tiled_panes_count: 10,
            selectable_floating_panes_count: 10,
            tab_id: TabId::new(1),
            has_bell_notification: false,
            is_flashing_bell: false,
        },
        TabInfo::default(),
    ]);
    let protobuf_event: ProtobufEvent = tab_update_event.clone().try_into().unwrap();
    let serialized_protobuf_event = protobuf_event.encode_to_vec();
    let deserialized_protobuf_event: ProtobufEvent =
        Message::decode(serialized_protobuf_event.as_slice()).unwrap();
    let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
    assert_eq!(
        tab_update_event, deserialized_event,
        "Event properly serialized/deserialized without change"
    );
}

#[test]
fn serialize_pane_update_event() {
    use prost::Message;

    let mut panes = HashMap::new();
    panes.insert(
        TabPosition::new(0),
        vec![PaneInfo {
            id: yinz_types::pane_id::PaneId::from_legacy(42, false),
            title: "shell".into(),
            display_title: Some("host project:worktree: shell".into()),
            ..Default::default()
        }],
    );
    let pane_update_event = Event::PaneUpdate(PaneManifest { panes });
    let protobuf_event: ProtobufEvent = pane_update_event.clone().try_into().unwrap();
    let serialized_protobuf_event = protobuf_event.encode_to_vec();
    let deserialized_protobuf_event: ProtobufEvent =
        Message::decode(serialized_protobuf_event.as_slice()).unwrap();
    let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
    assert_eq!(
        pane_update_event, deserialized_event,
        "Event properly serialized/deserialized without change"
    );
}

#[test]
fn serialize_hint_text_event() {
    use prost::Message;
    let hint_text_event = Event::HintText(BTreeMap::from([
        (
            10,
            StyledText {
                text: "group".to_owned(),
                indices: vec![vec![0]],
            },
        ),
        (
            42,
            StyledText {
                text: "resize".to_owned(),
                indices: vec![vec![0, 1], vec![3]],
            },
        ),
    ]));
    let protobuf_event: ProtobufEvent = hint_text_event.clone().try_into().unwrap();
    let serialized_protobuf_event = protobuf_event.encode_to_vec();
    let deserialized_protobuf_event: ProtobufEvent =
        Message::decode(serialized_protobuf_event.as_slice()).unwrap();
    let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
    assert_eq!(
        hint_text_event, deserialized_event,
        "Event properly serialized/deserialized without change"
    );
}

#[test]
fn serialize_active_pane_scroll_event() {
    use prost::Message;
    for active_pane_scroll_event in [
        Event::ActivePaneScroll(Some((3, 12))),
        Event::ActivePaneScroll(None),
    ] {
        let protobuf_event: ProtobufEvent = active_pane_scroll_event.clone().try_into().unwrap();
        let serialized_protobuf_event = protobuf_event.encode_to_vec();
        let deserialized_protobuf_event: ProtobufEvent =
            Message::decode(serialized_protobuf_event.as_slice()).unwrap();
        let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
        assert_eq!(
            active_pane_scroll_event, deserialized_event,
            "Event properly serialized/deserialized without change"
        );
    }
}

#[test]
fn serialize_key_event() {
    use prost::Message;
    use yinz_types::data::BareKey;
    let key_event = Event::Key(KeyWithModifier::new(BareKey::Char('a')).with_ctrl_modifier());
    let protobuf_event: ProtobufEvent = key_event.clone().try_into().unwrap();
    let serialized_protobuf_event = protobuf_event.encode_to_vec();
    let deserialized_protobuf_event: ProtobufEvent =
        Message::decode(serialized_protobuf_event.as_slice()).unwrap();
    let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
    assert_eq!(
        key_event, deserialized_event,
        "Event properly serialized/deserialized without change"
    );
}

#[test]
fn serialize_mouse_event() {
    use prost::Message;
    let mouse_event = Event::Mouse(Mouse::LeftClick(1, 1));
    let protobuf_event: ProtobufEvent = mouse_event.clone().try_into().unwrap();
    let serialized_protobuf_event = protobuf_event.encode_to_vec();
    let deserialized_protobuf_event: ProtobufEvent =
        Message::decode(serialized_protobuf_event.as_slice()).unwrap();
    let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
    assert_eq!(
        mouse_event, deserialized_event,
        "Event properly serialized/deserialized without change"
    );
}

#[test]
fn serialize_mouse_event_without_position() {
    use prost::Message;
    let mouse_event = Event::Mouse(Mouse::ScrollUp(17));
    let protobuf_event: ProtobufEvent = mouse_event.clone().try_into().unwrap();
    let serialized_protobuf_event = protobuf_event.encode_to_vec();
    let deserialized_protobuf_event: ProtobufEvent =
        Message::decode(serialized_protobuf_event.as_slice()).unwrap();
    let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
    assert_eq!(
        mouse_event, deserialized_event,
        "Event properly serialized/deserialized without change"
    );
}

#[test]
fn serialize_timer_event() {
    use prost::Message;
    let timer_event = Event::Timer(yinz_types::units::FractionalSeconds::new(1.5));
    let protobuf_event: ProtobufEvent = timer_event.clone().try_into().unwrap();
    let serialized_protobuf_event = protobuf_event.encode_to_vec();
    assert_eq!(serialized_protobuf_event, vec![8, 5, 61, 0, 0, 192, 63]);
    let deserialized_protobuf_event: ProtobufEvent =
        Message::decode(serialized_protobuf_event.as_slice()).unwrap();
    let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
    assert_eq!(
        timer_event, deserialized_event,
        "Event properly serialized/deserialized without change"
    );
}

#[test]
fn serialize_session_update_event() {
    use prost::Message;
    let session_update_event = Event::SessionUpdate(Default::default(), Default::default());
    let protobuf_event: ProtobufEvent = session_update_event.clone().try_into().unwrap();
    let serialized_protobuf_event = protobuf_event.encode_to_vec();
    let deserialized_protobuf_event: ProtobufEvent =
        Message::decode(serialized_protobuf_event.as_slice()).unwrap();
    let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
    assert_eq!(
        session_update_event, deserialized_event,
        "Event properly serialized/deserialized without change"
    );
}

#[test]
fn serialize_session_update_event_with_non_default_values() {
    use prost::Message;
    let tab_infos = vec![
        TabInfo {
            group: Some(yinz_types::tab_bar::TabGroupInfo {
                id: 7,
                name: "Work".into(),
            }),
            position: TabPosition::new(0),
            name: "First tab".to_owned(),
            active: true,
            panes_to_hide: 2,
            is_fullscreen_active: true,
            is_sync_panes_active: false,
            are_floating_panes_visible: true,
            other_focused_clients: vec![ClientId::new(2), ClientId::new(3), ClientId::new(4)],
            other_focused_client_slots: vec![ColorSlot::new(2), None, ColorSlot::new(10)],
            active_swap_layout_name: Some("my cool swap layout".to_owned()),
            is_swap_layout_dirty: false,
            viewport_rows: yinz_types::position::RowCount::new(10),
            viewport_columns: yinz_types::position::ColumnCount::new(10),
            display_area_rows: yinz_types::position::RowCount::new(10),
            display_area_columns: yinz_types::position::ColumnCount::new(10),
            selectable_tiled_panes_count: 10,
            selectable_floating_panes_count: 10,
            tab_id: TabId::new(0),
            has_bell_notification: false,
            is_flashing_bell: false,
            // deliberately larger than the display area, and different on each axis: the two pairs
            // are separate rectangles and a round trip that swapped them would still pass if they
            // agreed
        },
        TabInfo {
            group: Some(yinz_types::tab_bar::TabGroupInfo {
                id: 7,
                name: "Work".into(),
            }),
            position: TabPosition::new(1),
            name: "Secondtab".to_owned(),
            active: false,
            panes_to_hide: 5,
            is_fullscreen_active: false,
            is_sync_panes_active: true,
            are_floating_panes_visible: true,
            other_focused_clients: vec![ClientId::new(1), ClientId::new(5), ClientId::new(111)],
            other_focused_client_slots: vec![ColorSlot::new(2), None, ColorSlot::new(10)],
            active_swap_layout_name: None,
            is_swap_layout_dirty: true,
            viewport_rows: yinz_types::position::RowCount::new(10),
            viewport_columns: yinz_types::position::ColumnCount::new(10),
            display_area_rows: yinz_types::position::RowCount::new(10),
            display_area_columns: yinz_types::position::ColumnCount::new(10),
            selectable_tiled_panes_count: 10,
            selectable_floating_panes_count: 10,
            tab_id: TabId::new(1),
            has_bell_notification: false,
            is_flashing_bell: false,
        },
        TabInfo::default(),
    ];
    let mut panes = HashMap::new();
    let mut index_in_pane_group_1 = BTreeMap::new();
    index_in_pane_group_1.insert(ClientId::new(1), 0);
    index_in_pane_group_1.insert(ClientId::new(2), 0);
    index_in_pane_group_1.insert(ClientId::new(3), 0);
    let mut index_in_pane_group_2 = BTreeMap::new();
    index_in_pane_group_2.insert(ClientId::new(1), 1);
    index_in_pane_group_2.insert(ClientId::new(2), 1);
    index_in_pane_group_2.insert(ClientId::new(3), 1);
    let panes_list = vec![
        PaneInfo {
            pane_tabs: None,
            multipane: None,
            host: Some("build-box".to_owned()),
            target: PaneTarget::Ssh {
                host: "build-box".to_owned(),
            },
            agent: None,
            id: yinz_types::pane_id::PaneId::from_legacy(1, false),
            is_focused: true,
            is_fullscreen: true,
            is_floating: false,
            is_suppressed: false,
            is_stack_list_member: false,
            title: "pane 1".to_owned(),
            display_title: None,
            exited: false,
            exit_status: None,
            restart_error: Some("Cannot restart agent: unsupported print option".into()),
            last_agent_session_id: Some("ended-session".into()),
            is_held: false,
            pane_x: yinz_types::position::Column::new(0),
            pane_content_x: yinz_types::position::Column::new(1),
            pane_y: yinz_types::position::Row::new(0),
            pane_content_y: yinz_types::position::Row::new(1),
            pane_rows: yinz_types::position::RowCount::new(5),
            pane_content_rows: yinz_types::position::RowCount::new(4),
            pane_columns: yinz_types::position::ColumnCount::new(22),
            pane_content_columns: yinz_types::position::ColumnCount::new(21),
            cursor_coordinates_in_pane: Some((0, 0).into()),
            terminal_command: Some("foo".to_owned()),
            plugin_url: None,
            is_selectable: true,
            index_in_pane_group: index_in_pane_group_1,
            default_fg: None,
            default_bg: None,
            cwd: Some("/work/agent".to_owned()),
        },
        PaneInfo {
            pane_tabs: None,
            multipane: None,
            host: None,
            target: Default::default(),
            agent: None,
            id: yinz_types::pane_id::PaneId::from_legacy(1, true),
            is_focused: true,
            is_fullscreen: true,
            is_floating: false,
            is_suppressed: false,
            is_stack_list_member: false,
            title: "pane 1".to_owned(),
            display_title: None,
            exited: false,
            exit_status: None,
            restart_error: None,
            last_agent_session_id: None,
            is_held: false,
            pane_x: yinz_types::position::Column::new(0),
            pane_content_x: yinz_types::position::Column::new(1),
            pane_y: yinz_types::position::Row::new(0),
            pane_content_y: yinz_types::position::Row::new(1),
            pane_rows: yinz_types::position::RowCount::new(5),
            pane_content_rows: yinz_types::position::RowCount::new(4),
            pane_columns: yinz_types::position::ColumnCount::new(22),
            pane_content_columns: yinz_types::position::ColumnCount::new(21),
            cursor_coordinates_in_pane: Some((0, 0).into()),
            terminal_command: None,
            plugin_url: Some("i_am_a_fake_plugin".to_owned()),
            is_selectable: true,
            index_in_pane_group: index_in_pane_group_2,
            default_fg: None,
            default_bg: None,
            cwd: None,
        },
    ];
    panes.insert(TabPosition::new(0), panes_list);
    let mut plugins = BTreeMap::new();
    let mut plugin_configuration = BTreeMap::new();
    plugin_configuration.insert("config_key".to_owned(), "config_value".to_owned());
    plugins.insert(
        yinz_types::internal_ids::PluginId::new(1),
        PluginInfo {
            location: "file:/path/to/my-plugin.scm".to_owned(),
            configuration: plugin_configuration,
        },
    );
    let mut tab_history = BTreeMap::new();
    tab_history.insert(
        ClientId::new(1),
        [1, 2, 3].into_iter().map(TabId::new).collect(),
    );
    tab_history.insert(
        ClientId::new(2),
        [1, 2, 3].into_iter().map(TabId::new).collect(),
    );
    let session_info_1 = SessionInfo {
        name: "session 1".to_owned().into(),
        tabs: tab_infos,
        panes: PaneManifest { panes },
        connected_clients: 2,
        is_current_session: true,
        available_layouts: vec![
            LayoutInfo::File(
                "layout 1".to_owned(),
                LayoutMetadata {
                    tabs: vec![],
                    creation_time: "0".to_owned(),
                    update_time: "0".to_owned(),
                },
            ),
            LayoutInfo::BuiltIn("layout2".to_owned()),
            LayoutInfo::File(
                "layout3".to_owned(),
                LayoutMetadata {
                    tabs: vec![],
                    creation_time: "0".to_owned(),
                    update_time: "0".to_owned(),
                },
            ),
        ],
        plugins,
        tab_history,
        pane_history: Default::default(),
        creation_time: Duration::from_secs(100),
    };
    let session_info_2 = SessionInfo {
        name: "session 2".to_owned().into(),
        tabs: vec![],
        panes: PaneManifest {
            panes: HashMap::new(),
        },
        connected_clients: 0,
        is_current_session: false,
        available_layouts: vec![
            LayoutInfo::File(
                "layout 1".to_owned(),
                LayoutMetadata {
                    tabs: vec![],
                    creation_time: "0".to_owned(),
                    update_time: "0".to_owned(),
                },
            ),
            LayoutInfo::BuiltIn("layout2".to_owned()),
            LayoutInfo::File(
                "layout3".to_owned(),
                LayoutMetadata {
                    tabs: vec![],
                    creation_time: "0".to_owned(),
                    update_time: "0".to_owned(),
                },
            ),
        ],
        plugins: Default::default(),
        tab_history: Default::default(),
        pane_history: Default::default(),
        creation_time: Duration::from_secs(200),
    };
    let session_infos = vec![session_info_1, session_info_2];
    let resurrectable_sessions = vec![];

    let session_update_event = Event::SessionUpdate(session_infos, resurrectable_sessions);
    let protobuf_event: ProtobufEvent = session_update_event.clone().try_into().unwrap();
    let serialized_protobuf_event = protobuf_event.encode_to_vec();
    let deserialized_protobuf_event: ProtobufEvent =
        Message::decode(serialized_protobuf_event.as_slice()).unwrap();
    let deserialized_event: Event = deserialized_protobuf_event.try_into().unwrap();
    assert_eq!(
        session_update_event, deserialized_event,
        "Event properly serialized/deserialized without change"
    );
}

#[test]
fn cursor_protobuf_preserves_xy_order_and_rejects_invalid_coordinates() {
    use prost::Message;
    let cursor = CellCoordinates::new(Column::new(7), Row::new(3));
    let proto = ProtobufPosition::try_from(cursor).unwrap();
    assert_eq!(proto.encode_to_vec(), [8, 3, 16, 7]);
    assert_eq!(CellCoordinates::try_from(proto).unwrap(), cursor);
    for (line, column) in [(-1, 0), (0, -1)] {
        assert!(CellCoordinates::try_from(ProtobufPosition { line, column }).is_err());
    }
    if usize::BITS == 64 {
        assert!(ProtobufPosition::try_from(CellCoordinates::from((usize::MAX, 0))).is_err());
        assert!(ProtobufPosition::try_from(CellCoordinates::from((0, usize::MAX))).is_err());
        let pane = PaneInfo {
            pane_columns: ColumnCount::new(usize::MAX),
            ..Default::default()
        };
        assert!(ProtobufPaneInfo::try_from(pane).is_err());
    }
}

#[test]
fn multipane_manifest_metadata_roundtrips_and_rejects_invalid_ratios() {
    use yinz_types::multipane::*;
    let info = MultipaneInfo {
        id: MultipaneId(71),
        children: [
            yinz_types::pane_id::PaneId::from_legacy(3, true),
            yinz_types::pane_id::PaneId::from_legacy(4, false),
        ],
        x: 5,
        y: 6,
        rows: 20,
        columns: 80,
        split: MultipaneSplit {
            direction: MultipaneDirection::TopBottom,
            ratio: 3700,
        },
    };
    let pane = PaneInfo {
        multipane: Some(info.clone()),
        ..Default::default()
    };
    let wire = ProtobufPaneInfo::try_from(pane.clone()).unwrap();
    assert_eq!(PaneInfo::try_from(wire.clone()).unwrap(), pane);
    let mut wire = wire;
    wire.multipane.as_mut().unwrap().ratio = 10_000;
    assert!(PaneInfo::try_from(wire).is_err());
}

#[cfg(test)]
#[test]
fn notification_events_and_decisions_round_trip_through_plugin_wire() {
    use yinz_types::internal_ids::{NotificationId, PluginId, TerminalId};
    use yinz_types::notifications::*;
    let notification = NotificationData {
        id: NotificationId::new(7),
        received_at_unix_ms: yinz_types::units::UnixMillis::new(123),
        source: NotificationSource::Osc99,
        pane_id: Some(PaneId::Terminal(TerminalId::new(3))),
        plugin_id: Some(PluginId::new(9)),
        title: "title".into(),
        body: "original body".into(),
        delivery: NotificationDelivery::Desktop,
    };
    for decision in [
        NotificationDecision::Pass,
        NotificationDecision::Suppress,
        NotificationDecision::Replace {
            title: None,
            body: Some("rewritten".into()),
            delivery: Some(NotificationDelivery::Both),
        },
    ] {
        let outcome = NotificationResolution::from_decision(
            notification.clone(),
            decision.clone(),
            Some("example".into()),
        );
        for event in [
            Event::NotificationReceived(notification.clone()),
            Event::NotificationRequested(notification.clone()),
            Event::NotificationResolved(outcome.clone()),
        ] {
            let wire: ProtobufEvent = event.clone().try_into().unwrap();
            assert_eq!(Event::try_from(wire).unwrap(), event);
        }
        let command = yinz_types::data::PluginCommand::ResolveNotification {
            id: notification.id,
            decision,
        };
        let wire: crate::plugin_command::ProtobufPluginCommand =
            command.clone().try_into().unwrap();
        assert_eq!(
            yinz_types::data::PluginCommand::try_from(wire).unwrap(),
            command
        );
        let info = yinz_types::data::NotificationInfo {
            id: notification.id,
            received_at_unix_ms: notification.received_at_unix_ms,
            message: outcome.message(),
            outcome: Some(outcome),
        };
        let wire: crate::plugin_command::ProtobufNotificationInfo = info.clone().into();
        assert_eq!(yinz_types::data::NotificationInfo::from(wire), info);
    }
}
