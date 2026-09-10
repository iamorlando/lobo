use super::PriceType;
use std::{
    fmt,
    num::ParseIntError,
    ops::{Add, Div, Sub},
    str::FromStr,
};

#[repr(transparent)]
#[derive(Clone, Copy, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[cfg_attr(feature = "serde", serde(transparent))]
pub struct Price128(u128);

impl Price128 {
    pub const MIN: Self = Self(u128::MIN);
    pub const MAX: Self = Self(u128::MAX);

    #[inline(always)]
    pub const fn from_raw(raw: u128) -> Self {
        Self(raw)
    }
    #[inline(always)]
    pub const fn raw(self) -> u128 {
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

impl PriceType for Price128 {
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
        self.0
    }
}

impl fmt::Debug for Price128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl fmt::Display for Price128 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl FromStr for Price128 {
    type Err = ParseIntError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        value.parse().map(Self)
    }
}
impl From<u8> for Price128 {
    fn from(value: u8) -> Self {
        Self(value as u128)
    }
}
impl From<u16> for Price128 {
    fn from(value: u16) -> Self {
        Self(value as u128)
    }
}
impl From<u32> for Price128 {
    fn from(value: u32) -> Self {
        Self(value as u128)
    }
}
impl From<u64> for Price128 {
    fn from(value: u64) -> Self {
        Self(value as u128)
    }
}
impl From<u128> for Price128 {
    fn from(value: u128) -> Self {
        Self(value)
    }
}
impl From<usize> for Price128 {
    fn from(value: usize) -> Self {
        Self(value as u128)
    }
}
impl TryFrom<i32> for Price128 {
    type Error = std::num::TryFromIntError;
    fn try_from(value: i32) -> Result<Self, Self::Error> {
        u128::try_from(value).map(Self)
    }
}
impl TryFrom<i64> for Price128 {
    type Error = std::num::TryFromIntError;
    fn try_from(value: i64) -> Result<Self, Self::Error> {
        u128::try_from(value).map(Self)
    }
}
impl From<Price128> for u128 {
    fn from(value: Price128) -> Self {
        value.0
    }
}
impl Add for Price128 {
    type Output = Self;
    #[inline(always)]
    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}
impl Sub for Price128 {
    type Output = Self;
    #[inline(always)]
    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}
impl Div for Price128 {
    type Output = Self;
    #[inline(always)]
    fn div(self, rhs: Self) -> Self {
        Self(self.0 / rhs.0)
    }
}
