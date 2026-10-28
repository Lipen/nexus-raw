#!/bin/sh
# The docs gate: the site builds strictly and every docs contract holds.
# One list shared by `just check-docs`, the Docs workflow and the release
# gate, so a check added here cannot be missing from one of them.
#
#   check-docs.sh [zensical args...]
#
# The optional arguments go to `zensical build`, for a local preview.
#
# Exit codes: 0 clean, 1 a check or the build failed.
set -eu

here="$(dirname "$0")"

# shellcheck source=ui.sh
. "$here/ui.sh"

run_check() {
    label=$1
    shift
    ui_phase "docs" "checking $label"
    if ! output=$("$@" 2>&1); then
        ui_fail "$label"
        printf '%s\n' "$output" >&2
        exit 1
    fi
    ui_ok "$label"
}

run_check "hints" "$here/check-hints.sh"
run_check "the CLI surface" "$here/check-cli-surface.sh"
run_check "llms.txt" "$here/llms-check.sh"
run_check "the bridge surface" "$here/check-bridge-surface.sh"
run_check "the skill" "$here/check-skill.sh"
run_check "action pins" "$here/check-action-pins.sh"

ui_phase "docs" "building the site"
uvx zensical@0.0.73 build --strict "$@" || {
    ui_fail "docs build"
    ui_hint 'run `just docs` to preview the site'
    exit 1
}
ui_ok "docs build"
