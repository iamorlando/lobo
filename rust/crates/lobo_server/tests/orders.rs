#[cfg(feature = "feed-tests")]
use lobo_replay::{custom::MarketDataAdapter, order_messages::OrderMessages};
use lobo_models::{
    Side,
    server::{Command, FeedMessage, LimitOrder, MarketOrder, Order, OrderFields},
};
use lobo_primitives::{CompressedPrice, uuid::Uuid};
use lobo_server::Registry;
use std::sync::Arc;
use tokio::sync::watch;

fn registry() -> Arc<Registry> {
    Arc::new(Registry::new(8192, watch::channel(false).1))
}
fn fields(id: u128, side: Side, quantity: u64) -> OrderFields {
    OrderFields {
        id: Uuid::from_u128(id),
        trader: Uuid::nil(),
        side,
        quantity,
    }
}
fn limit(id: u128, side: Side, price: u32, quantity: u64) -> Order {
    Order::Limit(LimitOrder {
        fields: fields(id, side, quantity),
        price,
    })
}
#[cfg(feature = "feed-tests")]
fn feed(adapter: &mut OrderMessages, message: &FeedMessage) {
    adapter
        .receive(&serde_json::to_vec(message).unwrap(), false)
        .unwrap();
}

#[test]
fn commands_mutate_native_books_and_preview_does_not_publish() {
    let registry = registry();
    let book = registry.register(Some("TEST".into()), 2, 0).unwrap();
    book.submit(Command::Add {
        order: limit(1, Side::Sell, 10100, 10),
    })
    .unwrap();
    book.submit(Command::Add {
        order: limit(2, Side::Sell, 10200, 20),
    })
    .unwrap();
    let mut events = registry.events.subscribe();
    let order = Order::Market(MarketOrder {
        fields: fields(3, Side::Buy, 15),
    });
    let preview = book
        .submit(Command::Simulate {
            order: order.clone(),
        })
        .unwrap();
    assert!(preview.simulated);
    assert_eq!(preview.filled, 15);
    assert_eq!(preview.executions.len(), 2);
    assert!(preview.executions.iter().all(|e| e.simulated));
    assert_eq!(preview.resting_order_id, None);
    assert!(events.try_recv().is_err());
    assert_eq!(book.native.lock().order_storage.asks.visible_quantity, 30);
    let filled = book.submit(Command::Fill { order }).unwrap();
    assert!(!filled.simulated);
    assert_eq!(filled.filled, 15);
    assert!((filled.average_price.unwrap() - 10133.333333333334).abs() < 1e-8);
    assert_eq!(filled.sequence, 3);
    assert_eq!(book.native.lock().order_storage.asks.visible_quantity, 15);
    assert!(matches!(
        &*events.try_recv().unwrap(),
        FeedMessage::Update {
            command: Command::Fill { .. },
            ..
        }
    ));
    book.submit(Command::Cancel {
        id: Uuid::from_u128(2),
        quantity: 4,
    })
    .unwrap();
    let executed = book
        .submit(Command::Execute {
            id: Uuid::from_u128(2),
            quantity: 3,
            price: Some(10250),
        })
        .unwrap();
    assert_eq!(executed.executions[0].price, 10250);
    assert_eq!(executed.filled, 3);
    book.submit(Command::Modify {
        id: Uuid::from_u128(2),
        quantity: 12,
        price: Some(10300),
        new_id: Some(Uuid::from_u128(4)),
    })
    .unwrap();
    assert!(
        book.native
            .lock()
            .order_storage
            .order(Uuid::from_u128(2))
            .is_none()
    );
    assert_eq!(
        book.native.lock().order_storage.asks.best().unwrap().0,
        &CompressedPrice::from(10300u32)
    );
    book.submit(Command::Remove {
        id: Uuid::from_u128(4),
    })
    .unwrap();
    assert_eq!(book.native.lock().order_storage.asks.visible_quantity, 0);
}

