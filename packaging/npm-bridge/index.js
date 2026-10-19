'use strict'

// The unscoped bridge: every export re-exports @nexus-raw/nxr's enriched one.
// Dot assignments, not an object literal: cjs-module-lexer only detects this
// shape, and named ESM imports (`import { up } from 'nexus-raw'`) resolve
// through it. Keep every export as its own `exports.<name> = ...` line.
// The surface mirrors crates/nexus-raw-napi/index.js 1:1: the shim adds nothing.
// `check-bridge-surface.sh` (docs CI) fails the pair on drift.
const nxr = require('@nexus-raw/nxr')

exports.get = nxr.get
exports.put = nxr.put
exports.head = nxr.head
exports.sha = nxr.sha
exports.up = nxr.up
exports.down = nxr.down
exports.clone = nxr.clone
exports.clonePlan = nxr.clonePlan
exports.diff = nxr.diff
exports.rm = nxr.rm
exports.mirror = nxr.mirror
exports.verify = nxr.verify
exports.channelGet = nxr.channelGet
exports.channelSet = nxr.channelSet
exports.pointClear = nxr.pointClear
exports.lsAssets = nxr.lsAssets
exports.serviceRepos = nxr.serviceRepos
exports.serviceAssets = nxr.serviceAssets
exports.lsEntries = nxr.lsEntries
