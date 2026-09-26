# Use in CI

`nxr` is built for pipelines: stable exit codes, machine-readable events, free resume, credentials from the environment.
This page wires a release job and a consumer job into GitLab CI. The shapes apply to any runner.

## Credentials

Set masked variables in your CI system and let `nxr` resolve them:

```yaml
variables:
  NXR_USERNAME: ci-bot          # masked
  NXR_PASSWORD: $SECRET_TOKEN   # masked
```

or precompute the compact form once and store it as a single secret:

```yaml
variables:
  NXR_AUTH: $BASE64_CREDENTIALS   # base64 of "user:pass"
```

`printf 'ci-bot:%s' "$TOKEN" | base64` produces the `NXR_AUTH` value.
There is no rc-file to leak and no profile to forget: every command takes its URL from argv, so the secret and the target live and die with the job.

Half-set credentials are a misuse error, caught before any request:

```console
$ NXR_USERNAME=ci-bot nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
error: misuse: NXR_USERNAME and NXR_PASSWORD must be set together
hint: check the command line arguments
```

!!! warning "Avoid `-u user:pass` in scripts"

    `-u` is for interactive debugging.
    In a script it lands in the command line, which `ps` shows to every user on the machine.

## Exit codes are the API

| Code | Meaning | A pipeline should |
|:-----|:--------|:------------------|
| `0` | ok: transferred, converged or a successful check | continue |
| `1` | data problem: mismatch, incomplete, missing, cannot enumerate | stop and page a human |
| `2` | misuse: bad flags, unsafe name, half-set credentials | fix the pipeline definition |
| `3` | transport: network, auth, TLS, 5xx after retries | retry the job later |

The classes are worth the distinction: exit 1 means the data is wrong and a retry will fail identically, exit 3 means the pipe hiccuped and the same command is expected to succeed later.
Transfers are resumable and re-runs are free, so a retry loop is always safe to write.

A small retry harness around any command:

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

`--json` prints one event per line with stable shapes, ready for `jq`:

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/ --json
{"download":[],"event":"plan","skip":[],"upload":["app-1.4.0.zip","bom/linux-x86_64.json","manifest.json","pinned.xml"]}
{"done":0,"event":"artifact","name":"bom/linux-x86_64.json","state":"uploading","total":39}
{"done":0,"event":"artifact","name":"app-1.4.0.zip","state":"uploading","total":3000000}
…
{"done":3000000,"event":"artifact","name":"app-1.4.0.zip","state":"done","total":3000000}
{"downloaded":0,"event":"summary","failed":[],"skipped":0,"uploaded":4}
```

The last line is all a job usually gates on, and a re-run of the same job converges instead of transferring:

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/ --json \
  | jq -c 'select(.event=="summary")'
{"downloaded":0,"event":"summary","failed":[],"skipped":4,"uploaded":0}
```

`uploaded: 0` with everything `skipped` is the converged signature.
`failed` is the list to be empty.

The event kinds:

| Event | Shape |
|:------|:------|
| `plan` | `{"event":"plan","upload":[…],"download":[…],"skip":[…]}` |
| `artifact` | `{"event":"artifact","name":"…","state":"uploading","done":0,"total":123}` |
| `retrying` | `{"event":"retrying","name":"…","attempt":2,"reason":"…"}` |
| `summary` | `{"event":"summary","uploaded":0,"downloaded":0,"skipped":0,"failed":[]}` |

Artifact states: `uploading`, `downloading`, `done`, `skipped`.
Byte progress is coalesced (at most one event per 200 ms per name), so a slow link does not flood the log.
Retries surface as `retrying` events. Here a server answered `503` twice before cooperating:

```console
$ nxr down "$BASE/$V/" vendor/app/ --json | jq -c 'select(.event=="retrying")'
{"attempt":2,"event":"retrying","name":"","reason":"transport: …/manifest.json: HTTP 503"}
{"attempt":3,"event":"retrying","name":"","reason":"transport: …/manifest.json: HTTP 503"}
```

An empty `"name"` means the retry happened during enumeration, before any artifact was named.

Simple commands are different: `head`, `put`, `sha`, `get -o` and `channel get` print **one JSON object**, not a stream (see [the CLI reference](../reference/cli.md#output)).

## A release job, end to end

```yaml
publish:
  stage: deploy
  script:
    - export NXR_USERNAME=ci-bot NXR_PASSWORD="$SECRET_TOKEN"
    - V="1.$CI_PIPELINE_ID.0"
    - make "dist/$V"
    - nxr up "dist/$V/" "https://nexus.example.com/repository/raw-main/$V/" --json > upload.ndjson
    - jq -e 'select(.event=="summary") | .failed == []' upload.ndjson > /dev/null
    - nxr channel set https://nexus.example.com/repository/raw-main/latest "$V" --if-forward
```

No marker step exists because `up` generates and uploads the `.sha256` siblings itself.
The `up` line is re-runnable as-is: a retried job uploads only what is missing.
`--if-forward` makes the last line rollback-proof even when two pipelines race.

## A consumer job

```yaml
fetch:
  stage: build
  script:
    - export NXR_USERNAME=ci-bot NXR_PASSWORD="$SECRET_TOKEN"
    - BASE=https://nexus.example.com/repository/raw-main
    - V=$(nxr channel get "$BASE/latest")
    - nxr down "$BASE/$V/" vendor/app/ --json | jq -e 'select(.event=="summary") | .failed == []'
    - nxr verify vendor/app/
```

A retried job picks up the part files of the killed one by default and resumes them through `Range` requests, so the retry needs no flag.
The offline `verify` is the gate before anything links against the download: no network, pure hashing.

## Check the setup before the run

`doctor` verifies credentials, settings and reachability without printing secrets:

```console
$ nxr doctor https://nexus.example.com/repository/raw-main/1.4.0/manifest.json
doctor:
  [  ok  ] credentials: resolved from -u flag
  [  ok  ] tls: verification is ON
  [  ok  ] settings: workers 8, retry 4, stall 30s, connect 15s
  [  ok  ] probe: HEAD https://nexus.example.com/repository/raw-main/1.4.0/manifest.json → HTTP 200
  all checks passed
```

Without credentials it fails fast and cheap:

```console
$ nxr doctor https://nexus.example.com/repository/raw-main/1.4.0/manifest.json
doctor:
  [ FAIL ] credentials: none found: anonymous requests; pass -u or export NXR_AUTH
  [  ok  ] tls: verification is ON
  [  ok  ] settings: workers 8, retry 4, stall 30s, connect 15s
  [  ok  ] probe: HEAD https://nexus.example.com/repository/raw-main/1.4.0/manifest.json → HTTP 401
1 check(s) failed
```

Exit 2 with no runner time wasted on a doomed transfer. Run it at the top of a pipeline.
The exit code tells you which side to fix: `2` is your job definition, `3` is the server or the network.

## Next steps

- What the producer side publishes and why: [publish a version](publish.md).
- What the consumer job downloads and how it verifies: [consume artifacts](consume.md).
- Decoding a failure from its `error:` line: [when it breaks](troubleshoot.md).
- The full event and exit-code contract: [errors and exit codes](../reference/errors.md).
