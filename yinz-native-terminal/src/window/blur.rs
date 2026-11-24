//! Blur behind the native window: what each windowing backend can do, what a
//! request amounts to against the window's opacity and transparency, and the
//! deduplicated diagnostics for it.
//!
//! winit's `set_blur` is a no-op on X11 and, on Wayland, binds only KDE's
//! `org_kde_kwin_blur_manager`, logging nothing Yinz can show when it is
//! missing. winit reports no capability, so Wayland support is detected here
//! with one registry query on winit's own display when the window opens.
//!
//! Every item except [`probe_wayland_interfaces`] is reached from the
//! cross-platform [`blur_support`], so a macOS build compiles and uses the
//! same items as Linux. The probe keeps its Linux-only imports and types in
//! its body.

use super::WindowBackend;
use yinz_runtime::errors::prelude::*;
use yinz_types::input::native_terminal::TerminalConfig;

/// KDE's Wayland blur protocol, the only one winit binds.
pub(crate) const KDE_BLUR_MANAGER: &str = "org_kde_kwin_blur_manager";
/// The staging background-effect protocol, which Yinz does not use yet.
pub(crate) const BACKGROUND_EFFECT_MANAGER: &str = "ext_background_effect_manager_v1";
/// What applies blur on macOS (winit's private window background blur radius).
pub(crate) const MACOS_BLUR: &str = "macOS window background blur";

/// Whether the window's backend can blur what shows through it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BlurSupport {
    /// Applied through the named protocol or API.
    Available(&'static str),
    Unsupported(BlurUnsupported),
    /// The capability could not be determined, so blur is requested anyway.
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlurUnsupported {
    /// X11, including Xwayland: winit has no X11 blur.
    X11,
    /// A Wayland compositor without [`KDE_BLUR_MANAGER`].
    Wayland { background_effect_offered: bool },
}

/// Classifies a Wayland compositor from its advertised global interfaces.
/// winit binds any version of [`KDE_BLUR_MANAGER`], so its presence suffices.
pub(crate) fn wayland_blur_support<'a>(
    interfaces: impl IntoIterator<Item = &'a str>,
) -> BlurSupport {
    let mut background_effect_offered = false;
    for interface in interfaces {
        if interface == KDE_BLUR_MANAGER {
            return BlurSupport::Available(KDE_BLUR_MANAGER);
        }
        background_effect_offered |= interface == BACKGROUND_EFFECT_MANAGER;
    }
    BlurSupport::Unsupported(BlurUnsupported::Wayland {
        background_effect_offered,
    })
}

/// The blur support of a window on `backend`. `wayland_interfaces` is the
/// result of [`probe_wayland_interfaces`] on Wayland and `None` when no probe
/// ran; a failed or missing probe means [`BlurSupport::Unknown`].
pub(crate) fn blur_support(
    backend: WindowBackend,
    wayland_interfaces: Option<Result<Vec<String>>>,
) -> BlurSupport {
    match backend {
        WindowBackend::X11 => BlurSupport::Unsupported(BlurUnsupported::X11),
        WindowBackend::Mac => BlurSupport::Available(MACOS_BLUR),
        WindowBackend::Wayland => match wayland_interfaces {
            Some(Ok(interfaces)) => wayland_blur_support(interfaces.iter().map(String::as_str)),
            Some(Err(error)) => {
                log::warn!(
                    "could not list the Wayland compositor's globals to detect terminal blur \
                     support, so blur is requested without knowing whether it applies: {error:#}"
                );
                BlurSupport::Unknown
            },
            None => BlurSupport::Unknown,
        },
    }
}

/// Lists the global interfaces the compositor advertises on `display`,
/// winit's live `wl_display`, with one roundtrip on a private event queue.
#[cfg(target_os = "linux")]
pub(crate) fn probe_wayland_interfaces(
    display: std::ptr::NonNull<std::ffi::c_void>,
) -> Result<Vec<String>> {
    use wayland_client::globals::{registry_queue_init, GlobalListContents};
    use wayland_client::protocol::wl_registry::{self, WlRegistry};
    use wayland_client::{Connection, Dispatch, QueueHandle};

    /// The initial roundtrip records globals itself; later registry events
    /// are never dispatched because the queue is dropped right away.
    struct Probe;
    impl Dispatch<WlRegistry, GlobalListContents> for Probe {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: wl_registry::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    // SAFETY: `display` comes from the live event loop's display handle and
    // is used synchronously before this function returns, while winit keeps
    // the display connected. A foreign-display backend creates its own event
    // queue and, when dropped, destroys only its own proxies and queue; it
    // never disconnects the display.
    let backend = unsafe {
        wayland_backend::sys::client::Backend::from_foreign_display(display.as_ptr().cast())
    };
    let connection = Connection::from_backend(backend);
    let (globals, _queue) =
        registry_queue_init::<Probe>(&connection).context("query the Wayland registry")?;
    Ok(globals
        .contents()
        .with_list(|list| list.iter().map(|global| global.interface.clone()).collect()))
}

/// What a blur request amounts to for the current window.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum BlurOutcome {
    Off,
    /// The backend cannot blur, whatever the opacity.
    Unsupported(BlurUnsupported),
    /// Opacity below 1 was requested, but the window draws opaque because the
    /// display offers no transparent surface, so blur shows nothing.
    HiddenByOpaqueFallback,
    /// Requested while the window is opaque (opacity 1); blur never makes the
    /// window see-through itself.
    NoVisibleEffect {
        protocol: Option<&'static str>,
    },
    /// Requested while the window is see-through.
    Requested {
        protocol: Option<&'static str>,
    },
}

