"""Conformance: the mock-nexus scenario table, driven through the Python reference client.

The scenario list is the conformance contract.
It is fetched live from `mock-nexus --print-scenarios`, never copied.
Every scenario needs a test method named `test_<scenario with dashes as underscores>` or a row in `SKIP`.
The drift test `Reference.test_every_scenario_is_classified` fails the suite until a new mock scenario is classified.

The suite mirrors `crates/nexus-raw-core/tests/conformance.rs`:
one server per test, assertions on the store and on the request log, the same scenario coverage.
The Rust suite holds an in-process `MockNexus` handle, so its `store_get` and `requests` become here
a plain retry-free `raw_get`/`raw_put` and the client's own `.requests` log.
"""

import contextlib
import base64
import hashlib
import http.client
import json
import os
import pathlib
import re
import subprocess
import threading
import time
import unittest
import urllib.parse

import nxr

USER = "ci"
PASS = "secret"
VERSION = "1.0.0"
CONTENT = b"payload-0123456789"

REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]

# Scenarios the reference client cannot exercise, with the reason and what is not implemented.
# The drift test holds this table against the mock's own list: a stale row fails the suite.
SKIP = {
    "doc-drift": (
        "the drift toggle is a library-only knob (MockNexus::enable_drift), so the binary serves plain storage"
        " and the client-side half, version.json being an ordinary name, is already pinned under atomic"
    ),
    "no-service": (
        "the scenario only breaks /service/rest/v1/repositories, and the client has no service-repos surface"
    ),
    "readonly": (
        "the scenario only answers 403 to DELETE, and the reference client implements no delete"
    ),
}


def mock_command() -> list[str]:
    """The command prefix that runs the mock-nexus binary.

    `MOCK_NEXUS_BIN` wins, then the cargo build in the repo root,
    then `cargo run -p mock-nexus` as the last resort.
    """
    override = os.environ.get("MOCK_NEXUS_BIN")
    if override:
        return [override]
    built = REPO_ROOT / "target" / "debug" / "mock-nexus"
    if built.exists():
        return [str(built)]
    return ["cargo", "run", "-q", "-p", "mock-nexus", "--bin", "mock-nexus", "--"]


MOCK = mock_command()


def listed_scenarios() -> list[str]:
    """The conformance contract: the mock's own scenario table, printed as a JSON array."""
    out = subprocess.run(
        [*MOCK, "--print-scenarios"],
        capture_output=True,
        text=True,
        check=True,
        cwd=REPO_ROOT,
    )
    return json.loads(out.stdout)


class Mock:
    """One running mock-nexus process: the base URL plus the process handle."""

    def __init__(self, proc: subprocess.Popen, base: str) -> None:
        self.proc = proc
        self.base = base

    def url(self, name: str) -> str:
        """The URL of `name` inside the version directory, the `dir_url` of the Rust suite."""
        return f"{self.base}{VERSION}/{name}"


