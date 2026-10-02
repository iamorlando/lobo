# LOBO for Rust

LOBO provides configurable limit order books, market-data replay, built-in
adapters, event aggregation, and a native REST order API. The `lobo-rs` package
exposes the `lobo` library. It is the public entry point; its modules re-export
the implementation crates.


```sh
cargo add lobo-rs --rename lobo
```

```toml
[dependencies]
lobo = { package = "lobo-rs", version = "0.1" }
```


The default `full` feature enables all native Rust functionality: replay,
ITCH/Kraken/Bitfinex adapters, native transports, JSON definitions, JIT,
concurrency, Polars, the server, Arrow/Feather, and GPU outputs. It does not
enable Python bindings or depend on `lobo_py` or `lobo_wasm`.

For just the core order-book library, disable default features:

```toml
[dependencies]
lobo = { package = "lobo-rs", version = "0.1", default-features = false }
```

To add only selected capabilities:

```toml
[dependencies]
lobo = { package = "lobo-rs", version = "0.1", default-features = false, features = ["kraken", "native"] }
```

Cargo features are additive. Every dependency on `lobo` in your application
must disable defaults if you want to avoid the full build. Enabling `full`
explicitly is equivalent to the default configuration. The full configuration
targets native platforms; for WebAssembly, start without default features and
select the capabilities supported by your target.

## Create a book and submit orders

```rust
use lobo::prelude::*;

let mut book: OrderBook = OrderBook::new();
let trader = Uuid::new_v4();

// Prices and quantities are integer units chosen by your application.
// For prices in cents, 10_000 represents $100.00.
let added = book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(
    Command::Add {
        order: LimitOrder::new(Some(10_000_u64), 25, trader, Side::Sell),
    },
);
assert!(added.is_ok());
assert_eq!(book.order_storage.asks.visible_quantity, 25);

let filled = book.submit::<MarketOrderData, LimitOrderData, MutatingFills>(
    Command::Fill {
        order: MarketOrder::new(10, Uuid::new_v4(), Side::Buy),
        execution: MutatingFills,
        reports: Reports::default(),
    },
);
assert!(filled.is_ok());
assert_eq!(book.order_storage.asks.visible_quantity, 15);
```

`OrderBook` uses 64-bit prices, FIFO queues, B-tree price sorting, and user and
hidden-quantity tracking. `ReplayBook` uses intrusive FIFO levels and sorted
price vectors, and skips user indexing and hidden-quantity tracking for visible
order replay. Both accept a different price type, such as `OrderBook<Price128>`.
For full policy control, use `books::price_time_priority::Book` and the types in
`storage`. Publishers can be attached with `with_publisher`.

Runnable examples, requiring no downloaded data or services:

```sh
cargo run -p lobo-rs --example order_book --no-default-features
cargo run -p lobo-rs --example replay --no-default-features --features replay
```

The replay example applies decoded add, cancel, and remove events through
`lobo::replay::custom::AdaptForReplay`.

## Modules and features

`books`, `models`, `primitives`, `storage`, `events`, and `prelude` are always
available. Optional features automatically enable the components they need.

| Feature | Adds |
| --- | --- |
| `full` (default) | All the native Rust capabilities below |
| `batchers` | `lobo::batchers`: event aggregation and metrics |
| `context` | `lobo::context`: feed contexts and sinks; includes batchers |
| `replay` | `lobo::replay`: typed replay, custom protocols, simulation; includes context |
| `adapters` | `lobo::adapters`: adapter interfaces; includes replay |
| `itch`, `kraken`, `bitfinex` | Individual built-in adapters |
| `native` | File, HTTP, and WebSocket transports for replay |
| `order-api` | Typed order API messages |
| `json` | JSON adapter definitions; includes the order API |
| `jit` | Native JIT compilation for JSON definitions |
| `concurrent` | Parallel replay with Rayon |
| `polars` | DataFrame outputs; extends replay/adapters when they are enabled |
| `server` | `lobo::server`: REST API and terminal hosting; includes native replay and JIT |
| `arrow` | Arrow event batches |
| `feather` | Feather/Arrow IPC output |
| `gpu` | GPU batch destinations using wgpu |

For example, `polars` by itself does not pull in adapters, native transports,
the server, or GPU support. Core mode still includes dependencies used directly
by the order-book implementation, including Tokio's synchronization types.

The server hosts terminal assets supplied by the caller. Those assets are
built from the repository's `web/` project and are not bundled in this crate.
See the [server guide](https://github.com/iamorlando/lobo/tree/main/rust/crates/lobo_server).
Python users can use the separate `pylobo` package or build it from the repository.

## Development and releases

See the [Rust release guide](https://github.com/iamorlando/lobo/blob/main/docs/rust-packaging.md)
for checks and publishing the workspace in one command.

Licensed under MIT.
