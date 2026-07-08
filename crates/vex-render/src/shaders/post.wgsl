// Post chain: threshold-less mip bloom + exposure soft-clip + optional CRT.
// Everything before this rendered linear HDR into an RGBA16F target; the
// palette's >1.0 "phosphor" values only become glow here.

struct FsIn {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// One oversized triangle covers the screen; uv (0,0) is top-left texel.
@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> FsIn {
    let xy = vec2<f32>(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: FsIn;
    out.pos = vec4<f32>(xy.x * 2.0 - 1.0, 1.0 - xy.y * 2.0, 0.0, 1.0);
    out.uv = xy;
    return out;
}

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

// 4 bilinear taps in a box — with the half-res target this averages a
// 4×4 source footprint. Good enough for glows on black.
@fragment
fn fs_downsample(in: FsIn) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(src));
    var color = textureSample(src, samp, in.uv + texel * vec2<f32>(-1.0, -1.0)).rgb;
    color += textureSample(src, samp, in.uv + texel * vec2<f32>(1.0, -1.0)).rgb;
    color += textureSample(src, samp, in.uv + texel * vec2<f32>(-1.0, 1.0)).rgb;
    color += textureSample(src, samp, in.uv + texel * vec2<f32>(1.0, 1.0)).rgb;
    return vec4<f32>(color * 0.25, 1.0);
}

// Tent upsample; the pipeline blends additively into the destination mip,
// accumulating blur radii on the way back up the chain.
@fragment
fn fs_upsample(in: FsIn) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(src));
    var color = textureSample(src, samp, in.uv + texel * vec2<f32>(-0.5, -0.5)).rgb;
    color += textureSample(src, samp, in.uv + texel * vec2<f32>(0.5, -0.5)).rgb;
    color += textureSample(src, samp, in.uv + texel * vec2<f32>(-0.5, 0.5)).rgb;
    color += textureSample(src, samp, in.uv + texel * vec2<f32>(0.5, 0.5)).rgb;
    return vec4<f32>(color * 0.25, 1.0);
}

struct PostParams {
    exposure: f32,
    bloom_strength: f32,
    // 0 = clean; 1 = full CRT package (barrel + chroma + vignette).
    crt: f32,
    // 1 when the target format is linear (WebGPU swapchains) and this
    // shader must apply the sRGB transfer itself; 0 when the format does.
    srgb_encode: f32,
    viewport: vec2<f32>,
    _pad: vec2<f32>,
};

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var bloom: texture_2d<f32>;
@group(0) @binding(2) var post_samp: sampler;
@group(0) @binding(3) var<uniform> params: PostParams;

@fragment
fn fs_composite(in: FsIn) -> @location(0) vec4<f32> {
    let aspect = params.viewport.x / max(params.viewport.y, 1.0);
    var uv = in.uv;
    var centered = (uv - 0.5) * vec2<f32>(aspect, 1.0);

    // Barrel distortion.
    if (params.crt > 0.0) {
        let r2 = dot(centered, centered);
        let warped = centered * (1.0 + 0.055 * params.crt * r2);
        uv = warped / vec2<f32>(aspect, 1.0) + 0.5;
        centered = warped;
    }
    // Anything warped off-screen is tube bezel.
    let inside = step(0.0, uv.x) * step(uv.x, 1.0) * step(0.0, uv.y) * step(uv.y, 1.0);

    // Chromatic aberration: phosphor triads misconverging toward the edges.
    // Clean mode (crt == 0, the default) takes one sample instead of three
    // — the offsets would be zero anyway, and this full-res pass is pure
    // bandwidth on small GPUs. The branch is uniform (params is a uniform
    // buffer), so sampling inside it satisfies WGSL uniformity rules.
    var color: vec3<f32>;
    if (params.crt > 0.0) {
        let chroma = centered * 0.0035 * params.crt;
        color = vec3<f32>(
            textureSample(scene, post_samp, uv + chroma).r,
            textureSample(scene, post_samp, uv).g,
            textureSample(scene, post_samp, uv - chroma).b,
        );
    } else {
        color = textureSample(scene, post_samp, uv).rgb;
    }

    color += textureSample(bloom, post_samp, uv).rgb * params.bloom_strength;

    // Exposure with a soft shoulder: saturated beams roll to white instead
    // of clipping hue.
    color = vec3<f32>(1.0) - exp(-color * params.exposure);

    // Vignette.
    color *= 1.0 - params.crt * 0.45 * smoothstep(0.45, 0.85, length(centered));

    // Linear targets (the browser swapchain) need manual gamma encoding —
    // without it, dim glow halos are crushed to black on display.
    if (params.srgb_encode > 0.5) {
        color = linear_to_srgb(color);
    }

    return vec4<f32>(color * inside, 1.0);
}
