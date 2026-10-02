#!/usr/bin/env python3
"""Package native binaries and derive a Homebrew formula from real checksums."""
import argparse
import hashlib
import io
import json
import re
import subprocess
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[4]
TARGETS = {
    "aarch64-apple-darwin": ("macos", "arm"),
    "x86_64-apple-darwin": ("macos", "intel"),
    "aarch64-unknown-linux-gnu": ("linux", "arm"),
    "x86_64-unknown-linux-gnu": ("linux", "intel"),
}
BOTTLE_TAGS = {
    "aarch64-apple-darwin": "arm64_monterey",
    "x86_64-apple-darwin": "monterey",
    "aarch64-unknown-linux-gnu": "arm64_linux",
    "x86_64-unknown-linux-gnu": "x86_64_linux",
}


def bottle(archive_path, target, version, formula_text):
    """Make a relocatable keg from the version-checked native archive.

    Bottles avoid Homebrew's source-build developer-tool requirements. Nothing
    is extracted onto the host; archive members are streamed into the keg.
    """
    destination = archive_path.parent / f"lobo-{version}.{BOTTLE_TAGS[target]}.bottle.tar.gz"
    prefix = f"lobo/{version}"
    completions = {"completions/lobo.bash": "etc/bash_completion.d/lobo",
                   "completions/_lobo": "share/zsh/site-functions/_lobo",
                   "completions/lobo.fish": "share/fish/vendor_completions.d/lobo.fish"}
    def add(tar, path, content, mode=0o644):
        entry = tarfile.TarInfo(f"{prefix}/{path}")
        entry.size = len(content)
        entry.mode = mode
        tar.addfile(entry, io.BytesIO(content))
    with tarfile.open(archive_path, "r:gz") as source, tarfile.open(destination, "w:gz") as tar:
        names = {entry.name for entry in source.getmembers()}
        if "bin/lobo" not in names:
            raise ValueError(f"native archive has no executable: {archive_path}")
        for entry in source.getmembers():
            if entry.isdir():
                continue
            if not entry.isfile() or entry.name.startswith("/") or ".." in Path(entry.name).parts:
                raise ValueError(f"unsafe native archive member: {entry.name}")
            path = completions.get(entry.name)
            if path is None:
                path = entry.name if entry.name.startswith("bin/") else f"share/doc/lobo/{entry.name}"
            with source.extractfile(entry) as content:
                add(tar, path, content.read(), entry.mode)
        receipt = {"built_as_bottle": True, "poured_from_bottle": False,
                   "runtime_dependencies": [], "arch": "arm64" if TARGETS[target][1] == "arm" else "x86_64",
                   "source": {"tap": "iamorlando/lobo", "spec": "stable",
                              "versions": {"stable": version, "version_scheme": 0}},
                   "built_on": {"os": "Macintosh" if TARGETS[target][0] == "macos" else "Linux"}}
        add(tar, "INSTALL_RECEIPT.json", json.dumps(receipt).encode())
        add(tar, ".brew/lobo.rb", formula_text.encode())
    return destination


def archive(binary, target, version, output):
    if target not in TARGETS:
        raise ValueError(f"unsupported release target: {target}")
    if not binary.is_file():
        raise ValueError(f"missing built binary: {binary}")
    expected = f"lobo {version}"
    actual = subprocess.check_output([str(binary.resolve()), "--version"], text=True).strip()
    if actual != expected:
        raise ValueError(f"binary version {actual!r} does not match {expected!r}")
    output.mkdir(parents=True, exist_ok=True)
    name = f"lobo-{version}-{target}"
    staging = output / name
    (staging / "completions").mkdir(parents=True, exist_ok=True)
    for shell, suffix in [("bash", "lobo.bash"), ("zsh", "_lobo"), ("fish", "lobo.fish")]:
        result = subprocess.check_output([str(binary.resolve()), "completions", shell])
        (staging / "completions" / suffix).write_bytes(result)
    destination = output / f"{name}.tar.gz"
    with tarfile.open(destination, "w:gz") as tar:
        tar.add(binary, arcname="bin/lobo")
        tar.add(ROOT / "rust/crates/lobo_cli/examples/tmux-dashboard.sh", arcname="bin/lobo-tmux")
        tar.add(ROOT / "LICENSE-MIT.md", arcname="LICENSE-MIT.md")
        tar.add(ROOT / "rust/crates/lobo_cli/README.md", arcname="README.md")
        tar.add(ROOT / "rust/crates/lobo_cli/docs/dashboard.png", arcname="docs/dashboard.png")
        tar.add(ROOT / "rust/crates/lobo_cli/docs/agents", arcname="docs/agents")
        tar.add(ROOT / "rust/crates/lobo_cli/packaging/RELEASE.md", arcname="packaging/RELEASE.md")
        tar.add(ROOT / "rust/crates/lobo_cli/docs/multipane.png", arcname="docs/multipane.png")
        tar.add(staging / "completions", arcname="completions")
        tar.add(ROOT / "skills/lobo-terminal", arcname="skills/lobo-terminal")
    digest = hashlib.sha256(destination.read_bytes()).hexdigest()
    (output / f"{name}.sha256").write_text(f"{digest}  {destination.name}\n")
    print(destination)


