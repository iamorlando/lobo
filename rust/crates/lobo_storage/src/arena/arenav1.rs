// type ArenaKey = usize;

use std::mem::replace;

type Generation = u64;
#[derive(Clone)]
pub struct ArenaNode<T> {
    pub value: T,
    pub next: Option<ArenaKey>,
    pub prev: Option<ArenaKey>,
}

#[derive(Copy, Clone)]
pub struct ArenaLinks {
    pub(crate) next: Option<ArenaKey>,
    pub(crate) prev: Option<ArenaKey>,
}

impl<T> ArenaNode<T> {
    fn new(value: T) -> Self {
        Self {
            value,
            next: None,
            prev: None,
        }
    }

    #[inline(always)]
    pub(crate) fn links(&self) -> ArenaLinks {
        ArenaLinks {
            next: self.next,
            prev: self.prev,
        }
    }
}
#[derive(Copy, Clone, Hash, Eq, PartialEq, Debug)]
pub struct ArenaKey {
    idx: usize,
    generation: Generation,
}
impl ArenaKey {
    fn unlock_mut<T>(self, slot: &mut Slot<T>) -> Option<&mut Slot<T>> {
        if let Slot::Occupied { generation, .. } = slot
            && *generation == self.generation
        {
            return Some(slot);
        }
        None
    }

    fn unlock<T>(self, slot: &Slot<T>) -> Option<&Slot<T>> {
        if let Slot::Occupied { generation, .. } = slot
            && *generation == self.generation
        {
            return Some(slot);
        }
        None
    }
}

#[derive(Clone)]
enum Slot<T> {
    Occupied {
        generation: Generation,
        value: ArenaNode<T>,
    },
    Vacant,
    Retired,
}
#[derive(Clone)]
pub struct Arena<T> {
    slots: Vec<Slot<T>>,
    free: Vec<ArenaKey>,
    len: usize,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        let fi: Vec<ArenaKey> = Vec::new();
        let vec: Vec<Slot<T>> = Vec::new();
        Self {
            slots: vec,
            free: fi,
            len: 0,
        }
    }
}

impl<T> Arena<T> {
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn get_slot_mut(slots: &mut [Slot<T>], key: ArenaKey) -> Option<&mut Slot<T>> {
        let slot = slots.get_mut(key.idx)?;
        key.unlock_mut(slot)
    }
    pub fn get_node_mut(&mut self, key: ArenaKey) -> Option<&mut ArenaNode<T>> {
        if let Slot::Occupied { value, .. } =
            key.unlock_mut(Self::get_slot_mut(&mut self.slots, key)?)?
        {
            Some(value)
        } else {
            None
        }
    }
    pub fn get_mut(&mut self, key: ArenaKey) -> Option<&mut T> {
        Some(&mut self.get_node_mut(key)?.value)
    }

    fn get_slot(&self, key: ArenaKey) -> Option<&Slot<T>> {
        let slot = self.slots.get(key.idx)?;
        key.unlock(slot)
    }
    pub fn get_node(&self, key: ArenaKey) -> Option<&ArenaNode<T>> {
        if let Slot::Occupied { value, .. } = key.unlock(self.get_slot(key)?)? {
            Some(value)
        } else {
            None
        }
    }
    pub fn get(&self, key: ArenaKey) -> Option<&T> {
        Some(&self.get_node(key)?.value)
    }

    #[inline(always)]
    pub(crate) unsafe fn get_node_unchecked(&self, key: ArenaKey) -> &ArenaNode<T> {
        // SAFETY: the caller guarantees that key identifies a live slot.
        let slot = unsafe { self.slots.get_unchecked(key.idx) };
        let Slot::Occupied { value, .. } = slot else {
            // SAFETY: a live key always addresses an occupied slot.
            unsafe { std::hint::unreachable_unchecked() }
        };
        value
    }

    #[inline(always)]
    pub(crate) unsafe fn get_node_unchecked_mut(&mut self, key: ArenaKey) -> &mut ArenaNode<T> {
        // SAFETY: the caller guarantees that key identifies a live slot.
        let slot = unsafe { self.slots.get_unchecked_mut(key.idx) };
        let Slot::Occupied { value, .. } = slot else {
            // SAFETY: a live key always addresses an occupied slot.
            unsafe { std::hint::unreachable_unchecked() }
        };
        value
    }

    #[inline(always)]
    pub(crate) unsafe fn get_unchecked(&self, key: ArenaKey) -> &T {
        // SAFETY: forwarded from the caller.
        &unsafe { self.get_node_unchecked(key) }.value
    }

