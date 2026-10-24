# The mount

`nxr-mount` turns a Nexus raw repository into a local directory: `ls`, `cat`, `diff -r` and `cp -r` read from the repository lazily.
It is a companion binary, not a subcommand of `nxr`: the CLI stays a portable curl, and the mount carries the platform-specific FUSE dependency on its own.

The mount is read-only today.
It answers listings, attributes and reads.
The Linux release archives carry `nxr-mount` next to `nxr`. `just mount` runs it from the workspace.
Every write gets `EROFS`.

## Run it

```bash
nxr-mount https://nexus.example/repository/raw /mnt/nexus
```

The URL is the repository directory to mount.
Credentials come from `-u user:pass`, `NXR_AUTH` (base64) or `NXR_USERNAME` + `NXR_PASSWORD`, in that order, exactly like `nxr`.
`--tls-insecure` skips certificate verification, `--ca-bundle FILE` adds a private CA (also `NXR_CA_BUNDLE`), and `--workers` caps parallel requests and FUSE dispatch threads.

Unmount with `fusermount3 -u /mnt/nexus`.

## How the tree is read

A raw repository has no directories: a path is a directory while any object sits under it.
The mount therefore resolves a node in two steps:

1. `HEAD` the URL.
   A `200` makes it a file, with the size from `Content-Length`.
2. On a `404`, list the URL through the search API.
   A non-empty listing makes it a directory, an empty one makes the path `ENOENT`.

`.sha256` markers are hidden from listings, exactly as `nxr ls` hides them.
Each name segment is percent-encoded when a repository URL is built, so a foreign name with spaces, `?`, `#`, `%` or unicode resolves.

Reads are lazy and block-cached: one block is `--block-kib` KiB (256 by default), fetched with a ranged GET, and the cache is bounded by `--cache-mib` (256 by default).
A server that ignores `Range` still works: the whole body is sliced to the requested span.
A read past the end of a file is empty, and `release` drops that file's blocks.

Listings and attributes are cached for two seconds, so a rapid `ls -R` does not re-walk every directory.
A listing reports a synthetic inode for an entry the kernel has not looked up. The real inode is assigned on lookup, and the node is dropped when the kernel forgets it.

## Limits

- Linux only in this build.
  `fuser` has no Windows backend, and macOS needs macFUSE and a separate build.
- The search API is required: a server without `/service/rest/v1/search/assets` answers `404`, and the mount cannot list directories.
- There is no write path yet: `create`, `write`, `unlink` and `mkdir` answer `EROFS`.
- A directory that exists with no objects under it cannot be represented, because the repository has no empty directories to prove.

## Flags

| Flag | Default | Meaning |
| :-- | :-- | :-- |
| `-u, --user` | env | Credentials as `user:pass`, curl style. |
| `--tls-insecure` | off | Skip TLS certificate verification. |
| `--ca-bundle` | none | A PEM bundle of extra trust roots for a private CA (also `NXR_CA_BUNDLE`). The bundle carries the CA that signed the server certificate, not the bare server leaf. Conflicts with `--tls-insecure`. |
| `--workers` | 8 | Parallel requests and FUSE dispatch threads. |
| `--retry` | 4 | Attempts per HTTP request. |
| `--connect-timeout-secs` | 15 | TCP connect timeout. |
| `--stall-secs` | 30 | Fail a request when no bytes arrive for this long. |
| `--block-kib` | 256 | One read block in KiB (minimum 4). |
| `--cache-mib` | 256 | Read-block cache budget in MiB (minimum 1). |

## Verify

`just mount-smoke` drives the whole path against the mock server on Linux.
It seeds a tree, waits out the search lag, mounts, compares the tree byte for byte, checks the size and the digest, and confirms that a write is refused.
The script skips with exit 0 when the platform or `/dev/fuse` is missing.
