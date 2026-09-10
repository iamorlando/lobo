use lobo_events::{BookEvent, PriceLevelChangeEvent};
use lobo_primitives::Price64;
use std::collections::{BTreeMap, HashMap, VecDeque};

pub fn compact_levels(
    pending: &mut BTreeMap<(u64, u8), BookEvent<PriceLevelChangeEvent<Price64>>>,
    uploaded: Option<&HashMap<u64, u32>>,
) {
    // Retain clears for slots that were visible before drawing was suspended.
    pending.retain(|(price, _), event| {
        event.event().visible_quantity() > 0
            || uploaded.is_some_and(|prices| prices.contains_key(price))
    });
}

/// Deferred presentation keeps the GPU ring's tail per instrument. The native
/// aggregator still consumes every execution and owns its complete bar count.
pub struct RecentBars<T> {
    books: HashMap<String, VecDeque<T>>,
}

impl<T> Default for RecentBars<T> {
    fn default() -> Self {
        Self {
            books: HashMap::new(),
        }
    }
}

impl<T> RecentBars<T> {
    pub fn push(&mut self, symbol: &str, bar: T) {
        if let Some(bars) = self.books.get_mut(symbol) {
            if bars.len() == 256 {
                bars.pop_front();
            }
            bars.push_back(bar);
        } else {
            self.books.insert(symbol.to_owned(), VecDeque::from([bar]));
        }
    }

    pub fn drain(&mut self) -> impl Iterator<Item = T> + '_ {
        self.books.drain().flat_map(|(_, bars)| bars)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_transient_deleted_prices_but_preserves_gpu_clears_on_both_sides() {
        use lobo_models::Side;
        let mut pending = BTreeMap::new();
        for (price, quantity, side) in [
            (100, 0, Side::Buy),
            (100, 0, Side::Sell),
            (101, 0, Side::Sell),
            (102, 50, Side::Sell),
        ] {
            pending.insert(
                (price, side as u8),
                BookEvent::from_event(
                    PriceLevelChangeEvent::new(
                        quantity,
                        0,
                        price.into(),
                        usize::from(quantity > 0),
                        side,
                    ),
                    1,
                    "book".into(),
                ),
            );
        }
        compact_levels(&mut pending, Some(&HashMap::from([(100, 5)])));
        assert_eq!(
            pending.keys().copied().collect::<Vec<_>>(),
            vec![(100, 0), (100, 1), (102, 1)]
        );
        compact_levels(&mut pending, None);
        assert_eq!(pending.keys().copied().collect::<Vec<_>>(), vec![(102, 1)]);
    }

    #[test]
    fn keeps_each_instruments_ordered_tail_without_starving_quiet_books() {
        let mut bars = RecentBars::default();
        bars.push("quiet", ("quiet", 7));
        for index in 0..10_000 {
            bars.push("busy", ("busy", index));
            if index % 2 == 0 {
                bars.push("other", ("other", index));
            }
            assert!(bars.books.values().all(|tail| tail.len() <= 256));
        }
        let drained = bars.drain().collect::<Vec<_>>();
        assert_eq!(drained.len(), 513);
        assert!(drained.contains(&("quiet", 7)));
        assert_eq!(
            drained
                .iter()
                .filter(|b| b.0 == "busy")
                .map(|b| b.1)
                .collect::<Vec<_>>(),
            (9744..10_000).collect::<Vec<_>>()
        );
        assert_eq!(
            drained
                .iter()
                .filter(|b| b.0 == "other")
                .map(|b| b.1)
                .collect::<Vec<_>>(),
            (9488..10_000).step_by(2).collect::<Vec<_>>()
        );
        assert_eq!(bars.drain().count(), 0);
        bars.push("busy", ("busy", 10_000));
        assert_eq!(bars.drain().collect::<Vec<_>>(), vec![("busy", 10_000)]);
    }
}
