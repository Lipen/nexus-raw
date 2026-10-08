"""A minimal nexus-raw reference client on the Python standard library.

This file proves the nexus-raw protocol is implementable from scratch over bare HTTP:
`http.client` for the wire, `hashlib` for the digests, nothing to install.
The protocol is the contract: `crates/nexus-raw-core/src/protocol.md`.
The surface is the core of the CLI: `get`, `put`, `head`, `sha`, `ls`, `channel get/set`.
Deletion is deliberately absent.
Clarity beats cleverness: every verb is a few lines a porter can read against the protocol text.
"""

from __future__ import annotations

import base64
import hashlib
import http.client
import json
import random
import re
import time
from dataclasses import dataclass
from urllib.parse import unquote, urlencode, urlsplit

__all__ = [
    "AuthError",
    "DigestMismatch",
    "EnumerateError",
    "HeadInfo",
    "HttpError",
    "MarkerMissing",
    "Nxr",
    "NxrError",
    "SearchRepoMissing",
    "TransportError",
    "UnsafeName",
    "format_marker",
    "parse_marker",
]

# The retry policy mirrors the Rust transport: up to 4 attempts per request.
# The backoff is 0.5s x 2^n capped at 60s, plus a jitter of at most 250ms.
ATTEMPTS = 4
BASE_BACKOFF = 0.5
MAX_BACKOFF = 60.0
MAX_JITTER = 0.25

# A hostile endpoint can emit continuation tokens forever: the walk gives up instead of scrolling.
MAX_SEARCH_PAGES = 100

_HEX64 = re.compile(r"\A[0-9a-f]{64}\Z")
_SEGMENT = re.compile(r"\A[A-Za-z0-9._-]{1,255}\Z")


class NxrError(Exception):
    """The base class of every protocol failure.

    The message is one human sentence naming the URL.
    `status` carries the HTTP status when one exists, `None` for pure transport failures.
    `hint` is the action line a CLI would print after `hint:`.
    """

    status: int | None = None

    def __init__(self, message: str, *, status: int | None = None, hint: str = "") -> None:
        super().__init__(message)
        if status is not None:
            self.status = status
        self.hint = hint


class TransportError(NxrError):
    """A broken or exhausted transport: resets, timeouts, truncated bodies, spent 5xx/429 retries."""


class AuthError(NxrError):
    """The server refused the credentials with 401 or 403.
    Auth failures never retry.
    """


class HttpError(NxrError):
    """An unexpected HTTP status: misses, redirects and refusals that are never retried."""


class UnsafeName(NxrError):
    """A name outside the grammar, or carrying the reserved `.sha256` suffix."""


class DigestMismatch(NxrError):
    """The stored marker does not describe the received bytes: a divergent remote object."""


class MarkerMissing(NxrError):
    """Bytes exist without a sha-sibling: the remote object is present but unverifiable."""


class EnumerateError(NxrError):
    """The search endpoint could not produce a listing."""


class SearchRepoMissing(EnumerateError):
    """A repository-scoped search answered 400: the repository is missing or is not raw."""


def format_marker(name: str, digest_hex: str) -> str:
    """The canonical marker line: `<64 lowercase hex chars>␠␠<name>\\n`, as `sha256sum -c` reads it."""
    return f"{digest_hex}  {name}\n"


def parse_marker(data: bytes) -> tuple[str, str]:
    """Strictly parse one marker line and return `(digest, name)`.

    The rules mirror the protocol: LF only, exactly two spaces, 64 lowercase hex chars, a non-empty name, the trailing newline required.
    A loose parse would bless foreign markers, so every deviation raises `ValueError`.
    """
    try:
        text = data.decode("ascii")
    except UnicodeDecodeError:
        raise ValueError("marker is not ASCII") from None
    if not text.endswith("\n"):
        raise ValueError("missing trailing newline")
    body = text[:-1]
    if "\n" in body:
        raise ValueError("expected exactly one line")
    if "\r" in body:
        raise ValueError("CR found: CRLF is not allowed")
    if "  " not in body:
        raise ValueError("two-space separator not found")
    hex_part, name = body.split("  ", 1)
    if not name or "  " in name or name.startswith(" "):
        raise ValueError("name must be non-empty with exactly two spaces before it")
    if not _HEX64.match(hex_part):
        raise ValueError("bad digest: expected 64 lowercase hex chars")
    return hex_part, name


