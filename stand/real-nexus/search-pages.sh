#!/bin/sh
# The `q`-as-page-cutter measurement for the search-backed listing (`nxr ls`).
#
# The listing walks the whole repository through `/service/rest/v1/search/assets` and filters the prefix client-side, because the `group` parameter lies on raw paths (see `crates/nexus-raw-core/src/layout/ls.rs`).
# The question this script answers with numbers: can the `q` parameter cut the walk to one directory, and does the cut stay a superset of the full walk's rows for that directory?
# A cut that drops names is not usable, whatever it saves.
#
# It seeds a tree over several prefixes and depths, waits for the search index, then measures the full walk and a battery of `q=` queries against it, comparing returned path sets (not only counts) with the full walk's set for the same directory.
# Every printed number is a real run against the Nexus in compose.yaml.
# `sh stand/real-nexus/run.sh` boots that stack and runs bootstrap.sh.
# This script needs the stack up and the credentials bootstrap.sh writes to $STAND_ENV.
#
# Usage: sh stand/real-nexus/search-pages.sh [stage1|stage2|both]
#   stage1  seed the sub-cap tree and measure  (under nxr's 100-page cap)
#   stage2  seed the larger tree and measure   (past that cap)
#   both    both stages (default), one repo
# Knobs: NEXUS_PORT, NEXUS_BASE, STAND_ENV, NXR, SP_JOBS, SP_MAX_PAGES, SP_KEEP, SP1_*/SP2_* for the counts, SP_REPO for the repository name.
# Seeding is plain PUTs of a tiny blob: the tree, not the content, is the fixture.
set -eu

DIR=$(cd "$(dirname "$0")" && pwd)
BASE="${NEXUS_BASE:-http://localhost:${NEXUS_PORT:-18081}}"
REPO="${SP_REPO:-stand}"
URL="$BASE/repository/$REPO"
STAND_ENV="${STAND_ENV:-/tmp/nxr-stand.env}"

# The tree shape per stage.
# Stage 2 is a superset of stage 1: seeding re-PUTs the stage-1 names (idempotent) and adds the rest.
SP1_FLAT="${SP1_FLAT:-1200}"; SP1_DEEP="${SP1_DEEP:-600}"; SP1_WIDE="${SP1_WIDE:-600}"
SP2_FLAT="${SP2_FLAT:-3000}"; SP2_DEEP="${SP2_DEEP:-1200}"; SP2_WIDE="${SP2_WIDE:-1800}"
SP_JOBS="${SP_JOBS:-16}"
SP_MAX_PAGES="${SP_MAX_PAGES:-400}"
# nxr's own walk cap, for the note printed with the numbers.
NXR_CAP=100

for tool in curl jq python3; do
    command -v "$tool" >/dev/null 2>&1 || { echo "$tool is required" >&2; exit 1; }
done
if [ ! -f "$STAND_ENV" ]; then
    echo "$STAND_ENV not found: run sh $DIR/bootstrap.sh (or the whole 'just stand') first" >&2
    exit 1
fi
# shellcheck disable=SC1090
. "$STAND_ENV"
AUTH="admin:${STAND_PASS:?STAND_PASS missing from $STAND_ENV}"

WORK="${SP_WORK:-$(mktemp -d)}"
mkdir -p "$WORK"
cleanup() { [ -n "${SP_KEEP:-}" ] || rm -rf "$WORK"; }
trap cleanup EXIT

# The `/status` endpoint answers an empty body.
# The version rides the `Server` response header.
NEXUS_VERSION=$(curl -sI "$BASE/" | tr -d '\r' | awk -F'[:/ ]+' '/^Server:/ { print $3 }')
NEXUS_VERSION="${NEXUS_VERSION:-unknown}"
echo "nexus $NEXUS_VERSION at $BASE, repository $REPO"
echo "tree shape: flat=$SP2_FLAT deep=$SP2_DEEP wide=$SP2_WIDE plus fixed tricky/decoy names"
echo "workspace: $WORK"

# --- seeding ----------------------------------------------------------------

