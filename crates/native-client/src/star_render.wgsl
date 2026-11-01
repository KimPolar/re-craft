struct State { matrix: mat4x4<f32>, color: vec4<f32> };
@group(0) @binding(0) var<uniform> state: State;
@vertex fn star_vertex(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    return state.matrix * vec4<f32>(position, 1.0);
}
@fragment fn star_fragment() -> @location(0) vec4<f32> { return state.color; }
