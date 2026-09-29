// Drift guard for the hand-maintained index.d.ts.
// The napi CLI regenerates binding.d.ts from the Rust source on every build;
// index.d.ts is maintained by hand (its header says so) and must describe the
// same surface. Run after a build: `node check-dts.mjs` (CI does).
//
// The two files intentionally differ in form, so the comparison is
// normalized, not textual:
// - the generated optional options carry ` | undefined | null`;
// - the generated option interfaces flatten what index.d.ts inherits
//   through `extends NxrCommonOpts`;
// - `export declare function` vs `export function`, whitespace, wrapping.
// One-sided exports are allow-listed: NxrEvent is hand-documented only,
// __napiBindingTarget is loader machinery the CLI adds.
//
// Granularity: function names and full signatures, interface names, and the
// field names of every interface. A renamed parameter or a dropped field
// fails; a pure type rewording inside one field may not.

import { readFileSync } from 'node:fs'

const INDEX_ONLY_INTERFACES = new Set(['NxrEvent'])
const BINDING_ONLY_EXPORTS = new Set(['__napiBindingTarget'])

const stripComments = (s) => s.replaceAll(/\/\*[\s\S]*?\*\//g, '').replaceAll(/^\s*\/\/.*$/gm, '')
// `X | undefined | null` on an optional option is the same optional type;
// strip the exact sequence, keep meaningful unions like `string | Buffer`.
// A multi-line param list may end in a trailing comma before the `)`.
const normType = (s) => s.replace(/\s+/g, '').replaceAll('|undefined|null', '').replace(/,$/, '')

function parseFunctions(src) {
  const fns = new Map()
  const re = /export\s+(?:declare\s+)?function\s+(\w+)\s*\(([^)]*)\)\s*:\s*([^;\n]+)/g
  for (const [, name, params, ret] of src.matchAll(re)) {
    fns.set(name, `${name}(${normType(params)}):${normType(ret)}`)
  }
  return fns
}

function parseInterfaces(src) {
  const ifaces = new Map()
  const re = /export\s+interface\s+(\w+)(?:\s+extends\s+([\w\s,]+?))?\s*\{([^}]*)\}/g
  for (const [, name, parents, body] of src.matchAll(re)) {
    const fields = new Set()
    for (const m of body.matchAll(/(^|\n)\s*(\w+)\s*\??\s*:/g)) fields.add(m[2])
    ifaces.set(name, {
      parents: parents ? parents.split(',').map((p) => p.trim()).filter(Boolean) : [],
      fields,
    })
  }
  return ifaces
}

function effectiveFields(ifaces, name, seen = new Set()) {
  const iface = ifaces.get(name)
  if (!iface || seen.has(name)) return new Set()
  seen.add(name)
  const fields = new Set(iface.fields)
  for (const parent of iface.parents) {
    for (const field of effectiveFields(ifaces, parent, seen)) fields.add(field)
  }
  return fields
}

function load(path) {
  try {
    return stripComments(readFileSync(new URL(path, import.meta.url), 'utf8'))
  } catch {
    console.error(`check-dts: cannot read ${path}; build the addon first (npm run build:debug)`)
    process.exit(1)
  }
}

const indexSrc = load('./index.d.ts')
const bindingSrc = load('./binding.d.ts')
const problems = []

const indexFns = parseFunctions(indexSrc)
const bindingFns = parseFunctions(bindingSrc)
for (const [name, sig] of indexFns) {
  if (!bindingFns.has(name)) problems.push(`function ${name} exists only in index.d.ts`)
  else if (bindingFns.get(name) !== sig) {
    problems.push(`signature drift on ${name}:\n  index:   ${sig}\n  binding: ${bindingFns.get(name)}`)
  }
}
for (const name of bindingFns.keys()) {
  if (!indexFns.has(name)) problems.push(`function ${name} missing from index.d.ts`)
}

const indexIfaces = parseInterfaces(indexSrc)
const bindingIfaces = parseInterfaces(bindingSrc)
for (const name of INDEX_ONLY_INTERFACES) indexIfaces.delete(name)
for (const name of BINDING_ONLY_EXPORTS) bindingIfaces.delete(name)
for (const [name] of indexIfaces) {
  if (!bindingIfaces.has(name)) problems.push(`interface ${name} exists only in index.d.ts`)
}
for (const [name] of bindingIfaces) {
  if (!indexIfaces.has(name)) problems.push(`interface ${name} missing from index.d.ts`)
}
for (const name of indexIfaces.keys()) {
  if (!bindingIfaces.has(name)) continue
  const mine = effectiveFields(indexIfaces, name)
  const theirs = effectiveFields(bindingIfaces, name)
  for (const field of mine) {
    if (!theirs.has(field)) problems.push(`interface ${name}: field ${field} missing from binding.d.ts`)
  }
  for (const field of theirs) {
    if (!mine.has(field)) problems.push(`interface ${name}: field ${field} missing from index.d.ts`)
  }
}

if (problems.length > 0) {
  console.error(`check-dts: index.d.ts drifted from the generated binding.d.ts:\n`)
  for (const problem of problems) console.error(`- ${problem}`)
  process.exit(1)
}
console.log('dts ok: index.d.ts matches the generated binding.d.ts')