    #[inline(always)]
    pub(crate) unsafe fn get_unchecked_mut(&mut self, key: ArenaKey) -> &mut T {
        // SAFETY: forwarded from the caller.
        &mut unsafe { self.get_node_unchecked_mut(key) }.value
    }

    #[inline(always)]
    pub(crate) unsafe fn linked_next_unchecked(&self, key: ArenaKey) -> Option<ArenaKey> {
        // SAFETY: forwarded from the caller.
        unsafe { self.get_node_unchecked(key) }.next
    }

    #[inline(always)]
    pub(crate) unsafe fn push_back_unchecked(
        &mut self,
        head: &mut Option<ArenaKey>,
        tail: &mut Option<ArenaKey>,
        key: ArenaKey,
    ) {
        let previous = *tail;
        {
            // SAFETY: the caller guarantees that key is live and detached.
            let node = unsafe { self.get_node_unchecked_mut(key) };
            node.prev = previous;
            node.next = None;
        }

        match previous {
            Some(previous) => {
                // SAFETY: the current tail is a live node in this list.
                unsafe { self.get_node_unchecked_mut(previous) }.next = Some(key);
            }
            None => *head = Some(key),
        }
        *tail = Some(key);
    }

    #[inline(always)]
    pub(crate) unsafe fn unlink_unchecked(
        &mut self,
        head: &mut Option<ArenaKey>,
        tail: &mut Option<ArenaKey>,
        key: ArenaKey,
    ) {
        // SAFETY: the caller guarantees that key is a live member of this list.
        let links = unsafe { self.get_node_unchecked(key) }.links();
        // SAFETY: every linked neighbor is live while key is linked.
        unsafe { self.relink_remaining_nodes_unchecked(head, tail, links) };

        // SAFETY: key remains live until the caller explicitly removes it.
        let node = unsafe { self.get_node_unchecked_mut(key) };
        node.prev = None;
        node.next = None;
    }

    #[inline(always)]
    pub(crate) unsafe fn relink_remaining_nodes_unchecked(
        &mut self,
        head: &mut Option<ArenaKey>,
        tail: &mut Option<ArenaKey>,
        links: ArenaLinks,
    ) {
        match links.prev {
            Some(previous) => {
                // SAFETY: the caller guarantees all linked neighbors are live.
                unsafe { self.get_node_unchecked_mut(previous) }.next = links.next;
            }
            None => *head = links.next,
        }

        match links.next {
            Some(next) => {
                // SAFETY: the caller guarantees all linked neighbors are live.
                unsafe { self.get_node_unchecked_mut(next) }.prev = links.prev;
            }
            None => *tail = links.prev,
        }
    }
    pub fn transform(&mut self, key: ArenaKey, f: impl FnOnce(&mut T)) -> bool {
        let Some(value) = self.get_mut(key) else {
            return false;
        };

        f(value);
        true
    }

    #[inline(always)]
    fn push_with_free_key(&mut self, item: T) -> Result<ArenaKey, T> {
        if let Some(key) = self.free.pop() {
            let slot = &mut self.slots[key.idx];
            assert!(
                matches!(slot, Slot::Vacant),
                "A free index led to a non-vacated slot."
            );
            *slot = Slot::Occupied {
                generation: key.generation,
                value: ArenaNode::new(item),
            };
            self.len += 1;
            Ok(key)
        } else {
            Err(item)
        }
    }
    #[inline(always)]
    fn push_with_new_key(&mut self, item: T) -> ArenaKey {
        {
            let gen_ = 0_u64;
            self.slots.push(Slot::Occupied {
                generation: gen_,
                value: ArenaNode::new(item),
            });
            self.len += 1;
            ArenaKey {
                idx: self.slots.len() - 1,
                generation: gen_,
            }
        }
    }
    #[inline(always)]
    pub fn push(&mut self, item: T) -> ArenaKey {
        match self.push_with_free_key(item) {
            Ok(key) => key,
            Err(item) => self.push_with_new_key(item),
        }
    }
    fn remove_slot(&mut self, key: ArenaKey) -> Option<Slot<T>> {
        let slot = Self::get_slot_mut(&mut self.slots, key)?;
        let next_generation = key.generation.checked_add(1);
        let local_slot = match next_generation {
            Some(next_generation) => {
                self.free.push(ArenaKey {
                    idx: key.idx,
                    generation: next_generation,
                });
                replace(slot, Slot::Vacant)
            }
            None => replace(slot, Slot::Retired),
        };
        self.len -= 1;
        Some(local_slot)
    }
    pub fn remove_and_return_node(&mut self, key: ArenaKey) -> Option<ArenaNode<T>> {
        let slot = self.remove_slot(key)?;
        let Slot::Occupied { value, .. } = slot else {
            unreachable!("remove_slot returned a non-occupied slot")
        };
        Some(value)
    }
    pub fn remove_and_return_value(&mut self, key: ArenaKey) -> Option<T> {
        let node = self.remove_and_return_node(key)?;
        Some(node.value)
    }

