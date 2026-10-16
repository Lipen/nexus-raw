#!/bin/sh
# Assert the bridge package re-exports exactly the main package's surface:
# the shim's whole value is that `nexus-raw` keeps meaning the same thing as
# `@nexus-raw/nxr`, and a one-sided export breaks that quietly.
#
# Called from `just check-docs`: a binding surface change without the bridge
# is a red gate here, not a silent shim gap.
set -eu

usage() {
  echo "usage: check-bridge-surface.sh"
  echo "  asserts packaging/npm-bridge re-exports crates/nexus-raw-napi's exports 1:1"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
  "") ;;
  *) usage_err ;;
esac

main_idx=crates/nexus-raw-napi/index.js
bridge_idx=packaging/npm-bridge/index.js
[ -f "$main_idx" ] || { echo "check-bridge-surface: no $main_idx" >&2; exit 1; }
[ -f "$bridge_idx" ] || { echo "check-bridge-surface: no $bridge_idx" >&2; exit 1; }

main_exports="$(grep -o 'exports\.[a-zA-Z]*' "$main_idx" | sort -u)"
bridge_exports="$(grep -o 'exports\.[a-zA-Z]*' "$bridge_idx" | sort -u)"

failed=0
if [ "$(printf '%s\n' "$main_exports")" != "$(printf '%s\n' "$bridge_exports")" ]; then
  echo "check-bridge-surface: the bridge surface diverged from the main package:" >&2
  diff <(printf '%s\n' "$main_exports") <(printf '%s\n' "$bridge_exports") >&2 || true
  echo "check-bridge-surface: mirror every exports.<name> line 1:1" >&2
  failed=1
fi

[ "$failed" = 0 ] || exit 1
echo "check-bridge-surface: $(printf '%s\n' "$main_exports" | wc -l | tr -d ' ') exports mirror"
