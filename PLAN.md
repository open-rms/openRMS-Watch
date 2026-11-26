# openRMS-Watch: community watcher plan (agreed 2026-11-25)

> openRMS-Directory was renamed **openRMS-Atlas** (`https://atlas.openrms.dev`) on 2026-11-25;
> this plan uses the new name throughout. The watcher's config section is `[atlas]`
> (`[directory]` still loads, with a warning).

**Scope (maintainer, 2026-11-25): Linux only for now**, every distribution family and
architecture we can build for. Windows and macOS are out of scope until further notice.

Goal: anyone with an SDR can install a watcher, pick their radio and bands in a settings page,
and either keep the results to themselves or contribute them live to the openRMS site. The feed
contract with Atlas is `../openRMS-Spec/watch/feed.md`.

## 1. Shape: one engine, two front ends

```
openrms-watch (Rust, one binary)                   front ends
 ├─ SDR input (SoapySDR | command | file)           ├─ Desktop: Tauri app = tray icon + webview of the
 ├─ wideband scan + decoder pool (Mercury procs)    │   local web UI, start at login, starts/stops the engine
 ├─ local SQLite + local web UI (incl. Settings)    └─ Headless: systemd service (system or
 └─ uploader (optional, to Atlas)               user unit); settings only via the web UI
```

The web UI is the only settings UI, so desktop and headless never drift apart. The desktop app
adds what a browser can't: a tray icon and status, start at login, opening the page in its own
window, and native notifications ("token rejected", "SDR unplugged"). Tauri reuses the same HTML
(the umbrella plan already names Tauri for this).

## 2. Settings (web UI, both modes)

Stored in the same TOML file (written by the settings API, so hand edits keep working). Every key
is declared in the openRMS-Spec config schema with a default, help text and tier (Standard,
Advanced or Expert), per the umbrella rule.

| Page | Standard | Advanced / Expert |
|---|---|---|
| **Radio** | Device picker (enumerated, with "Test" showing a live spectrum strip); band presets (40/30/20m, 80m, 60m, 17m...) checked off; gain slider with a clipping meter | Sample rate, centre frequency (auto-chosen to fit the bands and avoid DC), upconverter offset, direct sampling, antenna port, command/file sources |
| **Calibrate** | Wizard: ATSC pilots (US), WWV/CHU carrier, or enter ppm | Per-receiver fixed offset |
| **Decoding** | Number of decoders (with a CPU estimate) | Detector threshold, pre-roll, hold times, connect-only |
| **Contribute** | Off (local only) / On; "Link to openRMS" (device code) or paste a token; watcher name, grid, location precision, show me in the contributor list | Atlas URL (self-hosted directories), upload interval |
| **Web & ports** | Web page port; who can open it: this computer only (default) or my network (requires a password) | Decoder port range, data retention |

A "what will this hear?" panel turns the choices into plain words: which bands, roughly how many
CPU cores, and warnings such as "the Pluto+ can't tune HF without an upconverter".

## 3. Ports

- Web page default **17373** (not a common port). Override with `--port N`, `OPENRMS_WATCH_PORT`,
  the installer prompt, or the settings page. Binds `127.0.0.1` unless LAN access is enabled.
- Decoder (Mercury) ports: picked automatically from a free range (default 17400-17999), checked
  with a test bind, never fixed. Mercury must listen on localhost only (patch item below).

## 4. SDR input across operating systems

- **SoapySDR** (Boost licence) through the `soapysdr` crate: one API to enumerate and stream
  RTL-SDR, HackRF, Airspy, PlutoSDR, LimeSDR, USRP, and SDRplay (needs the vendor API). Works on
  Packages depend on the distribution's SoapySDR and driver modules; the AppImage bundles them.
- Device profiles supply sane defaults and limits: rate options, gain stages, HF capability
  (RTL direct sampling; Pluto needs an upconverter; HackRF works but is weak on HF), DC handling.
- The existing `command` and `file` sources stay as Expert options (testing, unusual radios).
- Later: native KiwiSDR client for watching fixed channels from a remote Kiwi.

## 5. Mercury: one patch set, offered upstream

Bundled as a separate process (GPL, aggregation; source and patch shipped alongside, as now).
Patch items:

1. Done: CRC fields on MONITOR lines, MFSK500 in the monitor, `MERCURY_MONITOR_CONNECT_ONLY`.
2. **Bind the TNC ports to 127.0.0.1** (an option, default unchanged upstream; the watcher always
   sets it). Today a monitor's control port is open on every network interface.

The maintainer is fine with offering these upstream to Rhizomatica once they are stable (no
Windows work). If accepted, their Debian/ARM release builds replace our patched build.
A cross-platform audio input for Windows is dropped along with Windows; Linux uses the FIFO input.

