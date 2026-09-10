// Full comparison:
//
// cargo test -p lobo_adapters \
//   --features itchy \
//   --test intrusive_cpp_comparator \
//   --release \
//   -- --nocapture
//
// Profile a representative slice:
//
// LOBO_PROFILE_SKIP=100000 \
// LOBO_PROFILE_COUNT=1000000 \
// CARGO_PROFILE_BENCH_DEBUG=true \
// cargo flamegraph \
//   -p lobo_adapters \
//   --features itchy \
//   --example intrusive_cpp_comparator \
//   --release \
//   -o intrusive_cpp_comparator.svg \
//   --open \
//   -- --nocapture

use std::{
    env,
    error::Error,
    fs,
    hint::black_box,
    path::PathBuf,
    time::{Duration, Instant},
};

use itchy::{Body, MessageStream};

use lobo_adapters::itch::messages::adapt_message;
use lobo_books::price_time_priority::Book;
use lobo_primitives::CompressedPrice;
use lobo_storage::{
    DoNotUpdateUserMap, policies::DoNotUpdateHiddenQuantity, price_level::IntrusivePriceLevel,
    price_sorting::SortedVectorPriceSorting,
};

type IntrusiveBook = Book<
    IntrusivePriceLevel<CompressedPrice, DoNotUpdateHiddenQuantity>,
    SortedVectorPriceSorting,
    DoNotUpdateUserMap,
    DoNotUpdateHiddenQuantity,
>;

// Relative to this crate's manifest directory.
const DEFAULT_PATH: &str = "../../../data/NASDAQ/01302020.NASDAQ_ITCH50";

const PATH_ENV: &str = "LOBO_ITCH_PATH";

const PROFILE_SKIP_ENV: &str = "LOBO_PROFILE_SKIP";
const PROFILE_COUNT_ENV: &str = "LOBO_PROFILE_COUNT";

// Published C++ full_book results.
const COMPARATOR_MESSAGES: u64 = 423_285_709;
const COMPARATOR_OPERATIONS: u64 = 417_219_234;

const COMPARATOR_ADDS: u64 = 186_610_705;
const COMPARATOR_EXECUTIONS: u64 = 8_555_084;
const COMPARATOR_DELETES: u64 = 180_285_101;
const COMPARATOR_REPLACES: u64 = 36_777_372;
const COMPARATOR_CANCELS: u64 = 4_990_972;

const COMPARATOR_SECONDS: f64 = 61.673;
const COMPARATOR_SYMBOLS: usize = 8_900;

fn env_u64(name: &str) -> Option<u64> {
    env::var(name)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
}

