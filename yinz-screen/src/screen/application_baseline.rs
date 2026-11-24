//! The server-owned color baseline shared applications are told about.
//!
//! Applications in shared panes ask the terminal for its colors (OSC 10/11/4) and
//! appearance (CSI ?996n, ?2031h notifications). Those answers come from one session
//! baseline, resolved only from session-wide configuration plus the session-wide
//! `SetDarkTheme` / `SetLightTheme` / `ToggleTheme` pins. A viewer's host reports, its
//! own theme options and theme previews never change it. When the baseline has a
//! terminal block, the rendered pane palette (`style.terminal`) follows it outside a
//! theme preview, so what an application is told matches what it is painted on.
//! Without one, panes render the viewer's host defaults while applications are told
//! the xterm fallback. See `docs/TERMINAL_COLORS.md`.

use super::Screen;
use crate::panes::PaneId;
use crate::pty_writer::PtyWriteInstruction;
use yinz_config::screen_config::ApplicationThemeConfig;
use yinz_runtime::errors::prelude::*;
use yinz_types::internal_ids::TerminalId;
use yinz_types::style::{
    detect_theme_hue, ApplicationBaseline, HostTerminalThemeMode, TerminalTheme,
};

/// Resolve the application baseline and the terminal block panes render with.
///
/// With a `theme_dark` / `theme_light` pair the mode is the session pin, then
/// `explicit_theme_hue`, then dark. Without a pair pins are ignored and the mode is
/// `explicit_theme_hue`, then the static theme's hue. The baseline colors are the
/// render block, or the documented xterm fallback for the mode when there is none.
/// The render block is `None` in that case, so panes keep host default colors and
/// may differ from the fallback applications are told.
pub(crate) fn resolve_application_baseline(
    config: &ApplicationThemeConfig,
    pin: Option<HostTerminalThemeMode>,
) -> (ApplicationBaseline, Option<TerminalTheme>) {
    let explicit = config.explicit_theme_hue.map(HostTerminalThemeMode::from);
    let (mode, render) = match config.pair {
        Some((dark, light)) => {
            let mode = pin.or(explicit).unwrap_or(HostTerminalThemeMode::Dark);
            let render = match mode {
                HostTerminalThemeMode::Dark => dark,
                HostTerminalThemeMode::Light => light,
            };
            (mode, render)
        },
        None => {
            let mode = explicit.unwrap_or_else(|| {
                let background = config
                    .static_terminal
                    .map(|terminal| yinz_types::colors::PaletteColor::Rgb(terminal.background))
                    .unwrap_or(config.static_background);
                detect_theme_hue(background).into()
            });
            (mode, config.static_terminal)
        },
    };
    let baseline = match render {
        Some(theme) => ApplicationBaseline { theme, mode },
        None => ApplicationBaseline::fallback(mode),
    };
    (baseline, render)
}

impl Screen {
    /// A session-wide theme change from the server: the only way configuration
    /// reaches the application baseline after startup.
    pub fn set_application_theme_config(&mut self, config: ApplicationThemeConfig) -> Result<()> {
        if config == self.core.application_theme {
            return Ok(());
        }
        // A changed explicit hue outranks an earlier manual pin, as it does for chrome.
        if config.explicit_theme_hue != self.core.application_theme.explicit_theme_hue {
            self.core.application_pin = None;
        }
        self.core.application_theme = config;
        self.refresh_application_baseline();
        self.render()
            .context("failed to render after an application theme change")
    }

