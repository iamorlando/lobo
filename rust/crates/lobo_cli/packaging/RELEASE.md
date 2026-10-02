Native terminal applications: candles, book, flow, simulate, orders and dashboard.

Use `lobo session --socket /tmp/lobo.sock` once, then attach each pane with
`--attach /tmp/lobo.sock`. The bundled `lobo-tmux` launcher starts four panes
sharing one native feed, replay clock and simulation.

These archives include the executable, built-in themes, native feed adapters and shell completions. No Node, Python, Rust, wasm-pack or separate server is needed to run them.

Publish this draft, then upload the native archives **and all four `.bottle.tar.gz` files**, then put the generated lobo.rb in Formula/lobo.rb in the iamorlando/homebrew-lobo tap (or your own tap). Users can then run brew install iamorlando/lobo/lobo, or brew tap iamorlando/lobo followed by brew install lobo. A bare brew install lobo on an untapped machine requires a Homebrew core formula.

The formula uses relocatable Homebrew bottles to avoid source-build tool checks.
Current Homebrew may request `brew trust --formula iamorlando/lobo/lobo` for a
third-party formula. Bare untapped installation still requires Homebrew core.
