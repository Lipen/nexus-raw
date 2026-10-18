# Audit a repository for integrity

`nxr audit <REPO_URL>` answers the question `up` and `down` never ask: is what the repository holds still whole?
The command inspects the storage, prints one line per problem, then exits: it is a verdict, not a gate.
Two modes exist, and choosing between them is the whole decision.
The default structural mode inspects metadata only.
`--bytes` additionally reads every asset.
The modes catch different problem classes, and the price differs accordingly.

## What can go wrong

A raw repository stores whatever was put into it, and the damage classes follow from the write protocol:

| Class | What it is | How it happens |
|:------|:-----------|:---------------|
| `orphan` | bytes exist, no marker | a crash between the two PUTs of `up`, or manual fiddling |
| `dangling_marker` | marker exists, no bytes | a consumer gets 404 on a name the marker list still has |
| `unparseable_marker` | a marker outside the `sha256sum -c` format | manual marker edits, or a foreign tool writing siblings |
| `diverged_fact` | marker digest and search fact disagree | a rewrite over the name, or a manual marker edit |
| silent bitrot | bytes changed on disk, no metadata moved | blob store corruption, invisible to metadata |

The first four are structural: metadata proves or disproves them, and the structural mode catches all four.
Silent bitrot is the exception: only reading bytes and hashing them can see it.

## The default: structural audit

```
nxr audit <REPO_URL> [--name NAME]... [--prefix PREFIX]... [--json] [--workers N]
```

The structural mode builds its picture from three sources, in authority order:

- the raw tree: a full listing of the repository, markers included, the authoritative set of existing names.
- markers: one small GET per marker (about 80 bytes), fetched in parallel with AIMD pacing.
- search facts: when the search index is alive, an independent cross-check of every marker's digest.

Asset bytes are never read: the price is O(names), not O(content).
The mode classifies every problem it finds as `orphan`, `dangling_marker`, `unparseable_marker` or `diverged_fact`:

```console
$ nxr audit https://nexus.example.com/repository/raw-main/1.4.0/
orphan releases/2.0.0/big.bin: bytes without marker
dangling_marker releases/2.0.0/app.zip: marker without bytes
unparseable_marker releases/2.0.0/notes.txt: marker does not parse as sha256sum -c
diverged_fact releases/1.4.0/app-1.4.0.zip: marker f732… diverges from search fact a8a2…
audited 412, findings: 4, bytes checked 0
$ echo $?
1
```

One finding per line, `class name detail`, and the final summary names every class count.
The exit is 1: findings are data, not an instrument failure.
A clean repository prints only the summary:

```console
$ nxr audit https://nexus.example.com/repository/raw-main/1.4.0/ --prefix pinned/
audited 2, findings: 0, bytes checked 0
$ echo $?
0
```

Fresh uploads are never findings by absence: an object the search index has not indexed yet is not `diverged_fact`, because the tree and the markers are the truth, not the index.

## When `--bytes`: catching silent bitrot

```
nxr audit <REPO_URL> --bytes [--sample N] [--name NAME]... [--prefix PREFIX]... [--json] [--workers N]
```

`--bytes` does everything the structural mode does, then downloads every asset and sha256-checks it against its marker.
Only this mode catches silent bitrot: corruption that changed bytes without moving any marker or search digest.
No metadata-only run can find bitrot, because nothing in the metadata moved.

```console
$ nxr audit https://nexus.example.com/repository/raw-main/1.4.0/ --bytes
corrupted releases/1.4.0/app-1.4.0.zip: sha256 mismatch: marker a8a2… vs bytes f712…
audited 412, findings: 1, bytes checked 412
$ echo $?
1
```

The price is honest: `--bytes` reads every object, so the cost is the cost of the repository.
A 2 TB repository under `--bytes` is a 2 TB download, paced through the same worker pool and AIMD as `up` and `down`.
A structural audit of the same repository is one listing plus one small GET per marker.

## `--sample N`: deterministically cheaper

Huge repositories make full `--bytes` a project of its own.
`--sample N` byte-checks a deterministic sample of N objects instead of all of them:

```console
$ nxr audit https://nexus.example.com/repository/raw-main/2.0.0/ --bytes --sample 50
corrupted releases/2.0.0/big.bin: sha256 mismatch: marker a8a2… vs bytes f712…
audited 412, findings: 1, bytes checked 50
$ echo $?
1
```

The sample is a deterministic stride over the sorted name list, so the same repository selects the same objects and a finding is reproducible and a sampled audit is a schedulable, comparable routine.
`--sample` without `--bytes` is misuse: exit 2, there is nothing to sample without byte checks.

## The read-only guarantee

Neither mode writes anything.
Both issue GET and HEAD requests only: listing, markers, search, and under `--bytes` the asset bodies.
No request from `audit` mutates the repository, ever.
Run it against production with read-only credentials: nothing can go wrong on the server side by construction (see [the invariant row](../reference/invariants.md)).
`nxr doctor --deep` carries the same guarantee.

## Refusal when search is dead

When the search index refuses, the audit refuses with the enumeration hint instead of running over an incomplete listing.
A partial tree would silently shrink the checked set and report a clean repository that was never fully read.

A search 404 is a data refusal (exit 1):

```console
$ nxr audit https://nexus.example.com/repository/raw-main/1.4.0/
error: cannot enumerate: https://nexus.example.com/service/rest/v1/search/assets?repository=raw-main: the search endpoint answered 404
hint: repeat --name, use --names-from FILE, or use --ls when the server has the search API
$ echo $?
1
```

A search 400 means the repository is missing or is not raw: exit 3 with the repository hint, not the enumeration one.

A fresh upload missing from the not-yet-caught-up index is a different thing: the index answers, the fact for that one name is just not there yet, and the audit stays silent about it instead of inventing a finding.

## Filters, JSON and pacing

`--name` and `--prefix` narrow the audited set before any request goes out, with the same whole-segment semantics `diff` uses.
`--json` switches stdout to NDJSON: one `finding` event per finding, then the final `summary` object.
The transfer mechanics are the proven ones: the same worker pool, the same AIMD pacing, `--workers` as the ceiling.

| Exit | When |
|:----:|:-----|
| `0` | clean: no findings, whatever the mode |
| `1` | findings exist: the report on stdout is the payload |
| `2` | misuse: `--sample` without `--bytes`, a non-http URL |
| `3` | transport or auth failure |

Findings are data, not errors: the instrument worked when it found and reported them.
Every flag in detail: [the CLI reference](../reference/cli.md#nxr-audit).
The catalogue behind every refusal: [errors and exit codes](../reference/errors.md).

## Next steps

- Check the server itself before a big `--bytes` run: [check the server before the transfer](troubleshoot.md#check-the-server-before-the-transfer).
- Publish and consume workflows around the audits: [publish a version](publish.md), [consume artifacts](consume.md).
- What the checks put on the wire: [the wire protocol](../reference/protocol.md).
