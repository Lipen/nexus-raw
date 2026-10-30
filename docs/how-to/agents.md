# nxr for agents

One anchor page: agent intent -> command -> expected result.
Every command below exists.
Install: `cargo install nexus-raw` (or `npm install @nexus-raw/nxr` when the environment is Node).
`service eula --accept` needs nx-admin on the server.
Read access works for any user the server answers.

## Cookbook

| Intent | Command | Success looks like | Exit |
|:--|:--|:--|:--|
| is the server alive? | `nxr service status <url>` | `alive` / `alive, writable` | 0 |
| what repository is this? | `nxr service repo <url>` | `name format type url` | 0 |
| what is where (any format)? | `nxr service assets <repo-url>` | NDJSON lines `asset` + `summary` | 0 |
| the server refuses writes? | `nxr service eula <url> --accept` | `accepted` | 0 |
| what is in a raw folder? | `nxr ls <dir-url>` | one line per entry | 0 |
| the same, for a script | `nxr ls <dir-url> --json` | NDJSON: `{"entry":...,"kind":...}` | 0 |
| download a file | `nxr get <url> -o <file>` | bytes on disk, digest checked | 0 |
| download a file in 8 spans | `nxr get <url> -o <file> --segments 8` | bytes on disk, digest checked | 0 |
| download one byte span | `nxr get <url> --range 100-199` | the span on stdout, or on disk with `-o` | 0 |
| download to stdout | `nxr get <url>` (no `-o`) | the stream on stdout | 0 |
| download a folder | `nxr down <dir-url> <dst-dir> --names-from list.txt` | `downloaded N` | 0 |
| clone a repository | `nxr clone <repo-url> <dst-dir>` | a clone with the fact cache | 0 |
| upload a folder | `nxr up <dir> <dir-url>` | `uploaded N` + `.sha256` markers | 0 |
| upload a subtree | `nxr up <dir> <dir-url> --prefix <dir>/` | `uploaded N` for the selected names | 0 |
| what differs? | `nxr diff <dir> <dir-url> --ls` | the delta: local/remote/same | 0 |
| copy a version | `nxr mirror <src-url> <dst-url> --names-from list.txt` | `uploaded N, downloaded N` | 0 |
| move a version | `nxr mv <src-url> <dst-url> --names-from list.txt` | mirror + delete the source | 0 |
| delete a version | `nxr rm <dir-url> --names-from list.txt` | the marker deleted first, then the bytes | 0 |
| drop a pointer | `nxr point <ptr-url>` | the pointer file is gone | 0 |
| verify locally | `nxr verify <dir>` | digests and markers agree | 0 |
| diagnose access | `nxr doctor <url>` | credentials/TLS/reachability, item by item | 0 |

Note: enumeration needs a name source (`--name`, `--names-from`, or the search API through `--ls`).
Without one it exits 1 (cannot enumerate) and a `hint:` line names the source.

`up --prefix` scans only the selected subtrees. Entries outside them are never read.
A private CA is `--ca-bundle ca.pem` (or `NXR_CA_BUNDLE`). The bundle carries the signing CA, not the server leaf.

## The contract for scripts

- `--json`: one NDJSON line per event, the final event always present.
- Exit codes: `0` success, `1` data (404, digest miss), `2` misuse (bad invocation), `3` transport.
- Every error on stderr carries a `hint:` line naming the next action.
  The hint includes `server says:`, the short body of a 4xx/5xx response (for example the refusal before accepting the EULA, CE 3.79+).
- Credentials: `-u user:pass`, else `NXR_AUTH` (base64 `user:pass`), else `NXR_USERNAME` + `NXR_PASSWORD`.
  Without `-R` no config file is in play, by design.
- A long URL? `-R <alias>` from `~/.config/nxr/config.toml`: `nxr -R releases ls 1.4.0/`.
  The file is read only when the flag names an alias, never picked up implicitly.

## Quirks that are not bugs

- `GET /service/rest/v1/status` answers 200 with an empty body on Nexus 3.79.
- The `/v1/repositories` collection is trimmed (`size`, empty `attributes`) even for an admin.
  The full settings are only in the admin `GET /v1/repositories/{format}/{type}/{name}`.
- An anonymous user and a logged-in user see different repository lists.
  A 403 on `status/check` means "the user is not an admin", not "the server is down".
- CE 3.79+: every PUT answers 403 until the EULA is accepted.
  The fix: `nxr service eula <url> --accept` or POST `/v1/system/eula` with the disclaimer echoed.
- `-o` must be a real path: nxr writes a sibling `<name>.part` and renames it after the resume (the resume scheme).
  For stdout, omit `-o` (or pass `-`).
- Right after `up`/`put`, enumeration through search may be empty for a few seconds.
  That is the search index lag, not a lost write.
- A repository filled by an external publisher may be only partly visible through the search API: some subtrees are indexed, some are not.
  When the listing looks short, check presence by name with `head`/`get`. `service assets` goes through the same search and sees the same gap.
- `/-/` in paths is an npm protocol segment (dist-tags, tarballs), not part of the nxr protocol.
- A raw repository can hold foreign files: spaces, brackets, Unicode.
  The tree `ls` shows them, but a transfer needs the `[A-Za-z0-9._-]` alphabet: an enumeration hides such a name and names the count, so `down --ls` cannot fetch it.
- npm repositories: browse them through `nxr service assets`.
  Downloading packages is the npm client's job (`npm view`/`npm pack --registry ...`).

## Next

- The full CLI surface: [reference/cli.md](../reference/cli.md).
- The error taxonomy and every hint: [reference/errors.md](../reference/errors.md).
- The storage and the protocol rules: [reference/protocol.md](../reference/protocol.md).
