# Building and testing the Python distribution

The release artifacts are a `pylobo` source distribution (`.tar.gz`) and native
CPython stable-ABI wheels (`.whl`). A `cp311-abi3` wheel supports compatible
CPython versions from 3.11 onward on its target OS and architecture; it is not
tied to the Python minor version that built it. The distribution is installed
with `pip install pylobo`; the library and CLI are named `lobo`. A package-named
entry point also lets `uvx pylobo demo` launch the same CLI.
This workflow builds locally and in GitHub Actions, and publishes version-tag
releases to PyPI after all platform builds and tests pass.

## What is included

- The PyO3 extension with all shipped adapter bindings (ITCH, Kraken, Bitfinex),
  custom protocol/JIT support, the native order API/server, parallel replay, and
  native Polars conversions.
- The Python public modules, generated type stubs, and `py.typed` marker.
- The production static Next.js terminal, local fonts, and WebAssembly module
  under `lobo/_web`. The local host serves these files without Node.js at runtime.
- `lobo serve`, `lobo web --server URL`, `lobo demo`, and `python -m lobo`
  entry points.

## Run the demo locally

```sh
uvx pylobo demo
```

This runs the existing web demo on localhost and opens it in your default
browser. It serves the bundled app with its normal Nasdaq streaming, live
exchange feeds, file picker, book scope, playback, and chart controls. The
app keeps its existing defaults, including AAPL as the initial Nasdaq scope.

The launcher chooses a free local port. Use `--port 8000` to choose one or
`--no-open` to print the URL without opening a browser. Ctrl-C stops the local
host. Set `LOBO_ITCH_PATH` to supply the app's repository-file source; browser
file selection also works as usual. Streaming requires internet access, and
rendering requires WebGPU. No Node.js server or separate demo UI is needed.

In a development checkout, run `poetry run lobo demo`.
To test an unreleased wheel with uv:

```sh
uvx --from /path/to/pylobo-VERSION-cp311-abi3-PLATFORM.whl pylobo demo
```

The short `uvx pylobo demo` command uses the published PyPI release; changes
in a checkout become available there after the next Python package release.

## Runtime requirements

Runtime dependencies are Polars 1.43.2 and Windows timezone data. Keep the Python
Polars pin aligned with `pyo3-polars` and the Polars tag in `Cargo.toml`.
Pandas, benchmarks, and C++ comparison tools remain repository development
dependencies, outside the published package. `poetry install` installs the
repository tools.

The package requires CPython 3.11+. Python 3.11–3.14 are tested. The packaging
script runs with any compatible CPython 3.11+ interpreter and prepares its own
tools; it does not use Poetry's Python 3.14 development environment.
`project.requires-python` controls installation eligibility, the Python
classifiers select local compatibility tests, and `tool.cibuildwheel` selects
CI test interpreters. Keep those tested versions aligned when adding support.

## Build locally

From the repository root, using any CPython 3.11+ interpreter:

```sh
python scripts/build_package.py
```

The script installs its Python build tools under `.package-test/build-tools`
and selects the Node major declared in `web/package.json` automatically. It
downloads and caches that Node version locally under `.package-test/node-24`,
including npm. No global Node installation, shell activation, or version switch
is required. Rust 1.96.1+ and the platform linker must be installed; the web build
installs Rust's WASM target and matching wasm-bindgen. Initial downloads require
network access, including access to the locked Polars Git source.

It runs `npm ci` and the static terminal build, then uses PEP 517 to build the
sdist and **build the wheel from that sdist**. Both artifacts are checked before
being copied into `dist/`; the wheel also passes a strict `abi3audit` check.
Its Cargo cache is `target/package`. Use `--out-dir` to choose another output
directory or `--sdist-only` to produce only the source archive for CI.
Use `--skip-web` only after rebuilding assets from the current source. The build
backend checks the terminal entry point, JS chunks, WASM bindings, and WASM file
before creating a distribution. Direct `maturin` commands bypass that guard;
use the build script or `python -m build` for release artifacts.

The sdist includes the compiled terminal, so users rebuilding it only need
Python, Rust, the native platform linker, and access to the Cargo dependencies.
They do not need Node.js or the web source tree. The sdist also includes the
tests and small recorded feed fixtures used by wheel CI. No replay datasets,
secrets, development environments, or node_modules are packaged.

## Install and test the artifact

Packaging tests live in `packaging_tests/`: `test_build_backend.py` checks build
guards, and `packaging/` exercises installed wheels through `scripts/test_wheel.py`.
Library tests remain under `python/tests/`.

Build and test the same wheel across all supported Python minors automatically:

```sh
python scripts/build_package.py --test
```

The script uses uv to discover or download Python 3.11–3.14 and creates a fresh
environment for each. Downloaded interpreters and caches stay in `.package-test`;
the temporary test environments are removed afterward. Every interpreter installs
the **same** `cp311-abi3` wheel. `--only-binary=:all:` makes missing dependency
wheels visible instead of invoking a local compiler. The test runner checks the
installed import path, copies tests/examples/recordings to a
temporary directory, and runs them with no source `lobo` package on PYTHONPATH.
It exercises native matching, each adapter, custom JIT protocols, concurrent
replay, real Polars LazyFrames, Feather finalization, server HTTP/WebSocket
behavior, and the packaged CLI. Packaging-specific tests fetch the actual
terminal/WASM from two independent servers and verify server selection.

To try it manually in any compatible Python environment, install the wheel
printed by the build script, then open its terminal:

