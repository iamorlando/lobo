#!/usr/bin/env python3
"""Bundle complete Lobo guides and Python API declarations for the terminal skill.

Reads library sources only. --check detects drift without writing. The CLI embeds
this same tree, so Vercel Skills and native installs receive identical resources.
"""
import argparse
import hashlib
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SKILL = ROOT / 'skills/lobo-terminal'
CLI = ROOT / 'rust/crates/lobo_cli'


def bundle():
    sources = {
        ROOT / 'README.md': 'overview',
        CLI / 'README.md': 'terminal',
        ROOT / 'web/README.md': 'web',
        ROOT / 'rust/crates/lobo_server/README.md': 'server',
        ROOT / 'rust/crates/lobo_adapters/README.md': 'adapters',
        ROOT / 'rust/crates/lobo_replay/src/custom/README.md': 'custom-adapters',
        ROOT / 'agents/references/adapter-contract.md': 'adapter-contract',
        ROOT / 'agents/references/hosted-adapters.md': 'hosted-adapters',
    }
    for path in sorted((ROOT / 'rust/crates').glob('*/README.md')):
        sources.setdefault(path, 'rust-' + path.parent.name.removeprefix('lobo_'))
    for path in sorted((ROOT / 'docs').glob('*.md')):
        sources[path] = path.stem
    for path in sorted((ROOT / 'python').rglob('README.md')):
        sources[path] = '-'.join(path.relative_to(ROOT).parts[:-1])
    for path in sorted((CLI / 'docs/agents').glob('*.md')):
        sources[path] = path.stem
    # All links between bundled guides remain local. Source/code/image links
    # resolve to the repository instead of nonexistent consumer checkout paths.
    def rewrite(source, content):
        def link(match):
            prefix, target, suffix = match.groups()
            if re.match(r'^[a-zA-Z][a-zA-Z0-9+.-]*:', target) or target.startswith('#'):
                return match.group()
            path, sep, anchor = target.partition('#')
            resolved = (source.parent / path).resolve()
            if resolved in sources:
                url = sources[resolved] + '.md' + (sep + anchor if sep else '')
            else:
                try:
                    relative = resolved.relative_to(ROOT)
                except ValueError:
                    return match.group()
                url = 'https://github.com/iamorlando/lobo/blob/main/' + relative.as_posix() + (sep + anchor if sep else '')
            return prefix + url + suffix
        return re.sub(r'(!?\[[^\]]*\]\()([^\s)]+)(\))', link, content)
    files, provenance = {'LICENSE': (ROOT / 'LICENSE-MIT.md').read_text()}, []
    for source, name in sorted(sources.items(), key=lambda x: x[1]):
        data = source.read_text()
        target = f'references/docs/{name}.md'
        files[target] = rewrite(source, data)
        provenance.append({'source': str(source.relative_to(ROOT)), 'resource': target,
                           'sha256': hashlib.sha256(source.read_bytes()).hexdigest()})
    for source in sorted((ROOT / 'python/lobo').rglob('*.pyi')):
        target = 'references/python-api/' + source.relative_to(ROOT / 'python/lobo').as_posix()
        files[target] = source.read_text()
        provenance.append({'source': str(source.relative_to(ROOT)), 'resource': target,
                           'sha256': hashlib.sha256(source.read_bytes()).hexdigest()})
    index = '# Bundled documentation\n\nComplete maintained guides from this release. The installed binary is the\nauthority for flags (`lobo completions api`) and wire types (`lobo api schema\n--json`). Library Rustdoc and Python API declarations supplement these guides;\nthe Python declarations bundled below describe this repository version.\n\n'
    for source, name in sorted(sources.items(), key=lambda x: x[1]):
        index += f'- [{name}](docs/{name}.md) — `{source.relative_to(ROOT)}`\n'
    index += '\n## Python API declarations\n\n'
    for target in files:
        if target.startswith('references/python-api/'):
            index += f'- [{target.removeprefix("references/python-api/")}]({target.removeprefix("references/")})\n'
    files['references/index.md'] = index
    files['references/sources.json'] = json.dumps(provenance, indent=2) + '\n'
    return files


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    stale = []
    generated = bundle()
    existing = set()
    for parent in ['references/docs', 'references/python-api']:
        existing.update(str(p.relative_to(SKILL)) for p in (SKILL / parent).rglob('*') if p.is_file())
    for name in sorted(existing - generated.keys()):
        if args.check:
            stale.append(name + ' (obsolete)')
        else:
            (SKILL / name).unlink()
    for name, content in generated.items():
        path = SKILL / name
        if args.check:
            if not path.is_file() or path.read_text() != content:
                stale.append(name)
        else:
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
    if stale:
        parser.exit(1, 'Stale terminal skill resources; run agents/scripts/package_terminal.py:\n' + '\n'.join(stale) + '\n')
    print('Terminal skill resources verified' if args.check else 'Terminal skill resources bundled')


if __name__ == '__main__':
    main()
