struct Camera {
    view_projection: mat4x4<f32>,
    viewport: vec2<f32>,
    padding: vec2<f32>,
}
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var weather_texture: texture_2d<f32>;
@group(0) @binding(2) var weather_sampler: sampler;
@group(1) @binding(0) var<uniform> lightmap: array<vec4<f32>, 256>;
struct Fog { eye: vec4<f32>, color: vec4<f32>, range: vec4<f32> }
@group(2) @binding(0) var<uniform> fog: Fog;

struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) opacity: f32,
    @location(2) tint: vec3<f32>,
    @location(3) distance: f32,
}
@vertex
fn vertex(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) opacity: f32, @location(3) light: u32) -> Output {
    var result: Output;
    result.position = camera.view_projection * vec4<f32>(position, 1.0);
    result.uv = uv;
    result.opacity = opacity;
    result.tint = lightmap[min(light, 255u)].rgb;
    result.distance = length(position - fog.eye.xyz);
    return result;
}
@fragment
fn fragment(input: Output) -> @location(0) vec4<f32> {
    let texel = textureSample(weather_texture, weather_sampler, input.uv);
    let alpha = texel.a * input.opacity;
    if alpha < 0.003921569 { discard; }
    var fog_amount = 0.0;
    if fog.range.z > 0.5 && input.distance > fog.range.x {
        if input.distance >= fog.range.y { fog_amount = 1.0; }
        else { fog_amount = (input.distance - fog.range.x) / (fog.range.y - fog.range.x); }
    }
    return vec4<f32>(mix(texel.rgb * input.tint, fog.color.rgb, fog_amount), alpha);
}
