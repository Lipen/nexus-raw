# Server facts that are not bugs

Everything here is observed behaviour of a real Nexus or a deliberate nxr decision.
Reach for it before filing a bug or working around a symptom.

## The EULA gate (Nexus CE 3.79 and later)

Every PUT answers 403 until the End User License Agreement is accepted, which reads as a permissions problem and is not one.
Accept it once per server with `nxr service eula <url> --accept`, or `POST /service/rest/v1/system/eula` with the disclaimer echoed back.
Accepting needs nx-admin.

## The status endpoint

`GET /service/rest/v1/status` answers 200 with an empty body on 3.79 and later.
An empty body is a healthy answer, not a truncated response.

## The service API is a different surface from storage

`nxr service ...` talks to the Sonatype REST API and never touches the raw store layout.

- The `/v1/repositories` collection is trimmed even for an admin: `size` is present, `attributes` is empty.
  Full settings live behind the admin-only `GET /v1/repositories/{format}/{type}/{name}`, which `nxr service repo <url> --detail` calls.
- Anonymous and authenticated users can see different repository lists.
- A 403 from `status/check` means the caller is not an admin, not that the server is down.
- `search-400`: the search API answers 400 when the `repository` parameter names a repository the instance does not serve.
  nxr maps that to a missing-or-not-raw diagnosis rather than a generic enumeration failure.

## The search index is a lagging, partial view

The search API is an index, not the store, and it is the only enumeration source nxr has.

- Right after an `up` or a `put`, a listing through search can be empty for a few seconds.
  That is index lag, not a lost write.
- A repository filled by a foreign publisher can be indexed partially: some subtrees appear, some do not.
  When a listing looks short, confirm a specific name with `head` or `get` instead of trusting the listing.
- `ls --assets`, `down --ls`, `diff --via search`, `mirror --ls`, `mv --ls`, `rm --ls`, `clone`, `audit` and `service assets` all read that same index and inherit the same blind spot.
- Adds and deletes in the store are visible to a direct `get`/`head` immediately, whatever the index says.

## Transfer mechanics

- `-o` must be a real path.
  nxr writes a neighbouring `<name>.part` and renames it to the final name only after the bytes are complete and the digest checked out, which is the resume scheme.
  A refused or failed name leaves only the part file, never a truncated artifact under the real name.
- Dropping `-o` streams the body to stdout and prints no events.
  `--json` without `-o` is refused as misuse.
- A killed transfer resumes by rerunning the same command: finished names are skipped and part files are the resume fuel.
  `--fresh` discards the part files instead.
- Downloads resume with a single open-ended `Range: bytes=N-`, answered as 206.
  Any other `Range` form is ignored by the server, and an out-of-range start answers 416.
- Every existing directory between the destination root and a file must be a real directory.
  nxr refuses a path whose intermediate component is a symlink, so a planted `bom -> ~/.config` cannot land an artifact outside the destination.
- `up --claim-first <name>` sends that one name first, alone, before any other: a killed publish then leaves a document ahead of its bytes rather than bytes behind a missing document.

## Names and the alphabet

- The grammar `[A-Za-z0-9._-]`, 1 to 255 bytes per segment, no empty, `.` or `..` segments, applies to what nxr writes and to every name it accepts from the network.
  A hostile or broken listing that carries a traversal or absolute path is dropped rather than followed.
- The `.sha256` suffix is reserved: an artifact name must not end with it.
- The `.part` suffix is reserved too: those names are local download staging.
- A `.nxr`-prefixed segment is reserved too: those names are the transfer tool's local staging and cache.
- A raw repository still holds foreign files with spaces, parens and unicode.
  `ls` and `down` display and fetch them, and the alphabet constrains only what nxr itself writes.
- The local scan refuses a symlink or any other non-regular entry: it never follows a link out of the tree.
- `version.json`, `latest` and `nightly` are ordinary names with no protocol meaning.
  nxr never claims or rewrites them implicitly.

## Channels and pointers

- A channel is any token file, and the token is opaque to nxr.
  The only comparison nxr imposes is dotted-numeric order, and only when `--if-forward` asks for it.
- A pointer is a token file you delete with `nxr point <url>`.
- `channel get` on an unset channel prints `unset`, which is a value, not an error.

## Transport behaviour

- Redirects are never followed.
  A 301 on a read surfaces as a plain transport error with exit 3, and the client does not chase the `Location`.
- TLS verification is on by default, and `--tls-insecure` is the only way off.
- A private CA is `--ca-bundle FILE` (or `NXR_CA_BUNDLE`), a PEM bundle of additional roots. It conflicts with `--tls-insecure`.
  The bundle carries the CA that signed the server certificate, not the bare server leaf.
- Retries cover transport failures, 429 with `Retry-After`, and 5xx.
  A 404 is a data fact rather than a transport failure, so it is not retried and exits 1.
- A stalled transfer fails once no bytes arrive for `--stall-secs`, and rerunning is safe.

## Things nxr refuses on purpose

- No config file unless `-R` names an alias, so nothing is picked up implicitly.
- No manifest and no lockfile: the tree layout is the caller's, and a version is a directory.
- No overwrite of a divergent complete artifact: the digest decides, and a mismatch is an error rather than a silent replacement.
- `down`, `diff`, `rm` and `mirror` never guess an enumeration source: without `--name`, `--names-from` or `--ls` they refuse.
