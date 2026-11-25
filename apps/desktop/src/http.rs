//! The HTTP client GPUI loads remote images with (a description's `![](https://…)` lines), as
//! Zed gives it its own client at start; this one sends GETs with ureq on a thread of their own.

use std::sync::Arc;
use std::time::Duration;

use futures::{FutureExt as _, channel::oneshot, future::BoxFuture};
use gpui::http_client::{AsyncBody, HttpClient, Request, Response, Url, http::HeaderValue};

/// The largest image read, so that a wrong link cannot fill the memory.
const MAX_BODY: u64 = 32 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const GLOBAL_TIMEOUT: Duration = Duration::from_secs(30);

pub struct UreqHttpClient {
    user_agent: HeaderValue,
    agent: ureq::Agent,
}

impl UreqHttpClient {
    pub fn new() -> Arc<Self> {
        let user_agent = HeaderValue::from_str(&format!("fern/{}", env!("CARGO_PKG_VERSION"))).expect("the version is ASCII");
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(GLOBAL_TIMEOUT))
            .build()
            .into();
        Arc::new(Self { user_agent, agent })
    }
}

impl HttpClient for UreqHttpClient {
    fn user_agent(&self) -> Option<&HeaderValue> {
        Some(&self.user_agent)
    }

    fn proxy(&self) -> Option<&Url> {
        None
    }

    fn send(&self, request: Request<AsyncBody>) -> BoxFuture<'static, anyhow::Result<Response<AsyncBody>>> {
        let uri = request.uri().to_string();
        let user_agent = self.user_agent.to_str().unwrap_or_default().to_owned();
        let agent = self.agent.clone();
        let (tx, rx) = oneshot::channel();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<Response<AsyncBody>> {
                let mut response = agent.get(&uri).header("User-Agent", &user_agent).call()?;
                let status = response.status().as_u16();
                let body = response.body_mut().with_config().limit(MAX_BODY).read_to_vec()?;
                Ok(Response::builder().status(status).body(AsyncBody::from(body))?)
            })();
            tx.send(result).ok();
        });
        async move { rx.await.unwrap_or_else(|_| Err(anyhow::anyhow!("the request's thread ended"))) }.boxed()
    }
}
