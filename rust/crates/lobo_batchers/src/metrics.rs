bitflags::bitflags! {
    /// Optional observer metrics, applied by batching after native publication.
    /// Visible quantity, price, side, identity and order count remain available.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct PriceLevelMetrics: u32 {
        const HIDDEN_QUANTITY = 1 << 0;
    }
}
impl PriceLevelMetrics {
    /// Keep output schemas stable while withholding unselected hidden totals.
    #[inline]
    pub fn hidden_quantity(self, quantity: u64) -> u64 {
        if self.contains(Self::HIDDEN_QUANTITY) {
            quantity
        } else {
            0
        }
    }
}