/// Combines the request, the backend's support, the configured opacity and
/// whether see-through pixels can reach the screen. Unsupported wins
/// regardless of opacity and transparency availability.
pub(crate) fn blur_outcome(
    enabled: bool,
    support: &BlurSupport,
    configured_opacity: f32,
    transparency_available: bool,
) -> BlurOutcome {
    let protocol = match support {
        _ if !enabled => return BlurOutcome::Off,
        BlurSupport::Unsupported(reason) => return BlurOutcome::Unsupported(*reason),
        BlurSupport::Available(protocol) => Some(*protocol),
        BlurSupport::Unknown => None,
    };
    if configured_opacity >= 1.0 {
        BlurOutcome::NoVisibleEffect { protocol }
    } else if !transparency_available {
        BlurOutcome::HiddenByOpaqueFallback
    } else {
        BlurOutcome::Requested { protocol }
    }
}

impl BlurOutcome {
    /// The diagnostic for this outcome; `None` for [`BlurOutcome::Off`],
    /// which is recorded silently.
    pub(crate) fn message(&self) -> Option<(log::Level, String)> {
        let through = |protocol: &Option<&'static str>| match protocol {
            Some(protocol) => format!("through {protocol}"),
            None => "although the compositor's blur support is unknown".to_owned(),
        };
        Some(match self {
            Self::Off => return None,
            Self::Unsupported(BlurUnsupported::X11) => (
                log::Level::Warn,
                "terminal blur was requested, but the window uses the X11 backend (including \
                 Xwayland), where Yinz has no blur protocol, so nothing is blurred; run the native \
                 terminal under a Wayland compositor that offers org_kde_kwin_blur_manager (KDE \
                 Plasma), or set `blur false`"
                    .to_owned(),
            ),
            Self::Unsupported(BlurUnsupported::Wayland {
                background_effect_offered,
            }) => (
                log::Level::Warn,
                format!(
                    "terminal blur was requested, but the Wayland compositor does not offer \
                     org_kde_kwin_blur_manager, so nothing is blurred; use a compositor that \
                     offers it (KDE Plasma), or set `blur false`{}",
                    if *background_effect_offered {
                        " (the compositor offers ext_background_effect_manager_v1, which Yinz \
                         does not use yet)"
                    } else {
                        ""
                    }
                ),
            ),
            Self::HiddenByOpaqueFallback => (
                log::Level::Warn,
                "terminal blur was requested, but the window stays opaque because the display \
                 offers no transparent surface (see the terminal opacity warning), so blur has no \
                 visible effect; it applies once transparency is available, or set `blur false`"
                    .to_owned(),
            ),
            Self::NoVisibleEffect { protocol } => (
                log::Level::Info,
                format!(
                    "terminal blur is requested {}, but has no visible effect while terminal \
                     opacity is 1; lower opacity to see it",
                    through(protocol)
                ),
            ),
            Self::Requested { protocol } => (
                log::Level::Info,
                format!("terminal blur is requested {}", through(protocol)),
            ),
        })
    }
}

/// The window's blur support, the value last handed to the window and the
/// outcome last reported.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BlurState {
    pub(crate) support: BlurSupport,
    pub(crate) applied: Option<bool>,
    pub(crate) reported: Option<BlurOutcome>,
}

impl Default for BlurState {
    fn default() -> Self {
        Self {
            support: BlurSupport::Unknown,
            applied: None,
            reported: None,
        }
    }
}

impl BlurState {
    /// Brings the window in line with `settings`. `apply` sets blur on the
    /// window and is `None` while no window exists; then nothing is applied
    /// or recorded, so the window's creation applies and reports the setting.
    /// With a window, `apply` runs only when the request changed and the
    /// backend is not unsupported. The outcome is logged only when it differs
    /// from the last report; `Off` is recorded silently and re-arms the rest.
    /// Returns the logged diagnostic.
    pub(crate) fn sync(
        &mut self,
        settings: &TerminalConfig,
        transparency_available: bool,
        apply: Option<impl FnOnce(bool)>,
    ) -> Option<(log::Level, String)> {
        let apply = apply?;
        let enabled = settings.blur_enabled();
        if !matches!(self.support, BlurSupport::Unsupported(_)) && self.applied != Some(enabled) {
            apply(enabled);
            self.applied = Some(enabled);
        }
        let outcome = blur_outcome(
            enabled,
            &self.support,
            settings.opacity_value(),
            transparency_available,
        );
        if self.reported.as_ref() == Some(&outcome) {
            return None;
        }
        let message = outcome.message();
        self.reported = Some(outcome);
        let (level, text) = message?;
        log::log!(level, "{text}");
        Some((level, text))
    }
}
