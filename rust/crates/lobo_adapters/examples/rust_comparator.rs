// cargo test -p lobo_adapters --features itchy --test cpp_comparator --release -- --nocapture
use std::{env, error::Error, fs, hint::black_box, path::PathBuf, time::Instant};

use lobo_adapters::itch::ItchReplaySource;
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

// Relative to this crate's manifest directory.
const DEFAULT_PATH: &str = "../../../data/NASDAQ/01302020.NASDAQ_ITCH50";

const PATH_ENV: &str = "LOBO_ITCH_PATH";

// Published comparator numbers:
// suhasghorp/nasdaq-itch-orderbook
const COMPARATOR_MESSAGES: u64 = 423_285_708;
const COMPARATOR_UPDATES: u64 = 1_993_352;
const COMPARATOR_SECONDS: f64 = 21.77;

fn main() -> Result<(), Box<dyn Error>> {
    let path = env::var_os(PATH_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_PATH));

    let file_size = fs::metadata(&path)?.len();

    let source = ItchReplaySource::from_file(path.clone(), "AAPL")?;

    let mut context = ItchReplayContext::new(None);

    // Same basic timed region we care about:
    //
    // full ITCH scan
    // -> parse
    // -> AAPL filtering
    // -> adaptation
    // -> book updates
    let started = Instant::now();

    let stats = context.replay_from(&source)?;

    let elapsed = started.elapsed();

    black_box(&context);

    let seconds = elapsed.as_secs_f64();

    let messages_per_second = stats.source_messages as f64 / seconds;

    let million_messages_per_second = messages_per_second / 1_000_000.0;

    // Their implementation calls this MB/s but calculates using 1024^2,
    // so reproduce that calculation.
    let mib_per_second = file_size as f64 / (1024.0 * 1024.0) / seconds;

    println!();
    println!("========================================");
    println!("RUST AAPL COMPARATOR");
    println!("========================================");

    println!("file={}", path.display());
    println!("symbol=AAPL");
    println!("file_bytes={file_size}");

    println!();
    println!("Processed {} messages", stats.source_messages);
    println!("Applied {} orderbook updates", stats.replay_events);

    println!("Processing completed in {:.3}s", seconds);

    println!("Throughput: {:.2} MB/s", mib_per_second);

    println!("Messages/sec: {:.3}", messages_per_second);

    println!("Million messages/sec: {:.3}", million_messages_per_second);

    println!();
    println!("AAPL:");
    println!("security_messages={}", stats.security_messages);
    println!("replay_messages={}", stats.replay_messages);
    println!("replay_events={}", stats.replay_events);

    println!();
    println!("final book:");
    let book = context
        .get("AAPL")
        .ok_or("AAPL book was not created during replay")?;
    println!(
        "bids: orders={} levels={} visible={}",
        book.order_storage.bids.len(),
        book.order_storage.bids.price_level_count(),
        book.order_storage.bids.visible_quantity,
    );

    println!(
        "asks: orders={} levels={} visible={}",
        book.order_storage.asks.len(),
        book.order_storage.asks.price_level_count(),
        book.order_storage.asks.visible_quantity,
    );

    println!();
    println!("========================================");
    println!("PUBLISHED RUST RESULT");
    println!("========================================");

    println!("messages={COMPARATOR_MESSAGES}");
    println!("orderbook_updates={COMPARATOR_UPDATES}");
    println!("seconds={COMPARATOR_SECONDS:.2}");
    println!(
        "million_messages_per_second={:.3}",
        COMPARATOR_MESSAGES as f64 / COMPARATOR_SECONDS / 1_000_000.0
    );

    println!();
    println!("========================================");
    println!("RELATIVE");
    println!("========================================");

    println!(
        "lobo_vs_published_time={:.3}x",
        COMPARATOR_SECONDS / seconds
    );

    if stats.source_messages == COMPARATOR_MESSAGES {
        println!("source_message_count=MATCH");
    } else {
        println!(
            "source_message_count=DIFF lobo={} comparator={}",
            stats.source_messages, COMPARATOR_MESSAGES
        );
    }

    if stats.replay_events == COMPARATOR_UPDATES {
        println!("aapl_update_count=MATCH");
    } else {
        println!(
            "aapl_update_count=DIFF lobo={} comparator={}",
            stats.replay_events, COMPARATOR_UPDATES
        );
    }

    Ok(())
}
