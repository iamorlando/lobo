---
name: lobo-terminal
description: Bootstrap Lobo terminal market views, tick/time/volume/notional bars, Bitfinex or Kraken feeds, replay, order simulations, and shared chart panes using the native lobo CLI. Inspect its commands, documentation, and APIs for agent automation.
---

# Lobo terminal

Use the installed native `lobo` binary as the authority for syntax and capabilities.
`lobo --help` identifies the terminal CLI; a Python installation may also provide
an unrelated `lobo` entrypoint. The native binary offers `completions api`.

Start with `lobo completions api`. It returns versioned JSON containing all CLI
commands and flags, interactive/session commands, ten presets, the documentation
index, API endpoints, and generated schemas. It works offline without a terminal,
GPU, or running server. Use `lobo docs TOPIC --json` to read just the relevant
bundled guide; `lobo docs all --json` retrieves the complete documentation bundle.
See [the guide index](references/index.md) when working without the executable.

## Bootstrap a view

Select `--preset NAME` and override settings explicitly. A supplied view command
wins over the preset's view. User flags and environment values win over defaults.

```sh
lobo --preset simulator
lobo --preset simulator --order-kind limit --price 99.98 --quantity 200
lobo --preset tick-bars --bar-size 250
lobo --preset tick-bars --source kraken --symbol ETH/USD --bar-size 100
lobo --preset bitfinex --symbol tBTCUSD
lobo --preset time-bars --file /path/session.gz --symbol AAPL --speed 10
```

For headless validation append `--renderer cpu --snapshot-json --capture-seconds
0.1`. Check the process exit code and parse stdout as JSON; errors go to stderr.
Capture duration is a wall-clock budget, not proof that a live feed is warm. Check
`warming`, `feed_error`, and actual quotes before claiming live data is ready.
Use a real terminal for an ongoing chart. Bare `lobo` starts the dashboard; it is
not a discovery command. Presets start views, not background services or files.
Read [presets](references/docs/presets.md) for all ten choices and bar semantics.

## Share one feed and control it

Start `lobo session --socket /tmp/lobo.sock --preset tick-bars`, then attach views
with `lobo candles --attach /tmp/lobo.sock` and `lobo book --attach /tmp/lobo.sock`.
Retain the session process handle and its chosen socket; do not take over another
session. Use `lobo ctl --attach /tmp/lobo.sock --execute 'aggregation ticks 250'`
for shared changes. `lobo dashboard --attach /tmp/lobo.sock --snapshot-json
--renderer cpu --capture-seconds 0.1` reads the shared state without a TTY.
Send `stop` when the user is finished with the session you own.

The six views, clock, simulation and scope are explained in
[terminal](references/docs/terminal.md). The complete interactive command catalog
is in [commands](references/docs/commands.md). Source and aggregation settings
belong to the session; pane themes and rendering settings stay local.

## Simulation and API use

L2 feeds support nonmutating market previews. L3 demo/replay also supports isolated
limit timelines and real FIFO inspection. Do not invent queue IDs for an L2 feed.
`:sim` simulates; `add`, `limit`, `market`, `cancel`, `execute`, `modify` and `remove`
mutate the demo or the explicitly configured hosted order endpoint. Submit these
only when the user requested that mutation; do not retry uncertain writes.
Terminal values use displayed units. HTTP order values use integer atoms.

Use `lobo api schema --json` for the actual snapshot/session/native order schemas,
and inspect the target server's `/api/server-context` and `/api/schema` before
using its hosted adapter API. Read [API use](references/docs/api.md),
[server](references/docs/server.md), and [hosted adapters](references/docs/hosted-adapters.md)
as needed. The [completions guide](references/docs/completions.md) documents the
agent discovery contract. This API discovers Lobo capabilities; it does not call
an LLM or require model credentials.
