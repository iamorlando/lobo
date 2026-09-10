//! Replay sources with shared stream grouping and instrument routing.

use std::{collections::HashMap, hash::Hash};

use lobo_books::price_time_priority::Book;
use lobo_events::{BookPublisherFactory, NullPublisher};
use lobo_primitives::time::{DateTime, Utc};
use lobo_storage::{
    UserMapUpdatePolicy, policies::HiddenQuantityPolicy, price_level::PriceLevelContract,
    price_sorting::PriceSortingPolicy,
};

use crate::{ReplaySource, ReplayStats};

/// Instrument routing selected when a format is constructed. Dense numeric
/// identifiers and arbitrary keys need not pay for the same lookup strategy.
pub trait Routing<K> {
    fn insert(&mut self, key: K, index: usize) -> Option<usize>;
    fn get(&self, key: &K) -> Option<usize>;
}

pub struct HashRouting<K>(HashMap<K, usize>);
impl<K> Default for HashRouting<K> {
    fn default() -> Self {
        Self(HashMap::new())
    }
}
impl<K: Eq + Hash> Routing<K> for HashRouting<K> {
    fn insert(&mut self, key: K, index: usize) -> Option<usize> {
        self.0.insert(key, index)
    }
    #[inline(always)]
    fn get(&self, key: &K) -> Option<usize> {
        self.0.get(key).copied()
    }
}

/// A bounded numeric directory. Formats validate their key range while
/// decoding, so the route itself is one indexed load.
pub struct DenseRouting(Vec<Option<usize>>);
impl DenseRouting {
    pub fn new(capacity: usize) -> Self {
        Self(vec![None; capacity])
    }
}
impl<K: Copy + Into<usize>> Routing<K> for DenseRouting {
    fn insert(&mut self, key: K, index: usize) -> Option<usize> {
        self.0[key.into()].replace(index)
    }
    #[inline(always)]
    fn get(&self, key: &K) -> Option<usize> {
        self.0[(*key).into()]
    }
}

/// Wire decoding, transport and filtering for a replay.
///
/// The iterator may use a file, decompressor, socket, or an existing parser. `group` identifies streams that can be scanned together;
/// it must include any configuration that changes decoding or stream contents.
/// Neither this contract nor the source knows which venue implements it.
pub trait ReplayFormat: Clone {
    type Message;
    type Error;
    type StreamError: Into<Self::Error>;
    type Stream: Iterator<Item = Result<Self::Message, Self::StreamError>>;
    type Stats: ReplayStats<Self::Message>;
    type Key: Clone + Eq;
    type Group: Clone + Eq + Hash;
    type Routing: Routing<Self::Key>;

    fn open(&self) -> Result<Self::Stream, Self::Error>;
    fn group(&self) -> &Self::Group;
    fn routing(&self) -> Self::Routing;
    fn key(&self, message: &Self::Message) -> Self::Key;

    #[inline(always)]
    fn includes(&self, _message: &Self::Message) -> bool {
        true
    }
    fn with_cutoff(self, _cutoff: Option<DateTime<Utc>>) -> Self {
        self
    }
    #[inline(always)]
    fn before_cutoff(&self, _message: &Self::Message) -> bool {
        true
    }
}

/// Apply a decoded message directly to the existing book. Publishers,
/// price storage and book policies remain compile-time parameters.
pub trait ApplyReplay<L, S, U, H, Pub = NullPublisher>: ReplayFormat
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
{
    fn apply(&self, message: Self::Message, book: &mut Book<L, S, U, H, Pub>) -> bool;
}

/// A resolved instrument in a source. Build these from a directory
/// iterator once, then pass them to the ordinary `ReplayContext`.
#[derive(Clone)]
pub struct CustomReplaySource<D: ReplayFormat> {
    pub format: D,
    pub key: D::Key,
    ticker: String,
}
impl<D: ReplayFormat> CustomReplaySource<D> {
    pub fn new(format: D, key: D::Key, ticker: impl Into<String>) -> Self {
        Self {
            format,
            key,
            ticker: ticker.into(),
        }
    }
    pub fn from_directory(
        format: D,
        instruments: impl IntoIterator<Item = (String, D::Key)>,
    ) -> Vec<Self> {
        instruments
            .into_iter()
            .map(|(ticker, key)| Self::new(format.clone(), key, ticker))
            .collect()
    }
    pub fn ticker(&self) -> &str {
        &self.ticker
    }

