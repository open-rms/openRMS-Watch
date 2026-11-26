# openRMS-Watch

**Provides:** `openrms-watch`, a passive monitor that logs Mercury connections heard on the
air (client callsign, RMS identity, frequency, time, SNR) to SQLite, and serves a filterable
web page and JSON/CSV API. · **Consumes:** a patched Mercury (separate process, TCP only),
SDRs through SoapySDR (or any IQ/audio command, e.g. KiwiSDR), and optionally the
openRMS-Atlas (callsign list, uploads; formerly openRMS-Atlas) and openRMS-Accounts (linking). · **Status:** alpha (verified on recorded and synthetic
audio; not yet heard a real Mercury station on air). · **License:** AGPL-3.0-or-later.

**Website and docs:** https://watch.openrms.dev (source: `docs/` and `site/`). Docs are
CC-BY-4.0 (`LICENSE-CC-BY-4.0`); code is AGPL-3.0-or-later (`LICENSE`).

## How it works

```
SDR / KiwiSDR command --s16le 8 kHz--> [ppm/offset shift] --> FIFO --> mercury -x fifo (MONITOR ON)
                                                                        |
                                    MONITOR lines over TCP <------------+
                                            |
                         SQLite: frames (append-only) -> attempts (derived) -> web page
```

- A Mercury CALL carries the **caller's callsign in clear** but only a **CRC16 of the called
  station**; the RMS's ACCEPT carries only CRCs. The patch in `patches/` makes Mercury's monitor
  print those CRCs. Stations are keyed by CRC and named from a callsign list (`calls_file`,
  plus callsigns heard in CALL and CQ frames). One match is shown as *inferred*, not proven.
- **Status** is computed at query time: *active* (the station itself transmitted within
  `stale_hours`), *stale* / *gone* (silent that long **while we were listening** on its
  channels), *unverified* (not enough listening to say), *called-only* (never heard answering).
- Repeats of one frame (CALL retries, several receivers, offset-search decoders) fold into one
  connection attempt. `openrms-watch rebuild` recomputes attempts from the raw frame log.
- Each Mercury gets a generated INI (PTT none), so a personal `~/.config/mercury/mercury.ini`
  is never read and nothing can key a radio.

## Band scanning (`[[wideband]]`)

One SDR captures whole band segments (e.g. a HackRF at 12.8 MS/s covers 40m, 30m and 20m at
once). Each segment is searched for DATAC16-shaped bursts (about 190 Hz wide, 3 s long, the
carrier every Mercury CALL/ACCEPT/CQ/DISCONNECT uses). A decoder from a pool is pointed at each
burst's measured centre, fed 5 s of buffered audio so the first CALL is not lost, and kept
on that frequency while it decodes. A frequency where the RMS itself is heard (ACCEPT or CQ) is
pinned, so the watcher learns the band's RMS channels on its own. Triggers that decode nothing
(RTTY, interference spurs) are released after 45 s and held off for longer each time.

Measured: on a synthetic 40m recording, both exchanges were found and decoded (+6 dB and 0 dB
SNR), dial frequencies within 3 and 8 Hz, with no decoder spent on RTTY, CW or a wide data
signal. DSP throughput on this laptop: 43.8 MS/s for three segments, 3.4x the HackRF's 12.8.
Frequencies are reported from the spectrum, so receiver ppm error shifts the reported dial
(set `ppm`) but does not stop decoding.

See `watch.toml` (HackRF, 40-20m) and `examples/band-test.toml` (synthetic recording from
`scripts/make_iq_band.py`). `scripts/hf_survey.py` checks a raw capture for clipping, noise
floor and FT8/WWV markers, a quick antenna sanity check.

## Limits worth knowing

- Mercury acquires only within about **±50 Hz** (measured: ±40 and −50 Hz decode, +80 Hz does
  not). Watch exact RMS dial frequencies and correct each receiver (`ppm`, `offset_hz`), or add
  `shifts_hz = [-100, 0, 100]` to a channel to search (each entry costs another decoder).
