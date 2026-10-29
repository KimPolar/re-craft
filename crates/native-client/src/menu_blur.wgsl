@group(0) @binding(0) var image: texture_2d<f32>;
@group(0) @binding(1) var image_sampler: sampler;
@group(0) @binding(2) var<uniform> config: vec4<u32>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vertex(@builtin(vertex_index) index: u32) -> VertexOutput {
    let positions = array<vec2<f32>, 3>(vec2<f32>(-1.0, 1.0), vec2<f32>(-1.0, -3.0), vec2<f32>(3.0, 1.0));
    var output: VertexOutput;
    output.position = vec4<f32>(positions[index], 0.0, 1.0);
    return output;
}

fn blurred(position: vec2<i32>, direction: vec2<i32>) -> vec4<f32> {
    let size = vec2<f32>(textureDimensions(image));
    let origin = (vec2<f32>(position) + vec2<f32>(0.5)) / size;
    let step = vec2<f32>(direction) / size;
    let radius = i32(config.x);
    var color = vec4<f32>(0.0);
    for (var offset = -radius; offset < radius; offset += 2) {
        color += textureSampleLevel(image, image_sampler, origin + step * (f32(offset) + 0.5), 0.0);
    }
    color += textureSampleLevel(image, image_sampler, origin + step * f32(radius), 0.0) * 0.5;
    return color / (f32(radius) + 0.5);
}

@fragment
fn horizontal(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return blurred(vec2<i32>(position.xy), vec2<i32>(1, 0));
}

@fragment
fn vertical(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    return blurred(vec2<i32>(position.xy), vec2<i32>(0, 1));
}
