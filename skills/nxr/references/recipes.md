# Recipes

Concrete tasks with the exact commands.
`$NEXUS` stands for a server root such as `https://nexus.example.com`, and every URL below is a directory URL inside a raw repository.

## Publish a version directory

```sh
nxr up dist/1.4.0/ "$NEXUS/repository/releases/1.4.0/"
```

`up` walks the local tree, skips the names already complete and identical on the server, uploads the rest, and writes a `<name>.sha256` marker after each name's bytes.
Success looks like `uploaded N` and exit 0.
To put the version document on the server before anything else, add `--claim-first version.json` so a killed publish never leaves discoverable bytes behind a missing document.

## Resume a transfer that was killed

Rerun the same command.
There is no resume flag to remember.

```sh
nxr up dist/1.4.0/ "$NEXUS/repository/releases/1.4.0/"
```

Finished names are skipped, and the `<name>.part` files left by the killed run are the resume fuel.
Use `--fresh` when you want every name to start from zero instead.

## Download one large artifact through concurrent ranges

```sh
nxr get "$NEXUS/repository/releases/1.4.0/app.zip" -o app.zip --segments 8
```

Each span is a closed Range request, and a finished span survives a killed run, so a rerun fetches only the unfinished spans.
The worker budget still caps the total concurrency, and a server that ignores Range degrades to the single stream.
The same flag works on a directory fetch:

```sh
nxr down "$NEXUS/repository/releases/1.4.0/" dist/ --names-from names.txt --segments 8
```

## Verify the local copy without the network

```sh
nxr verify dist/1.4.0/
```

Exit 0 means bytes, markers and digests agree.
Exit 1 names the incomplete or diverging entries, and the `summary` event lists them under `failed`.

## Clone a repository for backup

```sh
nxr clone "$NEXUS/repository/releases/" backup/releases/
```

The clone enumerates the repository through the search API, verifies every artifact against its remote marker, and writes a local marker for an object whose bytes already match.
A rerun is the update: unchanged files skip without a hash, and the optional cache lives at `backup/releases/.nxr-state.json`.
An object that diverges from its marker is refused rather than overwritten.
The clone never deletes a local name the repository no longer lists.

## Compare before publishing

```sh
nxr diff dist/1.4.0/ "$NEXUS/repository/releases/1.4.0/" --names-from names.txt
```

`diff` reports one line per entry: `same`, or a divergence by size, by sha, or `unverifiable` when neither side carries a digest and the sizes match.
It reads and never writes.

## Mirror a version between two repositories

```sh
nxr mirror "$NEXUS/repository/raw-main/1.4.0/" "$NEXUS/repository/releases/1.4.0/" --names-from names.txt
```

Enumerate from the source with `--ls` instead of a file when the search index is warm.
A destination that is complete with a different digest is refused rather than overwritten, and the pour finishes with `uploaded N, downloaded N`.

## Move a version

```sh
nxr mv "$NEXUS/repository/raw-main/1.4.0/" "$NEXUS/repository/releases/1.4.0/" --names-from names.txt
```

`mv` mirrors the names into the destination and only then deletes the same names at the source.
Nothing is deleted until the pour converged, so a failed move leaves a duplicate rather than a loss.

## Promote a channel, forward only

```sh
nxr channel set "$NEXUS/repository/releases/latest" 1.4.0 --if-forward
nxr channel get "$NEXUS/repository/releases/latest"
```

`--if-forward` keeps the current token when it already compares greater than or equal to the new one, so a retried older pipeline cannot roll a channel back.
An unset channel reads back as `unset`.

## Delete a version safely

```sh
nxr rm "$NEXUS/repository/releases/1.0.0/" --names-from names.txt
```

Without `--yes` this prints the plan and touches nothing.
Then run it again with `--yes` to delete for real.
The marker of each name goes first, and a name whose DELETE answers 404 is reported as `missing` rather than as a failure.

## Audit a repository

```sh
nxr audit "$NEXUS/repository/releases/"
```

The default structural pass reads metadata only: the raw tree, one small GET per marker, and the search facts as an independent cross-check.
It classifies `orphan` (bytes with no marker), `dangling_marker` (a marker with no bytes), `unparseable_marker` and `diverged_fact`.
Add `--bytes` to download and hash every object, which is the only pass that catches silent bitrot, and narrow it with `--sample N` for a deterministic sample whose seed prints in the summary.
`--sample` without `--bytes` is misuse and exits 2.

## Gate a pipeline on a channel

```sh
token="$(nxr channel get "$NEXUS/repository/releases/latest")"
if [ "$token" = "unset" ]; then
  echo "no release published yet" >&2
  exit 1
fi
nxr down "$NEXUS/repository/releases/$token/" dist/ --names-from names.txt
```

`channel get` prints the token itself, so it composes in a command substitution.

## Diagnose a failing pipeline

```sh
nxr doctor "$NEXUS"
```

It validates credentials, TLS mode, workers and timeouts locally, then probes one HEAD request for reachability.
A rejected probe (401 or 403) exits 3 and hints at the credentials, and a genuinely unreachable host hints at the network instead.

## Point a command at a named remote

```sh
nxr -R releases ls 1.4.0/
```

`-R <alias>` names a remote in `~/.config/nxr/config.toml`, expands URL arguments that look relative, and supplies the credentials.
The file is read only when the flag names an alias, and a `-u` on the same call wins over the alias credentials.

## Read the summary in a script

```sh
nxr up dist/1.4.0/ "$NEXUS/repository/releases/1.4.0/" --json | tail -1
```

The last line of a transfer is the `summary` event, so `tail -1` gets it without parsing the stream.
Branch on the `failed` array inside that event, or on the exit code, whichever the pipeline already uses.
`failed` carries the human strings, while `failures` carries `{code, name}` in the same order, so a mixed run can be split by class:

```sh
nxr up dist/1.4.0/ "$NEXUS/repository/releases/1.4.0/" --json \
  | jq -r 'select(.event=="summary") | .failures[] | "\(.code) \(.name)"'
```

Codes: `1` data, `2` misuse, `3` transport.
