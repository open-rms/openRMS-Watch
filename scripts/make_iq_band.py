#!/usr/bin/env python3
"""Make a synthetic wideband IQ recording (cs8, like a HackRF) of part of 40m with
real Mercury exchanges at arbitrary frequencies plus interferers and noise.

Usage: make_iq_band.py OUT.cs8 FIXTURE_A.s16 FIXTURE_B.s16 [--seconds 80]

Signals (centre = dial + 1500 Hz):
  A  Mercury exchange, dial 7,101,337 Hz, starts 5 s,  SNR +6 dB in 2.5 kHz
  B  Mercury exchange, dial 7,243,912 Hz, starts 25 s, SNR  0 dB in 2.5 kHz
  C  RTTY 170 Hz shift, 45.45 Bd, 7,080,000 Hz, strong, continuous (false trigger bait)
  D  CW carrier 7,050,000 Hz, strong
  E  2.4 kHz wide data signal around 7,180,000 Hz
"""
import argparse
import numpy as np
from scipy.signal import hilbert, resample_poly, firwin, lfilter

FS = 1_600_000
FC = 7_150_000
NOISE_SIGMA = 6.0  # counts per I/Q component


def baseband_from_audio(path):
    a = np.fromfile(path, dtype="<i2").astype(np.float64)
    z = hilbert(a) * np.exp(-2j * np.pi * 1500 * np.arange(len(a)) / 8000)
    return z / np.sqrt(np.mean(np.abs(z[np.abs(a) > 50]) ** 2))  # unit power while keyed


def band_noise(seconds, rng):
    n = int(seconds * 8000)
    z = rng.standard_normal(n) + 1j * rng.standard_normal(n)
    h = firwin(255, 1200, fs=8000)
    z = lfilter(h, 1, z)
    return z / np.sqrt(np.mean(np.abs(z) ** 2))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("fa")
    ap.add_argument("fb")
    ap.add_argument("--seconds", type=float, default=80)
    args = ap.parse_args()
    rng = np.random.default_rng(1)
    noise_2500 = 2 * NOISE_SIGMA**2 * 2500 / FS

    def amp(snr_db):
        return np.sqrt(10 ** (snr_db / 10) * noise_2500)

    sigs = [  # (baseband @8k, start s, rf centre, amplitude)
        (baseband_from_audio(args.fa), 5.0, 7_101_337 + 1500, amp(6)),
        (baseband_from_audio(args.fb), 25.0, 7_243_912 + 1500, amp(0)),
        (band_noise(args.seconds, rng), 0.0, 7_180_000, amp(10)),
    ]
    up = FS // 8000
    margin = 400
    total = int(args.seconds * FS)
    out = open(args.out, "wb")
    blk = FS  # 1 s
    rtty_phase = 0.0
    bits = rng.integers(0, 2, int(args.seconds * 46) + 10)
    for b0 in range(0, total, blk):
        n = min(blk, total - b0)
        t = (b0 + np.arange(n)) / FS
        y = (rng.standard_normal(n) + 1j * rng.standard_normal(n)) * NOISE_SIGMA
        for z8, start, rf, a in sigs:
            s0 = int(round((b0 / FS - start) * 8000))
            s1 = s0 + n // up
            lo, hi = s0 - margin, s1 + margin
            seg = np.zeros(hi - lo, dtype=complex)
            src_lo, src_hi = max(lo, 0), min(hi, len(z8))
            if src_hi > src_lo:
                seg[src_lo - lo:src_hi - lo] = z8[src_lo:src_hi]
                u = resample_poly(seg, up, 1)[margin * up:margin * up + n]
                y += a * u * np.exp(2j * np.pi * (rf - FC) * t)
        # RTTY: mark/space +-85 Hz around 7.080 MHz
        bit = bits[((b0 + np.arange(n)) * 45.45 / FS).astype(int)]
        f = (7_080_000 - FC) + np.where(bit == 1, 85.0, -85.0)
        ph = rtty_phase + 2 * np.pi * np.cumsum(f) / FS
        rtty_phase = ph[-1]
        y += amp(25) * np.exp(1j * ph)
        y += amp(30) * np.exp(2j * np.pi * (7_050_000 - FC) * t)
        q = np.empty(2 * n, dtype=np.int8)
        q[0::2] = np.clip(np.round(y.real), -127, 127)
        q[1::2] = np.clip(np.round(y.imag), -127, 127)
        q.tofile(out)
    out.close()
    print(f"wrote {args.out}: {args.seconds:.0f} s at {FS/1e6} MS/s centred {FC/1e6} MHz")


if __name__ == "__main__":
    main()
