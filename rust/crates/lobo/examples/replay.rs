use lobo::{
    prelude::*,
    replay::custom::{
        AdaptForReplay,
        orders::{AddOrder, CancelOrder, RemoveOrder},
    },
};

fn main() {
    let mut book: ReplayBook = ReplayBook::new();
    let id = Uuid::new_v4();

    // An adapter decodes its wire format into these typed mutations.
    let added = AddOrder {
        timestamp: 1_000,
        id,
        side: Side::Buy,
        quantity: 25,
        price: Price64::from(10_000_u64),
    }
    .process(&mut book);
    assert!(added.is_ok(), "replaying the add failed");

    let canceled = CancelOrder {
        timestamp: 2_000,
        id,
        quantity: 10,
    }
    .process(&mut book);
    assert!(canceled.is_ok(), "replaying the partial cancel failed");
    assert_eq!(book.order_storage.bids.visible_quantity, 15);
    println!(
        "Quantity after partial cancel: {}",
        book.order_storage.bids.visible_quantity
    );

    let removed = RemoveOrder {
        timestamp: 3_000,
        id,
    }
    .process(&mut book);
    assert!(removed.is_ok(), "replaying the remove failed");
    assert!(book.order_storage.bids.is_empty());
}