```sh
python -m pip install /path/to/pylobo-0.1.1-cp311-abi3-PLATFORM.whl
lobo serve --port 0 --book AAPL --open
```

Replace the wheel path with the generated filename for your OS and architecture.
Installing a wheel requires neither Node.js nor Rust.

The browser-opening test substitutes the OS browser launcher; it does not open
windows on CI. HTTP and WebSocket tests use loopback only. Live exchange
availability is deliberately excluded from deterministic install tests. Visual
WebGPU rendering still needs a supported browser/GPU and is a separate manual
check using `lobo serve --open`.

## GitHub Actions matrix

The [workflow](../.github/workflows/python-package.yml) builds web assets once,
creates a source distribution, then builds every wheel from that same archive.
Each wheel is installed and executed on its corresponding native runner. Linux
builds use manylinux 2.28 containers and cibuildwheel's audit/repair step.

| OS | Architecture | Runner | Wheel target |
| --- | --- | --- | --- |
| Linux, glibc 2.28+ | x86-64 | `ubuntu-24.04` | `manylinux_x86_64` |
| Linux, glibc 2.28+ | ARM64 | `ubuntu-24.04-arm` | `manylinux_aarch64` |
| macOS 11+ | Intel | `macos-15-intel` | `macosx_x86_64` |
| macOS 11+ | Apple Silicon | `macos-15` | `macosx_arm64` |
| Windows | x86-64 | `windows-2025` | `win_amd64` |
| Windows | ARM64 | `windows-11-arm` | `win_arm64` |

Each row builds one `cp311-abi3` wheel and tests that same wheel on Python 3.11,
3.12, 3.13, and 3.14. Cibuildwheel reuses the compatible wheel between interpreters.
Free-threaded and prerelease interpreters are outside this selection.

All six platforms run on every matching push and pull request, as well as on
manual runs and release tags. Cargo downloads and compiled outputs are cached
separately for each runner and architecture. Cache keys include the pinned Rust
and cibuildwheel versions, Cargo manifests/lockfile, and Python build settings;
compatible earlier caches can seed builds after dependency changes. Cargo still
checks which crates need rebuilding. Each commit/run attempt can refresh the
cache, including after a wheel test failure, so a failed test does not discard
completed dependency compilation.

The build output directory lives under the runner's temporary directory, outside
cibuildwheel's extracted source archive. On Linux, the manylinux container uses
that directory through `/host` and mounts a persistent Cargo home. This lets the
cache action restore and save the actual container build outputs. The first run
on each platform is still a full compilation; savings require a cache from a
previous run. Dependency or toolchain changes and cache eviction can require
another full build.

Runner labels follow [GitHub's hosted runner list](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
The architecture-specific build and installed-wheel tests use
[cibuildwheel](https://cibuildwheel.pypa.io/en/stable/options/).
The pinned [Polars runtime release](https://pypi.org/project/polars-runtime-32/1.43.2/#files)
provides wheels for all six targets. Standard x86-64 Polars wheels require a
compatible CPU instruction set; these builds do not promise support for every
historical x86 processor.

The initial test matrix excludes Alpine/musl, 32-bit targets, PyPy, free-threaded
Python, and untested Python minor versions. A cross-platform pass is established
only after all six jobs succeed; a local macOS result does not establish it.
ARM runner availability and billing depend on the repository's GitHub plan.

The workflow runs on pull requests, pushes to `main`/`master`/`develop` and
`release/**`/`hotfix/**`, `v*` tags, and manual dispatch. Download `python-sdist` and
`wheels-*` from the run's artifacts to install elsewhere. After committing and
pushing the changes, it can also be started with:

```sh
gh workflow run python-package.yml --ref YOUR_BRANCH
gh run list --workflow python-package.yml
```

Artifacts from branch and manual runs are retained for 7 days; version-tag
artifacts are retained for 30 days. Published distributions remain on PyPI.

## Publish to PyPI

The pending Trusted Publisher for `pylobo` uses repository `iamorlando/lobo`,
workflow `python-package.yml`, and GitHub environment `pypi`. The publish job
matches these values and uses `id-token: write`; no PyPI API token is needed.
The first successful upload creates the PyPI project and activates the pending
publisher. See the [PyPI Trusted Publishing guide](https://docs.pypi.org/trusted-publishers/creating-a-project-through-oidc/).

Commit and push the release changes, then push a tag matching `project.version`
in `pyproject.toml`. For version `0.1.1`:

```sh
git tag v0.1.1
git push origin v0.1.1
```

The same `v0.1.1` tag also triggers the Rust release. To publish both together,
set the Python project version, Rust workspace version, and all local workspace
dependency versions to `0.1.1`, update `Cargo.lock`, and commit before tagging.
Use a fresh tag and version for each release; published versions cannot be
overwritten by moving an old tag. A separate `rust-v0.1.1` tag is unnecessary
for a combined release.

The workflow rejects a mismatched tag before building. Once the source archive
and all six wheel jobs succeed, the publish job downloads their audited
artifacts and uploads them to [PyPI](https://pypi.org/project/pylobo/) from a
separate Linux job. The GitHub `pypi` environment must allow the release tag;
any environment approval rules apply before publishing. Branch pushes, pull
requests, and manual workflow dispatches build and test without publishing.

Users can then install the release with `python -m pip install pylobo` and
continue to use `import lobo` and the `lobo` command. For each later release,
update the package version and matching version assertions, then push a new
matching tag.
