# Named remotes (aliases)

The config file gives a short name to a long URL and its credentials.
The file is read **only** when the invocation names an alias with `-R/--remote`: without the flag nxr never touches it, and the call keeps the curl-model contract (URL in argv, credentials from env).

## The file

`$XDG_CONFIG_HOME/nxr/config.toml`, then `~/.config/nxr/config.toml`.
`NXR_CONFIG` names the file explicitly (a CI layout, a second server set).

```toml
# ~/.config/nxr/config.toml
[alias.corp]
url = "https://nexus.example.com:10443"
user_env = "NEXUS_USERNAME"        # the env variable carrying the user
pass_env = "NEXUS_PASSWORD"        # ...and the password

[alias.releases]
url = "https://nexus.example.com:10443/repository/releases-raw"
user = "deploy"                    # inline credentials work too...
pass = "..."                       # ...but the file then carries a secret

[alias.prod]
url = "https://prod.example.com"
user = "ci"
pass_cmd = "op read 'op://vault/nexus/password'"   # a secret manager instead

[alias.anon]
url = "https://public.example.com/repository/mirror"
# no credential source: the alias runs anonymously
```

## The calls

```console
# The alias expands relative paths onto its URL:
$ nxr -R releases ls 1.4.0/
$ nxr -R releases up ./dist 1.4.0/
$ nxr -R releases get 1.4.0/app.zip -o app.zip

# The alias may point at a repository root; `.` stays the root itself:
$ nxr -R releases service status .
$ nxr -R corp service repo /repository/releases-raw

# Absolute URLs pass through unchanged, the alias only carries credentials:
$ nxr -R releases get https://other.example.com/repository/x/1.0/file -o file
```

An alias may name a version directory, a repository root or a server root: the alias URL is exactly the base every relative path joins onto.

## Priority

1. `-u user:pass` wins over everything.
2. The alias credentials (resolved at call time: env variables read, `pass_cmd` run).
3. Ambient env (`NXR_AUTH`, `NXR_USERNAME`+`NXR_PASSWORD`) — but only when neither `-u` nor the alias carry credentials.

`NXR_CONFIG=/path/to/file` moves the file. The priority stays.

## What breaks loudly

- An unknown alias: misuse (exit 2) listing the known names.
- A missing env variable or a failing `pass_cmd`: misuse (exit 2) naming the source.
- A half credential source (a user without a password): misuse (exit 2).
- A broken or missing file: misuse (exit 2) — and it is only read when `-R` named an alias.

Anonymous through a bare alias is legal: a real Nexus answers 401 with the usual hint when the credentials were required.

## Shell completion

`nxr complete` scripts know `-R`. The alias names come from your file at edit time, not from the completion.
