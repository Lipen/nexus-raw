# Node example: a consumer of the local `nexus-raw` addon

A standalone npm project that links to `crates/nexus-raw-napi` through a local link dependency.
It runs the source under test, so the demo and a local build stay in sync.
The published package is covered by the release consumer smoke and the nightly `smoke.yml`.
This example is the local end-to-end run.

Build the addon, install the link and run (from the repository root):

```bash
(cd crates/nexus-raw-napi && pnpm ci && pnpm run build:debug)
cd examples/node && pnpm install --lockfile=false
node publish-and-consume.mjs
```

The demo spawns the mock server on a free port (`--port 0`, so parallel runs never collide), reads the `listening http://…` URL from the mock's stdout and publishes a version directory (claim-first), names it through a channel, downloads it back and verifies the result offline.

To run against a real repository instead:

```bash
node publish-and-consume.mjs https://nexus.example.com/repository/demo/
```

`MOCK_NEXUS_BIN` overrides the mock server binary path
(defaults to the repo's `target/debug/mock-nexus`).
