#!/bin/bash
# The conformance battery against the real Nexus from compose.yaml: the same
# class of checks phase 1 ran by hand, numbered and loud. Every check prints
# `ok`/`FAIL` with a running number; the exit code is nonzero if any failed.
# `just stand` builds nxr, boots the container and calls this script.
set -u
DIR=$(cd "$(dirname "$0")" && pwd)
BASE="${NEXUS_BASE:-http://localhost:${NEXUS_PORT:-18081}}"
URL="$BASE/repository/stand"
VER="$URL/1.4.0/"

NXR="${NXR:-}"
if [ -z "$NXR" ]; then
    NXR=$(ls "$DIR/../../target/release/nxr" "$DIR/../../target/debug/nxr" 2>/dev/null | head -1)
fi
if [ ! -x "$NXR" ]; then
    echo "no nxr binary: cargo build --release -p nexus-raw, or set NXR=" >&2
    exit 2
fi

# Credentials from bootstrap.sh: the battery works as admin, which stays
# stable across nexus versions where the anonymous setup does not.
if [ -f "${STAND_ENV:-/tmp/nxr-stand.env}" ]; then
    . "${STAND_ENV:-/tmp/nxr-stand.env}"
    BASE="${NEXUS_BASE:-${STAND_URL%/repository/stand}}"
    export NXR_USERNAME="admin" NXR_PASSWORD="$STAND_PASS"
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
pass=0
fail=0
n=0
ok() { n=$((n + 1)); pass=$((pass + 1)); printf 'ok %2d - %s\n' "$n" "$1"; }
no() { n=$((n + 1)); fail=$((fail + 1)); printf 'FAIL %2d - %s\n' "$n" "$1"; }
# The command must succeed.
expect_ok() { local desc="$1"; shift; if "$@" > /dev/null 2>&1; then ok "$desc"; else no "$desc"; fi; }
# The command must fail.
expect_fail() { local desc="$1"; shift; if "$@" > /dev/null 2>&1; then no "$desc"; else ok "$desc"; fi; }

# --- fixtures ---------------------------------------------------------------

mkdir -p "$WORK/src/nested"
head -c 2000000 /dev/urandom > "$WORK/src/big.bin"
printf 'hello, stand\n' > "$WORK/src/hello.txt"
printf '{"n": 1, "version": "1.4.0", "artifacts": ["m.json", "hello.txt", "big.bin", "nested/sub.bin"]}\n' > "$WORK/src/m.json"
printf 'nested bytes\n' > "$WORK/src/nested/sub.bin"
cp -r "$WORK/src" "$WORK/orig"

# --- upload -----------------------------------------------------------------

