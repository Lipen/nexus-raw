# nexus-raw panel

A demo web panel over the `nexus-raw` Node bindings.
One static page, one `node:http` server, zero npm dependencies beyond the linked addon.

## What it shows

- The repositories of a Nexus server (`serviceRepos`).
- Only raw repositories are navigable: other formats render dimmed with a badge and refuse clicks.
- A raw repository as what it is: an arbitrary file tree (`lsEntries`).
  Breadcrumbs on top, `name/` rows for folders, `name` rows for files, any depth.
  Folders sort first, then names compare with `localeCompare`.
- Downloads and deletes as server jobs with live progress bars over SSE.
- Single-file uploads with per-file progress bars, dropped on the tree or picked from the file input.
- Toasts for quick outcomes, inline errors under the form that caused them, and a flow log for job output.

## Strict startup

The server validates its servers before it binds the port.

```bash
node server.mjs --url http://127.0.0.1:8099/
# the same thing, positionally:
node server.mjs http://127.0.0.1:8099/
# several servers at once:
node server.mjs --url http://127.0.0.1:8099/ --url http://127.0.0.1:8100/
```

Every `--url` server is probed with `serviceRepos` before the port binds.
An unreachable server, a server without the management API, or a server that reports no raw repository prints the error plus a hint and exits non-zero.
There is nothing to configure in the browser that the server did not already check.

Flags:

- `--url <server>`: a server to validate and prefill, repeatable, or pass the URLs positionally.
- `--port N`: the port to bind, default 8123.
- `--lazy`: skip the startup probe, for quick experiments against a server that may not be up yet.

## Layout

- `server.mjs`: the HTTP facade.
  `GET /api/servers` lists the configured servers.
  `GET /api/repos?url=<server>` lists repositories.
  `GET /api/entries?url=<dir>` lists the immediate children of a raw directory.
  `POST /api/down` with `{"url": <dir>, "path": <subtree>, "dir": <target>}` starts a download job and answers `{"id": ...}`.
  `POST /api/rm` with `{"url": <dir>, "path": <subtree>, "file": <name?>, "dryRun": true}` answers the delete plan without deleting; the same body without `dryRun` starts a delete job.
  `POST /api/put?url=<file-url>&sha=1` takes the raw request body as the file bytes, uploads them through `nxr.put` and writes the `.sha256` marker; `sha=0` opts out of the marker.
  The put body is capped at 100 MiB per file.
  `GET /api/progress/<id>` is the SSE stream of a job.
- `index.html`: repositories on the left, the tree browser on the right, a transfer section and a progress section below.
  No external assets, no build step: icons are inline SVG, the favicon is a `data:` URL.

## Tree navigation and download semantics

The browser knows nothing about versions or objects.
Clicking a raw repository lands at the repository root, clicking a `name/` row descends, clicking a breadcrumb segment ascends.
File names are direct links to the raw bytes: `<a href="repo-url/subtree/name" target="_blank" rel="noopener">`, so the browser or a download manager fetches straight from Nexus.
The panel itself never proxies file contents.

The download always targets the directory on screen, repository root or any subtree below it.
The server walks that subtree with recursive `lsEntries` calls, collects every file name relative to the repository root, and passes the whole list to `down` as explicit `names` against the repository URL.
Nothing is enumerated through a manifest: the files the browser showed are the files that download.
The local target directory mirrors the repository layout, so a download of `app/core` lands as `<target>/app/core/...`.

## Keyboard

The shortcuts work whenever focus is not in a text field.

- `↑` / `↓`: move the active row, wrapping at the ends.
- `Enter`: descend into the active folder, open the active file in a new tab.
- `Backspace` or `Esc`: up one level.
- `Home`: back to the repository root.
- `/`: focus the server URL field.

The active row is always visible: an accent outline plus `aria-selected`, tracked for screen readers with `aria-activedescendant`.

## Uploads

The tree is a drop zone: drag files over the browser pane and an overlay names the target directory.
The file input below the tree does the same without drag and drop, `multiple` included.
Every file goes to `POST /api/put?url=<its-target-url>&sha=1` as one XHR with its own progress bar, and the listing refreshes once the batch settles.
Finished uploads collapse into a one-line tally, failures stay on their row.

## Deletes

`Delete this directory…` plans the deletion of the current subtree, the trash button on a file row plans a single-file deletion.
Both are two-step: the first call is a dry run, the dialog lists every name with `✕ rm` or `○ already absent` plus its size.
Confirming sends the real delete into the same kind of SSE job as a download, so the stream shows every `removing`, `removed` and `missing` event.
Nothing is deleted without that confirmation.

