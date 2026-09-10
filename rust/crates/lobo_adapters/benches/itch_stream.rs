//! Full-file replay benchmarks. Each sample starts with empty books.
//!
//! Defaults to the bundled 2020-01-30 session. When changing LOBO_ITCH_PATH,
//! set LOBO_ITCH_SESSION_DATE=YYYY-MM-DD to the file's session date too.
//! Cutoffs filter messages; replay still scans the complete input file.

use std::{env, hint::black_box, path::PathBuf, time::Duration};

use chrono::{DateTime, NaiveDate, NaiveTime, TimeZone, Utc};
use chrono_tz::America::New_York;
use criterion::{
    BatchSize, BenchmarkGroup, Criterion, SamplingMode, criterion_group, criterion_main,
    measurement::WallTime,
};
use lobo_adapters::itch::{ItchReplayError, ItchReplaySource};
use lobo_primitives::CompressedPrice;
use lobo_replay::ReplayContext;
use lobo_storage::{
    DoNotUpdateUserMap, policies::DoNotUpdateHiddenQuantity, price_level::IntrusivePriceLevel,
    price_sorting::SortedVectorPriceSorting,
};

type ItchReplayContext = ReplayContext<
    IntrusivePriceLevel<CompressedPrice, DoNotUpdateHiddenQuantity>,
    SortedVectorPriceSorting,
    DoNotUpdateUserMap,
    DoNotUpdateHiddenQuantity,
>;

fn itch_path() -> PathBuf {
    env::var_os("LOBO_ITCH_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../../data/NASDAQ/01302020.NASDAQ_ITCH50")
        })
}

fn session_cutoff(time: NaiveTime) -> DateTime<Utc> {
    let date = env::var("LOBO_ITCH_SESSION_DATE").unwrap_or_else(|_| "2020-01-30".into());
    let date = NaiveDate::parse_from_str(&date, "%Y-%m-%d")
        .expect("LOBO_ITCH_SESSION_DATE must be a valid YYYY-MM-DD session date");
    New_York
        .from_local_datetime(&date.and_time(time))
        .single()
        .expect("benchmark cutoff must resolve to one Eastern-time instant")
        .with_timezone(&Utc)
}

fn bench_replay(
    group: &mut BenchmarkGroup<'_, WallTime>,
    name: &str,
    cutoff_time: Option<NaiveTime>,
    load_sources: impl Fn() -> Result<Vec<ItchReplaySource>, ItchReplayError>,
) {
    group.bench_function(name, |bencher| {
        // Discovery and timezone-aware input construction are outside timing.
        // Keeping them here also lets --list and filtered runs skip unused data.
        let sources = load_sources().expect("ITCH sources must be readable");
        assert!(
            !sources.is_empty(),
            "ITCH file must declare at least one ticker"
        );
        let source_refs = sources.iter().collect::<Vec<_>>();
        let cutoff_time = cutoff_time.map(session_cutoff);

        bencher.iter_batched_ref(
            || ItchReplayContext::with_capacity_and_cutoff(sources.len(), cutoff_time),
            |context| {
                context
                    .replay_from_sources(black_box(&source_refs))
                    .expect("ITCH replay must succeed");
                black_box(&context.books);
            },
            // Exclude context setup and populated-book destruction from timing,
            // and retain only one all-ticker book set at a time.
            BatchSize::PerIteration,
        );
    });
}

fn all_tickers_cutoff_2pm(group: &mut BenchmarkGroup<'_, WallTime>) {
    // 14:00:00 + 21_321_817_658 ns = 14:00:21.321817658 Eastern.
    let cutoff = NaiveTime::from_hms_nano_opt(14, 0, 21, 321_817_658).unwrap();
    bench_replay(group, "all_tickers_cutoff_2pm", Some(cutoff), || {
        ItchReplaySource::from_file_all(itch_path())
    });
}

fn aapl_cutoff_none(group: &mut BenchmarkGroup<'_, WallTime>) {
    bench_replay(group, "aapl_cutoff_none", None, || {
        ItchReplaySource::from_file(itch_path(), "AAPL").map(|source| vec![source])
    });
}

fn aapl_cutoff_end_of_day(group: &mut BenchmarkGroup<'_, WallTime>) {
    // 86_399_999_999_999 ns from midnight = 23:59:59.999999999 Eastern.
    // The adapter owns conversion to its internal nanosecond cutoff.
    let cutoff = NaiveTime::from_hms_nano_opt(23, 59, 59, 999_999_999).unwrap();
    bench_replay(group, "aapl_cutoff_end_of_day", Some(cutoff), || {
        ItchReplaySource::from_file(itch_path(), "AAPL").map(|source| vec![source])
    });
}

fn cutoff_benches(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("itch_replay");
    group.sampling_mode(SamplingMode::Flat);
    all_tickers_cutoff_2pm(&mut group);
    aapl_cutoff_none(&mut group);
    aapl_cutoff_end_of_day(&mut group);
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(10));
    targets = cutoff_benches
}
criterion_main!(benches);
