# Rust example: an external consumer of `nexus-raw-core`

A standalone crate that pulls the library the way an outside user would:
a path dependency today, a crates.io version once published.
It is deliberately outside the nexus-raw workspace.

Run it:

```bash
cargo run --manifest-path examples/rust/Cargo.toml
```

The demo spawns the mock server on :8090, publishes a version directory
(`up` with claim-first), names it through a channel, downloads it back
and verifies the result offline.

To run against a real repository instead:

```bash
cargo run --manifest-path examples/rust/Cargo.toml -- https://nexus.example.com/repository/demo/
```

`MOCK_NEXUS_BIN` overrides the mock server binary path
(defaults to the repo's `target/debug/mock-nexus`).
