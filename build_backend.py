"""PEP 517 hooks: distributions must contain the prebuilt terminal and WASM."""

from collections.abc import Mapping
from pathlib import Path

from maturin import (
    build_editable,
    get_requires_for_build_editable,
    get_requires_for_build_sdist,
    get_requires_for_build_wheel,
    prepare_metadata_for_build_editable,
    prepare_metadata_for_build_wheel,
)
from maturin import build_sdist as _build_sdist
from maturin import build_wheel as _build_wheel

__all__ = [
    "build_editable",
    "build_sdist",
    "build_wheel",
    "get_requires_for_build_editable",
    "get_requires_for_build_sdist",
    "get_requires_for_build_wheel",
    "prepare_metadata_for_build_editable",
    "prepare_metadata_for_build_wheel",
]


def check_web_assets() -> None:
    """Fail before compilation if a wheel would ship without its terminal."""
    assets = Path(__file__).parent / "python/lobo/_web"
    for name in ("index.html", "wasm/lobo_wasm.js", "wasm/lobo_wasm_bg.wasm"):
        if not (assets / name).is_file() or (assets / name).stat().st_size == 0:
            raise RuntimeError(
                f"Missing packaged web asset: {name}. From a checkout, run "
                "python scripts/build_package.py first. "
                "Official source distributions already contain these assets."
            )
    if not any((assets / "_next/static").rglob("*.js")):
        raise RuntimeError(
            "Packaged terminal JavaScript is missing; rebuild web assets."
        )


def build_wheel(
    wheel_directory: str,
    config_settings: Mapping[str, str | list[str]] | None = None,
    metadata_directory: str | None = None,
) -> str:
    """Build the native wheel with the previously built, portable web assets."""
    check_web_assets()
    return _build_wheel(wheel_directory, config_settings, metadata_directory)


def build_sdist(
    sdist_directory: str,
    config_settings: Mapping[str, str | list[str]] | None = None,
) -> str:
    """Include web assets so rebuilding an sdist requires no Node.js installation."""
    check_web_assets()
    return _build_sdist(sdist_directory, config_settings)
