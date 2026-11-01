struct State { view_projection: mat4x4<f32>, background: vec4<f32>, params: vec4<f32> };
@group(0) @binding(0) var<uniform> state: State;
@group(0) @binding(1) var sun_texture: texture_2d<f32>;
@group(0) @binding(2) var sun_sampler: sampler;
struct Output { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> };
@vertex fn vertex(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>) -> Output {
    var output: Output;
    output.position = state.view_projection * vec4<f32>(position, 1.0);
    output.uv = uv;
    return output;
}
fn composite(input: Output) -> vec4<f32> {
    let texel = textureSample(sun_texture, sun_sampler, input.uv);
    return vec4<f32>(min(state.background.rgb + texel.rgb * texel.a * state.params.x, vec3<f32>(1.0)), 1.0);
}
@fragment fn fragment_unorm_target(input: Output) -> @location(0) vec4<f32> {
    return composite(input);
}
@fragment fn fragment_srgb_target(input: Output) -> @location(0) vec4<f32> {
    let color = composite(input);
    let low = color.rgb / 12.92;
    let high = pow((color.rgb + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return vec4<f32>(select(low, high, color.rgb > vec3<f32>(0.04045)), color.a);
}
