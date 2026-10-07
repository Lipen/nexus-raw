// The panel server: a tiny node:http facade over the nexus-raw Node bindings.
// No frameworks and no dependencies beyond the linked addon.
//
// Endpoints:
// - GET  /                    the single static page
// - GET  /api/servers         the server URLs configured at startup
// - GET  /api/repos?url=      the repository list of a server
// - GET  /api/entries?url=    the immediate children of a raw directory URL
// - POST /api/down            starts a download job, answers `{id}`
// - POST /api/rm              starts a delete job (or answers a dry-run plan)
// - POST /api/put             uploads a raw body to `?url=` with the sha marker
// - GET  /api/progress/<id>   the SSE stream of a job, one `data:` frame per step
//
// Startup is strict by default: every `--url` server is probed with
// `serviceRepos` before the port binds, and an unreachable server or one
// without raw repositories exits non-zero with a hint.
// `--lazy` skips that probe for quick local experiments.
//
// A job replays its frames to late subscribers, so a browser that opens the
// stream after the POST still sees the whole history.

import { createServer } from 'node:http'
import { randomUUID } from 'node:crypto'
import { readFile } from 'node:fs/promises'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import nxr from 'nexus-raw'

const here = dirname(fileURLToPath(import.meta.url))
const indexHtml = await readFile(join(here, 'index.html'))

// One kept job per download: the frame log doubles as the replay buffer.
const jobs = new Map()
const JOB_HISTORY_MAX = 50

// The memory cap of one /api/put body: a single uploaded file.
const PUT_BODY_CAP = 100 * 1024 * 1024

function rememberJob(job) {
  jobs.set(job.id, job)
  while (jobs.size > JOB_HISTORY_MAX) {
    jobs.delete(jobs.keys().next().value)
  }
}

// Push a frame to the log and to every live subscriber.
// The `end` frame is panel plumbing: the Summary event itself travels as a
// plain core event one frame earlier.
function publish(job, frame) {
  job.frames.push(frame)
  const data = `data: ${JSON.stringify(frame)}\n\n`
  for (const res of [...job.listeners]) {
    try {
      res.write(data)
      if (frame.type === 'end') {
        res.end()
      }
    } catch {
      job.listeners.delete(res)
    }
  }
  if (frame.type === 'end') {
    job.done = true
    job.listeners.clear()
  }
}

async function runDownload(job, repoUrl, dir, names) {
  publish(job, { type: 'start', op: 'down', url: repoUrl, dir, names: names.length })
  try {
    const summary = await nxr.down(repoUrl, dir, {
      names,
      fresh: true,
      onEvent: (event) => publish(job, { type: 'event', event }),
    })
    publish(job, { type: 'end', ok: true, summary })
  } catch (err) {
    publish(job, { type: 'end', ok: false, error: err?.message ?? String(err), exitCode: err?.exitCode, hint: err?.hint })
  }
}

class HttpError extends Error {
  constructor(status, payload) {
    super(payload.error)
    this.status = status
    this.payload = payload
  }
}

function badRequest(message) {
  return new HttpError(400, { error: message })
}

function errorPayload(err) {
  return { error: err?.message ?? String(err), exitCode: err?.exitCode, hint: err?.hint }
}

function sendJson(res, status, body) {
  const data = JSON.stringify(body)
  res.writeHead(status, {
    'content-type': 'application/json; charset=utf-8',
    'content-length': Buffer.byteLength(data),
  })
  res.end(data)
}

// The raw bytes of a request body, refusing anything past `limit`.
function readRawBody(req, limit = 1024 * 1024) {
  return new Promise((resolve, reject) => {
    let size = 0
    const chunks = []
    req.on('data', (chunk) => {
      size += chunk.length
      if (size > limit) {
        req.pause()
        reject(badRequest(`request body too large (the cap is ${limit} bytes)`))
        // The error reply needs a moment to flush before the socket dies.
        setTimeout(() => req.destroy(), 250)
        return
      }
      chunks.push(chunk)
    })
    req.on('end', () => resolve(Buffer.concat(chunks)))
    req.on('error', reject)
  })
}

