"""Audit built wheel/sdist contents before local installation or publication."""

import argparse
import tarfile
import zipfile
from email.parser import BytesParser
from pathlib import Path


def read_distribution(path: Path) -> tuple[dict[str, bytes], str, bytes]:
    """Read an archive and verify the files specific to its distribution format."""
    if path.suffix == ".whl":
        with zipfile.ZipFile(path) as archive:
            files = {
                n: archive.read(n) for n in archive.namelist() if not n.endswith("/")
            }
        prefix = "lobo/"
        metadata = next(
            v for k, v in files.items() if k.endswith(".dist-info/METADATA")
        )
        wheel_metadata = next(
            v for k, v in files.items() if k.endswith(".dist-info/WHEEL")
        )
        tags = BytesParser().parsebytes(wheel_metadata).get_all("Tag", [])
        if not tags or any(not tag.startswith("cp311-abi3-") for tag in tags):
            raise ValueError(f"Expected a Python 3.11+ stable-ABI wheel, got {tags}")
        extensions = [
            n
            for n in files
            if n.startswith("lobo/_lobo.") and n.endswith((".so", ".pyd"))
        ]
        if len(extensions) != 1:
            raise ValueError(f"Expected one native extension, got {extensions}")
        for required in (
            "cli.py",
            "__main__.py",
            "py.typed",
            "replay/adapters/__init__.py",
        ):
            if prefix + required not in files:
                raise ValueError(f"Missing {required}")
        entrypoints = next(
            v for k, v in files.items() if k.endswith(".dist-info/entry_points.txt")
        )
        if b"lobo.cli:main" not in entrypoints:
            raise ValueError("Missing lobo console entry point")
    else:
        with tarfile.open(path, "r:gz") as archive:
            files = {
                m.name.split("/", 1)[1]: archive.extractfile(m).read()
                for m in archive.getmembers()
                if m.isfile()
            }
        prefix = "python/lobo/"
        metadata = files["PKG-INFO"]
        for required in (
            "build_backend.py",
            "pyproject.toml",
            "Cargo.lock",
            "scripts/test_wheel.py",
            "packaging_tests/packaging/test_installed_package.py",
            "python/tests/lobo/test_builtin_adapters.py",
            "python/examples/kraken/adapter.py",
            "rust/crates/lobo_adapters/tests/fixtures/kraken_snapshot.json",
            "rust/crates/lobo_replay/tests/definitions/server.json",
        ):
            if required not in files:
                raise ValueError(f"Missing sdist file {required}")
    return files, prefix, metadata


def check(path: Path) -> None:
    """Reject missing assets, native extensions, and unintended package contents."""
    files, prefix, metadata = read_distribution(path)
    parsed = BytesParser().parsebytes(metadata)
    if parsed["Name"] != "pylobo" or not parsed.get_payload().strip():
        raise ValueError("Missing package name or README metadata")
    dependencies = parsed.get_all("Requires-Dist", [])
    if not any(d.startswith("polars==1.43.2") for d in dependencies):
        raise ValueError(f"Native/Python Polars versions must match: {dependencies}")
    if any(not d.startswith(("polars", "tzdata")) for d in dependencies):
        raise ValueError(f"Unexpected runtime dependencies: {dependencies}")
    for name in ("index.html", "wasm/lobo_wasm.js", "wasm/lobo_wasm_bg.wasm"):
        if not files.get(prefix + "_web/" + name):
            raise ValueError(f"Missing packaged web asset {name}")
    if not any(
        n.startswith(prefix + "_web/_next/static/") and n.endswith(".js") for n in files
    ):
        raise ValueError("Missing Next.js client chunks")
    for name in files:
        parts = Path(name).parts
        if any(
            p in {"__pycache__", "node_modules", ".env.local", ".DS_Store"}
            for p in parts
        ):
            raise ValueError(f"Unintended artifact content: {name}")
    print(f"Verified {path.name}: native package metadata and bundled terminal")


def main() -> None:
    """Check all distributions in a directory."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    args = parser.parse_args()
    artifacts = sorted(
        [*args.directory.glob("*.whl"), *args.directory.glob("*.tar.gz")]
    )
    if not artifacts:
        parser.error("no distributions found")
    for path in artifacts:
        check(path)


if __name__ == "__main__":
    main()
