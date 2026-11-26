#!/bin/sh
# Build a release tarball for another Linux platform in a container.
#   scripts/build-in-docker.sh linux/arm64          (ubuntu base)
#   scripts/build-in-docker.sh linux/arm/v7
#   scripts/build-in-docker.sh linux/riscv64
#   scripts/build-in-docker.sh linux/amd64 alpine   (musl)
# Non-native platforms run under QEMU (binfmt): slow, but exact. Output in dist/.
set -eu
plat=${1:?platform, e.g. linux/arm64}
base=${2:-ubuntu}
here=$(cd "$(dirname "$0")/.." && pwd)
eng=$(command -v docker || command -v podman)
tag="openrms-watch-build:$base-$(echo "$plat" | tr '/' '-')"
"$eng" build --platform "$plat" -t "$tag" -f "$here/packaging/docker/Dockerfile.$base" "$here/packaging/docker"
# a separate target dir and Mercury build per platform, so hosts don't clash
key=$(echo "$base-$plat" | tr '/' '-')
# The container runs as root: trust the mounted repositories (safe.directory only
# counts in global config, not env), and hand every
# file it writes back to the caller when it is done.
ref=""
[ -d "$here/../mercury/.git" ] && ref="-v $here/../mercury:/mercury-ref:ro"
"$eng" run --rm --platform "$plat" \
  -v "$here":/src $ref \
  -e CARGO_TARGET_DIR=/src/.build/target-$key \
  -e MERCURY_BUILD_DIR=/src/.build/mercury-$key \
  -e HOST_UID="$(id -u)" -e HOST_GID="$(id -g)" \
  "$tag" sh -c 'git config --global --add safe.directory "*"; rc=0; scripts/package.sh || rc=$?; chown -R "$HOST_UID:$HOST_GID" /src/dist /src/.build/target-* /src/.build/mercury-* 2>/dev/null; exit $rc'
