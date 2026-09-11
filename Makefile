
CARGO_TARGET_DIR ?= $(CURDIR)/rust/target
RUST_PUBLISH_SCOPE = --workspace --exclude lobo_py --exclude lobo_wasm --exclude lobo_replay_tests
RUST_SCOPE = --workspace --exclude lobo_py --no-default-features
BENCH_SCOPE = -p lobo_books --bench simulation --no-default-features
PYTHON_TESTS = python/tests/lobo python/tests/benchmarks
RUST_COVERAGE_DIR = rust/reports/coverage/rust
RUST_COVERAGE_TARGET_DIR = $(CARGO_TARGET_DIR)/coverage
BENCHMARK_BASELINE ?= lobo-main
BENCHMARK_REGRESSION_PERCENT ?= 10
BENCHMARK_EXPECTED_RESULTS = 100
BENCHMARK_ROUNDS = 5

PY_BENCH_SCRIPT = python/benchmarks/bench.py
PY_BENCH_ARTIFACTS = python/benchmarks/artifacts
PY_BENCH_SCENARIO ?= all
PY_BENCH_OPTIONS ?= --processes 1 --values 1 --warmups 0 --loops 1
PY_BENCH_BEFORE ?= $(PY_BENCH_ARTIFACTS)/before.json
PY_BENCH_AFTER ?= $(PY_BENCH_ARTIFACTS)/after.json

REPLAY_SMALL_SAMPLES ?= 100000
REPLAY_SAMPLES = $(if $(samples),$(samples),$(if $(filter small,$(MAKECMDGOALS)),$(REPLAY_SMALL_SAMPLES),))

.PHONY: run replay small

run:
	@:

REPLAY_BINARY = $(CURDIR)/rust/target/profiling/examples/intrusive_cpp_comparator

replay:
	@echo "Building profiling executable"
	CARGO_TARGET_DIR="$(CURDIR)/rust/target" \
	cargo build \
		--profile profiling \
		-p lobo_adapters \
		--features itchy \
		--example intrusive_cpp_comparator

	@echo "Generating macOS debug symbols"
	rm -rf "$(REPLAY_BINARY).dSYM"
	xcrun dsymutil \
		"$(REPLAY_BINARY)" \
		-o "$(REPLAY_BINARY).dSYM"

	@echo "Profiling $(if $(REPLAY_SAMPLES),$(REPLAY_SAMPLES) messages,full file)"
	@export LOBO_PROFILE_SKIP=0; \
	if [ -n "$(REPLAY_SAMPLES)" ]; then \
		export LOBO_PROFILE_COUNT="$(REPLAY_SAMPLES)"; \
	else \
		unset LOBO_PROFILE_COUNT; \
	fi; \
	flamegraph \
		-F 997 \
		-o intrusive_cpp_comparator.svg \
		-- "$(REPLAY_BINARY)"

	@open intrusive_cpp_comparator.svg


small:
	@:

.PHONY: build build-rust test test-rust test-python test-benches coverage bench bench-force bench-baseline bench-list py

build: build-rust py

build-rust:
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" cargo build $(RUST_SCOPE)

.PHONY: check-lobo publish-rust-check publish-rust

# Test the public crate and each independently selectable feature.
check-lobo:
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" python3 scripts/check_lobo.py

# Cargo publishes workspace dependencies in order (Cargo 1.90+).
publish-rust-check:
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" cargo publish $(RUST_PUBLISH_SCOPE) --dry-run

publish-rust:
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" cargo publish $(RUST_PUBLISH_SCOPE)

test: coverage

test-rust:
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" cargo test $(RUST_SCOPE)
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" cargo test --release $(BENCH_SCOPE)

test-python: py
	poetry run coverage erase
	poetry run coverage run -m pytest $(PYTHON_TESTS) -q
	poetry run coverage report
	poetry run coverage html

