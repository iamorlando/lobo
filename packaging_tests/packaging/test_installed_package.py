"""Test behavior that editable installs and placeholder web fixtures cannot cover."""

# Local HTTP fixtures and the installed console executable only.
# ruff: noqa: S310, S603

import json
import queue
import subprocess
import sys
import threading
from importlib.metadata import distribution
from importlib.resources import files
from pathlib import Path
from urllib.request import urlopen

import pytest
from websockets.sync.client import connect

import lobo
from lobo import server_context
from lobo.cli import main


def read(url: str) -> bytes:
    with urlopen(url, timeout=5) as response:
        return response.read()


def test_wheel_metadata_and_assets() -> None:
    package = distribution("pylobo")
    assert package.version == "0.1.0"
    assert any(
        e.name == "lobo" and e.value == "lobo.cli:main" for e in package.entry_points
    )
    assert files(lobo).joinpath("py.typed").is_file()
    assert (
        files(lobo).joinpath("_web/wasm/lobo_wasm_bg.wasm").read_bytes()[:4]
        == b"\0asm"
    )


def test_real_packaged_terminal_and_websocket_use_the_chosen_server() -> None:
    with server_context(port=0) as first, server_context(port=0) as second:
        first.book("FIRST")
        second.book("SECOND")
        for server, symbol in ((first, "FIRST"), (second, "SECOND")):
            html = read(server.url).decode()
            assert "/_next/static/" in html
            config = json.loads(read(f"{server.url}/api/server-context"))
            assert config["mode"] == "server"
            assert [b["symbol"] for b in config["books"]] == [symbol]
            assert read(f"{server.url}/wasm/lobo_wasm_bg.wasm")[:4] == b"\0asm"
            with connect(
                server.url.replace("http://", "ws://") + "/api/feed", open_timeout=5
            ) as socket:
                directory = json.loads(socket.recv(timeout=5))
                assert directory["books"][0]["symbol"] == symbol
                snapshot = json.loads(socket.recv(timeout=5))
                assert snapshot["book"]["symbol"] == symbol
    assert first.closed and second.closed


def test_web_command_opens_exact_server_without_shell(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    opened = []
    monkeypatch.setattr("webbrowser.open", lambda url, **_: opened.append(url) or True)
    assert main(["web", "--server", "https://example.test:8443/"]) == 0
    assert opened == ["https://example.test:8443/"]
    with pytest.raises(SystemExit):
        main(["web", "--server", "file:///etc/passwd"])


def test_installed_console_server_starts_and_stops() -> None:
    # Launch the generated console script, not a module from the source tree.
    console = Path(sys.executable).parent / (
        "lobo.exe" if sys.platform == "win32" else "lobo"
    )
    child = subprocess.Popen(
        [str(console), "serve", "--port", "0", "--book", "CLI"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    lines = queue.Queue()
    threading.Thread(
        target=lambda: lines.put(child.stdout.readline()), daemon=True
    ).start()
    try:
        url = lines.get(timeout=30).strip()
        assert url.startswith("http://127.0.0.1:"), url
        books = json.loads(read(url + "/api/books"))
        assert [book["symbol"] for book in books] == ["CLI"]
        assert b"/_next/static/" in read(url)
    finally:
        child.terminate()
        try:
            child.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()
            child.communicate(timeout=5)
            raise
    if sys.platform != "win32":
        assert child.returncode == 0