# The names of one stage, repo-relative, one per line.
# Dirty names (space, plus, dots, upper case, unicode) are deliberate: `q` has to survive them.
seed_names() {
    scale="$1"
    case "$scale" in
    1) FLAT=$SP1_FLAT DEEP=$SP1_DEEP WIDE=$SP1_WIDE ;;
    2) FLAT=$SP2_FLAT DEEP=$SP2_DEEP WIDE=$SP2_WIDE ;;
    esac
    [ $((WIDE % 3)) -eq 0 ] || { echo "SP*_WIDE must divide by 3" >&2; exit 1; }
    i=1; while [ "$i" -le "$FLAT" ]; do printf 'bench/flat/flat-%05d.bin\n' "$i"; i=$((i + 1)); done
    i=1; while [ "$i" -le "$DEEP" ]; do printf 'bench/deep/l2/l3/l4/l5/l6/deep-%05d.bin\n' "$i"; i=$((i + 1)); done
    n=$((WIDE / 3))
    for d in a b c; do
        i=1; while [ "$i" -le "$n" ]; do printf 'bench/wide/%s/wide-%s-%05d.bin\n' "$d" "$d" "$i"; i=$((i + 1)); done
    done
    # A sibling whose name extends the flat prefix: `q=bench/flat` must not be trusted to mean `bench/flat/`.
    # The decoy makes the over-match visible.
    i=1; while [ "$i" -le 40 ]; do printf 'bench/flat2/flat2-%04d.bin\n' "$i"; i=$((i + 1)); done
    # Path names that punish naive query encoding or matching.
    i=1; while [ "$i" -le 10 ]; do printf 'bench/tricky/v1.2.3/dot-%04d.bin\n' "$i"; i=$((i + 1)); done
    i=1; while [ "$i" -le 10 ]; do printf 'bench/tricky/name with space/space-%04d.bin\n' "$i"; i=$((i + 1)); done
    i=1; while [ "$i" -le 10 ]; do printf 'bench/tricky/pl+us/plus-%04d.bin\n' "$i"; i=$((i + 1)); done
    i=1; while [ "$i" -le 10 ]; do printf 'bench/tricky/UPPER/upper-%04d.bin\n' "$i"; i=$((i + 1)); done
    i=1; while [ "$i" -le 5 ]; do printf 'bench/tricky/ψ-unicode/uni-%04d.bin\n' "$i"; i=$((i + 1)); done
    i=1; while [ "$i" -le 5 ]; do printf 'bench/tricky/amp&ersand/amp-%04d.bin\n' "$i"; i=$((i + 1)); done
    i=1; while [ "$i" -le 5 ]; do printf 'bench/tricky/hash#tag/hash-%04d.bin\n' "$i"; i=$((i + 1)); done
}

seed() {
    scale="$1"
    names="$WORK/names.$scale"
    seed_names "$scale" > "$names"
    want=$(wc -l < "$names")
    echo "==> seeding stage $scale: $want names (PUT, $SP_JOBS parallel)" >&2
    printf 'x' > "$WORK/blob"
    # `@uri` encodes every byte including the slashes.
    # The slashes are put back so the names stay a path.
    # `-d '\n'` keeps names with spaces whole.
    jq -Rr '@uri' < "$names" | sed 's|%2F|/|g' | awk -v p="$URL/" '{ print p $0 }' > "$WORK/urls.$scale"
    t0=$(date +%s)
    if xargs -d '\n' -P "$SP_JOBS" -n 1 sh -c 'curl -sf --max-time 60 -o /dev/null -u "$0" -T "$1" "$2"' \
        "$AUTH" "$WORK/blob" < "$WORK/urls.$scale"; then
        echo "==> seeded $want names in $(( $(date +%s) - t0 ))s" >&2
    else
        echo "some PUTs failed (xargs exit $?)" >&2
        exit 1
    fi
    printf '%s' "$want"
}

# --- the index --------------------------------------------------------------

