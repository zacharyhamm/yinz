use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::engine::Engine as _;
use yinz_ipc::ipc::ClientToServerMsg;
use yinz_types::input::actions::Action;

pub type ClipboardHandle = Arc<Mutex<Clipboard>>;

pub struct Clipboard {
    backend: Backend,
}

enum Backend {
    System(Box<arboard::Clipboard>),
    Memory { clipboard: String, primary: String },
}

impl Clipboard {
    pub fn open() -> Self {
        match arboard::Clipboard::new() {
            Ok(clipboard) => Self {
                backend: Backend::System(Box::new(clipboard)),
            },
            Err(e) => {
                report!(
                    "no system clipboard ({}); copy and paste stay inside this window",
                    e
                );
                Self::in_memory()
            },
        }
    }

    pub fn in_memory() -> Self {
        Self {
            backend: Backend::Memory {
                clipboard: String::new(),
                primary: String::new(),
            },
        }
    }

    pub fn set(&mut self, text: &str) {
        match &mut self.backend {
            Backend::System(clipboard) => {
                if let Err(e) = clipboard.set_text(text) {
                    report!("failed to write the clipboard: {}", e);
                }
            },
            Backend::Memory { clipboard, .. } => {
                clipboard.clear();
                clipboard.push_str(text);
            },
        }
    }

    pub fn get(&mut self) -> Option<String> {
        match &mut self.backend {
            Backend::System(clipboard) => reported(clipboard.get_text(), true),
            Backend::Memory { clipboard, .. } => (!clipboard.is_empty()).then(|| clipboard.clone()),
        }
    }

    pub fn get_primary(&mut self) -> Option<String> {
        match &mut self.backend {
            Backend::System(clipboard) => primary_text(clipboard),
            Backend::Memory { primary, .. } => (!primary.is_empty()).then(|| primary.clone()),
        }
    }

    pub fn set_primary(&mut self, text: &str) {
        match &mut self.backend {
            Backend::System(clipboard) => set_primary_text(clipboard, text),
            Backend::Memory { primary, .. } => {
                primary.clear();
                primary.push_str(text);
            },
        }
    }
}

#[cfg(freedesktop)]
fn primary_text(clipboard: &mut arboard::Clipboard) -> Option<String> {
    use arboard::{GetExtLinux, LinuxClipboardKind};
    reported(
        clipboard
            .get()
            .clipboard(LinuxClipboardKind::Primary)
            .text(),
        false,
    )
}

#[cfg(freedesktop)]
fn set_primary_text(clipboard: &mut arboard::Clipboard, text: &str) {
    use arboard::{LinuxClipboardKind, SetExtLinux};
    if let Err(e) = clipboard
        .set()
        .clipboard(LinuxClipboardKind::Primary)
        .text(text.to_owned())
    {
        report!("failed to write the primary selection: {}", e);
    }
}

#[cfg(not(freedesktop))]
fn set_primary_text(_clipboard: &mut arboard::Clipboard, _text: &str) {}

#[cfg(not(freedesktop))]
fn primary_text(_clipboard: &mut arboard::Clipboard) -> Option<String> {
    None
}

fn reported(read: Result<String, arboard::Error>, announce_empty: bool) -> Option<String> {
    match read {
        Ok(text) if !text.is_empty() => Some(text),
        Ok(_) => None,
        Err(arboard::Error::ContentNotAvailable) => {
            if announce_empty {
                report!("the clipboard holds no text; nothing to paste");
            }
            None
        },
        Err(e) => {
            report!("failed to read the clipboard: {}", e);
            None
        },
    }
}

pub fn osc52_reply(query_bytes: &[u8], clipboard: &ClipboardHandle) -> Option<Vec<u8>> {
    let (selection, terminator) = parse_osc52_query(query_bytes)?;
    let text = clipboard
        .lock()
        .map_err(|_| ())
        .ok()
        .and_then(|mut clipboard| clipboard.get())
        .unwrap_or_default();

    let mut reply = format!("\u{1b}]52;{};{}", selection, BASE64.encode(text)).into_bytes();
    reply.extend_from_slice(terminator);
    Some(reply)
}

fn parse_osc52_query(bytes: &[u8]) -> Option<(char, &[u8])> {
    let rest = bytes.strip_prefix(b"\x1b]52;")?;
    let (selection, rest) = rest.split_first()?;
    let rest = rest.strip_prefix(b";")?;
    let rest = rest.strip_prefix(b"?")?;
    match rest {
        b"\x07" | b"\x1b\\" => Some((*selection as char, rest)),
        _ => None,
    }
}

pub fn paste_message(chars: String) -> ClientToServerMsg {
    paste_message_into(chars, None)
}

pub fn paste_message_into(
    chars: String,
    pane_id: Option<yinz_types::data::PaneId>,
) -> ClientToServerMsg {
    ClientToServerMsg::Action {
        action: Action::Paste { chars, pane_id },
        terminal_id: None,
        client_id: None,
        is_cli_client: false,
    }
}
