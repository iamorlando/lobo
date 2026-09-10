mod compressed;
mod price128;
mod price64;

use std::{
    fmt::{Debug, Display},
    hash::Hash,
    ops::{Add, Div, Sub},
};

pub use compressed::CompressedPrice;
pub use price64::Price64;
pub use price128::Price128;

/// Compile-time contract for an unsigned fixed-point price representation.
///
/// Matching code is generic over this trait and monomorphized. These methods
/// are direct integer operations; there is no runtime representation tag or
/// dynamic dispatch.
pub trait PriceType:
    'static
    + Copy
    + Clone
    + Default
    + Eq
    + Ord
    + Hash
    + Debug
    + Display
    + Add<Output = Self>
    + Sub<Output = Self>
    + Div<Output = Self>
    + From<u8>
    + From<u16>
    + From<u32>
    + TryFrom<u64>
    + TryFrom<i32>
    + TryFrom<i64>
    + TryFrom<u128>
{
    const MIN: Self;
    const MAX: Self;

    fn checked_add(self, rhs: Self) -> Option<Self>;
    fn checked_sub(self, rhs: Self) -> Option<Self>;
    fn abs_diff(self, rhs: Self) -> Self;
    fn into_u128(self) -> u128;
}
