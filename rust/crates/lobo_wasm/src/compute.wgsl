// Shared state for every active book; one GPU submission advances the whole market.
struct Params { size: vec4<u32>, timing: vec4<u32>, counts: vec4<u32>, flags: vec4<u32> }
struct Slot { price: atomic<u32>, book: atomic<u32>, bid: atomic<u32>, ask: atomic<u32> }
struct Bin { bid: atomic<u32>, ask: atomic<u32> }
struct Book { bins: array<Bin,64>, depth: array<vec2<f32>,64>, scale: vec2<f32>, total: vec2<f32>, first: u32, epoch: u32, previous: u32, pad: u32 }
struct History { pixels: array<u32,16384>, totals: array<vec2<f32>,256>, ranges: array<vec2<f32>,256> }
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read_write> slots: array<Slot>;
@group(0) @binding(2) var<storage, read_write> books: array<Book>;
// x = minimum price, y = span, z = epoch, w = frozen column + 1 (u32 bits).
@group(0) @binding(3) var<storage, read> cameras: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> history: array<History>;
@group(1) @binding(0) var<storage, read> prices: array<u32>;
@group(1) @binding(1) var<storage, read> quantities: array<u32>;
@group(1) @binding(2) var<storage, read> sides: array<u32>;
@group(1) @binding(3) var<storage, read> routes: array<vec4<u32>>;
fn swap32(n: u32) -> u32 { return ((n & 255u)<<24u) | ((n & 65280u)<<8u) | ((n>>8u)&65280u) | (n>>24u); }
fn add_bid(book: u32, bin: u32, value: f32) {
    var old = atomicLoad(&books[book].bins[bin].bid);
    loop {
        let result = atomicCompareExchangeWeak(&books[book].bins[bin].bid, old, bitcast<u32>(bitcast<f32>(old)+value));
        if result.exchanged { break; }
        old = result.old_value;
    }
}
fn add_ask(book: u32, bin: u32, value: f32) {
    var old = atomicLoad(&books[book].bins[bin].ask);
    loop {
        let result = atomicCompareExchangeWeak(&books[book].bins[bin].ask, old, bitcast<u32>(bitcast<f32>(old)+value));
        if result.exchanged { break; }
        old = result.old_value;
    }
}
@compute @workgroup_size(256)
fn apply(@builtin(global_invocation_id) id: vec3<u32>) {
    let row = id.x;
    if row >= p.size.z { return; }
    let slot = routes[row].x;
    atomicStore(&slots[slot].price, routes[row].z);
    atomicStore(&slots[slot].book, routes[row].y+1u);
    // routes.w contains only the hidden quantity selected by the observer batcher.
    let quantity = f32(quantities[row*2u]) + f32(quantities[row*2u+1u])*4294967296.0 + bitcast<f32>(routes[row].w);
    let side = (sides[row/4u] >> ((row%4u)*8u)) & 255u;
    if side == 0u { atomicStore(&slots[slot].bid,bitcast<u32>(quantity)); }
    else { atomicStore(&slots[slot].ask,bitcast<u32>(quantity)); }
}
@compute @workgroup_size(256)
fn clear_bins(@builtin(global_invocation_id) id: vec3<u32>) {
    let book = id.x/64u; let bin = id.x%64u;
    if book >= p.counts.x { return; }
    atomicStore(&books[book].bins[bin].bid,0u);
    atomicStore(&books[book].bins[bin].ask,0u);
}
@compute @workgroup_size(256)
fn aggregate(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= p.counts.y { return; }
    let key = atomicLoad(&slots[id.x].book);
    if key == 0u { return; }
    let book = key-1u;
    let y = (bitcast<f32>(atomicLoad(&slots[id.x].price))-cameras[book].x)/cameras[book].y;
    if y < 0.0 || y >= 1.0 { return; }
    let bin = min(u32(y*64.0),63u);
    let bid = bitcast<f32>(atomicLoad(&slots[id.x].bid));
    let ask = bitcast<f32>(atomicLoad(&slots[id.x].ask));
    if bid > 0.0 { add_bid(book,bin,bid); }
    if ask > 0.0 { add_ask(book,bin,ask); }
}
// Logarithmic 16-bit quantities keep every book's raster history compact. The
// current depth/volume use f32; exact native book quantities never go through this.
fn pack(values: vec2<f32>) -> u32 {
    let q = vec2<u32>(clamp(round(log2(values+1.0)*1024.0),vec2<f32>(0.0),vec2<f32>(65535.0)));
    return q.x | (q.y<<16u);
}
@compute @workgroup_size(64)
fn capture(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) bin: u32) {
    let local = group.x;
    let book = p.counts.z+local;
    if book >= p.counts.x { return; }
    let frozen = bitcast<u32>(cameras[book].w);
    let absolute = select(p.timing.x, min(p.timing.x, frozen-1u), frozen != 0u);
    let epoch = bitcast<u32>(cameras[book].z);
    let reset = books[book].epoch != epoch;
    let previous_epoch = books[book].epoch;
    let delta = select(min(absolute-books[book].previous,256u),1u,reset);
    let values = vec2<f32>(bitcast<f32>(atomicLoad(&books[book].bins[bin].bid)),bitcast<f32>(atomicLoad(&books[book].bins[bin].ask)));
    // Every lane must reach this barrier, including frozen books: storage-dependent
    // early returns are non-uniform in WebGPU. Read the old epoch/clock first.
    storageBarrier();
    if p.flags.x == 1u {
        if reset && previous_epoch != 0u {
            for(var x=0u;x<256u;x++) { history[local].pixels[x*64u+bin]=0u; }
        }
        for(var k=0u;k<max(delta,1u);k++) {
            let x = (absolute+256u-k)%256u;
            // Undrawn intervals are gaps, not invented intermediate snapshots.
            history[local].pixels[x*64u+bin]=select(pack(values),0u,p.timing.y != 0u && k > 0u);
        }
    }
    if bin == 0u {
        var total = vec2<f32>(0.0); var peak = 1.0;
        var cumulative_bid = 0.0;
        for(var i=0u;i<64u;i++) {
            let v = vec2<f32>(bitcast<f32>(atomicLoad(&books[book].bins[i].bid)),bitcast<f32>(atomicLoad(&books[book].bins[i].ask)));
            total+=v; peak=max(peak,max(v.x,v.y));
            // Asks accumulate from the lowest price, bids from the highest.
            // Reuse this existing per-book pass; fragments perform no scans.
            cumulative_bid += bitcast<f32>(atomicLoad(&books[book].bins[63u-i].bid));
            books[book].depth[i].y = total.y;
            books[book].depth[63u-i].x = cumulative_bid;
        }
        books[book].total = total;
        let old_scale = select(books[book].scale,vec2<f32>(1.0),reset);
        books[book].scale = max(vec2<f32>(max(total.x,total.y),peak),old_scale*select(0.998,1.0,frozen != 0u));
        if p.flags.x == 1u {
            if reset {
                books[book].first=absolute; books[book].epoch=epoch;
                if previous_epoch != 0u {
                    for(var x=0u;x<256u;x++) { history[local].totals[x]=vec2<f32>(0.0); }
                }
            }
            for(var k=0u;k<max(delta,1u);k++) {
                let x = (absolute+256u-k)%256u;
                history[local].totals[x]=select(total,vec2<f32>(0.0),p.timing.y != 0u && k > 0u);
                history[local].ranges[x]=cameras[book].xy;
            }
            books[book].previous=absolute;
        }
    }
}
