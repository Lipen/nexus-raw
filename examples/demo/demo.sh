#!/usr/bin/env bash
#
# The local stand: a full nexus-raw session against the repo's mock server and
# stub payloads, with every command printed as it runs.
#
#   just demo                     # the story: publish, name, consume, verify, refuse
#   just demo flaky               # the same story against another failure scenario
#   just demo slow --chunk-delay-ms 400 --chunk-size 4096
#
# Nothing is simulated at the nexus-raw layer: the `nxr` binary that ships runs
# these commands, and the mock server is the one the conformance suites use.
# The only stub is the payload directory, and it is generated deterministically,
# so digests and transcripts stay stable between runs.
#
# Two modes beyond the plain run:
#
#   NXR_DEMO_CAST=<file>   record the session (`just demo-cast` writes the docs
#                          landing animation): every command and every output
#                          line lands in the file with its own timestamp.
#   NXR_DEMO_KEEP=1        leave the last mock server running and print its URL.
#   NXR_DEMO_EXTRA=0       skip the extra failure-scenario phases at the end.

set -uo pipefail

SCENARIO="${1:-slow}"
shift || true

INVOKED_FROM="$PWD"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"

# The commands print as a user types them: `nxr` from PATH, paths relative to
# this directory. Absolute scratch paths would make a recording unreadable.
export PATH="$ROOT/target/debug:$PATH"
MOCK="mock-nexus"
NXR="nxr"

DIST="dist/1.4.0"
VENDOR="vendor/1.4.0"
VERSION="1.4.0"
EXTRA="${NXR_DEMO_EXTRA:-1}"

# Ambient credentials would change the session, and the mock wants none.
unset NXR_AUTH NXR_USERNAME NXR_PASSWORD

CAST_RAW=""
CAST_START_MS=0
MOCK_PIDS=()
MOCK_URL=""

cleanup() {
  for pid in "${MOCK_PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null
  done
  [ -n "$CAST_RAW" ] && rm -f "$CAST_RAW"
}
trap cleanup EXIT

# ---- output --------------------------------------------------------------

now_ms() {
  if [ -n "${EPOCHREALTIME:-}" ]; then
    printf '%s' "$(( ${EPOCHREALTIME/./} / 1000 ))"
  else
    printf '%s' "$(( $(date +%s) * 1000 ))"
  fi
}

# `say` is part of the recorded session; `note` is commentary for the human.
say() {
  printf '%s\n' "$1"
  if [ -n "$CAST_RAW" ]; then
    local text
    text="$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\t/\\t/g')"
    printf '["%s","%s"]\n' "$(( $(now_ms) - CAST_START_MS ))" "$text" >> "$CAST_RAW"
  fi
}

note() {
  printf '%s\n' "$1"
}

# Assemble the recorded lines into the file the docs player reads.
# `paste -sd,` also handles the trailing-comma problem.
write_cast() {
  [ -n "${NXR_DEMO_CAST:-}" ] || return 0
  local out="$NXR_DEMO_CAST"
  mkdir -p "$(dirname "$out")"
  {
    printf '{"v":1,"scenario":"%s","version":"%s","tool":"%s","lines":[\n' \
      "$SCENARIO" "$VERSION" "$("$NXR" --version 2>/dev/null | head -1)"
    paste -sd, "$CAST_RAW"
    printf '\n]}\n'
  } > "$out"
  note "cast written: $out ($(wc -l < "$CAST_RAW") lines)"
}

# Run a command: print it, then stream its output line by line as it arrives.
# The command's own exit status is what `run` returns, so a caller can branch.
run() {
  say "\$ $*"
  "$@" 2>&1 | while IFS= read -r line; do say "$line"; done
  return "${PIPESTATUS[0]}"
}

# Run a command that is expected to fail, and record the code it exited with.
run_expecting_failure() {
  local status=0
  run "$@" || status=$?
  say "(exit $status)"
}

# ---- payloads ------------------------------------------------------------

# Deterministic filler: the same bytes every run, so the digests in a recording
# stay stable. /dev/urandom would make every session print different output.
fill() {
  local path="$1" bytes="$2" seed="${3:-nxr demo payload line}"
  mkdir -p "$(dirname "$path")"
  yes "$seed" | head -c "$bytes" > "$path" || true
}

build_payloads() {
  rm -rf dist vendor
  fill "$DIST/app-1.4.0.zip" 98304 "application bytes for the 1.4.0 release"
  fill "$DIST/bom/linux-x86_64.json" 8192 '{"component":"demo","platform":"linux-x86_64"}'
  fill "$DIST/pinned.xml" 512 '<pinned><component name="demo"/></pinned>'
  cat > "$DIST/manifest.json" <<'JSON'
{
  "artifacts": ["app-1.4.0.zip", "bom/linux-x86_64.json", "pinned.xml"]
}
JSON
}

# ---- the mock ------------------------------------------------------------

