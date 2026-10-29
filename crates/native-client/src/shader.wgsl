struct Camera {
    view_projection: mat4x4<f32>,
    viewport: vec2<f32>,
    padding: vec2<f32>,
}

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
@group(1) @binding(0) var<uniform> lightmap: array<vec4<f32>, 256>;
struct Fog {
    eye: vec4<f32>,
    color: vec4<f32>,
    range: vec4<f32>,
}
@group(2) @binding(0) var<uniform> fog: Fog;

struct WorldOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) shade: f32,
    @location(2) tint: vec3<f32>,
    @location(3) fog_distance: f32,
}

@vertex
fn world_vertex(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) shade: f32, @location(3) tint: vec3<f32>) -> WorldOutput {
    var output: WorldOutput;
    output.position = camera.view_projection * vec4<f32>(position, 1.0);
    output.uv = uv;
    output.shade = shade;
    output.tint = tint;
    output.fog_distance = 0.0;
    return output;
}

@vertex
fn terrain_vertex(@location(0) position: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) shade: f32, @location(3) tint: vec3<f32>, @location(4) light: u32) -> WorldOutput {
    var output: WorldOutput;
    output.position = camera.view_projection * vec4<f32>(position, 1.0);
    output.uv = uv;
    output.shade = shade;
    output.tint = tint;
    output.fog_distance = length(position - fog.eye.xyz);
    if light < 256u { output.tint *= lightmap[light].rgb; }
    return output;
}

@fragment
fn terrain_fragment(input: WorldOutput) -> @location(0) vec4<f32> {
    let color = textureSample(atlas, atlas_sampler, input.uv);
    if color.a < 0.1 { discard; }
    let source = color.rgb * input.shade * input.tint;
    let amount = select(0.0, clamp((input.fog_distance - fog.range.x) / (fog.range.y - fog.range.x), 0.0, 1.0), fog.range.z > 0.5);
    return vec4<f32>(mix(source, fog.color.rgb, amount), color.a);
}

@fragment
fn world_fragment(input: WorldOutput) -> @location(0) vec4<f32> {
    let color = textureSample(atlas, atlas_sampler, input.uv);
    if color.a < 0.1 { discard; }
    return vec4<f32>(color.rgb * input.shade * input.tint, color.a);
}

@fragment
fn orb_fragment(input: WorldOutput) -> @location(0) vec4<f32> {
    let color = textureSample(atlas, atlas_sampler, input.uv);
    if color.a < 0.1 { discard; }
    return vec4<f32>(color.rgb * input.tint, color.a * (128.0 / 255.0));
}

@vertex
fn crosshair_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let points = array<vec2<f32>, 12>(
        vec2<f32>(-7.0, -1.0), vec2<f32>(7.0, -1.0), vec2<f32>(7.0, 1.0),
        vec2<f32>(-7.0, -1.0), vec2<f32>(7.0, 1.0), vec2<f32>(-7.0, 1.0),
        vec2<f32>(-1.0, -7.0), vec2<f32>(1.0, -7.0), vec2<f32>(1.0, 7.0),
        vec2<f32>(-1.0, -7.0), vec2<f32>(1.0, 7.0), vec2<f32>(-1.0, 7.0)
    );
    return vec4<f32>(points[index] * 2.0 / camera.viewport, 0.0, 1.0);
}

@fragment
fn crosshair_fragment() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0, 1.0, 1.0, 0.9);
}
