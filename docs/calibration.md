---
title: Calibration
description: Measuring your SDR's frequency error so reported frequencies are right
order: 4
---

# Calibration

Cheap SDRs run a few to a few dozen parts per million (ppm) off frequency. A HackRF at 20 ppm
is 280 Hz off at 14 MHz.

When scanning bands, this doesn't stop decoding: the watcher measures each signal's frequency
from the spectrum and points a decoder exactly there. It does make the **reported** frequency
wrong, which matters when you compare with gateway listings or other watchers. Calibrate once,
and the frequencies on the page are good to about 10 Hz.

The error is a fixed fraction of frequency, so measuring it anywhere (even at UHF) fixes it for
HF too.

## ATSC TV pilots (North America)

Every ATSC digital TV channel carries a pilot tone exactly 309,440.559 Hz above the channel's
lower edge, locked to the station's precision reference. `scripts/atsc_cal.py` (in the source
repository) finds the strongest pilots and measures them. It drives a HackRF directly, so stop
the watcher first:

```sh
python3 scripts/atsc_cal.py
...
ppm = -24.06  (median of 2; spread -24.15 .. -23.96)
```

Two or more pilots that agree within a fraction of a ppm is a good result. The stock antenna is
enough.

## Other references

- **WWV / WWVH** (5, 10, 15, 20 MHz) or **CHU** (3.33, 7.85, 14.67 MHz): tune the carrier in
  any SDR program and read how far off it shows. ppm = offset ÷ frequency in MHz, with the sign
  explained below.
- A known strong carrier on a published frequency works the same way.

## Entering it

Put the figure in **Settings → Frequency error (ppm)** and save. The watcher's convention:
**positive means signals read low** (the watcher shifts them up). `atsc_cal.py` prints the
figure in this convention already. If you measured a carrier that appears *above* its true
frequency, the ppm is negative.

The error drifts a little with temperature (a few tenths of a ppm); re-measure if you move the
radio somewhere much warmer or colder.

## Upconverters

An upconverter's own oscillator has an error too, a fixed number of Hz rather than ppm. Enter the
upconverter's nominal offset in **Upconverter offset** and fold any measured error into it.