# 1. The claim uploads first and alone: its artifact event leads the stream.
if "$NXR" up "$WORK/src" "$VER" --claim-first m.json --workers 4 --json > "$WORK/up.jsonl" 2> /dev/null &&
    [ "$(grep -m1 '"event":"artifact"' "$WORK/up.jsonl" | sed 's/.*"name":"\([^"]*\)".*/\1/')" = "m.json" ]; then
    ok "claim-first uploads the claim first"
else
    no "claim-first uploads the claim first"
fi

# 2. The run completes with the summary line.
if tail -1 "$WORK/up.jsonl" 2> /dev/null | grep -q '"event":"summary"'; then
    ok "up ends with the summary event"
else
    no "up ends with the summary event"
fi

# 3. The head reports the size of the stored artifact.
"$NXR" head "$VER/big.bin" > "$WORK/head.txt" 2> /dev/null
if [ $? -eq 0 ] && grep -q 2000000 "$WORK/head.txt"; then
    ok "head reports the stored size"
else
    no "head reports the stored size"
fi

# 4. Downloaded bytes are the uploaded bytes.
expect_ok "get returns the stored bytes" bash -c "\"$NXR\" get '$VER/big.bin' -o '$WORK/got-big.bin' && cmp -s '$WORK/orig/big.bin' '$WORK/got-big.bin'"

# 5. Nested names survive the server's path handling.
expect_ok "nested names survive" bash -c "\"$NXR\" get '$VER/nested/sub.bin' -o '$WORK/got-sub.bin' && cmp -s '$WORK/orig/nested/sub.bin' '$WORK/got-sub.bin'"

# 6. The marker is a sha256sum-style line over the artifact: byte-equal to
# what sha256sum prints for the same name.
(cd "$WORK/orig" && sha256sum hello.txt) > "$WORK/want-marker.txt"
expect_ok "the marker matches sha256sum output" bash -c "\"$NXR\" get '$VER/hello.txt.sha256' -o '$WORK/got-marker.txt' && cmp -s '$WORK/want-marker.txt' '$WORK/got-marker.txt'"

# 7. A rerun uploads nothing.
if out=$("$NXR" up "$WORK/src" "$VER" --workers 4 -q 2> /dev/null) && printf '%s' "$out" | grep -q 'uploaded 0' && printf '%s' "$out" | grep -q 'skipped 4'; then
    ok "a rerun skips everything"
else
    no "a rerun skips everything"
fi

# 8-9. Diverged local bytes are refused and the server keeps the old content.
printf 'diverged\n' > "$WORK/src/hello.txt"
expect_fail "up refuses a diverged artifact" "$NXR" up "$WORK/src" "$VER" --workers 4
expect_ok "the server keeps the old content" bash -c "\"$NXR\" get '$VER/hello.txt' -o '$WORK/got-old.txt' && cmp -s '$WORK/orig/hello.txt' '$WORK/got-old.txt'"
cp "$WORK/orig/hello.txt" "$WORK/src/hello.txt"

# 10. Resume: a partial file continues from its length (the 206 path).
head -c 100000 "$WORK/orig/big.bin" > "$WORK/resume-big.bin"
expect_ok "get --continue resumes a partial file" bash -c "\"$NXR\" get '$VER/big.bin' -o '$WORK/resume-big.bin' --continue && cmp -s '$WORK/orig/big.bin' '$WORK/resume-big.bin'"

# 11. Resume on a complete file finalizes without a refetch (the 416 path).
cp "$WORK/orig/big.bin" "$WORK/full-big.bin"
expect_ok "get --continue finalizes a complete file" bash -c "\"$NXR\" get '$VER/big.bin' -o '$WORK/full-big.bin' --continue && cmp -s '$WORK/orig/big.bin' '$WORK/full-big.bin'"

# 12-15. Channels: a forward move works, a backward one is refused.
expect_ok "the channel takes a first version" "$NXR" channel set "$URL/latest" 1.4.0
if "$NXR" channel get "$URL/latest" 2> /dev/null | grep -q 1.4.0; then
    ok "channel get reads the version back"
else
    no "channel get reads the version back"
fi
# 14. --if-forward on an older version is a kept pointer, not an error: the
# operation succeeds by refusing to move backward.
if "$NXR" channel set "$URL/latest" 1.3.9 --if-forward 2> /dev/null | grep -q 'forward-only' &&
    [ "$("$NXR" channel get "$URL/latest" 2> /dev/null)" = "1.4.0" ]; then
    ok "--if-forward keeps the pointer on an older version"
else
    no "--if-forward keeps the pointer on an older version"
fi
expect_ok "--if-forward takes a newer version" "$NXR" channel set "$URL/latest" 1.4.1 --if-forward

# 16-17. The search-backed listing shows the tree: the version folder among the
# entries, then the objects of the version through `--assets`. The index lags
# the writes, so both listings poll.
seen=0
for _ in 1 2 3 4 5 6; do
    if "$NXR" ls "$URL" 2> /dev/null | grep -q '1.4.0/'; then seen=1; break; fi
    sleep 2
done
if [ "$seen" = 1 ]; then ok "ls lists the version folder"; else no "ls lists the version folder"; fi
seen=0
for _ in 1 2 3 4 5 6; do
    if "$NXR" ls "$VER" --assets 2> /dev/null | grep -q 'big.bin'; then seen=1; break; fi
    sleep 2
done
if [ "$seen" = 1 ]; then ok "ls --assets lists the objects of a version"; else no "ls --assets lists the objects of a version"; fi

# 18-19. Missing objects: head answers, get fails.
if "$NXR" head "$VER/nope.bin" 2> /dev/null | grep -q 'head: 404'; then
    ok "head reports 404 for a missing object"
else
    no "head reports 404 for a missing object"
fi
expect_fail "get of a missing object fails" "$NXR" get "$VER/nope.bin" -o "$WORK/nope.bin"

# 20. A dry run plans but sends nothing: the version listing must not move.
before=$("$NXR" ls "$URL" 2> /dev/null | wc -l)
expect_ok "up --dry-run plans without sending" "$NXR" up "$WORK/src" "$VER" --dry-run
after=$("$NXR" ls "$URL" 2> /dev/null | wc -l)
if [ "$before" = "$after" ]; then ok "dry run sends nothing"; else no "dry run sends nothing"; fi

# 21-22. Down by the manifest mirrors the version, markers included.
expect_ok "down by manifest mirrors the version" "$NXR" down "$VER" "$WORK/dst" --manifest "$VER/m.json" --workers 4
trees=1
for f in m.json hello.txt big.bin nested/sub.bin; do
    cmp -s "$WORK/orig/$f" "$WORK/dst/$f" || trees=0
done
if [ "$trees" = 1 ] && [ -f "$WORK/dst/hello.txt.sha256" ]; then
    ok "the mirrored tree is byte-equal, markers in place"
else
    no "the mirrored tree is byte-equal, markers in place"
fi
expect_ok "verify passes offline on the mirror" "$NXR" verify "$WORK/dst"

# --- summary ----------------------------------------------------------------

printf '\n%d passed, %d failed\n' "$pass" "$fail"
[ "$fail" -eq 0 ]
