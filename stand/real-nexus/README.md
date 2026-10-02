# The real-Nexus stand

A dockerized Nexus3 plus a battery of conformance checks: `nxr` drives a real
server, not only the mock.
The same battery runs nightly in CI (`.github/workflows/nightly.yml`) and on
demand with `just stand`.

## Run

```bash
just stand
```

Docker is the only prerequisite; the pinned image is pulled on first use.
`NEXUS_BASE` and `NXR` retarget the battery at a different server or binary.

## Covered here, and what stays with the mock

- Covered: claim-first ordering, markers in the sha256sum format, resume
  (206/416), divergence refusal, channels, the search-backed listing, dry
  runs, down by manifest, offline verify.
- With the mock: auth-401, flaky 5xx, dropped connections - scenarios a real
  server cannot be asked to produce on demand.
- Not covered anywhere yet: TLS against a real CA (needs the lab), 429
  (no rate limiting locally).
