//! Pins each instruction enum to the `*Context` enum that names its variants.
//!
//! Every `From<&…Instruction> for …Context` resolves the common case through
//! `ScreenContext::from_str(instruction.into())`, which is not compiler-exhaustive the way the
//! hand-written matches it replaced were. These tests take that job over, and do more than the
//! matches could: they also catch the two enums' names drifting apart, and a context variant no
//! instruction can reach.

use strum::VariantNames;

use crate::background_jobs::BackgroundJob;
use crate::plugins::PluginInstruction;
use crate::pty::PtyInstruction;
use crate::pty_writer::PtyWriteInstruction;
use crate::screen::ScreenInstruction;
use crate::ServerInstruction;
use std::str::FromStr;

#[test]
fn asynchronous_plugin_events_preserve_generation_and_legacy_routing() {
    use yinz_types::data::Event;
    use yinz_types::internal_ids::{ClientId, PluginId};

    let plugin = PluginId::new(7);
    let client = ClientId::new(2);
    let tagged =
        PluginInstruction::for_plugin_event(plugin, client, Some(41), Event::InputReceived);
    assert!(matches!(
        tagged,
        PluginInstruction::PluginEvent(id, origin, 41, Event::InputReceived)
            if id == plugin && origin == client
    ));
    assert_eq!(PluginContext::from(&tagged), PluginContext::Update);
    match PluginInstruction::for_plugin_event(plugin, client, None, Event::InputReceived) {
        PluginInstruction::Update(events) => {
            assert_eq!(
                events,
                vec![(Some(plugin), Some(client), Event::InputReceived)]
            );
        },
        other => panic!("legacy completion changed routing: {other:?}"),
    }
}
use yinz_types::diagnostics::BackgroundJobContext;
use yinz_types::diagnostics::PluginContext;
use yinz_types::diagnostics::PtyContext;
use yinz_types::diagnostics::PtyWriteContext;
use yinz_types::diagnostics::ScreenContext;
use yinz_types::diagnostics::ServerContext;
use yinz_types::geometry::{Direction, Resize, ResizeStrategy};
use yinz_types::input::cli_assets::CliAssets;
use yinz_types::internal_ids::{ClientId, PluginId, TerminalId};
use yinz_types::pane_size::Size;

/// Generates the three guards for one instruction/context pair.
///
/// `special:` lists the instruction variants the `From` impl spells out by hand — either because
/// the context is chosen by payload or because the two enums name it differently. `only_special:`
/// lists context variants that only such an arm can produce, so `every_context_is_reachable` does
/// not report them as dead.
/// The two guards that need no hand-written arm: every instruction name is a context name,
/// and every context name is an instruction name (or listed as reachable only by a special
/// arm).
macro_rules! by_name_guards {
    ($instruction:ty, $context:ty) => {
        /// Stands in for the exhaustive match the conversion used to be.
        #[test]
        fn every_instruction_resolves() {
            let unmapped: Vec<&str> = <$instruction>::VARIANTS
                .iter()
                .copied()
                .filter(|name| !SPECIAL.contains(name))
                .filter(|name| <$context>::from_str(name).is_err())
                .collect();
            assert!(
                unmapped.is_empty(),
                concat!(
                    "these ",
                    stringify!($instruction),
                    " variants have no ",
                    stringify!($context),
                    " of the same name; add the variant there or \
                         special-case it in the From impl: {:?}"
                ),
                unmapped
            );
        }

        /// A context no instruction can produce is dead weight.
        #[test]
        fn every_context_is_reachable() {
            let unreachable: Vec<&str> = <$context>::VARIANTS
                .iter()
                .copied()
                .filter(|name| !ONLY_SPECIAL.contains(name))
                .filter(|name| !<$instruction>::VARIANTS.contains(name))
                .collect();
            assert!(
                unreachable.is_empty(),
                concat!(
                    "no ",
                    stringify!($instruction),
                    " can produce these ",
                    stringify!($context),
                    " variants, so they are dead: {:?}"
                ),
                unreachable
            );
        }
    };
}

