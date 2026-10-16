#!/usr/bin/env node
'use strict'

// The bin wrapper installed as `nxr` by the npm packages: resolve this
// platform's prebuilt package, spawn the real binary with every argument
// and stdio passed through. No node re-exec, no buffering: the Rust process
// is the whole show, signals and exit codes travel as-is.

const { spawn } = require('node:child_process')
const path = require('node:path')

// linux carries one static musl binary per arch, so the gnu-suffixed
// package serves every libc; darwin and win32 map one to one.
const table = {
  'linux-x64': '@nexus-raw/nxr-linux-x64-gnu',
  'linux-arm64': '@nexus-raw/nxr-linux-arm64-gnu',
  'darwin-x64': '@nexus-raw/nxr-darwin-x64',
  'darwin-arm64': '@nexus-raw/nxr-darwin-arm64',
  'win32-x64': '@nexus-raw/nxr-win32-x64-msvc',
}

const key = `${process.platform}-${process.arch}`
const pkgName = table[key]

if (!pkgName) {
  console.error(`nxr: no prebuilt binary for ${key}.`)
  console.error('Install from https://github.com/Lipen/nexus-raw/releases, or build from source: cargo install nexus-raw')
  process.exit(2)
}

let pkgRoot
try {
  pkgRoot = path.dirname(require.resolve(`${pkgName}/package.json`))
} catch {
  console.error(`nxr: the platform package ${pkgName} is not installed.`)
  console.error('Reinstall @nexus-raw/nxr, or install from https://github.com/Lipen/nexus-raw/releases')
  process.exit(2)
}

const exe = path.join(pkgRoot, 'bin', process.platform === 'win32' ? 'nxr.exe' : 'nxr')

const child = spawn(exe, process.argv.slice(2), { stdio: 'inherit' })
child.on('error', (err) => {
  console.error(`nxr: cannot run ${exe}: ${err.message}`)
  process.exit(2)
})
child.on('exit', (code, signal) => {
  if (signal !== undefined && signal !== null) {
    process.kill(process.pid, signal)
  } else {
    process.exit(code ?? 0)
  }
})
