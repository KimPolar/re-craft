@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var<uniform> state: vec4<f32>;
struct Output { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vertex(@builtin(vertex_index) index: u32) -> Output {
    let positions = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    let p = positions[index];
    var output: Output;
    output.position = vec4<f32>(p, 0.0, 1.0);
    output.uv = vec2<f32>(state.x + (1.0 - p.x) * 2.0, state.y + (1.0 - p.y) * 2.0);
    return output;
}
fn sample_color(input: Output) -> vec4<f32> {
    let texel = textureSample(image, image_sampler, input.uv);
    return vec4<f32>(texel.rgb * state.z, texel.a * state.w);
}
@fragment fn fragment_unorm(input: Output) -> @location(0) vec4<f32> { return sample_color(input); }
@fragment fn fragment_srgb(input: Output) -> @location(0) vec4<f32> {
    let color = sample_color(input);
    let low = color.rgb / 12.92;
    let high = pow((color.rgb + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return vec4<f32>(select(low, high, color.rgb > vec3<f32>(0.04045)), color.a);
}
