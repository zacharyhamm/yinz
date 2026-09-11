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
use zellij_types::diagnostics::BackgroundJobContext;
use zellij_types::diagnostics::PluginContext;
use zellij_types::diagnostics::PtyContext;
use zellij_types::diagnostics::PtyWriteContext;
use zellij_types::diagnostics::ScreenContext;
use zellij_types::diagnostics::ServerContext;
use zellij_types::geometry::{Direction, Resize, ResizeStrategy};
use zellij_types::input::cli_assets::CliAssets;
use zellij_types::internal_ids::{ClientId, PluginId, TerminalId};
use zellij_types::pane_size::Size;

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
        targets: [$(($target_name:literal, $sample:expr, $target:literal)),* $(,)?];
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
        "PtyBytes",
        "SubscribePaneOutput",
        "UnsubscribePaneOutput",
        "RemotePaneReady",
        "ClearAllPluginHighlights",
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
            ScreenInstruction::SubscribePaneOutput(PluginId::new(1), ClientId::new(1), None),
            "HandlePtyBytes"
        ),
        (
            "UnsubscribePaneOutput",
            ScreenInstruction::UnsubscribePaneOutput(PluginId::new(1)),
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
    special: ["PostMessagesToPluginWorker"];
    only_special: ["PostMessageToPluginWorker"];
    targets: [(
        "PostMessagesToPluginWorker",
        PluginInstruction::PostMessagesToPluginWorker(
            PluginId::new(1),
            ClientId::new(1),
            "worker".to_owned(),
            vec![],
        ),
        "PostMessageToPluginWorker"
    )];
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
    special: ["FirstClientConnected", "AttachWatcherClient", "ResolveLockedEscape"];
    only_special: ["NewClient", "AttachClient"];
    targets: [
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
            "ResolveLockedEscape",
            ServerInstruction::ResolveLockedEscape(ClientId::new(1), uuid::Uuid::nil()),
            "ChangeMode"
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
