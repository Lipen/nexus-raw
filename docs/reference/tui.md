# TUI reference

`nxr-tui` is a terminal browser over Nexus raw repositories.
It lists the repositories of a server, opens any raw repository as a tree at full depth, downloads entries and subtrees, and keeps one tab per server.
The same core drives it as the `nxr` CLI: the listings go through the search API, the downloads through the verified `down` pipeline.

Every invocation is self-sufficient in the `nxr` sense: credentials come from `-u` or the environment, never from the config file.
What the config file adds is presets: named servers the TUI can open directly, and download defaults.

The bootstrap validates everything before the terminal switches to the alternate screen.
Every configured server must answer `service_repos`, and the repository lists are the first screen, already populated.
A failure prints `error:` plus its `hint:` and exits with the core exit code: 0 ok, 1 data, 2 misuse, 3 transport.
The TUI never opens on unknown ground.

## Servers, tabs and presets

Three ways to say which servers open:

| Invocation | Opens |
|:-----------|:------|
| `nxr-tui http://host:8081/` | that server, the tab named after the host |
| `nxr-tui --server main --server backup` | the named presets of the config, one tab each |
| `nxr-tui` | every preset of the config file |

Positional URLs and `--server` presets compose: both open, in that order.
An unknown preset name is a usage error (exit 2) that names the known presets.

Inside the TUI, `s` opens the servers overlay: every server the session knows, with the open ones marked.
`enter` focuses the tab of the selected server or connects a new one in the background; `a` opens the add form.
A successfully added server joins the config as a preset under a fresh unique name (`host`, `host-2`, ...), so the next `nxr-tui` without arguments opens it too.
`tab`/`backtab` and `1`-`9` switch tabs; every tab keeps its own screen, cursor and filter.

The overlay works against a refusing server the same way the CLI does: the error lands in the status bar with its hint, the form keeps the typed URL, nothing closes.

## Keys

| Key | Does |
|:----|:-----|
| `up`/`down`, `k`/`j` | move the selection |
| `pgup`/`pgdown` | move by page |
| `home`/`end`, `g`/`G` | first/last row |
| `right`, `enter` | open a folder or repository; on a file, open the file card |
| `left`, `esc`/`backspace` | up one level, then back to the repositories |
| `e` | toggle the tree navigation mode (see below) |
| `d` | download the selected entry into the destination folder |
| `D` | download with a dialog: folder and name, prefilled from the selection |
| `p` | upload a local folder into the current tree position (source picker) |
| `c` | copy the URL of the selection: the repository, the file or the folder |
| `x` | cancel the running transfer |
| `v` | toggle the local pane; `h`/`l` move the focus to it and back |
| `space` | mark the selection; `d` transfers the marks as a queue |
| `up`/`down` | in the pickers: walk the confirmed values of the session |
| `o` | pick the destination folder of the session |
| `i` | card of the selected repository (repositories screen) |
| `r` | refresh the current listing, keeping the filter and the cursor |
| `/` | filter the tree: type to narrow, enter applies, esc closes; backspace to empty closes too |
| `tab`/`backtab`, `1`-`9` | switch server tabs |
| `s` | servers overlay, `a` opens the add form |
| `?` | keybindings overlay |
| `q`/`ctrl-c` | quit; twice while a transfer runs; inside an overlay or a modal, `q` closes it |
| mouse | wheel scrolls, click selects, double click opens a folder or the file card |

## The file card

`enter` on a file opens a card, never a download.
The card shows the name, the path from the repository root, the full URL, the size from a HEAD request and the sha256 from the `.sha256` marker when the server has one.
Inside the card: `s` downloads the file into the destination folder, `o` picks another destination and returns to the card, `c` copies the URL, `esc`/`enter`/`q` close.
The download is a direct `get` with the digest verified against the marker: no enumeration is involved.

## The destination folder

Downloads always land in an explicitly chosen folder, shown as `→ path` in the status bar.
The session starts with `download.dir` from the config.
`o` opens a picker prefilled with the current destination: edit and press enter, or esc to cancel.
A file lands as `{destination}/{name}`, a subtree as `{destination}/{name}/...`, a whole repository as `{destination}/{repo}/...`.
Nothing is written outside the destination, and nothing is written before a dialog is confirmed.

## Put

`p` on the tree uploads a local folder into the current position: the breadcrumb of the tree is the remote target, and to fill a nested folder you descend into it first.
On the repositories screen `p` only reports: `the put works in the tree`.

