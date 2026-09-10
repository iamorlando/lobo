//! Exact wire decimals. Never pass checksum operands through a binary float.
use serde_json::Value;
#[derive(Clone, Debug)]
pub struct Decimal {
    coefficient: u128,
    scale: u32,
}
impl Decimal {
    pub fn from_json(value: &Value) -> Result<Self, String> {
        match value {
            Value::Number(number) => Self::parse(&number.to_string()),
            Value::String(text) => Self::parse(text),
            _ => Err("Expected a decimal price or quantity".into()),
        }
    }
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.is_empty() || text.len() > 128 {
            return Err("Invalid decimal length".into());
        }
        let (mantissa, exponent) = match text.split_once(['e', 'E']) {
            Some((m, e)) => (m, e.parse::<i32>().map_err(|_| "Invalid decimal exponent")?),
            None => (text, 0),
        };
        if exponent.unsigned_abs() > 38 {
            return Err("Decimal exponent exceeds exact storage".into());
        }
        let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
        if whole.is_empty()
            || !whole
                .bytes()
                .chain(fraction.bytes())
                .all(|c| c.is_ascii_digit())
        {
            return Err("Expected a non-negative decimal".into());
        }
        let mut digits = format!("{whole}{fraction}");
        let scale = fraction.len() as i32 - exponent;
        if scale < 0 {
            digits.extend(std::iter::repeat_n('0', (-scale) as usize));
        }
        let coefficient = digits
            .parse::<u128>()
            .map_err(|_| "Decimal exceeds exact storage")?;
        if scale > 38 {
            return Err("Unsupported decimal precision".into());
        }
        Ok(Self {
            coefficient,
            scale: scale.max(0) as u32,
        })
    }
    #[cfg(feature = "kraken")]
    pub fn scale(&self) -> u32 {
        self.scale
    }
    pub fn atoms(&self, decimals: u8) -> Result<u64, String> {
        let value = if self.scale > decimals as u32 {
            let divisor = 10u128
                .checked_pow(self.scale - decimals as u32)
                .ok_or("Unsupported decimal precision")?;
            if self.coefficient % divisor != 0 {
                return Err("Book decimal exceeds instrument precision".into());
            }
            self.coefficient / divisor
        } else {
            self.coefficient
                .checked_mul(10u128.pow(decimals as u32 - self.scale))
                .ok_or("Decimal value overflow")?
        };
        u64::try_from(value)
            .map_err(|_| "Decimal exceeds 64-bit native quantity/price storage".into())
    }
}
