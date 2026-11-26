#!/bin/sh
# Build a release tarball for this machine's architecture:
#   dist/openrms-watch-<version>-linux-<arch>.tar.gz
# containing bin/openrms-watch, bin/mercury (patched), packaging/, install.sh,
# and source/ (the Mercury patch and commit, for GPL compliance).
# Cross-architecture builds run in CI (see PLAN.md section 6).
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
arch=$(uname -m)
[ -f /etc/alpine-release ] && arch="$arch-musl"
merc="${MERCURY_BUILD_DIR:-.build/mercury}"
tgt="${CARGO_TARGET_DIR:-target}"
[ -x "$merc/mercury" ] || scripts/build-mercury.sh
cargo build --release
name="openrms-watch-$ver-linux-$arch"
out="dist/$name"
rm -rf "$out"; mkdir -p "$out/bin" "$out/source"
install -m 755 "$tgt/release/openrms-watch" "$out/bin/"
install -m 755 "$merc/mercury" "$out/bin/"
strip "$out/bin/openrms-watch" 2>/dev/null || true
cp -r packaging "$out/packaging"
cp packaging/install.sh "$out/install.sh"
cp patches/mercury-monitor-crc.patch patches/MERCURY_COMMIT "$out/source/"
cp README.md PLAN.md "$out/"
tar -C dist -czf "dist/$name.tar.gz" "$name"
echo "dist/$name.tar.gz ($(du -h "dist/$name.tar.gz" | cut -f1))"