# What the index reports for the repository: `totalCount` when the server has the field, empty otherwise.
# Cheap, so the wait can poll it.
index_count() {
    page=$(curl -sf -u "$AUTH" "$BASE/service/rest/v1/search/assets?repository=$REPO" || true)
    [ -n "$page" ] || return 0
    printf '%s' "$page" | jq -r 'if has("totalCount") and (.totalCount != null) then .totalCount else "" end'
}

wait_index() {
    want="$1"
    i=0
    while :; do
        got=$(index_count || true)
        if [ -z "$got" ]; then
            # No totalCount: a full walk is the only count there is.
            walk indexwait "" "$WORK/paths.indexwait"
            got=$(wc -l < "$WORK/paths.indexwait")
        fi
        if [ "$got" = "$want" ]; then
            echo "==> search index reports $got assets" >&2
            return 0
        fi
        i=$((i + 1))
        if [ "$i" -gt 60 ]; then
            echo "index never reached $want (last: $got); measuring anyway" >&2
            return 0
        fi
        [ "$i" -eq 1 ] && echo "==> waiting for the index: $got of $want" >&2
        sleep 3
    done
}

# --- walking ----------------------------------------------------------------

# Walks the search API the way the client does (`repository`, then `continuationToken` pages), optionally with `q`, following tokens to the end.
# Prints one path per line into `$3` and writes `pages rows secs totalCount capped firstPageRows` into `$WORK/stats.<label>`.
walk() {
    wlabel="$1"; q="$2"; paths="$3"
    : > "$paths"
    qs="repository=$REPO"
    if [ -n "$q" ]; then
        case "$q" in
        # `RAW:` sends the value exactly as written, the way a client that concatenates the query string would.
        # Otherwise the value is percent-encoded like `Url::query_pairs_mut().append_pair` does.
        RAW:*) qs="$qs&q=${q#RAW:}" ;;
        *) qs="$qs&q=$(jq -rn --arg v "$q" '$v|@uri')" ;;
        esac
    fi
    url="$BASE/service/rest/v1/search/assets?$qs"
    if [ "$wlabel" = full ]; then
        echo "    full-walk page 1 URL: $url" >&2
    fi
    token=""; pages=0; rows=0; tc="-"; secs=0; capped=0; firstrows=0
    while :; do
        pages=$((pages + 1))
        if [ "$pages" -gt "$SP_MAX_PAGES" ]; then
            capped=1
            echo "    $wlabel: stopped at SP_MAX_PAGES=$SP_MAX_PAGES" >&2
            break
        fi
        u="$url"
        if [ -n "$token" ]; then
            u="$url&continuationToken=$(jq -rn --arg v "$token" '$v|@uri')"
        fi
        st=$(curl -s -u "$AUTH" -o "$WORK/page.json" -w '%{http_code} %{time_total}' "$u")
        code=${st%% *}; t=${st#* }
        secs=$(awk -v a="$secs" -v b="$t" 'BEGIN { printf "%.2f", a + b }')
        if [ "$code" != 200 ]; then
            echo "    $wlabel: HTTP $code on page $pages" >&2
            capped=1
            break
        fi
        n=$(jq '.items | length' "$WORK/page.json")
        rows=$((rows + n))
        if [ "$pages" = 1 ]; then
            firstrows=$n
            jq -c 'keys' "$WORK/page.json" > "$WORK/env.$wlabel"
            jq -c '[.items[]? | keys] | add // [] | unique' "$WORK/page.json" > "$WORK/itemkeys.$wlabel"
        fi
        jq -r '.items[]?.path' "$WORK/page.json" >> "$paths"
        if [ "$tc" = "-" ]; then
            tc=$(jq -r 'if has("totalCount") and (.totalCount != null) then .totalCount else "-" end' "$WORK/page.json")
        fi
        token=$(jq -r '.continuationToken // empty' "$WORK/page.json")
        [ -n "$token" ] || break
    done
    printf '%s %s %s %s %s %s\n' "$pages" "$rows" "$secs" "$tc" "$capped" "$firstrows" > "$WORK/stats.$wlabel"
}

