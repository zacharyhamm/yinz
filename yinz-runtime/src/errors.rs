//! Error context system based on a thread-local representation of the call stack, itself based on
//! the instructions that are sent between threads.
//!
//! # Help wanted
//!
//! There is an ongoing endeavor to improve the state of error handling in yinz. Currently, many
//! functions rely on [`unwrap`]ing [`Result`]s rather than returning and hence propagating
//! potential errors. New code returns them with context instead; see the Conventions section of
//! `AGENTS.md`. Upstream Zellij, which this system comes from, describes it further in [its docs
//! about error handling](https://github.com/zellij-org/zellij/tree/main/docs/ERROR_HANDLING.md).

use anyhow::Context;
use colored::*;
use log::error;

/// Re-exports of common error-handling code.
pub mod prelude {
    pub use super::FatalError;
    pub use super::LoggableError;
    pub use super::ToAnyhow;
    pub use super::YinzError;
    pub use anyhow::anyhow;
    pub use anyhow::bail;
    pub use anyhow::Context;
    pub use anyhow::Error as anyError;
    pub use anyhow::Result;
}

pub trait ErrorInstruction {
    fn error(err: String) -> Self;
}

/// Helper trait to easily log error types.
///
/// The `print_error` function takes a closure which takes a `&str` and fares with it as necessary
/// to log the error to some usable location. For convenience, logging to stdout, stderr and
/// `log::error!` is already implemented.
///
/// Note that the trait functions pass the error through unmodified, so they can be chained with
/// the usual handling of [`std::result::Result`] types.
pub trait LoggableError<T>: Sized {
    /// Gives a formatted error message derived from `self` to the closure `fun` for
    /// printing/logging as appropriate.
    ///
    /// # Examples
    ///
    /// ```should_panic
    /// use anyhow;
    /// use yinz_runtime::errors::LoggableError;
    ///
    /// let my_err: anyhow::Result<&str> = Err(anyhow::anyhow!("Test error"));
    /// my_err
    ///     .print_error(|msg| println!("{msg}"))
    ///     .unwrap();
    /// ```
    #[track_caller]
    fn print_error<F: Fn(&str)>(self, fun: F) -> Self;

    /// Convenienve function, calls `print_error` and logs the result as error.
    ///
    /// This is not a wrapper around `log::error!`, because the `log` crate uses a lot of compile
    /// time macros from `std` to determine caller locations/module names etc. Since these are
    /// resolved at compile time in the location they are written, they would always resolve to the
    /// location in this function where `log::error!` is called, masking the real caller location.
    /// Hence, we build the log message ourselves. This means that we lose the information about
    /// the calling module (Because it can only be resolved at compile time), however the callers
    /// file and line number are preserved.
    #[track_caller]
    fn to_log(self) -> Self {
        let caller = std::panic::Location::caller();
        self.print_error(|msg| {
            // Build the log entry manually
            // NOTE: The log entry has no module path associated with it. This is because `log`
            // gets the module path from the `std::module_path!()` macro, which is replaced at
            // compile time in the location it is written!
            log::logger().log(
                &log::Record::builder()
                    .level(log::Level::Error)
                    .args(format_args!("{}", msg))
                    .file(Some(caller.file()))
                    .line(Some(caller.line()))
                    .module_path(None)
                    .build(),
            );
        })
    }

    /// Convenienve function, calls `print_error` with the closure `|msg| eprintln!("{}", msg)`.
    fn to_stderr(self) -> Self {
        self.print_error(|msg| eprintln!("{}", msg))
    }

    /// Convenienve function, calls `print_error` with the closure `|msg| println!("{}", msg)`.
    fn to_stdout(self) -> Self {
        self.print_error(|msg| println!("{}", msg))
    }
}

impl<T> LoggableError<T> for anyhow::Result<T> {
    fn print_error<F: Fn(&str)>(self, fun: F) -> Self {
        if let Err(ref err) = self {
            fun(&format!("{:?}", err));
        }
        self
    }
}

