# Lobo adapter skills

Two self-contained [Agent Skills](https://skills.sh/docs) for creating and testing
market-data adapters with an installed lobo Python wheel:

| Skill | Result |
| --- | --- |
| `lobo-adapter` | Adapter declaration, instrument discovery, and a runnable server using the wheel's bundled terminal. |
| `lobo-adapter-tests` | Deterministic packet/transport tests, Polars checks, and browser inspection. |

The canonical skills live in the repository-root `skills/` directory. They use
standard `SKILL.md` files and bundled resources, so they work with Claude Code,
Cursor, Gemini CLI, Codex, and other compatible agents. Neither requires an
OpenAI plugin, vendor CLI, MCP server, library checkout, or GitHub access at use
time. Python execution requires a compatible wheel; Rust and web build tools are
not needed.

## Install through skills.sh

After these files are published to the GitHub repository, install from any
consumer project:

```sh
npx skills add iamorlando/loblib --skill lobo-adapter lobo-adapter-tests --copy
```

The [Vercel skills CLI](https://skills.sh/docs/cli) prompts for target agents.
Use `--global` for all projects, or select agents explicitly, for example:

```sh
npx skills add iamorlando/loblib --skill lobo-adapter lobo-adapter-tests --agent claude-code cursor gemini-cli --copy
```

Either skill can be installed alone. The CLI discovers the root `skills/`
directory; no plugin-specific tree URL or npm publication is needed. Installing
from a private repository still requires access at installation time. Public
installation without credentials requires publishing the skills in a public
repository or mirror. The installed skills then carry their documentation with
them, including when installed globally or copied into another project.

Example requests:

- “Use lobo-adapter to create a live adapter for this feed, with discovery and a runnable server.”
- “Use lobo-adapter-tests to test my adapter's packets and subscriptions and inspect its terminal.”

## Included in each skill

- Relevant content from `rust/crates/lobo_replay/src/custom/README.md`, including complete packet and bootstrap examples, expression/action semantics, transport, and lifecycle.
- Consumer sections of `rust/crates/lobo_server/README.md`, covering the Python server, policies, order API, feed, and errors.
- Relevant `web/README.md` sections about selection, scope, OHLC, simulations, and GPU rendering, plus a guide distinguishing hosted adapters from the standalone viewer.
- Full Python declarations for Kraken, Bitfinex, Polymarket, ITCH, and order-feed adapters.
- Runnable packet, binary replay, simulation, hosted-discovery, and Polars reference tests, plus a local browser fixture server.
- A standard-library setup helper and `references/sources.json` with source paths and SHA-256 hashes.

The test skill additionally includes a Python Playwright runner. All links and
resources are local to each skill. GitHub refreshes are optional; installed
public `help()` resolves differences between a bundled snapshot and a wheel.
Maintainer-only Rust/build instructions are excluded from the embedded guides.

## Consumer setup

Run the helper from the consumer project using its installed path:

```sh
python /path/to/installed/skill/scripts/lobo_agent.py setup --package /path/to/publisher.whl
```

Omit `--package` when the intended distribution is available from the configured
package index. The helper checks publisher metadata before installing a downloaded
wheel; a similarly named distribution is rejected. Both the publisher's legacy
`iamorlando/lobo` and current `iamorlando/loblib` metadata are accepted. Use
`--distribution` for a renamed distribution and `--venv` for another environment.
The helper currently creates a CPython 3.14 environment (using `uv` if needed) and
prints the interpreter path. An existing compatible environment can be used
directly. No source build is attempted.

For tests, `setup --testing` also installs pytest, websockets, Python Playwright,
and Chromium. Then run the bundled tests with that interpreter:

```sh
<consumer-python> -m pytest /path/to/installed/skill/assets/tests -q
<consumer-python> /path/to/installed/lobo-adapter-tests/scripts/browser_smoke.py --server server.py --switch-symbol --require-quotes
```

Baseline tests demonstrate the library contract; adapt them to construct the
user's own adapter. The local `assets/fixture_server.py` needs only loopback
connections and supplies deterministic quotes and a subscription log. Browser
checks need WebGPU; Linux may also need Chromium system libraries. See the skill
for expected results and artifacts.

MCP integration is optional. Reuse available tools; the helper's `mcp` command
defaults to making no configuration changes. Explicit `--client claude`, `cursor`,
or `codex` can configure Context7 and, with `--testing`, Playwright MCP. Any other
agent can use the bundled guides, official documentation, and Python browser
runner. The skills CLI does not register MCP servers or install Python packages.

## Maintain and verify

Edit skill instructions and the browser runner under `skills/`. Shared authored
guides live in `agents/references/`; edit the shared helper at
`agents/scripts/lobo_agent.py`. README excerpts, Python example/test copies, and
provenance manifests are generated by the packager from repository sources:

```sh
python agents/scripts/package.py
python -m unittest discover -s agents/tests -v
python agents/scripts/package.py --check
```

Commit the generated resources under `skills/` so GitHub installations include
them. `--check` detects stale README excerpts, examples, tests, helpers, and plugin
copies without modifying sources. Packaging only reads files under `rust/`.

`agents/dist/` contains a universal `lobo-skills-<version>.zip`, one ZIP per skill,
and SHA-256 checksums. An optional native plugin remains under
`agents/plugins/lobo-adapters/` for existing users; its skills are generated copies
of the canonical skills. Only that compatibility wrapper carries vendor UI metadata
and MCP declarations. Universal installs do not use it.

For a local installer check from a separate scratch project:

```sh
npx skills add /path/to/loblib --skill lobo-adapter lobo-adapter-tests --agent claude-code gemini-cli --copy -y
```

The root path exercises normal repository discovery. To test a ZIP, extract it
and pass the extracted `skills` directory (or individual skill) to the same CLI.
