//! Parse Mercury's `MONITOR <mode> <kind> <detail> SNR=<snr>` control-port lines.
//!
//! The CRC fields (`TO_CRC`, `CALLER_CRC`, `CALLED_CRC`) come from
//! `patches/mercury-monitor-crc.patch`; an unpatched Mercury omits them and the
//! watcher still logs the frame, just without a way to name the RMS.

use serde::Serialize;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonitorFrame {
    pub mode: String,
    /// CALL, ACCEPT, CQ, DISCONNECT, STATION, BCAST_CTRL, BCAST_DATA, CONTROL, DATA, FRAME
    pub kind: String,
    pub from: Option<String>,
    pub to_crc: Option<u16>,
    pub caller_crc: Option<u16>,
    pub called_crc: Option<u16>,
    pub sid: Option<u8>,
    pub bw: Option<u32>,
    pub start_level: Option<u8>,
    pub mfsk_tx: bool,
    /// "another protocol version" frames (Mercury 1.9.x).
    pub legacy: bool,
    pub snr: Option<f32>,
    pub raw: String,
}

fn hex16(v: Option<&&str>) -> Option<u16> {
    v.and_then(|s| u16::from_str_radix(s, 16).ok())
}

pub fn parse_line(line: &str) -> Option<MonitorFrame> {
    let line = line.trim();
    let rest = line.strip_prefix("MONITOR ")?;
    let mut parts = rest.splitn(3, ' ');
    let mode = parts.next()?.to_string();
    let kind = parts.next()?.to_string();
    let detail = parts.next().unwrap_or("");

    let mut kv: HashMap<&str, &str> = HashMap::new();
    let mut flags = Vec::new();
    for tok in detail.split_whitespace() {
        match tok.split_once('=') {
            Some((k, v)) => {
                kv.insert(k, v);
            }
            None => flags.push(tok),
        }
    }
    let from = kv
        .get("FROM")
        .filter(|s| **s != "?")
        .map(|s| crate::crc::normalise(s));
    Some(MonitorFrame {
        mode,
        kind,
        from,
        to_crc: hex16(kv.get("TO_CRC")),
        caller_crc: hex16(kv.get("CALLER_CRC")),
        called_crc: hex16(kv.get("CALLED_CRC")),
        sid: kv.get("SID").and_then(|s| s.parse().ok()),
        bw: kv.get("BW").and_then(|s| s.parse().ok()),
        start_level: kv.get("START").and_then(|s| s.parse().ok()),
        mfsk_tx: flags.contains(&"MFSK-TX"),
        legacy: detail.contains("another protocol version"),
        snr: kv.get("SNR").and_then(|s| s.parse().ok()),
        raw: line.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Lines exactly as the patched Mercury printed them for the fixture.
    #[test]
    fn call() {
        let f = parse_line("MONITOR DATAC16 CALL FROM=Q1TEST TO_CRC=BD24 BW=2300 SID=125 SNR=23.8").unwrap();
        assert_eq!(f.kind, "CALL");
        assert_eq!(f.from.as_deref(), Some("Q1TEST"));
        assert_eq!(f.to_crc, Some(0xBD24));
        assert_eq!(f.sid, Some(125));
        assert_eq!(f.bw, Some(2300));
        assert_eq!(f.snr, Some(23.8));
    }

    #[test]
    fn accept() {
        let f = parse_line(
            "MONITOR DATAC16 ACCEPT BW=2300 SID=125 START=6 CALLER_CRC=9591 CALLED_CRC=BD24 SNR=24.1",
        )
        .unwrap();
        assert_eq!(f.kind, "ACCEPT");
        assert_eq!(f.caller_crc, Some(0x9591));
        assert_eq!(f.called_crc, Some(0xBD24));
        assert_eq!(f.start_level, Some(6));
        assert_eq!(f.from, None);
    }

    #[test]
    fn unpatched_accept_and_legacy() {
        let f = parse_line("MONITOR DATAC16 ACCEPT BW=2300 SID=42 START=3 SNR=17.9").unwrap();
        assert_eq!(f.called_crc, None);
        let l = parse_line("MONITOR DATAC16 CALL another protocol version (Mercury 1.9.x?) SNR=3.0").unwrap();
        assert!(l.legacy);
        assert_eq!(l.snr, Some(3.0));
    }

    #[test]
    fn cq_and_noise() {
        let f = parse_line("MONITOR DATAC16 CQ FROM=q3cq BW=500 SNR=-3.5").unwrap();
        assert_eq!(f.from.as_deref(), Some("Q3CQ"));
        assert!(parse_line("PTT ON").is_none());
        assert!(parse_line("MONITOR DATAC16 CALL FROM=? SNR=1.0").unwrap().from.is_none());
    }
}