    fn replay_grouped<L, S, U, H, Pub, const STATS: bool>(
        sources: &[&Self],
        books: &mut HashMap<String, Book<L, S, U, H, Pub>>,
    ) -> Result<D::Stats, D::Error>
    where
        L: PriceLevelContract,
        S: PriceSortingPolicy,
        U: UserMapUpdatePolicy,
        H: HiddenQuantityPolicy,
        Pub: BookPublisherFactory<L::Price>,
        D: ApplyReplay<L, S, U, H, Pub>,
    {
        let mut group_indices = HashMap::new();
        let mut groups: Vec<Vec<&Self>> = Vec::new();
        for source in sources.iter().copied() {
            let group = *group_indices
                .entry(source.format.group().clone())
                .or_insert_with(|| {
                    groups.push(Vec::new());
                    groups.len() - 1
                });
            groups[group].push(source);
        }
        let mut stats = D::Stats::default();
        for group in groups {
            let mut dispatch = group[0].format.routing();
            let mut routed = Vec::with_capacity(group.len());
            for source in group {
                // Duplicate directories preserve the first source, as the
                // existing replay contract does. This is preparation, not IO.
                if dispatch.get(&source.key).is_some() {
                    continue;
                }
                dispatch.insert(source.key.clone(), routed.len());
                let ticker = source.ticker.trim().to_ascii_lowercase();
                let book = books.remove(&ticker).unwrap_or_default();
                routed.push((source, ticker, book));
            }
            let result = (|| {
                for message in routed[0].0.format.open()? {
                    let message = message.map_err(Into::into)?;
                    if STATS {
                        stats.record_source_message(&message);
                    }
                    let Some(index) = dispatch.get(&routed[0].0.format.key(&message)) else {
                        continue;
                    };
                    let (source, _, book) = &mut routed[index];
                    if STATS {
                        stats.record_security_message(&message);
                    }
                    if !source.format.includes(&message) || !source.format.before_cutoff(&message) {
                        continue;
                    }
                    if STATS {
                        stats.record_replay_message(&message);
                    }
                    let applied = source.format.apply(message, book);
                    if STATS && applied {
                        stats.record_replay_event();
                    }
                }
                Ok(())
            })();
            // Restore partial books on parser errors as well as success.
            books.extend(routed.into_iter().map(|(_, ticker, book)| (ticker, book)));
            result?;
        }
        Ok(stats)
    }
}

impl<D, L, S, U, H, Pub> ReplaySource<L, S, U, H, Pub> for CustomReplaySource<D>
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: BookPublisherFactory<L::Price>,
    D: ApplyReplay<L, S, U, H, Pub>,
{
    type Error = D::Error;
    type Message = D::Message;
    type StreamError = D::StreamError;
    type Stream = D::Stream;
    type Stats = D::Stats;
    type DefaultBookConcreteType = Book<L, S, U, H, Pub>;

    fn with_cutoff(mut self, cutoff: Option<DateTime<Utc>>) -> Self {
        self.format = self.format.with_cutoff(cutoff);
        self
    }
    #[inline(always)]
    fn is_at_or_before_cutoff(&self, message: &Self::Message) -> bool {
        self.format.before_cutoff(message)
    }
    fn ticker(&self) -> &str {
        &self.ticker
    }
    fn new_book(&self) -> Self::DefaultBookConcreteType {
        Book::new()
    }
    fn get_stream(&self) -> Result<Self::Stream, Self::Error> {
        self.format.open()
    }
    #[inline(always)]
    fn include_message(&self, message: &Self::Message) -> bool {
        self.format.includes(message)
    }
    #[inline(always)]
    fn is_relevant(&self, message: &Self::Message) -> bool {
        self.key == self.format.key(message)
    }
    #[inline(always)]
    fn adapt_message(
        &self,
        message: Self::Message,
        book: &mut Self::DefaultBookConcreteType,
    ) -> bool {
        self.format.apply(message, book)
    }
    fn for_each_source(
        sources: &[&Self],
        books: &mut HashMap<String, Self::DefaultBookConcreteType>,
    ) -> Result<(), Self::Error> {
        Self::replay_grouped::<L, S, U, H, Pub, false>(sources, books).map(|_| ())
    }
    fn for_each_source_with_stats(
        sources: &[&Self],
        books: &mut HashMap<String, Self::DefaultBookConcreteType>,
    ) -> Result<Self::Stats, Self::Error> {
        Self::replay_grouped::<L, S, U, H, Pub, true>(sources, books)
    }
}

#[cfg(feature = "polars")]
impl<D: ReplayFormat + crate::ReplayRows<D::Message>> crate::ReplayRows<D::Message>
    for CustomReplaySource<D>
{
    type RowState = D::RowState;
    fn row_schema(&self) -> polars::prelude::Schema {
        self.format.row_schema()
    }
    fn row_data(
        &self,
        message: &D::Message,
        state: &mut Self::RowState,
    ) -> polars::frame::row::Row<'static> {
        self.format.row_data(message, state)
    }
}
