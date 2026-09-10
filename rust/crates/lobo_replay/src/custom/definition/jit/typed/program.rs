use super::super::{Executable, compiler::Compiler};
use super::{
    decode::{self, Decoder},
    emit::Builder,
    layout::Layout,
    runtime::{self, ExecutionState, Frame, Header, Slots, State},
    storage::*,
};
use crate::{
    custom::{
        definition::{
            DefinedProtocol, checksum,
            schema::{Action, Definition, Format},
        },
        observer::EventSink,
    },
    feed::FeedState,
};
use cranelift_codegen::ir::InstBuilder;
use lobo_context::FeedMode;
use serde_json::Value;
use std::{
    collections::BTreeMap,
    mem::offset_of,
    num::NonZeroUsize,
    sync::{Arc, Mutex, OnceLock},
};

pub(crate) struct Program {
    pub definition: Arc<Definition>,
    pub checksum: Option<Arc<lobo_storage::policies::checksum::Prepared>>,
    layout: Arc<Layout>,
    decoder: Decoder,
    code: Executable,
    packet: usize,
    slots: Slots,
    empty_symbol: Arc<str>,
    pub(in crate::custom::definition) framing: Option<Arc<super::super::binary::Decoder>>,
    directory: Option<usize>,
    sequences: BTreeMap<usize, usize>,
    bootstrap: BTreeMap<String, usize>,
    _constants: Arena,
    _strings: Vec<Box<String>>,
}
pub(crate) fn prepare<O: EventSink>(
    definition: Arc<Definition>,
    mode: FeedMode,
) -> Result<Arc<Program>, String> {
    type Cache = lru::LruCache<(String, Vec<usize>, bool), Arc<Program>>;
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let replay = mode == FeedMode::Replay;
    let symbols = symbols::<O>(replay);
    let key = (
        serde_json::to_string(&*definition).map_err(|e| e.to_string())?,
        symbols.iter().map(|(_, a, _)| *a as usize).collect(),
        replay,
    );
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(Cache::new(NonZeroUsize::new(64).expect("cache capacity"))))
        .lock()
        .map_err(|_| "Compiler cache poisoned")?;
    if let Some(program) = cache.get(&key) {
        return Ok(program.clone());
    }
    // Generated code retains pointers to checksum configurations in this definition.
    // Prepare them before compilation, including eager Python construction.
    let mut definition = definition;
    let checksum = checksum::prepare(Arc::make_mut(&mut definition))?;
    let layout = Arc::new(Layout::new(&definition)?);
    let decoder = Decoder::compile(&layout)?;
    let copies: BTreeMap<_, _> = decoder
        .copies
        .keys()
        .map(|id| (*id, format!("copy_{id}")))
        .collect();
    let decoders: BTreeMap<_, _> = decoder
        .entries
        .keys()
        .map(|id| (*id, format!("decode_{id}")))
        .collect();
    let mut imports = symbols;
    for (id, name) in &copies {
        imports.push((name.as_str(), decoder.copies[id] as *const u8, 4));
    }
    for (id, name) in &decoders {
        imports.push((name.as_str(), decoder.entries[id] as *const u8, 2));
    }
    let code = Compiler::with_symbols(&imports)?;
    let mut builder = Builder {
        code,
        layout: layout.clone(),
        constants: Arena::default(),
        missing: decoder.missing.record,
        strings: Vec::new(),
        copies,
        fast_binary: false,
        wire_fields: None,
    };
    let framing = match &definition.format {
        Format::Binary(spec) => Some(super::super::binary::Decoder::compile(
            spec,
            &[],
            &BTreeMap::new(),
        )?),
        _ => None,
    };
    let directory = if let Format::Binary(spec) = &definition.format {
        Some(builder.function(layout.root, |e| {
            let tag = e.load(e.frame, offset_of!(Frame, header) + offset_of!(Header, tag));
            let done = e.builder.create_block();
            let mut switch = cranelift_frontend::Switch::new();
            let mut cases = Vec::new();
            for (tag, record) in &spec.records {
                let actions = record
                    .actions
                    .iter()
                    .filter(|a| crate::custom::definition::compiled::metadata(a))
                    .cloned()
                    .collect::<Vec<_>>();
                if !actions.is_empty() {
                    let block = e.builder.create_block();
                    switch.set_entry(*tag as u128, block);
                    cases.push((block, record, actions));
                }
            }
            switch.emit(&mut e.builder, tag, done);
            for (block, record, actions) in cases {
                e.builder.switch_to_block(block);
                let size = e.word(record.size as u64);
                e.call("packet_binary_size", &[e.frame, size], true);
                e.owner.wire_fields = Some(record.fields.clone());
                e.item = e.root;
                if whole_record(&serde_json::to_value(&actions).map_err(|e| e.to_string())?) {
                    e.binary_record(record)?;
                }
                e.call("packet_binary_start", &[e.frame], true);
                e.actions(&actions, false, false)?;
                e.owner.wire_fields = None;
                e.builder.ins().jump(done, &[]);
            }
            e.builder.switch_to_block(done);
            Ok(())
        })?)
    } else {
        None
    };
    let packet =
        builder.function(layout.root, |e| {
            match &definition.format {
                Format::Json { messages } => {
                    e.decode_root(layout.root)?;
                    e.call("packet_packet_start", &[e.frame], true);
                    for message in messages {
                        let yes = e.condition(&message.condition)?;
                        e.branch(
                            yes,
                            |e| e.actions(&message.actions, false, false),
                            |_| Ok(()),
                        )?;
                    }
                    e.call("packet_packet_finish", &[e.frame], true);
                }
                Format::Binary(spec) => {
                    let tag = e.load(e.frame, offset_of!(Frame, header) + offset_of!(Header, tag));
                    let done = e.builder.create_block();
                    let mut switch = cranelift_frontend::Switch::new();
                    let mut cases = Vec::new();
                    let plan =
                        crate::custom::definition::compiled::Plan::new(Arc::new(spec.clone())).ok();
                    for (tag, record) in &spec.records {
                        let block = e.builder.create_block();
                        switch.set_entry(*tag as u128, block);
                        cases.push((
                            block,
                            record,
                            plan.as_ref().is_some_and(|plan| plan.streamable(*tag)),
                        ));
                    }
                    switch.emit(&mut e.builder, tag, done);
                    for (block, record, fast) in cases {
                        e.builder.switch_to_block(block);
                        let size = e.word(record.size as u64);
                        e.call("packet_binary_size", &[e.frame, size], true);
                        e.owner.wire_fields = Some(record.fields.clone());
                        e.item = e.root;
                        if whole_record(
                            &serde_json::to_value(&record.actions).map_err(|e| e.to_string())?,
                        ) {
                            e.binary_record(record)?;
                        }
                        if fast {
                            let known = e.call("packet_binary_fast_start", &[e.frame], false);
                            let metadata = record
                                .actions
                                .iter()
                                .filter(|a| crate::custom::definition::compiled::metadata(a))
                                .cloned()
                                .collect::<Vec<_>>();
                            e.branch(
                                known,
                                |_| Ok(()),
                                |e| {
                                    e.call("packet_binary_start", &[e.frame], true);
                                    e.actions(&metadata, false, false)?;
                                    e.call("packet_binary_fast_start", &[e.frame], false);
                                    Ok(())
                                },
                            )?;
                            let entered = e.call("packet_binary_order", &[e.frame], true);
                            e.owner.fast_binary = true;
                            e.branch(
                                entered,
                                |e| {
                                    for action in record.actions.iter().filter(|a| {
                                        !crate::custom::definition::compiled::metadata(a)
                                    }) {
                                        e.order(&action.operation, false)?;
                                    }
                                    Ok(())
                                },
                                |_| Ok(()),
                            )?;
                            e.owner.fast_binary = false;
                        } else {
                            e.call("packet_binary_start", &[e.frame], true);
                            e.actions(&record.actions, false, false)?;
                        }
                        e.owner.wire_fields = None;
                        e.builder.ins().jump(done, &[]);
                    }
                    e.builder.switch_to_block(done);
                }
            }
            Ok(())
        })?;
    let mut sequences = BTreeMap::new();
    for actions in [
        &definition.connect,
        &definition.subscriptions,
        &definition.keepalive,
    ] {
        let index = builder.function(layout.root, |e| e.actions(actions, false, false))?;
        sequences.insert(actions.as_ptr() as usize, index);
    }
    let mut bootstrap = BTreeMap::new();
    for request in &definition.bootstrap {
        let root = layout.bootstrap[&request.name];
        let index = builder.function(root, |e| {
            e.decode_root(root)?;
            e.actions(&request.actions, false, false)
        })?;
        bootstrap.insert(request.name.clone(), index);
    }
    let program = Arc::new(Program {
        definition,
        checksum,
        framing,
        directory,
        empty_symbol: Arc::from(""),
        slots: Slots::new(&layout),
        layout,
        decoder,
        code: builder.code.finish()?,
        packet,
        sequences,
        bootstrap,
        _constants: builder.constants,
        _strings: builder.strings,
    });
    cache.put(key, program.clone());
    Ok(program)
}
impl Program {
    pub fn state(&self) -> ExecutionState {
        ExecutionState::new(State::new(&self.layout, unsafe {
            *(self.decoder.missing.record as *const Record)
        }))
    }
    fn run<O: EventSink>(
        &self,
        entry: usize,
        p: &mut DefinedProtocol<O>,
        s: &mut FeedState,
        bytes: &[u8],
        connection: u32,
        header: Header,
    ) -> Result<(), String> {
        let address = p.execution.as_ptr();
        let environment = &mut *p.execution;
        let mut frame = environment.frames.pop().unwrap_or_default();
        frame.failed = 0;
        frame.error = None;
        frame.symbol = self.empty_symbol.clone();
        frame.books.clear();
        frame.directories.clear();
        frame.sorting.clear();
        frame.levels.clear();
        frame.message.clear();
        frame.record = Default::default();
        frame.temporary.reset();
        frame.variables = environment.variables.as_mut_ptr() as usize;
        frame.tables = &mut **environment
            .connections
            .entry(connection)
            .or_insert_with(|| {
                Box::new(
                    (0..self.layout.tables.len())
                        .map(|_| Default::default())
                        .collect(),
                )
            }) as *mut Vec<_> as usize;
        frame.environment = address as usize;
        environment.depth += 1;
        frame.state = s as *mut FeedState as usize;
        frame.protocol = p as *mut DefinedProtocol<O> as usize;
        frame.bytes = bytes.as_ptr() as usize;
        frame.length = bytes.len();
        frame.header = header;
        frame.connection = connection;
        frame.slots = self.slots;
        frame.missing = self.decoder.missing.record;
        frame.item = frame.missing;
        frame.root = frame.missing;
        unsafe {
            (self.code.entries[entry])((&mut *frame as *mut Frame).cast());
        }
        let result = match frame.error.take() {
            Some(error) => {
                for scope in frame.books.iter().rev() {
                    p.invalidate(s, &scope.symbol);
                }
                Err(error)
            }
            None => Ok(()),
        };
        let environment = &mut *p.execution;
        environment.retired.append(&mut frame.retired);
        environment.depth -= 1;
        if environment.depth == 0 {
            environment.pool.append(&mut environment.retired);
        }
        environment.frames.push(frame);
        result
    }
    pub(in crate::custom::definition) fn directory<O: EventSink>(
        &self,
        p: &mut DefinedProtocol<O>,
        s: &mut FeedState,
        bytes: &[u8],
        header: super::super::binary::Header,
    ) -> Result<(), String> {
        self.run(
            self.directory.ok_or("Directory requires binary input")?,
            p,
            s,
            bytes,
            0,
            Header {
                timestamp: header.timestamp,
                key: header.key,
                tag: header.tag,
            },
        )
    }
    pub fn receive<O: EventSink>(
        &self,
        p: &mut DefinedProtocol<O>,
        s: &mut FeedState,
        bytes: &[u8],
        connection: u32,
    ) -> Result<(), String> {
        self.run(self.packet, p, s, bytes, connection, Header::default())
    }
    pub fn record<O: EventSink>(
        &self,
        p: &mut DefinedProtocol<O>,
        s: &mut FeedState,
        bytes: &[u8],
        timestamp: u64,
        key: u64,
        tag: u64,
    ) -> Result<(), String> {
        self.run(
            self.packet,
            p,
            s,
            bytes,
            0,
            Header {
                timestamp,
                key,
                tag,
            },
        )
    }
    pub fn lifecycle<O: EventSink>(
        &self,
        p: &mut DefinedProtocol<O>,
        s: &mut FeedState,
        actions: &[Action],
        _item: &Value,
        _root: &Value,
    ) -> Option<Result<(), String>> {
        self.sequences
            .get(&(actions.as_ptr() as usize))
            .map(|&entry| self.run(entry, p, s, &[], p.connection(), Header::default()))
    }
    pub fn bootstrap<O: EventSink>(
        &self,
        p: &mut DefinedProtocol<O>,
        s: &mut FeedState,
        id: &str,
        bytes: &[u8],
    ) -> Result<(), String> {
        self.run(
            *self.bootstrap.get(id).ok_or("Unknown bootstrap")?,
            p,
            s,
            bytes,
            0,
            Header::default(),
        )
    }
    /// Import a lifecycle value. Message actions use generated slot writes.
    pub fn bind(&self, state: &mut State, name: &str, value: &Value) {
        let Some(index) = self.layout.variables.keys().position(|n| n == name) else {
            return;
        };
        let shape = self.layout.variables[name];
        let mut buffer = state.buffers.pop().unwrap_or_default();
        let bytes = serde_json::to_vec(value).expect("JSON lifecycle value");
        let source = self
            .decoder
            .parse(shape, &bytes, &mut buffer)
            .expect("validated lifecycle value");
        let mut arena = state.pool.pop().unwrap_or_default();
        arena.reset();
        let mut record = unsafe { *(self.decoder.missing.record as *const Record) };
        unsafe {
            (self.decoder.copies[&shape])(
                &mut arena,
                self.decoder.missing.record as *const Record,
                source,
                &mut record,
            );
        }
        state.variables[index] = record;
        let old = std::mem::replace(&mut state.owners[index], arena);
        if state.depth == 0 {
            state.pool.push(old);
        } else {
            state.retired.push(old);
        }
        state.buffers.push(buffer);
    }
}

