# Design

This page records the decisions behind `nxr` and the alternatives they beat.
The exact wire behavior lives in the [protocol reference](../reference/protocol.md), the commands in the [CLI reference](../reference/cli.md), and the test evidence behind every guarantee in [Conformance](conformance.md).

## The curl model

Scripted artifact flows grow hand-rolled curl snippets: retries, stall detection, digest checks, resume, rewritten in every repository that needs them.
`nxr` replaces those snippets with one binary whose every invocation is self-sufficient: URL in argv, credentials from `-u` or the environment, nothing else.

| Concern | The hand-rolled script | `nxr` |
| :-- | :-- | :-- |
| Retries with backoff | a local loop | built in, transport-level |
| Stalled connections | `--speed-limit` and `--speed-time`, set by hand, forgotten in half the scripts | per-connection stall timeout, on by default |
| Partial artifacts | temp files and `mv`, or the partial survives a crash | hidden part files, rename only after verify |
| Digest check | a second fetch piped to `sha256sum -c` | checked against the remote marker while streaming |
| Markers | written by hand, or forgotten | generated from the bytes, written after them |
| Resume after a break | `curl -C -`, one URL at a time, digest unchecked | Range-resume by default, stable part files per name, digest re-checked |
| What failed | an exit code, if you are lucky | 1 data / 2 misuse / 3 transport, each with a `hint:` line |
| Re-running | re-downloads everything | diffs first, transfers only what is missing |

Config files were rejected for one reason: transferability.
A pipeline must be able to reproduce any call from any machine that has the binary and the environment.
An rc-file quietly breaks that property, because the call now depends on a state the URL does not carry.
The cost is bounded: repeating a long URL is the caller's problem, solved with a shell alias or a CI variable, which is the caller's own tooling rather than a second configuration format.

## Four layers, dependencies point one way

The crate is embeddable at four depths.
Dependencies point one way, from UX down to transport, and no layer ever imports upward.

