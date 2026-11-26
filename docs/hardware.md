---
title: Choosing an SDR and antenna
description: Which radios work, how much of HF each covers, and getting a usable signal
order: 2
---

# Choosing an SDR and antenna

The watcher scans whole band segments for Mercury connections, so the radio decides how many
bands you can watch at once, and the antenna decides whether you hear anything at all.

## Radios

Anything SoapySDR supports works. These are the common ones:

| Radio | HF? | Bands at once | Notes |
|---|---|---|---|
| **HackRF One** | Yes, directly | 40, 30, 20 and 17 m at 16 MS/s | 8-bit, so strong broadcast stations can overload it. Keep the amp off and the gain moderate; an HF low-pass filter helps a lot. Its clock is typically 10-25 ppm off: [calibrate](calibration.md) it. |
| **RTL-SDR Blog V3/V4** | With direct sampling, or an upconverter | One band (2.4 MS/s) | Cheap and a good fit for a Raspberry Pi. In Settings set the driver setting `direct_samp` to `2` for HF. |
| **ADALM-PLUTO / Pluto+** | Only with an upconverter | Up to several bands | Tunes 70 MHz and up. Use a 125 MHz upconverter (e.g. Ham It Up) and enter its offset in Settings. 12-bit, so it copes better with strong signals than a HackRF. |
| **Airspy HF+, SDRplay** | Yes | One band | Good HF receivers. SDRplay needs the vendor's API installed for its SoapySDR module. |
| **KiwiSDR** (someone else's) | Yes | One frequency per user slot | Can't scan a band, but can watch a known RMS frequency. See `examples/watch.toml`. Public Kiwis have user limits; use them sparingly. |

Run `openrms-watch devices --probe` to see what the watcher finds and what each radio can do.
The Settings page shows the same, with a warning when a radio can't reach the bands you picked.

## The antenna matters more than the radio

Mercury stations are often weak. The watcher needs real HF signal, which a stock telescopic
antenna doesn't give you.

**Quick test:** with the watcher running, open **Status**. Each band's spectrum should show
bumps above the grey noise floor. FT8 at 7.074, 10.136 and 14.074 MHz is busy nearly all
day: if you can't see it, the watcher won't hear Mercury either. You can also listen for WWV
on 10 or 15 MHz (clock ticks every second) in any SDR program.

What works:

- **A wire outdoors.** Even 10-20 m of wire out of a window or along a fence, through a 9:1
  unun, is a huge step up from anything indoors. An end-fed, a dipole, or your station antenna
  all work.
- **Distance from the computer.** Laptops, USB hubs and switching power supplies radiate a comb
  of noise across HF. Put the SDR on a USB extension cable away from the computer and its
  charger. Evenly spaced spikes every few kHz in the spectrum are that noise.
- **Filtering.** An HF low-pass filter (below 30 MHz) keeps FM broadcast and VHF out of an
  8-bit SDR. A broadcast-band notch helps if you're near AM or shortwave transmitters.

### Sharing your station antenna

Never connect an SDR straight to an antenna you transmit on: your own transmitter will destroy
its front end. Use a receive port on the radio, a T/R switch that grounds the SDR during
transmit, or a protector made for the purpose.

## Gain

Too little gain and weak stations vanish in the SDR's own noise; too much and strong stations
overload it, which buries everything else. In **Status**:

- **"Clipping"** in red: lower the gain.
- **"Close to full scale"**: lower the gain, or filter out the strong stations.
- **"Very low level"**: raise the gain, or check the antenna.

Start with the amp off and the gain around the middle, then adjust while watching the levels.