    #[inline(always)]
    pub(crate) unsafe fn remove_and_return_value_unchecked(&mut self, key: ArenaKey) -> T {
        // SAFETY: the caller guarantees that key identifies a live slot.
        let slot = unsafe { self.slots.get_unchecked_mut(key.idx) };
        let next_generation = key.generation.checked_add(1);
        let occupied = match next_generation {
            Some(next_generation) => {
                self.free.push(ArenaKey {
                    idx: key.idx,
                    generation: next_generation,
                });
                replace(slot, Slot::Vacant)
            }
            None => replace(slot, Slot::Retired),
        };
        self.len -= 1;

        let Slot::Occupied { value, .. } = occupied else {
            // SAFETY: a live key always removes an occupied slot.
            unsafe { std::hint::unreachable_unchecked() }
        };
        value.value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::HashSet;
    use std::rc::Rc;

    #[test]
    fn default_arena_is_empty() {
        let arena = Arena::<i32>::default();

        assert!(arena.slots.is_empty());
        assert!(arena.free.is_empty());
    }

    #[test]
    fn push_returns_key_for_inserted_value() {
        let mut arena = Arena::default();

        let key = arena.push(42);

        assert_eq!(key.idx, 0);
        assert_eq!(key.generation, 0);
        assert_eq!(arena.get(key), Some(&42));
    }

    #[test]
    fn multiple_values_have_distinct_keys() {
        let mut arena = Arena::default();

        let first = arena.push("first");
        let second = arena.push("second");
        let third = arena.push("third");

        assert_ne!(first, second);
        assert_ne!(second, third);
        assert_ne!(first, third);

        assert_eq!(arena.get(first), Some(&"first"));
        assert_eq!(arena.get(second), Some(&"second"));
        assert_eq!(arena.get(third), Some(&"third"));
    }

    #[test]
    fn get_mut_modifies_value_in_place() {
        let mut arena = Arena::default();
        let key = arena.push(String::from("before"));

        arena
            .get_mut(key)
            .expect("key should be valid")
            .push_str(" after");

        assert_eq!(arena.get(key).map(String::as_str), Some("before after"));
    }

    #[test]
    fn invalid_index_returns_none() {
        let mut arena = Arena::<i32>::default();

        let invalid = ArenaKey {
            idx: usize::MAX,
            generation: 0,
        };

        assert_eq!(arena.get(invalid), None);
        assert_eq!(arena.get_mut(invalid), None);
        assert_eq!(arena.remove_and_return_value(invalid), None);
    }

    #[test]
    fn incorrect_generation_returns_none() {
        let mut arena = Arena::default();
        let key = arena.push(42);

        let incorrect = ArenaKey {
            idx: key.idx,
            generation: key.generation + 1,
        };

        assert_eq!(arena.get(incorrect), None);
        assert_eq!(arena.get_mut(incorrect), None);
        assert_eq!(arena.remove_and_return_value(incorrect), None);

        // The valid value must remain untouched.
        assert_eq!(arena.get(key), Some(&42));
    }

    #[test]
    fn remove_returns_owned_value() {
        let mut arena = Arena::default();
        let key = arena.push(String::from("owned value"));

        let removed = arena.remove_and_return_value(key);

        assert_eq!(removed, Some(String::from("owned value")));
        assert_eq!(arena.get(key), None);
    }

    #[test]
    fn remove_supports_non_copy_values() {
        let mut arena = Arena::default();
        let key = arena.push(vec![1, 2, 3]);

        let removed = arena.remove_and_return_value(key);

        assert_eq!(removed, Some(vec![1, 2, 3]));
    }

    #[test]
    fn removing_twice_returns_none_second_time() {
        let mut arena = Arena::default();
        let key = arena.push(42);

        assert_eq!(arena.remove_and_return_value(key), Some(42));
        assert_eq!(arena.remove_and_return_value(key), None);
    }

    #[test]
    fn removal_makes_slot_vacant_and_advances_generation() {
        let mut arena = Arena::default();
        let key = arena.push(42);

        assert_eq!(arena.remove_and_return_value(key), Some(42));

        assert!(matches!(arena.slots.get(key.idx), Some(Slot::Vacant)));

        assert_eq!(
            arena.free,
            vec![ArenaKey {
                idx: key.idx,
                generation: key.generation + 1,
            }]
        );
    }

    #[test]
    fn push_reuses_vacant_slot() {
        let mut arena = Arena::default();

        let old_key = arena.push("old");
        assert_eq!(arena.remove_and_return_value(old_key), Some("old"));

        let new_key = arena.push("new");

        assert_eq!(new_key.idx, old_key.idx);
        assert_eq!(new_key.generation, old_key.generation + 1);
        assert_eq!(arena.get(new_key), Some(&"new"));
    }

    #[test]
    fn stale_key_does_not_access_replacement() {
        let mut arena = Arena::default();

        let stale_key = arena.push(String::from("old"));
        arena.remove_and_return_value(stale_key);

        let current_key = arena.push(String::from("new"));

        assert_eq!(stale_key.idx, current_key.idx);
        assert_eq!(arena.get(stale_key), None);
        assert_eq!(arena.get_mut(stale_key), None);

        assert_eq!(arena.get(current_key).map(String::as_str), Some("new"));
    }

    #[test]
    fn stale_key_cannot_remove_replacement() {
        let mut arena = Arena::default();

        let stale_key = arena.push(10);
        arena.remove_and_return_value(stale_key);

        let current_key = arena.push(20);

        assert_eq!(arena.remove_and_return_value(stale_key), None);
        assert_eq!(arena.get(current_key), Some(&20));
    }

    #[test]
    fn unchecked_removal_advances_generation() {
        let mut arena = Arena::default();
        let stale_key = arena.push(10);

        // SAFETY: stale_key identifies the live value inserted immediately above.
        assert_eq!(
            unsafe { arena.remove_and_return_value_unchecked(stale_key) },
            10
        );

        let current_key = arena.push(20);

        assert_eq!(current_key.idx, stale_key.idx);
        assert_eq!(current_key.generation, stale_key.generation + 1);
        assert_eq!(arena.get(stale_key), None);
        assert_eq!(arena.get(current_key), Some(&20));
    }

    #[test]
    fn removing_one_value_does_not_affect_others() {
        let mut arena = Arena::default();

        let first = arena.push(10);
        let middle = arena.push(20);
        let last = arena.push(30);

        assert_eq!(arena.remove_and_return_value(middle), Some(20));

        assert_eq!(arena.get(first), Some(&10));
        assert_eq!(arena.get(middle), None);
        assert_eq!(arena.get(last), Some(&30));

        let replacement = arena.push(40);

        assert_eq!(replacement.idx, middle.idx);
        assert_eq!(arena.get(first), Some(&10));
        assert_eq!(arena.get(replacement), Some(&40));
        assert_eq!(arena.get(last), Some(&30));
    }

    #[test]
    fn generation_advances_across_repeated_reuse() {
        let mut arena = Arena::default();
        let mut key = arena.push(0_u64);

        for expected_generation in 1..=100_u64 {
            let stale_key = key;

            assert_eq!(
                arena.remove_and_return_value(stale_key),
                Some(expected_generation - 1)
            );

            key = arena.push(expected_generation);

            assert_eq!(key.idx, 0);
            assert_eq!(key.generation, expected_generation);
            assert_eq!(arena.get(stale_key), None);
            assert_eq!(arena.get(key), Some(&expected_generation));
        }
    }

    #[test]
    fn free_slots_are_reused_in_lifo_order() {
        let mut arena = Arena::default();

        let first = arena.push(10);
        let second = arena.push(20);
        let third = arena.push(30);

        arena.remove_and_return_value(first);
        arena.remove_and_return_value(third);

        let replacement_for_third = arena.push(300);
        let replacement_for_first = arena.push(100);

        assert_eq!(replacement_for_third.idx, third.idx);
        assert_eq!(replacement_for_first.idx, first.idx);
        assert_eq!(arena.get(second), Some(&20));
    }

    #[test]
    fn maximum_generation_is_retired_on_removal() {
        let mut arena = Arena {
            slots: vec![Slot::Occupied {
                generation: Generation::MAX,
                value: ArenaNode::new(String::from("last generation")),
            }],
            free: Vec::new(),
            len: 1,
        };

        let key = ArenaKey {
            idx: 0,
            generation: Generation::MAX,
        };

        assert_eq!(
            arena.remove_and_return_value(key),
            Some(String::from("last generation"))
        );

        assert!(matches!(arena.slots.first(), Some(Slot::Retired)));

        assert!(arena.free.is_empty());
        assert_eq!(arena.get(key), None);
    }

    #[test]
    fn retired_slot_is_never_reused() {
        let mut arena = Arena {
            slots: vec![Slot::Occupied {
                generation: Generation::MAX,
                value: ArenaNode::new(10),
            }],
            free: Vec::new(),
            len: 1,
        };

        let retired_key = ArenaKey {
            idx: 0,
            generation: Generation::MAX,
        };

        assert_eq!(arena.remove_and_return_value(retired_key), Some(10));

        let new_key = arena.push(20);

        assert_eq!(new_key.idx, 1);
        assert_eq!(new_key.generation, 0);
        assert_eq!(arena.slots.len(), 2);
        assert_eq!(arena.get(new_key), Some(&20));
    }

    #[test]
    fn maximum_generation_can_be_used_before_retirement() {
        let mut arena = Arena {
            slots: vec![Slot::Occupied {
                generation: Generation::MAX - 1,
                value: ArenaNode::new(10),
            }],
            free: Vec::new(),
            len: 1,
        };

        let penultimate_key = ArenaKey {
            idx: 0,
            generation: Generation::MAX - 1,
        };

        assert_eq!(arena.remove_and_return_value(penultimate_key), Some(10));

        let maximum_key = arena.push(20);

        assert_eq!(maximum_key.idx, 0);
        assert_eq!(maximum_key.generation, Generation::MAX);
        assert_eq!(arena.get(maximum_key), Some(&20));

        assert_eq!(arena.remove_and_return_value(maximum_key), Some(20));

        assert!(matches!(arena.slots.first(), Some(Slot::Retired)));
        assert!(arena.free.is_empty());
    }

    #[test]
    #[should_panic(expected = "A free index led to a non-vacated slot.")]
    fn corrupted_free_list_panics_for_occupied_slot() {
        let mut arena = Arena::default();
        let occupied_key = arena.push(10);

        // Simulate internal corruption.
        arena.free.push(occupied_key);

        let _ = arena.push(20);
    }

    #[test]
    #[should_panic]
    fn corrupted_free_list_panics_for_out_of_bounds_index() {
        let mut arena = Arena::<i32>::default();

        // Simulate internal corruption.
        arena.free.push(ArenaKey {
            idx: 1,
            generation: 1,
        });

        let _ = arena.push(20);
    }

    #[test]
    fn removed_value_is_dropped_exactly_once() {
        struct DropSpy {
            drop_count: Rc<Cell<usize>>,
        }

        impl Drop for DropSpy {
            fn drop(&mut self) {
                self.drop_count.set(self.drop_count.get() + 1);
            }
        }

        let drop_count = Rc::new(Cell::new(0));
        let mut arena = Arena::default();

        let key = arena.push(DropSpy {
            drop_count: Rc::clone(&drop_count),
        });

        let removed = arena
            .remove_and_return_value(key)
            .expect("value should be removable");

        assert_eq!(drop_count.get(), 0);

        drop(removed);

        assert_eq!(drop_count.get(), 1);

        // The arena must no longer own another copy.
        drop(arena);

        assert_eq!(drop_count.get(), 1);
    }

    #[test]
    fn occupied_values_are_dropped_with_arena() {
        struct DropSpy {
            drop_count: Rc<Cell<usize>>,
        }

        impl Drop for DropSpy {
            fn drop(&mut self) {
                self.drop_count.set(self.drop_count.get() + 1);
            }
        }

        let drop_count = Rc::new(Cell::new(0));

        {
            let mut arena = Arena::default();

            arena.push(DropSpy {
                drop_count: Rc::clone(&drop_count),
            });

            arena.push(DropSpy {
                drop_count: Rc::clone(&drop_count),
            });

            assert_eq!(drop_count.get(), 0);
        }

        assert_eq!(drop_count.get(), 2);
    }

    #[test]
    fn arena_key_supports_hash_set_membership() {
        let mut arena = Arena::default();
        let key = arena.push(42);

        let mut keys = HashSet::new();

        assert!(keys.insert(key));
        assert!(keys.contains(&key));
        assert!(!keys.insert(key));
    }
}
