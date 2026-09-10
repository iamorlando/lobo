#!/usr/bin/env bash

set -euo pipefail

mode=${1:-check}
repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repository_root"

benchmark_baseline=${BENCHMARK_BASELINE:-lobo-main}
max_regression_percent=${BENCHMARK_REGRESSION_PERCENT:-10}
expected_results=${BENCHMARK_EXPECTED_RESULTS:-100}
rounds_per_scenario=${BENCHMARK_ROUNDS:-5}
target_dir=${CARGO_TARGET_DIR:-"$repository_root/target"}
criterion_dir="$target_dir/criterion"
cache_marker="$criterion_dir/.lobo-benchmark-pass"
force=${BENCHMARK_FORCE:-0}
baseline_backup_suffix=".lobo-baseline-backup-$$"

validate_baseline_name() {
    case "$benchmark_baseline" in
        ''|.|..|*[!A-Za-z0-9._-]*)
            printf 'Invalid benchmark baseline name: %q\n' "$benchmark_baseline" >&2
            exit 2
            ;;
    esac
}

find_named_baseline_directories() {
    if [[ -d "$criterion_dir" ]]; then
        find "$criterion_dir" \
            -mindepth 2 \
            -type d \
            -name "$benchmark_baseline" \
            -prune \
            -print
    fi
}

remove_named_baseline_directories() {
    while IFS= read -r baseline_dir; do
        rm -rf -- "$baseline_dir"
    done < <(find_named_baseline_directories)
}

backup_named_baseline_directories() {
    while IFS= read -r baseline_dir; do
        mv -- "$baseline_dir" "${baseline_dir}${baseline_backup_suffix}"
    done < <(find_named_baseline_directories)
}

finish_baseline_replacement() {
    status=$?
    trap - EXIT

    if [[ $status -eq 0 ]]; then
        find "$criterion_dir" \
            -mindepth 2 \
            -type d \
            -name "${benchmark_baseline}${baseline_backup_suffix}" \
            -prune \
            -exec rm -rf -- {} +
    else
        remove_named_baseline_directories
        while IFS= read -r backup_dir; do
            mv -- "$backup_dir" "${backup_dir%${baseline_backup_suffix}}"
        done < <(
            find "$criterion_dir" \
                -mindepth 2 \
                -type d \
                -name "${benchmark_baseline}${baseline_backup_suffix}" \
                -prune \
                -print
        )
    fi

    exit "$status"
}

source_fingerprint=$(
    {
        rustc -Vv
        cargo -V
        printf '%s\n' \
            "$benchmark_baseline" \
            "$max_regression_percent" \
            "$expected_results" \
            "$rounds_per_scenario"
        find \
            Cargo.toml \
            Cargo.lock \
            Makefile \
            rust/crates \
            scripts/check_benchmark_regressions.py \
            scripts/run_benchmarks.sh \
            -type f \
            \( \
                -name '*.rs' \
                -o -name 'Cargo.toml' \
                -o -name 'Cargo.lock' \
                -o -name 'Makefile' \
                -o -name '*.py' \
                -o -name '*.sh' \
            \) \
            -print \
            | LC_ALL=C sort \
            | while IFS= read -r path; do
                shasum -a 256 "$path"
            done
    } | shasum -a 256 | awk '{print $1}'
)

verify_baseline() {
    poetry run python scripts/check_benchmark_regressions.py verify-baseline \
        --criterion-dir "$criterion_dir" \
        --name "$benchmark_baseline" \
        --expected "$expected_results"
}

baseline_fingerprint() {
    find "$criterion_dir" \
        -type f \
        -path "*/$benchmark_baseline/estimates.json" \
        -print \
        | LC_ALL=C sort \
        | while IFS= read -r path; do
            shasum -a 256 "$path"
        done \
        | shasum -a 256 \
        | awk '{print $1}'
}

cache_key() {
    printf '%s\n%s\n' "$source_fingerprint" "$(baseline_fingerprint)" \
        | shasum -a 256 \
        | awk '{print $1}'
}

write_cache_marker() {
    mkdir -p "$criterion_dir"
    cache_key >"$cache_marker"
}

case "$mode" in
    baseline)
        validate_baseline_name
        backup_named_baseline_directories
        trap finish_baseline_replacement EXIT
        cargo bench -p lobo_books --bench simulation --no-default-features -- \
            --save-baseline "$benchmark_baseline"
        verify_baseline
        write_cache_marker
        printf 'Saved benchmark baseline %q and cached this passing source state.\n' \
            "$benchmark_baseline"
        ;;
    check)
        verify_baseline
        current_cache_key=$(cache_key)
        if [[ "$force" != "1" && -f "$cache_marker" ]]; then
            cached_key=$(<"$cache_marker")
            if [[ "$cached_key" == "$current_cache_key" ]]; then
                printf 'Benchmark inputs and baseline are unchanged; using cached passing result.\n'
                exit 0
            fi
        fi

        comparison_started=$(( $(date +%s) - 1 ))
        cargo bench -p lobo_books --bench simulation --no-default-features -- \
            --baseline "$benchmark_baseline"
        poetry run python scripts/check_benchmark_regressions.py check \
            --criterion-dir "$criterion_dir" \
            --since "$comparison_started" \
            --expected "$expected_results" \
            --rounds-per-scenario "$rounds_per_scenario" \
            --max-regression-percent "$max_regression_percent"
        write_cache_marker
        ;;
    *)
        printf 'Usage: %s [baseline|check]\n' "$0" >&2
        exit 2
        ;;
esac
