// sprite.wgsl: STUB. Write the sprite shader here.
//
// `renderer.rs` includes this file with `include_str!` once you create the pipeline, so it is
// not compiled yet (an empty module is fine until then).
//
// Expected bindings (suggested layout; the Rust side must match whatever you choose):
//
//   @group(0) @binding(0) var<uniform> camera : Camera;
//       struct Camera { view_projection : mat4x4<f32> }
//       Upload `frame.camera.view_projection` as-is: it is column-major, world -> clip,
//       wgpu clip space (y up, depth 0..1). With no camera, use the identity or skip drawing.
//
//   @group(1) @binding(0) var sprite_texture : texture_2d<f32>;
//   @group(1) @binding(1) var sprite_sampler : sampler;
//       One bind group per GPU texture, built from `frame.textures` changes. Handle id 0
//       ("no texture") should bind a 1x1 white texture so the tint alone is drawn.
//
// Per-instance vertex attributes (one instance per `SpriteInstance` in `frame.sprites`,
// already sorted by layer; fields as in the engine's `SpriteInstance`):
//
//   world_x, world_y, world_z, world_w : vec4<f32>   // `world` columns (quad -> world)
//   size                               : vec2<f32>   // world units, never zero
//   uv_min, uv_max                     : vec2<f32>   // texture coordinates (0,0 = top-left)
//   color                              : vec4<f32>   // tint, multiply with the texture
//   flip                               : u32         // bit 0 mirror x, bit 1 mirror y
//
// The vertex stage builds the quad from `@builtin(vertex_index)` (a unit square centred on
// the origin, scaled by `size`, transformed by `world`), the fragment stage samples the
// texture at the interpolated uv and multiplies by `color`. Enable alpha blending; the
// engine sorts sprites back to front already.
//
// (no code yet)