macro_rules! pin_context_to_instruction {
    // A `From` impl that is pure `from_str` has no arm to pin, so the two list-keeping tests
    // would be empty functions that can never fail. This arm emits only the by-name guards;
    // an instantiation reaches it by spelling all three lists empty on purpose.
    (
        mod $name:ident;
        instruction: $instruction:ty;
        context: $context:ty;
        special: [];
        only_special: [];
        targets: [];
    ) => {
        mod $name {
            use super::*;

            const SPECIAL: &[&str] = &[];
            const ONLY_SPECIAL: &[&str] = &[];

            by_name_guards!($instruction, $context);
        }
    };
    (
        mod $name:ident;
        instruction: $instruction:ty;
        context: $context:ty;
        special: [$($special:literal),* $(,)?];
        only_special: [$($only_special:literal),* $(,)?];
        targets: [$(($target_name:literal, $sample:expr_2021, $target:literal)),* $(,)?];
    ) => {
        mod $name {
            use super::*;

            const SPECIAL: &[&str] = &[$($special),*];
            const ONLY_SPECIAL: &[&str] = &[$($only_special),*];

            by_name_guards!($instruction, $context);

            /// Keeps the two lists honest: a name that no longer exists, or a special case
            /// `from_str` could have resolved anyway, is a stale entry.
            #[test]
            fn the_special_case_lists_are_current() {
                for name in SPECIAL {
                    assert!(
                        <$instruction>::VARIANTS.contains(name),
                        "{name} is no longer a {} variant", stringify!($instruction)
                    );
                    assert!(
                        <$context>::from_str(name).is_err(),
                        "{name} now names a {} too, so its arm in the From impl is redundant",
                        stringify!($context)
                    );
                }
                for name in ONLY_SPECIAL {
                    assert!(
                        <$context>::VARIANTS.contains(name),
                        "{name} is no longer a {} variant", stringify!($context)
                    );
                }
            }

            /// The hand-written arms are the ones the compiler cannot check, so this pins where
            /// each one lands: every `SPECIAL` name has at least one sample instruction here, and
            /// converting it yields the context the sample says. A name in `SPECIAL` with no arm
            /// behind it falls to the catch-all and lands on the wrong context, which the
            /// other three tests cannot see.
            #[test]
            fn special_cases_land_on_their_context() {
                let targets: Vec<(&str, $instruction, &str)> =
                    vec![$(($target_name, $sample, $target)),*];
                for (name, sample, target) in &targets {
                    let sample_name: &'static str = sample.into();
                    assert_eq!(
                        sample_name, *name,
                        "the sample for {name} is a {sample_name}"
                    );
                    assert!(
                        SPECIAL.contains(name),
                        "{name} has a target but is not listed in SPECIAL"
                    );
                    let expected = <$context>::from_str(target).unwrap_or_else(|_| {
                        panic!("{target} is not a {} variant", stringify!($context))
                    });
                    assert_eq!(
                        <$context>::from(sample),
                        expected,
                        "{name} does not land on {}::{target}", stringify!($context)
                    );
                }
                for name in SPECIAL {
                    assert!(
                        targets.iter().any(|(target_name, ..)| target_name == name),
                        "{name} is in SPECIAL but has no sample in targets, so nothing checks \
                         where its arm points"
                    );
                }
            }
        }
    };
}

