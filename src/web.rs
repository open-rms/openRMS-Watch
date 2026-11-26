//! Read-only HTTP API + the filter page. Every handler opens its own SQLite
//! connection (WAL lets readers run alongside the writer).

use crate::db::{self, now_ms, StatusPolicy};
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub db: PathBuf,
    pub stale_hours: f64,
    pub gone_days: f64,
    pub config_path: PathBuf,
    pub password_hash: Option<String>,
    /// the page is reachable from the network
    pub lan: bool,
    /// saving settings asks the process to restart itself
    pub restart: Arc<tokio::sync::Notify>,
    pub restart_flag: Arc<std::sync::atomic::AtomicBool>,
    pub atlas_url: Option<String>,
    pub accounts_url: String,
    pub link: Arc<std::sync::Mutex<LinkState>>,
    /// the short-lived Accounts token while the user picks a watcher (never sent to the page)
    pub link_token: Arc<std::sync::Mutex<Option<String>>>,
}

/// One-click linking to openRMS-Atlas (RFC 8628 device flow, feed.md section 1).
#[derive(Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct LinkState {
    /// idle | pending | choose | done | error
    pub status: &'static str,
    /// the user's watchers on openRMS-Atlas, to pick from ("choose")
    pub watchers: Vec<serde_json::Value>,
    pub user_code: Option<String>,
    pub verification_uri: Option<String>,
    pub verification_uri_complete: Option<String>,
    pub watcher: Option<String>,
    pub error: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct Filter {
    q: Option<String>,
    band: Option<String>,
    status: Option<String>,
    receiver: Option<String>,
    kind: Option<String>,
    accepted: Option<String>,
    since_h: Option<f64>,
    limit: Option<i64>,
    format: Option<String>,
}

impl Filter {
    fn q(&self) -> Option<String> {
        self.q.as_ref().map(|s| s.trim().to_ascii_uppercase()).filter(|s| !s.is_empty())
    }
    fn set(v: &Option<String>) -> Option<&str> {
        v.as_deref().filter(|s| !s.is_empty() && *s != "all")
    }
    fn since(&self, now: i64, default_h: f64) -> i64 {
        let h = self.since_h.unwrap_or(default_h);
        if h <= 0.0 { 0 } else { now - (h * 3.6e6) as i64 }
    }
}

type ApiResult<T> = Result<T, (StatusCode, String)>;

fn err(e: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

async fn with_db<T: Send + 'static>(
    st: &AppState,
    f: impl FnOnce(&rusqlite::Connection) -> anyhow::Result<T> + Send + 'static,
) -> ApiResult<T> {
    let path = st.db.clone();
    tokio::task::spawn_blocking(move || {
        let c = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        c.busy_timeout(std::time::Duration::from_secs(5))?;
        f(&c)
    })
    .await
    .map_err(err)?
    .map_err(err)
}

fn csv_or_json<T: Serialize>(rows: Vec<T>, csv: bool, name: &str) -> Response {
    if !csv {
        return Json(rows).into_response();
    }
    let mut out = String::new();
    let vals: Vec<serde_json::Value> = rows.iter().map(|r| serde_json::to_value(r).unwrap()).collect();
    if let Some(serde_json::Value::Object(first)) = vals.first() {
        let keys: Vec<&String> = first.keys().collect();
        out += &keys.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(",");
        out += "\n";
        for v in &vals {
            let line: Vec<String> = keys
                .iter()
                .map(|k| match &v[k.as_str()] {
                    serde_json::Value::Null => String::new(),
                    serde_json::Value::String(s) => format!("\"{}\"", s.replace('"', "\"\"")),
                    serde_json::Value::Array(a) => format!(
                        "\"{}\"",
                        a.iter().map(|x| x.as_str().map(String::from).unwrap_or(x.to_string())).collect::<Vec<_>>().join(" ")
                    ),
                    other => other.to_string(),
                })
                .collect();
            out += &line.join(",");
            out += "\n";
        }
    }
    (
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8".to_string()),
            (header::CONTENT_DISPOSITION, format!("attachment; filename=\"{name}.csv\"")),
        ],
        out,
    )
        .into_response()
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../assets/index.html"))
}

