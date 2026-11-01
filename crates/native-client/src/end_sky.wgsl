struct Camera { view_projection: mat4x4<f32>, background: vec4<f32> };
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var sky_texture: texture_2d<f32>;
@group(0) @binding(2) var sky_sampler: sampler;

struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex fn vertex(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>) -> Output {
    var output: Output;
    output.position = camera.view_projection * vec4<f32>(position, 1.0);
    output.uv = uv;
    return output;
}

fn tinted(input: Output) -> vec4<f32> {
    let texel = textureSample(sky_texture, sky_sampler, input.uv);
    return vec4<f32>(mix(camera.background.rgb, texel.rgb * (40.0 / 255.0), texel.a), 1.0);
}

@fragment fn fragment_unorm_target(input: Output) -> @location(0) vec4<f32> {
    return tinted(input);
}

@fragment fn fragment_srgb_target(input: Output) -> @location(0) vec4<f32> {
    let color = tinted(input);
    let low = color.rgb / 12.92;
    let high = pow((color.rgb + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return vec4<f32>(select(low, high, color.rgb > vec3<f32>(0.04045)), color.a);
}
