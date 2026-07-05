// Depth-only occluder pass. Surfaces are invisible in this engine — they
// exist purely to eat the lines behind them. The pipeline applies a positive
// depth bias so edges lying exactly on a face survive the line pass's test.

@vertex
fn vs_main(@location(0) pos: vec3<f32>) -> @builtin(position) vec4<f32> {
    return camera.view_proj * vec4<f32>(pos, 1.0);
}