@dataclass
class HeadInfo:
    """The answer of `head`: the status plus whatever metadata the server sent."""

    status: int
    """The HTTP status: 200 present, 404 absent."""

    length: int | None
    """The Content-Length, or `None` when the server sent none.
    A 2xx without a length is present-but-unverifiable, never absent.
    """

    content_type: str | None
    """The Content-Type, when the server sent one."""


class Nxr:
    """The client: six verbs over bare HTTP.

    One HTTP connection per attempt, because the server may close after every response.
    Every attempt is appended to `.requests` as a `(method, path)` pair, oldest first.
    The log counts attempts the client made, including ones that never reached the server:
    a connection that fails to connect, or a body that stalls mid-send, is recorded too,
    so it is a superset of the mock's received-request log.
    """

    def __init__(
        self,
        auth: str | None = None,
        *,
        attempts: int = ATTEMPTS,
        timeout: float = 30.0,
    ) -> None:
        """`auth` is `user:pass` for Basic auth on every request (the `-u` of the CLI).

        `attempts` is the retry budget per request.
        `timeout` is the per-operation socket timeout, and so the stall watchdog:
        an attempt that sees no bytes for this long dies retryable, while a slow but moving transfer never trips it.
        There is no total-per-request timeout, because a big artifact on a slow link is legitimate.
        """
        self.attempts = attempts
        self.timeout = timeout
        self.requests: list[tuple[str, str]] = []
        if auth is None:
            self._auth_header: str | None = None
        else:
            token = base64.b64encode(auth.encode()).decode()
            self._auth_header = f"Basic {token}"

    # ------------------------------------------------------------ the verbs

    def get(self, url: str) -> bytes:
        """GET the bytes and verify them against the `.sha256` sibling.

        Returns the verified bytes.
        Completion is bytes plus the sibling with a matching digest: anything else is an error.
        """
        payload = self._send("GET", url)[2]
        sibling_url = f"{url}.sha256"
        try:
            marker = self._send("GET", sibling_url)[2]
        except HttpError as err:
            if err.status == 404:
                raise MarkerMissing(
                    f"markerless: {url}: the bytes exist but {sibling_url} is absent",
                    hint="publish the sha256 sibling, so the object is verifiable",
                ) from None
            raise
        digest, _ = parse_marker(marker)
        actual = hashlib.sha256(payload).hexdigest()
        if actual != digest:
            raise DigestMismatch(
                f"diverged: {url}: the marker says {digest}, the bytes hash to {actual}",
                hint="the remote object diverges from its marker: refuse, never overwrite, republish the pair",
            )
        return payload

    def put(self, url: str, data: bytes, *, sha: bool = True) -> str | None:
        """PUT the bytes, then PUT the `.sha256` marker of the same name.

        The marker strictly follows the bytes of the same name:
        a crash between the two leaves a markerless object, which every reader refuses.
        The marker's name field is the percent-decoded final path segment of the URL.
        With `sha=False` the marker is skipped (`--no-sha`).
        Returns the digest hex, or `None` when the marker was skipped.
        """
        name = _marker_name(url)
        _check_name(name)
        self._send("PUT", url, data)
        if not sha:
            return None
        digest = hashlib.sha256(data).hexdigest()
        self._send("PUT", f"{url}.sha256", format_marker(name, digest).encode())
        return digest

    def head(self, url: str) -> HeadInfo:
        """HEAD a URL: status and metadata.

        404 is a normal result, not an error.
        A 2xx without Content-Length reports `length=None`: the object is present but unverifiable, never absent.
        """
        try:
            status, headers, _ = self._send("HEAD", url)
        except HttpError as err:
            if err.status == 404:
                return HeadInfo(404, None, None)
            raise
        length = headers.get("Content-Length")
        return HeadInfo(
            status,
            int(length) if length is not None else None,
            headers.get("Content-Type"),
        )

    def sha(self, url: str) -> str:
        """GET the object and hash it, without verification (the `nxr sha` primitive)."""
        return hashlib.sha256(self._send("GET", url)[2]).hexdigest()

    def ls(self, base_url: str, repository: str) -> list[str]:
        """List a repository through the search API, following continuation tokens.

        Returns the raw asset paths, relative to the repository root.
        The walk is best-effort: the search API exists on common Nexus 3 releases but is not guaranteed.
        The server-side `group` parameter is deliberately not sent: on a real Nexus it matches Maven
        coordinates rather than raw paths, which makes nested listings lie (a filter, if ever needed, belongs client-side).
        """
        parts = urlsplit(base_url)
        endpoint = f"{parts.scheme}://{parts.netloc}/service/rest/v1/search/assets"
        first_page = f"{endpoint}?{urlencode({'repository': repository})}"
        paths: list[str] = []
        token: str | None = None
        for _page in range(MAX_SEARCH_PAGES):
            url = first_page if token is None else f"{first_page}&{urlencode({'continuationToken': token})}"
            try:
                _status, _headers, payload = self._send("GET", url)
            except HttpError as err:
                if err.status == 400:
                    raise SearchRepoMissing(
                        f"cannot enumerate: {url}: the search answered 400 for repository {repository!r}",
                        status=400,
                        hint="the repository is missing on the server or is not a raw repository",
                    ) from None
                if err.status == 404:
                    raise EnumerateError(
                        f"cannot enumerate: {url}: the search endpoint answered 404",
                        status=404,
                        hint="the server predates the search API: publish a manifest.json and read it instead",
                    ) from None
                raise
            try:
                page = json.loads(payload)
            except ValueError as err:
                raise EnumerateError(
                    f"cannot enumerate: {url}: the search response is not JSON: {err}"
                ) from None
            if not isinstance(page, dict):
                raise EnumerateError(
                    f"cannot enumerate: {url}: the search response is not a JSON object"
                ) from None
            for item in page.get("items") or []:
                path = item.get("path") if isinstance(item, dict) else None
                if isinstance(path, str):
                    paths.append(path)
            token = page.get("continuationToken")
            if not isinstance(token, str):
                return paths
        raise EnumerateError(
            f"cannot enumerate: {first_page}: search pagination exceeded {MAX_SEARCH_PAGES} pages",
            hint="the endpoint never stops emitting continuation tokens",
        )

    def channel_get(self, url: str) -> str | None:
        """Read a channel token file: exactly one line, `<token>\\n`.

        Returns `None` on 404: an unset channel is a normal state.
        """
        try:
            payload = self._send("GET", url)[2]
        except HttpError as err:
            if err.status == 404:
                return None
            raise
        try:
            text = payload.decode("utf-8")
        except UnicodeDecodeError:
            raise NxrError(
                f"channel: {url}: the file is not UTF-8 text",
                hint="rewrite the channel with channel_set",
            ) from None
        if len(text) < 2 or not text.endswith("\n") or "\r" in text or "\n" in text[:-1]:
            raise NxrError(
                f"channel: {url}: the file is not one token line",
                hint="rewrite the channel with channel_set",
            )
        return text[:-1]

    def channel_set(self, url: str, token: str) -> None:
        """Write the channel token file: exactly one line, `<token>\\n`."""
        if not token or "\n" in token or "\r" in token:
            raise NxrError(
                f"channel token {token!r} is not one line",
                hint="a channel token is one non-empty line with no CR",
            )
        self._send("PUT", url, f"{token}\n".encode())

    # ------------------------------------------------------------ the wire

    def _send(
        self, method: str, url: str, body: bytes | None = None
    ) -> tuple[int, http.client.HTTPMessage, bytes]:
        """One logical request through the retry loop, returning `(status, headers, payload)`.

        Transport failures and 5xx retry with backoff.
        A 429 honors its `Retry-After` pause in place of the backoff, clamped to 1..=60 seconds.
        A 401/403 fails fast with no retries, like every other unexpected status.
        """
        attempt = 0
        while True:
            attempt += 1
            try:
                status, headers, payload = self._attempt(method, url, body)
            except (OSError, http.client.HTTPException) as err:
                if attempt >= self.attempts:
                    raise TransportError(
                        f"transport: {url}: {type(err).__name__}: {err}",
                        hint="the connection broke or the server stalled: check the URL and the server, then retry",
                    ) from err
                time.sleep(self._backoff(attempt))
                continue
            if status in (401, 403):
                raise AuthError(
                    f"auth: {url}: HTTP {status}",
                    status=status,
                    hint='pass credentials as Nxr(auth="user:pass")',
                )
            if status == 429 or 500 <= status < 600:
                if attempt >= self.attempts:
                    raise TransportError(
                        f"transport: {url}: HTTP {status}",
                        status=status,
                        hint="the server kept failing: retry the operation later",
                    )
                # The Retry-After pause belongs to the rate limit alone: a 5xx always backs off.
                retry_after = headers.get("Retry-After") if status == 429 else None
                time.sleep(self._pause(retry_after, attempt))
                continue
            if 200 <= status < 300:
                return status, headers, payload
            raise HttpError(
                f"http: {url}: HTTP {status}",
                status=status,
                hint=_status_hint(status),
            )

    def _attempt(
        self, method: str, url: str, body: bytes | None
    ) -> tuple[int, http.client.HTTPMessage, bytes]:
        """One wire attempt on a fresh connection.

        `Accept-Encoding: identity` is pinned, so every byte on the wire is the stored byte and digests always describe the original.
        Redirects are returned as statuses, never followed: `http.client` does not auto-follow and the protocol forbids it.
        """
        parts = urlsplit(url)
        if parts.scheme == "http":
            conn: http.client.HTTPConnection = http.client.HTTPConnection(
                parts.hostname, parts.port, timeout=self.timeout
            )
        elif parts.scheme == "https":
            conn = http.client.HTTPSConnection(parts.hostname, parts.port, timeout=self.timeout)
        else:
            raise NxrError(
                f"unsupported URL scheme: {parts.scheme!r}", hint="use an http or https URL"
            )
        try:
            path = parts.path or "/"
            if parts.query:
                path = f"{path}?{parts.query}"
            self.requests.append((method, path))
            headers: dict[str, str] = {"Accept-Encoding": "identity"}
            if self._auth_header is not None:
                headers["Authorization"] = self._auth_header
            if body is not None:
                headers["Content-Length"] = str(len(body))
            conn.request(method, path, body=body, headers=headers)
            resp = conn.getresponse()
            payload = resp.read()
            return resp.status, resp.headers, payload
        finally:
            conn.close()

    def _backoff(self, attempt: int) -> float:
        """The pause after failed attempt `attempt` (1-based): 0.5s x 2^(n-1), capped, plus jitter."""
        return min(BASE_BACKOFF * 2 ** (attempt - 1), MAX_BACKOFF) + random.uniform(
            0, MAX_JITTER
        )

    def _pause(self, retry_after: str | None, attempt: int) -> float:
        """The pause before the next attempt: the `Retry-After` seconds when present, the backoff otherwise.

        Only the seconds form is honored, clamped to 1..=60 like the Rust transport.
        A negative or unparseable value, or the HTTP-date form, falls back to the regular backoff.
        """
        if retry_after is not None:
            try:
                secs = int(retry_after.strip())
            except ValueError:
                pass
            else:
                if secs >= 0:
                    return float(min(max(secs, 1), 60))
        return self._backoff(attempt)


