> Bundled from `agents/references/hosted-adapters.md` in https://github.com/iamorlando/lobo.
> This content is included in the installed skill; no checkout or download is needed.
> Check the installed wheel's public `help()` for version-specific signatures.

# Hosting and inspecting a custom adapter

This consumer guide combines the server README with the public Python examples,
`test_custom_adapter.py`, `test_hosted_discovery.py`, and the browser runner.
The terminal's static assets and WebAssembly ship in the binary wheel. Use that
installed app; repository commands such as `make py`, `make web`, Cargo, and npm
builds are for library maintainers and are not consumer prerequisites.

## Runnable server

Given an `adapter.py` exposing `build_adapter()`, save this as `server.py`:

```python
import argparse
import webbrowser
from threading import Event

from lobo import server_context

if __package__:
    from .adapter import build_adapter
else:
    from adapter import build_adapter


def main():
    parser = argparse.ArgumentParser(description="Serve a market-data adapter")
    parser.add_argument("--port", type=int, default=8000)
    parser.add_argument("--open", action="store_true")
    args = parser.parse_args()
    adapter = build_adapter()
    # Attach before start()/run(); server_context starts the source itself.
    with server_context(adapters=[adapter], port=args.port) as server:
        print(server.url, flush=True)
        if args.open:
            webbrowser.open(server.url)
        try:
            Event().wait()
        except KeyboardInterrupt:
            pass


if __name__ == "__main__":
    main()
```

Run `python server.py --port 0 --open` with the consumer environment's interpreter.
The adapter builder must choose a valid discovered symbol and any required user
configuration. `port=0` chooses an available loopback port. The server context
closes its resources on exit. Use `adapter.wait()` only for finite sources, never
as the shutdown strategy for an unbounded live feed.

## Discovery, subscriptions, and hosted HTTP behavior

`GET /api/server-context` describes the actual terminal deployment. Inspect its
`adapters` entries instead of assuming the standalone viewer's source catalog.
Each attached custom adapter advertises `observer`, `level`, `endpoint`, and
`subscriptionsEndpoint` fields. For adapter index zero the observed feed path is
`/api/adapters/0/feed`; use the advertised paths where available.

A feed connection receives an initial `snapshot` with `instruments` and `actions`,
then ordered `update` messages; heartbeat packets may appear between updates.
Directory instruments can exist before they have a book action or quote. A
reconnecting client must reconstruct from a fresh snapshot and respect sequence
numbers. Do not read a directory count as a count of allocated books.

POST JSON to the advertised `subscriptionsEndpoint`:

```json
{"selected": "BBB"}
```

This selects and requests BBB if allowed. To request a set and choose its view:

```json
{"symbols": ["BBB", "CCC"], "selected": "BBB"}
```

Use actual discovered symbols. `scope=None` permits subscribe-as-visited behavior
for live adapters; a Python `scope=["AAA"]` limits available books and rejects BBB.
An invalid member rejects the whole request with HTTP 422 before subscribing any
member. Explicit scope members are subscribed; scope is separate from the initial
`symbol`. Visited live subscriptions can remain active when selection changes.
The local fixture in `assets/tests/test_hosted_discovery.py` demonstrates this API
and records the exact requests received by the upstream WebSocket.

`POST /api/adapters/{index}/orders` takes
`{"book": "SYMBOL", "command": {...}}` for an attached adapter. The `command`
uses the order schema in the bundled server guide. Standalone Python books instead
use `/api/books/{symbol}/orders`. Tests should check both the HTTP response and
public adapter state/feed output. Do not send fixture orders to a live endpoint.

## Hosted terminal checks

A browser needs WebGPU and a secure context: loopback HTTP works; remote hosting
requires HTTPS. Standalone Python books run in live L3 mode with source and scope
selectors hidden. Attached adapters describe their own capabilities; consult
`/api/server-context` and the actual rendered controls before applying checks.
The standalone viewer's Nasdaq/local-file/source-switching workflow is not a
requirement for a Python adapter server.

The bundled runner uses these existing UI hooks:

- `#ticker`: selected symbol input.
- `#ticker-options [role="option"]`: instrument suggestions, with the symbol in `strong`.
- `.bid-quote strong` and `.ask-quote strong`: displayed best quotes.
- `.quote-panel`: quote panel text.

Wait for actual directory entries and known non-empty snapshots. Select a second
symbol through autocomplete and verify a successful subscription POST as well as
the displayed selection. Assert the upstream fixture saw only the intended
subscriptions. Explicit scope can be tested through Python construction and the
HTTP subscription API even if a scope UI control is hidden.

Use the bundled terminal guide for expected OHLC and simulation behavior: L2 has
aggregate market previews and no limit/FIFO simulation; L3 supports queues and
separate limit simulations. Trade events drive OHLC bars; cancellations or L2
quantity reductions alone must not fabricate volume. Simulation should leave the
main book unchanged. Missing WebGPU is an environment failure, not evidence of
an adapter decoding failure.

## Runnable baseline tests without this repository

Each skill includes `assets/tests` and `assets/fixture_server.py`. These are real
Python files using installed public APIs, synthetic packets, temporary files,
and loopback servers. They do not require private GitHub examples or a checkout.

```sh
<consumer-python> -m pytest /path/to/installed/skill/assets/tests -q
```

The packet tests cover exact IDs, binary routing, simulations, and HTTP/feed state.
The local transport test covers unrestricted and restricted discovery. The Polars
tests show `levels_lazy(book.orders.bids).collect()`, exact unsigned schemas, book
priority, filtering/projection/limits, and empty-side output. For grouped binary
replay, `adapter.run(concurrent=False)` returns native books usable with the same
`lobo.levels.levels_lazy` API. Packet/live adapters expose public `levels()` rows;
check their documented units rather than silently treating them as display prices.

Copy the fixture server into a scratch consumer directory before running it. It
serves AAA, BBB, CCC with known bid/ask atoms 59/60 at price precision 2 and records
subscriptions to `subscriptions.jsonl` in that directory. The test skill bundles
`scripts/browser_smoke.py`; any other capable browser tool can exercise the same
fixture. Use a fresh directory per run when checking subscription counts.

The reference tests create a minimal temporary `web_root` for API tests. Those
checks do not validate the packaged terminal. The fixture server leaves `web_root`
unset so browser tests use the actual wheel's terminal. Baseline tests are examples
to adapt: also construct and test the user's own protocol and discovery builder.
