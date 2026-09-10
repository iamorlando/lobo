// Resolved web design tokens, in sRGB. Presentation only; history stores data.
struct Theme {
    canvas: vec4<f32>, panel: vec4<f32>, grid: vec4<f32>,
    bid: vec4<f32>, ask: vec4<f32>, bid_low: vec4<f32>, ask_low: vec4<f32>,
    bid_high: vec4<f32>, ask_high: vec4<f32>,
    background_top: vec4<f32>, background_bottom: vec4<f32>,
    primary: vec4<f32>, loading_accent: vec4<f32>,
}
@group(1) @binding(0) var<uniform> theme: Theme;

// A quiet warm glow behind the data, evaluated directly in the fragment shader.
fn canvas_background(uv: vec2<f32>) -> vec3<f32> {
    let top = (1.0 - smoothstep(0.0, 0.75, uv.y)) * (0.45 + 0.55 * (1.0 - uv.x));
    let bottom = smoothstep(0.25, 1.0, uv.y) * (0.3 + 0.7 * uv.x);
    return mix(mix(theme.canvas.rgb, theme.background_top.rgb, top), theme.background_bottom.rgb, bottom);
}

fn side_color(bid: bool) -> vec3<f32> { return select(theme.ask.rgb, theme.bid.rgb, bid); }
fn side_low(bid: bool) -> vec3<f32> { return select(theme.ask_low.rgb, theme.bid_low.rgb, bid); }
fn side_high(bid: bool) -> vec3<f32> { return select(theme.ask_high.rgb, theme.bid_high.rgb, bid); }
fn liquidity_color(bid: bool, intensity: f32) -> vec3<f32> {
    // Retain a wide range of subdued shades; reserve highlights for dense cells.
    let strength = pow(intensity, 3.0);
    let low = mix(side_low(bid), side_color(bid), strength);
    return mix(low, side_high(bid), smoothstep(0.85, 1.0, strength));
}
fn area_color(bid: bool, position: f32) -> vec3<f32> {
    return mix(side_low(bid), side_color(bid), 0.12 + 0.65 * clamp(position, 0.0, 1.0));
}
fn output_color(color: vec3<f32>) -> vec4<f32> {
    // sRGB surfaces encode linear fragment output. Unorm surfaces take it as-is.
    if SURFACE_SRGB {
        return vec4<f32>(select(pow((color + 0.055) / 1.055, vec3<f32>(2.4)), color / 12.92, color <= vec3<f32>(0.04045)), 1.0);
    }
    return vec4<f32>(color, 1.0);
}
