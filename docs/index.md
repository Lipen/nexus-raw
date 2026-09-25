# nexus-raw

<img src="assets/nxr.svg" width="120" align="right" alt="The nxr logo" />

<p style="font-size:1.05em">
<code>nxr</code> uploads and downloads version directories to a Sonatype Nexus raw repository, verifies sha256 sibling markers and resumes interrupted transfers by diffing local and remote state.
</p>

<div class="grid cards" markdown>

<div markdown>
:material-upload: **Publish**  
`up` pushes a version directory: claim first, then bytes and markers in parallel workers.
</div>

<div markdown>
:material-download: **Consume**  
`down` resolves `latest` or a version, fetches only what is missing, verifies every digest.
</div>

<div markdown>
:currency-usd: **Free resume**  
An interrupted transfer is finished by repeating the same command — nothing is re-uploaded.
</div>

<div markdown>
:shield-check: **Trustworthy completion**  
Bytes plus a `<name>.sha256` marker, `sha256sum -c` format; divergent artifacts are never overwritten.
</div>

<div markdown>
:json: **Scriptable**  
`--json` emits stable NDJSON events; exit codes split data problems from transport trouble.
</div>

<div markdown>
:language-rust: **Embeddable**  
The `nexus-raw-core` crate exposes the same operations as a small async Rust API.
</div>

</div>

---

## Try it in one minute

```bash
cargo install --path crates/nexus-raw
just mock atomic --port 8080          # a throwaway Nexus on localhost
nxr --base http://127.0.0.1:8080/ up --dir dist/1.4.0
nxr --base http://127.0.0.1:8080/ down --pointer latest --dir vendor/
```

## Where to go

| You want | Page |
| :-- | :-- |
| a working end-to-end tour in five minutes | [get started](get-started.md) |
| publish a version, tame pointers and CI | [publish](how-to/publish.md) |
| fetch artifacts, verify, subset a version | [consume](how-to/consume.md) |
| profiles, config files, credentials | [configure](how-to/configure.md) |
| pipelines: NDJSON events and exit codes | [use in CI](how-to/ci.md) |
| decode a failure and recover | [when it breaks](how-to/troubleshoot.md) |
| every flag of every command | [CLI reference](reference/cli.md) |
| the wire protocol in full | [protocol](reference/protocol.md) |
| the Rust API of `nexus-raw-core` | [Rust API](reference/api.md) |
| why it is shaped this way | [design](explanation/design.md) |
