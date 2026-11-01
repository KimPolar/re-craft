@group(0) @binding(0) var overlay: texture_2d<f32>;
@group(1) @binding(0) var<uniform> gui: vec4<f32>;

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, 1.0), vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0),
        vec2<f32>(-1.0, 1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0)
    );
    return vec4<f32>(positions[index], 0.0, 1.0);
}

@fragment
fn fragment(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let dimensions = textureDimensions(overlay);
    let pixel = min(vec2<u32>(position.xy / gui.x), dimensions - vec2<u32>(1u));
    return textureLoad(overlay, vec2<i32>(pixel), 0);
}
