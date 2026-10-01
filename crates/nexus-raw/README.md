# nxr

curl for a Sonatype Nexus raw repository.

## Install

```bash
cargo install nexus-raw
```

Needs Rust 1.85 or newer.

From the repository:

```bash
cargo install --git https://github.com/Lipen/nexus-raw nexus-raw --locked
```

From a checkout:

```bash
cargo install --path crates/nexus-raw --locked
```

Check the install with `nxr --version`.

## Quick start

```bash
BASE=https://nexus.example.com/repository/raw-main

# down needs a name list: manifest.json provides it
printf '{"artifacts": ["app-1.4.0.zip"]}\n' > dist/1.4.0/manifest.json
nxr up dist/1.4.0/ "$BASE/1.4.0/"
nxr channel set "$BASE/latest" 1.4.0 --if-forward
V=$(nxr channel get "$BASE/latest")
nxr down "$BASE/$V/" vendor/prebuilt
nxr verify vendor/prebuilt
```

If a transfer is interrupted, run the same command again: what already landed is skipped, the rest continues.

## Scripts

- Credentials, in checked order: `-u user:pass`, `NXR_AUTH` (base64 of `user:pass`), `NXR_USERNAME` + `NXR_PASSWORD`. `nxr doctor [URL]` names the resolved source.
- Exit codes: 0 ok, 1 data problem, 2 misuse, 3 transport.
- Every error prints a `hint:` line. `--json` emits one JSON object per line on stdout.

## More

- Guides and reference: <https://lipen.github.io/nexus-raw/>
- The repository: <https://github.com/Lipen/nexus-raw>
- License: MIT, see <https://github.com/Lipen/nexus-raw/blob/master/LICENSE>
