use lobo::prelude::*;

fn main() {
    let mut book: OrderBook = OrderBook::new();
    let added = book.submit::<LimitOrderData, LimitOrderData, MutatingFills>(Command::Add {
        order: LimitOrder::new(Some(10_000_u64), 25, Uuid::new_v4(), Side::Sell),
    });
    assert!(added.is_ok(), "adding the limit order failed");

    let filled = book.submit::<MarketOrderData, LimitOrderData, MutatingFills>(Command::Fill {
        order: MarketOrder::new(10, Uuid::new_v4(), Side::Buy),
        execution: MutatingFills,
        reports: Reports::default(),
    });
    assert!(filled.is_ok(), "filling the market order failed");
    assert_eq!(book.order_storage.asks.visible_quantity, 15);
    println!(
        "Remaining sell quantity: {}",
        book.order_storage.asks.visible_quantity
    );
}
