#![cfg(all(feature = "json", feature = "native"))]
#[path = "../../lobo_replay/tests/support/definitions.rs"]
mod definitions;
mod itch_file {
    use lobo_replay::custom::{definition::binary::FileFormat, source::CustomReplaySource};
    pub struct ItchFile(std::path::PathBuf);
    impl ItchFile {
        pub fn new(path: impl AsRef<std::path::Path>) -> Self {
            Self(path.as_ref().into())
        }
        pub fn all(&self) -> Result<Vec<CustomReplaySource<FileFormat>>, String> {
            let definition = super::definitions::definition("itch");
            let format = FileFormat::new(self.0.clone(), &definition, "America/New_York")?;
            let directory =
                format.directory(definition, super::definitions::descriptor("itch", "AAPL"))?;
            Ok(CustomReplaySource::from_directory(
                format,
                directory.into_iter().map(|e| (e.instrument.symbol, e.key)),
            ))
        }
    }
}

use lobo_adapters::itch::ItchReplaySource;
use lobo_models::Side;
use lobo_primitives::CompressedPrice;
use lobo_replay::ReplayContext;
use lobo_storage::{
    DoNotUpdateUserMap, policies::DoNotUpdateHiddenQuantity, price_level::IntrusivePriceLevel,
    price_sorting::SortedVectorPriceSorting,
};

type Level = IntrusivePriceLevel<CompressedPrice, DoNotUpdateHiddenQuantity>;
type Context =
    ReplayContext<Level, SortedVectorPriceSorting, DoNotUpdateUserMap, DoNotUpdateHiddenQuantity>;

#[path = "../../lobo_replay/tests/support/itch.rs"]
mod fixture;
use fixture::file;
#[test]
fn file_directory_statistics_and_native_orders_match_reference() {
    let file = file();
    let original = ItchReplaySource::from_file_all(&file.0).unwrap();
    let custom = itch_file::ItchFile::new(&file.0).all().unwrap();
    assert_eq!(
        original.iter().map(|s| s.ticker()).collect::<Vec<_>>(),
        custom.iter().map(|s| s.ticker()).collect::<Vec<_>>()
    );
    let mut old = Context::new(None);
    let mut new = Context::new(None);
    let a = old
        .replay_from_sources_with_stats(&original.iter().collect::<Vec<_>>())
        .unwrap();
    let b = new
        .replay_from_sources_with_stats(&custom.iter().collect::<Vec<_>>())
        .unwrap();
    assert_eq!(
        (
            a.source_messages,
            a.security_messages,
            a.replay_messages,
            a.replay_events,
            a.adds,
            a.executes,
            a.cancels,
            a.deletes,
            a.replaces
        ),
        (
            b.source_messages,
            b.security_messages,
            b.replay_messages,
            b.replay_events,
            b.adds,
            b.executes,
            b.cancels,
            b.deletes,
            b.replaces
        )
    );
    for (symbol, book) in &old.books {
        let other = &new.books[symbol];
        assert_eq!(
            book.order_storage.bids.visible_quantity,
            other.order_storage.bids.visible_quantity
        );
        for side in [Side::Buy, Side::Sell] {
            let a = book.order_storage.queue_view(
                side,
                CompressedPrice::from(0u32)..=CompressedPrice::from(u32::MAX),
            );
            let b = other.order_storage.queue_view(
                side,
                CompressedPrice::from(0u32)..=CompressedPrice::from(u32::MAX),
            );
            assert_eq!(format!("{a:?}"), format!("{b:?}"));
        }
    }
}

/// Run explicitly with --release --ignored; this reads the complete replay twice.
#[cfg(feature = "concurrent")]
#[test]
#[ignore = "requires the full NASDAQ replay file"]
fn full_file_directory_and_every_fifo_order_match() {
    use std::{collections::BTreeMap, hash::{Hash, Hasher}, time::Instant};
    fn fingerprints(context: &Context) -> BTreeMap<String, (usize, u64)> {
        context.books.iter().map(|(symbol, book)| {
            let mut hash = std::hash::DefaultHasher::new();
            let mut count = 0;
            for side in [Side::Buy, Side::Sell] {
                for order in book.order_storage.queue_view(side, CompressedPrice::from(0u32)..=CompressedPrice::from(u32::MAX)) {
                    (side as u8, order.id, u64::from(order.price), order.quantity, order.created_at).hash(&mut hash);
                    count += 1;
                }
            }
            (symbol.clone(), (count, hash.finish()))
        }).collect()
    }
    let path = std::env::var_os("LOBO_ITCH_PATH").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../data/NASDAQ/01302020.NASDAQ_ITCH50"));
    let reference = ItchReplaySource::from_file_all(&path).unwrap();
    let declarations = itch_file::ItchFile::new(&path).all().unwrap();
    assert_eq!(reference.iter().map(|s|s.ticker()).collect::<Vec<_>>(), declarations.iter().map(|s|s.ticker()).collect::<Vec<_>>());
    // A midday cutoff keeps outstanding orders, while both scanners still read
    // the complete file (timestamps need not be monotonic across instruments).
    let cutoff = "2020-01-30T17:00:00Z".parse().unwrap();
    let mut original = Context::new(Some(cutoff));
    let started = Instant::now();
    original.parallel_replay_from_sources(&reference.iter().collect::<Vec<_>>(), 12).unwrap();
    println!("Existing replay: {:.3}s", started.elapsed().as_secs_f64());
    let expected = fingerprints(&original);
    drop(original);
    let mut custom = Context::new(Some(cutoff));
    let started = Instant::now();
    custom.parallel_replay_from_sources(&declarations.iter().collect::<Vec<_>>(), 12).unwrap();
    println!("Python-declared replay: {:.3}s", started.elapsed().as_secs_f64());
    assert!(expected.values().map(|(count,_)|count).sum::<usize>() > 100_000);
    assert_eq!(expected, fingerprints(&custom));
    println!("Matched all {} books and {} remaining orders, including FIFO position and timestamp", expected.len(), expected.values().map(|(count,_)|count).sum::<usize>());
}
#[cfg(feature = "test-polars")]
#[test]
fn tabular_conversion_preserves_declared_fields_and_wire_bytes() {
    let file = file();
    let sources = itch_file::ItchFile::new(&file.0).all().unwrap();
    let table = sources[0]
        .format
        .table(&[sources[0].key].into())
        .unwrap()
        .collect()
        .unwrap();
    assert_eq!(table.height(), 8);
    assert_eq!(
        table.column("record_type").unwrap().u64().unwrap().get(1),
        Some(u64::from(b'A'))
    );
    assert_eq!(table.column("id").unwrap().u64().unwrap().get(1), Some(1));
    assert_eq!(
        table.column("quantity").unwrap().u64().unwrap().get(1),
        Some(100)
    );
    assert_eq!(
        table.column("price").unwrap().u64().unwrap().get(1),
        Some(10000)
    );
    assert_eq!(
        table
            .column("record")
            .unwrap()
            .binary()
            .unwrap()
            .get(1)
            .unwrap()
            .len(),
        36
    );
}
