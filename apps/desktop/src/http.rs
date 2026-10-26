//! The HTTP client GPUI loads remote images with (a description's `![](https://…)` lines), as
//! Zed gives it its own client at start; this one sends GETs with ureq on a thread of their own.

use std::sync::Arc;

use futures::{FutureExt as _, channel::oneshot, future::BoxFuture};
use gpui::http_client::{AsyncBody, HttpClient, Request, Response, Url, http::HeaderValue};

/// The largest image read, so that a wrong link cannot fill the memory.
const MAX_BODY: u64 = 32 * 1024 * 1024;

pub struct UreqHttpClient {
    user_agent: HeaderValue,
}

impl UreqHttpClient {
    pub fn new() -> Arc<Self> {
        let user_agent = HeaderValue::from_str(&format!("fern/{}", env!("CARGO_PKG_VERSION"))).expect("the version is ASCII");
        Arc::new(Self { user_agent })
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
        let (tx, rx) = oneshot::channel();
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<Response<AsyncBody>> {
                let mut response = ureq::get(&uri).header("User-Agent", &user_agent).call()?;
                let status = response.status().as_u16();
                let body = response.body_mut().with_config().limit(MAX_BODY).read_to_vec()?;
                Ok(Response::builder().status(status).body(AsyncBody::from(body))?)
            })();
            tx.send(result).ok();
        });
        async move { rx.await.unwrap_or_else(|_| Err(anyhow::anyhow!("the request's thread ended"))) }.boxed()
    }
}
