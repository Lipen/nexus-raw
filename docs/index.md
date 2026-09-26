# nexus-raw

<img src="assets/nxr.svg" width="120" align="right" alt="The nxr logo" />

<p style="font-size:1.05em">
<code>nxr</code> is curl for a Sonatype Nexus raw repository: HTTP-grade primitives with retries, stall detection and TLS on, verified directory transfers on top, channel refs and manifests above those — one static binary, zero config files.
</p>

<div class="grid cards" markdown>

- :material-curling: **Primitives** — `get`, `put`, `head`, `sha`: every call self-sufficient, URL in argv, credentials from `-u` or the environment.
- :material-upload: **Publish** — `up` pushes a whole directory: markers generated and checked by default, bytes and siblings in parallel workers.
- :material-download: **Consume** — `down` fetches exactly the enumerated names, hashes on the fly, resumes part files with `Range: bytes=N-`.
- :material-shield-check: **Trustworthy completion** — an artifact counts as done only with its `<name>.sha256` marker in `sha256sum -c` format, and divergent artifacts are never overwritten.
- :material-code-json: **Scriptable** — `--json` for one-object commands and NDJSON event streams, exit codes split data problems from transport trouble, and every error carries a `hint:`.
- :material-language-rust: **Embeddable** — the `nexus-raw-core` crate exposes four public layers, from raw transport to the `Nxr` facade.

</div>

![A real nxr session: publish, name with a channel, consume, verify](assets/img/hero-terminal.png)

---

## The 30-second version

The transcript above is real output, verbatim.
Publish a version directory — markers included, re-runs free:

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

`nxr` is that script, minus the prayer:

| Concern | The hand-rolled script | `nxr` |
| :-- | :-- | :-- |
| Retries with backoff | a loop someone wrote at 2 a.m. | built in, transport-level |
| Stalled connections | `--speed-limit` folklore | per-connection stall timeout |
| Partial artifacts | temp files, `mv`, crossed fingers | hidden part files, rename only after verify |
| Digest check | a second fetch and `sha256sum -c` | checked against the remote marker while streaming |
| Resume after a break | nothing, or `curl -C -` per URL | `--continue`, stable part files per name |
| What failed | an exit code, if you are lucky | 1 data / 2 misuse / 3 transport, each with a `hint:` line |
| Re-running | re-downloads everything | diffs first, transfers only what is missing |

The script also has one bug class `nxr` refuses to inherit: it overwrites whatever is at the destination, including an artifact that diverged from the server.
A divergence is a human decision — the transfer stops with exit 1 instead of picking a winner.

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
