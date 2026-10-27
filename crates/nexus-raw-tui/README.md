# nxr-tui

A terminal browser over Nexus raw repositories.
It shows one tab per server, the repository tree at any depth, subtree downloads, a live filter, and saved server presets.
It is built on the [`nexus-raw-core`](https://crates.io/crates/nexus-raw-core) facade, so every download follows the same rules as `nxr down`: verified, resumable, and never overwriting a divergent object.

## Install and run

```bash
cargo install nexus-raw-tui

nxr-tui http://127.0.0.1:8081/          # one server from the command line
nxr-tui                                 # every preset of the config file
nxr-tui --server main --server backup   # two named presets as two tabs
```

A temporary server for a trial run comes from the repository mock:

```bash
just mock &          # or: cargo run -p mock-nexus -- atomic --port 8081
just tui http://127.0.0.1:8081/
```

Credentials follow the same rules as `nxr` and are resolved once per session for all servers: `-u user:pass`, `NXR_AUTH`, or `NXR_USERNAME` + `NXR_PASSWORD`.
Passwords are never stored in the config file.
For TLS, `--ca-bundle FILE` (or `NXR_CA_BUNDLE`) adds a private CA and `--tls-insecure` skips verification. The two options exclude each other.

## Keys

| Key | Does |
|:----|:-----|
| `up`/`down`, `k`/`j` | move the selection |
| `pgup`/`pgdown` | move by page |
| `home`/`end`, `g`/`G` | first/last row |
| `right`, `enter` | open a folder or repository, download a file |
| `left`, `esc`/`backspace` | up one level, then back to the repositories |
| `e` | toggle the tree navigation mode |
| `d` | download the selected entry, a folder downloads its subtree |
| `D` | download the whole current directory |
| `r` | refresh the current listing |
| `i` | info on the selected entry (HEAD size for files) |
| `/` | filter the tree: type to narrow, enter keeps, esc clears |
| `tab`/`backtab`, `1`-`9` | switch server tabs |
| `s` | servers overlay: switch or add, `a` opens the add form |
| `?` | keybindings overlay |
| `q`/`ctrl-c` | quit, twice while a download runs |
| mouse | wheel scrolls, click selects, double click opens |

`e` toggles how the arrows navigate: `nav enter` (the default, `right` enters a folder) or `nav expand` (`right` expands a folder inline, `left` collapses it).
The status line shows the mode, and the choice is saved to the config as `tui.nav`.

## Config

The file is `$XDG_CONFIG_HOME/nxr-tui/config.toml`, falling back to `$HOME/.config/nxr-tui/config.toml`.
`--config` overrides both, and `nxr-tui --init-config` writes a commented template without overwriting anything.

```toml
version = 1

[download]
dir = "nxr-tui-downloads"

[tui]
all_formats = false
nav = "enter"

[[server]]
name = "main"
url = "http://127.0.0.1:8081/"
```

Every `[[server]]` is a preset: it opens with `--server <name>`, and all of them open when `nxr-tui` runs with no arguments.
The `s` overlay adds a server and saves it as a fresh preset.
`--no-color` (or a non-empty `NO_COLOR`) drops the error and toast colours.

## Downloads

A download is one `down` call with explicit names, from the repository root into `<download.dir>/<repo>/`, mirroring repo-relative paths.
One at a time: while the panel runs, `d` and `D` answer `download in flight: one at a time`, and everything else keeps working.
A file is either complete or absent, a rerun skips what is already present, and an error only updates the status bar instead of closing the TUI.
A panel shows the plan, the files in flight with byte progress, and the final summary.

## Fail-fast bootstrap

Before the terminal switches to the alternate screen, every server must answer `service_repos`.
Any failure prints `error:` and the matching `hint:` to stderr and exits with the core exit code: `0` ok, `1` data, `2` misuse, `3` transport.
The TUI starts only after this check succeeds, so it has no error screen.

`--smoke` runs the same bootstrap and a browse-and-download flow headless: the repository list of every server, a two-level tree walk, then a subtree download.
CI exercises the binary that way.

<details>
<summary>For contributors: layout and tests</summary>

| Path | Role |
|:-----|:------|
| `src/lib.rs` | the crate docs and the re-exports |
| `src/args.rs` | clap arguments and server resolution (URLs, presets, config) |
| `src/config.rs` | the config file: load, save, template, XDG path |
| `src/app.rs` | the state machine: tabs, screens, overlays, message handlers |
| `src/net.rs` | async operations over the `Nxr` facade |
| `src/ui.rs` | rendering: tab bar, lists, download panel, status, overlays |
| `src/tui.rs` | the terminal loop: bootstrap, raw mode, input thread, pump |
| `src/smoke.rs` | `--smoke`: the same flow headless |

```bash
cargo test -p nexus-raw-tui
```

The suite covers argument resolution, the config roundtrip, navigation including the arrow keys, the filter, per-tab generations and stale-result drops, the add-server flow with config persistence, the quit guard, mouse wheel and click, the fail-fast bootstrap against a live mock and against dead and malformed URLs, the recursive subtree walk, and end-to-end binary runs including a config-preset smoke.

</details>

## More

- Keys, config, presets and the smoke mode in full: [the TUI reference](https://lipen.github.io/nexus-raw/reference/tui/).
- The library underneath: [nexus-raw-core](https://crates.io/crates/nexus-raw-core).
- The CLI for scripts: [nexus-raw](https://crates.io/crates/nexus-raw).
- License: MIT.
