# Run it as a container

The release ships a container image of `nxr` for CI systems that have a container runtime and no package manager, and for images that want the binary at build time.
The image is one static binary on a distroless base: no shell, no libc, nothing to install.

## The image

```console
$ docker pull ghcr.io/lipen/nexus-raw:latest
```

| Tag | What it is |
|:----|:-----------|
| `ghcr.io/lipen/nexus-raw:<version>` | one release, immutable, what a pipeline should pin |
| `ghcr.io/lipen/nexus-raw:latest` | the newest release, moved by the release workflow |

Both tags are multi-architecture (`linux/amd64` and `linux/arm64`), and the runtime picks the one the host speaks.
A cross-build that needs the other architecture asks for it:

```console
$ docker pull --platform linux/arm64 ghcr.io/lipen/nexus-raw:latest
```

## Run a transfer

The entrypoint is the binary itself, so everything after the image name is an `nxr` argument.
Mount the tree you transfer: the container filesystem is ephemeral and belongs to root.

```console
$ docker run --rm --network host --user "$(id -u):$(id -g)" -v "$PWD/dist:/w" -w /w \
    ghcr.io/lipen/nexus-raw:latest up /w/1.4.0/ http://127.0.0.1:8099/1.4.0/
plan: 3 to upload, 0 to download, 0 up to date
↑ bom/linux-x86_64.json ok
↑ pinned.xml ok
↑ app-1.4.0.zip ok
uploaded 3, downloaded 0, skipped 0
```

That run is against the mock server on the same machine (`mock-nexus atomic --port 8099`), which is also why it carries `--network host`.
The consumer side works the same way, and a re-run converges instead of transferring again:

```console
$ docker run --rm --network host --user "$(id -u):$(id -g)" -v "$PWD/out:/out" \
    -v "$PWD/names.txt:/names.txt" \
    ghcr.io/lipen/nexus-raw:latest down http://127.0.0.1:8099/1.4.0/ /out/ --names-from /names.txt
plan: 0 to upload, 3 to download, 0 up to date
↓ bom/linux-x86_64.json ok
↓ pinned.xml ok
↓ app-1.4.0.zip ok
uploaded 0, downloaded 3, skipped 0
```

The flags around the command, and why each is there:

| Flag | Why |
|:-----|:----|
| `-v "$PWD/dist:/w"` | the container filesystem dies with `--rm`, so the tree being transferred has to be a mount |
| `-w /w` | the container's working directory: argv paths are read inside the container, and the part files, markers and siblings `up` and `down` write land in the same tree |
| `--user "$(id -u):$(id -g)"` | the image runs as `nonroot` (uid 65532) by default. A bind mount belongs to your uid, so a write into it needs your identity: without the flag, `up` stops with `error: io: … Permission denied` |
| `--network host` | a server running on the host machine is not on the container network. A real Nexus is reachable by name and needs no flag |

That `docker run` line is the whole build step of a GitLab or Jenkins job: no package manager, no install step, one command.

Credentials arrive as environment variables, exactly like they do outside the container:

```console
$ export NXR_USERNAME=ci-bot NXR_PASSWORD="$SECRET_TOKEN"
$ docker run --rm --user "$(id -u):$(id -g)" -v "$PWD/out:/out" \
    -e NXR_USERNAME -e NXR_PASSWORD \
    ghcr.io/lipen/nexus-raw:latest down https://nexus.example.com/repository/raw-main/1.4.0/ /out/ --name app-1.4.0.zip
```

`-e NXR_USERNAME` without a value forwards the variable from your shell, so the secret never lands in the command line.
`-e NXR_AUTH` works the same way.
A run that only reads (`ls`, `head`, `get` to stdout, `verify`) writes nothing, so it needs no `--user` mapping: the default `nonroot` identity is enough.

## Bake the binary into your own image

The image is a carrier: the same static `nxr` sits at `/nxr`, one stage away from any base you like.

```dockerfile
# the tool, as a stage to copy from
FROM ghcr.io/lipen/nexus-raw:<version> AS nxr

# your image
FROM node:24-slim
COPY --from=nxr /nxr /usr/local/bin/nxr
```

When the base already carries a trust store and nothing else is wanted from the image, one line does it without a stage:

```dockerfile
COPY --from=ghcr.io/lipen/nexus-raw:<version> /nxr /usr/local/bin/nxr
```

!!! warning "The downstream base needs a trust store"

    The binary is static, but TLS verification reads the system trust store through `rustls-native-certs`, so the image it runs in must carry `/etc/ssl/certs/ca-certificates.crt`.
    A base built without it fails every HTTPS request with an unknown issuer.
    Copy the bundle from the same stage, which is what the release image itself relies on:

    ```dockerfile
    COPY --from=nxr /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt
    ```

    Distroless, Debian and Alpine bases with `ca-certificates` installed are fine as they are.

## Why the base is not `scratch`

- The binary is a static-pie musl executable, so it needs no libc and no loader.
- It does need the trust store: `gcr.io/distroless/static:nonroot` carries `/etc/ssl/certs/ca-certificates.crt` and sets `SSL_CERT_FILE`, and a `scratch` image has neither.
- The base has no shell and no package manager, so there is no `docker run -it <image> sh` and nothing in the image that could run one.
- It runs as the `nonroot` user (uid 65532, the entry is in `/etc/passwd`), never as root.
- The image is assembled from the release archives, not rebuilt from source: the release workflow unpacks `nxr-linux-x64.tar.gz` and `nxr-linux-aarch64.tar.gz` into `bin/amd64/` and `bin/arm64/`, and `packaging/container/Dockerfile` copies the binary out of there.
  No step runs the binary under emulation, and the `nxr` in the image is bit-for-bit the one the release archive carries.

## Tags and publishing

The release workflow builds the image and pushes both tags to GHCR after the gate and every platform build are green.
A rehearsal dispatch (`dry-run`) builds and smokes the image and stops short of the push.
The image is not a separate release step: a tag on the repository attests the image of that version too.

A GHCR package published by a workflow starts private, and an anonymous `docker pull` gets `denied` until someone flips its visibility once in the package settings.
An authenticated pull works either way:

```console
$ echo "$GH_TOKEN" | docker login ghcr.io -u <user> --password-stdin   # a token with read:packages
```

## Next steps

- How the image fits a pipeline with credentials, retries and exit codes: [use in CI](ci.md).
- What a consumer job fetches and verifies: [consume artifacts](consume.md).
- The flags, in full: [CLI reference](../reference/cli.md).
- The failure behind a refused write into a mount: [when it breaks](troubleshoot.md).
