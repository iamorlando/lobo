#!/usr/bin/env python3
"""Validate a Rust release and publish its remaining workspace packages in one batch."""

import argparse
import json
import os
from pathlib import Path
import shlex
import subprocess
import tomllib
import urllib.error
import urllib.parse
import urllib.request


ROOT = Path(__file__).resolve().parents[1]


def publishable(package):
    registries = package.get("publish")
    return registries is None or "crates-io" in registries


def validate_release(tag, workspace, packages):
    version = workspace["package"]["version"]
    if tag not in {f"rust-v{version}", f"v{version}"}:
        raise ValueError(f"Release tag {tag!r} must be rust-v{version} or v{version}")
    for package in packages:
        if publishable(package) and package["version"] != version:
            raise ValueError(f"{package['name']} must use workspace version {version}")
    for name, dependency in workspace.get("dependencies", {}).items():
        if isinstance(dependency, dict) and "path" in dependency:
            if dependency.get("version") != version:
                raise ValueError(f"workspace.dependencies.{name}.version must be {version}")
    return version


def already_published(package):
    name = urllib.parse.quote(package["name"], safe="")
    version = urllib.parse.quote(package["version"], safe="")
    request = urllib.request.Request(
        f"https://crates.io/api/v1/crates/{name}/{version}",
        headers={"User-Agent": "lobo-release-ci (https://github.com/iamorlando/lobo)"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            data = json.load(response)
    except urllib.error.HTTPError as error:
        if error.code == 404:
            return False
        raise
    # Yanked versions are also immutable and must not be uploaded again.
    if data["version"]["num"] != package["version"]:
        raise ValueError(f"Unexpected registry response for {package['name']}")
    return True


def publish_command(packages, exists=already_published):
    excluded = []
    pending = []
    for package in sorted(packages, key=lambda package: package["name"]):
        if not publishable(package) or exists(package):
            excluded.append(package["name"])
        else:
            pending.append(package["name"])
    if not pending:
        return None
    command = ["cargo", "publish", "--workspace", "--registry", "crates-io"]
    for name in excluded:
        command.extend(["--exclude", name])
    return command


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--check", action="store_true", help="Validate versions without registry access")
    mode.add_argument("--publish", action="store_true", help="Upload packages; otherwise only print the plan")
    args = parser.parse_args()
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=ROOT, text=True,
    ))
    members = set(metadata["workspace_members"])
    packages = [package for package in metadata["packages"] if package["id"] in members]
    version = validate_release(args.tag, workspace, packages)
    print(f"Validated Rust release {version} ({args.tag}).", flush=True)
    if args.check:
        return
    command = publish_command(packages)
    if command is None:
        print("All workspace packages at this version are already published.")
        return
    print(shlex.join(command), flush=True)
    if args.publish:
        if not os.environ.get("CARGO_REGISTRY_TOKEN"):
            raise ValueError("The cargo environment must provide the CARGO_REGISTRY_TOKEN secret")
        subprocess.run(command, cwd=ROOT, check=True)
    else:
        print("Plan only; no packages were uploaded.")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, urllib.error.URLError, subprocess.CalledProcessError) as error:
        raise SystemExit(str(error)) from error
