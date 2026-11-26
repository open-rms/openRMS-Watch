#!/usr/bin/env python3
"""Look for WWV/WWVH carriers in a cs8 capture: carrier offset (-> ppm), SNR, and
the once-per-second tick in the AM envelope.
Usage: wwv_check.py FILE.cs8 CENTER_HZ RATE CARRIER_HZ [CARRIER_HZ ...]"""
import sys
import numpy as np
from scipy.signal import resample_poly

path, fc, fs = sys.argv[1], float(sys.argv[2]), int(sys.argv[3])
raw = np.fromfile(path, dtype=np.int8).astype(np.float32)
x = raw[0::2] + 1j * raw[1::2]
n = len(x)
print(f"{n/fs:.1f} s, rms {np.sqrt(np.mean(np.abs(x)**2)):.1f}, clipped {np.mean(np.abs(raw)>=127)*100:.3f}%")
out_rate = 8000
for f0 in map(float, sys.argv[4:]):
    t = np.arange(n) / fs
    y = resample_poly(x * np.exp(-2j * np.pi * (f0 - fc) * t).astype(np.complex64), 1, fs // out_rate)
    # carrier search +-1 kHz with ~0.1 Hz bins
    N = len(y)
    Y = np.fft.fftshift(np.abs(np.fft.fft(y * np.hanning(N))) ** 2)
    f = np.fft.fftshift(np.fft.fftfreq(N, 1 / out_rate))
    m = np.abs(f) < 1000
    k = np.argmax(np.where(m, Y, 0))
    floor = np.median(Y[m])
    snr = 10 * np.log10(Y[k] / floor)
    off = f[k]
    # AM envelope around the found carrier: 1 Hz tick periodicity
    yb = y * np.exp(-2j * np.pi * off * np.arange(N) / out_rate)
    env = np.abs(resample_poly(yb, 1, 8))  # 1 kHz, ~500 Hz bandwidth
    env = env - env.mean()
    L = len(env)
    ac = np.correlate(env, env, "full")[L - 1:]
    ac /= ac[0]
    tick = ac[1000] if len(ac) > 1000 else float("nan")  # lag 1 s at 1 kHz
    print(f"{f0/1e6:6.3f} MHz: peak at {off:+7.1f} Hz ({-off/f0*1e6:+6.1f} ppm), "
          f"{snr:5.1f} dB over floor, 1 s tick correlation {tick:+.2f}")
