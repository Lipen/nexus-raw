# Install with Homebrew

The formula lives in this repository, which doubles as a tap.

```bash
brew tap lipen/nexus-raw https://github.com/Lipen/nexus-raw
brew install lipen/nexus-raw/nexus-raw
```

The same `brew` works on Linux (Homebrew on Linux).
The formula selects the static musl archive there, so it does not depend on the host glibc.

Upgrade and remove:

```bash
brew upgrade nexus-raw
brew uninstall nexus-raw
```

The formula installs the `nxr` binary, the README and the LICENSE.
The agent skill is not part of the Homebrew install.
Take it from the repository with `scripts/install-skill.sh`.

## Regenerate

The formula pins the version and one `sha256` per platform.
After a release, regenerate it from that release's `SHA256SUMS`:

```bash
just brew-formula
just brew-formula 0.22.1
```

The formula follows the published releases, not the workspace manifest: a version bump on master is ahead of it until the release lands.
`scripts/check-brew-formula.sh` catches a forgotten or half-done bump:

```bash
just brew-check
```

It re-renders the latest release's formula from its `SHA256SUMS` and diffs it against the committed file, so a wrong version and a hand-edited checksum both fail.
The daily smoke workflow and CI on master run it, which makes a formula left behind red within a day.
