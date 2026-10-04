// The panel server: a tiny node:http facade over the nexus-raw Node bindings.
// No frameworks and no dependencies beyond the linked addon.
//
// Endpoints:
// - GET  /                    the single static page
// - GET  /api/repos?url=      the repository list of a server
// - GET  /api/versions?url=   the version tokens of a repository
// - GET  /api/assets?url=     the asset names of a version directory
// - POST /api/down            starts a download job, answers `{id}`
// - GET  /api/progress/<id>   the SSE stream of a job, one `data:` frame per step
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

async function runDownload(job, url, dir, opts) {
  publish(job, { type: 'start', url, dir })
  try {
    const summary = await nxr.down(url, dir, {
      ...opts,
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

function readBody(req, limit = 1024 * 1024) {
  return new Promise((resolve, reject) => {
    let size = 0
    const chunks = []
    req.on('data', (chunk) => {
      size += chunk.length
      if (size > limit) {
        reject(badRequest('request body too large'))
        req.destroy()
        return
      }
      chunks.push(chunk)
    })
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')))
    req.on('error', reject)
  })
}

// The URL query parameter every GET endpoint shares.
function queryUrl(url, name = 'url') {
  const value = url.searchParams.get(name)
  if (!value || !/^https?:\/\//.test(value)) {
    throw badRequest(`the ${name} query parameter must be an http(s) URL`)
  }
  return value
}

async function startDownload(body) {
  const { url, dir, names } = body ?? {}
  if (typeof url !== 'string' || !/^https?:\/\//.test(url)) {
    throw badRequest('url must be an http(s) directory URL')
  }
  if (typeof dir !== 'string' || dir.length === 0) {
    throw badRequest('dir must be a local target directory path')
  }
  if (names !== undefined && (!Array.isArray(names) || names.some((n) => typeof n !== 'string' || n.length === 0))) {
    throw badRequest('names must be an array of non-empty strings when given')
  }
  const opts = { fresh: true }
  // An empty names list means the addon's own fallback: the conventional
  // manifest.json at the directory URL.
  if (Array.isArray(names) && names.length > 0) {
    opts.names = names
  }
  const job = { id: randomUUID(), frames: [], listeners: new Set(), done: false }
  rememberJob(job)
  runDownload(job, url, dir, opts)
  return { id: job.id }
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

async function route(req, res) {
  const url = new URL(req.url, `http://${req.headers.host ?? 'localhost'}`)
  if (req.method === 'GET' && (url.pathname === '/' || url.pathname === '/index.html')) {
    res.writeHead(200, { 'content-type': 'text/html; charset=utf-8' })
    res.end(indexHtml)
    return
  }
  if (req.method === 'GET' && url.pathname === '/api/repos') {
    sendJson(res, 200, await nxr.serviceRepos(queryUrl(url)))
    return
  }
  if (req.method === 'GET' && url.pathname === '/api/versions') {
    sendJson(res, 200, await nxr.lsVersions(queryUrl(url)))
    return
  }
  if (req.method === 'GET' && url.pathname === '/api/assets') {
    sendJson(res, 200, await nxr.lsAssets(queryUrl(url)))
    return
  }
  if (req.method === 'POST' && url.pathname === '/api/down') {
    let body
    try {
      body = JSON.parse((await readBody(req)) || '{}')
    } catch {
      throw badRequest('the request body must be JSON')
    }
    sendJson(res, 200, await startDownload(body))
    return
  }
  const progress = req.method === 'GET' && url.pathname.match(/^\/api\/progress\/([A-Za-z0-9-]+)$/)
  if (progress) {
    serveProgress(req, res, progress[1])
    return
  }
  sendJson(res, 404, { error: `no route for ${req.method} ${url.pathname}` })
}

// --port N or --port=N, default 8123.
function argValue(name) {
  const inline = process.argv.find((a) => a.startsWith(`--${name}=`))
  if (inline !== undefined) {
    return inline.slice(name.length + 3)
  }
  const i = process.argv.indexOf(`--${name}`)
  return i !== -1 && i + 1 < process.argv.length ? process.argv[i + 1] : undefined
}

const port = Number(argValue('port') ?? 8123)
if (!Number.isInteger(port) || port < 0 || port > 65535) {
  console.error(`panel: bad --port value: ${argValue('port')}`)
  process.exit(2)
}

const server = createServer((req, res) => {
  route(req, res).catch((err) => {
    const payload = errorPayload(err)
    sendJson(res, err instanceof HttpError ? err.status : 502, payload)
  })
})

server.listen(port, '127.0.0.1', () => {
  console.log(`panel listening http://localhost:${server.address().port}`)
})
