/// A compact, unique-key map backed by a sorted vector.
///
/// Entries are always stored in ascending key order. Side-specific order-book
/// priority is expressed by the key type: asks use `Price`, while bids use
/// `Reverse<Price>`.
#[derive(Clone, Debug)]
pub struct SortedVector<K, V> {
    entries: Vec<(K, V)>,
}

impl<K, V> Default for SortedVector<K, V> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl<K: Ord, V> SortedVector<K, V> {
    #[inline(always)]
    fn search(&self, key: &K) -> Result<usize, usize> {
        self.entries
            .binary_search_by(|(candidate, _)| candidate.cmp(key))
    }

    #[inline(always)]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        self.entries.reserve(additional);
    }

    #[inline(always)]
    pub fn contains_key(&self, key: &K) -> bool {
        self.search(key).is_ok()
    }

    #[inline(always)]
    pub fn get(&self, key: &K) -> Option<&V> {
        let index = self.search(key).ok()?;
        Some(&self.entries[index].1)
    }

    #[inline(always)]
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        let index = self.search(key).ok()?;
        Some(&mut self.entries[index].1)
    }

    #[inline]
    pub fn get_or_insert_with(&mut self, key: K, create: impl FnOnce() -> V) -> &mut V {
        let index = match self.search(&key) {
            Ok(index) => index,
            Err(index) => {
                self.entries.insert(index, (key, create()));
                index
            }
        };

        &mut self.entries[index].1
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&K, &mut V) -> bool) {
        self.entries.retain_mut(|(key, value)| keep(key, value));
    }

    /// Returns the exclusive end index for all keys less than or equal to
    /// `key`. `None` selects the complete vector.
    #[inline(always)]
    pub fn inclusive_end(&self, key: Option<&K>) -> usize {
        key.map_or(self.entries.len(), |key| match self.search(key) {
            Ok(index) => index + 1,
            Err(index) => index,
        })
    }

    #[inline(always)]
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&K, &V)> {
        self.entries.iter().map(|(key, value)| (key, value))
    }

    pub fn range(&self, bounds: std::ops::RangeInclusive<K>) -> impl Iterator<Item = (&K, &V)> {
        let start = self
            .entries
            .partition_point(|(key, _)| key < bounds.start());
        let end = self.entries.partition_point(|(key, _)| key <= bounds.end());
        self.entries[start..end]
            .iter()
            .map(|(key, value)| (key, value))
    }

    #[inline(always)]
    pub fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = (&K, &mut V)> {
        self.entries.iter_mut().map(|(key, value)| (&*key, value))
    }

    #[inline(always)]
    pub fn values(&self) -> impl DoubleEndedIterator<Item = &V> {
        self.entries.iter().map(|(_, value)| value)
    }

    #[inline(always)]
    pub fn values_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut V> {
        self.entries.iter_mut().map(|(_, value)| value)
    }

    #[inline(always)]
    pub fn prefix(&self, end: usize) -> impl Iterator<Item = (&K, &V)> {
        self.entries[..end].iter().map(|(key, value)| (key, value))
    }

    #[inline(always)]
    pub fn prefix_mut(&mut self, end: usize) -> impl Iterator<Item = (&K, &mut V)> {
        self.entries[..end]
            .iter_mut()
            .map(|(key, value)| (&*key, value))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::SortedVector;

    #[test]
    fn inserts_at_beginning_middle_and_end_without_duplicate_keys() {
        let mut entries = SortedVector::default();
        entries.get_or_insert_with(20, || "twenty");
        entries.get_or_insert_with(10, || "ten");
        entries.get_or_insert_with(30, || "thirty");
        entries.get_or_insert_with(25, || "twenty-five");
        entries.get_or_insert_with(20, || panic!("existing key recreated"));

        let keys: Vec<_> = entries.iter().map(|(key, _)| *key).collect();
        assert_eq!(keys, [10, 20, 25, 30]);
        assert_eq!(entries.get(&20), Some(&"twenty"));
    }

    #[test]
    fn inclusive_end_includes_exact_keys_and_excludes_larger_keys() {
        let mut entries = SortedVector::default();
        for key in [10, 20, 30] {
            entries.get_or_insert_with(key, || key);
        }

        assert_eq!(entries.inclusive_end(Some(&5)), 0);
        assert_eq!(entries.inclusive_end(Some(&20)), 2);
        assert_eq!(entries.inclusive_end(Some(&25)), 2);
        assert_eq!(entries.inclusive_end(Some(&35)), 3);
        assert_eq!(entries.inclusive_end(None), 3);
    }

    #[test]
    fn randomized_operations_match_btree_map_order_and_values() {
        let mut entries = SortedVector::default();
        let mut reference = BTreeMap::new();
        let mut state = 0x9e37_79b9_u32;

        for value in 0..10_000_u32 {
            // A deterministic LCG gives broad insertion/update coverage without
            // adding a test-only random-number dependency.
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let key = state % 257;
            *entries.get_or_insert_with(key, || value) = value;
            reference.insert(key, value);
        }

        let actual: Vec<_> = entries.iter().map(|(key, value)| (*key, *value)).collect();
        let expected: Vec<_> = reference.into_iter().collect();
        assert_eq!(actual, expected);
    }
}
