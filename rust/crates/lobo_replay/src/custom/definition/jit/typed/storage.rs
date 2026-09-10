//! Storage for generated layouts. Object member offsets and variable offsets
//! are assigned by the compiler. This module has no field names or lookups.
use std::{
    mem::{align_of, size_of},
    ptr,
};

pub(super) const NULL: u64 = 0;
pub(super) const BOOL: u64 = 1;
pub(super) const NUMBER: u64 = 2;
pub(super) const TEXT: u64 = 3;
pub(super) const ARRAY: u64 = 4;
pub(super) const OBJECT: u64 = 5;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Span {
    pub address: usize,
    pub length: usize,
}
impl Span {
    pub unsafe fn bytes<'a>(self) -> &'a [u8] {
        if self.length == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(self.address as *const u8, self.length) }
    }
    pub unsafe fn text<'a>(self) -> &'a str {
        // String tokens were decoded and checked by the scanner.
        unsafe { std::str::from_utf8_unchecked(self.bytes()) }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Number {
    pub low: u64,
    pub high: u64,
    pub scale: i64,
    pub negative: u64,
}
impl Number {
    pub fn parse(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.is_empty() || bytes.len() > 128 {
            return Err("Invalid decimal length");
        }
        let mut i = usize::from(bytes.first() == Some(&b'-'));
        let negative = i as u64;
        let mut coefficient = 0u128;
        let mut digits = |i: &mut usize| -> Result<usize, &'static str> {
            let start = *i;
            while let Some(byte @ b'0'..=b'9') = bytes.get(*i) {
                coefficient = coefficient
                    .checked_mul(10)
                    .and_then(|n| n.checked_add(u128::from(*byte - b'0')))
                    .ok_or("Decimal exceeds exact storage")?;
                *i += 1;
            }
            if *i == start {
                return Err("Expected decimal digits");
            }
            Ok(*i - start)
        };
        digits(&mut i)?;
        let fraction = if bytes.get(i) == Some(&b'.') {
            i += 1;
            digits(&mut i)? as i64
        } else {
            0
        };
        let mut exponent = 0i64;
        if matches!(bytes.get(i), Some(b'e' | b'E')) {
            i += 1;
            let sign = if bytes.get(i) == Some(&b'-') {
                i += 1;
                -1
            } else {
                if bytes.get(i) == Some(&b'+') {
                    i += 1;
                }
                1
            };
            let start = i;
            while let Some(byte @ b'0'..=b'9') = bytes.get(i) {
                exponent = exponent
                    .checked_mul(10)
                    .and_then(|n| n.checked_add(i64::from(*byte - b'0')))
                    .ok_or("Decimal exponent exceeds exact storage")?;
                i += 1;
            }
            if i == start || exponent > 38 {
                return Err("Invalid decimal exponent");
            }
            exponent *= sign;
        }
        if i != bytes.len() {
            return Err("Invalid decimal");
        }
        let scale = fraction - exponent;
        if scale > 38 {
            return Err("Unsupported decimal precision");
        }
        Ok(Self {
            low: coefficient as u64,
            high: (coefficient >> 64) as u64,
            scale,
            negative,
        })
    }
    pub fn coefficient(self) -> u128 {
        u128::from(self.low) | (u128::from(self.high) << 64)
    }
    pub fn atoms(self, places: u8) -> Result<u64, &'static str> {
        let delta = i64::from(places)
            .checked_sub(self.scale)
            .ok_or("Decimal scale overflow")?;
        let factor = POWERS
            .get(delta.unsigned_abs() as usize)
            .copied()
            .ok_or("Unsupported decimal precision")?;
        let coefficient = self.coefficient();
        let atoms = if delta >= 0 {
            coefficient
                .checked_mul(factor)
                .ok_or("Decimal value overflow")?
        } else {
            if coefficient % factor != 0 {
                return Err("Book decimal exceeds instrument precision");
            }
            coefficient / factor
        };
        u64::try_from(atoms).map_err(|_| "Decimal exceeds 64-bit storage")
    }
}
const POWERS: [u128; 39] = {
    let mut values = [1u128; 39];
    let mut i = 1;
    while i < values.len() {
        values[i] = values[i - 1] * 10;
        i += 1;
    }
    values
};

