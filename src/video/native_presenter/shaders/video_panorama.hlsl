Texture2D<float4> source_tex : register(t0);
SamplerState repeat_clamp_sampler : register(s0);
SamplerState clamp_sampler : register(s1);

cbuffer PanoramaConstants : register(b0) {
    float4 pose; // yaw, pitch, fov_y, viewport aspect
    float4 crop; // sphere UV offset.xy and scale.zw
    float4 proj; // projection mode, CPU-computed coefficient k, reserved
};

cbuffer OrientationConstants : register(b1) {
    float4 source_oriented; // raw width/height, oriented width/height
    float4 inverse_axes; // d(raw xy)/d(oriented x), d(raw xy)/d(oriented y)
    float4 inverse_offset; // raw xy at oriented (0,0), unused
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

static const float PI = 3.141592653589793;
static const float INV_TWO_PI = 0.15915494309189535;
static const float INV_PI = 0.3183098861837907;

// These codes are a 1:1 mirror of PanoProjection::shader_code().
static const uint PROJ_PERSPECTIVE = 0;
static const uint PROJ_STEREOGRAPHIC = 1;
static const uint PROJ_EQUIDISTANT = 2;
static const uint PROJ_EQUISOLID = 3;

struct ProjTheta {
    float theta;
    bool valid;
};

// Keep this branch structure in lockstep with panorama_wgpu::projection_theta.
ProjTheta projection_theta(uint mode, float k, float r) {
    float arg = r * k;
    ProjTheta output;
    if (mode == PROJ_STEREOGRAPHIC) {
        output.theta = 2.0 * atan(arg);
        output.valid = true;
    } else if (mode == PROJ_EQUIDISTANT) {
        output.theta = arg;
        output.valid = arg <= PI;
    } else if (mode == PROJ_EQUISOLID) {
        output.theta = 2.0 * asin(clamp(arg, -1.0, 1.0));
        output.valid = arg <= 1.0;
    } else {
        output.theta = atan(arg);
        output.valid = true;
    }
    return output;
}

float4 ps_orient(VsOut input) : SV_Target {
    float2 oriented_position = input.position.xy - 0.5;
    float2 raw_position =
        round(oriented_position.x) * inverse_axes.xy
        + round(oriented_position.y) * inverse_axes.zw
        + inverse_offset.xy;
    int2 raw_max = int2(source_oriented.xy) - 1;
    int2 raw_coord = clamp(int2(round(raw_position)), int2(0, 0), raw_max);
    return source_tex.Load(int3(raw_coord, 0));
}

float4 ps_main(VsOut input) : SV_Target {
    float yaw = pose.x;
    float pitch = pose.y;
    float aspect = pose.w;
    uint proj_mode = (uint)(proj.x + 0.5);
    float proj_k = proj.y;

    float2 ndc = float2(input.uv.x * 2.0 - 1.0, 1.0 - input.uv.y * 2.0);
    float2 plane = float2(ndc.x * aspect, ndc.y);
    float radius = length(plane);
    ProjTheta projected = projection_theta(proj_mode, proj_k, radius);
    float sin_theta = sin(projected.theta);
    float2 dir_xy = radius > 1.0e-6
        ? plane / max(radius, 1.0e-6) * sin_theta
        : float2(0.0, 0.0);
    float3 cam_dir = float3(dir_xy.x, dir_xy.y, -cos(projected.theta));

    float cp = cos(pitch);
    float sp = sin(pitch);
    float3 p1 = float3(
        cam_dir.x,
        cp * cam_dir.y - sp * cam_dir.z,
        sp * cam_dir.y + cp * cam_dir.z
    );
    float cy = cos(yaw);
    float sy = sin(yaw);
    float3 world_dir = float3(
        cy * p1.x + sy * p1.z,
        p1.y,
        -sy * p1.x + cy * p1.z
    );

    float lon = atan2(world_dir.x, -world_dir.z);
    float lat = asin(clamp(world_dir.y, -1.0, 1.0));
    float2 sphere_uv = float2(lon * INV_TWO_PI + 0.5, 0.5 - lat * INV_PI);

    // Undo the U=1 -> U=0 branch cut before supplying explicit gradients. Both
    // derivatives are evaluated in uniform control flow, including pixels that
    // are outside a wide fisheye projection's valid image circle.
    float2 sphere_dx_raw = ddx(sphere_uv);
    float2 sphere_dy_raw = ddy(sphere_uv);
    float2 sphere_dx = float2(
        sphere_dx_raw.x - round(sphere_dx_raw.x),
        sphere_dx_raw.y
    );
    float2 sphere_dy = float2(
        sphere_dy_raw.x - round(sphere_dy_raw.x),
        sphere_dy_raw.y
    );
    float2 texture_dx = sphere_dx / crop.zw;
    float2 texture_dy = sphere_dy / crop.zw;
    float2 texture_uv_raw = (sphere_uv - crop.xy) / crop.zw;

    bool u_crop = (crop.z < 0.999) || (abs(crop.x) > 0.001);
    bool v_crop = (crop.w < 0.999) || (abs(crop.y) > 0.001);
    uint source_width;
    uint source_height;
    source_tex.GetDimensions(source_width, source_height);
    float2 dimensions = float2(source_width, source_height);
    float2 half_texel = 0.5 / dimensions;
    float2 max_uv = float2(1.0, 1.0) - half_texel;
    float2 texture_uv = float2(
        u_crop
            ? clamp(texture_uv_raw.x, half_texel.x, max_uv.x)
            : texture_uv_raw.x,
        v_crop
            ? clamp(texture_uv_raw.y, half_texel.y, max_uv.y)
            : texture_uv_raw.y
    );

    float4 sampled;
    if (u_crop) {
        sampled = source_tex.SampleGrad(clamp_sampler, texture_uv, texture_dx, texture_dy);
    } else {
        sampled = source_tex.SampleGrad(
            repeat_clamp_sampler,
            texture_uv,
            texture_dx,
            texture_dy
        );
    }

    // A single exit keeps ddx/ddy above in uniform control flow.
    return projected.valid ? sampled : float4(0.0, 0.0, 0.0, 1.0);
}
