# Replay and adapter comparisons

This repository-only crate holds tests and benchmarks that depend on both
`lobo_replay` and `lobo_adapters`. Keeping them here prevents a circular
dependency during the first workspace publication. It has `publish = false`.

The tests reuse fixtures in the sibling crates; they run from a repository checkout.

```sh
cargo test -p lobo_replay_tests --all-features
cargo bench -p lobo_replay_tests --bench custom_adapters
```

The full NASDAQ file comparison remains ignored unless explicitly requested.
