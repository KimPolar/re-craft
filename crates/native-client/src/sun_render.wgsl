struct State { view_projection: mat4x4<f32>, background: vec4<f32>, params: vec4<f32>,
    sun_from_body: mat4x4<f32>, sun_origin: vec4<f32>, sun_rect: vec4<f32> };
@group(0) @binding(0) var<uniform> state: State;
@group(0) @binding(1) var sun_texture: texture_2d<f32>;
@group(0) @binding(2) var sun_sampler: sampler;
@group(0) @binding(3) var previous_sun: texture_2d<f32>;
struct Output { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) local: vec3<f32> };
@vertex fn vertex(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>) -> Output {
    var output: Output;
    output.position = state.view_projection * vec4<f32>(position, 1.0);
    output.uv = uv;
    output.local = position;
    return output;
}
fn composite(input: Output) -> vec4<f32> {
    let texel = textureSample(sun_texture, sun_sampler, input.uv);
    var color = state.background.rgb;
    if state.params.y > 0.0 {
        let body_point = (state.sun_from_body * vec4<f32>(input.local, 1.0)).xyz;
        let direction = body_point - state.sun_origin.xyz;
        if abs(direction.y) > 0.000001 {
            let distance = -state.sun_origin.y / direction.y;
            let point = state.sun_origin.xyz + distance * direction;
            if distance > 0.0 && all(abs(point.xz) <= vec2<f32>(1.0)) {
                let uv = mix(state.sun_rect.xy, state.sun_rect.zw, point.xz * 0.5 + vec2<f32>(0.5));
                let previous = textureSampleLevel(previous_sun, sun_sampler, uv, 0.0);
                color = min(color + previous.rgb * previous.a * state.params.x, vec3<f32>(1.0));
            }
        }
    }
    return vec4<f32>(min(color + texel.rgb * texel.a * state.params.x, vec3<f32>(1.0)), 1.0);
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