/// Special trait to mark fatal/non-fatal errors.
///
/// This works in tandem with `LoggableError` above and is meant to make reading code easier with
/// regard to whether an error is fatal or not (i.e. can be ignored, or at least doesn't make the
/// application crash).
///
/// This essentially degrades any `std::result::Result<(), _>` to a simple `()`.
pub trait FatalError<T> {
    /// Mark results as being non-fatal.
    ///
    /// If the result is an `Err` variant, this will [print the error to the log][`to_log`].
    /// Discards the result type afterwards.
    ///
    /// [`to_log`]: LoggableError::to_log
    #[track_caller]
    fn non_fatal(self);

    /// Mark results as being fatal.
    ///
    /// If the result is an `Err` variant, this will unwrap the error and panic the application.
    /// If the result is an `Ok` variant, the inner value is unwrapped and returned instead.
    ///
    /// # Panics
    ///
    /// If the given result is an `Err` variant.
    #[track_caller]
    fn fatal(self) -> T;
}

/// Helper function to silence `#[warn(unused_must_use)]` cargo warnings. Used exclusively in
/// `FatalError::non_fatal`!
fn discard_result<T>(_arg: anyhow::Result<T>) {}

impl<T> FatalError<T> for anyhow::Result<T> {
    fn non_fatal(self) {
        if self.is_err() {
            discard_result(self.context("a non-fatal error occurred").to_log());
        }
    }

    fn fatal(self) -> T {
        if let Ok(val) = self {
            val
        } else {
            self.context("a fatal error occurred")
                .expect("Program terminates")
        }
    }
}

use yinz_types::diagnostics::{ContextType, ErrorContext};

use thiserror::Error;
#[derive(Debug, Error)]
pub enum YinzError {
    #[error("could not find command '{command}' for terminal {terminal_id}")]
    CommandNotFound {
        terminal_id: yinz_types::internal_ids::TerminalId,
        command: String,
    },

    /// A pane that could not be started, for a reason the user should read
    /// rather than a reason that should end the session.
    ///
    /// Carries the reserved terminal id so the failure can be shown *inside* the
    /// pane it was meant to be, the way a missing command already is. Opening a
    /// pane on another machine fails in a dozen ways no local pane can — an ssh
    /// that will not connect, a broker that is not there — and none of them are
    /// worth taking every other pane in the session down with.
    #[error("could not start a pane in terminal {terminal_id}: {message}")]
    PaneStartFailed {
        terminal_id: yinz_types::internal_ids::TerminalId,
        message: String,
    },

    #[error("could not determine default editor")]
    NoEditorFound,

    #[error("failed to allocate another terminal id")]
    NoMoreTerminalIds,

    #[error("failed to start PTY")]
    FailedToStartPty,

    #[error("Cannot resize fixed panes")]
    CantResizeFixedPanes {
        pane_ids: Vec<yinz_types::pane_id::PaneId>,
    },
    #[error("Pane size remains unchanged")]
    PaneSizeUnchanged,
    /// The solved layout was refused before anything moved: a pane stack came out shorter than
    /// the number of panes in it. Distinct from `PaneSizeUnchanged`, which is a valid layout
    /// that happened to round to the same geometry.
    #[error("Pane layout rejected: a pane stack does not fit its minimum height")]
    PaneLayoutRejected,

    #[error("an error occurred")]
    GenericError { source: anyhow::Error },

    #[error("Client {client_id} is too slow to handle incoming messages")]
    ClientTooSlow {
        client_id: yinz_types::internal_ids::ClientId,
    },

    #[error("The plugin does not exist")]
    PluginDoesNotExist,

    #[error("Ran out of room for spans")]
    RanOutOfRoomForSpans,
}

pub use panic_context::*;

mod panic_context {
    use super::*;
    use crate::channels::{SenderWithContext, ASYNCOPENCALLS, OPENCALLS};
    use miette::{Diagnostic, GraphicalReportHandler, GraphicalTheme, Report};
    use std::panic::PanicHookInfo;
    use thiserror::Error as ThisError;

