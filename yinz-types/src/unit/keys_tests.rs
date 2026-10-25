use super::*;
use hegel::generators::{self, Generator};

/// Any `BareKey`, including `F` indices and characters outside the ones the
/// parser knows about.
///
/// Two characters are excluded from the `Char` arm, because the string form genuinely
/// cannot represent them rather than because of a defect:
/// * the arrow glyphs `←↓↑→`, which `Display` also uses for `Left`/`Down`/`Up`/`Right`,
///   so `Char('←')` and `Left` share a spelling and only one of them can win.
/// * ASCII whitespace, which `KeyWithModifier::from_str` uses as its modifier
///   delimiter (`split_ascii_whitespace`); `BareKey::Tab` is how you spell a tab.
#[hegel::composite]
fn bare_keys(tc: &hegel::TestCase) -> BareKey {
    tc.draw(hegel::one_of!(
        generators::sampled_from(vec![
            BareKey::PageDown,
            BareKey::PageUp,
            BareKey::Left,
            BareKey::Down,
            BareKey::Up,
            BareKey::Right,
            BareKey::Home,
            BareKey::End,
            BareKey::Backspace,
            BareKey::Delete,
            BareKey::Insert,
            BareKey::Tab,
            BareKey::Esc,
            BareKey::Enter,
            BareKey::CapsLock,
            BareKey::ScrollLock,
            BareKey::NumLock,
            BareKey::PrintScreen,
            BareKey::Pause,
            BareKey::Menu,
        ]),
        generators::integers::<u8>().map(BareKey::F),
        generators::text()
            .min_size(1)
            .map(|s| s.chars().next().unwrap())
            .filter(|c| !"←↓↑→".contains(*c) && !c.is_ascii_whitespace())
            .map(BareKey::Char),
    ))
}

#[hegel::composite]
fn keys_with_modifiers(tc: &hegel::TestCase) -> KeyWithModifier {
    let bare_key = tc.draw(bare_keys());
    let key_modifiers: BTreeSet<KeyModifier> = tc
        .draw(generators::vecs(generators::sampled_from(vec![
            KeyModifier::Ctrl,
            KeyModifier::Alt,
            KeyModifier::Shift,
            KeyModifier::Super,
        ])))
        .into_iter()
        .collect();
    KeyWithModifier {
        bare_key,
        key_modifiers,
    }
}

#[hegel::test]
fn key_with_modifier_roundtrips_through_its_string_form(tc: hegel::TestCase) {
    let key = tc.draw(keys_with_modifiers());
    let stringified = key.to_string();
    let parsed = KeyWithModifier::from_str(&stringified);
    // NOTE: `KeyWithModifier`'s `PartialEq` case-folds, so compare the fields.
    match parsed {
        Ok(parsed) => {
            assert_eq!(
                parsed.bare_key, key.bare_key,
                "{:?} stringified to {:?} and parsed back as {:?}",
                key, stringified, parsed
            );
            assert_eq!(
                parsed.key_modifiers, key.key_modifiers,
                "{:?} stringified to {:?} and parsed back as {:?}",
                key, stringified, parsed
            );
        },
        Err(e) => panic!(
            "{:?} stringified to {:?} which does not parse: {}",
            key, stringified, e
        ),
    }
}
