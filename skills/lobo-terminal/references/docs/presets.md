# Preset views

Run `lobo presets --json` for the installed binary's authoritative defaults.
Use `lobo --preset NAME` to open a view, then pass normal flags to customize it.
An explicit command wins over the preset's view. Explicit flags and environment
values win over its defaults, including flags after a subcommand. Presets do not
write configuration files or launch background processes.

| Name | View | Default |
| --- | --- | --- |
| dashboard | dashboard | Offline demo, all charts |
| simulator | simulate | Nonmutating buy market preview, quantity 100 |
| tick-bars | candles | 100 execution messages per candle |
| time-bars | candles | Five-second aligned intervals |
| volume-bars | candles | 5,000 displayed quantity units |
| notional-bars | candles | 100,000 quote-currency units |
| bitfinex | book | Live BTC/USD depth, 100-tick aggregation |
| kraken | candles | Live BTC/USD, five-second candles |
| fifo | orders | Offline L3 queue inspection |
| flow | flow | Net visible changes over 120 seconds |

All except bitfinex and kraken default to the offline demo. A replay file selects
ITCH automatically. Choose a suitable threshold for the instrument: for example,
`lobo --preset volume-bars --source kraken --bar-size 1` for one BTC per bar.
Volume bars split executions at exact quantity boundaries. Tick bars count
execution messages; notional bars include the crossing execution. Time bars use
source timestamps and omit empty intervals.

```sh
lobo --preset simulator --order-kind limit --price 99.98 --quantity 200
lobo --preset tick-bars --bar-size 250
lobo --preset tick-bars --source kraken --symbol ETH/USD
lobo --preset bitfinex --symbol tETHUSD
lobo --preset time-bars --file session.gz --symbol AAPL --start-at 09:30:00
lobo book --preset tick-bars --bar-size 50
lobo --preset fifo --snapshot-json --renderer cpu --capture-seconds 0.1
```

For one feed shared by many panes, run `lobo session --socket /tmp/lobo.sock
--preset tick-bars --bar-size 250`. Pane startup data arguments do not reconfigure
an attached session. Change shared aggregation via `lobo ctl --attach
/tmp/lobo.sock --execute 'aggregation ticks 100'`.
