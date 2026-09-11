"""Host the existing standalone web app and its data-source routes locally."""

from __future__ import annotations

import json
import os
import re
from collections.abc import Iterator
from contextlib import contextmanager, suppress
from functools import partial
from http.client import HTTPException
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from socketserver import TCPServer
from threading import Lock, Thread
from time import monotonic
from typing import Any, TypedDict
from urllib.error import URLError
from urllib.parse import parse_qs, quote, unquote, urljoin, urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

NASDAQ_DIRECTORY = "https://emi.nasdaq.com/ITCH/Nasdaq%20ITCH/"
INPUT_CHUNK_BYTES = 1_048_576
MARKET_DATA_URL = "https://api-pub.bitfinex.com/v2/conf/pub:list:pair:exchange"
SESSION_NAME = re.compile(
    r"(?:\d{8}\.NASDAQ_ITCH50|S\d{6}-v50\.txt|itch50_[\w-]+)\.gz", re.IGNORECASE
)


class ReplayHTTPError(Exception):
    """An HTTP response that can be displayed by the existing replay UI."""

    def __init__(self, status: int, message: str) -> None:
        """Keep the response status alongside the user-facing message."""
        super().__init__(message)
        self.status = status


class NoRedirects(HTTPRedirectHandler):
    """Keep the proxy on the Nasdaq origin, just like the Next.js handler."""

    def redirect_request(self, req, fp, code, msg, headers, newurl) -> None:  # noqa: ARG002
        """Reject redirects before following an upstream Location header."""
        return


class Session(TypedDict):
    """A listed Nasdaq session and its compressed size in bytes."""

    name: str
    size: int


def parse_sessions(html: str) -> list[Session]:
    """Read only ITCH 5.0 gzip entries from the public Nasdaq directory."""
    base = urlsplit(NASDAQ_DIRECTORY)
    sessions: dict[str, Session] = {}
    for match in re.finditer(
        r'(\d+)\s*<a\s+href="([^"]+)"[^>]*>[^<]*</a>', html, re.IGNORECASE
    ):
        url = urlsplit(urljoin(NASDAQ_DIRECTORY, match[2]))
        name = unquote(url.path[len(base.path) :])
        size = int(match[1])
        if (
            url.scheme == base.scheme
            and url.netloc == base.netloc
            and url.path.startswith(base.path)
            and not url.query
            and not url.fragment
            and SESSION_NAME.fullmatch(name)
            and 0 < size <= 2**53 - 1
        ):
            sessions[name] = {"name": name, "size": size}
    return [sessions[name] for name in sorted(sessions, key=str.casefold)]


class NasdaqSessions:
    """Match the app's directory/range API without storing a replay file."""

    def __init__(self) -> None:
        """Create a shared directory cache and bounded HTTP client."""
        self.opener = build_opener(NoRedirects())
        self.lock = Lock()
        self.cached: list[Session] = []
        self.expires = 0.0

    def catalog(self) -> list[Session]:
        """Cache the small directory listing for five minutes."""
        with self.lock:
            if self.cached and monotonic() < self.expires:
                return self.cached
            request = Request(NASDAQ_DIRECTORY, headers={"Accept-Encoding": "identity"})
            with self.opener.open(request, timeout=15) as response:
                content = response.read(INPUT_CHUNK_BYTES + 1)
            if len(content) > INPUT_CHUNK_BYTES:
                raise ReplayHTTPError(502, "Nasdaq session directory is too large")
            sessions = parse_sessions(content.decode("utf-8"))
            if not sessions:
                raise ReplayHTTPError(
                    502, "No ITCH 5.0 gzip sessions are listed by Nasdaq"
                )
            self.cached = sessions
            self.expires = monotonic() + 300
            return sessions

    def get(self, query: str) -> tuple[str, bytes]:
        """Return the catalog or one exact, at-most-1-MiB compressed range."""
        values = parse_qs(query, keep_blank_values=True)
        if any(len(value) != 1 for value in values.values()):
            raise ReplayHTTPError(400, "Invalid Nasdaq session or byte range")
        name = values.get("file", [None])[0]
        try:
            offset = int(values.get("offset", ["0"])[0])
            length = int(values.get("length", [str(INPUT_CHUNK_BYTES)])[0])
        except ValueError as error:
            raise ReplayHTTPError(
                400, "Invalid Nasdaq session or byte range"
            ) from error
        if name is None:
            return "application/json", json.dumps(self.catalog()).encode()
        if (
            not SESSION_NAME.fullmatch(name)
            or not 0 <= offset <= 2**53 - 1
            or not 1 <= length <= INPUT_CHUNK_BYTES
        ):
            raise ReplayHTTPError(400, "Invalid Nasdaq session or byte range")
        session = next((item for item in self.catalog() if item["name"] == name), None)
        if session is None:
            raise ReplayHTTPError(
                404, "Session is not in the Nasdaq ITCH 5.0 directory"
            )
        if offset >= session["size"]:
            raise ReplayHTTPError(416, "Byte range is past the session end")
        end = min(offset + length, session["size"]) - 1
        request = Request(  # noqa: S310 — fixed HTTPS origin, validated filename
            NASDAQ_DIRECTORY + quote(name, safe=""),
            headers={"Range": f"bytes={offset}-{end}", "Accept-Encoding": "identity"},
        )
        with self.opener.open(request, timeout=30) as response:
            # Reject a full-file response before reading any of its body.
            if (
                response.status != 206
                or response.headers.get("Content-Range")
                != f"bytes {offset}-{end}/{session['size']}"
            ):
                raise ReplayHTTPError(
                    502, "Nasdaq did not return the requested byte range"
                )
            content = response.read(end - offset + 2)
        if len(content) != end - offset + 1:
            raise ReplayHTTPError(
                502, "Nasdaq session range was truncated or oversized"
            )
        return "application/octet-stream", content


