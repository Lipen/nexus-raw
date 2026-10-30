#!/bin/bash
# The conformance battery against the real Nexus from compose.yaml: the same
# class of checks phase 1 ran by hand, numbered and loud. Every check prints
# `[ok]`/`[fail]` with a running number; the exit code is nonzero if any failed.
# `just stand` builds nxr, boots the container and calls this script.
set -u
DIR=$(cd "$(dirname "$0")" && pwd)

# shellcheck source=../../scripts/ui.sh
. "$DIR/../../scripts/ui.sh"

BASE="${NEXUS_BASE:-http://localhost:${NEXUS_PORT:-18081}}"
URL="$BASE/repository/stand"
VER="$URL/1.4.0/"

NXR="${NXR:-}"
if [ -z "$NXR" ]; then
    NXR=$(ls "$DIR/../../target/release/nxr" "$DIR/../../target/debug/nxr" 2>/dev/null | head -1)
fi
if [ ! -x "$NXR" ]; then
    ui_fail "no nxr binary: \`cargo build --release -p nexus-raw\`, or set NXR="
    exit 2
fi

# Credentials from bootstrap.sh: the battery works as admin, which stays
# stable across nexus versions where the anonymous setup does not.
if [ -f "${STAND_ENV:-/tmp/nxr-stand.env}" ]; then
    # shellcheck source=/dev/null
    . "${STAND_ENV:-/tmp/nxr-stand.env}"
    BASE="${NEXUS_BASE:-${STAND_URL%/repository/stand}}"
    export NXR_USERNAME="admin" NXR_PASSWORD="$STAND_PASS"
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT
pass=0
fail=0
n=0
ok() {
    n=$((n + 1)); pass=$((pass + 1))
    if [ "${BATTERY_VERBOSE:-0}" = 1 ]; then
        ui_ok "$(printf '%2d - %s' "$n" "$1")"
    elif [ -t 1 ] && [ "$((n % 10))" -eq 0 ]; then
        ui_note "battery" "$n done"
    fi
}
no() {
    n=$((n + 1)); fail=$((fail + 1))
    ui_fail "$(printf '%2d - %s' "$n" "$1")"
}
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

# 20. A dry run plans but sends nothing.
# The listing reads the search index and the index lags the writes, so a
# before/after line count is not evidence: an unrelated entry (the channel file
# written a few checks earlier) can land in the window and move the count.
# The plan must name the one object the server does not have yet, and afterwards
# that name must be absent remotely (a HEAD answers 404) and locally (the sibling
# `up` generates on the fly was not written either): that needs no index at all.
printf 'not yet uploaded\n' > "$WORK/src/pending.bin"
if out=$("$NXR" up "$WORK/src" "$VER" --dry-run 2> /dev/null) && printf '%s' "$out" | grep -q 'pending.bin'; then
    ok "up --dry-run plans without sending"
else
    no "up --dry-run plans without sending"
fi
if "$NXR" head "$VER/pending.bin" 2> /dev/null | grep -q 'head: 404' && [ ! -e "$WORK/src/pending.bin.sha256" ]; then
    ok "dry run sends nothing"
else
    no "dry run sends nothing"
fi
rm -f "$WORK/src/pending.bin"

# 22-23. Down by explicit names mirrors the version, markers included.
expect_ok "down by names mirrors the version" "$NXR" down "$VER" "$WORK/dst" --name m.json --name hello.txt --name big.bin --name nested/sub.bin --workers 4
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

# --- the remaining paths against the real server ---------------------------

# The search-side delta reads the real index (sizes and checksums per asset)
# once it catches up: the mirrored copy and the storage agree on every name.
seen=0
for _ in 1 2 3 4 5 6; do
    if "$NXR" diff "$WORK/orig" "$VER" --via search --ls > "$WORK/diff-search.txt" 2> /dev/null; then
        seen=1
        break
    fi
    sleep 2
done
if [ "$seen" = 1 ] && ! grep -qE '^(missing|diverged)' "$WORK/diff-search.txt"; then
    ok "diff --via search reads the real index as equal"
else
    no "diff --via search reads the real index as equal"
fi

# The segmented read against a real 206 answer: four spans, one byte-equal file.
expect_ok "get --segments reassembles the real partial-content answer" bash -c "\"$NXR\" get '$VER/big.bin' -o '$WORK/seg-big.bin' --segments 4 && cmp -s '$WORK/orig/big.bin' '$WORK/seg-big.bin'"

