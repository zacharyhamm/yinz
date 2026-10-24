//! OSC title, color, clipboard, hyperlink, and notification commands.

use super::{
    osc_color_reply_body, parse_osc7_path, rgb_of_ansi_code, Grid, Osc133MarkerKind, BASE64_DECODER,
};
use crate::panes::alacritty_functions::parse_number;
use crate::panes::alacritty_functions::xparse_color;
use crate::panes::terminal_character::CursorShape;
use std::rc::Rc;
use std::str;
use yinz_host_protocol::notifications::parse_osc99_metadata;
use yinz_host_protocol::notifications::split_osc99_payload;
use yinz_host_protocol::notifications::Osc99PayloadType;
use yinz_host_protocol::notifications::PendingNotification;

use base64::engine::Engine as _;

impl Grid {
    pub(super) fn dispatch_osc(&mut self, params: &[&[u8]], bell_terminated: bool) {
        let terminator = if bell_terminated { "\x07" } else { "\x1b\\" };

        if params.is_empty() || params[0].is_empty() {
            return;
        }

        match params[0] {
            // Set window title.
            b"0" | b"2" => {
                if params.len() >= 2 {
                    let title = params[1..]
                        .iter()
                        .flat_map(|x| str::from_utf8(x))
                        .collect::<Vec<&str>>()
                        .join(";")
                        .trim()
                        .to_owned();
                    self.set_title(title);
                }
            },

            b"7" => {
                if let Some(raw) = params.get(1)
                    && let Some(path) = parse_osc7_path(raw)
                {
                    self.pending_osc7_cwd = Some(path);
                }
                // Keep the original URI for the host, including semicolons and percent encoding.
                if params.len() >= 2 {
                    let segments: Option<Vec<&str>> =
                        params[1..].iter().map(|x| str::from_utf8(x).ok()).collect();
                    if let Some(segments) = segments {
                        let uri = segments.join(";");
                        if !uri.is_empty() && !uri.chars().any(|c| c.is_control()) {
                            self.osc7_payload = Some(uri);
                        }
                    }
                }
            },

            // Set color index.
            b"4" => {
                for chunk in params[1..].chunks(2) {
                    let index = chunk.first().and_then(|index| parse_number(index));
                    let color = chunk.get(1).and_then(|color| xparse_color(color));
                    if let (Some(i), Some(c)) = (index, color) {
                        if self.changed_colors.is_none() {
                            self.changed_colors = Some([None; 256]);
                        }
                        self.changed_colors.as_mut().unwrap()[i as usize] = Some(c);
                        self.output_buffer.update_all_lines();
                    } else if chunk.get(1).as_ref().and_then(|c| c.first()) == Some(&b'?')
                        && let Some(index) = index
                    {
                        if let Some(rgb) = self
                            .effective_colors()
                            .and_then(|colors| colors[index as usize])
                            .and_then(rgb_of_ansi_code)
                        {
                            self.pending_messages_to_pty.push(
                                format!("\x1b]4;{index};{}{terminator}", osc_color_reply_body(rgb))
                                    .into_bytes(),
                            );
                            continue;
                        }
                        self.pending_forwarded_queries.push(
                            crate::host_query::HostQuery::PaletteRegister {
                                index,
                                terminator: crate::host_query::OscTerminator::from_bell_terminated(
                                    bell_terminated,
                                ),
                            },
                        );
                    }
                }
            },

            // define hyperlink
            b"8" => self.cursor.pending_styles.update(|styles| {
                styles.link_anchor = self.link_handler.borrow_mut().dispatch_osc8(params)
            }),

            // Get/set Foreground (b"10") or background (b"11") colors
            b"10" | b"11" => {
                if params.len() >= 2
                    && let Some(mut dynamic_code) = parse_number(params[0])
                {
                    for param in &params[1..] {
                        if param == b"?" {
                            // If this pane has a local override for
                            // the channel being queried (set via
                            // `yinz action set-pane-color` or via
                            // a prior OSC 10;<rgb> / 11;<rgb> from
                            // inside the pane), answer with that
                            // override directly instead of
                            // forwarding to the host. Apps inside
                            // the pane must see the colors Yinz
                            // is actually rendering for them, not
                            // the host terminal's background.
                            let local_override = match dynamic_code {
                                10 => self.effective_foreground(),
                                11 => self.effective_background(),
                                _ => None,
                            };
                            if let Some(rgb) = local_override {
                                self.observe_agent_terminal_color(dynamic_code, rgb);
                                let reply = format!(
                                    "\u{1b}]{};{}{}",
                                    dynamic_code,
                                    osc_color_reply_body(rgb),
                                    terminator
                                );
                                self.pending_messages_to_pty.push(reply.as_bytes().to_vec());
                            } else {
                                // No local override — forward to
                                // the host so the app observes the
                                // terminal's actual color. Yinz's
                                // cached copy is refreshed via the
                                // double-dispatch on the reply.
                                let term = crate::host_query::OscTerminator::from_bell_terminated(
                                    bell_terminated,
                                );
                                let query = match dynamic_code {
                                    10 => crate::host_query::HostQuery::DefaultForeground {
                                        terminator: term,
                                    },
                                    11 => crate::host_query::HostQuery::DefaultBackground {
                                        terminator: term,
                                    },
                                    _ => {
                                        // Out-of-range dynamic_code
                                        // (shouldn't happen since
                                        // the outer match pins it to
                                        // 10 or 11): skip.
                                        dynamic_code += 1;
                                        continue;
                                    },
                                };
                                self.agent_color_queries
                                    .push_back((dynamic_code, Rc::downgrade(&self.agent_theme)));
                                self.pending_forwarded_queries.push(query);
                            }
                        } else {
                            // Set: parse color and store as pane
                            // default. Only literal RGB is stored;
                            // palette-indexed / named variants (or
                            // a parse failure) are silently dropped
                            // to keep the pane-default fields
                            // narrow.
                            if let Some(rgb) = xparse_color(param).and_then(rgb_of_ansi_code) {
                                if dynamic_code == 10 {
                                    self.application_fg = Some(rgb);
                                } else if dynamic_code == 11 {
                                    self.application_bg = Some(rgb);
                                }
                                self.output_buffer.update_all_lines();
                            }
                        }
                        dynamic_code += 1;
                    }
                }
            },

            b"12" => {
                // get/set cursor color currently unimplemented
            },

            b"133" => {
                let marker = params.get(1).and_then(|subcommand| match *subcommand {
                    b"A" | b"P" => Some(Osc133MarkerKind::Prompt),
                    b"B" | b"I" => Some(Osc133MarkerKind::Input),
                    b"C" => Some(Osc133MarkerKind::Output),
                    b"D" => Some(Osc133MarkerKind::End),
                    _ => None,
                });
                if let (Some(marker), Some(row)) = (marker, self.viewport.get_mut(self.cursor.y)) {
                    row.add_osc133_marker(self.cursor.x, marker);
                    self.osc133_markers_seen = true;
                }
            },

            // Set cursor style.
            b"50" => {
                if params.len() >= 2
                    && params[1].len() >= 13
                    && params[1][0..12] == *b"CursorShape="
                {
                    let shape = match params[1][12] as char {
                        '0' => Some(CursorShape::Block),
                        '1' => Some(CursorShape::Beam),
                        '2' => Some(CursorShape::Underline),
                        _ => None,
                    };
                    if let Some(cursor_shape) = shape {
                        self.cursor.change_shape(cursor_shape);
                    }
                }
            },

            // Set clipboard.
            b"52" => {
                if params.len() < 3 {
                    return;
                }

                let clipboard = *params[1].first().unwrap_or(&b'c');
                match params[2] {
                    b"?" => {
                        self.pending_forwarded_queries.push(
                            crate::host_query::HostQuery::ClipboardContent {
                                selection: clipboard as char,
                                terminator: crate::host_query::OscTerminator::from_bell_terminated(
                                    bell_terminated,
                                ),
                            },
                        );
                    },
                    base64 => {
                        if let Ok(bytes) = BASE64_DECODER.decode(base64)
                            && let Ok(string) = String::from_utf8(bytes)
                        {
                            self.pending_clipboard_update = Some(string);
                        };
                    },
                }
            },

            // Reset color index.
            b"104" => {
                // Reset all color indexes when no parameters are given.
                if params.len() == 1 {
                    self.changed_colors = None;
                    self.output_buffer.update_all_lines();
                    return;
                }

                // Reset color indexes given as parameters.
                for param in &params[1..] {
                    if let Some(index) = parse_number(param) {
                        if let Some(changed_colors) = &mut self.changed_colors {
                            changed_colors[index as usize] = None;
                        }
                        self.output_buffer.update_all_lines();
                    }
                }

                // Reset all color indexes when no parameters are given.
                if params.len() == 1 {
                    // TBD - reset all color changes - currently unsupported
                    return;
                }

                // Reset color indexes given as parameters.
                for param in &params[1..] {
                    if let Some(_index) = parse_number(param) {
                        // TBD - reset color index - currently unimplemented
                    }
                }
            },

            // Reset foreground color.
            b"110" => {
                self.application_fg = None;
                self.output_buffer.update_all_lines();
            },

            // Reset background color.
            b"111" => {
                self.application_bg = None;
                self.output_buffer.update_all_lines();
            },

            // Reset text cursor color.
            b"112" => {
                // TBD - reset text cursor color - currently unimplemented
            },

            b"99" => {
                if params.len() > 1 {
                    let payload = params
                        .get(1..)
                        .unwrap_or_default()
                        .iter()
                        .flat_map(|x| str::from_utf8(x))
                        .collect::<Vec<&str>>()
                        .join(";");
                    if !payload.is_empty() {
                        let (metadata, rest) = split_osc99_payload(&payload);
                        let metadata = parse_osc99_metadata(metadata);
                        let id = metadata.get("i").copied().unwrap_or_default();
                        let payload_type = metadata
                            .get("p")
                            .map(|value| Osc99PayloadType::from_metadata_value(value))
                            .unwrap_or(Osc99PayloadType::Title);
                        let is_done = metadata.get("d").copied().unwrap_or("1") != "0";
                        let decoded = if metadata.get("e").copied() == Some("1") {
                            BASE64_DECODER
                                .decode(rest.as_bytes())
                                .map(|decoded| String::from_utf8_lossy(&decoded).into_owned())
                                .unwrap_or_else(|_| rest.to_owned())
                        } else {
                            rest.to_owned()
                        };
                        let wants_report = self
                            .notification_tracker
                            .wants_report(id, metadata.get("a").copied());
                        let display =
                            self.notification_tracker
                                .assemble(id, payload_type, is_done, decoded);
                        self.pending_desktop_notifications
                            .push(PendingNotification::Osc99 {
                                payload,
                                terminator: terminator.to_string(),
                                wants_report,
                                display,
                            });
                    }
                }
            },

            b"9" => {
                if params.len() > 1 {
                    // ConEmu reuses OSC 9 for its own subcommands. Only the two that other
                    // terminals emit and honour are recognised - `9;4;<state>;<pct>` progress
                    // and `9;9;<cwd>` - so a notification whose body happens to start with a
                    // small number (`9;3;builds remaining`) is still delivered.
                    let is_conemu_subcommand = params.len() > 2
                        && match params.get(1).copied() {
                            Some(b"4") => params
                                .get(2)
                                .and_then(|state| str::from_utf8(state).ok())
                                .map(|state| state.trim().parse::<u8>().is_ok())
                                .unwrap_or(false),
                            Some(b"9") => true,
                            _ => false,
                        };
                    let body = params
                        .get(1..)
                        .unwrap_or_default()
                        .iter()
                        .flat_map(|x| str::from_utf8(x))
                        .collect::<Vec<&str>>()
                        .join(";");
                    if !body.is_empty() && !is_conemu_subcommand {
                        self.pending_desktop_notifications
                            .push(PendingNotification::Osc9 { body });
                    }
                }
            },

            b"777" if params.get(1) == Some(&b"yinz-agent-history".as_slice()) => {
                use yinz_types::agent::theme::{AgentThemeHandoff, AgentThemeLaunch};
                match params.get(2).copied() {
                    Some(b"begin") => {
                        self.history_replay = None;
                        if let Some(encoded) = params.get(3).filter(|v| v.len() <= 8 * 1024 * 1024)
                            && let Ok(bytes) = BASE64_DECODER.decode(encoded)
                            && let Ok(metadata) =
                                serde_json::from_slice::<AgentThemeHandoff>(&bytes)
                            && metadata.valid("")
                        {
                            let palettes: Vec<_> = metadata
                                .snapshots
                                .iter()
                                .map(|s| {
                                    Rc::new(crate::agent_theme::SourcePalette::new(
                                        Some(s.clone()),
                                        self.style,
                                    ))
                                })
                                .collect();
                            self.agent_palettes
                                .extend(palettes.iter().map(Rc::downgrade));
                            self.begin_agent_run();
                            self.history_replay = Some((metadata, palettes));
                        }
                    },
                    Some(b"select") => {
                        if let Some((_, palettes)) = &self.history_replay {
                            if params.get(3) == Some(&b"none".as_slice()) {
                                self.install_agent_palette(None);
                            } else if let Some(palette) = params
                                .get(3)
                                .and_then(|v| std::str::from_utf8(v).ok())
                                .and_then(|v| v.parse::<usize>().ok())
                                .and_then(|i| palettes.get(i))
                            {
                                self.agent_theme = palette.clone();
                            }
                        }
                    },
                    Some(b"end") => {
                        if let Some((metadata, palettes)) = self.history_replay.take() {
                            self.begin_agent_run();
                            if let Some(active) = metadata.active {
                                self.agent_theme = palettes[active.palette].clone();
                                self.agent_launch = Some(AgentThemeLaunch {
                                    launch_id: active.launch_id,
                                    snapshot: metadata.snapshots[active.palette].clone(),
                                });
                            }
                        }
                    },
                    _ => {},
                }
            },
            b"777" if params.get(1) == Some(&b"yinz-agent-theme".as_slice()) => {
                if let Some(encoded) = params
                    .get(2)
                    .filter(|value| value.len() <= yinz_types::agent::theme::MAX_SNAPSHOT_BYTES * 2)
                {
                    use base64::Engine;
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded)
                        && bytes.len() <= yinz_types::agent::theme::MAX_SNAPSHOT_BYTES
                        && let Ok(event) = serde_json::from_slice(&bytes)
                    {
                        self.apply_agent_theme_event(event);
                    }
                }
            },
            b"777"
                if !bell_terminated
                    && params == [b"777".as_slice(), b"yinz", b"implicit-blank"] =>
            {
                if let Some(x) = self.cursor.x.checked_sub(1)
                    && let Some(row) = self.viewport.get_mut(self.cursor.y)
                {
                    row.writing(&self.agent_theme);
                    let index = row.absolute_character_index(x);
                    if let Some(character) = row.columns.get_mut(index) {
                        character.mark_implicit_blank();
                    }
                }
            },

            b"777" => {
                let parts: Vec<&str> = params
                    .get(1..)
                    .unwrap_or_default()
                    .iter()
                    .flat_map(|x| str::from_utf8(x))
                    .collect();
                if parts.first().copied() == Some("notify") {
                    let title = parts.get(1).map(|s| s.to_string()).unwrap_or_default();
                    let body = parts
                        .get(2..)
                        .map(|rest| rest.join(";"))
                        .unwrap_or_default();
                    if !title.is_empty() || !body.is_empty() {
                        self.pending_desktop_notifications
                            .push(PendingNotification::Osc777 { title, body });
                    }
                }
            },

            _ => {
                if self.debug {
                    log::warn!("Unhandled osc: {:?}", params);
                }
            },
        }
    }
}
