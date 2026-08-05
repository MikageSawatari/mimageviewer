struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

struct ResampleRegionParams {
    target_len: u32,
    source_start: f32,
    source_len: f32,
    cross_start: u32,
};

@group(0) @binding(0)
var source_texture: texture_2d<f32>;

@group(0) @binding(1)
var<uniform> params: ResampleRegionParams;

const PI: f32 = 3.14159265358979323846;
const LANCZOS_SUPPORT: f32 = 3.0;

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

// NOTE: `sinc` / `lanczos3` are duplicated from gpu_lanczos_spike.wgsl and must stay
// identical to it. They are kept separate because this pass takes a source-region
// origin and has no blur_factor, and the downscale shader is a shipped, verified
// path that a shared-uniform refactor would have to disturb.
fn sinc(x: f32) -> f32 {
    if abs(x) < 1.0e-6 {
        return 1.0;
    }
    let pix = PI * x;
    return sin(pix) / pix;
}

fn lanczos3(x: f32) -> f32 {
    if abs(x) >= LANCZOS_SUPPORT {
        return 0.0;
    }
    return sinc(x) * sinc(x / LANCZOS_SUPPORT);
}

fn sample_bounds(center: f32, source_len: u32) -> vec2<i32> {
    let start = i32(floor(center - 0.5 - LANCZOS_SUPPORT)) + 1;
    let end = i32(ceil(center - 0.5 + LANCZOS_SUPPORT));
    return vec2<i32>(
        clamp(start, 0, i32(source_len)),
        clamp(end, 0, i32(source_len)),
    );
}

fn source_center(target_index: u32) -> f32 {
    let scale = f32(params.target_len) / params.source_len;
    return params.source_start + (f32(target_index) + 0.5) / scale;
}

fn resample_vertical(target_coord: vec2<u32>) -> vec4<f32> {
    let source_size = textureDimensions(source_texture, 0);
    let center = source_center(target_coord.y);
    let bounds = sample_bounds(center, source_size.y);

    var color_sum = vec4<f32>(0.0);
    var weight_sum = 0.0;
    for (var source_y = bounds.x; source_y < bounds.y; source_y++) {
        let weight = lanczos3(f32(source_y) + 0.5 - center);
        color_sum += textureLoad(
            source_texture,
            vec2<i32>(i32(params.cross_start + target_coord.x), source_y),
            0,
        ) * weight;
        weight_sum += weight;
    }

    if abs(weight_sum) < 1.0e-8 {
        let nearest = clamp(i32(floor(center)), 0, i32(source_size.y) - 1);
        return textureLoad(
            source_texture,
            vec2<i32>(i32(params.cross_start + target_coord.x), nearest),
            0,
        );
    }
    return color_sum / weight_sum;
}

fn resample_horizontal(target_coord: vec2<u32>) -> vec4<f32> {
    let source_size = textureDimensions(source_texture, 0);
    let center = source_center(target_coord.x);
    let bounds = sample_bounds(center, source_size.x);

    var color_sum = vec4<f32>(0.0);
    var weight_sum = 0.0;
    for (var source_x = bounds.x; source_x < bounds.y; source_x++) {
        let weight = lanczos3(f32(source_x) + 0.5 - center);
        color_sum += textureLoad(
            source_texture,
            vec2<i32>(source_x, i32(target_coord.y)),
            0,
        ) * weight;
        weight_sum += weight;
    }

    if abs(weight_sum) < 1.0e-8 {
        let nearest = clamp(i32(floor(center)), 0, i32(source_size.x) - 1);
        return textureLoad(
            source_texture,
            vec2<i32>(nearest, i32(target_coord.y)),
            0,
        );
    }
    return color_sum / weight_sum;
}

@fragment
fn fs_vertical(input: VertexOutput) -> @location(0) vec4<f32> {
    return resample_vertical(vec2<u32>(input.position.xy));
}

@fragment
fn fs_horizontal(input: VertexOutput) -> @location(0) vec4<f32> {
    return resample_horizontal(vec2<u32>(input.position.xy));
}
