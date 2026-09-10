//! Prepared binary record accessors and grouped file replay.
use super::{
    DefinedProtocol,
    schema::{Definition, Format},
};
use crate::custom::{
    AdaptForReplay, AdapterDescriptor,
    orders::*,
    source::{ApplyReplay, HashRouting, ReplayFormat},
};
use crate::{ReplayStats, feed::FeedState};
use lobo_books::price_time_priority::Book;
use lobo_models::BookPolicy;
use lobo_primitives::{
    time::{DateTime, Utc},
    uuid::Uuid,
};
use lobo_storage::{
    UserMapUpdatePolicy, policies::HiddenQuantityPolicy, price_level::PriceLevelContract,
    price_sorting::PriceSortingPolicy,
};
use std::{
    io::{BufRead, BufReader, Read},
    path::PathBuf,
    sync::Arc,
};

#[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
use super::compiled::metadata;
pub use super::compiled::{Message, Plan};
pub struct Reader {
    reader: BufReader<Box<dyn Read + Send>>,
    pub plan: Arc<Plan>,
    scratch: Vec<u8>,
}
impl Reader {
    pub fn open(path: &PathBuf, plan: Arc<Plan>) -> Result<Self, String> {
        let mut file = BufReader::new(std::fs::File::open(path).map_err(|e| e.to_string())?);
        let gzip = file
            .fill_buf()
            .map_err(|e| e.to_string())?
            .starts_with(&[0x1f, 0x8b]);
        let stream: Box<dyn Read + Send> = if gzip {
            Box::new(flate2::read::MultiGzDecoder::new(file))
        } else {
            Box::new(file)
        };
        Ok(Self {
            reader: BufReader::with_capacity(1024 * 1024, stream),
            plan,
            scratch: Vec::new(),
        })
    }
    pub fn record<R>(
        &mut self,
        visit: impl FnMut(&[u8]) -> Result<R, String>,
    ) -> Result<Option<R>, String> {
        read_record(&mut self.reader, &mut self.scratch, &self.plan, visit)
    }
}
fn read_record<R>(
    reader: &mut BufReader<Box<dyn Read + Send>>,
    scratch: &mut Vec<u8>,
    plan: &Plan,
    mut visit: impl FnMut(&[u8]) -> Result<R, String>,
) -> Result<Option<R>, String> {
    if reader.fill_buf().map_err(|e| e.to_string())?.is_empty() {
        return Ok(None);
    }
    let spec = &plan.spec;
    let prefix = spec.length.size;
    let mut bytes = [0u8; 8];
    reader
        .read_exact(&mut bytes[..prefix])
        .map_err(|e| e.to_string())?;
    #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
    let size = plan.code.length(&bytes[..prefix])?;
    #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
    let size = spec.length.number(&bytes[..prefix])? as usize;
    #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
    if size == 0 || size > spec.max_record_size {
        return Err("Invalid record length".into());
    }
    let available = reader.fill_buf().map_err(|e| e.to_string())?;
    if available.len() >= size {
        let result = visit(&available[..size]);
        reader.consume(size);
        result.map(Some)
    } else {
        scratch.resize(size, 0);
        reader.read_exact(scratch).map_err(|e| e.to_string())?;
        visit(scratch).map(Some)
    }
}
impl Iterator for Reader {
    type Item = Result<Message, String>;
    fn next(&mut self) -> Option<Self::Item> {
        let plan = &self.plan;
        match read_record(&mut self.reader, &mut self.scratch, &plan, |bytes| {
            plan.decode(bytes)
        }) {
            Ok(v) => v.map(Ok),
            Err(e) => Some(Err(e)),
        }
    }
}
#[derive(Clone)]
pub struct DirectoryEntry {
    pub instrument: crate::feed::Instrument,
    pub key: u64,
    pub policy: BookPolicy,
}
#[derive(Clone)]
pub struct FileFormat {
    pub(super) path: PathBuf,
    pub bulk_error: Option<String>,
    pub plan: Arc<Plan>,
    cutoff: u64,
    timezone: chrono_tz::Tz,
    group: (PathBuf, usize, u64),
}
impl FileFormat {
    pub fn new(path: PathBuf, definition: &Definition, timezone: &str) -> Result<Self, String> {
        let Format::Binary(binary) = &definition.format else {
            return Err(
                "run() requires a binary file definition; use start() for streaming sources".into(),
            );
        };
        let spec = Arc::new(binary.clone());
        let (plan, bulk_error) = match Plan::new(spec.clone()) {
            Ok(plan) => (Arc::new(plan), None),
            Err(error) => (Arc::new(Plan::metadata_only(spec)?), Some(error)),
        };
        let group = (path.clone(), Arc::as_ptr(&plan) as usize, u64::MAX);
        Ok(Self {
            path,
            plan,
            bulk_error,
            cutoff: u64::MAX,
            timezone: timezone
                .parse()
                .map_err(|_| "Timezone must be an IANA timezone")?,
            group,
        })
    }
    pub fn directory(
        &self,
        definition: Arc<Definition>,
        descriptor: AdapterDescriptor,
    ) -> Result<Vec<DirectoryEntry>, String> {
        let mut protocol =
            DefinedProtocol::new(definition, descriptor.clone(), &descriptor.default_symbol)?;
        let mut state = FeedState::new(&descriptor.default_symbol)?;
        let mut reader = Reader::open(&self.path, self.plan.clone())?;
        #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
        let binary = self.plan.spec.clone();
        #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
        let program = protocol.program.clone();
        while let Some(()) = reader.record(|bytes| {
            #[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
            {
                program.directory(
                    &mut protocol,
                    &mut state,
                    bytes,
                    self.plan.code.header(bytes)?,
                )?;
            }
            #[cfg(not(all(feature = "jit", not(target_arch = "wasm32"))))]
            {
                let tag = binary.tag.number(bytes)?;
                let route = binary.key.number(bytes)?;
                protocol.vars.insert("key".into(), route.into());
                if let Some(record) = binary.records.get(&tag) {
                    let row = serde_json::Value::Object(
                        record
                            .fields
                            .iter()
                            .map(|(name, field)| field.value(bytes).map(|v| (name.clone(), v)))
                            .collect::<Result<_, _>>()?,
                    );
                    let actions = record
                        .actions
                        .iter()
                        .filter(|a| metadata(a))
                        .cloned()
                        .collect::<Vec<_>>();
                    protocol.actions(&actions, &mut state, &row, &row)?;
                }
            }
            Ok(())
        })? {
            if protocol.directory_complete {
                break;
            }
        }
        protocol
            .routes
            .into_iter()
            .map(|(key, symbol)| {
                Ok(DirectoryEntry {
                    instrument: state
                        .instruments
                        .get(&symbol)
                        .ok_or("Missing directory metadata")?
                        .clone(),
                    key,
                    policy: protocol.policies[&symbol],
                })
            })
            .collect()
    }
}
#[derive(Default, Clone, serde::Serialize)]
pub struct Stats {
    pub source_messages: u64,
    pub security_messages: u64,
    pub replay_messages: u64,
    pub replay_events: u64,
    pub adds: u64,
    pub executes: u64,
    pub cancels: u64,
    pub deletes: u64,
    pub replaces: u64,
}
impl ReplayStats<Message> for Stats {
    #[inline(always)]
    fn record_source_message(&mut self, _: &Message) {
        self.source_messages += 1;
    }
    #[inline(always)]
    fn record_security_message(&mut self, _: &Message) {
        self.security_messages += 1;
    }
    #[inline(always)]
    fn record_replay_event(&mut self) {
        self.replay_events += 1;
    }
    #[inline(always)]
    fn record_replay_message(&mut self, m: &Message) {
        self.replay_messages += 1;
        match m.kind {
            1 => self.adds += 1,
            2 => self.executes += 1,
            3 => self.cancels += 1,
            4 => self.deletes += 1,
            5 => self.replaces += 1,
            _ => (),
        }
    }
    fn merge(&mut self, o: Self) {
        self.source_messages += o.source_messages;
        self.security_messages += o.security_messages;
        self.replay_messages += o.replay_messages;
        self.replay_events += o.replay_events;
        self.adds += o.adds;
        self.executes += o.executes;
        self.cancels += o.cancels;
        self.deletes += o.deletes;
        self.replaces += o.replaces;
    }
}
impl ReplayFormat for FileFormat {
    type Message = Message;
    type Error = String;
    type StreamError = String;
    type Stream = Reader;
    type Stats = Stats;
    type Key = u64;
    type Group = (PathBuf, usize, u64);
    type Routing = HashRouting<u64>;
    fn open(&self) -> Result<Reader, String> {
        Reader::open(&self.path, self.plan.clone())
    }
    fn group(&self) -> &Self::Group {
        &self.group
    }
    fn routing(&self) -> Self::Routing {
        HashRouting::default()
    }
    #[inline(always)]
    fn key(&self, message: &Message) -> u64 {
        message.key
    }
    #[inline(always)]
    fn includes(&self, message: &Message) -> bool {
        message.kind != 0
    }
    fn with_cutoff(mut self, cutoff: Option<DateTime<Utc>>) -> Self {
        if let Some(c) = cutoff {
            use chrono::Timelike;
            let c = c.with_timezone(&self.timezone);
            self.cutoff = u64::from(c.num_seconds_from_midnight()) * 1_000_000_000
                + u64::from(c.nanosecond());
            self.group.2 = self.cutoff;
        }
        self
    }
    #[inline(always)]
    fn before_cutoff(&self, message: &Message) -> bool {
        message.timestamp <= self.cutoff
    }
}
impl<L, S, U, H, Pub> ApplyReplay<L, S, U, H, Pub> for FileFormat
where
    L: PriceLevelContract,
    S: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
    Pub: lobo_events::BookPublisherFactory<L::Price>,
{
    fn apply(&self, m: Message, book: &mut Book<L, S, U, H, Pub>) -> bool {
        let id = Uuid::from_u128(u128::from(m.id));
        let result = match m.kind {
            1 => {
                let Ok(price) = L::Price::try_from(m.price) else {
                    return false;
                };
                let Some(side) = m.side else {
                    return false;
                };
                AddOrder {
                    timestamp: m.timestamp,
                    id,
                    side,
                    price,
                    quantity: m.quantity,
                }
                .process(book)
            }
            2 => {
                let price = if m.execution_price {
                    let Ok(price) = L::Price::try_from(m.price) else {
                        return false;
                    };
                    Some(price)
                } else {
                    None
                };
                ExecuteOrder {
                    timestamp: m.timestamp,
                    id,
                    price,
                    quantity: m.quantity,
                }
                .process(book)
            }
            3 => CancelOrder {
                timestamp: m.timestamp,
                id,
                quantity: m.quantity,
            }
            .process(book),
            4 => {
                let (storage, mut publish) = book.storage_and_publisher_at(m.timestamp);
                storage
                    .remove_and_return_order(id, &mut publish)
                    .map(|_| ())
            }
            5 => {
                let Ok(price) = L::Price::try_from(m.price) else {
                    return false;
                };
                ReplaceOrder {
                    timestamp: m.timestamp,
                    id,
                    new_id: Uuid::from_u128(u128::from(m.new_id)),
                    quantity: m.quantity,
                    price,
                }
                .process(book)
            }
            _ => return false,
        };
        result.is_ok()
    }
}