## 6. Platforms, packaging, CI

| Target | Desktop (Tauri, WebKitGTK) | Headless |
|---|---|---|
| x86_64 glibc | AppImage, .deb, .rpm, Arch PKGBUILD | same packages + systemd unit (own user, udev rules for SDRs) |
| aarch64 glibc (Pi 4/5, ARM laptops) | AppImage, .deb, .rpm | .deb/.rpm + systemd; later a ready Pi image |
| armv7 glibc (Pi 2/3, 32-bit Pi OS) | (not targeted) | .deb + systemd |
| riscv64 glibc | (not targeted) | .deb + systemd, best effort |
| x86_64 / aarch64 musl (Alpine, static) | (not targeted) | static tarball + OpenRC/systemd unit |

The installer script (and package post-install) asks Desktop or Headless and the web port;
`--headless` and `--port N` make it non-interactive.

CI: GitHub Actions on x86_64 and arm64 Linux runners, cross-compiling armv7, riscv64 and musl
(`cross`/`cargo-zigbuild`; Mercury with matching gcc cross toolchains), building the engine,
patched Mercury and the Tauri shell. Smoke test: the replay and synthetic-band tests that already
pass, under QEMU for non-native targets. Builds are unsigned for now.

## 7. Sizing

Measured: 43.8 MS/s of scan DSP on a laptop (three segments), so a HackRF at 12.8 MS/s uses about
a third of one core, plus about 0.2 core per busy decoder. A first-run benchmark sets the
defaults: a Raspberry Pi 4 probably gets one band at 2.4 MS/s and 4 decoders (to be measured).

## 8. Uploader

Local outbox table (event ULID, sent flag), batch upload every 10 minutes (up to 4900 events / 4.5 MB per POST, early when 4000 are
waiting, halved on HTTP 413), retry with backoff, survives restarts and offline periods. Status on the page: linked account, last upload, queue length,
token errors. Token: an openRMS API key (`ormk_`, see "Accounts-minted keys" below) from
one-click linking, or pasted.

Daily refresh of `/api/v1/watch/callsigns` (callsigns with their listed dial frequencies) replaces
the hand-made calls file. Naming follows the same rules as Atlas (feed.md section 3):
CRC16 collisions are certain at this list size, so a CRC maps to candidates, narrowed by a listed
Mercury channel within 200 Hz of the heard dial, then Mercury-capable gateways, then callsigns
heard in clear; anything still ambiguous shows as "one of: A, B". Token kept in the OS keychain on desktop, otherwise in the config file with
0600 permissions.

## 9. Phases

| Phase | Watcher | Depends on |
|---|---|---|
| **W1** ✅ | SoapySDR input + device enumeration; settings API + pages; `--port` (default 17373), free decoder ports; Mercury localhost bind | — |
| **W2** ✅ | Uploader (pasted token), callsign list with frequency-aware naming, contributor settings | Atlas A (watchers, watcher keys, ingest, callsign list) |
| **W2b** ✅ | One-click "Link to openRMS" (device code) | Atlas D (device authorization) |
| **W3** (mostly) | Done: tarball + `install.sh` (desktop/headless, `--port`), systemd units, Tauri desktop app (tray, autostart) as .deb/.rpm/AppImage, Docker builds per architecture, release workflow. To do: first CI run, download page on watch.openrms.dev | — |
| **W4** | Pi image, auto-update, KiwiSDR source, more modes: modem73 / Data2G detectors, and VARA/XARA via XARA's MIT-licensed `xara-monitor` (github.com/NR5P/xara, Rust; a passive VARA HF monitor, so it can sit beside Mercury the same way). Some gateways already list XARA + MERCURY + DATA2G service codes | — |

Atlas side, agreed with Atlas session (estimates are theirs): **A** watchers, watcher
key type, ingest, callsign list, "My watchers" page (about 1 day); **B** sightings, "Heard on air"
on gateway pages, contributor list and page (about 1 day); **C** live SSE stream and /live page
(half a day); **D** device-code linking (half a day); plus a moderator Watchers tab.

## Identity moving to openRMS-Accounts (2026-11-25)

The user decided to move sign-in out of Atlas into openRMS-Accounts
(`https://auth.openrms.dev`, contract `../openRMS-Spec/accounts/identity.md`). For the watcher:

