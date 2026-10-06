# Publish a version

The producer side: take a build directory and publish it to a Sonatype Nexus raw repository with authentication on.
The scenario behind every transcript here: a release directory `1.4.0/` with an application archive, a bill of materials and a pin file, going to the `raw-main` repository.

## Prerequisites

- Reachable repository URL, for example `https://nexus.example.com/repository/raw-main/`.
- Credentials that may write to it.
- A build output directory on disk.

Put the credentials in the environment once and every later command picks them up:

```bash
export NXR_USERNAME=deployer
export NXR_PASSWORD="…"        # (1)!
```

1. Prefer your CI's masked variable here.
   `nxr` also accepts `NXR_AUTH` (base64 `user:pass`) or `-u user:pass` per call.

## The shape of a version directory

```text
dist/1.4.0/
├── app-1.4.0.zip              # bytes
├── bom/
│   └── linux-x86_64.json      # subdirectories are names too
├── manifest.json              # optional: the enumeration source for consumers
└── pinned.xml
```

Every relative path under the directory is an artifact name.
Segments match `[A-Za-z0-9._-]+`, and only the `.sha256` suffix is reserved: `latest`, `nightly` and `version.json` are ordinary names if your convention uses them.
Before anything touches the network, every local file is classified against its sibling marker (see [the four states of an object](troubleshoot.md#the-four-states-of-an-object)).

## Look before you leap: the plan

`--plan` prints the planned actions and transfers nothing:

```console
$ nxr up --plan dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
upload app-1.4.0.zip
upload bom/linux-x86_64.json
upload manifest.json
upload pinned.xml
```

Against a directory the server already holds, the same command prints `skip <name>` per up-to-date artifact.
The plan lists artifact names, not marker uploads, because each name carries its marker along automatically.
`nxr diff <local-dir> <url>` goes further: it reports the whole delta (`same`, `missing-local`, `missing-remote`, `diverged`) without writing anything.

## Publish

```bash
nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/   # (1)!
```

1. Re-running this exact line is always safe: finished names are skipped, missing ones transfer, diverging ones refuse.

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
plan: 4 to upload, 0 to download, 0 up to date
↑ pinned.xml ok
↑ manifest.json ok
↑ app-1.4.0.zip ok
↑ bom/linux-x86_64.json ok
uploaded 4, downloaded 0, skipped 0
```

Each name goes up as a pair: the bytes first, then its `sha256sum`-style marker.
With the default `--workers 8`, several names travel at once, but within a name the marker is never sent before its bytes:

```mermaid
%%{init: {"theme": "base", "themeVariables": {"primaryColor": "#eef2f2", "primaryTextColor": "#243b3a", "primaryBorderColor": "#5f7470", "lineColor": "#5f7470", "noteBkgColor": "#f4f1e8", "noteTextColor": "#4a4636", "noteBorderColor": "#c9c2a6", "fontFamily": "inherit"}}}%%
sequenceDiagram
    autonumber
    participant W as one nxr worker
    participant N as Nexus
    note over W,N: every name runs this pair, --workers names at a time
    W->>N: PUT 1.4.0/app.zip
    N-->>W: 201 Created
    W->>N: PUT 1.4.0/app.zip.sha256
    N-->>W: 201 Created
    note over W: markerless on the server? the next up fills exactly this gap
```

A crash between the two PUTs leaves the object with bytes but no marker.
The next `up` repairs exactly that.
[interrupted transfers](troubleshoot.md#interrupted-transfers) shows the repair.

Re-run the same command and the second transfer is free:

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
plan: 0 to upload, 0 to download, 4 up to date
○ app-1.4.0.zip skipped
○ bom/linux-x86_64.json skipped
○ manifest.json skipped
○ pinned.xml skipped
uploaded 0, downloaded 0, skipped 4
```

### Markers: against `--no-sha`

| | default (`up`, `put --sha`) | `up --no-sha` |
|:--|:--|:--|
| What is uploaded | bytes + a `<name>.sha256` marker per name | bytes only |
| Marker source | an existing sibling is checked, a missing one is generated from the bytes | none, none generated |
| Consumer side | `down` can check digests, `verify` works offline | nothing to check against |
| Divergence detection | refuses on any disagreement | blind spots everywhere |
| Use it for | anything a human or a pipeline will consume | scratch data nobody verifies |

A hand-written sibling is used as-is: `up` checks it against the bytes and refuses on disagreement instead of overwriting it.
On the server, the marker is an ordinary object, 80 bytes of `digest  name`:

```console
$ nxr head https://nexus.example.com/repository/raw-main/1.4.0/app-1.4.0.zip.sha256
head: 200 80 -
```

!!! warning "`--no-sha` leaves objects uncertified"

    Bytes uploaded with `--no-sha` have no marker on the server.
    A later `up` of a marker-bearing directory classifies them as `Markerless` and re-sends them complete with markers, but until then nothing can detect corruption or divergence for those names.

## Claim first: `--claim-first`

`--claim-first <NAME>` uploads one name first and alone, before any other name starts:

```bash
nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/ --claim-first manifest.json
```

The natural claim is `manifest.json`: the enumeration document becomes visible at the version URL before any byte it names, so a watcher sees the publish declare itself instead of a random artifact arriving first.
A failed claim aborts the run with nothing else sent.
A claim name outside the scanned directory is misuse (exit 2):

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/claim/ --claim-first nope.txt
error: misuse: claim-first: nope.txt is not among the scanned names of dist/1.4.0/
hint: check the command line arguments
$ echo $?
2
```

## When `up` refuses

Two situations stop the run before a single byte moves, because continuing would overwrite something.

A local file disagrees with its own marker because the bytes drifted after the marker was written:

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
error: mismatch: pinned.xml: local object is broken and must not be overwritten: digest mismatch: sibling 8eed95dc200589a2b132c182fac5ce9335866b9d2aa0225760ef944bac4b09c5, actual 4925cd5b6a8a02b33a3bd303661d19a84a640c533b32b78229659be743c1d2f6
hint: the two sides diverge; delete or fix one copy, never let nxr overwrite a diverging object
```

A local copy is complete and valid, but the server already holds a different complete object under the same name, for example a rebuild with different flags published twice into one version:

```console
$ nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/
error: mismatch: app-1.4.0.zip: complete on both sides with different digests: local 38e98b1651bd21fdc5da48c407b711947a0115b28a2b979cf637a5497c3720b5, remote 405e28e8bcf3d4a6a1098029dacc54c7ba14d9b08ecf2b3e15c377c0b896636a
hint: the two sides diverge; delete or fix one copy, never let nxr overwrite a diverging object
```

Both refuse with exit 1.
Fix the cause (rebuild, or republish under a new version directory) and run again.
A transfer never deletes and never overwrites a diverging object.
Deletion is the explicit `nxr rm`.

## Restrict the transfer with a manifest

`--manifest` takes a file, a URL, or `-` for stdin and limits the run to the listed names.
Everything listed must exist locally, and one ghost name refuses the whole run:

```console
$ echo '{"artifacts": ["app-1.4.0.zip", "ghost.zip"]}' | nxr up dist/1.4.0/ https://nexus.example.com/repository/raw-main/1.4.0/ --manifest -
error: missing: ghost.zip: exist nowhere
hint: the name is absent on both sides; check spelling and the manifest
```

Exit 1, nothing uploaded.
A listed name that exists and matches on both sides is skipped like any other name in the plan.

## Ship a manifest for consumers

A `manifest.json` in the version directory is what lets consumers run `down` without extra flags:

```json
{"artifacts": ["app-1.4.0.zip", "bom/linux-x86_64.json", "pinned.xml"]}
```

Keep the file in the version directory before uploading.
`up` ships it like any artifact, marker included.
`down` reads it as the enumeration source and fetches the listed names.
It does not pull `manifest.json` itself into the target directory.

Without it, consumers must pass `--manifest`, `--name` or `--ls` (the decision table: [choosing an enumeration source](consume.md#choosing-an-enumeration-source)).

## Name the version with a channel

A channel is a token file at any URL.
Write it after a successful publish and any consumer can resolve the current version without hard-coding one:

```bash
nxr channel set https://nexus.example.com/repository/raw-main/latest 1.4.0 --if-forward   # (1)!
```

1. `--if-forward` writes only when the new token compares forward in dotted-numeric version order.

```console
$ nxr channel set https://nexus.example.com/repository/raw-main/latest 1.4.0 --if-forward
channel: set https://nexus.example.com/repository/raw-main/latest → 1.4.0
$ nxr channel set https://nexus.example.com/repository/raw-main/latest 1.0.0 --if-forward
channel: kept https://nexus.example.com/repository/raw-main/latest at 1.4.0 (forward-only)
$ nxr channel get https://nexus.example.com/repository/raw-main/latest
1.4.0
```

The `kept` line is what stops a late CI job on an older branch from rolling `latest` back.
Drop `--if-forward` only when you mean to force the token.

## A nightly pattern

```bash
V="nightly-$(date +%F)"
BASE=https://nexus.example.com/repository/raw-main
mkdir -p "dist/$V" && install out/* "dist/$V/"
( cd "dist/$V" && printf '{"artifacts": [%s]}\n' \
    "$(ls | sed 's/.*/"&"/' | paste -sd, -)" > manifest.json )
nxr up "dist/$V/" "$BASE/$V/"                 # (1)!
nxr channel set "$BASE/nightly" "$V"          # (2)!
```

1. No marker step: `up` generates and uploads the `.sha256` siblings itself.
2. No `--if-forward`: each nightly is a fresh directory, so the token always moves forward.

Version directories are never reused, which is what keeps re-runs, resume and forward-only channels cheap.

!!! note "Retiring a version"

    Transfers never delete.
    `nxr rm` is the deliberate exception ([the CLI reference](../reference/cli.md#nxr-rm)).
    Retire a version explicitly: `rm` the enumerated names, then move the channel forward or `nxr point --clear` it.

## Next steps

- The consumer mirror of this page: [consume artifacts](consume.md).
- Wire it into a pipeline: [use in CI](ci.md).
- What a marker looks like on the wire: [the wire protocol](../reference/protocol.md).
- When `up` refuses and what the words mean: [when it breaks](troubleshoot.md).
