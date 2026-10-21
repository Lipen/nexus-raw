#!/bin/sh
# The release tag rule: vX.Y.Z is either free or points at HEAD.
# The preview uses the codes to render ok, note or block; the release
# workflow uses them to fail before any build or publish.
#
#   check-release-tag.sh <version> [remote]
#
# Exit codes:
#   0 the tag is free
#   1 the tag exists on another commit
#   2 the remote could not be checked
#   3 the tag points at HEAD
#
# The remote argument defaults to origin.
set -eu

usage() {
  echo "usage: check-release-tag.sh <version> [remote]"
  echo "  0 free, 1 another commit, 2 unreachable, 3 on HEAD"
}
usage_err() { usage >&2; exit 2; }
case "${1:-}" in
  -h|--help) usage; exit 0 ;;
esac
[ "$#" -ge 1 ] && [ "$#" -le 2 ] || usage_err
version="$1"
remote="${2:-origin}"
case "$version" in
  ''|*[!0-9A-Za-z._-]*) usage_err ;;
esac
tag="v$version"

rc=0
git ls-remote --exit-code "$remote" "refs/tags/$tag" >/dev/null 2>&1 || rc=$?
case "$rc" in
  0) ;;
  2)
    echo "check-release-tag: $tag is free"
    exit 0
    ;;
  *)
    echo "check-release-tag: cannot check $tag on $remote (exit $rc)" >&2
    exit 2
    ;;
esac

peeled="$(git ls-remote "$remote" "refs/tags/$tag^{}" 2>/dev/null || true)"
direct="$(git ls-remote "$remote" "refs/tags/$tag" 2>/dev/null || true)"
tag_sha="$(printf '%s\n' "$peeled" | cut -f1)"
[ -n "$tag_sha" ] || tag_sha="$(printf '%s\n' "$direct" | cut -f1)"
if [ -z "$tag_sha" ]; then
  echo "check-release-tag: cannot resolve $tag on $remote" >&2
  exit 2
fi

head_sha="$(git rev-parse HEAD)"
if [ "$tag_sha" = "$head_sha" ]; then
  echo "check-release-tag: $tag points at HEAD ($head_sha)"
  exit 3
fi
echo "check-release-tag: $tag points at $tag_sha, not HEAD ($head_sha)" >&2
exit 1
