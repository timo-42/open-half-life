// World-surface shader: diffuse texture modulated by the baked lightmap.
//
// GoldSrc composites lighting in gamma space with no overbright multiplier,
// so the baseline here is a plain product of the two samples. When the
// render target is an sRGB format the hardware will encode the result on
// write, so the shader converts back to linear first to keep the on-screen
// result identical to the non-sRGB path.

struct Camera {
    view_projection: mat4x4<f32>,
    // x: 1.0 when the render target is sRGB, 0.0 otherwise.
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

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let diffuse = textureSample(diffuse_texture, diffuse_sampler, input.uv);
    if (diffuse.a < 0.5) {
        discard;
    }
    let light = textureSample(lightmap_texture, lightmap_sampler, input.lightmap_uv).rgb;
    var color = diffuse.rgb * light;
    if (camera.parameters.x > 0.5) {
        color = pow(color, vec3<f32>(2.2));
    }
    return vec4<f32>(color, 1.0);
}
