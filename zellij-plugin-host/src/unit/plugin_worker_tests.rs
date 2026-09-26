use super::*;
use std::sync::mpsc;
use wasmi::{Engine, Linker, Module};

#[test]
fn callbacks_run_outside_tokio_and_sender_drop_releases_the_worker() {
    let engine = Engine::default();
    let module = Module::new(
        &engine,
        r#"(module
            (import "test" "callback" (func $callback))
            (func (export "test_worker") call $callback)
        )"#,
    )
    .unwrap();
    let mut store = Store::new(&engine, crate::test_env::test_env());
    let mut linker = Linker::new(&engine);
    let (called, calls) = mpsc::channel();
    linker
        .func_wrap("test", "callback", move || {
            called
                .send(tokio::runtime::Handle::try_current().is_ok())
                .unwrap();
        })
        .unwrap();
    let instance = linker.instantiate_and_start(&mut store, &module).unwrap();
    let worker = plugin_worker(RunningWorker::new(store, instance, "test_worker")).unwrap();
    let timeout = zellij_test_support::default_timeout();
    for _ in 0..2 {
        worker
            .send(MessageToWorker::Message("job".into(), "payload".into()))
            .unwrap();
        assert!(
            !calls.recv_timeout(timeout).unwrap(),
            "guest entered the shared I/O runtime"
        );
    }
    drop(worker);
    // The host callback owns the sender: channel disconnection proves the store and
    // callback were dropped after the final job, without keeping a thread alive.
    drop(linker);
    assert_eq!(
        calls.recv_timeout(timeout),
        Err(mpsc::RecvTimeoutError::Disconnected)
    );
}
