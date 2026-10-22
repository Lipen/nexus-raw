#!/usr/bin/env bash
#
# A full nexus-raw session against the repo's mock server and generated payloads.
# Every command prints as it runs, exactly as a user would type it.
#
#   just demo                     # publish, name, consume, verify, refuse
#   just demo flaky               # the same session against another failure scenario
#   just demo slow --chunk-delay-ms 400 --chunk-size 4096
#
# The `nxr` and `mock-nexus` binaries are real.
# The only stub is the payload directory, generated deterministically, so digests and transcripts stay stable between runs.
#
# Environment:
#   NXR_DEMO_SCOPE=core    stop after verify: the four beats the landing animation shows
#   NXR_DEMO_CAST=<file>   record the session: every command and output line with its arrival time, bucketed to 50 ms (`just demo-cast` writes the docs landing animation)
#   NXR_DEMO_KEEP=1        leave the last mock server running and print its URL

set -uo pipefail

SCENARIO="${1:-slow}"
shift || true

INVOKED_FROM="$PWD"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"

# The shared terminal style (colour off for pipes and NO_COLOR).
. "$ROOT/scripts/ui.sh"

# The commands print as a user types them: `nxr` from PATH, paths relative to this directory.
# Absolute scratch paths would make a recording unreadable.
export PATH="$ROOT/target/debug:$PATH"
MOCK="mock-nexus"
NXR="nxr"

DIST="dist/1.4.0"
VENDOR="vendor/1.4.0"
VERSION="1.4.0"
SCOPE="${NXR_DEMO_SCOPE:-full}"

# Ambient credentials would change the session, and the mock wants none.
unset NXR_AUTH NXR_USERNAME NXR_PASSWORD

# Recorded times are bucketed: a millisecond-exact recording of a local transfer is machine jitter, and it would churn the diff of a committed file on every regeneration.
# The pauses that matter are seconds long.
CAST_QUANTUM_MS=50
CAST_RAW=""
CAST_START_MS=0
MOCK_PIDS=()
MOCK_URL=""
MAIN_MOCK_PID=""
MAIN_MOCK_URL=""

cleanup() {
  for pid in "${MOCK_PIDS[@]:-}"; do
    kill "$pid" 2>/dev/null
  done
  [ -n "$CAST_RAW" ] && rm -f "$CAST_RAW"
}
trap cleanup EXIT INT TERM

# ---- output --------------------------------------------------------------

now_ms() {
  if [ -n "${EPOCHREALTIME:-}" ]; then
    printf '%s' "$(( ${EPOCHREALTIME/./} / 1000 ))"
  else
    printf '%s' "$(( $(date +%s) * 1000 ))"
  fi
}

# `say` is part of the recorded session.
# The display may style the line, but `CAST_RAW` takes `$1` unchanged.
# The cast and the landing SVG must stay ANSI-free.
# `note` is commentary for the human.
say() {
  case "$1" in
    '$ '*) ui_command "$1" ;;
    '# '*) printf '%s%s%s\n' "$UI_DIM" "$1" "$UI_RESET" ;;
    '(exit '*) printf '%s%s%s\n' "$UI_DIM" "$1" "$UI_RESET" ;;
    *) printf '%s\n' "$1" ;;
  esac
  if [ -n "$CAST_RAW" ]; then
    local text t
    text="$(printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\t/\\t/g')"
    t="$(( ($(now_ms) - CAST_START_MS) / CAST_QUANTUM_MS * CAST_QUANTUM_MS ))"
    printf '[%s,"%s"]\n' "$t" "$text" >> "$CAST_RAW"
  fi
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
  ui_note "cast" "written: $out ($(wc -l < "$CAST_RAW") lines)"
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

# Deterministic filler: the same bytes every run, so the digests in a recording stay stable.
# /dev/urandom would make every session print different output.
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
  printf 'app-1.4.0.zip\nbom/linux-x86_64.json\npinned.xml\n' > "$DIST/names.txt"
}

# ---- the mock ------------------------------------------------------------

# Start the mock server on an ephemeral port.
# Its URL arrives on stdout.
# A fixed port would collide with whatever else runs on this machine.
start_mock() {
  local scenario="$1"
  shift
  local log
  log="$(mktemp)"
  # Ephemeral by default, but a caller-supplied --port wins: passing both would work only by accident of parse order.
  local port_args=(--port 0)
  local arg
  for arg in "$@"; do
    [ "$arg" = "--port" ] && port_args=()
  done
  "$MOCK" "$scenario" "${port_args[@]}" "$@" > "$log" 2>&1 &
  local pid=$!
  MOCK_PIDS+=("$pid")

  local url=""
  for _ in $(seq 1 200); do
    url="$(grep -m1 -o 'listening http://[^ ]*' "$log" 2>/dev/null || true)"
    [ -n "$url" ] && break
    if ! kill -0 "$pid" 2>/dev/null; then
      ui_fail "mock-nexus ($scenario) exited before it listened"
      head -5 "$log"
      exit 1
    fi
    sleep 0.05
  done
  [ -n "$url" ] || { ui_fail "mock-nexus ($scenario) never reported a port"; exit 1; }

  MOCK_URL="${url#listening }"
  if [ -z "$MAIN_MOCK_PID" ]; then
    MAIN_MOCK_PID="$pid"
    MAIN_MOCK_URL="$MOCK_URL"
  fi
}

