#!/usr/bin/env bash
set -euo pipefail

# Vercel's bundled Rust lacks the WASM target. Keep a complete Rustup toolchain
# and Cargo build cache in Next.js's persistent build cache.
if [[ "${VERCEL:-}" == "1" && "$(uname -s)" == "Linux" ]]; then
  export CARGO_HOME="$PWD/.next/cache/cargo"
  export RUSTUP_HOME="$PWD/.next/cache/rustup"
  export CARGO_TARGET_DIR="$PWD/.next/cache/rust-target"
  export PATH="$CARGO_HOME/bin:$PATH"
  export RUSTUP_TOOLCHAIN=1.96.1
  if [[ ! -x "$CARGO_HOME/bin/rustup" ]]; then
    curl --proto '=https' --tlsv1.2 --fail --silent --show-error https://sh.rustup.rs |
      sh -s -- -y --profile minimal --default-toolchain none --no-modify-path
  fi
  rustup toolchain install "$RUSTUP_TOOLCHAIN" --profile minimal --target wasm32-unknown-unknown
fi

npm run build
