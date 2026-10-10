<div class="nxr-hero" markdown>

<img class="hero-mark" src="assets/nxr.svg" alt="The nxr logo" width="120" height="120" align="right" />

# curl for a Nexus raw repository

`nxr` publishes and fetches version directories whose files are digest-verified on the way in and on the way out.
One binary, no config file, nothing to install on the server.

[Get started](get-started.md){ .md-button .md-button--primary }
[The 30-second version](#the-30-second-version){ .md-button }

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
    <text x="87" y="96" text-anchor="middle" textLength="86" lengthAdjust="spacingAndGlyphs">dist/1.4.0/</text>
    <text x="360" y="78" text-anchor="middle" textLength="60" lengthAdjust="spacingAndGlyphs">1.4.0/</text>
    <text x="360" y="98" text-anchor="middle" font-size="10.5" textLength="170" lengthAdjust="spacingAndGlyphs">manifest.json · bytes · .sha256</text>
    <text x="633" y="96" text-anchor="middle" textLength="86" lengthAdjust="spacingAndGlyphs">vendor/1.4.0/</text>
    <text x="633" y="142" text-anchor="middle" textLength="118" lengthAdjust="spacingAndGlyphs">nxr verify · offline</text>
  </g>
  <g fill="var(--nxr-hero-teal)" font-size="12.5" font-family="var(--md-code-font-family, ui-monospace, monospace)">
    <text x="216" y="66" text-anchor="middle">nxr up</text>
    <text x="504" y="66" text-anchor="middle">nxr down</text>
  </g>
  <g>
    <rect x="286" y="152" width="148" height="30" rx="15" fill="none" stroke="var(--nxr-hero-amber)"/>
    <text x="360" y="171" text-anchor="middle" fill="var(--nxr-hero-amber)" font-size="12.5" font-family="var(--md-code-font-family, ui-monospace, monospace)" textLength="96" lengthAdjust="spacingAndGlyphs">latest → 1.4.0</text>
    <text x="444" y="171" fill="currentColor" fill-opacity="0.55" font-size="10.5" font-family="var(--md-code-font-family, ui-monospace, monospace)">--if-forward</text>
  </g>
</svg>

<figure class="nxr-term" data-cast="assets/cast/session.json" markdown>

<img src="assets/img/session.svg" alt="An animated transcript of a real session against the mock server: publish, name, fetch, verify.">

</figure>

What the recording shows, one beat per command:

1. **Publish**: `nxr up --claim-first manifest.json` puts a version directory on the server, the enumeration document first, then the bytes and their `.sha256` markers.
2. **Name**: `nxr channel set --if-forward` points `latest` at it, and a pointer only moves forward.
3. **Fetch**: `nxr down` pulls that version back, and an interrupted pull resumes by repeating the same command.
4. **Verify**: `nxr verify` checks every digest offline, no network.

Every line is real output: the binary this repository builds, against its [mock server](explanation/conformance.md).
`just demo` replays the session on your machine and takes it further: an edited artifact, the refusal to overwrite it, a flaky and an authenticating server.
The full tour: [run the demo](how-to/demo.md).
[Get started](get-started.md) is the same four commands, copy-pasteable.

## What do you want to do?

<div class="nxr-go" markdown>

- [:material-language-rust: **First transfer in Rust** *about 5 minutes*. The facade, one transfer, the local mock. `cargo add nexus-raw-core`](tutorials/rust/)
- [:material-language-typescript: **First transfer in Node** *about 5 minutes*. Promises over the same surface, a diff before any write. `pnpm add nexus-raw`](tutorials/node/)
- [:material-console: **First transfer in a terminal** *about 5 minutes*. The release binary and the one-liners. `nxr up`](tutorials/ops/)
- [:material-upload: **Publish a version** *about 2 minutes*. The manifest lands first, bytes and markers follow. `nxr up --claim-first`](how-to/publish/)
- [:material-download: **Fetch artifacts** *about 2 minutes*: named files come down verified, an interrupted pull resumes. `nxr down`](how-to/consume/)
- [:material-play-circle: **See it run** *half a minute*. The full session against the mock, then on your machine. `just demo`](how-to/demo/)
- [:material-robot: **Use it in CI** *about 5 minutes*. Exit codes, NDJSON events, credentials from the environment. `nxr --json`](how-to/ci/)
- [:material-wrench: **When it breaks**. Decode the error, recover, rerun safely. `nxr doctor`](how-to/troubleshoot/)

</div>

<p class="nxr-go__under" markdown>Under the hood: [every flag of every command](reference/cli.md), the four primitives included · [the wire protocol](reference/protocol.md) · [the Rust API](reference/api.md) · [why it is shaped this way](explanation/design.md).</p>
