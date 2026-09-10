# Choose tests that match the feed

| Area | Observable assertions |
| --- | --- |
| Discovery | Pagination, requested ranking/limit, disabled/empty markets, unique labels, exact IDs, errors on failed/malformed responses. |
| Subscriptions | Unvisited symbols are listed; only the initial selection is requested; selecting a second obtains its snapshot; explicit scope subscribes its members and restricts choices. |
| Snapshots | Actual array/object variants; a repeated snapshot removes stale levels/orders. |
| L2 | Absolute totals, zero deletion, decimal atoms, both sides, instrument isolation. |
| L3 | ID identity, updates/deletes, FIFO, replenishment/hidden quantities where supported. |
| Trades/outputs | Maker/taker convention, no double subtraction, bars/OHLC/volume, Polars schemas and values through public outputs. |
| Transport | Real local fixture receives expected payloads; acknowledgment, heartbeat, reconnect, rejection, sequences/checksums as applicable. |
| Lifecycle | Cleanup, finite completion, bounded timeouts, worker errors reaching the test. |
| Hosted UI | Installed static app, actual WebGPU availability, autocomplete, quotes, chart, small scope, and real selection requests; record page/console errors. |

Use `Source.packets` for decoding/state assertions and localhost fixtures for transport. Synthetic IDs belong in local fixtures; runnable live examples should use discovery or real user configuration.

Prefer deadlines/events over long sleeps. Never call an unbounded live `wait()` in tests. Close adapter, server, browser, and fixture threads on both success and failure.

Start from the bundled `assets/tests/test_custom_adapter.py`, `test_hosted_discovery.py`, and `test_levels.py`, then construct the user's adapter in the tests. The baseline uses finite packets, temporary files, and localhost transport with no library checkout. Use `assets/fixture_server.py` with the browser runner to verify the wheel's actual terminal.

When bundled or optional remote examples are ahead of the wheel, test the installed public API and record the mismatch. GitHub authentication is unnecessary for bundled references. Missing runtime dependencies or WebGPU support are reported gaps, not passing checks.
