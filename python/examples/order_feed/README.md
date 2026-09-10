# Order feed custom adapter

`adapter.py` connects to the WebSocket feed of an existing order server. To create a standalone book and send orders to it, use the [Order API example](../order_api/README.md).

The feed contains three message types:

| Type | Contents | Mapping |
| --- | --- | --- |
| `directory` | Book symbols, precision and policies | Register available books |
| `snapshot` | Book metadata, resting orders and sequence number | Restore orders and reset the sequence |
| `update` | Book symbol, sequence number and order command | Apply the command in sequence |

`protocol()` declares those mappings with `Directory`, `Register`, `Book`, `OrderCommand` and `CheckSequence`. `build_adapter(endpoint, symbol="BOOK")` combines the declaration with a WebSocket source in live L3 mode.

The [order-API notebook](../../../notebooks/custom_adapter_order_api.ipynb) demonstrates order commands and snapshot restoration. The parity tests use the declaration in this file, exported by [export_definitions.py](../export_definitions.py).
