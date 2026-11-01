@group(0) @binding(0) var sky: texture_2d<f32>;
@vertex fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let vertices = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(vertices[index], 0.0, 1.0);
}
fn pixel(position: vec4<f32>) -> vec3<f32> { return textureLoad(sky, vec2<i32>(position.xy), 0).rgb; }
@fragment fn fragment_unorm(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(pixel(position), 1.0);
}
@fragment fn fragment_srgb(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let color = pixel(position);
    let low = color / 12.92;
    let high = pow((color + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return vec4<f32>(select(low, high, color > vec3<f32>(0.04045)), 1.0);
}