fn main() -> Result<(), Box<dyn Error>> {
    let path = env::var_os(PATH_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_PATH));

    let profile_skip = env_u64(PROFILE_SKIP_ENV).unwrap_or(0);
    let profile_count = env_u64(PROFILE_COUNT_ENV);

    let sliced_run = profile_skip != 0 || profile_count.is_some();

    let file_size = fs::metadata(&path)?.len();

    let stream = MessageStream::from_file(&path)?;

    // stock_locate is a u16, so use it directly as an index.
    let mut books: Vec<Option<IntrusiveBook>> = (0..=u16::MAX).map(|_| None).collect();

    // Total messages actually read, including warm-up.
    let mut source_messages_scanned = 0_u64;

    // These counters cover only the measured window.
    let mut source_messages = 0_u64;
    let mut book_messages = 0_u64;
    let mut replay_events = 0_u64;

    let mut adds = 0_u64;
    let mut executions = 0_u64;
    let mut cancels = 0_u64;
    let mut deletes = 0_u64;
    let mut replaces = 0_u64;

    // With no warm-up, timing begins immediately.
    // Otherwise it begins after the first `profile_skip`
    // source messages have been fully applied to the books.
    let mut started = if profile_skip == 0 {
        Some(Instant::now())
    } else {
        None
    };

    let mut elapsed = Duration::ZERO;

    for message in stream {
        let message = message?;
        source_messages_scanned += 1;

        // Warm-up is complete. Start timing immediately before
        // processing the first message in the measured window.
        if started.is_none() && source_messages_scanned > profile_skip {
            started = Some(Instant::now());
        }

        let measuring = source_messages_scanned > profile_skip;

        // Stop before processing another measured message once
        // the requested window size has been reached.
        if measuring && profile_count.is_some_and(|max| source_messages >= max) {
            elapsed = started.unwrap().elapsed();
            break;
        }

        if measuring {
            source_messages += 1;
        }

        // Important:
        //
        // Even during warm-up, book-changing messages MUST be applied.
        // Later executes/cancels/deletes/replaces may reference orders
        // created in the warm-up prefix.
        let is_book_message = match &message.body {
            Body::AddOrder(_) => {
                if measuring {
                    adds += 1;
                }
                true
            }

            Body::OrderExecuted { .. } | Body::OrderExecutedWithPrice { .. } => {
                if measuring {
                    executions += 1;
                }
                true
            }

            Body::OrderCancelled { .. } => {
                if measuring {
                    cancels += 1;
                }
                true
            }

            Body::DeleteOrder { .. } => {
                if measuring {
                    deletes += 1;
                }
                true
            }

            Body::ReplaceOrder(_) => {
                if measuring {
                    replaces += 1;
                }
                true
            }

            _ => false,
        };

        if !is_book_message {
            continue;
        }

        if measuring {
            book_messages += 1;
        }

        let stock_locate = message.stock_locate as usize;

        let book = books[stock_locate].get_or_insert_with(Default::default);

        if let Some(result) = adapt_message(message, book) {
            result.map_err(|error| format!("ITCH replay failed: {error:?}"))?;
            if measuring {
                replay_events += 1;
            }
        }
    }

    // If the loop ended naturally rather than through PROFILE_COUNT.
    if elapsed.is_zero() {
        let Some(started) = started else {
            return Err(format!(
                "LOBO_PROFILE_SKIP={profile_skip} exceeds the number of messages in the file"
            )
            .into());
        };

        elapsed = started.elapsed();
    }

    let seconds = elapsed.as_secs_f64();

    black_box(&books);

    let messages_per_second = source_messages as f64 / seconds;

    let operations_per_second = replay_events as f64 / seconds;

    let book_count = books.iter().filter(|book| book.is_some()).count();

    let live_orders: usize = books
        .iter()
        .filter_map(Option::as_ref)
        .map(|book| book.order_storage.bids.len() + book.order_storage.asks.len())
        .sum();

    let live_price_levels: usize = books
        .iter()
        .filter_map(Option::as_ref)
        .map(|book| {
            book.order_storage.bids.price_level_count()
                + book.order_storage.asks.price_level_count()
        })
        .sum();

    println!();
    println!("========================================");

    if sliced_run {
        println!("LOBO FULL_BOOK PROFILE WINDOW");
    } else {
        println!("LOBO FULL_BOOK");
    }

    println!("========================================");

    println!("file={}", path.display());
    println!("file_bytes={file_size}");

    println!();

    if sliced_run {
        println!("profile_skip={profile_skip}");

        match profile_count {
            Some(count) => println!("profile_count={count}"),
            None => println!("profile_count=ALL_REMAINING"),
        }

        println!("source_messages_scanned_including_warmup={source_messages_scanned}");
    }

    println!();
    println!("MEASURED WINDOW");
    println!("source_messages={source_messages}");
    println!("book_messages={book_messages}");
    println!("replay_events={replay_events}");

    println!();
    println!("adds={adds}");
    println!("executions={executions}");
    println!("deletes={deletes}");
    println!("replaces={replaces}");
    println!("cancels={cancels}");

    println!();
    println!("books={book_count}");
    println!("live_orders={live_orders}");
    println!("live_price_levels={live_price_levels}");

    println!();
    println!("elapsed_seconds={seconds:.3}");

    println!("messages_per_second={messages_per_second:.3}");

    println!(
        "million_messages_per_second={:.3}",
        messages_per_second / 1_000_000.0
    );

    println!("book_operations_per_second={operations_per_second:.3}");

    println!(
        "million_book_operations_per_second={:.3}",
        operations_per_second / 1_000_000.0
    );

    // This calculation is meaningful only when the entire file was timed.
    if !sliced_run {
        let mib_per_second = file_size as f64 / (1024.0 * 1024.0) / seconds;

        println!("throughput_MB_per_second={mib_per_second:.3}");
    }

    // Full comparator validation only makes sense for a full-file run.
    if !sliced_run {
        println!();
        println!("========================================");
        println!("PUBLISHED C++ FULL_BOOK");
        println!("========================================");

        println!("source_messages={COMPARATOR_MESSAGES}");
        println!("book_operations={COMPARATOR_OPERATIONS}");
        println!("adds={COMPARATOR_ADDS}");
        println!("executions={COMPARATOR_EXECUTIONS}");
        println!("deletes={COMPARATOR_DELETES}");
        println!("replaces={COMPARATOR_REPLACES}");
        println!("cancels={COMPARATOR_CANCELS}");
        println!("symbols={COMPARATOR_SYMBOLS}");
        println!("live_orders=0");
        println!("elapsed_seconds={COMPARATOR_SECONDS}");

        println!(
            "million_messages_per_second={:.3}",
            COMPARATOR_MESSAGES as f64 / COMPARATOR_SECONDS / 1_000_000.0
        );

        println!();
        println!("========================================");
        println!("VALIDATION");
        println!("========================================");

        macro_rules! compare {
            ($name:literal, $actual:expr, $expected:expr) => {
                if $actual == $expected {
                    println!("{}=MATCH ({})", $name, $actual);
                } else {
                    println!("{}=DIFF lobo={} comparator={}", $name, $actual, $expected,);
                }
            };
        }

        compare!("source_messages", source_messages, COMPARATOR_MESSAGES);

        compare!("book_messages", book_messages, COMPARATOR_OPERATIONS);

        compare!("adds", adds, COMPARATOR_ADDS);

        compare!("executions", executions, COMPARATOR_EXECUTIONS);

        compare!("deletes", deletes, COMPARATOR_DELETES);

        compare!("replaces", replaces, COMPARATOR_REPLACES);

        compare!("cancels", cancels, COMPARATOR_CANCELS);

        compare!("symbols", book_count, COMPARATOR_SYMBOLS);

        if live_orders == 0 {
            println!("live_orders=MATCH (0)");
        } else {
            println!("live_orders=DIFF lobo={live_orders} comparator=0");
        }

        println!();
        println!(
            "lobo_vs_cpp_wall_time={:.3}x",
            COMPARATOR_SECONDS / seconds
        );
    }

    Ok(())
}
