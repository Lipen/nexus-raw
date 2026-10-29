#!/bin/sh
# The tap formula follows the latest published release, not the workspace
# manifest: it can only name a version whose archives exist, so a version
# bump on master is ahead of it until the release lands. A forgotten bump
# after a release leaves `brew upgrade` on the previous version in silence,
# which is what this check catches.
#
#   check-brew-formula.sh
#
# Exit codes:
#   0 the formula names the latest release and matches its SHA256SUMS
#   1 the formula lags the latest release, or drifted from it
#   2 usage, or the latest release could not be read
set -eu

usage() {
  echo "usage: check-brew-formula.sh"
  echo "  checks Formula/nexus-raw.rb against the latest published release"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h | --help)
    usage
    exit 0
    ;;
  "") ;;
  *) usage_err ;;
esac

here=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$here"

formula=Formula/nexus-raw.rb
[ -f "$formula" ] || {
  echo "check-brew-formula: no $formula (run from the repo root)" >&2
  exit 1
}

# The latest release tag: the redirect target of the releases page, so no
# token is involved and the check runs anywhere.
url=https://github.com/Lipen/nexus-raw/releases/latest
resolved=$(curl -fsSL -o /dev/null -w '%{url_effective}' "$url") || {
  echo "check-brew-formula: cannot resolve $url" >&2
  exit 2
}
latest=${resolved##*/tag/v}
case "$latest" in
  "$resolved" | '')
    echo "check-brew-formula: no release tag in $resolved" >&2
    exit 2
    ;;
  *[!0-9A-Za-z._-]*)
    echo "check-brew-formula: odd release tag in $resolved" >&2
    exit 2
    ;;
esac

named=$(sed -n 's/^  version "\(.*\)"$/\1/p' "$formula")
[ -n "$named" ] || {
  echo "check-brew-formula: no version line in $formula" >&2
  exit 1
}
if [ "$named" != "$latest" ]; then
  echo "check-brew-formula: the formula names $named, the latest release is v$latest" >&2
  echo "check-brew-formula: run \`just brew-formula $latest\` and commit the result" >&2
  exit 1
fi

# Same version: the committed file must equal a fresh render of that release,
# which catches a hand-edited checksum and a half-done bump alike.
exec "$here/scripts/brew-formula.sh" "$latest" --check
