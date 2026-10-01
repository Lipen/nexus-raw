<div class="nxr-hero" markdown>

<img class="hero-mark" src="assets/nxr.svg" alt="The nxr logo" width="120" height="120" align="right" />

# curl for a Nexus raw repository

`nxr` publishes and fetches version directories whose files are digest-verified on the way in and on the way out.
One binary, no config file, nothing to install on the server.

[Get started](get-started.md){ .md-button .md-button--primary }
[The 30-second version](#the-30-second-version){ .md-button }

</div>

<div class="nxr-cards" markdown>

- :material-console: **Primitives** — `get`, `put`, `head`, `sha`: the URL in argv, credentials from `-u` or the environment.
- :material-upload: **Publish** — `up` scans a directory, diffs it against the server, then PUTs bytes and markers with parallel workers.
- :material-download: **Consume** — `down` fetches exactly the enumerated names and resumes part files through `Range: bytes=N-`.
- :material-brain: **Exit codes** — 0 ok, 1 data, 2 misuse, 3 transport. Every error prints a `hint:` line, and pipelines read NDJSON events with `--json`.
- :material-swap-horizontal: **Re-runs** — an interrupted transfer finishes by repeating the same command.
- :material-language-rust: **Rust API** — `nexus-raw-core` exposes the same operations as a library, one module per concern.

</div>

## The 30-second version

<svg role="img" viewBox="0 0 720 196" width="100%" style="max-width:720px" xmlns="http://www.w3.org/2000/svg">
  <title>How nxr moves artifacts: a producer publishes a version directory to a Nexus raw repository, names it with a channel, and a consumer fetches and verifies it</title>
  <g fill="none" stroke-width="1.5">
    <rect x="12" y="42" width="150" height="76" rx="12" stroke="currentColor" stroke-opacity="0.35"/>
    <rect x="265" y="24" width="190" height="112" rx="12" stroke="var(--nxr-hero-teal)"/>
    <rect x="558" y="42" width="150" height="76" rx="12" stroke="currentColor" stroke-opacity="0.35"/>
    <path d="M170 80 h84" stroke="var(--nxr-hero-teal)"/>
    <path d="m246 74 10 6-10 6" stroke="var(--nxr-hero-teal)"/>
    <path d="M458 80 h84" stroke="var(--nxr-hero-teal)"/>
    <path d="m534 74 10 6-10 6" stroke="var(--nxr-hero-teal)"/>
    <path d="M360 136v16" stroke="var(--nxr-hero-amber)" stroke-dasharray="3 4"/>
  </g>
  <g fill="currentColor" font-size="14">
    <text x="87" y="74" text-anchor="middle">producer</text>
    <text x="360" y="52" text-anchor="middle">Nexus raw repo</text>
    <text x="633" y="74" text-anchor="middle">consumer</text>
  </g>
  <g fill="currentColor" fill-opacity="0.55" font-size="11.5" font-family="var(--md-code-font-family, ui-monospace, monospace)">
    <text x="87" y="96" text-anchor="middle">dist/1.4.0/</text>
    <text x="360" y="78" text-anchor="middle">1.4.0/</text>
    <text x="360" y="98" text-anchor="middle" font-size="10.5">claim.json · bytes · .sha256</text>
    <text x="633" y="96" text-anchor="middle">vendor/1.4.0/</text>
    <text x="633" y="142" text-anchor="middle">nxr verify · offline</text>
  </g>
  <g fill="var(--nxr-hero-teal)" font-size="12.5" font-family="var(--md-code-font-family, ui-monospace, monospace)">
    <text x="216" y="66" text-anchor="middle">nxr up</text>
    <text x="504" y="66" text-anchor="middle">nxr down</text>
  </g>
  <g>
    <rect x="286" y="152" width="148" height="30" rx="15" fill="none" stroke="var(--nxr-hero-amber)"/>
    <text x="360" y="171" text-anchor="middle" fill="var(--nxr-hero-amber)" font-size="12.5" font-family="var(--md-code-font-family, ui-monospace, monospace)">latest → 1.4.0</text>
    <text x="444" y="171" fill="currentColor" fill-opacity="0.55" font-size="10.5" font-family="var(--md-code-font-family, ui-monospace, monospace)">--if-forward</text>
  </g>
</svg>

<figure class="nxr-term" data-cast="assets/cast/session.json" markdown>

<img src="assets/img/session.svg" alt="An animated transcript of a real session against the mock server: publish, name, fetch, verify.">

</figure>

What the recording shows, one beat per command:

1. **Publish** — `nxr up` puts a version directory on the server: the claim lands first, then the bytes and their `.sha256` markers.
2. **Name** — `nxr channel set --if-forward` points `latest` at it, and a pointer only moves forward.
3. **Fetch** — `nxr down` pulls that version back. An interrupted pull resumes by repeating the same command.
4. **Verify** — `nxr verify` checks every digest offline, no network.

Every line is real output: the binary this repository builds, against its [mock server](explanation/conformance.md).
`just demo` replays the session on your machine and takes it further — an edited artifact, the refusal to overwrite it, a flaky and an authenticating server: [run the demo](how-to/demo.md).
[Get started](get-started.md) is the same four commands, copy-pasteable.

## Where to go

| You want | Page |
| :-- | :-- |
| the recorded session, runnable on your machine | [run the demo](how-to/demo.md) |
| a working end-to-end tour | [get started](get-started.md) |
| publish a version and name it with a channel | [publish](how-to/publish.md) |
| fetch artifacts, verify, resume an interrupted pull | [consume](how-to/consume.md) |
| pipelines: NDJSON events and exit codes | [use in CI](how-to/ci.md) |
| decode a failure and recover | [when it breaks](how-to/troubleshoot.md) |
| every flag of every command, credentials and URLs | [CLI reference](reference/cli.md) |
| the wire protocol in full | [protocol](reference/protocol.md) |
| the Rust API of `nexus-raw-core` | [Rust API](reference/api.md) |
| why it is shaped this way | [design](explanation/design.md) |
