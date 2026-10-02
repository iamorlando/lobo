# Interactive and session commands

Run `lobo completions api` for every CLI command and flag derived from the parser.
Press `:` in a view for the commands below. Shared controls can also be sent with
`lobo ctl --attach SOCKET --execute COMMAND`. Pane commands affect the pane in
which they are entered; sending them through ctl targets the headless owner and
does not change attached pane presentation. All input amounts use displayed units.

| Syntax | Aliases | Scope | Effect |
| --- | --- | --- | --- |
| `symbol SYMBOL` |  | shared | Select the active instrument |
| `scope selected\|all\|top-tech\|sp500\|SYMBOL,SYMBOL` |  | shared | Rebuild the source with a book scope |
| `source auto\|demo\|itch\|nasdaq\|kraken\|bitfinex\|server [SYMBOL]` |  | shared | Switch source and clear file/URL |
| `file PATH` |  | shared | Rebuild from a local ITCH file; path may contain spaces |
| `url URL` |  | shared | Rebuild with a replay/feed URL; select source first |
| `start HH:MM:SS` |  | shared | Reconstruct at exchange clock time |
| `restart` |  | shared | Restart/reconnect the selected source |
| `speed MULTIPLIER` |  | shared | Set replay speed, positive and at most 1000 |
| `pause` |  | shared | Pause replay |
| `play` |  | shared | Resume replay |
| `theme NAME` |  | pane | Choose a palette; full name may contain spaces |
| `theme-file PATH` |  | pane | Load a local palette |
| `aggregation volume\|time\|ticks\|notional SIZE` | bars | shared | Set bar kind and positive integer size |
| `history SECONDS` |  | shared | Set positive history duration |
| `range PRICE_SPAN` |  | pane | Set positive visible price span |
| `sim buy\|sell market QUANTITY \| sim buy\|sell limit QUANTITY PRICE` | simulate | shared | Nonmutating market preview or isolated L3 timeline |
| `main` |  | shared | Return to the main timeline |
| `queue buy\|sell PRICE` |  | shared | Inspect an L3 FIFO level |
| `add buy\|sell PRICE QUANTITY` |  | shared | Add a resting limit order |
| `limit buy\|sell PRICE QUANTITY` |  | shared | Match a limit order and rest its remainder |
| `market buy\|sell QUANTITY` |  | shared | Match a market order |
| `cancel UUID_OR_ROW QUANTITY` |  | shared | Reduce a resting order |
| `execute UUID_OR_ROW QUANTITY` |  | shared | Execute against a resting order |
| `modify UUID_OR_ROW QUANTITY [PRICE]` |  | shared | Modify an order |
| `remove UUID_OR_ROW` |  | shared | Remove an order |
| `help` |  | pane | Show the interactive help panel |
| `stop` |  | session-only | Stop the session; ctl only |

Order writes act only on the local demo or explicit hosted order endpoint.
Numeric order rows are one-based and resolved to UUIDs from the current FIFO.
Use UUIDs from a fresh snapshot in agent automation. A queued hosted write needs
its eventual response checked; do not automatically retry uncertain writes.

Keys: space pause/play, r restart, s simulate, m main timeline, arrows or scroll
pan, +/− zoom, Home recenter, t cycle theme, [/] change ticker, 1–6 choose views,
? help, Esc close help, q quit. Quitting a pane does not stop a shared feed.
