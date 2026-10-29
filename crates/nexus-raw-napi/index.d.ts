// Hand-maintained declarations for the nexus-raw Node bindings.
// They must stay in sync with the item definitions under `crates/nexus-raw-napi/src/`.
// `binding.d.ts` is generated at build time and consumed by check-dts.mjs, it does not ship in the npm package.
//
// Every command is one self-sufficient call, like the `nxr` CLI: the base URL
// is an argument, credentials come from `auth` or the environment
// (`NXR_AUTH`, then `NXR_USERNAME` + `NXR_PASSWORD`), no config file.
// A promise rejects with an `Error` carrying `.exitCode` (0 ok, 1 data,
// 2 misuse, 3 transport) and `.hint`.

/**
 * Credentials for the `Authorization` header, curl style.
 * When absent, the environment fallback applies.
 */
export interface NxrAuth {
  user: string
  pass: string
}

/**
 * The options every command shares: transport tuning, credentials, events.
 */
export interface NxrCommonOpts {
  /** Explicit credentials. They win over the environment fallback. */
  auth?: NxrAuth
  /** Parallel artifact transfers, 1..=64, default 8. */
  workers?: number
  /** Attempts per HTTP request, default 4. */
  retry?: number
  /** TCP connect timeout in milliseconds, default 15000. */
  connectTimeoutMs?: number
  /** Fail a transfer when no bytes move for this long, default 30000. */
  stallMs?: number
  /** Skip TLS certificate verification, default false. */
  tlsInsecure?: boolean
  /** A PEM bundle of extra trust roots for a private CA. Conflicts with `tlsInsecure`. */
  caBundle?: string
  /**
   * Progress stream: the JSON-parsed events the CLI prints as NDJSON lines.
   * Events cross threads: every event has reached the callback by the time the
   * promise settles.
   * Shapes: `plan`, `artifact` (states uploading/downloading/done/skipped),
   * `retrying`, `summary`, and `removing`/`removed`/`missing` from `rm`.
   */
  onEvent?: (event: NxrEvent) => void
}

/**
 * One NDJSON event of the core, as parsed JSON.
 * `plan`/`artifact`/`retrying` come from transfers (`up`, `down`, `mirror`),
 * `removing`/`removed`/`missing` from `rm`, `summary` from everything that moves or deletes bytes.
 */
export interface NxrEvent {
  event?: 'plan' | 'artifact' | 'retrying' | 'removing' | 'removed' | 'missing' | 'warning' | 'summary'
  /** `plan` only: the name lists of the diff. */
  upload?: string[]
  download?: string[]
  skip?: string[]
  /** `artifact`, `retrying`, `removing`, `removed` and `missing` only. */
  name?: string
  /** `artifact` only: uploading, downloading, done or skipped. */
  state?: 'uploading' | 'downloading' | 'done' | 'skipped'
  /** `artifact` only: bytes moved so far. */
  done?: number
  /** `artifact` only: total bytes when known. */
  total?: number | null
  /** `retrying` only. */
  attempt?: number
  reason?: string
  /** `warning` only: rows the run had to leave out, one sentence. */
  text?: string
  /** `summary` only. */
  uploaded?: number
  downloaded?: number
  skipped?: number
  /** `summary` only: names the call deleted (`rm`). */
  removed?: number
  failed?: string[]
  /** `summary` only: the same names as `failed`, with their exit-code class. */
  failures?: NxrFailure[]
  /** `summary` only: the stream this line closes. The audit summary (`'audit'`) is CLI-only. */
  kind?: 'transfer'
}

/** One name that did not land, with the exit-code class of its error. */
export interface NxrFailure {
  name: string
  /** The exit code the failure would produce: 1 data, 2 misuse, 3 transport. */
  code: number
}

/** The final summary of a transfer or verification. */
export interface NxrSummary {
  uploaded: number
  downloaded: number
  skipped: number
  /** Local markers a `clone` rewrote from bytes that already matched the remote marker. */
  sealed: number
  /** Names the call deleted (`rm`). Transfers never delete and stay at 0. */
  removed: number
  failed: string[]
  /** The same names as `failed`, with their exit-code class. */
  failures: NxrFailure[]
}

