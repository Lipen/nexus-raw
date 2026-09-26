# Get started

One binary, five minutes, a throwaway server: prepare a version directory, publish it, name it, consume it, break it and recover.
No Nexus installation is needed: the repository ships a mock server that speaks the same protocol.

## Prerequisites

- Rust 1.85+ through `rustup`, the only hard requirement.
- `just` is optional and only wraps the recipes below.

## Install

=== "From the checkout"

    ```bash
    cargo install --path crates/nexus-raw
    ```

=== "From the source tree (no install)"

    ```bash
    cargo run -p nexus-raw -- --version
    ```

```console
$ nxr --version
nxr 0.1.0
```

## Start a throwaway Nexus

The `mock-nexus` binary serves one failure scenario at a time.
`atomic` is the honest one: every fully received request is stored and served.

```bash
cargo run -p mock-nexus -- atomic --port 8080
```

```console
listening http://127.0.0.1:8080
```

Everything below talks to that process.
In a real deployment the URL is your repository, for example `https://nexus.example.com/repository/raw-main/`, and credentials travel per call (see [the CLI reference](reference/cli.md#credentials-and-urls)).

## Prepare a version directory

Any directory works: every relative path under it becomes an artifact name.

```text
dist/1.4.0/
├── app-1.4.0.zip
├── bom/
│   └── linux-x86_64.json
├── manifest.json             # (1)!
└── pinned.xml
```

1. The enumeration source for consumers.
   Without it, `down` needs `--manifest`, `--name` or `--ls`, a refusal you will trigger later in this tour.

```bash
nxr up dist/1.4.0/ http://127.0.0.1:8080/1.4.0/   # (1)!
```

1. Source directory first, destination URL second.
   No config file, no login step: the whole instruction is on one line.

```console
$ nxr up dist/1.4.0/ http://127.0.0.1:8080/1.4.0/
plan: 4 to upload, 0 to download, 0 up to date
↑ bom/linux-x86_64.json ok
↑ pinned.xml ok
↑ manifest.json ok
↑ app-1.4.0.zip ok
up: 4 sent, 0 fetched, 0 skipped
uploaded 4, downloaded 0, skipped 0
```

No marker step appears anywhere because `up` generated every `<name>.sha256` from the bytes before uploading.
The markers are ordinary `sha256sum -c` lines, and `up` leaves a copy next to your files:

```console
$ cat dist/1.4.0/app-1.4.0.zip.sha256
58e575b66d6c9388e63409246633bac6ecd070229fc1743b350b52436004a09a  app-1.4.0.zip
```

Run the same command again and nothing transfers, because the server already holds byte-identical copies with equal markers:

```console
$ nxr up dist/1.4.0/ http://127.0.0.1:8080/1.4.0/
plan: 0 to upload, 0 to download, 4 up to date
up: 0 sent, 0 fetched, 4 skipped
○ app-1.4.0.zip skipped
○ bom/linux-x86_64.json skipped
○ manifest.json skipped
○ pinned.xml skipped
uploaded 0, downloaded 0, skipped 4
```

A publishing job can be retried blindly: finished names are skipped, missing ones transfer, diverging ones refuse.

## Name the version

A channel is a token file at any URL. `latest` is just the most common name.
`--if-forward` compares tokens in dotted-numeric version order, so an older token never replaces a newer one:

```console
$ nxr channel set http://127.0.0.1:8080/latest 1.4.0 --if-forward
channel: set http://127.0.0.1:8080/latest → 1.4.0
$ nxr channel set http://127.0.0.1:8080/latest 1.3.0 --if-forward
channel: kept http://127.0.0.1:8080/latest at 1.4.0 (forward-only)
$ nxr channel get http://127.0.0.1:8080/latest
1.4.0
```

## Consume the version

Point `down` at the version URL.
It reads `manifest.json`, fetches exactly the listed names, hashes each stream on the fly, checks the digest against the server marker, and only then renames the file into place:

```console
$ nxr down http://127.0.0.1:8080/1.4.0/ vendor/app/
plan: 0 to upload, 3 to download, 0 up to date
↓ bom/linux-x86_64.json ok
↓ pinned.xml ok
↓ app-1.4.0.zip ok
down: 0 sent, 3 fetched, 0 skipped
uploaded 0, downloaded 3, skipped 0
```

Each artifact arrives with its own `<name>.sha256` sibling, so the result verifies offline:

```console
$ nxr verify vendor/app/
verify: 3 ok, FAILED: none
```

## Break it on purpose

Publish a version without a `manifest.json` and `down` refuses to guess instead of inventing a name list:

```console
$ nxr down http://127.0.0.1:8080/1.3.0/ vendor/old/
error: cannot enumerate: http://127.0.0.1:8080/1.3.0/: no manifest.json on the server and no --manifest/--name/--ls given
hint: pass --manifest <file|url|->, repeat --name, or use --ls when the server has the search API
```

Every failure prints this way: the kind, the facts, and a `hint:` line naming the next check.
[When it breaks](how-to/troubleshoot.md) catalogues them all.

Now interrupt a transfer mid-flight.
Run a second mock in its `slow` scenario, which trickles bytes, and kill `down` three seconds in (`timeout` here stands in for `Ctrl-C`):

```console
$ cargo run -p mock-nexus -- slow --chunk-delay-ms 300 --chunk-size 65536 --port 8095 &
listening http://127.0.0.1:8095
$ nxr up dist/1.4.0/ http://127.0.0.1:8095/1.4.0/ >/dev/null
$ timeout 3 nxr down http://127.0.0.1:8095/1.4.0/ vendor/resume/
plan: 0 to upload, 3 to download, 0 up to date
↓ bom/linux-x86_64.json ok
↓ pinned.xml ok
killed, timeout exit=124
$ ls -A vendor/resume/
.nxr-part-ad2572febfb75df3  bom  pinned.xml  pinned.xml.sha256
```

Two names finished, and the third died as a hidden part file (`.nxr-part-<hash>`).
A partially fetched name never reaches its final path, so the directory holds no half-truths.
Repeat the command with `--continue`. Finished names are skipped, and the part file is picked up through a `Range: bytes=N-` request:

```console
$ nxr down http://127.0.0.1:8095/1.4.0/ vendor/resume/ --continue
plan: 0 to upload, 1 to download, 2 up to date
○ bom/linux-x86_64.json skipped
○ pinned.xml skipped
↓ app-1.4.0.zip ok
down: 0 sent, 1 fetched, 2 skipped
```

The part file is gone, `app-1.4.0.zip` is complete, and `nxr verify vendor/resume/` passes.

## Next steps

- For producers, [publish a version](how-to/publish.md) shows markers, refusals, channels and a nightly pattern against a real server.
- For consumers, [consume artifacts](how-to/consume.md) covers enumeration sources, subsets and offline verification.
- For pipelines, [use in CI](how-to/ci.md) shows NDJSON events, exit codes and masked credentials.
- Every flag of every command: [CLI reference](reference/cli.md).