## Progress

Every job gets its own `<details>` card with a header line `job <id> <scope>` and a status that ends in `ok` or `err`.
Artifact events draw one bar per active file, indeterminate when the total size is unknown, and collapse into a one-line tally as files finish.
A dropped SSE connection reconnects on its own: the server replays the whole frame log, so a late or reconnecting browser loses nothing.
The `end` frame closes the stream, and a toast reports the outcome.

## Run

Build the addon first, then install and start the panel.

```bash
cd crates/nexus-raw-napi
pnpm install
pnpm run build
cd ../../examples/panel
pnpm install
node server.mjs --url http://127.0.0.1:8099/
```

Open http://localhost:8123.

Or let one recipe do all of it: `just panel` builds the addon and the mock, starts the mock on :8099, seeds a tree, starts the panel on :8123, and cleans the mock up when the panel exits.
Extra arguments pass through to `server.mjs`, for example `just panel --port 9000`.

## Try it against the mock

`just panel` already runs this stand.
By hand, in a second terminal, serve the mock and seed it.

```bash
cd <repo root>
cargo build -p mock-nexus -p nexus-raw
target/debug/mock-nexus atomic --port 8099
```

The mock starts with the repositories document only.
Seed a three-level tree plus the search page the listings read.

```bash
mkdir -p /tmp/panel-seed/app/core/util /tmp/panel-seed/app/bin /tmp/panel-seed/dist
printf 'readme\n' > /tmp/panel-seed/README.txt
printf 'lib\n' > /tmp/panel-seed/app/core/lib.rs
printf 'math\n' > /tmp/panel-seed/app/core/util/math.rs
printf 'shebang\n' > /tmp/panel-seed/app/bin/nxr.sh
printf 'svg\n' > /tmp/panel-seed/dist/logo.svg
cargo run -p nexus-raw -- up /tmp/panel-seed http://127.0.0.1:8099/repository/raw-main/
printf '{"continuationToken":null,"items":[{"path":"README.txt"},{"path":"app/core/lib.rs"},{"path":"app/core/util/math.rs"},{"path":"app/bin/nxr.sh"},{"path":"dist/logo.svg"}]}' > /tmp/search-page.json
cargo run -p nexus-raw -- put http://127.0.0.1:8099/service/rest/v1/search/assets -f /tmp/search-page.json
```

The search page goes through the same flat store as any object, which is how the core's own tests feed the search API.
Paths in the page are repository-relative.
The mock strips the query string, so one static page answers every listing request and the tree filter is applied client-side.

In the panel, keep `http://127.0.0.1:8099`, click `raw-main`, descend through `app/` and `core/`, then press `Download this directory`.
Raise the target to a fresh directory: the files land as `<target>/app/core/...`, byte-identical to the seed.
Then try the keyboard: `↓` to a folder, `Enter` to descend, `Home` back to the root.
Drop a file on the tree, watch its bar, then delete it again through the trash button and its confirmation list.

## Persistence

The page remembers its last state in `localStorage`.
The last server URL is restored on load, the configured server is only the fallback.
The download target directory and the tree position (server, repository, path) survive a reload and are re-entered when the repository still exists.

## SSE shape

One `data:` JSON frame per step.

- `{"type":"start","op":"down"|"rm","url":...,"dir":...,"names":N}`: the operation, the repository URL, the scope (a local target for `down`, a remote path for `rm`) and the file count the walk produced.
- `{"type":"event","event":{...}}`: the core events (`plan`, `artifact`, `retrying`, `summary`, and `removing`/`removed`/`missing` from `rm`).
- `{"type":"end","ok":true,"summary":{...}}` or `{"type":"end","ok":false,"error":...}`.

The stream ends right after the `end` frame.
Late subscribers replay the whole frame log, so opening the stream after the POST loses nothing.
The replay also rewinds the job card: the `start` frame resets it, so a reconnecting browser converges instead of duplicating bars.

## Limitations

- Demo-grade: no authentication UI, no HTTPS, the server binds `127.0.0.1`.
- The download target directory is whatever the browser sends: the panel trusts its operator.
- Uploads are loose single files only: there is no directory upload through staging.
- File sizes come from listings and events only, never from HEAD requests.
- Files open and download straight from Nexus: the panel never proxies bytes.
- The server URL list is fixed at startup: there is no way to add a root from the page that the server did not validate.
- The mock has no real search endpoint, so `lsEntries` needs the seeded page described above.
- The mock's group repository does not dispatch the search API, so listings against `raw-all` answer with an enumerate error.
- Job history is capped at the last 50 jobs in memory.
