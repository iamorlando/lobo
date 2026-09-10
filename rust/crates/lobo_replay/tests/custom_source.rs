use lobo_books::price_time_priority::Book;
use lobo_events::BookPublisherFactory;
use lobo_models::Side;
use lobo_primitives::{
    Price64,
    time::{DateTime, Utc},
    uuid::Uuid,
};
use lobo_replay::{
    ReplayContext, ReplayStats,
    custom::{AdaptForReplay, orders::AddOrder, source::*},
};
use lobo_storage::{
    DoNotUpdateUserMap, UserMapUpdatePolicy,
    policies::{DoNotUpdateHiddenQuantity, HiddenQuantityPolicy},
    price_level::{IntrusivePriceLevel, PriceLevelContract},
    price_sorting::{PriceSortingPolicy, SortedVectorPriceSorting},
};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

type Level = IntrusivePriceLevel<Price64, DoNotUpdateHiddenQuantity>;
type Context =
    ReplayContext<Level, SortedVectorPriceSorting, DoNotUpdateUserMap, DoNotUpdateHiddenQuantity>;

#[derive(Clone)]
struct Record {
    key: u16,
    time: u64,
    quantity: u64,
}
#[derive(Clone)]
struct Format {
    group: usize,
    records: Arc<Vec<Result<Record, &'static str>>>,
    opens: Arc<AtomicUsize>,
    cutoff: u64,
}
#[derive(Debug, Default, PartialEq)]
struct Stats([u64; 4]);
impl ReplayStats<Record> for Stats {
    fn record_source_message(&mut self, _: &Record) {
        self.0[0] += 1;
    }
    fn record_security_message(&mut self, _: &Record) {
        self.0[1] += 1;
    }
    fn record_replay_message(&mut self, _: &Record) {
        self.0[2] += 1;
    }
    fn record_replay_event(&mut self) {
        self.0[3] += 1;
    }
    fn merge(&mut self, other: Self) {
        for (a, b) in self.0.iter_mut().zip(other.0) {
            *a += b;
        }
    }
}
impl ReplayFormat for Format {
    type Message = Record;
    type Error = &'static str;
    type StreamError = &'static str;
    type Stream = std::vec::IntoIter<Result<Record, &'static str>>;
    type Stats = Stats;
    type Key = u16;
    type Group = usize;
    type Routing = HashRouting<u16>;
    fn group(&self) -> &usize {
        &self.group
    }
    fn routing(&self) -> Self::Routing {
        HashRouting::default()
    }
    fn key(&self, message: &Record) -> u16 {
        message.key
    }
    fn open(&self) -> Result<Self::Stream, Self::Error> {
        self.opens.fetch_add(1, Ordering::Relaxed);
        Ok(self.records.as_ref().clone().into_iter())
    }
    fn includes(&self, message: &Record) -> bool {
        message.quantity > 0
    }
    fn with_cutoff(mut self, time: Option<DateTime<Utc>>) -> Self {
        self.cutoff = time.map_or(u64::MAX, |t| t.timestamp_nanos_opt().unwrap() as u64);
        self
    }
    fn before_cutoff(&self, message: &Record) -> bool {
        message.time <= self.cutoff
    }
}
impl<L, S, U, H, Pub> ApplyReplay<L, S, U, H, Pub> for Format
where
    L: PriceLevelContract<Price = Price64>,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: BookPublisherFactory<Price64>,
{
    fn apply(&self, message: Record, book: &mut Book<L, S, U, H, Pub>) -> bool {
        AddOrder {
            timestamp: message.time,
            id: Uuid::from_u128(message.time as u128),
            side: Side::Buy,
            quantity: message.quantity,
            price: Price64::from(100u64),
        }
        .process(book)
        .is_ok()
    }
}
fn format() -> Format {
    Format {
        group: 1,
        records: Arc::new(vec![
            Ok(Record {
                key: 10,
                time: 1,
                quantity: 10,
            }),
            Ok(Record {
                key: 20,
                time: 2,
                quantity: 20,
            }),
            Ok(Record {
                key: 30,
                time: 3,
                quantity: 30,
            }),
            Ok(Record {
                key: 10,
                time: 4,
                quantity: 0,
            }),
            Ok(Record {
                key: 20,
                time: 5,
                quantity: 50,
            }),
        ]),
        opens: Arc::new(AtomicUsize::new(0)),
        cutoff: u64::MAX,
    }
}
fn sources(format: Format) -> Vec<CustomReplaySource<Format>> {
    CustomReplaySource::from_directory(format, [("ONE".into(), 10), ("TWO".into(), 20)])
}
#[test]
fn shared_stream_is_opened_once_and_cutoff_and_stats_match() {
    let format = format();
    let sources = sources(format.clone());
    for stats in [false, true] {
        let mut context = Context::new(Some(DateTime::from_timestamp_nanos(2)));
        let refs = sources.iter().collect::<Vec<_>>();
        if stats {
            assert_eq!(
                context.replay_from_sources_with_stats(&refs).unwrap(),
                Stats([5, 4, 2, 2])
            );
        } else {
            context.replay_from_sources(&refs).unwrap();
        }
        assert_eq!(
            context
                .get("one")
                .unwrap()
                .order_storage
                .bids
                .visible_quantity,
            10
        );
        assert_eq!(
            context
                .get("two")
                .unwrap()
                .order_storage
                .bids
                .visible_quantity,
            20
        );
    }
    assert_eq!(format.opens.load(Ordering::Relaxed), 2);
}
#[test]
fn duplicate_routes_and_separate_streams_preserve_book_order() {
    let first = format();
    let mut second = first.clone();
    second.group = 2;
    second.records = Arc::new(vec![Ok(Record {
        key: 10,
        time: 100,
        quantity: 7,
    })]);
    let first_sources = sources(first.clone());
    let second_sources = sources(second);
    let mut context = Context::new(None);
    context
        .replay_from_sources(&[
            &first_sources[0],
            &first_sources[0],
            &first_sources[1],
            &second_sources[0],
        ])
        .unwrap();
    assert_eq!(
        context
            .get("one")
            .unwrap()
            .order_storage
            .bids
            .visible_quantity,
        17
    );
    assert_eq!(
        context
            .get("two")
            .unwrap()
            .order_storage
            .bids
            .visible_quantity,
        70
    );
    assert_eq!(first.opens.load(Ordering::Relaxed), 2);
}
#[test]
fn parse_error_restores_partial_books_and_prepared_cutoff_is_cleared() {
    let mut format = format();
    format.records = Arc::new(vec![
        Ok(Record {
            key: 10,
            time: 1,
            quantity: 10,
        }),
        Err("truncated input"),
    ]);
    format.cutoff = 0;
    let sources = sources(format);
    let mut context = Context::new(None);
    assert_eq!(
        context.replay_from_sources(&sources.iter().collect::<Vec<_>>()),
        Err("truncated input")
    );
    assert_eq!(
        context
            .get("one")
            .unwrap()
            .order_storage
            .bids
            .visible_quantity,
        10
    );
    assert!(context.get("two").is_some());
}
#[cfg(feature = "concurrent")]
#[test]
fn scanner_partitions_match_serial_replay() {
    let format = format();
    let sources = sources(format.clone());
    let mut context = Context::new(None);
    context
        .parallel_replay_from_sources(&sources.iter().collect::<Vec<_>>(), 2)
        .unwrap();
    assert_eq!(
        context
            .get("one")
            .unwrap()
            .order_storage
            .bids
            .visible_quantity,
        10
    );
    assert_eq!(
        context
            .get("two")
            .unwrap()
            .order_storage
            .bids
            .visible_quantity,
        70
    );
    assert_eq!(format.opens.load(Ordering::Relaxed), 2);
}