#[test]
fn errors_leave_native_state_and_feed_sequence_unchanged() {
    let registry = registry();
    let book = registry.register(Some("TEST".into()), 0, 0).unwrap();
    book.submit(Command::Add {
        order: limit(1, Side::Buy, 100, 10),
    })
    .unwrap();
    let before = serde_json::to_value(book.snapshot()).unwrap()["orders"].clone();
    let mut events = registry.events.subscribe();
    for command in [
        Command::Add {
            order: limit(1, Side::Buy, 100, 10),
        },
        Command::Execute {
            id: Uuid::from_u128(1),
            quantity: 11,
            price: None,
        },
        Command::Cancel {
            id: Uuid::from_u128(1),
            quantity: 0,
        },
        Command::Remove {
            id: Uuid::from_u128(99),
        },
        Command::Modify {
            id: Uuid::from_u128(99),
            quantity: 1,
            price: None,
            new_id: None,
        },
        Command::Add {
            order: Order::Market(MarketOrder {
                fields: fields(2, Side::Buy, 10),
            }),
        },
    ] {
        assert!(book.submit(command).is_err());
    }
    assert!(events.try_recv().is_err());
    let after = serde_json::to_value(book.snapshot()).unwrap();
    assert_eq!(before, after["orders"]);
    assert_eq!(after["sequence"], 1);
}

#[test]
#[cfg(feature = "feed-tests")]
fn server_and_browser_adapter_agree_through_iceberg_replenishment_and_reconnect() {
    let registry = registry();
    let book = registry.register(Some("TEST".into()), 2, 0).unwrap();
    let mut adapter = OrderMessages::new("TEST").unwrap();
    feed(&mut adapter, &book.snapshot());
    let mut messages = registry.events.subscribe();
    let commands = [
        Command::Add {
            order: Order::Iceberg(lobo_models::server::IcebergOrder {
                fields: fields(1, Side::Sell, 5),
                price: 10100,
                hidden_quantity: 10,
                peak_quantity: 5,
            }),
        },
        Command::Add {
            order: limit(2, Side::Sell, 10100, 7),
        },
        Command::Execute {
            id: Uuid::from_u128(1),
            quantity: 5,
            price: None,
        },
        Command::Fill {
            order: Order::Market(MarketOrder {
                fields: fields(3, Side::Buy, 9),
            }),
        },
        Command::Fill {
            order: Order::Market(MarketOrder {
                fields: fields(4, Side::Buy, 5),
            }),
        },
    ];
    for command in commands {
        book.submit(command).unwrap();
        feed(&mut adapter, &messages.try_recv().unwrap());
        let native = book.native.lock();
        let viewed = adapter.selected_book().unwrap();
        assert_eq!(
            native.order_storage.asks.visible_quantity,
            viewed.visible_quantity(Side::Sell)
        );
        assert_eq!(
            native.order_storage.order_to_arena_map.len(),
            viewed.order_count()
        );
    }
    adapter.disconnected();
    feed(&mut adapter, &book.snapshot());
    assert!(!adapter.state().warming);
    assert_eq!(
        adapter
            .selected_book()
            .unwrap()
            .visible_quantity(Side::Sell),
        book.native.lock().order_storage.asks.visible_quantity
    );
}

#[test]
#[cfg(feature = "feed-tests")]
fn live_simulation_fills_in_native_fifo_and_keeps_main_timeline_independent() {
    let registry = registry();
    let book = registry.register(Some("TEST".into()), 0, 0).unwrap();
    book.submit(Command::Add {
        order: limit(1, Side::Buy, 100, 5),
    })
    .unwrap();
    book.submit(Command::Add {
        order: limit(2, Side::Sell, 101, 5),
    })
    .unwrap();
    let mut adapter = OrderMessages::new("TEST").unwrap();
    feed(&mut adapter, &book.snapshot());
    adapter
        .simulate(lobo_models::orders::order_types::LimitOrder::new(
            Some(lobo_primitives::Price64::from(100u32)),
            3,
            Uuid::nil(),
            Side::Buy,
        ))
        .unwrap();
    let mut messages = registry.events.subscribe();
    book.submit(Command::Fill {
        order: Order::Market(MarketOrder {
            fields: fields(3, Side::Sell, 7),
        }),
    })
    .unwrap();
    feed(&mut adapter, &messages.try_recv().unwrap());
    let branch = adapter.state().simulation.as_ref().unwrap();
    assert_eq!(branch.report.filled, 2);
    assert_eq!(branch.remaining(), 1);
    assert_eq!(book.native.lock().order_storage.bids.visible_quantity, 0);
    book.submit(Command::Fill {
        order: limit(4, Side::Sell, 100, 1),
    })
    .unwrap();
    feed(&mut adapter, &messages.try_recv().unwrap());
    assert!(adapter.state().simulation.as_ref().unwrap().complete());
    adapter.return_to_main();
    assert_eq!(
        adapter
            .selected_book()
            .unwrap()
            .visible_quantity(Side::Sell),
        6
    );
}