/** One dry-run action, shaped like the CLI `--json` dry-run lines. */
export interface NxrPlanAction {
  action: 'skip' | 'upload' | 'download'
  name: string
  /** Byte size when the action transfers bytes. Absent otherwise. */
  size?: number
}

/** The plan a dry run resolves to. */
export interface NxrPlan {
  actions: NxrPlanAction[]
}

export interface NxrGetOpts extends NxrCommonOpts {
  /** Output file. Without it the body streams to the process stdout, like the CLI. */
  out?: string
  /** Resume from an existing `<out>.part` through a Range request. */
  cont?: boolean
  /** Fetch one byte range: START-END or START- (inclusive). Conflicts with `cont`. */
  range?: string
}

export interface NxrGetResult {
  /** Final size in bytes (stdout mode: bytes written). */
  size: number
  /** The digest when a file was produced (stdout streaming does not hash). Absent otherwise. */
  sha256?: string
  /** The offset the transfer resumed from (0 for a fresh download). */
  resumedFrom: number
}

export interface NxrPutOpts extends NxrCommonOpts {
  /** Also PUT `<url>`.sha256 with the sha256sum-style marker. */
  sha?: boolean
}

export interface NxrPutResult {
  size: number
  /** The uploaded digest when the sha-sibling was requested. Absent otherwise. */
  sha256?: string
}

export interface NxrHeadResult {
  status: number
  /** `Content-Length` when the answer carried one. Absent otherwise. */
  size?: number
  contentType?: string
}

export interface NxrUpOpts extends NxrCommonOpts {
  /** Restrict the transfer to these explicit names. */
  names?: string[]
  /** Restrict the transfer to these whole-segment prefixes, keeping their paths. */
  prefixes?: string[]
  /** Upload this file first, alone, before any other name (claim-first). */
  claimFirst?: string
  /** Skip marker generation and marker uploads. */
  noSha?: boolean
  /** Resolve to the plan without transferring anything (`up --plan`). */
  dryRun?: boolean
}

export interface NxrDownOpts extends NxrCommonOpts {
  /** Explicit names to download. Excludes `ls`. */
  names?: string[]
  /** Best-effort enumeration through the server search API (`--ls`). Excludes `names`: one source per call. */
  ls?: boolean
  /** Ignore existing part files: every name downloads from zero. */
  fresh?: boolean
  /** Split each artifact into this many concurrent Range spans, 2..=64. The worker budget still caps the total concurrency. */
  segments?: number
}

export interface NxrCloneOpts extends NxrCommonOpts {
  /** Where to keep the local fact cache. Default: `<dstDir>/.nxr-state.json`. */
  state?: string
  /** Disable the cache: hash every local file. */
  noState?: boolean
  /** Ignore the cache and hash every local file, even unchanged ones. */
  verify?: boolean
  /** Keep only names under these whole-segment prefixes. */
  prefixes?: string[]
}

/** One clone plan action. */
export interface NxrClonePlanAction {
  action: 'download' | 'skip' | 'seal'
  name: string
  /** Byte size when the action knows it. Absent otherwise. */
  size?: number
}

export interface NxrDiffOpts extends NxrCommonOpts {
  /** Explicit names to compare. Excludes `ls`. */
  names?: string[]
  /** Best-effort enumeration through the server search API (`--ls`). Excludes `names`: one source per call. */
  ls?: boolean
  /** Keep only names under these whole-segment prefixes. */
  prefixes?: string[]
}

export interface NxrRmOpts extends NxrCommonOpts {
  /** Explicit names to delete. Excludes `ls`. */
  names?: string[]
  /** Best-effort enumeration through the server search API (`--ls`). Excludes `names`: one source per call, so a destructive call never widens. */
  ls?: boolean
  /** Delete for real. Without it the promise resolves to the plan (`rm --yes`). */
  confirm?: boolean
}

/** One planned deletion, shaped like the CLI `rm` plan lines. */
export interface NxrRmPlanAction {
  /** `rm` when the remote copy exists and would be deleted, `missing` when it is already absent. */
  action: 'rm' | 'missing'
  name: string
  /** Byte size when the remote copy exists. Absent otherwise. */
  size?: number
}

