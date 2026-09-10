//! Canvas geometry shared by the GPU shader, labels, and native queue hit-testing.
pub const BINS: u32 = 64;
pub const BOOK_BYTES: u64 = 1056;
pub const THEME_BYTES: u64 = 13 * 16;
pub const COLUMNS: u32 = 256;
// Packed bid/ask bins, quantity totals, and the captured price minimum/span.
pub const HISTORY_BYTES: u64 = (BINS as u64 * 4 + 8 + 8) * COLUMNS as u64;
pub const HEATMAP_WIDTH: f32 = 0.74;
pub const DEPTH_FULL: f32 = 0.08;
pub const DEPTH_ZERO: f32 = 0.92;
pub const PRICE_TOP: f32 = 0.025;
pub const PRICE_BOTTOM: f32 = 0.875;
pub const VOLUME_TOP: f32 = 0.925;
pub const VOLUME_BOTTOM: f32 = 0.985;

pub fn layout_json() -> String {
    serde_json::json!({
        "bins": BINS, "heatmapWidth": HEATMAP_WIDTH,
        "depthFull": DEPTH_FULL, "depthZero": DEPTH_ZERO,
        "priceTop": PRICE_TOP, "priceBottom": PRICE_BOTTOM,
        "volumeTop": VOLUME_TOP, "volumeBottom": VOLUME_BOTTOM,
    })
    .to_string()
}

/// Bottom-to-top position in the same coordinate system as the GPU bins.
pub fn price_fraction(y: f32) -> f32 {
    1.0 - (y - PRICE_TOP) / (PRICE_BOTTOM - PRICE_TOP)
}

pub fn bin_at(x: f32, y: f32) -> Option<u32> {
    if !x.is_finite()
        || !y.is_finite()
        || !(0.0..=1.0).contains(&x)
        || !(PRICE_TOP..PRICE_BOTTOM).contains(&y)
    {
        return None;
    }
    Some(((price_fraction(y) * BINS as f32) as u32).min(BINS - 1))
}

/// Empty sides keep the remaining side selectable throughout its depth profile.
pub fn midpoint(bid: f64, ask: f64) -> f64 {
    match (bid > 0.0, ask > 0.0) {
        (true, true) => (bid + ask) / 2.0,
        (true, false) => f64::INFINITY,
        (false, true) => f64::NEG_INFINITY,
        (false, false) => 0.0,
    }
}

pub fn display_shader(srgb: bool) -> String {
    format!(
        "const HEATMAP_WIDTH: f32 = {HEATMAP_WIDTH:?};\n\
         const DEPTH_FULL: f32 = {DEPTH_FULL:?};\n\
         const DEPTH_ZERO: f32 = {DEPTH_ZERO:?};\n\
         const PRICE_TOP: f32 = {PRICE_TOP:?};\n\
         const PRICE_BOTTOM: f32 = {PRICE_BOTTOM:?};\n\
         const VOLUME_TOP: f32 = {VOLUME_TOP:?};\n\
         const VOLUME_BOTTOM: f32 = {VOLUME_BOTTOM:?};\n{}",
        themed_shader(include_str!("display.wgsl"), srgb)
    )
}

pub fn themed_shader(source: &str, srgb: bool) -> String {
    format!(
        "const SURFACE_SRGB: bool = {srgb};\n{}\n{source}",
        include_str!("palette.wgsl")
    )
}
