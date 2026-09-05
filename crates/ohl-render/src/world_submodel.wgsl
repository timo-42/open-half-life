// Brush-entity submodel shader: identical diffuse * lightmap composition as
// world.wgsl, but the camera's view_projection is pre-multiplied on the CPU
// by the entity's placement transform (see `renderer.rs`,
// `WorldRenderer::draw_world_submodel`), and the render-mode parameters
// (`ohl_render::render_props::RenderProps`, Valve Developer Community
// "Render Modes"; see `docs/FORMAT_SOURCES.md`, "Rendering conventions")
// select the entity's alpha and an optional colour substitution.

struct Camera {
    view_projection: mat4x4<f32>, // camera.view_projection() * instance.transform
    // x: 1.0 when sRGB output. y: RenderProps::alpha(). z: 1.0 when this
    // mode substitutes render_color for the texture's own colour
    // (RenderProps::uses_render_color()).
    parameters: vec4<f32>,
    // rgb: RenderProps::color, normalised to 0..1. w: unused.
    render_color: vec4<f32>,
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
    var base_color = diffuse.rgb;
    if (camera.parameters.z > 0.5) {
        base_color = camera.render_color.rgb;
    }
    var color = base_color * light;
    if (camera.parameters.x > 0.5) {
        color = pow(color, vec3<f32>(2.2));
    }
    return vec4<f32>(color, camera.parameters.y);
}
