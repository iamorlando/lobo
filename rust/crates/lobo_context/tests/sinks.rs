use arrow_array::UInt64Array;
use arrow_ipc::reader::FileReader;
use lobo_batchers::arrow::{batch::PriceLevelArrowBatcher, sink::FeatherSink};
use lobo_context::{AsyncSink, BatchSink, MpscSinks, NullSink, Sink};
use lobo_events::{BookEvent, PriceLevelChangeEvent, PublisherFactory, Receiver};
use lobo_models::Side;
use lobo_primitives::Price64;
use std::{
    io::{self, Cursor, Write},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::mpsc, time::timeout};

#[derive(Clone, Default)]
struct Bytes(Arc<Mutex<Vec<u8>>>);
impl Write for Bytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn null_sink_needs_no_runtime() {
    let (publisher, _) = <NullSink as Sink<u32>>::connect(NullSink).unwrap();
    publisher.publisher(|_| panic!("null callback must not run"))(3_u32);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feather_receives_full_batches_during_production_and_flushes_the_tail() {
    let bytes = Bytes::default();
    let batcher = PriceLevelArrowBatcher::new(2);
    let feather = FeatherSink::try_new(bytes.clone(), &batcher.schema()).unwrap();
    let initial_bytes = bytes.0.lock().unwrap().len();
    let (written_tx, mut written_rx) = mpsc::unbounded_channel();
    let sinks = MpscSinks::new().with(BatchSink::with_output(batcher, feather, move |()| {
        written_tx.send(()).unwrap();
    }));
    let (publisher, tasks) = sinks.connect().unwrap();
    let send = |sequence| {
        publisher.send(BookEvent::from_event(
            PriceLevelChangeEvent::new(sequence * 10, 0, Price64::from(100_u32), 1, Side::Buy),
            sequence,
            "ABC".into(),
        ))
    };
    send(1);
    send(2);
    // The producer remains alive and can send more. Full batches reach IO now.
    timeout(Duration::from_secs(5), written_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert!(bytes.0.lock().unwrap().len() > initial_bytes);
    send(3);
    drop(publisher);
    timeout(Duration::from_secs(5), tasks.finish())
        .await
        .unwrap()
        .unwrap();
    let data = bytes.0.lock().unwrap().clone();
    let batches = FileReader::try_new(Cursor::new(data), None)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        batches.iter().map(|b| b.num_rows()).collect::<Vec<_>>(),
        [2, 1]
    );
    let sequences = batches
        .iter()
        .flat_map(|batch| {
            batch
                .column(0)
                .as_any()
                .downcast_ref::<UInt64Array>()
                .unwrap()
                .values()
                .to_vec()
        })
        .collect::<Vec<_>>();
    assert_eq!(sequences, [1, 2, 3]);
}

#[tokio::test]
async fn sink_failure_is_returned_after_other_consumers_drain() {
    let (observed_tx, mut observed_rx) = mpsc::unbounded_channel();
    let sinks = MpscSinks::new()
        .with(AsyncSink::new(|_: Receiver<u32>| async {
            Err(io::Error::other("destination failed").into())
        }))
        .with(AsyncSink::new(move |mut rx: Receiver<u32>| async move {
            while let Some(event) = rx.recv().await {
                observed_tx.send(*event.event())?;
            }
            Ok(())
        }));
    let (publisher, tasks) = sinks.connect().unwrap();
    publisher.send(BookEvent::from_event(1, 1, "ABC".into()));
    publisher.send(BookEvent::from_event(2, 2, "ABC".into()));
    drop(publisher);
    let error = timeout(Duration::from_secs(5), tasks.finish())
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.to_string(), "destination failed");
    assert_eq!(observed_rx.recv().await, Some(1));
    assert_eq!(observed_rx.recv().await, Some(2));
    assert_eq!(observed_rx.recv().await, None);
}
