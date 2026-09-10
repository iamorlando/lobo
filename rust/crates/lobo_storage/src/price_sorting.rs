use std::{
    collections::BTreeMap,
    ops::Bound::{Included, Unbounded},
};

use crate::sorted_vector::SortedVector;

/// Shared ordered-map operations required by a sided order store.
///
/// Implementations are selected statically through [`PriceSortingPolicy`], so
/// the matching hot path remains monomorphized for either backend.
pub trait PriceLevelMap<K: Ord, V>: Default {
    /// Visit entries in key order, removing those rejected by the predicate.
    fn retain(&mut self, keep: impl FnMut(&K, &mut V) -> bool);
    fn contains_key(&self, key: &K) -> bool;
    fn get(&self, key: &K) -> Option<&V>;
    fn get_mut(&mut self, key: &K) -> Option<&mut V>;
    fn get_or_insert_with(&mut self, key: K, create: impl FnOnce() -> V) -> &mut V;

    fn iter<'a>(&'a self) -> impl DoubleEndedIterator<Item = (&'a K, &'a V)>
    where
        K: 'a,
        V: 'a;
    fn iter_mut<'a>(&'a mut self) -> impl DoubleEndedIterator<Item = (&'a K, &'a mut V)>
    where
        K: 'a,
        V: 'a;
    fn values<'a>(&'a self) -> impl DoubleEndedIterator<Item = &'a V>
    where
        V: 'a;
    fn values_mut<'a>(&'a mut self) -> impl DoubleEndedIterator<Item = &'a mut V>
    where
        V: 'a;

    fn range<'a>(
        &'a self,
        bounds: std::ops::RangeInclusive<K>,
    ) -> impl Iterator<Item = (&'a K, &'a V)>
    where
        K: 'a,
        V: 'a;

    /// Iterate over keys up to and including `upper_bound`. `None` selects all
    /// entries. Keys are already side-adjusted, so ascending map order is
    /// matching priority order for both bids and asks.
    fn prefix<'a>(&'a self, upper_bound: Option<K>) -> impl Iterator<Item = (&'a K, &'a V)>
    where
        K: 'a,
        V: 'a;
    fn prefix_mut<'a>(
        &'a mut self,
        upper_bound: Option<K>,
    ) -> impl Iterator<Item = (&'a K, &'a mut V)>
    where
        K: 'a,
        V: 'a;
}

impl<K: Ord, V> PriceLevelMap<K, V> for SortedVector<K, V> {
    fn range<'a>(
        &'a self,
        bounds: std::ops::RangeInclusive<K>,
    ) -> impl Iterator<Item = (&'a K, &'a V)>
    where
        K: 'a,
        V: 'a,
    {
        SortedVector::range(self, bounds)
    }
    fn retain(&mut self, keep: impl FnMut(&K, &mut V) -> bool) {
        SortedVector::retain(self, keep);
    }

    #[inline(always)]
    fn contains_key(&self, key: &K) -> bool {
        SortedVector::contains_key(self, key)
    }

    #[inline(always)]
    fn get(&self, key: &K) -> Option<&V> {
        SortedVector::get(self, key)
    }

    #[inline(always)]
    fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        SortedVector::get_mut(self, key)
    }

    #[inline(always)]
    fn get_or_insert_with(&mut self, key: K, create: impl FnOnce() -> V) -> &mut V {
        SortedVector::get_or_insert_with(self, key, create)
    }

    #[inline(always)]
    fn iter<'a>(&'a self) -> impl DoubleEndedIterator<Item = (&'a K, &'a V)>
    where
        K: 'a,
        V: 'a,
    {
        SortedVector::iter(self)
    }

    #[inline(always)]
    fn iter_mut<'a>(&'a mut self) -> impl DoubleEndedIterator<Item = (&'a K, &'a mut V)>
    where
        K: 'a,
        V: 'a,
    {
        SortedVector::iter_mut(self)
    }

    #[inline(always)]
    fn values<'a>(&'a self) -> impl DoubleEndedIterator<Item = &'a V>
    where
        V: 'a,
    {
        SortedVector::values(self)
    }

    #[inline(always)]
    fn values_mut<'a>(&'a mut self) -> impl DoubleEndedIterator<Item = &'a mut V>
    where
        V: 'a,
    {
        SortedVector::values_mut(self)
    }

    #[inline(always)]
    fn prefix<'a>(&'a self, upper_bound: Option<K>) -> impl Iterator<Item = (&'a K, &'a V)>
    where
        K: 'a,
        V: 'a,
    {
        let end = self.inclusive_end(upper_bound.as_ref());
        SortedVector::prefix(self, end)
    }

    #[inline(always)]
    fn prefix_mut<'a>(
        &'a mut self,
        upper_bound: Option<K>,
    ) -> impl Iterator<Item = (&'a K, &'a mut V)>
    where
        K: 'a,
        V: 'a,
    {
        let end = self.inclusive_end(upper_bound.as_ref());
        SortedVector::prefix_mut(self, end)
    }
}

