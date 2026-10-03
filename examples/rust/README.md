# Rust example: an external consumer of `nexus-raw-core`

A standalone crate that pulls the library the way an outside user would:
from crates.io, pinned to the compatible minor (`"0.3"`).
It is deliberately outside the nexus-raw workspace.

Run it:

```bash
cargo run --manifest-path examples/rust/Cargo.toml
```

The demo spawns the mock server on a free port (`--port 0`, so parallel runs never collide), reads the `listening http://…` banner from its stdout and publishes a version directory (`up` with claim-first), names it through a channel, downloads it back and verifies the result offline.

To run against a real repository instead:

```bash
cargo run --manifest-path examples/rust/Cargo.toml -- https://nexus.example.com/repository/demo/
```

To develop against the workspace instead of the published crate, flip the dependency back to the path and run the same way:

```toml
nexus-raw-core = { path = "../../crates/nexus-raw-core" }
```

`MOCK_NEXUS_BIN` overrides the mock server binary path
(defaults to the repo's `target/debug/mock-nexus`).
