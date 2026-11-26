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
make -j"$(nproc)" all HAVE_HAMLIB=0 > "$dst.build.log" 2>&1 || { tail -20 "$dst.build.log"; exit 1; }
echo "built $dst/mercury ($(git rev-parse --short HEAD) + patch)"