    #[derive(Debug, ThisError, Diagnostic)]
    #[error("{0}{backtrace}", backtrace = self.show_backtrace())]
    #[diagnostic(help("{}", self.show_help()))]
    struct Panic(String);

    impl Panic {
        // We already capture a backtrace with `anyhow` using the `backtrace` crate in the background.
        // The advantage is that this is the backtrace of the real errors source (i.e. where we first
        // encountered the error and turned it into an `anyhow::Error`), whereas the backtrace recorded
        // here is the backtrace leading to the call to any `panic`ing function. Since now we propagate
        // errors up before `unwrap`ing them (e.g. in `yinz_screen::screen_thread_main`), the
        // former is what we really want to diagnose.
        // We still keep the second one around just in case the first backtrace isn't meaningful or
        // non-existent in the first place (Which really shouldn't happen, but you never know).
        fn show_backtrace(&self) -> String {
            if let Ok(var) = std::env::var("RUST_BACKTRACE")
                && !var.is_empty()
                && var != "0"
            {
                return format!("\n\nPanic backtrace:\n{:?}", backtrace::Backtrace::new());
            }
            "".into()
        }

        fn show_help(&self) -> String {
            format!(
                "If you are seeing this message, it means that something went wrong.

-> To get additional information, check the log at: {}
-> To see a backtrace next time, reproduce the error with: RUST_BACKTRACE=1 yinz [...]
-> To help us fix this, please open an issue: https://github.com/zacharyhamm/yinz/issues

",
                crate::consts::YINZ_TMP_LOG_FILE.display()
            )
        }
    }

