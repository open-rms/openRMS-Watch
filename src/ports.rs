//! Pick free localhost ports for Mercury decoders (control, data, broadcast).

use anyhow::{bail, Result};
use std::net::TcpListener;

pub struct PortAlloc {
    next: u32,
    end: u32,
}

impl PortAlloc {
    /// Search [start, start + span).
    pub fn new(start: u16, span: u32) -> PortAlloc {
        PortAlloc { next: start as u32, end: (start as u32 + span).min(65535) }
    }

    fn free(p: u32) -> bool {
        TcpListener::bind(("127.0.0.1", p as u16)).is_ok()
    }

    /// Three consecutive free ports; returns the first.
    pub fn take3(&mut self) -> Result<u16> {
        while self.next + 2 < self.end {
            let p = self.next;
            self.next += 3;
            if (p..p + 3).all(Self::free) {
                return Ok(p as u16);
            }
        }
        bail!("no free ports left for decoders (raise base_port or free some ports)")
    }
}

/// Apply --port / OPENRMS_WATCH_PORT to a "host:port" listen address.
pub fn with_port(listen: &str, port: u16) -> String {
    match listen.rsplit_once(':') {
        Some((host, _)) => format!("{host}:{port}"),
        None => format!("{listen}:{port}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_busy_ports() {
        let busy = TcpListener::bind("127.0.0.1:0").unwrap();
        let p = busy.local_addr().unwrap().port();
        let mut a = PortAlloc::new(p - 1, 30);
        let got = a.take3().unwrap();
        assert!(!(got..got + 3).contains(&p), "allocated a busy port");
    }

    #[test]
    fn port_override() {
        assert_eq!(with_port("127.0.0.1:17373", 18000), "127.0.0.1:18000");
        assert_eq!(with_port("0.0.0.0:1", 5), "0.0.0.0:5");
    }
}