pin_context_to_instruction! {
    mod screen;
    instruction: ScreenInstruction;
    context: ScreenContext;
    // `Resize` picks its context from the payload; the rest are named differently
    special: [
        "Resize",
        "PaletteThemeSaved",
        "PtyBytes",
        "SubscribePaneOutput",
        "UnsubscribePaneOutput",
        "RemotePaneReady",
        "ClearAllPluginHighlights",
        "CommandPalette",
        "RenderWriterReady",
        "AgentWorktreeResolved",
    ];
    only_special: [
        "HandlePtyBytes",
        "ResizeIncreaseAll",
        "ResizeIncreaseLeft",
        "ResizeIncreaseDown",
        "ResizeIncreaseUp",
        "ResizeIncreaseRight",
        "ResizeDecreaseAll",
        "ResizeDecreaseLeft",
        "ResizeDecreaseDown",
        "ResizeDecreaseUp",
        "ResizeDecreaseRight",
    ];
    targets: [
        ("PaletteThemeSaved", ScreenInstruction::PaletteThemeSaved { client: ClientId::new(1), request: uuid::Uuid::nil(), result: Ok(()) }, "ToggleCommandPalette"),
        ("RenderWriterReady", ScreenInstruction::RenderWriterReady, "RenderToClients"),
        ("AgentWorktreeResolved", ScreenInstruction::AgentWorktreeResolved { cwd: "/tmp".into(), generation: 1, root: None }, "RenderToClients"),
        (
            "CommandPalette",
            ScreenInstruction::CommandPalette(crate::command_palette::Instruction::Discover { client: ClientId::new(1), opening: 1, revision: 1 }),
            "ToggleCommandPalette"
        ),
        (
            "Resize",
            ScreenInstruction::Resize(
                ClientId::new(1),
                ResizeStrategy::new(Resize::Increase, Some(Direction::Left)),
                None,
            ),
            "ResizeIncreaseLeft"
        ),
        (
            "Resize",
            ScreenInstruction::Resize(
                ClientId::new(1),
                ResizeStrategy::new(Resize::Increase, Some(Direction::Down)),
                None,
            ),
            "ResizeIncreaseDown"
        ),
        (
            "Resize",
            ScreenInstruction::Resize(
                ClientId::new(1),
                ResizeStrategy::new(Resize::Increase, Some(Direction::Up)),
                None,
            ),
            "ResizeIncreaseUp"
        ),
        (
            "Resize",
            ScreenInstruction::Resize(
                ClientId::new(1),
                ResizeStrategy::new(Resize::Increase, Some(Direction::Right)),
                None,
            ),
            "ResizeIncreaseRight"
        ),
        (
            "Resize",
            ScreenInstruction::Resize(
                ClientId::new(1),
                ResizeStrategy::new(Resize::Increase, None),
                None,
            ),
            "ResizeIncreaseAll"
        ),
        (
            "Resize",
            ScreenInstruction::Resize(
                ClientId::new(1),
                ResizeStrategy::new(Resize::Decrease, Some(Direction::Left)),
                None,
            ),
            "ResizeDecreaseLeft"
        ),
        (
            "Resize",
            ScreenInstruction::Resize(
                ClientId::new(1),
                ResizeStrategy::new(Resize::Decrease, Some(Direction::Down)),
                None,
            ),
            "ResizeDecreaseDown"
        ),
        (
            "Resize",
            ScreenInstruction::Resize(
                ClientId::new(1),
                ResizeStrategy::new(Resize::Decrease, Some(Direction::Up)),
                None,
            ),
            "ResizeDecreaseUp"
        ),
        (
            "Resize",
            ScreenInstruction::Resize(
                ClientId::new(1),
                ResizeStrategy::new(Resize::Decrease, Some(Direction::Right)),
                None,
            ),
            "ResizeDecreaseRight"
        ),
        (
            "Resize",
            ScreenInstruction::Resize(
                ClientId::new(1),
                ResizeStrategy::new(Resize::Decrease, None),
                None,
            ),
            "ResizeDecreaseAll"
        ),
        (
            "PtyBytes",
            ScreenInstruction::PtyBytes(TerminalId::new(1), vec![]),
            "HandlePtyBytes"
        ),
        (
            "SubscribePaneOutput",
            ScreenInstruction::SubscribePaneOutput(PluginId::new(1), ClientId::new(1), None, None),
            "HandlePtyBytes"
        ),
        (
            "UnsubscribePaneOutput",
            ScreenInstruction::UnsubscribePaneOutput(PluginId::new(1), None),
            "HandlePtyBytes"
        ),
        (
            "RemotePaneReady",
            ScreenInstruction::RemotePaneReady(TerminalId::new(1)),
            "HandlePtyBytes"
        ),
        (
            "ClearAllPluginHighlights",
            ScreenInstruction::ClearAllPluginHighlights(PluginId::new(1)),
            "ClearPluginHighlights"
        ),
    ];
}

