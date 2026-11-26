# Mercury patches

`mercury-monitor-crc.patch` applies to Rhizomatica's Mercury
(https://github.com/Rhizomatica/mercury, GPL-3.0-or-later) at the commit in
`MERCURY_COMMIT`. It is a derivative of Mercury and is licensed **GPL-3.0-or-later**,
like Mercury itself, not under this repository's AGPL.

It adds what openrms-watch needs from Mercury's passive monitor:

- CRC fields on `MONITOR` lines (`TO_CRC` on CALL, `CALLER_CRC`/`CALLED_CRC` on ACCEPT),
  so the RMS behind a call can be named.
- MFSK500 in the monitor (calls from 500 Hz stations escalate to it).
- `MERCURY_MONITOR_CONNECT_ONLY=1`: decode only the carriers connect frames use.
- `MERCURY_BIND_ADDR=127.0.0.1`: keep the TNC ports off the network.

openrms-watch never links Mercury. `scripts/build-mercury.sh` builds the patched Mercury
as a separate program, and the watcher runs it as a child process and talks to it over
localhost TCP. These changes are intended to be offered upstream.
