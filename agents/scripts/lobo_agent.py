"""Portable setup and GitHub references for installed lobo Python packages.

The packager copies this file into each skill so either skill installs alone.
Only wheels, public Python APIs, and GitHub's Python examples are used.
"""

import argparse
import base64
import email
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import zipfile
from email.message import Message
from pathlib import Path
from urllib.error import HTTPError
from urllib.parse import quote, urlsplit
from urllib.request import Request, urlopen

REPOSITORIES = {"iamorlando/lobo", "iamorlando/lobo"}
CONTEXT7 = {"type": "http", "url": "https://mcp.context7.com/mcp"}
PLAYWRIGHT = {
    "command": "npx",
    "args": [
        "-y",
        "@playwright/mcp@latest",
        "--isolated",
        "--headless",
        "--browser",
        "chromium",
    ],
}


def run(command, timeout=300, **kwargs):
    return subprocess.run(command, check=True, timeout=timeout, **kwargs)


def register_codex(command, timeout=20):
    """Bound the CLI's automatic OAuth flow after saving a server entry."""
    process = subprocess.Popen(
        command,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=os.name != "nt",
    )
    try:
        process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        if os.name == "nt":
            subprocess.run(
                ["taskkill", "/PID", str(process.pid), "/T", "/F"],
                capture_output=True,
                timeout=10,
                check=False,
            )
        else:
            os.killpg(process.pid, signal.SIGKILL)
        process.communicate(timeout=5)
        return "Automatic authentication timed out; enable/login to the MCP server in your agent if needed."
    if process.returncode:
        raise RuntimeError("Codex could not register an MCP dependency")
    return None


def repository_from_urls(urls):
    for value in urls:
        label, _, url = value.partition(",")
        parsed = urlsplit(url.strip())
        if parsed.hostname == "github.com" and label.strip().lower() in {
            "repository",
            "source",
            "source code",
            "homepage",
        }:
            parts = parsed.path.strip("/").removesuffix(".git").split("/")
            if len(parts) == 2:
                return "/".join(parts)
    raise RuntimeError(
        "The package metadata does not identify a GitHub source repository"
    )


def validate_metadata(metadata, distribution="lobo"):
    name = metadata.get("Name", "").lower().replace("_", "-")
    if name != distribution.lower().replace("_", "-"):
        raise RuntimeError(f"Expected distribution {distribution!r}; got {name!r}")
    repository = repository_from_urls(metadata.get_all("Project-URL", []))
    if repository.lower() not in REPOSITORIES:
        raise RuntimeError(
            f"This is a different package: its repository is {repository}, expected one of {sorted(REPOSITORIES)}. "
            "Supply the intended published wheel with --package; do not install the unrelated PyPI package."
        )
    return repository


def installed_metadata(python, distribution):
    code = """import importlib.metadata,json,sys
try:
 m=importlib.metadata.metadata(sys.argv[1]); print(json.dumps(list(m.items())))
except importlib.metadata.PackageNotFoundError:
 print('null')
"""
    result = run(
        [str(python), "-c", code, distribution], capture_output=True, text=True
    )
    rows = json.loads(result.stdout)
    if rows is None:
        return None
    metadata = Message()
    for key, value in rows:
        metadata[key] = value
    return metadata


def python_in(directory):
    return Path(directory).resolve() / (
        "Scripts/python.exe" if os.name == "nt" else "bin/python"
    )


def wheel_request(value):
    """Reject direct source inputs, which pip may build despite only-binary."""
    parsed = urlsplit(value)
    if parsed.scheme in {"http", "https"}:
        if not parsed.path.lower().endswith(".whl"):
            raise RuntimeError("Package URLs must point to a binary .whl file")
    elif value.lower().endswith(".whl"):
        if not Path(value).is_file():
            raise RuntimeError("The supplied wheel file does not exist")
    elif not re.fullmatch(
        r"[A-Za-z0-9][A-Za-z0-9_.-]*(?:[<>=!~]+[A-Za-z0-9.*+!,<>=~_-]+)?", value
    ):
        raise RuntimeError(
            "Use a package requirement or binary wheel path/URL; source directories, archives, and VCS installs are unsupported"
        )
    return value


