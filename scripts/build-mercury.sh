#!/bin/sh
# Build the Mercury the watcher runs: upstream at a pinned commit, plus
# patches/mercury-monitor-crc.patch (CRC fields on MONITOR lines, MFSK500 in
# the monitor, MERCURY_MONITOR_CONNECT_ONLY). Output: .build/mercury/mercury
#
# Usage: scripts/build-mercury.sh [mercury-git-url-or-path]
# Default source is the reference clone ../mercury, else GitHub.
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
src=${1:-}
if [ -z "$src" ]; then
  if [ -d "$here/../mercury/.git" ]; then src="$here/../mercury"
  elif [ -d /mercury-ref/.git ]; then src=/mercury-ref
  else src=https://github.com/Rhizomatica/mercury.git; fi
fi
commit=$(cat "$here/patches/MERCURY_COMMIT")
dst="${MERCURY_BUILD_DIR:-$here/.build/mercury}"
mkdir -p "$here/.build"
[ -d "$dst/.git" ] || git clone -q "$src" "$dst"
cd "$dst"
git fetch -q origin 2>/dev/null || true
git checkout -q -f "$commit"
git apply "$here/patches/mercury-monitor-crc.patch"
# Only Mercury and its libraries (not its developer utilities). Extra link flags
# go in SAN_LDFLAGS, the variable at the end of the link line (earlier ones can
# be dropped by --as-needed). RISC-V needs libatomic for 1-byte atomics.
ld=""
case "$(uname -m)" in riscv64) ld="-latomic" ;; esac
# musl's default thread stack is 128 KiB (glibc: 8 MiB) and Mercury's decoder
# threads keep large buffers on the stack; musl takes the default from this.
if ldd --version 2>&1 | grep -qi musl; then ld="$ld -Wl,-z,stack-size=8388608"; fi
extra=""
[ -n "$ld" ] && extra="SAN_LDFLAGS=$ld"
{ make -j"$(nproc)" internal_deps HAVE_HAMLIB=0 && make -j"$(nproc)" mercury HAVE_HAMLIB=0 ${extra:+"$extra"}; } \
  > "$dst.build.log" 2>&1 || { tail -20 "$dst.build.log"; exit 1; }
echo "built $dst/mercury ($(git rev-parse --short HEAD) + patch)"
