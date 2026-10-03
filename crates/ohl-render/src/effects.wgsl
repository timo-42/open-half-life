// Project-authored untextured effect geometry. No game assets are embedded.
struct Camera {
    view_projection: mat4x4<f32>,
    params: vec4<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) color: vec4<f32>,
}
struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}
@vertex
fn vertex_main(input: VertexInput) -> VertexOutput {
    var output: VertexOutput;
    output.position = camera.view_projection * vec4<f32>(input.position, 1.0);
    output.color = input.color;
    return output;
}
@fragment
fn fragment_main(input: VertexOutput) -> @location(0) vec4<f32> {
    var color = input.color.rgb;
    if (camera.params.x > 0.5) {
        color = pow(color, vec3<f32>(2.2));
    }
    return vec4<f32>(color, input.color.a);
}
