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
  (206/416), divergence refusal, channels, the search-backed listing, dry
  runs, down by name, offline verify.
- With the mock: auth-401, flaky 5xx, dropped connections - scenarios a real
  server cannot be asked to produce on demand.
- Not covered anywhere yet: TLS against a real CA (needs the lab), 429
  (no rate limiting locally).

## The `q` page-cutter measurement (manual, not part of the battery)

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
