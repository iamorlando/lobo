@group(0) @binding(0) var<uniform> p: vec4<f32>;
struct Vertex {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) mask: vec2<u32>,
    @location(2) @interpolate(flat) background: u32,
    @location(3) phase: f32,
}
@vertex fn vertex(@builtin(vertex_index) index: u32, @builtin(instance_index) instance: u32,
    @location(0) cell: vec4<f32>, @location(1) flow: vec2<f32>, @location(2) mask: vec2<u32>) -> Vertex {
    let corners = array<vec2<f32>,6>(vec2(0,0),vec2(1,0),vec2(0,1),vec2(0,1),vec2(1,0),vec2(1,1));
    let uv = corners[index];
    var out: Vertex;
    out.uv = uv; out.mask = mask; out.background = select(0u,1u,instance==0u); out.phase = flow.y;
    if instance == 0u { out.position = vec4(uv * vec2(2,-2) + vec2(-1,1),0,1); return out; }
    let period = p.x + flow.x + 40.0;
    let moving = cell.x - p.z * 10.0 * flow.y;
    let origin = moving - floor((moving + flow.x + 20.0) / period) * period;
    let pixel = vec2(origin + cell.z * cell.w * 0.65, cell.y) + uv * vec2(cell.w * 0.55, cell.w);
    out.position = vec4(pixel / p.xy * vec2(2,-2) + vec2(-1,1),0,1);
    return out;
}
@fragment fn fragment(in: Vertex) -> @location(0) vec4<f32> {
    let screen = in.position.xy / (p.xy * p.w);
    let base = canvas_background(screen);
    if in.background == 1u {
        let grid = min(fract(screen.x * p.x / 80.0), fract(screen.y * p.y / 80.0));
        return output_color(mix(base, theme.grid.rgb, select(0.0,0.3,grid<0.012)));
    }
    let cell = min(vec2<u32>(in.uv * vec2(5,7)),vec2<u32>(4,6));
    let bit = cell.y * 5u + cell.x;
    let mask = select(in.mask.x, in.mask.y, bit >= 32u);
    if ((mask >> (bit % 32u)) & 1u) == 0u { discard; }
    let pixel = fract(in.uv * vec2(5,7));
    let edge = min(min(pixel.x,1.0-pixel.x),min(pixel.y,1.0-pixel.y));
    let shape = smoothstep(0.0,0.11,edge);
    let pulse = 0.62 + 0.2 * sin(p.z * 0.8 + in.phase * 8.0 + screen.x * 4.0);
    let color = mix(theme.loading_accent.rgb, theme.primary.rgb, in.uv.y);
    return output_color(mix(base,color,pulse*shape));
}
