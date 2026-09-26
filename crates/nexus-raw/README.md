# nxr

curl for a Sonatype Nexus raw repository: primitives with retries and TLS on, verified directory transfers, channel refs and manifests.
Every call is self-sufficient — URL in argv, credentials from `-u` or the environment.

## Install

```bash
cargo install --path crates/nexus-raw
```

## Quickstart

```bash
BASE=https://nexus.example.com/repository/raw-main

# publish a version directory: markers are generated and uploaded by default
nxr up dist/1.4.0/ "$BASE/1.4.0/"

# name it with a channel (any name works)
nxr channel set "$BASE/latest" 1.4.0 --if-forward

# fetch it elsewhere; manifest.json in the version directory drives the enumeration
nxr down "$BASE/1.4.0/" vendor/ --continue

# plain primitives
nxr put "$BASE/1.4.0/notes.txt" -f notes.txt --sha
nxr get "$BASE/1.4.0/notes.txt" -o notes.txt
nxr head "$BASE/1.4.0/notes.txt"
nxr sha notes.txt

# offline check of a local directory
nxr verify dist/1.4.0/
```

An interrupted transfer finishes by repeating the same command.
`down` without an enumeration source refuses with a hint instead of guessing names.

## Scripts

- Credentials: `-u user:pass`, `NXR_AUTH` (base64 `user:pass`) or `NXR_USERNAME` + `NXR_PASSWORD` — `-u` wins.
- `--json` emits machine output: one JSON object for `head`/`put`/`sha`/`get -o`/`channel get`, NDJSON events (`plan`, `artifact`, `retrying`, `summary`) for transfers.
- Exit codes: `0` ok, `1` data problem, `2` misuse, `3` transport.
- Every error prints a `hint:` line on stderr.

## More

- Full command reference and guides: [docs/](../../docs/)
- The library behind the CLI: [nexus-raw-core](../nexus-raw-core/)
