#!/usr/bin/env bash
set -euo pipefail

# SHA-256 values from Zig's official 0.14.1 download index.
case "${RUNNER_ARCH:?}" in
  X64) arch=x86_64; sha=24aeeec8af16c381934a6cd7d95c807a8cb2cf7df9fa40d359aa884195c4716c ;;
  ARM64) arch=aarch64; sha=f7a654acc967864f7a050ddacfaa778c7504a0eca8d2b678839c21eea47c992b ;;
  *) echo "Unsupported runner architecture" >&2; exit 1 ;;
esac
archive="${RUNNER_TEMP:?}/zig.tar.xz"
curl -fsSL "https://ziglang.org/download/0.14.1/zig-${arch}-linux-0.14.1.tar.xz" -o "$archive"
printf '%s  %s\n' "$sha" "$archive" | sha256sum -c -
tar -xJf "$archive" -C "$RUNNER_TEMP"
printf '%s\n' "$RUNNER_TEMP/zig-${arch}-linux-0.14.1" >> "${GITHUB_PATH:?}"
