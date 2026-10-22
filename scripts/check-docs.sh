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
"$here/check-hints.sh"
"$here/check-cli-surface.sh"
"$here/llms-check.sh"
"$here/check-bridge-surface.sh"
"$here/check-skill.sh"
"$here/check-action-pins.sh"
uvx zensical@0.0.71 build --strict "$@"
