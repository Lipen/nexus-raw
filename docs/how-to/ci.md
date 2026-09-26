# Use in CI

`nxr` is built for pipelines: stable exit codes, machine-readable events, free resume, credentials from the environment.

## Credentials

Set masked variables in your CI system and let `nxr` resolve them:

```yaml
variables:
  NXR_USERNAME: ci-bot        # masked
  NXR_PASSWORD: $SECRET_TOKEN # masked
```

or precompute the compact form once and store it as a single secret:

```yaml
variables:
  NXR_AUTH: $BASE64_CREDENTIALS   # base64 of "user:pass"
```

`printf 'ci-bot:%s' "$TOKEN" | base64` produces the `NXR_AUTH` value.
There is no rc-file to leak and no profile to forget: every command takes its URL from argv, so the secret and the target live and die with the job.
For interactive debugging, `-u user:pass` works too — remember it is visible in `ps`.

## Exit codes are the API

| Code | Meaning | A pipeline should |
|:-----|:--------|:------------------|
| `0` | ok: transferred, converged or a successful check | continue |
| `1` | data problem: mismatch, incomplete, missing, cannot enumerate | stop and page a human |
| `2` | misuse: bad flags, unsafe name, half-set credentials | fix the pipeline definition |
| `3` | transport: network, auth, TLS, 5xx after retries | retry the job later |

```bash
BASE=https://nexus.example.com/repository/raw-main
nxr up "dist/$VERSION/" "$BASE/$VERSION/" || status=$?
case ${status:-0} in
  0) echo published ;;
  3) echo "transport trouble, retrying later"; sleep 60
     nxr up "dist/$VERSION/" "$BASE/$VERSION/" ;;
  *) exit 1 ;;
esac
```

## NDJSON events

`--json` prints one event per line, stable shapes, easy to filter with `jq`:

```bash
nxr up "dist/$VERSION/" "$BASE/$VERSION/" --json | jq -c 'select(.event=="summary")'
```

```json
{"downloaded":0,"event":"summary","failed":[],"skipped":2,"uploaded":1}
```

The event kinds:

| Event | Shape |
|:------|:------|
| `plan` | `{"event":"plan","upload":[…],"download":[…],"skip":[…]}` |
| `artifact` | `{"event":"artifact","name":"…","state":"uploading","done":0,"total":123}` |
| `retrying` | `{"event":"retrying","name":"…","attempt":2,"reason":"…"}` |
| `summary` | `{"event":"summary","uploaded":0,"downloaded":0,"skipped":0,"failed":[]}` |

States: `uploading`, `downloading`, `done`, `skipped`.
Byte events are coalesced (at most one per 200 ms per name), so a slow link does not flood the log.
Simple commands are different: `head`, `put`, `sha`, `get -o` and `channel get` print **one JSON object**, not a stream — see the [CLI reference](../reference/cli.md#output).

## A release job, end to end

```bash
set -euo pipefail
V="1.$CI_PIPELINE_ID.0"
BASE=https://nexus.example.com/repository/raw-main
make "dist/$V"
nxr up "dist/$V/" "$BASE/$V/" --json > upload.ndjson
jq -e 'select(.event=="summary") | .failed == []' upload.ndjson > /dev/null
nxr channel set "$BASE/latest" "$V" --if-forward
```

No marker step: `up` generates and uploads the `.sha256` siblings itself.
The `up` line is re-runnable as-is — a retried job uploads only what is missing.

## A consumer job

```bash
set -euo pipefail
V=$(nxr channel get "$BASE/latest")
nxr down "$BASE/$V/" vendor/app/ --continue --json | jq -e 'select(.event=="summary") | .failed == []'
nxr verify vendor/app/
```

`--continue` makes a retried job resume the part files of the killed one.
The offline `verify` is the gate before anything links against the download.

## Check the setup before the run

```bash
nxr doctor "$BASE/latest" || exit 1
```

Fails with exit 2 when the job has no credentials and exit 3 when the server is unreachable — cheap to run at the top of a pipeline, before wasting a runner slot.
