//! The yinz server's operating-system backend.
//!
//! [`ServerOsInputOutput`] implements `yinz_bus::os_input_output::ServerOsApi` over real ptys,
//! pane supervisors, remote brokers, Firecracker VMs and the client sockets' writer threads. Only
//! the binary and this crate's tests construct it; library crates see it through the trait. The
//! in-process whole-app tests use a fake, so beyond this crate's tests the backend runs only under
//! tests that spawn the `yinz` binary, such as the root package's tests and the integration
//! suite's plugin-test sessions.

mod os_input_output;
mod os_input_output_unix;

pub use os_input_output::{
    get_server_os_input, get_server_os_input_from_termios_arg, get_server_os_input_with_termios,
    ServerOsInputOutput,
};

// The moved files keep their `crate::` paths to shared items through these imports, as they had
// them at `yinz-bus`'s root.
use yinz_process::{firecracker, pty_host};
use yinz_types::internal_ids::{ClientId, TerminalId};
use yinz_types::pane_id::PaneId;
