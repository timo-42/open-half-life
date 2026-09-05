// Studio-model shader: GPU vertex skinning, a single directional light plus
// ambient, and the documented GoldSrc chrome/additive/masked texture modes.
//
// Skinning is one bone per vertex, which is what the MDL v10 format stores:
// the vertex is transformed by its bone's world matrix and then by the
// instance's placement matrix. Lighting is evaluated per vertex (Lambert
// against one directional light, plus an ambient term the caller samples
// from the map's lightmap at the model's origin) and interpolated, which
// matches the low-polygon look of the era closely enough for this
// milestone and is documented as an approximation in `docs/MILESTONES.md`.

// Must match `ohl_world::MAX_BONES`.
const MAX_BONES: u32 = 128u;

struct Instance {
    view_projection: mat4x4<f32>,
    // The world-space view matrix, used for the chrome sphere mapping.
    view: mat4x4<f32>,
    // Placement of the model in the world.
    model: mat4x4<f32>,
    // xyz: ambient colour; w: unused.
    ambient: vec4<f32>,
    // xyz: unit direction the light travels in; w: 1.0 when the render
    // target is sRGB, 0.0 otherwise.
    light_direction: vec4<f32>,
    // xyz: directional light colour; w: unused.
    light_color: vec4<f32>,
    bones: array<mat4x4<f32>, MAX_BONES>,
}

struct Material {
    // x: chrome, y: fullbright, z: masked, w: additive (each 0.0 or 1.0).
    flags: vec4<f32>,
}

@group(0) @binding(0) var<uniform> instance: Instance;

@group(1) @binding(0) var diffuse_texture: texture_2d<f32>;
@group(1) @binding(1) var diffuse_sampler: sampler;
@group(1) @binding(2) var<uniform> material: Material;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) bone: u32,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) light: vec3<f32>,
}

@vertex
fn vertex_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    let bone = min(input.bone, MAX_BONES - 1u);
    let skin = instance.model * instance.bones[bone];
    let world_position = skin * vec4<f32>(input.position, 1.0);
    let world_normal = normalize((skin * vec4<f32>(input.normal, 0.0)).xyz);
    output.clip_position = instance.view_projection * world_position;

    if (material.flags.x > 0.5) {
        // Chrome: the stored texture coordinates are unused. The surface
        // normal is projected into view space and its XY used as a
        // spherical environment ("matcap") lookup, which is the documented
        // behaviour of GoldSrc's chrome mode expressed as a sphere map.
        let view_normal = normalize((instance.view * vec4<f32>(world_normal, 0.0)).xyz);
        output.uv = view_normal.xy * 0.5 + vec2<f32>(0.5, 0.5);
    } else {
        output.uv = input.uv;
    }

    if (material.flags.y > 0.5) {
        output.light = vec3<f32>(1.0, 1.0, 1.0);
    } else {
        let lambert = max(dot(world_normal, -instance.light_direction.xyz), 0.0);
        output.light = instance.ambient.xyz + instance.light_color.xyz * lambert;
    }
    return output;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let diffuse = textureSample(diffuse_texture, diffuse_sampler, input.uv);
    if (material.flags.z > 0.5 && diffuse.a < 0.5) {
        discard;
    }
    var color = diffuse.rgb * min(input.light, vec3<f32>(1.0, 1.0, 1.0));
    if (instance.light_direction.w > 0.5) {
        // The colour target encodes sRGB on write; undo that so both target
        // formats produce the same gamma-space result, exactly as the world
        // shader does.
        color = pow(color, vec3<f32>(2.2));
    }
    // Additive surfaces reach the target through an additive blend state,
    // so a dark texel simply contributes nothing; alpha stays opaque.
    return vec4<f32>(color, 1.0);
}
