struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

struct PixelAaParams {
    target_size: vec2<u32>,
    source_size: vec2<u32>,
    source_region: vec4<f32>,
};

@group(0) @binding(0)
var source_texture: texture_2d<f32>;

@group(0) @binding(1)
var<uniform> params: PixelAaParams;

const ALPHA_EPSILON: f32 = 1.0e-6;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var output: VertexOutput;
    let position = vec2<f32>(
        f32((vertex_index << 1u) & 2u),
        f32(vertex_index & 2u),
    );
    output.position = vec4<f32>(position * 2.0 - 1.0, 0.0, 1.0);
    return output;
}

// NOTE: These two sRGB transfer functions are duplicated from
// vendor/egui-wgpu/src/egui.wgsl and must stay identical to it.
fn linear_from_gamma_rgb(srgb: vec3<f32>) -> vec3<f32> {
    let cutoff = srgb < vec3<f32>(0.04045);
    let lower = srgb / vec3<f32>(12.92);
    let higher = pow((srgb + vec3<f32>(0.055)) / vec3<f32>(1.055), vec3<f32>(2.4));
    return select(higher, lower, cutoff);
}

fn gamma_from_linear_rgb(rgb: vec3<f32>) -> vec3<f32> {
    let cutoff = rgb < vec3<f32>(0.0031308);
    let lower = rgb * vec3<f32>(12.92);
    let higher = vec3<f32>(1.055) * pow(rgb, vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(higher, lower, cutoff);
}

// Keep this identical to `pixel_aa_axis_weight` in gpu_lanczos.rs. This is the
// sharpness-1.0 slopestep from libretro pixel_aa: the transition width is exactly
// one destination pixel. A slope above 1.0 is a possible future generalization.
fn pixel_aa_axis_weight(frac: f32, tx_per_px: f32) -> f32 {
    let lower_bound = 0.5 - 0.5 * tx_per_px;
    let upper_bound = 0.5 + 0.5 * tx_per_px;
    let width = upper_bound - lower_bound;
    if width < 1.0e-6 {
        if frac < 0.5 {
            return 0.0;
        }
        return 1.0;
    }
    return clamp((frac - lower_bound) / width, 0.0, 1.0);
}

fn load_linear_premultiplied(coord: vec2<i32>) -> vec4<f32> {
    let sample = textureLoad(source_texture, coord, 0);
    if sample.a <= 0.0 {
        return vec4<f32>(0.0);
    }
    let gamma_rgb = sample.rgb / max(sample.a, ALPHA_EPSILON);
    return vec4<f32>(linear_from_gamma_rgb(gamma_rgb) * sample.a, sample.a);
}

@fragment
fn fs_pixel_aa(input: VertexOutput) -> @location(0) vec4<f32> {
    let target_coord = vec2<u32>(input.position.xy);
    let scale = vec2<f32>(params.target_size) / params.source_region.zw;
    let tx_per_px = vec2<f32>(1.0) / scale;
    let center =
        params.source_region.xy + (vec2<f32>(target_coord) + vec2<f32>(0.5)) / scale;
    let coord = center - vec2<f32>(0.5);
    let base = vec2<i32>(floor(coord));
    let fraction = coord - floor(coord);
    let weight = vec2<f32>(
        pixel_aa_axis_weight(fraction.x, tx_per_px.x),
        pixel_aa_axis_weight(fraction.y, tx_per_px.y),
    );

    let maximum = vec2<i32>(params.source_size) - vec2<i32>(1);
    let p00 = load_linear_premultiplied(clamp(base, vec2<i32>(0), maximum));
    let p10 = load_linear_premultiplied(clamp(base + vec2<i32>(1, 0), vec2<i32>(0), maximum));
    let p01 = load_linear_premultiplied(clamp(base + vec2<i32>(0, 1), vec2<i32>(0), maximum));
    let p11 = load_linear_premultiplied(clamp(base + vec2<i32>(1, 1), vec2<i32>(0), maximum));

    let top = mix(p00, p10, weight.x);
    let bottom = mix(p01, p11, weight.x);
    let mixed = mix(top, bottom, weight.y);
    if mixed.a <= 0.0 {
        return vec4<f32>(0.0);
    }

    let linear_rgb = mixed.rgb / max(mixed.a, ALPHA_EPSILON);
    var rgb = gamma_from_linear_rgb(linear_rgb) * mixed.a;
    rgb = min(rgb, vec3<f32>(mixed.a));
    return vec4<f32>(rgb, mixed.a);
}
