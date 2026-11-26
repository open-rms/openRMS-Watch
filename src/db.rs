//! SQLite storage.
//!
//! `frames` is the append-only record of everything Mercury's monitor reported.
//! `attempts` (connection attempts) is derived from it and can be rebuilt at any
//! time (`openrms-watch rebuild`). Station status is computed at query time from
//! attempts, CQ frames, and listening `coverage`, never stored.

use crate::crc::{callsign_crc16, normalise};
use crate::monitor::MonitorFrame;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::Path;

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

pub(crate) const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
CREATE TABLE IF NOT EXISTS receivers(
  id TEXT PRIMARY KEY, name TEXT, kind TEXT, location TEXT, updated_ms INTEGER);
CREATE TABLE IF NOT EXISTS channels(
  id INTEGER PRIMARY KEY, receiver_id TEXT NOT NULL, dial_hz INTEGER NOT NULL,
  label TEXT, band TEXT, UNIQUE(receiver_id, dial_hz));
CREATE TABLE IF NOT EXISTS coverage(
  id INTEGER PRIMARY KEY, channel_id INTEGER NOT NULL, start_ms INTEGER NOT NULL, end_ms INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS coverage_ch ON coverage(channel_id, end_ms);
CREATE TABLE IF NOT EXISTS frames(
  id INTEGER PRIMARY KEY, ts_ms INTEGER NOT NULL, receiver_id TEXT, channel_id INTEGER,
  dial_hz INTEGER, shift_hz REAL, mode TEXT, kind TEXT, from_call TEXT,
  to_crc INTEGER, caller_crc INTEGER, called_crc INTEGER, sid INTEGER, bw INTEGER,
  start_level INTEGER, mfsk_tx INTEGER, legacy INTEGER, snr REAL, raw TEXT);
CREATE INDEX IF NOT EXISTS frames_ts ON frames(ts_ms);
CREATE INDEX IF NOT EXISTS frames_kind ON frames(kind, ts_ms);
CREATE TABLE IF NOT EXISTS known_calls(
  callsign TEXT PRIMARY KEY, crc INTEGER NOT NULL, source TEXT, added_ms INTEGER);
CREATE INDEX IF NOT EXISTS known_calls_crc ON known_calls(crc);
CREATE TABLE IF NOT EXISTS attempts(
  id INTEGER PRIMARY KEY, dial_hz INTEGER NOT NULL,
  first_ms INTEGER NOT NULL, last_ms INTEGER NOT NULL,
  client_call TEXT, client_crc INTEGER, rms_crc INTEGER, sid INTEGER, bw INTEGER,
  n_call INTEGER DEFAULT 0, n_accept INTEGER DEFAULT 0,
  last_call_ms INTEGER, last_accept_ms INTEGER, disconnect_ms INTEGER,
  snr_call REAL, snr_accept REAL, receivers TEXT DEFAULT '');
CREATE INDEX IF NOT EXISTS attempts_last ON attempts(last_ms);
CREATE INDEX IF NOT EXISTS attempts_rms ON attempts(rms_crc);
CREATE TABLE IF NOT EXISTS directory_calls(
  callsign TEXT PRIMARY KEY, crc INTEGER NOT NULL, dial_json TEXT, modems TEXT, source TEXT);
CREATE INDEX IF NOT EXISTS directory_calls_crc ON directory_calls(crc);
CREATE TABLE IF NOT EXISTS outbox(
  id TEXT PRIMARY KEY, body TEXT NOT NULL, created_ms INTEGER NOT NULL, sent_ms INTEGER, error TEXT);
CREATE INDEX IF NOT EXISTS outbox_unsent ON outbox(sent_ms, created_ms);
CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT);
"#;

/// Frequencies measured from the spectrum vary by a few Hz between bursts;
/// two dial readings within this are the same channel.
pub const SAME_DIAL_HZ: i64 = 80;

pub fn open(path: &Path) -> Result<Connection> {
    let c = Connection::open(path)?;
    c.busy_timeout(std::time::Duration::from_secs(5))?;
    c.execute_batch(SCHEMA)?;
    migrate(&c)?;
    Ok(c)
}

