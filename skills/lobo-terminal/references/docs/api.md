# APIs and automation

`lobo completions api` exposes every CLI command/option, interactive command,
preset, documentation topic and the API catalog in versioned JSON.
`lobo api schema --json` returns schemas generated from the native CLI snapshot,
shared-session request/reply types, and the library's HTTP order/feed types.
`lobo api schema --output PATH` saves the same bundle. No server is required.

## Capture and shared session

`lobo --snapshot-json --renderer cpu --capture-seconds 0.1` produces one Snapshot
JSON document. The same flags with `--attach SOCKET` capture a shared session.
Use `session_sequence` and `session_revision` to identify shared state. Check
`warming`, `feed_error`, `complete`, quotes, and simulation fields as appropriate.
Capture duration does not guarantee a live upstream is ready.

Start a server with `lobo session --socket SOCKET`; attach panes using `--attach`.
`lobo ctl --attach SOCKET --execute 'pause'` waits for an acknowledgement and
prints its text. A nonzero exit indicates an error. `stop` terminates that session.
The interactive command catalog describes commands and scopes. HTTP writes are
queued off-thread; an acknowledgement that a command was queued does not prove
it succeeded. Observe the session response/message or verify resulting state.

Direct Unix socket clients use protocol 2: four-byte big-endian payload length,
then UTF-8 JSON, maximum 64 MiB. The server streams externally tagged `Frame`
replies and `Ack` replies. A request has an `id` and an `action`, such as
`{"id":1,"action":{"Command":"pause"}}`. Correlate acks by ID. The schema
bundle describes exact fields and tags. Depth frames may be deltas: replace on
`reset_depth`, otherwise append and bound history using `config.capacity`.
The published snapshot excludes depth history, which is carried separately.
Session sockets have mode 0600. Prefer the installed CLI client for protocol
handling, reconnect errors, sequencing, and framed I/O.

`Submit` accepts a native typed command plus the observed symbol and revision.
The server rejects stale context. Native commands target only the demo or the
explicitly configured `--order-endpoint`. Never retry a write automatically when
its result is uncertain.

## Hosted HTTP and WebSocket APIs

Read `GET /api/server-context` first for books, adapters, capabilities and their
advertised endpoint URLs. Use the URLs returned by that server. Read
`GET /api/schema` for its version's command, adapter command/subscription,
response, feed and order schemas. The completions catalog's `apis.endpoints`
lists all native server routes plus the standalone web session-list route.

The native library schemas are bundled for offline reference. Runtime-only
adapter envelopes and playback commands are documented in `lobo docs server`
and `lobo docs hosted-adapters`, and must be checked against the target server.
API fields use integer price/quantity atoms; terminal arguments and commands use
displayed units and perform precision validation. L2 adapters cannot fabricate
L3 FIFO or isolated limit simulations. The server's policies determine which
writes are accepted; opening a view never places an actual hosted order.

For Python, adapter configuration, custom feeds, Rust library APIs, web controls,
and examples, use `lobo docs --json` to find the complete bundled guides, or the
installed skill's references/index.md. This is a documentation/discovery surface;
it does not expose a new HTTP service or model-generation endpoint.
