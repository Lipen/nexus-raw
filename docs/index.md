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
`up` pushes a whole directory: markers generated and verified by default, bytes and siblings in parallel workers.
</div>

<div markdown>
:material-download: **Consume**  
`down` fetches exactly the enumerated names, hashes on the fly, resumes from part files with `Range: bytes=N-`.
</div>

<div markdown>
:shield-check: **Trustworthy completion**  
Bytes plus a `<name>.sha256` marker in `sha256sum -c` format.
Divergent artifacts are never overwritten.
</div>

<div markdown>
:json: **Scriptable**  
`--json` for one-object commands and NDJSON event streams.
Exit codes split data problems from transport trouble, and every error carries a `hint:`.
</div>

<div markdown>
:language-rust: **Embeddable**  
The `nexus-raw-core` crate exposes four public layers, from raw transport to the `Nxr` facade.
</div>

</div>

---

## Try it in one minute

```bash
cargo install --path crates/nexus-raw
just mock atomic --port 8080                                        # a throwaway Nexus on localhost
nxr up dist/1.4.0/ http://127.0.0.1:8080/1.4.0/                     # markers included
nxr down http://127.0.0.1:8080/1.4.0/ vendor/app/ --name app.zip
```

## Where to go

| You want | Page |
| :-- | :-- |
| a working end-to-end tour in five minutes | [get started](get-started.md) |
| publish a version and name it with a channel | [publish](how-to/publish.md) |
| fetch artifacts, verify, subset a version | [consume](how-to/consume.md) |
| pipelines: NDJSON events and exit codes | [use in CI](how-to/ci.md) |
| decode a failure and recover | [when it breaks](how-to/troubleshoot.md) |
| every flag of every command, credentials and URLs | [CLI reference](reference/cli.md) |
| the wire protocol in full | [protocol](reference/protocol.md) |
| the Rust API of `nexus-raw-core` | [Rust API](reference/api.md) |
| why it is shaped this way | [design](explanation/design.md) |
