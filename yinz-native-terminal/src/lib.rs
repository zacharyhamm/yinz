//! Native window client, adapted from the Zellij terminal renderer.
#[macro_use]
mod diagnostics {
    macro_rules! report {
        ($($arg:tt)*) => { log::warn!($($arg)*) };
    }
}
pub mod emulation;
pub mod input {
    pub mod clipboard;
    pub mod composition;
    pub mod keyboard;
    pub mod links;
}
pub mod render;
pub mod text;
pub mod platform {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Platform {
        Linux,
        MacOs,
        Windows,
    }
    impl Platform {
        pub const fn current() -> Self {
            if cfg!(target_os = "macos") {
                Self::MacOs
            } else {
                Self::Linux
            }
        }
    }
}
#[cfg(test)]
mod testing {
    pub mod painter;
}

mod session;
mod window;
pub fn run(
    args: yinz_cli::args::TerminalArgs,
    opts: yinz_cli::args::CliArgs,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        opts.remote.is_none(),
        "native terminal windows currently support local sessions; omit --remote"
    );
    if args.headless {
        let connection = session::connect(&args, &opts, args.opening_size())?;
        session::headless(connection, &args)
    } else {
        window::run(args, opts, None)
    }
}

pub mod host_reply;

/// Attach over an established Yinz forward. The transport owner supplies the stream and its origin.
pub fn run_forwarded(
    args: yinz_cli::args::TerminalArgs,
    opts: yinz_cli::args::CliArgs,
    stream: interprocess::local_socket::Stream,
    host: Option<String>,
    name: String,
    focus: &yinz_types::data::SessionConnection,
) -> anyhow::Result<()> {
    let connection = session::forwarded(stream, host, name, &args, focus)?;
    if args.headless {
        session::headless(connection, &args)
    } else {
        window::run(args, opts, Some(connection))
    }
}

#[cfg(feature = "attachment-test-support")]
pub use window::live_window::exercise_live_switch;
