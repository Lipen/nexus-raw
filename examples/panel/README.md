# nexus-raw panel

A minimal web panel over the `nexus-raw` Node bindings.
One static page, one `node:http` server, zero npm dependencies beyond the linked addon.

## What it shows

- The repositories of a Nexus server (`serviceRepos`).
- Only raw repositories are navigable: other formats render dimmed with a badge and refuse clicks.
- A raw repository as what it is: an arbitrary file tree (`lsEntries`).
  Breadcrumbs on top, `name/` rows for folders, `name` rows for files, any depth.
- A download form that runs `down` in the server process and streams every core event to the browser over SSE.

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
  `GET /api/servers` lists the configured servers, `GET /api/repos?url=<server>` lists repositories.
  `GET /api/entries?url=<dir>` lists the immediate children of a raw directory.
  `POST /api/down` with `{"url": <dir>, "path": <subtree>, "dir": <target>}` starts a job and answers `{"id": ...}`.
  `GET /api/progress/<id>` is the SSE stream of that job.
- `index.html`: repositories on the left, the tree browser on the right, a download form and a progress log below.
  No external assets, no build step.

## Tree navigation and download semantics

The browser knows nothing about versions or objects.
Clicking a raw repository lands at the repository root, clicking a `name/` row descends, clicking a breadcrumb segment ascends.

The download always targets the directory on screen, repository root or any subtree below it.
The server walks that subtree with recursive `lsEntries` calls, collects every file name relative to the repository root, and passes the whole list to `down` as explicit `names` against the repository URL.
Nothing is enumerated through a manifest: the files the browser showed are the files that download.
The local target directory mirrors the repository layout, so a download of `app/core` lands as `<target>/app/core/...`.

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

## Try it against the mock

In a second terminal, serve the mock and seed it.

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

## SSE shape

One `data:` JSON frame per step.

- `{"type":"start","url":...,"dir":...,"names":N}`: the repository URL, the local target and the file count the walk produced.
- `{"type":"event","event":{...}}`: the core events (`plan`, `artifact`, `retrying`, `summary`).
- `{"type":"end","ok":true,"summary":{...}}` or `{"type":"end","ok":false,"error":...}`.

The stream ends right after the `end` frame.
Late subscribers replay the whole frame log, so opening the stream after the POST loses nothing.

## Limitations

- Demo-grade: no authentication UI, no HTTPS, the server binds `127.0.0.1`.
- The download target directory is whatever the browser sends: the panel trusts its operator.
- The mock has no real search endpoint, so `lsEntries` needs the seeded page described above.
- The mock's group repository does not dispatch the search API, so listings against `raw-all` answer with an enumerate error.
- Job history is capped at the last 50 downloads in memory.