- CPU: ~21% of a core per decoder with `connect_only` (default), ~45% without.
- In-session data frames are not decodable (the session seeds their CRC); you see
  CALL/ACCEPT/CQ/DISCONNECT.

## Install (Linux)

```sh
scripts/package.sh                       # builds dist/openrms-watch-<ver>-linux-<arch>.tar.gz
tar xzf dist/openrms-watch-*.tar.gz && cd openrms-watch-*/
./install.sh                             # asks: desktop or headless, and the port
./install.sh --headless --port 18000     # non-interactive: system service on port 18000
```

- **Desktop:** runs as you (systemd user unit) with an "openRMS Watch" app-menu entry that
  opens the page. Or install the desktop app instead (below).
- **Headless:** a system service (`openrms-watch.service`, own user, state in
  `/var/lib/openrms-watch`). Settings only via the web page. To reach it from another computer,
  set a password (`openrms-watch set-password`) and tick "Allow other computers", or use an SSH
  tunnel.
- The installer offers to install SoapySDR and the driver modules (apt, dnf, pacman, zypper, apk).

Then open the page (default **http://127.0.0.1:17373/**, override with `--port`,
`OPENRMS_WATCH_PORT` or Settings), go to **Settings**, pick the SDR and tick the bands. The
watcher picks the sample rate and centre frequency itself and says if the bands don't fit.
**Status** shows SDR levels (clipping, near full scale), a spectrum per band and the decoders.

**Contributing** is off by default. Set the Atlas URL (production:
`https://atlas.openrms.dev`), then **Link to openRMS**: the watcher gets a code from
openRMS-Accounts (`accounts_url`, default `https://auth.openrms.dev`), you approve it on the
site, pick or name your watcher in the watcher's own Settings page, and openRMS-Atlas hands it a
watcher key. Or paste a key made under "My watchers" on the site. Events queue locally and
upload in batches every 10 minutes (sooner if 4000 are waiting; a server that refuses a batch
as too large gets smaller ones).

## Desktop app

`scripts/build-desktop.sh` builds `.deb`, `.rpm` and an AppImage (Tauri, WebKitGTK) with the
engine and the patched Mercury inside. The app is a window onto the same web page plus a tray
icon: status line (bands, busy decoders, contributing, SDR clipping), Open, Restart watcher,
Start at login, Stop watcher. Closing the window keeps watching. If the installer's systemd user
service exists it uses that; otherwise it runs the engine itself, which stops when the app does.
On first run it writes `~/.config/openrms-watch/watch.toml` (the AppImage copies Mercury to
`~/.local/share/openrms-watch/` because its own mount path changes every launch).

## Release builds

`scripts/build-in-docker.sh linux/arm64` (or `linux/amd64`, `linux/arm/v7`, `linux/riscv64`,
add `alpine` for musl) builds a tarball in an Ubuntu 22.04 (or Alpine) container, under QEMU
for other CPUs, so it runs on Ubuntu 22.04+, Debian 12, Raspberry Pi OS, Fedora and Arch.
`.github/workflows/release.yml` does all targets plus the desktop packages on a version tag.

## Commands

`run` (engine + page), `serve` (page only), `devices [--probe]`, `check`, `open`,
`set-password PW`, `import-calls FILE`, `rebuild`, `crc CALL...`. Global: `-c CONFIG`,
`--port N`, `--listen ADDR`.

Developer quick start: `scripts/build-mercury.sh`, `cargo test`, then
`openrms-watch -c examples/band-test.toml run` replays a synthetic 40m recording
(`scripts/make_iq_band.py`).

## Test fixtures

`scripts/make_fixture.py` records a real Mercury CALL/ACCEPT/DISCONNECT between two instances;
`scripts/replay_monitor.py` replays a recording into a monitor (with `--shift HZ` for mistuning
tests). `tests/fixtures/q1test-q1rms10.s16` is the recorded exchange (Q1TEST calls Q1RMS-10).

## Next

- First CI run of the release workflow; a `watch.openrms.dev` download page.
- A calibration wizard in Settings (today: `scripts/atsc_cal.py`, then enter the ppm).
- modem73 and Data2G detectors.
