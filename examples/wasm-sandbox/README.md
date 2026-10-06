# nxr wasm sandbox

The GO/NO-GO spike for running `nexus-raw-core` in the browser.
`src/lib.rs` exports one call, `nxr_ls(dir_url, auth)`, on top of the wasm-compatible slice of core: transport, `ls`, manifests, channels, pointers.
Everything that touches the local filesystem is native-only and gated out by `cfg(target_arch = "wasm32")` in the core crate.

## Build

```console
cd examples/wasm-sandbox
cargo build --target wasm32-unknown-unknown --release
wasm-bindgen --target web --out-dir pkg \
    target/wasm32-unknown-unknown/release/nexus_raw_example_wasm_sandbox.wasm
```

## Run

```console
node serve.mjs                 # built-in fake Nexus at /nexus/
node serve.mjs --upstream http://127.0.0.1:8081   # a real Nexus behind the same-origin proxy
```

Open `http://localhost:8134/`: the page lists `/nexus/repository/demo/` on load.

## Why a proxy

A real Nexus sends no CORS headers, so the browser cannot call it directly.
The page therefore speaks only to its own origin, and the server forwards `/nexus/*`.
The `fetch`-based reqwest backend needs no other change: TLS and timeouts are the browser's business.
