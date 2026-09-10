use lobo_batchers::{VolumeBars, traits::Sink};
use lobo_events::{BookEvent, TradedVolumeEvent, VolumeBar};
use lobo_models::Side;
use lobo_primitives::Price64;
use std::{convert::Infallible, num::NonZeroU64, sync::Arc};
#[derive(Default)]
struct Output(Vec<BookEvent<VolumeBar<Price64>>>);
impl Sink<BookEvent<VolumeBar<Price64>>> for Output {
    type SinkResult = ();
    type SinkError = Infallible;
    fn write(&mut self, event: &BookEvent<VolumeBar<Price64>>) -> Result<(), Infallible> {
        self.0.push(event.clone());
        Ok(())
    }
    fn flush(&mut self) -> Result<(), Infallible> {
        Ok(())
    }
    fn finish(&mut self) -> Result<(), Infallible> {
        Ok(())
    }
}
fn trade(
    symbol: &str,
    sequence: u64,
    price: u64,
    quantity: u64,
) -> BookEvent<TradedVolumeEvent<Price64>> {
    BookEvent::from_event(
        TradedVolumeEvent {
            price: price.into(),
            quantity,
            side: Side::Buy,
        },
        sequence,
        Arc::from(symbol),
    )
}
#[test]
fn split_trades_preserve_volume_ohlc_and_book_isolation() {
    let mut sink = VolumeBars::new(NonZeroU64::new(10).unwrap(), Output::default());
    sink.write(&trade("A", 1, 100, 4)).unwrap();
    sink.write(&trade("B", 1, 50, 3)).unwrap();
    sink.write(&trade("A", 2, 105, 3)).unwrap();
    sink.write(&trade("A", 3, 95, 26)).unwrap();
    let bars = &sink.destination.0;
    assert_eq!(bars.len(), 3);
    assert_eq!(
        *bars[0].event(),
        VolumeBar {
            open: 100u64.into(),
            high: 105u64.into(),
            low: 95u64.into(),
            close: 95u64.into(),
            volume: 10,
            index: 0,
            ticks: 3,
            start_ns: 0,
            end_ns: 0,
        }
    );
    assert_eq!(bars[1].event().open, 95u64.into());
    assert_eq!(bars[2].event().index, 2);
    assert!(
        bars.iter().all(|bar| bar.event().volume == 10
            && bar.book_id() == "A"
            && bar.sequence_number() == 3)
    );
    assert_eq!(sink.forming("A").unwrap().volume, 3);
    assert_eq!(sink.forming("B").unwrap().volume, 3);
    <VolumeBars<_, _> as Sink<BookEvent<TradedVolumeEvent<Price64>>>>::finish(&mut sink).unwrap();
    assert_eq!(sink.destination.0.len(), 3); // incomplete bars are not completed output
}
#[cfg(feature = "feather")]
#[test]
fn completed_messages_batch_into_feather() {
    use lobo_batchers::{
        arrow::{sink::FeatherSink, volume::VolumeBarArrowBatcher},
        traits::BatchedDestination,
    };
    let batcher = VolumeBarArrowBatcher::new(2);
    let feather = FeatherSink::try_new(Vec::new(), &batcher.schema()).unwrap();
    let mut bars = VolumeBars::new(
        NonZeroU64::new(10).unwrap(),
        BatchedDestination {
            batcher,
            destination: feather,
        },
    );
    bars.write(&trade("A", 1, 100, 35).at_timestamp(42))
        .unwrap();
    <VolumeBars<_, _> as Sink<BookEvent<TradedVolumeEvent<Price64>>>>::finish(&mut bars).unwrap();
    let bytes = bars.destination.destination.get_ref();
    let reader = arrow_ipc::reader::FileReader::try_new(std::io::Cursor::new(bytes), None).unwrap();
    let batches: Vec<_> = reader.map(Result::unwrap).collect();
    assert_eq!(batches.iter().map(|b| b.num_rows()).sum::<usize>(), 3);
    assert_eq!(batches[0].schema().field(3).name(), "open");
    assert_eq!(batches[1].num_rows(), 1);
    for name in ["ticks", "start_ns", "end_ns"] {
        let column = batches[1]
            .column_by_name(name)
            .unwrap()
            .as_any()
            .downcast_ref::<arrow_array::UInt64Array>()
            .unwrap();
        assert_eq!(column.value(0), if name == "ticks" { 1 } else { 42 });
    }
}

