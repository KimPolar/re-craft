struct Camera { view_projection: mat4x4<f32>, viewport: vec2<f32>, padding: vec2<f32> }
@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var item_texture: texture_2d<f32>;
@group(0) @binding(2) var item_sampler: sampler;
@group(1) @binding(0) var glint_texture: texture_2d<f32>;
@group(1) @binding(1) var glint_sampler: sampler;
struct GlintUniform { offset: vec2<f32>, strength: f32, enabled: f32 }
@group(1) @binding(2) var<uniform> glint: GlintUniform;

struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) shade: f32,
    @location(2) tint: vec3<f32>,
    @location(3) glint_uv: vec3<f32>,
}

@vertex
fn vertex(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>,
          @location(2) shade: f32, @location(3) tint: vec3<f32>,
          @location(4) glint_uv: vec3<f32>, @location(5) c0: vec4<f32>,
          @location(6) c1: vec4<f32>, @location(7) c2: vec4<f32>,
          @location(8) c3: vec4<f32>) -> Output {
    var out: Output;
    out.position = camera.view_projection * mat4x4<f32>(c0, c1, c2, c3) * vec4<f32>(position, 1.0);
    out.uv = uv;
    out.shade = shade;
    out.tint = tint;
    out.glint_uv = glint_uv;
    return out;
}

@fragment
fn fragment(input: Output) -> @location(0) vec4<f32> {
    let color = textureSample(item_texture, item_sampler, input.uv);
    if color.a < 0.1 { discard; }
    var rgb = color.rgb * input.tint * input.shade;
    var opacity = color.a;
    if glint.enabled > 0.5 && input.glint_uv.z > 0.5 {
        let scale = select(8.0, 0.125, input.glint_uv.z > 1.5);
        let uv = input.glint_uv.xy * scale;
        let rotated = vec2<f32>(0.98480775 * uv.x - 0.17364818 * uv.y,
                                0.17364818 * uv.x + 0.98480775 * uv.y);
        let texel = textureSampleLevel(glint_texture, glint_sampler, rotated + glint.offset, 0.0).rgb;
        let contribution = texel * glint.strength;
        rgb += contribution * contribution;
        opacity = max(opacity, glint.strength);
    }
    return vec4<f32>(rgb, opacity);
}
