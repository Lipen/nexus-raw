# Compare two servers

Staging drifted from production, or two Nexus instances should serve the same version.
The compare is `mirror --dry-run`: the plan it prints is exactly the delta `mirror`
would transfer, and nothing is written.

## Is the content identical?

```bash
nxr mirror https://staging.example.com/repository/raw-main/1.4.0/ \
           https://prod.example.com/repository/raw-main/1.4.0/ --ls --dry-run
```

An empty plan: the two directories serve identical name-and-digest sets.
A non-empty plan: every line names the artifact and the direction of the difference.
The digest comparison is content-addressed: two servers agreeing on a sha256 agree on bytes, whatever wrote them.

The enumeration source is required, as everywhere in nxr:
`--ls` walks the search API (the normal case for a real Nexus),
`--name`/`--names-from` carry an explicit list when the search is dead or the compare is narrow:

```bash
printf '%s\n' app.jar deps.jar | nxr mirror SRC/1.4.0/ DST/1.4.0/ --names-from - --dry-run
```

Credentials per side, when the two servers authenticate differently:

```bash
nxr mirror SRC DST --ls --src-user staging-reader:pw --dst-user prod-reader:pw --dry-run
```

Compare reads only: `--dry-run` enumerates and digests, it never PUTs.

## Then converge

Drop `--dry-run` and the same command transfers the difference: resumable, digest-verified, markers written after bytes.
The rerun prints an empty plan: convergence is provable, not assumed.

## Local versus remote

A local checkout against a server is `diff`: the same name-and-digest report, one side on disk.

```bash
nxr diff ./dist/1.4.0 https://prod.example.com/repository/raw-main/1.4.0/ --ls
```

An empty report: the local tree is exactly what the server serves.
`diff` needs an enumeration source like the transfers: `--ls` for the search API, or `--name`/`--names-from` for an explicit list.

## Compare without probing every object

`diff --via search --ls` takes the remote side from the search API's per-asset facts (name, digest, size), the same listing `nxr service assets` reads.
One paged listing replaces the HEAD-and-sibling pair per name, so a big tree compares in O(pages) requests.

```bash
nxr diff ./dist/1.4.0 https://nexus.example.com/repository/raw-main/1.4.0/ --via search --ls
```

`--prefix` filters the search facts client-side.
`--name`/`--names-from` are refused with `--via search` because the listing is the name source.
With `--fallback` they become the degraded source: a server without the search API announces the refusal, then the report is rebuilt from the probe-based enumeration.

A searched asset that reports no digest counts as `diverged`, never as `same`.
A search refusal is `cannot enumerate`, exit 1, never a false clean delta.
The full rules live in the [CLI reference](../reference/cli.md).