<figure>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 720 472" role="img" aria-labelledby="dg-layers-t dg-layers-d" font-family="inherit">
<title id="dg-layers-t">The four layers of nxr and their one-way dependencies</title>
<desc id="dg-layers-d">L3 UX depends on L2 layout, which depends on L1 transfer, which depends on L0 transport. Each layer is blind to everything above it.</desc>
<defs>
<marker id="dg-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
<path d="M0,1 L9,5 L0,9 Z" class="dg-head"/>
</marker>
</defs>
<style>
.dg-card { fill: var(--md-default-bg-color); stroke: var(--dg-line); stroke-width: 1.5; }
.dg-chip { fill: var(--dg-accent); }
.dg-ico { stroke: var(--md-default-bg-color); stroke-width: 2; fill: none; stroke-linecap: round; stroke-linejoin: round; }
.dg-name { font-size: 15px; font-weight: 700; fill: var(--dg-ink); }
.dg-knows { font-size: 12px; fill: var(--dg-muted); }
.dg-blind { font-size: 11.5px; font-style: italic; fill: var(--dg-accent); }
.dg-link { stroke: var(--dg-muted); stroke-width: 2; fill: none; }
.dg-head { fill: var(--dg-muted); }
.dg-axis { font-size: 11px; font-weight: 700; fill: var(--dg-muted); letter-spacing: 0.1em; }
.dg-dep { font-size: 11px; fill: var(--dg-muted); }
</style>
<!-- header axis -->
<text x="64" y="22" class="dg-axis">DEPENDENCIES POINT DOWN</text>
<!-- the central spine: one straight line, midline x=360, from L3 bottom into L0 top -->
<!-- L3 card: x=64..656, y=32..128 -->
<g filter="url(#dg-shadow)">
<rect x="64" y="32" width="592" height="96" rx="12" class="dg-card"/>
</g>
<rect x="84" y="56" width="48" height="48" rx="12" class="dg-chip"/>
<g transform="translate(84,56)" class="dg-ico"><g transform="translate(12,12)">
<path d="M4 18h16M7 18v-8m5 8V5m5 13v-5"/>
</g></g>
<text x="152" y="60" class="dg-name">L3 · UX (the CLI)</text>
<text x="152" y="80" class="dg-knows">doctor · hints · NDJSON · exit codes</text>
<text x="152" y="102" class="dg-blind">may not speak HTTP</text>
<!-- link L3 -> L2: spine x=360, card gap 128..152 -->
<line x1="360" y1="128" x2="360" y2="152" class="dg-link" marker-end="url(#dg-arrow)"/>
<text x="376" y="145" class="dg-dep">imports L2</text>
<!-- L2 card: y=152..240 -->
<g filter="url(#dg-shadow)">
<rect x="64" y="152" width="592" height="88" rx="12" class="dg-card"/>
</g>
<rect x="84" y="172" width="48" height="48" rx="12" class="dg-chip"/>
<g transform="translate(84,172)" class="dg-ico"><g transform="translate(12,12)">
<path d="M9 4h6m-6 8h12m-12 8h8"/><circle cx="4" cy="4" r="1.8"/><circle cx="4" cy="12" r="1.8"/><circle cx="4" cy="20" r="1.8"/>
</g></g>
<text x="152" y="176" class="dg-name">L2 · layout helpers</text>
<text x="152" y="196" class="dg-knows">channels · listings · pointers</text>
<text x="152" y="218" class="dg-blind">may not know the CLI exists</text>
<!-- link L2 -> L1 -->
<line x1="360" y1="240" x2="360" y2="264" class="dg-link" marker-end="url(#dg-arrow)"/>
<text x="376" y="257" class="dg-dep">imports L1</text>
<!-- L1 card: y=264..352 -->
<g filter="url(#dg-shadow)">
<rect x="64" y="264" width="592" height="88" rx="12" class="dg-card"/>
</g>
<rect x="84" y="284" width="48" height="48" rx="12" class="dg-chip"/>
<g transform="translate(84,284)" class="dg-ico"><g transform="translate(12,12)">
<path d="M12 3v11m0 0 4-4m-4 4-4-4M4 16v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3"/>
</g></g>
<text x="152" y="288" class="dg-name">L1 · transfer</text>
<text x="152" y="308" class="dg-knows">sync up/down · symmetric diff · markers · Range-resume · workers</text>
<text x="152" y="330" class="dg-blind">may not know versions or channels exist</text>
<!-- link L1 -> L0 -->
<line x1="360" y1="352" x2="360" y2="376" class="dg-link" marker-end="url(#dg-arrow)"/>
<text x="376" y="369" class="dg-dep">imports L0</text>
<!-- L0 card: y=376..464 -->
<g filter="url(#dg-shadow)">
<rect x="64" y="376" width="592" height="88" rx="12" class="dg-card"/>
</g>
<rect x="84" y="396" width="48" height="48" rx="12" class="dg-chip"/>
<g transform="translate(84,396)" class="dg-ico"><g transform="translate(12,12)">
<path d="M3 11h18M9 11V7m6 4V7"/><circle cx="4" cy="18" r="1.8"/><circle cx="12" cy="18" r="1.8"/><circle cx="20" cy="18" r="1.8"/>
</g></g>
<text x="152" y="400" class="dg-name">L0 · transport + primitives</text>
<text x="152" y="420" class="dg-knows">retries · stall · TLS · auth · get/put/head/sha</text>
<text x="152" y="442" class="dg-blind">may not know directories or markers exist</text>
<filter id="dg-shadow" x="-10%" y="-10%" width="120%" height="135%">
<feGaussianBlur in="SourceAlpha" stdDeviation="3"/>
<feOffset dy="2"/>
<feComponentTransfer><feFuncA type="linear" slope="0.10"/></feComponentTransfer>
<feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
</filter>
</svg>
</figure>

Each layer is blind on purpose.
L0 knows HTTP, retries, stall detection, TLS and auth, and does not know that directories or markers exist.
L1 knows scans, diffs, markers, workers and Range-resume, and does not know that versions or channels exist.
L2 knows channels and listings, and does not know the CLI exists.
L3 is the only layer allowed to talk to a human.

The point of the blindness: a wrapper that wants raw PUT-with-retry embeds L0 and nothing above it.
One that wants verified directory sync without opinionated naming embeds L1.
Both enter through the [Rust facade](../reference/api.md).

## Completion is bytes plus a marker

Checksum catalogs, name lists and lockfiles share one flaw: the catalog and the content are two objects with two lifecycles, and every divergence between them is a bug class.
The sha-sibling puts the proof next to the bytes, in `sha256sum -c` format Unix tools already speak.
A name is complete exactly when bytes and sibling both exist and the digest matches.

