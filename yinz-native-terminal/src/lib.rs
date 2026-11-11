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
        let connection = session::connect(
            &args,
            &opts,
            yinz_types::pane_size::Size {
                rows: args.rows as usize,
                cols: args.cols as usize,
            },
        )?;
        session::headless(connection, &args)
    } else {
        window::run(args, opts)
    }
}

pub mod host_reply;
