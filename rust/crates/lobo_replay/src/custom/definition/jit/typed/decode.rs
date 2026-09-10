//! JSON syntax comes from Jiter. The generated functions own field selection,
//! record layout and child decoding; no runtime schema walker or JSON DOM is
//! involved in reading a packet.
use super::super::ModuleMemory;
use super::{
    layout::{Id, Layout},
    storage::*,
};
use cranelift_codegen::ir::{self, AbiParam, InstBuilder, MemFlagsData, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};
use jiter::Jiter;
use std::{collections::BTreeMap, mem::offset_of, sync::Mutex};

#[derive(Default)]
pub(super) struct Buffers {
    pub arena: Arena,
    arrays: Vec<Vec<usize>>,
    used_arrays: usize,
}
impl Buffers {
    pub fn reset(&mut self) {
        self.arena.reset();
        self.used_arrays = 0;
    }
}

#[repr(C)]
pub(super) struct Scan<'a> {
    failed: u64,
    key: Span,
    parser: Jiter<'a>,
    input: &'a [u8],
    buffers: &'a mut Buffers,
    missing: &'a Record,
    error: Option<String>,
}
impl Scan<'_> {
    fn fail(&mut self, error: impl ToString) -> u64 {
        self.failed = 1;
        self.error = Some(error.to_string());
        0
    }
}
extern "C" fn position(scan: &Scan<'_>) -> u64 {
    scan.parser.current_index() as u64
}
extern "C" fn preserve(scan: &Scan<'_>, record: &mut Record, start: usize) -> u64 {
    record.raw = Span {
        address: unsafe { scan.input.as_ptr().add(start) } as usize,
        length: scan.parser.current_index() - start,
    };
    0
}
extern "C" fn own_raw(arena: &mut Arena, record: &mut Record) -> u64 {
    record.raw = arena.bytes(unsafe { record.raw.bytes() });
    0
}
extern "C" fn peek(scan: &mut Scan<'_>) -> u64 {
    match scan.parser.peek() {
        Ok(peek) => u64::from(peek.into_inner()),
        Err(error) => scan.fail(error),
    }
}
extern "C" fn skip(scan: &mut Scan<'_>) -> u64 {
    match scan.parser.next_skip() {
        Ok(()) => 0,
        Err(error) => scan.fail(error),
    }
}
extern "C" fn scalar_null(scan: &mut Scan<'_>, _: &mut Record) -> u64 {
    match scan.parser.known_null() {
        Ok(()) => 0,
        Err(error) => scan.fail(error),
    }
}
extern "C" fn scalar_bool(scan: &mut Scan<'_>, record: &mut Record) -> u64 {
    match scan.parser.next_bool() {
        Ok(value) => {
            record.kind = BOOL;
            record.number.low = u64::from(value);
            0
        }
        Err(error) => scan.fail(error),
    }
}
extern "C" fn scalar_number<const ID: bool>(scan: &mut Scan<'_>, record: &mut Record) -> u64 {
    match scan.parser.next_number_bytes() {
        Ok(bytes) => {
            record.kind = NUMBER;
            record.text = Span {
                address: bytes.as_ptr() as usize,
                length: bytes.len(),
            };
            match Number::parse(bytes) {
                Ok(number) => {
                    record.number(number);
                    if ID && record.valid & VALID_UNSIGNED != 0 {
                        record.identifier =
                            lobo_primitives::uuid::Uuid::from_u128(u128::from(record.unsigned));
                        record.valid |= VALID_ID;
                    }
                    0
                }
                // Unselected fields may legally contain numbers beyond book precision.
                // A selected projection reports the conversion failure.
                Err(_) => 0,
            }
        }
        Err(error) => scan.fail(error),
    }
}
extern "C" fn scalar_text<const NUMBER: bool, const ID: bool, const SIDE: bool>(
    scan: &mut Scan<'_>,
    record: &mut Record,
) -> u64 {
    match scan.parser.known_str() {
        Ok(text) => {
            record.kind = TEXT;
            let address = text.as_ptr() as usize;
            let start = scan.input.as_ptr() as usize;
            record.text = if address >= start && address + text.len() <= start + scan.input.len() {
                Span {
                    address,
                    length: text.len(),
                }
            } else {
                // Escapes use Jiter's reusable scratch space. Retain only
                // these decoded bytes; ordinary strings borrow the input.
                scan.buffers.arena.bytes(text.as_bytes())
            };
            record.text_projection::<NUMBER, ID, SIDE>();
            0
        }
        Err(error) => scan.fail(error),
    }
}
extern "C" fn new_record(scan: &mut Scan<'_>) -> u64 {
    scan.buffers.arena.put(Record {
        present: 1,
        ..*scan.missing
    }) as u64
}
extern "C" fn fields(scan: &mut Scan<'_>, record: &mut Record, count: u64) -> u64 {
    record.kind = OBJECT;
    let fields = scan.buffers.arena.allocate::<usize>(count as usize);
    unsafe {
        for i in 0..count as usize {
            fields.add(i).write(scan.missing as *const Record as usize);
        }
    }
    record.fields = fields as usize;
    0
}
pub(super) fn key_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
    }
    // Zero denotes end-of-object in the scanner ABI.
    hash | 1
}
fn save_key(scan: &mut Scan<'_>, first: bool) -> u64 {
    let key = if first {
        scan.parser.known_object()
    } else {
        scan.parser.next_key()
    };
    match key {
        Ok(Some(key)) => {
            scan.key = Span {
                address: key.as_ptr() as usize,
                length: key.len(),
            };
            key_hash(key.as_bytes())
        }
        Ok(None) => 0,
        Err(error) => scan.fail(error),
    }
}
extern "C" fn object_start(scan: &mut Scan<'_>) -> u64 {
    save_key(scan, true)
}
extern "C" fn object_step(scan: &mut Scan<'_>) -> u64 {
    save_key(scan, false)
}
unsafe extern "C" fn key_equal(scan: &Scan<'_>, pointer: *const u8, length: usize) -> u64 {
    u64::from(unsafe { scan.key.bytes() == std::slice::from_raw_parts(pointer, length) })
}
extern "C" fn array_length(scan: &mut Scan<'_>, record: &mut Record) -> u64 {
    record.kind = ARRAY;
    let mut count = 0;
    let result = (|| {
        let mut next = scan.parser.known_array()?;
        while next.is_some() {
            scan.parser.next_skip()?;
            count += 1;
            next = scan.parser.array_step()?;
        }
        Ok::<_, jiter::JiterError>(())
    })();
    match result {
        Ok(()) => {
            record.elements.length = count;
            0
        }
        Err(error) => scan.fail(error),
    }
}
extern "C" fn array_start(scan: &mut Scan<'_>, record: &mut Record) -> u64 {
    record.kind = ARRAY;
    let index = scan.buffers.used_arrays;
    scan.buffers.used_arrays += 1;
    if index == scan.buffers.arrays.len() {
        scan.buffers.arrays.push(Vec::new());
    }
    scan.buffers.arrays[index].clear();
    // While parsing, address is a pool index; array_finish replaces it with
    // the completed contiguous pointer buffer before expressions can read it.
    record.elements.address = index;
    match scan.parser.known_array() {
        Ok(next) => u64::from(next.is_some()),
        Err(error) => scan.fail(error),
    }
}
extern "C" fn array_push(scan: &mut Scan<'_>, record: &mut Record, row: usize) -> u64 {
    scan.buffers.arrays[record.elements.address].push(row);
    0
}
extern "C" fn array_step(scan: &mut Scan<'_>) -> u64 {
    match scan.parser.array_step() {
        Ok(next) => u64::from(next.is_some()),
        Err(error) => scan.fail(error),
    }
}
extern "C" fn array_finish(scan: &mut Scan<'_>, record: &mut Record) -> u64 {
    let rows = &scan.buffers.arrays[record.elements.address];
    record.elements = Span {
        address: rows.as_ptr() as usize,
        length: rows.len(),
    };
    0
}

