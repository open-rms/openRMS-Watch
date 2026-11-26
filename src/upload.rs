//! Contributing to the openRMS Directory (openRMS-Spec/watch/feed.md), and the
//! Directory's callsign list for local naming.
//!
//! Events go into a local `outbox` table first, so nothing heard is lost while
//! offline or after a restart; a loop sends them in NDJSON batches. Uploading is
//! opt-in; the callsign list is fetched whenever a Directory URL is set.

use crate::db::{self, kv_get, kv_set, now_ms};
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

pub const SOFTWARE: &str = concat!("openrms-watch ", env!("CARGO_PKG_VERSION"));

/// RFC 3339 UTC with milliseconds, from unix ms.
pub fn iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let milli = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{milli:03}Z",
        sod / 3600,
        (sod / 60) % 60,
        sod % 60
    )
}

fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// What the event builders need to know about each receiver.
#[derive(Clone)]
pub struct ReceiverMeta {
    pub ppm: f64,
}

/// Optional fields are left out rather than sent as null (the Directory's
/// validation, and smaller batches).
fn strip_nulls(v: &Value) -> Value {
    match v {
        Value::Object(m) => Value::Object(
            m.iter().filter(|(_, x)| !x.is_null()).map(|(k, x)| (k.clone(), strip_nulls(x))).collect(),
        ),
        Value::Array(a) => Value::Array(a.iter().map(strip_nulls).collect()),
        x => x.clone(),
    }
}

pub fn enqueue(c: &Connection, id: &str, body: &Value) -> Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO outbox(id,body,created_ms) VALUES(?1,?2,?3)",
        params![id, strip_nulls(body).to_string(), now_ms()],
    )?;
    Ok(())
}

fn crc_hex(v: Option<u16>) -> Value {
    v.map(|x| json!(format!("{x:04X}"))).unwrap_or(Value::Null)
}

/// The `frame` event for a stored frame.
pub fn frame_event(c: &Connection, h: &db::Heard, eid: &str, meta: &HashMap<String, ReceiverMeta>) -> Result<Value> {
    let f = &h.frame;
    let calibrated = meta.get(&h.receiver_id).is_some_and(|m| m.ppm != 0.0);
    let named = f.to_crc.or(f.called_crc);
    let hint = match named {
        Some(crc) => Some(crate::naming::resolve(c, crc, Some(h.dial_hz as i64))?),
        None => None,
    };
    Ok(json!({
        "type": "frame",
        "id": eid,
        "ts": iso(h.ts_ms),
        "modem": "mercury",
        "dialHz": h.dial_hz,
        "freqSource": h.freq_source,
        "freqAccuracyHz": if h.freq_source == "scan" { if calibrated { 10 } else { 300 } } else { 50 },
        "mode": f.mode,
        "kind": f.kind,
        "from": f.from,
        "toCrc": crc_hex(f.to_crc),
        "callerCrc": crc_hex(f.caller_crc),
        "calledCrc": crc_hex(f.called_crc),
        "sid": f.sid,
        "bw": f.bw,
        "snrDb": f.snr,
        "raw": f.raw,
        "hint": hint,
    }))
}