test-benches:
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" cargo test --release $(BENCH_SCOPE)
	poetry run pytest python/tests/benchmarks -q

coverage:
	@command -v cargo-llvm-cov >/dev/null || { \
		echo "cargo-llvm-cov is required: cargo install cargo-llvm-cov"; \
		exit 1; \
	}
	@rm -f "$(CURDIR)"/*.profraw
	CARGO_TARGET_DIR="$(RUST_COVERAGE_TARGET_DIR)" sh -ec '\
		restore_uninstrumented_extension() { \
			status=$$?; \
			cargo llvm-cov clean --workspace >/dev/null 2>&1 || true; \
			unset LLVM_PROFILE_FILE \
				__CARGO_LLVM_COV_RUSTC_WRAPPER \
				__CARGO_LLVM_COV_RUSTC_WRAPPER_RUSTFLAGS \
				__CARGO_LLVM_COV_RUSTC_WRAPPER_CRATE_NAMES \
				RUSTC_WRAPPER \
				CARGO_LLVM_COV \
				CARGO_LLVM_COV_SHOW_ENV \
				CARGO_LLVM_COV_TARGET_DIR \
				CARGO_LLVM_COV_BUILD_DIR; \
			CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" $(MAKE) py >/dev/null \
				|| status=$$?; \
			rm -f "$(CURDIR)"/*.profraw; \
			trap - EXIT; \
			exit $$status; \
		}; \
		trap restore_uninstrumented_extension EXIT; \
		eval "$$(cargo llvm-cov show-env --sh)"; \
		cargo llvm-cov clean --workspace; \
		cargo test $(RUST_SCOPE); \
		cargo test --release $(BENCH_SCOPE); \
		$(MAKE) py; \
		poetry run coverage erase; \
		poetry run coverage run -m pytest $(PYTHON_TESTS) -q; \
		poetry run coverage report; \
		poetry run coverage html; \
		cargo llvm-cov report --summary-only --fail-under-lines 90; \
		cargo llvm-cov report --html --output-dir $(RUST_COVERAGE_DIR)'

bench:
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" \
	BENCHMARK_BASELINE="$(BENCHMARK_BASELINE)" \
	BENCHMARK_REGRESSION_PERCENT="$(BENCHMARK_REGRESSION_PERCENT)" \
	BENCHMARK_EXPECTED_RESULTS="$(BENCHMARK_EXPECTED_RESULTS)" \
	BENCHMARK_ROUNDS="$(BENCHMARK_ROUNDS)" \
	bash scripts/run_benchmarks.sh check

bench-force:
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" \
	BENCHMARK_BASELINE="$(BENCHMARK_BASELINE)" \
	BENCHMARK_REGRESSION_PERCENT="$(BENCHMARK_REGRESSION_PERCENT)" \
	BENCHMARK_EXPECTED_RESULTS="$(BENCHMARK_EXPECTED_RESULTS)" \
	BENCHMARK_ROUNDS="$(BENCHMARK_ROUNDS)" \
	BENCHMARK_FORCE=1 \
	bash scripts/run_benchmarks.sh check

bench-baseline:
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" \
	BENCHMARK_BASELINE="$(BENCHMARK_BASELINE)" \
	BENCHMARK_REGRESSION_PERCENT="$(BENCHMARK_REGRESSION_PERCENT)" \
	BENCHMARK_EXPECTED_RESULTS="$(BENCHMARK_EXPECTED_RESULTS)" \
	BENCHMARK_ROUNDS="$(BENCHMARK_ROUNDS)" \
	bash scripts/run_benchmarks.sh baseline

bench-list:
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" cargo bench $(BENCH_SCOPE) -- --list



py: web-server-assets
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" poetry run maturin develop --generate-stubs --features "polars concurrent"
	poetry run isort python/lobo

py-release: web-server-assets
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" poetry run maturin develop --profile profiling --generate-stubs --no-default-features --features "polars concurrent"
	poetry run isort python/lobo

py-bench:
	CARGO_TARGET_DIR="$(CARGO_TARGET_DIR)" poetry run maturin develop --profile profiling --generate-stubs --no-default-features --features "concurrent"

PY_BENCH_CASES = aapl all feather feather-aapl \
	aapl-single-threaded all-single-threaded feather-single-threaded feather-aapl-single-threaded

.PHONY: py-bench bench-py bench-py-record bench-py-compare \
	$(addprefix bench-py-,$(PY_BENCH_CASES)) \
	$(addprefix bench-py-record-,$(PY_BENCH_CASES)) \
	$(addprefix bench-py-compare-,$(PY_BENCH_CASES))

bench-py: py-bench
	poetry run python $(PY_BENCH_SCRIPT) \
		--scenario "$(PY_BENCH_SCENARIO)" $(PY_BENCH_OPTIONS)

bench-py-record: py-bench
	@mkdir -p "$(dir $(PY_BENCH_BEFORE))"
	@rm -f "$(PY_BENCH_BEFORE).tmp"
	poetry run python $(PY_BENCH_SCRIPT) \
		--scenario "$(PY_BENCH_SCENARIO)" $(PY_BENCH_OPTIONS) \
		--output "$(PY_BENCH_BEFORE).tmp"
	@mv "$(PY_BENCH_BEFORE).tmp" "$(PY_BENCH_BEFORE)"

bench-py-compare: py-bench
	@mkdir -p "$(dir $(PY_BENCH_AFTER))"
	@rm -f "$(PY_BENCH_AFTER).tmp"
	poetry run python $(PY_BENCH_SCRIPT) \
		--scenario "$(PY_BENCH_SCENARIO)" $(PY_BENCH_OPTIONS) \
		--output "$(PY_BENCH_AFTER).tmp"
	@mv "$(PY_BENCH_AFTER).tmp" "$(PY_BENCH_AFTER)"
	poetry run python -m pyperf compare_to \
		"$(PY_BENCH_BEFORE)" "$(PY_BENCH_AFTER)" --table

# Keep the existing "all" target name for the all-tickers scenario.
$(addprefix bench-py-,$(PY_BENCH_CASES)): bench-py-%:
	$(MAKE) bench-py PY_BENCH_SCENARIO=$(patsubst all%,all-tickers%,$*)

$(addprefix bench-py-record-,$(PY_BENCH_CASES)): bench-py-record-%:
	$(MAKE) bench-py-record \
		PY_BENCH_SCENARIO=$(patsubst all%,all-tickers%,$*) \
		PY_BENCH_BEFORE=$(PY_BENCH_ARTIFACTS)/before-$(patsubst all%,all-tickers%,$*).json

$(addprefix bench-py-compare-,$(PY_BENCH_CASES)): bench-py-compare-%:
	$(MAKE) bench-py-compare \
		PY_BENCH_SCENARIO=$(patsubst all%,all-tickers%,$*) \
		PY_BENCH_BEFORE=$(PY_BENCH_ARTIFACTS)/before-$(patsubst all%,all-tickers%,$*).json \
		PY_BENCH_AFTER=$(PY_BENCH_ARTIFACTS)/after-$(patsubst all%,all-tickers%,$*).json


.PHONY: web web-build wasm
.PHONY: web-server-assets
web-server-assets:
	cd web && npm install && npm run build:server

web:
	cd web && npm install && npm run dev

web-build:
	cd web && npm ci && npm run build

wasm:
	cd web && npm install && npm run wasm

.PHONY: samply
samply:
	@export LOBO_PROFILE_SKIP=0; \
	if [ -n "$(REPLAY_SAMPLES)" ]; then \
		export LOBO_PROFILE_COUNT="$(REPLAY_SAMPLES)"; \
	else \
		unset LOBO_PROFILE_COUNT; \
	fi; \
	samply record "$(REPLAY_BINARY)"