extern "C" fn own_record(arena: &mut Arena, missing: &Record) -> u64 {
    arena.put(*missing) as u64
}
extern "C" fn own_words(arena: &mut Arena, count: usize) -> u64 {
    arena.allocate::<usize>(count) as u64
}
extern "C" fn own_text(arena: &mut Arena, target: &mut Record) -> u64 {
    target.text = arena.bytes(unsafe { target.text.bytes() });
    0
}

pub(super) type Entry = unsafe extern "C" fn(*mut Scan<'_>, *mut Record) -> u64;
pub(super) type CopyEntry =
    unsafe extern "C" fn(*mut Arena, *const Record, *const Record, *mut Record) -> u64;
pub(super) struct Decoder {
    pub entries: BTreeMap<Id, Entry>,
    pub copies: BTreeMap<Id, CopyEntry>,
    pub missing: Missing,
    _keys: Vec<Box<[u8]>>,
    _module: Mutex<ModuleMemory>,
}
impl Decoder {
    pub fn compile(layout: &Layout) -> Result<Self, String> {
        let imports: &[(&str, *const u8, usize)] = &[
            ("position", position as *const u8, 1),
            ("preserve", preserve as *const u8, 3),
            ("own_raw", own_raw as *const u8, 2),
            ("peek", peek as *const u8, 1),
            ("skip", skip as *const u8, 1),
            ("null", scalar_null as *const u8, 2),
            ("bool", scalar_bool as *const u8, 2),
            ("number", scalar_number::<false> as *const u8, 2),
            ("number_id", scalar_number::<true> as *const u8, 2),
            (
                "text_000",
                scalar_text::<false, false, false> as *const u8,
                2,
            ),
            (
                "text_001",
                scalar_text::<false, false, true> as *const u8,
                2,
            ),
            (
                "text_010",
                scalar_text::<false, true, false> as *const u8,
                2,
            ),
            ("text_011", scalar_text::<false, true, true> as *const u8, 2),
            (
                "text_100",
                scalar_text::<true, false, false> as *const u8,
                2,
            ),
            ("text_101", scalar_text::<true, false, true> as *const u8, 2),
            ("text_110", scalar_text::<true, true, false> as *const u8, 2),
            ("text_111", scalar_text::<true, true, true> as *const u8, 2),
            ("new_record", new_record as *const u8, 1),
            ("fields", fields as *const u8, 3),
            ("object_start", object_start as *const u8, 1),
            ("object_step", object_step as *const u8, 1),
            ("key_equal", key_equal as *const u8, 3),
            ("array_length", array_length as *const u8, 2),
            ("array_start", array_start as *const u8, 2),
            ("array_push", array_push as *const u8, 3),
            ("array_step", array_step as *const u8, 1),
            ("array_finish", array_finish as *const u8, 2),
            ("own_record", own_record as *const u8, 2),
            ("own_words", own_words as *const u8, 2),
            ("own_text", own_text as *const u8, 2),
        ];
        let mut jit = JITBuilder::with_flags(
            &[("opt_level", "speed")],
            cranelift_module::default_libcall_names(),
        )
        .map_err(|e| e.to_string())?;
        for &(name, address, _) in imports {
            jit.symbol(name, address);
        }
        let mut module = ModuleMemory::new(JITModule::new(jit));
        let mut ids = BTreeMap::new();
        for &(name, _, arity) in imports {
            let mut signature = module.make_signature();
            signature.params = vec![AbiParam::new(types::I64); arity];
            signature.returns.push(AbiParam::new(types::I64));
            ids.insert(
                name,
                module
                    .declare_function(name, Linkage::Import, &signature)
                    .map_err(|e| e.to_string())?,
            );
        }
        let mut functions = BTreeMap::new();
        let mut signature = module.make_signature();
        signature.params = vec![AbiParam::new(types::I64); 2];
        signature.returns.push(AbiParam::new(types::I64));
        for id in 0..layout.shapes.len() {
            if layout.canonical(id) == id {
                functions.insert(
                    id,
                    module
                        .declare_anonymous_function(&signature)
                        .map_err(|e| e.to_string())?,
                );
            }
        }
        let mut keys = Vec::new();
        for (&id, &function) in &functions {
            let mut context = module.make_context();
            context.func.signature = signature.clone();
            let mut frontend = FunctionBuilderContext::new();
            {
                let mut builder = FunctionBuilder::new(&mut context.func, &mut frontend);
                let start = builder.create_block();
                builder.append_block_params_for_function_params(start);
                builder.switch_to_block(start);
                let scan = builder.block_params(start)[0];
                let record = builder.block_params(start)[1];
                let done = builder.create_block();
                let mut emit = Generator {
                    builder,
                    module: &mut module,
                    imports: &ids,
                    done,
                    scan,
                };
                let start = if layout.shapes[id].preserve {
                    Some(emit.call("position", &[scan], false))
                } else {
                    None
                };
                let tag = emit.call("peek", &[scan], true);
                let scalar = emit.builder.create_block();
                let text = emit.builder.create_block();
                let null = emit.builder.create_block();
                let boolean = emit.builder.create_block();
                let object = emit.builder.create_block();
                let array = emit.builder.create_block();
                let mut switch = Switch::new();
                for (byte, block) in [
                    (b'n', null),
                    (b't', boolean),
                    (b'f', boolean),
                    (b'"', text),
                    (b'{', object),
                    (b'[', array),
                ] {
                    switch.set_entry(u128::from(byte), block);
                }
                switch.emit(&mut emit.builder, tag, scalar);
                let shape = &layout.shapes[id];
                let text_name = match (shape.numeric, shape.identifier, shape.side) {
                    (false, false, false) => "text_000",
                    (false, false, true) => "text_001",
                    (false, true, false) => "text_010",
                    (false, true, true) => "text_011",
                    (true, false, false) => "text_100",
                    (true, false, true) => "text_101",
                    (true, true, false) => "text_110",
                    (true, true, true) => "text_111",
                };
                for (block, name) in [
                    (
                        scalar,
                        if shape.identifier {
                            "number_id"
                        } else {
                            "number"
                        },
                    ),
                    (text, text_name),
                    (null, "null"),
                    (boolean, "bool"),
                ] {
                    emit.builder.switch_to_block(block);
                    emit.call(name, &[scan, record], false);
                    emit.builder.ins().jump(done, &[]);
                }
                emit.builder.switch_to_block(object);
                let shape = &layout.shapes[id];
                let count = emit.constant(shape.fields.len() as u64);
                emit.call("fields", &[scan, record, count], false);
                let first = emit.call("object_start", &[scan], true);
                let next_key = emit.builder.create_block();
                let process_key = emit.builder.create_block();
                emit.builder.append_block_param(process_key, types::I64);
                emit.builder
                    .ins()
                    .brif(first, process_key, &[first.into()], done, &[]);
                emit.builder.switch_to_block(process_key);
                let hash = emit.builder.block_params(process_key)[0];
                let length = emit.builder.ins().load(
                    types::I64,
                    MemFlagsData::new(),
                    record,
                    offset_of!(Record, object_length) as i32,
                );
                let length = emit.builder.ins().iadd_imm(length, 1);
                emit.builder.ins().store(
                    MemFlagsData::new(),
                    length,
                    record,
                    offset_of!(Record, object_length) as i32,
                );
                let unknown = emit.builder.create_block();
                let mut hashes: BTreeMap<u64, Vec<(usize, &String, Id)>> = BTreeMap::new();
                for (offset, (name, child)) in shape.fields.iter().enumerate() {
                    hashes
                        .entry(key_hash(name.as_bytes()))
                        .or_default()
                        .push((offset, name, *child));
                }
                let mut cases = Vec::new();
                let mut switch = Switch::new();
                for (hash, fields) in hashes {
                    let block = emit.builder.create_block();
                    switch.set_entry(hash as u128, block);
                    cases.push((block, fields));
                }
                switch.emit(&mut emit.builder, hash, unknown);
                for (block, fields) in cases {
                    emit.builder.switch_to_block(block);
                    for (offset, name, child_shape) in fields {
                        let bytes = name.as_bytes().to_vec().into_boxed_slice();
                        let pointer = emit.constant(bytes.as_ptr() as u64);
                        let length = emit.constant(bytes.len() as u64);
                        keys.push(bytes);
                        let equal = emit.call("key_equal", &[scan, pointer, length], false);
                        let matched = emit.builder.create_block();
                        let different = emit.builder.create_block();
                        emit.builder.ins().brif(equal, matched, &[], different, &[]);
                        emit.builder.switch_to_block(matched);
                        let child = emit.call("new_record", &[scan], false);
                        let fields = emit.builder.ins().load(
                            types::I64,
                            MemFlagsData::new(),
                            record,
                            offset_of!(Record, fields) as i32,
                        );
                        emit.builder.ins().store(
                            MemFlagsData::new(),
                            child,
                            fields,
                            (offset * size_of::<usize>()) as i32,
                        );
                        emit.decode(functions[&child_shape], child);
                        emit.builder.ins().jump(next_key, &[]);
                        emit.builder.switch_to_block(different);
                    }
                    emit.builder.ins().jump(unknown, &[]);
                }
                emit.builder.switch_to_block(unknown);
                emit.call("skip", &[scan], true);
                emit.builder.ins().jump(next_key, &[]);
                emit.builder.switch_to_block(next_key);
                let key = emit.call("object_step", &[scan], true);
                emit.builder
                    .ins()
                    .brif(key, process_key, &[key.into()], done, &[]);

                emit.builder.switch_to_block(array);
                if let Some(child_shape) = shape.element {
                    let first = emit.call("array_start", &[scan, record], true);
                    let row = emit.builder.create_block();
                    let finished = emit.builder.create_block();
                    emit.builder.ins().brif(first, row, &[], finished, &[]);
                    emit.builder.switch_to_block(row);
                    let child = emit.call("new_record", &[scan], false);
                    emit.decode(functions[&child_shape], child);
                    emit.call("array_push", &[scan, record, child], false);
                    let next = emit.call("array_step", &[scan], true);
                    emit.builder.ins().brif(next, row, &[], finished, &[]);
                    emit.builder.switch_to_block(finished);
                    emit.call("array_finish", &[scan, record], false);
                } else {
                    emit.call("array_length", &[scan, record], true);
                }
                emit.builder.ins().jump(done, &[]);
                emit.builder.switch_to_block(done);
                if let Some(start) = start {
                    emit.call("preserve", &[scan, record, start], false);
                }
                let zero = emit.constant(0);
                emit.builder.ins().return_(&[zero]);
                emit.builder.seal_all_blocks();
                emit.builder.finalize();
            }
            module
                .define_function(function, &mut context)
                .map_err(|e| e.to_string())?;
        }
        let copies = compile_copies(&mut module, &ids, layout)?;
        module.finalize_definitions().map_err(|e| e.to_string())?;
        let entries = functions
            .into_iter()
            .map(|(id, function)| {
                let pointer = module.get_finalized_function(function);
                (id, unsafe {
                    std::mem::transmute::<*const u8, Entry>(pointer)
                })
            })
            .collect();
        let copies = copies
            .into_iter()
            .map(|(id, function)| {
                let pointer = module.get_finalized_function(function);
                (id, unsafe {
                    std::mem::transmute::<*const u8, CopyEntry>(pointer)
                })
            })
            .collect();
        let max_fields = layout
            .shapes
            .iter()
            .map(|s| s.fields.len())
            .max()
            .unwrap_or(0);
        Ok(Self {
            entries,
            copies,
            missing: Missing::new(max_fields),
            _keys: keys,
            _module: Mutex::new(module),
        })
    }

    pub fn parse(
        &self,
        root: Id,
        bytes: &[u8],
        buffers: &mut Buffers,
    ) -> Result<*const Record, String> {
        buffers.reset();
        let missing = unsafe { &*(self.missing.record as *const Record) };
        let record = buffers.arena.put(Record {
            present: 1,
            ..*missing
        });
        let mut scan = Scan {
            failed: 0,
            key: Span::default(),
            parser: Jiter::new(bytes),
            input: bytes,
            buffers,
            missing,
            error: None,
        };
        // The selected entry owns the exact schema and all its child targets.
        unsafe {
            (self.entries[&root])(&mut scan, record);
        }
        if let Some(error) = scan.error {
            return Err(error);
        }
        scan.parser.finish().map_err(|e| e.to_string())?;
        Ok(record)
    }
}

fn compile_copies(
    module: &mut ModuleMemory,
    imports: &BTreeMap<&'static str, FuncId>,
    layout: &Layout,
) -> Result<BTreeMap<Id, FuncId>, String> {
    let mut signature = module.make_signature();
    signature.params = vec![AbiParam::new(types::I64); 4];
    signature.returns.push(AbiParam::new(types::I64));
    let mut functions = BTreeMap::new();
    for id in 0..layout.shapes.len() {
        if layout.canonical(id) == id {
            functions.insert(
                id,
                module
                    .declare_anonymous_function(&signature)
                    .map_err(|e| e.to_string())?,
            );
        }
    }
    for (&id, &function) in &functions {
        let mut context = module.make_context();
        context.func.signature = signature.clone();
        let mut frontend = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut context.func, &mut frontend);
            let start = builder.create_block();
            builder.append_block_params_for_function_params(start);
            builder.switch_to_block(start);
            let args = builder.block_params(start).to_vec();
            let (arena, missing, source, target) = (args[0], args[1], args[2], args[3]);
            let done = builder.create_block();
            let mut emit = Generator {
                builder,
                module,
                imports,
                done,
                scan: arena,
            };
            for offset in (0..size_of::<Record>()).step_by(8) {
                let word =
                    emit.builder
                        .ins()
                        .load(types::I64, MemFlagsData::new(), source, offset as i32);
                emit.builder
                    .ins()
                    .store(MemFlagsData::new(), word, target, offset as i32);
            }
            if layout.shapes[id].preserve {
                emit.call("own_raw", &[arena, target], false);
            }
            let kind = emit.builder.ins().load(
                types::I64,
                MemFlagsData::new(),
                source,
                offset_of!(Record, kind) as i32,
            );
            let text = emit.builder.create_block();
            let object = emit.builder.create_block();
            let array = emit.builder.create_block();
            let mut switch = Switch::new();
            switch.set_entry(TEXT as u128, text);
            switch.set_entry(NUMBER as u128, text);
            switch.set_entry(OBJECT as u128, object);
            switch.set_entry(ARRAY as u128, array);
            switch.emit(&mut emit.builder, kind, done);
            emit.builder.switch_to_block(text);
            emit.call("own_text", &[arena, target], false);
            emit.builder.ins().jump(done, &[]);
            emit.builder.switch_to_block(object);
            let shape = &layout.shapes[id];
            let count = emit.constant(shape.fields.len() as u64);
            let fields = emit.call("own_words", &[arena, count], false);
            emit.builder.ins().store(
                MemFlagsData::new(),
                fields,
                target,
                offset_of!(Record, fields) as i32,
            );
            let source_fields = emit.builder.ins().load(
                types::I64,
                MemFlagsData::new(),
                source,
                offset_of!(Record, fields) as i32,
            );
            for (offset, child_shape) in shape.fields.values().enumerate() {
                let slot = (offset * size_of::<usize>()) as i32;
                let child =
                    emit.builder
                        .ins()
                        .load(types::I64, MemFlagsData::new(), source_fields, slot);
                let kind = emit.builder.ins().load(
                    types::I64,
                    MemFlagsData::new(),
                    child,
                    offset_of!(Record, present) as i32,
                );
                let present = emit.builder.create_block();
                let next = emit.builder.create_block();
                emit.builder
                    .ins()
                    .store(MemFlagsData::new(), missing, fields, slot);
                emit.builder.ins().brif(kind, present, &[], next, &[]);
                emit.builder.switch_to_block(present);
                let output = emit.call("own_record", &[arena, missing], false);
                let function = emit
                    .module
                    .declare_func_in_func(functions[child_shape], emit.builder.func);
                emit.builder
                    .ins()
                    .call(function, &[arena, missing, child, output]);
                emit.builder
                    .ins()
                    .store(MemFlagsData::new(), output, fields, slot);
                emit.builder.ins().jump(next, &[]);
                emit.builder.switch_to_block(next);
            }
            emit.builder.ins().jump(done, &[]);
            emit.builder.switch_to_block(array);
            if let Some(child_shape) = shape.element {
                let length = emit.builder.ins().load(
                    types::I64,
                    MemFlagsData::new(),
                    source,
                    (offset_of!(Record, elements) + offset_of!(Span, length)) as i32,
                );
                let inputs = emit.builder.ins().load(
                    types::I64,
                    MemFlagsData::new(),
                    source,
                    offset_of!(Record, elements) as i32,
                );
                let outputs = emit.call("own_words", &[arena, length], false);
                emit.builder.ins().store(
                    MemFlagsData::new(),
                    outputs,
                    target,
                    offset_of!(Record, elements) as i32,
                );
                let row = emit.builder.create_block();
                emit.builder.append_block_param(row, types::I64);
                let zero = emit.constant(0);
                emit.builder
                    .ins()
                    .brif(length, row, &[zero.into()], done, &[]);
                emit.builder.switch_to_block(row);
                let index = emit.builder.block_params(row)[0];
                let offset = emit.builder.ins().ishl_imm(index, 3);
                let input = emit.builder.ins().iadd(inputs, offset);
                let child = emit
                    .builder
                    .ins()
                    .load(types::I64, MemFlagsData::new(), input, 0);
                let output = emit.call("own_record", &[arena, missing], false);
                let function = emit
                    .module
                    .declare_func_in_func(functions[&child_shape], emit.builder.func);
                emit.builder
                    .ins()
                    .call(function, &[arena, missing, child, output]);
                let destination = emit.builder.ins().iadd(outputs, offset);
                emit.builder
                    .ins()
                    .store(MemFlagsData::new(), output, destination, 0);
                let index = emit.builder.ins().iadd_imm(index, 1);
                let more =
                    emit.builder
                        .ins()
                        .icmp(ir::condcodes::IntCC::UnsignedLessThan, index, length);
                emit.builder
                    .ins()
                    .brif(more, row, &[index.into()], done, &[]);
            } else {
                emit.builder.ins().jump(done, &[]);
            }
            emit.builder.switch_to_block(done);
            let zero = emit.constant(0);
            emit.builder.ins().return_(&[zero]);
            emit.builder.seal_all_blocks();
            emit.builder.finalize();
        }
        module
            .define_function(function, &mut context)
            .map_err(|e| e.to_string())?;
    }
    Ok(functions)
}

