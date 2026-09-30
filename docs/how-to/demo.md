# Run the demo locally

The [landing page](https://lipen.github.io/nexus-raw/) plays a recording of the stand that lives in this repository: the `nxr` binary doing the transfers, the mock server from the conformance suites underneath, stub payloads on disk.
This page runs the same session on your machine and shows how to aim it at the failure you care about.

## One command

```bash
just demo
```

What it does, in order:

- builds `mock-nexus` and `nxr` if they are not built;
- generates the payloads under `examples/demo/dist/1.4.0/` — deterministically, so the digests in every run are the same;
- starts the mock server on an ephemeral port, so it cannot collide with whatever else you run;
- prints every command it runs and streams the output as it arrives;
- kills the server when it exits.

The session is the whole story: publish a version directory, name it with a channel, fetch that version back, verify it offline, then edit a file by hand and watch the next fetch refuse instead of overwriting it.

## The same story, a different failure

The server is chosen by scenario, and every command stays the same:

```bash
just demo flaky                # the first request per path answers 503: watch the retries
just demo auth-401             # every request wants credentials: wrong ones are exit 3
just demo markerless           # markers are acknowledged and never stored: the run ends incomplete
just demo slow --chunk-delay-ms 400 --chunk-size 4096
```

`just mock --print-scenarios` lists them, and [conformance](../explanation/conformance.md) explains what each one breaks.

## Keep it running and poke at it

```bash
NXR_DEMO_KEEP=1 just demo slow --port 8734
```

The script runs its session, leaves the server serving and prints the URL it bound, so the examples below work as written.
Without `--port` the server picks an ephemeral port: use the URL the banner prints.

```bash
nxr head http://127.0.0.1:8734/1.4.0/app-1.4.0.zip
nxr get  http://127.0.0.1:8734/1.4.0/manifest.json
nxr up   examples/demo/dist/1.4.0/ http://127.0.0.1:8734/1.4.0/ --dry-run
```

`just mock <scenario>` starts a server on its own, and `just nxr -- <args>` runs the CLI from the workspace.
Both run the binaries this checkout builds, so the stand always matches the code you are reading.

## How the landing animation is made

```bash
just demo-cast
```

That runs the session with the extra failure phases off, timestamps every command and every output line, and writes `docs/assets/cast/session.json`.
The file is plain JSON — one `[millisecond, line]` pair per entry — so a recording is reviewable in a diff.

The landing player reads that file and decides when a line becomes visible, because the recorded timings are the tool's own speed: most lines land within milliseconds of each other.
The player gives each line a minimum time on screen and shortens long pauses; the recorded numbers stay in the file.
The still image in `docs/assets/img/hero-terminal.png` is a frame of the same recording, so the picture and the animation never tell different stories.

Regenerate the cast after any change to the demo commands, to the output format or to the payloads.
