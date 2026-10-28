<div class="nxr-hero" markdown>

<img class="hero-mark" src="assets/nxr.svg" alt="The nxr logo" width="120" height="120" align="right" />

# curl for a Nexus raw repository

`nxr` is a client for a Sonatype Nexus raw repository: the HTTP primitives, parallel directory transfers with sha256 markers, and resume after an interruption.
The repository layout is chosen by the caller, and the CLI is one static binary that needs no server-side component.

[Get started](get-started.md){ .md-button .md-button--primary }
[In 30 seconds](#in-30-seconds){ .md-button }

</div>

```bash
brew tap lipen/nexus-raw https://github.com/Lipen/nexus-raw
brew install lipen/nexus-raw/nexus-raw
cargo binstall nexus-raw                  # a prebuilt static archive
npm install @nexus-raw/nxr                # the Node package, `nxr` included
```

Release archives are published for Linux x64 and arm64, macOS Intel and Apple silicon, and Windows x64, together with a distroless container image.
[Get started](get-started.md) covers installation and the first transfer.

## In 30 seconds

<svg role="img" viewBox="0 0 720 196" width="100%" style="max-width:720px" xmlns="http://www.w3.org/2000/svg">
  <title>How nxr moves artifacts: a producer publishes a directory to a Nexus raw repository, optionally names it with a channel, and a consumer fetches and verifies it</title>
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

The recording runs four commands:

1. **Publish**: `nxr up` uploads a directory in parallel, generates the missing `.sha256` markers, and skips files that are already present.
2. **Name**: `nxr channel set --if-forward` writes the token file `latest`, and the guard rejects a move to an older token.
3. **Fetch**: `nxr down` downloads the listed files, hashes each stream and checks it against the server marker before the file gets its final name.
4. **Verify**: `nxr verify` checks every digest offline.

The recording uses a release directory and a `latest` channel, both conventions of the caller.
The output is real: the binary built from this repository, running against its [mock server](explanation/conformance.md).
`just demo` replays the session locally and also shows an edited artifact, the refusal to overwrite it, a flaky server and a server that requires credentials.
The full demo: [run it](how-to/demo.md).

## Guarantees

The following properties hold for every command rather than depending on a flag:

- With markers enabled, an artifact is complete only when its bytes and a `<name>.sha256` marker are both on the server.
  `up` writes the markers by default and `--no-sha` turns them off, `down` checks the downloaded stream against them, and `verify` checks the pair offline.
- A complete artifact whose digest differs from the local copy is reported and not overwritten.
- A repeated command transfers only what is missing, and an interrupted download continues from a hidden part file with one `Range` request.
- Exit codes separate the failure classes: `0` success, `1` data, `2` misuse, `3` transport.
  Every error carries a `hint:` line naming the next check.
- The server needs no agent or plugin, so the repository contents are the state.
  Commands that need a listing use the server search API and refuse with a hint when it is unavailable.

## Start here

<div class="nxr-go" markdown>

- [:material-console: **First transfer in a terminal** *Install the binary, then publish, name, fetch and verify a directory.* `nxr up`](get-started.md)
- [:material-language-rust: **First transfer in Rust** *The `Nxr` facade and one transfer against the local mock.* `cargo add nexus-raw-core`](tutorials/rust/)
- [:material-language-typescript: **First transfer in Node** *The same operations as promises, with a diff before any write.* `pnpm add @nexus-raw/nxr`](tutorials/node/)
- [:material-upload: **Publish a directory** *Upload order, markers, channels and a nightly release pattern.* `nxr up --claim-first`](how-to/publish/)
- [:material-download: **Fetch artifacts** *Enumerating names, downloading a subset, resuming an interrupted pull.* `nxr down`](how-to/consume/)
- [:material-play-circle: **See it run** *The recorded session against the mock server, and the same run on your machine.* `just demo`](how-to/demo/)
- [:material-robot: **Use it in CI** *Exit codes, NDJSON events and credentials from the environment.* `nxr --json`](how-to/ci/)
- [:material-wrench: **When it breaks** *Read the error and its hint, fix the cause, repeat the command.* `nxr doctor`](how-to/troubleshoot/)

</div>

## Other clients and libraries

The same protocol is implemented by three other clients and one library.
Each reads and writes the same store, so anything published by the CLI is visible to all of them.

<div class="nxr-go" markdown>

- [:material-view-dashboard: **Browse in a terminal** *`nxr-tui` opens a tab per server and shows the tree at any depth, with subtree downloads and a live filter.* `cargo install nexus-raw-tui`](reference/tui.md)
- [:material-folder-network: **Mount the repository** *`nxr-mount` exposes a repository as a read-only directory with lazy ranged reads over FUSE.* `nxr-mount <URL> /mnt/nexus`](reference/mount.md)
- [:material-transit-connection-variant: **Copy between servers** *`nxr mirror` copies a set of names between repositories and `nxr clone` copies a whole repository.* `nxr mirror`](how-to/compare.md)
- [:material-shield-check: **Audit integrity** *`nxr audit` reports orphans, dangling markers and disagreeing facts, over metadata or over every byte.* `nxr audit --bytes`](how-to/audit.md)

</div>

<p class="nxr-go__under" markdown>In process instead of in a shell: [the Rust facade](tutorials/rust.md) and [the Node promises](tutorials/node.md).
More: [every flag of every command](reference/cli.md), including the four primitives · [the wire protocol](reference/protocol.md) · [why it is shaped this way](explanation/design.md).</p>

## For agents

An agent can work from [the intent cookbook](how-to/agents.md), which maps a task to a command and to the output that confirms it.
Every error carries a `hint:` line, and `--json` produces NDJSON that a program can parse.
The installable skill lives in [`skills/nxr`](https://github.com/Lipen/nexus-raw/tree/master/skills/nxr), and the machine-readable index is [llms.txt](llms.txt).
