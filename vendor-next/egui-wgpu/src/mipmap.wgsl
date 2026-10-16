struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

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

@group(0) @binding(0)
var source_texture: texture_2d<f32>;

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let source_size_u = textureDimensions(source_texture, 0);
    let target_size_u = max(source_size_u / 2u, vec2<u32>(1u));
    let target_coord_u = vec2<u32>(input.position.xy);

    let source_size = vec2<f32>(source_size_u);
    let target_size = vec2<f32>(target_size_u);
    let area_start = vec2<f32>(target_coord_u) * source_size / target_size;
    let area_end = vec2<f32>(target_coord_u + 1u) * source_size / target_size;
    let first_texel = vec2<u32>(floor(area_start));

    var color_sum = vec4<f32>(0.0);
    var weight_sum = 0.0;
    for (var y = 0u; y < 3u; y++) {
        for (var x = 0u; x < 3u; x++) {
            let texel = first_texel + vec2<u32>(x, y);
            if (all(texel < source_size_u)) {
                let texel_start = vec2<f32>(texel);
                let texel_end = texel_start + 1.0;
                let overlap = max(vec2<f32>(0.0), min(area_end, texel_end) - max(area_start, texel_start));
                let weight = overlap.x * overlap.y;
                color_sum += textureLoad(source_texture, vec2<i32>(texel), 0) * weight;
                weight_sum += weight;
            }
        }
    }

    return color_sum / weight_sum;
}
