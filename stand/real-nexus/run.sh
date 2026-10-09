#!/bin/bash
# The whole real-Nexus stand in one entry point: build, boot the container,
# bootstrap, run the battery, and clean up no matter how the run ends.
# The cleanup trap is the reason this is a script and not a justfile recipe:
# a trap runs on Ctrl-C, a recipe line simply dies and orphans the container.
# Known hole when run through `just`: just does not forward SIGINT to the
# recipe's children, so Ctrl-C kills just and orphans this script and the
# container; run this script directly for a graceful Ctrl-C, or clean up with
# `docker compose -f stand/real-nexus/compose.yaml down -v`.
# Port and URL knobs: NEXUS_PORT (default 18081) for the host port,
# NEXUS_BASE for a full URL override (CI batteries elsewhere).
set -eu

DIR=$(cd "$(dirname "$0")" && pwd)
BASE="${NEXUS_BASE:-http://localhost:${NEXUS_PORT:-18081}}"

compose() { docker compose -f "$DIR/compose.yaml" "$@"; }

cleanup() {
    status=$?
    if [ "$status" -ne 0 ]; then
        compose logs --tail 200 >&2 || :
    fi
    compose down -v || :
    exit "$status"
}
trap cleanup EXIT

echo "==> building nxr" >&2
cargo build -q --locked -p nexus-raw

echo "==> booting nexus3 (the first run pulls the image, ~700 MB)" >&2
compose up -d

"$DIR/bootstrap.sh"

echo "==> running the battery" >&2
"$DIR/battery.sh" "$@"