// A JSON body with a size cap: only the parse failure becomes "must be JSON",
// a cap error from `readRawBody` passes through with its own message.
async function readJson(req, limit) {
  const raw = (await readRawBody(req, limit)).toString('utf8')
  try {
    return JSON.parse(raw || '{}')
  } catch {
    throw badRequest('the request body must be JSON')
  }
}

// The URL query parameter every GET endpoint shares.
function queryUrl(url, name = 'url') {
  const value = url.searchParams.get(name)
  if (!value || !/^https?:\/\//.test(value)) {
    throw badRequest(`the ${name} query parameter must be an http(s) URL`)
  }
  return value
}

// Split `.../repository/<name>/<subtree>/` into the repo root and the decoded
// subtree path. The root keeps its trailing slash, so names append directly.
function splitRepoUrl(dirUrl) {
  const parsed = new URL(dirUrl)
  const segs = parsed.pathname.split('/').filter((s) => s !== '')
  const at = segs.indexOf('repository')
  if (at === -1 || at + 1 >= segs.length) {
    return null
  }
  const root = new URL(dirUrl)
  root.pathname = `/${segs.slice(0, at + 2).join('/')}/`
  const subtree = segs.slice(at + 2).map(decodeSegment).join('/')
  return { repoUrl: root.toString(), subtree }
}

// A decoded path segment; a malformed escape falls back to the raw text.
function decodeSegment(seg) {
  try {
    return decodeURIComponent(seg)
  } catch {
    return seg
  }
}

// A light path check on the subtree the browser sends: relative segments only.
function validSubtree(path) {
  if (path === '') {
    return true
  }
  return path.split('/').every((seg) => seg !== '' && seg !== '.' && seg !== '..')
}

// Walk the subtree below `dirUrl` through `lsEntries`, collecting every file
// name relative to the repo root: exactly the list the download call needs.
async function walkFiles(dirUrl, prefix, out) {
  for (const entry of await nxr.lsEntries(dirUrl)) {
    const name = prefix === '' ? entry.name : `${prefix}/${entry.name}`
    if (entry.kind === 'dir') {
      await walkFiles(`${dirUrl}${encodeURIComponent(entry.name)}/`, name, out)
    } else {
      out.push(name)
    }
  }
  return out
}

async function startDownload(body) {
  const { url, path, dir } = body ?? {}
  if (typeof dir !== 'string' || dir.length === 0) {
    throw badRequest('dir must be a local target directory path')
  }
  const split = validateSubtree(url, path)
  const files = await walkFiles(subtreeUrl(split.repoUrl, path), path, [])
  if (files.length === 0) {
    throw badRequest(`no files under ${url}`)
  }
  const job = newJob()
  runDownload(job, split.repoUrl, dir, files)
  return { id: job.id }
}

// The URL validation every transferring or destructive call shares: a
// repository directory URL whose subtree path agrees with the body.
function validateSubtree(url, path) {
  if (typeof url !== 'string' || !/^https?:\/\//.test(url)) {
    throw badRequest('url must be an http(s) directory URL')
  }
  if (typeof path !== 'string' || !validSubtree(path)) {
    throw badRequest('path must be a relative subtree path inside the repository, possibly empty')
  }
  const split = splitRepoUrl(url)
  if (split === null) {
    throw badRequest('url must point inside /repository/<name>/')
  }
  if (split.subtree !== path) {
    throw badRequest(`path ${JSON.stringify(path)} does not match the URL subtree ${JSON.stringify(split.subtree)}`)
  }
  return split
}

// The browse URL of a subtree below the repository root.
function subtreeUrl(repoUrl, path) {
  return `${repoUrl}${path === '' ? '' : `${path.split('/').map(encodeURIComponent).join('/')}/`}`
}

function newJob() {
  const job = { id: randomUUID(), frames: [], listeners: new Set(), done: false }
  rememberJob(job)
  return job
}