    /// Recompute the baseline from the session theme and pin, push it to every pane,
    /// keep the rendered pane palette on it, and announce a changed mode once to
    /// applications that opted in with `CSI ? 2031 h`.
    pub(super) fn refresh_application_baseline(&mut self) {
        let (baseline, render) =
            resolve_application_baseline(&self.core.application_theme, self.core.application_pin);
        let previous_mode = self.core.style.application.mode;
        if self.core.style.application != baseline {
            self.core.style.application = baseline;
            for tab in self.core.tabs.values_mut() {
                tab.update_application_baseline(baseline);
            }
        }
        if let Some(preview) = self.palette.theme_preview.as_mut() {
            // The preview keeps rendering its own block; cancel and save return to this one.
            preview.original_terminal = render;
            let client = preview.client;
            self.refresh_theme_preview(client);
        } else if self.core.style.terminal != render {
            self.set_render_terminal(render);
        }
        if previous_mode != baseline.mode {
            self.announce_application_mode(baseline.mode);
        }
    }

    /// Render panes with `terminal`, without touching chrome.
    pub(super) fn set_render_terminal(&mut self, terminal: Option<TerminalTheme>) {
        self.core.style.terminal = terminal;
        for tab in self.core.tabs.values_mut() {
            tab.update_terminal_theme(terminal);
            tab.set_force_render();
        }
    }

    /// Pin the baseline mode (`SetDarkTheme` / `SetLightTheme` / `ToggleTheme`).
    pub(super) fn pin_application_mode(&mut self, mode: HostTerminalThemeMode) {
        self.core.application_pin = Some(mode);
        self.refresh_application_baseline();
    }

    /// Whether the session theme defines a dark/light pair, the gate for manual pins.
    pub(super) fn application_theme_has_pair(&self) -> bool {
        self.core.application_theme.pair.is_some()
    }

    fn announce_application_mode(&mut self, mode: HostTerminalThemeMode) {
        let mut pty_writes: Vec<(Vec<u8>, TerminalId)> = vec![];
        for tab in self.core.tabs.values_mut() {
            for pane_id in tab.get_all_pane_ids() {
                if let PaneId::Terminal(terminal_id) = pane_id
                    && let Some(pane) = tab.get_terminal_mut(pane_id)
                {
                    pane.push_color_palette_dsr(mode);
                    for bytes in pane.drain_messages_to_pty() {
                        pty_writes.push((bytes, terminal_id));
                    }
                }
            }
        }
        for (bytes, terminal_id) in pty_writes {
            let _ = self
                .core
                .bus
                .senders
                .send_to_pty_writer(PtyWriteInstruction::Write(bytes, terminal_id, None));
        }
    }

    /// The defensive answer for a color query that reached the screen: always the
    /// baseline, never a viewer's cached host colors.
    pub(super) fn application_color_reply(
        &self,
        query: &crate::host_query::HostQuery,
    ) -> Option<Vec<u8>> {
        use crate::host_query::HostQuery;
        let baseline = self.core.style.application;
        let body = |(r, g, b): (u8, u8, u8)| {
            let expand = |c: u8| (c as u16) * 0x0101;
            format!("rgb:{:04x}/{:04x}/{:04x}", expand(r), expand(g), expand(b))
        };
        let reply = match query {
            HostQuery::DefaultForeground { terminator } => {
                let mut out = format!("\x1b]10;{}", body(baseline.theme.foreground)).into_bytes();
                out.extend_from_slice(terminator.as_bytes());
                out
            },
            HostQuery::DefaultBackground { terminator } => {
                let mut out = format!("\x1b]11;{}", body(baseline.theme.background)).into_bytes();
                out.extend_from_slice(terminator.as_bytes());
                out
            },
            HostQuery::PaletteRegister { index, terminator } => {
                let mut out =
                    format!("\x1b]4;{index};{}", body(baseline.register(*index))).into_bytes();
                out.extend_from_slice(terminator.as_bytes());
                out
            },
            HostQuery::ColorPaletteMode => match baseline.mode {
                HostTerminalThemeMode::Dark => b"\x1b[?997;1n".to_vec(),
                HostTerminalThemeMode::Light => b"\x1b[?997;2n".to_vec(),
            },
            _ => return None,
        };
        Some(reply)
    }
}
