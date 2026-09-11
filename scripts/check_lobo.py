#!/usr/bin/env python3
"""Check the public Rust API and feature isolation from a downstream project."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parents[1]
CRATE = ROOT / "rust/crates/lobo"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()
    extra = ["--offline"] if args.offline else []
    env = dict(os.environ)
    env.setdefault("CARGO_TARGET_DIR", str(ROOT / "target"))
    manifest = tomllib.loads((CRATE / "Cargo.toml").read_text())
    features = manifest["features"]
    package = manifest["package"]["name"]

    def cargo(*command, capture=False):
        return subprocess.run(
            ["cargo", *command, *extra], cwd=ROOT, env=env, check=True,
            text=True, stdout=subprocess.PIPE if capture else None,
        ).stdout

    cargo("test", "-p", package, "--no-default-features")
    cargo("test", "-p", package)
    cargo("test", "-p", "lobo_replay_tests", "--all-features")
    cargo("check", "-p", "lobo_replay_tests", "--benches", "--all-features")
    cargo("run", "-p", package, "--example", "order_book", "--no-default-features")
    cargo("run", "-p", package, "--example", "replay", "--no-default-features", "--features", "replay")

    def enabled(names):
        result = set(names)
        pending = list(names)
        while pending:
            for child in features[pending.pop()]:
                if child in features and child not in result:
                    result.add(child)
                    pending.append(child)
        return result

    # Each configuration is a real consumer with only one direct dependency.
    # This prevents workspace/Python feature unification from masking failures.
    configurations = [("minimal", []), ("default", ["default"]), ("full", ["full"])]
    configurations += [(name, [name]) for name in features if name not in {"default", "full"}]
    optional_modules = {"adapters", "batchers", "context", "replay", "server"}
    forbidden = {"lobo_py", "lobo_wasm", "lobo_replay_tests", "pyo3", "pyo3-ffi", "pyo3-polars"}
    minimal_forbidden = optional_modules | {
        "lobo_adapters", "lobo_batchers", "lobo_context", "lobo_replay", "lobo_server",
        "polars", "wgpu", "reqwest", "rayon", "cranelift-jit",
    }

    with tempfile.TemporaryDirectory(prefix="lobo-consumer-") as directory:
        consumer = Path(directory)
        (consumer / "src").mkdir()
        for label, selected in configurations:
            print(f"Checking isolated consumer: {label}", flush=True)
            # Retain the workspace's pinned Git revisions and versions, while
            # letting Cargo resolve features solely for this consumer.
            shutil.copyfile(ROOT / "Cargo.lock", consumer / "Cargo.lock")
            dependency = (
                f'package = {json.dumps(package)}, path = {json.dumps(str(CRATE))}'
            )
            if label != "default":
                dependency += f', default-features = false, features = {json.dumps(selected)}'
            (consumer / "Cargo.toml").write_text(
                '[package]\nname = "lobo-consumer-check"\nversion = "0.0.0"\nedition = "2024"\n'
                '[workspace]\n[dependencies]\nlobo = { ' + dependency + ' }\n'
            )
            active = enabled(selected)
            source = "pub use lobo::{books, events, models, primitives, storage, prelude};\n"
            source += "pub fn book() -> lobo::OrderBook { lobo::OrderBook::new() }\n"
            for module in sorted(active & optional_modules):
                source += f"pub use lobo::{module};\n"
            for adapter in sorted(active & {"itch", "kraken", "bitfinex"}):
                source += f"pub use lobo::adapters::{adapter};\n"
            if "json" in active:
                source += "pub use lobo::replay::custom::definition;\n"
            if "native" in active:
                source += "pub use lobo::replay::custom::runtime;\n"
            if "arrow" in active:
                source += "pub use lobo::batchers::arrow;\n"
            (consumer / "src/lib.rs").write_text(source)
            selection = ("--manifest-path", str(consumer / "Cargo.toml"))
            tree = cargo("tree", *selection, "--edges", "normal,build", "--prefix", "none", "--format", "{p}", capture=True)
            dependencies = {line.split()[0] for line in tree.splitlines() if line.strip()}
            unexpected = dependencies & forbidden
            if label == "minimal":
                unexpected |= dependencies & minimal_forbidden
            if unexpected:
                raise SystemExit(f"{label} pulled in unexpected dependencies: {sorted(unexpected)}")
            cargo("check", *selection)
        cargo("doc", "-p", package, "--all-features", "--no-deps")
    print(f"Public API, examples, docs, and {len(configurations)} isolated feature configurations passed.")


if __name__ == "__main__":
    main()