struct Generator<'a, 'b> {
    builder: FunctionBuilder<'a>,
    module: &'b mut ModuleMemory,
    imports: &'b BTreeMap<&'static str, FuncId>,
    done: ir::Block,
    scan: ir::Value,
}
impl Generator<'_, '_> {
    fn constant(&mut self, value: u64) -> ir::Value {
        self.builder.ins().iconst(types::I64, value as i64)
    }
    fn checked(&mut self) {
        let failed = self.builder.ins().load(
            types::I64,
            MemFlagsData::new(),
            self.scan,
            offset_of!(Scan<'_>, failed) as i32,
        );
        let next = self.builder.create_block();
        self.builder.ins().brif(failed, self.done, &[], next, &[]);
        self.builder.switch_to_block(next);
    }
    fn call(&mut self, name: &str, arguments: &[ir::Value], checked: bool) -> ir::Value {
        let target = self
            .module
            .declare_func_in_func(self.imports[name], self.builder.func);
        let call = self.builder.ins().call(target, arguments);
        let result = self.builder.inst_results(call)[0];
        if checked {
            self.checked();
        }
        result
    }
    fn decode(&mut self, function: FuncId, record: ir::Value) {
        let function = self
            .module
            .declare_func_in_func(function, self.builder.func);
        self.builder.ins().call(function, &[self.scan, record]);
        self.checked();
    }
}

// The packet entry creates only lexical state here. Its next instruction calls
// the schema-specific decoder directly; there is no schema lookup in this ABI.
pub(super) extern "C" fn scan_start(frame: &mut super::runtime::Frame) -> u64 {
    frame.buffers.reset();
    let bytes = unsafe { std::slice::from_raw_parts(frame.bytes as *const u8, frame.length) };
    let missing = unsafe { &*(frame.missing as *const Record) };
    let scan = Scan {
        failed: 0,
        key: Span::default(),
        parser: Jiter::new(bytes),
        input: bytes,
        buffers: unsafe { &mut *(&mut frame.buffers as *mut Buffers) },
        missing,
        error: None,
    };
    // Scan contains a scratch buffer and must be dropped by scan_finish.
    let pointer = frame.temporary.allocate::<Scan<'_>>(1);
    unsafe {
        pointer.write(scan);
    }
    pointer as u64
}
pub(super) unsafe extern "C" fn scan_finish(
    frame: &mut super::runtime::Frame,
    pointer: *mut Scan<'_>,
) -> u64 {
    let mut scan = unsafe { pointer.read() };
    match scan
        .error
        .take()
        .map_or_else(|| scan.parser.finish().map_err(|e| e.to_string()), Err)
    {
        Ok(()) => 0,
        Err(error) => frame.fail(error),
    }
}
