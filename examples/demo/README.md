# The local stand

A full nexus-raw session against the repo's mock server and stub payloads, with every command printed as it runs.

```bash
just demo                      # publish, name, consume, verify, refuse, repair
just demo flaky                # the same story against another failure scenario
just demo slow --chunk-delay-ms 400 --chunk-size 4096
```

Nothing is simulated at the nexus-raw layer: the `nxr` binary does the transfers and `mock-nexus` is the same server the conformance suites drive.
The payloads in `dist/1.4.0/` are generated on every run, deterministically, so digests and transcripts stay stable between runs.
`dist/` and `vendor/` are working directories, not sources.

## What the session shows

| Beat | What it proves |
| :-- | :-- |
| `up --claim-first manifest.json` | the enumeration source lands before any other name, so a consumer never sees half a version |
| `channel set --if-forward` | a version gets a name, and the pointer only moves forward |
| `down --workers 1` | exactly the enumerated names, one at a time, so you can watch each land |
| `verify` | digests check offline, no network |
| an edited local file | the next `down` refuses instead of overwriting a diverged object (exit 1) |
| the same file deleted | the re-run fetches exactly what is missing |

The trailing phases swap the server underneath the same commands: `flaky` answers the first request per path with `503` and the run recovers on its own, `auth-401` turns wrong credentials into exit 3.

## Poking at it

```bash
NXR_DEMO_KEEP=1 just demo slow --port 8734   # keep the mock serving and print its URL
curl -s http://127.0.0.1:8734/1.4.0/manifest.json
just mock flaky --flaky 3                    # another server, another failure mode
just nxr -- up --help
```

Without `--port` the server takes an ephemeral port and the keep banner prints the URL to use.
`NXR_DEMO_EXTRA=0` stops after the main story; `NXR_DEMO_KEEP=1` (or `--port` with it) keeps the last mock alive.

## Recording the landing animation

```bash
just demo-cast
```

That runs the same session with `NXR_DEMO_EXTRA=0`, timestamps every line, and writes `docs/assets/cast/session.json`.
The file is plain JSON lines, so the diff of a recording is readable: the landing player replays it with the recorded timings, and the still in `docs/assets/img/hero-terminal.png` is a frame of the same recording.
Regenerate it after any change to the commands, the output format or the payloads.
