//! HTTP & OAuth QuickJS host API bindings (`fern.http`, `fern.oauth`).

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread;

use anyhow::anyhow;
use futures::channel::oneshot;
use rquickjs::function::Opt;
use rquickjs::promise::Promised;
use rquickjs::{Ctx, Exception, Function, Object, Value};
use serde_json::{Map, Value as JsonValue, json};

use super::convert::{js_to_json, json_to_js, throw_scrubbed};
use super::{PluginApiState, PluginUiEffect};
use crate::http::{HttpRequest, validate_target_uri_syntax};
use crate::oauth::{
    DEFAULT_OAUTH_TIMEOUT_SEC, DEFAULT_REDIRECT_HOST, DEFAULT_REDIRECT_PATH, DEFAULT_REDIRECT_PORT,
    OAuthAuthorizeOptions, OAuthAuthorizeRequest, OAuthRefreshRequest, OAuthTokenAuth,
    run_oauth_authorize_with_options, run_oauth_refresh,
};

pub(super) fn install_http<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;
    let st = Rc::clone(&state);

    obj.set(
        "fetch",
        Function::new(
            ctx.clone(),
            move |ctx: Ctx<'js>, url: String, opts: Opt<Value<'js>>| -> rquickjs::Result<Value<'js>> {
                let uri: ureq::http::Uri = url
                    .trim()
                    .parse()
                    .map_err(|e| Exception::throw_message(&ctx, &format!("invalid URL: {e}")))?;
                validate_target_uri_syntax(&uri, &st.permissions)
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;

                let opts_json = match opts.0 {
                    Some(v) if !v.is_undefined() && !v.is_null() => {
                        js_to_json(&ctx, v).map_err(|e| throw_scrubbed(&ctx, &st, e))?
                    }
                    _ => JsonValue::Null,
                };
                let method = opts_json
                    .get("method")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("GET")
                    .to_owned();
                let timeout_ms = opts_json.get("timeoutMs").and_then(JsonValue::as_u64);
                let mut headers = Vec::new();
                if let Some(hdrs) = opts_json.get("headers").and_then(JsonValue::as_object) {
                    for (k, v) in hdrs {
                        if let Some(vs) = v.as_str() {
                            headers.push((k.clone(), vs.to_owned()));
                        }
                    }
                }
                let interpolated_secret = headers.iter().any(|(_, v)| v.contains("${secret:"));
                let body = match opts_json.get("body") {
                    Some(JsonValue::String(s)) => Some(s.as_bytes().to_vec()),
                    Some(JsonValue::Null) | None => None,
                    Some(other) => Some(serde_json::to_vec(other).unwrap_or_default()),
                };

                let req = HttpRequest {
                    method,
                    url,
                    headers,
                    body,
                    timeout_ms,
                };
                let rx = st.http_pool.fetch(
                    st.plugin_id.clone(),
                    st.permissions.clone(),
                    st.vault.clone(),
                    req,
                );
                let st_for_fut = Rc::clone(&st);
                let ctx_for_fut = ctx.clone();

                let promised = Promised::from(async move {
                    let resp = match rx.await {
                        Ok(Ok(r)) => r,
                        Ok(Err(err)) => return Err(throw_scrubbed(&ctx_for_fut, &st_for_fut, err)),
                        Err(_) => {
                            return Err(Exception::throw_message(
                                &ctx_for_fut,
                                "HTTP request cancelled",
                            ));
                        }
                    };
                    let raw_body = resp.body_text();
                    let body_text = if interpolated_secret {
                        st_for_fut.scrub(&raw_body)
                    } else {
                        raw_body
                    };
                    let resp_url = if interpolated_secret {
                        st_for_fut.scrub(&resp.url)
                    } else {
                        resp.url.clone()
                    };
                    let mut hdr_map = Map::new();
                    for (k, v) in &resp.headers {
                        let val = if interpolated_secret {
                            st_for_fut.scrub(v)
                        } else {
                            v.clone()
                        };
                        hdr_map.insert(k.to_ascii_lowercase(), JsonValue::String(val));
                    }
                    let obj = Object::new(ctx_for_fut.clone())?;
                    obj.set("ok", resp.ok())?;
                    obj.set("status", i32::from(resp.status))?;
                    obj.set("url", resp_url)?;
                    obj.set("headers", json_to_js(&ctx_for_fut, &JsonValue::Object(hdr_map))?)?;
                    obj.set("body", body_text.clone())?;
                    let body_for_text = body_text.clone();
                    obj.set(
                        "text",
                        Function::new(ctx_for_fut.clone(), move || body_for_text.clone())?,
                    )?;
                    let st_for_json = Rc::clone(&st_for_fut);
                    obj.set(
                        "json",
                        Function::new(ctx_for_fut.clone(), move |ctx: Ctx<'js>| -> rquickjs::Result<Value<'js>> {
                            let parsed: JsonValue = serde_json::from_str(&body_text).map_err(|err| {
                                let msg = st_for_json.scrub(&format!("invalid JSON body: {err}"));
                                Exception::throw_message(&ctx, &msg)
                            })?;
                            json_to_js(&ctx, &parsed)
                        })?,
                    )?;
                    Ok(obj.into_value())
                });

                rquickjs::IntoJs::into_js(promised, &ctx)
            },
        )?,
    )?;

    fern.set("http", obj)?;
    Ok(())
}