def _market_data(query: str) -> tuple[str, bytes]:
    values = parse_qs(query, keep_blank_values=True)
    if values.get("url") != [MARKET_DATA_URL]:
        raise ReplayHTTPError(400, "Unsupported market data resource")
    request = Request(
        MARKET_DATA_URL,
        headers={
            "User-Agent": "lobo (https://github.com/iamorlando/lobo)",
            "Accept": "application/json",
        },
    )
    with build_opener(NoRedirects()).open(request, timeout=10) as response:
        body = response.read(INPUT_CHUNK_BYTES + 1)
    if len(body) > INPUT_CHUNK_BYTES:
        raise ReplayHTTPError(502, "Instrument directory too large")
    return "application/json", body


def _replay(query: str, path: Path) -> tuple[str, bytes]:
    values = parse_qs(query, keep_blank_values=True)
    try:
        with path.open("rb") as source:
            size = os.fstat(source.fileno()).st_size
            if "info" in values:
                return "application/json", json.dumps(
                    {"name": path.name, "size": size}
                ).encode()
            try:
                offset = int(values.get("offset", ["0"])[0])
                length = int(values.get("length", [str(INPUT_CHUNK_BYTES)])[0])
            except ValueError as error:
                raise ReplayHTTPError(400, "Invalid byte range") from error
            if not 0 <= offset <= 2**53 - 1 or not 1 <= length <= INPUT_CHUNK_BYTES:
                raise ReplayHTTPError(400, "Invalid byte range")
            source.seek(offset)
            return "application/octet-stream", source.read(length)
    except OSError as error:
        raise ReplayHTTPError(
            404,
            "Repository ITCH file unavailable. Select a local file or set LOBO_ITCH_PATH.",
        ) from error


class _DemoHandler(SimpleHTTPRequestHandler):
    def __init__(
        self,
        *args: Any,
        root: Path,
        nasdaq: NasdaqSessions,
        replay_path: Path,
        **kwargs: Any,
    ) -> None:
        self.nasdaq = nasdaq
        self.replay_path = replay_path
        self.extensions_map = {
            **SimpleHTTPRequestHandler.extensions_map,
            ".wasm": "application/wasm",
        }
        super().__init__(*args, directory=str(root), **kwargs)

    def log_message(self, format: str, *args: Any) -> None:
        # The UI displays stream progress; avoid logging every 1-MiB range.
        pass

    def _send_body(self, status: int, content_type: str, body: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        with suppress(BrokenPipeError, ConnectionResetError):
            self._get()

    def _get(self) -> None:
        url = urlsplit(self.path)
        if url.path == "/api/server-context":
            self._send_body(
                200,
                "application/json",
                json.dumps({"mode": "standalone"}).encode(),
            )
        elif url.path in {"/api/nasdaq-sessions", "/api/market-data", "/api/replay"}:
            try:
                if url.path == "/api/nasdaq-sessions":
                    content_type, body = self.nasdaq.get(url.query)
                elif url.path == "/api/market-data":
                    content_type, body = _market_data(url.query)
                else:
                    content_type, body = _replay(url.query, self.replay_path)
            except ReplayHTTPError as error:
                self._send_body(error.status, "text/plain", str(error).encode())
            except (URLError, OSError, HTTPException, UnicodeError) as error:
                self._send_body(
                    502,
                    "text/plain",
                    f"Cannot read the source: {error}".encode(),
                )
            else:
                self._send_body(200, content_type, body)
        elif url.path.startswith("/api/"):
            self._send_body(
                404,
                "text/plain",
                b"Use a hosted Nasdaq session or open a local replay file in the browser.",
            )
        else:
            super().do_GET()


class _DemoServer(ThreadingHTTPServer):
    """Bind the loopback address without waiting for reverse DNS."""

    def server_bind(self) -> None:
        """Use the numeric address as the name of this local server."""
        # HTTPServer.server_bind calls getfqdn(), which can stall on macOS.
        TCPServer.server_bind(self)
        self.server_name, self.server_port = self.server_address[:2]


@contextmanager
def demo_context(*, port: int = 0) -> Iterator[str]:
    """Serve the bundled standalone app and its existing data-source APIs."""
    root = Path(__file__).with_name("_web")
    if not (root / "index.html").is_file():
        raise RuntimeError(
            "The bundled web terminal is missing; rebuild the Python package"
        )
    replay_path = Path(
        os.environ.get("LOBO_ITCH_PATH", "data/NASDAQ/01302020.NASDAQ_ITCH50")
    ).resolve()
    handler = partial(
        _DemoHandler, root=root, nasdaq=NasdaqSessions(), replay_path=replay_path
    )
    with _DemoServer(("127.0.0.1", port), handler) as server:
        thread = Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            yield f"http://127.0.0.1:{server.server_address[1]}"
        finally:
            server.shutdown()
            thread.join()
