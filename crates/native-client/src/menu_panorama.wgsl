@group(0) @binding(0) var panorama: texture_cube<f32>;
@group(0) @binding(1) var face_sampler: sampler;
@group(0) @binding(2) var<uniform> parameters: vec4<f32>;
@group(0) @binding(3) var panorama_overlay: texture_2d<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let positions = array<vec2<f32>, 3>(vec2<f32>(-1.0, 1.0), vec2<f32>(-1.0, -3.0), vec2<f32>(3.0, 1.0));
    let coordinates = array<vec2<f32>, 3>(vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 2.0), vec2<f32>(2.0, 0.0));
    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    output.uv = coordinates[index];
    return output;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    // CubeMap.render uses an 85-degree vertical FOV and a 10-degree pitch.
    let u = (2.0 * input.uv.x - 1.0) * parameters.x * 0.9163312;
    let v = (1.0 - 2.0 * input.uv.y) * 0.9163312;
    let pitched_y = v * 0.98480773 - 0.17364818;
    let pitched_z = v * 0.17364818 + 0.98480773;
    let cs = cos(parameters.y);
    let sn = sin(parameters.y);
    let dx = u * cs + pitched_z * sn;
    let dy = pitched_y;
    let dz = -u * sn + pitched_z * cs;
    let color = textureSampleLevel(panorama, face_sampler, vec3<f32>(dx, -dy, dz), 0.0);
    let overlay_size = textureDimensions(panorama_overlay);
    let overlay_xy = clamp(vec2<i32>(input.uv * vec2<f32>(overlay_size)),
                           vec2<i32>(0), vec2<i32>(overlay_size) - vec2<i32>(1));
    let overlay = textureLoad(panorama_overlay, overlay_xy, 0);
    return vec4<f32>(mix(color.rgb, overlay.rgb, overlay.a), 1.0);
}
