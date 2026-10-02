struct Params {
    size: vec4<f32>, scale: vec4<f32>, bg: vec4<f32>, grid: vec4<f32>,
    bid: vec4<f32>, ask: vec4<f32>, sim: vec4<f32>, quotes: vec4<f32>
}
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> records: array<vec4<f32>>;
@group(0) @binding(2) var<storage, read_write> pixels: array<u32>;
fn blend(a: vec3<f32>, b: vec3<f32>, f: f32) -> vec3<f32> { return mix(a,b,clamp(f,0.0,1.0)); }
@compute @workgroup_size(8,8)
fn chart(@builtin(global_invocation_id) id: vec3<u32>) {
    let w=u32(p.size.x); let h=u32(p.size.y);
    if id.x>=w || id.y>=h { return; }
    let x=f32(id.x)+0.5; let y=f32(id.y)+0.5; let n=u32(p.size.w);
    var color=p.bg.rgb;
    if id.y%10u==0u || id.x%24u==0u { color=p.grid.rgb; }
    if n>0u {
        if p.size.z==0.0 {
            let slot=min((id.x*2u+1u)*n/(w*2u),n-1u);
            let v=records[slot*2u]; let bar_info=records[slot*2u+1u];
            let ph=f32(h)*0.79; let price=1.0-y/ph;
            let local=(id.x*2u+1u)*n-slot*w*2u;
            let body=local*100u>w*2u*17u && local*100u<w*2u*83u;
            let distance=max(local,w)-min(local,w);
            let wick=distance*1000u<max(w*90u,n*1100u);
            var ink=p.ask.rgb; if v.w>=v.x { ink=p.bid.rgb; }
            if bar_info.y>0.0 { ink=blend(p.bg.rgb,ink,0.52); }
            let edge=0.6/ph;
            if y<ph && wick && price>=v.z-edge && price<=v.y+edge { color=ink; }
            if y<ph && body && price>=min(v.x,v.w)-edge && price<=max(v.x,v.w)+edge { color=ink; }
            if y>ph+2.0 && body && y>=f32(h)-(f32(h)-ph-3.0)*bar_info.x/max(p.scale.x,0.00001) { color=blend(p.bg.rgb,ink,0.65); }
        } else {
            let hw=f32(w)*0.76;
            let slot=select(n,min(u32(x/hw*f32(n)),n-1u),x<hw);
            let col=records[slot]; let begin=u32(col.x); let count=u32(col.y);
            let ph=f32(h)*0.84; let price=1.0-y/ph; let half=0.6/ph;
            let quantities=records[begin+min(id.y,count-1u)];
            let bid=quantities.x; let ask=quantities.y;
            let cb=quantities.z; let ca=quantities.w;
            if x<hw {
                if bid+ask>0.0 { let ink=select(p.ask.rgb,p.bid.rgb,bid>=ask); color=blend(color,ink,0.15+0.85*sqrt(max(bid,ask)/max(p.scale.x,0.00001))); }
                if abs(price-p.quotes.x)<half || abs(price-p.quotes.y)<half { color=select(p.ask.rgb,p.bid.rgb,abs(price-p.quotes.x)<half); }
            } else {
                let q=(f32(w)-1.0-x)/(f32(w)-hw)*max(p.scale.y,0.00001);
                if cb>=q && price<=p.quotes.x { color=blend(p.bg.rgb,p.bid.rgb,0.8); }
                if ca>=q && price>=p.quotes.y { color=blend(p.bg.rgb,p.ask.rgb,0.8); }
                if abs(x-hw)<0.6 { color=p.grid.rgb; }
            }
            if y>=ph {
                color=p.bg.rgb;
                let history=records[min(u32(x/f32(w)*f32(n)),n-1u)];
                let band=f32(h)*0.08;
                if y<f32(h)*0.92 {
                    if y>=f32(h)*0.92-band*history.z/p.scale.z { color=p.bid.rgb; }
                } else if y>=f32(h)-band*history.w/p.scale.z { color=p.ask.rgb; }
            }
        }
    }
    let c=vec3<u32>(round(clamp(color,vec3(0.0),vec3(1.0))*255.0));
    pixels[id.y*w+id.x]=(c.r<<16u)|(c.g<<8u)|c.b;
}
