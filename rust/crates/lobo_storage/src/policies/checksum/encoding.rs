use super::{Error, Input, Precision};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Field {
    Id,
    Price,
    Quantity,
    SignedQuantity,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumberFormat {
    DecimalDigits,
    Decimal,
    EcmaScript,
}

pub(super) type Writer = fn(&Input, Precision, &mut Vec<u8>) -> Result<(), Error>;

pub(super) fn writer(field: Field, format: NumberFormat) -> Writer {
    fn select<const FIELD: u8>(format: NumberFormat) -> Writer {
        match format {
            NumberFormat::DecimalDigits => write::<FIELD, 0>,
            NumberFormat::Decimal => write::<FIELD, 1>,
            NumberFormat::EcmaScript => write::<FIELD, 2>,
        }
    }
    match field {
        Field::Id => select::<0>(format),
        Field::Price => select::<1>(format),
        Field::Quantity => select::<2>(format),
        Field::SignedQuantity => select::<3>(format),
    }
}

fn write<const FIELD: u8, const FORMAT: u8>(
    input: &Input,
    precision: Precision,
    output: &mut Vec<u8>,
) -> Result<(), Error> {
    if FIELD == 0 {
        output.extend_from_slice(itoa::Buffer::new().format(input.id).as_bytes());
        return Ok(());
    }
    let (atoms, places, width) = if FIELD == 1 {
        (input.price, precision.price, input.widths.price)
    } else {
        (
            u128::from(input.quantity),
            precision.quantity,
            input.widths.quantity,
        )
    };
    if FIELD == 3 && input.side == lobo_models::Side::Sell {
        output.push(b'-');
    }
    if FORMAT == 0 {
        decimal_digits(atoms, places, width, output)
    } else {
        decimal(atoms, places, FORMAT == 2, output);
        Ok(())
    }
}

/// Preserve wire decimal widths without converting fixed-point values to floats.
pub fn decimal_digits(
    atoms: u128,
    places: u8,
    width: u32,
    output: &mut Vec<u8>,
) -> Result<(), Error> {
    let coefficient = if width > u32::from(places) {
        let scale = 10u128
            .checked_pow(width - u32::from(places))
            .ok_or("Checksum scale overflow")?;
        atoms
            .checked_mul(scale)
            .ok_or("Checksum coefficient overflow")?
    } else {
        let scale = 10u128
            .checked_pow(u32::from(places) - width)
            .ok_or("Checksum scale overflow")?;
        if atoms % scale != 0 {
            return Err("Checksum width loses declared precision");
        }
        atoms / scale
    };
    if coefficient != 0 {
        output.extend_from_slice(itoa::Buffer::new().format(coefficient).as_bytes());
    }
    Ok(())
}

fn decimal(atoms: u128, places: u8, scientific: bool, output: &mut Vec<u8>) {
    if atoms == 0 {
        output.push(b'0');
        return;
    }
    let mut buffer = itoa::Buffer::new();
    let digits = buffer.format(atoms);
    let places = usize::from(places);
    if scientific && places >= digits.len() + 6 {
        output.push(digits.as_bytes()[0]);
        let tail = digits[1..].trim_end_matches('0');
        if !tail.is_empty() {
            output.push(b'.');
            output.extend_from_slice(tail.as_bytes());
        }
        output.extend_from_slice(b"e-");
        output.extend_from_slice(
            itoa::Buffer::new()
                .format(places - digits.len() + 1)
                .as_bytes(),
        );
    } else if places == 0 {
        output.extend_from_slice(digits.as_bytes());
    } else if digits.len() > places {
        let split = digits.len() - places;
        output.extend_from_slice(digits[..split].as_bytes());
        let tail = digits[split..].trim_end_matches('0');
        if !tail.is_empty() {
            output.push(b'.');
            output.extend_from_slice(tail.as_bytes());
        }
    } else {
        output.extend_from_slice(b"0.");
        output.resize(output.len() + places - digits.len(), b'0');
        output.extend_from_slice(digits.trim_end_matches('0').as_bytes());
    }
}