#[test]
fn time_buckets_use_source_time_close_at_boundaries_and_skip_empty_intervals() {
    use lobo_batchers::Aggregation;
    let mut bars = VolumeBars::new(
        Aggregation::Time(NonZeroU64::new(10).unwrap()),
        Output::default(),
    );
    bars.write(&trade("A", 1, 100, 2).at_timestamp(12)).unwrap();
    bars.write(&trade("B", 1, 50, 3).at_timestamp(13)).unwrap();
    bars.write(&trade("A", 2, 110, 5).at_timestamp(19)).unwrap();
    assert!(bars.destination.0.is_empty());
    bars.write(&trade("A", 3, 90, 1).at_timestamp(20)).unwrap();
    let first = &bars.destination.0[0];
    assert_eq!((first.event().start_ns, first.event().end_ns), (10, 20));
    assert_eq!((first.event().volume, first.event().ticks), (7, 2));
    assert_eq!(
        (first.event().open, first.event().close),
        (100u64.into(), 110u64.into())
    );
    assert_eq!((first.sequence_number(), first.timestamp_ns()), (2, 19));
    bars.advance_time(20).unwrap(); // closes B without another B trade
    assert_eq!(bars.completed("B"), 1);
    bars.advance_time(50).unwrap(); // closes A's [20,30), no artificial empty bars
    assert_eq!(bars.completed("A"), 2);
    assert_eq!(bars.destination.0.len(), 3);
    bars.advance_time(100).unwrap();
    assert_eq!(bars.destination.0.len(), 3);
    bars.write(&trade("A", 4, 120, 1).at_timestamp(102))
        .unwrap();
    assert_eq!(bars.forming("A").unwrap().start_ns, 100);
    assert_eq!(bars.progress("A", 105), 5.0);
}

#[test]
fn tick_bars_count_executions_not_quantity_and_ignore_zero_volume() {
    use lobo_batchers::Aggregation;
    let mut bars = VolumeBars::new(
        Aggregation::Ticks(NonZeroU64::new(3).unwrap()),
        Output::default(),
    );
    bars.write(&trade("A", 1, 100, 1000).at_timestamp(1))
        .unwrap();
    bars.write(&trade("A", 2, 1, 0).at_timestamp(2)).unwrap();
    bars.write(&trade("A", 3, 90, 1).at_timestamp(3)).unwrap();
    assert_eq!(bars.progress("A", 3), 2.0);
    bars.write(&trade("A", 4, 110, 2).at_timestamp(4)).unwrap();
    let bar = bars.destination.0[0].event();
    assert_eq!(
        (bar.volume, bar.ticks, bar.start_ns, bar.end_ns),
        (1003, 3, 1, 4)
    );
    assert_eq!(
        (bar.open, bar.high, bar.low, bar.close),
        (100u64.into(), 110u64.into(), 90u64.into(), 110u64.into())
    );
    assert!(bars.forming("A").is_none());
}

#[test]
fn notional_uses_fixed_point_execution_value_and_includes_the_crossing_execution() {
    use lobo_batchers::Aggregation;
    use std::num::NonZeroU128;
    let mut bars = VolumeBars::new(
        Aggregation::Notional(NonZeroU64::new(1000).unwrap()),
        Output::default(),
    );
    bars.set_notional_scale("A", NonZeroU128::new(100).unwrap());
    bars.set_notional_scale("B", NonZeroU128::new(10000).unwrap());
    bars.write(&trade("A", 1, 4250, 10)).unwrap(); // 425 quote units
    bars.write(&trade("B", 1, 425000, 10)).unwrap();
    assert_eq!(bars.progress("A", 0), 425.0);
    assert_eq!(bars.progress("B", 0), 425.0);
    bars.write(&trade("A", 2, 4300, 20)).unwrap(); // 1,285 total, whole execution closes
    assert_eq!(bars.destination.0.len(), 1);
    let bar = bars.destination.0[0].event();
    assert_eq!((bar.volume, bar.ticks), (30, 2));
    assert_eq!((bar.open, bar.close), (4250u64.into(), 4300u64.into()));
    assert_eq!(bars.completed("B"), 0);
    assert!(bars.forming("A").is_none());
    bars.write(&trade("A", 3, 1000, 1)).unwrap();
    assert_eq!(bars.progress("A", 0), 10.0); // no fabricated spill into next bar
}

#[test]
fn changing_aggregation_resets_partial_bars_and_preserves_quote_and_scale_metadata() {
    use lobo_batchers::Aggregation;
    use lobo_events::PriceChangeEvent;
    let mut bars = VolumeBars::new(NonZeroU64::new(10).unwrap(), Output::default());
    bars.set_notional_scale("A", std::num::NonZeroU128::new(100).unwrap());
    let quote = PriceChangeEvent {
        price: Some(100u64.into()),
        quantity: 5,
        number_of_orders: 1,
        side: Side::Buy,
    };
    bars.write(&BookEvent::from_event(quote, 1, Arc::from("A")))
        .unwrap();
    bars.write(&trade("A", 2, 100, 5)).unwrap();
    bars.reset(Aggregation::Notional(NonZeroU64::new(10).unwrap()));
    assert!(bars.forming("A").is_none());
    assert_eq!(bars.quotes("A")[Side::Buy as usize], Some(quote));
    bars.write(&trade("A", 3, 100, 5)).unwrap();
    assert_eq!(bars.progress("A", 0), 5.0);
    assert!(bars.destination.0.is_empty());
    bars.write(&trade("A", 4, 100, 5)).unwrap();
    assert_eq!(bars.destination.0[0].event().volume, 10);
    assert_eq!(bars.destination.0[0].event().index, 0);
}