#[test]
fn concurrent_producers_publish_in_the_same_order_as_native_mutations() {
    let registry = registry();
    let book = registry.register(Some("TEST".into()), 0, 0).unwrap();
    let mut events = registry.events.subscribe();
    std::thread::scope(|scope| {
        for producer in 0..4 {
            let book = book.clone();
            scope.spawn(move || {
                for index in 0..100 {
                    book.submit(Command::Add {
                        order: limit(1 + producer * 100 + index, Side::Buy, 100, 1),
                    })
                    .unwrap();
                }
            });
        }
    });
    for expected in 1..=400 {
        match &*events.try_recv().unwrap() {
            FeedMessage::Update { sequence, .. } => assert_eq!(*sequence, expected),
            _ => panic!("expected update"),
        }
    }
    assert_eq!(book.native.lock().order_storage.bids.visible_quantity, 400);
}

#[test]
fn snapshot_reconstruction_preserves_replenished_fifo_and_original_timestamps() {
    use lobo_replay::order_messages::ApplyOrderCommand;
    use lobo_books::price_time_priority::CommandResult;
    use lobo_models::events::Reports;
    use lobo_server::registry::NativeBook;

    let registry = registry();
    let book = registry.register(Some("TEST".into()), 0, 0).unwrap();
    book.submit(Command::Add {
        order: Order::Iceberg(lobo_models::server::IcebergOrder {
            fields: fields(1, Side::Sell, 3),
            price: 100,
            hidden_quantity: 6,
            peak_quantity: 3,
        }),
    })
    .unwrap();
    book.submit(Command::Add {
        order: limit(2, Side::Sell, 100, 4),
    })
    .unwrap();
    let original_time = book
        .native
        .lock()
        .order_storage
        .order(Uuid::from_u128(1))
        .unwrap()
        .common_data
        .creation_time
        .timestamp_nanos_opt()
        .unwrap() as u64;
    // Exercise the matching engine's replenishment, not just the Execute adapter.
    book.submit(Command::Fill {
        order: Order::Market(MarketOrder {
            fields: fields(3, Side::Buy, 3),
        }),
    })
    .unwrap();
    let FeedMessage::Snapshot { orders, .. } = book.snapshot() else {
        panic!("snapshot expected");
    };
    assert_eq!(
        orders
            .iter()
            .map(|row| row.order.fields().id)
            .collect::<Vec<_>>(),
        [Uuid::from_u128(2), Uuid::from_u128(1)]
    );
    assert_eq!(orders[1].timestamp_ns, original_time);
    let mut reconstructed: NativeBook = NativeBook::new();
    for row in orders {
        Command::Add { order: row.order }
            .apply(&mut reconstructed, row.timestamp_ns, Reports::default())
            .unwrap();
    }
    let command = Command::Fill {
        order: Order::Market(MarketOrder {
            fields: fields(4, Side::Buy, 6),
        }),
    };
    let native = book.submit(command.clone()).unwrap();
    let CommandResult::Filled {
        report: Some(report),
        ..
    } = command
        .apply(
            &mut reconstructed,
            original_time + 1,
            Reports {
                include_fills: true,
                ..Reports::default()
            },
        )
        .unwrap()
    else {
        panic!("fills expected");
    };
    assert_eq!(
        native
            .executions
            .iter()
            .map(|fill| (fill.maker_id, fill.quantity))
            .collect::<Vec<_>>(),
        report
            .fills
            .unwrap()
            .iter()
            .map(|fill| (fill.maker_order_id, fill.fill_quantity))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        reconstructed.order_storage.asks.visible_quantity,
        book.native.lock().order_storage.asks.visible_quantity
    );
    assert_eq!(
        reconstructed.order_storage.asks.hidden_quantity,
        book.native.lock().order_storage.asks.hidden_quantity
    );
}