Markers are mandatory by default because a tool that writes bytes should not leave them uncertified.
A Markerless object is unverifiable, which makes it exactly as useful as a missing object, only harder to notice.
`up` generates missing local siblings before uploading and writes the remote sibling strictly after the bytes.
`--no-sha` makes the opt-out an explicit, greppable decision recorded in the command line instead of a default anyone gets by accident.
The result of that flag is Markerless objects the server never certifies: a deliberate downgrade, and the tests pin that it stays one.

## One diff, two directions, refusals included

The same classification table answers "what is missing here?" for both directions: up asks before sending, down asks before receiving.
Four states per side (Absent, Markerless, Broken, Complete) remove the two hardest bugs of scripted artifact flows: re-uploading what is already there, and consuming a half-written file.
States are per side, and the decision is about the pair.
The full table is in the [protocol reference](../reference/protocol.md).

<figure>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 760 300" role="img" aria-labelledby="dg-state-t dg-state-d" font-family="inherit">
<title id="dg-state-t">The four states of one name and the refused transitions</title>
<desc id="dg-state-d">A name moves from Absent through Markerless to Complete. Complete and Markerless decay to Broken through tampering or foreign markers. Refused transfers self-loop on Broken and Complete with exit 1.</desc>
<defs>
<marker id="dg-arrow2" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
<path d="M0,1 L9,5 L0,9 Z" class="dg-head2"/>
</marker>
</defs>
<style>
.dg-st { fill: var(--md-default-bg-color); stroke: var(--dg-line); stroke-width: 1.5; }
.dg-st-good { stroke: var(--dg-ok); fill: color-mix(in oklch, var(--dg-ok) 10%, var(--md-default-bg-color)); }
.dg-st-refuse { stroke: var(--dg-err); fill: color-mix(in oklch, var(--dg-err) 8%, var(--md-default-bg-color)); }
.dg-st-name { font-size: 14px; font-weight: 700; fill: var(--dg-ink); }
.dg-st-sub { font-size: 11px; fill: var(--dg-muted); }
.dg-edge { font-size: 11px; fill: var(--dg-muted); }
.dg-ln { stroke: var(--dg-muted); stroke-width: 1.5; fill: none; }
.dg-ln-soft { stroke: var(--dg-faint); stroke-width: 1.5; fill: none; }
.dg-head2 { fill: var(--dg-muted); }
.dg-plate { fill: var(--md-default-bg-color); }
</style>
<!-- states on one axis, y=110..170, centers x=100/300/500/700 -->
<g filter="url(#dg-shadow2)">
<rect x="28" y="110" width="144" height="60" rx="30" class="dg-st"/>
</g>
<text x="100" y="133" text-anchor="middle" class="dg-st-name">Absent</text>
<text x="100" y="151" text-anchor="middle" class="dg-st-sub">no bytes on this side</text>
<g filter="url(#dg-shadow2)">
<rect x="228" y="110" width="144" height="60" rx="30" class="dg-st"/>
</g>
<text x="300" y="133" text-anchor="middle" class="dg-st-name">Markerless</text>
<text x="300" y="151" text-anchor="middle" class="dg-st-sub">bytes, no marker yet</text>
<g filter="url(#dg-shadow2)">
<rect x="428" y="110" width="144" height="60" rx="30" class="dg-st-good"/>
</g>
<text x="500" y="133" text-anchor="middle" class="dg-st-name">Complete</text>
<text x="500" y="151" text-anchor="middle" class="dg-st-sub">digest matches</text>
<g filter="url(#dg-shadow2)">
<rect x="628" y="110" width="128" height="60" rx="30" class="dg-st-refuse"/>
</g>
<text x="692" y="133" text-anchor="middle" class="dg-st-name">Broken</text>
<text x="692" y="151" text-anchor="middle" class="dg-st-sub">marker disagrees</text>
<!-- entry: start dot into Absent -->
<circle cx="8" cy="140" r="5" class="dg-chip"/>
<line x1="13" y1="140" x2="28" y2="140" class="dg-ln" marker-end="url(#dg-arrow2)"/>
<!-- forward path: Absent -> Markerless -> Complete -->
<line x1="172" y1="140" x2="228" y2="140" class="dg-ln" marker-end="url(#dg-arrow2)"/>
<g transform="translate(200,96)"><rect x="-34" y="-9" width="68" height="18" class="dg-plate"/><text text-anchor="middle" dominant-baseline="central" font-size="11" class="dg-edge">bytes land</text></g>
<line x1="372" y1="140" x2="428" y2="140" class="dg-ln" marker-end="url(#dg-arrow2)"/>
<g transform="translate(400,96)"><rect x="-42" y="-9" width="84" height="18" class="dg-plate"/><text text-anchor="middle" dominant-baseline="central" font-size="11" class="dg-edge">marker follows</text></g>
<!-- clean transfer: Absent arcs over the top to Complete -->
<path d="M 100,110 L 100,64 Q 100,52 112,52 L 488,52 Q 500,52 500,64 L 500,110" class="dg-ln" marker-end="url(#dg-arrow2)"/>
<g transform="translate(300,52)"><rect x="-46" y="-9" width="92" height="18" class="dg-plate"/><text text-anchor="middle" dominant-baseline="central" font-size="11" class="dg-edge">clean transfer</text></g>
<!-- decay: Complete -> Broken, Markerless -> Broken (two parallel horizontals at y=140; second lane routed below, y=200, rounded) -->
<line x1="572" y1="140" x2="628" y2="140" class="dg-ln" marker-end="url(#dg-arrow2)"/>
<g transform="translate(600,96)"><rect x="-38" y="-9" width="76" height="18" class="dg-plate"/><text text-anchor="middle" dominant-baseline="central" font-size="11" class="dg-edge">tampering</text></g>
<path d="M 340,170 L 340,196 Q 340,208 352,208 L 680,208 Q 692,208 692,196 L 692,170" class="dg-ln" marker-end="url(#dg-arrow2)"/>
<g transform="translate(516,208)"><rect x="-56" y="-9" width="112" height="18" class="dg-plate"/><text text-anchor="middle" dominant-baseline="central" font-size="11" class="dg-edge">foreign marker</text></g>
<!-- refusals: self loops on top of Complete and Broken -->
<path d="M 460,110 C 460,56 540,56 540,110" class="dg-ln-soft" stroke-dasharray="5 4" marker-end="url(#dg-arrow2)"/>
<g transform="translate(500,50)"><rect x="-88" y="-9" width="176" height="18" class="dg-plate"/><text text-anchor="middle" dominant-baseline="central" font-size="11" class="dg-edge">divergent refused, exit 1</text></g>
<path d="M 656,110 C 656,20 728,20 728,52 Q 728,60 720,60 L 704,60 Q 692,60 692,72 L 692,110" class="dg-ln-soft" stroke-dasharray="5 4" marker-end="url(#dg-arrow2)"/>
<g transform="translate(692,34)"><rect x="-60" y="-9" width="120" height="18" class="dg-plate"/><text text-anchor="middle" dominant-baseline="central" font-size="11" class="dg-edge">transfer refused, exit 1</text></g>
<filter id="dg-shadow2" x="-10%" y="-10%" width="120%" height="135%">
<feGaussianBlur in="SourceAlpha" stdDeviation="3"/>
<feOffset dy="2"/>
<feComponentTransfer><feFuncA type="linear" slope="0.10"/></feComponentTransfer>
<feMerge><feMergeNode/><feMergeNode in="SourceGraphic"/></feMerge>
</filter>
</svg>
</figure>


