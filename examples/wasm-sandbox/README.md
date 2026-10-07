# nxr wasm sandbox

The GO/NO-GO spike for running `nexus-raw-core` in the browser.
`src/lib.rs` exports one call, `nxr_ls(dir_url, auth)`, on top of the wasm-compatible slice of core: transport, `ls`, manifests, channels, pointers.
Everything that touches the local filesystem is native-only and gated out by `cfg(target_arch = "wasm32")` in the core crate.

## Build and run

From the repository root, one recipe does both:

```console
just wasm                 # builds and serves the built-in fake Nexus
just wasm --upstream http://127.0.0.1:8081   # a real Nexus behind the same-origin proxy
```

The first build needs the wasm32 target (`rustup target add wasm32-unknown-unknown`) and `wasm-bindgen-cli`.
The manual equivalent, for when you want the steps separately:

```console
cd examples/wasm-sandbox
cargo build --target wasm32-unknown-unknown --release --locked
wasm-bindgen --target web --out-dir pkg \
    target/wasm32-unknown-unknown/release/nexus_raw_example_wasm_sandbox.wasm
node serve.mjs
```

Open `http://localhost:8134/`: the page lists `/repository/demo/` on load.

## Serving an upstream

The fake's repository is named `demo`; a real Nexus knows nothing about it.
Open the page with the repository you want: `http://localhost:8134/?repo=<name>`
The listing then points at `/repository/<name>/`.

A real Nexus answers `400` on a search for a repository that does not exist or is not raw.
The mock answers `404` until its search page is seeded, the same one-shot the panel recipe runs:

```console
printf '{"continuationToken":null,"items":[{"path":"a.txt"}]}' > /tmp/search-page.json
target/debug/nxr put http://127.0.0.1:8081/service/rest/v1/search/assets -f /tmp/search-page.json
```

The page explains both cases right in its error block.

### The guard flags

Upstream mode forwards only what the page needs, and the guards all default to the strict end:

```console
--upstream URL              the only host ever dialed
--allow-host HOST           lets a request name HOST in its Host header; repeatable, empty by default
--rate-limit N              proxied requests per 10 seconds per client; 60 by default, 0 turns it off
--max-response-bytes N      caps an upstream answer; 8388608 by default
```

The allowlist is a guard against turning the sandbox into a relay, not a routing table: the proxy dials `--upstream` and nothing else, whatever a request asks for.
A request is forwarded only when it is `GET` or `HEAD`, targets an origin-form path under `/repository/` or `/service/rest/`, and names this origin (or an `--allow-host` entry) in its `Host` header.
Everything else is refused without touching the network: a forward-proxy request (`curl --proxy http://localhost:8134 http://elsewhere/`, an absolute-form target) answers `403`, another host in `Host` answers `403`, a path outside the two prefixes answers `404` locally, a client past its window answers `429` with `Retry-After`, and an upstream answer past the cap is cut off with `502`.
`--allow-host` exists for reaching the sandbox under another name (a container, a tunnel), where the browser's `Host` is not the loopback origin.
## Why the Nexus is mounted at the root

The core URL grammar demands `/repository/<name>/...`, so the page's origin must *be* the Nexus, address-wise: no mount prefix.
The fake and the proxy both answer `/repository/*` and `/service/rest/*` directly.

## Why a proxy at all

A real Nexus sends no CORS headers, so the browser cannot call it directly.
The proxy forwards GET and HEAD with query and `Authorization` intact and answers 405 to anything else.
The `fetch`-based reqwest backend needs no other change: TLS and timeouts are the browser's business.
