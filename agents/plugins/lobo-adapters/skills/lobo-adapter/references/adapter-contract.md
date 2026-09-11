> Bundled from `agents/references/adapter-contract.md` in https://github.com/iamorlando/lobo.
> This content is included in the installed skill; no checkout or download is needed.
> Check the installed wheel's public `help()` for version-specific signatures.

# Decisions that affect adapter correctness

- **Directory versus books:** `Instrument` registration describes symbols and precision without subscribing or allocating all books. Discovery feeds ticker/scope choices. Initial selection opens one book. Explicit scope members stay subscribed; visited live books may remain subscribed.
- **Discovery:** prefer `Bootstrap`/`Register` or WebSocket directory messages when suitable. Python HTTP discovery is appropriate for pagination, ranking, or transformation. Derive token/channel mappings rather than requiring copied IDs.
- **Symbols:** check installed `Instrument` rules. Current symbols are normalized ASCII labels of at most 64 bytes. Preserve unique identity when truncating questions; include market ID/outcome suffixes, keep opaque wire IDs separate, and retain each outcome's book.
- **L2:** snapshots replace levels; absolute updates replace totals; zero removes a level. A reported trade may not also subtract liquidity. Model that explicitly.
- **L3:** preserve order IDs, side, price, quantities, and FIFO. Follow the feed's update, removal, and replenishment semantics.
- **Numbers:** convert decimals to integer atoms exactly. Declare price/quantity precision and timestamp units. Avoid floating-point round trips for IDs/prices. Apply sequences and checksums at the correct connection/instrument scope.
- **Transport:** implement acknowledgment, channel mapping, heartbeat, sharding, and reconnect/snapshot rules using the public protocol/transport APIs. Declare mode, book level, timezone, and trade support accurately.
- **Errors:** propagate rejected subscriptions and inconsistent snapshots. Warming/disconnected books are not current quotes. Report unexpressible standard features as public-API gaps.
- **Outputs:** use installed public `levels`, `stats`, `drain_bars`, `to_lazy_frame`, and supported sink APIs. Distinguish atom units from display units.

Read the bundled `references/custom-adapters.md` for API explanations and `references/examples/` for complete Python declarations. Verify version-specific signatures with the installed package's public `help()`; GitHub refreshes are optional.
