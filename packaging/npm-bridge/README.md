# The unscoped bridge package

npm has no rename: `nexus-raw` (0.10.0, frozen) and `@nexus-raw/nxr` (current) are two
registries entries forever. This package is the bridge in the shape babel-core used for
its 7.0 bridge: the old name publishes again, as a thin re-export of the new one.

- `require('nexus-raw')` and `import { up } from 'nexus-raw'` resolve to the current code
  through the dependency, nothing is duplicated.
- The version moves in lockstep with the workspace: the release workflow publishes it
  right after the main package, and `scripts/version-check.sh` pins the pair.
- The import surface mirrors `crates/nexus-raw-napi/index.js` 1:1: the shim adds nothing.

The old frozen versions (< 0.17.0) are deprecated separately:
`npm deprecate nexus-raw@"<0.17.0" "renamed to @nexus-raw/nxr - install that instead"`.
