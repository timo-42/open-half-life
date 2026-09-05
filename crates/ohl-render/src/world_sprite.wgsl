// The sprite billboard pass: a camera-facing (or fixed-orientation) quad per
// instance, built entirely on the GPU from a per-instance right/up axis pair
// computed on the CPU (see `renderer.rs`'s `draw_sprites`) from the
// documented SPR `type` field. Depth-tested against the already-rendered
// opaque/world pass but never depth-written, matching every other
// translucent pass in this renderer (liquid, submodel alpha/additive).

struct Instance {
    view_projection: mat4x4<f32>,
    // xyz: world-space quad centre.
    origin: vec4<f32>,
    // xyz: the quad's local +X (`corner.x`) world-space axis.
    right: vec4<f32>,
    // xyz: the quad's local +Y (`corner.y`) world-space axis.
    up: vec4<f32>,
    // x: half-width in world units. y: half-height in world units.
    // z: this instance's alpha (`RenderProps::alpha`). w: 1.0 when sRGB
    // output.
    params: vec4<f32>,
}

@group(0) @binding(0) var<uniform> instance: Instance;

@group(1) @binding(0) var sprite_texture: texture_2d<f32>;
@group(1) @binding(1) var sprite_sampler: sampler;

struct VertexInput {
    // A unit quad corner in `-1..=1` on both axes.
    @location(0) corner: vec2<f32>,
    @location(1) uv: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vertex_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let world_position = instance.origin.xyz
        + instance.right.xyz * (input.corner.x * instance.params.x)
        + instance.up.xyz * (input.corner.y * instance.params.y);
    output.clip_position = instance.view_projection * vec4<f32>(world_position, 1.0);
    output.uv = input.uv;
    return output;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    var sample = textureSample(sprite_texture, sprite_sampler, input.uv);
    var color = sample.rgb;
    if (instance.params.w > 0.5) {
        color = pow(color, vec3<f32>(2.2));
    }
    return vec4<f32>(color, sample.a * instance.params.z);
}
