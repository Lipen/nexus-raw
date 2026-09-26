# Consume artifacts

The consumer side: fetch a version (or part of it) into a directory you can build against.

## The enumeration rule

`down` must know *which names* to fetch, and a Nexus raw repository has no guaranteed directory listing.
So every `down` names its enumeration source, explicitly or through one convention:

| Source | Invocation |
|:-------|:-----------|
| `manifest.json` at the version URL (the recommended publisher convention) | `nxr down <ver-url>/ vendor/app/` |
| a manifest file, URL or stdin | `nxr down <ver-url>/ vendor/app/ --manifest manifest.json` |
| explicit names | `nxr down <ver-url>/ vendor/app/ --name app.zip --name pinned.xml` |
| server search API (best-effort, experimental) | `nxr down <ver-url>/ vendor/app/ --ls` |

Without any source and without a server-side `manifest.json`, the run refuses with `cannot enumerate` and a hint — it never guesses names.

## Resolve through a channel

A channel is any token file naming a version.
Resolve it yourself, then `down` the directory it points at:

```bash
V=$(nxr channel get https://nexus.example.com/repository/raw-main/latest)
nxr down "https://nexus.example.com/repository/raw-main/$V/" vendor/app/
```

The second call needs no extra flags when the version directory carries a `manifest.json` — the publisher shipped one with `up`.

## What lands where

The version directory is mirrored under the target, including subdirectories:

```text
vendor/app/
├── app-1.4.0.zip
├── app-1.4.0.zip.sha256
├── bom/
│   └── linux-x86_64.json
│   └── linux-x86_64.json.sha256
└── pinned.xml
```

Downloads stream into a stable part file (`.nxr-part-<hash>`) in the destination directory, hash on the fly, and are checked against the remote marker before the final rename.
The local `<name>.sha256` sibling is always written, computed from the received bytes, so the result verifies offline even when the server had no marker.
A killed transfer leaves only part files — and `--continue` picks them up:

```bash
nxr down https://nexus.example.com/repository/raw-main/1.4.0/ vendor/app/ --continue
```

## Fetch a subset

```bash
nxr down https://nexus.example.com/repository/raw-main/1.4.0/ vendor/app/ \
    --name app-1.4.0.zip --name pinned.xml
```

`--name` (repeatable) fetches exactly the listed names — useful for large versions where a job needs one file.
A name that exists neither remotely nor locally is a data error, not a warning.

## Verify without the network

After any manual manipulation, or before building against the result:

```bash
nxr verify vendor/app/
```

`verify` re-hashes every artifact against its marker, entirely offline — a good first gate in a consumer's CI.
Pass `--manifest` to check a subset.
A failure exit 1 lists the names that are incomplete, broken or missing.

## Re-runs are free

`down` is idempotent: the second run diffs the local directory against the server and transfers only what is missing or divergent.
Interrupting a transfer and repeating the command is the intended recovery, not an error path.

!!! warning "Local changes are not overwritten blindly"

    A locally complete artifact whose digest matches the server is skipped.
    If digests diverge, `nxr` refuses with `mismatch` instead of silently overwriting either side — see [when it breaks](troubleshoot.md).
