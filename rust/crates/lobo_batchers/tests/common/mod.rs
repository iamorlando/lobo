use arrow_array::RecordBatch;
use lobo_batchers::{arrow::batch::PriceLevelArrowBatcher, traits::Batch};
use lobo_events::{BookEvent, PriceLevelChangeEvent};
use lobo_models::Side;
use lobo_primitives::Price128;

pub fn event(sequence: u64) -> BookEvent<PriceLevelChangeEvent<Price128>> {
    BookEvent::from_event(
        PriceLevelChangeEvent::new(
            10 + sequence,
            20 + sequence,
            Price128::from_raw(u128::MAX - u128::from(sequence)),
            3,
            Side::Buy,
        ),
        sequence,
        "ABC".into(),
    )
}

pub fn batch() -> RecordBatch {
    let mut batcher = PriceLevelArrowBatcher::new(2);
    assert!(batcher.push(&event(1)).unwrap().is_none());
    batcher.push(&event(2)).unwrap().unwrap()
}
