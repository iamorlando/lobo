"""Hosted example reads are bounded and preserve the app's range API."""

# ruff: noqa: D103, S101

import json
from pathlib import Path
from unittest.mock import MagicMock
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import urlopen

import pytest

from lobo import demo
from lobo.demo import (
    INPUT_CHUNK_BYTES,
    NASDAQ_DIRECTORY,
    NasdaqSessions,
    ReplayHTTPError,
    parse_sessions,
)

SESSION = "01302019.NASDAQ_ITCH50.gz"


def entry(name: str, size: int = 2_000_000) -> str:
    return f'{size} <A HREF="/ITCH/Nasdaq%20ITCH/{name}">{name}</A><br>'


def response(body: bytes, status: int = 200, content_range: str = "") -> MagicMock:
    result = MagicMock()
    result.__enter__.return_value = result
    result.status = status
    result.headers = {"Content-Range": content_range}
    result.read.side_effect = lambda limit: body[:limit]
    return result


def test_directory_excludes_sidecars_other_formats_and_external_urls() -> None:
    names = [SESSION, "S061226-v50.txt.gz", "itch50_05_15.gz"]
    html = "".join(
        entry(name) for name in [*names, "tvagg.gz", SESSION + ".md5sum", "old.zip"]
    )
    html += f'12 <a href="https://example.com/{SESSION}">external</a>'
    html += entry("../" + SESSION)
    html += entry("S010101-v50.txt.gz", size=2**53)
    assert parse_sessions(html) == [
        {"name": name, "size": 2_000_000} for name in sorted(names, key=str.casefold)
    ]


def test_catalog_is_cached_and_only_the_requested_range_is_read() -> None:
    api = NasdaqSessions()
    directory = response(entry(SESSION).encode())
    chunk = response(b"\x1f\x8b\x08\x00", 206, "bytes 5-8/2000000")
    with pytest.MonkeyPatch.context() as patch:
        opened = MagicMock(side_effect=[directory, chunk])
        patch.setattr(api.opener, "open", opened)
        assert json.loads(api.get("")[1]) == api.catalog()
        assert api.get(f"file={SESSION}&offset=5&length=4") == (
            "application/octet-stream",
            b"\x1f\x8b\x08\x00",
        )
    assert opened.call_count == 2
    assert opened.call_args_list[0].args[0].full_url == NASDAQ_DIRECTORY
    request = opened.call_args.args[0]
    assert request.full_url == NASDAQ_DIRECTORY + SESSION
    assert request.get_header("Range") == "bytes=5-8"
    assert request.get_header("Accept-encoding") == "identity"
    chunk.read.assert_called_once_with(5)


@pytest.mark.parametrize(
    ("query", "status"),
    [
        (f"file=../{SESSION}", 400),
        ("file=https://example.com/session.gz", 400),
        (f"file={SESSION}&offset=-1", 400),
        (f"file={SESSION}&offset=1.5", 400),
        (f"file={SESSION}&length={INPUT_CHUNK_BYTES + 1}", 400),
        (f"file={SESSION}&length=0", 400),
        (f"file={SESSION}&length=1&length=2", 400),
        ("file=S010101-v50.txt.gz", 404),
        (f"file={SESSION}&offset=2000000", 416),
    ],
)
def test_invalid_requests_never_open_the_upstream_file(query: str, status: int) -> None:
    api = NasdaqSessions()
    api.catalog = MagicMock(return_value=[{"name": SESSION, "size": 2_000_000}])
    with pytest.MonkeyPatch.context() as patch:
        opened = MagicMock()
        patch.setattr(api.opener, "open", opened)
        with pytest.raises(ReplayHTTPError) as error:
            api.get(query)
    assert error.value.status == status
    opened.assert_not_called()


