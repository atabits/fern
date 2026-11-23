use super::*;
use crate::permissions::Permission;

#[test]
fn percent_encode_and_query_decode_roundtrip() {
    let encoded = encode_form_pairs(&[
        ("scope", "tasks:read tasks:write".to_owned()),
        ("redirect_uri", "http://127.0.0.1:18481/callback".to_owned()),
    ]);
    let parsed = parse_query_params(&encoded);
    assert_eq!(
        parsed,
        vec![
            ("scope".to_owned(), "tasks:read tasks:write".to_owned()),
            (
                "redirect_uri".to_owned(),
                "http://127.0.0.1:18481/callback".to_owned()
            ),
        ]
    );
}

#[test]
fn loopback_oauth_authorize_and_refresh_store_tokens_in_vault() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let vault = SecretsVault::open(tmp.path().join("vault.bin"), tmp.path().join("vault.key"))?;
    vault.set("com.test.oauth", "client_secret", "super-secret-xyz")?;

    // Local mock OAuth token server.
    let token_srv = TcpListener::bind("127.0.0.1:0")?;
    let token_port = token_srv.local_addr()?.port();
    let token_thread = thread::spawn(move || {
        for idx in 0..2 {
            let (mut stream, _) = token_srv.accept().expect("accept token request");
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .expect("set read timeout");
            let mut raw = Vec::new();
            let mut tmp_buf = [0u8; 4096];
            let mut expected_total: Option<usize> = None;
            loop {
                let n = stream.read(&mut tmp_buf).expect("read token request");
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&tmp_buf[..n]);
                if expected_total.is_none()
                    && let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n")
                {
                    let hdr_end = pos + 4;
                    let hdrs = String::from_utf8_lossy(&raw[..pos]);
                    let content_len = hdrs
                        .lines()
                        .find_map(|line| {
                            let (k, v) = line.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    expected_total = Some(hdr_end + content_len);
                }
                if let Some(total) = expected_total
                    && raw.len() >= total
                {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&raw).into_owned();
            assert!(
                text.to_ascii_lowercase().contains("authorization: basic "),
                "expected Basic auth header, got: {text}"
            );
            let json_body = if idx == 0 {
                assert!(text.contains("grant_type=authorization_code"));
                assert!(text.contains("code=auth-code-999"));
                r#"{"access_token":"tok-initial-123","refresh_token":"ref-456","token_type":"bearer","expires_in":3600,"scope":"tasks:read tasks:write"}"#
            } else {
                assert!(text.contains("grant_type=refresh_token"));
                assert!(text.contains("refresh_token=ref-456"));
                r#"{"access_token":"tok-refreshed-789","token_type":"bearer","expires_in":7200}"#
            };
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json_body}",
                json_body.len()
            );
            stream.write_all(resp.as_bytes()).expect("write token response");
        }
    });

    // Reserve a free callback port for the test.
    let cb_port = {
        let probe = TcpListener::bind("127.0.0.1:0")?;
        probe.local_addr()?.port()
    };

    let perms = PermissionSet::new(&[
        Permission::Network(vec![format!("127.0.0.1:{token_port}")]),
        Permission::NetworkLocal,
    ]);
    let cancel = AtomicBool::new(false);

    let summary = run_oauth_authorize(
        "com.test.oauth",
        &perms,
        &vault,
        &OAuthAuthorizeRequest {
            authorize_url: format!("http://127.0.0.1:{token_port}/oauth/authorize"),
            token_url: format!("http://127.0.0.1:{token_port}/oauth/token"),
            client_id: "my-client".to_owned(),
            client_secret_key: Some("client_secret".to_owned()),
            scopes: vec!["tasks:read".to_owned(), "tasks:write".to_owned()],
            redirect_host: "127.0.0.1".to_owned(),
            redirect_port: cb_port,
            redirect_path: "/callback".to_owned(),
            token_auth: OAuthTokenAuth::Basic,
            save_access_token_as: "access_token".to_owned(),
            save_refresh_token_as: Some("refresh_token".to_owned()),
            extra_authorize_params: Vec::new(),
            timeout_sec: 10,
        },
        &cancel,
        move |opened_url| {
            // Simulate browser hitting /favicon.ico first, then the real callback with matching state.
            thread::spawn(move || {
                let query = opened_url.split_once('?').map(|(_, q)| q).unwrap_or("");
                let state = parse_query_params(query)
                    .into_iter()
                    .find(|(k, _)| k == "state")
                    .map(|(_, v)| v)
                    .expect("state param present");

                let mut s1 = TcpStream::connect(("127.0.0.1", cb_port)).expect("connect favicon");
                let _ = s1.write_all(b"GET /favicon.ico HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");

                let mut s2 = TcpStream::connect(("127.0.0.1", cb_port)).expect("connect callback");
                let req = format!(
                    "GET /callback?code=auth-code-999&state={state} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n"
                );
                s2.write_all(req.as_bytes()).expect("send callback");
                let mut reply = String::new();
                let _ = s2.read_to_string(&mut reply);
                assert!(reply.contains("200 OK"));
            });
        },
    )?;

    assert!(summary.ok);
    assert!(summary.has_refresh_token);
    assert_eq!(summary.expires_in, Some(3600));
    assert_eq!(
        vault.get("com.test.oauth", "access_token")?.as_deref(),
        Some("tok-initial-123")
    );
    assert_eq!(
        vault.get("com.test.oauth", "refresh_token")?.as_deref(),
        Some("ref-456")
    );

    let refreshed = run_oauth_refresh(
        "com.test.oauth",
        &perms,
        &vault,
        &OAuthRefreshRequest {
            token_url: format!("http://127.0.0.1:{token_port}/oauth/token"),
            client_id: "my-client".to_owned(),
            client_secret_key: Some("client_secret".to_owned()),
            refresh_token_key: "refresh_token".to_owned(),
            scopes: vec!["tasks:read".to_owned(), "tasks:write".to_owned()],
            token_auth: OAuthTokenAuth::Basic,
            save_access_token_as: "access_token".to_owned(),
            save_refresh_token_as: Some("refresh_token".to_owned()),
        },
    )?;
    assert!(refreshed.ok);
    assert_eq!(refreshed.expires_in, Some(7200));
    assert_eq!(
        vault.get("com.test.oauth", "access_token")?.as_deref(),
        Some("tok-refreshed-789")
    );

    token_thread.join().expect("mock token server thread");
    Ok(())
}