The gap between `Absent` and `Markerless` is the crash window: the bytes landed, the marker write is still ahead.

The table is conservative in a direction that surprises people once: divergent objects are never overwritten.
A local Complete and a remote Complete with different digests is a mismatch: the transfer refuses and escalates the decision to a human with exit 1.
The client does not pick a winner because it cannot know which side is the truth, and an artifact store is the wrong place to find out.
The same refusal protects a Broken marker on either side, because a foreign object is never shadowed.

Mirror-delete was rejected for the same reason.
A remote-only name found by `up` is a mismatch, not a to-do: acting on it would mean deleting on a server's behalf, and a client that deletes is a different tool.
As far as this client is concerned, the repository stays append-only.

## The core is resume, and resume is asymmetric on purpose

An upload is a pure function of the file content: replaying it after a break converges on the same result.
That is why uploads need no resume protocol (no offsets, no sessions, no server state), and why the cost is bandwidth on re-transfer.
Correctness never depends on the transport remembering anything.

Downloads can and do resume, because HTTP Range is already there.
`down` keeps each name's bytes in a stable part file (`.nxr-part-<hash>`) by default, sends `Range: bytes=N-` when the part holds N bytes, appends on `206`, and restarts from zero on a `200`, the answer of a server that ignored the range.
A resumed part that belongs to an older remote version fails the digest check and is discarded once, and the name restarts from zero under the same check, so a rerun self-heals after a remote update instead of refusing.
`--fresh` starts every name over, and only a fresh download that still diverges refuses with exit 1.
The asymmetry that remains is deliberate: the `get` primitive resumes only under `--continue`, because a primitive trusts the caller to know its part files, while `down` resumes by default, because a rerun of a directory transfer should converge.