# The put primitive: bytes plus the marker of the same name in one call.
printf 'put bytes\n' > "$WORK/put.bin"
expect_ok "put writes one object with its marker" "$NXR" put "$VER/put.bin" -f "$WORK/put.bin" --sha
expect_ok "get returns the put bytes" bash -c "\"$NXR\" get '$VER/put.bin' -o '$WORK/put-got.bin' && cmp -s '$WORK/put.bin' '$WORK/put-got.bin'"

# clone the version through the search API, then re-run with --verify: the
# second run re-hashes the local tree and downloads nothing, and the clone
# verifies offline.
# The index lags the writes, and a name the first clone missed would arrive in
# the second run as a download: wait for the last uploaded name first, so both
# runs enumerate the same set.
seen=0
for _ in 1 2 3 4 5 6; do
    if "$NXR" ls "$VER" --assets 2> /dev/null | grep -q 'put.bin'; then
        seen=1
        break
    fi
    sleep 2
done
if [ "$seen" = 1 ]; then
    ok "the index catches up before the clone"
else
    no "the index catches up before the clone"
fi
expect_ok "clone mirrors the version" "$NXR" clone "$VER" "$WORK/clone" --workers 4
expect_ok "clone --verify re-hashes and skips" bash -c "\"$NXR\" clone '$VER' '$WORK/clone' --verify > '$WORK/clone2.txt' && grep -q 'downloaded 0' '$WORK/clone2.txt'"
expect_ok "verify passes on the clone" "$NXR" verify "$WORK/clone"

# audit --bytes hashes every object against its marker on a real index.
expect_ok "audit --bytes finds no bitrot" "$NXR" audit "$VER" --bytes

# doctor --deep runs its probes (version, range, content-length, search, repo
# type, eula) against the real server and reports no failure.
expect_ok "doctor --deep passes on the real server" "$NXR" doctor "$VER" --deep

# A new name goes up and the index poll sees it: --wait-index holds the run
# until the real search API answers with every uploaded name.
printf 'wait for me\n' > "$WORK/src/waited.bin"
expect_ok "up --wait-index waits for the real index" bash -c "\"$NXR\" up '$WORK/src' '$VER' --wait-index --json > '$WORK/wait.jsonl' && grep -q '\"timeout\":false' '$WORK/wait.jsonl'"

# The one-span read against the real server: the returned bytes equal the
# source prefix.
expect_ok "get --range reads one span" bash -c "\"$NXR\" get '$VER/big.bin' --range 0-99 -o '$WORK/span.bin' && head -c 100 '$WORK/orig/big.bin' | cmp -s - '$WORK/span.bin'"

# --if-changed on a converged tree skips everything: every remote marker
# already matches the local copy.
if out=$("$NXR" up "$WORK/src" "$VER" --if-changed -q 2> /dev/null) && printf '%s' "$out" | grep -q 'uploaded 0'; then
    ok "up --if-changed skips a converged tree"
else
    no "up --if-changed skips a converged tree"
fi

# The segmented download against the same real 206 answer.
expect_ok "down --segments reassembles a real 206 answer" bash -c "\"$NXR\" down '$VER' '$WORK/seg-down' --name big.bin --segments 4 && cmp -s '$WORK/orig/big.bin' '$WORK/seg-down/big.bin'"

# --prefix scans only the selected subtree and keeps its relative path: the
# nested file lands, the sibling at the root does not.
expect_ok "up --prefix publishes the selected subtree" "$NXR" up "$WORK/orig" "$URL/2.0.0/" --prefix nested/
if "$NXR" head "$URL/2.0.0/nested/sub.bin" 2> /dev/null | grep -q 'head: 200' &&
    "$NXR" head "$URL/2.0.0/hello.txt" 2> /dev/null | grep -q 'head: 404'; then
    ok "the prefix kept its path and left the rest out"
else
    no "the prefix kept its path and left the rest out"
fi