# Compares the walk-restricted set with a q set and prints one measurement row.
# Args: label, prefix to compare against, stats label.
# Reads the walk and the q pages from `$WORK/paths.<stats label>`-side files: the q walk is passed as `q`, the full walk is `$WORK/paths.full`.
row() {
    r_label="$1"; prefix="$2"; label_stats="$3"
    read -r pages _ secs tc capped firstrows < "$WORK/stats.$label_stats"
    python3 - "$WORK/paths.full" "$WORK/paths.$label_stats" "$prefix" "$WORK/cmp" "$WORK/cmp.tags" <<'PY'
import sys

def load(path):
    out = []
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            # The server serves asset paths with a leading slash.
            # The client's prefix filter drops empty segments, so both sides are compared without it.
            line = line.rstrip("\r\n").lstrip("/")
            if line:
                out.append(line)
    return out

walk, q, prefix, out, tags = sys.argv[1:6]
w = [p for p in load(walk) if p.startswith(prefix)]
ws, qs = set(w), set(load(q))
missing, extra = sorted(ws - qs), sorted(qs - ws)
verdict = "equal" if not missing and not extra else ("superset" if not missing else "DROPS")
with open(out, "w", encoding="utf-8") as fh:
    fh.write(f"{len(qs)} {len(ws)} {len(qs & ws)} {len(missing)} {len(extra)} {verdict}\n")
with open(tags, "w", encoding="utf-8") as fh:
    fh.write(" ".join(missing[:2]) + "|" + " ".join(extra[:2]) + "\n")
PY
    read -r qrows kept present missing extra verdict < "$WORK/cmp"
    r_tags=$(cat "$WORK/cmp.tags")
    drop=$((qrows - present))
    guard="-"
    if [ "$tc" != "-" ]; then
        if [ "$tc" = "$qrows" ]; then guard="rows=tc"; else guard="rows!=tc"; fi
    fi
    printf '%-38s %6s %6s %6s %6s %6s %6s %8s %9s %6s %8s\n' \
        "$r_label" "$pages" "$qrows" "$kept" "$drop" "$missing" "$extra" "$verdict" "$guard" "$secs" "$tc"
    if [ "$missing" != 0 ] || [ "$extra" != 0 ]; then
        echo "      first dropped: ${r_tags%%|*}"
        echo "      first extra:   ${r_tags##*|}"
    fi
}

# --- cases ------------------------------------------------------------------

# The walk's set against the seeded fixture.
# The index is the only source both sides read, so this check says whether that source is telling the truth about what was written.
fixture_check() {
    python3 - "$WORK/names.$1" "$WORK/paths.full" <<'PY'
import sys

def load(path, strip_slash):
    out = []
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            line = line.rstrip("\r\n")
            if strip_slash:
                line = line.lstrip("/")
            if line:
                out.append(line)
    return out

seeded, walk = set(load(sys.argv[1], False)), set(load(sys.argv[2], True))
missing, extra = seeded - walk, walk - seeded
verdict = "the walk equals the fixture" if not missing and not extra else "DIFFERS"
print(f"      fixture check: seeded {len(seeded)}, walk {len(walk)}, not in walk {len(missing)}, not seeded {len(extra)}: {verdict}")
PY
}

