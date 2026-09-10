use lobo_models::Side;
use lobo_primitives::PriceType;
use lobo_replay::feed::FeedState;
use lobo_storage::price_level::{HasHiddenQuantity, PriceLevelContract};
use serde_json::{Value, json};
pub fn state(state: &FeedState) -> Value {
    let books = state.instruments.keys().map(|symbol| {
        let book = state.book(symbol).map(|book| {
            let levels = lobo_replay::dispatch_feed_book!(book, native, native.order_storage.bids.visible_price_levels().chain(native.order_storage.asks.visible_price_levels()).map(|(p,l)| json!([p.into_u128().to_string(),l.side() as u8,l.visible_quantity(),l.hidden_quantity(),l.len()])).collect::<Vec<_>>());
            let orders = [Side::Buy,Side::Sell].into_iter().flat_map(|side|book.queue_view(side,0u64.into()..=u64::MAX.into())).map(|o|format!("{o:?}")).collect::<Vec<_>>();
            json!({"policy":format!("{:?}",book.policy()),"levels":levels,"orders":orders,"sequence":book.sequence()})
        });
        let output = state.outputs[symbol].borrow();
        json!({"symbol":symbol,"book":book,"sync":output.synchronized,"generation":output.generation,"count":output.count,"events":output.pending.values().map(|e|format!("{e:?}")).collect::<Vec<_>>()})
    }).collect::<Vec<_>>();
    let simulation = state.simulation.as_ref().map(|branch| json!({"state":self::state(&branch.feed),"filled":branch.report.filled,"remaining":branch.remaining(),"ignored":branch.ignored,"stopped":branch.stopped()}));
    json!({"books":books,"selected":state.selected,"clock":state.clock_ns,"start":state.start_ns,"warming":state.warming,"complete":state.complete,"input":state.needs_input,"consumed":state.consumed,"messages":state.messages,"checks":state.checksum_checks,"failures":state.checksum_failures,"bars":state.volume_bars().borrow().destination.0.borrow().iter().map(|e|format!("{e:?}")).collect::<Vec<_>>(),"simulation":simulation,"preview":state.market_preview.as_ref().map(|r|json!([r.filled,r.requested]))})
}

/// Compare observable liquidity, FIFO, execution bars and simulation outcomes.
/// Input counters and frame generations depend on each protocol's framing.
pub fn market(state: &FeedState) -> Value {
    let mut value = self::state(state);
    for key in ["input", "warming", "messages", "consumed"] {
        value.as_object_mut().unwrap().remove(key);
    }
    for entry in value["books"].as_array_mut().unwrap() {
        for key in ["generation", "count", "events"] {
            entry.as_object_mut().unwrap().remove(key);
        }
        if let Some(book) = entry["book"].as_object_mut() {
            book.remove("sequence");
        }
    }
    if let Some(branch) = &state.simulation {
        value["simulation"]["state"] = market(&branch.feed);
    }
    value
}
