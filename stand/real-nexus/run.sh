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

# shellcheck source=../../scripts/ui.sh
. "$DIR/../../scripts/ui.sh"

compose() { docker compose -f "$DIR/compose.yaml" "$@"; }

cleanup() {
    status=$?
    if [ "$status" -ne 0 ]; then
        ui_error "stand" "failed (exit $status); last 200 log lines follow"
        compose logs --tail 200 >&2 || :
    else
        ui_plain "tearing down"
    fi
    compose down -v || :
    exit "$status"
}
trap cleanup EXIT

ui_phase "build" "building nxr"
start=$SECONDS
cargo build --locked -p nexus-raw
ui_ok "nxr built in $((SECONDS - start))s"

ui_phase "boot" "nexus3 (the first run pulls the image, ~700 MB)"
# A container left behind by a killed run would serve its old store, and the
# next battery would meet diverged bytes: start from nothing, always.
compose down -v > /dev/null 2>&1 || :
compose up -d
ui_ok "nexus3 container started"

ui_phase "bootstrap" "readiness, EULA, repository"
"$DIR/bootstrap.sh"

ui_phase "battery" "running"
"$DIR/battery.sh" "$@"

ui_success "stand" "passed"