/// The scalar and collection payloads used by generated records. `kind` is
/// the incoming wire discriminant (e.g. an exchange frame can be an array or
/// a control object), not a runtime operation or policy selector.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(super) struct Record {
    pub kind: u64,
    pub valid: u64,
    pub unsigned: u64,
    pub identifier: lobo_primitives::uuid::Uuid,
    pub side: u64,
    pub present: u64,
    pub object_length: u64,
    pub raw: Span,
    pub number: Number,
    pub text: Span,
    pub fields: usize,
    pub elements: Span,
}

const PAGE_WORDS: usize = 1024;
#[derive(Default)]
pub(super) struct Arena {
    pages: Vec<Box<[u64]>>,
    page: usize,
    used: usize,
}
impl Arena {
    pub fn reset(&mut self) {
        self.page = 0;
        self.used = 0;
    }
    pub fn allocate<T>(&mut self, count: usize) -> *mut T {
        assert!(align_of::<T>() <= align_of::<u64>());
        let words = size_of::<T>()
            .checked_mul(count)
            .expect("compiled allocation size")
            .div_ceil(8);
        if self.page >= self.pages.len() {
            self.pages
                .push(vec![0; PAGE_WORDS.max(words)].into_boxed_slice());
        }
        if self.used + words > self.pages[self.page].len() {
            self.page += 1;
            self.used = 0;
            if self.page >= self.pages.len() {
                self.pages
                    .push(vec![0; PAGE_WORDS.max(words)].into_boxed_slice());
            }
            if self.pages[self.page].len() < words {
                self.pages[self.page] = vec![0; PAGE_WORDS.max(words)].into_boxed_slice();
            }
        }
        let pointer = unsafe { self.pages[self.page].as_mut_ptr().add(self.used) };
        self.used += words;
        pointer.cast()
    }
    pub fn put<T: Copy>(&mut self, value: T) -> *mut T {
        let pointer = self.allocate::<T>(1);
        unsafe {
            pointer.write(value);
        }
        pointer
    }
    pub fn bytes(&mut self, bytes: &[u8]) -> Span {
        let pointer = self.allocate::<u8>(bytes.len());
        unsafe {
            ptr::copy_nonoverlapping(bytes.as_ptr(), pointer, bytes.len());
        }
        Span {
            address: pointer as usize,
            length: bytes.len(),
        }
    }
}

/// Missing members safely point back to missing members, so a generated
/// fixed-offset field load needs no object/optional/type dispatcher.
pub(super) struct Missing {
    _arena: Arena,
    pub record: usize,
}
impl Missing {
    pub fn new(max_fields: usize) -> Self {
        let mut arena = Arena::default();
        let record = arena.put(Record::default());
        let fields = arena.allocate::<usize>(max_fields.max(1));
        unsafe {
            for i in 0..max_fields.max(1) {
                fields.add(i).write(record as usize);
            }
            (*record).fields = fields as usize;
        }
        Self {
            _arena: arena,
            record: record as usize,
        }
    }
}

pub(super) const VALID_NUMBER: u64 = 1;
pub(super) const VALID_UNSIGNED: u64 = 2;
pub(super) const VALID_ID: u64 = 4;
pub(super) const VALID_SIDE: u64 = 8;
impl Record {
    pub fn number(&mut self, number: Number) {
        self.number = number;
        self.valid |= VALID_NUMBER;
        if number.negative == 0 {
            if let Ok(unsigned) = number.atoms(0) {
                self.unsigned = unsigned;
                self.valid |= VALID_UNSIGNED;
            }
        }
    }
    pub fn text_projection<const NUMERIC: bool, const ID: bool, const SIDE: bool>(&mut self) {
        let text = unsafe { self.text.text() };
        if NUMERIC {
            if let Ok(number) = Number::parse(text.as_bytes()) {
                self.number(number);
            }
        }
        if ID {
            if let Ok(id) = lobo_primitives::uuid::Uuid::parse_str(text).or_else(|_| {
                text.parse::<u128>()
                    .map(lobo_primitives::uuid::Uuid::from_u128)
                    .map_err(|_| ())
            }) {
                self.identifier = id;
                self.valid |= VALID_ID;
            }
        }
        if SIDE {
            match text {
                "buy" | "bid" | "B" => {
                    self.side = 0;
                    self.valid |= VALID_SIDE;
                }
                "sell" | "ask" | "S" => {
                    self.side = 1;
                    self.valid |= VALID_SIDE;
                }
                _ => {}
            }
        }
    }
}
