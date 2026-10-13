# mock-nexus

A mock Sonatype Nexus raw server with a failure-scenario table: the shared conformance fixture of this repository.
The Rust test suites, the Python reference suite, a human debugging a client, and external harnesses all drive the same list of scenarios, so a behavior pinned against the mock is pinned against one contract, not against whoever ran the test.

Std-only: one dependency (`serde_json`), one request per connection, `Connection: close` everywhere.
No keep-alive, no chunked responses, no async runtime: every byte on the wire is either storage contract or a named failure mode, and nothing else competes for attention.

## Quick starts

As a library in a Rust test:

```rust
use mock_nexus::{MockNexus, Scenario};

let server = MockNexus::start(Scenario::Atomic)?;
let url = format!("{}repository/raw-main/1.0.0/app.bin", server.base_url());
server.insert("repository/raw-main/1.0.0/app.bin", b"bytes")?;
```

As a binary in a terminal or a script:

```bash
cargo run -p mock-nexus -- atomic --port 8080
# listening http://127.0.0.1:8080
```

As a seeded instance with composed behavior:

```bash
cargo run -p mock-nexus -- atomic --seed-dir ./fixture --config rate-limit.json
```

The full surface: the [mock server reference](mock.md) (scenario table, flags, `--seed-dir`, `--config`, the library API, and how to add a scenario), and the generated [wire invariants](invariants.md) page.

## Presets and composition

Thirty-three named scenarios cover the failure modes a correct client must survive: transport cuts, framing lies, pacing, auth gates, service-API shapes, integrity litter.
Each is one `Scenario` variant, one name in `--print-scenarios`, one row on the invariants page.

When a test needs a combination no preset expresses, compose a `Spec` instead:

```rust
use mock_nexus::{MockNexus, Spec};

let server = MockNexus::start_spec(
    Spec::atomic().with_auth("ci", "secret").with_flaky(1),
)?;
```

Presets stay canonical and guarded.
Specs are code-level composition and never enter the contract tables.

## The drift guards

The scenario table is a contract held in four places: the Rust enum, the binary's name table, the generated invariants page, and the Python suite's classification table.
Four drift tests refuse a change that updates one place and not the others.
Adding a scenario is a documented, mechanical procedure.
Changing one means touching all four in the same commit.
