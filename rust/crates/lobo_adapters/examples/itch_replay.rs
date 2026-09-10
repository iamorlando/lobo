use std::{env, error::Error, path::PathBuf};

use itchy::{Body, MessageStream};

use lobo_adapters::itch::messages::adapt_message;
use lobo_books::price_time_priority::Book;
use lobo_primitives::CompressedPrice;
use lobo_storage::{
    DoNotUpdateUserMap,
    policies::DoNotUpdateHiddenQuantity,
    price_level::{HasHiddenQuantity, IntrusivePriceLevel, PriceLevelContract},
    price_sorting::SortedVectorPriceSorting,
};

type ItchBook = Book<
    IntrusivePriceLevel<CompressedPrice, DoNotUpdateHiddenQuantity>,
    SortedVectorPriceSorting,
    DoNotUpdateUserMap,
    DoNotUpdateHiddenQuantity,
>;

// Relative to this crate's manifest directory.
const DEFAULT_PATH: &str = "../../../data/NASDAQ/01302020.NASDAQ_ITCH50";

const PATH_ENV: &str = "LOBO_ITCH_PATH";
const PRINT_ENV: &str = "LOBO_REPLAY_PRINT";
const MAX_MESSAGES_ENV: &str = "LOBO_REPLAY_MAX_MESSAGES";

fn itch_path() -> PathBuf {
    env::var_os(PATH_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(DEFAULT_PATH))
}

fn print_events_enabled() -> bool {
    matches!(
        env::var(PRINT_ENV).as_deref(),
        Ok("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

/// None = replay the entire file.
fn max_messages() -> Option<u64> {
    env::var(MAX_MESSAGES_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
}

fn find_stock_locate(path: &PathBuf, ticker: &str) -> Result<u16, Box<dyn Error>> {
    let stream = MessageStream::from_file(path)?;

    for message in stream {
        let message = message?;

        if let Body::StockDirectory(directory) = &message.body
            && directory.stock.as_str().trim() == ticker
        {
            return Ok(message.stock_locate);
        }
    }

    Err(format!("ticker {ticker} not found").into())
}

fn main() -> Result<(), Box<dyn Error>> {
    let path = itch_path();
    let print_events = print_events_enabled();
    let max_messages = max_messages();

    let stock_locate = find_stock_locate(&path, "AAPL")?;

    println!("file={}", path.display());
    println!("ticker=AAPL");

    match max_messages {
        Some(max) => println!("message_limit={max}"),
        None => println!("message_limit=ALL"),
    }

    println!();

    let stream = MessageStream::from_file(&path)?;
    let mut book = ItchBook::default();

    let mut source_messages_scanned = 0_u64;
    let mut aapl_messages_seen = 0_u64;
    let mut replay_messages = 0_u64;
    let mut replay_events = 0_u64;

    for message in stream {
        let message = message?;
        source_messages_scanned += 1;

        if message.stock_locate != stock_locate {
            continue;
        }

        aapl_messages_seen += 1;

        let event_description = print_events.then(|| format!("{:?}", &message.body));

        // The ITCH adapter now applies the update directly to the replay book.
        let Some(result) = adapt_message(message, &mut book) else {
            continue;
        };

        replay_messages += 1;
        result.map_err(|error| format!("ITCH replay failed: {error:?}"))?;
        replay_events += 1;

        if let Some(description) = event_description {
            println!("#{replay_events} {description}");
        }

        // Test-only truncation.
        if max_messages.is_some_and(|max| replay_messages >= max) {
            break;
        }
    }

    assert!(replay_events > 0, "no AAPL replay events were consumed");

    println!();
    println!("==============================");
    println!("REPLAY COMPLETE");
    println!("==============================");
    println!("source_messages_scanned={source_messages_scanned}");
    println!("aapl_messages_seen={aapl_messages_seen}");
    println!("replay_messages={replay_messages}");
    println!("replay_events={replay_events}");

    println!();
    println!("==============================");
    println!("CURRENT BOOK");
    println!("==============================");

    println!(
        "bids: orders={} levels={} visible={} hidden={}",
        book.order_storage.bids.len(),
        book.order_storage.bids.price_level_count(),
        book.order_storage.bids.visible_quantity,
        book.order_storage.bids.hidden_quantity,
    );

    println!(
        "asks: orders={} levels={} visible={} hidden={}",
        book.order_storage.asks.len(),
        book.order_storage.asks.price_level_count(),
        book.order_storage.asks.visible_quantity,
        book.order_storage.asks.hidden_quantity,
    );

    println!();
    println!("BEST BID:");

    match book.order_storage.bids.best() {
        Some((price, level)) => {
            println!(
                "price={price:?} quantity={} hidden={} orders={}",
                level.visible_quantity(),
                level.hidden_quantity(),
                level.len(),
            );
        }

        None => println!("none"),
    }

    println!("BEST ASK:");

    match book.order_storage.asks.best() {
        Some((price, level)) => {
            println!(
                "price={price:?} quantity={} hidden={} orders={}",
                level.visible_quantity(),
                level.hidden_quantity(),
                level.len(),
            );
        }

        None => println!("none"),
    }

    println!();
    println!("TOP 5 BIDS:");

    for (i, (price, level)) in book.order_storage.bids.price_levels().take(5).enumerate() {
        println!(
            "{:>2}: price={price:?} quantity={} hidden={} orders={}",
            i + 1,
            level.visible_quantity(),
            level.hidden_quantity(),
            level.len(),
        );
    }

    println!();
    println!("TOP 5 ASKS:");

    for (i, (price, level)) in book.order_storage.asks.price_levels().take(5).enumerate() {
        println!(
            "{:>2}: price={price:?} quantity={} hidden={} orders={}",
            i + 1,
            level.visible_quantity(),
            level.hidden_quantity(),
            level.len(),
        );
    }

    Ok(())
}