#[derive(Serialize)]
struct Summary {
    now_ms: i64,
    stale_hours: f64,
    gone_days: f64,
    frames: i64,
    frames_24h: i64,
    attempts_24h: i64,
    stations: i64,
    active: i64,
    stale: i64,
    unverified: i64,
    known_calls: i64,
    last_frame_ms: Option<i64>,
    receivers: Vec<String>,
    bands: Vec<String>,
}

async fn summary(State(st): State<Arc<AppState>>) -> ApiResult<Json<Summary>> {
    let pol = StatusPolicy { stale_hours: st.stale_hours, gone_days: st.gone_days };
    let (sh, gd) = (st.stale_hours, st.gone_days);
    with_db(&st, move |c| {
        let now = now_ms();
        let day = now - 86_400_000;
        let one = |q: &str, p: &[&dyn rusqlite::ToSql]| -> anyhow::Result<i64> { Ok(c.query_row(q, p, |r| r.get(0))?) };
        let stations = db::stations(c, &pol, now)?;
        let count = |s: &str| stations.iter().filter(|x| x.status == s).count() as i64;
        let mut receivers: Vec<String> =
            c.prepare("SELECT id FROM receivers ORDER BY id")?.query_map([], |r| r.get(0))?.collect::<Result<_, _>>()?;
        receivers.dedup();
        let bands: Vec<String> = c
            .prepare("SELECT DISTINCT band FROM channels WHERE band IS NOT NULL ORDER BY dial_hz")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_, _>>()?;
        Ok(Json(Summary {
            now_ms: now,
            stale_hours: sh,
            gone_days: gd,
            frames: one("SELECT count(*) FROM frames", &[])?,
            frames_24h: one("SELECT count(*) FROM frames WHERE ts_ms>?1", &[&day])?,
            attempts_24h: one("SELECT count(*) FROM attempts WHERE last_ms>?1", &[&day])?,
            stations: stations.len() as i64,
            active: count("active"),
            stale: count("stale") + count("gone"),
            unverified: count("unverified"),
            known_calls: one("SELECT count(*) FROM known_calls", &[])?,
            last_frame_ms: c.query_row("SELECT max(ts_ms) FROM frames", [], |r| r.get(0))?,
            receivers,
            bands,
        }))
    })
    .await
}

async fn stations(State(st): State<Arc<AppState>>, Query(f): Query<Filter>) -> ApiResult<Response> {
    let pol = StatusPolicy { stale_hours: st.stale_hours, gone_days: st.gone_days };
    let rows = with_db(&st, move |c| db::stations(c, &pol, now_ms())).await?;
    let q = f.q();
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|s| {
            q.as_ref().is_none_or(|q| {
                s.crc.contains(q.as_str())
                    || s.callsign.as_deref().is_some_and(|c| c.contains(q.as_str()))
                    || s.candidates.iter().any(|c| c.contains(q.as_str()))
            })
        })
        .filter(|s| Filter::set(&f.band).is_none_or(|b| s.bands.contains(&b)))
        .filter(|s| Filter::set(&f.status).is_none_or(|x| s.status == x))
        .filter(|s| Filter::set(&f.receiver).is_none_or(|r| s.receivers.iter().any(|x| x == r)))
        .collect();
    Ok(csv_or_json(rows, f.format.as_deref() == Some("csv"), "stations"))
}

async fn attempts(State(st): State<Arc<AppState>>, Query(f): Query<Filter>) -> ApiResult<Response> {
    let now = now_ms();
    let since = f.since(now, 24.0 * 7.0);
    let limit = f.limit.unwrap_or(500).clamp(1, 10_000);
    // over-fetch, then filter here
    let rows = with_db(&st, move |c| db::attempts(c, since, 20_000)).await?;
    let q = f.q();
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|a| {
            q.as_ref().is_none_or(|q| {
                a.client_call.as_deref().is_some_and(|c| c.contains(q.as_str()))
                    || a.rms_crc.as_deref().is_some_and(|c| c.contains(q.as_str()))
                    || a.rms_candidates.iter().any(|c| c.contains(q.as_str()))
                    || a.dial_hz.to_string().contains(q.as_str())
            })
        })
        .filter(|a| Filter::set(&f.band).is_none_or(|b| a.band == b))
        .filter(|a| Filter::set(&f.receiver).is_none_or(|r| a.receivers.iter().any(|x| x == r)))
        .filter(|a| match Filter::set(&f.accepted) {
            Some("yes") => a.accepted,
            Some("no") => !a.accepted,
            _ => true,
        })
        .take(limit as usize)
        .collect();
    Ok(csv_or_json(rows, f.format.as_deref() == Some("csv"), "attempts"))
}

