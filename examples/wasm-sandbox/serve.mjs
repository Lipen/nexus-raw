// The sandbox server: a tiny node:http facade that makes the wasm page work with zero CORS setup.
// No frameworks and no dependencies beyond node itself.
//
// Endpoints:
// - GET /                      the single static page
// - GET /pkg/*                 the wasm-bindgen glue and the module (mime: application/wasm)
// - GET /beacon                the page's completion ping, logged for the headless check
// - everything else            the fake Nexus below, or a guarded pass-through with --upstream URL
//
// The nexus mount is the URL ROOT, not a prefix: the core URL grammar demands
// `/repository/<name>/...`, so the proxy must be the nexus, address-wise.
// A real Nexus sends no CORS headers, which is why the proxy, not the browser, owns that hop.
//
// In upstream mode the proxy is a facade, not a forward proxy: only what the page
// needs crosses over, and only as a same-origin request. The guards below
// (`--allow-host`, `--rate-limit`, `--max-response-bytes`) all default to
// the strict end, and every refusal is answered without dialing out.

import { createServer, request as httpRequest } from 'node:http'
import { readFile } from 'node:fs/promises'
import { dirname, extname, join, normalize } from 'node:path'
import { fileURLToPath } from 'node:url'
import { fakeNexus, faults } from './fake-nexus.mjs'

const here = dirname(fileURLToPath(import.meta.url))

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.wasm': 'application/wasm',
  '.json': 'application/json',
}

// The page registers the client-side ServiceWorker fake only when no server of ours
// marks it: serve.mjs rewrites the marker below, static hosting leaves it alone.
const SERVE_MARKER = 'NXR_SANDBOX_SERVE = "static"'

// Only paths under these prefixes are ever forwarded to the upstream.
const PROXIED_PREFIXES = ['/repository/', '/service/rest/']

// The rate limit is a fixed window per client address.
const RATE_WINDOW_MS = 10_000
const DEFAULT_RATE_LIMIT = 60
const DEFAULT_MAX_RESPONSE_BYTES = 8 * 1024 * 1024