pub(crate) fn migrate(c: &Connection) -> Result<()> {
    let cols: Vec<String> = c
        .prepare("SELECT name FROM pragma_table_info('channels')")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    if !cols.iter().any(|n| n == "lo_hz") {
        c.execute_batch(
            "ALTER TABLE channels ADD COLUMN lo_hz INTEGER; ALTER TABLE channels ADD COLUMN hi_hz INTEGER;
             UPDATE channels SET lo_hz=dial_hz, hi_hz=dial_hz WHERE lo_hz IS NULL;",
        )?;
    }
    let has = |table: &str, col: &str| -> Result<bool> {
        let n: i64 = c.query_row(
            &format!("SELECT count(*) FROM pragma_table_info('{table}') WHERE name=?1"),
            [col],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    };
    if !has("frames", "event_id")? {
        c.execute_batch("ALTER TABLE frames ADD COLUMN event_id TEXT;")?;
    }
    if !has("attempts", "event_id")? {
        c.execute_batch("ALTER TABLE attempts ADD COLUMN event_id TEXT; ALTER TABLE attempts ADD COLUMN uploaded INTEGER DEFAULT 0;")?;
    }
    Ok(())
}

pub fn kv_get(c: &Connection, key: &str) -> Option<String> {
    c.query_row("SELECT value FROM kv WHERE key=?1", [key], |r| r.get(0)).ok()
}

pub fn kv_set(c: &Connection, key: &str, value: &str) -> Result<()> {
    c.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=?2", [key, value])?;
    Ok(())
}

pub fn upsert_receiver(c: &Connection, id: &str, name: &str, kind: &str, location: &str) -> Result<()> {
    c.execute(
        "INSERT INTO receivers(id,name,kind,location,updated_ms) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(id) DO UPDATE SET name=?2, kind=?3, location=?4, updated_ms=?5",
        params![id, name, kind, location, now_ms()],
    )?;
    Ok(())
}

pub fn upsert_channel(c: &Connection, receiver: &str, dial_hz: u64, label: &str) -> Result<i64> {
    upsert_range(c, receiver, dial_hz, dial_hz, label)
}

/// A scanned segment is stored as a channel keyed by its low edge.
pub fn upsert_range(c: &Connection, receiver: &str, lo_hz: u64, hi_hz: u64, label: &str) -> Result<i64> {
    let dial_hz = lo_hz;
    c.execute(
        "INSERT INTO channels(receiver_id,dial_hz,label,band,lo_hz,hi_hz) VALUES(?1,?2,?3,?4,?5,?6)
         ON CONFLICT(receiver_id,dial_hz) DO UPDATE SET label=?3, band=?4, lo_hz=?5, hi_hz=?6",
        params![receiver, dial_hz as i64, label, crate::config::band_of((lo_hz + hi_hz) / 2), lo_hz as i64, hi_hz as i64],
    )?;
    Ok(c.query_row(
        "SELECT id FROM channels WHERE receiver_id=?1 AND dial_hz=?2",
        params![receiver, dial_hz as i64],
        |r| r.get(0),
    )?)
}

/// Add a callsign to the CRC lookup list. Returns true if new.
pub fn add_known_call(c: &Connection, call: &str, source: &str) -> Result<bool> {
    let call = normalise(call);
    if call.is_empty() || call.starts_with('#') {
        return Ok(false);
    }
    let n = c.execute(
        "INSERT OR IGNORE INTO known_calls(callsign,crc,source,added_ms) VALUES(?1,?2,?3,?4)",
        params![call, callsign_crc16(&call) as i64, source, now_ms()],
    )?;
    Ok(n > 0)
}

pub fn import_calls_file(c: &Connection, path: &Path) -> Result<usize> {
    let text = std::fs::read_to_string(path)?;
    let tx = c.unchecked_transaction()?;
    let mut n = 0;
    for line in text.lines() {
        // first whitespace/comma-separated token; '#' starts a comment
        let line = line.split('#').next().unwrap_or("");
        if let Some(tok) = line.split(|ch: char| ch.is_whitespace() || ch == ',').find(|t| !t.is_empty()) {
            if add_known_call(&tx, tok, "file")? {
                n += 1;
            }
        }
    }
    tx.commit()?;
    Ok(n)
}

/// One decoded frame as it came off a channel.
pub struct Heard {
    pub ts_ms: i64,
    pub receiver_id: String,
    pub channel_id: i64,
    pub dial_hz: u64,
    pub shift_hz: f64,
    /// "scan" (measured from the spectrum) or "channel" (a configured frequency)
    pub freq_source: &'static str,
    pub frame: MonitorFrame,
}

/// Store a frame; returns its event id (ULID), used when uploading.
pub fn insert_frame(c: &Connection, h: &Heard) -> Result<String> {
    let f = &h.frame;
    let eid = ulid::Ulid::from_datetime(
        std::time::UNIX_EPOCH + std::time::Duration::from_millis(h.ts_ms.max(0) as u64),
    )
    .to_string();
    c.execute(
        "INSERT INTO frames(ts_ms,receiver_id,channel_id,dial_hz,shift_hz,mode,kind,from_call,
           to_crc,caller_crc,called_crc,sid,bw,start_level,mfsk_tx,legacy,snr,raw,event_id)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
        params![
            h.ts_ms, h.receiver_id, h.channel_id, h.dial_hz as i64, h.shift_hz, f.mode, f.kind, f.from,
            f.to_crc, f.caller_crc, f.called_crc, f.sid, f.bw, f.start_level, f.mfsk_tx, f.legacy, f.snr, f.raw, eid
        ],
    )?;
    Ok(eid)
}