The picker is prefilled with the last confirmed source of the session, with the working directory of `nxr-tui` on the first put.
It shows the resolved path live, paste works, and nothing leaves the machine before enter.
Both pickers keep the values confirmed this session (sixteen at most, separately per picker): `up` and `down` walk them, `enter` confirms the one on screen, and typing returns to the draft.
Enter resolves the buffer against the working directory and checks it locally: a path that is not a folder answers `not a directory: {path}` and keeps the picker open, an empty buffer answers `the source is empty` the same way.
A confirmed folder starts the upload immediately: `put {folder} -> {position}`.
With the local pane open, `p` prefills the picker with the folder of the pane.

The upload drives the same core pipeline as `nxr up`: a local scan (hidden files, `.sha256` markers and `.part` files stay out, one unsafe name refuses the whole run before the first byte), marker generation for markerless files, the symmetric diff against the remote, then the transfer.
Markers are always generated: the TUI has no `--no-sha`.
Same-digest objects are skipped, a divergence refuses the whole run, and an upload never deletes anything on the server.

While the upload runs, the panel shows the remote base, the source folder, the plan, the files in flight with byte progress and the summary.
Navigation, filters, cards and tab switches keep working: the position the put goes into is fixed at the start, browsing never retargets it.
A finished put quietly refreshes the tree position it went into, so the new names appear without a manual `r`.
An empty source opens the error modal without a retry.
`r` offers a rerun of the same put into the same base for every refusal the core allows to retry, which excludes grammar refusals of unsafe names.

## Dual-pane

`v` splits the body: the left half shows a folder of the local filesystem, the right half keeps the repositories or the tree unchanged.
The gate is a terminal of eighty columns; a narrower one answers `the terminal is too narrow for dual-pane` and nothing opens.
A second `v` closes the pane, and the anchors return to the session destination.

`h` moves the keyboard focus into the pane, `l` back to the remote side.
Inside the pane the usual keys move the cursor: `enter` descends into a folder, `esc`/`backspace` climb one folder up (at the filesystem root it says so), `enter` on a file answers `enter opens folders here` because a local card does not exist in this wave.
The remote verbs refuse in the pane: `d` and `p` answer `switch to the remote pane (l)`, and the filter answers `the filter works in the tree`.
The pane is a session surface: tab switches never close or reset it.

While the pane is open it anchors the transfers: the status bar shows its folder instead of the destination, a file lands as `{pane}/{name}`, a subtree as `{pane}/{rel}/...`, a whole repository as `{pane}/{repo}/...`.
The session destination itself is never rewritten by a pane-anchored transfer.
`p` prefills the source picker with the pane folder.
A download that landed in the pane folder makes it reread itself, so the new files show up on their own.
A click selects in the pane whose half it hit and moves the focus there.

## Marks

`space` marks the selected row: a repository on the repositories screen, an entry on the tree.
Marked rows render a `*` before the name, and the status line confirms every toggle.
`d` over marks does not take the cursor row: the marked transfers line up for the single slot and run one by one, the panel always shows the one in flight.
A file lands flat by its name under the anchor, a folder mirrors from the repository root, a repository mirrors under its own name.
The first refusal ends the queue, and so does `x`.
The marks are names, not row indices: a refresh keeps them, while any navigation away from the listing (deeper, up, opening a repository) clears them.
`p` never reads the marks.

## Cancel

`x` aborts the running transfer, whichever direction.
The panel ends with `cancelled` and the status line says so; the stragglers of the aborted task are ignored, no modal opens.
There is no retry behind a cancel: starting again is a normal `d` or `p`.
An upload cancelled in the middle leaves markerless objects behind, exactly like any interrupted `nxr up`: the next put completes them.

## Errors

A failed listing or download opens an error modal instead of the status line.
It shows a plain-language cause, the facts (server, repository, names, destination, HTTP status), the hint from the core error, and the full error text.
`y` copies the full text to the clipboard (OSC52 with a fallback to the system clipboard, `tui.osc52` disables OSC52), `r` retries where a retry makes sense, `esc`/`enter` closes.
The modal never destroys what is under it: a destination picker, the source picker, a dialog, the servers overlay, an add form or a card comes back exactly as it was when the modal closes, typed text included.
A retry hands the layer straight back too: the rerun proceeds behind the dialog.
An error over an error passes the covered layer on, so a second refusal never buries the first dialog.

## Tree modes

`e` toggles how the left/right arrows navigate, and the status line shows the mode (`nav enter` / `nav expand`); the choice is saved to the config file.

- `nav enter` (default): `right` enters a folder, `left` goes back up.
- `nav expand`: `right` expands a folder inline (`▸`/`▾` markers, indented children, loading folders show `…`), `left` collapses it or jumps to the parent row. `enter` keeps entering folders, and the expansion folds on descend, on refresh and on the mode switch.

Only `format == "raw"` repositories are enterable.
Repositories of other formats stay visible, dimmed, with the format as a badge such as `[maven2]`, and `enter` only prints a status hint.
`--all-formats` lifts the filter.
The derived `.sha256` siblings stay hidden, exactly as in `nxr ls`.

