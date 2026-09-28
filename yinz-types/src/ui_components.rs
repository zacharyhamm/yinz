//! The DCS sequence a plugin prints to have the host draw a UI component:
//! `ESC P y <component>;<params> ESC \`.

/// The final byte that marks a DCS sequence as a UI component. This is the fork's marker:
/// upstream Zellij uses `z`, so ported upstream serializers and parsers must use this instead.
pub const UI_COMPONENT_DCS_FINAL: char = 'y';

/// `ESC P` followed by [`UI_COMPONENT_DCS_FINAL`]: what every serialized UI component starts with.
pub const UI_COMPONENT_DCS_PREFIX: &str = "\u{1b}Py";

const _: () = assert!(
    UI_COMPONENT_DCS_PREFIX.len() == 3
        && UI_COMPONENT_DCS_PREFIX.as_bytes()[2] == UI_COMPONENT_DCS_FINAL as u8,
    "the prefix must end with the final byte"
);
