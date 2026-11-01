@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var<uniform> projection: mat4x4<f32>;

struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex fn vertex(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>) -> Output {
    var output: Output;
    output.position = projection * vec4<f32>(position, 1.0);
    output.uv = uv;
    return output;
}

fn color(input: Output) -> vec4<f32> {
    let texel = textureSample(image, image_sampler, input.uv);
    return vec4<f32>(texel.rgb, texel.a * (229.0 / 255.0));
}

@fragment fn fragment_unorm(input: Output) -> @location(0) vec4<f32> { return color(input); }
@fragment fn fragment_srgb(input: Output) -> @location(0) vec4<f32> {
    let value = color(input);
    let low = value.rgb / 12.92;
    let high = pow((value.rgb + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return vec4<f32>(select(low, high, value.rgb > vec3<f32>(0.04045)), value.a);
}