// The delete twin of the download job: same walk, same SSE stream shape.
// With `file` the scope is one entry of the directory at `path` and the name
// list is that single file, no walk. With `dryRun` the answer is the plan
// (`rm` vs `missing` plus sizes) and nothing is deleted: the browser shows
// the list before the real run.
async function startRemove(body) {
  const { url, path, file, dryRun } = body ?? {}
  if (file !== undefined && (typeof file !== 'string' || file === '' || file.includes('/') || !validSubtree(file))) {
    throw badRequest('file must be one path segment naming an entry inside the directory at path')
  }
  const split = validateSubtree(url, path)
  let names
  let scope
  if (file !== undefined) {
    names = [path === '' ? file : `${path}/${file}`]
    scope = names[0]
  } else {
    names = await walkFiles(subtreeUrl(split.repoUrl, path), path, [])
    scope = path === '' ? '(repository root)' : path
  }
  if (dryRun === true) {
    if (names.length === 0) {
      return { actions: [] }
    }
    return nxr.rm(split.repoUrl, { names, dryRun: true })
  }
  if (names.length === 0) {
    throw badRequest(`no files under ${url}`)
  }
  const job = newJob()
  runRemove(job, split.repoUrl, scope, names)
  return { id: job.id }
}

async function runRemove(job, repoUrl, scope, names) {
  publish(job, { type: 'start', op: 'rm', url: repoUrl, dir: scope, names: names.length })
  try {
    const summary = await nxr.rm(repoUrl, {
      names,
      onEvent: (event) => publish(job, { type: 'event', event }),
    })
    publish(job, { type: 'end', ok: true, summary })
  } catch (err) {
    publish(job, { type: 'end', ok: false, error: err?.message ?? String(err), exitCode: err?.exitCode, hint: err?.hint })
  }
}

function serveProgress(req, res, id) {
  const job = jobs.get(id)
  if (!job) {
    sendJson(res, 404, { error: `unknown job ${id}` })
    return
  }
  res.writeHead(200, {
    'content-type': 'text/event-stream',
    'cache-control': 'no-cache',
    connection: 'keep-alive',
  })
  for (const frame of job.frames) {
    res.write(`data: ${JSON.stringify(frame)}\n\n`)
  }
  if (job.done) {
    res.end()
    return
  }
  job.listeners.add(res)
  const heartbeat = setInterval(() => {
    try {
      res.write(': ping\n\n')
    } catch {
      clearInterval(heartbeat)
    }
  }, 15000)
  req.on('close', () => {
    clearInterval(heartbeat)
    job.listeners.delete(res)
  })
}

// Browsers attach an Origin header to every cross-site POST, including
// `no-cors` ones that skip preflight. A foreign origin means some other page
// is driving the panel, so the mutating routes refuse it: the bindings would
// otherwise attach the process credentials to whatever host that page chose.
// No Origin at all (curl, same-origin fetch) passes.
function refuseForeignOrigin(req) {
  const origin = req.headers.origin
  if (origin === undefined) {
    return
  }
  let originHost
  try {
    originHost = new URL(origin).host
  } catch {
    throw badRequest('the Origin header must be a URL')
  }
  if (originHost !== (req.headers.host ?? '')) {
    throw new HttpError(403, { error: `cross-origin POST refused: origin ${origin} is not this panel` })
  }
}

