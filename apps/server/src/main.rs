//! `fern-server`: the self-hosted sync server.
//!
//! ```text
//! FERN_TOKEN=<secret> fern-server [--listen 127.0.0.1:8650] [--data fern-server.db]
//! ```
//!
//! It speaks plain HTTP; put it behind a reverse proxy with TLS to reach it over the internet.

use std::io::Read as _;

use anyhow::{Context as _, Result};
use fern_server::{Server, authorized};
use tiny_http::{Header, Method, Response};

/// Requests larger than this are refused.
const MAX_BODY: usize = 32 * 1024 * 1024;

fn main() -> Result<()> {
    let mut listen = "127.0.0.1:8650".to_owned();
    let mut data = std::path::PathBuf::from("fern-server.db");
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => listen = args.next().context("--listen needs an address")?,
            "--data" => data = args.next().context("--data needs a path")?.into(),
            other => anyhow::bail!("unknown argument {other}; usage: FERN_TOKEN=… fern-server [--listen ADDR] [--data FILE]"),
        }
    }
    let token = std::env::var("FERN_TOKEN").context("set FERN_TOKEN to the token devices sync with")?;
    anyhow::ensure!(token.len() >= 16, "FERN_TOKEN must be at least 16 characters");

    let mut server = Server::open(&data)?;
    let http = tiny_http::Server::http(&listen).map_err(|err| anyhow::anyhow!("cannot listen on {listen}: {err}"))?;
    eprintln!("fern-server listening on {listen}, data in {}", data.display());
    for mut request in http.incoming_requests() {
        let auth = request.headers().iter().find(|h| h.field.equiv("Authorization")).map(|h| h.value.as_str().to_owned());
        let (status, body) = if request.method() != &Method::Post || request.url() != "/sync" {
            (404, r#"{"error":"not found"}"#.to_owned())
        } else if !authorized(auth.as_deref(), &token) {
            (401, r#"{"error":"unauthorized"}"#.to_owned())
        } else {
            let mut text = String::new();
            let read = request.as_reader().take(MAX_BODY as u64 + 1).read_to_string(&mut text);
            match read {
                Ok(n) if n > MAX_BODY => (413, r#"{"error":"too large"}"#.to_owned()),
                Ok(_) => match serde_json::from_str(&text).map_err(anyhow::Error::from).and_then(|req| server.sync(&req)) {
                    Ok(answer) => (200, answer.to_string()),
                    Err(err) => (400, serde_json::json!({ "error": format!("{err:#}") }).to_string()),
                },
                Err(err) => (400, serde_json::json!({ "error": err.to_string() }).to_string()),
            }
        };
        let header = Header::from_bytes("Content-Type", "application/json").expect("a valid header");
        if let Err(err) = request.respond(Response::from_string(body).with_status_code(status).with_header(header)) {
            eprintln!("failed to answer: {err}");
        }
    }
    Ok(())
}
