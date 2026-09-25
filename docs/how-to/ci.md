# Use in CI

`nxr` is built for pipelines: stable exit codes, machine-readable events, free resume, environment-only credentials.

## Credentials in CI

Set masked variables in your CI system, then map them to `nxr`'s chain:

```yaml
variables:
  NXR_USERNAME: ci-bot        # masked
  NXR_PASSWORD: $SECRET_TOKEN # masked
```

or precompute the compact form:

```yaml
variables:
  NXR_AUTH: $BASE64_CREDENTIALS
```

## Exit codes are the API

| Code | Meaning | A pipeline should |
|:-----|:--------|:------------------|
| `0` | converged, nothing to do or transfer finished | continue |
| `1` | data problem: mismatch, incomplete build, claim drift, missing artifact | stop and page a human |
| `2` | misuse: bad flags, broken config, unsafe name | fix the pipeline definition |
| `3` | transport: network, auth, TLS, 5xx after retries | retry the job later |

```bash
nxr up --profile main --dir "dist/$VERSION"
case $? in
  0) echo published ;;
  3) echo "transport trouble, retrying later"; sleep 60; nxr up --profile main --dir "dist/$VERSION" ;;
  *) exit 1 ;;
esac
```

## NDJSON events

`--json` prints one event per line, stable shapes, easy to filter with `jq`:

```bash
nxr up --profile main --dir "dist/$VERSION" --json | jq -c 'select(.event=="summary")'
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
Bytes events are coalesced (at most one per 200 ms per name), so a slow link does not flood the log.

## A release job, end to end

```bash
set -euo pipefail
V="1.$CI_PIPELINE_ID.0"
make dist/ VERSION="$V"
sha256sum dist/"$V"/* > /dev/null   # however your markers are produced
nxr up   --profile main --dir dist/"$V" --json > upload.ndjson
jq -e 'select(.event=="summary") | .failed == []' upload.ndjson > /dev/null
nxr point latest "$V" --if-newer --profile main
```

The `up` line above is re-runnable as-is: a retried job uploads only what is missing.
