# Terminal quickstart

Install the `nxr` binary and drive a Nexus raw repository from the shell.
`nxr` is curl for a raw repository: URL in argv, credentials from `-u` or the environment, no config file, nothing to install on the server.

The same operations as a Rust library: [the Rust quickstart](rust.md).
From Node: [the Node quickstart](node.md).

## Install

Prebuilt archives ship on every [GitHub release](https://github.com/Lipen/nexus-raw/releases), one per platform: `nxr-linux-x64`, `nxr-darwin-x64`, `nxr-darwin-arm64`, `nxr-windows-x64`.
The linux build is a musl static binary, so it runs on any glibc too.
Every archive unpacks `nxr` at its root, with README.md and LICENSE next to it, and each archive carries a `.sha256` sidecar:

```bash
curl -fsSLO https://github.com/Lipen/nexus-raw/releases/download/v0.7.0/nxr-linux-x64.tar.gz
curl -fsSLO https://github.com/Lipen/nexus-raw/releases/download/v0.7.0/nxr-linux-x64.tar.gz.sha256
sha256sum -c nxr-linux-x64.tar.gz.sha256
sudo tar -xzf nxr-linux-x64.tar.gz -C /usr/local/bin nxr
nxr --version
```

```console
nxr-linux-x64.tar.gz: OK
nxr 0.7.0
```

On macOS the archive is `nxr-darwin-x64.tar.gz`, on Apple Silicon `nxr-darwin-arm64.tar.gz`, on Windows `nxr-windows-x64.tar.gz` with `nxr.exe` inside.
If you already have a Rust toolchain, `cargo install nexus-raw` builds the same binary, and `cargo binstall nexus-raw` fetches the release archive through cargo.

## Credentials

Three sources, in precedence order: `-u user:pass` per call, `NXR_AUTH` (base64 of `user:pass`), `NXR_USERNAME` + `NXR_PASSWORD`.
`doctor` reports which source resolved, without printing the value, and probes the server:

```bash
export NXR_USERNAME=deployer
export NXR_PASSWORD="$SECRET_TOKEN"
nxr doctor https://nexus.example.com/repository/raw-main/1.4.0/
```

```console
doctor:
  [  ok  ] credentials: resolved from NXR_USERNAME + NXR_PASSWORD
  [  ok  ] tls: verification is ON
  [  ok  ] settings: workers 8, retry 4, stall 30s, connect 15s
  [  ok  ] proxy: no proxy in the environment
  [  ok  ] probe: HEAD https://nexus.example.com/repository/raw-main/1.4.0/ → HTTP 404
  all checks passed
```

Keep `-u` for interactive one-offs: the value is visible in `ps` and shell history.
The credential forms and their precedence: [the CLI reference](../reference/cli.md#credentials-and-urls).

## The one-liners

Point `BASE` at your repository and `V` at the version:

```bash
BASE=https://nexus.example.com/repository/raw-main
V=1.4.0
```

No server handy?
From the repository checkout, `cargo run -p mock-nexus -- atomic --port 8080` plays one; set `BASE=http://127.0.0.1:8080/repository/raw-main`.
The transcripts below ran against that mock, with the host shown as `nexus.example.com`.

One object, up and down:

```bash
printf 'release notes\n' > notes.txt
nxr put "$BASE/scratch/notes.txt" -f notes.txt --sha
nxr get "$BASE/scratch/notes.txt" -o notes.back
```

```console
$ nxr put "$BASE/scratch/notes.txt" -f notes.txt --sha
put: 14 bytes + marker 48b1a29e44eeff814abc6250e43395bf8ac81827f5791261378cb13b6699e37f → https://nexus.example.com/repository/raw-main/scratch/notes.txt
$ nxr get "$BASE/scratch/notes.txt" -o notes.back
get: 14 bytes → notes.back
```

A version directory, prepared once.
The `manifest.json` at the version root names what consumers may fetch, and `down` and `diff` read it by convention:

```bash
mkdir -p "dist/$V/bom"
printf 'payload\n' > "dist/$V/app.zip"
printf '{"dep":"x"}\n' > "dist/$V/bom/sbom.json"
printf '{"artifacts":["app.zip","bom/sbom.json","manifest.json"]}\n' > "dist/$V/manifest.json"
```

Compare before writing, then publish, then pull it back on another machine:

```bash
nxr diff "dist/$V/" "$BASE/$V/"          # both sides, nothing moves
nxr up "dist/$V/" "$BASE/$V/" --plan     # the upload plan only
nxr up "dist/$V/" "$BASE/$V/"            # the transfer
nxr down "$BASE/$V/" "vendor/$V/"        # the same names, verified, resumable
nxr verify "vendor/$V/"                  # digests again, offline
```

```console
$ nxr diff "dist/$V/" "$BASE/$V/"
same app.zip
same bom/sbom.json
same manifest.json
$ echo $?
0
$ nxr up "dist/$V/" "$BASE/$V/" --plan
upload app.zip
upload bom/sbom.json
upload manifest.json
$ nxr up "dist/$V/" "$BASE/$V/"
plan: 3 to upload, 0 to download, 0 up to date
↑ app.zip ok
↑ bom/sbom.json ok
↑ manifest.json ok
uploaded 3, downloaded 0, skipped 0
$ nxr down "$BASE/$V/" "vendor/$V/"
plan: 0 to upload, 3 to download, 0 up to date
↓ bom/sbom.json ok
↓ app.zip ok
↓ manifest.json ok
uploaded 0, downloaded 3, skipped 0
$ nxr verify "vendor/$V/"
verify: 3 ok, FAILED: none
uploaded 0, downloaded 0, skipped 3
```

An interrupted `up` or `down` finishes by repeating the same command: finished names are skipped, part files resume through `Range` requests.

Name the version, read the name back:

```bash
nxr channel set "$BASE/latest" "$V" --if-forward
nxr channel get "$BASE/latest"
```

```console
$ nxr -u deployer:s3cret channel set "$BASE/latest" "$V" --if-forward
channel: set https://nexus.example.com/repository/raw-main/latest → 1.4.0
$ nxr channel get "$BASE/latest"
1.4.0
```

`--if-forward` compares tokens in dotted-numeric order, so an older pipeline cannot roll `latest` back.

## In a pipeline

The machine-readable stream and the exit codes are the API:

```bash
nxr up "dist/$VERSION/" "$BASE/$VERSION/" --json \
  | jq -e 'select(.event=="summary") | .failed == []'
```

Exit `0` continues, `1` is a data problem (a retry fails identically), `2` is a broken job definition, `3` is transport trouble (the same command is expected to succeed on a later run).
The full table: [errors and exit codes](../reference/errors.md).

## Where to go next

- The whole release-and-consume walkthrough: [use in CI](../how-to/ci.md).
- Every flag of every command: [the CLI reference](../reference/cli.md).
- The same operations from Rust: [the Rust quickstart](rust.md).
- The same operations from Node: [the Node quickstart](node.md).
