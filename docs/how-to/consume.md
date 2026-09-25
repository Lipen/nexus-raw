# Consume artifacts

The consumer side: fetch a version (or part of it) into a directory you can build against.

## Resolve and fetch

=== "By pointer"

    ```bash
    nxr down --profile main --pointer latest --dir vendor/panda
    ```

    The pointer file is read, its version token is resolved, and that version's claim drives the download.

=== "By version"

    ```bash
    nxr down --profile main --version 1.4.0 --dir vendor/app
    ```

Both create the target directory if needed.

## What lands where

The version directory is mirrored under `--dir`, including subdirectories and markers:

```text
vendor/panda/
├── claim.json
├── app-1.4.0.zip
├── app-1.4.0.zip.sha256
└── bom/
    └── linux-x86_64.json
    └── linux-x86_64.json.sha256
```

Downloads stream into a dot-prefixed temp file in the destination directory, hash on the fly, and are compared against the remote marker **before** the final rename.
A killed transfer leaves no half-written artifact behind.

## Fetch a subset

```bash
nxr down --profile main --version 1.4.0 --only app-1.4.0.zip --only pinned.xml --dir vendor/app
```

`--only` restricts the transfer to the named claim artifacts — useful for large versions where a job needs one file.

## Verify without the network

After any manual manipulation, or before publishing your own copy:

```bash
nxr verify --dir vendor/app
```

`verify` re-hashes every claim artifact against its marker, entirely offline — a good first gate in a consumer's CI.

## Re-runs are free

`down` is idempotent: the second run diffs the local directory against the server and transfers only what is missing or divergent.
Interrupting a transfer and repeating the command is the intended recovery, not an error path.

!!! warning "Local changes are not overwritten blindly"

    A locally complete artifact whose digest matches the server is skipped.
    If digests diverge, `nxr` refuses with `mismatch` instead of silently overwriting either side — see [when it breaks](troubleshoot.md).
