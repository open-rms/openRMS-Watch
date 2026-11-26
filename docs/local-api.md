---
title: Local API
description: The JSON and CSV API every watcher serves, for scripts and dashboards
order: 7
---

# Local API

Every watcher serves a small read API next to its web page (http://127.0.0.1:17373 by
default). It's what the page itself uses, and it's handy for scripts, dashboards and
exporting data. It only covers **this** watcher; the network-wide data (live feed, sightings,
contributors) is [openRMS Atlas's API](https://atlas.openrms.dev/docs).

Times are Unix milliseconds. Frequencies are USB dial frequencies in Hz (signal centre minus
1500 Hz). Checksums (CRCs) are 4 hex digits.

## Data

All of these accept `format=csv` to download the same rows as CSV.

### `GET /api/stations`

RMS stations heard or called, newest activity first.

Filters: `q` (callsign, candidate or CRC), `band` (e.g. `40m`), `status`
(`active`, `stale`, `gone`, `unverified`, `called-only`), `receiver`.

```json
{
  "crc": "BD24",
  "callsign": "Q1RMS-10",
  "name_confidence": "inferred",
  "candidates": ["Q1RMS-10"],
  "status": "active",
  "first_ms": 1795665714540,
  "last_heard_ms": 1795665718746,
  "last_called_ms": 1795665714540,
  "attempts": 1, "accepted": 1, "unanswered": 0, "clients": 1, "cq": 0,
  "channels": [7101000], "bands": ["40m"],
  "receivers": ["hackrf"],
  "best_snr": 24.1,
  "listened_h_since": 0.0
}
```

`name_confidence` is `heard` (callsign received in clear, from a CQ), `inferred` (exactly one
known callsign has this CRC), `ambiguous` (several; see `candidates`) or `unknown`.
`listened_h_since` is how long the watcher was listening on the station's frequencies since it
was last heard, which is what *stale* is judged on.

### `GET /api/attempts`

Connection attempts (a client's calls, the RMS's answer and the disconnect, folded together).

Filters: `q` (client, RMS, CRC or frequency), `band`, `receiver`, `accepted` (`yes` / `no`),
`since_h` (default 168; `0` = all time), `limit` (default 500, at most 10000).

```json
{
  "id": 1, "dial_hz": 7101000, "band": "40m",
  "first_ms": 1795665714540, "last_ms": 1795665748093,
  "client_call": "Q1TEST", "client_crc": "9591",
  "rms_crc": "BD24", "rms_call": "Q1RMS-10", "rms_candidates": ["Q1RMS-10"], "rms_match": "inferred",
  "sid": 125, "bw": 2300,
  "n_call": 1, "n_accept": 1, "accepted": true,
  "disconnect_ms": 1795665748093,
  "snr_call": 23.8, "snr_accept": 24.1,
  "receivers": ["hackrf"]
}
```

### `GET /api/frames`

Every frame Mercury decoded, newest first.

Filters: `q` (any text in the raw line), `kind` (`CALL`, `ACCEPT`, `CQ`, `DISCONNECT`, ...),
`band`, `receiver`, `since_h` (default 24), `limit`.

```json
{
  "id": 6, "ts_ms": 1795665748093, "receiver_id": "hackrf",
  "dial_hz": 7101000, "band": "40m", "shift_hz": 0.0,
  "mode": "DATAC16", "kind": "DISCONNECT",
  "from_call": null, "to_crc": null, "caller_crc": null, "called_crc": null,
  "sid": 125, "snr": 25.0,
  "raw": "MONITOR DATAC16 DISCONNECT SID=125 SNR=25.0"
}
```

### `GET /api/channels`

What the watcher listens to: scanned band segments (`lo_hz`..`hi_hz`) and fixed channels
(`lo_hz` = `hi_hz`), with listening hours and frames in the last 24 hours.

### `GET /api/summary`

Counts for the page header: stations by status, connections and frames in the last 24 hours,
known callsigns, the last frame time, receivers and bands.

## Live state

### `GET /api/live`

What the engine is doing right now (the Status page polls this every 2 s):

- `engine.receivers`: per SDR, `clip_pct`, `rms_dbfs`, `msps`, `error`;
- `engine.segments`: per scanned band, `db` and `floor` (about 800 points each, low to high
  frequency) and `threshold_db`;
- `engine.decoders`: `state` (`idle`, `listening`, `confirmed`, `pinned`) and `dial_hz`;
- `contribute`: `enabled`, `lastOkMs`, `error`, `queue`, `sent24h`, `directoryCallsigns`;
- `version`.

## Settings (password-protected when a password is set)

These change the watcher, so they need the web password if one is set (HTTP Basic, any user
name), and are refused from the network until one is.

| | |
|---|---|
| `GET /api/settings` | The settings as the page shows them (keys and passwords are never returned) |
| `PUT /api/settings` | Save new settings and restart |
| `POST /api/plan` | Check bands against an SDR: the sample rate, centre frequency and CPU estimate it would use, or why it can't |
| `GET /api/devices` | SDRs found, with what each can do |
| `POST /api/link`, `GET /api/link`, `POST /api/link/finish` | The "Link to openRMS" steps |

## Examples

```sh
# answered connections on 40 m in the last day, as CSV
curl -s 'http://127.0.0.1:17373/api/attempts?band=40m&accepted=yes&since_h=24&format=csv'

# stations that went quiet while you were listening
curl -s 'http://127.0.0.1:17373/api/stations?status=stale' | jq -r '.[] | "\(.callsign // .crc) \(.channels)"'
```
