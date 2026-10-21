#!/bin/sh
# The published-package consumer gate: install the version from the registry,
# run the example against the mock, exercise the CLI, and resolve the bridge
# through both `require` and a named ESM import.
#
#   verify-published.sh <version>
#
# The version is exact (`0.22.0`) or a dist-tag (`latest`, `next`).
# MOCK_NEXUS_BIN points at a built mock server; the default is the repo's
# target/debug/mock-nexus (or .exe).
#
# Exit codes: 0 the published package works, 1 a check failed, 2 bad input.
set -eu

usage() {
  echo "usage: verify-published.sh <version>"
  echo "  installs @nexus-raw/nxr@<version> and nexus-raw@<version>"
  echo "  runs the published example against MOCK_NEXUS_BIN"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
esac
[ "$#" -eq 1 ] || usage_err
version="$1"
case "$version" in
  ''|*[!A-Za-z0-9._-]*) usage_err ;;
esac

repo="$(cd "$(dirname "$0")/.." && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

mock="${MOCK_NEXUS_BIN:-}"
if [ -z "$mock" ]; then
  if [ -f "$repo/target/debug/mock-nexus.exe" ]; then
    mock="$repo/target/debug/mock-nexus.exe"
  else
    mock="$repo/target/debug/mock-nexus"
  fi
fi
[ -x "$mock" ] || { echo "verify-published: no mock server at $mock, build it first" >&2; exit 1; }

cd "$tmp"
echo '{"name":"published-example","private":true}' > package.json
npm install --no-package-lock --no-save --no-audit --no-fund "@nexus-raw/nxr@$version"
cp "$repo/examples/node/publish-and-consume.mjs" .
MOCK_NEXUS_BIN="$mock" node publish-and-consume.mjs
npx --no-install nxr --version

# The full declared surface resolves, not only the core the example touches.
node <<'EOF'
const fs = require("fs")
const path = require("path")
const nxr = require("@nexus-raw/nxr")
const entry = require.resolve("@nexus-raw/nxr")
const dts = fs.readFileSync(path.join(path.dirname(entry), "index.d.ts"), "utf8")
const declared = [...dts.matchAll(/^export function (\w+)/gm)].map((m) => m[1])
if (declared.length < 10) {
  throw new Error(`verify-published: only ${declared.length} functions parsed from index.d.ts`)
}
const missing = declared.filter((m) => typeof nxr[m] !== "function")
if (missing.length > 0) {
  throw new Error(`verify-published: the surface is missing ${missing.join(", ")}`)
}
console.log(`verify-published: ${declared.length} declared functions resolve`)
EOF

# The bridge is a separate package with its own dependency pin.
mkdir -p bridge
cd bridge
echo '{"name":"bridge-smoke","private":true}' > package.json
npm install --no-package-lock --no-save --no-audit --no-fund "nexus-raw@$version"
node <<'EOF'
const nxr = require("nexus-raw")
const missing = ["up", "down", "clone", "verify"].filter((m) => typeof nxr[m] !== "function")
if (missing.length > 0) {
  throw new Error(`bridge smoke: missing ${missing.join(", ")}`)
}
console.log("bridge smoke: require resolves the current surface")
EOF
cat > smoke.mjs <<'EOF'
import { up, down, clone, verify } from "nexus-raw"
const missing = [up, down, clone, verify].filter((fn) => typeof fn !== "function")
if (missing.length > 0) {
  throw new Error("bridge ESM smoke: named imports are missing")
}
console.log("bridge ESM smoke: named imports resolve")
EOF
node smoke.mjs
