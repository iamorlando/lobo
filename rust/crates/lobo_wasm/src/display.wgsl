struct Params { size: vec4<u32>, timing: vec4<u32>, counts: vec4<u32>, flags: vec4<u32> }
struct Book { bins: array<vec2<f32>,64>, depth: array<vec2<f32>,64>, scale: vec2<f32>, total: vec2<f32>, first: u32, epoch: u32, previous: u32, pad: u32 }
struct History { pixels: array<u32,16384>, totals: array<vec2<f32>,256>, ranges: array<vec2<f32>,256> }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> books: array<Book>;
@group(0) @binding(2) var<storage, read> history: array<History>;
@vertex fn vertex(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let positions=array(vec2<f32>(-1.0,-1.0),vec2<f32>(3.0,-1.0),vec2<f32>(-1.0,3.0));
    return vec4<f32>(positions[i],0.0,1.0);
}
fn grid(uv: vec2<f32>, counts: vec2<f32>, pixels: vec2<f32>) -> bool {
    let distance = abs(fract(uv * counts + 0.5) - 0.5) / counts * pixels;
    return distance.x < 0.5 || distance.y < 0.5;
}
@fragment fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv=position.xy/vec2<f32>(p.size.xy);
    var color=canvas_background(uv);
    let book=p.size.w;
    if book >= p.counts.x { return output_color(color); }
    let local_book=book-p.counts.z;
    let valid=select(min(books[book].previous-books[book].first+1u,256u),0u,books[book].epoch==0u);
    let price_height = PRICE_BOTTOM - PRICE_TOP;
    if uv.y >= PRICE_TOP && uv.y < PRICE_BOTTOM {
        let price_position = 1.0 - (uv.y - PRICE_TOP) / price_height;
        let y = min(u32(price_position * 64.0), 63u);
        if uv.x < HEATMAP_WIDTH {
            let local = vec2<f32>(uv.x / HEATMAP_WIDTH, 1.0 - price_position);
            if grid(local, vec2<f32>(8.0, 8.0), vec2<f32>(p.size.xy) * vec2<f32>(HEATMAP_WIDTH, price_height)) { color = mix(color, theme.grid.rgb, 0.65); }
            let x = min(u32(local.x * 256.0),255u);
            if x >= 256u-valid {
                let column = (books[book].previous+1u+x)%256u;
                // Project the current price into the column's original range.
                // Zoom/pan never erase, resample, or read back raster history.
                let range = history[local_book].ranges[column];
                let price = bitcast<f32>(p.flags.z) + price_position * bitcast<f32>(p.flags.w);
                let row = (price - range.x) / range.y;
                if row >= 0.0 && row < 1.0 {
                    let packed=history[local_book].pixels[column*64u+min(u32(row*64.0),63u)];
                    let values=exp2(vec2<f32>(f32(packed&65535u),f32(packed>>16u))/1024.0)-1.0;
                    let intensity=clamp(log(1.0+max(values.x,values.y))/log(1.0+max(books[book].scale.y,2.0)),0.0,1.0);
                    let tint=liquidity_color(values.x>=values.y, intensity);
                    color=mix(color,tint,smoothstep(0.0,0.2,intensity));
                }
            }
        } else {
            let local = vec2<f32>((uv.x - HEATMAP_WIDTH) / (1.0 - HEATMAP_WIDTH), 1.0 - price_position);
            let pixels = vec2<f32>(p.size.xy) * vec2<f32>(1.0 - HEATMAP_WIDTH, price_height);
            color = mix(canvas_background(uv), theme.panel.rgb, 0.45);
            // The same price bin is clickable across the entire rail. The side
            // follows price relative to the spread, rather than two x regions.
            let ask_side = price_position >= bitcast<f32>(p.flags.y);
            let values = books[book].depth[y];
            let quantity = select(values.x, values.y, ask_side);
            let peak = max(max(books[book].total.x, books[book].total.y), 1.0);
            // Zero is the right-hand baseline. More depth extends the filled
            // region leftward; filling beyond an increasing boundary inverted it.
            let boundary = DEPTH_ZERO - (DEPTH_ZERO - DEPTH_FULL) * quantity / peak;
            let above = books[book].depth[min(y + 1u, 63u)];
            let above_boundary = DEPTH_ZERO - (DEPTH_ZERO - DEPTH_FULL) * select(above.x, above.y, ask_side) / peak;
            let tint = select(theme.bid.rgb, theme.ask.rgb, ask_side);
            if quantity > 0.0 {
                if local.x >= boundary && local.x <= DEPTH_ZERO {
                    let distance = (DEPTH_ZERO - local.x) / max(DEPTH_ZERO - boundary, 0.001);
                    color = area_color(!ask_side, distance);
                }
                let vertical = abs(local.x - boundary) * pixels.x < 1.4;
                let horizontal = (1.0 - fract(price_position * 64.0)) * pixels.y / 64.0 < 1.4
                    && local.x >= min(boundary, above_boundary) && local.x <= max(boundary, above_boundary);
                if vertical || horizontal { color = tint; }
            }
            if grid(local, vec2<f32>(4.0,64.0), pixels) { color = mix(color, theme.grid.rgb, 0.3); }
            if local.x * pixels.x < 1.0 { color = theme.grid.rgb; }
        }
    } else if uv.y >= VOLUME_TOP && uv.y < VOLUME_BOTTOM {
        // Quantity history spans the whole workstation width under both charts.
        let local = vec2<f32>(uv.x,(uv.y-VOLUME_TOP)/(VOLUME_BOTTOM-VOLUME_TOP));
        let x = min(u32(local.x*256.0),255u);
        if grid(local, vec2<f32>(8.0,2.0), vec2<f32>(p.size.xy) * vec2<f32>(1.0,VOLUME_BOTTOM-VOLUME_TOP)) { color = theme.grid.rgb; }
        if x >= 256u-valid {
            let values=history[local_book].totals[(books[book].previous+1u+x)%256u]/max(books[book].scale.x,1.0);
            if local.y<0.5 && 0.5-local.y<values.x*0.46 { color=area_color(true,(0.5-local.y)/max(values.x*0.46,0.001)); }
            if local.y>=0.5 && local.y-0.5<values.y*0.46 { color=area_color(false,(local.y-0.5)/max(values.y*0.46,0.001)); }
        }
    }
    return output_color(color);
}
