"""Build and optionally test portable Python wheels with isolated build tools."""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
import venv
from pathlib import Path
from urllib.request import urlopen

BUILD_TOOLS = (
    "build>=1.4,<2",
    "twine>=6.2,<7",
    "nodeenv>=1.10,<2",
    "abi3audit>=0.0.24,<1",
)


def environment_python(directory: Path) -> Path:
    """Locate the interpreter inside a virtual environment on this platform."""
    return directory / ("Scripts/python.exe" if os.name == "nt" else "bin/python")


def prepare_tools(root: Path, *, testing: bool) -> Path:
    """Install build dependencies without changing the invoking environment."""
    directory = root / ".package-test/build-tools"
    python = environment_python(directory)
    if not python.is_file():
        venv.EnvBuilder(with_pip=True).create(directory)
    requirements = [*BUILD_TOOLS, *(["uv>=0.11,<1"] if testing else [])]
    subprocess.run(
        [
            str(python),
            "-m",
            "pip",
            "install",
            "--disable-pip-version-check",
            *requirements,
        ],
        check=True,
    )
    return python


def node_environment(root: Path, python: Path, env: dict[str, str]) -> dict[str, str]:
    """Provision the web project's Node major locally and select it for children."""
    engine = json.loads((root / "web/package.json").read_text())["engines"]["node"]
    match = re.fullmatch(r"(\d+)\.x", engine)
    if match is None:
        raise ValueError(f"Expected a Node major such as 24.x, got {engine!r}")
    major = match[1]
    directory = root / f".package-test/node-{major}"
    bin_dir = directory / ("Scripts" if os.name == "nt" else "bin")
    node = bin_dir / ("node.exe" if os.name == "nt" else "node")
    selected = {**env, "PATH": os.pathsep.join((str(bin_dir), env.get("PATH", "")))}
    ready = False
    if node.is_file() and shutil.which("npm", path=str(bin_dir)):
        result = subprocess.run(
            [str(node), "--version"], capture_output=True, text=True, check=False
        )
        ready = result.returncode == 0 and result.stdout.strip().startswith(
            f"v{major}."
        )
    if not ready:
        # Resolve a complete version: nodeenv's released CLI expects x.y.z.
        with urlopen("https://nodejs.org/dist/index.json", timeout=30) as response:
            releases = json.load(response)
        versions = [
            item["version"][1:]
            for item in releases
            if re.fullmatch(rf"v{major}\.\d+\.\d+", item["version"])
        ]
        version = max(versions, key=lambda value: tuple(map(int, value.split("."))))
        if directory.exists():
            shutil.rmtree(directory)
        print(f"Preparing isolated Node.js {version}", flush=True)
        subprocess.run(
            [
                str(python),
                "-m",
                "nodeenv",
                "--config-file=",
                f"--node={version}",
                str(directory),
            ],
            env=env,
            check=True,
        )
    subprocess.run([str(node), "--version"], env=selected, check=True)
    return selected


def test_versions(root: Path) -> list[str]:
    """Use the same supported Python minors declared in the package classifiers."""
    project = tomllib.loads((root / "pyproject.toml").read_text())["project"]
    prefix = "Programming Language :: Python :: "
    versions = [
        value.removeprefix(prefix)
        for value in project["classifiers"]
        if re.fullmatch(r"Programming Language :: Python :: 3\.\d+", value)
    ]
    if not versions:
        raise ValueError("No supported Python versions are declared")
    return sorted(versions, key=lambda value: tuple(map(int, value.split("."))))


