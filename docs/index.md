# nexus-raw

<img src="assets/nxr.svg" width="120" align="right" alt="The nxr logo" />

<p style="font-size:1.05em">
<code>nxr</code> is curl for a Sonatype Nexus raw repository: HTTP-grade primitives with retries, stall detection and TLS on, verified directory transfers on top, channel refs and manifests above those — one static binary, zero config files.
</p>

<div class="grid cards" markdown>

<div markdown>
:material-curling: **Primitives**  
`get`, `put`, `head`, `sha` — every call self-sufficient: URL in argv, credentials from `-u` or the environment.
</div>

<div markdown>
:material-upload: **Publish**  
`up` pushes a whole directory: markers generated and checked by default, bytes and siblings in parallel workers.
</div>

<div markdown>
:material-download: **Consume**  
`down` fetches exactly the enumerated names, hashes on the fly, resumes part files with `Range: bytes=N-`.
</div>

<div markdown>
:material-shield-check: **Trustworthy completion**  
An artifact counts as done only with its `<name>.sha256` marker in `sha256sum -c` format.
Divergent artifacts are never overwritten.
</div>

<div markdown>
:material-code-json: **Scriptable**  
`--json` for one-object commands and NDJSON event streams.
Exit codes split data problems from transport trouble, and every error carries a `hint:`.
</div>

<div markdown>
:material-language-rust: **Embeddable**  
The `nexus-raw-core` crate exposes four public layers, from raw transport to the `Nxr` facade.
</div>

</div>

![A real nxr session: publish, name with a channel, consume, verify](assets/img/hero-terminal.png)

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
