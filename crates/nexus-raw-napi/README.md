# @nexus-raw/nxr

Node bindings for [nexus-raw](https://github.com/Lipen/nexus-raw): the `nxr` command surface as promises over Sonatype Nexus raw storage.

Each command is one function, the URL is an argument, and there is no config file: credentials come from the call or from the environment.
Transfers follow the same rules as the CLI: an artifact carries a `<name>.sha256` marker, every run starts with a symmetric diff, and a complete artifact with a different digest is rejected instead of overwritten.

The package also installs the `nxr` command, so one install serves `require()` and the shell.

## Install

```bash
npm install @nexus-raw/nxr
```

Prebuilt addons ship for `linux-x64-gnu`, `linux-arm64-gnu`, `darwin-x64`, `darwin-arm64` and `win32-x64-msvc`.
Node 18 or newer.

## Quick start

```js
import { up, down, diff } from '@nexus-raw/nxr'

const repo = 'https://nexus.example.com/repository/raw-main/1.4.0/'

// Publish: bytes and their .sha256 markers go up in parallel.
const published = await up('dist/1.4.0', repo)
console.log(`uploaded ${published.uploaded}`)

// Fetch: the names you pass decide what comes down, each one verified.
const fetched = await down(repo, 'vendor/prebuilt', { names: ['app-1.4.0.zip', 'manifest.json'] })
console.log(`downloaded ${fetched.downloaded}`)

// Compare before writing anything.
const report = await diff('dist/1.4.0', repo)
for (const entry of report.entries) console.log(entry.state, entry.path)
```

Every call is self-sufficient, and a rerun of an interrupted transfer resumes instead of starting over.

## Errors and events

A rejected promise carries the CLI exit-code class and the same hint line the terminal prints:

```js
try {
  await up('dist/1.4.0', repo, { auth: { user: 'ci-bot', pass: process.env.NEXUS_TOKEN } })
} catch (err) {
  console.error(err.hint)     // "hint: ..." names the next thing to check
  process.exit(err.exitCode)  // 1 data, 2 misuse, 3 transport
}
```

`onEvent` receives the NDJSON events the CLI prints with `--json`: `plan`, `artifact` (with `state` and byte progress), `retrying`, and the final `summary`.
Every event has reached the callback by the time the promise settles.

```js
await up('dist/1.4.0', repo, {
  onEvent: (e) => {
    if (e.event === 'artifact' && e.state === 'done') console.log(`done ${e.name}`)
  },
})
```

## The surface

| Task | Functions |
|:---|:---|
| publish and fetch | `up`, `down`, `verify` |
| compare and clone | `diff`, `clone`, `clonePlan` |
| primitives | `get`, `put`, `head`, `sha` |
| layout | `channelGet`, `channelSet`, `lsEntries`, `lsAssets`, `pointClear` |
| deletion and copy | `rm`, `mirror` |
| server API | `serviceRepos`, `serviceAssets` |

Shared options: `auth`, `workers`, `retry`, `connectTimeoutMs`, `stallMs`, `tlsInsecure`, `caBundle`, `onEvent`.
The full types, including every option and result shape, are in [`index.d.ts`](https://github.com/Lipen/nexus-raw/blob/master/crates/nexus-raw-napi/index.d.ts).

The unscoped `nexus-raw` package on npm is a frozen pre-rename name.
It now re-exports this one, so `require('nexus-raw')` keeps working, but new code should depend on `@nexus-raw/nxr`.

<details>
<summary>Build the addon locally</summary>

The addon is a cargo build wrapped by the napi CLI, which also generates the loader and the declarations:

```bash
cargo build -p nexus-raw-napi          # the cdylib, like any workspace crate
pnpm install                           # the @napi-rs/cli build tool
pnpm run build:debug                   # addon + binding.cjs + binding.d.ts (the d.ts feeds check-dts.mjs, it does not ship)
pnpm run build                         # the same, in release mode
```

The generated files (`binding.cjs`, `binding.d.ts`, `*.node`) are local build outputs and gitignored.
`index.js` is the hand-written entry that enriches rejections with `exitCode` and `hint`, `index.d.ts` is the hand-maintained type surface, and the generated twin must agree with it (`node check-dts.mjs` fails on drift, CI runs it after every build).

Offline check of the built addon, and the from-source development loop:

```bash
node smoke.mjs
```

The pnpm project that consumes the package from source lives in [examples/node](https://github.com/Lipen/nexus-raw/tree/master/examples/node).

</details>

## More

- The Node quickstart, end to end with a local mock: [the Node tutorial](https://lipen.github.io/nexus-raw/tutorials/node/).
- Choosing between the CLI, the library and the bindings: [integrate nexus-raw](https://lipen.github.io/nexus-raw/how-to/integrate/).
- A web panel built on these bindings: [examples/panel](https://github.com/Lipen/nexus-raw/tree/master/examples/panel).
- License: MIT.
