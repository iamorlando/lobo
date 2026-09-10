use super::super::super::{
    expression::{Expr, Operator},
    projection::OrderRecord,
    schema::Action,
};
use super::super::compiler::Compiler;
use super::{
    layout::{Id, Layout},
    runtime::Frame,
    storage::*,
};
use cranelift_codegen::ir::{
    self, AbiParam, InstBuilder, MemFlagsData, StackSlotData, StackSlotKind, types,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::Module;
use serde_json::Value;
use std::{collections::BTreeMap, mem::offset_of, sync::Arc};

#[derive(Clone, Copy)]
pub(super) enum Form {
    Record,
    Unsigned,
    Boolean,
}
#[derive(Clone, Copy)]
pub(super) struct ValueRef {
    pub value: ir::Value,
    pub form: Form,
}
#[derive(Clone, Copy)]
pub(super) struct View {
    pub address: ir::Value,
    pub shape: Id,
}

pub(super) struct Builder {
    pub code: Compiler,
    pub layout: Arc<Layout>,
    pub constants: Arena,
    pub missing: usize,
    pub strings: Vec<Box<String>>,
    pub copies: BTreeMap<Id, String>,
    pub fast_binary: bool,
    pub wire_fields: Option<BTreeMap<String, super::super::super::schema::BinaryField>>,
}

impl Builder {
    pub fn constant(&mut self, value: &Value, shape: Id) -> Result<usize, String> {
        self.layout
            .project(value, shape, &mut self.constants, self.missing)
    }

    pub fn function(
        &mut self,
        root: Id,
        body: impl FnOnce(&mut Emit<'_, '_>) -> Result<(), String>,
    ) -> Result<usize, String> {
        let mut context = self.code.module.make_context();
        context
            .func
            .signature
            .params
            .push(AbiParam::new(types::I64));
        let id = self
            .code
            .module
            .declare_anonymous_function(&context.func.signature)
            .map_err(|e| e.to_string())?;
        let mut frontend = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut context.func, &mut frontend);
            let start = builder.create_block();
            builder.append_block_params_for_function_params(start);
            builder.switch_to_block(start);
            let frame = builder.block_params(start)[0];
            let failure = builder.create_block();
            let item = builder.ins().load(
                types::I64,
                MemFlagsData::new(),
                frame,
                offset_of!(Frame, item) as i32,
            );
            let root_address = builder.ins().load(
                types::I64,
                MemFlagsData::new(),
                frame,
                offset_of!(Frame, root) as i32,
            );
            let mut emit = Emit {
                owner: self,
                builder,
                frame,
                failure,
                item: View {
                    address: item,
                    shape: root,
                },
                root: View {
                    address: root_address,
                    shape: root,
                },
            };
            body(&mut emit)?;
            emit.builder.ins().return_(&[]);
            emit.builder.switch_to_block(failure);
            emit.builder.ins().return_(&[]);
            emit.builder.seal_all_blocks();
            emit.builder.finalize();
        }
        self.code
            .module
            .define_function(id, &mut context)
            .map_err(|e| e.to_string())?;
        let index = self.code.functions.len();
        self.code.functions.push(id);
        Ok(index)
    }
}