# ---- session -------------------------------------------------------------

ui_phase "demo" "nexus-raw session"
ui_plain "scenario: $SCENARIO    payloads: dist/1.4.0/"

ui_phase "build" "the demo binaries"
if ! (cd "$ROOT" && cargo build -q -p mock-nexus -p nexus-raw); then
  ui_fail "the demo binaries did not build"
  exit 1
fi
ui_ok "binaries built"

cd "$HERE" || exit 1

if [ -n "${NXR_DEMO_CAST:-}" ]; then
  # The session runs from this directory; a relative cast path belongs to the caller's, so resolve it before any `cd`.
  case "$NXR_DEMO_CAST" in
    /*) ;;
    *) NXR_DEMO_CAST="$INVOKED_FROM/$NXR_DEMO_CAST" ;;
  esac
  CAST_RAW="$(mktemp)"
  CAST_START_MS="$(now_ms)"
fi


build_payloads
start_mock "$SCENARIO" "$@"
REPO="${MOCK_URL%/}"

ui_phase "publish" "a version directory"
run "$NXR" up "$DIST/" "$REPO/$VERSION/" --claim-first app-1.4.0.zip --workers 1
say ""

ui_phase "name" "consumers never hard-code a version"
say "# give the version a name, so consumers never hard-code it"
run "$NXR" channel set "$REPO/latest" "$VERSION" --if-forward
# A real `channel get`, not a `V=$(...)` pseudo-prompt: the shell prints nothing for an assignment, and a command with no output reads as one that did nothing.
# The recorded hero skips the read-back and names the version it just set.
if [ "$SCOPE" = "full" ]; then
  say ""
  say "# read the pointer back"
  run "$NXR" channel get "$REPO/latest"
  VERSION_AT_CHANNEL="$("$NXR" channel get "$REPO/latest")"
else
  VERSION_AT_CHANNEL="$VERSION"
fi
say ""

ui_phase "consume" "exactly that version"
say "# fetch that version back, digests checked on the way in"
run "$NXR" down "$REPO/$VERSION_AT_CHANNEL/" "$VENDOR/" --names-from "$DIST/names.txt" --workers 1
say ""

ui_phase "verify" "offline, no network"
say "# check every digest offline, no network"
run "$NXR" verify "$VENDOR/"

if [ "$SCOPE" = "full" ]; then
say ""
say "# someone edits a downloaded artifact: bytes and marker disagree"
say "\$ printf 'edited by hand\\n' >> vendor/1.4.0/app-1.4.0.zip"
printf 'edited by hand\n' >> "$VENDOR/app-1.4.0.zip"
run_expecting_failure "$NXR" verify "$VENDOR/"
say ""
say "# the command that fetched it refuses to overwrite a diverged file"
run_expecting_failure "$NXR" down "$REPO/$VERSION_AT_CHANNEL/" "$VENDOR/" --names-from "$DIST/names.txt" --workers 1
say ""
say "# delete the bad copy; a re-run fetches exactly what is missing"
say "\$ rm -f vendor/1.4.0/app-1.4.0.zip vendor/1.4.0/app-1.4.0.zip.sha256"
rm -f "$VENDOR/app-1.4.0.zip" "$VENDOR/app-1.4.0.zip.sha256"
run "$NXR" down "$REPO/$VERSION_AT_CHANNEL/" "$VENDOR/" --names-from "$DIST/names.txt" --workers 1
run "$NXR" verify "$VENDOR/"
say ""
fi

if [ "$SCOPE" = "full" ]; then
  say ""
  say "# the same publish, over a server that fails the first request per path"
  start_mock flaky --flaky 1
  run "$NXR" up "$DIST/" "${MOCK_URL%/}/$VERSION/" --claim-first app-1.4.0.zip --workers 1
  say ""
  say "# a server that wants credentials: wrong ones are transport, exit 3"
  start_mock auth-401 --auth ci:secret
  AUTHED="${MOCK_URL%/}"
  run_expecting_failure "$NXR" up "$DIST/" "$AUTHED/$VERSION/" -u ci:wrong
  say "# the right credentials, and it completes"
  run env NXR_USERNAME=ci NXR_PASSWORD=secret "$NXR" up "$DIST/" "$AUTHED/$VERSION/" -q
  say ""
fi

ui_note "exit" "0 ok, 1 data (divergence, missing, failed digest), 2 misuse, 3 transport"
ui_note "mock" "$(cd "$ROOT" && "$MOCK" --print-scenarios | grep -c '"') scenarios, run \`just mock --print-scenarios\` to list them"
ui_phase "try" "poke it yourself"
ui_plain "just mock flaky           -- serve the flaky scenario"
ui_plain "just nxr -- up --help        -- show the upload flags"
write_cast
if [ -n "${NXR_DEMO_KEEP:-}" ]; then
  # Hand over the main server, the one holding the published version, and drop
  # the failure-scenario servers: a poke session wants that version, not the
  # auth wall of whichever scenario started last.
  for pid in "${MOCK_PIDS[@]}"; do
    [ "$pid" = "$MAIN_MOCK_PID" ] && continue
    kill "$pid" 2>/dev/null
  done
  MOCK_PIDS=("$MAIN_MOCK_PID")
  ui_note "mock" "still serving $MAIN_MOCK_URL (scenario $SCENARIO); ctrl-c to stop it"
  wait "$MAIN_MOCK_PID"
fi
