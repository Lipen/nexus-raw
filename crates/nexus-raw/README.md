# nxr

A general-purpose CLI for a Sonatype Nexus raw repository.
It uploads and downloads version directories described by a `claim.json`, verifies sha256 sibling markers and resumes interrupted transfers by diffing local and remote state.

## Install

```bash
cargo install --path crates/nexus-raw
```

## Quickstart

```bash
# publish a version directory (claim.json + artifacts + .sha256 markers)
nxr up --profile main --dir dist/1.4.0

# name it
nxr point latest 1.4.0 --if-newer --profile main

# fetch it elsewhere
nxr down --profile main --pointer latest --dir vendor/

# check a local build offline
nxr verify --dir dist/1.4.0
```

An interrupted transfer resumes by repeating the same command.

## Scripts

- `--json` emits NDJSON events (`plan`, `artifact`, `retrying`, `summary`).
- Exit codes: `0` converged, `1` data problem, `2` misuse, `3` transport.
- Credentials come from env only: `NXR_<PROFILE>_AUTH`, `NXR_AUTH`, `NXR_USERNAME` + `NXR_PASSWORD`.

## More

- Full command reference and guides: [docs/](../../docs/)
- The library behind the CLI: [nexus-raw-core](../nexus-raw-core/)