pin_context_to_instruction! {
    mod plugin;
    instruction: PluginInstruction;
    context: PluginContext;
    special: ["DataRequest", "RefreshDataWatches", "PostMessagesToPluginWorker", "PluginTick", "PluginPalette", "PaletteAgent", "PaletteAgentLabels", "PluginEvent", "PluginReply", "PluginDispose", "ReleasePluginInstancePipes", "PipeCompleted", "InputBarrier"];
    only_special: ["PostMessageToPluginWorker", "UnblockCliPipes"];
    targets: [(
        "RefreshDataWatches",
        PluginInstruction::RefreshDataWatches { namespace: "shared:agents".into(), entities: Default::default() },
        "Update"
    ), (
        "DataRequest",
        PluginInstruction::DataRequest { request: yinz_types::cli::DataRequest::Catalog { namespace: None }, reply: std::sync::mpsc::channel().0 },
        "Update"
    ), (
        "InputBarrier",
        PluginInstruction::InputBarrier(PluginId::new(1), crate::route::NotificationEnd::new(tokio::sync::oneshot::channel().0)),
        "Update"
    ), (
        "PostMessagesToPluginWorker",
        PluginInstruction::PostMessagesToPluginWorker(
            PluginId::new(1),
            ClientId::new(1),
            "worker".to_owned(),
            vec![],
        ),
        "PostMessageToPluginWorker"
    ), (
        "PaletteAgent",
        PluginInstruction::PaletteAgent(Box::new(crate::command_palette::AgentRequest {
            timezone: Default::default(),
            client: ClientId::new(1), opening: 1, step: 1,
            source: Default::default(), action: crate::command_palette::AgentAction::Fork,
            values: vec![], active: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        })),
        "Update"
    ), (
        "PaletteAgentLabels",
        PluginInstruction::PaletteAgentLabels(crate::command_palette::AgentLabelsRequest {
            client: ClientId::new(1), opening: 1, revision: 1, sources: vec![],
        }),
        "Update"
    ), (
        "PluginPalette",
        PluginInstruction::PluginPalette(crate::command_palette::Delivery {
            owner: crate::command_palette::Owner { plugin_id: PluginId::new(1), generation: 1, client_id: None },
            client: ClientId::new(1), message: yinz_types::command_palette::Message::Disconnect, active: None,
        }),
        "Update"
    ), (
        "PluginTick",
        PluginInstruction::PluginTick(PluginId::new(1), ClientId::new(1), 1),
        "Update"
    ), (
        "PluginEvent",
        PluginInstruction::PluginEvent(PluginId::new(1), ClientId::new(1), 1, yinz_types::data::Event::BeforeClose),
        "Update"
    ), (
        "PluginReply",
        {
            let _mailbox = crate::thread_bus::PluginReplies::new(9001);
            let (sender, receiver) = yinz_runtime::channels::unbounded();
            let senders = crate::thread_bus::ThreadSenders { to_plugin: Some(yinz_runtime::channels::SenderWithContext::new(sender)), ..Default::default() };
            senders.send_to_plugin(PluginInstruction::PluginEvent(PluginId::new(1), ClientId::new(1), 9001, yinz_types::data::Event::BeforeClose)).unwrap();
            receiver.try_recv().unwrap().0
        },
        "Update"
    ), (
        "PluginDispose",
        PluginInstruction::PluginDispose(PluginId::new(1), 1),
        "Unload"
    ), (
        "PipeCompleted",
        PluginInstruction::PipeCompleted(crate::plugins::PipeCompletion {
            plugin_id: PluginId::new(1), instance_id: Default::default(), changes: Default::default(), callback: None,
        }),
        "UnblockCliPipes"
    ), (
        "ReleasePluginInstancePipes",
        PluginInstruction::ReleasePluginInstancePipes(PluginId::new(1), Default::default()),
        "UnblockCliPipes"
    )];
}