# Every case is one real walk: (label, q value, comparison prefix).
run_cases() {
    scale="$1"
    walk full "" "$WORK/paths.full"
    read -r fp fr fs ftc fcapped ffirst < "$WORK/stats.full"
    guard="-"
    if [ "$ftc" != "-" ]; then
        if [ "$ftc" = "$fr" ]; then guard="rows=tc"; else guard="rows!=tc"; fi
    fi
    printf '%-38s %6s %6s %6s %6s %6s %6s %8s %9s %6s %8s\n' \
        "full-walk (no q)" "$fp" "$fr" "$fr" 0 0 0 "equal" "$guard" "$fs" "$ftc"
    echo "      rows per page: $ffirst"
    echo "      q is percent-encoded like the client's encoder, the continuation token too"
    echo "      response envelope keys: $(jq -r 'join(", ")' "$WORK/env.full")"
    echo "      item keys: $(jq -r 'join(", ")' "$WORK/itemkeys.full")"
    fixture_check "$scale"
    if [ "$fcapped" = 1 ]; then
        echo "      NOTE: the walk was cut off before its end (page cap or HTTP error)"
    fi
    if [ "$fp" -gt "$NXR_CAP" ]; then
        echo "      NOTE: $fp pages is past nxr's MAX_SEARCH_PAGES=$NXR_CAP: this listing fails today"
    fi

    # q=<directory>, several prefixes and depths.
    # `q=bench/flat` (no slash) and `q=fla` probe what the parameter actually matches.
    # A bare token may be satisfied by a substring or a search token instead of a path prefix.
    i=0
    while IFS='|' read -r label q prefix; do
        i=$((i + 1))
        walk "q$i" "$q" "$WORK/paths.q$i"
        row "$label" "$prefix" "q$i"
    done <<'CASES'
q=bench/flat/|bench/flat/|bench/flat/
q=bench/wide/b/|bench/wide/b/|bench/wide/b/
q=bench/wide/ (parent)|bench/wide|bench/wide/
q=bench/deep/l2/l3/l4/l5/l6/|bench/deep/l2/l3/l4/l5/l6/|bench/deep/l2/l3/l4/l5/l6/
q=bench/ (tree root)|bench/|bench/
q=bench/flat (no slash)|bench/flat|bench/flat/
q=fla (bare token)|fla|bench/flat/
q=ench/flat/ (mid-segment)|ench/flat/|bench/flat/
q=bench/flat/ (wire-raw slashes)|RAW:bench/flat/|bench/flat/
q=deep-00001.bin (basename)|deep-00001.bin|bench/deep/l2/l3/l4/l5/l6/
q=bench/tricky/pl+us/ (client-encoded)|bench/tricky/pl+us/|bench/tricky/pl+us/
q=bench/tricky/pl+us/ (wire-raw +)|RAW:bench/tricky/pl+us/|bench/tricky/pl+us/
q=bench/tricky/pl%2Bus/ (pre-encoded)|RAW:bench/tricky/pl%2Bus/|bench/tricky/pl+us/
q=bench/tricky/name with space/|bench/tricky/name with space/|bench/tricky/name with space/
q=bench/tricky/name+with+space/ (wire +)|RAW:bench/tricky/name+with+space/|bench/tricky/name with space/
q=bench/tricky/amp&ersand/ (client-encoded)|bench/tricky/amp&ersand/|bench/tricky/amp&ersand/
q=bench/tricky/hash#tag/ (client-encoded)|bench/tricky/hash#tag/|bench/tricky/hash#tag/
q=bench/tricky/v1.2.3/|bench/tricky/v1.2.3/|bench/tricky/v1.2.3/
q=bench/tricky/UPPER/|bench/tricky/UPPER/|bench/tricky/UPPER/
q=bench/upper/ (wrong case)|bench/upper/|bench/tricky/UPPER/
q=ψ-unicode/|ψ-unicode/|bench/tricky/ψ-unicode/
q=bench/nothing-here|bench/nothing-here|bench/nothing-here/
CASES
    adjacent
}

# Not part of the q question, but one request's worth of data next to it.
# The walk pays whatever the server's own page size is, so a caller-set page size would be an alternative lever.
adjacent() {
    echo
    echo "adjacent probe: pageSize on /search/assets (nxr sends none)"
    for ps in "" 5 1000; do
        u="$BASE/service/rest/v1/search/assets?repository=$REPO"
        [ -n "$ps" ] && u="$u&pageSize=$ps"
        n=$(curl -s -u "$AUTH" "$u" | jq '.items | length')
        printf '  pageSize=%-6s -> %s rows on page 1\n' "${ps:-none}" "$n"
    done
}

