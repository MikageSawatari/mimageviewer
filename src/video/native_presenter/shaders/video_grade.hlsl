Texture2D<float4> source_tex : register(t0);
Texture3D<float4> lut_tex : register(t1);
SamplerState linear_sampler : register(s0);

cbuffer GradeConstants : register(b0) {
    float4 tone0;       // brightness, contrast, gamma, saturation
    float4 tone1;       // temperature, black point, white point, midtone
    float4 domain_min;  // rgb, unused
    float4 domain_inv;  // rgb, LUT strength
    float4 lut_coord;   // scale, offset, enabled, unused
};

struct VsOut {
    float4 position : SV_Position;
    float2 uv : TEXCOORD0;
};

VsOut vs_main(uint vertex_id : SV_VertexID) {
    VsOut output;
    float2 uv = float2((vertex_id << 1) & 2, vertex_id & 2);
    output.position = float4(uv * float2(2.0, -2.0) + float2(-1.0, 1.0), 0.0, 1.0);
    output.uv = uv;
    return output;
}

float4 ps_main(VsOut input) : SV_Target {
    float4 sampled = source_tex.SampleLevel(linear_sampler, input.uv, 0.0);
    float3 color = sampled.rgb;

    float range = max(tone1.z - tone1.y, 1.0 / 255.0);
    color = pow(saturate((color - tone1.y) / range), 1.0 / max(tone1.w, 0.1));
    color = pow(saturate(color), 1.0 / max(tone0.z, 0.2));

    float contrast_factor =
        (259.0 * (tone0.y + 255.0)) / (255.0 * (259.0 - tone0.y));
    color = (contrast_factor * (color * 255.0 - 128.0)
        + 128.0 + tone0.x * 2.55) / 255.0;

    if (tone0.w <= -99.999) {
        color = dot(color, float3(0.299, 0.587, 0.114)).xxx;
    } else {
        float lum = (max(color.r, max(color.g, color.b))
            + min(color.r, min(color.g, color.b))) * 0.5;
        color = lum.xxx + (color - lum.xxx) * (1.0 + tone0.w / 100.0);
    }
    color.r += tone1.x * 0.5 / 255.0;
    color.b -= tone1.x * 0.5 / 255.0;
    color = saturate(color);

    if (lut_coord.z > 0.5) {
        float3 normalized = saturate((color - domain_min.rgb) * domain_inv.rgb);
        float3 coord = normalized * lut_coord.x + lut_coord.y;
        float3 graded = lut_tex.SampleLevel(linear_sampler, coord, 0.0).rgb;
        color = lerp(color, graded, domain_inv.w);
    }
    return float4(saturate(color), sampled.a);
}
