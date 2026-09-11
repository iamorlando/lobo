# Rust packaging and releases

The library is named `lobo`, and its crates.io package is `lobo-rs`. Consumers
can keep the dependency key and Rust imports named `lobo`:

```toml
[dependencies]
lobo = { package = "lobo-rs", version = "0.1" }
```

`lobo_py`, `lobo_wasm`, and `lobo_replay_tests` have `publish = false`. They are repository build
targets for Python, the web terminal, and cross-crate comparison tests; they
are not dependencies of `lobo-rs`.
Even `lobo-rs`'s `full` feature does not enable PyO3 or Python bindings.

## Check a release

Use the Rust release workflow's toolchain (currently Rust/Cargo 1.97.0).
Native workspace publishing requires Cargo 1.90 or later; no release plugin
or per-crate publishing loop is necessary.
Cargo 1.96.1 has a workspace publishing bug that can report a false deadlock
while uploaded dependencies are still awaiting registry confirmation. The
[upstream fix](https://github.com/rust-lang/cargo/pull/17071) shipped in 1.97.

```sh
make check-lobo
make publish-rust-check
```

`check-lobo` tests the minimal and full public APIs, runs the standalone
examples and cross-crate replay comparisons, builds every selectable feature in an isolated downstream project,
checks that Python bindings stay out of the build graph, checks the minimal
dependency graph, and builds documentation. It uses Python 3.11+ as repository
test tooling; the library itself does not require Python.

`publish-rust-check` runs:

```sh
cargo publish --workspace --exclude lobo_py --exclude lobo_wasm --exclude lobo_replay_tests --dry-run
```

Cargo packages the selected workspace members, substitutes registry versions
for workspace path/Git dependencies, and verifies that the packaged code
builds. The dry run does not upload anything. During development, an explicit
`--allow-dirty` can be added to that Cargo command to verify uncommitted changes.
The Make targets intentionally require a clean working tree for a release.

The Polars Git tag in the workspace is coupled to the Python Polars version.
The Git tag provides Polars 0.54.4 and a PyO3-0.29-compatible binding still
versioned 0.27. On crates.io, the compatible pair is Polars 0.55 and
pyo3-polars 0.28. The bounded version ranges support both pairs, keeping the
Python extension on its existing Git tag. The package dry run checks the
registry build separately from the local Git-source build.

To inspect package contents:

```sh
cargo package --workspace --exclude lobo_py --exclude lobo_wasm --exclude lobo_replay_tests --list
```

Cross-crate adapter comparisons live in the non-published `lobo_replay_tests`
crate so `lobo_replay` does not depend back on `lobo_adapters` during release.
Run them directly with `cargo test -p lobo_replay_tests --all-features`.

## Publish through GitHub Actions

The **Rust library** workflow checks pull requests and pushes to `master` and
`main`. Pushing `rust-vX.Y.Z` or `vX.Y.Z` runs the same checks and then publishes
the workspace to crates.io. The tag must match `[workspace.package].version`,
every publishable crate's version, and the workspace's local dependency versions.

The publish job uses the GitHub environment **`cargo`**, with the environment
secret **`CARGO_REGISTRY_TOKEN`** exposed only to the publishing step. The token
needs publishing access to `lobo-rs` and its supporting crates. CI does not run
`cargo login` or store the token in a credential file.

After committing and pushing the release version and workflow, publish the
first Rust release with:

```sh
git tag rust-v0.1.0
git push origin rust-v0.1.0
```

Use `rust-v` tags for independent Rust releases. A `v` tag also triggers the
existing Python release workflow, whose version check uses `pyproject.toml`.
For later Rust releases, update the workspace and local dependency versions
together, update `Cargo.lock`, commit the changes, and push the matching new tag.
The workflow publishes new versions; it does not choose version numbers or
modify your source automatically.

To retry a release, rerun its failed workflow jobs. The publishing helper
queries crates.io and excludes versions that were already uploaded, so a
partial workspace release can resume. A fully published version is a successful
no-op. Registry errors stop the job instead of being treated as missing packages.
Only one publish job runs at a time, and an active publish is not canceled by
a newer release.

Manual workflow runs only check by default. To publish an existing release tag
after rerunning its checks:

```sh
gh workflow run rust-package.yml --ref master -f release_tag=rust-v0.1.0 -f publish=true
```

This uses the workflow from `master` while checking and publishing the source
at `rust-v0.1.0`. Commit and push any workflow fix to `master` first. GitHub's
**Re-run jobs** uses the original workflow commit, so it will not pick up a
toolchain fix added after the release was tagged. The tag and package versions
do not need to change to resume a partial release.

The publish job checks out the exact commit that passed the check job. Ordinary
branch runs cannot publish; manual publication requires an existing release tag
and `publish=true`. Publication starts only after all checks pass.
`lobo_py`, `lobo_wasm`, and `lobo_replay_tests` are excluded automatically by
their `publish = false` metadata.

The release helper can also validate the version locally, without publishing
or accessing the registry:

```sh
python3 scripts/publish_rust.py --tag rust-v0.1.0 --check
```

## Publish the workspace locally

After the checks pass, authenticate to crates.io with `cargo login` and run:

```sh
make publish-rust
```

This runs `cargo publish --workspace --exclude lobo_py --exclude lobo_wasm --exclude lobo_replay_tests`.
Cargo determines dependency order and waits for dependencies to become
available in the registry before publishing dependents. The supporting crates
still exist as separate registry packages; users only need to add `lobo-rs`.
The existing workspace structure cannot be published as a single registry
package without restructuring the implementation crates into modules.

Publishing a workspace is not atomic: some crates may publish before another
fails. Inspect the result before retrying, and exclude already published
versions with additional `--exclude` arguments if needed.

For a coordinated release, update `[workspace.package].version` and the local
crate version requirements in `[workspace.dependencies]` in the root
`Cargo.toml`, update `Cargo.lock`, and rerun the checks. An existing version
cannot be overwritten. Python package versions are managed separately.

See Cargo's official [workspace publishing announcement](https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/),
[publish command](https://doc.rust-lang.org/cargo/commands/cargo-publish.html), and
[dependency source rules](https://doc.rust-lang.org/cargo/reference/specifying-dependencies.html#multiple-locations).