#[test]
fn oauth_rejects_remote_http_param_pollution_fragments_and_token_redirects() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let vault = SecretsVault::open(tmp.path().join("v.bin"), tmp.path().join("v.key"))?;
    vault.set("p", "sec", "my-secret-value")?;
    let perms = PermissionSet::new(&[
        Permission::Network(vec!["*".to_owned()]),
        Permission::NetworkLocal,
    ]);
    let cancel = AtomicBool::new(false);

    let base_req = OAuthAuthorizeRequest {
        authorize_url: "https://auth.example.com/authorize".to_owned(),
        token_url: "https://auth.example.com/token".to_owned(),
        client_id: "cid".to_owned(),
        client_secret_key: Some("sec".to_owned()),
        scopes: vec!["tasks:read".to_owned()],
        redirect_host: "127.0.0.1".to_owned(),
        redirect_port: 18482,
        redirect_path: "/callback".to_owned(),
        token_auth: OAuthTokenAuth::Basic,
        save_access_token_as: "access_token".to_owned(),
        save_refresh_token_as: None,
        extra_authorize_params: Vec::new(),
        timeout_sec: 5,
    };

    // 1. Plaintext http:// to remote host or LAN IP must fail even with network:local.
    for bad_url in [
        "http://auth.example.com/authorize",
        "http://192.168.1.20/authorize",
        "http://evil.localhost/authorize",
    ] {
        let mut r = base_req.clone();
        r.authorize_url = bad_url.to_owned();
        let err = run_oauth_authorize("p", &perms, &vault, &r, &cancel, |_| {}).unwrap_err();
        assert!(
            format!("{err:#}").contains("loopback") || format!("{err:#}").contains("https://"),
            "unexpected error for {bad_url}: {err:#}"
        );
    }

    // 2. Fragment `#` and reserved query keys in authorizeUrl or extraAuthorizeParams must fail.
    for reserved in ["redirect_uri", "state", "client_id", "response_type", "scope", "code_challenge"] {
        let mut r1 = base_req.clone();
        r1.authorize_url = format!("https://auth.example.com/authorize?{reserved}=evil");
        assert!(
            run_oauth_authorize("p", &perms, &vault, &r1, &cancel, |_| {}).is_err(),
            "authorizeUrl allowed reserved param {reserved}"
        );

        let mut r2 = base_req.clone();
        r2.extra_authorize_params = vec![(reserved.to_owned(), "evil".to_owned())];
        assert!(
            run_oauth_authorize("p", &perms, &vault, &r2, &cancel, |_| {}).is_err(),
            "extraAuthorizeParams allowed reserved param {reserved}"
        );
    }
    {
        let mut r = base_req;
        r.authorize_url = "https://auth.example.com/authorize#frag".to_owned();
        assert!(run_oauth_authorize("p", &perms, &vault, &r, &cancel, |_| {}).is_err());
    }

    // 3. Token exchange must refuse HTTP 307 redirects (max_redirects = 0).
    let redir_srv = TcpListener::bind("127.0.0.1:0")?;
    let redir_port = redir_srv.local_addr()?.port();
    let redir_thread = thread::spawn(move || {
        if let Ok((mut stream, _)) = redir_srv.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
            let mut raw = Vec::new();
            let mut tmp_buf = [0u8; 4096];
            let mut expected_total: Option<usize> = None;
            while let Ok(n) = stream.read(&mut tmp_buf) {
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&tmp_buf[..n]);
                if expected_total.is_none()
                    && let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n")
                {
                    let hdrs = String::from_utf8_lossy(&raw[..pos]);
                    let cl = hdrs
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    expected_total = Some(pos + 4 + cl);
                }
                if let Some(total) = expected_total
                    && raw.len() >= total
                {
                    break;
                }
            }
            let _ = stream.write_all(
                b"HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.flush();
        }
    });
    vault.set("p", "ref", "refresh-tok-secret")?;
    let redir_err = run_oauth_refresh(
        "p",
        &perms,
        &vault,
        &OAuthRefreshRequest {
            token_url: format!("http://127.0.0.1:{redir_port}/oauth/token"),
            client_id: "cid".to_owned(),
            client_secret_key: Some("sec".to_owned()),
            refresh_token_key: "ref".to_owned(),
            scopes: vec![],
            token_auth: OAuthTokenAuth::Body,
            save_access_token_as: "access_token".to_owned(),
            save_refresh_token_as: None,
        },
    )
    .unwrap_err();
    assert!(
        format!("{redir_err:#}").contains("redirects are forbidden"),
        "{redir_err:#}"
    );
    let _ = redir_thread.join();

    Ok(())
}

