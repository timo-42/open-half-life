// The sky pass: an inside-out unit cube, drawn with the camera's rotation
// only (translation stripped from the view matrix), sampled as a cubemap.
//
// Drawn after world (and studio) geometry into the same colour/depth
// target, with depth writes disabled and the standard `Less` compare: it
// only shows through pixels the opaque passes left at the cleared far
// depth, i.e. "world occludes sky".

struct Camera {
    // Rotation-only view * projection.
    view_projection: mat4x4<f32>,
    // x: 1.0 when the render target is sRGB, 0.0 otherwise.
    parameters: vec4<f32>,
}

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var sky_texture: texture_cube<f32>;
@group(0) @binding(2) var sky_sampler: sampler;

// A unit cube's 12 triangles (36 corners), scaled up in the vertex shader.
// The camera always sits at this cube's centre (translation is stripped),
// so any half-extent that fits between the near and far clip planes covers
// the whole viewport from every direction.
const HALF_EXTENT: f32 = 64.0;

const POSITIONS = array<vec3<f32>, 36>(
    // +X
    vec3<f32>(1.0, -1.0, -1.0), vec3<f32>(1.0, 1.0, -1.0), vec3<f32>(1.0, 1.0, 1.0),
    vec3<f32>(1.0, -1.0, -1.0), vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(1.0, -1.0, 1.0),
    // -X
    vec3<f32>(-1.0, 1.0, -1.0), vec3<f32>(-1.0, -1.0, -1.0), vec3<f32>(-1.0, -1.0, 1.0),
    vec3<f32>(-1.0, 1.0, -1.0), vec3<f32>(-1.0, -1.0, 1.0), vec3<f32>(-1.0, 1.0, 1.0),
    // +Y
    vec3<f32>(1.0, 1.0, -1.0), vec3<f32>(-1.0, 1.0, -1.0), vec3<f32>(-1.0, 1.0, 1.0),
    vec3<f32>(1.0, 1.0, -1.0), vec3<f32>(-1.0, 1.0, 1.0), vec3<f32>(1.0, 1.0, 1.0),
    // -Y
    vec3<f32>(-1.0, -1.0, -1.0), vec3<f32>(1.0, -1.0, -1.0), vec3<f32>(1.0, -1.0, 1.0),
    vec3<f32>(-1.0, -1.0, -1.0), vec3<f32>(1.0, -1.0, 1.0), vec3<f32>(-1.0, -1.0, 1.0),
    // +Z (up)
    vec3<f32>(-1.0, -1.0, 1.0), vec3<f32>(1.0, -1.0, 1.0), vec3<f32>(1.0, 1.0, 1.0),
    vec3<f32>(-1.0, -1.0, 1.0), vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(-1.0, 1.0, 1.0),
    // -Z (down)
    vec3<f32>(-1.0, 1.0, -1.0), vec3<f32>(1.0, 1.0, -1.0), vec3<f32>(1.0, -1.0, -1.0),
    vec3<f32>(-1.0, 1.0, -1.0), vec3<f32>(1.0, -1.0, -1.0), vec3<f32>(-1.0, -1.0, -1.0),
);

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) direction: vec3<f32>,
}

@vertex
fn vertex_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let position = POSITIONS[vertex_index % 36u];
    var output: VertexOutput;
    output.clip_position = camera.view_projection * vec4<f32>(position * HALF_EXTENT, 1.0);
    // Force the post-divide depth to exactly `1.0` (the far plane, matching
    // this project's depth-clear value), regardless of `HALF_EXTENT`'s
    // arbitrary geometric size. Without this, the sky cube's own clip-space
    // depth reflects its ~64-unit distance from the camera, which is
    // *nearer* than any opaque geometry farther than that — and the sky
    // pass's `LessEqual` depth test then lets the sky win and overpaint
    // real, correctly depth-written occluding geometry beyond that
    // distance (fidelity finding F1). Setting `z = w` is the standard
    // "sky at infinity" trick: after the perspective divide (`z / w`), the
    // depth is `1.0` no matter how far along the ray that division
    // happened, so the sky only ever draws where the depth buffer still
    // holds the cleared far value, i.e. exactly "nothing else was drawn
    // here", independent of `HALF_EXTENT`.
    output.clip_position.z = output.clip_position.w;
    output.direction = position;
    return output;
}

@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    var color = textureSample(sky_texture, sky_sampler, normalize(input.direction)).rgb;
    if (camera.parameters.x > 0.5) {
        color = pow(color, vec3<f32>(2.2));
    }
    return vec4<f32>(color, 1.0);
}
