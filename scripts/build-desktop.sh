#!/bin/sh
# Build the desktop app packages (.deb, .rpm, AppImage) with the engine and the
# patched Mercury bundled inside. Output: desktop/target/release/bundle/
set -eu
here=$(cd "$(dirname "$0")/.." && pwd)
cd "$here"
[ -x .build/mercury/mercury ] || scripts/build-mercury.sh
cargo build --release
triple=$(rustc -vV | sed -n 's/^host: //p')
mkdir -p desktop/binaries desktop/resources
install -m 755 target/release/openrms-watch "desktop/binaries/openrms-watch-$triple"
install -m 755 .build/mercury/mercury desktop/resources/mercury
cd desktop
cargo tauri build "$@"
ls -1 target/release/bundle/*/* 2>/dev/null | grep -E '\.(deb|rpm|AppImage)$'
