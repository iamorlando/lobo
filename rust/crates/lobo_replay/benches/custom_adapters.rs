//! Protocol parsing, book mutations, and publication.
//! --paired alternates execution order in adjacent timing blocks to distinguish
//! implementation overhead from machine load/frequency changes between cases.
#[path = "../tests/support/definitions.rs"]
mod examples;
use criterion::{BenchmarkId, Criterion, Throughput};
use lobo_replay::custom::MarketDataAdapter;
use std::{
    hint::black_box,
    time::{Duration, Instant},
};
struct Case<'a> {
    venue: &'static str,
    name: &'static str,
    records: u64,
    run: Box<dyn FnMut() -> (u64, Duration) + 'a>,
}
fn itch_input() -> Vec<u8> {
    let mut input = Vec::new();
    for id in 1u64..=2048 {
        let mut body = id.to_be_bytes().to_vec();
        body.push(b'B');
        body.extend(100u32.to_be_bytes());
        body.extend(b"AAPL    ");
        body.extend(1000000u32.to_be_bytes());
        for (tag, body) in [(b'A', body), (b'D', id.to_be_bytes().to_vec())] {
            input.extend(((11 + body.len()) as u16).to_be_bytes());
            input.push(tag);
            input.extend(1u16.to_be_bytes());
            input.extend(0u16.to_be_bytes());
            input.extend(&id.to_be_bytes()[2..]);
            input.extend(body);
        }
    }
    input
}
fn benchmarks(c: &mut Criterion, paired: bool) {
    let itch = itch_input();
    let directory=br#"{"channel":"instrument","data":{"pairs":[{"symbol":"BTC/USD","price_precision":1,"qty_precision":8,"status":"online"}]}}"#;
    let snapshot = include_bytes!("../../lobo_adapters/tests/fixtures/kraken_snapshot.json");
    let trace = include_str!("../../lobo_adapters/tests/fixtures/bitfinex_r0.jsonl")
        .lines()
        .filter(|s| !s.is_empty())
        .map(|s| s.as_bytes().to_vec())
        .collect::<Vec<_>>();
    let server_snapshot=br#"{"type":"snapshot","book":{"symbol":"BOOK","price_decimals":4,"quantity_decimals":0,"policy":"full"},"sequence":0,"timestamp_ns":0,"orders":[]}"#;
    let mut commands = Vec::new();
    for n in 0..100 {
        commands.push(format!(r#"{{"type":"update","book":"BOOK","sequence":{},"timestamp_ns":{},"command":{{"op":"add","order":{{"type":"limit","id":"00000000-0000-0000-0000-000000000001","side":"buy","quantity":10,"price":10000}}}}}}"#,2*n+1,n));
        commands.push(format!(r#"{{"type":"update","book":"BOOK","sequence":{},"timestamp_ns":{},"command":{{"op":"remove","id":"00000000-0000-0000-0000-000000000001"}}}}"#,2*n+2,n));
    }
    macro_rules! itch {
        ($name:expr,$new:expr) => {
            Case {
                venue: "itch",
                name: $name,
                records: 4096,
                run: Box::new(|| {
                    let mut a = $new;
                    let processing = Instant::now();
                    a.receive(black_box(&itch), true).unwrap();
                    a.advance(u64::MAX, 10000).unwrap();
                    (black_box(a.state().messages), processing.elapsed())
                }),
            }
        };
    }
    macro_rules! kraken {
        ($name:expr,$new:expr) => {
            Case {
                venue: "kraken",
                name: $name,
                records: 100,
                run: Box::new(|| {
                    let mut a = $new;
                    let processing = Instant::now();
                    a.connected().unwrap();
                    a.receive(directory, false).unwrap();
                    for _ in 0..100 {
                        a.receive(black_box(snapshot), false).unwrap();
                    }
                    (black_box(a.state().checksum_checks), processing.elapsed())
                }),
            }
        };
    }
    macro_rules! bitfinex {
        ($name:expr,$new:expr) => {
            Case {
                venue: "bitfinex",
                name: $name,
                records: trace.len() as u64,
                run: Box::new(|| {
                    let mut a = $new;
                    let processing = Instant::now();
                    a.bootstrap("instruments", br#"[["BTCUSD"]]"#).unwrap();
                    a.connected().unwrap();
                    for frame in &trace {
                        a.receive(black_box(frame), false).unwrap();
                    }
                    (black_box(a.state().messages), processing.elapsed())
                }),
            }
        };
    }
    macro_rules! server {
        ($name:expr,$new:expr) => {
            Case {
                venue: "server",
                name: $name,
                records: commands.len() as u64,
                run: Box::new(|| {
                    let mut a = $new;
                    let processing = Instant::now();
                    a.receive(server_snapshot, false).unwrap();
                    for command in &commands {
                        a.receive(black_box(command.as_bytes()), false).unwrap();
                    }
                    (black_box(a.state().messages), processing.elapsed())
                }),
            }
        };
    }
    let mut cases = vec![
        itch!(
            "original",
            lobo_adapters::itch::stream::ItchStream::new("AAPL", 0).unwrap()
        ),
        itch!("custom", examples::itch::new("AAPL", 0).unwrap()),
        kraken!(
            "original",
            lobo_adapters::kraken::Kraken::new("BTC/USD", 10).unwrap()
        ),
        kraken!("custom", examples::kraken::new("BTC/USD", 10).unwrap()),
        bitfinex!(
            "original",
            lobo_adapters::bitfinex::Bitfinex::new("BTCUSD").unwrap()
        ),
        bitfinex!("custom", examples::bitfinex::new("BTCUSD").unwrap()),
        server!(
            "original",
            lobo_replay::order_messages::OrderMessages::new("BOOK").unwrap()
        ),
        server!("custom", examples::server::new("BOOK").unwrap()),
    ];
    if let Ok(name) = std::env::var("LOBO_PROFILE_CASE") {
        let case = cases
            .iter_mut()
            .find(|case| format!("{}.{}", case.venue, case.name) == name)
            .expect("unknown profiling case");
        println!("Profiling {name}; PID {}", std::process::id());
        let end = Instant::now() + Duration::from_secs(15);
        while Instant::now() < end {
            black_box((case.run)());
        }
        return;
    }
    if paired {
        compare_pairs(&mut cases);
        return;
    }
    let mut group = c.benchmark_group("custom_adapter_parity");
    for case in cases {
        group.throughput(Throughput::Elements(case.records));
        let mut run = case.run;
        group.bench_function(BenchmarkId::new(case.venue, case.name), |b| {
            b.iter_custom(|loops| (0..loops).map(|_| black_box(run()).1).sum::<Duration>())
        });
    }
    group.finish();
}
fn timed(case: &mut Case<'_>, loops: usize) -> f64 {
    let elapsed: Duration = (0..loops).map(|_| black_box((case.run)()).1).sum();
    elapsed.as_secs_f64() / loops as f64
}
fn compare_pairs(cases: &mut [Case<'_>]) {
    const ROUNDS: usize = 100;
    const LOOPS: usize = 16;
    let mut output = serde_json::Map::new();
    for pair in cases.chunks_mut(2) {
        assert_eq!(pair[0].venue, pair[1].venue);
        for _ in 0..50 {
            black_box((pair[0].run)());
            black_box((pair[1].run)());
        }
        let mut samples = Vec::with_capacity(ROUNDS);
        for round in 0..ROUNDS {
            let mut seconds = [0.0; 2];
            for index in [round % 2, 1 - round % 2] {
                seconds[index] = timed(&mut pair[index], LOOPS);
            }
            samples.push(seconds);
        }
        let median = |mut values: Vec<f64>| {
            values.sort_by(f64::total_cmp);
            values[values.len() / 2]
        };
        let old = median(samples.iter().map(|s| s[0]).collect());
        let new = median(samples.iter().map(|s| s[1]).collect());
        let ratio = median(samples.iter().map(|s| s[1] / s[0]).collect());
        println!(
            "{}: original {:.3} us | custom {:.3} us | median paired ratio {:.4}",
            pair[0].venue,
            old * 1e6,
            new * 1e6,
            ratio
        );
        output.insert(pair[0].venue.into(),serde_json::json!({"timing_scope":"processing_after_adapter_construction","rounds":ROUNDS,"iterations_per_block":LOOPS,"records_per_iteration":pair[0].records,"median_paired_ratio":ratio,"original_median_seconds":old,"custom_median_seconds":new,"samples_seconds":samples}));
    }
    let path = std::env::var_os("LOBO_PAIRED_OUTPUT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| "custom-protocol-paired.json".into());
    std::fs::write(path, serde_json::to_vec_pretty(&output).unwrap()).unwrap();
}
#[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
fn startup() {
    for name in ["itch", "kraken", "bitfinex", "server"] {
        let definition = examples::definition(name);
        let mut elapsed = Vec::new();
        for iteration in 0..10 {
            let mut definition = (*definition).clone();
            // The cache key includes this existing metadata field. Distinct
            // definitions measure actual compilation, not cache retrieval.
            definition.simulation_note = Some(format!(
                "startup-{}-{iteration}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let descriptor = examples::descriptor(name, "BOOK");
            let start = Instant::now();
            let compiled = lobo_replay::custom::definition::DefinedProtocol::new(
                std::sync::Arc::new(definition),
                descriptor,
                "BOOK",
            )
            .unwrap();
            elapsed.push(start.elapsed().as_secs_f64());
            black_box(compiled);
        }
        elapsed.sort_by(f64::total_cmp);
        println!(
            "{name}: uncached protocol construction {:.3} ms",
            elapsed[elapsed.len() / 2] * 1000.0
        );
    }
}
fn main() {
    #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
    if std::env::args().any(|a| a == "--startup") {
        startup();
        return;
    }
    let paired = std::env::args().any(|a| a == "--paired");
    let mut criterion = if paired {
        Criterion::default()
    } else {
        Criterion::default().configure_from_args()
    };
    benchmarks(&mut criterion, paired);
    if !paired {
        criterion.final_summary();
    }
}
