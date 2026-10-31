#!/usr/bin/env bash
set -euo pipefail

binary=${1:?expected path to GNU/Linux binary}
versions=$(readelf -W --version-info "$binary")
# Non-numeric requirements such as GLIBC_ABI_DT_RELR (glibc 2.36+) carry no
# version number to compare, so reject any of them outright.
if abi=$(grep -oE 'GLIBC_ABI_[A-Za-z0-9_]+' <<<"$versions" | sort -u) && test -n "$abi"; then
  printf 'unexpected glibc ABI requirement: %s\n' $abi >&2
  exit 1
fi
maximum=$(grep -oE 'GLIBC_[0-9]+(\.[0-9]+)+' <<<"$versions" | sort -Vu | tail -n 1)
test -n "$maximum"
test "$(printf '%s\n%s\n' "$maximum" GLIBC_2.28 | sort -V | tail -n 1)" = GLIBC_2.28
"$binary" --version
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
printf '%s\n' '{"name":"xray-release-smoke","version":"1.0.0","main":"index.js"}' > "$fixture/package.json"
printf '%s\n' '// @flow' 'import type { T } from "sample-types";' > "$fixture/index.js"
"$binary" --package "$fixture" --json > "$fixture/result.json"
node -e 'const r=require(process.argv[1]); if (!r.complete || !r.packages.some(p=>p.findings.some(f=>f.dependency==="sample-types"))) process.exit(1)' "$fixture/result.json"
