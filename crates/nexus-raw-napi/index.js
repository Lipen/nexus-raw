'use strict'

// nexus-raw Node bindings: the `nxr` surface as promises over Nexus raw storage.
// The generated loader (`binding.cjs`) picks the native addon: a local crate
// build first, then the per-platform prebuilt package.
//
// Prebuilt targets:
// - linux-x64-gnu is wired in package.json ("napi"."targets").
// - Pending runners, wired in when a release runner exists (no prebuilt
//   packages until then, the loader asks for a local build instead):
//   darwin-x64, darwin-arm64, win32-x64-msvc.

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

module.exports = {
  get: withEnrichedRejection(raw.get),
  put: withEnrichedRejection(raw.put),
  head: withEnrichedRejection(raw.head),
  sha: withEnrichedRejection(raw.sha),
  up: withEnrichedRejection(raw.up),
  down: withEnrichedRejection(raw.down),
  verify: withEnrichedRejection(raw.verify),
  channelGet: withEnrichedRejection(raw.channelGet),
  channelSet: withEnrichedRejection(raw.channelSet),
}
