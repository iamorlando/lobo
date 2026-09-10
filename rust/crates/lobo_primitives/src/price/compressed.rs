use super::PriceType;
use std::{
    fmt,
    num::ParseIntError,
    ops::{Add, Div, Sub},
    str::FromStr,
};

/// An ITCH-style unsigned price stored as four bytes.
///
/// The value is the raw fixed-point integer; its decimal scale belongs to
/// the instrument or feed definition, just as it does in Nasdaq ITCH.
#[repr(transparent)]
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(
    feature = "python",
    derive(pyo3::FromPyObject, pyo3::IntoPyObject, pyo3::IntoPyObjectRef)
)]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct CompressedPrice(u32);

impl CompressedPrice {
    pub const MIN: Self = Self(u32::MIN);
    pub const MAX: Self = Self(u32::MAX);

    #[inline(always)]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    #[inline(always)]
    pub const fn raw(self) -> u32 {
        self.0
    }

    #[inline(always)]
    pub const fn checked_add(self, rhs: Self) -> Option<Self> {
        match self.0.checked_add(rhs.0) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    #[inline(always)]
    pub const fn checked_sub(self, rhs: Self) -> Option<Self> {
        match self.0.checked_sub(rhs.0) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }

    #[inline(always)]
    pub const fn abs_diff(self, rhs: Self) -> Self {
        Self(self.0.abs_diff(rhs.0))
    }
}

impl PriceType for CompressedPrice {
    const MIN: Self = Self::MIN;
    const MAX: Self = Self::MAX;

    #[inline(always)]
    fn checked_add(self, rhs: Self) -> Option<Self> {
        self.checked_add(rhs)
    }

    #[inline(always)]
    fn checked_sub(self, rhs: Self) -> Option<Self> {
        self.checked_sub(rhs)
    }

    #[inline(always)]
    fn abs_diff(self, rhs: Self) -> Self {
        self.abs_diff(rhs)
    }

    #[inline(always)]
    fn into_u128(self) -> u128 {
        self.0 as u128
    }
}

impl fmt::Debug for CompressedPrice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl fmt::Display for CompressedPrice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl PartialEq<i32> for CompressedPrice {
    fn eq(&self, other: &i32) -> bool {
        i32::try_from(self.0) == Ok(*other)
    }
}

impl PartialEq<u32> for CompressedPrice {
    fn eq(&self, other: &u32) -> bool {
        self.0 == *other
    }
}

impl PartialEq<u64> for CompressedPrice {
    fn eq(&self, other: &u64) -> bool {
        u64::from(self.0) == *other
    }
}

impl FromStr for CompressedPrice {
    type Err = ParseIntError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}

impl From<u8> for CompressedPrice {
    fn from(value: u8) -> Self {
        Self(u32::from(value))
    }
}

impl From<u16> for CompressedPrice {
    fn from(value: u16) -> Self {
        Self(u32::from(value))
    }
}

impl From<u32> for CompressedPrice {
    fn from(value: u32) -> Self {
        Self(value)
    }
}

macro_rules! impl_try_from {
    ($($source:ty),+ $(,)?) => {$(
        impl TryFrom<$source> for CompressedPrice {
            type Error = std::num::TryFromIntError;

            fn try_from(value: $source) -> Result<Self, Self::Error> {
                u32::try_from(value).map(Self)
            }
        }
    )+};
}

impl_try_from!(u64, i32, i64, u128, usize);

impl From<CompressedPrice> for u32 {
    fn from(value: CompressedPrice) -> Self {
        value.0
    }
}

impl From<CompressedPrice> for u64 {
    fn from(value: CompressedPrice) -> Self {
        u64::from(value.0)
    }
}

impl From<CompressedPrice> for u128 {
    fn from(value: CompressedPrice) -> Self {
        u128::from(value.0)
    }
}

impl Add for CompressedPrice {
    type Output = Self;

    #[inline(always)]
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl Sub for CompressedPrice {
    type Output = Self;

    #[inline(always)]
    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl Div for CompressedPrice {
    type Output = Self;

    #[inline(always)]
    fn div(self, rhs: Self) -> Self::Output {
        Self(self.0 / rhs.0)
    }
}
