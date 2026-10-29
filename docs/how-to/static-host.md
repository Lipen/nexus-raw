# Read from a static host

`nxr` is curl for a raw repository, and a raw repository can be any HTTP server that serves files: nginx, a bucket, a CDN.
The read side needs nothing but `GET`, `HEAD` and `Range`, so the consumer flow works against a host that is not Nexus at all.

This page is that read profile.
Writes, the search API and the service API are what a static host does not offer.

## What works

| Command | Why |
|:--|:--|
| `nxr head URL` | one `HEAD`, the status and the size |
| `nxr get URL -o FILE` | one `GET`, `.part` resume through `Range`, digest check when the host serves a `.sha256` sibling |
| `nxr get URL --segments 4` | concurrent range spans, assembled and checked |
| `nxr down URL DIR --names-from FILE` | the name list is explicit, so no search API is involved |
| `nxr verify DIR` | offline: bytes, markers and digests |
| `nxr doctor URL` | reachability, TLS and credentials, read-only |

## The recipe

A static host has no search API, so the name list is yours: take it from the host's own directory index (an `autoindex` page, a bucket listing, `find` over the served tree) and pass it once.

```bash
# one name per line, relative to the version directory
printf 'app-1.4.0.zip\nmanifest.json\n' > names.txt

nxr down https://static.example/app/1.4.0/ vendor/app --names-from names.txt
nxr verify vendor/app
```

`down` writes the `.sha256` marker it computed from the received bytes, so the local tree verifies offline from then on, and a rerun resumes instead of downloading again.
The first run cannot compare against a remote marker, because the host has none.

`--segments N` splits each file into N concurrent range requests.
A host that ignores `Range` and answers `200` with the whole body is detected, and the download falls back to one stream, so the flag is safe to leave on.

## What does not work

| Command | What happens |
|:--|:--|
| `nxr down URL --ls`, `nxr ls URL` | both need the server search API, so the run refuses with `cannot enumerate` (exit 1) |
| `nxr put`, `nxr up` | the host answers `405` to every write, so the run refuses with `read-only` (exit 1) and never retries |
| `nxr rm`, `nxr channel set`, `nxr mv` | the same read-only refusal |
| `nxr service repos/status` | exit 3: a static host has no Nexus management API |

A listing also needs a Nexus-shaped URL (`/repository/<name>/…`) to derive the repository name, so a host served under some other path fails the URL-shape check first, with exit 2.
That is why the name list is the way in.

## Try it locally

The `static-host` mock scenario is the same profile: reads and ranges, `405` to every write, no service plane.

```bash
mkdir -p site/repository/raw-main/1.0.0
printf 'payload\n' > site/repository/raw-main/1.0.0/app.bin
just mock static-host --port 8080 --seed-dir site
```

From another shell:

```bash
printf 'app.bin\n' > names.txt
nxr down http://127.0.0.1:8080/repository/raw-main/1.0.0/ vendor/app --names-from names.txt
nxr verify vendor/app
```

```console
$ nxr down http://127.0.0.1:8080/repository/raw-main/1.0.0/ vendor/app --names-from names.txt
plan: 0 to upload, 1 to download, 0 up to date
↓ app.bin ok
uploaded 0, downloaded 1, skipped 0
$ nxr verify vendor/app
uploaded 0, downloaded 0, skipped 1
verify: 1 ok, FAILED: none
```

A write goes the other way:

```console
$ nxr put -f site/repository/raw-main/1.0.0/app.bin http://127.0.0.1:8080/repository/raw-main/1.0.0/app.bin
error: read-only: http://127.0.0.1:8080/repository/raw-main/1.0.0/app.bin: HTTP 405
hint: the repository answered 405 to a write: a group repository is read-only (target a hosted member), a hosted one needs the write role, and a host without PUT support refuses the same way; that write changed nothing, so rerunning is safe
$ echo $?
1
```

## Credentials and TLS

The same rules as every other command: `-u user:pass`, `NXR_AUTH`, `NXR_USERNAME` plus `NXR_PASSWORD`, `--ca-bundle` for a private CA, and `--tls-insecure` only as the last resort.
See [the CLI reference](../reference/cli.md) for the full surface.

## Next steps

- The scenario behind this page, and the invariant it pins: [wire invariants](../reference/invariants.md).
- Serving the other direction, from a real Nexus: [publish a version](publish.md).
- The failure catalogue: [when it breaks](troubleshoot.md).
