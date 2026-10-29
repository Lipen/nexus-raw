# nxr-mount

A Nexus raw repository as a local directory.
`ls`, `cat`, `diff -r` and `cp -r` read from the repository lazily, without downloading it first.

It is a companion binary rather than a subcommand of `nxr`, so the CLI stays a portable curl and the mount keeps the platform-specific FUSE dependency.
It speaks FUSE through pure-Rust `fuser`, so there is no libfuse at build time.

## Run it

```bash
nxr-mount https://nexus.example/repository/raw /mnt/nexus
```

```console
$ ls /mnt/nexus/1.4.0/
app-1.4.0.zip  bom  manifest.json  pinned.xml
$ sha256sum /mnt/nexus/1.4.0/app-1.4.0.zip
405e28e8bcf3d4a6a1098029dacc54c7ba14d9b08ecf2b3e15c377c0b896636a  /mnt/nexus/1.4.0/app-1.4.0.zip
```

Unmount with `fusermount3 -u /mnt/nexus`.

Credentials follow the `nxr` rules: `-u user:pass`, `NXR_AUTH`, or `NXR_USERNAME` + `NXR_PASSWORD`, in that order.
`--tls-insecure` skips certificate verification, `--ca-bundle FILE` (or `NXR_CA_BUNDLE`) adds a private CA, and the two exclude each other.

The binary ships in the Linux release archives next to `nxr`.
To build it from a checkout, run `cargo build -p nexus-raw-mount --release`.

## What it does

- A path is a directory while any object sits under it, so a node resolves by `HEAD` (a `200` is a file with its `Content-Length`), then by the search API (a non-empty listing is a directory, an empty one is `ENOENT`).
- Reads are lazy and block-cached: one block is `--block-kib` KiB (256 by default) fetched with a ranged GET, and the cache is bounded by `--cache-mib` (256 by default).
- Listings and attributes are cached for two seconds, so a rapid `ls -R` does not re-walk every directory.
- `.sha256` markers are hidden from listings, exactly as `nxr ls` hides them.
- Every name segment is percent-encoded when a URL is built, so foreign names with spaces, `?`, `#`, `%` or unicode resolve.
- The URL must point inside `/repository/<name>/…`: the listing needs the repository name from the URL, so any other shape is refused at startup (exit 2) instead of mounting an empty tree.

## Limits

- Read-only: `create`, `write`, `unlink` and `mkdir` answer `EROFS`.
- Linux only in this build.
  `fuser` has no Windows backend, and macOS needs macFUSE and a separate build.
- The search API is required, so a server without `/service/rest/v1/search/assets` cannot be browsed through the mount.
- A directory with no objects under it cannot be represented, because the repository has no empty directories to prove.

## More

- FUSE semantics, every flag and the smoke test: [the mount reference](https://lipen.github.io/nexus-raw/reference/mount/).
- The CLI over the same data: [nexus-raw](https://crates.io/crates/nexus-raw).
- The library underneath: [nexus-raw-core](https://crates.io/crates/nexus-raw-core).
- License: MIT.
