# Building and testing the Python distribution

The release artifacts are a `lobo` source distribution (`.tar.gz`) and native
CPython stable-ABI wheels (`.whl`). A `cp311-abi3` wheel supports compatible
CPython versions from 3.11 onward on its target OS and architecture; it is not
tied to the Python minor version that built it. This workflow builds locally and in GitHub
Actions; it contains no PyPI upload or release-publishing step.

## What is included

- The PyO3 extension with all shipped adapter bindings (ITCH, Kraken, Bitfinex),
  custom protocol/JIT support, the native order API/server, parallel replay, and
  native Polars conversions.
- The Python public modules, generated type stubs, and `py.typed` marker.
- The production static Next.js terminal, local fonts, and WebAssembly module
  under `lobo/_web`. Rust serves these files; there is no Node server at runtime.
- `lobo serve`, `lobo web --server URL`, and `python -m lobo` entry points.

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
python -m pip install /path/to/lobo-0.1.0-cp311-abi3-PLATFORM.whl
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
`release/**`/`hotfix/**`, and manual dispatch. Download `python-sdist` and
`wheels-*` from the run's artifacts to install elsewhere. After committing and
pushing the changes, it can also be started with:

```sh
gh workflow run python-package.yml --ref YOUR_BRANCH
gh run list --workflow python-package.yml
```

Publishing remains a later step: select a release version/name on PyPI, verify
every target, and configure publishing credentials or trusted publishing then.