/** The plan `rm` resolves to by default. */
export interface NxrRmPlan {
  actions: NxrRmPlanAction[]
}

/** The content facts one side holds for a name. An absent field means the fact is unknown, never that it differs. */
export interface NxrDeltaSide {
  /** The sha256 digest, when the side carries one. */
  digest?: string
  /** Byte size, when the side reports one. */
  size?: number
}

/** One delta entry, shaped like the CLI `diff --json` lines. */
export interface NxrDeltaEntry {
  /** `same`, `missing-local`, `missing-remote` or `diverged`. */
  state: 'same' | 'missing-local' | 'missing-remote' | 'diverged'
  path: string
  /** The shared digest when the state is `same`. Absent otherwise. */
  digest?: string
  /** The local-side facts when the state is `missing-remote` or `diverged`. */
  local?: NxrDeltaSide
  /** The storage-side facts when the state is `missing-local` or `diverged`. */
  remote?: NxrDeltaSide
  /** The sizes are known on both sides and differ, `diverged` only. Absent otherwise. */
  size?: boolean
  /** The digests are known on both sides and differ, `diverged` only. Absent otherwise. */
  sha?: boolean
}

/**
 * The delta report a `diff` resolves to: one entry per name in the union of
 * the local scan and the enumeration, in name order.
 */
export interface NxrDeltaReport {
  entries: NxrDeltaEntry[]
  /** How many names the report holds. */
  count: number
}

export interface NxrMirrorOpts extends NxrCommonOpts {
  /** Explicit credentials for the source. They win over `auth` and the environment fallback. */
  srcAuth?: NxrAuth
  /** Explicit credentials for the destination. They win over `auth` and the environment fallback. */
  dstAuth?: NxrAuth
  /** Explicit names to copy. Excludes `ls`. */
  names?: string[]
  /** Best-effort enumeration through the server search API (`--ls`). Excludes `names`: one source per call. */
  ls?: boolean
}

export interface NxrPointClearResult {
  /** `cleared` when the pointer existed and is deleted, `absent` when it was already gone. */
  outcome: 'cleared' | 'absent'
}

export interface NxrVerifyOpts extends NxrCommonOpts {
  /** Check exactly these explicit names. */
  names?: string[]
}

export interface NxrChannelSetResult {
  /** False when the forward-only guard kept the current token. */
  written: boolean
  /** The previous token when one was readable. Absent otherwise. */
  from?: string
  /** The kept token when the write was skipped. Absent otherwise. */
  current?: string
}

/** GET a URL to a file or the process stdout. */
export function get(url: string, opts?: NxrGetOpts): Promise<NxrGetResult>

/**
 * PUT a file path or an exact byte body, optionally with its sha-sibling.
 * A string source is a file path (like the CLI `-f`).
 * A `Uint8Array` (including a Node `Buffer`) is the exact bytes.
 */
export function put(url: string, source: string | Uint8Array, opts?: NxrPutOpts): Promise<NxrPutResult>

/** HEAD a URL: status, size, content type. */
export function head(url: string, opts?: NxrCommonOpts): Promise<NxrHeadResult>

/** The sha256 of a local file or an http(s) URL, as 64 lowercase hex chars. */
export function sha(target: string, opts?: NxrCommonOpts): Promise<string>

/**
 * Upload a local directory: verified, parallel, marker-perfect.
 * With `dryRun` the promise resolves to the plan instead of a summary.
 */
export function up(srcDir: string, dstUrl: string, opts?: NxrUpOpts): Promise<NxrSummary | NxrPlan>

/**
 * Download a remote directory into a local one.
 * The enumeration source is mandatory: explicit `names`, or `ls` when the
 * server has the search API.
 */
export function down(srcUrl: string, dstDir: string, opts?: NxrDownOpts): Promise<NxrSummary>

/**
 * Clone a repository into a local directory (`nxr clone`).
 * The optional fact cache lives under `dstDir` and skips re-hashing files
 * whose size and mtime still match the last verified copy.
 */
export function clone(srcUrl: string, dstDir: string, opts?: NxrCloneOpts): Promise<NxrSummary>

