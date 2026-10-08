# Node quickstart

From an empty directory to a verified transfer in Node, in about five minutes.
The npm package `nexus-raw` exposes the `nxr` command surface as promises: one function per command, the URL as an argument, no config file.

Prebuilt native addons ship for Linux x64, Linux arm64, macOS x64 and macOS arm64.

The same operations from Rust: [the Rust quickstart](rust.md).
From a shell instead of a library: [the terminal quickstart](ops.md).

## Prerequisites

- Node 18+.
- `pnpm`, or npm, which works the same.
- A Rust toolchain, for the mock server of the next step.

## Start the mock

From the `nexus-raw` repository checkout:

```bash
cargo run -p mock-nexus -- atomic --port 8080
```

Without a checkout, `cargo install mock-nexus` puts the same server on your `PATH` (see [the Rust quickstart](rust.md#start-the-mock)).
`atomic` is the correct-server scenario: PUT/GET/HEAD, `Range` resume, 404 on unknown paths.
Everything below talks to `http://127.0.0.1:8080`.

## Create the project

```bash
mkdir first-transfer-js
cd first-transfer-js
pnpm init
pnpm add nexus-raw
```

## The script

Write `quickstart.mjs`:

```js
import { mkdirSync, writeFileSync } from 'node:fs'
import { up, down, diff } from 'nexus-raw'

// A version directory: the bytes plus the manifest that names them.
mkdirSync('dist/bom', { recursive: true })
writeFileSync('dist/app.bin', Buffer.alloc(4096, 7))
writeFileSync('dist/bom/deps.json', '{"webpack":"5"}')
writeFileSync(
  'dist/manifest.json',
  JSON.stringify({ artifacts: ['app.bin', 'bom/deps.json', 'manifest.json'] }),
)

const repo = 'http://127.0.0.1:8080/demo/2.0.0/'

// Publish: the bytes and their .sha256 markers go up in parallel.
const upSummary = await up('dist', repo)
console.log(`up: uploaded ${upSummary.uploaded}`)

// Fetch the version back: the manifest at the directory URL names what comes down.
const downSummary = await down(repo, 'vendor')
console.log(`down: downloaded ${downSummary.downloaded}`)

// Compare before writing anything: the local tree against the storage.
const report = await diff('dist', repo)
for (const entry of report.entries) {
  console.log(entry.state, entry.path)
}
```

Every function maps one-to-one to an `nxr` command: `up`, `down`, `diff`, and the primitives `get`, `put`, `head`, `sha` with the URL in argv.
Options carry the tuning, the credentials and the event stream: `auth: { user, pass }`, or `NXR_AUTH` and `NXR_USERNAME` + `NXR_PASSWORD` from the environment, exactly like the CLI, and `onEvent` for the NDJSON events.
The manifest at the version URL names what `down` fetches and what `diff` compares.

## Run it

```bash
node quickstart.mjs
```

```console
up: uploaded 3
down: downloaded 3
same app.bin
same bom/deps.json
same manifest.json
```

## Compare before you upload

Add a local file and ask for the delta again, still writing nothing:

```js
writeFileSync('dist/notes.txt', 'scratch')

const second = await diff('dist', repo)
for (const entry of second.entries) {
  if (entry.state !== 'same') console.log(entry.state, entry.path)
}
```

```console
missing-remote notes.txt
```

The promise resolves to the report whatever the delta holds, so the verdict stays yours.
`up` takes a `dryRun` option the same way: the plan instead of a transfer.

## Where to go next

- The full function list with signatures: [the API reference](../reference/api.md#node-bindings).
- The same operations from Rust: [the Rust quickstart](rust.md).
- The same operations from a shell: [the terminal quickstart](ops.md).
- What the wire actually carries: [the protocol](../reference/protocol.md).
