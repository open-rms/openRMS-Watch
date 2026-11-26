#!/usr/bin/env python3
"""Record a real Mercury CALL/ACCEPT/DISCONNECT exchange as raw audio.

Runs two Mercury instances (a client and a listening "RMS") on the fifo audio
backend, joins them with a real-time paced bridge, and writes the mix of both
transmitters to a raw s16le 8 kHz mono file. That file is the test fixture the
watcher replays into a monitor-mode Mercury.

Each instance gets its own throwaway INI (PTT method none), so a personal
~/.config/mercury/mercury.ini can never key a real radio from here.

Usage: make_fixture.py MERCURY_BIN OUT.raw [--caller Q1TEST] [--called Q1RMS-10]
"""
import argparse
import os
import select
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time

RATE = 8000
CHUNK = 160  # 20 ms


def write_ini(path, port, bport, fifo_in, fifo_out):
    with open(path, "w") as f:
        f.write(f"""[main]
ui_enabled = false
sound_system = fifo
input_device = {fifo_in}
output_device = {fifo_out}
arq_tcp_base_port = {port}
broadcast_tcp_port = {bport}
[ptt]
method = none
[store]
enabled = false
""")


class Ctl:
    def __init__(self, port, name):
        self.name = name
        for _ in range(100):
            try:
                self.s = socket.create_connection(("127.0.0.1", port), timeout=1)
                break
            except OSError:
                time.sleep(0.2)
        else:
            raise SystemExit(f"{name}: control port {port} never opened")
        self.s.settimeout(0.2)
        self.buf = b""
        self.lines = []

    def send(self, cmd):
        print(f"[{self.name}] > {cmd}", flush=True)
        self.s.sendall(cmd.encode() + b"\r")

    def pump(self):
        try:
            d = self.s.recv(4096)
        except socket.timeout:
            return
        if not d:
            return
        self.buf += d
        while b"\r" in self.buf:
            line, self.buf = self.buf.split(b"\r", 1)
            line = line.decode(errors="replace").strip()
            if line:
                print(f"[{self.name}] < {line}", flush=True)
                self.lines.append(line)

    def wait_for(self, prefix, timeout):
        end = time.time() + timeout
        while time.time() < end:
            self.pump()
            for l in self.lines:
                if l.startswith(prefix):
                    return l
        return None


def bridge(a_out, b_out, a_in, b_in, record, stop):
    """Pace both transmitters at 8 kHz, cross-feed them, record the mix."""
    fds = {}
    for name, path, mode in (("a_out", a_out, os.O_RDONLY), ("b_out", b_out, os.O_RDONLY),
                             ("a_in", a_in, os.O_WRONLY), ("b_in", b_in, os.O_WRONLY)):
        while not stop.is_set():
            try:
                fds[name] = os.open(path, mode | os.O_NONBLOCK)
                break
            except OSError:
                time.sleep(0.05)
    pend = {"a_out": b"", "b_out": b""}
    t0 = time.monotonic()
    n = 0
    need = CHUNK * 4
    while not stop.is_set():
        for k in pend:
            while True:
                try:
                    d = os.read(fds[k], 65536)
                except BlockingIOError:
                    break
                if not d:
                    break
                pend[k] += d
        frames = {}
        for k in pend:
            take, pend[k] = pend[k][:need], pend[k][need:]
            take += b"\0" * (need - len(take))
            frames[k] = struct.unpack(f"<{CHUNK}i", take)
        for src, dst in (("a_out", "b_in"), ("b_out", "a_in")):
            try:
                os.write(fds[dst], struct.pack(f"<{CHUNK}i", *frames[src]))
            except BlockingIOError:
                pass
        mix = [max(-32768, min(32767, (x + y) >> 16)) for x, y in zip(frames["a_out"], frames["b_out"])]
        record.write(struct.pack(f"<{CHUNK}h", *mix))
        n += 1
        delay = t0 + n * CHUNK / RATE - time.monotonic()
        if delay > 0:
            time.sleep(delay)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mercury")
    ap.add_argument("out")
    ap.add_argument("--caller", default="Q1TEST")
    ap.add_argument("--called", default="Q1RMS-10")
    ap.add_argument("--hold", type=float, default=20.0, help="seconds to stay connected")
    args = ap.parse_args()

    tmp = tempfile.mkdtemp(prefix="mfix-")
    paths = {k: os.path.join(tmp, k) for k in ("a_in", "a_out", "b_in", "b_out")}
    for p in paths.values():
        os.mkfifo(p)
    write_ini(os.path.join(tmp, "a.ini"), 9310, 9110, paths["a_in"], paths["a_out"])
    write_ini(os.path.join(tmp, "b.ini"), 9320, 9120, paths["b_in"], paths["b_out"])

    procs = []
    for tag in ("a", "b"):
        log = open(os.path.join(tmp, f"{tag}.log"), "w")
        procs.append(subprocess.Popen([args.mercury, "-C", os.path.join(tmp, f"{tag}.ini")],
                                      stdout=log, stderr=subprocess.STDOUT, cwd=tmp))
    stop = threading.Event()
    rec = open(args.out, "wb")
    th = threading.Thread(target=bridge, args=(paths["a_out"], paths["b_out"],
                                               paths["a_in"], paths["b_in"], rec, stop))
    th.start()
    ok = False
    try:
        a = Ctl(9310, "client")
        b = Ctl(9320, "rms")
        b.send(f"MYCALL {args.called}")
        b.send("LISTEN ON")
        a.send(f"MYCALL {args.caller}")
        time.sleep(3)  # leading quiet
        a.send(f"CONNECT {args.caller} {args.called}")
        ok = a.wait_for("CONNECTED", 90) is not None
        if ok:
            end = time.time() + args.hold
            while time.time() < end:
                a.pump(); b.pump()
            a.send("DISCONNECT")
            a.wait_for("DISCONNECTED", 60)
        time.sleep(3)  # trailing quiet
    finally:
        stop.set()
        th.join()
        rec.close()
        for p in procs:
            p.terminate()
        for p in procs:
            p.wait(timeout=10)
    print(f"logs in {tmp}; fixture {args.out} ({os.path.getsize(args.out)} bytes); connected={ok}")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
