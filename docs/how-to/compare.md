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
nxr diff ./dist/1.4.0 https://prod.example.com/repository/raw-main/1.4.0/
```

An empty report: the local tree is exactly what the server serves.
