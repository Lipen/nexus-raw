// Smoke test for the built addon: `npm run build` first, then `node smoke.mjs`.
// It stays offline: the mapping, the summary event and the error paths all
// run through `verify` and the local `sha`, which need no network.
import { createHash } from 'node:crypto'
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
  assert.deepEqual(summary, { uploaded: 0, downloaded: 0, skipped: 1, failed: [] })
  assert.equal(events.length, 1)
  assert.equal(events[0].event, 'summary')
  assert.deepEqual(events[0].failed, [])

  // An incomplete artifact rejects with the data exit code and a hint.
  writeFileSync(join(dir, 'b.txt'), 'orphan\n')
  await assert.rejects(
    nxr.verify(dir, { names: ['b.txt'] }),
    (err) =>
      err instanceof Error &&
      err.exitCode === 1 &&
      err.code === 'NXR_EXIT_1' &&
      typeof err.hint === 'string',
  )

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

  console.log('smoke ok')
} finally {
  rmSync(dir, { recursive: true, force: true })
}
