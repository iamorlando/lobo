pub mod core;
pub mod order_types;
pub mod traits;

#[cfg(test)]
mod tests {
    use super::order_types::MarketOrder;
    use crate::Side;
    use lobo_primitives::CompressedPrice;
    use lobo_primitives::uuid::Uuid;

    #[test]
    fn order_modules_are_available_together() {
        let order = MarketOrder::<CompressedPrice>::new(3, Uuid::new_v4(), Side::Buy);
        assert_eq!(order.common_data.quantity, 3);
    }
}