#[test]
fn oauth_callback_ignores_bad_host_and_unauthenticated_error_and_supports_pkce_s256() -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let vault = SecretsVault::open(tmp.path().join("v.bin"), tmp.path().join("v.key"))?;

    let token_srv = TcpListener::bind("127.0.0.1:0")?;
    let token_port = token_srv.local_addr()?.port();
    let captured_challenge = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let challenge_for_srv = std::sync::Arc::clone(&captured_challenge);

    let token_thread = thread::spawn(move || {
        let (mut stream, _) = token_srv.accept().expect("accept token");
        let mut raw = Vec::new();
        let mut tmp_buf = [0u8; 4096];
        let mut expected_total: Option<usize> = None;
        loop {
            let n = stream.read(&mut tmp_buf).expect("read token");
            if n == 0 {
                break;
            }
            raw.extend_from_slice(&tmp_buf[..n]);
            if expected_total.is_none()
                && let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n")
            {
                let hdrs = String::from_utf8_lossy(&raw[..pos]);
                let cl = hdrs
                    .lines()
                    .find_map(|l| {
                        let (k, v) = l.split_once(':')?;
                        k.eq_ignore_ascii_case("content-length")
                            .then(|| v.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                expected_total = Some(pos + 4 + cl);
            }
            if let Some(total) = expected_total
                && raw.len() >= total
            {
                break;
            }
        }
        let text = String::from_utf8_lossy(&raw);
        let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
        let form = parse_query_params(body);
        let verifier = form
            .iter()
            .find(|(k, _)| k == "code_verifier")
            .map(|(_, v)| v.as_str())
            .expect("code_verifier must be sent when pkce=true");
        let expected_challenge =
            BASE64_URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        assert_eq!(expected_challenge, *challenge_for_srv.lock().unwrap());
        let json_body = r#"{"access_token":"pkce-tok","token_type":"bearer"}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json_body}",
            json_body.len()
        );
        stream.write_all(resp.as_bytes()).expect("write resp");
    });

    let cb_port = {
        let probe = TcpListener::bind("127.0.0.1:0")?;
        probe.local_addr()?.port()
    };
    let perms = PermissionSet::new(&[
        Permission::Network(vec![format!("127.0.0.1:{token_port}")]),
        Permission::NetworkLocal,
    ]);
    let cancel = AtomicBool::new(false);
    let challenge_for_open = std::sync::Arc::clone(&captured_challenge);

    let summary = run_oauth_authorize_with_options(
        "com.test.pkce",
        &perms,
        &vault,
        &OAuthAuthorizeRequest {
            authorize_url: format!("http://127.0.0.1:{token_port}/oauth/authorize"),
            token_url: format!("http://127.0.0.1:{token_port}/oauth/token"),
            client_id: "pkce-client".to_owned(),
            client_secret_key: None,
            scopes: vec!["read".to_owned()],
            redirect_host: "127.0.0.1".to_owned(),
            redirect_port: cb_port,
            redirect_path: "/callback".to_owned(),
            token_auth: OAuthTokenAuth::Body,
            save_access_token_as: "access_token".to_owned(),
            save_refresh_token_as: None,
            extra_authorize_params: Vec::new(),
            timeout_sec: 10,
        },
        &OAuthAuthorizeOptions { pkce: true },
        &cancel,
        move |opened_url| {
            let query = opened_url.split_once('?').map(|(_, q)| q).unwrap_or("");
            let params = parse_query_params(query);
            let state = params
                .iter()
                .find(|(k, _)| k == "state")
                .map(|(_, v)| v.clone())
                .expect("state present");
            let challenge = params
                .iter()
                .find(|(k, _)| k == "code_challenge")
                .map(|(_, v)| v.clone())
                .expect("code_challenge present");
            assert_eq!(
                params
                    .iter()
                    .find(|(k, _)| k == "code_challenge_method")
                    .map(|(_, v)| v.as_str()),
                Some("S256")
            );
            *challenge_for_open.lock().unwrap() = challenge;

            thread::spawn(move || {
                // 1. Attacker sends bad Host header -> must get 400 and NOT abort flow.
                let mut s_host = TcpStream::connect(("127.0.0.1", cb_port)).unwrap();
                let _ = s_host.write_all(
                    format!("GET /callback?code=evil&state={state} HTTP/1.1\r\nHost: attacker.com\r\n\r\n")
                        .as_bytes(),
                );
                let mut r1 = String::new();
                let _ = s_host.read_to_string(&mut r1);
                assert!(r1.contains("400 Bad Request"), "{r1}");

                // 2. Attacker sends `?error=spoofed` WITHOUT state -> must get 400 and NOT abort flow.
                let mut s_err = TcpStream::connect(("127.0.0.1", cb_port)).unwrap();
                let _ = s_err.write_all(
                    format!("GET /callback?error=access_denied&error_description=phishing HTTP/1.1\r\nHost: 127.0.0.1:{cb_port}\r\n\r\n")
                        .as_bytes(),
                );
                let mut r2 = String::new();
                let _ = s_err.read_to_string(&mut r2);
                assert!(r2.contains("400 Bad Request"), "{r2}");

                // 3. Attacker sends wrong state -> must get 400 and NOT abort flow.
                let mut s_bad_state = TcpStream::connect(("127.0.0.1", cb_port)).unwrap();
                let _ = s_bad_state.write_all(
                    format!("GET /callback?code=evil&state=wrong HTTP/1.1\r\nHost: 127.0.0.1:{cb_port}\r\n\r\n")
                        .as_bytes(),
                );
                let mut r3 = String::new();
                let _ = s_bad_state.read_to_string(&mut r3);
                assert!(r3.contains("400 Bad Request"), "{r3}");

                // 4. Genuine callback succeeds!
                let mut s_ok = TcpStream::connect(("127.0.0.1", cb_port)).unwrap();
                let _ = s_ok.write_all(
                    format!("GET /callback?code=real-code&state={state} HTTP/1.1\r\nHost: 127.0.0.1:{cb_port}\r\n\r\n")
                        .as_bytes(),
                );
                let mut r4 = String::new();
                let _ = s_ok.read_to_string(&mut r4);
                assert!(r4.contains("200 OK"), "{r4}");
            });
        },
    )?;
    assert!(summary.ok);
    assert_eq!(
        vault.get("com.test.pkce", "access_token")?.as_deref(),
        Some("pkce-tok")
    );
    token_thread.join().expect("token thread");

    // Verify serde default for OAuthAuthorizeOptions is pkce = false.
    let opts: OAuthAuthorizeOptions = serde_json::from_str("{}")?;
    assert!(!opts.pkce);
    Ok(())
}

#[test]
fn localhost_listener_fails_closed_when_ipv6_port_already_occupied() -> Result<()> {
    // Bind an ephemeral port on [::1]; if IPv6 loopback is supported on this host,
    // `bind_loopback_listeners("localhost", port)` must refuse to fall back to IPv4-only.
    let Ok(v6_occupier) = TcpListener::bind(SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 0))
    else {
        return Ok(());
    };
    let port = v6_occupier.local_addr()?.port();
    let res = bind_loopback_listeners("localhost", port);
    assert!(res.is_err(), "expected fail-closed error when [::1]:{port} is occupied");
    drop(v6_occupier);
    Ok(())
}
