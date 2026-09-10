//! Isolated AAPL add, execute, and replace benchmark for lobo's intrusive book.
//!
//! This consumes the exact neutral fixture produced by the native itchcpp
//! benchmark. Fixture decoding, execute/replace pre-seeding, validation, and
//! state checksums are outside every timed interval.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    collections::BTreeSet,
    env,
    error::Error,
    fs,
    hint::black_box,
    mem::{align_of, size_of},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering::Relaxed},
    thread,
    time::{Duration, Instant},
};

use lobo_models::{
    Side,
    events::{OrderDetails, OrderTransition},
    orders::core::{OrderCore, RestingOrder, RestingOrderData},
};
use lobo_primitives::{
    CompressedPrice,
    time::{DateTime, Utc},
    uuid::Uuid,
};
use lobo_storage::{
    DoNotUpdateUserMap, OrderStorage,
    arena::arenav1::{Arena, ArenaKey, ArenaNode},
    policies::DoNotUpdateHiddenQuantity,
    price_level::IntrusivePriceLevel,
    price_sorting::SortedVectorPriceSorting,
};

type ReplayLevel = IntrusivePriceLevel<CompressedPrice, DoNotUpdateHiddenQuantity>;
type ReplayStorage = OrderStorage<
    ReplayLevel,
    SortedVectorPriceSorting,
    DoNotUpdateUserMap,
    DoNotUpdateHiddenQuantity,
>;

const DEFAULT_FIXTURE: &str = "/Users/orlando/Documents/Codex/2026-08-27/\
referenced-chatgpt-conversation-this-is-an/outputs/aapl-path-cases-v1.bin";
const FIXTURE_ENV: &str = "LOBO_AAPL_PATH_FIXTURE";

struct CountingAllocator;

static ALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static ALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static DEALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static DEALLOC_BYTES: AtomicU64 = AtomicU64::new(0);
static REALLOC_CALLS: AtomicU64 = AtomicU64::new(0);
static REALLOC_OLD_BYTES: AtomicU64 = AtomicU64::new(0);
static REALLOC_NEW_BYTES: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingAllocator = CountingAllocator;

// The benchmark is single-threaded. Relaxed counters diagnose allocation
// traffic; they do not participate in program correctness.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOC_CALLS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Relaxed);
        // SAFETY: forwarding the allocator contract unchanged to System.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOC_CALLS.fetch_add(1, Relaxed);
        ALLOC_BYTES.fetch_add(layout.size() as u64, Relaxed);
        // SAFETY: forwarding the allocator contract unchanged to System.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        DEALLOC_CALLS.fetch_add(1, Relaxed);
        DEALLOC_BYTES.fetch_add(layout.size() as u64, Relaxed);
        // SAFETY: forwarding the allocator contract unchanged to System.
        unsafe { System.dealloc(ptr, layout) };
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        REALLOC_CALLS.fetch_add(1, Relaxed);
        REALLOC_OLD_BYTES.fetch_add(layout.size() as u64, Relaxed);
        REALLOC_NEW_BYTES.fetch_add(new_size as u64, Relaxed);
        // SAFETY: forwarding the allocator contract unchanged to System.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct AllocationStats {
    alloc_calls: u64,
    alloc_bytes: u64,
    dealloc_calls: u64,
    dealloc_bytes: u64,
    realloc_calls: u64,
    realloc_old_bytes: u64,
    realloc_new_bytes: u64,
}

fn reset_allocation_stats() {
    ALLOC_CALLS.store(0, Relaxed);
    ALLOC_BYTES.store(0, Relaxed);
    DEALLOC_CALLS.store(0, Relaxed);
    DEALLOC_BYTES.store(0, Relaxed);
    REALLOC_CALLS.store(0, Relaxed);
    REALLOC_OLD_BYTES.store(0, Relaxed);
    REALLOC_NEW_BYTES.store(0, Relaxed);
}