<figure>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 720 560" role="img" aria-labelledby="dg-seq-t dg-seq-d" font-family="inherit">
<title id="dg-seq-t">How down resumes a broken download</title>
<desc id="dg-seq-d">nxr down reads the marker, sends a range request, and either finalizes a complete part on 416 or appends on 206, then verifies the digest before rename.</desc>
<style>
.dg-life { stroke: var(--dg-faint); stroke-width: 1.5; }
.dg-msg { stroke: var(--dg-muted); stroke-width: 1.5; fill: none; }
.dg-msg-r { stroke: var(--dg-muted); stroke-width: 1.5; fill: none; stroke-dasharray: 6 4; }
.dg-head3 { fill: var(--dg-muted); }
.dg-actor { fill: var(--md-default-bg-color); stroke: var(--dg-line); stroke-width: 1.5; }
.dg-actor-t { font-size: 14px; font-weight: 700; fill: var(--dg-ink); }
.dg-msg-t { font-size: 12px; fill: var(--dg-ink); }
.dg-note { fill: color-mix(in oklch, var(--dg-accent) 12%, var(--md-default-bg-color)); stroke: color-mix(in oklch, var(--dg-accent) 35%, transparent); stroke-width: 1; }
.dg-note-t { font-size: 11.5px; fill: var(--dg-accent); }
.dg-alt { stroke: var(--dg-faint); stroke-width: 1; fill: none; }
.dg-alt-t { font-size: 11.5px; font-weight: 700; fill: var(--dg-muted); letter-spacing: 0.06em; }
.dg-phase { fill: color-mix(in oklch, var(--md-default-fg-color) 4%, transparent); stroke: none; }
.dg-phase-t { font-size: 11px; font-weight: 700; fill: var(--dg-muted); letter-spacing: 0.08em; }
</style>
<defs>
<marker id="dg-arrow3" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
<path d="M0,1 L9,5 L0,9 Z" class="dg-head3"/>
</marker>
</defs>
<!-- actors -->
<rect x="80" y="16" width="160" height="36" rx="18" class="dg-actor"/>
<text x="160" y="34" text-anchor="middle" dominant-baseline="central" class="dg-actor-t">nxr down</text>
<rect x="480" y="16" width="160" height="36" rx="18" class="dg-actor"/>
<text x="560" y="34" text-anchor="middle" dominant-baseline="central" class="dg-actor-t">Nexus raw</text>
<!-- lifelines -->
<line x1="160" y1="52" x2="160" y2="540" class="dg-life"/>
<line x1="560" y1="52" x2="560" y2="540" class="dg-life"/>
<!-- phase band: probe -->
<rect x="24" y="76" width="672" height="112" class="dg-phase"/>
<text x="32" y="92" class="dg-phase-t">PROBE</text>
<line x1="160" y1="108" x2="560" y2="108" class="dg-msg" marker-end="url(#dg-arrow3)"/>
<text x="356" y="100" text-anchor="middle" class="dg-msg-t">HEAD 1.0.0/a.zip</text>
<line x1="560" y1="132" x2="160" y2="132" class="dg-msg-r" marker-end="url(#dg-arrow3)"/>
<text x="364" y="124" text-anchor="middle" class="dg-msg-t">200 OK, Content-Length</text>
<line x1="160" y1="160" x2="560" y2="160" class="dg-msg" marker-end="url(#dg-arrow3)"/>
<text x="356" y="152" text-anchor="middle" class="dg-msg-t">GET 1.0.0/a.zip.sha256</text>
<line x1="560" y1="180" x2="160" y2="180" class="dg-msg-r" marker-end="url(#dg-arrow3)"/>
<text x="364" y="172" text-anchor="middle" class="dg-msg-t">200 OK, digest line</text>
<!-- phase band: resume the part: two alternatives, not a sequence -->
<rect x="24" y="196" width="672" height="200" class="dg-phase"/>
<text x="32" y="212" class="dg-phase-t">RESUME THE PART</text>
<rect x="176" y="222" width="240" height="22" rx="6" class="dg-note"/>
<text x="296" y="233" text-anchor="middle" dominant-baseline="central" class="dg-note-t">part file holds N bytes</text>
<text x="32" y="262" class="dg-alt-t">IF THE PART HELD EVERYTHING</text>
<line x1="160" y1="276" x2="560" y2="276" class="dg-msg" marker-end="url(#dg-arrow3)"/>
<text x="356" y="268" text-anchor="middle" class="dg-msg-t">GET …, Range: bytes=N-</text>
<line x1="560" y1="300" x2="160" y2="300" class="dg-msg-r" marker-end="url(#dg-arrow3)"/>
<text x="364" y="292" text-anchor="middle" class="dg-msg-t">416 · yes</text>
<rect x="176" y="312" width="280" height="22" rx="6" class="dg-note"/>
<text x="316" y="323" text-anchor="middle" dominant-baseline="central" class="dg-note-t">finalize, no bytes re-fetched</text>
<text x="32" y="354" class="dg-alt-t">IF A PARTIAL PREFIX</text>
<line x1="160" y1="368" x2="560" y2="368" class="dg-msg" marker-end="url(#dg-arrow3)"/>
<text x="356" y="360" text-anchor="middle" class="dg-msg-t">GET …, Range: bytes=N-</text>
<line x1="560" y1="386" x2="160" y2="386" class="dg-msg-r" marker-end="url(#dg-arrow3)"/>
<text x="364" y="378" text-anchor="middle" class="dg-msg-t">206 · append from byte N</text>
<!-- phase band: verify and land -->
<rect x="24" y="404" width="672" height="140" class="dg-phase"/>
<text x="32" y="420" class="dg-phase-t">VERIFY AND LAND</text>
<rect x="176" y="432" width="300" height="22" rx="6" class="dg-note"/>
<text x="326" y="443" text-anchor="middle" dominant-baseline="central" class="dg-note-t">part digest vs remote sibling</text>
<rect x="176" y="464" width="320" height="22" rx="6" class="dg-note"/>
<text x="336" y="475" text-anchor="middle" dominant-baseline="central" class="dg-note-t">match: rename into place, write the marker</text>
<rect x="176" y="496" width="344" height="22" rx="6" class="dg-note"/>
<text x="348" y="507" text-anchor="middle" dominant-baseline="central" class="dg-note-t">differ: delete the part, refuse with exit 1</text>
</svg>
</figure>

