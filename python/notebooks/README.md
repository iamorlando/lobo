# Adapter notebooks

Use the Python environment where `lobo` is installed. The local ITCH examples use `data/NASDAQ/01302020.NASDAQ_ITCH50`.

| Notebook | Contents |
| --- | --- |
| [Connect sources](adapter_connections.ipynb) | Construct adapters for local ITCH, an online Nasdaq gzip session, and Bitfinex |
| [Build ITCH](custom_adapter_itch.ipynb) | Binary layouts, raw-record execution, FIFO quantities, and file replay |
| [Build Kraken](custom_adapter_kraken.ipynb) | Recorded snapshots/updates with checksum results, then the live L2 feed |
| [Build Bitfinex](custom_adapter_bitfinex.ipynb) | A 294-frame recording, FIFO and checksum results, then the live L3 feed |
| [Order commands and snapshots](custom_adapter_order_api.ipynb) | A Python-defined recording envelope, typed commands, snapshot restoration, and iceberg FIFO |

The build notebooks contain the complete Python protocol definitions. `CustomAdapter` compiles the declaration during construction; the recorded examples show construction separately from source startup/completion. They print resulting levels, queues or executions and retain executed outputs. These short demonstrations are not throughput benchmarks. Saved performance results and their limitations link to the [validation report](../python/examples/VALIDATION.md).

The recorded sections work offline. The ITCH source section needs the local data file; live and HTTP sections need internet access. Each notebook also creates a `CustomAdapter`, passes it to `server_context(adapters=[adapter])`, and displays a link to the terminal. Run cells individually to explore, then run cleanup. Run All reaches cleanup and closes the server; saved links refer to servers from the execution that produced those outputs.

JSON still builds a parsed value tree before the compiled field projections. The notebooks do not claim direct single-pass JSON decoding or streaming performance parity.

The reusable versions of these definitions, plus the order-API feed definition, are in [`python/examples`](../python/examples/README.md).