async function route(req, res) {
  const url = new URL(req.url, `http://${req.headers.host ?? 'localhost'}`)
  if (req.method === 'GET' && (url.pathname === '/' || url.pathname === '/index.html')) {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
    res.end(indexHtml)
    return
  }
  if (req.method === 'GET' && url.pathname === '/api/servers') {
    sendJson(res, 200, servers)
    return
  }
  if (req.method === 'GET' && url.pathname === '/api/repos') {
    sendJson(res, 200, await nxr.serviceRepos(queryUrl(url)))
    return
  }
  if (req.method === 'GET' && url.pathname === '/api/entries') {
    sendJson(res, 200, await nxr.lsEntries(queryUrl(url)))
    return
  }
  if (req.method === 'POST') {
    refuseForeignOrigin(req)
  }
  if (req.method === 'POST' && url.pathname === '/api/down') {
    sendJson(res, 200, await startDownload(await readJson(req)))
    return
  }
  if (req.method === 'POST' && url.pathname === '/api/rm') {
    sendJson(res, 200, await startRemove(await readJson(req)))
    return
  }
  // A single-file upload: the raw body IS the file, `nxr.put` stages it and
  // writes the `.sha256` marker itself. The cap bounds one file in memory.
  // The target must be a repository path: the panel is not a relay to
  // arbitrary hosts.
  if (req.method === 'POST' && url.pathname === '/api/put') {
    const target = queryUrl(url)
    if (splitRepoUrl(target) === null) {
      throw badRequest('url must point inside /repository/<name>/')
    }
    const withSha = url.searchParams.get('sha') !== '0'
    const body = await readRawBody(req, PUT_BODY_CAP)
    const result = await nxr.put(target, body, { sha: withSha })
    sendJson(res, 200, { size: result.size, sha256: result.sha256 ?? null })
    return
  }
  const progress = req.method === 'GET' && url.pathname.match(/^\/api\/progress\/([A-Za-z0-9-]+)$/)
  if (progress) {
    serveProgress(req, res, progress[1])
    return
  }
  sendJson(res, 404, { error: `no route for ${req.method} ${url.pathname}` })
}

// Parse the command line: `--url U` (repeatable) or positional server URLs,
// `--port N` (default 8123), and the `--lazy` switch.
function parseArgs(argv) {
  const servers = []
  let port = 8123
  let lazy = false
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]
    if (arg === '--lazy') {
      lazy = true
      continue
    }
    if (arg === '--url' || arg.startsWith('--url=')) {
      const value = arg === '--url' ? argv[++i] : arg.slice('--url='.length)
      if (value === undefined || value === '') {
        fail(`--url needs a server URL`)
      }
      servers.push(value)
      continue
    }
    if (arg === '--port' || arg.startsWith('--port=')) {
      const value = arg === '--port' ? argv[++i] : arg.slice('--port='.length)
      port = Number(value)
      if (!Number.isInteger(port) || port < 0 || port > 65535) {
        fail(`bad --port value: ${value}`)
      }
      continue
    }
    if (arg.startsWith('--')) {
      fail(`unknown option ${arg}`)
    }
    servers.push(arg)
  }
  for (const server of servers) {
    if (!/^https?:\/\//.test(server)) {
      fail(`server URL must be http(s): ${server}`)
    }
  }
  return { servers, port, lazy }
}

function fail(message) {
  console.error(`panel: ${message}`)
  console.error('usage: node server.mjs [--url URL]... [--port N] [--lazy]')
  process.exit(2)
}

// Strict startup: every configured server must answer `serviceRepos` and own
// at least one raw repository, or the panel refuses to start.
async function validateServers(urls) {
  for (const server of urls) {
    let repos
    try {
      repos = await nxr.serviceRepos(server)
    } catch (err) {
      console.error(`panel: ${server}: ${err?.message ?? String(err)}`)
      if (err?.hint) {
        console.error(`hint: ${err.hint}`)
      }
      process.exit(err?.exitCode ?? 1)
    }
    const raw = repos.filter((r) => r.format === 'raw')
    if (raw.length === 0) {
      const formats = repos.map((r) => r.format).join(', ')
      console.error(`panel: ${server}: no raw repositories on the server (formats: ${formats || 'none'})`)
      console.error('hint: create a repository with the raw format, or point --url at a server that has one')
      process.exit(1)
    }
    console.log(`panel: ${server}: raw repositories: ${raw.map((r) => r.name).join(', ')}`)
  }
}

const { servers, port, lazy } = parseArgs(process.argv.slice(2))
if (!lazy && servers.length === 0) {
  fail('no server URL given: pass --url <server> at least once, or add --lazy to skip startup validation')
}
if (!lazy) {
  await validateServers(servers)
}

const server = createServer((req, res) => {
  route(req, res).catch((err) => {
    const payload = errorPayload(err)
    try {
      sendJson(res, err instanceof HttpError ? err.status : 502, payload)
    } catch {
      // The socket is already gone (a destroyed upload, a dropped client):
      // there is nothing left to answer.
    }
  })
})

server.listen(port, '127.0.0.1', () => {
  console.log(`panel listening http://localhost:${server.address().port}`)
})