# 24-28. A directory listing asks the search API with `q=<directory>/` and keeps the prefix
# filter client-side (the rule the protocol page names). The seeded fixture is the manual
# measurement in miniature: the listing must equal the repository-wide walk restricted to
# that directory, and the copy seeded under another parent, which the server's loose `q`
# match rides, must never reach it.
mkdir -p "$WORK/qsrc/bench/flat" "$WORK/qsrc/backup/bench/flat"
for i in 1 2 3 4 5 6; do printf 'flat %s\n' "$i" > "$WORK/qsrc/bench/flat/q-$i.bin"; done
for i in 1 2; do printf 'copy %s\n' "$i" > "$WORK/qsrc/backup/bench/flat/decoy-$i.bin"; done
expect_ok "the directory fixture uploads" "$NXR" up "$WORK/qsrc" "$URL/" --workers 2

# The index lags the writes: wait until the directory's last name is searchable.
seen=0
for _ in 1 2 3 4 5 6; do
    if "$NXR" service assets "$URL/" --q 'bench/flat/' --json 2> /dev/null | grep -q 'q-6.bin'; then seen=1; break; fi
    sleep 2
done
if [ "$seen" = 1 ]; then ok "the seeded directory reaches the search index"; else no "the seeded directory reaches the search index"; fi

# The two sides of the comparison: the full walk (no q) reduced to the directory's artifacts,
# and the listing of the directory (which asks with q and filters client-side).
"$NXR" service assets "$URL/" --json 2> /dev/null |
    grep -o '"path":"bench/flat/[^"]*"' | sed 's/^"path":"//; s/"$//' |
    grep -v '\.sha256$' | sed 's|.*/||' | sort > "$WORK/walk-q.txt"
"$NXR" ls "$URL/bench/flat/" --json 2> /dev/null |
    grep -o '"entry":"[^"]*","kind":"file"' | sed 's/^"entry":"//; s/","kind":"file"$//' |
    sort > "$WORK/ls-q.txt"
if [ -s "$WORK/walk-q.txt" ] && cmp -s "$WORK/walk-q.txt" "$WORK/ls-q.txt"; then
    ok "a directory listing equals the walk restricted to it"
else
    no "a directory listing equals the walk restricted to it"
fi

# The server's `q` is loose: the same query rides the copy under another parent, so the
# equality above is the client-side filter's doing, not the server's narrowness.
if "$NXR" service assets "$URL/" --q 'bench/flat/' --json 2> /dev/null | grep -q 'backup/bench/flat/decoy-1.bin'; then
    ok "the server's q is a loose path substring"
else
    no "the server's q is a loose path substring"
fi
if "$NXR" ls "$URL/bench/flat/" 2> /dev/null | grep -q 'decoy-'; then
    no "the listing never surfaces the loose match's copy"
else
    ok "the listing never surfaces the loose match's copy"
fi

# --- deletion and the service plane ----------------------------------------

# The management API of the real server: a live status and the repository list.
if "$NXR" service status "$BASE/" 2> /dev/null | grep -qi alive; then
    ok "service status reports a live server"
else
    no "service status reports a live server"
fi
if "$NXR" service repos "$BASE/" 2> /dev/null | grep -q 'stand'; then
    ok "service repos lists the stand repository"
else
    no "service repos lists the stand repository"
fi

# rm deletes exactly the named objects on a real server, marker first, and a
# rerun sees only 404s and still exits 0.
expect_ok "rm removes the named object" "$NXR" rm "$URL/2.0.0/" --name nested/sub.bin --yes
if "$NXR" head "$URL/2.0.0/nested/sub.bin" 2> /dev/null | grep -q 'head: 404' &&
    "$NXR" head "$URL/2.0.0/nested/sub.bin.sha256" 2> /dev/null | grep -q 'head: 404'; then
    ok "the bytes and the marker are gone"
else
    no "the bytes and the marker are gone"
fi
expect_ok "an rm rerun is idempotent" "$NXR" rm "$URL/2.0.0/" --name nested/sub.bin --yes

# point clears a pointer file: the channel reads unset afterwards.
expect_ok "a throwaway channel takes a token" "$NXR" channel set "$URL/throwaway" 1.0.0
expect_ok "point clears the pointer" "$NXR" point "$URL/throwaway"
if [ "$("$NXR" channel get "$URL/throwaway" 2> /dev/null)" = "unset" ]; then
    ok "the cleared pointer reads unset"
else
    no "the cleared pointer reads unset"
fi

# --- summary ----------------------------------------------------------------

if [ "$fail" -eq 0 ]; then
    ui_ok "$pass passed, 0 failed"
else
    ui_fail "$pass passed, $fail failed"
fi
[ "$fail" -eq 0 ]
