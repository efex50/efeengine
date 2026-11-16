// sprite.wgsl: the built-in sprite shader. One instanced unit quad per sprite.
//
// group(0): the camera (world -> clip), one per frame.
// group(1): one texture + sampler, switched between batches.
// Vertex buffer 0 (per INSTANCE, see `SpriteGpu` in sprite_renderer.rs): the sprite.
// There is no per-vertex buffer: the six corners of the quad come from `vertex_index`.

struct Camera {
    view_projection : mat4x4<f32>,
}
@group(0) @binding(0) var<uniform> camera : Camera;
@group(1) @binding(0) var sprite_texture : texture_2d<f32>;
@group(1) @binding(1) var sprite_sampler : sampler;

struct Instance {
    @location(0) world_x : vec4<f32>,   // the four columns of the quad -> world matrix
    @location(1) world_y : vec4<f32>,
    @location(2) world_z : vec4<f32>,
    @location(3) world_w : vec4<f32>,
    @location(4) size    : vec2<f32>,   // world units
    @location(5) uv_min  : vec2<f32>,   // top-left of the image region
    @location(6) uv_max  : vec2<f32>,   // bottom-right
    @location(7) color   : vec4<f32>,   // tint
    @location(8) flip    : u32,         // bit 0 mirror x, bit 1 mirror y
}

struct VsOut {
    @builtin(position) position : vec4<f32>,
    @location(0) uv    : vec2<f32>,
    @location(1) color : vec4<f32>,
}

// Two triangles covering the unit square; (0,0) is the top-left corner, y grows downwards
// like image rows do.
var<private> CORNERS : array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
    vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
);

@vertex
fn vs_main(@builtin(vertex_index) vertex : u32, inst : Instance) -> VsOut {
    let corner = CORNERS[vertex];
    // quad space: centred on the origin, y up (the corner's y is flipped), scaled by size
    let local = vec2<f32>(corner.x - 0.5, 0.5 - corner.y) * inst.size;
    let world_matrix = mat4x4<f32>(inst.world_x, inst.world_y, inst.world_z, inst.world_w);

    // flipping just swaps which UV corner a vertex gets
    var t = corner;
    if ((inst.flip & 1u) != 0u) { t.x = 1.0 - t.x; }
    if ((inst.flip & 2u) != 0u) { t.y = 1.0 - t.y; }

    var out : VsOut;
    out.position = camera.view_projection * world_matrix * vec4<f32>(local, 0.0, 1.0);
    out.uv = mix(inst.uv_min, inst.uv_max, t);
    out.color = inst.color;
    return out;
}

@fragment
fn fs_main(in : VsOut) -> @location(0) vec4<f32> {
    return textureSample(sprite_texture, sprite_sampler, in.uv) * in.color;
}