## Config

The config file is `$XDG_CONFIG_HOME/nxr-tui/config.toml`, falling back to `$HOME/.config/nxr-tui/config.toml`; `--config` overrides both.
A missing file is the default config, so the TUI works with nothing on disk.
`nxr-tui --init-config` writes a commented template and refuses to overwrite an existing one.

```toml
version = 1

[download]
# Where downloads land, relative to the working directory of nxr-tui.
dir = "nxr-tui-downloads"

[tui]
# Let repositories of every format be opened, not only raw.
all_formats = false
# Left/right navigation in the tree: "enter" or "expand" (toggled with `e`).
nav = "enter"
# Copy through the terminal (OSC52) in addition to the system clipboard.
osc52 = true

[[server]]
name = "main"
url = "http://127.0.0.1:8081/"
```

| Key | Default | Meaning |
|:----|:--------|:--------|
| `version` | `1` | the format version, for future migrations |
| `download.dir` | `nxr-tui-downloads` | where downloads land, relative to the working directory |
| `tui.all_formats` | `false` | let repositories of every format be opened |
| `tui.nav` | `enter` | the left/right navigation mode: `enter` or `expand` |
| `tui.osc52` | `true` | copy through the terminal (OSC52) alongside the system clipboard |
| `server` (list) | none | the presets: `name` for the tab and `--server`, `url` for the server root |

Passwords never live in the config.
Credentials come from `-u user:pass`, `NXR_AUTH` (base64 `user:pass`) or `NXR_USERNAME` + `NXR_PASSWORD`, in that order, and apply to every server of the session.
Values never appear in logs or on screen.

## Downloads

A file entry downloads through the direct `get` primitive into `{destination}/{rel}`, with the `.sha256` marker verified when the server has one.
A folder entry (`d` on a folder, `D` on the current directory) and a whole repository (`d` on the repositories screen) walk the subtree with `ls_entries()` recursively and download the set in one transfer.
The `D` dialog shows the target path live before anything starts, and nothing is written until it is confirmed.

Downloads are one at a time: while the panel runs, `d` and `D` answer `transfer in flight: one at a time`.
Everything else keeps working: browsing, filters, refreshes, cards, tab switches, even adding a server.
A running transfer is never interrupted by the UI.
Files land complete or not at all: the sha sibling is verified, a diverging complete object is never overwritten, reruns skip what already landed.
A panel titled `download` or `upload` shows the plan, the files in flight with byte progress and the final summary, and stays until the next transfer replaces it.
`x` cancels the running transfer, and the panel keeps the outcome until the next one starts.
Failures open the error modal and never close the TUI.

## Headless smoke mode

`--smoke` runs the browse-and-download flow without the terminal and prints what it sees:

```console
$ nxr-tui --smoke http://127.0.0.1:8081/
server http://127.0.0.1:8081/
  raw-main     raw     hosted
tree of raw-main:
  app/
  docs/
  README.txt
level 1: app/
    core/
    readme.txt
plan: 3 files -> nxr-tui-smoke/raw-main (subtree app)
diff: 3 to download, 0 already complete
  done app/core/util/helpers.py
  done app/core/lib.rs
  done app/readme.txt
summary: downloaded 3, skipped 0, failed 0
smoke ok: 3 complete under nxr-tui-smoke/raw-main
$ echo $?
0
```

It lists the repositories of every server, walks the tree of the first hosted raw repository two levels deep, downloads the subtree of the first root folder and prints the summary.
The exit code follows `Error::exit_code()`.
CI uses the same mode: `cargo test -p nexus-raw-tui` drives the real binary against `mock-nexus`, including a run driven entirely by a config preset.

## Flags

| Flag | Meaning |
|:-----|:--------|
| `BASE_URL...` | server root URLs to open (positional, repeatable) |
| `-s, --server <NAME>` | open the named config preset (repeatable) |
| `-u, --user <USER:PASS>` | credentials, curl style; env fallback as in `nxr` |
| `--all-formats` | let repositories of every format be opened, not only raw |
| `--smoke` | the headless browse-and-download flow |
| `--config <PATH>` | alternative config file path |
| `--init-config` | write the commented template to the config path and exit |

## Limitations

- One credential set per session: `-u` or the environment applies to every server.
- The search page cap of the core (100 pages) bounds every listing; a repository with more files needs the manifest enumeration path instead, which the TUI does not expose.
- A group repository passes the raw filter, but the search API it needs may refuse on the server, which the status bar shows as an error with its hint.
- TLS verification is always on; there is no `--insecure` flag.
- The tree listing is best-effort: the search API must exist on the server.

Embedding the same operations without a terminal: [the API](api.md), [the CLI](cli.md).
