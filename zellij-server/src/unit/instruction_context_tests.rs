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
use zellij_utils::errors::{
    BackgroundJobContext, PluginContext, PtyContext, PtyWriteContext, ScreenContext, ServerContext,
};

/// Generates the three guards for one instruction/context pair.
///
/// `special:` lists the instruction variants the `From` impl spells out by hand — either because
/// the context is chosen by payload or because the two enums name it differently. `only_special:`
/// lists context variants that only such an arm can produce, so `every_context_is_reachable` does
/// not report them as dead.
macro_rules! pin_context_to_instruction {
    (
        mod $name:ident;
        instruction: $instruction:ty;
        context: $context:ty;
        special: [$($special:literal),* $(,)?];
        only_special: [$($only_special:literal),* $(,)?];
    ) => {
        mod $name {
            use super::*;

            const SPECIAL: &[&str] = &[$($special),*];
            const ONLY_SPECIAL: &[&str] = &[$($only_special),*];

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
                        "these ", stringify!($instruction), " variants have no ",
                        stringify!($context), " of the same name; add the variant there or \
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
                        "no ", stringify!($instruction), " can produce these ",
                        stringify!($context), " variants, so they are dead: {:?}"
                    ),
                    unreachable
                );
            }

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
}

pin_context_to_instruction! {
    mod plugin;
    instruction: PluginInstruction;
    context: PluginContext;
    special: ["PostMessagesToPluginWorker"];
    only_special: ["PostMessageToPluginWorker"];
}

pin_context_to_instruction! {
    mod pty;
    instruction: PtyInstruction;
    context: PtyContext;
    special: [];
    only_special: [];
}

pin_context_to_instruction! {
    mod server;
    instruction: ServerInstruction;
    context: ServerContext;
    special: ["FirstClientConnected", "AttachWatcherClient"];
    only_special: ["NewClient", "AttachClient"];
}

pin_context_to_instruction! {
    mod background_job;
    instruction: BackgroundJob;
    context: BackgroundJobContext;
    // the two cancel/finish jobs share the WebRequest context with the request itself
    special: ["CancelWebRequest", "WebRequestStreamFinished"];
    only_special: [];
}

pin_context_to_instruction! {
    mod pty_write;
    instruction: PtyWriteInstruction;
    context: PtyWriteContext;
    special: [];
    only_special: [];
}
