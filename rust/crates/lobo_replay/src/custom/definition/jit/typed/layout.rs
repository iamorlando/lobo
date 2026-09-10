//! Construction-time shape inference. Names exist in this module, never in
//! generated field/variable reads. A row has a fixed projection layout even
//! when the wire protocol permits several JSON representations of that row.
use super::super::super::{
    expression::{Expr, Operator},
    schema::{Action, Definition, Format, Operation},
};
use super::storage::*;
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) type Id = usize;

#[derive(Clone, Default, Debug)]
pub(super) struct Shape {
    pub fields: BTreeMap<String, Id>,
    pub element: Option<Id>,
    pub numeric: bool,
    pub identifier: bool,
    pub side: bool,
    pub preserve: bool,
}

#[derive(Default)]
pub(super) struct Layout {
    parent: Vec<Id>,
    pub shapes: Vec<Shape>,
    pub variables: BTreeMap<String, Id>,
    pub tables: BTreeMap<String, Id>,
    pub root: Id,
    pub bootstrap: BTreeMap<String, Id>,
    expressions: BTreeMap<(Id, Id, String), Id>,
}

impl Layout {
    pub fn new(definition: &Definition) -> Result<Self, String> {
        let mut layout = Self::default();
        layout.root = layout.node();
        for name in [
            "connection",
            "clock",
            "symbol",
            "price_decimals",
            "quantity_decimals",
            "key",
        ] {
            layout.variable(name);
        }
        match &definition.format {
            Format::Json { messages } => {
                for message in messages {
                    layout.expression(&message.condition, layout.root, layout.root)?;
                    layout.actions(&message.actions, layout.root, layout.root)?;
                }
            }
            Format::Binary(spec) => {
                for record in spec.records.values() {
                    for name in record.fields.keys() {
                        layout.field(layout.root, name);
                    }
                    layout.binary_groups(&record.groups, layout.root);
                    layout.actions(&record.actions, layout.root, layout.root)?;
                }
            }
        }
        for actions in [
            &definition.connect,
            &definition.subscriptions,
            &definition.keepalive,
        ] {
            layout.actions(actions, layout.root, layout.root)?;
        }
        for bootstrap in &definition.bootstrap {
            let root = layout.node();
            layout.actions(&bootstrap.actions, root, root)?;
            layout.bootstrap.insert(bootstrap.name.clone(), root);
        }
        layout.normalize();
        layout.check_acyclic()?;
        Ok(layout)
    }

    fn binary_groups(&mut self, groups: &[crate::custom::definition::schema::Group], parent: Id) {
        for group in groups {
            let array = self.field(parent, &group.name);
            let entry = self.element(array);
            for name in group.fields.keys() {
                self.field(entry, name);
            }
            self.binary_groups(&group.groups, entry);
        }
    }

    fn node(&mut self) -> Id {
        let id = self.shapes.len();
        self.shapes.push(Shape::default());
        self.parent.push(id);
        id
    }

    pub fn canonical(&self, mut id: Id) -> Id {
        while self.parent[id] != id {
            id = self.parent[id];
        }
        id
    }

    fn join(&mut self, a: Id, b: Id) -> Id {
        let a = self.canonical(a);
        let b = self.canonical(b);
        if a == b {
            return a;
        }
        self.parent[b] = a;
        let other = std::mem::take(&mut self.shapes[b]);
        self.shapes[a].numeric |= other.numeric;
        self.shapes[a].identifier |= other.identifier;
        self.shapes[a].side |= other.side;
        self.shapes[a].preserve |= other.preserve;
        for (name, child) in other.fields {
            match self.shapes[a].fields.get(&name).copied() {
                Some(old) => {
                    self.join(old, child);
                }
                None => {
                    self.shapes[a].fields.insert(name, child);
                }
            }
        }
        match (self.shapes[a].element, other.element) {
            (Some(old), Some(child)) => {
                self.join(old, child);
            }
            (None, child) => self.shapes[a].element = child,
            _ => {}
        }
        a
    }

    fn variable(&mut self, name: &str) -> Id {
        if let Some(id) = self.variables.get(name) {
            return *id;
        }
        let id = self.node();
        self.variables.insert(name.to_owned(), id);
        id
    }

    fn table(&mut self, name: &str) -> Id {
        if let Some(id) = self.tables.get(name) {
            return *id;
        }
        let id = self.node();
        self.tables.insert(name.to_owned(), id);
        id
    }

