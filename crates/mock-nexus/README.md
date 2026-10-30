# mock-nexus

A mock Sonatype Nexus raw server with a failure-scenario table: the shared conformance fixture of the [nexus-raw](https://github.com/Lipen/nexus-raw) repository.

The Rust suites, the Python reference client, a human debugging a client and any external harness drive the same list of scenarios.
A behavior pinned against the mock is therefore pinned against a single contract rather than against one particular test run.

The server depends only on the standard library and `serde_json`.
It handles one request per connection and closes every connection, so there is no keep-alive, no chunked encoding and no async runtime.

## Quick starts

As a library in a Rust test:

```rust
use mock_nexus::{MockNexus, Scenario};

let server = MockNexus::start(Scenario::Atomic)?;
let url = format!("{}repository/raw-main/1.0.0/app.bin", server.base_url());
server.insert("repository/raw-main/1.0.0/app.bin", b"bytes")?;
```

As a binary a script or a terminal can drive:

```bash
cargo run -p mock-nexus -- atomic --port 8080
# listening http://127.0.0.1:8080
```

Seeded with files, with a behavior override on top:

```bash
cargo run -p mock-nexus -- atomic --seed-dir ./fixture --config rate-limit.json
```

The `atomic` scenario is the correct server: PUT, GET, HEAD, `Range` resume, `404` on unknown paths.
`mock-nexus --print-scenarios` names all of them.

The key is the path as served: a tree meant for a repository URL carries the prefix in the seed directory (`fixture/repository/raw-main/1.0.0/a.bin`), while the same file without it is served at `http://host/1.0.0/a.bin` and stays invisible to `/repository/raw-main/…` requests.

## Presets and composition

A table of named scenarios covers what a correct client must survive: transport cuts, framing lies, pacing, auth gates, service-API shapes, integrity litter.
Each is one `Scenario` variant, one name in `--print-scenarios`, one row on the generated invariants page.

When a test needs a combination no preset expresses, compose a `Spec`:

```rust
use mock_nexus::{MockNexus, Spec};

let server = MockNexus::start_spec(
    Spec::atomic().with_auth("ci", "secret").with_flaky(1),
)?;
```

Presets stay canonical and guarded.
Specs are code-level composition and never enter the contract tables.

## More

- The scenario table, the flags, `--seed-dir`, `--config`, the library API and how to add a scenario: [mock server reference](https://lipen.github.io/nexus-raw/reference/mock/).
- Every scenario and the invariants it pins: [wire invariants](https://lipen.github.io/nexus-raw/reference/invariants/).
- License: MIT.

<details>
<summary>For contributors: the drift guards</summary>

The scenario table is a contract held in four places: the Rust enum, the binary's name table, the generated invariants page, and the Python suite's classification table.
Four drift tests refuse a change that updates one place and not the others.
Adding a scenario is a documented, mechanical procedure.
Changing one means touching all four in the same commit.

</details>