- (Superseded 2026-11-26 by "Accounts-minted keys" below: `ormsw_` keys are retired.)
- **Changes (done 2026-11-25 after the user's go, relayed by openrms-84; tested end to end against Accounts :8789 and Atlas :8787):** "Link to openRMS" runs the
  RFC 8628 device flow against Accounts (client `openrms-watch`, with
  `resource=https://atlas.openrms.dev` so the token's audience is Atlas). The result is a
  short-lived OAuth access token, used once as `Authorization: Bearer` to get a watcher key from
  Atlas, then discarded. Config gains an Accounts URL next to Atlas URL (both
  default to the production hosts once they exist).
- **Agreed with openrms-84 (identity.md §5, "Watch linking after the split"):** the device
  authorization request carries `resource=https://atlas.openrms.dev` (supported by
  `@better-auth/oauth-provider` 1.7.28, bound to the approval). With that token as Bearer the
  watcher calls `GET /api/v1/me/watchers` (to let the user pick one in the watcher's own page)
  and `POST /api/v1/watch/link` `{watcherId}` or `{name, gridsquare?}` and gets
  `{key, watcherId, name}`. The Accounts approval page knows nothing about watchers.
- (Superseded: there were no live `ormsw_` keys to migrate.)
- To do once openRMS-Libs has a hosted remote: replace the inline device flow in `src/web.rs`
  (`link_start` / `link_poll`) with the shared `openrms-auth` crate (tag `openrms-auth-v0.1.0`,
  `wait_for_device` inside `spawn_blocking`, scope without `offline_access`, cancel on a new
  Link). A local `file://` git dependency would break the container builds, so not before.
- Accounts dev server (openrms-84, 2026-11-25): `http://localhost:8789`; read
  `device_authorization_endpoint` and `token_endpoint` from
  `/.well-known/openid-configuration`. Device code request is form-encoded:
  `client_id=openrms-watch`, `scope=openid callsign clubs offline_access`,
  `resource=<Atlas URL>`; poll the token endpoint with the device_code grant. The access
  token (JWT, ~15 min, no `role` claim) is the Bearer for Atlas's `GET /me/watchers` and
  `POST /watch/link`. Keep the current Atlas-native flow until Atlas switches.
- Hosts (user, 2026-11-25): one subdomain per project, no shared `api.` name. Atlas
  `atlas.openrms.dev` (token audience `https://atlas.openrms.dev`); this project
  `watch.openrms.dev` for downloads and docs (static, Cloudflare; set up with the first release).

## 10. Decisions (maintainer, 2026-11-25)

1. Client callsigns: stored, admin-only at first; an admin toggle publishes them; opt-out list.
2. Any account may contribute; only verified callsigns count toward `confirmed`.
3. Linux only (all distributions and architectures); Windows and macOS dropped for now.
4. Mercury patches may be offered upstream once stable; no Windows work.
5. Unsigned builds for now; web port 17373.

## Accounts-minted keys (2026-11-26)

Accounts mints every API key (identity.md §5); Atlas only verifies. Atlas-minted `ormsw_` keys
are retired (no live watchers had them). The watcher now:

- **Links:** device code at Accounts with `scope=openid callsign clubs keys offline_access` and two
  resources, the issuer and Atlas. It polls with `resource=<issuer>` (a token for Accounts),
  swaps the refresh token for an Atlas token (`resource=<atlas>`) only to list
  `GET /api/v1/me/watchers`, and after the user picks: `POST {issuer}/api/v1/keys`
  `{aud: <atlas>, scopes: [watch:ingest], name: "openRMS Watch", expires_in_days: 365}` (on 401,
  refresh and retry), then `POST {atlas}/api/v1/watch/link` with `Bearer ormk_` and
  `{watcherId}` or `{name, gridsquare}` → `{watcherId, name}`. It stores `token` and `watcher_id`
  in `[contribute]` and revokes the refresh token. A minted key whose link failed is kept for
  the retry, so a retry doesn't mint another.
- **Uploads** with `Authorization: Bearer ormk_`. On `403 {code: not_linked}` (a pasted key, or
  a renewal Atlas couldn't match) it relinks to the saved `watcher_id` (no watcher yet: a new
  one named "openRMS Watch") and saves the id.
- **Renews** before uploading when less than a third of the key's life is left (at most 30
  days ahead): `POST {issuer}/api/v1/keys/renew` with the current key, only for `renewable` keys
  issued by the configured Accounts. The new key (with `replaces`) is saved to watch.toml first;
  Atlas moves the watcher to it on first use.
- Tested 2026-11-26 against the dev Accounts (:8789) and Atlas (:8787): device link with both
  resources, watcher listing, minting, linking, uploads, relinking a pasted key after
  `not_linked`, and an upload with a renewed key (no relink needed). Throwaway user deleted.
  The uploader's own renewal (check, renew, save, swap, rebind via `replaces`) ran against a
  local stand-in for Accounts and Atlas with 60-second keys. Not run yet: the refresh-and-retry
  when minting after the 15-minute access token has expired.