def prepare_environment(args):
    requested = wheel_request(args.package or args.distribution)
    destination = Path(args.venv).resolve()
    python = python_in(destination)
    if not python.exists():
        candidate = args.python or shutil.which("python3.14") or sys.executable
        compatible = (
            subprocess.run(
                [
                    candidate,
                    "-c",
                    "import sys;sys.exit(sys.version_info < (3,14) or sys.implementation.name != 'cpython')",
                ],
                timeout=20,
            ).returncode
            == 0
        )
        if compatible:
            run([candidate, "-m", "venv", str(destination)])
        elif not args.python and shutil.which("uv"):
            run(
                [
                    shutil.which("uv"),
                    "venv",
                    "--seed",
                    "--python",
                    "3.14",
                    str(destination),
                ]
            )
        else:
            raise RuntimeError(
                "Install CPython 3.14 (or uv), then rerun setup with --python pointing to it"
            )
    run(
        [
            str(python),
            "-c",
            "import sys;sys.exit(sys.version_info < (3,14) or sys.implementation.name != 'cpython')",
        ]
    )
    # uv-created environments may not have pip. ensurepip uses Python's bundled
    # installer and does not require a system-wide installation.
    run([str(python), "-m", "ensurepip", "--upgrade"], stdout=subprocess.DEVNULL)
    current = installed_metadata(python, args.distribution)
    if args.package or current is None:
        with tempfile.TemporaryDirectory(prefix="lobo-wheel-") as temporary:
            run(
                [
                    str(python),
                    "-m",
                    "pip",
                    "download",
                    "--only-binary=:all:",
                    "--no-deps",
                    "--dest",
                    temporary,
                    requested,
                ]
            )
            wheels = list(Path(temporary).glob("*.whl"))
            if len(wheels) != 1:
                raise RuntimeError("Setup requires exactly one compatible binary wheel")
            with zipfile.ZipFile(wheels[0]) as wheel:
                entries = [
                    p for p in wheel.namelist() if p.endswith(".dist-info/METADATA")
                ]
                if len(entries) != 1:
                    raise RuntimeError("Invalid wheel metadata")
                metadata = email.message_from_bytes(wheel.read(entries[0]))
            validate_metadata(metadata, args.distribution)
            run(
                [
                    str(python),
                    "-m",
                    "pip",
                    "install",
                    "--only-binary=:all:",
                    "--force-reinstall",
                    str(wheels[0]),
                ]
            )
    else:
        validate_metadata(current, args.distribution)
    if args.testing:
        run(
            [
                str(python),
                "-m",
                "pip",
                "install",
                "--only-binary=:all:",
                "pytest>=9,<10",
                "websockets>=15,<17",
                "playwright>=1.55,<2",
            ]
        )
        run([str(python), "-m", "playwright", "install", "chromium"])
    run(
        [
            str(python),
            "-c",
            "from lobo import server_context; from lobo.replay.adapters import CustomAdapter, Protocol; import polars; print('Public lobo Python APIs and Polars are ready')",
        ]
    )
    metadata = installed_metadata(python, args.distribution)
    return {
        "python": str(python),
        "distribution": args.distribution,
        "version": metadata["Version"],
        "repository": validate_metadata(metadata, args.distribution),
    }


def configure_mcp(client, testing=False):
    """Preserve existing servers; dependency configuration is best effort."""
    if client == "none":
        return {"mcp": "Using the agent's existing tools"}
    servers = {"context7": CONTEXT7, **({"playwright": PLAYWRIGHT} if testing else {})}
    try:
        if client == "codex":
            codex = shutil.which("codex")
            if not codex:
                raise RuntimeError("Codex CLI is unavailable")
            result = run(
                [codex, "mcp", "list", "--json"],
                timeout=20,
                capture_output=True,
                text=True,
            )
            existing = {item["name"].lower() for item in json.loads(result.stdout)}
            notes = []
            for name, config in servers.items():
                if name in existing:
                    continue
                command = [codex, "mcp", "add", name]
                command += (
                    ["--url", config["url"]]
                    if "url" in config
                    else ["--", config["command"], *config["args"]]
                )
                note = register_codex(command)
                if note:
                    notes.append(note)
            result = run(
                [codex, "mcp", "list", "--json"],
                timeout=20,
                capture_output=True,
                text=True,
            )
            registered = {item["name"].lower() for item in json.loads(result.stdout)}
            if not set(servers).issubset(registered):
                raise RuntimeError("An MCP dependency was not registered")
            return {
                "mcp": "Codex dependencies configured; reload the agent to expose newly added tools",
                "notes": notes,
            }
        path = Path(".cursor/mcp.json") if client == "cursor" else Path(".mcp.json")
        data = json.loads(path.read_text()) if path.exists() else {}
        existing = data.setdefault("mcpServers", {})
        for name, config in servers.items():
            if name.lower() not in {key.lower() for key in existing}:
                existing[name] = config
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(data, indent=2) + "\n")
        return {
            "mcp": f"Dependencies configured in {path}; enable them in the current agent"
        }
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        # Do not echo CLI output: an existing MCP configuration can contain keys.
        return {
            "mcp": f"Optional MCP setup unavailable ({type(error).__name__}). Use official docs and the bundled Playwright runner."
        }


