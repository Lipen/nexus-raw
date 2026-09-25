# Configure profiles

One config file, named profiles, credentials from the environment — and `--base` for everything else.

## The config file

`~/.config/nxr/config.toml`, or wherever `$NXR_CONFIG` points:

```toml
--8<-- "docs/snippets/config.toml"
```

!!! note "New in this page"

    The snippet above is included verbatim from the repository's example config,
    so it cannot drift from what `nxr` actually reads.

A profile carries a `url` and an optional `tls_insecure`.
Nothing else is allowed in a profile: typos are refused, and `auth` or `password` keys are rejected outright — credentials never live on disk.

## Flags beat profiles

| Source | Precedence |
|:-------|:-----------|
| `--base <url>` | highest, skips profile URL entirely |
| `--profile <name>` | selects the profile explicitly |
| `default_profile` in the config | used when no flag is given |

`--tls-insecure` ORs with the profile's `tls_insecure`; everything else is a plain override.

## Credentials

Credentials resolve from the environment, in order, and stop at the first hit:

1. `NXR_<PROFILE>_AUTH` — base64 `user:pass`, profile uppercased, `-` becomes `_`;
2. `NXR_AUTH` — the same, for every profile;
3. `NXR_USERNAME` + `NXR_PASSWORD`.

Generate the compact form once:

```bash
printf 'ci-bot:%s' "$TOKEN" | base64
```

```bash
export NXR_MAIN_AUTH="Y2ktYm90OnRva2Vu"      # profile main
export NXR_AUTH="Y2ktYm90OnRva2Vu"           # every profile
export NXR_USERNAME=ci-bot NXR_PASSWORD="…"  # the readable form
```

Setting exactly one of `NXR_USERNAME` / `NXR_PASSWORD` is a configuration error, not a silent skip.

!!! warning "Where credentials must never appear"

    Not in argv, not in logs, not in `--json` output, not in the TOML config.
    `nxr` enforces this; keep shell history and CI logs honest by using env vars.

## Skip the config entirely

```bash
nxr --base http://127.0.0.1:8080/ up --dir dist/1.4.0
nxr --no-config --base https://nexus.example.com/repository/raw-main/ ls --version 1.4.0
```

## Where the file lives

| How | Path |
|:----|:-----|
| explicit | `--config /path/to/config.toml` (must exist) |
| environment | `$NXR_CONFIG` (must exist) |
| XDG default | `$XDG_CONFIG_HOME/nxr/config.toml`, else `~/.config/nxr/config.toml` |
| opt out | `--no-config` |
