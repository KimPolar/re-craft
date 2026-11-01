@group(0) @binding(0) var overlay: texture_2d<f32>;
@group(0) @binding(1) var overlay_sampler: sampler;
@group(0) @binding(2) var glint_mask: texture_2d<u32>;
@group(0) @binding(3) var glint_texture: texture_2d<f32>;
@group(0) @binding(4) var glint_sampler: sampler;

struct GlintUniform {
    offset: vec2<f32>,
    strength: f32,
    enabled: f32,
}
@group(0) @binding(5) var<uniform> glint: GlintUniform;
@group(1) @binding(0) var<uniform> gui: vec4<f32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, 1.0), vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0)
    );
    let uv = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(1.0, 0.0)
    );
    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    output.uv = uv[index];
    return output;
}

@fragment
fn fragment(input: VertexOutput) -> @location(0) vec4<f32> {
    let overlay_dimensions = textureDimensions(overlay);
    let pixel = min(vec2<u32>(input.position.xy / gui.x), overlay_dimensions - vec2<u32>(1u));
    var color = textureLoad(overlay, vec2<i32>(pixel), 0);
    if glint.enabled > 0.5 {
        let mask = textureLoad(glint_mask, vec2<i32>(pixel), 0);
        if mask.a > 0u {
            // Unsupported models retain the old local-coordinate approximation.
            let scale = select(0.125, 8.0, mask.b != 0u);
            let uv = vec2<f32>(mask.rg) * (scale / 65535.0);
            let rotated = vec2<f32>(0.98480775 * uv.x - 0.17364818 * uv.y,
                                    0.17364818 * uv.x + 0.98480775 * uv.y);
            let texel = textureSampleLevel(glint_texture, glint_sampler, rotated + glint.offset, 0.0).rgb;
            let contribution = texel * glint.strength;
            color = vec4<f32>(color.rgb + contribution * contribution * (f32(mask.a) / 255.0), color.a);
        }
    }
    return color;
}
