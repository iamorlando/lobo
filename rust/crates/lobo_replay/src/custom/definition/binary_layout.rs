//! Checked, protocol-independent variable binary layouts shared by native paths.
use super::schema::{Binary, BinaryField, Group, Record};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};

fn extent(fields: &BTreeMap<String, BinaryField>) -> usize {
    fields
        .values()
        .map(|f| f.offset + f.size)
        .max()
        .unwrap_or(0)
}

pub(super) fn uint(field: &BinaryField, maximum: usize) -> Result<(), String> {
    field.validate(maximum)?;
    if field.kind != "uint" {
        return Err("Binary framing and dimension fields must be UInt".into());
    }
    Ok(())
}

fn validate_groups(
    fields: &BTreeMap<String, BinaryField>,
    groups: &[Group],
    maximum: usize,
    depth: usize,
) -> Result<(), String> {
    if depth > 16 {
        return Err("Binary groups cannot nest more than 16 levels".into());
    }
    let mut names: BTreeSet<_> = fields.keys().map(String::as_str).collect();
    for group in groups {
        if group.name.is_empty() || !names.insert(&group.name) {
            return Err("Binary field and group names must be nonempty and unique".into());
        }
        if group.header_size == 0
            || group.header_size > maximum
            || group.max_count == 0
            || group.max_count > 65535
            || !group.alignment.is_power_of_two()
            || group.alignment > maximum
            || group.offset.is_some_and(|offset| offset > maximum)
        {
            return Err("Invalid binary group header, count bound, offset or alignment".into());
        }
        uint(&group.count, group.header_size)?;
        uint(&group.block_length, group.header_size)?;
        for field in group.fields.values() {
            field.validate(maximum)?;
        }
        validate_groups(&group.fields, &group.groups, maximum, depth + 1)?;
    }
    Ok(())
}

impl Binary {
    pub(crate) fn minimum_header(&self) -> usize {
        [&self.tag, &self.key, &self.timestamp]
            .into_iter()
            .map(|f| f.offset + f.size)
            .max()
            .unwrap_or(0)
    }

    /// Convert a transmitted length to payload bytes, checking the configured bound.
    /// Payload offsets always start immediately after the prefix.
    pub fn payload_length(&self, encoded: u64) -> Result<usize, String> {
        payload_length(
            encoded,
            self.length.size,
            self.length_includes_prefix,
            self.max_record_size,
        )
    }
}

pub(super) fn payload_length(
    encoded: u64,
    prefix: usize,
    inclusive: bool,
    maximum: usize,
) -> Result<usize, String> {
    let size = usize::try_from(encoded)
        .map_err(|_| "Invalid record length")?
        .checked_sub(if inclusive { prefix } else { 0 })
        .ok_or("Invalid record length")?;
    if size == 0 || size > maximum {
        return Err("Invalid record length".into());
    }
    Ok(size)
}

impl Record {
    pub(crate) fn variable(&self) -> bool {
        self.size.is_none() || self.block_length.is_some() || !self.groups.is_empty()
    }

    pub(crate) fn validate_layout(&self, maximum: usize, header: usize) -> Result<(), String> {
        let limit = self.size.unwrap_or(maximum);
        if limit == 0 || limit > maximum || header > limit || self.block_offset > limit {
            return Err("Invalid binary record size".into());
        }
        for field in self.fields.values() {
            field.validate(limit)?;
        }
        if let Some(length) = &self.block_length {
            uint(length, limit)?;
        }
        if self.block_length.is_none() && self.block_offset != 0 {
            return Err("block_offset requires block_length".into());
        }
        if self.size.is_some() && self.allow_trailing {
            return Err("allow_trailing requires size=None".into());
        }
        validate_groups(&self.fields, &self.groups, limit, 0)
    }

    /// Decode and validate the complete payload before applying its actions.
    /// Returned objects and arrays remain native Rust values; no Python callback
    /// or intermediate text encoding participates in binary replay.
    pub(crate) fn decode(&self, bytes: &[u8], header: usize) -> Result<Value, String> {
        if self.size.is_some_and(|size| size != bytes.len()) {
            return Err("Binary record has an unexpected length".into());
        }
        let minimum = extent(&self.fields).max(header).max(
            self.block_length
                .as_ref()
                .map_or(0, |field| field.offset + field.size),
        );
        let mut cursor = if let Some(field) = &self.block_length {
            let length =
                usize::try_from(field.number(bytes)?).map_err(|_| "Invalid root block length")?;
            self.block_offset
                .checked_add(length)
                .ok_or("Invalid root block length")?
        } else if self.groups.is_empty() {
            self.size.unwrap_or(minimum)
        } else {
            minimum
        };
        if cursor < minimum || cursor > bytes.len() {
            return Err("Truncated or undersized binary root block".into());
        }
        let mut object = fields(&self.fields, &bytes[..cursor])?;
        // A message-wide bound also limits multiplication through nested groups.
        let mut entries_left = 65535usize;
        groups(
            &self.groups,
            bytes,
            &mut cursor,
            &mut object,
            &mut entries_left,
        )?;
        if cursor != bytes.len() && !self.allow_trailing {
            return Err("Binary record has undeclared trailing bytes".into());
        }
        Ok(Value::Object(object))
    }
}

fn fields(
    spec: &BTreeMap<String, BinaryField>,
    bytes: &[u8],
) -> Result<Map<String, Value>, String> {
    spec.iter()
        .map(|(name, field)| field.value(bytes).map(|value| (name.clone(), value)))
        .collect()
}

fn groups(
    spec: &[Group],
    bytes: &[u8],
    cursor: &mut usize,
    object: &mut Map<String, Value>,
    entries_left: &mut usize,
) -> Result<(), String> {
    for group in spec {
        let offset = group.offset.unwrap_or(*cursor);
        if offset < *cursor {
            return Err(format!("Group {} overlaps preceding data", group.name));
        }
        let start = offset
            .checked_add(group.alignment - 1)
            .ok_or("Group offset overflow")?
            & !(group.alignment - 1);
        let end = start
            .checked_add(group.header_size)
            .ok_or("Group header overflow")?;
        let header = bytes
            .get(start..end)
            .ok_or("Truncated binary group header")?;
        let count =
            usize::try_from(group.count.number(header)?).map_err(|_| "Invalid group count")?;
        let block = usize::try_from(group.block_length.number(header)?)
            .map_err(|_| "Invalid group block length")?;
        if count > group.max_count || count > *entries_left {
            return Err("Binary group count exceeds limit".into());
        }
        if block < extent(&group.fields) || (count != 0 && block == 0 && group.groups.is_empty()) {
            return Err("Binary group block length is too small".into());
        }
        let minimum = count.checked_mul(block).ok_or("Group size overflow")?;
        if minimum > bytes.len() - end {
            return Err("Truncated binary group entries".into());
        }
        *entries_left -= count;
        *cursor = end;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let entry = bytes.get(*cursor..).ok_or("Truncated binary group entry")?;
            let fixed = entry.get(..block).ok_or("Truncated binary group entry")?;
            let mut value = fields(&group.fields, fixed)?;
            let mut consumed = block;
            groups(
                &group.groups,
                entry,
                &mut consumed,
                &mut value,
                entries_left,
            )?;
            *cursor = cursor.checked_add(consumed).ok_or("Group size overflow")?;
            entries.push(Value::Object(value));
        }
        object.insert(group.name.clone(), Value::Array(entries));
    }
    Ok(())
}
