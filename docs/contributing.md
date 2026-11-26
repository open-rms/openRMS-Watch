---
title: Contributing and privacy
description: Sharing what your watcher hears with openRMS, what is sent and what stays local
order: 5
---

# Contributing and privacy

Contributing is **off** by default: everything the watcher hears stays on your computer. Turn
it on and the same events are also sent to [openRMS Atlas](https://atlas.openrms.dev), the
open gateway registry. There they become:

- a **live feed** of Mercury activity heard across the network;
- **"Heard on the air"** evidence on gateway pages: proof that a listed Mercury channel is
  really on the air, and where. It never edits a listing;
- the **contributor list**, which credits watchers active in the last few days.

## Turning it on

1. Make an account on the openRMS site (a callsign is required).
2. In the watcher, **Settings → Contribute to openRMS**: set the **Atlas URL**
   (`https://atlas.openrms.dev`) and press **Link to openRMS**.
3. Approve the code shown, on the page that opens (or type it in at the address shown, from any
   device).
4. Back in the watcher, pick one of your watchers or name a new one, and press
   **Use this watcher**. The watcher saves its key, turns contributing on, and restarts.

Prefer not to sign in from the watcher? Create a watcher under **My watchers** on the site,
copy its key (`ormsw_...`) and paste it into **Or paste a watcher key**.

Uploads go in batches every 10 minutes (sooner if a lot is waiting). If the internet is down,
events wait in a local queue and go out later; nothing is lost.

## What is sent

| Event | Contents |
|---|---|
| **frame** | Time, dial frequency, Mercury mode and frame kind (CALL, ACCEPT, CQ, DISCONNECT), the caller's callsign for a CALL or CQ, the checksums of the stations in a connect, session id, bandwidth, SNR, and the raw line Mercury printed |
| **attempt** | One connection attempt built from those frames: start and end, frequency, caller, checksums, whether it was answered, number of calls, best SNR |
| **coverage** | Which frequency ranges you were listening to, and when (so a silent station can be told apart from a band nobody was watching) |
| **status** | Every 5 minutes: software version, OS and CPU type, SDR name, number of decoders, CPU use, ppm, uptime |

Each event also carries the watcher's own guess at the station's name; Atlas keeps it but
names stations itself, from the current gateway list.

**Not sent:** audio or IQ recordings, your exact location, anything from your computer beyond
the status line above, or your settings.

## Privacy on the site

- **Your location** appears at the precision you choose for the watcher: a 4-character grid
  (the default), a 6-character grid, or hidden.
- **Client callsigns** (the people connecting to a gateway) are stored, but the public site
  shows "a client" unless the site's operators turn publishing on. Any station can ask to be
  added to an opt-out list, which keeps its callsign off the public site either way.
- **The contributor list** shows your callsign and watcher name unless you mark the watcher as
  not public. You still count in the totals.
- Amateur transmissions are public by nature (like PSKReporter spots); the watcher records what
  anyone with a receiver could hear.

## Trust

- Anyone with an account can contribute. A sighting becomes **confirmed** only when watchers
  belonging to two or more callsign-verified accounts hear the station within 10 minutes.
- The key only works for uploads, only for that watcher, and you can revoke or replace it on
  the site at any time. A revoked key shows "token rejected" in the watcher's Status page; the
  watcher keeps working locally.
- Moderators can disable a watcher that sends bad data.

## Turning it off

Untick **Contribute** in Settings and save. Revoke the key on the site if you won't use it again.