async fn frames(State(st): State<Arc<AppState>>, Query(f): Query<Filter>) -> ApiResult<Response> {
    let now = now_ms();
    let since = f.since(now, 24.0);
    let limit = f.limit.unwrap_or(500).clamp(1, 10_000);
    let rows = with_db(&st, move |c| db::frames(c, since, 20_000)).await?;
    let q = f.q();
    let rows: Vec<_> = rows
        .into_iter()
        .filter(|r| q.as_ref().is_none_or(|q| r.raw.to_ascii_uppercase().contains(q.as_str())))
        .filter(|r| Filter::set(&f.kind).is_none_or(|k| r.kind.as_deref() == Some(k)))
        .filter(|r| Filter::set(&f.band).is_none_or(|b| r.band == b))
        .filter(|r| Filter::set(&f.receiver).is_none_or(|x| r.receiver_id.as_deref() == Some(x)))
        .take(limit as usize)
        .collect();
    Ok(csv_or_json(rows, f.format.as_deref() == Some("csv"), "frames"))
}

async fn channels(State(st): State<Arc<AppState>>, Query(f): Query<Filter>) -> ApiResult<Response> {
    let rows = with_db(&st, move |c| db::channels(c, now_ms())).await?;
    Ok(csv_or_json(rows, f.format.as_deref() == Some("csv"), "channels"))
}

// ---------------------------------------------------------------- settings

/// Settings need the password when one is set; a network-reachable page with
/// no password refuses changes outright.
fn check_auth(st: &AppState, headers: &HeaderMap, write: bool) -> Result<(), Response> {
    if let Some(hash) = &st.password_hash {
        let ok = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Basic "))
            .and_then(|b| base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b).ok())
            .and_then(|b| String::from_utf8(b).ok())
            .and_then(|up| up.split_once(':').map(|(_, p)| p.to_string()))
            .is_some_and(|pw| crate::settings::check_password(hash, &pw));
        if !ok {
            return Err((
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Basic realm=\"openrms-watch settings\"")],
                "password required",
            )
                .into_response());
        }
    } else if write && st.lan {
        return Err((
            StatusCode::FORBIDDEN,
            "this page is reachable from the network: set a password first (openrms-watch set-password)",
        )
            .into_response());
    }
    Ok(())
}

async fn get_settings(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(r) = check_auth(&st, &headers, false) {
        return r;
    }
    match crate::config::Config::load(&st.config_path) {
        Ok(cfg) => Json(crate::settings::from_config(&cfg)).into_response(),
        Err(e) => err(format!("{e:#}")).into_response(),
    }
}

/// Capabilities of a device: from the running engine if it holds it, else probed.
async fn caps_for(args: String) -> Result<crate::sdr::Caps, String> {
    if let Some(v) = crate::live::snapshot().caps.get(&args) {
        return serde_json::from_value(v.clone()).map_err(|e| e.to_string());
    }
    tokio::task::spawn_blocking(move || crate::sdr::probe(&args).map_err(|e| format!("{e:#}")))
        .await
        .map_err(|e| e.to_string())?
}

async fn plan_for(s: &crate::settings::Settings) -> Result<(crate::plan::Plan, Vec<String>), String> {
    if s.radio.device.is_empty() {
        return Err("choose an SDR".into());
    }
    let caps = caps_for(s.radio.device.clone()).await?;
    let bands: Vec<(u64, u64)> = s
        .radio
        .bands
        .iter()
        .map(|b| crate::plan::band(b).ok_or(format!("unknown band {b}")))
        .collect::<Result<_, _>>()?;
    let p = crate::plan::plan(&bands, &caps.rates, caps.freq_min_hz, caps.freq_max_hz, s.radio.upconverter_hz, s.decoding.decoders)?;
    Ok((p, caps.notes))
}

async fn post_plan(State(st): State<Arc<AppState>>, headers: HeaderMap, Json(s): Json<crate::settings::Settings>) -> Response {
    if let Err(r) = check_auth(&st, &headers, false) {
        return r;
    }
    match plan_for(&s).await {
        Ok((p, notes)) => Json(serde_json::json!({"ok": true, "plan": p, "notes": notes})).into_response(),
        Err(e) => Json(serde_json::json!({"ok": false, "error": e})).into_response(),
    }
}