/// The same transmission heard by two receivers or two shifted decoders
/// arrives within this many ms; count it once.
const DUP_MS: i64 = 2500;
/// A DISCONNECT closes a session that was accepted up to this long ago.
const SESSION_MAX_MS: i64 = 3 * 3600 * 1000;

fn add_receiver(list: &str, r: &str) -> String {
    if list.split(',').any(|x| x == r) {
        list.to_string()
    } else if list.is_empty() {
        r.to_string()
    } else {
        format!("{list},{r}")
    }
}

fn fmax(a: Option<f64>, b: Option<f32>) -> Option<f64> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b as f64)),
        (a, None) => a,
        (None, Some(b)) => Some(b as f64),
    }
}

/// Fold one frame into the derived `attempts` table (and learn callsigns).
pub fn apply_frame(c: &Connection, h: &Heard, window_s: i64) -> Result<()> {
    let f = &h.frame;
    if f.legacy {
        return Ok(());
    }
    let dial = h.dial_hz as i64;
    let since = h.ts_ms - window_s * 1000;
    match f.kind.as_str() {
        "CALL" => {
            let Some(from) = f.from.as_deref() else { return Ok(()) };
            add_known_call(c, from, "heard-call")?;
            let ccrc = callsign_crc16(from) as i64;
            let row: Option<(i64, Option<i64>, Option<f64>, String)> = c
                .query_row(
                    "SELECT id,last_call_ms,snr_call,receivers FROM attempts
                     WHERE abs(dial_hz-?1)<=80 AND client_crc=?2 AND rms_crc IS ?3 AND last_ms>=?4
                     ORDER BY last_ms DESC LIMIT 1",
                    params![dial, ccrc, f.to_crc, since],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            match row {
                Some((id, last_call, snr, recv)) => {
                    let dup = last_call.is_some_and(|t| (h.ts_ms - t).abs() < DUP_MS);
                    c.execute(
                        "UPDATE attempts SET last_ms=max(last_ms,?2), last_call_ms=max(coalesce(last_call_ms,0),?2),
                           n_call=n_call+?3, snr_call=?4, receivers=?5, client_call=?6, sid=coalesce(?7,sid), bw=coalesce(?8,bw)
                         WHERE id=?1",
                        params![id, h.ts_ms, if dup { 0 } else { 1 }, fmax(snr, f.snr),
                                add_receiver(&recv, &h.receiver_id), from, f.sid, f.bw],
                    )?;
                }
                None => {
                    c.execute(
                        "INSERT INTO attempts(dial_hz,first_ms,last_ms,client_call,client_crc,rms_crc,sid,bw,
                           n_call,last_call_ms,snr_call,receivers)
                         VALUES(?1,?2,?2,?3,?4,?5,?6,?7,1,?2,?8,?9)",
                        params![dial, h.ts_ms, from, ccrc, f.to_crc, f.sid, f.bw, f.snr, h.receiver_id],
                    )?;
                }
            }
        }
        "ACCEPT" => {
            // Patched Mercury: match on both CRCs. Unpatched: fall back to SID.
            let row: Option<(i64, Option<i64>, Option<f64>, String)> = if let (Some(cc), Some(rc)) = (f.caller_crc, f.called_crc) {
                c.query_row(
                    "SELECT id,last_accept_ms,snr_accept,receivers FROM attempts
                     WHERE abs(dial_hz-?1)<=80 AND client_crc=?2 AND rms_crc=?3 AND last_ms>=?4
                     ORDER BY last_ms DESC LIMIT 1",
                    params![dial, cc, rc, since],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?
            } else {
                c.query_row(
                    "SELECT id,last_accept_ms,snr_accept,receivers FROM attempts
                     WHERE abs(dial_hz-?1)<=80 AND sid IS ?2 AND last_ms>=?3 ORDER BY last_ms DESC LIMIT 1",
                    params![dial, f.sid, since],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?
            };
            match row {
                Some((id, last_acc, snr, recv)) => {
                    let dup = last_acc.is_some_and(|t| (h.ts_ms - t).abs() < DUP_MS);
                    c.execute(
                        "UPDATE attempts SET last_ms=max(last_ms,?2), last_accept_ms=max(coalesce(last_accept_ms,0),?2),
                           n_accept=n_accept+?3, snr_accept=?4, receivers=?5, sid=coalesce(sid,?6), bw=coalesce(bw,?7)
                         WHERE id=?1",
                        params![id, h.ts_ms, if dup { 0 } else { 1 }, fmax(snr, f.snr),
                                add_receiver(&recv, &h.receiver_id), f.sid, f.bw],
                    )?;
                }
                None => {
                    // Heard the RMS answer without hearing the caller.
                    c.execute(
                        "INSERT INTO attempts(dial_hz,first_ms,last_ms,client_crc,rms_crc,sid,bw,
                           n_accept,last_accept_ms,snr_accept,receivers)
                         VALUES(?1,?2,?2,?3,?4,?5,?6,1,?2,?7,?8)",
                        params![dial, h.ts_ms, f.caller_crc, f.called_crc, f.sid, f.bw, f.snr, h.receiver_id],
                    )?;
                }
            }
        }
        "DISCONNECT" => {
            c.execute(
                "UPDATE attempts SET disconnect_ms=?3, last_ms=max(last_ms,?3)
                 WHERE id=(SELECT id FROM attempts WHERE abs(dial_hz-?1)<=80 AND sid IS ?2 AND disconnect_ms IS NULL
                           AND last_ms>=?4 ORDER BY last_ms DESC LIMIT 1)",
                params![dial, f.sid, h.ts_ms, h.ts_ms - SESSION_MAX_MS],
            )?;
        }
        "CQ" => {
            if let Some(from) = f.from.as_deref() {
                add_known_call(c, from, "heard-cq")?;
            }
        }
        _ => {}
    }
    Ok(())
}

pub fn record_coverage(c: &Connection, channel_id: i64, start_ms: i64, end_ms: i64) -> Result<()> {
    // Extend the channel's latest interval if it touches this one.
    let n = c.execute(
        "UPDATE coverage SET end_ms=?3 WHERE id=(SELECT id FROM coverage WHERE channel_id=?1
           ORDER BY end_ms DESC LIMIT 1) AND end_ms>=?2-5000",
        params![channel_id, start_ms, end_ms],
    )?;
    if n == 0 {
        c.execute(
            "INSERT INTO coverage(channel_id,start_ms,end_ms) VALUES(?1,?2,?3)",
            params![channel_id, start_ms, end_ms],
        )?;
    }
    Ok(())
}

