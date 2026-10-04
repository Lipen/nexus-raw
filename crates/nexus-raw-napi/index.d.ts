// Hand-maintained declarations for the nexus-raw Node bindings.
// They must stay in sync with the item definitions under `crates/nexus-raw-napi/src/`;
// `binding.d.ts` next to this file is the mechanically generated surface.
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
  /** Explicit credentials; they win over the environment fallback. */
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
  /**
   * Progress stream: the JSON-parsed events the CLI prints as NDJSON lines.
   * Events cross threads; every event has reached the callback by the time the
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
  event?: 'plan' | 'artifact' | 'retrying' | 'removing' | 'removed' | 'missing' | 'summary'
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
  /** `summary` only. */
  uploaded?: number
  downloaded?: number
  skipped?: number
  /** `summary` only: names the call deleted (`rm`). */
  removed?: number
  failed?: string[]
}

/** The final summary of a transfer or verification. */
export interface NxrSummary {
  uploaded: number
  downloaded: number
  skipped: number
  /** Names the call deleted (`rm`); transfers never delete and stay at 0. */
  removed: number
  failed: string[]
}

/** One dry-run action, shaped like the CLI `--json` dry-run lines. */
export interface NxrPlanAction {
  action: 'skip' | 'upload' | 'download'
  name: string
  /** Byte size when the action transfers bytes. */
  size?: number
}

/** The plan a dry run resolves to. */
export interface NxrPlan {
  actions: NxrPlanAction[]
}

export interface NxrGetOpts extends NxrCommonOpts {
  /** Output file; without it the body streams to the process stdout, like the CLI. */
  out?: string
  /** Resume from an existing `<out>.part` through a Range request. */
  cont?: boolean
}

export interface NxrGetResult {
  /** Final size in bytes (stdout mode: bytes written). */
  size: number
  /** The digest when a file was produced (stdout streaming does not hash). */
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
  /** The uploaded digest when the sha-sibling was requested. */
  sha256?: string
}

export interface NxrHeadResult {
  status: number
  size?: number
  contentType?: string
}

export interface NxrUpOpts extends NxrCommonOpts {
  /** Restrict the transfer to these names: a manifest file, URL or `-` for stdin. */
  manifest?: string
  /** Restrict the transfer to these explicit names. */
  names?: string[]
  /** Upload this file first, alone, before any other name (claim-first). */
  claimFirst?: string
  /** Skip marker generation and marker uploads. */
  noSha?: boolean
  /** Resolve to the plan without transferring anything (`up --dry-run`). */
  dryRun?: boolean
}

export interface NxrDownOpts extends NxrCommonOpts {
  /** Enumeration source: a manifest file, URL or `-` for stdin. */
  manifest?: string
  /** Explicit names to download. */
  names?: string[]
  /** Best-effort enumeration through the server search API (`--ls`). */
  ls?: boolean
  /** Ignore existing part files: every name downloads from zero. */
  fresh?: boolean
}

export interface NxrRmOpts extends NxrCommonOpts {
  /** Enumeration source: a manifest file, URL or `-` for stdin. */
  manifest?: string
  /** Explicit names to delete. */
  names?: string[]
  /** Best-effort enumeration through the server search API (`--ls`). */
  ls?: boolean
  /** Resolve to the plan without deleting anything (`rm --dry-run`). */
  dryRun?: boolean
}

/** One planned deletion, shaped like the CLI `rm --dry-run` lines. */
export interface NxrRmPlanAction {
  /** `rm` when the remote copy exists and would be deleted, `missing` when it is already absent. */
  action: 'rm' | 'missing'
  name: string
  /** Byte size when the remote copy exists. */
  size?: number
}

/** The plan an `rm` dry run resolves to. */
export interface NxrRmPlan {
  actions: NxrRmPlanAction[]
}

export interface NxrMirrorOpts extends NxrCommonOpts {
  /** Explicit credentials for the source; they win over `auth` and the environment fallback. */
  srcAuth?: NxrAuth
  /** Explicit credentials for the destination; they win over `auth` and the environment fallback. */
  dstAuth?: NxrAuth
  /** Enumeration source at the source repository: a manifest file, URL or `-` for stdin. */
  manifest?: string
  /** Explicit names to copy. */
  names?: string[]
  /** Best-effort enumeration through the server search API (`--ls`). */
  ls?: boolean
}

export interface NxrPointClearResult {
  /** `cleared` when the pointer existed and is deleted, `absent` when it was already gone. */
  outcome: 'cleared' | 'absent'
}

export interface NxrVerifyOpts extends NxrCommonOpts {
  /** Check exactly these names: a manifest file, URL or `-` for stdin. */
  manifest?: string
  /** Check exactly these explicit names. */
  names?: string[]
}

export interface NxrChannelSetResult {
  /** False when the forward-only guard kept the current token. */
  written: boolean
  /** The previous token, when one was readable. */
  from?: string
  /** The kept token, when the write was skipped. */
  current?: string
}

/** GET a URL to a file or the process stdout. */
export function get(url: string, opts?: NxrGetOpts): Promise<NxrGetResult>

/**
 * PUT a file path or an exact byte body, optionally with its sha-sibling.
 * A string source is a file path (like the CLI `-f`); a Buffer is the exact bytes.
 */
export function put(url: string, source: string | Buffer, opts?: NxrPutOpts): Promise<NxrPutResult>

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
 * The enumeration source is mandatory: `ls`, `manifest`, explicit `names`,
 * or the conventional `manifest.json` at the directory URL.
 */
export function down(srcUrl: string, dstDir: string, opts?: NxrDownOpts): Promise<NxrSummary>

/**
 * Delete the enumerated names from the remote directory (`rm`).
 * Divergence is never checked: `rm` deletes names, not content.
 * The enumeration source is mandatory: `ls`, `manifest`, explicit `names`,
 * or the conventional `manifest.json` at the directory URL.
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

/** DELETE a pointer file: absence is a normal outcome, like `point --clear`. */
export function pointClear(url: string, opts?: NxrCommonOpts): Promise<NxrPointClearResult>

/** List the asset names the server search API reports for this directory (`ls --assets`). */
export function lsAssets(url: string, opts?: NxrCommonOpts): Promise<Array<string>>

/** List the version tokens the server search API reports for this directory (`ls`). */
export function lsVersions(url: string, opts?: NxrCommonOpts): Promise<Array<string>>

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
