---
name: lobo-adapter
description: Create or update lobo Python market-data adapters and runnable servers, including instrument discovery, snapshots, updates, trades, and L2/L3 feed protocols. Includes adapter documentation, complete examples, and test fixtures for use with an installed wheel without a library checkout.
---

# Create a lobo Python adapter

Deliver `adapter.py`, a runnable `server.py`, and usage/dependency instructions in the user's project. Use public `lobo` and `lobo.replay.adapters` APIs with an installed binary wheel. This skill works with any agent that reads Agent Skills and can run Python; no particular vendor, MCP server, or companion skill is required. Do not import `lobo._lobo`, build native code, or change the library to make an adapter work. Report a public-API gap if the requested behavior cannot be expressed.

## Read the bundled guide

Resolve paths relative to this installed `SKILL.md`, not the working directory. The following files contain the actual repository documentation and code, not instructions to fetch it. Read the custom-adapter guide and contract before implementing, then the server guides when hosting:

- [Custom adapters](references/custom-adapters.md): imports, complete finite packet example, expressions, L2/L3 actions, bootstrap discovery, subscriptions, channel maps, checksums, sources, lifecycle, and public `help()` entry points.
- [Adapter contract](references/adapter-contract.md): discovery versus book allocation, precision, snapshots, trades, and failure semantics.
- [Server API](references/server.md): `server_context`, registered books, integer units, commands, feed, HTTP errors, and observer policies.
- [Hosting custom adapters](references/hosted-adapters.md): runnable server, discovery/subscription endpoints, hosted UI differences, and consumer test commands.
- [Terminal behavior](references/terminal.md): ticker selection, scope, OHLC bars, simulation capabilities, and WebGPU requirements. Standalone viewer source controls differ from Python-hosted mode as explained in the hosting guide.

Choose a bundled complete declaration to adapt:

| Feed | Reference |
| --- | --- |
| JSON L2, WebSocket directory, exact decimals, checksums, public trades | [Kraken](references/examples/kraken.md) |
| JSON array L3, channel IDs, sequence checks, sharding, reconciliation | [Bitfinex](references/examples/bitfinex.md) |
| HTTP discovery, pagination/ranking, outcome IDs, public L2 books | [Polymarket](references/examples/polymarkets.md) |
| Binary L3, byte layouts, instrument keys, local/HTTP replay | [ITCH](references/examples/itch.md) |
| Existing order-server snapshots and commands | [Order feed](references/examples/order_feed.md) |

These are bundled source snapshots, with source paths and hashes in [sources.json](references/sources.json). Check installed signatures with `help(Protocol)`, `help(CustomAdapter)`, `help(lm.Book)`, and other relevant public classes/methods. Do not assume a bundled example matches every wheel version.

## Prepare the consumer environment

Run `python /path/to/this/skill/scripts/lobo_agent.py setup` from the user's project. Replace `/path/to/this/skill` with this skill's installed directory. The standard-library helper creates `.venv-lobo`, installs binary wheels only, checks publisher metadata and public APIs, and prints the interpreter path for subsequent commands. It currently uses CPython 3.14 or `uv`; `--python` selects an interpreter and `--venv` selects an environment. On Windows the returned path ends in `Scripts/python.exe`.

Use `--package /path/to/publisher.whl` or a publisher-supplied HTTPS wheel URL when needed; use `--distribution` if the distribution is renamed. The helper checks identity before installing the downloaded wheel, including the publisher's `lobo` and `loblib` repository names. A missing compatible wheel is a dependency gap; do not substitute a similarly named package or attempt a source build. An existing compatible consumer environment can be used directly.

Use available documentation/search tools for the requested venue's official protocol. Context7 is optional. MCP setup is optional and client-specific; `scripts/lobo_agent.py mcp` defaults to using existing tools without changing configuration. Its explicit `--client` options support `claude`, `cursor`, and `codex`; agents outside that list use their own tools or the bundled guides directly.

GitHub downloads are optional supplements. For a newer example, run the helper's `references --python <consumer-python> --example <name>` command. It uses installed package metadata, tries version tags, records a commit, and marks a default-branch fallback. If network access, credentials, or a release tag is unavailable, continue with the bundled content and public Python help. No clone or repository access is required to use this skill.

## Implement and verify

- Expose `build_adapter(...) -> CustomAdapter`; separate the protocol and discovery builders when useful for fixtures. Keep endpoints configurable for local HTTP/WebSocket tests.
- Register available instruments without opening all books. Choose a valid discovered default. `scope=None` follows live symbols as visited; explicit scope subscribes its members. Some bundled exchange examples fix a one-symbol scope: adapt that setting when the user needs discovery and switching. Rank/limit metadata rather than loading every book.
- Use exact integer atoms, preserve wire IDs, and declare mode, level, precision, timezone, and trade capability accurately. Snapshots replace state; L2 updates set absolute totals; trades must not subtract liquidity twice.
- Use `server_context(adapters=[adapter], port=...)` before starting the adapter. Print `server.url` with flushing, accept `--port 0`, support `--open`, and close on interruption. Use the wheel's bundled terminal. The hosting guide includes a complete server script.
- Validate imports and representative packets. This skill also contains [packet/simulation tests](assets/tests/test_custom_adapter.py), [local subscription tests](assets/tests/test_hosted_discovery.py), and [Polars tests](assets/tests/test_levels.py) to use as patterns. Adapt them to the delivered adapter; passing these library examples alone does not validate a new feed. For a full suite and browser workflow, use `lobo-adapter-tests` when installed, or the bundled hosting guide and local [fixture server](assets/fixture_server.py).

Hand off the run command, package version, checks actually performed, bundled source hashes or optional remote revision, and any remaining API limitation.