/// Recompute `attempts` from `frames` (after changing the window, or a bug fix).
pub fn rebuild(c: &Connection, window_s: i64) -> Result<usize> {
    let tx = c.unchecked_transaction()?;
    tx.execute("DELETE FROM attempts", [])?;
    let mut n = 0;
    {
        let mut st = tx.prepare(
            "SELECT ts_ms,receiver_id,channel_id,dial_hz,shift_hz,raw FROM frames ORDER BY ts_ms, id",
        )?;
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?,
                r.get::<_, f64>(4)?, r.get::<_, String>(5)?))
        })?;
        for row in rows {
            let (ts, recv, ch, dial, shift, raw) = row?;
            if let Some(frame) = crate::monitor::parse_line(&raw) {
                let h = Heard { ts_ms: ts, receiver_id: recv, channel_id: ch, dial_hz: dial as u64, shift_hz: shift, freq_source: "channel", frame };
                apply_frame(&tx, &h, window_s)?;
                n += 1;
            }
        }
    }
    // attempts from before a rebuild were already uploaded under other ids
    tx.execute("UPDATE attempts SET uploaded=1", [])?;
    tx.commit()?;
    Ok(n)
}

// ---------------------------------------------------------------- queries

#[derive(Serialize)]
pub struct Station {
    /// CRC16 of the callsign, hex: the identity a Mercury RMS has on the air.
    pub crc: String,
    /// Best name: a callsign heard in clear (CQ) or the single known match.
    pub callsign: Option<String>,
    /// heard (in clear on air) | inferred (one CRC match) | ambiguous | unknown
    pub name_confidence: &'static str,
    pub candidates: Vec<String>,
    /// active | stale | gone | unverified | called-only
    pub status: &'static str,
    pub first_ms: i64,
    pub last_heard_ms: Option<i64>,
    pub last_called_ms: Option<i64>,
    pub attempts: i64,
    pub accepted: i64,
    pub unanswered: i64,
    pub clients: i64,
    pub cq: i64,
    pub channels: Vec<i64>,
    pub bands: Vec<&'static str>,
    pub receivers: Vec<String>,
    pub best_snr: Option<f64>,
    /// Hours we were listening on its channels since it was last heard.
    pub listened_h_since: f64,
}

pub struct StatusPolicy {
    pub stale_hours: f64,
    pub gone_days: f64,
}

