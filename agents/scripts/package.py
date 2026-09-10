"""Embed repository guides and bundle universal skills as reproducible ZIPs.

Only reads source documentation and Python examples/tests. Consumer skills
include real files and never need this script or a repository checkout.
"""

import argparse
import hashlib
import json
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPO = ROOT.parent
SKILLS = REPO / "skills"
PLUGIN = ROOT / "plugins" / "lobo-adapters"
VERSION = "0.2.0"
REPOSITORY = "https://github.com/iamorlando/loblib"
SKILL_NAMES = ("lobo-adapter", "lobo-adapter-tests")


def excerpt(source, start, end=None):
    """Select explicit README boundaries; fail if a source heading changes."""
    begin = source.index(start)
    finish = source.index(end, begin) if end else len(source)
    return source[begin:finish].strip()


def embedded_resources():
    resources = {}
    sources = []

    def add(output, source, *, sections=None, code=False, title=None):
        data = (REPO / source).read_bytes()
        body = data.decode("utf-8")
        if sections:
            body = "\n\n".join(excerpt(body, *bounds) for bounds in sections)
        digest = hashlib.sha256(data).hexdigest()
        if output.endswith(".md"):
            if code:
                body = f"# {Path(source).parent.name} adapter example\n\n```python\n{body.rstrip()}\n```"
            if title:
                body = f"# {title}\n\n{body}"
            body = (
                f"> Bundled from `{source}` in {REPOSITORY}.\n"
                "> This content is included in the installed skill; no checkout or download is needed.\n"
                "> Check the installed wheel's public `help()` for version-specific signatures.\n\n"
                + body.strip()
                + "\n"
            )
        resources[output] = body.encode("utf-8")
        sources.append(
            {
                "file": output,
                "source": source,
                "source_sha256": digest,
                "bundled_sha256": hashlib.sha256(resources[output]).hexdigest(),
                "sections": sections,
            }
        )

    add(
        "references/custom-adapters.md",
        "rust/crates/lobo_replay/src/custom/README.md",
        sections=[
            ("# Custom adapters", "## Running and serving"),
            ("- `start()`", "Complete Python definitions"),
        ],
    )
    add(
        "references/server.md",
        "rust/crates/lobo_server/README.md",
        sections=[("```python", "## Transport and build")],
        title="Hosted Python books and order API",
    )
    add("references/hosted-adapters.md", "agents/references/hosted-adapters.md")
    add("references/adapter-contract.md", "agents/references/adapter-contract.md")
    add(
        "references/terminal.md",
        "web/README.md",
        sections=[
            ("- The symbol box autocompletes", "- Files must contain"),
            ("- **Book scope**", "- **iTerm theme**"),
            ("## OHLC bars", "The workspace fills"),
            ("Kraken supports the same four", "`PriceChangeEvent`"),
            ("## Order simulation and level queues", "An L3 limit simulation"),
            ("**No chart pixels", "To retain the whole market"),
        ],
        title="Terminal behavior and adapter checks",
    )
    for example in ("kraken", "bitfinex", "polymarkets", "itch", "order_feed"):
        add(
            f"references/examples/{example}.md",
            f"python/examples/{example}/adapter.py",
            code=True,
        )
    for name in ("test_custom_adapter.py", "test_hosted_discovery.py", "test_levels.py"):
        add(f"assets/tests/{name}", f"python/tests/lobo/{name}")
    add("assets/fixture_server.py", "agents/tests/fixture_server.py")
    resources["references/sources.json"] = (
        json.dumps(
            {
                "repository": REPOSITORY,
                "snapshot": "Repository working-tree sources at packaging time; hashes identify exact content.",
                "files": sources,
            },
            indent=2,
        )
        + "\n"
    ).encode("utf-8")
    return resources


def synchronize_file(target, content, check):
    if check:
        if not target.is_file() or target.read_bytes() != content:
            raise RuntimeError(
                f"Stale bundled resource: {target}. Run agents/scripts/package.py"
            )
    else:
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(content)


def synchronize(check=False):
    resources = embedded_resources()
    resources["scripts/lobo_agent.py"] = (ROOT / "scripts" / "lobo_agent.py").read_bytes()
    for name in SKILL_NAMES:
        skill = SKILLS / name
        for relative, content in resources.items():
            synchronize_file(skill / relative, content, check)
        # Retain the optional native plugin as a compatibility distribution.
        # Root skills are authoritative; its vendor UI metadata stays separate.
        for path in sorted(skill.rglob("*")):
            if path.is_file() and "__pycache__" not in path.parts:
                synchronize_file(
                    PLUGIN / "skills" / name / path.relative_to(skill),
                    path.read_bytes(),
                    check,
                )


def archive(source, output):
    with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
        for path in sorted(source.rglob("*")):
            if path.is_symlink():
                raise RuntimeError(
                    f"Skill bundles must not depend on symlinks: {path}"
                )
            if not path.is_file() or any(
                part in {"__pycache__", ".DS_Store", ".pytest_cache", ".ruff_cache"}
                for part in path.parts
            ):
                continue
            name = (Path(source.name) / path.relative_to(source)).as_posix()
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.create_system = 3
            info.external_attr = 0o100644 << 16
            bundle.writestr(info, path.read_bytes())
    return hashlib.sha256(output.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=ROOT / "dist")
    parser.add_argument(
        "--check",
        action="store_true",
        help="Check embedded guides, examples, tests, helpers, and optional plugin copies",
    )
    args = parser.parse_args()
    synchronize(args.check)
    if args.check:
        return
    args.out.mkdir(parents=True, exist_ok=True)
    archives = {}
    for source in [SKILLS, PLUGIN, *(SKILLS / name for name in SKILL_NAMES)]:
        name = f"{'lobo-skills' if source == SKILLS else source.name}-{VERSION}.zip"
        archives[name] = archive(source, args.out / name)
    (args.out / "SHA256SUMS").write_text(
        "".join(f"{digest}  {name}\n" for name, digest in archives.items()),
        encoding="utf-8",
    )
    print(json.dumps({"output": str(args.out.resolve()), "sha256": archives}, indent=2))


if __name__ == "__main__":
    main()
