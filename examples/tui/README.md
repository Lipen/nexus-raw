# nxr-tui

A terminal browser over a Nexus raw repository, built on the `nexus-raw-core` facade.
It drills down from repositories to versions to objects and downloads a version subtree with live progress.

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
cargo build -p mock-nexus -p nxr
target/debug/mock-nexus atomic --port 8099
```

The mock starts with an empty raw-main repository.
Seed it with the `nxr` CLI, for example:

```text
echo hello > /tmp/notes.txt
target/debug/nxr put http://127.0.0.1:8099/repository/raw-main/1.14.0/notes.txt -f /tmp/notes.txt --sha
```

The versions and objects screens use the Nexus search API (`/service/rest/v1/search/assets`).
A real Nexus provides it.
The mock does not out of the box, so also PUT a static search page listing the object paths:

```text
printf '{"items":[{"path":"1.14.0/notes.txt"},{"path":"1.14.0/notes.txt.sha256"}]}' > /tmp/search.json
target/debug/nxr put http://127.0.0.1:8099/service/rest/v1/search/assets -f /tmp/search.json
```

## Screens and keys

Screen 1 lists the repositories of every configured server, with format and kind, from `service_repos()`.
Screen 2 lists the versions of the selected raw repository through `ls_versions()`.
Screen 3 lists the objects of the selected version through `ls_assets()`.

Keys:

- `up`/`down` or `k`/`j` move the selection.
- `enter` drills down, or on the object screen downloads the whole version subtree.
- `esc` goes back one screen.
- `q` or `ctrl-c` quits.

The download lands in `./nxr-tui-downloads/<repo>/<version>/` and runs through `down` with an explicit name list taken from the object listing.
A panel shows the plan, the files in flight with byte progress, and the final summary.
The status bar shows hints and, on failure, the error message plus its `Error::hint()` text.
Errors never close the TUI: a refused listing or download only paints the status bar.

## Headless smoke mode

`--smoke` runs the same browse-and-download flow without the terminal and prints what it sees:

```text
nxr-tui --smoke http://127.0.0.1:8099/
```

It picks the first hosted raw repository, lists versions and objects, downloads the first version into `./nxr-tui-smoke/` and prints the summary.
The exit code follows `Error::exit_code()`: 0 ok, 1 data, 2 misuse, 3 transport.

## What works

- Multi-server repository listing with format and kind.
- Version and object listings through the search API, with stale-result guards when you back out mid-load.
- Full version subtree download with the sha-sibling markers, byte progress, retry notes and a completion summary.
- Auth via `-u` or the environment, applied to every request.
- Headless smoke mode for CI.

## Limitations

- The download target directory is fixed to `./nxr-tui-downloads/` and the whole version is always fetched.
- There is no single-file pick and no `--fresh` flag.
- Any raw repository can be selected, but only a hosted one holds objects.
- A group repository refuses the listings on a real server, which the status bar shows as an error with its hint.
- The search page cap of the core (100 pages) bounds the listing size.
- A repository with more objects than that needs the manifest enumeration path instead.
- No mouse support.
- Navigation is keyboard only.
- TLS verification is always on.
- There is no `--insecure` flag.
