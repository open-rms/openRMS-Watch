//! CRC16-CCITT of a callsign, exactly as Mercury puts it in CALL/ACCEPT frames.
//!
//! Mercury (`arq_protocol_callsign_crc16`) uppercases the callsign, keeps the
//! SSID, and runs codec2's `freedv_gen_crc16`: CRC-16/CCITT-FALSE (poly 0x1021,
//! init 0xFFFF, no reflection, no final xor). Reimplemented here so nothing
//! links Mercury's GPL code.

pub fn callsign_crc16(callsign: &str) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for b in callsign.trim().to_ascii_uppercase().bytes() {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    crc
}

/// Normalise a callsign for storage: trimmed, uppercase.
pub fn normalise(callsign: &str) -> String {
    callsign.trim().to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vectors taken from a real on-air exchange recorded from Mercury
    /// (tests/fixtures/q1test-q1rms10.s16): the CALL's TO_CRC and the
    /// ACCEPT's CALLER_CRC as Mercury's own monitor printed them.
    #[test]
    fn matches_mercury_on_air_frames() {
        assert_eq!(callsign_crc16("Q1RMS-10"), 0xBD24);
        assert_eq!(callsign_crc16("Q1TEST"), 0x9591);
    }

    #[test]
    fn case_insensitive_like_mercury() {
        assert_eq!(callsign_crc16("q1rms-10"), callsign_crc16("Q1RMS-10"));
    }

    #[test]
    fn ssid_matters() {
        assert_ne!(callsign_crc16("Q1RMS"), callsign_crc16("Q1RMS-10"));
    }
}
