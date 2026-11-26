#!/usr/bin/env python3
"""Replay a raw s16le 8 kHz file into a monitor-mode Mercury and print MONITOR lines.

Usage: replay_monitor.py MERCURY_BIN FILE.s16 [--shift HZ]
--shift applies a frequency offset (Hz) to test receiver mistuning.
"""
import argparse
import os
import socket
import struct
import subprocess
import tempfile
import threading
import time

RATE = 8000
CHUNK = 160


def crc16_ccitt(s: str) -> int:
    crc = 0xFFFF
    for b in s.upper().encode():
        crc ^= b << 8
        for _ in range(8):
            crc = ((crc << 1) ^ 0x1021) & 0xFFFF if crc & 0x8000 else (crc << 1) & 0xFFFF
    return crc


def shifted(samples, hz):
    """Shift the whole audio spectrum by hz (exact analytic-signal SSB shift)."""
    if not hz:
        return samples
    import numpy as np
    x = np.asarray(samples, dtype=np.float64)
    n = len(x)
    X = np.fft.fft(x)
    h = np.zeros(n)
    h[0] = 1
    h[1:(n + 1) // 2] = 2
    if n % 2 == 0:
        h[n // 2] = 1
    a = np.fft.ifft(X * h)
    y = np.real(a * np.exp(2j * np.pi * hz * np.arange(n) / RATE))
    return [int(v) for v in np.clip(y, -32768, 32767)]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mercury")
    ap.add_argument("file")
    ap.add_argument("--shift", type=float, default=0)
    ap.add_argument("--port", type=int, default=9330)
    args = ap.parse_args()

    raw = open(args.file, "rb").read()
    samples = list(struct.unpack(f"<{len(raw)//2}h", raw))
    samples = shifted(samples, args.shift)

    tmp = tempfile.mkdtemp(prefix="mmon-")
    fin = os.path.join(tmp, "in")
    os.mkfifo(fin)
    ini = os.path.join(tmp, "m.ini")
    open(ini, "w").write(f"""[main]
ui_enabled = false
sound_system = fifo
input_device = {fin}
output_device = /dev/null
arq_tcp_base_port = {args.port}
broadcast_tcp_port = {args.port - 200}
[ptt]
method = none
[store]
enabled = false
""")
    log = open(os.path.join(tmp, "m.log"), "w")
    p = subprocess.Popen([args.mercury, "-C", ini], stdout=log, stderr=subprocess.STDOUT, cwd=tmp)
    for _ in range(100):
        try:
            s = socket.create_connection(("127.0.0.1", args.port), timeout=1)
            break
        except OSError:
            time.sleep(0.2)
    s.sendall(b"MONITOR ON\r")
    s.settimeout(0.1)
    lines = []

    def reader():
        buf = b""
        while not done.is_set():
            try:
                d = s.recv(4096)
            except socket.timeout:
                continue
            if not d:
                break
            buf += d
            while b"\r" in buf:
                l, buf = buf.split(b"\r", 1)
                l = l.decode(errors="replace").strip()
                if l.startswith("MONITOR"):
                    print(l, flush=True)
                    lines.append(l)

    done = threading.Event()
    t = threading.Thread(target=reader)
    t.start()
    time.sleep(1.5)  # let MONITOR ON take effect
    fd = os.open(fin, os.O_WRONLY)
    t0 = time.monotonic()
    pad = [0] * (RATE * 3)
    allsamp = samples + pad
    for n in range(0, len(allsamp), CHUNK):
        blk = allsamp[n:n + CHUNK]
        os.write(fd, struct.pack(f"<{len(blk)}i", *[x << 16 for x in blk]))
        d = t0 + (n + CHUNK) / RATE - time.monotonic()
        if d > 0:
            time.sleep(d)
    time.sleep(2)
    done.set()
    t.join()
    os.close(fd)
    try:
        st = open(f"/proc/{p.pid}/stat").read().rsplit(")", 1)[1].split()
        cpu = (int(st[11]) + int(st[12])) / os.sysconf("SC_CLK_TCK")
        wall = len(allsamp) / RATE
        print(f"--- mercury CPU {cpu:.1f}s over {wall:.0f}s audio = {100*cpu/wall:.0f}% of a core")
    except OSError:
        pass
    p.terminate()
    p.wait(timeout=10)
    print(f"--- {len(lines)} MONITOR lines; log {tmp}/m.log")
    for c in ("Q1TEST", "Q1RMS-10"):
        print(f"crc16({c}) = {crc16_ccitt(c):04X}")


if __name__ == "__main__":
    main()
