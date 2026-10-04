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
`NXR_DEMO_SCOPE=core` stops after verify, the four beats the landing animation shows.
`NXR_DEMO_KEEP=1` (or `--port` with it) keeps the last mock alive.

## Recording the landing animation

```bash
just demo-cast
```

One run, two files: the recipe records the session in its `core` scope, then stages and draws it.

| File | What it is |
| :-- | :-- |
| `docs/assets/cast/session.json` | the recording: every line with the time it arrived, then `title`, `at` and `kinds` |
| `docs/assets/img/session.svg` | the same session as an animated SVG, embedded by both readmes |

The commands run at their default verbosity.
`-v` prints every name three times (as a plan entry, as a transfer start and on completion), and a transcript wants one line per file.

`stage.py` writes the two display fields from the recording.
`at` is when each line appears: the recorded times are the tool's own speed, and a reader needs a pace, so a long line is held longer, a blank line is a beat and a command is held before its output starts.
`kinds` is what each line is (command, note, digest result, error), which is the color both renderers paint it with.
The landing player and the SVG replay that same timeline, so the animation and the readme show one session at one pace.
`svg.py` only lays it out.
The recorded times are what the machine measured, so recording again moves them.
`at` and `kinds` come from the line contents, so the animation, its length and the SVG do not.

Regenerate both files after any change to the commands, the output format or the payloads.