async fn put_settings(State(st): State<Arc<AppState>>, headers: HeaderMap, Json(s): Json<crate::settings::Settings>) -> Response {
    if let Err(r) = check_auth(&st, &headers, true) {
        return r;
    }
    if s.web.lan && st.password_hash.is_none() && s.web.password.as_deref().is_none_or(str::is_empty) {
        return (StatusCode::BAD_REQUEST, "set a password before opening the page to the network").into_response();
    }
    let plan = if s.radio.device.is_empty() {
        None
    } else {
        match plan_for(&s).await {
            Ok((p, _)) => Some(p),
            Err(e) => return (StatusCode::BAD_REQUEST, e).into_response(),
        }
    };
    let text = match std::fs::read_to_string(&st.config_path) {
        Ok(t) => t,
        Err(e) => return err(e).into_response(),
    };
    let new = match crate::settings::apply(&text, &s, plan.as_ref()) {
        Ok(t) => t,
        Err(e) => return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response(),
    };
    if let Err(e) = crate::settings::save(&st.config_path, &new) {
        return (StatusCode::BAD_REQUEST, format!("{e:#}")).into_response();
    }
    tracing::info!("settings saved to {}; restarting", st.config_path.display());
    st.restart_flag.store(true, std::sync::atomic::Ordering::SeqCst);
    let n = st.restart.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        n.notify_one();
    });
    Json(serde_json::json!({"ok": true, "restarting": true, "plan": plan})).into_response()
}

async fn devices(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(r) = check_auth(&st, &headers, false) {
        return r;
    }
    let list = match tokio::task::spawn_blocking(crate::sdr::list).await {
        Ok(Ok(l)) => l,
        Ok(Err(e)) => return err(format!("{e:#}")).into_response(),
        Err(e) => return err(e).into_response(),
    };
    let mut out = Vec::new();
    for d in list {
        let caps = caps_for(d.args.clone()).await;
        out.push(serde_json::json!({"device": d, "caps": caps.as_ref().ok(), "error": caps.err()}));
    }
    Json(out).into_response()
}

async fn live(State(st): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let snap = crate::live::snapshot();
    let up = with_db(&st, |c| {
        let q = |k: &str| crate::db::kv_get(c, k);
        let queue: i64 = c.query_row("SELECT count(*) FROM outbox WHERE sent_ms IS NULL", [], |r| r.get(0)).unwrap_or(0);
        let sent24: i64 = c
            .query_row("SELECT count(*) FROM outbox WHERE sent_ms > ?1", [crate::db::now_ms() - 86_400_000], |r| r.get(0))
            .unwrap_or(0);
        let calls: i64 = c.query_row("SELECT count(*) FROM directory_calls", [], |r| r.get(0)).unwrap_or(0);
        Ok(serde_json::json!({
            "enabled": q("contribute_enabled").as_deref() == Some("1"),
            "lastOkMs": q("upload_last_ok").and_then(|v| v.parse::<i64>().ok()),
            "error": q("upload_error").filter(|e| !e.is_empty()),
            "queue": queue, "sent24h": sent24,
            "directoryCallsigns": calls,
            "callsignsUpdatedMs": q("callsigns_updated").and_then(|v| v.parse::<i64>().ok()),
        }))
    })
    .await?;
    Ok(Json(serde_json::json!({"engine": snap, "contribute": up, "version": crate::upload::SOFTWARE})))
}

// ---------------------------------------------------------------- device linking

fn link_set(st: &AppState, f: impl FnOnce(&mut LinkState)) {
    f(&mut st.link.lock().unwrap());
}

fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => format!("HTTP {code}: {}", r.into_string().unwrap_or_default()),
        e => e.to_string(),
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tokio::task::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

