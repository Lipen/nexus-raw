#!/bin/sh
# Waits for the Nexus in compose.yaml to become ready, reads the initial admin
# password from the container and makes sure the `stand` raw repository exists.
# Every wait polls; the only sleep is inside the poll loop.
set -eu
DIR=$(cd "$(dirname "$0")" && pwd)
BASE="${NEXUS_BASE:-http://localhost:8081}"

compose() { docker compose -f "$DIR/compose.yaml" "$@"; }

# nexus3 accepts connections well before it can serve; the writable status is
# the honest signal. Five minutes is generous even for a cold CI volume.
i=0
until curl -sf "$BASE/service/rest/v1/status/writable" > /dev/null 2>&1; do
    i=$((i + 1))
    if [ "$i" -gt 150 ]; then
        echo "nexus did not become ready in time; last logs:" >&2
        compose logs --tail 100 >&2 || :
        exit 1
    fi
    sleep 3
done

# The first boot generates the admin password inside the data directory.
PASS=$(compose exec -T nexus cat /nexus-data/admin.password)

# Nexus 3.71+ gates every content path behind the EULA acceptance: until the
# disclaimer is posted back with accepted flipped to true, even admin gets
# 403 on /repository/*, while service paths work. GET returns it, POST
# accepts it; both are idempotent.
DISCLAIMER=$(curl -sf -u "admin:$PASS" "$BASE/service/rest/v1/system/eula")
ACCEPTED=$(printf '%s' "$DISCLAIMER" | sed 's/"accepted"[[:space:]]*:[[:space:]]*false/"accepted": true/')
curl -sf -u "admin:$PASS" -H 'Content-Type: application/json' \
    -X POST "$BASE/service/rest/v1/system/eula" -d "$ACCEPTED" > /dev/null

# The repository is created once; later runs of the stand reuse it. Checking
# first keeps the POST a pure creation instead of guessing what a 400 meant.
code=$(curl -s -o /dev/null -w '%{http_code}' -u "admin:$PASS" \
    "$BASE/service/rest/v1/repositories/stand")
if [ "$code" = 404 ]; then
    code=$(curl -s -o /dev/null -w '%{http_code}' -u "admin:$PASS" \
        -H 'Content-Type: application/json' \
        -X POST "$BASE/service/rest/v1/repositories/raw/hosted" \
        -d '{"name":"stand","online":true,"storage":{"blobStoreName":"default","strictContentTypeValidation":false,"writePolicy":"ALLOW"}}')
    if [ "$code" != 201 ]; then
        echo "repository creation failed with HTTP $code" >&2
        exit 1
    fi
elif [ "$code" != 200 ]; then
    echo "repository check failed with HTTP $code" >&2
    exit 1
fi

# The battery drives the server as admin: the anonymous machinery differs
# across nexus versions (realm ids drift), credentials do not. The file is
# 0600 and lives in /tmp for the stand's duration.
(umask 077 && printf 'STAND_URL=%s/repository/stand\nSTAND_PASS=%s\n' "$BASE" "$PASS" > "${STAND_ENV:-/tmp/nxr-stand.env}")
echo "stand repository ready at $BASE/repository/stand/"
