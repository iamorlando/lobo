//! Book-owned checksum policies. Mutation hooks only invalidate cached inputs;
//! adapters finish the checksum at a complete message boundary.
mod encoding;
use crate::{
    OrderStorage, UserMapUpdatePolicy, arena::arenav1::Arena, policies::HiddenQuantityPolicy,
    price_level::PriceLevelContract, price_sorting::PriceSortingPolicy,
};
pub use encoding::{Field, NumberFormat};
pub use lobo_models::CheckSum;
use lobo_models::{
    Side,
    orders::{core::RestingOrder, traits::Trades},
};
use lobo_primitives::PriceType;
use std::{marker::PhantomData, sync::Arc};

pub type Error = &'static str;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Precision {
    pub price: u8,
    pub quantity: u8,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DecimalWidths {
    pub price: u32,
    pub quantity: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum View {
    Levels,
    Orders,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Priority {
    Fifo,
    Id,
}

/// A checksum's byte layout, independent of the adapter supplying messages.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Specification {
    pub view: View,
    pub depth: usize,
    pub sides: Vec<Side>,
    pub fields: Vec<Field>,
    pub format: NumberFormat,
    pub priority: Priority,
    pub separator: String,
    pub interleave: bool,
    pub signed: bool,
}

impl Specification {
    pub fn kraken() -> Self {
        Self {
            view: View::Levels,
            depth: 10,
            sides: vec![Side::Sell, Side::Buy],
            fields: vec![Field::Price, Field::Quantity],
            format: NumberFormat::DecimalDigits,
            priority: Priority::Fifo,
            separator: String::new(),
            interleave: false,
            signed: false,
        }
    }
    pub fn bitfinex() -> Self {
        Self {
            view: View::Orders,
            depth: 25,
            sides: vec![Side::Buy, Side::Sell],
            fields: vec![Field::Id, Field::SignedQuantity],
            format: NumberFormat::EcmaScript,
            priority: Priority::Id,
            separator: ":".into(),
            interleave: true,
            signed: true,
        }
    }
    pub fn policy(&self) -> CheckSum {
        match self.view {
            View::Levels => CheckSum::Kraken,
            View::Orders => CheckSum::BitFinex,
        }
    }
    pub fn prepare(self) -> Result<Arc<Prepared>, Error> {
        if self.depth == 0 {
            return Err("Checksum depth must be positive");
        }
        let writers = self
            .fields
            .iter()
            .map(|&field| encoding::writer(field, self.format))
            .collect();
        Ok(Arc::new(Prepared {
            specification: self,
            writers,
        }))
    }
}

/// Field encoders are selected once, before input processing begins.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub specification: Specification,
    writers: Vec<encoding::Writer>,
}

