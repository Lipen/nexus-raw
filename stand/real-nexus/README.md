# The real-Nexus stand

A dockerized Nexus3 plus a battery of conformance checks: `nxr` drives a real
server, not only the mock.
The same battery runs nightly in CI (`.github/workflows/nightly.yml`) and on
demand with `just stand`.

## Run

```bash
just stand
```

Docker is the only prerequisite.
The pinned image is pulled on first use.
`NEXUS_BASE` and `NXR` retarget the battery at a different server or binary.

## Covered here, and what stays with the mock

- Covered: claim-first ordering, markers in the sha256sum format, resume
  (206/416), divergence refusal, channels, the search-backed listing (the
  directory listing included: a seeded directory's `q` answer must equal the
  full walk restricted to it), dry runs, down by name, offline verify, the
  segmented read against a real 206 answer, the `put` primitive, `clone` with
  its fact cache and `clone --verify`, `diff --via search` against the real
  index, `audit --bytes`, `doctor --deep`, `up --wait-index` waiting for the
  real index, `down --segments`, `up --prefix` keeping the selected subtree's
  path, `get --range`, `up --if-changed` on a converged tree, `rm` with its
  idempotent rerun, `point` clearing a channel, the management API
  (`service status`, `service repos`, `service repo`, `service assets`), and
  cross-repository `mirror` and `mv` against a second repository (`stand2`,
  created by the bootstrap).
- With the mock: auth-401, flaky 5xx, dropped connections - scenarios a real
  server cannot be asked to produce on demand.
- Not covered anywhere yet: TLS against a real CA (needs the lab), 429
  (no rate limiting locally).

## The `q` page-cutter measurement (manual)

`battery.sh` pins the resulting rule.

`search-pages.sh` seeds a few thousand names over several prefixes and depths
and measures whether the search `q` parameter may cut the full walk that the
listing pays today. It compares returned path sets per directory (the walk's
set against the `q` set: dropped, extra, equal) and prints tables, plus the
server's own request count per `nxr ls` from the request log.
It is a measurement, not a gate, so it is not wired into the battery or CI.
With the stack from `compose.yaml` up and bootstrapped:

```sh
sh stand/real-nexus/search-pages.sh both   # stage1 under nxr's page cap, stage2 past it
```

The property the measurement established is what `battery.sh` pins: a directory listing asks
the search with `q=<directory>/`, and on the pinned version that answer, filtered client-side,
equals the repository-wide walk restricted to the directory.
The battery seeds a handful of names for it and compares the two sets on every run.