// The real-Nexus hop guards, in order: origin-form target, GET/HEAD only,
// a proxied prefix, an allowlisted Host, then the per-client rate limit.
// The proxy dials the --upstream host and nothing else, so no combination of
// requests can turn it into a relay to a third host.
function proxyNexus(req, res, guards) {
  const url = new URL(req.url ?? '/', 'http://localhost')
  const deny = (status, body) => {
    console.log(`proxy refused ${req.socket.remoteAddress} ${url.pathname}: ${status}`)
    res.writeHead(status, { 'content-type': 'text/plain; charset=utf-8' })
    res.end(body)
  }
  // An absolute-form target (`GET http://elsewhere/ HTTP/1.1`, what `curl --proxy` sends)
  // is forward-proxy speech: the sandbox is same-origin and answers origin-form only.
  if (!req.url?.startsWith('/')) {
    return deny(403, 'the sandbox proxy is same-origin: it answers origin-form targets only, not forward-proxy requests\n')
  }
  if (req.method !== 'GET' && req.method !== 'HEAD') {
    res.writeHead(405, { 'content-type': 'text/plain; charset=utf-8' })
    return res.end('the sandbox proxy forwards GET and HEAD only\n')
  }
  if (!PROXIED_PREFIXES.some((prefix) => url.pathname.startsWith(prefix))) {
    return deny(404, 'the sandbox proxy forwards repository and service paths only\n')
  }
  // An absent Host is rare and harmless (nothing keys off it downstream); a present
  // Host must name this origin or a host the operator explicitly allowlisted.
  const host = req.headers.host ?? ''
  if (host && !guards.allowedHosts.has(host)) {
    return deny(403, `host "${host}" is not allowlisted (pass --allow-host ${host} to allow it)\n`)
  }
  const retryAfter = limitRate(guards, req.socket.remoteAddress ?? '')
  if (retryAfter !== null) {
    res.writeHead(429, { 'content-type': 'text/plain; charset=utf-8', 'retry-after': String(retryAfter) })
    return res.end(`rate limited: at most ${guards.rateLimit} proxied requests per ${RATE_WINDOW_MS / 1000} seconds per client\n`)
  }
  const target = new URL(guards.upstream)
  // An absent Authorization must stay absent: some upstreams treat an empty header as broken auth.
  const headers = {}
  if (req.headers.authorization) headers.authorization = req.headers.authorization
  const out = httpRequest(
    {
      protocol: target.protocol,
      hostname: target.hostname,
      port: target.port,
      method: req.method,
      path: req.url,
      headers,
    },
    (answer) => {
      const declared = Number(answer.headers['content-length'] ?? 0)
      if (declared > guards.maxResponseBytes) {
        console.log(`proxy cut ${url.pathname}: upstream declared ${declared} bytes, over the ${guards.maxResponseBytes} cap`)
        answer.destroy()
        res.writeHead(502, { 'content-type': 'text/plain; charset=utf-8' })
        return res.end(`upstream response is larger than the ${guards.maxResponseBytes} byte cap (raise --max-response-bytes)\n`)
      }
      let received = 0
      let headSent = false
      answer.on('data', (chunk) => {
        received += chunk.length
        if (received > guards.maxResponseBytes) {
          console.log(`proxy cut ${url.pathname}: upstream passed the ${guards.maxResponseBytes} byte cap`)
          answer.destroy()
          // Past the cap with headers gone there is no status left to send: drop the connection.
          if (!headSent) {
            res.writeHead(502, { 'content-type': 'text/plain; charset=utf-8' })
            res.end(`upstream response exceeded the ${guards.maxResponseBytes} byte cap (raise --max-response-bytes)\n`)
          } else {
            res.destroy()
          }
        }
      })
      res.writeHead(answer.statusCode ?? 502, { 'content-type': answer.headers['content-type'] ?? 'application/octet-stream' })
      headSent = true
      answer.pipe(res)
    },
  )
  res.on('close', () => out.destroy())
  out.on('error', (err) => {
    res.writeHead(502, { 'content-type': 'text/plain; charset=utf-8' })
    res.end(`upstream ${guards.upstream} unreachable: ${err.message}\n`)
  })
  out.end()
}

// One bucket per client address: at most `rateLimit` forwarded requests per window,
// then 429 with Retry-After until the window turns. Returns the wait in seconds, or null.
function limitRate(guards, address) {
  if (!guards.rateLimit) return null
  const now = Date.now()
  let bucket = guards.clients.get(address)
  if (!bucket || now - bucket.start >= RATE_WINDOW_MS) {
    // This address just opened a fresh window: drop every stale bucket while we are here.
    for (const [key, old] of guards.clients) {
      if (now - old.start >= RATE_WINDOW_MS) guards.clients.delete(key)
    }
    bucket = { start: now, count: 0 }
    guards.clients.set(address, bucket)
  }
  bucket.count += 1
  return bucket.count > guards.rateLimit
    ? Math.ceil((bucket.start + RATE_WINDOW_MS - now) / 1000)
    : null
}

function numberArg(value, flag) {
  const n = Number(value)
  if (!Number.isInteger(n) || n < 0) {
    console.error(`${flag} wants a non-negative integer, got "${value}"`)
    process.exit(2)
  }
  return n
}

function parseArgs(argv) {
  const args = {
    port: 8134,
    upstream: null,
    failFirst: 0,
    allowHosts: [],
    rateLimit: DEFAULT_RATE_LIMIT,
    maxResponseBytes: DEFAULT_MAX_RESPONSE_BYTES,
  }
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--port') args.port = numberArg(argv[++i], '--port')
    else if (argv[i] === '--upstream') args.upstream = argv[++i]
    else if (argv[i] === '--fail-first') args.failFirst = numberArg(argv[++i], '--fail-first')
    else if (argv[i] === '--allow-host') {
      const host = argv[++i]
      if (!host) usage('--allow-host wants a host')
      args.allowHosts.push(host)
    }
    else if (argv[i] === '--rate-limit') args.rateLimit = numberArg(argv[++i], '--rate-limit')
    else if (argv[i] === '--max-response-bytes') args.maxResponseBytes = numberArg(argv[++i], '--max-response-bytes')
    else usage(`unknown argument: ${argv[i]}`)
  }
  return args
}

