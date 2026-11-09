use super::*;
use crate::{channels, Shutdown};
use yinz_types::web::{WebAuth, WebMethod, WebReply};

#[test]
fn web_backend_shares_site_identity_quiesce_activation_and_reply() {
    let shutdown = Shutdown::default();
    let (sender, receiver) = channels::unbounded();
    let senders = ThreadSenders {
        to_plugin: Some(shutdown.sender(sender)),
        ..Default::default()
    };
    let site = SiteHandle::default();
    let backend = PluginBackend::new(senders.clone(), site.clone());
    assert!(backend.site().is_none());
    site.set(Some(Arc::new(Site {
        generation: 9,
        ..Default::default()
    })));
    assert_eq!(backend.site().unwrap().generation, 9);
    senders.session.set("renamed".into());
    assert_eq!(backend.session_name(), "renamed");
    senders
        .session_update_quiesced
        .store(true, Ordering::Release);
    assert!(backend.quiesced());
    backend.activate();
    assert!(matches!(
        receiver.recv().unwrap().0,
        PluginInstruction::Web(WebInstruction::Activate)
    ));
    let request = WebRequest {
        id: 1,
        method: WebMethod::Get,
        path: "/api/test".into(),
        segments: vec![],
        query: vec![],
        headers: vec![],
        body: None,
        auth: WebAuth {
            session: "test".into(),
            label: "test".into(),
        },
        deadline_ms: 0,
    };
    let future = backend.dispatch(request);
    let PluginInstruction::Web(WebInstruction::Request { reply, .. }) = receiver.recv().unwrap().0
    else {
        panic!("request")
    };
    reply.send(WebReply::Failed("test".into()));
    assert!(matches!(
        crate::global_async_runtime::get_tokio_runtime().block_on(future),
        Some(WebReply::Failed(_))
    ));
    drop(receiver);
    assert_eq!(
        crate::global_async_runtime::get_tokio_runtime().block_on(backend.dispatch(WebRequest {
            id: 2,
            method: WebMethod::Get,
            path: "/".into(),
            segments: vec![],
            query: vec![],
            headers: vec![],
            body: None,
            auth: WebAuth {
                session: "test".into(),
                label: "test".into()
            },
            deadline_ms: 0
        })),
        None
    );
}

#[test]
fn web_configuration_is_disabled_by_default_and_invalid_is_nonfatal() {
    let mut config = yinz_config::config::Config::default();
    let dir = tempfile::tempdir().unwrap();
    assert!(configure(&config, dir.path()).unwrap().is_none());
    config.web.enabled = Some(true);
    let (settings, params) = configure(&config, dir.path()).unwrap().unwrap();
    assert!(params.site.current().is_none());
    config.web.port = Some(0);
    assert_eq!(settings.bind.port(), 7419); // snapshot remains independent of later edits
    config.web.address = Some("not-an-address".into());
    assert!(configure(&config, dir.path()).is_err());
}

#[test]
fn web_listener_releases_port_before_session_worker_joins() {
    use crate::screen::ScreenInstruction;
    use std::time::Duration;
    let temp = tempfile::tempdir().unwrap();
    yinz_web::auth::password::write(temp.path(), &"server test password".to_owned().into())
        .unwrap();
    let mut config = yinz_config::config::Config::default();
    config.web.enabled = Some(true);
    config.web.port = Some(0);
    let (settings, params) = configure(&config, temp.path()).unwrap().unwrap();
    let mut session = crate::session_shutdown_tests::empty_session();
    let (plugin_tx, plugin_rx) = channels::unbounded();
    let (screen_tx, screen_rx) = channels::unbounded();
    session.senders.to_plugin = Some(session.shutdown.sender(plugin_tx));
    session.senders.to_screen = Some(session.shutdown.sender(screen_tx));
    let server = yinz_web::WebServer::start(
        settings,
        Arc::new(PluginBackend::new(session.senders.clone(), params.site)),
        crate::global_async_runtime::get_tokio_runtime().handle(),
    );
    assert!(matches!(
        plugin_rx.recv_timeout(Duration::from_secs(10)).unwrap().0,
        PluginInstruction::Web(WebInstruction::Activate)
    ));
    let addr = server.local_addr().unwrap();
    session.web_server = Some(server);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    session.screen_thread = Some(std::thread::spawn(move || {
        assert!(matches!(
            screen_rx.recv_timeout(Duration::from_secs(10)).unwrap().0,
            ScreenInstruction::Exit
        ));
        entered_tx.send(()).unwrap();
        release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    }));
    let dropper = std::thread::spawn(move || drop(session));
    entered_rx.recv_timeout(Duration::from_secs(10)).unwrap();
    let bound = crate::global_async_runtime::get_tokio_runtime().block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut poll = tokio::time::interval(Duration::from_millis(1));
            loop {
                poll.tick().await;
                if let Ok(listener) = tokio::net::TcpListener::bind(addr).await {
                    break listener;
                }
            }
        })
        .await
    });
    // Release the worker even if the assertion fails, so this test owns all its threads.
    release_tx.send(()).unwrap();
    dropper.join().unwrap();
    assert!(
        bound.is_ok(),
        "web port remained held while shutdown waited for screen"
    );
}
