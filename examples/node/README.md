# Node example: an external npm consumer of `nexus-raw`

A standalone npm project that installs the package the way an outside user would:
from the npm registry, which also pulls the prebuilt native addon for the platform.
It is deliberately outside the nexus-raw workspace.

Run it:

```bash
pnpm install
node publish-and-consume.mjs
```

The demo spawns the mock server on a free port (`--port 0`, so parallel runs never collide), reads the `listening http://…` URL from the mock's stdout and publishes a version directory (claim-first), names it through a channel, downloads it back and verifies the result offline.

To run against a real repository instead:

```bash
node publish-and-consume.mjs https://nexus.example.com/repository/demo/
```

To develop against the local addon instead of the registry build, build the addon and point the dependency back at the crate:

```bash
(cd crates/nexus-raw-napi && pnpm install && pnpm run build:debug)
```

Then set `"nexus-raw": "link:../../crates/nexus-raw-napi"` in `package.json` and run `pnpm install`.

`MOCK_NEXUS_BIN` overrides the mock server binary path
(defaults to the repo's `target/debug/mock-nexus`).
