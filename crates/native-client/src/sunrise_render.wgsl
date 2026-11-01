struct State {
    view_projection: mat4x4<f32>,
    color: vec4<f32>,
};
@group(0) @binding(0) var<uniform> state: State;
struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) alpha: f32,
};
@vertex fn vertex(@location(0) position: vec3<f32>, @location(1) alpha: f32) -> Output {
    var output: Output;
    output.position = state.view_projection * vec4<f32>(position, 1.0);
    output.alpha = alpha;
    return output;
}
@fragment fn fragment(input: Output) -> @location(0) vec4<f32> {
    return vec4<f32>(state.color.rgb, state.color.a * input.alpha);
}
