struct State {
    matrix: mat4x4<f32>,
    color: vec4<f32>,
    fog_color: vec4<f32>,
    params: vec4<f32>,
};
@group(0) @binding(0) var<uniform> state: State;
struct Output {
    @builtin(position) position: vec4<f32>,
    @location(0) distance: f32,
};
@vertex fn vertex(@location(0) position: vec3<f32>) -> Output {
    var output: Output;
    output.position = state.matrix * vec4<f32>(position, 1.0);
    output.distance = length(position);
    return output;
}
@fragment fn fragment(input: Output) -> @location(0) vec4<f32> {
    var amount = 0.0;
    if input.distance > 0.0 {
        if input.distance >= state.params.x {
            amount = 1.0;
        } else {
            amount = input.distance / state.params.x;
        }
    }
    return vec4<f32>(mix(state.color.rgb, state.fog_color.rgb, amount * state.fog_color.a), state.color.a);
}
