// The sandbox server: a tiny node:http facade that makes the wasm page work with zero CORS setup.
// No frameworks and no dependencies beyond node itself.
//
// Endpoints:
// - GET /                      the single static page
// - GET /pkg/*                 the wasm-bindgen glue and the module (mime: application/wasm)
// - GET /beacon                the page's completion ping, logged for the headless check
// - everything else            the fake Nexus below, or a pass-through proxy with --upstream URL
//
// The nexus mount is the URL ROOT, not a prefix: the core URL grammar demands
// `/repository/<name>/...`, so the proxy must be the nexus, address-wise.
// A real Nexus sends no CORS headers, which is why the proxy, not the browser, owns that hop.

import { createServer, request as httpRequest } from 'node:http'
import { readFile } from 'node:fs/promises'
import { dirname, extname, join, normalize } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.wasm': 'application/wasm',
  '.json': 'application/json',
}

// The demo tree the fake Nexus serves: two folders, three files, plus the `.sha256` markers the client must hide.
const demoTree = new Map([
  ['repository/demo/app/README.md', 'demo: README\n'],
  ['repository/demo/app/core/lib.rs', 'demo: fn main() {}\n'],
  ['repository/demo/app/core/lib.rs.sha256', 'demo-mark lib.rs\n'],
  ['repository/demo/bom/x.bin', '\0\0demo\0'],
  ['repository/demo/root.txt', 'demo: root\n'],
  ['repository/demo/root.txt.sha256', 'demo-mark root\n'],
])

// Search pages of three items each, so the wasm pagination loop really turns.
const PAGE_SIZE = 3

// `GET /nexus/service/rest/v1/search/assets?repository=<name>`: every path of that repository, paged.
function searchAssets(query) {
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
function fakeNexus(path, query) {
  if (path === '/service/rest/v1/search/assets') {
    return { status: 200, body: JSON.stringify(searchAssets(query)) }
  }
  if (demoTree.has(path.slice(1))) {
    return { status: 200, body: demoTree.get(path.slice(1)) }
  }
  return { status: 404, body: 'not found\n' }
}

// The real-Nexus hop: forward method, query and Authorization, stream the answer back.
// Only GET and HEAD are sandbox-relevant; anything else answers 405.
function proxyNexus(upstream, req, res) {
  const target = new URL(upstream)
  const out = httpRequest(
    {
      protocol: target.protocol,
      hostname: target.hostname,
      port: target.port,
      method: req.method,
      path: req.url,
      headers: { authorization: req.headers.authorization ?? '' },
    },
    (answer) => {
      res.writeHead(answer.statusCode ?? 502, { 'content-type': answer.headers['content-type'] ?? 'application/octet-stream' })
      answer.pipe(res)
    },
  )
  out.on('error', (err) => {
    res.writeHead(502, { 'content-type': 'text/plain; charset=utf-8' })
    res.end(`upstream ${upstream} unreachable: ${err.message}\n`)
  })
  out.end()
}

function parseArgs(argv) {
  const args = { port: 8134, upstream: null }
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--port') args.port = Number(argv[++i])
    else if (argv[i] === '--upstream') args.upstream = argv[++i]
  }
  return args
}

const { port, upstream } = parseArgs(process.argv.slice(2))

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? '/', 'http://localhost')
  if (url.pathname === '/beacon') {
    console.log(`page run: rows=${url.searchParams.get('rows')} failed="${url.searchParams.get('failed') ?? ''}"`)
    res.writeHead(204)
    return res.end()
  }
  if (url.pathname === '/' || url.pathname === '/index.html') {
    // Read per request: the page is tiny and edit-reload loops must not need a server restart.
    const indexHtml = await readFile(join(here, 'index.html'))
    res.writeHead(200, { 'content-type': MIME['.html'] })
    return res.end(indexHtml)
  }
  if (url.pathname.startsWith('/pkg/')) {
    // Static glue: path traversal is normalized away first.
    const file = join(here, 'pkg', normalize(url.pathname).replace(/^[/\\]pkg[/\\]/, ''))
    if (!file.startsWith(join(here, 'pkg'))) {
      res.writeHead(403)
      return res.end()
    }
    try {
      const body = await readFile(file)
      res.writeHead(200, { 'content-type': MIME[extname(file)] ?? 'application/octet-stream' })
      return res.end(body)
    } catch {
      res.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' })
      return res.end('not found: build the wasm first (see README.md)\n')
    }
  }
  // Everything below is the nexus, address-wise.
  if (upstream) return proxyNexus(upstream, req, res)
  const fake = fakeNexus(url.pathname, url.searchParams)
  res.writeHead(fake.status, { 'content-type': 'text/plain; charset=utf-8' })
  res.end(fake.body)
})

server.listen(port, '127.0.0.1', () => {
  console.log(`sandbox listening http://localhost:${server.address().port}`)
  console.log(upstream ? `proxying everything but /, /pkg and /beacon to ${upstream}` : 'serving the built-in fake Nexus (pass --upstream URL for a real one)')
})
