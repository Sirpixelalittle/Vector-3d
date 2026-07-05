// Shared camera block, prepended to every pass shader at pipeline creation.
// Layout must match `CameraUniform` in camera.rs (112 bytes).

struct Camera {
    view_proj: mat4x4<f32>,
    viewport: vec2<f32>,
    line_width: f32,
    fog_density: f32,
    eye: vec3<f32>,
    time: f32,
    // Master glow dial: scales how far palette colors may exceed 1.0.
    // 0 = clamp to baseline (nothing flares), 1 = full authored strength.
    glow: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0) var<uniform> camera: Camera;