    fn field(&mut self, parent: Id, name: &str) -> Id {
        let parent = self.canonical(parent);
        if let Some(id) = self.shapes[parent].fields.get(name) {
            return *id;
        }
        let child = self.node();
        self.shapes[parent].fields.insert(name.to_owned(), child);
        child
    }

    fn element(&mut self, parent: Id) -> Id {
        let parent = self.canonical(parent);
        if let Some(id) = self.shapes[parent].element {
            return id;
        }
        let child = self.node();
        self.shapes[parent].element = Some(child);
        child
    }

    fn path(&mut self, mut parent: Id, path: &[Value]) -> Result<Id, String> {
        for part in path {
            parent = match part {
                Value::String(name) => self.field(parent, name),
                Value::Number(n) if n.as_i64().is_some() => self.element(parent),
                _ => {
                    return Err(
                        "A compiled field path requires string keys or integer indices".into(),
                    );
                }
            };
        }
        Ok(parent)
    }

    fn constant(&mut self, value: &Value) -> Id {
        let node = self.node();
        match value {
            Value::Object(fields) => {
                for (name, value) in fields {
                    let child = self.constant(value);
                    self.shapes[node].fields.insert(name.clone(), child);
                }
            }
            Value::Array(values) => {
                let child = self.element(node);
                for value in values {
                    let value = self.constant(value);
                    self.join(child, value);
                }
            }
            _ => {}
        }
        node
    }

    fn expression(&mut self, expr: &Expr, item: Id, root: Id) -> Result<Id, String> {
        use Operator::*;
        let key = (
            item,
            root,
            serde_json::to_string(expr).map_err(|e| e.to_string())?,
        );
        if let Some(id) = self.expressions.get(&key) {
            return Ok(*id);
        }
        let id = match expr.op {
            Field => self.path(item, &expr.path)?,
            Root => self.path(root, &expr.path)?,
            Variable => self.variable(&expr.name),
            Literal => self.constant(&expr.value),
            Lookup => {
                self.expression(&expr.args[1], item, root)?;
                let name = expr.args[0]
                    .value
                    .as_str()
                    .filter(|_| matches!(expr.args[0].op, Literal))
                    .ok_or("A compiled lookup requires a table name known at construction")?;
                self.table(name)
            }
            Get => {
                let value = self.expression(&expr.args[0], item, root)?;
                let path = expr.args[1]
                    .value
                    .as_array()
                    .filter(|_| matches!(expr.args[1].op, Literal))
                    .ok_or("A compiled get requires a field path known at construction")?;
                self.path(value, path)?
            }
            Choose => {
                self.expression(&expr.args[0], item, root)?;
                let yes = self.expression(&expr.args[1], item, root)?;
                let no = self.expression(&expr.args[2], item, root)?;
                self.join(yes, no)
            }
            Array => {
                let array = self.node();
                let child = self.element(array);
                for arg in &expr.args {
                    let value = self.expression(arg, item, root)?;
                    self.join(child, value);
                }
                array
            }
            Object => {
                let object = self.node();
                for (name, arg) in expr.path.iter().zip(&expr.args) {
                    let value = self.expression(arg, item, root)?;
                    self.shapes[object].fields.insert(
                        name.as_str().ok_or("Object key must be text")?.into(),
                        value,
                    );
                }
                object
            }
            Eq | Ne => {
                let a = self.expression(&expr.args[0], item, root)?;
                let b = self.expression(&expr.args[1], item, root)?;
                self.join(a, b);
                self.node()
            }
            Map => {
                self.expression(&expr.args[0], item, root)?;
                let table = self.expression(&expr.args[1], item, root)?;
                let result = self.expression(&expr.args[2], item, root)?;
                let table = self.canonical(table);
                for child in self.shapes[table]
                    .fields
                    .values()
                    .copied()
                    .collect::<Vec<_>>()
                {
                    self.join(result, child);
                }
                result
            }
            _ => {
                for arg in &expr.args {
                    self.expression(arg, item, root)?;
                }
                self.node()
            }
        };
        if matches!(expr.op, Decimal | Abs | Gt | Lt | Timestamp) {
            for arg in &expr.args {
                let value = self.expression(arg, item, root)?;
                let value = self.canonical(value);
                self.shapes[value].numeric = true;
            }
        }
        self.expressions.insert(key, id);
        Ok(id)
    }