#[test]
fn disconnected_cli_pipes_have_a_diagnostic_context() {
    assert_eq!(
        PluginContext::from(&PluginInstruction::CliPipesDisconnected(vec![
            "owned-pipe".into(),
        ])),
        PluginContext::CliPipesDisconnected,
    );
}

pin_context_to_instruction! {
    mod pty;
    instruction: PtyInstruction;
    context: PtyContext;
    special: [];
    only_special: [];
    targets: [];
}

pin_context_to_instruction! {
    mod server;
    instruction: ServerInstruction;
    context: ServerContext;
    special: ["ConfigurationEditor", "SavePaletteTheme", "FirstClientConnected", "AttachWatcherClient", "ResolveLockedEscape", "SetCommandPaletteOpen", "KillSessionWithCompletion", "CliPipeError"];
    only_special: ["NewClient", "AttachClient"];
    targets: [
        (
            "ConfigurationEditor",
            ServerInstruction::ConfigurationEditor { edit: None, theme: None, reply: crossbeam::channel::bounded(1).0 },
            "Reconfigure"
        ),
        (
            "SavePaletteTheme",
            ServerInstruction::SavePaletteTheme { cancelled: Default::default(), client: ClientId::new(1), request: uuid::Uuid::nil(), name: "default".into(), permanent: true },
            "Reconfigure"
        ),
        (
            "FirstClientConnected",
            ServerInstruction::FirstClientConnected(CliAssets::default(), ClientId::new(1)),
            "NewClient"
        ),
        (
            "AttachWatcherClient",
            ServerInstruction::AttachWatcherClient(ClientId::new(1), Size::default()),
            "AttachClient"
        ),
        (
            "KillSessionWithCompletion",
            ServerInstruction::KillSessionWithCompletion { client_id: ClientId::new(1), report_progress: true },
            "KillSession"
        ),
        (
            "SetCommandPaletteOpen",
            ServerInstruction::SetCommandPaletteOpen(ClientId::new(1), true),
            "ChangeMode"
        ),
        (
            "ResolveLockedEscape",
            ServerInstruction::ResolveLockedEscape(ClientId::new(1), uuid::Uuid::nil()),
            "ChangeMode"
        ),
        (
            "CliPipeError",
            ServerInstruction::CliPipeError("pipe".into(), "load failed".into()),
            "LogError"
        ),
    ];
}

pin_context_to_instruction! {
    mod background_job;
    instruction: BackgroundJob;
    context: BackgroundJobContext;
    // the two cancel/finish jobs share the WebRequest context with the request itself
    special: ["CancelWebRequest", "WebRequestStreamFinished"];
    only_special: [];
    targets: [
        (
            "CancelWebRequest",
            BackgroundJob::CancelWebRequest(PluginId::new(1), ClientId::new(1), "s".to_owned()),
            "WebRequest"
        ),
        (
            "WebRequestStreamFinished",
            BackgroundJob::WebRequestStreamFinished(
                (PluginId::new(1), ClientId::new(1), "s".to_owned()),
                0,
                None,
            ),
            "WebRequest"
        ),
    ];
}

pin_context_to_instruction! {
    mod pty_write;
    instruction: PtyWriteInstruction;
    context: PtyWriteContext;
    special: [];
    only_special: [];
    targets: [];
}