    /// Custom panic handler/hook. Prints the [`ErrorContext`].
    pub fn handle_panic<T>(info: &PanicHookInfo<'_>, sender: Option<&SenderWithContext<T>>)
    where
        T: ErrorInstruction + Clone,
    {
        use std::{process, thread};
        let thread = thread::current();
        let thread = thread.name().unwrap_or("unnamed");

        let msg = match info.payload().downcast_ref::<&'static str>() {
            Some(s) => Some(*s),
            None => info.payload().downcast_ref::<String>().map(|s| &**s),
        }
        .unwrap_or("An unexpected error occurred!");

        let err_ctx = OPENCALLS.with(|ctx| *ctx.borrow());

        let mut report: Report = Panic(format!("\u{1b}[0;31m{}\u{1b}[0;0m", msg)).into();

        let mut location_string = String::new();
        if let Some(location) = info.location() {
            location_string = format!(
                "At {}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            );
            report = report.wrap_err(location_string.clone());
        }

        if !err_ctx.is_empty() {
            report = report.wrap_err(format_context(&err_ctx));
        }

        report = report.wrap_err(format!(
            "Thread '\u{1b}[0;31m{}\u{1b}[0;0m' panicked.",
            thread
        ));

        error!(
            "Panic occurred:
             thread: {}
             location: {}
             message: {}",
            thread, location_string, msg
        );

        if thread == "main" || sender.is_none() {
            // here we only show the first line because the backtrace is not readable otherwise
            // a better solution would be to escape raw mode before we do this, but it's not trivial
            // to get os_input here
            println!("\u{1b}[2J{}", fmt_report(report));
            process::exit(1);
        } else if let Some(sender) = sender {
            let _ = sender.send(T::error(fmt_report(report)));
        }
    }

    pub fn get_current_ctx() -> ErrorContext {
        ASYNCOPENCALLS
            .try_with(|ctx| *ctx.borrow())
            .unwrap_or_else(|_| OPENCALLS.with(|ctx| *ctx.borrow()))
    }

    fn fmt_report(diag: Report) -> String {
        let mut out = String::new();
        GraphicalReportHandler::new_themed(GraphicalTheme::unicode())
            .render_report(&mut out, diag.as_ref())
            .unwrap();
        out
    }

    /// Publish a received or extended context to the current task or thread.
    pub fn update_thread_ctx(context: &ErrorContext) {
        ASYNCOPENCALLS
            .try_with(|ctx| *ctx.borrow_mut() = *context)
            .unwrap_or_else(|_| OPENCALLS.with(|ctx| *ctx.borrow_mut() = *context));
    }

    pub fn add_call(context: &mut ErrorContext, call: ContextType) {
        context.add_call(call);
        update_thread_ctx(context);
    }

    /// Helper trait to convert error types that don't satisfy `anyhow`s trait requirements to
    /// anyhow errors.
    pub trait ToAnyhow<U> {
        fn to_anyhow(self) -> anyhow::Result<U>;
    }

    /// `SendError` doesn't satisfy `anyhow`s trait requirements due to `T` possibly being a
    /// `PluginInstruction` type, which wraps an `mpsc::Send` and isn't `Sync`. Due to this, in turn,
    /// the whole error type isn't `Sync` and doesn't work with `anyhow` (or pretty much any other
    /// error handling crate).
    ///
    /// Takes the `SendError` and creates an `anyhow` error type with the message that was sent
    /// (formatted as string), attaching the [`ErrorContext`] as anyhow context to it.
    impl<T: std::fmt::Debug, U> ToAnyhow<U>
        for Result<U, crate::channels::SendError<(T, ErrorContext)>>
    {
        fn to_anyhow(self) -> anyhow::Result<U> {
            match self {
                Ok(val) => anyhow::Ok(val),
                Err(e) => {
                    let (msg, context) = e.into_inner();
                    if *crate::consts::DEBUG_MODE.get().unwrap_or(&true) {
                        Err(anyhow::anyhow!(
                            "failed to send message to channel: {:#?}",
                            msg
                        ))
                        .with_context(|| format_context(&context))
                    } else {
                        Err(anyhow::anyhow!("failed to send message to channel"))
                            .with_context(|| format_context(&context))
                    }
                },
            }
        }
    }

    impl<U> ToAnyhow<U> for Result<U, std::sync::PoisonError<U>> {
        fn to_anyhow(self) -> anyhow::Result<U> {
            match self {
                Ok(val) => anyhow::Ok(val),
                Err(e) => {
                    if *crate::consts::DEBUG_MODE.get().unwrap_or(&true) {
                        Err(anyhow::anyhow!("cannot acquire poisoned lock for {e:#?}"))
                    } else {
                        Err(anyhow::anyhow!("cannot acquire poisoned lock"))
                    }
                },
            }
        }
    }
}

pub fn format_context_type(context: ContextType) -> String {
    if let Some((left, right)) = match context {
        ContextType::Screen(c) => Some(("screen_thread:", format!("{:?}", c))),
        ContextType::Pty(c) => Some(("pty_thread:", format!("{:?}", c))),
        ContextType::Plugin(c) => Some(("plugin_thread:", format!("{:?}", c))),
        ContextType::Client(c) => Some(("main_thread:", format!("{:?}", c))),
        ContextType::IPCServer(c) => Some(("ipc_server:", format!("{:?}", c))),
        ContextType::StdinHandler => Some(("stdin_handler_thread:", "AcceptInput".to_string())),
        ContextType::AsyncTask => Some(("stream_terminal_bytes:", "AsyncTask".to_string())),
        ContextType::PtyWrite(c) => Some(("pty_writer_thread:", format!("{:?}", c))),
        ContextType::BackgroundJob(c) => Some(("background_jobs_thread:", format!("{:?}", c))),
        ContextType::Empty => None,
    } {
        format!("{} {}", left.purple(), right.green())
    } else {
        String::new()
    }
}

/// Render diagnostic context using the runtime's terminal color policy.
pub fn format_context(context: &ErrorContext) -> String {
    use std::fmt::Write;
    let mut output = String::from("Originating Thread(s)\n");
    for (index, call) in context.calls().iter().enumerate() {
        if *call == ContextType::Empty {
            break;
        }
        writeln!(
            output,
            "\t\u{1b}[0;0m{}. {}",
            index + 1,
            format_context_type(*call)
        )
        .expect("writing to a String cannot fail");
    }
    output
}

#[cfg(test)]
#[path = "unit/errors_context_tests.rs"]
mod context_tests;
