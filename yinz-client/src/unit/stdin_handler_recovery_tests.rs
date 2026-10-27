use super::*;
use crate::os_input_output::stamp_stdin_consumption;
use yinz_types::keys::BareKey;

#[test]
fn input_consumption_across_recovery_never_receives_a_connected_stamp() {
    use crate::{command_is_executing::CommandIsExecuting, remote_recovery::RecoveryControl};
    for start_offline in [false, true] {
        let control = RecoveryControl::new(CommandIsExecuting::new());
        if start_offline {
            control.lost(0, "outage".into());
        }
        let (bytes, stamp) = stamp_stdin_consumption(Some(&control), || {
            let bytes = b"pasted during outage";
            // Simulate the pump being descheduled after reading, before its final stamp.
            control.lost(0, "outage".into());
            control.replacing();
            control.repainted();
            bytes
        });
        assert_eq!(bytes, b"pasted during outage");
        assert_eq!(stamp, Some((1, false)));
        assert_eq!(
            stamp_stdin_consumption(Some(&control), || b"fresh").1,
            Some((1, true))
        );
    }
}

#[test]
fn input_consumption_during_repainting_or_disconnection_stays_ineligible() {
    use crate::{command_is_executing::CommandIsExecuting, remote_recovery::RecoveryControl};
    let control = RecoveryControl::new(CommandIsExecuting::new());
    assert_eq!(
        stamp_stdin_consumption(Some(&control), || ()).1,
        Some((0, true))
    );
    control.lost(0, "outage".into());
    assert_eq!(
        stamp_stdin_consumption(Some(&control), || ()).1,
        Some((0, false))
    );
    control.replacing();
    assert_eq!(
        stamp_stdin_consumption(Some(&control), || control.repainted()).1,
        Some((1, false))
    );
    assert_eq!(
        stamp_stdin_consumption(Some(&control), || control.lost(1, "outage".into())).1,
        Some((1, false))
    );
    assert_eq!(stamp_stdin_consumption(None, || b"local"), (b"local", None));
}

#[test]
fn discarded_csi_does_not_block_later_kitty_keys() {
    let kitty = b"\x1b[97;1:1u";
    for trigger in [
        b"\x1b[?1u".as_slice(),
        b"\x1b[<u",
        b"\x1b[<0;1M",
        b"\x1b[<0;;1m",
        b"\x1b[<999;1;1M",
    ] {
        for split in 0..=trigger.len() {
            for idle in [false, true] {
                let mut parser = InputParser::new();
                let mut pending = Vec::new();
                let mut held = BTreeSet::new();
                for chunk in [&trigger[..split], &trigger[split..]] {
                    let (instructions, _) = parse_keyboard_bytes(
                        &mut parser,
                        &mut pending,
                        chunk,
                        true,
                        true,
                        false,
                        &mut held,
                    );
                    assert!(instructions.is_empty(), "{trigger:?}: {instructions:?}");
                }
                assert!(pending.is_empty(), "{trigger:?}: {pending:?}");
                if idle {
                    let (instructions, _) = parse_keyboard_bytes(
                        &mut parser,
                        &mut pending,
                        &[],
                        true,
                        false,
                        false,
                        &mut held,
                    );
                    assert!(instructions.is_empty());
                    assert!(pending.is_empty());
                }
                let (instructions, _) = parse_keyboard_bytes(
                    &mut parser,
                    &mut pending,
                    kitty,
                    true,
                    true,
                    false,
                    &mut held,
                );
                assert_kitty_a(&instructions, kitty);
                assert!(pending.is_empty());
            }
        }

        let mut parser = InputParser::new();
        let mut pending = Vec::new();
        let mut held = BTreeSet::new();
        let (instructions, _) = parse_keyboard_bytes(
            &mut parser,
            &mut pending,
            &[trigger, kitty].concat(),
            true,
            true,
            false,
            &mut held,
        );
        assert_kitty_a(&instructions, kitty);
        assert!(pending.is_empty());
    }
}

fn assert_kitty_a(instructions: &[InputInstruction], expected_raw: &[u8]) {
    match instructions {
        [InputInstruction::KeyWithModifierEvent(key, raw, true)] => {
            assert!(key.is_key_without_modifier(BareKey::Char('a')));
            assert_eq!(raw, expected_raw);
        },
        other => panic!("expected Kitty a, got {other:?}"),
    }
}
