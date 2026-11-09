use super::*;
use crate::DispatchFuture;
use yinz_types::web::{Site, WebAuth, WebMethod, WebResponse};
struct Reply(Option<WebReply>, bool);
impl Backend for Reply {
    fn dispatch(&self, _: WebRequest) -> DispatchFuture {
        let reply = self.0.clone();
        let pending = self.1;
        Box::pin(async move {
            if pending {
                std::future::pending().await
            } else {
                reply
            }
        })
    }
    fn site(&self) -> Option<Arc<Site>> {
        None
    }
    fn quiesced(&self) -> bool {
        false
    }
    fn activate(&self) {}
    fn session_name(&self) -> String {
        String::new()
    }
}
fn request() -> WebRequest {
    WebRequest {
        id: 0,
        method: WebMethod::Get,
        path: "/".into(),
        segments: vec![],
        query: vec![],
        headers: vec![],
        body: None,
        auth: WebAuth {
            session: "test".into(),
            label: "test".into(),
        },
        deadline_ms: 0,
    }
}
#[tokio::test(start_paused = true)]
async fn dispatch_contract_without_http_auth_bypass() {
    for (reply, pending, status) in [
        (None, false, 503),
        (
            Some(WebReply::Unavailable {
                reason: "<unsafe>".into(),
                retry_after_s: 7,
            }),
            false,
            503,
        ),
        (Some(WebReply::Failed("<unsafe>".into())), false, 502),
        (None, true, 504),
        (
            Some(WebReply::Response(WebResponse {
                status: 200,
                headers: vec![
                    ("content-security-policy".into(), "bad".into()),
                    ("access-control-allow-origin".into(), "*".into()),
                    ("content-type".into(), "text/plain".into()),
                ],
                body: "good".into(),
            })),
            false,
            200,
        ),
        (
            Some(WebReply::Response(WebResponse {
                status: 100,
                headers: vec![],
                body: String::new(),
            })),
            false,
            502,
        ),
    ] {
        let response = finish(
            dispatch(&Reply(reply, pending), request(), Duration::from_secs(1)).await,
            false,
            true,
            false,
        );
        assert_eq!(response.status().as_u16(), status);
        assert_eq!(response.headers()["vary"], "Authorization");
        assert!(response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .starts_with("default-src 'none'"));
        assert!(!response
            .headers()
            .contains_key("access-control-allow-origin"));
    }
}
#[test]
fn pages_escape_untrusted_text() {
    let page = pages::page("<script>&\"'", "<img src=x>");
    assert!(!page.contains("<script>"));
    assert!(!page.contains("<img"));
    assert!(page.contains("&lt;script&gt;&amp;&quot;&#39;"));
}