@pytest.mark.parametrize(
    ("status", "content_range", "body", "read_body"),
    [
        (200, "", b"full file", False),
        (206, "bytes 0-3/999", b"abcd", False),
        (206, "bytes 0-3/2000000", b"abc", True),
        (206, "bytes 0-3/2000000", b"abcde", True),
    ],
)
def test_ignored_mismatched_or_incomplete_ranges_fail(
    status: int, content_range: str, body: bytes, *, read_body: bool
) -> None:
    api = NasdaqSessions()
    api.catalog = MagicMock(return_value=[{"name": SESSION, "size": 2_000_000}])
    chunk = response(body, status, content_range)
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(api.opener, "open", MagicMock(return_value=chunk))
        with pytest.raises(ReplayHTTPError) as error:
            api.get(f"file={SESSION}&length=4")
    assert error.value.status == 502
    if read_body:
        chunk.read.assert_called_once_with(5)
    else:
        chunk.read.assert_not_called()
    chunk.__exit__.assert_called_once()


def test_final_range_is_clamped_to_the_session_size() -> None:
    api = NasdaqSessions()
    api.catalog = MagicMock(return_value=[{"name": SESSION, "size": 10}])
    chunk = response(b"ab", 206, "bytes 8-9/10")
    with pytest.MonkeyPatch.context() as patch:
        opened = MagicMock(return_value=chunk)
        patch.setattr(api.opener, "open", opened)
        assert api.get(f"file={SESSION}&offset=8&length=4")[1] == b"ab"
    assert opened.call_args.args[0].get_header("Range") == "bytes=8-9"
    chunk.read.assert_called_once_with(3)


def test_local_app_starts_without_reverse_dns(monkeypatch: pytest.MonkeyPatch) -> None:
    lookup = MagicMock(
        side_effect=AssertionError("Local demo startup must not depend on reverse DNS")
    )
    monkeypatch.setattr("socket.gethostbyaddr", lookup)
    with (
        demo.demo_context() as url,
        urlopen(url + "/api/server-context", timeout=5) as reply,  # noqa: S310
    ):
        assert json.load(reply) == {"mode": "standalone"}
    lookup.assert_not_called()


def test_local_app_routes_preserve_standalone_mode_and_configured_file(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    path = tmp_path / "session.bin"
    path.write_bytes(b"abcdefgh")
    monkeypatch.setenv("LOBO_ITCH_PATH", str(path))
    with demo.demo_context() as url:
        with urlopen(url + "/api/server-context", timeout=5) as reply:  # noqa: S310
            assert json.load(reply) == {"mode": "standalone"}
        with urlopen(url + "/api/replay?info", timeout=5) as reply:  # noqa: S310
            assert json.load(reply) == {"name": "session.bin", "size": 8}
        with urlopen(url + "/api/replay?offset=2&length=3", timeout=5) as reply:  # noqa: S310
            assert reply.read() == b"cde"
        with pytest.raises(HTTPError) as error:
            urlopen(url + "/api/replay?offset=-1", timeout=5)  # noqa: S310
        assert error.value.code == 400
        with urlopen(url, timeout=5) as reply:  # noqa: S310
            assert b"/_next/static/" in reply.read()
        with urlopen(url + "/wasm/lobo_wasm_bg.wasm", timeout=5) as reply:  # noqa: S310
            assert reply.headers["Content-Type"] == "application/wasm"
            assert reply.read(4) == b"\0asm"


def test_live_directory_proxy_only_accepts_the_app_resource(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    upstream = response(b'[["BTCUSD","ETHUSD"]]')
    opener = MagicMock()
    opener.open.return_value = upstream
    monkeypatch.setattr(demo, "build_opener", lambda *_: opener)
    for target in (
        "file:///etc/passwd",
        "http://127.0.0.1/",
        demo.MARKET_DATA_URL + "?other=1",
    ):
        with pytest.raises(ReplayHTTPError) as error:
            demo._market_data(urlencode({"url": target}))
        assert error.value.status == 400
    opener.open.assert_not_called()
    assert demo._market_data(urlencode({"url": demo.MARKET_DATA_URL})) == (
        "application/json",
        b'[["BTCUSD","ETHUSD"]]',
    )
    request = opener.open.call_args.args[0]
    assert request.full_url == demo.MARKET_DATA_URL
    assert request.get_header("User-agent").startswith("lobo ")
    assert request.get_header("Accept") == "application/json"
    upstream.read.assert_called_once_with(INPUT_CHUNK_BYTES + 1)
    upstream.read.side_effect = lambda limit: b"x" * limit
    with pytest.raises(ReplayHTTPError) as error:
        demo._market_data(urlencode({"url": demo.MARKET_DATA_URL}))
    assert error.value.status == 502
