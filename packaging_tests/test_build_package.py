"""Check tool selection and compatibility tests without downloading toolchains."""

import io
import json
import os
import subprocess
import zipfile
from pathlib import Path

import build_package
import pytest
from check_distributions import read_distribution


@pytest.fixture
def project(tmp_path: Path) -> Path:
    (tmp_path / "web").mkdir()
    (tmp_path / "web/package.json").write_text('{"engines": {"node": "24.x"}}')
    (tmp_path / ".package-test").mkdir()
    (tmp_path / "pyproject.toml").write_text(
        "[project]\nclassifiers = [\n"
        '"Programming Language :: Python :: 3.14",\n'
        '"Programming Language :: Python :: 3.11",\n'
        '"Programming Language :: Rust"]\n'
        '[tool.cibuildwheel]\ntest-requires = ["pytest>=9,<10"]\n'
    )
    return tmp_path


@pytest.mark.parametrize("cached_version", [None, "v26.0.0", "v24.1.0"])
def test_node_selection_ignores_the_shell_and_reuses_a_compatible_cache(
    project: Path, monkeypatch: pytest.MonkeyPatch, cached_version: str | None
) -> None:
    node_dir = project / ".package-test/node-24"
    bin_dir = build_package.environment_python(node_dir).parent
    node = bin_dir / ("node.exe" if os.name == "nt" else "node")
    if cached_version:
        bin_dir.mkdir(parents=True)
        node.touch()
    calls = []
    downloads = []
    releases = [{"version": value} for value in ("v26.0.0", "v24.9.0", "v24.10.0")]

    def download(*args: object, **_kwargs: object) -> io.BytesIO:
        downloads.append(args)
        return io.BytesIO(json.dumps(releases).encode())

    def run(command: list[str], **kwargs: object) -> subprocess.CompletedProcess[str]:
        calls.append((command, kwargs))
        return subprocess.CompletedProcess(command, 0, stdout=cached_version or "")

    monkeypatch.setattr(build_package, "urlopen", download)
    monkeypatch.setattr(build_package.shutil, "which", lambda *_args, **_kwargs: "npm")
    monkeypatch.setattr(build_package.subprocess, "run", run)
    original = {"PATH": "/existing/node26", "CARGO_TARGET_DIR": "target/package"}
    selected = build_package.node_environment(project, Path("builder"), original)
    assert selected["PATH"].split(os.pathsep)[0] == str(bin_dir)
    assert original["PATH"] == "/existing/node26"
    assert selected["CARGO_TARGET_DIR"] == "target/package"
    installations = [args for args, _ in calls if "nodeenv" in args]
    if cached_version == "v24.1.0":
        assert not installations
        assert not downloads
    else:
        assert len(installations) == 1
        assert "--node=24.10.0" in installations[0]
        assert "--config-file=" in installations[0]
    assert calls[-1][0] == [str(node), "--version"]


def test_each_supported_interpreter_installs_the_same_wheel(
    project: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    calls = []

    def run(command: list[str], **kwargs: object) -> None:
        calls.append((command, kwargs))

    monkeypatch.setattr(build_package.subprocess, "run", run)
    wheel = project / "lobo-0.1.0-cp311-abi3-platform.whl"
    build_package.test_wheel(project, Path("builder"), wheel, {"PATH": "/original"})
    creations = [args for args, _ in calls if "venv" in args]
    assert [args[args.index("--python") + 1] for args in creations] == [
        "cpython@3.11",
        "cpython@3.14",
    ]
    assert all("--no-project" in args for args in creations)
    installs = [args for args, _ in calls if "install" in args]
    assert len(installs) == 2
    assert all(
        str(wheel) in args and "--only-binary=:all:" in args for args in installs
    )
    assert all("pytest>=9,<10" in args for args in installs)
    runners = [
        args for args, _ in calls if args[-1] == str(project / "scripts/test_wheel.py")
    ]
    assert len(runners) == 2
    assert runners[0][0] != runners[1][0]
    assert all(not Path(args[0]).parent.parent.exists() for args in runners)


@pytest.mark.parametrize("tag", ["cp314-cp314-platform", "cp312-abi3-platform", ""])
def test_distribution_audit_rejects_wheels_that_do_not_support_python_311(
    tmp_path: Path, tag: str
) -> None:
    wheel = tmp_path / "lobo-0.1.0-test.whl"
    with zipfile.ZipFile(wheel, "w") as archive:
        archive.writestr("lobo-0.1.0.dist-info/METADATA", "Name: lobo\n")
        archive.writestr("lobo-0.1.0.dist-info/WHEEL", f"Tag: {tag}\n" if tag else "")
    with pytest.raises(ValueError, match="Python 3.11"):
        read_distribution(wheel)