impl Default for Prepared {
    fn default() -> Self {
        let spec = Specification::kraken();
        let writers = spec
            .fields
            .iter()
            .map(|&field| encoding::writer(field, spec.format))
            .collect();
        Self {
            specification: spec,
            writers,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Input {
    id: u128,
    price: u128,
    quantity: u64,
    side: Side,
    widths: DecimalWidths,
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct Fragments {
    bytes: Vec<u8>,
    ends: Vec<usize>,
}
impl Fragments {
    fn clear(&mut self) {
        self.bytes.clear();
        self.ends.clear();
    }
    fn len(&self) -> usize {
        self.ends.len()
    }
    fn get(&self, index: usize) -> &[u8] {
        let start = if index == 0 { 0 } else { self.ends[index - 1] };
        &self.bytes[start..self.ends[index]]
    }
    fn push(&mut self, bytes: &[u8]) {
        self.bytes.extend_from_slice(bytes);
        self.ends.push(self.bytes.len());
    }
}

#[derive(Clone)]
pub struct LevelCache {
    dirty: bool,
    generation: u64,
    widths: Option<DecimalWidths>,
    inputs: Vec<Input>,
    encoded: Fragments,
}
impl Default for LevelCache {
    fn default() -> Self {
        Self {
            dirty: true,
            generation: 0,
            widths: None,
            inputs: Vec::new(),
            encoded: Fragments::default(),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct SideCache {
    current: Fragments,
    scratch: Fragments,
}
impl SideCache {
    pub(crate) fn begin(&mut self) {
        self.scratch.clear();
    }
    pub(crate) fn len(&self) -> usize {
        self.scratch.len()
    }
    pub(crate) fn append(&mut self, level: &LevelCache, depth: usize) {
        let count = level.encoded.len().min(depth - self.scratch.len());
        for i in 0..count {
            self.scratch.push(level.encoded.get(i));
        }
    }
    pub(crate) fn finish(&mut self) -> bool {
        let changed = self.current != self.scratch;
        std::mem::swap(&mut self.current, &mut self.scratch);
        changed
    }
}

#[derive(Clone)]
pub struct State {
    prepared: Arc<Prepared>,
    precision: Precision,
    bids: SideCache,
    asks: SideCache,
    hasher: crc32fast::Hasher,
    value: u32,
    generation: u64,
    #[cfg(test)]
    calculations: usize,
}
impl State {
    fn new(specification: Specification) -> Self {
        Self {
            prepared: specification
                .prepare()
                .expect("constant checksum specification"),
            precision: Precision::default(),
            bids: SideCache::default(),
            asks: SideCache::default(),
            hasher: crc32fast::Hasher::new(),
            value: 0,
            generation: 1,
            #[cfg(test)]
            calculations: 0,
        }
    }
    fn finish(&mut self) -> u32 {
        self.hasher.reset();
        let spec = &self.prepared.specification;
        let mut first = true;
        let mut append = |bytes: &[u8]| {
            if !first {
                self.hasher.update(spec.separator.as_bytes());
            }
            self.hasher.update(bytes);
            first = false;
        };
        let side = |side| {
            if side == Side::Buy {
                &self.bids.current
            } else {
                &self.asks.current
            }
        };
        if spec.interleave {
            for i in 0..spec.depth {
                for &which in &spec.sides {
                    let entries = side(which);
                    if i < entries.len() {
                        append(entries.get(i));
                    }
                }
            }
        } else {
            for &which in &spec.sides {
                let entries = side(which);
                for i in 0..entries.len() {
                    append(entries.get(i));
                }
            }
        }
        self.value = self.hasher.clone().finalize();
        #[cfg(test)]
        {
            self.calculations += 1;
        }
        self.value
    }
}

/// Static hooks: the null policy has no state and inherits the empty methods.
pub trait ChecksumPolicy: Clone + Default + Send + Sync + 'static {
    type LevelState: Clone + Default + Send + Sync;
    type State: Clone + Send + Sync;
    const SPEC: CheckSum;
    fn new_state() -> Self::State;
    #[inline(always)]
    fn changed(_state: &mut Self::LevelState) {}
    #[inline(always)]
    fn wire_widths(_state: &mut Self::LevelState, _widths: DecimalWidths) {}
    fn configure(
        _state: &mut Self::State,
        _prepared: Arc<Prepared>,
        _precision: Precision,
    ) -> Result<(), Error> {
        Ok(())
    }
    #[inline(always)]
    fn calculate<L, Sort, U, H>(_storage: &mut OrderStorage<L, Sort, U, H>) -> Result<u32, Error>
    where
        L: PriceLevelContract<Checksum = Self>,
        Sort: PriceSortingPolicy,
        U: UserMapUpdatePolicy,
        H: HiddenQuantityPolicy,
    {
        Ok(0)
    }
    #[inline(always)]
    fn calculate_with<L, Sort, U, H>(
        _storage: &mut OrderStorage<L, Sort, U, H>,
        _prepared: &Arc<Prepared>,
    ) -> Result<u32, Error>
    where
        L: PriceLevelContract<Checksum = Self>,
        Sort: PriceSortingPolicy,
        U: UserMapUpdatePolicy,
        H: HiddenQuantityPolicy,
    {
        Ok(0)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NoChecksum;
impl ChecksumPolicy for NoChecksum {
    type LevelState = ();
    type State = ();
    const SPEC: CheckSum = CheckSum::Null;
    #[inline(always)]
    fn new_state() {}
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WithChecksum<S>(PhantomData<S>);
pub type Kraken = WithChecksum<Levels>;
pub type BitFinex = WithChecksum<Orders>;

pub trait Selection: Clone + Default + Send + Sync + 'static {
    const SPEC: CheckSum;
    const WIRE_WIDTHS: bool;
    fn specification() -> Specification;
    fn read<L: PriceLevelContract>(
        level: &L,
        arena: &Arena<RestingOrder<L::Price>>,
        prepared: &Prepared,
        precision: Precision,
        widths: Option<DecimalWidths>,
        output: &mut Vec<Input>,
    );
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Levels;
impl Selection for Levels {
    const SPEC: CheckSum = CheckSum::Kraken;
    const WIRE_WIDTHS: bool = true;
    fn specification() -> Specification {
        Specification::kraken()
    }
    fn read<L: PriceLevelContract>(
        level: &L,
        _arena: &Arena<RestingOrder<L::Price>>,
        _prepared: &Prepared,
        precision: Precision,
        widths: Option<DecimalWidths>,
        output: &mut Vec<Input>,
    ) {
        output.push(Input {
            id: 0,
            price: level.price().into_u128(),
            quantity: level.visible_quantity(),
            side: level.side(),
            widths: widths.unwrap_or(DecimalWidths {
                price: precision.price.into(),
                quantity: precision.quantity.into(),
            }),
        });
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Orders;
impl Selection for Orders {
    const SPEC: CheckSum = CheckSum::BitFinex;
    const WIRE_WIDTHS: bool = false;
    fn specification() -> Specification {
        Specification::bitfinex()
    }
    fn read<L: PriceLevelContract>(
        level: &L,
        arena: &Arena<RestingOrder<L::Price>>,
        prepared: &Prepared,
        precision: Precision,
        _widths: Option<DecimalWidths>,
        output: &mut Vec<Input>,
    ) {
        match prepared.specification.priority {
            Priority::Id => read_orders::<true, L>(
                level,
                arena,
                prepared.specification.depth,
                precision,
                output,
            ),
            Priority::Fifo => read_orders::<false, L>(
                level,
                arena,
                prepared.specification.depth,
                precision,
                output,
            ),
        }
    }
}

fn read_orders<const BY_ID: bool, L: PriceLevelContract>(
    level: &L,
    arena: &Arena<RestingOrder<L::Price>>,
    depth: usize,
    precision: Precision,
    output: &mut Vec<Input>,
) {
    level.for_each_order(arena, |order| {
        let input = Input {
            id: order.uuid().as_u128(),
            price: level.price().into_u128(),
            quantity: order.quantity(),
            side: level.side(),
            widths: DecimalWidths {
                price: precision.price.into(),
                quantity: precision.quantity.into(),
            },
        };
        // Constant policy choice; keep the selected prefix without changing FIFO.
        if BY_ID {
            let at = output.partition_point(|entry| entry.id < input.id);
            if at < depth {
                if output.len() == depth {
                    output.pop();
                }
                output.insert(at, input);
            }
        } else if output.len() < depth {
            output.push(input);
        }
    });
}

impl<S: Selection> ChecksumPolicy for WithChecksum<S> {
    type LevelState = LevelCache;
    type State = State;
    const SPEC: CheckSum = S::SPEC;
    fn new_state() -> State {
        State::new(S::specification())
    }
    #[inline(always)]
    fn changed(state: &mut LevelCache) {
        state.dirty = true;
    }
    #[inline(always)]
    fn wire_widths(state: &mut LevelCache, widths: DecimalWidths) {
        if S::WIRE_WIDTHS && state.widths != Some(widths) {
            state.widths = Some(widths);
            state.dirty = true;
        }
    }
    fn configure(
        state: &mut State,
        prepared: Arc<Prepared>,
        precision: Precision,
    ) -> Result<(), Error> {
        if prepared.specification.policy() != S::SPEC {
            return Err("Checksum policy and input view differ");
        }
        if precision.price > 38 || precision.quantity > 38 {
            return Err("Unsupported checksum precision");
        }
        state.prepared = prepared;
        state.precision = precision;
        state.generation += 1;
        Ok(())
    }
    fn calculate<L, Sort, U, H>(storage: &mut OrderStorage<L, Sort, U, H>) -> Result<u32, Error>
    where
        L: PriceLevelContract<Checksum = Self>,
        Sort: PriceSortingPolicy,
        U: UserMapUpdatePolicy,
        H: HiddenQuantityPolicy,
    {
        calculate::<S, L, Sort, U, H>(storage, false)
    }
    fn calculate_with<L, Sort, U, H>(
        storage: &mut OrderStorage<L, Sort, U, H>,
        prepared: &Arc<Prepared>,
    ) -> Result<u32, Error>
    where
        L: PriceLevelContract<Checksum = Self>,
        Sort: PriceSortingPolicy,
        U: UserMapUpdatePolicy,
        H: HiddenQuantityPolicy,
    {
        let changed = !Arc::ptr_eq(&storage.checksum.prepared, prepared);
        if changed {
            storage.checksum.prepared = prepared.clone();
            storage.checksum.generation += 1;
        }
        calculate::<S, L, Sort, U, H>(storage, changed)
    }
}

fn calculate<S, L, Sort, U, H>(
    storage: &mut OrderStorage<L, Sort, U, H>,
    force: bool,
) -> Result<u32, Error>
where
    S: Selection,
    L: PriceLevelContract<Checksum = WithChecksum<S>>,
    Sort: PriceSortingPolicy,
    U: UserMapUpdatePolicy,
    H: HiddenQuantityPolicy,
{
    let state = &mut storage.checksum;
    let bids = storage.bids.checksum_data::<S>(
        &storage.arena,
        &state.prepared,
        state.precision,
        state.generation,
        &mut state.bids,
    )?;
    let asks = storage.asks.checksum_data::<S>(
        &storage.arena,
        &state.prepared,
        state.precision,
        state.generation,
        &mut state.asks,
    )?;
    Ok(if force || bids || asks {
        state.finish()
    } else {
        state.value
    })
}

pub(crate) fn refresh_level<S: Selection, L: PriceLevelContract>(
    level: &mut L,
    arena: &Arena<RestingOrder<L::Price>>,
    prepared: &Prepared,
    precision: Precision,
    generation: u64,
) -> Result<(), Error>
where
    L::Checksum: ChecksumPolicy<LevelState = LevelCache>,
{
    if !level.checksum_state().dirty && level.checksum_state().generation == generation {
        return Ok(());
    }
    let mut cache = std::mem::take(level.checksum_state_mut());
    cache.inputs.clear();
    S::read(
        level,
        arena,
        prepared,
        precision,
        cache.widths,
        &mut cache.inputs,
    );
    cache.encoded.clear();
    let result = (|| {
        for input in &cache.inputs {
            for (index, writer) in prepared.writers.iter().enumerate() {
                if index != 0 {
                    cache
                        .encoded
                        .bytes
                        .extend_from_slice(prepared.specification.separator.as_bytes());
                }
                writer(input, precision, &mut cache.encoded.bytes)?;
            }
            cache.encoded.ends.push(cache.encoded.bytes.len());
        }
        Ok(())
    })();
    cache.dirty = result.is_err();
    cache.generation = generation;
    *level.checksum_state_mut() = cache;
    result
}

#[cfg(test)]
mod tests;