/// Start "Link to openRMS": a device code from openRMS-Accounts (RFC 8628) for
/// openRMS-Atlas as audience (identity.md section 3).
async fn link_start(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(r) = check_auth(&st, &headers, true) {
        return r;
    }
    let Some(dir) = st.atlas_url.clone().map(|u| u.trim_end_matches('/').to_string()) else {
        return (StatusCode::BAD_REQUEST, "set the Atlas URL in Settings first, save, then link").into_response();
    };
    let accounts = st.accounts_url.clone();
    let (acc, d2) = (accounts.clone(), dir.clone());
    let r = blocking(move || {
        let disc: serde_json::Value = ureq::get(&format!("{acc}/.well-known/openid-configuration"))
            .call()
            .map_err(http_err)?
            .into_json()
            .map_err(|e| e.to_string())?;
        let dev = disc["device_authorization_endpoint"].as_str().ok_or("Accounts has no device login")?.to_string();
        let tok = disc["token_endpoint"].as_str().ok_or("Accounts has no token endpoint")?.to_string();
        let code: serde_json::Value = ureq::post(&dev)
            .send_form(&[
                ("client_id", "openrms-watch"),
                ("scope", "openid callsign clubs"), // no offline_access: the token is used once, so no refresh token
                ("resource", d2.as_str()),
            ])
            .map_err(http_err)?
            .into_json()
            .map_err(|e| e.to_string())?;
        Ok((tok, code))
    })
    .await;
    let (token_endpoint, code) = match r {
        Ok(x) => x,
        Err(e) => return (StatusCode::BAD_GATEWAY, format!("couldn't reach openRMS Accounts at {accounts}: {e}")).into_response(),
    };
    let Some(device_code) = code["device_code"].as_str().map(String::from) else {
        return (StatusCode::BAD_GATEWAY, "unexpected answer from openRMS Accounts").into_response();
    };
    let interval = code["interval"].as_u64().unwrap_or(5).max(1);
    let expires = code["expires_in"].as_u64().unwrap_or(900);
    link_set(&st, |l| {
        *l = LinkState {
            status: "pending",
            user_code: code["user_code"].as_str().map(String::from),
            verification_uri: code["verification_uri"].as_str().map(String::from),
            verification_uri_complete: code["verification_uri_complete"].as_str().map(String::from),
            ..Default::default()
        }
    });
    let st2 = st.clone();
    tokio::spawn(async move {
        let r = async {
            let token = link_poll(&token_endpoint, &device_code, interval, expires).await?;
            let (d, t) = (dir.clone(), token.clone());
            let list: serde_json::Value = blocking(move || {
                ureq::get(&format!("{d}/api/v1/me/watchers"))
                    .set("authorization", &format!("Bearer {t}"))
                    .call()
                    .map_err(http_err)?
                    .into_json()
                    .map_err(|e| e.to_string())
            })
            .await?;
            Ok::<_, String>((token, list))
        }
        .await;
        match r {
            Ok((token, list)) => {
                *st2.link_token.lock().unwrap() = Some(token);
                let watchers = list["watchers"].as_array().cloned().unwrap_or_default();
                link_set(&st2, |l| {
                    l.status = "choose";
                    l.watchers = watchers
                        .iter()
                        .map(|w| serde_json::json!({"id": w["id"], "name": w["name"], "gridsquare": w["gridsquare"], "hasKey": w["hasKey"]}))
                        .collect();
                });
            }
            Err(e) => link_set(&st2, |l| {
                l.status = "error";
                l.error = Some(e);
            }),
        }
    });
    Json(st.link.lock().unwrap().clone()).into_response()
}

