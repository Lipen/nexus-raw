# nxr-tui

A terminal browser over a Nexus raw repository, built on the `nexus-raw-core` facade.
A raw repository is an arbitrary file tree, and the TUI treats it as one: repositories first, then the tree of the selected repository at full depth.

## Run

From this directory:

```text
cargo run --release -- http://127.0.0.1:8099/
```

The arguments are one or more server root URLs.
Credentials are optional and curl style: `-u user:pass`, or the `NXR_AUTH` / `NXR_USERNAME` + `NXR_PASSWORD` environment variables.

A quick way to get a server is the repository mock.
Build it once from the repository root, then start it:

```text
cargo build -p mock-nexus -p nexus-raw
target/debug/mock-nexus atomic --port 8099
```

The mock starts with an empty raw-main repository.
Seed it with the `nxr` CLI, for example:

```text
echo hello > /tmp/notes.txt
target/debug/nxr put http://127.0.0.1:8099/repository/raw-main/app/notes.txt -f /tmp/notes.txt --sha
```

The listings go through the Nexus search API (`/service/rest/v1/search/assets`).
A real Nexus provides it.
The mock does not out of the box, so also PUT a static search page listing the repo-relative object paths:

```text
printf '{"continuationToken":null,"items":[{"path":"app/notes.txt"}]}' > /tmp/search.json
target/debug/nxr put http://127.0.0.1:8099/service/rest/v1/search/assets -f /tmp/search.json
```

## The fail-fast contract

The bootstrap validates everything before the terminal switches to the alternate screen.
Every base URL must resolve, `service_repos()` must answer on every server, and the repository list is the first screen, already populated.
Any failure prints `error:` and the matching `hint:` to stderr and exits with `Error::exit_code()`: 0 ok, 1 data, 2 misuse, 3 transport.
The TUI never opens on unknown ground, so there is no error screen and no dead alternate screen.
The same bootstrap runs in `--smoke` mode, which is how CI exercises it.

## Screens

Screen 1 lists the repositories of every configured server, with format and kind, from `service_repos()`.
Screen 2 is the tree of the selected repository, listed through `ls_entries()` at whatever depth you descend to.
Entries render straight from the listing: `name/` for folders and `name` for files.
Folders sort first, files after, and the derived `.sha256` siblings stay hidden.
The header carries the breadcrumb: the repository name plus every folder below it, for example `raw-main / app / core`.

## Repository filter

Only `format == "raw"` repositories are enterable.
Repositories of other formats stay visible, dimmed, with the format shown as a badge such as `[maven2]`, and enter only prints a status hint.
`--all-formats` lifts the filter and lets every format be opened.

## Keys

- `up`/`down` or `k`/`j` move the selection.
- `enter` opens a folder and descends, or on a file entry downloads that file.
- `d` downloads the selected entry: a folder downloads its whole subtree, a file downloads itself.
- `esc` or `backspace` climbs one level, from the repo root back to the repositories.
- `q` or `ctrl-c` quits.

The tree has no depth limit.
Every descent lists the child directory through `ls_entries()`, and the breadcrumb tracks the path from the repo root.

## Download semantics

The download always runs as one `down` call with `Enumeration::Names`.
For a file entry the plan is that one name.
For a folder entry the TUI walks the subtree with `ls_entries()` recursively and maps every file to a name relative to the repo root, then downloads the whole set in one transfer.
The plan count is shown before the transfer starts, in the status line and in the download panel.
Files land under `./nxr-tui-downloads/<repo>/`, mirroring their repo-relative paths.
A panel shows the plan, the files in flight with byte progress, and the final summary.
Errors never close the TUI: a refused listing or download only paints the status bar, with the message plus its `Error::hint()` text.

## Headless smoke mode

`--smoke` runs the browse-and-download flow without the terminal and prints what it sees:

```text
nxr-tui --smoke http://127.0.0.1:8099/
```

It lists the repositories of every server, walks the tree of the first hosted raw repository two levels deep, downloads the subtree of the first root folder, and prints the summary.
The exit code follows `Error::exit_code()`: 0 ok, 1 data, 2 misuse, 3 transport.

## Tests

```text
cargo test
```

The suite covers argument parsing, the raw filter, descend and ascend navigation, stale-listing guards, the fail-fast bootstrap against a live mock and against dead and malformed URLs, the recursive subtree walk, and end-to-end binary runs: a dead URL must exit non-zero with `error:` and `hint:` before the alternate screen ever opens, and `--smoke` must walk a seeded three-level tree and land the files on disk.

## What works

- Multi-server repository listing with format and kind, loaded before the TUI opens.
- Full-depth tree navigation with breadcrumbs and backspace.
- Whole-subtree and single-file downloads through one `down` call, with byte progress, retry notes and a completion summary.
- Auth via `-u` or the environment, applied to every request.
- Headless smoke mode for CI.

## Limitations

- The download target directory is fixed to `./nxr-tui-downloads/`.
- There is no `--fresh` flag; reruns skip files already complete.
- A group repository passes the raw filter, but the search API it needs may refuse on the server, which the status bar shows as an error with its hint.
- The search page cap of the core (100 pages) bounds every listing.
- A repository with more files than that needs the manifest enumeration path instead.
- No mouse support.
- Navigation is keyboard only.
- TLS verification is always on.
- There is no `--insecure` flag.
