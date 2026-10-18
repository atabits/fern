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
}