fn merged_coverage_ms(c: &Connection, dials: &[i64], since: i64, now: i64) -> Result<i64> {
    if dials.is_empty() {
        return Ok(0);
    }
    // a dial counts as covered by a fixed channel near it or a scanned segment holding it
    let q = format!(
        "SELECT max(cv.start_ms,?1), min(cv.end_ms,?2) FROM coverage cv JOIN channels ch ON ch.id=cv.channel_id
         WHERE cv.end_ms>?1 AND ({}) ORDER BY 1",
        dials
            .iter()
            .map(|d| format!("({d} BETWEEN coalesce(ch.lo_hz,ch.dial_hz)-{SAME_DIAL_HZ} AND coalesce(ch.hi_hz,ch.dial_hz)+{SAME_DIAL_HZ})"))
            .collect::<Vec<_>>()
            .join(" OR ")
    );
    let mut st = c.prepare(&q)?;
    let iv: Vec<(i64, i64)> = st
        .query_map(params![since, now], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    let (mut total, mut cur): (i64, Option<(i64, i64)>) = (0, None);
    for (s, e) in iv {
        if e <= s {
            continue;
        }
        cur = match cur {
            Some((cs, ce)) if s <= ce => Some((cs, ce.max(e))),
            Some((cs, ce)) => {
                total += ce - cs;
                Some((s, e))
            }
            None => Some((s, e)),
        };
    }
    if let Some((cs, ce)) = cur {
        total += ce - cs;
    }
    Ok(total)
}

pub fn stations(c: &Connection, pol: &StatusPolicy, now: i64) -> Result<Vec<Station>> {
    use std::collections::BTreeMap;
    struct Acc {
        first: i64,
        heard: Option<i64>,
        called: Option<i64>,
        attempts: i64,
        accepted: i64,
        unanswered: i64,
        clients: std::collections::BTreeSet<String>,
        cq: i64,
        cq_call: Option<String>,
        dials: std::collections::BTreeSet<i64>,
        receivers: std::collections::BTreeSet<String>,
        snr: Option<f64>,
    }
    let mut m: BTreeMap<i64, Acc> = BTreeMap::new();
    let new = |t: i64| Acc {
        first: t, heard: None, called: None, attempts: 0, accepted: 0, unanswered: 0,
        clients: Default::default(), cq: 0, cq_call: None, dials: Default::default(),
        receivers: Default::default(), snr: None,
    };
    {
        let mut st = c.prepare(
            "SELECT rms_crc,dial_hz,first_ms,last_call_ms,last_accept_ms,client_call,receivers,snr_accept
             FROM attempts WHERE rms_crc IS NOT NULL",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let crc: i64 = r.get(0)?;
            let first: i64 = r.get(2)?;
            let a = m.entry(crc).or_insert_with(|| new(first));
            a.first = a.first.min(first);
            a.attempts += 1;
            let called: Option<i64> = r.get(3)?;
            let acc: Option<i64> = r.get(4)?;
            if called.is_some() {
                a.called = a.called.max(called);
            }
            if let Some(t) = acc {
                a.accepted += 1;
                a.heard = a.heard.max(Some(t));
                a.dials.insert(r.get(1)?);
            } else {
                a.unanswered += 1;
                a.dials.insert(r.get(1)?);
            }
            if let Some(cl) = r.get::<_, Option<String>>(5)? {
                a.clients.insert(cl);
            }
            for rv in r.get::<_, String>(6)?.split(',').filter(|s| !s.is_empty()) {
                a.receivers.insert(rv.to_string());
            }
            if let Some(s) = r.get::<_, Option<f64>>(7)? {
                a.snr = Some(a.snr.map_or(s, |x: f64| x.max(s)));
            }
        }
    }
    {
        let mut st = c.prepare(
            "SELECT from_call, min(ts_ms), max(ts_ms), count(*), group_concat(DISTINCT dial_hz),
                    group_concat(DISTINCT receiver_id), max(snr)
             FROM frames WHERE kind='CQ' AND from_call IS NOT NULL GROUP BY from_call",
        )?;
        let mut rows = st.query([])?;
        while let Some(r) = rows.next()? {
            let call: String = r.get(0)?;
            let crc = callsign_crc16(&call) as i64;
            let first: i64 = r.get(1)?;
            let a = m.entry(crc).or_insert_with(|| new(first));
            a.first = a.first.min(first);
            a.heard = a.heard.max(Some(r.get(2)?));
            a.cq += r.get::<_, i64>(3)?;
            a.cq_call = Some(call);
            for d in r.get::<_, String>(4)?.split(',').filter_map(|s| s.parse().ok()) {
                a.dials.insert(d);
            }
            for rv in r.get::<_, String>(5)?.split(',').filter(|s| !s.is_empty()) {
                a.receivers.insert(rv.to_string());
            }
            if let Some(s) = r.get::<_, Option<f64>>(6)? {
                a.snr = Some(a.snr.map_or(s, |x: f64| x.max(s)));
            }
        }
    }

    let mut out = Vec::with_capacity(m.len());
    for (crc, a) in m {
        // nearby readings of one channel collapse to the first
        let mut dials: Vec<i64> = Vec::new();
        for d in a.dials {
            if dials.last().is_none_or(|l| d - l > SAME_DIAL_HZ) {
                dials.push(d);
            }
        }
        let hint = crate::naming::resolve(c, crc as u16, dials.first().copied())?;
        let (callsign, conf, candidates) = if let Some(cq) = &a.cq_call {
            (Some(cq.clone()), "heard", hint.candidates)
        } else {
            (hint.callsign, hint.match_, hint.candidates)
        };
        let (status, listened) = match a.heard {
            None => ("called-only", 0.0),
            Some(t) => {
                let age_h = (now - t) as f64 / 3.6e6;
                let listened_h = merged_coverage_ms(c, &dials, t, now)? as f64 / 3.6e6;
                let s = if age_h <= pol.stale_hours {
                    "active"
                } else if listened_h < pol.stale_hours {
                    "unverified"
                } else if age_h > pol.gone_days * 24.0 {
                    "gone"
                } else {
                    "stale"
                };
                (s, listened_h)
            }
        };
        let mut bands: Vec<&'static str> = dials.iter().map(|d| crate::config::band_of(*d as u64)).collect();
        bands.dedup();
        out.push(Station {
            crc: format!("{:04X}", crc),
            callsign,
            name_confidence: conf,
            candidates,
            status,
            first_ms: a.first,
            last_heard_ms: a.heard,
            last_called_ms: a.called,
            attempts: a.attempts,
            accepted: a.accepted,
            unanswered: a.unanswered,
            clients: a.clients.len() as i64,
            cq: a.cq,
            channels: dials,
            bands,
            receivers: a.receivers.into_iter().collect(),
            best_snr: a.snr,
            listened_h_since: (listened * 10.0).round() / 10.0,
        });
    }
    out.sort_by(|x, y| y.last_heard_ms.max(y.last_called_ms).cmp(&x.last_heard_ms.max(x.last_called_ms)));
    Ok(out)
}

#[derive(Serialize)]
pub struct Attempt {
    pub id: i64,
    pub dial_hz: i64,
    pub band: &'static str,
    pub first_ms: i64,
    pub last_ms: i64,
    pub client_call: Option<String>,
    pub client_crc: Option<String>,
    pub rms_crc: Option<String>,
    pub rms_call: Option<String>,
    pub rms_candidates: Vec<String>,
    /// heard | inferred | ambiguous | unknown
    pub rms_match: &'static str,
    pub sid: Option<i64>,
    pub bw: Option<i64>,
    pub n_call: i64,
    pub n_accept: i64,
    pub accepted: bool,
    pub disconnect_ms: Option<i64>,
    pub snr_call: Option<f64>,
    pub snr_accept: Option<f64>,
    pub receivers: Vec<String>,
}

pub fn attempts(c: &Connection, since_ms: i64, limit: i64) -> Result<Vec<Attempt>> {
    let mut st = c.prepare(
        "SELECT a.id,a.dial_hz,a.first_ms,a.last_ms,a.client_call,a.client_crc,a.rms_crc,a.sid,a.bw,
                a.n_call,a.n_accept,a.last_accept_ms,a.disconnect_ms,a.snr_call,a.snr_accept,a.receivers
         FROM attempts a WHERE a.last_ms>=?1 ORDER BY a.last_ms DESC LIMIT ?2",
    )?;
    let hex = |v: Option<i64>| v.map(|x| format!("{:04X}", x));
    let rows = st.query_map(params![since_ms, limit], |r| {
        let dial: i64 = r.get(1)?;
        Ok(Attempt {
            id: r.get(0)?,
            dial_hz: dial,
            band: crate::config::band_of(dial as u64),
            first_ms: r.get(2)?,
            last_ms: r.get(3)?,
            client_call: r.get(4)?,
            client_crc: hex(r.get(5)?),
            rms_crc: hex(r.get(6)?),
            rms_call: None,
            rms_candidates: vec![],
            rms_match: "unknown",
            sid: r.get(7)?,
            bw: r.get(8)?,
            n_call: r.get(9)?,
            n_accept: r.get(10)?,
            accepted: r.get::<_, Option<i64>>(11)?.is_some(),
            disconnect_ms: r.get(12)?,
            snr_call: r.get(13)?,
            snr_accept: r.get(14)?,
            receivers: r.get::<_, String>(15)?.split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
        })
    })?;
    let mut out: Vec<Attempt> = rows.collect::<rusqlite::Result<_>>()?;
    for a in out.iter_mut() {
        if let Some(crc) = a.rms_crc.as_deref().and_then(|h| u16::from_str_radix(h, 16).ok()) {
            let h = crate::naming::resolve(c, crc, Some(a.dial_hz))?;
            a.rms_call = h.callsign;
            a.rms_candidates = h.candidates;
            a.rms_match = h.match_;
        }
    }
    Ok(out)
}

#[derive(Serialize)]
pub struct FrameRow {
    pub id: i64,
    pub ts_ms: i64,
    pub receiver_id: Option<String>,
    pub dial_hz: Option<i64>,
    pub band: &'static str,
    pub shift_hz: Option<f64>,
    pub mode: Option<String>,
    pub kind: Option<String>,
    pub from_call: Option<String>,
    pub to_crc: Option<String>,
    pub caller_crc: Option<String>,
    pub called_crc: Option<String>,
    pub sid: Option<i64>,
    pub snr: Option<f64>,
    pub raw: String,
}

pub fn frames(c: &Connection, since_ms: i64, limit: i64) -> Result<Vec<FrameRow>> {
    let mut st = c.prepare(
        "SELECT id,ts_ms,receiver_id,dial_hz,shift_hz,mode,kind,from_call,to_crc,caller_crc,called_crc,sid,snr,raw
         FROM frames WHERE ts_ms>=?1 ORDER BY ts_ms DESC LIMIT ?2",
    )?;
    let hex = |v: Option<i64>| v.map(|x| format!("{:04X}", x));
    let rows = st.query_map(params![since_ms, limit], |r| {
        let dial: Option<i64> = r.get(3)?;
        Ok(FrameRow {
            id: r.get(0)?,
            ts_ms: r.get(1)?,
            receiver_id: r.get(2)?,
            dial_hz: dial,
            band: dial.map_or("other", |d| crate::config::band_of(d as u64)),
            shift_hz: r.get(4)?,
            mode: r.get(5)?,
            kind: r.get(6)?,
            from_call: r.get(7)?,
            to_crc: hex(r.get(8)?),
            caller_crc: hex(r.get(9)?),
            called_crc: hex(r.get(10)?),
            sid: r.get(11)?,
            snr: r.get(12)?,
            raw: r.get(13)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[derive(Serialize)]
pub struct ChannelRow {
    pub id: i64,
    pub receiver_id: String,
    pub receiver_name: Option<String>,
    pub receiver_kind: Option<String>,
    pub location: Option<String>,
    pub dial_hz: i64,
    pub band: Option<String>,
    pub label: Option<String>,
    pub lo_hz: Option<i64>,
    pub hi_hz: Option<i64>,
    pub listened_h_24: f64,
    pub last_coverage_ms: Option<i64>,
    pub frames_24: i64,
    pub last_frame_ms: Option<i64>,
}

pub fn channels(c: &Connection, now: i64) -> Result<Vec<ChannelRow>> {
    let day = now - 86_400_000;
    let mut st = c.prepare(
        "SELECT ch.id,ch.receiver_id,r.name,r.kind,r.location,ch.dial_hz,ch.band,ch.label,
           (SELECT coalesce(sum(min(end_ms,?1)-max(start_ms,?2)),0) FROM coverage WHERE channel_id=ch.id AND end_ms>?2),
           (SELECT max(end_ms) FROM coverage WHERE channel_id=ch.id),
           (SELECT count(*) FROM frames WHERE channel_id=ch.id AND ts_ms>?2),
           (SELECT max(ts_ms) FROM frames WHERE channel_id=ch.id),
           ch.lo_hz, ch.hi_hz
         FROM channels ch LEFT JOIN receivers r ON r.id=ch.receiver_id ORDER BY ch.dial_hz, ch.receiver_id",
    )?;
    let rows = st.query_map(params![now, day], |r| {
        Ok(ChannelRow {
            id: r.get(0)?,
            receiver_id: r.get(1)?,
            receiver_name: r.get(2)?,
            receiver_kind: r.get(3)?,
            location: r.get(4)?,
            dial_hz: r.get(5)?,
            band: r.get(6)?,
            label: r.get(7)?,
            listened_h_24: (r.get::<_, i64>(8)? as f64 / 3.6e6 * 10.0).round() / 10.0,
            last_coverage_ms: r.get(9)?,
            frames_24: r.get(10)?,
            last_frame_ms: r.get(11)?,
            lo_hz: r.get(12)?,
            hi_hz: r.get(13)?,
        })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::parse_line;

    fn heard(ts: i64, recv: &str, line: &str) -> Heard {
        Heard { ts_ms: ts, receiver_id: recv.into(), channel_id: 1, dial_hz: 7_101_000, shift_hz: 0.0, freq_source: "channel",
                frame: parse_line(line).unwrap() }
    }

    fn mem() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(SCHEMA).unwrap();
        migrate(&c).unwrap();
        c
    }

    fn ingest(c: &Connection, h: &Heard) {
        insert_frame(c, h).unwrap();
        apply_frame(c, h, 180).unwrap();
    }

    const CALL: &str = "MONITOR DATAC16 CALL FROM=Q1TEST TO_CRC=BD24 BW=2300 SID=127 SNR=24.0";
    const ACC: &str = "MONITOR DATAC16 ACCEPT BW=2300 SID=127 START=6 CALLER_CRC=9591 CALLED_CRC=BD24 SNR=24.4";
    const DISC: &str = "MONITOR DATAC16 DISCONNECT SID=127 SNR=25.0";

    #[test]
    fn call_accept_disconnect_is_one_attempt_and_resolves() {
        let c = mem();
        add_known_call(&c, "Q1RMS-10", "file").unwrap();
        let t = 1_000_000;
        ingest(&c, &heard(t, "a", CALL));
        ingest(&c, &heard(t + 500, "b", CALL)); // same frame, second receiver
        ingest(&c, &heard(t + 6_000, "a", CALL)); // a retry
        ingest(&c, &heard(t + 9_000, "a", ACC));
        ingest(&c, &heard(t + 60_000, "a", DISC));
        let a = attempts(&c, 0, 10).unwrap();
        assert_eq!(a.len(), 1);
        let a = &a[0];
        assert_eq!(a.n_call, 2, "dup across receivers counted once");
        assert!(a.accepted);
        assert_eq!(a.disconnect_ms, Some(t + 60_000));
        assert_eq!(a.rms_call.as_deref(), Some("Q1RMS-10"));
        assert_eq!(a.receivers, vec!["a", "b"]);

        let pol = StatusPolicy { stale_hours: 24.0, gone_days: 7.0 };
        let s = stations(&c, &pol, t + 3_600_000).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].callsign.as_deref(), Some("Q1RMS-10"));
        assert_eq!(s[0].name_confidence, "inferred");
        assert_eq!(s[0].status, "active");
    }

    #[test]
    fn stale_needs_coverage() {
        let c = mem();
        let ch = upsert_channel(&c, "a", 7_101_000, "").unwrap();
        let t = 1_000_000;
        ingest(&c, &heard(t, "a", CALL));
        ingest(&c, &heard(t + 9_000, "a", ACC));
        let pol = StatusPolicy { stale_hours: 24.0, gone_days: 7.0 };
        let later = t + 30 * 3_600_000;
        assert_eq!(stations(&c, &pol, later).unwrap()[0].status, "unverified");
        // listened the whole time since -> stale
        record_coverage(&c, ch, t, later).unwrap();
        assert_eq!(stations(&c, &pol, later).unwrap()[0].status, "stale");
        let much_later = t + 8 * 86_400_000;
        record_coverage(&c, ch, later, much_later).unwrap();
        assert_eq!(stations(&c, &pol, much_later).unwrap()[0].status, "gone");
    }

    #[test]
    fn unanswered_call_is_called_only_and_ambiguous_names() {
        let c = mem();
        ingest(&c, &heard(1000, "a", CALL));
        let pol = StatusPolicy { stale_hours: 24.0, gone_days: 7.0 };
        let s = stations(&c, &pol, 2000).unwrap();
        assert_eq!(s[0].status, "called-only");
        assert_eq!(s[0].name_confidence, "unknown");
        assert_eq!(s[0].unanswered, 1);
    }

    #[test]
    fn accept_without_call_and_rebuild() {
        let c = mem();
        insert_frame(&c, &heard(5000, "a", ACC)).unwrap();
        insert_frame(&c, &heard(1000, "a", CALL)).unwrap();
        // frames inserted out of order; rebuild replays by time
        rebuild(&c, 180).unwrap();
        let a = attempts(&c, 0, 10).unwrap();
        assert_eq!(a.len(), 1);
        assert!(a[0].accepted);
        assert_eq!(a[0].client_call.as_deref(), Some("Q1TEST"));
    }

    #[test]
    fn cq_names_station_in_clear() {
        let c = mem();
        ingest(&c, &heard(1000, "a", "MONITOR DATAC16 CQ FROM=Q1RMS-10 BW=2300 SNR=10.0"));
        ingest(&c, &heard(2000, "a", CALL));
        let pol = StatusPolicy { stale_hours: 24.0, gone_days: 7.0 };
        let s = stations(&c, &pol, 3000).unwrap();
        assert_eq!(s.len(), 1, "CQ and CALL target merge on CRC");
        assert_eq!(s[0].name_confidence, "heard");
        assert_eq!(s[0].status, "active");
    }
}
