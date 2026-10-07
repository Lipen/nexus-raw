// The fake Nexus: one module, several hosts.
// `serve.mjs` imports it for the Node fake, `sw.mjs` answers from a ServiceWorker
// with the same data, and `smoke.mjs` drives both through this single copy.
// Keeping the demo tree here means the client-side fake cannot drift from the server-side one.
//
// The nexus mount is the URL ROOT, not a prefix: the core URL grammar demands
// `/repository/<name>/...` and resolves the search endpoint against the origin.

// The demo tree the fake Nexus serves: two folders, three files, plus the `.sha256` markers the client must hide.
export const demoTree = new Map([
  ['repository/demo/app/README.md', 'demo: README\n'],
  ['repository/demo/app/core/lib.rs', 'demo: fn main() {}\n'],
  ['repository/demo/app/core/lib.rs.sha256', 'demo-mark lib.rs\n'],
  ['repository/demo/bom/x.bin', '\0\0demo\0'],
  ['repository/demo/root.txt', 'demo: root\n'],
  ['repository/demo/root.txt.sha256', 'demo-mark root\n'],
])

// Search pages of three items each, so the wasm pagination loop really turns.
export const PAGE_SIZE = 3

// Fault injection for the retry path: the first `failFirst` search requests answer 500.
// `serve.mjs --fail-first 2` makes the page burn two retryable failures before it sees data.
// The ServiceWorker leaves this at zero: a static host has nobody to inject faults for.
export const faults = { failFirst: 0, hits: 0 }

// `GET /service/rest/v1/search/assets?repository=<name>`: every path of that repository, paged.
export function searchAssets(query) {
  const repo = query.get('repository') ?? ''
  const prefix = `repository/${repo}/`
  const paths = [...demoTree.keys()]
    .filter((p) => p.startsWith(prefix))
    .map((p) => p.slice(prefix.length))
    .sort()
  const token = Number(query.get('continuationToken') ?? 0)
  const page = paths.slice(token, token + PAGE_SIZE)
  const next = token + PAGE_SIZE
  return {
    items: page.map((path) => ({ path })),
    continuationToken: next < paths.length ? String(next) : null,
  }
}

// The fake Nexus answers GETs for objects and the search endpoint, 404 otherwise.
// A `--fail-first N` run answers 500 (retryable) for the first N search requests.
export function fakeNexus(path, query) {
  if (path === '/service/rest/v1/search/assets') {
    faults.hits++
    if (faults.hits <= faults.failFirst) {
      console.log(`search hit ${faults.hits}: 500 (injected)`)
      return { status: 500, body: 'injected failure for the retry check\n' }
    }
    console.log(`search hit ${faults.hits}: 200`)
    return { status: 200, body: JSON.stringify(searchAssets(query)) }
  }
  if (demoTree.has(path.slice(1))) {
    return { status: 200, body: demoTree.get(path.slice(1)) }
  }
  return { status: 404, body: 'not found\n' }
}
