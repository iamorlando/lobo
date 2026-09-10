pub mod arenav1;

#[cfg(test)]
mod error;

#[cfg(test)]
mod tests {
    use super::arenav1::Arena;

    #[test]
    fn arena_module_reexports_the_primary_implementation() {
        let arena = Arena::<u8>::default();
        assert_eq!(arena.len(), 0);
    }
}
