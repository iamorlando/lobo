pub mod prelude {
    pub use lobo_books::price_time_priority::*;
    pub use lobo_models::*;
    pub use lobo_primitives::*;
    pub use lobo_storage::*;
}

#[cfg(test)]
mod tests {
    use super::prelude::{Book, CompressedPrice, Side};
    use lobo_storage::{policies::UpdateHiddenQuantity, price_level::DeepPriceLevel};

    #[test]
    fn prelude_reexports_the_public_library_surface() {
        let price = CompressedPrice::from(100_u32);
        let book = Book::<DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>>::new();
        assert_eq!(price, 100);
        assert_eq!(Side::Buy, Side::Buy);
        assert!(book.order_storage.bids.is_empty());
    }
}
