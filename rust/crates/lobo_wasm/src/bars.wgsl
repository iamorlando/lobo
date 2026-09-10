struct Params { a: vec4<u32>, b: vec4<u32>, range: vec4<f32>, volume: vec4<f32> }
struct Bar { ohlc: vec4<f32>, volume: f32, index: u32, pad: vec2<u32> }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> bars: array<Bar>;
struct Vertex { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vertex(@builtin(vertex_index) i: u32) -> Vertex {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: Vertex; out.position = vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0); out.uv = vec2<f32>(uv.x, 1.0 - uv.y); return out;
}
@fragment fn fragment(in: Vertex) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let background = canvas_background(uv);
    let grid = theme.grid.rgb;
    var color = background;
    if abs(fract(uv.y * 10.0) - 0.5) > 0.493 || abs(fract(uv.x * 10.0) - 0.5) > 0.497 { color = grid; }
    if p.a.w == 0u || p.range.y <= 0.0 { return output_color(color); }
    if uv.x >= 0.92 { return output_color(background); }
    let cells = f32(max(p.a.w, 20u));
    let column = u32(uv.x / 0.92 * cells);
    let local = fract(uv.x / 0.92 * cells);
    if column < p.a.w {
        let index = p.b.x + column;
        let bar = bars[p.a.z * 256u + index % 256u];
        if bar.index == index {
            let up = bar.ohlc.w >= bar.ohlc.x;
            let tint = select(theme.ask.rgb, theme.bid.rgb, up);
            let alpha = select(1.0, 0.5, p.b.z != 0u && index + 1u == p.b.y);
            let price = p.range.x + (1.0 - uv.y / 0.76) * p.range.y;
            let pixel = p.range.y / (f32(p.a.y) * 0.76);
            let body_low = min(bar.ohlc.x, bar.ohlc.w);
            let body_high = max(bar.ohlc.x, bar.ohlc.w);
            let wick = abs(local - 0.5) < max(0.015, cells / f32(p.a.x) * 0.65) && price >= bar.ohlc.z && price <= bar.ohlc.y;
            let body = local > 0.2 && local < 0.8 && price >= body_low - pixel && price <= body_high + pixel;
            if uv.y < 0.76 && (wick || body) {
                let height = clamp((price - body_low) / max(body_high - body_low, pixel), 0.0, 1.0);
                let fill = mix(tint, side_high(up), height * 0.7);
                color = mix(color, select(tint, fill, body), alpha);
            }
            if uv.y > 0.82 && local > 0.2 && local < 0.8 && (1.0 - uv.y) / 0.16 < bar.volume / p.volume.x { color = mix(color, area_color(up, (1.0-uv.y)/max(0.16*bar.volume/p.volume.x,0.001)), alpha); }
        }
    }
    // Best-price input messages drive these quote guides independently of fills.
    if uv.y < 0.76 && fract(uv.x * 80.0) < 0.55 {
        let price = p.range.x + (1.0 - uv.y / 0.76) * p.range.y;
        let tolerance = p.range.y / f32(p.a.y);
        if p.range.z > 0.0 && abs(price - p.range.z) < tolerance { color = mix(background, theme.bid.rgb, 0.65); }
        if p.range.w > 0.0 && abs(price - p.range.w) < tolerance { color = mix(background, theme.ask.rgb, 0.65); }
    }
    return output_color(color);
}
