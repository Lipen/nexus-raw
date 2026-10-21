#!/usr/bin/env bash
# The read-only mount, end to end, against the mock server.
#
# The mock's `search-lag` scenario serves a store-backed listing only after its
# catch-up deadline, and that deadline is armed by the first PUT. A seeded store
# alone never fills the index, so the script seeds the tree, PUTs one probe and
# waits out the lag. The probe is excluded from the tree comparison by name.
#
# Linux only: /dev/fuse and fusermount3 (or fusermount) are required.
# Not part of `just check`: the lag wait makes it slower than a unit test.
set -euo pipefail

usage() {
  echo "usage: mount-smoke.sh"
  echo "  drives nxr-mount against mock-nexus and compares the tree"
  echo "  skips with exit 0 when /dev/fuse or fusermount is absent"
}
case "${1:-}" in
  -h | --help)
    usage
    exit 0
    ;;
  "") ;;
  *)
    usage >&2
    exit 2
    ;;
esac

if [ "$(uname -s)" != Linux ]; then
  echo "mount-smoke: skipped (the mount is Linux-only)"
  exit 0
fi
if [ ! -e /dev/fuse ]; then
  echo "mount-smoke: skipped (no /dev/fuse)"
  exit 0
fi
fusermount=$(command -v fusermount3 || command -v fusermount || true)
if [ -z "$fusermount" ]; then
  echo "mount-smoke: skipped (no fusermount3 or fusermount; install fuse3)"
  exit 0
fi

here=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$here"
cargo build -q -p mock-nexus -p nexus-raw -p nexus-raw-mount
target_dir=$(cargo metadata --format-version 1 --no-deps | sed -n 's/.*"target_directory":"\([^"]*\)".*/\1/p')
bin="$target_dir/debug"

work=$(mktemp -d)
mnt="$work/mnt"
mkdir -p "$mnt" "$work/data/repository/raw/a/b"
printf 'hello world\n' > "$work/data/repository/raw/hello.txt"
head -c 1000000 /dev/urandom > "$work/data/repository/raw/a/big.bin"
printf 'nested\n' > "$work/data/repository/raw/a/b/n.txt"

mock_pid=""
mount_pid=""
cleanup() {
  if [ -n "$mount_pid" ]; then
    kill "$mount_pid" 2>/dev/null || true
    wait "$mount_pid" 2>/dev/null || true
  fi
  if mountpoint -q "$mnt" 2>/dev/null; then
    "$fusermount" -u "$mnt" 2>/dev/null || true
  fi
  if [ -n "$mock_pid" ]; then
    kill "$mock_pid" 2>/dev/null || true
    wait "$mock_pid" 2>/dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT INT TERM

# Port 0 lets the kernel pick: the log line carries the port the mock landed on.
"$bin/mock-nexus" search-lag --port 0 --seed-dir "$work/data" > "$work/mock.log" 2>&1 &
mock_pid=$!
base=""
for _ in $(seq 1 100); do
  port=$(awk -F: '/^listening http:\/\/127\.0\.0\.1:[0-9]+$/ { print $NF; exit }' "$work/mock.log")
  if [ -n "$port" ]; then
    base="http://127.0.0.1:$port/repository/raw"
    break
  fi
  sleep 0.1
done
if [ -z "$base" ]; then
  echo "mount-smoke: the mock did not start" >&2
  cat "$work/mock.log" >&2
  exit 1
fi

# The probe arms the search deadline; the seeded objects reach the index after the lag.
printf 'probe\n' | "$bin/nxr" put "$base/.probe" --stdin > /dev/null
sleep 7

listing=$("$bin/nxr" ls "$base")
case "$listing" in
  *hello.txt*) ;;
  *)
    echo "mount-smoke: the listing missed hello.txt:" >&2
    echo "$listing" >&2
    exit 1
    ;;
esac

"$bin/nxr-mount" "$base" "$mnt" > "$work/mount.log" 2>&1 &
mount_pid=$!
for _ in $(seq 1 100); do
  mountpoint -q "$mnt" && break
  sleep 0.1
done
if ! mountpoint -q "$mnt"; then
  echo "mount-smoke: the mount did not come up" >&2
  cat "$work/mount.log" >&2
  exit 1
fi

fail() {
  echo "mount-smoke: $*" >&2
  exit 1
}

[ "$(cat "$mnt/hello.txt")" = "hello world" ] || fail "hello.txt content diverged"
diff -r -x .probe "$work/data/repository/raw" "$mnt" || fail "the tree diverged"
[ "$(stat -c %s "$mnt/a/big.bin")" = "1000000" ] || fail "big.bin size diverged"
[ "$(sha256sum "$work/data/repository/raw/a/big.bin" | cut -d' ' -f1)" = "$(sha256sum "$mnt/a/big.bin" | cut -d' ' -f1)" ] ||
  fail "big.bin digest diverged"
if (printf x > "$mnt/new.txt") 2>/dev/null; then
  fail "a write to the read-only mount succeeded"
fi

"$fusermount" -u "$mnt"
wait "$mount_pid" || true
mount_pid=""
echo "mount-smoke: ok"
