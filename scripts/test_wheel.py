"""Test the installed wheel in an isolated copy of tests and input fixtures."""

import os
import shutil
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory


def main() -> None:
    """Exercise installed native APIs without importing lobo from the checkout."""
    import lobo

    root = Path(__file__).resolve().parents[1]
    installed = Path(lobo.__file__).resolve()
    if installed.is_relative_to(root / "python"):
        raise RuntimeError(
            f"Install the wheel into a clean environment first: {installed}"
        )
    print(f"Testing installed package: {installed}", flush=True)
    with TemporaryDirectory(prefix="lobo-wheel-tests-") as temporary:
        stage = Path(temporary)
        for relative in (
            "python/tests/lobo",
            "python/examples",
            "packaging_tests/packaging",
            "rust/crates/lobo_adapters/tests/fixtures",
            "rust/crates/lobo_replay/tests/definitions",
        ):
            shutil.copytree(
                root / relative,
                stage / relative,
                ignore=shutil.ignore_patterns("__pycache__", "*.pyc"),
            )
        (stage / "pytest.ini").write_text("[pytest]\n", encoding="utf-8")
        env = dict(os.environ)
        # Only the copied examples are importable here; no lobo source is copied.
        env["PYTHONPATH"] = str(stage / "python")
        env.pop("PYTHONHOME", None)
        env["PYTEST_DISABLE_PLUGIN_AUTOLOAD"] = "1"
        subprocess.run(
            [
                sys.executable,
                "-m",
                "pytest",
                "-c",
                str(stage / "pytest.ini"),
                "--import-mode=importlib",
                "-q",
                str(stage / "python/tests/lobo"),
                str(stage / "packaging_tests/packaging"),
            ],
            cwd=stage,
            env=env,
            check=True,
            timeout=300,
        )


if __name__ == "__main__":
    main()