pub(super) struct Emit<'a, 'b> {
    pub owner: &'b mut Builder,
    pub builder: FunctionBuilder<'a>,
    pub frame: ir::Value,
    pub failure: ir::Block,
    pub item: View,
    pub root: View,
}
impl Emit<'_, '_> {
    pub fn word(&mut self, value: u64) -> ir::Value {
        self.builder.ins().iconst(types::I64, value as i64)
    }
    pub fn load(&mut self, pointer: ir::Value, offset: usize) -> ir::Value {
        self.builder
            .ins()
            .load(types::I64, MemFlagsData::new(), pointer, offset as i32)
    }
    pub fn store(&mut self, pointer: ir::Value, offset: usize, value: ir::Value) {
        self.builder
            .ins()
            .store(MemFlagsData::new(), value, pointer, offset as i32);
    }
    pub fn call(&mut self, name: &str, args: &[ir::Value], fallible: bool) -> ir::Value {
        let function = self
            .owner
            .code
            .module
            .declare_func_in_func(self.owner.code.imports[name], self.builder.func);
        let call = self.builder.ins().call(function, args);
        let value = self.builder.inst_results(call)[0];
        if fallible {
            let failed = self.load(self.frame, offset_of!(Frame, failed));
            let next = self.builder.create_block();
            self.builder
                .ins()
                .brif(failed, self.failure, &[], next, &[]);
            self.builder.switch_to_block(next);
        }
        value
    }
    pub fn path(&mut self, mut view: View, path: &[Value]) -> Result<View, String> {
        for part in path {
            match part {
                Value::String(name) => {
                    let shape = &self.owner.layout.shapes[view.shape];
                    let (index, (_, child)) = shape
                        .fields
                        .iter()
                        .enumerate()
                        .find(|(_, (field, _))| *field == name)
                        .ok_or("Field has no compiled slot")?;
                    let child = *child;
                    let fields = self.load(view.address, offset_of!(Record, fields));
                    let address = self.load(fields, index * size_of::<usize>());
                    view = View {
                        address,
                        shape: child,
                    };
                }
                Value::Number(index) => {
                    let index = index.as_i64().ok_or("Array index exceeds i64")?;
                    let shape = self.owner.layout.shapes[view.shape]
                        .element
                        .ok_or("Array has no compiled row layout")?;
                    let length = self.load(
                        view.address,
                        offset_of!(Record, elements) + offset_of!(Span, length),
                    );
                    let offset = if index < 0 {
                        self.builder.ins().iadd_imm(length, index)
                    } else {
                        self.word(index as u64)
                    };
                    let valid = self.builder.ins().icmp(
                        ir::condcodes::IntCC::UnsignedLessThan,
                        offset,
                        length,
                    );
                    let yes = self.builder.create_block();
                    let done = self.builder.create_block();
                    self.builder.append_block_param(done, types::I64);
                    let missing = self.word(self.owner.missing as u64);
                    self.builder
                        .ins()
                        .brif(valid, yes, &[], done, &[missing.into()]);
                    self.builder.switch_to_block(yes);
                    let rows = self.load(view.address, offset_of!(Record, elements));
                    let offset = self.builder.ins().ishl_imm(offset, 3);
                    let pointer = self.builder.ins().iadd(rows, offset);
                    let row = self.load(pointer, 0);
                    self.builder.ins().jump(done, &[row.into()]);
                    self.builder.switch_to_block(done);
                    view = View {
                        address: self.builder.block_params(done)[0],
                        shape,
                    };
                }
                _ => return Err("Invalid compiled field path".into()),
            }
        }
        Ok(view)
    }
    pub fn result(&self, expr: &Expr) -> Result<Id, String> {
        self.owner
            .layout
            .result(expr, self.item.shape, self.root.shape)
    }
    pub fn reference(&mut self, value: ValueRef, shape: Id) -> View {
        let address = match value.form {
            Form::Record => value.value,
            Form::Unsigned => self.call("typed_make_number", &[self.frame, value.value], false),
            Form::Boolean => self.call("typed_make_boolean", &[self.frame, value.value], false),
        };
        View { address, shape }
    }
    pub fn record(&mut self, expr: &Expr) -> Result<View, String> {
        let value = self.expression(expr)?;
        let shape = self.result(expr)?;
        Ok(self.reference(value, shape))
    }
    pub fn uint(&mut self, expr: &Expr) -> Result<ir::Value, String> {
        let value = self.expression(expr)?;
        Ok(match value.form {
            Form::Unsigned => value.value,
            _ => {
                let shape = self.result(expr)?;
                let value = self.reference(value, shape);
                self.read_unsigned(value.address)
            }
        })
    }
    pub fn boolean(&mut self, value: ValueRef) -> ir::Value {
        match value.form {
            Form::Boolean => value.value,
            Form::Unsigned => self.word(0),
            Form::Record => {
                let kind = self.load(value.value, offset_of!(Record, kind));
                let value = self.load(
                    value.value,
                    offset_of!(Record, number) + offset_of!(Number, low),
                );
                let is_bool =
                    self.builder
                        .ins()
                        .icmp_imm(ir::condcodes::IntCC::Equal, kind, BOOL as i64);
                let value = self
                    .builder
                    .ins()
                    .icmp_imm(ir::condcodes::IntCC::NotEqual, value, 0);
                let yes = self.builder.ins().band(is_bool, value);
                self.builder.ins().uextend(types::I64, yes)
            }
        }
    }
    pub fn branch(
        &mut self,
        condition: ir::Value,
        yes: impl FnOnce(&mut Self) -> Result<(), String>,
        no: impl FnOnce(&mut Self) -> Result<(), String>,
    ) -> Result<(), String> {
        let left = self.builder.create_block();
        let right = self.builder.create_block();
        let done = self.builder.create_block();
        self.builder.ins().brif(condition, left, &[], right, &[]);
        self.builder.switch_to_block(left);
        yes(self)?;
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(right);
        no(self)?;
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(done);
        Ok(())
    }
    pub fn counted(
        &mut self,
        length: ir::Value,
        body: impl FnOnce(&mut Self, ir::Value) -> Result<(), String>,
    ) -> Result<(), String> {
        let next = self.builder.create_block();
        let done = self.builder.create_block();
        self.builder.append_block_param(next, types::I64);
        let zero = self.word(0);
        self.builder
            .ins()
            .brif(length, next, &[zero.into()], done, &[]);
        self.builder.switch_to_block(next);
        let index = self.builder.block_params(next)[0];
        body(self, index)?;
        let index = self.builder.ins().iadd_imm(index, 1);
        let more = self
            .builder
            .ins()
            .icmp(ir::condcodes::IntCC::UnsignedLessThan, index, length);
        self.builder
            .ins()
            .brif(more, next, &[index.into()], done, &[]);
        self.builder.switch_to_block(done);
        Ok(())
    }
    pub fn freeze(&mut self, value: View) -> View {
        let slot = self.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            size_of::<Record>() as u32,
            3,
        ));
        let address = self.builder.ins().stack_addr(types::I64, slot, 0);
        for offset in (0..size_of::<Record>()).step_by(8) {
            let word = self.load(value.address, offset);
            self.store(address, offset, word);
        }
        View {
            address,
            shape: value.shape,
        }
    }
    pub fn own(&mut self, value: View) {
        let value = self.freeze(value);
        self.call("typed_own_begin", &[self.frame], false);
        let arena = self
            .builder
            .ins()
            .iadd_imm(self.frame, offset_of!(Frame, own) as i64);
        let target = self
            .builder
            .ins()
            .iadd_imm(self.frame, offset_of!(Frame, owned_value) as i64);
        let missing = self.word(self.owner.missing as u64);
        let name = self.owner.copies[&value.shape].as_str();
        let function = self
            .owner
            .code
            .module
            .declare_func_in_func(self.owner.code.imports[name], self.builder.func);
        self.builder
            .ins()
            .call(function, &[arena, missing, value.address, target]);
    }
    pub fn text_constant(&mut self, value: &str) -> ir::Value {
        let value = Box::new(value.to_owned());
        let pointer = &*value as *const String as u64;
        self.owner.strings.push(value);
        self.word(pointer)
    }
    fn allocated_record(&mut self, shape: Id, kind: u64) -> View {
        let size = self.word(size_of::<Record>() as u64);
        let pointer = self.call("typed_allocate", &[self.frame, size], false);
        let missing = self.word(self.owner.missing as u64);
        for offset in (0..size_of::<Record>()).step_by(8) {
            let word = self.load(missing, offset);
            self.store(pointer, offset, word);
        }
        let kind = self.word(kind);
        self.store(pointer, offset_of!(Record, kind), kind);
        let one = self.word(1);
        self.store(pointer, offset_of!(Record, present), one);
        View {
            address: pointer,
            shape,
        }
    }
    pub fn expression(&mut self, expr: &Expr) -> Result<ValueRef, String> {
        use Operator::*;
        let shape = self.result(expr)?;
        let (value, form) = match expr.op {
            Literal => {
                if let Some(n) = expr.value.as_u64() {
                    (self.word(n), Form::Unsigned)
                } else if let Some(b) = expr.value.as_bool() {
                    (self.word(u64::from(b)), Form::Boolean)
                } else {
                    let pointer = self.owner.constant(&expr.value, shape)?;
                    (self.word(pointer as u64), Form::Record)
                }
            }
            Field | Root => {
                if matches!(expr.op, Root) || self.item.address == self.root.address {
                    if let Some(fields) = &self.owner.wire_fields {
                        if let [Value::String(name)] = expr.path.as_slice() {
                            if let Some(field) = fields.get(name).cloned() {
                                return self.wire_field(&field, shape);
                            }
                            return Ok(ValueRef {
                                value: self.word(self.owner.missing as u64),
                                form: Form::Record,
                            });
                        }
                    }
                }
                let view = if matches!(expr.op, Field) {
                    self.item
                } else {
                    self.root
                };
                let view = self.path(view, &expr.path)?;
                (view.address, Form::Record)
            }
            Variable => {
                if self.owner.fast_binary && matches!(expr.name.as_str(), "clock" | "key") {
                    let offset = if expr.name == "clock" {
                        offset_of!(super::runtime::Header, timestamp)
                    } else {
                        offset_of!(super::runtime::Header, key)
                    };
                    return Ok(ValueRef {
                        value: self.load(self.frame, offset_of!(Frame, header) + offset),
                        form: Form::Unsigned,
                    });
                }
                let index = self
                    .owner
                    .layout
                    .variables
                    .keys()
                    .position(|name| name == &expr.name)
                    .ok_or("Unknown variable slot")?;
                let variables = self.load(self.frame, offset_of!(Frame, variables));
                (
                    self.builder
                        .ins()
                        .iadd_imm(variables, (index * size_of::<Record>()) as i64),
                    Form::Record,
                )
            }
            Get => {
                let view = self.record(&expr.args[0])?;
                let path = expr.args[1]
                    .value
                    .as_array()
                    .ok_or("Field path must be fixed at construction")?;
                let view = self.path(view, path)?;
                (view.address, Form::Record)
            }
            Lookup => {
                let table = expr.args[0]
                    .value
                    .as_str()
                    .ok_or("Table name must be fixed at construction")?;
                let index = self
                    .owner
                    .layout
                    .tables
                    .keys()
                    .position(|name| name == table)
                    .ok_or("Unknown table slot")?;
                let key = self.record(&expr.args[1])?;
                let index = self.word(index as u64);
                (
                    self.call("typed_lookup", &[self.frame, index, key.address], false),
                    Form::Record,
                )
            }
            Choose => {
                let test = self.expression(&expr.args[0])?;
                let test = self.boolean(test);
                let yes = self.builder.create_block();
                let no = self.builder.create_block();
                let done = self.builder.create_block();
                self.builder.append_block_param(done, types::I64);
                self.builder.ins().brif(test, yes, &[], no, &[]);
                self.builder.switch_to_block(yes);
                let left = self.record(&expr.args[1])?;
                self.builder.ins().jump(done, &[left.address.into()]);
                self.builder.switch_to_block(no);
                let right = self.record(&expr.args[2])?;
                self.builder.ins().jump(done, &[right.address.into()]);
                self.builder.switch_to_block(done);
                (self.builder.block_params(done)[0], Form::Record)
            }
            And | Or => {
                let test = self.expression(&expr.args[0])?;
                let test = self.boolean(test);
                let rhs = self.builder.create_block();
                let done = self.builder.create_block();
                self.builder.append_block_param(done, types::I64);
                if matches!(expr.op, And) {
                    self.builder
                        .ins()
                        .brif(test, rhs, &[], done, &[test.into()]);
                } else {
                    self.builder
                        .ins()
                        .brif(test, done, &[test.into()], rhs, &[]);
                }
                self.builder.switch_to_block(rhs);
                let result = self.expression(&expr.args[1])?;
                let result = self.boolean(result);
                self.builder.ins().jump(done, &[result.into()]);
                self.builder.switch_to_block(done);
                (self.builder.block_params(done)[0], Form::Boolean)
            }
            Eq | Ne | Gt | Lt => {
                let a = self.expression(&expr.args[0])?;
                let b = self.expression(&expr.args[1])?;
                let result = if matches!((a.form, b.form), (Form::Unsigned, Form::Unsigned)) {
                    let comparison = match expr.op {
                        Gt => ir::condcodes::IntCC::UnsignedGreaterThan,
                        Lt => ir::condcodes::IntCC::UnsignedLessThan,
                        _ => ir::condcodes::IntCC::Equal,
                    };
                    let value = self.builder.ins().icmp(comparison, a.value, b.value);
                    self.builder.ins().uextend(types::I64, value)
                } else {
                    let ashape = self.result(&expr.args[0])?;
                    let bshape = self.result(&expr.args[1])?;
                    let a = self.reference(a, ashape);
                    let b = self.reference(b, bshape);
                    let (name, a, b) = match expr.op {
                        Gt => ("typed_greater", a, b),
                        Lt => ("typed_greater", b, a),
                        _ => ("typed_equal", a, b),
                    };
                    if name == "typed_equal" {
                        self.equal_views(a, b)?
                    } else {
                        self.call(name, &[self.frame, a.address, b.address], true)
                    }
                };
                (
                    if matches!(expr.op, Ne) {
                        self.builder.ins().bxor_imm(result, 1)
                    } else {
                        result
                    },
                    Form::Boolean,
                )
            }
            Exists | IsArray => {
                let value = self.record(&expr.args[0])?;
                let kind = self.load(value.address, offset_of!(Record, kind));
                let value = if matches!(expr.op, Exists) {
                    self.builder
                        .ins()
                        .icmp_imm(ir::condcodes::IntCC::NotEqual, kind, NULL as i64)
                } else {
                    self.builder
                        .ins()
                        .icmp_imm(ir::condcodes::IntCC::Equal, kind, ARRAY as i64)
                };
                (self.builder.ins().uextend(types::I64, value), Form::Boolean)
            }
            Length => {
                let value = self.record(&expr.args[0])?;
                let kind = self.load(value.address, offset_of!(Record, kind));
                let text = self.load(
                    value.address,
                    offset_of!(Record, text) + offset_of!(Span, length),
                );
                let elements = self.load(
                    value.address,
                    offset_of!(Record, elements) + offset_of!(Span, length),
                );
                let object = self.load(value.address, offset_of!(Record, object_length));
                let is_object =
                    self.builder
                        .ins()
                        .icmp_imm(ir::condcodes::IntCC::Equal, kind, OBJECT as i64);
                let collection = self.builder.ins().select(is_object, object, elements);
                let is_text =
                    self.builder
                        .ins()
                        .icmp_imm(ir::condcodes::IntCC::Equal, kind, TEXT as i64);
                (
                    self.builder.ins().select(is_text, text, collection),
                    Form::Unsigned,
                )
            }
            Decimal => {
                let absolute = matches!(expr.args[0].op, Abs);
                let source = if absolute {
                    &expr.args[0].args[0]
                } else {
                    &expr.args[0]
                };
                let source = self.record(source)?;
                let places = self.uint(&expr.args[1])?;
                (
                    self.call(
                        if absolute {
                            "typed_absolute_decimal"
                        } else {
                            "typed_decimal"
                        },
                        &[self.frame, source.address, places],
                        true,
                    ),
                    Form::Unsigned,
                )
            }
            Timestamp => {
                let unit = expr.args[1]
                    .value
                    .as_str()
                    .filter(|_| matches!(expr.args[1].op, Literal))
                    .ok_or("Timestamp units must be known at construction")?;
                if unit == "rfc3339" {
                    let value = self.record(&expr.args[0])?;
                    (
                        self.call("typed_timestamp", &[self.frame, value.address], true),
                        Form::Unsigned,
                    )
                } else {
                    let factor = match unit {
                        "ns" => 1,
                        "us" => 1_000,
                        "ms" => 1_000_000,
                        "s" => 1_000_000_000,
                        _ => return Err("Unknown timestamp unit".into()),
                    };
                    let value = self.uint(&expr.args[0])?;
                    let limit = self.word(u64::MAX / factor);
                    let fits = self.builder.ins().icmp(
                        ir::condcodes::IntCC::UnsignedLessThanOrEqual,
                        value,
                        limit,
                    );
                    self.branch(
                        fits,
                        |_| Ok(()),
                        |e| {
                            let message = e.text_constant("Timestamp overflow");
                            e.call("typed_fail", &[e.frame, message], true);
                            Ok(())
                        },
                    )?;
                    (
                        self.builder.ins().imul_imm(value, factor as i64),
                        Form::Unsigned,
                    )
                }
            }
            Abs | Trim | StripPrefix => {
                let value = self.record(&expr.args[0])?;
                let result = if matches!(expr.op, StripPrefix) {
                    let prefix = self.record(&expr.args[1])?;
                    self.call(
                        "typed_strip",
                        &[self.frame, value.address, prefix.address],
                        true,
                    )
                } else {
                    self.call(
                        if matches!(expr.op, Abs) {
                            "typed_absolute"
                        } else {
                            "typed_trim"
                        },
                        &[self.frame, value.address],
                        true,
                    )
                };
                (result, Form::Record)
            }
            Array | Object => {
                let record = self.allocated_record(
                    shape,
                    if matches!(expr.op, Array) {
                        ARRAY
                    } else {
                        OBJECT
                    },
                );
                let count = if matches!(expr.op, Array) {
                    expr.args.len()
                } else {
                    self.owner.layout.shapes[shape].fields.len()
                };
                let size = self.word((count * size_of::<usize>()) as u64);
                let fields = self.call("typed_allocate", &[self.frame, size], false);
                if matches!(expr.op, Array) {
                    self.store(record.address, offset_of!(Record, elements), fields);
                    let count = self.word(count as u64);
                    self.store(
                        record.address,
                        offset_of!(Record, elements) + offset_of!(Span, length),
                        count,
                    );
                } else {
                    self.store(record.address, offset_of!(Record, fields), fields);
                    let length = self.word(expr.args.len() as u64);
                    self.store(record.address, offset_of!(Record, object_length), length);
                    let missing = self.word(self.owner.missing as u64);
                    for i in 0..count {
                        self.store(fields, i * size_of::<usize>(), missing);
                    }
                }
                for (i, expr_arg) in expr.args.iter().enumerate() {
                    let value = self.record(expr_arg)?;
                    let value = self.freeze(value);
                    let index = if matches!(expr.op, Array) {
                        i
                    } else {
                        let name = expr.path[i].as_str().ok_or("Object key must be text")?;
                        self.owner.layout.shapes[shape]
                            .fields
                            .keys()
                            .position(|field| field == name)
                            .ok_or("Object field missing its compiled slot")?
                    };
                    self.store(fields, index * size_of::<usize>(), value.address);
                }
                (record.address, Form::Record)
            }
            Map => return self.map_expression(expr, shape),
            Concat => {
                let mut values = Vec::new();
                for arg in &expr.args {
                    values.push(self.record(arg)?);
                }
                self.call("typed_concat_start", &[self.frame], false);
                for value in values {
                    self.call("typed_concat_append", &[self.frame, value.address], false);
                }
                (
                    self.call("typed_concat_finish", &[self.frame], false),
                    Form::Record,
                )
            }
        };
        Ok(ValueRef { value, form })
    }
    fn map_expression(&mut self, expr: &Expr, _shape: Id) -> Result<ValueRef, String> {
        let key = self.record(&expr.args[0])?;
        let table = self.record(&expr.args[1])?;
        let fields = self.owner.layout.shapes[table.shape].fields.clone();
        let done = self.builder.create_block();
        self.builder.append_block_param(done, types::I64);
        for (i, name) in fields.keys().enumerate() {
            let name = self.text_constant(name);
            let equal = self.call("typed_key_equal", &[key.address, name], false);
            let yes = self.builder.create_block();
            let next = self.builder.create_block();
            self.builder.ins().brif(equal, yes, &[], next, &[]);
            self.builder.switch_to_block(yes);
            let slots = self.load(table.address, offset_of!(Record, fields));
            let value = self.load(slots, i * size_of::<usize>());
            let present = self.load(value, offset_of!(Record, present));
            self.builder
                .ins()
                .brif(present, done, &[value.into()], next, &[]);
            self.builder.switch_to_block(next);
        }
        let fallback = self.record(&expr.args[2])?;
        self.builder.ins().jump(done, &[fallback.address.into()]);
        self.builder.switch_to_block(done);
        Ok(ValueRef {
            value: self.builder.block_params(done)[0],
            form: Form::Record,
        })
    }
    pub fn project_uint(&mut self, expr: &Expr, offset: usize) -> Result<(), String> {
        let value = self.uint(expr)?;
        self.store(self.frame, offset_of!(Frame, record) + offset, value);
        Ok(())
    }
    pub fn project_id(&mut self, expr: &Expr, offset: usize) -> Result<(), String> {
        let value = self.expression(expr)?;
        let destination = self
            .builder
            .ins()
            .iadd_imm(self.frame, (offset_of!(Frame, record) + offset) as i64);
        if matches!(value.form, Form::Unsigned) {
            self.call("typed_identifier_word", &[value.value, destination], false);
        } else {
            let shape = self.result(expr)?;
            let value = self.reference(value, shape);
            self.require_valid(value.address, VALID_ID, "Invalid order identifier");
            for i in (0..size_of::<lobo_primitives::uuid::Uuid>()).step_by(8) {
                let word = self.load(value.address, offset_of!(Record, identifier) + i);
                self.store(destination, i, word);
            }
        }
        Ok(())
    }
    pub fn project_side(&mut self, expr: &Expr) -> Result<(), String> {
        let value = self.record(expr)?;
        self.require_valid(value.address, VALID_SIDE, "Invalid order side");
        assert_eq!(size_of::<lobo_models::Side>(), 1);
        let value = self.load(value.address, offset_of!(Record, side));
        let value = self.builder.ins().ireduce(types::I8, value);
        self.builder.ins().store(
            MemFlagsData::new(),
            value,
            self.frame,
            (offset_of!(Frame, record) + offset_of!(OrderRecord, side)) as i32,
        );
        Ok(())
    }
    fn require_valid(&mut self, address: ir::Value, mask: u64, message: &str) {
        let valid = self.load(address, offset_of!(Record, valid));
        let valid = self.builder.ins().band_imm(valid, mask as i64);
        let yes = self.builder.create_block();
        let no = self.builder.create_block();
        self.builder.ins().brif(valid, yes, &[], no, &[]);
        self.builder.switch_to_block(no);
        let message = self.text(message);
        self.call("typed_fail", &[self.frame, message], false);
        self.builder.ins().jump(self.failure, &[]);
        self.builder.switch_to_block(yes);
    }
    fn read_unsigned(&mut self, address: ir::Value) -> ir::Value {
        self.require_valid(address, VALID_UNSIGNED, "Expected an unsigned integer");
        self.load(address, offset_of!(Record, unsigned))
    }
}