# The current shape, per directory: the walk is one and the same for every directory, and the client-side filter drops everything outside it.
# Derived from the full walk that ran above.
current_shape() {
    read -r fp fr fs ftc fcapped ffirst < "$WORK/stats.full"
    echo
    echo "the current shape, per directory (one full walk reused; filter = client-side):"
    python3 - "$WORK/paths.full" "$fp" "$fr" "$fs" <<'PY'
import sys
walk_path, pages, rows, secs = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
dirs = ["bench/flat/", "bench/wide/b/", "bench/wide/", "bench/deep/l2/l3/l4/l5/l6/", "bench/tricky/"]
paths = [l.rstrip("\r\n").lstrip("/") for l in open(walk_path, encoding="utf-8") if l.strip()]
print(f"{'directory':38} {'pages':>6} {'rows':>6} {'kept':>6} {'dropped':>8} {'secs':>6}")
for d in dirs:
    kept = sum(1 for p in paths if p.startswith(d))
    print(f"{d:38} {pages:>6} {rows:>6} {kept:>6} {int(rows)-kept:>8} {secs:>6}")
PY
}

# --- nxr itself -------------------------------------------------------------

# What the binary does with the same trees, if one is around.
nxr_evidence() {
    nxr="${NXR:-}"
    if [ -z "$nxr" ]; then
        nxr=$(ls "$DIR/../../target/release/nxr" "$DIR/../../target/debug/nxr" 2>/dev/null | head -1)
    fi
    if [ ! -x "${nxr:-}" ]; then
        echo "nxr: no binary at NXR or target/{release,debug}/nxr, skipped"
        return 0
    fi
    # The battery's own way of handing credentials to the binary.
    NXR_USERNAME="admin" NXR_PASSWORD="$STAND_PASS" export NXR_USERNAME NXR_PASSWORD
    # The listing's cost is a fact in the server's request log, not a derivation from page counts.
    # Counted only when the stack is this machine's compose: nothing else touches the endpoint inside the window.
    counted=0
    if [ "$(docker compose -f "$DIR/compose.yaml" ps -q nexus 2>/dev/null)" != "" ]; then
        counted=1
    fi
    for d in "bench/flat/" "bench/" ""; do
        before=$(requests_now)
        t0=$(date +%s.%N)
        if out=$("$nxr" ls "$URL/$d" 2>"$WORK/nxr.err"); then
            n=$(printf '%s\n' "$out" | grep -c . || true)
            t1=$(date +%s.%N)
            printf 'nxr ls %-58s ok, %s entries, %ss' "$URL/$d" "$n" "$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.2f", b - a }')"
        else
            t1=$(date +%s.%N)
            printf 'nxr ls %-58s FAILED (%ss): %s' "$URL/$d" "$(awk -v a="$t0" -v b="$t1" 'BEGIN { printf "%.2f", b - a }')" "$(head -1 "$WORK/nxr.err")"
        fi
        if [ "$counted" = 1 ]; then
            printf ', %s search requests seen by the server' "$(( $(requests_now) - before ))"
        fi
        printf '\n'
    done
}

# Lines in the server's request log so far, as docker reports them, rotated segments included, so a rotation mid-window does not shift the count.
requests_now() {
    docker compose -f "$DIR/compose.yaml" exec -T nexus sh -c "grep -h 'GET /service/rest/v1/search/assets' /nexus-data/log/request*.log | wc -l" 2>/dev/null || echo 0
}

# --- run --------------------------------------------------------------------

stage="${1:-both}"
case "$stage" in stage1 | stage2 | both) ;; *) echo "unknown stage $stage (stage1|stage2|both)" >&2; exit 1 ;; esac
for s in 1 2; do
    case "$stage" in
    stage1) [ "$s" = 1 ] || continue ;;
    stage2) [ "$s" = 2 ] || continue ;;
    esac
    want=$(seed "$s")
    wait_index "$want"
    echo
    echo "== stage $s: $want seeded names (nexus $NEXUS_VERSION)"
    printf '%-38s %6s %6s %6s %6s %6s %6s %8s %9s %6s %8s\n' \
        "case" pages rows kept drop miss extra verdict guard secs totalCount
    run_cases "$s"
    current_shape
    nxr_evidence
    echo
done
echo "done; the stack is left running: stop it with docker compose -f $DIR/compose.yaml down -v"
