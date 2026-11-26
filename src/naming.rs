//! Name the station behind a Mercury CRC16, the same way openRMS-Atlas does
//! (openRMS-Spec/watch/feed.md section 3), the same rules openRMS-Atlas uses.
//!
//! CRC16 collisions are certain at the size of the gateway list, so a CRC maps to
//! candidates: callsigns we heard in clear plus openRMS-Atlas's list. They are
//! narrowed by (1) a listed Mercury channel within 200 Hz of the heard dial,
//! (2) Mercury-capable gateways over others, (3) callsigns heard in clear.
//! A station heard calling CQ under that CRC wins outright ("heard").

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Hint {
    pub callsign: Option<String>,
    /// heard | inferred | ambiguous | unknown
    #[serde(rename = "match")]
    pub match_: &'static str,
    pub candidates: Vec<String>,
}

const NEAR_HZ: i64 = 200;

struct Cand {
    call: String,
    dials: Vec<i64>,
    mercury: bool,
    in_clear: bool,
    cq: bool,
}

pub fn resolve(c: &Connection, crc: u16, dial_hz: Option<i64>) -> Result<Hint> {
    let mut cands: Vec<Cand> = Vec::new();
    {
        let mut st = c.prepare("SELECT callsign, source FROM known_calls WHERE crc=?1")?;
        let mut rows = st.query(params![crc as i64])?;
        while let Some(r) = rows.next()? {
            let call: String = r.get(0)?;
            let src: String = r.get(1)?;
            cands.push(Cand {
                call,
                dials: vec![],
                mercury: false,
                in_clear: src.starts_with("heard"),
                cq: src == "heard-cq",
            });
        }
    }
    {
        let mut st = c.prepare("SELECT callsign, dial_json, modems FROM directory_calls WHERE crc=?1")?;
        let mut rows = st.query(params![crc as i64])?;
        while let Some(r) = rows.next()? {
            let call: String = r.get(0)?;
            let dials: Vec<i64> = serde_json::from_str(&r.get::<_, String>(1)?).unwrap_or_default();
            let modems: Vec<String> = serde_json::from_str(&r.get::<_, String>(2)?).unwrap_or_default();
            let mercury = modems.iter().any(|m| m == "mercury");
            match cands.iter_mut().find(|x| x.call == call) {
                Some(x) => {
                    x.dials = dials;
                    x.mercury = mercury;
                }
                None => cands.push(Cand { call, dials, mercury, in_clear: false, cq: false }),
            }
        }
    }
    let all: Vec<String> = {
        let mut v: Vec<String> = cands.iter().map(|x| x.call.clone()).collect();
        v.sort();
        v
    };
    if let Some(x) = cands.iter().find(|x| x.cq) {
        return Ok(Hint { callsign: Some(x.call.clone()), match_: "heard", candidates: all });
    }
    let mut keep: Vec<&Cand> = cands.iter().collect();
    let narrow = |keep: &mut Vec<&Cand>, f: &dyn Fn(&Cand) -> bool| {
        if keep.len() > 1 && keep.iter().any(|x| f(x)) {
            keep.retain(|x| f(x));
        }
    };
    if let Some(d) = dial_hz {
        narrow(&mut keep, &|x: &Cand| x.dials.iter().any(|l| (l - d).abs() <= NEAR_HZ));
    }
    narrow(&mut keep, &|x: &Cand| x.mercury);
    narrow(&mut keep, &|x: &Cand| x.in_clear);
    let mut left: Vec<String> = keep.iter().map(|x| x.call.clone()).collect();
    left.sort();
    Ok(match left.len() {
        0 => Hint { callsign: None, match_: "unknown", candidates: all },
        1 => Hint { callsign: Some(left[0].clone()), match_: "inferred", candidates: left },
        _ => Hint { callsign: None, match_: "ambiguous", candidates: left },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crc::callsign_crc16;

    fn db() -> Connection {
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(crate::db::SCHEMA).unwrap();
        crate::db::migrate(&c).unwrap();
        c
    }

    fn dir(c: &Connection, call: &str, crc: u16, dials: &str, modems: &str) {
        c.execute(
            "INSERT INTO directory_calls(callsign,crc,dial_json,modems,source) VALUES(?1,?2,?3,?4,'openrms')",
            params![call, crc as i64, dials, modems],
        )
        .unwrap();
    }

    #[test]
    fn narrows_by_frequency_then_mercury() {
        let c = db();
        let crc = callsign_crc16("Q1RMS-10");
        // pretend two other gateways share the CRC (collisions are real at list size)
        dir(&c, "Q1RMS-10", crc, "[7101500]", r#"["mercury"]"#);
        dir(&c, "Q9XXX-5", crc, "[14105000]", r#"["mercury"]"#);
        dir(&c, "Q2YYY", crc, "[]", r#"["vara-hf"]"#);
        let h = resolve(&c, crc, Some(7_101_340)).unwrap();
        assert_eq!(h.callsign.as_deref(), Some("Q1RMS-10"));
        assert_eq!(h.match_, "inferred");
        // off any listed channel: Mercury-capable ones remain, still two -> ambiguous
        let h = resolve(&c, crc, Some(7_200_000)).unwrap();
        assert_eq!(h.match_, "ambiguous");
        assert_eq!(h.candidates, vec!["Q1RMS-10", "Q9XXX-5"]);
    }

    #[test]
    fn cq_heard_wins_and_unknown() {
        let c = db();
        let crc = callsign_crc16("Q1RMS-10");
        dir(&c, "Q9XXX-5", crc, "[7101500]", r#"["mercury"]"#);
        crate::db::add_known_call(&c, "Q1RMS-10", "heard-cq").unwrap();
        let h = resolve(&c, crc, Some(7_101_340)).unwrap();
        assert_eq!((h.callsign.as_deref(), h.match_), (Some("Q1RMS-10"), "heard"));
        assert_eq!(resolve(&c, 0x1234, None).unwrap().match_, "unknown");
    }
}