# Start the mock server on an ephemeral port; its URL arrives on stdout.
# A fixed port would collide with whatever else runs on this machine.
start_mock() {
  local scenario="$1"
  shift
  local log
  log="$(mktemp)"
  "$MOCK" "$scenario" --port 0 "$@" > "$log" 2>&1 &
  local pid=$!
  MOCK_PIDS+=("$pid")

  local url=""
  for _ in $(seq 1 200); do
    url="$(grep -m1 -o 'listening http://[^ ]*' "$log" 2>/dev/null || true)"
    [ -n "$url" ] && break
    if ! kill -0 "$pid" 2>/dev/null; then
      note "mock-nexus ($scenario) exited before it listened:"
      head -5 "$log"
      exit 1
    fi
    sleep 0.05
  done
  [ -n "$url" ] || { note "mock-nexus ($scenario) never reported a port"; exit 1; }

  MOCK_URL="${url#listening }"
}

# ---- session -------------------------------------------------------------

if [ ! -x "$ROOT/target/debug/mock-nexus" ] || [ ! -x "$ROOT/target/debug/nxr" ]; then
  note "building mock-nexus and nxr (first run only)"
  (cd "$ROOT" && cargo build -q -p mock-nexus -p nexus-raw) || exit 1
fi

cd "$HERE"

if [ -n "${NXR_DEMO_CAST:-}" ]; then
  # The session runs from this directory; a relative cast path belongs to the
  # caller's, so resolve it before any `cd`.
  case "$NXR_DEMO_CAST" in
    /*) ;;
    *) NXR_DEMO_CAST="$INVOKED_FROM/$NXR_DEMO_CAST" ;;
  esac
  CAST_RAW="$(mktemp)"
  CAST_START_MS="$(now_ms)"
fi

note "nexus-raw demo stand: mock server + stub payloads, every command real"
note "scenario: $SCENARIO    payloads: dist/1.4.0/"
note ""

build_payloads
start_mock "$SCENARIO" "$@"
REPO="${MOCK_URL%/}"

note "--- publish a version directory ----------------------------------"
run "$NXR" up "$DIST/" "$REPO/$VERSION/" --claim-first manifest.json --workers 1 -v
say ""

note "--- name it, so consumers never hard-code a version --------------"
run "$NXR" channel set "$REPO/latest" "$VERSION" --if-forward
say "\$ V=\$(nxr channel get $REPO/latest)"
VERSION_AT_CHANNEL="$("$NXR" channel get "$REPO/latest")"
say ""

note "--- consume exactly that version ---------------------------------"
run "$NXR" down "$REPO/$VERSION_AT_CHANNEL/" "$VENDOR/" --workers 1 -v
say ""

note "--- verify offline, no network -----------------------------------"
run "$NXR" verify "$VENDOR/"
say ""

note "--- the part that matters: a diverged local file -----------------"
note "someone edited a downloaded artifact: bytes and marker no longer agree"
say "\$ printf 'edited by hand\\n' >> vendor/1.4.0/app-1.4.0.zip"
printf 'edited by hand\n' >> "$VENDOR/app-1.4.0.zip"
run_expecting_failure "$NXR" verify "$VENDOR/"
say ""
note "the command that fetched it now refuses to overwrite it"
run_expecting_failure "$NXR" down "$REPO/$VERSION_AT_CHANNEL/" "$VENDOR/" --workers 1
say ""
note "delete the bad copy: a re-run fetches exactly what is missing"
say "\$ rm -f vendor/1.4.0/app-1.4.0.zip vendor/1.4.0/app-1.4.0.zip.sha256"
rm -f "$VENDOR/app-1.4.0.zip" "$VENDOR/app-1.4.0.zip.sha256"
run "$NXR" down "$REPO/$VERSION_AT_CHANNEL/" "$VENDOR/" --workers 1
run "$NXR" verify "$VENDOR/"
say ""

if [ "$EXTRA" != "0" ]; then
  note "--- the weak link: the same publish over a flaky server ----------"
  start_mock flaky --flaky 1
  run "$NXR" up "$DIST/" "${MOCK_URL%/}/$VERSION/" --claim-first manifest.json --workers 1
  say ""

  note "--- and a server that wants credentials --------------------------"
  start_mock auth-401 --auth ci:secret
  AUTHED="${MOCK_URL%/}"
  note "the server answers 401: transport, not data, so the exit code is 3"
  run_expecting_failure "$NXR" up "$DIST/" "$AUTHED/$VERSION/" -u ci:wrong
  note "the same command with the right credentials"
  run env NXR_USERNAME=ci NXR_PASSWORD=secret "$NXR" up "$DIST/" "$AUTHED/$VERSION/" -q
  say ""
fi

note "exit codes: 0 ok, 1 data (divergence, missing, failed digest), 2 misuse, 3 transport"
note "scenarios the mock speaks: $(cd "$ROOT" && "$MOCK" --print-scenarios | tr -d '\n ' | sed 's/\[//; s/\]//; s/"//g')"
note ""
note "poke it yourself:  just mock flaky   |   just nxr -- up --help"
write_cast
if [ -n "${NXR_DEMO_KEEP:-}" ]; then
  note "the last mock is still serving ${MOCK_URL} (scenario $SCENARIO); ctrl-c to stop it"
  wait
fi
