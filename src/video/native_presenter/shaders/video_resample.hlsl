Texture2D<float4> source_tex : register(t0);

cbuffer ResampleConstants : register(b0) {
    float4 source_target;  // raw source width/height, final target width/height
    float4 axis_filter;    // oriented source axis lengths, horizontal/vertical stretch
    float4 source_region;  // oriented source origin x/y and extent x/y
    float4 inverse_axes;   // d(raw xy)/d(oriented x), d(raw xy)/d(oriented y)
    float4 inverse_offset; // raw xy at oriented (0,0), unused
};

struct VsOut {
    float4 position : SV_Position;
};

VsOut vs_main(uint vertex_id : SV_VertexID) {
    VsOut output;
    float2 uv = float2((vertex_id << 1) & 2, vertex_id & 2);
    output.position = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    return output;
}

float sinc_pi(float value) {
    float x = value * 3.14159265358979323846;
    return abs(x) < 1.0e-5 ? 1.0 : sin(x) / x;
}

float lanczos3_weight(float distance, float stretch) {
    float x = abs(distance) / stretch;
    if (x >= 3.0) {
        return 0.0;
    }
    return sinc_pi(x) * sinc_pi(x / 3.0);
}

float4 ps_horizontal(VsOut input) : SV_Target {
    float source_axis_x = axis_filter.x;
    float source_axis_y = axis_filter.y;
    float target_width = source_target.z;
    float source_position = source_region.x
        + input.position.x * source_region.z / target_width - 0.5;
    if (source_position < -0.5 || source_position >= source_axis_x - 0.5) {
        return float4(0.0, 0.0, 0.0, 1.0);
    }
    float oriented_y = clamp(input.position.y - 0.5, 0.0, source_axis_y - 1.0);
    float stretch = axis_filter.z;
    int radius = (int)ceil(3.0 * stretch);
    int first = (int)floor(source_position) - radius;
    int last = (int)floor(source_position) + radius + 1;
    int2 raw_max = int2(source_target.xy) - 1;
    float4 accumulated = 0.0;
    float weight_sum = 0.0;
    [loop]
    for (int sample_index = first; sample_index <= last; ++sample_index) {
        float weight = lanczos3_weight(source_position - sample_index, stretch);
        float2 raw_position =
            (float)sample_index * inverse_axes.xy
            + oriented_y * inverse_axes.zw
            + inverse_offset.xy;
        int2 raw_coord = clamp(int2(round(raw_position)), int2(0, 0), raw_max);
        accumulated += source_tex.Load(int3(raw_coord, 0)) * weight;
        weight_sum += weight;
    }
    return abs(weight_sum) > 1.0e-6 ? accumulated / weight_sum : accumulated;
}

float4 ps_vertical(VsOut input) : SV_Target {
    float source_axis_y = axis_filter.y;
    float target_height = source_target.w;
    float source_position = source_region.y
        + input.position.y * source_region.w / target_height - 0.5;
    if (source_position < -0.5 || source_position >= source_axis_y - 0.5) {
        return float4(0.0, 0.0, 0.0, 1.0);
    }
    float stretch = axis_filter.w;
    int radius = (int)ceil(3.0 * stretch);
    int first = (int)floor(source_position) - radius;
    int last = (int)floor(source_position) + radius + 1;
    int source_x = clamp((int)input.position.x, 0, (int)source_target.z - 1);
    int source_y_max = (int)source_axis_y - 1;
    float4 accumulated = 0.0;
    float weight_sum = 0.0;
    [loop]
    for (int sample_index = first; sample_index <= last; ++sample_index) {
        float weight = lanczos3_weight(source_position - sample_index, stretch);
        int source_y = clamp(sample_index, 0, source_y_max);
        accumulated += source_tex.Load(int3(source_x, source_y, 0)) * weight;
        weight_sum += weight;
    }
    return abs(weight_sum) > 1.0e-6 ? accumulated / weight_sum : accumulated;
}

float4 ps_nearest(VsOut input) : SV_Target {
    float2 oriented_position = source_region.xy
        + input.position.xy * source_region.zw / source_target.zw - 0.5;
    if (any(oriented_position < float2(-0.5, -0.5))
        || any(oriented_position >= axis_filter.xy - float2(0.5, 0.5))) {
        return float4(0.0, 0.0, 0.0, 1.0);
    }
    float2 raw_position =
        round(oriented_position.x) * inverse_axes.xy
        + round(oriented_position.y) * inverse_axes.zw
        + inverse_offset.xy;
    int2 raw_max = int2(source_target.xy) - 1;
    int2 raw_coord = clamp(int2(round(raw_position)), int2(0, 0), raw_max);
    return source_tex.Load(int3(raw_coord, 0));
}