def _marker_name(url: str) -> str:
    """The marker's name field: the percent-decoded final path segment of the URL.

    A trailing slash leaves an empty segment, which `_check_name` refuses.
    Valid `%XX` escapes decode, invalid ones stay literal, and the result is lossy UTF-8, like the Rust core.
    """
    path = urlsplit(url).path
    return unquote(path.rsplit("/", 1)[-1])


def _check_name(name: str) -> None:
    """Refuse a final URL segment that cannot carry a marker.

    A segment matches `[A-Za-z0-9._-]{1,255}` and is never `.` or `..`.
    The `.sha256` suffix is reserved: markers are written by `put`, never uploaded as artifacts.
    """
    if not _SEGMENT.match(name):
        raise UnsafeName(
            f"unsafe name: {name!r}", hint="a name segment matches [A-Za-z0-9._-]{1,255}"
        )
    if name in (".", ".."):
        raise UnsafeName(f"unsafe name: {name!r}", hint="a name segment is never . or ..")
    if name.endswith(".sha256"):
        raise UnsafeName(
            f"unsafe name: {name!r}: reserved suffix .sha256",
            hint="the .sha256 suffix belongs to markers, which put writes itself",
        )


def _status_hint(status: int) -> str:
    """The action line for a non-retryable status."""
    if 300 <= status < 400:
        return "redirects are never followed: publish to and read from the target URL directly"
    if status == 404:
        return "no such object"
    if status == 405:
        return "the repository refuses this method: a group repository is read-only"
    return ""
