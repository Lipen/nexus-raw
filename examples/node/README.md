# Node example: an external npm consumer of `nexus-raw`

A standalone npm project that pulls the package the way an outside user would:
a `file:` dependency pointing at the bindings crate, no registry involved.
Once the package is on npm, swap the dependency to a version.

One-time setup (builds the native addon and links the dependency):

```bash
(cd crates/nexus-raw-napi && npm install && npm run build:debug)
cd examples/node && npm install
```

Run it:

```bash
node publish-and-consume.mjs
```

The demo spawns the mock server on a free port (`--port 0`, so parallel runs never collide), reads the `listening http://…` URL from the mock's stdout and publishes a version directory (claim-first), names it through a channel, downloads it back and verifies the result offline.

To run against a real repository instead:

```bash
node publish-and-consume.mjs https://nexus.example.com/repository/demo/
```

`MOCK_NEXUS_BIN` overrides the mock server binary path
(defaults to the repo's `target/debug/mock-nexus`).
