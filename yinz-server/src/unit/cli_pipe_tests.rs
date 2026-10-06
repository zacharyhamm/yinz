use super::*;

#[test]
fn pipe_output_that_fits_in_a_frame_reaches_the_cli_unchanged() {
    assert_eq!(
        cli_pipe_reply("pipe".into(), "{\"ok\":true}\n".into()),
        ServerToClientMsg::CliPipeOutput {
            pipe_name: "pipe".into(),
            output: "{\"ok\":true}\n".into(),
        }
    );
}

/// The thread writing to the client refuses a frame over the limit but can only log it, so
/// before the server checked, `yinz pipe` took the plugin's unblock that followed and exited 0
/// with an empty stdout, and `yinz agent store` waited out its deadline.
#[test]
fn pipe_output_too_large_for_a_frame_reaches_the_cli_as_an_error() {
    let reply = cli_pipe_reply("pipe".into(), "x".repeat(yinz_ipc::MAX_IPC_FRAME_LEN));
    // The variant's name, never `{reply:?}`, which would print 64 MiB.
    let ServerToClientMsg::LogError { lines } = reply else {
        panic!("expected LogError, got {}", <&'static str>::from(&reply));
    };
    let error = lines.concat();
    assert!(
        error.contains(&yinz_ipc::MAX_IPC_FRAME_LEN.to_string()),
        "{error}"
    );
}
