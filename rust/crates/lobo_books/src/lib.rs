use lobo_events::BookEvent;
pub mod price_time_priority;
struct Sequencer {
    value: u64,
}
impl Default for Sequencer {
    fn default() -> Self {
        Self { value: 0 }
    }
}
impl Sequencer {
    // pub fn new() -> Self {
    //     Self { value: 0 }
    // }
    fn current(&self) -> u64 {
        self.value
    }
    fn next(&mut self) -> u64 {
        self.value += 1;
        self.value
    }
}

#[cfg(test)]
mod tests {
    use super::price_time_priority::Book;
    use lobo_primitives::CompressedPrice;
    use lobo_storage::{policies::UpdateHiddenQuantity, price_level::DeepPriceLevel};

    #[test]
    fn price_time_priority_book_is_exported() {
        let book = Book::<DeepPriceLevel<CompressedPrice, UpdateHiddenQuantity>>::new();
        assert!(book.order_storage.bids.is_empty());
        assert!(book.order_storage.asks.is_empty());
    }
}
