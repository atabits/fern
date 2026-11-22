//! `fern-server`: the self-hosted sync server.
//!
//! ```text
//! FERN_TOKEN=<secret> fern-server [--listen 127.0.0.1:8650] [--data fern-server.db] [--gc-grace-hours 24]
//! fern-server --gc-now [--data fern-server.db] [--gc-grace-hours 24]
//! ```
//!
//! It speaks plain HTTP; put it behind a reverse proxy with TLS to reach it over the internet.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use fern_server::{Server, serve_with_gc_grace};

const DEFAULT_LISTEN: &str = "127.0.0.1:8650";
const DEFAULT_DATA: &str = "fern-server.db";
const DEFAULT_GC_GRACE_HOURS: u64 = 24;

const USAGE: &str = "\
usage:
  FERN_TOKEN=<secret> fern-server [--listen ADDR] [--data FILE] [--gc-grace-hours HOURS]
  fern-server --gc-now [--data FILE] [--gc-grace-hours HOURS]

environment variables:
  FERN_TOKEN           Bearer token devices sync with (required when serving, >= 16 chars)
  FERN_LISTEN          Fallback for --listen (default: 127.0.0.1:8650)
  FERN_DATA            Fallback for --data (default: fern-server.db)
  FERN_GC_GRACE_HOURS  Fallback for --gc-grace-hours (default: 24)";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Serve {
        listen: String,
        data: PathBuf,
        gc_grace_hours: u64,
    },
    GcNow {
        data: PathBuf,
        gc_grace_hours: u64,
    },
    Help,
    Version,
}

fn parse_args(
    args: impl IntoIterator<Item = String>,
    env_var: impl Fn(&str) -> Option<String>,
) -> Result<Command> {
    let mut listen_arg: Option<String> = None;
    let mut data_arg: Option<PathBuf> = None;
    let mut gc_grace_arg: Option<u64> = None;
    let mut gc_now = false;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--listen" => {
                listen_arg = Some(iter.next().context("--listen needs an address")?);
            }
            "--data" => {
                data_arg = Some(iter.next().context("--data needs a path")?.into());
            }
            "--gc-grace-hours" => {
                let raw = iter.next().context("--gc-grace-hours needs a number of hours")?;
                let hours = raw
                    .parse::<u64>()
                    .with_context(|| format!("invalid --gc-grace-hours value {raw:?}"))?;
                gc_grace_arg = Some(hours);
            }
            "--gc-now" => {
                gc_now = true;
            }
            "--help" | "-h" => return Ok(Command::Help),
            "--version" | "-V" => return Ok(Command::Version),
            other => bail!("unknown argument {other}\n\n{USAGE}"),
        }
    }

    let listen = listen_arg
        .or_else(|| env_var("FERN_LISTEN").filter(|v| !v.is_empty()))
        .unwrap_or_else(|| DEFAULT_LISTEN.to_owned());
    let data = data_arg
        .or_else(|| env_var("FERN_DATA").filter(|v| !v.is_empty()).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_DATA));
    let gc_grace_hours = match gc_grace_arg {
        Some(hours) => hours,
        None => match env_var("FERN_GC_GRACE_HOURS").filter(|v| !v.is_empty()) {
            Some(raw) => raw
                .parse::<u64>()
                .with_context(|| format!("invalid FERN_GC_GRACE_HOURS value {raw:?}"))?,
            None => DEFAULT_GC_GRACE_HOURS,
        },
    };
    let _ = gc_grace_hours
        .checked_mul(3600)
        .context("gc grace hours value is too large")?;

    if gc_now {
        Ok(Command::GcNow { data, gc_grace_hours })
    } else {
        Ok(Command::Serve { listen, data, gc_grace_hours })
    }
}

fn main() -> Result<()> {
    let command = parse_args(std::env::args().skip(1), |key| std::env::var(key).ok())?;
    match command {
        Command::Help => {
            println!("{USAGE}");
            Ok(())
        }
        Command::Version => {
            println!("fern-server {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::GcNow { data, gc_grace_hours } => {
            let server = Server::open(&data)?;
            let stats = server.gc_files(Duration::from_secs(gc_grace_hours * 3600))?;
            println!("{}", stats.to_json());
            Ok(())
        }
        Command::Serve { listen, data, gc_grace_hours } => {
            let token = std::env::var("FERN_TOKEN").context("set FERN_TOKEN to the token devices sync with")?;
            anyhow::ensure!(token.len() >= 16, "FERN_TOKEN must be at least 16 characters");

            let server = Server::open(&data)?;
            let http = tiny_http::Server::http(&listen)
                .map_err(|err| anyhow::anyhow!("cannot listen on {listen}: {err}"))?;
            eprintln!(
                "fern-server listening on {listen}, data in {}, gc grace {gc_grace_hours}h",
                data.display()
            );
            serve_with_gc_grace(&http, server, &token, Duration::from_secs(gc_grace_hours * 3600));
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn defaults_and_env_fallbacks() {
        let cmd = parse_args(Vec::<String>::new(), no_env).unwrap();
        assert_eq!(
            cmd,
            Command::Serve {
                listen: "127.0.0.1:8650".into(),
                data: PathBuf::from("fern-server.db"),
                gc_grace_hours: 24,
            }
        );

        let cmd = parse_args(Vec::<String>::new(), |k| match k {
            "FERN_LISTEN" => Some("0.0.0.0:9000".into()),
            "FERN_DATA" => Some("/data/sync.db".into()),
            "FERN_GC_GRACE_HOURS" => Some("12".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            cmd,
            Command::Serve {
                listen: "0.0.0.0:9000".into(),
                data: PathBuf::from("/data/sync.db"),
                gc_grace_hours: 12,
            }
        );
    }

    #[test]
    fn cli_flags_override_env_and_support_gc_now() {
        let args = ["--listen", "127.0.0.1:7777", "--data", "custom.db", "--gc-grace-hours", "0", "--gc-now"]
            .into_iter()
            .map(str::to_owned);
        let cmd = parse_args(args, |k| match k {
            "FERN_LISTEN" => Some("0.0.0.0:9000".into()),
            "FERN_DATA" => Some("/data/sync.db".into()),
            "FERN_GC_GRACE_HOURS" => Some("not-a-number".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(
            cmd,
            Command::GcNow {
                data: PathBuf::from("custom.db"),
                gc_grace_hours: 0,
            }
        );
    }

    #[test]
    fn invalid_args_fail_cleanly() {
        assert!(parse_args(["--unknown".into()], no_env).is_err());
        assert!(parse_args(["--gc-grace-hours".into(), "abc".into()], no_env).is_err());
        assert!(parse_args(Vec::<String>::new(), |k| (k == "FERN_GC_GRACE_HOURS").then(|| "bad".into())).is_err());
    }
}
