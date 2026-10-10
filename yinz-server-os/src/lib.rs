mod os_input_output;
mod os_input_output_unix;

pub use os_input_output::{
    get_server_os_input, get_server_os_input_from_termios_arg, get_server_os_input_with_termios,
    ServerOsInputOutput,
};

// The moved files keep their `crate::` paths to shared items through these imports, as they had
// them at `yinz-bus`'s root.
use yinz_process::pty_host;
use yinz_types::internal_ids::{ClientId, TerminalId};
use yinz_types::pane_id::PaneId;
