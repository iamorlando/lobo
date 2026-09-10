pub mod price;
pub mod time;
pub mod uuid;

pub use price::{CompressedPrice, Price64, Price128, PriceType};

pub type Quantity = u64;
pub type Notional = u128;
pub mod mixins;

#[cfg(test)]
mod tests {
    use super::{CompressedPrice, Notional, Price64, Price128, PriceType};

    #[test]
    fn compressed_price_is_four_bytes_and_preserves_raw_ordering() {
        assert_eq!(size_of::<CompressedPrice>(), 4);
        assert!(CompressedPrice::from(99_u32) < CompressedPrice::from(100_u32));
        assert_eq!(u32::from(CompressedPrice::from(123_456_u32)), 123_456);
        assert!(CompressedPrice::try_from(u64::from(u32::MAX) + 1).is_err());
    }

    #[test]
    fn price_representations_have_exact_layout_and_range() {
        assert_eq!(size_of::<CompressedPrice>(), 4);
        assert_eq!(size_of::<Price64>(), 8);
        assert_eq!(size_of::<Price128>(), 16);
        assert_eq!(Price64::MAX.into_u128(), u128::from(u64::MAX));
        assert_eq!(Price128::MAX.into_u128(), u128::MAX);
    }

    #[test]
    fn every_price_representation_sorts_by_raw_value() {
        fn assert_order<P: PriceType>() {
            assert!(P::from(99_u32) < P::from(100_u32));
        }

        assert_order::<CompressedPrice>();
        assert_order::<Price64>();
        assert_order::<Price128>();
    }

    #[test]
    fn notional_retains_full_u128_range() {
        let notional: Notional = u128::MAX;
        assert_eq!(notional, u128::MAX);
    }
}
