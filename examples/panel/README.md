# nexus-raw panel

A minimal web panel over the `nexus-raw` Node bindings.
One static page, one `node:http` server, zero npm dependencies beyond the linked addon.

## What it shows

- The repositories of a Nexus server (`serviceRepos`).
- The versions of a repository (`lsVersions`).
- The assets of a version directory (`lsAssets`).
- A download form that runs `down` in the server process and streams every core event to the browser over SSE.

## Layout

- `server.mjs`: the HTTP facade.
  `GET /api/repos?url=<server>`, `GET /api/versions?url=<repo>`, `GET /api/assets?url=<dir>`.
  `POST /api/down` with `{"url": <dir>, "dir": <target>, "names": [...]}` starts a job and answers `{"id": ...}`.
  `GET /api/progress/<id>` is the SSE stream of that job.
- `index.html`: three panes, a download form and a progress log.
  No external assets, no build step.

## Run

Build the addon first, then install and start the panel.

```bash
cd crates/nexus-raw-napi
pnpm install
pnpm run build
cd ../../examples/panel
pnpm install
pnpm start
# or: node server.mjs --port 8123
```

Open http://localhost:8123.

## Try it against the mock

In a second terminal, serve the mock and seed it.

```bash
cd <repo root>
cargo run -p mock-nexus -- atomic --port 8099
```

The mock starts with the repositories document only.
Seed one version plus the search page the panel listings read.

```bash
mkdir -p /tmp/panel-seed/1.14.0
printf 'payload\n' > /tmp/panel-seed/1.14.0/notes.txt
head -c 200000 /dev/urandom > /tmp/panel-seed/1.14.0/app.bin
printf '{"artifacts":["app.bin","notes.txt"]}' > /tmp/panel-seed/1.14.0/manifest.json
cargo run -p nexus-raw -- up /tmp/panel-seed http://127.0.0.1:8099/repository/raw-main/
printf '{"items":[{"path":"1.14.0/manifest.json"},{"path":"1.14.0/app.bin"},{"path":"1.14.0/notes.txt"}]}' > /tmp/search-page.json
cargo run -p nexus-raw -- put http://127.0.0.1:8099/service/rest/v1/search/assets -f /tmp/search-page.json
```

The search page goes through the same flat store as any object, which is how the core's own tests feed the search API.
In the panel, keep `http://127.0.0.1:8099`, press `List repositories`, click `raw-main`, then `1.14.0`.
Pick assets or none and press `Download selected`.

With no selection the server passes no enumeration source, so the addon falls back to the conventional `manifest.json` at the version URL.
With a selection the server passes explicit `names`.

## SSE shape

One `data:` JSON frame per step.

- `{"type":"start","url":...,"dir":...}`
- `{"type":"event","event":{...}}`: the core events (`plan`, `artifact`, `retrying`, `summary`).
- `{"type":"end","ok":true,"summary":{...}}` or `{"type":"end","ok":false,"error":...}`.

The stream ends right after the `end` frame.
Late subscribers replay the whole frame log, so opening the stream after the POST loses nothing.

## Limitations

- Demo-grade: no authentication UI, no HTTPS, the server binds `127.0.0.1`.
- The download target directory is whatever the browser sends: the panel trusts its operator.
- The mock has no real search endpoint, so `lsVersions` and `lsAssets` need the seeded page described above.
- The mock's group repository does not dispatch the search API, so listings against `raw-all` answer with an enumerate error.
- Job history is capped at the last 50 downloads in memory.