`416` means the part already holds the whole object: the previous run died between the last downloaded byte and the rename.
The client finalizes the part, digest-checks it against the remote sibling, and renames or discards.
No bytes are re-fetched for an already-complete part, and that exact absence of a range fetch is pinned by a conformance test.

One upstream quirk is documented honestly.
The Nexus version this client targets answers `500` instead of `416` when a Range request arrives for a zero-byte object, because the range computation in its `PartialFetchHandler` builds a Guava `Range.closed(0, -1)` and crashes.
`nxr` never ranges a zero-length object: a `Range` header goes out only when the part already holds bytes, and a complete part of a zero-byte object is empty, so the request is a plain unconditional GET.
That makes the quirk upstream behavior worth knowing rather than a client contract.

## Segmented transfers are an optional assembly

A single artifact can be split into `N` closed Range spans (`--segments N`, 2..=64).
The spans stage as separate files under `.nxr-segments-<32 hex>`, assemble positionally into the usual part, and one pass hashes the result.
A finished span survives a killed run.
An unfinished one starts over.

The worker budget still caps total concurrency: spans and whole artifacts acquire the same permits, so `--segments` changes the shape of a transfer but not the number of connections.
The spans request `Accept-Encoding: identity`, because byte offsets only address the stored representation.
A compressible object can therefore transfer more bytes than a compressed single stream.
When a server answers a ranged GET with `200` and the whole body, the staged spans are discarded and the object falls back to the single stream.
A zero-byte or unsplittable object takes the single stream from the start.

Segments are opt-in because the win depends on the link and the server: one large artifact on a high-latency path benefits, many small artifacts usually do not.

