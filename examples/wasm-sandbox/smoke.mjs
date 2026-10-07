// The sandbox smoke: builds nothing, drives everything, stays dependency-free.
//
// Three layers, in order:
// - unit checks of the shared fake module: the pagination loop, the 404 arm, markers in search;
// - the built wasm under plain node: `nxr_ls` against the same fake, one injected 500
//   burned on the way, so the retry path runs too;
// - when a chromium-family binary is around (or --require-browser insists), the page
//   itself in a headless browser against a backend-free static server: there the only
//   way to a listing is the ServiceWorker fake, and the beacon proves it happened.
//
// Exit code 0 means: the read surface still compiles, loads and lists.
// Run it after a build (see the `wasm-smoke` recipe or ci.yml).

import assert from 'node:assert/strict'
import { spawn } from 'node:child_process'
import { access, constants, mkdtemp, readdir, readFile, rm } from 'node:fs/promises'
import { createServer } from 'node:http'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { dirname, extname, join, normalize } from 'node:path'
import { fileURLToPath } from 'node:url'
import { fakeNexus, faults, searchAssets } from './fake-nexus.mjs'

const here = dirname(fileURLToPath(import.meta.url))

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.wasm': 'application/wasm',
  '.json': 'application/json',
}

// Static files beyond the pkg glue: the page, the worker, the shared fake.
const STATIC_FILES = new Map([
  ['/', 'index.html'],
  ['/index.html', 'index.html'],
  ['/sw.mjs', 'sw.mjs'],
  ['/fake-nexus.mjs', 'fake-nexus.mjs'],
])

// One server, two temperaments: `withApi` answers the nexus endpoints (for the node
// run of the wasm), otherwise every nexus request is refused (for the browser run,
// where a listing is only possible through the ServiceWorker).
function startServer(withApi) {
  const beacons = []
  let apiHits = 0
  const server = createServer(async (req, res) => {
    const url = new URL(req.url ?? '/', 'http://localhost')
    if (url.pathname === '/beacon') {
      const record = {
        rows: Number(url.searchParams.get('rows') ?? -1),
        failed: url.searchParams.get('failed') ?? '',
      }
      beacons.push(record)
      console.log(`smoke: beacon: rows=${record.rows} failed="${record.failed}"`)
      res.writeHead(204)
      return res.end()
    }
    const isApi = url.pathname.startsWith('/repository/') || url.pathname.startsWith('/service/rest/')
    if (isApi) {
      apiHits += 1
      if (!withApi) {
        res.writeHead(502, { 'content-type': 'text/plain; charset=utf-8' })
        return res.end('no backend here: the ServiceWorker fake should have answered\n')
      }
      const fake = fakeNexus(url.pathname, url.searchParams)
      res.writeHead(fake.status, { 'content-type': 'text/plain; charset=utf-8' })
      return res.end(fake.body)
    }
    const named = STATIC_FILES.get(url.pathname)
    if (!named && !url.pathname.startsWith('/pkg/')) {
      res.writeHead(404, { 'content-type': 'text/plain; charset=utf-8' })
      return res.end('not found\n')
    }
    // Path traversal is normalized away first, as in serve.mjs.
    const file = named ? join(here, named) : join(here, 'pkg', normalize(url.pathname).replace(/^[/\\]pkg[/\\]/, ''))
    if (!file.startsWith(join(here, named ? '' : 'pkg'))) {
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
  })
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => resolve({
      url: `http://127.0.0.1:${server.address().port}`,
      beacons,
      apiHits: () => apiHits,
      close: () => new Promise((done) => server.close(done)),
    }))
  })
}

