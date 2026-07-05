// Line segments expanded to screen-space quads with round caps and analytic
// anti-aliasing. One instance = one segment. The quad is shaded as a 2D
// capsule SDF in pixel units, so stroke width is resolution-independent and
// constant with distance — like a vector monitor beam.
//
// The shared `Camera` block (camera.wgsl) is prepended at pipeline creation.

struct Instance {
    @location(0) pos_a: vec3<f32>,
    @location(1) pos_b: vec3<f32>,
    @location(2) color: vec4<f32>,
    // x = dash period in world units (0 = solid), y = flicker amount.
    @location(3) style: vec2<f32>,
};

struct VsOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec3<f32>,
    // Segment-frame position in pixels. Interpolation must be linear in
    // screen space (not perspective-correct): these are 2D quantities.
    @location(1) @interpolate(linear) seg_pos: vec2<f32>,
    @location(2) @interpolate(linear) seg_len: f32,
    // World position for depth cueing (perspective-correct on purpose).
    @location(3) world_pos: vec3<f32>,
    // x = parameter along the segment (perspective-correct, so dashes are
    // spaced in world units), y = dashes per segment (0 = solid).
    @location(4) dash: vec2<f32>,
    // Flicker factor, computed per instance in the vertex stage.
    @location(5) @interpolate(flat) brightness: f32,
};

// Clip-space w below this is "behind the eye".
const NEAR_EPS: f32 = 1e-4;

@vertex
fn vs_main(
    @builtin(vertex_index) vi: u32,
    @builtin(instance_index) ii: u32,
    inst: Instance,
) -> VsOut {
    // corner.x: 0 = cap-A end, 1 = cap-B end. corner.y: across the stroke.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),  vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vi];

    var ca = camera.view_proj * vec4<f32>(inst.pos_a, 1.0);
    var cb = camera.view_proj * vec4<f32>(inst.pos_b, 1.0);

    var out: VsOut;
    out.world_pos = mix(inst.pos_a, inst.pos_b, corner.x);
    // Dashes per segment, from the world-space length and dash period.
    var dashes = 0.0;
    if (inst.style.x > 0.0) {
        dashes = distance(inst.pos_a, inst.pos_b) / max(inst.style.x, 1e-4);
    }
    out.dash = vec2<f32>(corner.x, dashes);
    // Flicker: per-instance phase so a wall of lights doesn't blink in sync.
    out.brightness =
        1.0 - inst.style.y * (0.5 + 0.5 * sin(camera.time * 13.0 + f32(ii) * 7.31));
    if (ca.w < NEAR_EPS && cb.w < NEAR_EPS) {
        // Entirely behind the eye: emit a vertex outside the clip volume.
        out.clip_pos = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        out.color = vec3<f32>(0.0);
        out.seg_pos = vec2<f32>(0.0);
        out.seg_len = 0.0;
        return out;
    }
    // Clip against w = NEAR_EPS so both endpoints project to finite points.
    if (ca.w < NEAR_EPS) {
        ca = mix(ca, cb, (NEAR_EPS - ca.w) / (cb.w - ca.w));
    } else if (cb.w < NEAR_EPS) {
        cb = mix(cb, ca, (NEAR_EPS - cb.w) / (ca.w - cb.w));
    }

    let half_vp = 0.5 * camera.viewport;
    let sa = ca.xy / ca.w * half_vp;
    let sb = cb.xy / cb.w * half_vp;

    let seg = sb - sa;
    let len = length(seg);
    var dir = vec2<f32>(1.0, 0.0);
    if (len > 1e-6) {
        dir = seg / len;
    }
    let norm = vec2<f32>(-dir.y, dir.x);

    // Reach past the stroke on every side: half a width for the round cap
    // plus a 1px apron for the AA falloff.
    let reach = 0.5 * camera.line_width + 1.0;
    let along = mix(-reach, len + reach, corner.x);
    let across = corner.y * reach;
    let screen_pos = sa + dir * along + norm * across;

    let w = mix(ca.w, cb.w, corner.x);
    let z = mix(ca.z, cb.z, corner.x);
    out.clip_pos = vec4<f32>(screen_pos / half_vp * w, z, w);
    // The engine's glow dial compresses overbright (HDR) colors while
    // preserving hue: assets author relative strengths, the scene decides
    // how hard anything is allowed to flare.
    var color = inst.color.rgb * inst.color.a;
    let peak = max(color.r, max(color.g, color.b));
    if (peak > 1.0) {
        color *= (1.0 + (peak - 1.0) * camera.glow) / peak;
    }
    out.color = color;
    out.seg_pos = vec2<f32>(along, across);
    out.seg_len = len;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Capsule SDF: distance to the segment [0, seg_len] along x.
    let nearest = clamp(in.seg_pos.x, 0.0, in.seg_len);
    let dist = length(vec2<f32>(in.seg_pos.x - nearest, in.seg_pos.y));
    // Approximate pixel coverage with a 1px linear falloff at the edge.
    let coverage = clamp(0.5 * camera.line_width - dist + 0.5, 0.0, 1.0);
    // Depth cueing: dim with distance from the eye, like a phosphor beam
    // losing energy. Computed per fragment so long segments fade correctly
    // along their length, not just at their endpoints.
    let fog = exp(-camera.fog_density * distance(in.world_pos, camera.eye));
    // World-unit dashes: `dash.x` is perspective-correct along the segment.
    var dash_on = 1.0;
    if (in.dash.y > 0.0) {
        dash_on = step(fract(in.dash.x * in.dash.y), 0.62);
    }
    let level = coverage * fog * in.brightness * dash_on;
    // Premultiplied for the additive target: beams sum.
    return vec4<f32>(in.color * level, level);
}
