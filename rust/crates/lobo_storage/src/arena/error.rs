#[cfg(test)]
mod tests {
    #[test]
    fn reserved_error_module_is_compiled_by_the_test_suite() {
        assert_eq!(module_path!(), "lobo_storage::arena::error::tests");
    }
}
