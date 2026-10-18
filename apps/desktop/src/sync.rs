//! Syncing with the user's own `fern-server`: the request and the answer travel as JSON over
//! HTTP(S); what they hold is the store's business (`fern_core::sync`).

use chrono::NaiveDateTime;
use fern_core::sync::SyncConfig;

/// How the last sync went, for the sidebar's sync button.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum SyncStatus {
    /// No server set.
    #[default]
    Off,
    Running,
    Done(NaiveDateTime),
    Failed(String),
}

/// How often the app syncs even without local changes, to fetch other devices' changes.
pub const PULL_EVERY: std::time::Duration = std::time::Duration::from_secs(5 * 60);
/// How often it checks for local changes to send.
pub const PUSH_CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// Sends one sync request and returns the server's answer. Blocking: run it off the UI thread.
pub fn post(config: &SyncConfig, body: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
    let url = format!("{}/sync", config.server.trim().trim_end_matches('/'));
    let mut response = ureq::post(&url)
        .header("Authorization", &format!("Bearer {}", config.token))
        .header("Content-Type", "application/json")
        .send(body.to_string())?;
    let text = response.body_mut().read_to_string()?;
    Ok(serde_json::from_str(&text)?)
}

/// Moves attached files after a sync: sends `upload` from the attachment folder `dir` and
/// fetches `download` into it. Returns the files that made it either way; a file another device
/// has not sent yet is simply tried again next time. Blocking: run it off the UI thread.
pub fn transfer_files(config: &SyncConfig, dir: &std::path::Path, upload: &[String], download: &[String]) -> Vec<String> {
    let url = |name: &str| format!("{}/files/{name}", config.server.trim().trim_end_matches('/'));
    let auth = format!("Bearer {}", config.token);
    let mut done = Vec::new();
    for name in upload {
        let sent = std::fs::read(dir.join(name))
            .map_err(anyhow::Error::from)
            .and_then(|data| Ok(ureq::put(&url(name)).header("Authorization", &auth).send(&data[..])?));
        match sent {
            Ok(_) => done.push(name.clone()),
            Err(err) => eprintln!("failed to send {name}: {err:#}"),
        }
    }
    for name in download {
        let fetched = ureq::get(&url(name))
            .header("Authorization", &auth)
            .call()
            .map_err(anyhow::Error::from)
            .and_then(|mut response| Ok(response.body_mut().with_config().limit(64 * 1024 * 1024).read_to_vec()?))
            .and_then(|data| {
                std::fs::create_dir_all(dir)?;
                Ok(std::fs::write(dir.join(name), data)?)
            });
        match fetched {
            Ok(()) => done.push(name.clone()),
            Err(err) => eprintln!("failed to fetch {name}: {err:#}"),
        }
    }
    done
}

#[cfg(test)]
mod tests {
    use fern_core::Store;

    use super::*;

    /// Two stores sync through a real fern-server over HTTP, the way the app does.
    #[test]
    fn sync_over_http() {
        let http = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = http.server_addr().to_ip().unwrap().port();
        let token = "a-token-for-the-test-only";
        std::thread::spawn(move || fern_server::serve(&http, fern_server::Server::open_in_memory().unwrap(), token));
        let config = SyncConfig { server: format!("http://127.0.0.1:{port}/"), token: token.to_owned() };

        let sync = |store: &Store| {
            let request = store.sync_request().unwrap();
            let answer = post(&config, &request.body).unwrap();
            store.finish_sync(&request, &answer).unwrap()
        };
        let laptop = Store::open_in_memory().unwrap();
        let phone = Store::open_in_memory().unwrap();
        let task = laptop.add_task("Renew the passport").unwrap();
        sync(&laptop);
        assert_eq!(sync(&phone), 1);
        assert_eq!(phone.task(task.id).unwrap().unwrap().title, "Renew the passport");

        let wrong = SyncConfig { token: "wrong".into(), ..config.clone() };
        assert!(post(&wrong, &laptop.sync_request().unwrap().body).is_err(), "a wrong token is refused");
    }

    /// An attached file travels from one device to the other through the server's file store.
    #[test]
    fn attachments_over_http() {
        let dir = std::env::temp_dir().join(format!("fern-sync-http-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let http = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = http.server_addr().to_ip().unwrap().port();
        let token = "a-token-for-the-test-only";
        let server = fern_server::Server::open(&dir.join("server.db")).unwrap();
        std::thread::spawn(move || fern_server::serve(&http, server, token));
        let config = SyncConfig { server: format!("http://127.0.0.1:{port}"), token: token.to_owned() };

        let sync = |store: &Store, files: &std::path::Path| {
            let request = store.sync_request().unwrap();
            store.finish_sync(&request, &post(&config, &request.body).unwrap()).unwrap();
            let (upload, download) = store.files_to_sync(files).unwrap();
            let done = transfer_files(&config, files, &upload, &download);
            store.mark_files_synced(&done).unwrap();
        };
        let (laptop_files, phone_files) = (dir.join("laptop"), dir.join("phone"));
        std::fs::write(dir.join("ticket.pdf"), b"%PDF ticket").unwrap();
        let laptop = Store::open_in_memory().unwrap();
        let phone = Store::open_in_memory().unwrap();
        let task = laptop.add_task("Flight").unwrap();
        let attachment = laptop.add_attachment(task.id, &dir.join("ticket.pdf"), &laptop_files).unwrap();
        sync(&laptop, &laptop_files);
        sync(&phone, &phone_files);
        assert_eq!(std::fs::read(attachment.path(&phone_files)).unwrap(), b"%PDF ticket");
        assert_eq!(phone.files_to_sync(&phone_files).unwrap(), (vec![], vec![]));
        std::fs::remove_dir_all(&dir).ok();
    }
}
