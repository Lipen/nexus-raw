# nexus-raw: Node bindings

Node bindings for nexus-raw: the `nxr` command surface as promises over Nexus raw storage, built on napi-rs 3 (async exports on its tokio runtime, `@napi-rs/cli` packaging).
The npm package name is `nexus-raw`.
Publishing to npm is not set up.
Today the package is consumed as a `file:` dependency or from a vendored copy: the working example is [examples/node](../../examples/node).

## Build locally

The addon is a cargo build wrapped by the napi CLI, which also generates the loader and the declarations:

```bash
cargo build -p nexus-raw-napi          # the cdylib, like any workspace crate
pnpm install                           # the @napi-rs/cli build tool
pnpm run build:debug                   # addon + binding.cjs + binding.d.ts
pnpm run build                         # the same, in release mode
```

The generated files (`binding.cjs`, `binding.d.ts`, `*.node`) are local build outputs and gitignored.
`index.js` is the hand-written entry that enriches rejections with `exitCode` and `hint`, and `index.d.ts` is the hand-maintained type surface.
The generated twin `binding.d.ts` must agree with it: `node check-dts.mjs` fails on drift, and CI runs it after every build.

The offline check of the built addon:

```bash
node smoke.mjs
```

## More

Integration from the outside — CLI, Rust library, Node bindings, vendoring: [docs/how-to/integrate.md](../../docs/how-to/integrate.md).
