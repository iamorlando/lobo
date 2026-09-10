# LOBO

[![GitHub](https://img.shields.io/badge/GitHub-source-181717?style=flat-square&logo=github)](https://github.com/iamorlando/loblib)
[![Python 3.11–3.14](https://img.shields.io/badge/Python-3.11%E2%80%933.14-3776AB?style=flat-square&logo=python&logoColor=white)](docs/packaging.md)
[![Rust](https://img.shields.io/badge/Rust-2024-DEA584?style=flat-square&logo=rust&logoColor=white)](rust/crates/lobo/Cargo.toml)
[![License: MIT](https://img.shields.io/badge/License-MIT-A3E635?style=flat-square)](LICENSE-MIT.md)


```py
pip install lobopy
```

- [LOBO](#lobo)
  - [Get started](#get-started)
  - [Build your own adapters](#build-your-own-adapters)
  - [Use LOBO in Rust](#use-lobo-in-rust)
  - [Further reading](#further-reading)



LOBO is a high-performance order book library built for fast replay. Written in Rust,
it exposes Python APIs to create and serve books through a [REST order API](rust/crates/lobo_server/README.md#order-api)
and an interactive [WebGPU terminal](web/README.md).

Define your own market data adapters in Python; LOBO JIT-compiles their declarations
into native code. Python-defined adapters are [benchmarked against their native counterparts](rust/crates/lobo_replay/benches/custom_adapters.rs).

[![LOBO demo: AAPL liquidity heatmap, live depth, and replay controls](docs/assets/lobo-demo.png)](https://lobo-demo.vercel.app)

_Explore the [live demo](https://lobo-demo.vercel.app) — click the screenshot to open it._

## Get started

For Python, [build a wheel](docs/packaging.md#build-locally), then install it from the repository root:

```sh
pip install dist/lobo-*.whl
```

To create and serve your first book, follow the [server guide](rust/crates/lobo_server/README.md)
or try the [standalone order API example](python/examples/order_api/README.md).

## Build your own adapters

Browse the [Python example adapters](python/examples/README.md) and [adapter reference](rust/crates/lobo_replay/src/custom/README.md),
or install the [agent skill](agents/README.md) to have an agent build one for you:

```sh
npx skills add iamorlando/loblib --skill lobo-adapter --copy
```

The examples cover Nasdaq ITCH, Kraken, Bitfinex, Polymarket, and LOBO's order feed.

## Use LOBO in Rust

Add LOBO to your Rust project directly from this repository:

```sh
cargo add lobo --git https://github.com/iamorlando/loblib.git
```
Then you can try prompt like this:
```text
uild me the cme adapter for lobo, using the lobo adapter skill. then use a sample file
    https://cmegroupclientsite.atlassian.net/wiki/spaces/EPICSANDBOX/pages/457223111/MBO+FIX#MBOFIX-SampleFiles to show one book in the
  app
  ```
## Further reading

| Guide                                                  | What you'll find                                                     |
| ------------------------------------------------------ | -------------------------------------------------------------------- |
| [Server & REST API](rust/crates/lobo_server/README.md) | Host books, submit orders, and stream updates.                       |
| [Order book viewer](web/README.md)                     | Run the terminal locally; explore depth, OHLC bars, and simulations. |
| [Python notebooks](python/notebooks/README.md)         | Replay, custom adapters, and data exports.                           |
| [Build & packaging](docs/packaging.md)                 | Build and test Python wheels, including the bundled terminal.        |