fn parse_oauth_scopes(val: Option<&JsonValue>) -> Vec<String> {
    match val {
        Some(JsonValue::Array(arr)) => arr
            .iter()
            .filter_map(JsonValue::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect(),
        Some(JsonValue::String(s)) => s
            .split_whitespace()
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

pub(super) fn install_oauth<'js>(
    ctx: &Ctx<'js>,
    fern: &Object<'js>,
    state: Rc<PluginApiState>,
) -> rquickjs::Result<()> {
    let obj = Object::new(ctx.clone())?;

    // authorize(opts) -> Promise<{ ok, tokenType, scope, expiresIn, hasRefreshToken }>
    {
        let st = Rc::clone(&state);
        obj.set(
            "authorize",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, opts_val: Value<'js>| -> rquickjs::Result<Value<'js>> {
                    let opts = js_to_json(&ctx, opts_val)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let authorize_url = opts
                        .get("authorizeUrl")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let token_url = opts
                        .get("tokenUrl")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let client_id = opts
                        .get("clientId")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let client_secret_key = opts
                        .get("clientSecretKey")
                        .and_then(JsonValue::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let scopes = parse_oauth_scopes(opts.get("scopes").or_else(|| opts.get("scope")));
                    let redirect_host = opts
                        .get("redirectHost")
                        .and_then(JsonValue::as_str)
                        .unwrap_or(DEFAULT_REDIRECT_HOST)
                        .to_owned();
                    let redirect_port = opts
                        .get("redirectPort")
                        .and_then(JsonValue::as_u64)
                        .and_then(|p| u16::try_from(p).ok())
                        .unwrap_or(DEFAULT_REDIRECT_PORT);
                    let redirect_path = opts
                        .get("redirectPath")
                        .and_then(JsonValue::as_str)
                        .unwrap_or(DEFAULT_REDIRECT_PATH)
                        .to_owned();
                    let token_auth = OAuthTokenAuth::parse(
                        opts.get("tokenAuth").and_then(JsonValue::as_str),
                    )
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let save_access_token_as = opts
                        .get("saveAccessTokenAs")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("access_token")
                        .to_owned();
                    let save_refresh_token_as = match opts.get("saveRefreshTokenAs") {
                        Some(JsonValue::Null) => None,
                        Some(JsonValue::String(s)) if s.trim().is_empty() => None,
                        Some(JsonValue::String(s)) => Some(s.trim().to_owned()),
                        _ => Some("refresh_token".to_owned()),
                    };
                    let mut extra_authorize_params = Vec::new();
                    if let Some(extra) = opts
                        .get("extraAuthorizeParams")
                        .and_then(JsonValue::as_object)
                    {
                        for (k, v) in extra {
                            if let Some(vs) = v.as_str() {
                                extra_authorize_params.push((k.clone(), vs.to_owned()));
                            }
                        }
                    }
                    let timeout_sec = opts
                        .get("timeoutSec")
                        .and_then(JsonValue::as_u64)
                        .unwrap_or(DEFAULT_OAUTH_TIMEOUT_SEC);
                    let pkce = opts.get("pkce").and_then(JsonValue::as_bool).unwrap_or(false);

                    let req = OAuthAuthorizeRequest {
                        authorize_url,
                        token_url,
                        client_id,
                        client_secret_key,
                        scopes,
                        redirect_host,
                        redirect_port,
                        redirect_path,
                        token_auth,
                        save_access_token_as,
                        save_refresh_token_as,
                        extra_authorize_params,
                        timeout_sec,
                    };
                    let auth_options = OAuthAuthorizeOptions { pkce };

                    let permit = st
                        .oauth_limiter
                        .try_acquire(&st.plugin_id)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;

                    let (tx, rx) = oneshot::channel();
                    let plugin_id = st.plugin_id.clone();
                    let permissions = st.permissions.clone();
                    let vault = st.vault.clone();
                    let cancel = Arc::clone(&st.shutdown_flag);
                    let emitter = Arc::clone(&st.effect_emitter);

                    if let Err(err) = thread::Builder::new()
                        .name("fern-oauth".to_owned())
                        .spawn(move || {
                            let _permit = permit;
                            let res = run_oauth_authorize_with_options(
                                &plugin_id,
                                &permissions,
                                &vault,
                                &req,
                                &auth_options,
                                &cancel,
                                |url| {
                                    emitter(PluginUiEffect::OpenExternal { url });
                                },
                            );
                            let _ = tx.send(res);
                        })
                    {
                        return Err(Exception::throw_message(
                            &ctx,
                            &format!("failed to start OAuth worker: {err}"),
                        ));
                    }

                    let st_for_fut = Rc::clone(&st);
                    let ctx_for_fut = ctx.clone();
                    let promised = Promised::from(async move {
                        let summary = match rx.await {
                            Ok(Ok(s)) => s,
                            Ok(Err(err)) => return Err(throw_scrubbed(&ctx_for_fut, &st_for_fut, err)),
                            Err(_) => {
                                return Err(Exception::throw_message(
                                    &ctx_for_fut,
                                    "OAuth authorization cancelled",
                                ));
                            }
                        };
                        let val = json!({
                            "ok": summary.ok,
                            "tokenType": summary.token_type,
                            "scope": summary.scope,
                            "expiresIn": summary.expires_in,
                            "hasRefreshToken": summary.has_refresh_token,
                        });
                        json_to_js(&ctx_for_fut, &val)
                    });
                    rquickjs::IntoJs::into_js(promised, &ctx)
                },
            )?,
        )?;
    }

    // refresh(opts) -> Promise<{ ok, tokenType, scope, expiresIn, hasRefreshToken }>
    {
        let st = Rc::clone(&state);
        obj.set(
            "refresh",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, opts_val: Value<'js>| -> rquickjs::Result<Value<'js>> {
                    let opts = js_to_json(&ctx, opts_val)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let token_url = opts
                        .get("tokenUrl")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let client_id = opts
                        .get("clientId")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let client_secret_key = opts
                        .get("clientSecretKey")
                        .and_then(JsonValue::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned);
                    let refresh_token_key = opts
                        .get("refreshTokenKey")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("refresh_token")
                        .to_owned();
                    let scopes = parse_oauth_scopes(opts.get("scopes").or_else(|| opts.get("scope")));
                    let token_auth = OAuthTokenAuth::parse(
                        opts.get("tokenAuth").and_then(JsonValue::as_str),
                    )
                    .map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                    let save_access_token_as = opts
                        .get("saveAccessTokenAs")
                        .and_then(JsonValue::as_str)
                        .unwrap_or("access_token")
                        .to_owned();
                    let save_refresh_token_as = match opts.get("saveRefreshTokenAs") {
                        Some(JsonValue::Null) => None,
                        Some(JsonValue::String(s)) if s.trim().is_empty() => None,
                        Some(JsonValue::String(s)) => Some(s.trim().to_owned()),
                        _ => Some("refresh_token".to_owned()),
                    };

                    let req = OAuthRefreshRequest {
                        token_url,
                        client_id,
                        client_secret_key,
                        refresh_token_key,
                        scopes,
                        token_auth,
                        save_access_token_as,
                        save_refresh_token_as,
                    };

                    let permit = st
                        .oauth_limiter
                        .try_acquire(&st.plugin_id)
                        .map_err(|e| throw_scrubbed(&ctx, &st, e))?;

                    let (tx, rx) = oneshot::channel();
                    let plugin_id = st.plugin_id.clone();
                    let permissions = st.permissions.clone();
                    let vault = st.vault.clone();
                    let cancel = Arc::clone(&st.shutdown_flag);

                    if let Err(err) = thread::Builder::new()
                        .name("fern-oauth-ref".to_owned())
                        .spawn(move || {
                            let _permit = permit;
                            let res = if cancel.load(Ordering::Acquire) {
                                Err(anyhow!("OAuth refresh cancelled"))
                            } else {
                                run_oauth_refresh(&plugin_id, &permissions, &vault, &req)
                            };
                            let _ = tx.send(res);
                        })
                    {
                        return Err(Exception::throw_message(
                            &ctx,
                            &format!("failed to start OAuth refresh worker: {err}"),
                        ));
                    }

                    let st_for_fut = Rc::clone(&st);
                    let ctx_for_fut = ctx.clone();
                    let promised = Promised::from(async move {
                        let summary = match rx.await {
                            Ok(Ok(s)) => s,
                            Ok(Err(err)) => return Err(throw_scrubbed(&ctx_for_fut, &st_for_fut, err)),
                            Err(_) => {
                                return Err(Exception::throw_message(
                                    &ctx_for_fut,
                                    "OAuth refresh cancelled",
                                ));
                            }
                        };
                        let val = json!({
                            "ok": summary.ok,
                            "tokenType": summary.token_type,
                            "scope": summary.scope,
                            "expiresIn": summary.expires_in,
                            "hasRefreshToken": summary.has_refresh_token,
                        });
                        json_to_js(&ctx_for_fut, &val)
                    });
                    rquickjs::IntoJs::into_js(promised, &ctx)
                },
            )?,
        )?;
    }

    // clearTokens(keys?) -> number (deletes stored OAuth tokens from SecretsVault without reading them)
    {
        let st = Rc::clone(&state);
        obj.set(
            "clearTokens",
            Function::new(
                ctx.clone(),
                move |ctx: Ctx<'js>, keys_opt: Opt<Value<'js>>| -> rquickjs::Result<i32> {
                    let keys: Vec<String> = match keys_opt.0 {
                        Some(v) if !v.is_undefined() && !v.is_null() => {
                            let j = js_to_json(&ctx, v).map_err(|e| throw_scrubbed(&ctx, &st, e))?;
                            match j {
                                JsonValue::Array(arr) => arr
                                    .into_iter()
                                    .filter_map(|item| item.as_str().map(str::to_owned))
                                    .collect(),
                                JsonValue::String(s) => vec![s],
                                _ => vec!["access_token".to_owned(), "refresh_token".to_owned()],
                            }
                        }
                        _ => vec!["access_token".to_owned(), "refresh_token".to_owned()],
                    };
                    let mut removed = 0i32;
                    for key in keys {
                        if !key.trim().is_empty()
                            && st
                                .vault
                                .delete(&st.plugin_id, key.trim())
                                .map_err(|e| throw_scrubbed(&ctx, &st, e))?
                        {
                            removed += 1;
                        }
                    }
                    Ok(removed)
                },
            )?,
        )?;
    }

    fern.set("oauth", obj)?;
    Ok(())
}
