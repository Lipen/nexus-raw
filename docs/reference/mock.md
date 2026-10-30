# The mock server

`mock-nexus` is the conformance fixture of the repository: a std-only HTTP/1.1 server speaking the exact storage contract `nxr` speaks, plus a table of failure scenarios a correct client must survive.
The same scenario list drives the Rust test suites, the Python reference suite, and the `mock-nexus` binary a human or an external harness drives.
One request per connection, `Connection: close` everywhere: no keep-alive, no chunked responses, nothing the storage protocol does not need.
That narrowness is the point: every byte on the wire is either contract or failure mode.

## The scenario table

The full table with per-scenario wire behavior and the pinned invariant lives in [invariants](invariants.md), generated from the same Rust constant the `--print-scenarios` output prints.
It covers: transport failures (`partial-put`, `cut-body`, `drop-connection`, `freeze-upload`), framing lies (`sizeless`, `foreign-marker`, `markerless`), pacing (`slow`, `flaky`, `retry-after-503`, `rate-limit` with `Retry-After`), server edges (`redirect`, `readonly`, `no-service`, `search-400`), service-API shapes (status, repositories, EULA, assets pagination), client-ergonomics (`detail-in-hint`, `ignore-range`), and integrity litter for `nxr audit` (`audit-litter`).

## Run the binary

```bash
cargo run -p mock-nexus -- atomic --port 8080
# listening http://127.0.0.1:8080
```

`--print-scenarios` prints the table as a JSON array, the same list the Python suite checks against.

Preset knobs ride the flags: `--partial-bytes N`, `--cut-after N --fake-length`, `--chunk-delay-ms N --chunk-size N`, `--flaky K`, `--lag-ms N` (the `search-lag` catch-up window, 6000 ms unless named), `--rate-429s N --retry-after-secs N`, `--location PATH`, `--auth user:pass`.

### `--seed-dir DIR`

The key is the path as served, so a tree meant for a repository URL carries the prefix in the seed directory (`DIR/repository/raw-main/1.0.0/a.bin`), and a tree without it is served at the server root (`http://host/1.0.0/a.bin`), invisible to `/repository/raw-main/…` requests.
Every file under `DIR` becomes an object at its relative path, recursively, before the first request:

```bash
cargo run -p mock-nexus -- atomic --seed-dir ./my-fixture
# seeded 12 object(s) from ./my-fixture
```

### `--config FILE`

A JSON manifest layers behavior on top of the preset scenario:

```json
{
  "flaky": { "first": 1 },
  "sizeless": true
}
```

Known keys: `auth.user` / `auth.pass`, `flaky.first`, `rate.first` + `rate.retry_after_secs`, `sizeless`, `ignore_range`, `read_only`, `markerless`, `foreign_marker`, `cut_body.after_bytes` + `cut_body.fake_length`, `partial_put.first_attempt_bytes`, `slow.chunk_delay_ms` + `slow.chunk_size`, `redirect.location_path`.
The manifest owns the knobs: preset flags for the same knobs are ignored when `--config` is present.

## Use the library

```rust
use mock_nexus::{MockNexus, Scenario};

let server = MockNexus::start(Scenario::Atomic)?;
let url = format!("{}repository/raw-main/1.0.0/app.bin", server.base_url());
server.insert("repository/raw-main/1.0.0/app.bin", b"bytes")?;
assert!(server.put_count("repository/raw-main/1.0.0/app.bin") > 0);
```

The handle is the test's window into the server: `base_url()`, `addr()`, `insert()` (a direct store write, no scenario gates), `store_get()`, `requests()` and `put_count()` (the request log), `enable_drift()` / `disable_drift()` (the library-only `doc-drift` toggle).
`start_group(&[&a, &b])` aggregates running instances into a read-only group: reads walk the members in order, writes refuse.

## Compose a custom behavior: `Spec`

Presets are named points in a behavior space.
[`Spec`](https://docs.rs/mock-nexus) is the space.
Layers stack, so a server that is both auth-gated and flaky - which no single preset expresses - is one chain:

```rust
use mock_nexus::{MockNexus, Spec};

let server = MockNexus::start_spec(
    Spec::atomic().with_auth("ci", "secret").with_flaky(1),
)?;
assert_eq!(server.scenario_name(), "spec");
```

`Spec::from(&scenario)` converts any preset losslessly, and every layer has a `with_*` builder (`with_partial_put`, `with_cut_body`, `with_drip`, `with_rate_limit`, `with_ignore_range`, `with_read_only`, and the rest).
A composed instance serves the plain atomic service surface: the content arms (seeded documents, the audit litter) are preset territory.

## Add a scenario (maintainers)

A scenario is contract, not code: four places must agree, and drift tests fail until they do.

1. `Scenario` enum + the `SCENARIOS` name table in `crates/mock-nexus/src/scenario.rs`.
2. A `SCENARIO_DOCS` row in `crates/mock-nexus/src/invariants.rs`.
3. Regenerate `docs/reference/invariants.md`: `UPDATE_INVARIANTS=1 cargo test -p mock-nexus invariant`.
4. A test or a SKIP row in `clients/python/test_conformance.py` (the drift guard refuses an unclassified scenario).

Then the behavior itself: the wire arms live in `http.rs` (pipeline gates) and `service_api.rs` (service REST pages), storage transforms in `storage.rs`, and a preset instance's knobs gather in `store.rs::Shared::hosted`.
`cargo test -p mock-nexus` runs the in-crate suite, including both drift guards.

## search-lag

The search index lags behind writes: the first storage PUT arms a catch-up deadline (6000 ms), and `GET /service/rest/v1/search/assets` answers the empty page until it passes, then a store-backed listing.
The store-backed items carry the size of the stored bytes and, when the writer left a marker, the marker's digest.
A marker row itself reports no digest, and the page never invents a placeholder.
The scenario models a real server's eventual search visibility, and pins the `up --wait-index` contract: the wait ends with every uploaded name reported visible, and an expired budget is a warning line, never a failed exit.
Storage reads and writes behave like `atomic`.
