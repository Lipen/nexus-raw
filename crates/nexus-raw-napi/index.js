'use strict'

// nexus-raw Node bindings: a subset of the `nxr` surface as promises over Nexus raw storage.
// The generated loader (`binding.cjs`) picks the native addon: a local crate
// build first, then the per-platform prebuilt package.
//
// Prebuilt targets:
// - linux-x64-gnu, darwin-x64 and darwin-arm64 ship as prebuilt packages.
// - win32-x64-msvc waits on the npm spam-filter ticket: the loader falls back
//   to a local crate build there.

const raw = require('./binding.cjs')

// The raw addon appends the machine fields to the rejection message:
// a trailing `nxr:exit <code>` line and, when present, a trailing `hint: ...`
// line. They are promoted to `.exitCode` and `.hint` on a clean Error here.
const EXIT_LINE = /^nxr:exit (\d+)$/

function parseRejectionMessage(message) {
  const lines = String(message).split('\n')
  let exitCode
  let hint
  while (lines.length > 0) {
    const last = lines[lines.length - 1]
    if (hint === undefined && last.startsWith('hint: ')) {
      hint = last.slice('hint: '.length)
    } else if (exitCode === undefined) {
      const match = EXIT_LINE.exec(last)
      if (match === null) {
        break
      }
      exitCode = Number(match[1])
    } else {
      break
    }
    lines.pop()
  }
  return { message: lines.join('\n'), exitCode, hint }
}

function enrichRejection(error) {
  if (!(error instanceof Error)) {
    return error
  }
  const { message, exitCode, hint } = parseRejectionMessage(error.message)
  if (exitCode === undefined) {
    return error
  }
  const enriched = new Error(message)
  enriched.code = `NXR_EXIT_${exitCode}`
  enriched.exitCode = exitCode
  if (hint !== undefined) {
    enriched.hint = hint
  }
  return enriched
}

function withEnrichedRejection(fn) {
  return async (...args) => {
    try {
      return await fn(...args)
    } catch (err) {
      throw enrichRejection(err)
    }
  }
}

// Dot assignments, not an object literal: cjs-module-lexer only detects this
// shape, and named ESM imports (`import { up } from 'nexus-raw'`) resolve
// through it. Keep every export as its own `exports.<name> = ...` line.
exports.get = withEnrichedRejection(raw.get)
exports.put = withEnrichedRejection(raw.put)
exports.head = withEnrichedRejection(raw.head)
exports.sha = withEnrichedRejection(raw.sha)
exports.up = withEnrichedRejection(raw.up)
exports.down = withEnrichedRejection(raw.down)
exports.verify = withEnrichedRejection(raw.verify)
exports.channelGet = withEnrichedRejection(raw.channelGet)
exports.channelSet = withEnrichedRejection(raw.channelSet)
