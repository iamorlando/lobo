"""Verify that ordinary PEP 517 builds cannot silently omit the terminal."""

from pathlib import Path

import build_backend
import pytest


def test_missing_assets_stop_before_the_native_build(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(build_backend, "__file__", str(tmp_path / "build_backend.py"))
    with pytest.raises(RuntimeError, match="Missing packaged web asset"):
        build_backend.build_wheel(str(tmp_path))
    with pytest.raises(RuntimeError, match="Missing packaged web asset"):
        build_backend.build_sdist(str(tmp_path))


def test_prebuilt_sdist_assets_delegate_without_node(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(build_backend, "__file__", str(tmp_path / "build_backend.py"))
    assets = tmp_path / "python/lobo/_web"
    for name in (
        "index.html",
        "wasm/lobo_wasm.js",
        "wasm/lobo_wasm_bg.wasm",
        "_next/static/chunks/app.js",
    ):
        path = assets / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"fixture")
    calls = []
    monkeypatch.setattr(
        build_backend,
        "_build_wheel",
        lambda *args: calls.append(args) or "test.whl",
    )
    settings = {"build-args": "--locked"}
    assert build_backend.build_wheel("out", settings, "metadata") == "test.whl"
    assert calls == [("out", settings, "metadata")]
    (assets / "_next/static/chunks/app.js").unlink()
    with pytest.raises(RuntimeError, match="JavaScript is missing"):
        build_backend.build_wheel("out")