def formula(artifacts, version, base_url, output):
    blocks = []
    for os_name in ["macos", "linux"]:
        blocks.append(f"  on_{os_name} do")
        for target, (os, arch) in TARGETS.items():
            if os != os_name:
                continue
            filename = f"lobo-{version}-{target}.tar.gz"
            path = artifacts / filename
            if not path.is_file():
                raise ValueError(f"refusing to create incomplete formula: missing {filename}")
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            blocks += [f"    on_{arch} do", f'      url "{base_url.rstrip("/")}/{filename}"', f'      sha256 "{digest}"', "    end"]
        blocks.append("  end")
    template = (ROOT / "rust/crates/lobo_cli/packaging/lobo.rb.in").read_text()
    output.parent.mkdir(parents=True, exist_ok=True)
    source_formula = template.replace("@VERSION@", version).replace("@PLATFORMS@", "\n".join(blocks))
    bottle_lines = ["  bottle do", f'    root_url "{base_url.rstrip("/")}"']
    bottles = []
    for target in TARGETS:
        path = artifacts / f"lobo-{version}-{target}.tar.gz"
        packaged = bottle(path, target, version, source_formula.replace("@BOTTLES@", ""))
        digest = hashlib.sha256(packaged.read_bytes()).hexdigest()
        bottle_lines.append(f'    sha256 cellar: :any_skip_relocation, {BOTTLE_TAGS[target]}: "{digest}"')
        bottles.append(packaged)
    bottle_lines.append("  end")
    output.write_text(source_formula.replace("@BOTTLES@", "\n".join(bottle_lines)))
    sums = []
    for target in TARGETS:
        path = artifacts / f"lobo-{version}-{target}.tar.gz"
        sums.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}")
    for path in bottles:
        sums.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}")
    (artifacts / "SHA256SUMS").write_text("\n".join(sums) + "\n")
    print(output)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("action", choices=["archive", "formula"])
    p.add_argument("--version", required=True)
    p.add_argument("--binary", type=Path)
    p.add_argument("--target", choices=TARGETS)
    p.add_argument("--artifacts", type=Path, default=ROOT / "rust/crates/lobo_cli/dist")
    p.add_argument("--base-url")
    p.add_argument("--output", type=Path)
    args = p.parse_args()
    if not re.fullmatch(r"\d+\.\d+\.\d+(?:-[A-Za-z0-9.-]+)?", args.version):
        p.error("version must be a semantic version")
    try:
        if args.action == "archive":
            if not args.binary or not args.target:
                p.error("archive requires --binary and --target")
            archive(args.binary, args.target, args.version, args.artifacts)
        else:
            if not args.base_url or not args.output:
                p.error("formula requires --base-url and --output")
            if not re.fullmatch(r"https://[A-Za-z0-9._~:/%+-]+", args.base_url):
                p.error("base URL must be an HTTPS URL without quotes or whitespace")
            formula(args.artifacts, args.version, args.base_url, args.output)
    except (ValueError, OSError, subprocess.CalledProcessError) as exc:
        p.exit(1, f"error: {exc}\n")


if __name__ == "__main__":
    main()
