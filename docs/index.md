<div class="nxr-hero" markdown>

<img class="hero-mark" src="assets/nxr.svg" alt="The nxr logo" />

# curl for a Nexus raw repository

`nxr` publishes and fetches version directories that are provably complete.
One binary, no config file, nothing to install on the server.

[Get started](get-started.md){ .md-button .md-button--primary }
[The 30-second version](#the-30-second-version){ .md-button }

</div>

<div class="nxr-strip" markdown>

**Trust is mechanical.** An artifact counts as done only when its `<name>.sha256` sibling matches the bytes — and a divergent complete artifact is never overwritten.

</div>

<div class="nxr-cards" markdown>

- :material-console: **Primitives** — `get`, `put`, `head`, `sha`: the URL in argv, credentials from `-u` or the environment.
- :material-upload: **Publish** — `up` scans, diffs, then PUTs bytes and markers in parallel workers.
- :material-download: **Consume** — `down` fetches exactly the enumerated names and resumes part files through `Range: bytes=N-`.
- :material-brain: **Fail honestly** — exit codes 0/1/2/3, a `hint:` on every error, NDJSON event streams for pipelines.
- :material-swap-horizontal: **Resume by default** — an interrupted transfer is finished by repeating the same command.
- :material-language-rust: **Embed** — `nexus-raw-core` exposes the same operations as a Rust library, four layers deep.

</div>

![A real nxr session: publish, name with a channel, consume, verify](assets/img/hero-terminal.png)

## The 30-second version

The transcript above is real output, verbatim.
Publish a version directory, markers included and re-runs free:

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
plan: 4 to upload, 0 to download, 0 up to date
↑ bom/linux-x86_64.json ok
↑ app-1.4.0.zip ok
…
uploaded 4, downloaded 0, skipped 0
```

Name it so consumers do not hard-code versions, then pull it somewhere and check it offline:

```console
$ nxr channel set https://nexus.example.com/repository/raw-main/latest 1.4.0 --if-forward
channel: set https://nexus.example.com/repository/raw-main/latest → 1.4.0
$ V=$(nxr channel get https://nexus.example.com/repository/raw-main/latest)
$ nxr down "https://nexus.example.com/repository/raw-main/$V/" vendor/app/
plan: 0 to upload, 3 to download, 0 up to date
↓ app-1.4.0.zip ok
…
uploaded 0, downloaded 3, skipped 0
$ nxr verify vendor/app/
verify: 3 ok, FAILED: none
```

Everything above is one binary and one URL per call.
The [five-minute tour](get-started.md) runs it against a throwaway server on your machine, breakage included.

## What you stop maintaining

Every repository that speaks raw Nexus eventually grows the same shell script: a retry loop, a stall watchdog, a temp-file dance, a checksum step and a prayer.
It breaks in a new way every quarter, and nobody owns it.

```console
# the script you keep rewriting, per repository, per language
$ curl -f --connect-timeout 15 --speed-limit 1 --speed-time 30 -o "$tmp" "$url" \
  && sha256sum -c <<< "$(curl -fsSL "$url.sha256")" \
  && mv "$tmp" "$dst" \
  || echo "which step failed, and did the partial survive?"
```

`nxr` is that script, minus the prayer.
The full concern-by-concern comparison lives in [the design essay](explanation/design.md#the-curl-model). The three lines that sell it:

| Concern | The hand-rolled script | `nxr` |
| :-- | :-- | :-- |
| Partial artifacts | temp files and `mv`, or the partial survives a crash | hidden part files, rename only after verify |
| Digest check | a second fetch piped to `sha256sum -c` | checked against the remote marker while streaming |
| Re-running | re-downloads everything | diffs first, transfers only what is missing |

The script also has one bug class `nxr` refuses to inherit: it overwrites whatever is at the destination, including an artifact that diverged from the server.
A divergence is a human decision: the transfer stops with exit 1 instead of picking a winner.

## Where to go

| You want | Page |
| :-- | :-- |
| a working end-to-end tour in five minutes | [get started](get-started.md) |
| publish a version and name it with a channel | [publish](how-to/publish.md) |
| fetch artifacts, verify, resume an interrupted pull | [consume](how-to/consume.md) |
| pipelines: NDJSON events and exit codes | [use in CI](how-to/ci.md) |
| decode a failure and recover | [when it breaks](how-to/troubleshoot.md) |
| every flag of every command, credentials and URLs | [CLI reference](reference/cli.md) |
| the wire protocol in full | [protocol](reference/protocol.md) |
| the Rust API of `nexus-raw-core` | [Rust API](reference/api.md) |
| why it is shaped this way | [design](explanation/design.md) |
