# Run the browser sandbox

The wasm sandbox is the proof that the read surface of the core runs in a browser: `nexus-raw-core` compiled to `wasm32-unknown-unknown`, listing a raw Nexus directory from a page.
The same protocol code answers as on the command line: the transport issues the search requests, the pagination walks the continuation tokens, and the `.sha256` markers arrive and are hidden by the client.
What stays native is the write half: the filesystem-bound slice of the core is gated out of the wasm build.

The code lives in `examples/wasm-sandbox/`, a standalone crate like the other examples.
Its [README](https://github.com/lipen/nexus-raw/blob/master/examples/wasm-sandbox/README.md) carries the full detail; this page is the tour.

## Run it locally

```bash
just wasm
```

The first build needs two tools:

```bash
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli
```

Then open `http://localhost:8134/`: the page lists `/repository/demo/` on load, the fake Nexus that ships with the sandbox server.
`just wasm-smoke` builds the same two steps and runs the smoke, which is what CI runs.

## Point it at a real Nexus

The fake's repository is named `demo`; a real one knows nothing about it.
Open the page with your repository: `http://localhost:8134/?repo=<name>`.

```bash
just wasm --upstream http://127.0.0.1:8081
```

The sandbox server becomes a guarded same-origin proxy.
A real Nexus sends no CORS headers, so the browser cannot call it directly, and the proxy owns that hop: it forwards `GET` and `HEAD` for `/repository/*` and `/service/rest/*`, and dials the `--upstream` host and nothing else.
The guards default to the strict end:

| Flag | Default | Meaning |
|:-----|:--------|:--------|
| `--allow-host HOST` | empty | lets a request name `HOST` in its `Host` header; repeatable |
| `--rate-limit N` | 60 | proxied requests per 10 seconds per client, then `429` with `Retry-After` |
| `--max-response-bytes N` | 8388608 | cuts an upstream answer off at the cap with `502` |

A forward-proxy request (`curl --proxy ...`) answers `403`: the proxy is a facade for the page, not a relay.
A `400` or `404` on the listing means the upstream has no searchable repository under that name, or it is not raw; the page says so in its error block.

## Static hosting, no backend

The fake Nexus ships with two faces over one module: `serve.mjs` imports it for the Node fake, and a ServiceWorker serves the same answers from the browser.
On a static host there is no Node process, so the page registers the worker instead, and the worker intercepts `/repository/*` and `/service/rest/*` with the same data: same pagination loop, same 404 arm, markers served so the client can hide them.
A directory of static files is the whole demo, which is what makes it landable on a docs site.

Three constraints apply:

- The page must be served over `https://` or from `localhost`: a ServiceWorker needs a secure context.
- The sandbox must sit at the origin root.
  The URL grammar of the [wire protocol](../reference/protocol.md) is origin-absolute, so the search endpoint always resolves against the bare origin, outside any deployment prefix and outside a worker scoped to one.
- The worker registers only where no sandbox server marks the page, so it never shadows the Node fake or the proxy.

## What CI watches

The `wasm` job builds the crate for `wasm32`, regenerates the wasm-bindgen glue at the locked version and runs the smoke.
The smoke drives the fake module, then the built wasm under plain node (a full listing, one injected failure retried on the way), then the page in a headless browser against a backend-free static server, where the only path to a listing is the ServiceWorker.
A break in the read surface fails the build, not a browser console.
