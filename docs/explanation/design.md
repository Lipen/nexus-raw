# Design

Why `nxr` is shaped like curl, and what each layer buys.

## The curl model

Nexus raw is plain HTTP, which is exactly why scripted artifact flows grow hand-rolled curl snippets: retries, stall detection, digest checks, resume — rewritten in every repository that needs them.
`nxr` replaces those scripts with one static binary whose every invocation is self-sufficient: URL in argv, credentials from `-u` or the environment, nothing else.
No config file, no profiles, no "home" server.

The cost of that model is real but bounded: repeating a long URL is the caller's problem, solved with a shell alias or a CI variable — the caller's own tooling, not a second configuration format.
The win is transferability: any machine with the binary and the environment can reproduce any call, which is what a pipeline needs and what a rc-file quietly breaks.

## The layers

Dependency discipline makes the crate embeddable at four depths, and each layer only imports downward:

| Layer | Knows about | Deliberately blind to |
|:------|:------------|:----------------------|
| L0 transport + primitives | HTTP, retries, stall, TLS, auth | directories, markers |
| L1 transfer | scans, diffs, markers, workers, Range-resume | versions, channels |
| L2 layout helpers | channels, manifests, listings | the CLI |
| L3 UX | doctor, hints, rendering | the protocol |

The CLI is a thin L3 shell.
A wrapper that wants raw PUT-with-retry uses L0.
One that wants verified directory sync without opinionated naming uses L1.

## Completion is bytes plus a marker

Checksum catalogs, manifests and lockfiles share one flaw: the catalog and the content are two different objects with two different lifecycles.
The sha-sibling puts the proof next to the bytes, in `sha256sum -c` format every Unix tool already speaks.

Consequences:

- "is this artifact whole?" is one file read plus one hash, no server logic.
- a crash between PUT bytes and PUT marker leaves `Markerless` — obviously unfinished, never mistaken for done.
- a consumer verifies with `sha256sum -c` alone if it wants to.

Markers are on by default in `up`, including local generation of missing siblings, because a tool that writes bytes should not leave them uncertified.
`--no-sha` exists so the opt-out is an explicit, greppable decision.

## The diff is symmetric

The same classification table answers "what is missing here?" for both directions.
Up asks it before uploading.
Down asks it before downloading.
That removes the two hardest bugs of scripted artifact flows: re-uploading what is already there, and consuming a half-written file.

The table is also conservative in one direction that surprises people the first time: `up` refuses to shadow a remote-only name.
Overwriting a divergent object is a decision, so the client escalates it (`mismatch`, exit 1) instead of picking a winner.

## PUTs are byte-idempotent

An upload is a pure function of the file content: replaying it after a break converges.
That is why resume needs no protocol on the upload side — no offsets, no sessions, no server state.
The cost is bandwidth on re-transfer.
The win is that correctness does not depend on the transport remembering anything.

Downloads *can* resume, because HTTP Range is already there: a `206` on `Range: bytes=N-` continues the part file.
That asymmetry — replay uploads, resume downloads — costs nothing on the server and removes the classic "start a 2 GB fetch over from scratch" failure.

## Enumeration is explicit

A raw repository has no guaranteed directory listing.
The search API varies between server releases, and HEAD-probing for likely names is guessing.
So `down` requires an enumeration source — a manifest, explicit names, or an explicitly requested best-effort `--ls` — and refuses otherwise.
The recommended convention is a `manifest.json` shipped inside each version directory, which `up` publishes like any artifact: the publisher already knows the name list, so encoding it costs one file.

This is the same philosophy as the exit codes: when the client cannot know, it says so and asks, rather than pretending.

## Channels generalize pointers

v0.2 had two reserved pointer names.
v0.3 has none.
A channel is any token file: `latest`, `stable`, `prod-1` — the meaning of a name belongs to the publisher, not the tool.
`--if-forward` compares tokens in dotted-numeric version order, which keeps the one property that mattered: a late CI job cannot roll a release back.

## The transport trusts no single signal

- Retries cover connect errors, timeouts, broken bodies and 5xx — never 4xx, where retrying would just re-authenticate a refusal.
- A stall timeout replaces a total timeout: a 100 MB artifact over a slow link may legitimately take hours, but no bytes for 30 s is a dead connection.
- Auth failures bypass the retry loop entirely — hammering a 401 is never useful.

## Exit codes encode the operator's decision

Data problems (exit 1) need a human.
Invocation problems (exit 2) need a pipeline fix.
Transport problems (exit 3) need another attempt later.
Every error carries a `hint:` line so the decision is cheap at 3 a.m.

## What was deliberately left out

Config files and profiles, deletion, repacking, unarchiving, a daemon, a server component.
The repository stays a dumb, verifiable object store.
The client stays a static binary with two dependencies of consequence: reqwest and tokio.