fn symbols<O: EventSink>(replay: bool) -> Vec<(&'static str, *const u8, usize)> {
    use super::host as h;
    let mut symbols = runtime::symbols();
    symbols.extend(super::command::symbols::<O>());
    macro_rules! import {
        ($name:ident,$arity:expr) => {
            symbols.push((
                concat!("packet_", stringify!($name)),
                h::$name::<O> as *const u8,
                $arity,
            ));
        };
    }
    import!(packet_start, 1);
    import!(packet_finish, 1);
    import!(send, 1);
    import!(subscribe, 1);
    import!(directory_complete, 1);
    import!(register, 6);
    import!(directory_begin, 1);
    import!(directory_remove_begin, 2);
    import!(directory_remove_end, 2);
    import!(directory_end, 1);
    import!(book_begin, 4);
    import!(book_depth, 2);
    import!(order_begin, 1);
    import!(apply_execute, 1);
    import!(apply_cancel, 1);
    import!(apply_remove, 1);
    import!(apply_replace, 1);
    import!(apply_trade, 1);
    import!(apply_history, 1);
    import!(trade_ready, 1);
    import!(levels_flush, 1);
    import!(binary_start, 1);
    import!(binary_fast_start, 1);
    import!(binary_order, 1);
    symbols.extend([
        (
            "packet_add",
            if replay {
                h::apply_add::<O, true, true> as *const u8
            } else {
                h::apply_add::<O, false, true> as *const u8
            },
            1,
        ),
        (
            "packet_add_fast",
            if replay {
                h::apply_add::<O, true, false> as *const u8
            } else {
                h::apply_add::<O, false, false> as *const u8
            },
            1,
        ),
        ("packet_modify", h::apply_update::<O, true> as *const u8, 1),
        ("packet_upsert", h::apply_update::<O, false> as *const u8, 1),
        ("packet_sequence", h::sequence::<O, false> as *const u8, 3),
        (
            "packet_sequence_reset",
            h::sequence::<O, true> as *const u8,
            3,
        ),
        ("packet_ready", h::book_finish::<O, true> as *const u8, 2),
        (
            "packet_not_ready",
            h::book_finish::<O, false> as *const u8,
            2,
        ),
        ("packet_checksum", h::checksum::<O, false> as *const u8, 3),
        (
            "packet_checksum_signed",
            h::checksum::<O, true> as *const u8,
            3,
        ),
        ("packet_book_skip", h::book_skip as *const u8, 1),
        ("packet_level", h::level_collect as *const u8, 1),
        ("packet_directory_start", h::directory_start as *const u8, 1),
        (
            "packet_directory_symbol",
            h::directory_symbol as *const u8,
            2,
        ),
        ("packet_sort_start", h::sort_start as *const u8, 1),
        ("packet_sort_push", h::sort_push as *const u8, 4),
        ("packet_sort_finish", h::sort_finish::<true> as *const u8, 1),
        (
            "packet_unique_finish",
            h::sort_finish::<false> as *const u8,
            1,
        ),
        ("packet_sort_row", h::sort_row as *const u8, 2),
        ("packet_sort_end", h::sort_end as *const u8, 1),
        ("packet_message_raw", h::message_raw as *const u8, 2),
        ("packet_message_start", h::message_start as *const u8, 1),
        ("packet_message_text", h::message_text as *const u8, 2),
        ("packet_message_scalar", h::message_scalar as *const u8, 2),
        ("packet_binary_size", h::binary_size as *const u8, 2),
        ("packet_binary_text", h::binary_text as *const u8, 3),
        ("packet_scan_start", decode::scan_start as *const u8, 1),
        ("packet_scan_finish", decode::scan_finish as *const u8, 2),
    ]);
    symbols
}

fn whole_record(value: &Value) -> bool {
    match value {
        Value::Array(values) => values.iter().any(whole_record),
        Value::Object(map) => {
            if matches!(
                map.get("op").and_then(Value::as_str),
                Some("field" | "root")
            ) {
                return !matches!(
                    map.get("path").and_then(Value::as_array).map(Vec::as_slice),
                    Some([Value::String(_)])
                );
            }
            map.values().any(whole_record)
        }
        _ => false,
    }
}
