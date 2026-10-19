# Clone and update a repository

`nxr clone` copies a whole raw repository into a local directory: every artifact, every marker, verified against the storage.
A rerun resumes the parts a killed run left and skips what already matches, so the same command is the update.

The clone needs the search API to enumerate the repository.
A server without it refuses with `cannot enumerate` and names the missing source.
The search index lags recent writes: a name uploaded seconds ago may join the next run instead of this one.

## First copy

```bash
BASE=https://nexus.example.com/repository/raw-main

nxr clone "$BASE/" vendor/raw-main
```

```
plan: 0 to upload, 3 to download, 0 up to date
↓ bom/linux-x86_64.json ok
↓ app-1.4.0.zip ok
↓ pinned.xml ok
downloaded 3, skipped 0
```

Every artifact is streamed into a part file, hashed, checked against its remote marker, renamed, and given a local sibling.
The result verifies offline:

```bash
nxr verify vendor/raw-main/
```

```
verify: 3 ok, FAILED: none
uploaded 0, downloaded 0, skipped 3
```

## The update run

The same command again:

```bash
nxr clone "$BASE/" vendor/raw-main
```

```
plan: 0 to upload, 0 to download, 3 up to date
○ bom/linux-x86_64.json skipped
○ app-1.4.0.zip skipped
○ pinned.xml skipped
downloaded 0, skipped 3
```

The default cache lives at `vendor/raw-main/.nxr-state.json`.
It remembers the size, mtime and digest of every verified object, so an unchanged file is skipped without a read.
The cache is an accelerator, never authority: a missing, corrupt or foreign cache means "hash everything", not "trust me".

| Flag | Meaning |
|:-----|:--------|
| `--state <FILE>` | keep the cache at an explicit path |
| `--no-state` | read and write no cache, hash every local file |
| `--verify` | ignore the cache for this run, even where size and mtime match |
| `--plan` | print `download`, `skip` and `seal` lines, write nothing |
| `--prefix <PREFIX>` | clone only the names under this whole-segment prefix, repeatable |

`--state` and `--no-state` exclude each other.

## What a seal means

A local object can hold the right bytes while its marker is gone, for example after a crash between the rename and the marker write.
The clone hashes such an object, compares it with the remote marker, and writes the missing local marker without fetching the bytes again:

```
sealed 1
```

A local object that diverges from its own marker is never overwritten.
The run refuses with `mismatch` and exit 1, and the bytes stay exactly as they were.

## Migrate with `up`

A clone is a normal local tree, so it is also the offline copy a migration reads from:

```bash
nxr up vendor/raw-main/ https://other.example.com/repository/raw-main/
```

The upload scans the clone, sees the complete markers, and sends bytes plus markers to the new server.

## Narrow the copy

A repository can hold several release lines.
`--prefix` keeps only whole-segment prefixes:

```bash
nxr clone "$BASE/" vendor/current --prefix release/ --prefix pinned/
```

The filter applies client-side after the search walk, so a name is kept only when a prefix matches complete segments.
A prefix without the trailing slash is misuse (exit 2).

## Inspect before writing

`--plan` resolves the same actions as the real run and writes nothing:

```bash
nxr clone "$BASE/" vendor/raw-main --plan
```

```
download app-1.4.0.zip
skip bom/linux-x86_64.json
seal pinned.xml
```

With `--json`, each line is one object:

```json
{"plan":"download","name":"app-1.4.0.zip","size":45}
{"plan":"skip","name":"bom/linux-x86_64.json"}
{"plan":"seal","name":"pinned.xml"}
```

The summary event gains `"sealed"`, the count of markers written without a transfer.

## What clone does not do

- It never deletes a local file that the repository does not list.
  A clone is additive; a removed remote name stays on disk until you remove it.
- It does not follow a redirected URL or accept a server without the search API.
- It does not write the cache when `--no-state` is set or during `--plan`.
- It does not replace `down`: `down` remains the transfer for an explicit name list.
