const ESC_BYTE: u8 = 0x1b;

pub const fn ctrl(character: char) -> [u8; 1] {
    [(character as u8) & 0x1f]
}

pub const fn alt(character: char) -> [u8; 2] {
    [ESC_BYTE, character as u8]
}

pub const fn key(character: char) -> [u8; 1] {
    [character as u8]
}

pub const ENTER: [u8; 1] = *b"\r";
pub const ESC: [u8; 1] = [ESC_BYTE];
pub const SPACE: [u8; 1] = *b" ";
pub const TAB: [u8; 1] = *b"\t";
/// Shift+Tab, which terminals spell CSI Z. There is no `BareKey::BackTab`: it reaches a plugin
/// as `Tab` with the Shift modifier, which is what `is_bar_key` and the agent list's ring step
/// both test for.
pub const SHIFT_TAB: [u8; 3] = *b"\x1b[Z";

pub const BRACKETED_PASTE_START: [u8; 6] = *b"\x1b[200~";
pub const BRACKETED_PASTE_END: [u8; 6] = *b"\x1b[201~";
