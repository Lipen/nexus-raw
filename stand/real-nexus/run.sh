#!/bin/bash
# The whole real-Nexus stand in one entry point: build, boot the container,
# bootstrap, run the battery, and clean up no matter how the run ends.
# The cleanup trap is the reason this is a script and not a justfile recipe:
# a recipe line that dies leaves the rest of the recipe (the cleanup) unrun,
# while this trap fires on exit, interrupt and termination alike. Verified:
# a group-wide SIGINT (the terminal Ctrl-C) through `just stand` tears the
# container down and leaves no orphans.
# Port and URL knobs: NEXUS_PORT (default 18081) for the host port,
# NEXUS_BASE for a full URL override (CI batteries elsewhere).
# bootstrap.sh and battery.sh read them, this script only orchestrates.
set -eu

DIR=$(cd "$(dirname "$0")" && pwd)

# Colour the step headers and the final status on a terminal.
# NO_COLOR and a dumb TERM keep the output plain for pipes and CI.
if [ -t 2 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != "dumb" ]; then
    c_reset=$'\033[0m'
    c_step=$'\033[1;36m'
    c_ok=$'\033[32m'
    c_fail=$'\033[31m'
    c_dim=$'\033[2m'
else
    c_reset='' c_step='' c_ok='' c_fail='' c_dim=''
fi

step() { printf '\n%s==> %s%s\n' "$c_step" "$*" "$c_reset" >&2; }

compose() { docker compose -f "$DIR/compose.yaml" "$@"; }

cleanup() {
    status=$?
    if [ "$status" -ne 0 ]; then
        printf '\n%s==> stand failed (exit %s); last 200 log lines follow%s\n' "$c_fail" "$status" "$c_reset" >&2
        compose logs --tail 200 >&2 || :
    else
        printf '\n%s==> tearing down%s\n' "$c_dim" "$c_reset" >&2
    fi
    compose down -v || :
    exit "$status"
}
trap cleanup EXIT

step "building nxr"
start=$SECONDS
cargo build --locked -p nexus-raw
step "nxr built in $((SECONDS - start))s"

step "booting nexus3 (the first run pulls the image, ~700 MB)"
compose up -d
step "nexus3 container started"

step "bootstrapping nexus3 (readiness, EULA, repository)"
"$DIR/bootstrap.sh"

step "running the battery"
"$DIR/battery.sh" "$@"

printf '\n%s==> stand passed%s\n' "$c_ok" "$c_reset" >&2
