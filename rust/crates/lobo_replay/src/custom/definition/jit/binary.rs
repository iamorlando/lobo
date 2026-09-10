//! Binary framing and bulk projections share the same generated fixed loads.
use super::{ModuleMemory, compiler::Compiler};
use crate::custom::definition::{
    compiled::{Mapping, Message, Operand},
    schema::{Binary, BinaryField},
};
use cranelift_codegen::ir::{self, AbiParam, InstBuilder, MemFlagsData, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Switch};
use cranelift_module::{FuncId, Module};
use lobo_models::Side;
use std::{
    collections::BTreeMap,
    mem::offset_of,
    sync::{Arc, Mutex},
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct Header {
    pub timestamp: u64,
    pub key: u64,
    pub tag: u64,
}
type Prefix = unsafe extern "C" fn(*const u8) -> u64;
type HeaderEntry = unsafe extern "C" fn(*const u8, *mut Header) -> u64;
type MessageEntry = unsafe extern "C" fn(*const u8, usize, *mut Message) -> u64;
pub(crate) struct Decoder {
    prefix: Prefix,
    header: HeaderEntry,
    message: MessageEntry,
    pub prefix_size: usize,
    minimum: usize,
    maximum: usize,
    _memory: Mutex<ModuleMemory>,
}
/// Emit a known-width load; width, byte order and location are compile-time inputs.
pub(super) fn integer(
    builder: &mut FunctionBuilder<'_>,
    bytes: ir::Value,
    field: &BinaryField,
) -> ir::Value {
    let mut value = builder.ins().iconst(types::I64, 0);
    let indices: Vec<_> = if field.byteorder == "little" {
        (0..field.size).rev().collect()
    } else {
        (0..field.size).collect()
    };
    for index in indices {
        let byte = builder.ins().load(
            types::I8,
            MemFlagsData::new(),
            bytes,
            (field.offset + index) as i32,
        );
        let byte = builder.ins().uextend(types::I64, byte);
        value = builder.ins().ishl_imm(value, 8);
        value = builder.ins().bor(value, byte);
    }
    value
}
fn operand(b: &mut FunctionBuilder<'_>, bytes: ir::Value, op: &Operand) -> ir::Value {
    match op.value {
        Some(n) => b.ins().iconst(types::I64, n as i64),
        None => integer(
            b,
            bytes,
            &BinaryField {
                kind: "uint".into(),
                offset: op.offset,
                size: op.width,
                byteorder: if op.little { "little" } else { "big" }.into(),
            },
        ),
    }
}
fn function(
    module: &mut ModuleMemory,
    arguments: usize,
    body: impl FnOnce(&mut FunctionBuilder<'_>, &[ir::Value]) -> Result<(), String>,
) -> Result<FuncId, String> {
    let mut context = module.make_context();
    context.func.signature.params = vec![AbiParam::new(types::I64); arguments];
    context
        .func
        .signature
        .returns
        .push(AbiParam::new(types::I64));
    let id = module
        .declare_anonymous_function(&context.func.signature)
        .map_err(|e| e.to_string())?;
    let mut frontend = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut context.func, &mut frontend);
        let block = b.create_block();
        b.append_block_params_for_function_params(block);
        b.switch_to_block(block);
        let args = b.block_params(block).to_vec();
        body(&mut b, &args)?;
        b.seal_all_blocks();
        b.finalize();
    }
    module
        .define_function(id, &mut context)
        .map_err(|e| e.to_string())?;
    Ok(id)
}
impl Decoder {
    pub(in crate::custom::definition) fn compile(
        spec: &Binary,
        records: &[Option<Mapping>],
        other: &BTreeMap<u64, Mapping>,
    ) -> Result<Arc<Self>, String> {
        let mut module = Compiler::with_symbols(&[])?.module;
        let prefix = function(&mut module, 1, |b, a| {
            let n = integer(b, a[0], &spec.length);
            b.ins().return_(&[n]);
            Ok(())
        })?;
        let header = function(&mut module, 2, |b, a| {
            for (field, offset) in [
                (&spec.timestamp, offset_of!(Header, timestamp)),
                (&spec.key, offset_of!(Header, key)),
                (&spec.tag, offset_of!(Header, tag)),
            ] {
                let n = integer(b, a[0], field);
                b.ins().store(MemFlagsData::new(), n, a[1], offset as i32);
            }
            let zero = b.ins().iconst(types::I64, 0);
            b.ins().return_(&[zero]);
            Ok(())
        })?;
        let message = function(&mut module, 3, |b, a| {
            let (bytes, length, output) = (a[0], a[1], a[2]);
            for (field, offset) in [
                (&spec.timestamp, offset_of!(Message, timestamp)),
                (&spec.key, offset_of!(Message, key)),
            ] {
                let n = integer(b, bytes, field);
                b.ins().store(MemFlagsData::new(), n, output, offset as i32);
            }
            let tag = integer(b, bytes, &spec.tag);
            let done = b.create_block();
            let size_error = b.create_block();
            let side_error = b.create_block();
            let mut switch = Switch::new();
            let mut cases = Vec::new();
            for (tag, mapping) in records
                .iter()
                .enumerate()
                .filter_map(|(i, m)| m.as_ref().map(|m| (i as u64, m)))
                .chain(other.iter().map(|(&i, m)| (i, m)))
            {
                let block = b.create_block();
                switch.set_entry(tag as u128, block);
                cases.push((block, mapping));
            }
            switch.emit(b, tag, done);
            for (block, mapping) in cases {
                b.switch_to_block(block);
                let valid =
                    b.ins()
                        .icmp_imm(ir::condcodes::IntCC::Equal, length, mapping.size as i64);
                let matched = b.create_block();
                b.ins().brif(valid, matched, &[], size_error, &[]);
                b.switch_to_block(matched);
                let kind = b.ins().iconst(types::I8, mapping.kind as i64);
                b.ins().store(
                    MemFlagsData::new(),
                    kind,
                    output,
                    offset_of!(Message, kind) as i32,
                );
                if mapping.kind != 0 {
                    for (op, offset) in [
                        (&mapping.id, offset_of!(Message, id)),
                        (&mapping.quantity, offset_of!(Message, quantity)),
                        (&mapping.price, offset_of!(Message, price)),
                        (&mapping.new_id, offset_of!(Message, new_id)),
                    ] {
                        let n = operand(b, bytes, op);
                        b.ins().store(MemFlagsData::new(), n, output, offset as i32);
                    }
                    let has_price = b
                        .ins()
                        .iconst(types::I8, i64::from(mapping.execution_price));
                    b.ins().store(
                        MemFlagsData::new(),
                        has_price,
                        output,
                        offset_of!(Message, execution_price) as i32,
                    );
                    if let Some(side) = &mapping.side {
                        let byte =
                            b.ins()
                                .load(types::I8, MemFlagsData::new(), bytes, side.offset as i32);
                        let byte = b.ins().uextend(types::I64, byte);
                        let bid = b.create_block();
                        let ask = b.create_block();
                        let mut values = Switch::new();
                        for (i, value) in side.values.iter().enumerate() {
                            if let Some(value) = value {
                                values.set_entry(
                                    i as u128,
                                    if *value == Side::Buy { bid } else { ask },
                                );
                            }
                        }
                        values.emit(b, byte, side_error);
                        for (block, side) in [(bid, Side::Buy), (ask, Side::Sell)] {
                            b.switch_to_block(block);
                            // Option<Side>'s exact Rust representation belongs to this build.
                            let representation =
                                unsafe { std::mem::transmute::<Option<Side>, u8>(Some(side)) };
                            let value = b.ins().iconst(types::I8, i64::from(representation));
                            b.ins().store(
                                MemFlagsData::new(),
                                value,
                                output,
                                offset_of!(Message, side) as i32,
                            );
                            b.ins().jump(done, &[]);
                        }
                    } else {
                        b.ins().jump(done, &[]);
                    }
                } else {
                    b.ins().jump(done, &[]);
                }
            }
            for (block, status) in [(done, 0), (size_error, 1), (side_error, 2)] {
                b.switch_to_block(block);
                let status = b.ins().iconst(types::I64, status);
                b.ins().return_(&[status]);
            }
            Ok(())
        })?;
        module.finalize_definitions().map_err(|e| e.to_string())?;
        let result = Self {
            prefix: unsafe {
                std::mem::transmute::<*const u8, Prefix>(module.get_finalized_function(prefix))
            },
            header: unsafe {
                std::mem::transmute::<*const u8, HeaderEntry>(module.get_finalized_function(header))
            },
            message: unsafe {
                std::mem::transmute::<*const u8, MessageEntry>(
                    module.get_finalized_function(message),
                )
            },
            prefix_size: spec.length.offset + spec.length.size,
            minimum: [&spec.tag, &spec.key, &spec.timestamp]
                .into_iter()
                .map(|f| f.offset + f.size)
                .max()
                .unwrap_or(0),
            maximum: spec.max_record_size,
            _memory: Mutex::new(module),
        };
        Ok(Arc::new(result))
    }
    pub(crate) fn length(&self, bytes: &[u8]) -> Result<usize, String> {
        if bytes.len() < self.prefix_size {
            return Err("Truncated record prefix".into());
        }
        let n = unsafe { (self.prefix)(bytes.as_ptr()) } as usize;
        if n == 0 || n > self.maximum {
            return Err("Invalid record length".into());
        }
        Ok(n)
    }
    pub(crate) fn header(&self, bytes: &[u8]) -> Result<Header, String> {
        if bytes.len() < self.minimum {
            return Err("Truncated binary header".into());
        }
        let mut header = Header::default();
        unsafe {
            (self.header)(bytes.as_ptr(), &mut header);
        }
        Ok(header)
    }
    pub(crate) fn decode(&self, bytes: &[u8]) -> Result<Message, String> {
        if bytes.len() < self.minimum {
            return Err("Truncated binary header".into());
        }
        let mut message = Message::default();
        match unsafe { (self.message)(bytes.as_ptr(), bytes.len(), &mut message) } {
            0 => Ok(message),
            1 => Err("Unexpected record size".into()),
            _ => Err("Invalid order side".into()),
        }
    }
}