def test_wheel(root: Path, python: Path, wheel: Path, env: dict[str, str]) -> None:
    """Install the same wheel in isolated environments for every supported minor."""
    uv = [str(python), "-m", "uv"]
    env = {
        **env,
        "UV_CACHE_DIR": str(root / ".package-test/uv-cache"),
        "UV_PYTHON_INSTALL_DIR": str(root / ".package-test/pythons"),
    }
    config = tomllib.loads((root / "pyproject.toml").read_text())
    requirements = config["tool"]["cibuildwheel"]["test-requires"]
    for version in test_versions(root):
        print(f"Testing {wheel.name} on CPython {version}", flush=True)
        with tempfile.TemporaryDirectory(
            prefix=f"test-{version}-", dir=root / ".package-test"
        ) as temporary:
            installed = environment_python(Path(temporary))
            subprocess.run(
                [
                    *uv,
                    "venv",
                    "--no-project",
                    "--python",
                    f"cpython@{version}",
                    temporary,
                ],
                env=env,
                check=True,
            )
            subprocess.run(
                [
                    *uv,
                    "pip",
                    "install",
                    "--python",
                    str(installed),
                    "--only-binary=:all:",
                    str(wheel),
                    *requirements,
                ],
                env=env,
                check=True,
            )
            subprocess.run(
                [str(installed), str(root / "scripts/test_wheel.py")],
                env=env,
                check=True,
            )


def main() -> None:
    """Build an sdist and stable-ABI wheel, with optional cross-version testing."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--skip-web", action="store_true", help="reuse verified web assets"
    )
    parser.add_argument(
        "--sdist-only", action="store_true", help="build only the source distribution"
    )
    parser.add_argument(
        "--test",
        action="store_true",
        help="test on all supported Pythons, downloading missing interpreters",
    )
    parser.add_argument("--out-dir", type=Path, default=Path("dist"))
    args = parser.parse_args()
    if sys.version_info < (3, 11) or sys.implementation.name != "cpython":
        parser.error("Run this script with CPython 3.11 or newer")
    if args.test and args.sdist_only:
        parser.error("--test requires a wheel; omit --sdist-only")
    root = Path(__file__).resolve().parents[1]
    if shutil.which("cargo") is None:
        parser.error("Rust and Cargo are required to compile the native extension")
    python = prepare_tools(root, testing=args.test)
    env = {
        **os.environ,
        "CARGO_TARGET_DIR": str(root / "target/package"),
        "CARGO_BUILD_JOBS": os.environ.get("CARGO_BUILD_JOBS", "2"),
    }
    if not args.skip_web:
        web_env = node_environment(root, python, env)
        npm = shutil.which("npm", path=web_env["PATH"])
        if npm is None:
            parser.error("The isolated Node installation did not provide npm")
        for command in ([npm, "ci"], [npm, "run", "build:server"]):
            subprocess.run(command, cwd=root / "web", env=web_env, check=True)
    output = args.out_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    # Stage this run separately so stale wheels cannot be mistaken for its output.
    with tempfile.TemporaryDirectory(
        prefix="build-", dir=root / ".package-test"
    ) as temporary:
        stage = Path(temporary)
        # The default builds the wheel FROM the sdist, checking its completeness.
        subprocess.run(
            [
                str(python),
                "-m",
                "build",
                *(["--sdist"] if args.sdist_only else []),
                "--outdir",
                str(stage),
                "--config-setting",
                "build-args=--locked",
                str(root),
            ],
            env=env,
            check=True,
        )
        subprocess.run(
            [str(python), str(root / "scripts/check_distributions.py"), str(stage)],
            check=True,
        )
        artifacts = sorted([*stage.glob("*.whl"), *stage.glob("*.tar.gz")])
        subprocess.run(
            [str(python), "-m", "twine", "check", "--strict", *map(str, artifacts)],
            check=True,
        )
        wheels = list(stage.glob("*.whl"))
        for wheel in wheels:
            subprocess.run(
                [str(python), "-m", "abi3audit", "--strict", "--summary", str(wheel)],
                check=True,
            )
            if args.test:
                test_wheel(root, python, wheel, env)
        for artifact in artifacts:
            shutil.copy2(artifact, output / artifact.name)
            print(f"Built {output / artifact.name}", flush=True)


if __name__ == "__main__":
    main()
