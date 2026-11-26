---
title: Settings and status
description: Setting up the watcher, reading the status page, and how scanning works
order: 3
---

# Settings and status

Everything is set from the watcher's web page (http://127.0.0.1:17373/ by default), in the
desktop app and on headless installs alike. Settings are saved to `watch.toml`; hand edits to
that file are kept, and a backup (`watch.toml.bak`) is written on every save.

## Settings

**Radio**

- **SDR**: the radios SoapySDR found. Press **Rescan** after plugging one in. Notes under the
  list warn about limits (for example "tunes 70 MHz and up: HF needs an upconverter").
- **Bands to watch**: tick the bands. The watcher works out the sample rate and centre
  frequency itself and tells you, for example, "Will capture 16.0 MS/s centred on 12.200 MHz".
  If the bands are too far apart for the radio, it says so. Choose fewer or closer bands.
- **Gain**: one slider per gain stage your radio has. See [Gain](hardware.md#gain).
- **Frequency error (ppm)**: see [Calibration](calibration.md).
- **Upconverter offset**: the upconverter's oscillator in Hz (125000000 for a Ham It Up), or 0.

**Station**: a name for this receiver and your grid square (shown in the local page; when
contributing, Atlas shows the grid at the precision you choose there).

**Decoding**

- **Decoders**: how many Mercury decoders can listen at once. Each busy one uses about a fifth
  of a CPU core; the page shows an estimate for the whole setup.
- **Stale after / gone after**: when an RMS that hasn't been heard is marked stale or gone (see
  below).

**Contribute to openRMS**: see [Contributing](contributing.md).

**Web page**: the port, whether other computers may open the page (needs a password), and the
password.

**Save and restart** checks the new settings, saves them and restarts the watcher with them.

## Status

- **Radio**: the SDR's level and how often it clips, with a plain verdict ("Levels look fine",
  "Clipping: lower the gain").
- **Contributing**: on or off, the last upload, how many events are waiting, and how many
  callsigns the watcher has from Atlas for naming stations.
- **Decoders**: what each decoder is doing:
  - *idle*: free.
  - *listening*: pointed at a burst that looked like Mercury, waiting to decode it. Released
    after 45 s if nothing decodes.
  - *confirmed*: Mercury decoded on this frequency. Kept for 15 minutes after the last activity.
  - *pinned*: the RMS itself was heard here (it answered a call, or called CQ). Kept for good,
    so the watcher learns where the band's RMS stations are.
- **Spectrum**: one strip per band. Blue is the signal, grey the local noise floor, dashed the
  detection threshold. A trace flat on the floor means no signal is getting in (check the
  [antenna](hardware.md)).

## The other tabs

- **RMS stations**: every station heard or called, with its status:
  - *active*: the station itself transmitted within the stale window.
  - *stale* / *gone*: not heard for longer than the stale / gone window **while the watcher was
    listening** on its frequency that whole time.
  - *unverified*: not heard lately, but the watcher hasn't listened long enough to say.
  - *called-only*: clients called it, but it was never heard answering (out of range, or off
    the air).
- **Connections**: each connection attempt: client, RMS, frequency, whether it was answered,
  how long it lasted.
- **Raw frames**: every decoded frame, as Mercury reported it.
- **Channels & receivers**: what was scanned and for how long.

Every table can be filtered, sorted, and downloaded as CSV. Filters are kept in the address,
so a filtered view can be bookmarked.

### Why some stations have no name

A Mercury call carries the caller's callsign in clear, but only a 16-bit checksum of the station
being called, and the RMS's answer carries only checksums. The watcher names a station by
matching that checksum against callsigns heard in clear and Atlas's gateway list, narrowed by
listed frequencies. Checksums collide, so a name is shown as *inferred* (one match),
*ambiguous* ("one of A, B") or *unknown*. Only a station heard calling CQ is *heard* by name.

## How scanning works

The watcher captures whole band segments at once, looks for bursts shaped like Mercury's
connect frames (about 190 Hz wide, about 3 s long, starting out of silence), and points a
decoder at each one with a few seconds of buffered audio, so the first call isn't missed.
Signals that are always there (interference, carriers, RTTY) never start out of silence and are
ignored; frequencies that keep triggering without decoding are held off for longer each time.