pub fn coverage_event(c: &Connection, channel_id: i64, start_ms: i64, end_ms: i64) -> Result<Value> {
    let (lo, hi, recv): (i64, i64, String) = c.query_row(
        "SELECT coalesce(lo_hz,dial_hz), coalesce(hi_hz,dial_hz), receiver_id FROM channels WHERE id=?1",
        [channel_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(json!({
        "type": "coverage", "id": new_id(), "start": iso(start_ms), "end": iso(end_ms),
        "ranges": [[lo, hi]], "receiver": recv,
    }))
}

/// Queue `attempt` events for attempts that are finished (disconnected, or quiet
/// for longer than the attempt window).
fn queue_attempts(c: &Connection, window_s: i64) -> Result<usize> {
    let cutoff = now_ms() - window_s * 1000;
    let mut st = c.prepare(
        "SELECT id,dial_hz,first_ms,last_ms,client_call,client_crc,rms_crc,sid,n_call,last_accept_ms,
                max(coalesce(snr_call,-99),coalesce(snr_accept,-99))
         FROM attempts WHERE coalesce(uploaded,0)=0 AND (disconnect_ms IS NOT NULL OR last_ms<?1) LIMIT 200",
    )?;
    let rows: Vec<(i64, i64, i64, i64, Option<String>, Option<i64>, Option<i64>, Option<i64>, i64, Option<i64>, f64)> = st
        .query_map([cutoff], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?, r.get(9)?, r.get(10)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let n = rows.len();
    for (id, dial, first, last, client, ccrc, rcrc, sid, ncall, acc, snr) in rows {
        // the frames this attempt was built from
        let frames: Vec<String> = c
            .prepare(
                "SELECT event_id FROM frames WHERE event_id IS NOT NULL AND abs(dial_hz-?1)<=80
                   AND ts_ms BETWEEN ?2 AND ?3
                   AND (from_call IS ?4 OR caller_crc IS ?5 OR to_crc IS ?6 OR called_crc IS ?6 OR sid IS ?7)
                 ORDER BY ts_ms",
            )?
            .query_map(params![dial, first - 1000, last + 1000, client, ccrc, rcrc, sid], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let hint = match rcrc {
            Some(crc) => Some(crate::naming::resolve(c, crc as u16, Some(dial))?),
            None => None,
        };
        let eid = new_id();
        let ev = json!({
            "type": "attempt", "id": eid, "start": iso(first), "end": iso(last), "modem": "mercury",
            "dialHz": dial, "from": client,
            "calledCrc": rcrc.map(|x| format!("{x:04X}")), "callerCrc": ccrc.map(|x| format!("{x:04X}")),
            "accepted": acc.is_some(), "calls": ncall,
            "bestSnrDb": if snr > -99.0 { json!(snr) } else { Value::Null },
            "frames": frames, "hint": hint,
        });
        enqueue(c, &eid, &ev)?;
        c.execute("UPDATE attempts SET uploaded=1, event_id=?2 WHERE id=?1", params![id, eid])?;
    }
    Ok(n)
}

fn proc_cpu_s() -> f64 {
    let Ok(s) = std::fs::read_to_string("/proc/self/stat") else { return 0.0 };
    let Some(rest) = s.rsplit_once(')').map(|x| x.1) else { return 0.0 };
    let f: Vec<&str> = rest.split_whitespace().collect();
    let tick = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    (f.get(11).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0) + f.get(12).and_then(|x| x.parse::<f64>().ok()).unwrap_or(0.0)) / tick
}

pub struct UploaderCfg {
    pub db: PathBuf,
    pub directory_url: Option<String>,
    pub enabled: bool,
    pub token: Option<String>,
    pub interval: Duration,
    pub window_s: i64,
    pub sdr_label: String,
    pub decoders: usize,
    pub ppm: f64,
}

/// Background loop: callsign list refresh (daily) and, if contributing, uploads.
pub fn run(cfg: UploaderCfg, stop: Arc<AtomicBool>) {
    let c = match db::open(&cfg.db) {
        Ok(c) => c,
        Err(e) => return warn!("uploader: {e:#}"),
    };
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(30)).user_agent(SOFTWARE).build();
    let started = Instant::now();
    let (mut last_cpu, mut last_cpu_t) = (proc_cpu_s(), Instant::now());
    let mut next_send = Instant::now() + Duration::from_secs(5);
    let mut next_status = Instant::now() + Duration::from_secs(10);
    let mut next_calls = Instant::now();
    let mut backoff = cfg.interval;
    let _ = kv_set(&c, "contribute_enabled", if cfg.enabled { "1" } else { "0" });
    while !stop.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(500));
        let now = Instant::now();
        if let Some(url) = &cfg.directory_url {
            if now >= next_calls {
                match refresh_callsigns(&c, &agent, url) {
                    Ok(Some(n)) => info!("callsign list: {n} callsigns from {url}"),
                    Ok(None) => {}
                    Err(e) => warn!("callsign list: {e:#}"),
                }
                next_calls = now + Duration::from_secs(24 * 3600);
            }
        }
        if !cfg.enabled {
            continue;
        }
        let (Some(url), Some(token)) = (&cfg.directory_url, &cfg.token) else { continue };
        if now >= next_status {
            let cpu = proc_cpu_s();
            let pct = 100.0 * (cpu - last_cpu) / last_cpu_t.elapsed().as_secs_f64().max(1.0);
            (last_cpu, last_cpu_t) = (cpu, Instant::now());
            let id = new_id();
            let ev = json!({
                "type": "status", "id": id, "ts": iso(now_ms()), "software": SOFTWARE,
                "os": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
                "sdr": cfg.sdr_label, "decoders": cfg.decoders, "cpuPct": (pct * 10.0).round() / 10.0,
                "calibrated": cfg.ppm != 0.0, "ppm": cfg.ppm, "uptimeS": started.elapsed().as_secs(),
            });
            let _ = enqueue(&c, &id, &ev);
            next_status = now + Duration::from_secs(300);
        }
        if now < next_send {
            continue;
        }
        if let Err(e) = queue_attempts(&c, cfg.window_s) {
            warn!("uploader: queueing attempts: {e:#}");
        }
        match send_batch(&c, &agent, url, token) {
            Ok(Sent::Done(n)) => {
                if n > 0 {
                    info!("uploaded {n} events");
                }
                backoff = cfg.interval;
                next_send = now + cfg.interval;
            }
            Ok(Sent::Retry(after)) => next_send = now + after,
            Ok(Sent::Rejected(why)) => {
                warn!("uploader: {why}; retrying in 5 min");
                next_send = now + Duration::from_secs(300);
            }
            Err(e) => {
                let _ = kv_set(&c, "upload_error", &format!("{e:#}"));
                warn!("uploader: {e:#}");
                backoff = (backoff * 2).min(Duration::from_secs(600));
                next_send = now + backoff;
            }
        }
    }
}