// A chromium-family binary: an explicit NXR_WASM_BROWSER wins, then PATH, then the
// playwright and puppeteer caches some machines carry. The headless shell is skipped
// on purpose: that build predates solid service worker support.
async function findBrowser() {
  if (process.env.NXR_WASM_BROWSER) return process.env.NXR_WASM_BROWSER
  const names = ['chromium', 'chromium-browser', 'google-chrome', 'google-chrome-stable', 'chrome']
  for (const dir of (process.env.PATH ?? '').split(':')) {
    for (const name of names) {
      if (await executable(join(dir, name))) return join(dir, name)
    }
  }
  const caches = [
    [join(process.env.HOME ?? '', '.cache', 'ms-playwright'), 'chromium-', ['chrome-linux64', 'chrome-linux'], 'chrome'],
    [join(process.env.HOME ?? '', '.cache', 'puppeteer', 'chrome'), 'linux-', ['chrome-linux64'], 'chrome'],
  ]
  for (const [cache, prefix, subs, leaf] of caches) {
    const versions = (await readdir(cache).catch(() => [])).filter((entry) => entry.startsWith(prefix)).sort()
    for (const version of versions) {
      for (const sub of subs) {
        const file = join(cache, version, sub, leaf)
        if (await executable(file)) return file
      }
    }
  }
  return null
}

async function executable(file) {
  return access(file, constants.X_OK).then(() => true, () => false)
}

// The browser run, two drivers, one contract. The page's load event does not wait
// for the module's top-level await, so both drivers wait for the beacon on the
// server side before reading anything: the rows render strictly before the ping.
// playwright-core, when the setup carries it (NXR_WASM_DRIVER names the module path),
// drives the page and returns the settled DOM. Without it the headless CLI takes
// over, dumps the DOM at its quiescence point and proves the same thing.
async function runBrowser(url, waitForBeacon) {
  const driver = process.env.NXR_WASM_DRIVER
  if (driver) {
    // A CJS require, so the path may be the package directory or its entry file.
    const playwright = createRequire(import.meta.url)(driver)
    const browser = await playwright.chromium.launch({ headless: true, args: ['--no-sandbox'] })
    const page = await browser.newPage()
    await page.goto(url, { waitUntil: 'load', timeout: 30_000 })
    await waitForBeacon()
    const dom = await page.content()
    await browser.close()
    return { dom, driver: 'playwright-core' }
  }
  const bin = await findBrowser()
  if (!bin) return null
  const profile = await mkdtemp(join(tmpdir(), 'nxr-wasm-smoke-'))
  const child = spawn(bin, [
    '--headless=new',
    '--no-sandbox',
    '--disable-gpu',
    '--disable-dev-shm-usage',
    `--user-data-dir=${profile}`,
    '--timeout=20000',
    '--dump-dom',
    url,
  ], { stdio: ['ignore', 'pipe', 'pipe'] })
  const killer = setTimeout(() => child.kill('SIGKILL'), 30_000)
  let dom = ''
  let err = ''
  child.stdout.on('data', (chunk) => { dom += chunk })
  child.stderr.on('data', (chunk) => { err += chunk })
  const [code] = await new Promise((done) => child.on('close', (...c) => done(c)))
  clearTimeout(killer)
  await rm(profile, { recursive: true, force: true }).catch(() => {})
  // The dump lands at quiescence, which includes the beacon; the grace wait covers
  // a browser that quit a beat before the server recorded the ping.
  await Promise.race([waitForBeacon(), new Promise((r) => setTimeout(r, 3000))])
  return { dom, driver: `${bin}${code === 0 ? '' : ` (exit ${code}: ${err.slice(0, 300)})`}` }
}