#[test]
#[cfg(feature = "feed-tests")]
fn duplicate_snapshots_do_not_rewind_and_reconnect_replaces_the_directory() {
    let registry = registry();
    let book = registry.register(Some("OLD".into()), 0, 0).unwrap();
    let old_snapshot = book.snapshot();
    let mut adapter = OrderMessages::new("OLD").unwrap();
    feed(&mut adapter, &old_snapshot);
    let mut events = registry.events.subscribe();
    book.submit(Command::Add {
        order: limit(1, Side::Buy, 100, 5),
    })
    .unwrap();
    feed(&mut adapter, &events.try_recv().unwrap());
    feed(&mut adapter, &old_snapshot);
    assert_eq!(
        adapter.selected_book().unwrap().visible_quantity(Side::Buy),
        5
    );
    adapter.disconnected();
    feed(
        &mut adapter,
        &FeedMessage::Directory {
            books: vec![lobo_models::server::BookInfo {
                symbol: "NEW".into(),
                price_decimals: 0,
                quantity_decimals: 0,
                policy: Default::default(),
            }],
        },
    );
    assert_eq!(adapter.tickers(), ["NEW"]);
    assert_eq!(adapter.state().selected, "NEW");
}

#[test]
#[cfg(feature = "feed-tests")]
fn live_amendments_keep_counterfactual_consumption_and_cross_changed_prices() {
    use lobo_models::orders::traits::Trades;
    let registry = registry();
    let book = registry.register(Some("TEST".into()), 0, 0).unwrap();
    for order in [
        limit(1, Side::Buy, 100, 5),
        limit(2, Side::Buy, 100, 10),
        limit(3, Side::Sell, 101, 4),
    ] {
        book.submit(Command::Add { order }).unwrap();
    }
    let mut adapter = OrderMessages::new("TEST").unwrap();
    feed(&mut adapter, &book.snapshot());
    adapter
        .simulate(lobo_models::orders::order_types::LimitOrder::new(
            Some(lobo_primitives::Price64::from(100u32)),
            3,
            Uuid::nil(),
            Side::Buy,
        ))
        .unwrap();
    let mut messages = registry.events.subscribe();
    // The wire execution names maker 2; the counterfactual fills native FIFO.
    book.submit(Command::Execute {
        id: Uuid::from_u128(2),
        quantity: 2,
        price: None,
    })
    .unwrap();
    feed(&mut adapter, &messages.try_recv().unwrap());
    book.submit(Command::Modify {
        id: Uuid::from_u128(1),
        quantity: 4,
        price: None,
        new_id: None,
    })
    .unwrap();
    feed(&mut adapter, &messages.try_recv().unwrap());
    assert_eq!(
        adapter
            .state_mut()
            .simulation
            .as_mut()
            .unwrap()
            .book_mut()
            .order(Uuid::from_u128(1))
            .unwrap()
            .quantity(),
        2
    );
    // Remove real bids so a changed ask crosses only the simulated bid.
    for id in [1, 2] {
        book.submit(Command::Remove {
            id: Uuid::from_u128(id),
        })
        .unwrap();
        feed(&mut adapter, &messages.try_recv().unwrap());
    }
    book.submit(Command::Modify {
        id: Uuid::from_u128(3),
        quantity: 4,
        price: Some(100),
        new_id: None,
    })
    .unwrap();
    feed(&mut adapter, &messages.try_recv().unwrap());
    let branch = adapter.state().simulation.as_ref().unwrap();
    assert!(branch.complete());
    assert_eq!(branch.report.filled, 3);
    assert_eq!(book.native.lock().order_storage.asks.visible_quantity, 4);
}

