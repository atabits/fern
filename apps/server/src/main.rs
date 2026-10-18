//! `fern-server`: the self-hosted sync server.
//!
//! ```text
//! FERN_TOKEN=<secret> fern-server [--listen 127.0.0.1:8650] [--data fern-server.db]
//! ```
//!
//! It speaks plain HTTP; put it behind a reverse proxy with TLS to reach it over the internet.

use anyhow::{Context as _, Result};
use fern_server::{Server, serve};

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

    let server = Server::open(&data)?;
    let http = tiny_http::Server::http(&listen).map_err(|err| anyhow::anyhow!("cannot listen on {listen}: {err}"))?;
    eprintln!("fern-server listening on {listen}, data in {}", data.display());
    serve(&http, server, &token);
    Ok(())
}