/// Poll the token endpoint until the code is approved.
async fn link_poll(token_endpoint: &str, device_code: &str, interval: u64, expires: u64) -> Result<String, String> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(expires);
    let mut wait = interval;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        if std::time::Instant::now() > deadline {
            return Err("the code expired before it was approved; start again".into());
        }
        let (ep, dc) = (token_endpoint.to_string(), device_code.to_string());
        let v = blocking(move || {
            let resp = ureq::post(&ep).send_form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", dc.as_str()),
                ("client_id", "openrms-watch"),
            ]);
            match resp {
                Ok(r) => r.into_json::<serde_json::Value>().map_err(|e| e.to_string()),
                // OAuth errors are 400 with a JSON body
                Err(ureq::Error::Status(_, r)) => r.into_json::<serde_json::Value>().map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            }
        })
        .await;
        let Ok(v) = v else { continue }; // network blip: keep polling until the deadline
        if let Some(t) = v["access_token"].as_str() {
            return Ok(t.to_string());
        }
        match v["error"].as_str() {
            Some("authorization_pending") | None => {}
            Some("slow_down") => wait += 5,
            Some("access_denied") => return Err("the request was declined on the site".into()),
            Some("expired_token") => return Err("the code expired; start again".into()),
            Some(other) => return Err(format!("openRMS Accounts said: {other}")),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LinkFinish {
    watcher_id: Option<String>,
    name: Option<String>,
    gridsquare: Option<String>,
}

/// The user picked (or named) a watcher: get its key from openRMS-Atlas, save it, restart.
async fn link_finish(State(st): State<Arc<AppState>>, headers: HeaderMap, Json(f): Json<LinkFinish>) -> Response {
    if let Err(r) = check_auth(&st, &headers, true) {
        return r;
    }
    let Some(token) = st.link_token.lock().unwrap().clone() else {
        return (StatusCode::BAD_REQUEST, "start linking first").into_response();
    };
    let Some(dir) = st.atlas_url.clone().map(|u| u.trim_end_matches('/').to_string()) else {
        return (StatusCode::BAD_REQUEST, "no Atlas URL").into_response();
    };
    let body = match (&f.watcher_id, &f.name) {
        (Some(id), _) if !id.is_empty() => serde_json::json!({"watcherId": id}),
        (_, Some(n)) if !n.trim().is_empty() => {
            let mut b = serde_json::json!({"name": n.trim()});
            if let Some(g) = f.gridsquare.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
                b["gridsquare"] = g.into();
            }
            b
        }
        _ => return (StatusCode::BAD_REQUEST, "pick a watcher or give the new one a name").into_response(),
    };
    let r = blocking(move || {
        ureq::post(&format!("{dir}/api/v1/watch/link"))
            .set("authorization", &format!("Bearer {token}"))
            .send_json(body)
            .map_err(http_err)?
            .into_json::<serde_json::Value>()
            .map_err(|e| e.to_string())
    })
    .await;
    let v = match r {
        Ok(v) => v,
        Err(e) => {
            link_set(&st, |l| {
                l.status = "error";
                l.error = Some(format!("openRMS-Atlas refused: {e}"));
            });
            return (StatusCode::BAD_GATEWAY, format!("openRMS-Atlas refused: {e}")).into_response();
        }
    };
    let Some(key) = v["key"].as_str().map(String::from) else {
        return (StatusCode::BAD_GATEWAY, "no key in openRMS-Atlas's answer").into_response();
    };
    let name = v["name"].as_str().unwrap_or("watcher").to_string();
    *st.link_token.lock().unwrap() = None; // single use: discard
    let saved = (|| -> anyhow::Result<()> {
        let text = std::fs::read_to_string(&st.config_path)?;
        let cfg = crate::config::Config::load(&st.config_path)?;
        let mut s = crate::settings::from_config(&cfg);
        s.contribute.enabled = true;
        s.contribute.token = Some(key);
        // keep the current tuning: no re-plan
        let plan = cfg.widebands.first().map(|w| crate::plan::Plan {
            rate: w.rate, center_hz: w.center_hz, segments: w.segments.clone(), cores: 0.0,
        });
        let new = crate::settings::apply(&text, &s, plan.as_ref())?;
        crate::settings::save(&st.config_path, &new)
    })();
    if let Err(e) = saved {
        return err(format!("linked, but saving the key failed: {e:#}")).into_response();
    }
    link_set(&st, |l| {
        l.status = "done";
        l.watcher = Some(name.clone());
        l.watchers.clear();
    });
    tracing::info!("linked as watcher {name:?}; restarting to start contributing");
    let st2 = st.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        st2.restart_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        st2.restart.notify_one();
    });
    Json(st.link.lock().unwrap().clone()).into_response()
}

async fn link_status(State(st): State<Arc<AppState>>) -> Json<LinkState> {
    let mut l = st.link.lock().unwrap().clone();
    if l.status.is_empty() {
        l.status = "idle";
    }
    Json(l)
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/summary", get(summary))
        .route("/api/stations", get(stations))
        .route("/api/attempts", get(attempts))
        .route("/api/frames", get(frames))
        .route("/api/channels", get(channels))
        .route("/api/settings", get(get_settings).put(put_settings))
        .route("/api/plan", axum::routing::post(post_plan))
        .route("/api/devices", get(devices))
        .route("/api/live", get(live))
        .route("/api/link", get(link_status).post(link_start))
        .route("/api/link/finish", axum::routing::post(link_finish))
        .with_state(Arc::new(state))
}