impl<K: Ord, V> PriceLevelMap<K, V> for BTreeMap<K, V> {
    fn range<'a>(
        &'a self,
        bounds: std::ops::RangeInclusive<K>,
    ) -> impl Iterator<Item = (&'a K, &'a V)>
    where
        K: 'a,
        V: 'a,
    {
        BTreeMap::range(self, bounds)
    }
    fn retain(&mut self, keep: impl FnMut(&K, &mut V) -> bool) {
        BTreeMap::retain(self, keep);
    }

    #[inline(always)]
    fn contains_key(&self, key: &K) -> bool {
        BTreeMap::contains_key(self, key)
    }

    #[inline(always)]
    fn get(&self, key: &K) -> Option<&V> {
        BTreeMap::get(self, key)
    }

    #[inline(always)]
    fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        BTreeMap::get_mut(self, key)
    }

    #[inline(always)]
    fn get_or_insert_with(&mut self, key: K, create: impl FnOnce() -> V) -> &mut V {
        self.entry(key).or_insert_with(create)
    }

    #[inline(always)]
    fn iter<'a>(&'a self) -> impl DoubleEndedIterator<Item = (&'a K, &'a V)>
    where
        K: 'a,
        V: 'a,
    {
        BTreeMap::iter(self)
    }

    #[inline(always)]
    fn iter_mut<'a>(&'a mut self) -> impl DoubleEndedIterator<Item = (&'a K, &'a mut V)>
    where
        K: 'a,
        V: 'a,
    {
        BTreeMap::iter_mut(self)
    }

    #[inline(always)]
    fn values<'a>(&'a self) -> impl DoubleEndedIterator<Item = &'a V>
    where
        V: 'a,
    {
        BTreeMap::values(self)
    }

    #[inline(always)]
    fn values_mut<'a>(&'a mut self) -> impl DoubleEndedIterator<Item = &'a mut V>
    where
        V: 'a,
    {
        BTreeMap::values_mut(self)
    }

    #[inline(always)]
    fn prefix<'a>(&'a self, upper_bound: Option<K>) -> impl Iterator<Item = (&'a K, &'a V)>
    where
        K: 'a,
        V: 'a,
    {
        self.range((Unbounded, upper_bound.map_or(Unbounded, Included)))
    }

    #[inline(always)]
    fn prefix_mut<'a>(
        &'a mut self,
        upper_bound: Option<K>,
    ) -> impl Iterator<Item = (&'a K, &'a mut V)>
    where
        K: 'a,
        V: 'a,
    {
        self.range_mut((Unbounded, upper_bound.map_or(Unbounded, Included)))
    }
}

/// Chooses the ordered-map implementation used to store price levels.
pub trait PriceSortingPolicy: Default {
    type PriceLevels<K: Ord, V>: PriceLevelMap<K, V>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SortedVectorPriceSorting;

impl PriceSortingPolicy for SortedVectorPriceSorting {
    type PriceLevels<K: Ord, V> = SortedVector<K, V>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BTreeMapPriceSorting;

impl PriceSortingPolicy for BTreeMapPriceSorting {
    type PriceLevels<K: Ord, V> = BTreeMap<K, V>;
}

#[cfg(test)]
mod tests {
    use super::{
        BTreeMapPriceSorting, PriceLevelMap, PriceSortingPolicy, SortedVectorPriceSorting,
    };

    fn assert_shared_api<P: PriceSortingPolicy>() {
        let mut levels = P::PriceLevels::<u32, &'static str>::default();
        levels.get_or_insert_with(20, || "twenty");
        levels.get_or_insert_with(10, || "ten");
        levels.get_or_insert_with(30, || "thirty");
        levels.get_or_insert_with(20, || panic!("existing key recreated"));

        assert!(levels.contains_key(&20));
        assert_eq!(levels.get(&10), Some(&"ten"));
        assert_eq!(
            levels
                .prefix(Some(20))
                .map(|(key, _)| *key)
                .collect::<Vec<_>>(),
            [10, 20]
        );

        *levels.get_mut(&20).unwrap() = "TWENTY";
        for (_, value) in levels.prefix_mut(Some(10)) {
            *value = "TEN";
        }

        assert_eq!(
            levels
                .iter()
                .map(|(key, value)| (*key, *value))
                .collect::<Vec<_>>(),
            [(10, "TEN"), (20, "TWENTY"), (30, "thirty")]
        );
    }

    #[test]
    fn sorted_vector_implements_shared_price_level_api() {
        assert_shared_api::<SortedVectorPriceSorting>();
    }

    #[test]
    fn btree_map_implements_shared_price_level_api() {
        assert_shared_api::<BTreeMapPriceSorting>();
    }
}