fn allocation_stats() -> AllocationStats {
    AllocationStats {
        alloc_calls: ALLOC_CALLS.load(Relaxed),
        alloc_bytes: ALLOC_BYTES.load(Relaxed),
        dealloc_calls: DEALLOC_CALLS.load(Relaxed),
        dealloc_bytes: DEALLOC_BYTES.load(Relaxed),
        realloc_calls: REALLOC_CALLS.load(Relaxed),
        realloc_old_bytes: REALLOC_OLD_BYTES.load(Relaxed),
        realloc_new_bytes: REALLOC_NEW_BYTES.load(Relaxed),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Mode {
    Add,
    Execute,
    ExecuteFull,
    ExecutePartial,
    Replace,
    Sizes,
}

impl Mode {
    fn parse(value: &str) -> Result<Self, Box<dyn Error>> {
        match value {
            "add" | "adds" => Ok(Self::Add),
            "execute" | "executes" => Ok(Self::Execute),
            "execute-full" => Ok(Self::ExecuteFull),
            "execute-partial" => Ok(Self::ExecutePartial),
            "replace" | "replaces" => Ok(Self::Replace),
            "sizes" => Ok(Self::Sizes),
            _ => Err(
                "mode must be add, execute, execute-full, execute-partial, replace, or sizes"
                    .into(),
            ),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Add => "add",
            Self::Execute => "execute",
            Self::ExecuteFull => "execute-full",
            Self::ExecutePartial => "execute-partial",
            Self::Replace => "replace",
            Self::Sizes => "sizes",
        }
    }
}

#[derive(Debug)]
struct Options {
    fixture: PathBuf,
    mode: Mode,
    rounds: usize,
    profile_operations: usize,
    ready_file: Option<PathBuf>,
    start_file: Option<PathBuf>,
    json_output: Option<PathBuf>,
}

impl Options {
    fn parse() -> Result<Self, Box<dyn Error>> {
        let mut arguments = env::args().skip(1);
        let mode = Mode::parse(&arguments.next().ok_or(
            "usage: aapl_path_bench <add|execute|execute-full|execute-partial|replace|sizes> \
             [--fixture PATH] \
             [--rounds N] [--profile-ops N] [--ready-file PATH] \
             [--start-file PATH] [--json PATH]",
        )?)?;
        let mut options = Self {
            fixture: env::var_os(FIXTURE_ENV)
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(DEFAULT_FIXTURE)),
            mode,
            rounds: 11,
            profile_operations: 0,
            ready_file: None,
            start_file: None,
            json_output: None,
        };

        while let Some(flag) = arguments.next() {
            let value = arguments
                .next()
                .ok_or_else(|| format!("{flag} requires a value"))?;
            match flag.as_str() {
                "--fixture" => options.fixture = value.into(),
                "--rounds" => options.rounds = value.parse()?,
                "--profile-ops" => options.profile_operations = value.parse()?,
                "--ready-file" => options.ready_file = Some(value.into()),
                "--start-file" => options.start_file = Some(value.into()),
                "--json" => options.json_output = Some(value.into()),
                _ => return Err(format!("unknown argument: {flag}").into()),
            }
        }

        if options.rounds == 0 {
            return Err("--rounds must be positive".into());
        }
        if options.ready_file.is_some() != options.start_file.is_some() {
            return Err("--ready-file and --start-file must be supplied together".into());
        }
        Ok(options)
    }
}

#[derive(Clone, Copy, Debug)]
struct AddCase {
    side: u8,
    shares: u32,
    price: u32,
}

#[derive(Clone, Copy, Debug)]
struct ExecuteCase {
    side: u8,
    shares_before: u32,
    executed_shares: u32,
    price: u32,
}

#[derive(Clone, Copy, Debug)]
struct ReplaceCase {
    side: u8,
    shares_before: u32,
    price_before: u32,
    replacement_shares: u32,
    replacement_price: u32,
}

#[derive(Debug)]
struct Fixture {
    adds: Vec<AddCase>,
    executes: Vec<ExecuteCase>,
    full_executes: Vec<ExecuteCase>,
    partial_executes: Vec<ExecuteCase>,
    replaces: Vec<ReplaceCase>,
}

struct FixtureReader<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> FixtureReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], Box<dyn Error>> {
        let end = self
            .position
            .checked_add(count)
            .ok_or("fixture offset overflow")?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or("fixture ended early")?;
        self.position = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, Box<dyn Error>> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, Box<dyn Error>> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn u64(&mut self) -> Result<u64, Box<dyn Error>> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }
}

