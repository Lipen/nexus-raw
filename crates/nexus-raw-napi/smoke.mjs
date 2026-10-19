// Smoke test for the built addon: `npm run build` first, then `node smoke.mjs`.
// It stays offline: the mapping, the summary event and the error paths all
// run through `verify` and the local `sha`, which need no network.
// The new commands are covered at the export level plus one parse-only `rm`
// rejection, and the onEvent throw contract runs through `verify`.
// `diff` is the one call that must touch a remote: it drives a throwaway
// loopback HTTP server answering 404, so the local-only name reports
// `missing-remote` without any external network.
import { createHash } from 'node:crypto'
import http from 'node:http'
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import assert from 'node:assert/strict'
import nxr from './index.js'

const dir = mkdtempSync(join(tmpdir(), 'nxr-smoke-'))
try {
  writeFileSync(join(dir, 'a.txt'), 'hello\n')
  writeFileSync(
    join(dir, 'a.txt.sha256'),
    `${createHash('sha256').update('hello\n').digest('hex')}  a.txt\n`,
  )

  // A complete directory verifies clean, and the summary event reaches the callback.
  // Events cross threads: they may land after the promise settles, so let the
  // loop drain before asserting on them.
  const flush = async () => {
    for (let i = 0; i < 8; i++) {
      await new Promise((resolve) => setImmediate(resolve))
    }
  }
  const events = []
  const summary = await nxr.verify(dir, { onEvent: (event) => events.push(event) })
  await flush()
  assert.deepEqual(summary, {
    uploaded: 0,
    downloaded: 0,
    skipped: 1,
    removed: 0,
    failed: [],
    failures: [],
  })
  assert.equal(events.length, 1)
  assert.equal(events[0].event, 'summary')
  assert.deepEqual(events[0].failed, [])
  assert.deepEqual(events[0].failures, [])

  // An incomplete artifact rejects with the data exit code and a hint, and
  // the summary event still names the failure with its exit-code class.
  writeFileSync(join(dir, 'b.txt'), 'orphan\n')
  const failedEvents = []
  await assert.rejects(
    nxr.verify(dir, { names: ['b.txt'], onEvent: (event) => failedEvents.push(event) }),
    (err) =>
      err instanceof Error &&
      err.exitCode === 1 &&
      err.code === 'NXR_EXIT_1' &&
      typeof err.hint === 'string',
  )
  await flush()
  const failedSummary = failedEvents.find((event) => event.event === 'summary')
  assert.deepEqual(failedSummary.failures, [{ name: 'b.txt', code: 1 }])

  // A bad name rejects with the misuse exit code and the grammar hint.
  await assert.rejects(
    nxr.verify(dir, { names: ['../escape'] }),
    (err) => err.exitCode === 2 && err.hint.includes('names must be relative paths'),
  )

  // Broken transport options reject as misuse.
  await assert.rejects(
    nxr.verify(dir, { workers: 0 }),
    (err) => err.exitCode === 2,
  )

  // The sha of a local file needs no client at all.
  const hex = createHash('sha256').update('hello\n').digest('hex')
  assert.equal(await nxr.sha(join(dir, 'a.txt')), hex)

  // The new exports exist and are callable: the surface is at parity with the CLI.
  for (const name of ['rm', 'mirror', 'diff', 'pointClear', 'lsAssets']) {
    assert.equal(typeof nxr[name], 'function', `${name} is exported`)
  }

  // rm parses the names before any network: a grammar violation is a misuse rejection.
  await assert.rejects(
    nxr.rm('http://127.0.0.1:9/raw/', { names: ['../escape'] }),
    (err) => err.exitCode === 2 && err.hint.includes('names must be relative paths'),
  )

  // diff resolves the report read-only: against a loopback server that answers
  // 404 to everything, the locally complete name is missing-remote.
  // Explicit names keep the enumeration offline; the probes are the only traffic.
  const absent = http.createServer((req, res) => {
    res.statusCode = 404
    res.end()
  })
  await new Promise((resolve) => absent.listen(0, '127.0.0.1', resolve))
  try {
    const report = await nxr.diff(dir, `http://127.0.0.1:${absent.address().port}/raw/`, {
      names: ['a.txt'],
    })
    // The name set is the union: the enumeration names a.txt, the scan also finds the orphan b.txt.
    assert.equal(report.count, 2)
    const byPath = new Map(report.entries.map((entry) => [entry.path, entry]))
    assert.equal(byPath.get('a.txt').state, 'missing-remote')
    assert.equal(byPath.get('b.txt').state, 'missing-remote')
    assert.equal(byPath.get('a.txt').local.digest, hex)
    assert.equal(byPath.get('a.txt').local.size, 6)
  } finally {
    absent.close()
    absent.closeAllConnections()
  }

  // put accepts a Node Buffer and a plain Uint8Array: the byte branch stages
  // through a temp file, so the loopback sink proves both runtime shapes.
  const putBodies = []
  const sink = http.createServer((req, res) => {
    if (req.method !== 'PUT') {
      res.statusCode = 201
      res.end()
      return
    }
    const chunks = []
    req.on('data', (chunk) => chunks.push(chunk))
    req.on('end', () => {
      putBodies.push(Buffer.concat(chunks))
      res.statusCode = 201
      res.end()
    })
  })
  await new Promise((resolve) => sink.listen(0, '127.0.0.1', resolve))
  try {
    const base = `http://127.0.0.1:${sink.address().port}/raw/`
    const fromBuffer = Buffer.from('buffer bytes\n')
    const fromArray = new Uint8Array([0, 1, 2, 255])
    await nxr.put(`${base}buffer.bin`, fromBuffer)
    await nxr.put(`${base}array.bin`, fromArray)
    assert.deepEqual(putBodies[0], fromBuffer)
    assert.deepEqual(putBodies[1], Buffer.from(fromArray))
  } finally {
    sink.close()
    sink.closeAllConnections()
  }

  // A throw inside onEvent rejects the command promise instead of crashing the process.
  // The command itself succeeds here, so the rejection carries the callback failure only.
  await assert.rejects(
    nxr.verify(dir, { names: ['a.txt'], onEvent: () => { throw new Error('callback boom') } }),
    (err) =>
      err instanceof Error &&
      err.exitCode === undefined &&
      err.message.includes('onEvent callback failed') &&
      err.message.includes('callback boom'),
  )

  // The command failing and the callback throwing: the exit code and hint stay the command's.
  await assert.rejects(
    nxr.verify(dir, { names: ['b.txt'], onEvent: () => { throw new Error('callback boom') } }),
    (err) =>
      err.exitCode === 1 &&
      typeof err.hint === 'string' &&
      err.message.includes('onEvent callback also failed'),
  )

  console.log('smoke ok')
} finally {
  rmSync(dir, { recursive: true, force: true })
}