## A clone cache is a hint, not a state file

`clone` re-reads the local tree on every run, so it can skip a file whose size, mtime and marker digest still match the last verified copy.
That cache lives at `<DIR>/.nxr-state.json` and is an accelerator, never authority.
A missing, corrupt or foreign cache means "hash everything", not "trust me".
`--verify` ignores the cache for one run, and `verify`, `audit` and `doctor` never read it.
The cache never decides existence: the local `stat` and the remote probes do.

The model stays curl-like because the cache is optional and local to a clone.
No other command reads it, and no correctness rule depends on it.

## Enumeration is explicit

A raw repository has no guaranteed directory listing.
The search API varies between server releases, and HEAD-probing for likely names is guessing with extra requests.
So `down` requires an enumeration source (explicit names, a name file, or an explicitly requested best-effort `--ls`) and refuses with a hint when it has none.

The caller owns the name list: repeat `--name`, or write the list to a file and pass `--names-from`.
Nothing is reserved: a publisher who wants a machine-readable index ships one as an ordinary artifact and documents its name, and `nxr` never reads it.
A name list the client cannot know stays a hard refusal, the same philosophy as the exit codes: when the client cannot know, it says so and asks.

## Channels generalize pointers

v0.2 reserved two pointer names.
v0.3 reserves none.
A channel is any token file (`latest`, `stable`, `prod-1`), and the meaning of a name belongs to the publisher rather than the tool.
`channel set` writes the token, and `--if-forward` compares tokens in dotted-numeric version order, which keeps the one property that mattered: a late CI job cannot roll a release back.

What was rejected along the way is claim immutability.
The old reserved pointer tried to be an immutable claim, but a dumb store cannot enforce immutability, so the guarantee was fictional and the special case leaked into every layer.
The special case was deleted and replaced by a comparison the client can perform.

## The transport trusts no single signal

Retries cover connect errors, timeouts, broken bodies and 5xx but never 4xx, where retrying would just re-authenticate a refusal.
A stall timeout replaces a total timeout: a 100 MB artifact over a slow link may legitimately take hours, but no bytes for 30 s is a dead connection.
Auth failures bypass the retry loop entirely, because hammering a 401 is never useful.
Exact numbers live in the [protocol reference](../reference/protocol.md), and the recovery story in [Conformance](conformance.md).

A `429` with `Retry-After` gets the additive-increase, multiplicative-decrease treatment: retries honor the header's wait, and the worker pool halves in flight at once, then re-grows one worker per success streak.
The design separates two clocks: the retry loop is per-request patience, the worker budget is per-run pressure, and a polite client under throttle needs both.
The pool never exceeds `--workers` and never reaches zero, so a throttled transfer converges to the pace the server allows instead of failing or hammering.

## Exit codes encode the operator's decision

Data problems (exit 1) need a human, invocation problems (exit 2) need a pipeline fix, and transport problems (exit 3) need another attempt later.
Every error carries a `hint:` line so the decision is cheap at 3 a.m.
The full mapping is in [errors and exit codes](../reference/errors.md), and [when it breaks](../how-to/troubleshoot.md) turns it into a procedure.

## Read-only diagnostics

`audit` and `doctor --deep` are read-only by contract: no request either command makes mutates the repository.
The guarantee is enforced by construction, not by politeness: the diagnostics run on the same transport as everything else, but their probe and finding paths contain only GET and HEAD, and the conformance tests assert that a full audit or deep-doctor run against `mock-nexus` issues no other method.
Write rights are deliberately not probed by doctor: a PUT probe would itself be a mutation, so the absence of write access surfaces on the first real `up`, with its usual exit code and hint.
The one deliberate extension to reading is `audit --bytes`, which downloads and hashes asset bytes: it reads more, and it still writes nothing.

## What was deliberately left out

Config files and profiles, repacking, unarchiving, a daemon, a server component.
Deletion is the one deliberate extension, and it stays narrow: `rm` deletes exactly the enumerated names of one version, `point` deletes exactly one pointer ([the protocol](../reference/protocol.md#deletion)).
The repository stays a dumb, verifiable object store.
The client stays a static binary whose dependencies of consequence are reqwest and tokio.
Working within those walls: [publishing a version](../how-to/publish.md), [consuming artifacts](../how-to/consume.md), [CI recipes](../how-to/ci.md).