enum Sent {
    Done(usize),
    Retry(Duration),
    Rejected(String),
}

fn send_batch(c: &Connection, agent: &ureq::Agent, url: &str, token: &str) -> Result<Sent> {
    let rows: Vec<(String, String)> = c
        .prepare("SELECT id, body FROM outbox WHERE sent_ms IS NULL ORDER BY created_ms LIMIT 500")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    if rows.is_empty() {
        let _ = kv_set(c, "upload_last_ok", &now_ms().to_string());
        return Ok(Sent::Done(0));
    }
    let body: String = rows.iter().map(|(_, b)| format!("{b}\n")).collect();
    let resp = agent
        .post(&format!("{}/api/v1/watch/ingest", url.trim_end_matches('/')))
        .set("x-api-key", token)
        .set("content-type", "application/x-ndjson")
        .send_string(&body);
    match resp {
        Ok(r) => {
            let v: Value = r.into_json().context("ingest response")?;
            let rejected: HashMap<String, String> = v["rejected"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| Some((x["id"].as_str()?.to_string(), x["error"].as_str().unwrap_or("").to_string())))
                        .collect()
                })
                .unwrap_or_default();
            let t = c.unchecked_transaction()?;
            let now = now_ms();
            for (id, _) in &rows {
                t.execute("UPDATE outbox SET sent_ms=?2, error=?3 WHERE id=?1", params![id, now, rejected.get(id)])?;
            }
            t.commit()?;
            for (id, why) in rejected.iter().take(3) {
                warn!("uploader: server rejected {id}: {why}");
            }
            kv_set(c, "upload_last_ok", &now.to_string())?;
            kv_set(c, "upload_error", "")?;
            Ok(Sent::Done(rows.len()))
        }
        Err(ureq::Error::Status(429, r)) => {
            let after = r.header("retry-after").and_then(|s| s.parse().ok()).unwrap_or(60);
            Ok(Sent::Retry(Duration::from_secs(after)))
        }
        Err(ureq::Error::Status(code @ (401 | 403), _)) => {
            let why = if code == 401 { "token rejected (revoked, rotated or wrong key)" } else { "watcher disabled by a moderator" };
            kv_set(c, "upload_error", why)?;
            Ok(Sent::Rejected(why.into()))
        }
        Err(ureq::Error::Status(413, _)) => {
            anyhow::bail!("batch too large (413)")
        }
        Err(e) => Err(e.into()),
    }
}

/// Fetch /api/v1/watch/callsigns if it changed. Some(n) when updated.
fn refresh_callsigns(c: &Connection, agent: &ureq::Agent, url: &str) -> Result<Option<usize>> {
    let mut req = agent.get(&format!("{}/api/v1/watch/callsigns", url.trim_end_matches('/')));
    if let Some(etag) = kv_get(c, "callsigns_etag") {
        req = req.set("if-none-match", &etag);
    }
    let r = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(304, _)) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if r.status() == 304 {
        return Ok(None);
    }
    let etag = r.header("etag").map(String::from);
    let list: Vec<Value> = r.into_json()?;
    let t = c.unchecked_transaction()?;
    t.execute("DELETE FROM directory_calls", [])?;
    let mut n = 0;
    for e in &list {
        let (Some(call), Some(crc)) = (e["callsign"].as_str(), e["crc"].as_str()) else { continue };
        let Ok(crc) = u16::from_str_radix(crc, 16) else { continue };
        t.execute(
            "INSERT OR REPLACE INTO directory_calls(callsign,crc,dial_json,modems,source) VALUES(?1,?2,?3,?4,?5)",
            params![crate::crc::normalise(call), crc as i64, e["dialHz"].to_string(), e["modems"].to_string(),
                    e["source"].as_str().unwrap_or("")],
        )?;
        n += 1;
    }
    t.commit()?;
    if let Some(etag) = etag {
        kv_set(c, "callsigns_etag", &etag)?;
    }
    kv_set(c, "callsigns_updated", &now_ms().to_string())?;
    Ok(Some(n))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nulls_are_dropped_not_sent() {
        let v = strip_nulls(&json!({"a": 1, "b": null, "hint": {"callsign": null, "match": "unknown"}}));
        assert_eq!(v, json!({"a": 1, "hint": {"match": "unknown"}}));
    }

    #[test]
    fn iso_format() {
        assert_eq!(iso(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso(1_795_642_572_466), "2026-11-25T21:36:12.466Z");
        assert_eq!(iso(951_782_400_000), "2000-02-29T00:00:00.000Z");
    }
}