/** The clone plan: every action, nothing written (`nxr clone --plan`). */
export function clonePlan(srcUrl: string, dstDir: string, opts?: NxrCloneOpts): Promise<Array<NxrClonePlanAction>>

/**
 * Compare a local directory against a remote one: the delta report (`nxr diff`).
 * Read-only on both sides: the promise resolves to the report whatever the delta is.
 * Every scanned or enumerated name lands in exactly one entry: `same`,
 * `missing-local`, `missing-remote` or `diverged`.
 * The enumeration source is mandatory: explicit `names`, or `ls` when the
 * server has the search API.
 */
export function diff(
  localDir: string,
  remoteUrl: string,
  opts?: NxrDiffOpts,
): Promise<NxrDeltaReport>

/**
 * Delete the enumerated names from the remote directory (`rm`).
 * Divergence is never checked: `rm` deletes names, not content.
 * The enumeration source is mandatory: explicit `names`, or `ls` when the
 * server has the search API.
 * With `dryRun` the promise resolves to the plan instead of a summary.
 */
export function rm(srcUrl: string, opts?: NxrRmOpts): Promise<NxrSummary | NxrRmPlan>

/**
 * Pour enumerated names from a source repository into a destination one (`mirror`).
 * Two facades on one event stream: enumeration and bytes come from the source,
 * the diff and the writes follow the destination.
 */
export function mirror(srcUrl: string, dstUrl: string, opts?: NxrMirrorOpts): Promise<NxrSummary>

/** Check local bytes, markers and digests. No network. */
export function verify(dir: string, opts?: NxrVerifyOpts): Promise<NxrSummary>

/** Read a channel ref: the token, or null when the channel is unset. */
export function channelGet(url: string, opts?: NxrCommonOpts): Promise<string | null>

/** Write a channel token with the optional forward-only guard. */
export function channelSet(
  url: string,
  token: string,
  ifForward: boolean,
  opts?: NxrCommonOpts,
): Promise<NxrChannelSetResult>

/** DELETE a pointer file: absence is a normal outcome, like `point`. */
export function pointClear(url: string, opts?: NxrCommonOpts): Promise<NxrPointClearResult>

/** Flat artifact names under a version directory URL: the URL must point inside
 * `/repository/<name>/<group...>/<version>/`. The listing runs through the server
 * search API, and `.sha256` siblings are never returned (fetch one by name). */
export function lsAssets(url: string, opts?: NxrCommonOpts): Promise<Array<string>>

/** One repository of a Nexus server, as the service REST API reports it. */
export interface NxrRepoInfo {
  name: string
  format: string
  /** Repository kind: `hosted`, `proxy` or `group`. */
  kind: string
  url: string
}

/** List the repositories of the server behind `url` (the service REST API, not the storage protocol). */
export function serviceRepos(url: string, opts?: NxrCommonOpts): Promise<Array<NxrRepoInfo>>

/** One asset of a repository, as the search API reports it: the path, the server-side
 * digest when the server carries one, and the size. The cheapest integrity probe:
 * one call returns the whole snapshot with sha256 per file. */
export interface NxrAssetEntry {
  path: string
  url: string
  /** Asset size in bytes, when the server reports it. */
  size?: number
  /** The sha256 digest, when the server reports it. */
  sha256?: string
  /** The last modification timestamp, when the server reports it. */
  lastModified?: string
}

/** Every asset of the repository behind `url` (a repository root or anything inside it),
 * through the search API, with pagination handled: the full snapshot in one call.
 * `q` filters by substring server-side, `prefix` narrows to whole-segment path prefixes. */
export function serviceAssets(url: string, q?: string, prefix?: Array<string>, opts?: NxrCommonOpts): Promise<Array<NxrAssetEntry>>

/** One immediate child of a raw directory: a folder or a file. */
export interface NxrLsEntry {
  name: string
  /** `"dir"` when the child has a subtree below it, `"file"` when it is a leaf. */
  kind: string
}

/** The immediate children of a raw directory URL, at any tree depth: folders first, then files. */
export function lsEntries(url: string, opts?: NxrCommonOpts): Promise<Array<NxrLsEntry>>