    fn actions(&mut self, actions: &[Action], item: Id, root: Id) -> Result<(), String> {
        use Operation::*;
        for action in actions {
            self.projections(&action.operation, item, root)?;
            match &action.operation {
                ForEach {
                    items,
                    actions,
                    order_by,
                    unique_by,
                } => {
                    let array = self.expression(items, item, root)?;
                    let row = self.element(array);
                    for key in [order_by, unique_by].into_iter().flatten() {
                        self.expression(key, row, root)?;
                    }
                    self.actions(actions, row, root)?;
                }
                Directory {
                    items,
                    symbol,
                    actions,
                    on_remove,
                } => {
                    let array = self.expression(items, item, root)?;
                    let row = self.element(array);
                    self.expression(symbol, row, root)?;
                    self.actions(actions, row, root)?;
                    self.actions(on_remove, item, root)?;
                }
                Send { message } => {
                    let value = self.expression(message, item, root)?;
                    self.preserve(value);
                }
                Let { name, value } => {
                    let value = self.expression(value, item, root)?;
                    let variable = self.variable(name);
                    self.join(variable, value);
                }
                Remember { table, key, value } => {
                    self.expression(key, item, root)?;
                    let value = self.expression(value, item, root)?;
                    let table = self.table(table);
                    self.join(table, value);
                }
                When {
                    condition,
                    actions,
                    otherwise,
                } => {
                    self.expression(condition, item, root)?;
                    self.actions(actions, item, root)?;
                    self.actions(otherwise, item, root)?;
                }
                Book {
                    symbol,
                    actions,
                    snapshot,
                    timestamp,
                    checksum,
                    ..
                } => {
                    for expr in [symbol, snapshot, timestamp] {
                        self.expression(expr, item, root)?;
                    }
                    self.actions(actions, item, root)?;
                    if let Some(checksum) = checksum {
                        self.expression(&checksum.expected, item, root)?;
                        self.actions(&checksum.on_failure, item, root)?;
                    }
                }
                OrderCommand { value, timestamp } => {
                    let command = self.expression(value, item, root)?;
                    self.expression(timestamp, item, root)?;
                    for name in ["op", "id", "quantity", "price", "new_id"] {
                        self.field(command, name);
                    }
                    let order = self.field(command, "order");
                    for name in [
                        "type",
                        "id",
                        "trader",
                        "side",
                        "quantity",
                        "price",
                        "hidden_quantity",
                        "peak_quantity",
                    ] {
                        self.field(order, name);
                    }
                }
                RestoreOrder { value } => {
                    let order = self.expression(value, item, root)?;
                    for name in [
                        "id",
                        "trader",
                        "side",
                        "quantity",
                        "price",
                        "hidden_quantity",
                        "peak_quantity",
                        "created_at_ns",
                    ] {
                        self.field(order, name);
                    }
                }
                _ => {
                    // The schema is inspected only at construction. This also
                    // keeps new scalar operations from needing a second list
                    // of operand names here.
                    self.operands(
                        &serde_json::to_value(&action.operation).map_err(|e| e.to_string())?,
                        item,
                        root,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn operands(&mut self, value: &Value, item: Id, root: Id) -> Result<(), String> {
        match value {
            Value::Object(map) if map.contains_key("op") => {
                let expr = serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
                self.expression(&expr, item, root)?;
            }
            Value::Object(map) => {
                for value in map.values() {
                    self.operands(value, item, root)?;
                }
            }
            Value::Array(values) => {
                for value in values {
                    self.operands(value, item, root)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn normalize(&mut self) {
        for id in 0..self.shapes.len() {
            let fields = self.shapes[id]
                .fields
                .iter()
                .map(|(name, child)| (name.clone(), self.canonical(*child)))
                .collect();
            let element = self.shapes[id].element.map(|child| self.canonical(child));
            self.shapes[id].fields = fields;
            self.shapes[id].element = element;
        }
        self.root = self.canonical(self.root);
        self.variables = self
            .variables
            .iter()
            .map(|(name, id)| (name.clone(), self.canonical(*id)))
            .collect();
        self.tables = self
            .tables
            .iter()
            .map(|(name, id)| (name.clone(), self.canonical(*id)))
            .collect();
        self.bootstrap = self
            .bootstrap
            .iter()
            .map(|(name, id)| (name.clone(), self.canonical(*id)))
            .collect();
        self.expressions = self
            .expressions
            .iter()
            .map(|((item, root, expr), id)| {
                (
                    (self.canonical(*item), self.canonical(*root), expr.clone()),
                    self.canonical(*id),
                )
            })
            .collect();
    }

    fn cached_result(&self, expr: &Expr, item: Id, root: Id) -> Result<Id, String> {
        self.expressions
            .get(&(
                self.canonical(item),
                self.canonical(root),
                serde_json::to_string(expr).map_err(|e| e.to_string())?,
            ))
            .copied()
            .ok_or_else(|| "Expression is missing its compiled layout".into())
    }

    fn check_acyclic(&self) -> Result<(), String> {
        fn visit(layout: &Layout, id: Id, colors: &mut [u8]) -> Result<(), String> {
            if colors[id] == 1 {
                return Err("The declaration creates a recursive saved-value layout".into());
            }
            if colors[id] == 2 {
                return Ok(());
            }
            colors[id] = 1;
            let node = &layout.shapes[id];
            for child in node.fields.values().copied().chain(node.element) {
                visit(layout, child, colors)?;
            }
            colors[id] = 2;
            Ok(())
        }
        let mut colors = vec![0; self.shapes.len()];
        for id in 0..self.shapes.len() {
            if self.canonical(id) == id {
                visit(self, id, &mut colors)?;
            }
        }
        Ok(())
    }
}

impl Layout {
    pub fn scalar(&self) -> Id {
        self.shapes
            .iter()
            .enumerate()
            .find(|(id, s)| {
                self.canonical(*id) == *id && s.fields.is_empty() && s.element.is_none()
            })
            .map(|(id, _)| id)
            .expect("scalar layout")
    }
    pub fn result(&self, expr: &Expr, item: Id, root: Id) -> Result<Id, String> {
        if let Ok(id) = self.cached_result(expr, item, root) {
            return Ok(id);
        }
        let path = |mut id: Id, path: &[Value]| -> Result<Id, String> {
            for part in path {
                id = match part {
                    Value::String(name) => {
                        *self.shapes[id].fields.get(name).ok_or("Undeclared field")?
                    }
                    Value::Number(_) => self.shapes[id].element.ok_or("Undeclared row")?,
                    _ => return Err("Invalid path".into()),
                };
            }
            Ok(id)
        };
        match expr.op {
            Operator::Field => path(item, &expr.path),
            Operator::Root => path(root, &expr.path),
            Operator::Literal if !expr.value.is_object() && !expr.value.is_array() => {
                Ok(self.scalar())
            }
            Operator::Get => path(
                self.result(&expr.args[0], item, root)?,
                expr.args[1]
                    .value
                    .as_array()
                    .ok_or("Static path required")?,
            ),
            _ => Err(format!("Missing compiled layout for {:?}", expr)),
        }
    }
}

impl Layout {
    fn require(&mut self, expr: &Expr, item: Id, root: Id, kind: u8) -> Result<(), String> {
        if matches!(expr.op, Operator::Choose) {
            self.require(&expr.args[1], item, root, kind)?;
            self.require(&expr.args[2], item, root, kind)?;
        }
        let id = self.expression(expr, item, root)?;
        let id = self.canonical(id);
        match kind {
            1 => self.shapes[id].identifier = true,
            2 => self.shapes[id].side = true,
            _ => self.shapes[id].numeric = true,
        }
        Ok(())
    }
    fn preserve(&mut self, id: Id) {
        let id = self.canonical(id);
        if self.shapes[id].preserve {
            return;
        }
        self.shapes[id].preserve = true;
        let children = self.shapes[id]
            .fields
            .values()
            .copied()
            .chain(self.shapes[id].element)
            .collect::<Vec<_>>();
        for child in children {
            self.preserve(child);
        }
    }
    fn projections(&mut self, op: &Operation, item: Id, root: Id) -> Result<(), String> {
        use Operation::*;
        let fields: Vec<(&Expr, u8)> = match op {
            Add {
                timestamp,
                id,
                side,
                price,
                quantity,
            } => vec![
                (timestamp, 0),
                (id, 1),
                (side, 2),
                (price, 0),
                (quantity, 0),
            ],
            Execute {
                timestamp,
                id,
                price,
                quantity,
            } => vec![(timestamp, 0), (id, 1), (price, 0), (quantity, 0)],
            Cancel {
                timestamp,
                id,
                quantity,
            }
            | Modify {
                timestamp,
                id,
                quantity,
            } => vec![(timestamp, 0), (id, 1), (quantity, 0)],
            Remove { timestamp, id } => vec![(timestamp, 0), (id, 1)],
            Replace {
                timestamp,
                id,
                new_id,
                price,
                quantity,
            } => vec![
                (timestamp, 0),
                (id, 1),
                (new_id, 1),
                (price, 0),
                (quantity, 0),
            ],
            Upsert {
                timestamp,
                id,
                side,
                price,
                quantity,
            } => vec![
                (timestamp, 0),
                (id, 1),
                (side, 2),
                (price, 0),
                (quantity, 0),
            ],
            Trade {
                timestamp,
                id,
                side,
                price,
                quantity,
            } => vec![
                (timestamp, 0),
                (id, 0),
                (side, 2),
                (price, 0),
                (quantity, 0),
            ],
            TradeHistory { id } => vec![(id, 0)],
            Level {
                side,
                price,
                quantity,
            } => vec![(side, 2), (price, 0), (quantity, 0)],
            Sequence { value, .. } => vec![(value, 0)],
            Register {
                price_decimals,
                quantity_decimals,
                key,
                ..
            } => vec![(price_decimals, 0), (quantity_decimals, 0), (key, 0)],
            Book { timestamp, .. } => vec![(timestamp, 0)],
            OrderCommand { value, .. } => {
                let cmd = self.expression(value, item, root)?;
                let order = self.field(cmd, "order");
                for (parent, names, kind) in [
                    (cmd, &["id", "new_id"][..], 1),
                    (cmd, &["quantity", "price"][..], 0),
                    (order, &["id", "trader"][..], 1),
                    (order, &["side"][..], 2),
                    (
                        order,
                        &["quantity", "price", "hidden_quantity", "peak_quantity"][..],
                        0,
                    ),
                ] {
                    for name in names {
                        let node = self.field(parent, name);
                        let node = self.canonical(node);
                        match kind {
                            1 => self.shapes[node].identifier = true,
                            2 => self.shapes[node].side = true,
                            _ => self.shapes[node].numeric = true,
                        }
                    }
                }
                vec![]
            }
            RestoreOrder { value } => {
                let order = self.expression(value, item, root)?;
                for (names, kind) in [
                    (&["id", "trader"][..], 1),
                    (&["side"][..], 2),
                    (
                        &[
                            "quantity",
                            "price",
                            "hidden_quantity",
                            "peak_quantity",
                            "created_at_ns",
                        ][..],
                        0,
                    ),
                ] {
                    for name in names {
                        let node = self.field(order, name);
                        let node = self.canonical(node);
                        match kind {
                            1 => self.shapes[node].identifier = true,
                            2 => self.shapes[node].side = true,
                            _ => self.shapes[node].numeric = true,
                        }
                    }
                }
                vec![]
            }
            _ => vec![],
        };
        for (expr, kind) in fields {
            self.require(expr, item, root, kind)?;
        }
        Ok(())
    }
}

impl Layout {
    /// Project native values into the same slots used by generated actions.
    pub(super) fn project(
        &self,
        value: &Value,
        shape: Id,
        arena: &mut Arena,
        missing_address: usize,
    ) -> Result<usize, String> {
        let missing = unsafe { *(missing_address as *const Record) };
        let mut record = Record {
            present: 1,
            ..missing
        };
        match value {
            Value::Null => {}
            Value::Bool(value) => {
                record.kind = BOOL;
                record.number.low = u64::from(*value);
            }
            Value::Number(value) => {
                record.kind = NUMBER;
                record.number(Number::parse(value.as_str().as_bytes()).map_err(str::to_owned)?);
                if record.valid & VALID_UNSIGNED != 0 {
                    record.identifier =
                        lobo_primitives::uuid::Uuid::from_u128(u128::from(record.unsigned));
                    record.valid |= VALID_ID;
                }
                record.text = arena.bytes(value.as_str().as_bytes());
            }
            Value::String(value) => {
                record.kind = TEXT;
                record.text = arena.bytes(value.as_bytes());
                record.text_projection::<true, true, true>();
            }
            Value::Array(values) => {
                record.kind = ARRAY;
                let child = self.shapes[shape]
                    .element
                    .ok_or("Array constant has no row layout")?;
                let rows = arena.allocate::<usize>(values.len());
                for (i, value) in values.iter().enumerate() {
                    let value = self.project(value, child, arena, missing_address)?;
                    unsafe {
                        rows.add(i).write(value);
                    }
                }
                record.elements = Span {
                    address: rows as usize,
                    length: values.len(),
                };
            }
            Value::Object(values) => {
                record.kind = OBJECT;
                let shape = self.shapes[shape].clone();
                let fields = arena.allocate::<usize>(shape.fields.len());
                for (i, (name, child)) in shape.fields.iter().enumerate() {
                    let value = match values.get(name) {
                        Some(value) => self.project(value, *child, arena, missing_address)?,
                        None => missing_address,
                    };
                    unsafe {
                        fields.add(i).write(value);
                    }
                }
                record.fields = fields as usize;
                record.object_length = values.len() as u64;
            }
        }
        Ok(arena.put(record) as usize)
    }
}
