// draw2d.wgsl: the Draw2D triangle pipeline. Vertices are in target pixels (origin top-left,
// y down); colours are sRGB-encoded and converted to linear when the target is an sRGB surface.
//
// group(0): the target size and the srgb flag.
// group(1): one texture + sampler per draw (texture 0 is a 1x1 white texture).

struct View {
    size : vec2<f32>,
    srgb : u32,
    _pad : u32,
}
@group(0) @binding(0) var<uniform> view : View;
@group(1) @binding(0) var tex : texture_2d<f32>;
@group(1) @binding(1) var samp : sampler;

struct Vertex {
    @location(0) pos   : vec2<f32>,
    @location(1) uv    : vec2<f32>,
    @location(2) color : vec4<f32>,
}

struct VsOut {
    @builtin(position) position : vec4<f32>,
    @location(0) uv    : vec2<f32>,
    @location(1) color : vec4<f32>,
}

fn srgb_to_linear(c : vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@vertex
fn vs_main(v : Vertex) -> VsOut {
    var out : VsOut;
    let ndc = vec2<f32>(v.pos.x / view.size.x * 2.0 - 1.0, 1.0 - v.pos.y / view.size.y * 2.0);
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = v.uv;
    var rgb = v.color.rgb;
    if (view.srgb != 0u) { rgb = srgb_to_linear(rgb); }
    out.color = vec4<f32>(rgb, v.color.a);
    return out;
}

@fragment
fn fs_main(in : VsOut) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, in.uv) * in.color;
}