def github(path):
    request = Request(
        "https://api.github.com/" + path,
        headers={
            "Accept": "application/vnd.github+json",
            "User-Agent": "lobo-adapters-skills",
        },
    )
    try:
        with urlopen(request, timeout=30) as response:
            return json.load(response)
    except HTTPError as error:
        if error.code not in (401, 403, 404) or not shutil.which("gh"):
            raise
        error.close()
        # gh uses the user's existing login without extracting credentials.
        result = run([shutil.which("gh"), "api", path], capture_output=True, text=True)
        return json.loads(result.stdout)


def references(args):
    metadata = installed_metadata(args.python, args.distribution)
    if metadata is None:
        raise RuntimeError("Install the Python package before resolving its examples")
    repository = validate_metadata(metadata, args.distribution)
    root = f"repos/{repository}"
    version = metadata["Version"]
    revision = None
    candidates = [args.ref] if args.ref else [f"v{version}", version]
    for candidate in candidates:
        try:
            revision = github(f"{root}/commits/{quote(candidate, safe='')}")["sha"]
            matched = candidate in {f"v{version}", version}
            break
        except (HTTPError, subprocess.CalledProcessError) as error:
            if args.ref:
                raise
            if isinstance(error, HTTPError):
                error.close()
    if revision is None:
        branch = github(root)["default_branch"]
        revision = github(f"{root}/commits/{quote(branch, safe='')}")["sha"]
        matched = False
    destination = Path(args.out).resolve()
    destination.mkdir(parents=True, exist_ok=True)
    manifest = {
        "distribution": args.distribution,
        "version": version,
        "repository": f"https://github.com/{repository}",
        "revision": revision,
        "version_matched_ref": matched,
        "requested_ref": args.ref,
        "reference_source": "explicit"
        if args.ref
        else "release_tag"
        if matched
        else "default_branch",
        "files": [],
    }
    directory = f"python/examples/{args.example}"
    listing = github(f"{root}/contents/{directory}?ref={revision}")
    paths = [
        item["path"]
        for item in listing
        if item["name"] in {"adapter.py", "server.py", "README.md"}
    ]
    if args.tests:
        listing = github(f"{root}/contents/python/tests/lobo?ref={revision}")
        names = {
            "test_custom_adapter.py",
            "test_hosted_discovery.py",
            "test_builtin_adapters.py",
            f"test_{args.example.removesuffix('s')}_adapter.py",
        }
        paths += [item["path"] for item in listing if item["name"] in names]
    if not any(path.endswith("/adapter.py") for path in paths):
        raise RuntimeError("That remote example does not contain adapter.py")
    for path in paths:
        # Write only known Python example/test filenames, never GitHub-provided
        # absolute paths or a checkout of the repository.
        data = github(f"{root}/contents/{path}?ref={revision}")
        output = destination / Path(path).name
        output.write_bytes(base64.b64decode(data["content"]))
        manifest["files"].append(
            {
                "file": output.name,
                "url": f"https://github.com/{repository}/blob/{revision}/{path}",
            }
        )
    (destination / "sources.json").write_text(json.dumps(manifest, indent=2) + "\n")
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    setup = commands.add_parser(
        "setup", help="Prepare an isolated environment from binary wheels"
    )
    setup.add_argument("--venv", default=".venv-lobo")
    setup.add_argument(
        "--python", help="CPython interpreter used to create the environment"
    )
    setup.add_argument(
        "--package",
        help="Wheel path/URL or package requirement from the intended publisher",
    )
    setup.add_argument("--distribution", default="lobo")
    setup.add_argument("--testing", action="store_true")
    mcp = commands.add_parser(
        "mcp",
        help="Optionally add MCP tools for a supported client; unnecessary for skill use",
    )
    mcp.add_argument(
        "--client", choices=["codex", "claude", "cursor", "none"], default="none"
    )
    mcp.add_argument("--testing", action="store_true")
    reference = commands.add_parser(
        "references", help="Fetch examples via installed package metadata and GitHub"
    )
    reference.add_argument("--python", default=str(python_in(".venv-lobo")))
    reference.add_argument("--distribution", default="lobo")
    reference.add_argument(
        "--example",
        choices=["kraken", "bitfinex", "polymarkets", "itch", "order_feed"],
        default="kraken",
    )
    reference.add_argument(
        "--ref",
        help="Explicit Git tag or commit; otherwise try release tags then record a default-branch fallback",
    )
    reference.add_argument("--out", default=".lobo-reference")
    reference.add_argument("--tests", action="store_true")
    args = parser.parse_args()
    try:
        result = (
            prepare_environment(args)
            if args.command == "setup"
            else configure_mcp(args.client, args.testing)
            if args.command == "mcp"
            else references(args)
        )
        print(json.dumps(result, indent=2))
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
        parser.exit(
            1,
            f"Setup failed: {error}\nUse a compatible published wheel and GitHub credentials for private examples. No Rust build is attempted.\n",
        )


if __name__ == "__main__":
    main()