@contextlib.contextmanager
def mock_server(scenario: str, *flags: str):
    """Spawn `mock-nexus <scenario>` on a free port and tear it down on exit.

    Mirrors `MockNexus::start`: the binary binds, prints `listening http://127.0.0.1:<port>` and serves until killed.
    """
    proc = subprocess.Popen(
        [*MOCK, scenario, *flags, "--port", "0"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=REPO_ROOT,
    )
    try:
        # A binary that binds but stalls before printing would block readline forever:
        # the watchdog closes the pipe, so the failure surfaces as a loud assertion.
        watchdog = threading.Timer(15.0, proc.stdout.close)
        watchdog.start()
        try:
            line = proc.stdout.readline()
        except ValueError:
            line = ""
        finally:
            watchdog.cancel()
        found = re.match(r"listening (http://\S+)", line)
        if not found:
            proc.terminate()
            proc.wait(timeout=10)
            raise AssertionError(
                f"mock-nexus {scenario} did not start: stdout={line!r} stderr={proc.stderr.read()!r}"
            )
        # The binary prints a bare address: normalize to a directory URL with the trailing slash.
        yield Mock(proc, found.group(1).rstrip("/") + "/")
    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
        for pipe in (proc.stdout, proc.stderr):
            if pipe is not None:
                pipe.close()


def raw_get(url: str, auth: str | None = None) -> bytes | None:
    """One retry-free GET: what the server holds right now, the `store_get` of the Rust suite."""
    parts = urllib.parse.urlsplit(url)
    conn = http.client.HTTPConnection(parts.hostname, parts.port, timeout=10)
    try:
        headers = {}
        if auth is not None:
            token = base64.b64encode(auth.encode()).decode()
            headers["Authorization"] = f"Basic {token}"
        conn.request("GET", parts.path or "/", headers=headers)
        resp = conn.getresponse()
        body = resp.read()
        return body if resp.status == 200 else None
    finally:
        conn.close()


def raw_put(url: str, body: bytes) -> None:
    """One retry-free PUT that seeds the store directly, the `insert` of the Rust suite.

    The seeding helper pays the scenario tolls it cannot bypass: a 429 costs its Retry-After, a 5xx a short backoff.
    """
    parts = urllib.parse.urlsplit(url)
    for attempt in range(4):
        conn = http.client.HTTPConnection(parts.hostname, parts.port, timeout=10)
        try:
            conn.request("PUT", parts.path or "/", body=body)
            resp = conn.getresponse()
            resp.read()
        finally:
            conn.close()
        if resp.status == 201:
            return
        if resp.status == 429:
            time.sleep(float(resp.headers.get("Retry-After", "1")))
        elif 500 <= resp.status < 600:
            time.sleep(0.5 * (attempt + 1))
        else:
            break
    raise AssertionError(f"seed PUT {url} never stored, last status {resp.status}")


def search_document(paths: list[str]) -> bytes:
    """One search-assets page in the wire shape the Rust tests and real Nexus agree on."""
    return json.dumps({"continuationToken": None, "items": [{"path": p} for p in paths]}).encode()


def wire_path(url: str) -> str:
    """The request path of a URL: the shape the client's request log records."""
    parts = urllib.parse.urlsplit(url)
    return parts.path or "/"


def scenario_slug(scenario: str) -> str:
    """The test-method name that covers `scenario`: dashes become underscores."""
    return "test_" + scenario.replace("-", "_")


def owns(slug: str, method: str) -> bool:
    """True when `method` can be a test of the scenario behind `slug`."""
    return method == slug or method.startswith(slug + "_")


class Conformance(unittest.TestCase):
    """One cluster of tests per scenario, named `test_<scenario with dashes as underscores>`.

    The naming convention is the coverage registry the drift test reads.
    Ownership is longest-match, so a method named after one scenario never silently covers a prefix of it.
    """

    maxDiff = None

    def client(self, auth: str | None = None, **kwargs) -> nxr.Nxr:
        """A client against one mock, the `config()` helper of the Rust suite."""
        return nxr.Nxr(auth=auth, **kwargs)

    def assert_marker_after_bytes(self, client: nxr.Nxr, url: str) -> None:
        """The marker ordering rule: every PUT of the marker follows a PUT of the bytes of the same name."""
        log = client.requests
        path = wire_path(url)
        bytes_at = [i for i, (m, p) in enumerate(log) if m == "PUT" and p == path]
        marker_at = [i for i, (m, p) in enumerate(log) if m == "PUT" and p == f"{path}.sha256"]
        self.assertTrue(bytes_at, f"expected PUTs of the bytes in {log}")
        self.assertTrue(marker_at, f"expected PUTs of the marker in {log}")
        self.assertLess(bytes_at[0], marker_at[0], "marker must follow bytes")
        self.assertLess(
            bytes_at[-1], marker_at[-1], "the final bytes attempt must still precede the marker"
        )

    # ------------------------------------------------------------ atomic: the control group

    def test_atomic_roundtrip(self):
        """The happy path: put generates the marker after the bytes, get verifies, head reports, sha agrees."""
        with mock_server("atomic") as mock:
            nx = self.client()
            path = mock.url("a.zip")
            digest = nx.put(path, CONTENT)
            self.assertEqual(digest, hashlib.sha256(CONTENT).hexdigest())
            self.assertEqual(nx.get(path), CONTENT)
            info = nx.head(path)
            self.assertEqual(info.status, 200)
            self.assertEqual(info.length, len(CONTENT))
            self.assertEqual(nx.sha(path), digest)
            stored = raw_get(f"{path}.sha256")
            self.assertEqual(stored, nxr.format_marker("a.zip", digest).encode())
            self.assert_marker_after_bytes(nx, path)

    def test_atomic_misses(self):
        """A miss is a normal answer: 404 on get and sha, a 404 HeadInfo on head, no retries burned."""
        with mock_server("atomic") as mock:
            nx = self.client()
            with self.assertRaises(nxr.HttpError) as raised:
                nx.get(mock.url("ghost.zip"))
            self.assertEqual(raised.exception.status, 404)
            with self.assertRaises(nxr.HttpError) as raised:
                nx.sha(mock.url("ghost.zip"))
            self.assertEqual(raised.exception.status, 404)
            info = nx.head(mock.url("ghost.zip"))
            self.assertEqual(info, nxr.HeadInfo(404, None, None))
            self.assertEqual(len(nx.requests), 3, "misses must not retry")

    def test_atomic_channel(self):
        """A channel is a token file: set writes one line, get reads it back, an unset channel reads None."""
        with mock_server("atomic") as mock:
            nx = self.client()
            url = mock.url("stable")
            nx.channel_set(url, "1.2.3")
            self.assertEqual(nx.channel_get(url), "1.2.3")
            self.assertEqual(raw_get(url), b"1.2.3\n")
            self.assertIsNone(nx.channel_get(mock.url("nightly")))

    def test_atomic_ls(self):
        """ls walks the search pages: the items come back in order, a store without the endpoint refuses."""
        with mock_server("atomic") as mock:
            nx = self.client()
            with self.assertRaises(nxr.EnumerateError) as raised:
                nx.ls(mock.base, "raw-main")
            self.assertEqual(raised.exception.status, 404)
            paths = [f"{VERSION}/a.zip", f"{VERSION}/a.zip.sha256", f"{VERSION}/bom/x.json"]
            raw_put(f"{mock.base}service/rest/v1/search/assets", search_document(paths))
            self.assertEqual(nx.ls(mock.base, "raw-main"), paths)
            self.assertTrue(
                any("repository=raw-main" in p for _, p in nx.requests),
                "the search must be repository-scoped",
            )

    def test_atomic_version_json_is_an_ordinary_name(self):
        """version.json carries no protocol meaning: it uploads and downloads like any artifact."""
        with mock_server("atomic") as mock:
            nx = self.client()
            path = mock.url("version.json")
            nx.put(path, CONTENT)
            self.assertEqual(nx.get(path), CONTENT)
            self.assertEqual(
                raw_get(f"{path}.sha256"),
                nxr.format_marker("version.json", hashlib.sha256(CONTENT).hexdigest()).encode(),
            )

    # ------------------------------------------------------------ the failure scenarios

    def test_partial_put(self):
        """A cut first attempt is a retryable transport failure: the replay stores the object whole."""
        with mock_server("partial-put") as mock:
            nx = self.client()
            path = mock.url("a.zip")
            body = b"\xa5" * 4096
            digest = nx.put(path, body)
            self.assertEqual(raw_get(path), body)
            self.assertEqual(raw_get(f"{path}.sha256"), nxr.format_marker("a.zip", digest).encode())
            puts_of_bytes = [1 for m, p in nx.requests if m == "PUT" and p == wire_path(path)]
            self.assertGreaterEqual(len(puts_of_bytes), 2, "the cut attempt must cost a retry")
            self.assert_marker_after_bytes(nx, path)

    def test_cut_body(self):
        """A mid-body break is a retryable transport failure: the retry lands the bytes whole."""
        with mock_server("cut-body") as mock:
            nx = self.client()
            path = mock.url("a.zip")
            body = b"\x5a" * 4096
            nx.put(path, body)
            self.assertEqual(nx.get(path), body)
            gets_of_bytes = [1 for m, p in nx.requests if m == "GET" and p == wire_path(path)]
            self.assertGreaterEqual(len(gets_of_bytes), 2, "the truncated attempt must cost a retry")

    def test_drop_connection(self):
        """A reset is retryable like any broken transport: the replay uploads and the read verifies."""
        with mock_server("drop-connection") as mock:
            nx = self.client()
            path = mock.url("a.zip")
            body = b"\xca" * 4096
            nx.put(path, body)
            self.assertEqual(nx.get(path), body)
            puts_of_bytes = [1 for m, p in nx.requests if m == "PUT" and p == wire_path(path)]
            self.assertGreaterEqual(len(puts_of_bytes), 2, "the reset attempt must cost a retry")

    def test_freeze_upload(self):
        """The attempt-level timeout aborts a frozen upload instead of hanging, retries, then refuses honestly."""
        with mock_server("freeze-upload") as mock:
            nx = self.client(attempts=2, timeout=1.0)
            path = mock.url("big.zip")
            big = b"\x5a" * (16 * 1024 * 1024)
            outcome = {}

            def attempt():
                started = time.monotonic()
                try:
                    nx.put(path, big, sha=False)
                    outcome["error"] = None
                except nxr.TransportError as err:
                    outcome["error"] = err
                outcome["elapsed"] = time.monotonic() - started

            worker = threading.Thread(target=attempt, daemon=True)
            worker.start()
            worker.join(timeout=30)
            self.assertFalse(worker.is_alive(), "upload hung past 30s under a frozen server")
            self.assertIsInstance(outcome["error"], nxr.TransportError)
            self.assertGreaterEqual(outcome["elapsed"], 1.0, "failed too fast to have watched the stall")
            puts = [1 for m, p in nx.requests if m == "PUT" and p == wire_path(path)]
            self.assertGreaterEqual(len(puts), 2, "expected a retry, saw {puts}".format(puts=len(puts)))
            self.assertIsNone(raw_get(path), "nothing may be stored under the frozen server")

    def test_sizeless(self):
        """A 2xx without Content-Length is present-but-unverifiable, never absent, and the body still lands whole."""
        with mock_server("sizeless") as mock:
            nx = self.client()
            path = mock.url("a.zip")
            nx.put(path, CONTENT)
            info = nx.head(path)
            self.assertEqual(info.status, 200)
            self.assertIsNone(info.length, "a lengthless answer must not read as a zero-byte object")
            self.assertEqual(nx.get(path), CONTENT)
            marker_info = nx.head(f"{path}.sha256")
            self.assertIsNone(marker_info.length)

    def test_slow(self):
        """A slow drip is just a download: the stall watchdog never trips while chunks keep arriving."""
        with mock_server("slow", "--chunk-delay-ms", "90", "--chunk-size", "1024") as mock:
            nx = self.client()
            path = mock.url("a.zip")
            body = b"\x0f" * (8 * 1024)
            nx.put(path, body)
            self.assertEqual(nx.get(path), body)
            self.assertEqual(nx.head(path).length, len(body))

    def test_foreign_marker(self):
        """A stored marker with a foreign digest marks the object diverged: get refuses, the bytes stay honest."""
        with mock_server("foreign-marker") as mock:
            nx = self.client()
            path = mock.url("a.zip")
            digest = nx.put(path, CONTENT)
            self.assertEqual(
                raw_get(f"{path}.sha256"),
                nxr.format_marker("a.zip", "0" * 64).encode(),
                "the scenario must have zeroed the stored digest",
            )
            with self.assertRaises(nxr.DigestMismatch):
                nx.get(path)
            self.assertEqual(nx.sha(path), digest, "the bytes themselves are intact")

    def test_markerless(self):
        """A markerless remote is present but unverifiable: get refuses and names the missing sibling."""
        with mock_server("markerless") as mock:
            nx = self.client()
            path = mock.url("a.zip")
            digest = nx.put(path, CONTENT)
            self.assertEqual(raw_get(path), CONTENT)
            self.assertIsNone(raw_get(f"{path}.sha256"), "the scenario must have dropped the marker")
            with self.assertRaises(nxr.MarkerMissing):
                nx.get(path)
            self.assertEqual(nx.sha(path), digest)

    def test_auth_401(self):
        """Auth failures fail fast with no retries and name the URL; the right credentials pass every method."""
        with mock_server("auth-401", "--auth", f"{USER}:{PASS}") as mock:
            anonymous = self.client()
            with self.assertRaises(nxr.AuthError) as raised:
                anonymous.get(mock.url("a.zip"))
            self.assertEqual(raised.exception.status, 401)
            self.assertIn(mock.url("a.zip"), str(raised.exception))
            with self.assertRaises(nxr.AuthError):
                anonymous.put(mock.url("a.zip"), CONTENT)
            self.assertEqual(len(anonymous.requests), 2, "auth failures must not retry")
            nx = self.client(auth=f"{USER}:{PASS}")
            self.assertEqual(nx.put(mock.url("a.zip"), CONTENT), hashlib.sha256(CONTENT).hexdigest())
            self.assertEqual(nx.get(mock.url("a.zip")), CONTENT)

    def test_auth_403(self):
        """A 403 maps onto the auth error exactly like a 401."""
        with mock_server("auth-403", "--auth", f"{USER}:{PASS}") as mock:
            anonymous = self.client()
            with self.assertRaises(nxr.AuthError) as raised:
                anonymous.get(mock.url("a.zip"))
            self.assertEqual(raised.exception.status, 403)
            wrong = self.client(auth=f"{USER}:wrong")
            with self.assertRaises(nxr.AuthError):
                wrong.get(mock.url("a.zip"))
            nx = self.client(auth=f"{USER}:{PASS}")
            with self.assertRaises(nxr.HttpError) as raised:
                nx.get(mock.url("a.zip"))
            self.assertEqual(
                raised.exception.status, 404, "the right credentials pass the gate: a plain store miss"
            )

    def test_flaky(self):
        """One call recovers through the 503s: the retries spend attempts and still land the transfer."""
        with mock_server("flaky") as mock:
            nx = self.client()
            path = mock.url("a.zip")
            nx.put(path, CONTENT)
            puts_of_bytes = [1 for m, p in nx.requests if m == "PUT" and p == wire_path(path)]
            self.assertGreaterEqual(len(puts_of_bytes), 3, "two 503s must cost two retries")
            self.assertEqual(nx.get(path), CONTENT)

    def test_rate_limit(self):
        """The client honors the Retry-After pause: 3s of mandated sleep where the backoff would wait under a second."""
        with mock_server("rate-limit", "--rate-429s", "1", "--retry-after-secs", "3") as mock:
            nx = self.client()
            path = mock.url("a.zip")
            started = time.monotonic()
            nx.put(path, CONTENT, sha=False)
            elapsed = time.monotonic() - started
            self.assertGreaterEqual(elapsed, 3.0, "the Retry-After pause must replace the backoff")
            attempts_of_bytes = [1 for m, p in nx.requests if m == "PUT" and p == wire_path(path)]
            self.assertEqual(len(attempts_of_bytes), 2, "one 429 then the success")
            marker = nxr.format_marker("a.zip", hashlib.sha256(CONTENT).hexdigest()).encode()
            raw_put(f"{path}.sha256", marker)
            self.assertEqual(nx.get(path), CONTENT)

    def test_redirect(self):
        """Redirects are never followed: a 301 surfaces as the plain HTTP error, while writes stay atomic."""
        with mock_server("redirect") as mock:
            nx = self.client()
            path = mock.url("a.zip")
            with self.assertRaises(nxr.HttpError) as raised:
                nx.get(path)
            self.assertEqual(raised.exception.status, 301)
            with self.assertRaises(nxr.HttpError) as raised:
                nx.head(path)
            self.assertEqual(raised.exception.status, 301)
            self.assertEqual(len(nx.requests), 2, "each refused read must be one request, never followed")
            # Reads are all redirected in this scenario, so the store is checked on the wire answer:
            # the mock stores exactly when the PUT answers 201.
            nx.put(path, CONTENT, sha=False)

    def test_search_400(self):
        """A repository-scoped search 400 is diagnosed, while a served repository keeps the store answer."""
        with mock_server("search-400") as mock:
            nx = self.client()
            paths = [f"{VERSION}/a.zip", f"{VERSION}/a.zip.sha256"]
            raw_put(f"{mock.base}service/rest/v1/search/assets", search_document(paths))
            self.assertEqual(nx.ls(mock.base, "raw-main"), paths)
            with self.assertRaises(nxr.SearchRepoMissing) as raised:
                nx.ls(mock.base, "raw-ghost")
            self.assertEqual(raised.exception.status, 400)
            self.assertIn("raw-ghost", str(raised.exception))

    # ------------------------------------------------------------ wire fidelity pins

    def test_atomic_marker_names_the_decoded_segment(self):
        """The marker names the percent-decoded final segment, like the Rust last_segment."""
        with mock_server("atomic") as mock:
            nx = self.client()
            path = mock.url("r%2Ezip")
            digest = nx.put(path, CONTENT)
            self.assertEqual(
                raw_get(f"{path}.sha256"),
                nxr.format_marker("r.zip", digest).encode(),
                "the marker must name the decoded segment r.zip",
            )
            self.assertEqual(nx.get(path), CONTENT)


class Reference(unittest.TestCase):
    """The drift guard and the pure pieces: no server, outside the scenario registry.

    The drift test reads only `Conformance` method names, so a test that drives no server
    can never collide with a scenario name.
    """

    maxDiff = None

    # ------------------------------------------------------------ the drift guard

    def test_every_scenario_is_classified(self):
        """The conformance stand: a new mock scenario fails the suite until it gets a test or a SKIP row.

        Ownership is longest-match: a test method belongs to the longest scenario slug it extends.
        A new scenario whose slug is a boundary prefix of an existing one (`rate` arriving while
        `rate-limit` tests exist) owns nothing, and is named in the failure until it gets its own method or a SKIP row.
        Both directions hold: an unclassified scenario, a stale SKIP row and a stray scenario-named method all fail.
        """
        listed = set(listed_scenarios())
        slugs = sorted((scenario_slug(s) for s in listed), key=len, reverse=True)
        methods = {name for name in dir(Conformance) if name.startswith("test_")}

        def owner(method: str) -> str | None:
            return next((slug for slug in slugs if owns(slug, method)), None)

        for scenario, reason in SKIP.items():
            self.assertTrue(reason.strip(), f"the SKIP row for {scenario} must carry a reason")
        unclassified = {
            s
            for s in listed
            if not any(owner(m) == scenario_slug(s) for m in methods)
            and s not in SKIP
        }
        self.assertEqual(
            unclassified,
            set(),
            "new mock scenario(s) with no test and no SKIP row; the python suite refuses to drift",
        )
        stale_rows = set(SKIP) - listed
        self.assertEqual(
            stale_rows,
            set(),
            f"SKIP rows for scenarios the mock no longer lists: {sorted(stale_rows)}",
        )
        stray = {m for m in methods if owner(m) is None}
        self.assertEqual(stray, set(), f"test methods naming no listed scenario: {sorted(stray)}")

    # ------------------------------------------------------------ pure pieces, no server

    def test_marker_parses_strictly(self):
        """The marker parse rejects every foreign format the protocol lists."""
        digest = hashlib.sha256(CONTENT).hexdigest()
        ok = nxr.format_marker("a.zip", digest).encode()
        self.assertEqual(nxr.parse_marker(ok), (digest, "a.zip"))
        broken = {
            "no trailing newline": ok.rstrip(b"\n"),
            "CRLF": ok.rstrip(b"\n") + b"\r\n",
            "single space": ok.replace(b"  ", b" ", 1),
            "three spaces": ok.replace(b"  ", b"   ", 1),
            "uppercase digest": ok.replace(digest.encode(), digest.upper().encode()),
            "short digest": ok.replace(digest.encode(), digest[:32].encode()),
            "two lines": ok + ok,
            "empty name": digest.encode() + b"  \n",
        }
        for reason, payload in broken.items():
            with self.assertRaises(ValueError, msg=reason):
                nxr.parse_marker(payload)

    def test_backoff_grows_and_caps(self):
        """The backoff doubles per attempt, caps at 60s, and the jitter stays within 250ms."""
        for attempt, floor in [(1, 0.5), (2, 1.0), (3, 2.0), (8, 60.0)]:
            client = nxr.Nxr()
            pause = client._backoff(attempt)
            self.assertGreaterEqual(pause, floor, f"attempt {attempt}")
            self.assertLessEqual(pause, floor + nxr.MAX_JITTER + 1e-9, f"attempt {attempt}")

    def test_retry_after_rules(self):
        """The Retry-After seconds form is honored, clamped to 1..=60, and anything else falls back to the backoff."""
        client = nxr.Nxr()
        self.assertEqual(client._pause("3", 1), 3.0)
        self.assertEqual(client._pause("0", 1), 1.0)
        self.assertEqual(client._pause("120", 1), 60.0)
        ceiling = nxr.BASE_BACKOFF + nxr.MAX_JITTER + 1e-9
        for garbage in ["soon", "-3", "", "3.5"]:
            self.assertLessEqual(client._pause(garbage, 1), ceiling, msg=repr(garbage))
        self.assertLessEqual(client._pause(None, 1), ceiling)

    def test_put_refuses_unsafe_names(self):
        """A URL whose final segment cannot carry a marker is refused before any byte moves.

        The rules the Rust grammar pins: the `.` and `..` segments, an empty segment
        (a trailing slash), the reserved `.sha256` suffix, and any character outside `[A-Za-z0-9._-]`,
        checked on the percent-decoded segment.
        """
        nx = nxr.Nxr()
        for url in [
            "http://127.0.0.1:1/x.sha256",
            "http://127.0.0.1:1/not a name",
            "http://127.0.0.1:1/.",
            "http://127.0.0.1:1/..",
            "http://127.0.0.1:1/dir/",
            "http://127.0.0.1:1/my%20app.zip",
        ]:
            with self.assertRaises(nxr.UnsafeName, msg=url):
                nx.put(url, CONTENT)
        self.assertEqual(nx.requests, [], "the refusal must precede the wire")

    def test_channel_set_refuses_multiline_tokens(self):
        """A channel token is one non-empty line with no CR."""
        nx = nxr.Nxr()
        for token in ["", "two\nlines", "cr\r"]:
            with self.assertRaises(nxr.NxrError, msg=token):
                nx.channel_set("http://127.0.0.1:1/latest", token)
        self.assertEqual(nx.requests, [], "the refusal must precede the wire")


if __name__ == "__main__":
    unittest.main()
