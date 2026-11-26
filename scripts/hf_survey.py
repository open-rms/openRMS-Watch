#!/usr/bin/env python3
"""Survey a HackRF cs8 capture: clipping, noise floor per band, known markers, PNG plot.
Usage: hf_survey.py FILE.cs8 CENTER_HZ RATE [OUT.png]"""
import sys
import numpy as np

path, fc, fs = sys.argv[1], float(sys.argv[2]), float(sys.argv[3])
raw = np.fromfile(path, dtype=np.int8)
clip = np.mean(np.abs(raw.astype(int)) >= 127) * 100
x = (raw[0::2].astype(np.float32) + 1j * raw[1::2].astype(np.float32))
print(f"samples {len(x)}  rms {np.sqrt(np.mean(np.abs(x)**2)):.1f}/127  clipped {clip:.3f}%")
N = 1 << 18
w = np.hanning(N).astype(np.float32)
P = np.zeros(N)
k = 0
for i in range(0, len(x) - N, N // 2):
    seg = x[i:i + N]
    P += np.abs(np.fft.fftshift(np.fft.fft((seg - seg.mean()) * w))) ** 2
    k += 1
P /= k
f = fc + np.fft.fftshift(np.fft.fftfreq(N, 1 / fs))
db = 10 * np.log10(P + 1e-9)
bw = fs / N
print(f"bin {bw:.1f} Hz, {k} averages")

def band(lo, hi):
    m = (f >= lo) & (f <= hi)
    return db[m], f[m]

for name, lo, hi in (("40m", 7.0e6, 7.3e6), ("30m", 10.1e6, 10.15e6), ("20m", 14.0e6, 14.35e6),
                     ("31m bcast", 9.4e6, 9.9e6), ("25m bcast", 11.6e6, 12.1e6)):
    d, ff = band(lo, hi)
    floor = np.median(d)
    top = np.argsort(d)[-3:][::-1]
    print(f"{name:10s} floor {floor:6.1f} dB  peaks " +
          ", ".join(f"{ff[t]/1e3:.2f}k +{d[t]-floor:.0f}dB" for t in top))

def marker(name, lo, hi, ref_lo, ref_hi):
    d, _ = band(lo, hi)
    r, _ = band(ref_lo, ref_hi)
    print(f"  {name:22s} {10*np.log10(np.mean(10**(d/10))/np.mean(10**(r/10))):+5.1f} dB over nearby floor")

print("markers:")
marker("FT8 40m 7.0742-7.0770", 7.0742e6, 7.0770e6, 7.0800e6, 7.0830e6)
marker("FT8 30m 10.1362-10.139", 10.1362e6, 10.1390e6, 10.1420e6, 10.1450e6)
marker("FT8 20m 14.0742-14.077", 14.0742e6, 14.0770e6, 14.0800e6, 14.0830e6)
marker("WWV 10 MHz carrier", 10.0e6 - 100, 10.0e6 + 100, 10.0e6 + 2000, 10.0e6 + 3000)
marker("CHU 7.85 MHz carrier", 7.85e6 - 100, 7.85e6 + 100, 7.85e6 + 2000, 7.85e6 + 3000)

if len(sys.argv) > 4:
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    fig, axs = plt.subplots(4, 1, figsize=(12, 11))
    axs[0].plot(f / 1e6, db, lw=0.4); axs[0].set_title("Whole capture"); axs[0].set_xlabel("MHz")
    for ax, (name, lo, hi) in zip(axs[1:], (("40m", 7.0e6, 7.3e6), ("30m", 10.1e6, 10.15e6), ("20m", 14.0e6, 14.35e6))):
        d, ff = band(lo, hi)
        ax.plot(ff / 1e3, d, lw=0.5); ax.set_title(name); ax.set_xlabel("kHz")
    fig.tight_layout(); fig.savefig(sys.argv[4], dpi=90)
    print("plot", sys.argv[4])
