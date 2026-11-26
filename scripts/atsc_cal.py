#!/usr/bin/env python3
"""Measure a HackRF's frequency error (ppm) from US ATSC digital TV pilots.

Each ATSC channel carries a pilot exactly 309,440.559 Hz above its lower edge,
locked to the station's reference. The error is a fixed fraction of frequency,
so the figure applies at HF too.

Usage: atsc_cal.py [--amp] [--lna 32] [--vga 30] [--workdir DIR]
Prints ppm in openrms-watch's convention: positive = signals come out LOW
(true = measured * (1 + ppm/1e6)).
"""
import argparse
import os
import subprocess
import tempfile
import numpy as np

PILOT = 309_440.559


def channels():
    ch = {2: 54e6, 3: 60e6, 4: 66e6, 5: 76e6, 6: 82e6}
    ch.update({n: 174e6 + 6e6 * (n - 7) for n in range(7, 14)})
    ch.update({n: 470e6 + 6e6 * (n - 14) for n in range(14, 37)})
    return ch


def capture(path, fc, fs, n, a):
    bb = {2_000_000: 1_750_000, 20_000_000: 20_000_000}[fs]
    cmd = ["hackrf_transfer", "-r", path, "-f", str(int(fc)), "-s", str(fs), "-b", str(bb),
           "-a", "1" if a.amp else "0", "-l", str(a.lna), "-g", str(a.vga), "-n", str(int(n))]
    subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=60, check=False)
    r = np.fromfile(path, dtype=np.int8).astype(np.float32)
    clip = np.mean(np.abs(r) >= 127) * 100
    return r[0::2] + 1j * r[1::2], clip


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--amp", action="store_true")
    ap.add_argument("--lna", type=int, default=32)
    ap.add_argument("--vga", type=int, default=30)
    ap.add_argument("--workdir", default=None)
    a = ap.parse_args()
    wd = a.workdir or tempfile.mkdtemp(prefix="atsc-")
    path = os.path.join(wd, "cap.cs8")
    ch = channels()

    # 1) survey: 20 MHz captures, look for a pilot spike inside each channel
    fs = 20_000_000
    centers = [61e6, 79e6, 181e6, 199e6, 213e6] + [477e6 + 18e6 * i for i in range(8)]
    found = []
    print("survey (channel: pilot over channel floor)")
    for fc in centers:
        x, clip = capture(path, fc, fs, fs * 0.25, a)
        if len(x) < 1 << 16:
            print(f"  capture at {fc/1e6:.0f} MHz failed")
            continue
        N = 1 << 14
        segs = x[: len(x) // N * N].reshape(-1, N) * np.hanning(N)
        P = np.mean(np.abs(np.fft.fftshift(np.fft.fft(segs, axis=1), axes=1)) ** 2, axis=0)
        f = fc + np.fft.fftshift(np.fft.fftfreq(N, 1 / fs))
        for n, edge in ch.items():
            if edge < fc - 9e6 or edge + 6e6 > fc + 9e6:
                continue
            fp = edge + PILOT
            if abs(fp - fc) < 100e3:
                continue
            win = np.abs(f - fp) < 20e3  # +-~33 ppm at 600 MHz
            body = (f > edge + 1e6) & (f < edge + 5e6)
            pk = 10 * np.log10(P[win].max() / np.median(P[body]))
            floor_body = 10 * np.log10(np.median(P[body]) / np.median(P))
            mark = "  <-- pilot" if pk > 8 else ""
            print(f"  ch {n:2d} ({edge/1e6:5.0f} MHz): pilot {pk:5.1f} dB, channel {floor_body:+5.1f} dB, clip {clip:.2f}%{mark}")
            if pk > 8:
                found.append((n, fp, pk))

    if not found:
        print("no ATSC pilots found; try --amp or a better UHF/VHF antenna")
        return

    # 2) fine measurement of each pilot: 2 MS/s, 4 s, centre 200 kHz below the pilot
    fs2 = 2_000_000
    res = []
    print("\nfine measurement")
    for n, fp, pk in sorted(found, key=lambda t: -t[2])[:8]:
        fc = fp - 200e3
        x, clip = capture(path, fc, fs2, fs2 * 4, a)
        meas = []
        for half in (x[: len(x) // 2], x[len(x) // 2:]):
            M = len(half)
            X = np.abs(np.fft.fft(half * np.hanning(M))) ** 2
            f = np.fft.fftfreq(M, 1 / fs2)
            win = np.abs(f - 200e3) < 20e3
            k = np.flatnonzero(win)[np.argmax(X[win])]
            # quadratic interpolation of the peak
            y0, y1, y2 = np.log(X[k - 1]), np.log(X[k]), np.log(X[k + 1])
            d = 0.5 * (y0 - y2) / (y0 - 2 * y1 + y2)
            meas.append(fc + (f[k] + d * fs2 / M))
        m = float(np.mean(meas))
        ppm = (fp - m) / fp * 1e6  # watch convention: + means signals read low
        res.append(ppm)
        print(f"  ch {n:2d}: expected {fp:,.1f} Hz, measured {m:,.1f} Hz "
              f"(halves differ {abs(meas[0]-meas[1]):.1f} Hz) -> ppm {ppm:+.3f}")

    r = np.array(res)
    print(f"\nppm = {np.median(r):+.2f}  (median of {len(r)}; spread {r.min():+.2f} .. {r.max():+.2f})")
    print(f"at 14.1 MHz that is {np.median(r)*14.1:+.0f} Hz")


if __name__ == "__main__":
    main()