async function main() {
  const requireBrowser = process.argv.includes('--require-browser')

  // 1. The shared fake module, as both hosts serve it.
  const page1 = searchAssets(new URLSearchParams('repository=demo'))
  assert.deepEqual(page1.items.map((i) => i.path), ['app/README.md', 'app/core/lib.rs', 'app/core/lib.rs.sha256'])
  assert.equal(page1.continuationToken, '3')
  const page2 = searchAssets(new URLSearchParams('repository=demo&continuationToken=3'))
  assert.deepEqual(page2.items.map((i) => i.path), ['bom/x.bin', 'root.txt', 'root.txt.sha256'])
  assert.equal(page2.continuationToken, null)
  // The 404 arm: unknown objects, unknown repositories, empty repositories are distinct.
  assert.equal(fakeNexus('/repository/demo/absent.txt', new URLSearchParams()).status, 404)
  assert.equal(fakeNexus('/repository/nope/root.txt', new URLSearchParams()).status, 404)
  assert.deepEqual(searchAssets(new URLSearchParams('repository=nope')).items, [])
  console.log('smoke: fake module: pagination, the 404 arm and markers in search checked')

  // 2. The built wasm under plain node, against the same fake.
  const api = await startServer(true)
  try {
    // The wasm-bindgen web glue expects browser globals; node grew the Web APIs
    // (fetch, Request, AbortController) that the reqwest wasm backend needs,
    // and the two aliases below are all that is missing.
    globalThis.self ??= globalThis
    globalThis.window ??= globalThis
    const glue = await import(new URL('./pkg/nexus_raw_example_wasm_sandbox.js', import.meta.url).href)
    await glue.default({ module_or_path: await readFile(new URL('./pkg/nexus_raw_example_wasm_sandbox_bg.wasm', import.meta.url)) })
    const rows = JSON.parse(await glue.nxr_ls(`${api.url}/repository/demo/`, null))
    assert.deepEqual(rows, [
      { name: 'app', dir: true },
      { name: 'bom', dir: true },
      { name: 'root.txt', dir: false },
    ])
    // The retry path: one injected 500 must burn and still list (1 + 2 search pages).
    faults.failFirst = 1
    faults.hits = 0
    const retried = JSON.parse(await glue.nxr_ls(`${api.url}/repository/demo/`, null))
    faults.failFirst = 0
    assert.deepEqual(retried, rows)
    assert.equal(faults.hits, 3)
    // The page pings the same beacon; the smoke pings it like the page does.
    await fetch(`${api.url}/beacon?rows=${rows.length}&failed=`)
    const beacon = api.beacons.at(-1)
    assert.ok(beacon.rows > 0 && beacon.failed === '', `node run beacon: ${JSON.stringify(beacon)}`)
    console.log(`smoke: wasm under node: ${rows.length} entries, one injected 500 burned on the way`)
  } finally {
    await api.close()
  }

  // 3. The page in a headless browser, backend-free: only the worker can answer.
  const staticHost = await startServer(false)
  // The page pings exactly once; the server-side record is the sync point.
  const waitForBeacon = async () => {
    const deadline = Date.now() + 30_000
    while (staticHost.beacons.length === 0) {
      if (Date.now() > deadline) throw new Error('no beacon within 30s: the page did not finish a listing')
      await new Promise((r) => setTimeout(r, 100))
    }
  }
  try {
    const run = await runBrowser(`${staticHost.url}/`, waitForBeacon)
    if (!run) {
      if (requireBrowser) throw new Error('no chromium-family browser found (set NXR_WASM_BROWSER or NXR_WASM_DRIVER)')
      console.log('smoke: browser: no chromium-family binary found, skipping the service worker check')
      return
    }
    const beacon = staticHost.beacons.at(-1)
    assert.equal(staticHost.apiHits(), 0, 'the browser reached the (absent) backend directly: the worker did not answer')
    // The backend refused everything (apiHits counts even those), yet a listing
    // landed: only the ServiceWorker fake can have answered it.
    assert.ok(beacon && beacon.rows > 0 && beacon.failed === '', `no passing beacon: ${JSON.stringify(staticHost.beacons)}`)
    // The dom dump is a bonus, not the contract: some chromium builds never reach
    // the dump's quiescence while a service worker stays registered, and the
    // `--timeout` flag does not save those. The beacon already proves the wasm
    // listed end to end; `--require-browser` demands the rendered rows anyway.
    if (run.dom.includes('app/')) {
      console.log(`smoke: browser (${run.driver}): beacon rows=${beacon.rows} failed="" api hits=0, rows rendered, so the service worker answered`)
    } else if (requireBrowser) {
      throw new Error(`--require-browser: the page did not render the app/ row (${run.driver})`)
    } else {
      console.log(`smoke: browser: the service worker answered (rows=${beacon.rows}, api hits=0); the dom dump did not land on this chromium build (${run.driver}), the beacon carries the proof`)
    }
  } finally {
    await staticHost.close()
  }
  console.log('smoke: PASS')
}

main().catch((e) => {
  console.error(String(e))
  process.exit(1)
})