function usage(message) {
  console.error(`${message}
usage: node serve.mjs [--port 8134] [--upstream URL] [--fail-first N]
                      [--allow-host HOST]... [--rate-limit N] [--max-response-bytes N]`)
  process.exit(2)
}

const parsed = parseArgs(process.argv.slice(2))
faults.failFirst = parsed.failFirst

let upstream = null
if (parsed.upstream) {
  try {
    upstream = new URL(parsed.upstream)
  } catch {
    usage(`--upstream wants an http(s) URL, got "${parsed.upstream}"`)
  }
  if (upstream.protocol !== 'http:' && upstream.protocol !== 'https:') {
    usage(`--upstream wants an http(s) URL, got "${parsed.upstream}"`)
  }
}

// Guards live in one bag so the proxy hop and the rate limiter read alike.
// The allowed hosts are fixed up after the bind, when the real port is known.
const guards = {
  upstream: upstream?.toString() ?? null,
  allowedHosts: new Set(),
  rateLimit: parsed.rateLimit,
  maxResponseBytes: parsed.maxResponseBytes,
  clients: new Map(),
}

const server = createServer(async (req, res) => {
  const url = new URL(req.url ?? '/', 'http://localhost')
  if (url.pathname === '/beacon') {
    console.log(`page run: rows=${url.searchParams.get('rows')} failed="${url.searchParams.get('failed') ?? ''}"`)
    res.writeHead(204)
    return res.end()
  }
  if (url.pathname === '/' || url.pathname === '/index.html') {
    // Read per request: the page is tiny and edit-reload loops must not need a server restart.
    // The rewritten marker keeps the ServiceWorker fake off: a fake or a proxy already answers here.
    const indexHtml = await readFile(join(here, 'index.html'), 'utf8')
    const mode = upstream ? 'upstream' : 'node-fake'
    const marked = indexHtml.replace(SERVE_MARKER, `NXR_SANDBOX_SERVE = "${mode}"`)
    if (marked === indexHtml) {
      res.writeHead(500, { 'content-type': 'text/plain; charset=utf-8' })
      return res.end('index.html lost the serve-mode marker: the page cannot tell serve.mjs from static hosting\n')
    }
    res.writeHead(200, { 'content-type': MIME['.html'] })
    return res.end(marked)
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
  if (upstream) return proxyNexus(req, res, guards)
  const fake = fakeNexus(url.pathname, url.searchParams)
  res.writeHead(fake.status, { 'content-type': 'text/plain; charset=utf-8' })
  res.end(fake.body)
})

server.listen(parsed.port, '127.0.0.1', () => {
  const bound = server.address().port
  // The Host header of a same-origin request names the origin the browser dialed:
  // loopback names on the bound port, plus whatever the operator allowlisted.
  guards.allowedHosts = new Set([
    `localhost:${bound}`,
    `127.0.0.1:${bound}`,
    `[::1]:${bound}`,
    ...parsed.allowHosts,
  ])
  console.log(`sandbox listening http://localhost:${bound}`)
  if (upstream) {
    console.log(`proxying /repository/* and /service/rest/* to ${upstream}`)
    console.log(`guards: hosts = this origin${parsed.allowHosts.length ? ` + ${parsed.allowHosts.join(', ')}` : ''}, rate = ${parsed.rateLimit || 'off'}/10s per client, response cap = ${parsed.maxResponseBytes} bytes`)
  } else {
    console.log('serving the built-in fake Nexus (pass --upstream URL for a real one)')
  }
})