impl Fixture {
    fn load(path: &Path) -> Result<Self, Box<dyn Error>> {
        let bytes = fs::read(path)?;
        let mut reader = FixtureReader::new(&bytes);
        if reader.take(8)? != b"AAPLFX1\0" {
            return Err("fixture magic does not match AAPLFX1".into());
        }
        if reader.u32()? != 1 {
            return Err("unsupported fixture version".into());
        }
        if reader.u32()? != 0x0102_0304 {
            return Err("fixture endian marker is invalid".into());
        }
        let add_count = usize::try_from(reader.u64()?)?;
        let execute_count = usize::try_from(reader.u64()?)?;
        let replace_count = usize::try_from(reader.u64()?)?;

        let mut adds = Vec::with_capacity(add_count);
        for _ in 0..add_count {
            let side = reader.u8()?;
            validate_side(side)?;
            adds.push(AddCase {
                side,
                shares: reader.u32()?,
                price: reader.u32()?,
            });
        }

        let mut executes = Vec::with_capacity(execute_count);
        for _ in 0..execute_count {
            let side = reader.u8()?;
            validate_side(side)?;
            executes.push(ExecuteCase {
                side,
                shares_before: reader.u32()?,
                executed_shares: reader.u32()?,
                price: reader.u32()?,
            });
        }

        let mut replaces = Vec::with_capacity(replace_count);
        for _ in 0..replace_count {
            let side = reader.u8()?;
            validate_side(side)?;
            replaces.push(ReplaceCase {
                side,
                shares_before: reader.u32()?,
                price_before: reader.u32()?,
                replacement_shares: reader.u32()?,
                replacement_price: reader.u32()?,
            });
        }
        if reader.position != bytes.len() {
            return Err("fixture has trailing bytes".into());
        }

        let (full_executes, partial_executes) = executes
            .iter()
            .copied()
            .partition(|case| case.executed_shares == case.shares_before);

        Ok(Self {
            adds,
            executes,
            full_executes,
            partial_executes,
            replaces,
        })
    }

    fn count(&self, mode: Mode) -> usize {
        match mode {
            Mode::Add => self.adds.len(),
            Mode::Execute => self.executes.len(),
            Mode::ExecuteFull => self.full_executes.len(),
            Mode::ExecutePartial => self.partial_executes.len(),
            Mode::Replace => self.replaces.len(),
            Mode::Sizes => 0,
        }
    }

    fn execute_cases(&self, mode: Mode) -> &[ExecuteCase] {
        match mode {
            Mode::Execute => &self.executes,
            Mode::ExecuteFull => &self.full_executes,
            Mode::ExecutePartial => &self.partial_executes,
            _ => unreachable!(),
        }
    }
}

fn validate_side(side: u8) -> Result<(), Box<dyn Error>> {
    if side <= 1 {
        Ok(())
    } else {
        Err(format!("invalid fixture side byte: {side}").into())
    }
}

#[inline(always)]
fn side(value: u8) -> Side {
    if value == 0 { Side::Buy } else { Side::Sell }
}

#[inline(always)]
fn order(
    reference: usize,
    side_value: u8,
    shares: u32,
    price: u32,
) -> RestingOrder<CompressedPrice> {
    RestingOrder::<CompressedPrice>::new_replay_order(OrderCore {
        uuid: Uuid::from_u128(reference as u128),
        price: Some(CompressedPrice::from(price)),
        creation_time: DateTime::<Utc>::MIN_UTC,
        quantity: shares as u64,
        trader: Uuid::nil(),
        side: side(side_value),
    })
}

#[inline(always)]
fn execute_transition(
    order: &mut RestingOrder<CompressedPrice>,
    details: OrderDetails<CompressedPrice>,
) {
    // The fixture contains only validated NASDAQ executions.
    let delta = unsafe { details.quantity.unwrap_unchecked() };
    order.common_data.quantity = order.common_data.quantity.wrapping_sub(delta);
}

#[inline(always)]
fn replace_transition(
    order: &mut RestingOrder<CompressedPrice>,
    details: OrderDetails<CompressedPrice>,
) {
    order.update_in_place(details);
}

#[inline(always)]
fn execute_details(shares: u32) -> OrderDetails<CompressedPrice> {
    OrderDetails {
        uuid: None,
        price: None,
        creation_time: None,
        quantity: Some(shares as u64),
        trader: None,
        side: None,
    }
}

