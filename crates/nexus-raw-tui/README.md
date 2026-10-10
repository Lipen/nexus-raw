# nxr-tui

A terminal browser over Nexus raw repositories, built on the [`nexus-raw-core`](../../crates/nexus-raw-core) facade.
One tab per server, the repository list of the server, then the tree of the selected repository at any depth, with subtree downloads, a live filter and a servers overlay that can add a server and save it as a preset.

```bash
nxr-tui http://127.0.0.1:8081/          # one server from the command line
nxr-tui                                 # every preset of the config file
nxr-tui --server main --server backup   # two named presets as two tabs
```

A quick way to get a server is the repository mock:

```bash
just mock &          # the mock server: cargo run -p mock-nexus -- atomic --port 8081
just tui http://127.0.0.1:8081/
```

## The fail-fast contract

The bootstrap validates everything before the terminal switches to the alternate screen.
Every server must answer `service_repos`, and the repository lists are the first screen, already populated.
Any failure prints `error:` and the matching `hint:` to stderr and exits with `Error::exit_code()`: 0 ok, 1 data, 2 misuse, 3 transport.
The TUI never opens on unknown ground, so there is no error screen and no dead alternate screen.
The same bootstrap runs in `--smoke` mode, which is how CI exercises it.

## Keys

| Key | Does |
|:----|:-----|
| `up`/`down`, `k`/`j` | move the selection |
| `pgup`/`pgdown` | move by page |
| `home`/`end`, `g`/`G` | first/last row |
| `right`, `enter` | open a folder or repository, download a file |
| `left`, `esc`/`backspace` | up one level, then back to the repositories |
| `e` | toggle the tree navigation mode (see below) |
| `d` | download the selected entry: a folder downloads its subtree |
| `D` | download the whole current directory |
| `r` | refresh the current listing |
| `i` | info on the selected entry (HEAD size for files) |
| `/` | filter the tree: type to narrow, enter keeps, esc clears |
| `tab`/`backtab`, `1`-`9` | switch server tabs |
| `s` | servers overlay: switch or add, `a` opens the add form |
| `?` | keybindings overlay |
| `q`/`ctrl-c` | quit. twice while a download runs |
| mouse | wheel scrolls, click selects, double click opens |

## Tree modes

`e` toggles how the left/right arrows navigate, and the status line shows the mode (`nav enter` / `nav expand`). The choice is saved to the config file.

- `nav enter` (default): `right` enters a folder, `left` goes back up.
- `nav expand`: `right` expands a folder inline (`▸`/`▾` markers, indented children, loading folders show `…`), `left` collapses it or jumps to the parent row. `enter` keeps entering folders, and the expansion folds on descend, on refresh and on the mode switch.

The mode is the config key `tui.nav = "enter" | "expand"`.

## Config

The config file is `$XDG_CONFIG_HOME/nxr-tui/config.toml`, falling back to `$HOME/.config/nxr-tui/config.toml`. `--config` overrides both.
`nxr-tui --init-config` writes a commented template and refuses to overwrite.

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
The `s` overlay inside the TUI adds a server and saves it as a preset under a fresh unique name.
Passwords never live in the config: credentials come from `-u` or the environment (`NXR_AUTH`, or `NXR_USERNAME` + `NXR_PASSWORD`) at every launch, one set for all servers of the session, exactly like the `nxr` CLI.

## Downloads

A download always runs as one `down` call with `Enumeration::Names`, from the repository root into `<download.dir>/<repo>/`, mirroring repo-relative paths.
Downloads are one at a time: while the panel runs, `d` and `D` answer `download in flight: one at a time`.
Everything else keeps working: browsing, filters, refreshes, info, tab switches, even adding a server.
Navigation within the tab cancels a still-walking request. A running transfer is never interrupted by the UI.
Files land complete or not at all: the sha sibling is verified, a diverging complete object is never overwritten, reruns skip what already landed.
A panel shows the plan, the files in flight with byte progress and the final summary.
Errors never close the TUI: a refused listing or download only paints the status bar, with the message plus its `Error::hint()` text.

## Headless smoke mode

`--smoke` runs the browse-and-download flow without the terminal and prints what it sees: the repository list of every server, a two-level tree walk of the first hosted raw repository, then a subtree download with the summary.
The exit code follows `Error::exit_code()`.

## Layout

| Path | Role |
|:-----|:-----|
| `src/lib.rs` | the crate docs and the re-exports |
| `src/args.rs` | clap arguments and server resolution (URLs, presets, config) |
| `src/config.rs` | the config file: load, save, template, XDG path |
| `src/app.rs` | the state machine: tabs, screens, overlays, message handlers |
| `src/net.rs` | async operations over the `Nxr` facade |
| `src/ui.rs` | rendering: tab bar, lists, download panel, status, overlays |
| `src/tui.rs` | the terminal loop: bootstrap, raw mode, input thread, pump |
| `src/smoke.rs` | `--smoke`: the same flow headless |

## Tests

```bash
cargo test -p nexus-raw-tui
```

The suite covers argument resolution, the config roundtrip, navigation including the arrow keys, the filter, per-tab generations and stale-result drops, the add-server flow with config persistence, the quit guard, mouse wheel and click, the fail-fast bootstrap against a live mock and against dead and malformed URLs, the recursive subtree walk, and end-to-end binary runs including a config-preset smoke.
