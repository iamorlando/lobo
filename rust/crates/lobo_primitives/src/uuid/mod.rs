pub use uuid::Uuid;

#[cfg(test)]
mod tests {
    use super::Uuid;

    #[test]
    fn reexports_uuid_with_v4_support() {
        let id = Uuid::new_v4();
        assert_eq!(id.get_version_num(), 4);
    }
}