impl Emit<'_, '_> {
    pub fn text(&mut self, value: &str) -> ir::Value {
        self.text_constant(value)
    }
    pub fn ref_expr(&mut self, expr: &Expr) -> Result<ir::Value, String> {
        Ok(self.record(expr)?.address)
    }
    pub fn condition(&mut self, expr: &Expr) -> Result<ir::Value, String> {
        let v = self.expression(expr)?;
        Ok(self.boolean(v))
    }
    pub fn literal(&mut self, value: &Value) -> ir::Value {
        let shape = self.owner.layout.scalar();
        let address = self
            .owner
            .constant(value, shape)
            .expect("compiler scalar constant");
        self.word(address as u64)
    }
    pub fn record_store(&mut self, offset: usize, value: ir::Value) {
        self.store(self.frame, offset_of!(Frame, record) + offset, value);
    }
    pub fn project(
        &mut self,
        expr: &Expr,
        target: super::super::compiler::Target,
    ) -> Result<(), String> {
        use super::super::compiler::Target::*;
        match target {
            Id => self.project_id(expr, offset_of!(OrderRecord, id))?,
            NewId => self.project_id(expr, offset_of!(OrderRecord, new_id))?,
            Trader => self.project_id(expr, offset_of!(OrderRecord, trader))?,
            Side => self.project_side(expr)?,
            OptionalPrice | TradeId => {
                let (offset, flag) = if matches!(target, OptionalPrice) {
                    (
                        offset_of!(OrderRecord, price),
                        offset_of!(OrderRecord, has_price),
                    )
                } else {
                    (
                        offset_of!(OrderRecord, trade_id),
                        offset_of!(OrderRecord, has_trade_id),
                    )
                };
                if matches!(expr.op, Operator::Literal) && expr.value.is_null() {
                    let zero = self.word(0);
                    self.record_store(flag, zero);
                    return Ok(());
                }
                let value = self.expression(expr)?;
                if matches!(value.form, Form::Unsigned) {
                    let one = self.word(1);
                    self.record_store(flag, one);
                    self.record_store(offset, value.value);
                } else {
                    let shape = self.result(expr)?;
                    let value = self.reference(value, shape);
                    let present = self.call("typed_exists", &[value.address], false);
                    self.record_store(flag, present);
                    self.branch(
                        present,
                        |e| {
                            let number = e.read_unsigned(value.address);
                            e.record_store(offset, number);
                            Ok(())
                        },
                        |_| Ok(()),
                    )?;
                }
            }
            LevelPrice | LevelQuantity => {
                let (offset, width, slot) = if matches!(target, LevelPrice) {
                    (
                        offset_of!(OrderRecord, price),
                        offset_of!(OrderRecord, price_width),
                        "price_decimals",
                    )
                } else {
                    (
                        offset_of!(OrderRecord, quantity),
                        offset_of!(OrderRecord, quantity_width),
                        "quantity_decimals",
                    )
                };
                self.project_uint(expr, offset)?;
                let width = (
                    width,
                    if matches!(expr.op, Operator::Decimal) {
                        self.load(self.frame, offset_of!(Frame, scale))
                    } else {
                        let index = self
                            .owner
                            .layout
                            .variables
                            .keys()
                            .position(|name| name == slot)
                            .expect("precision slot");
                        let vars = self.load(self.frame, offset_of!(Frame, variables));
                        self.load(
                            vars,
                            index * size_of::<Record>() + offset_of!(Record, number),
                        )
                    },
                );
                self.record_store(width.0, width.1);
            }
            _ => {
                let offset = match target {
                    Time => offset_of!(OrderRecord, timestamp),
                    Quantity => offset_of!(OrderRecord, quantity),
                    HistoryId => offset_of!(OrderRecord, trade_id),
                    Hidden => offset_of!(OrderRecord, hidden_quantity),
                    Peak => offset_of!(OrderRecord, peak_quantity),
                    _ => offset_of!(OrderRecord, price),
                };
                self.project_uint(expr, offset)?;
                if matches!(target, Price) {
                    let one = self.word(1);
                    self.record_store(offset_of!(OrderRecord, has_price), one);
                }
            }
        }
        Ok(())
    }
    pub fn each(
        &mut self,
        value: View,
        body: impl FnOnce(&mut Self) -> Result<(), String>,
    ) -> Result<(), String> {
        let parent = self.item;
        let shape = self.owner.layout.shapes[value.shape]
            .element
            .ok_or("Iteration has no element layout")?;
        let length = self.load(
            value.address,
            offset_of!(Record, elements) + offset_of!(Span, length),
        );
        let rows = self.load(value.address, offset_of!(Record, elements));
        self.counted(length, |e, index| {
            let offset = e.builder.ins().ishl_imm(index, 3);
            let pointer = e.builder.ins().iadd(rows, offset);
            let address = e.load(pointer, 0);
            e.item = View { address, shape };
            body(e)
        })?;
        self.item = parent;
        Ok(())
    }
    pub fn foreach(
        &mut self,
        value: View,
        actions: &[Action],
        order: Option<&Expr>,
        unique: Option<&Expr>,
        in_book: bool,
    ) -> Result<(), String> {
        let levels = matches!(
            actions,
            [Action {
                operation: super::super::super::schema::Operation::Level { .. },
                ..
            }]
        );
        if order.is_some() || unique.is_some() {
            self.call("packet_sort_start", &[self.frame], false);
            self.each(value, |e| {
                let order = match order {
                    Some(expr) => e.ref_expr(expr)?,
                    None => e.word(e.owner.missing as u64),
                };
                let unique = match unique {
                    Some(expr) => e.ref_expr(expr)?,
                    None => e.word(e.owner.missing as u64),
                };
                e.call(
                    "packet_sort_push",
                    &[e.frame, e.item.address, order, unique],
                    true,
                );
                Ok(())
            })?;
            let name = if order.is_some() {
                "packet_sort_finish"
            } else {
                "packet_unique_finish"
            };
            let rows = self.call(name, &[self.frame], false);
            let parent = self.item;
            let shape = self.owner.layout.shapes[value.shape]
                .element
                .ok_or("Missing row layout")?;
            self.counted(rows, |e, index| {
                let address = e.call("packet_sort_row", &[e.frame, index], false);
                e.item = View { address, shape };
                e.actions(actions, in_book, levels)
            })?;
            self.item = parent;
            self.call("packet_sort_end", &[self.frame], false);
        } else {
            self.each(value, |e| e.actions(actions, in_book, levels))?;
        }
        if levels {
            self.call("packet_levels_flush", &[self.frame], true);
        }
        Ok(())
    }
    pub fn serialize(&mut self, value: View) -> Result<(), String> {
        self.call("packet_message_start", &[self.frame], false);
        self.serialize_value(value)
    }
    fn json_text(&mut self, text: &str) {
        let text = self.text(text);
        self.call("packet_message_text", &[self.frame, text], false);
    }
    fn serialize_value(&mut self, value: View) -> Result<(), String> {
        if self.owner.layout.shapes[value.shape].preserve {
            let raw = self.load(
                value.address,
                offset_of!(Record, raw) + offset_of!(Span, length),
            );
            let yes = self.builder.create_block();
            let no = self.builder.create_block();
            let done = self.builder.create_block();
            self.builder.ins().brif(raw, yes, &[], no, &[]);
            self.builder.switch_to_block(yes);
            self.call("packet_message_raw", &[self.frame, value.address], false);
            self.builder.ins().jump(done, &[]);
            self.builder.switch_to_block(no);
            self.serialize_fields(value)?;
            self.builder.ins().jump(done, &[]);
            self.builder.switch_to_block(done);
            return Ok(());
        }
        self.serialize_fields(value)
    }
    fn serialize_fields(&mut self, value: View) -> Result<(), String> {
        let kind = self.load(value.address, offset_of!(Record, kind));
        let scalar = self.builder.create_block();
        let object = self.builder.create_block();
        let array = self.builder.create_block();
        let done = self.builder.create_block();
        let mut switch = cranelift_frontend::Switch::new();
        switch.set_entry(OBJECT as u128, object);
        switch.set_entry(ARRAY as u128, array);
        switch.emit(&mut self.builder, kind, scalar);
        self.builder.switch_to_block(scalar);
        self.call("packet_message_scalar", &[self.frame, value.address], true);
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(object);
        self.json_text("{");
        let fields = self.owner.layout.shapes[value.shape].fields.clone();
        for (index, (name, child)) in fields.iter().enumerate() {
            if index != 0 {
                self.json_text(",");
            }
            self.json_text(&format!(
                "{}:",
                serde_json::to_string(name).map_err(|e| e.to_string())?
            ));
            let slots = self.load(value.address, offset_of!(Record, fields));
            let address = self.load(slots, index * size_of::<usize>());
            self.serialize_value(View {
                address,
                shape: *child,
            })?;
        }
        self.json_text("}");
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(array);
        self.json_text("[");
        if self.owner.layout.shapes[value.shape].element.is_some() {
            let rows = self.load(value.address, offset_of!(Record, elements));
            let length = self.load(
                value.address,
                offset_of!(Record, elements) + offset_of!(Span, length),
            );
            let child = self.owner.layout.shapes[value.shape]
                .element
                .expect("checked at construction");
            self.counted(length, |e, index| {
                e.branch(
                    index,
                    |e| {
                        e.json_text(",");
                        Ok(())
                    },
                    |_| Ok(()),
                )?;
                let index = e.builder.ins().ishl_imm(index, 3);
                let pointer = e.builder.ins().iadd(rows, index);
                let address = e.load(pointer, 0);
                e.serialize_value(View {
                    address,
                    shape: child,
                })
            })?;
        }
        self.json_text("]");
        self.builder.ins().jump(done, &[]);
        self.builder.switch_to_block(done);
        Ok(())
    }
}
impl Emit<'_, '_> {
    pub fn decode_root(&mut self, shape: Id) -> Result<(), String> {
        let root = self.allocated_record(shape, NULL);
        let scan = self.call("packet_scan_start", &[self.frame], false);
        self.call(&format!("decode_{shape}"), &[scan, root.address], false);
        self.call("packet_scan_finish", &[self.frame, scan], true);
        self.root = root;
        self.item = root;
        Ok(())
    }
    pub fn binary_record(
        &mut self,
        record: &super::super::super::schema::Record,
    ) -> Result<(), String> {
        let root = self.allocated_record(self.owner.layout.root, OBJECT);
        let count = self.owner.layout.shapes[root.shape].fields.len();
        let slots = self.builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            (count.max(1) * size_of::<usize>()) as u32,
            3,
        ));
        let fields = self.builder.ins().stack_addr(types::I64, slots, 0);
        let missing = self.word(self.owner.missing as u64);
        for index in 0..count {
            self.store(fields, index * size_of::<usize>(), missing);
        }
        self.store(root.address, offset_of!(Record, fields), fields);
        let bytes = self.load(self.frame, offset_of!(Frame, bytes));
        for (name, field) in &record.fields {
            let (index, (_, shape)) = self.owner.layout.shapes[root.shape]
                .fields
                .iter()
                .enumerate()
                .find(|(_, (key, _))| *key == name)
                .ok_or("Missing binary field layout")?;
            let shape = *shape;
            let value = if field.kind == "text" {
                let offset = self.word(field.offset as u64);
                let length = self.word(field.size as u64);
                View {
                    address: self.call("packet_binary_text", &[self.frame, offset, length], true),
                    shape,
                }
            } else {
                let value = super::super::binary::integer(&mut self.builder, bytes, field);
                self.reference(
                    ValueRef {
                        value,
                        form: Form::Unsigned,
                    },
                    shape,
                )
            };
            self.store(fields, index * size_of::<usize>(), value.address);
        }
        self.root = root;
        self.item = root;
        Ok(())
    }
}

