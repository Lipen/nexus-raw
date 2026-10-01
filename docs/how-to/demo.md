# Run the demo locally

The [landing page](https://lipen.github.io/nexus-raw/) plays a recording of the stand that lives in this repository: the `nxr` binary doing the transfers, the mock server from the conformance suites underneath, stub payloads on disk.
This page runs the same session on your machine.

## One command

```bash
just demo
```

What it does, in order:

- builds `mock-nexus` and `nxr` (incremental, so a second run starts immediately);
- generates the payloads under `examples/demo/dist/1.4.0/` deterministically, so the digests are the same in every run;
- starts the mock server on an ephemeral port;
- prints every command it runs and streams the output as it arrives;
- kills the server when it exits.

The session: publish a version directory, name it with a channel, fetch that version back, verify it offline, then edit a file by hand and watch the next fetch refuse instead of overwriting it.

## The same session, a different failure

The scenario is the first argument, and the commands stay the same:

```bash
just demo flaky                # the first two requests per path answer 503: watch the retries
just demo auth-401             # every request wants credentials: each command stops with exit 3
just demo markerless           # markers are acknowledged and never stored: nothing is ever up to date
just demo slow --chunk-delay-ms 400 --chunk-size 4096
```

`just mock --print-scenarios` prints the full list.
What each scenario breaks: [conformance](../explanation/conformance.md).

## Keep it running and poke at it

```bash
NXR_DEMO_KEEP=1 just demo slow --port 8734
```

The script runs its session, then leaves the main server serving and prints the URL it bound:

```text
still serving http://127.0.0.1:8734 (scenario slow); ctrl-c to stop it
```

Without `--port` the server picks an ephemeral port: use the URL the banner prints.
The examples below work as written against it:

```bash
nxr head http://127.0.0.1:8734/1.4.0/app-1.4.0.zip
nxr get  http://127.0.0.1:8734/1.4.0/manifest.json
nxr up   examples/demo/dist/1.4.0/ http://127.0.0.1:8734/1.4.0/ --dry-run
```

```console
$ nxr head http://127.0.0.1:8734/1.4.0/app-1.4.0.zip
head: 200 98304 -
$ nxr get http://127.0.0.1:8734/1.4.0/manifest.json
{
  "artifacts": ["app-1.4.0.zip", "bom/linux-x86_64.json", "pinned.xml"]
}
$ nxr up examples/demo/dist/1.4.0/ http://127.0.0.1:8734/1.4.0/ --dry-run
skip app-1.4.0.zip
skip bom/linux-x86_64.json
skip manifest.json
skip pinned.xml
```

`just mock <scenario>` starts a server on its own, and `just nxr -- <args>` runs the CLI from the workspace.
Both run the binaries this checkout builds.

## How the landing animation is made

```bash
just demo-cast
```

That recipe runs the same session in its `core` scope — publish, name, resolve the channel, fetch, verify — pinned to port 8734, and records it into `docs/assets/cast/session.json`.
The file is plain JSON: one `[millisecond, line]` pair per entry, timestamps quantized to 50 ms, so a regenerated recording diffs only where the run really differed — the arrival times.
The pace and the colors are computed from the lines themselves, so the animation and the SVG stay put when only those times move.
Two more fields are display data, written from the recording by `examples/demo/stage.py`: `at`, when each line appears on screen, and `kinds`, what each line is — command, note, digest result, error.
The player above and `docs/assets/img/session.svg` both replay that timeline, which is why the readmes embed the same animation without a line of JavaScript.

The hero is four beats on one screen; the rest of the story — the edited artifact and the refusal, the flaky server, the auth wall — stays in the session itself (`just demo`).
Regenerate the cast, its pace and the SVG in one step after any change to the demo commands, to the output format or to the payloads.
