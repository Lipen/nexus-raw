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

`--check` compares the committed formula against the release and exits 1 on drift:

```bash
scripts/brew-formula.sh --check
```

`scripts/version-check.sh` holds the formula's version line to the workspace.
