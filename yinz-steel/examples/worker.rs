//! Small worker executable for testing the interpreter independently of the server.
fn main() -> anyhow::Result<()> {
    yinz_steel::worker::run_worker()
}