impl Emit<'_, '_> {
    fn wire_field(
        &mut self,
        field: &super::super::super::schema::BinaryField,
        _shape: Id,
    ) -> Result<ValueRef, String> {
        if field.kind == "text" {
            let offset = self.word(field.offset as u64);
            let length = self.word(field.size as u64);
            return Ok(ValueRef {
                value: self.call("packet_binary_text", &[self.frame, offset, length], true),
                form: Form::Record,
            });
        }
        let bytes = self.load(self.frame, offset_of!(Frame, bytes));
        let value = super::super::binary::integer(&mut self.builder, bytes, field);
        Ok(ValueRef {
            value,
            form: Form::Unsigned,
        })
    }
}

impl Emit<'_, '_> {
    fn equal_views(&mut self, a: View, b: View) -> Result<ir::Value, String> {
        let shape = self.owner.layout.shapes[a.shape].clone();
        if shape.fields.is_empty() && shape.element.is_none() {
            return Ok(self.call("typed_equal", &[self.frame, a.address, b.address], true));
        }
        let done = self.builder.create_block();
        self.builder.append_block_param(done, types::I64);
        let same = self.builder.create_block();
        let ka = self.load(a.address, offset_of!(Record, kind));
        let kb = self.load(b.address, offset_of!(Record, kind));
        let eq = self.builder.ins().icmp(ir::condcodes::IntCC::Equal, ka, kb);
        let zero = self.word(0);
        let one = self.word(1);
        self.builder.ins().brif(eq, same, &[], done, &[zero.into()]);
        self.builder.switch_to_block(same);
        let object = self.builder.create_block();
        let array = self.builder.create_block();
        let scalar = self.builder.create_block();
        let mut switch = cranelift_frontend::Switch::new();
        switch.set_entry(OBJECT as u128, object);
        switch.set_entry(ARRAY as u128, array);
        switch.emit(&mut self.builder, ka, scalar);
        self.builder.switch_to_block(scalar);
        let equal = self.call("typed_equal", &[self.frame, a.address, b.address], true);
        self.builder.ins().jump(done, &[equal.into()]);
        self.builder.switch_to_block(object);
        let al = self.load(a.address, offset_of!(Record, object_length));
        let bl = self.load(b.address, offset_of!(Record, object_length));
        let eq = self.builder.ins().icmp(ir::condcodes::IntCC::Equal, al, bl);
        let fields = self.builder.create_block();
        self.builder
            .ins()
            .brif(eq, fields, &[], done, &[zero.into()]);
        self.builder.switch_to_block(fields);
        let af = self.load(a.address, offset_of!(Record, fields));
        let bf = self.load(b.address, offset_of!(Record, fields));
        for (index, child) in shape.fields.values().enumerate() {
            let aa = self.load(af, index * size_of::<usize>());
            let ba = self.load(bf, index * size_of::<usize>());
            let ap = self.load(aa, offset_of!(Record, present));
            let bp = self.load(ba, offset_of!(Record, present));
            let eq = self.builder.ins().icmp(ir::condcodes::IntCC::Equal, ap, bp);
            let next = self.builder.create_block();
            self.builder.ins().brif(eq, next, &[], done, &[zero.into()]);
            self.builder.switch_to_block(next);
            let eq = self.equal_views(
                View {
                    address: aa,
                    shape: *child,
                },
                View {
                    address: ba,
                    shape: *child,
                },
            )?;
            let next = self.builder.create_block();
            self.builder.ins().brif(eq, next, &[], done, &[zero.into()]);
            self.builder.switch_to_block(next);
        }
        self.builder.ins().jump(done, &[one.into()]);
        self.builder.switch_to_block(array);
        let al = self.load(
            a.address,
            offset_of!(Record, elements) + offset_of!(Span, length),
        );
        let bl = self.load(
            b.address,
            offset_of!(Record, elements) + offset_of!(Span, length),
        );
        let eq = self.builder.ins().icmp(ir::condcodes::IntCC::Equal, al, bl);
        let rows = self.builder.create_block();
        self.builder.ins().brif(eq, rows, &[], done, &[zero.into()]);
        self.builder.switch_to_block(rows);
        if let Some(child) = shape.element {
            let aa = self.load(a.address, offset_of!(Record, elements));
            let ba = self.load(b.address, offset_of!(Record, elements));
            self.counted(al, |e, index| {
                let offset = e.builder.ins().ishl_imm(index, 3);
                let ap = e.builder.ins().iadd(aa, offset);
                let bp = e.builder.ins().iadd(ba, offset);
                let ap = e.load(ap, 0);
                let bp = e.load(bp, 0);
                let eq = e.equal_views(
                    View {
                        address: ap,
                        shape: child,
                    },
                    View {
                        address: bp,
                        shape: child,
                    },
                )?;
                let next = e.builder.create_block();
                e.builder.ins().brif(eq, next, &[], done, &[zero.into()]);
                e.builder.switch_to_block(next);
                Ok(())
            })?;
        }
        self.builder.ins().jump(done, &[one.into()]);
        self.builder.switch_to_block(done);
        Ok(self.builder.block_params(done)[0])
    }
}
