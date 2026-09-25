# Design

Why a client this small carries a diff, markers and an immutability rule — and what that buys.

## Completion is bytes plus a marker

Checksum catalogs, manifests and lockfiles all share one flaw: the catalog and the content are two different objects with two different lifecycles.
The sha-sibling puts the proof next to the bytes, in `sha256sum -c` format that every Unix tool already speaks.

Consequences:

- "is this artifact whole?" is one file read plus one hash, no server logic;
- a crash between PUT bytes and PUT marker leaves `Markerless` — obviously unfinished, never mistaken for done;
- a consumer verifies with `sha256sum -c` alone if it wants to.

## The diff is symmetric

The same classification table answers "what is missing here?" for both directions.
Up asks it before uploading; down asks it before downloading.
That removes the two hardest bugs of scripted artifact flows: re-uploading what is already there, and consuming a half-written file.

## PUTs are byte-idempotent

An upload is a pure function of the file content: replaying it after a break converges.
That is why resume needs no protocol — no offsets, no sessions, no server state.
The cost is bandwidth on re-transfer; the win is that correctness does not depend on the transport remembering anything.

Ranges on GET (partial download) are a deliberate v2 extension, not an omission.

## Claims are immutable

A claim names what a version contains, once.
Re-publishing a changed claim would turn "is this version whole?" into "is this version whole *for some definition published at some point*" — the exact ambiguity that makes mutable repositories untrustworthy.
Claim drift is therefore a refusal, and a corrected build ships as a new version.

## Pointers are the only mutable state

Forward-only pointers (`--if-newer`) plus immutable versions give release channels without a database: `latest` is a one-line file whose writer cannot corrupt anything older.

## The transport trusts no single signal

- Retries cover connect errors, timeouts, broken bodies and 5xx — never 4xx, where retrying would just re-authenticate a refusal.
- A stall timeout replaces a total timeout: a 100 MB artifact over a slow link may legitimately take hours, but no bytes for 30 s is a dead connection.
- Uploads detect stalls through a bounded channel: when the network stops draining the buffer, the producer notices within the stall window and the attempt restarts.
- Auth failures bypass the retry loop entirely — hammering a 401 is never useful.

## Exit codes encode the operator's decision

Data problems (exit 1) need a human; invocation problems (exit 2) need a pipeline fix; transport problems (exit 3) need another attempt later.
Everything else — NDJSON events, the summary's `failed` list — exists to make those three decisions cheap.

## What was deliberately left out

Deletion, repacking, unarchiving, a daemon, a server component.
The repository stays a dumb, verifiable object store; the client stays a static binary with two dependencies of consequence: reqwest and tokio.