#[test]
#[cfg(feature = "feed-tests")]
fn mixed_policies_preserve_native_snapshots_and_iceberg_simulations() {
    use lobo_models::BookPolicy;
    use lobo_primitives::Price64;
    let registry = registry();
    let mut adapter = OrderMessages::new("POLICY0").unwrap();
    for (index, policy) in [
        BookPolicy::Full,
        BookPolicy::NoUserMap,
        BookPolicy::NoHiddenQuantity,
        BookPolicy::NoUpdates,
    ]
    .into_iter()
    .enumerate()
    {
        let hidden = matches!(policy, BookPolicy::Full | BookPolicy::NoUserMap);
        let symbol = format!("POLICY{index}");
        let book = registry
            .register_policy(Some(symbol.clone()), 0, 0, policy)
            .unwrap();
        feed(&mut adapter, &book.snapshot());
        adapter.select_ticker(&symbol).unwrap();
        let mut messages = registry.events.subscribe();
        for order in [
            Order::Iceberg(lobo_models::server::IcebergOrder {
                fields: fields(1, Side::Sell, 3),
                price: 100,
                hidden_quantity: 6,
                peak_quantity: 3,
            }),
            limit(2, Side::Sell, 100, 4),
        ] {
            book.submit(Command::Add { order }).unwrap();
            feed(&mut adapter, &messages.try_recv().unwrap());
        }
        assert_eq!(adapter.selected_book().unwrap().policy(), policy);
        let before = serde_json::to_value(book.snapshot()).unwrap()["orders"].clone();
        adapter
            .simulate_market(lobo_models::orders::order_types::MarketOrder::new(
                15,
                Uuid::nil(),
                Side::Buy,
            ))
            .unwrap();
        let preview = adapter.state().market_preview.as_ref().unwrap();
        assert_eq!(preview.filled, if hidden { 13 } else { 7 });
        assert_eq!(preview.average_price(), Some(100.0));
        adapter.return_to_main();
        // Initial hypothetical fills cross multiple peaks, then rest without
        // matching the remainder twice. Only the fork changes.
        adapter
            .simulate(lobo_models::orders::order_types::LimitOrder::new(
                Some(Price64::from(100u32)),
                15,
                Uuid::nil(),
                Side::Buy,
            ))
            .unwrap();
        let branch = adapter.state().simulation.as_ref().unwrap();
        assert_eq!(branch.report.filled, if hidden { 13 } else { 7 });
        assert_eq!(branch.remaining(), if hidden { 2 } else { 8 });
        assert_eq!(
            adapter
                .selected_book()
                .unwrap()
                .visible_quantity(Side::Sell),
            0
        );
        assert_eq!(
            serde_json::to_value(book.snapshot()).unwrap()["orders"],
            before
        );
        adapter.return_to_main();
        for command in [
            Command::Execute {
                id: Uuid::from_u128(1),
                quantity: 3,
                price: None,
            },
            Command::Fill {
                order: Order::Market(MarketOrder {
                    fields: fields(3, Side::Buy, if hidden { 6 } else { 3 }),
                }),
            },
            Command::Modify {
                id: Uuid::from_u128(if hidden { 1 } else { 2 }),
                quantity: 2,
                price: None,
                new_id: None,
            },
            Command::Cancel {
                id: Uuid::from_u128(if hidden { 1 } else { 2 }),
                quantity: 2,
            },
            Command::Fill {
                order: Order::Market(MarketOrder {
                    fields: fields(4, Side::Buy, 4),
                }),
            },
        ] {
            book.submit(command).unwrap();
            feed(&mut adapter, &messages.try_recv().unwrap());
            let FeedMessage::Snapshot { orders, .. } = book.snapshot() else {
                panic!("snapshot");
            };
            let queue = adapter
                .selected_book()
                .unwrap()
                .queue_view(Side::Sell, Price64::from(0u32)..=Price64::from(u32::MAX));
            assert_eq!(
                queue.iter().map(|o| (o.id, o.quantity)).collect::<Vec<_>>(),
                orders
                    .iter()
                    .map(|o| (o.order.fields().id, o.order.fields().quantity))
                    .collect::<Vec<_>>()
            );
            for row in orders {
                assert!(
                    adapter
                        .selected_book()
                        .unwrap()
                        .order(row.order.fields().id)
                        .is_some()
                );
            }
        }
    }
    assert_eq!(adapter.tickers().len(), 4);
    // Reconnect may point to a new server using another concrete policy for
    // the same symbol. Invalidation must allow its fresh snapshot to load.
    adapter.disconnected();
    let replacement = self::registry()
        .register_policy(Some("POLICY0".into()), 0, 0, BookPolicy::NoUpdates)
        .unwrap();
    feed(
        &mut adapter,
        &FeedMessage::Directory {
            books: vec![replacement.info().clone()],
        },
    );
    feed(&mut adapter, &replacement.snapshot());
    assert_eq!(
        adapter.selected_book().unwrap().policy(),
        BookPolicy::NoUpdates
    );
}