#[inline(always)]
fn replacement_details(reference: usize, shares: u32, price: u32) -> OrderDetails<CompressedPrice> {
    OrderDetails {
        uuid: Some(Uuid::from_u128(reference as u128)),
        price: Some(CompressedPrice::from(price)),
        creation_time: None,
        quantity: Some(shares as u64),
        trader: None,
        side: None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct BookState {
    indexed_orders: usize,
    bid_orders: usize,
    ask_orders: usize,
    bid_levels: usize,
    ask_levels: usize,
    bid_visible: u64,
    ask_visible: u64,
}

impl BookState {
    fn capture(storage: &ReplayStorage) -> Self {
        Self {
            indexed_orders: storage.order_to_arena_map.len(),
            bid_orders: storage.bids.len(),
            ask_orders: storage.asks.len(),
            bid_levels: storage.bids.price_level_count(),
            ask_levels: storage.asks.price_level_count(),
            bid_visible: storage.bids.visible_quantity,
            ask_visible: storage.asks.visible_quantity,
        }
    }

    fn checksum(self) -> u64 {
        let values = [
            self.indexed_orders as u64,
            self.bid_orders as u64,
            self.ask_orders as u64,
            self.bid_levels as u64,
            self.ask_levels as u64,
            self.bid_visible,
            self.ask_visible,
        ];
        values
            .into_iter()
            .fold(0xcbf2_9ce4_8422_2325, |hash, value| {
                hash.wrapping_mul(0x0000_0100_0000_01b3) ^ value
            })
    }
}

#[derive(Debug)]
struct KernelResult {
    operations: usize,
    elapsed: Duration,
    state: BookState,
    allocations: AllocationStats,
}

impl KernelResult {
    fn nanoseconds_per_operation(&self) -> f64 {
        self.elapsed.as_nanos() as f64 / self.operations as f64
    }
}

struct PreparedKernel<'a> {
    fixture: &'a Fixture,
    mode: Mode,
    operations: usize,
    storage: ReplayStorage,
}

impl<'a> PreparedKernel<'a> {
    fn new(fixture: &'a Fixture, mode: Mode, operations: usize) -> Result<Self, Box<dyn Error>> {
        if operations == 0 {
            return Err("operation count must be positive".into());
        }
        let mut storage = OrderStorage::default();
        match mode {
            Mode::Execute | Mode::ExecuteFull | Mode::ExecutePartial => {
                let cases = fixture.execute_cases(mode);
                for index in 0..operations {
                    let case = cases[index % cases.len()];
                    storage
                        .add_order(
                            order(index + 1, case.side, case.shares_before, case.price),
                            &mut |_| {},
                        )
                        .expect("execute fixture order should be unique");
                }
            }
            Mode::Replace => {
                for index in 0..operations {
                    let case = fixture.replaces[index % fixture.replaces.len()];
                    storage
                        .add_order(
                            order(index + 1, case.side, case.shares_before, case.price_before),
                            &mut |_| {},
                        )
                        .expect("replace fixture order should be unique");
                }
            }
            Mode::Add => {}
            Mode::Sizes => return Err("sizes mode has no kernel".into()),
        }
        Ok(Self {
            fixture,
            mode,
            operations,
            storage,
        })
    }

    fn run(mut self) -> KernelResult {
        reset_allocation_stats();
        let started = Instant::now();
        match self.mode {
            Mode::Add => {
                for index in 0..self.operations {
                    let case = self.fixture.adds[index % self.fixture.adds.len()];
                    self.storage
                        .add_order(
                            order(index + 1, case.side, case.shares, case.price),
                            &mut |_| {},
                        )
                        .expect("add benchmark order should be unique");
                }
            }
            Mode::Execute | Mode::ExecuteFull | Mode::ExecutePartial => {
                let cases = self.fixture.execute_cases(self.mode);
                for index in 0..self.operations {
                    let case = cases[index % cases.len()];
                    self.storage
                        .modify_order_in_place(
                            Uuid::from_u128((index + 1) as u128),
                            execute_details(case.executed_shares),
                            execute_transition as OrderTransition<CompressedPrice>,
                            &mut |_| {},
                        )
                        .expect("execute benchmark order should exist");
                }
            }
            Mode::Replace => {
                let new_reference_base = self.operations + 1;
                for index in 0..self.operations {
                    let case = self.fixture.replaces[index % self.fixture.replaces.len()];
                    self.storage
                        .replace_in_place(
                            Uuid::from_u128((index + 1) as u128),
                            replacement_details(
                                new_reference_base + index,
                                case.replacement_shares,
                                case.replacement_price,
                            ),
                            replace_transition as OrderTransition<CompressedPrice>,
                            &mut |_| {},
                        )
                        .expect("replace benchmark order should exist");
                }
            }
            Mode::Sizes => unreachable!(),
        }
        let finished = Instant::now();
        let allocations = allocation_stats();
        black_box(&self.storage);
        let state = BookState::capture(&self.storage);
        KernelResult {
            operations: self.operations,
            elapsed: finished.duration_since(started),
            state,
            allocations,
        }
    }
}

fn expected_state(fixture: &Fixture, mode: Mode) -> BookState {
    let mut bid_levels = BTreeSet::new();
    let mut ask_levels = BTreeSet::new();
    let mut bid_orders = 0;
    let mut ask_orders = 0;
    let mut bid_visible = 0;
    let mut ask_visible = 0;

    let mut account = |side_value: u8, price: u32, quantity: u64| {
        if side_value == 0 {
            bid_orders += 1;
            bid_visible += quantity;
            bid_levels.insert(price);
        } else {
            ask_orders += 1;
            ask_visible += quantity;
            ask_levels.insert(price);
        }
    };

    match mode {
        Mode::Add => {
            for case in &fixture.adds {
                account(case.side, case.price, case.shares as u64);
            }
        }
        Mode::Execute | Mode::ExecuteFull | Mode::ExecutePartial => {
            for case in fixture.execute_cases(mode) {
                let remaining = case.shares_before - case.executed_shares;
                if remaining != 0 {
                    account(case.side, case.price, remaining as u64);
                }
            }
        }
        Mode::Replace => {
            for case in &fixture.replaces {
                account(
                    case.side,
                    case.replacement_price,
                    case.replacement_shares as u64,
                );
            }
        }
        Mode::Sizes => unreachable!(),
    }

    BookState {
        indexed_orders: bid_orders + ask_orders,
        bid_orders,
        ask_orders,
        bid_levels: bid_levels.len(),
        ask_levels: ask_levels.len(),
        bid_visible,
        ask_visible,
    }
}

fn percentile(mut values: Vec<f64>, fraction: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    let index = (fraction * (values.len() - 1) as f64) as usize;
    values[index]
}

fn print_layout() {
    macro_rules! layout {
        ($type:ty) => {
            println!(
                "layout type={} size={} align={}",
                stringify!($type),
                size_of::<$type>(),
                align_of::<$type>()
            );
        };
    }

    layout!(Uuid);
    layout!(CompressedPrice);
    layout!(OrderCore<CompressedPrice>);
    layout!(RestingOrderData);
    layout!(RestingOrder<CompressedPrice>);
    layout!(OrderDetails<CompressedPrice>);
    layout!(OrderTransition<CompressedPrice>);
    layout!(ArenaKey);
    layout!(Option<ArenaKey>);
    layout!(ArenaNode<RestingOrder<CompressedPrice>>);
    layout!(Arena<RestingOrder<CompressedPrice>>);
    layout!(ReplayLevel);
    layout!(ReplayStorage);
}

fn print_fixture(fixture: &Fixture, path: &Path) {
    println!("engine=lobo-intrusive");
    println!("symbol=AAPL");
    println!("fixture={}", path.display());
    println!("add_cases={}", fixture.adds.len());
    println!("execute_cases={}", fixture.executes.len());
    println!("full_execute_cases={}", fixture.full_executes.len());
    println!("partial_execute_cases={}", fixture.partial_executes.len());
    println!("replace_cases={}", fixture.replaces.len());
}

fn write_ready_and_wait(options: &Options) -> Result<(), Box<dyn Error>> {
    let (Some(ready_file), Some(start_file)) = (&options.ready_file, &options.start_file) else {
        return Ok(());
    };
    if let Some(parent) = ready_file.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(ready_file, format!("{}\n", std::process::id()))?;
    println!("profile_ready_pid={}", std::process::id());
    while !start_file.exists() {
        thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

fn write_json(
    path: &Path,
    options: &Options,
    results: &[KernelResult],
    median: f64,
    p95: f64,
) -> Result<(), Box<dyn Error>> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut output = String::new();
    output.push_str("{\n");
    output.push_str("  \"engine\": \"lobo-intrusive\",\n");
    output.push_str("  \"symbol\": \"AAPL\",\n");
    output.push_str(&format!("  \"mode\": \"{}\",\n", options.mode.name()));
    output.push_str(&format!("  \"rounds\": {},\n", results.len()));
    output.push_str(&format!("  \"median_ns_per_operation\": {median:.6},\n"));
    output.push_str(&format!("  \"p95_ns_per_operation\": {p95:.6},\n"));
    output.push_str("  \"samples\": [\n");
    for (index, result) in results.iter().enumerate() {
        output.push_str(&format!(
            "    {{\"operations\": {}, \"elapsed_ns\": {}, \
             \"ns_per_operation\": {:.6}, \"checksum\": {}, \
             \"alloc_calls\": {}, \"alloc_bytes\": {}, \
             \"dealloc_calls\": {}, \"dealloc_bytes\": {}, \
             \"realloc_calls\": {}, \"realloc_old_bytes\": {}, \
             \"realloc_new_bytes\": {}}}",
            result.operations,
            result.elapsed.as_nanos(),
            result.nanoseconds_per_operation(),
            result.state.checksum(),
            result.allocations.alloc_calls,
            result.allocations.alloc_bytes,
            result.allocations.dealloc_calls,
            result.allocations.dealloc_bytes,
            result.allocations.realloc_calls,
            result.allocations.realloc_old_bytes,
            result.allocations.realloc_new_bytes,
        ));
        output.push_str(if index + 1 == results.len() {
            "\n"
        } else {
            ",\n"
        });
    }
    output.push_str("  ]\n}\n");
    fs::write(path, output)?;
    Ok(())
}

fn run_profile(options: &Options, fixture: &Fixture) -> Result<(), Box<dyn Error>> {
    let kernel = PreparedKernel::new(fixture, options.mode, options.profile_operations)?;
    write_ready_and_wait(options)?;
    let result = kernel.run();
    println!("mode={}", options.mode.name());
    println!("profile_operations={}", result.operations);
    println!(
        "profile_elapsed_seconds={:.6}",
        result.elapsed.as_secs_f64()
    );
    println!(
        "profile_ns_per_operation={:.6}",
        result.nanoseconds_per_operation()
    );
    println!("profile_checksum={}", result.state.checksum());
    println!("profile_state={:?}", result.state);
    println!("profile_allocations={:?}", result.allocations);
    Ok(())
}

fn run_benchmark(options: &Options, fixture: &Fixture) -> Result<(), Box<dyn Error>> {
    let operations = fixture.count(options.mode);
    let expected = expected_state(fixture, options.mode);

    let warmup = PreparedKernel::new(fixture, options.mode, operations)?.run();
    if warmup.state != expected {
        return Err(format!(
            "warm-up state mismatch: actual={:?} expected={expected:?}",
            warmup.state
        )
        .into());
    }

    let mut results = Vec::with_capacity(options.rounds);
    for round in 0..options.rounds {
        let result = PreparedKernel::new(fixture, options.mode, operations)?.run();
        if result.state != expected {
            return Err(format!(
                "round {} state mismatch: actual={:?} expected={expected:?}",
                round + 1,
                result.state,
            )
            .into());
        }
        println!(
            "sample_round={} operations={} elapsed_ns={} ns_per_operation={:.6} \
             checksum={} alloc_calls={} alloc_bytes={} dealloc_calls={} realloc_calls={}",
            round + 1,
            result.operations,
            result.elapsed.as_nanos(),
            result.nanoseconds_per_operation(),
            result.state.checksum(),
            result.allocations.alloc_calls,
            result.allocations.alloc_bytes,
            result.allocations.dealloc_calls,
            result.allocations.realloc_calls,
        );
        results.push(result);
    }

    let times: Vec<_> = results
        .iter()
        .map(KernelResult::nanoseconds_per_operation)
        .collect();
    let median = percentile(times.clone(), 0.5);
    let p95 = percentile(times.clone(), 0.95);
    let min = times.iter().copied().fold(f64::INFINITY, f64::min);
    let max = times.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    println!("mode={}", options.mode.name());
    println!("operations_per_round={operations}");
    println!("rounds={}", options.rounds);
    println!("median_ns_per_operation={median:.6}");
    println!("p95_ns_per_operation={p95:.6}");
    println!("min_ns_per_operation={min:.6}");
    println!("max_ns_per_operation={max:.6}");
    println!(
        "median_million_operations_per_second={:.6}",
        1_000.0 / median
    );
    println!("validated_state={expected:?}");

    if let Some(path) = &options.json_output {
        write_json(path, options, &results, median, p95)?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let options = Options::parse()?;
    print_layout();
    if options.mode == Mode::Sizes {
        return Ok(());
    }

    let load_started = Instant::now();
    let fixture = Fixture::load(&options.fixture)?;
    print_fixture(&fixture, &options.fixture);
    println!(
        "fixture_load_seconds={:.6}",
        load_started.elapsed().as_secs_f64()
    );

    if options.profile_operations != 0 {
        run_profile(&options, &fixture)
    } else {
        run_benchmark(&options, &fixture)
    }
}
