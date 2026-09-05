// The liquid ("water") pass: same diffuse * lightmap composition as
// world.wgsl, but with a sine-based UV turbulence warp and alpha blending,
// drawn after the opaque world pass with depth writes disabled.
//
// The turbulence formula and its constants (0.125 amplitude, 4.0 cross-axis
// scale) must match `water.rs`'s `turbulence_offset`; see that module's
// doc comment for the (documented-effect-only, project-original) rationale.

struct Camera {
    view_projection: mat4x4<f32>,
    // x: 1.0 when sRGB output. y: elapsed seconds, for the turbulence phase.
    // z: this pass's alpha (`renderamt`/255, worldspawn default 1.0).
    parameters: vec4<f32>,
}

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var lightmap_texture: texture_2d<f32>;
@group(0) @binding(2) var lightmap_sampler: sampler;

@group(1) @binding(0) var diffuse_texture: texture_2d<f32>;
@group(1) @binding(1) var diffuse_sampler: sampler;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lightmap_uv: vec2<f32>,
}

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) lightmap_uv: vec2<f32>,
}

@vertex
fn vertex_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.clip_position = camera.view_projection * vec4<f32>(input.position, 1.0);
    output.uv = input.uv;
    output.lightmap_uv = input.lightmap_uv;
    return output;
}

fn turbulence(coordinate: f32, cross_coordinate: f32, time_seconds: f32) -> f32 {
    let phase = cross_coordinate * 4.0
        + time_seconds * 6.283185307
        + coordinate * 0.5;
    return sin(phase) * 0.125;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let time = camera.parameters.y;
    let warped_uv = vec2<f32>(
        input.uv.x + turbulence(input.uv.x, input.uv.y, time),
        input.uv.y + turbulence(input.uv.y, input.uv.x, time),
    );
    let diffuse = textureSample(diffuse_texture, diffuse_sampler, warped_uv);
    let light = textureSample(lightmap_texture, lightmap_sampler, input.lightmap_uv).rgb;
    var color = diffuse.rgb * light;
    if (camera.parameters.x > 0.5) {
        color = pow(color, vec3<f32>(2.2));
    }
    return vec4<f32>(color, diffuse.a * camera.parameters.z);
}
