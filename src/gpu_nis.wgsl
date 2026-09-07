// The MIT License(MIT)
//
// Copyright(c) 2022 NVIDIA CORPORATION & AFFILIATES. All rights reserved.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy of
// this software and associated documentation files(the "Software"), to deal in
// the Software without restriction, including without limitation the rights to
// use, copy, modify, merge, publish, distribute, sublicense, and / or sell copies of
// the Software, and to permit persons to whom the Software is furnished to do so,
// subject to the following conditions :
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS
// FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.IN NO EVENT SHALL THE AUTHORS OR
// COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER
// IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
// CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.

// Fragment-shader port of NVScaler from NVIDIA Image Scaling SDK v1.0.3.
// The official compute shader's workgroup tile and shared-memory staging are performance
// optimizations, not algorithm steps. This port evaluates the same 6x6 support, four
// directional filters, and adaptive sharpening independently for each output pixel.
// Coefficients below are copied from NIS_Config.h. SDR constants use sharpness 0.25,
// matching the shader used for mImageViewer's algorithm comparison.

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

struct NisParams {
    target_size: vec2<u32>,
    source_size: vec2<u32>,
    source_origin: vec2<f32>,
    source_extent: vec2<f32>,
    inverse_x: vec2<f32>,
    inverse_y: vec2<f32>,
    inverse_offset: vec2<f32>,
};

@group(0) @binding(0)
var source_texture: texture_2d<f32>;

@group(0) @binding(1)
var<uniform> params: NisParams;

const PHASE_COUNT: u32 = 64u;
const DETECT_RATIO: f32 = 2.201171875;
const DETECT_THRESHOLD: f32 = 0.0625;
const MIN_CONTRAST_RATIO: f32 = 2.0;
const RATIO_NORM: f32 = 0.125;
const CONTRAST_BOOST: f32 = 1.0;
const LTI_EPSILON: f32 = 0.00392156862745098;
const SHARP_START_Y: f32 = 0.45;
const SHARP_SCALE_Y: f32 = 2.2222222222222223;
const SHARP_STRENGTH_MIN: f32 = 0.1;
const SHARP_STRENGTH_SCALE: f32 = 0.7125;
const SHARP_LIMIT_MIN: f32 = 0.1;
const SHARP_LIMIT_SCALE: f32 = 0.25;

const COEF_SCALE: array<vec4<f32>, 128> = array<vec4<f32>, 128>(
    vec4<f32>(0.0, 0.0, 1.0000, 0.0),
    vec4<f32>(0.0, 0.0, 0.0, 0.0),
    vec4<f32>(0.0029, -0.0127, 1.0000, 0.0132),
    vec4<f32>(-0.0034, 0.0, 0.0, 0.0),
    vec4<f32>(0.0063, -0.0249, 0.9985, 0.0269),
    vec4<f32>(-0.0068, 0.0, 0.0, 0.0),
    vec4<f32>(0.0088, -0.0361, 0.9956, 0.0415),
    vec4<f32>(-0.0103, 0.0005, 0.0, 0.0),
    vec4<f32>(0.0117, -0.0474, 0.9932, 0.0562),
    vec4<f32>(-0.0142, 0.0005, 0.0, 0.0),
    vec4<f32>(0.0142, -0.0576, 0.9897, 0.0713),
    vec4<f32>(-0.0181, 0.0005, 0.0, 0.0),
    vec4<f32>(0.0166, -0.0674, 0.9844, 0.0874),
    vec4<f32>(-0.0220, 0.0010, 0.0, 0.0),
    vec4<f32>(0.0186, -0.0762, 0.9785, 0.1040),
    vec4<f32>(-0.0264, 0.0015, 0.0, 0.0),
    vec4<f32>(0.0205, -0.0850, 0.9727, 0.1206),
    vec4<f32>(-0.0308, 0.0020, 0.0, 0.0),
    vec4<f32>(0.0225, -0.0928, 0.9648, 0.1382),
    vec4<f32>(-0.0352, 0.0024, 0.0, 0.0),
    vec4<f32>(0.0239, -0.1006, 0.9575, 0.1558),
    vec4<f32>(-0.0396, 0.0029, 0.0, 0.0),
    vec4<f32>(0.0254, -0.1074, 0.9487, 0.1738),
    vec4<f32>(-0.0439, 0.0034, 0.0, 0.0),
    vec4<f32>(0.0264, -0.1138, 0.9390, 0.1929),
    vec4<f32>(-0.0488, 0.0044, 0.0, 0.0),
    vec4<f32>(0.0278, -0.1191, 0.9282, 0.2119),
    vec4<f32>(-0.0537, 0.0049, 0.0, 0.0),
    vec4<f32>(0.0288, -0.1245, 0.9170, 0.2310),
    vec4<f32>(-0.0581, 0.0059, 0.0, 0.0),
    vec4<f32>(0.0293, -0.1294, 0.9058, 0.2510),
    vec4<f32>(-0.0630, 0.0063, 0.0, 0.0),
    vec4<f32>(0.0303, -0.1333, 0.8926, 0.2710),
    vec4<f32>(-0.0679, 0.0073, 0.0, 0.0),
    vec4<f32>(0.0308, -0.1367, 0.8789, 0.2915),
    vec4<f32>(-0.0728, 0.0083, 0.0, 0.0),
    vec4<f32>(0.0308, -0.1401, 0.8657, 0.3120),
    vec4<f32>(-0.0776, 0.0093, 0.0, 0.0),
    vec4<f32>(0.0313, -0.1426, 0.8506, 0.3330),
    vec4<f32>(-0.0825, 0.0103, 0.0, 0.0),
    vec4<f32>(0.0313, -0.1445, 0.8354, 0.3540),
    vec4<f32>(-0.0874, 0.0112, 0.0, 0.0),
    vec4<f32>(0.0313, -0.1460, 0.8193, 0.3755),
    vec4<f32>(-0.0923, 0.0122, 0.0, 0.0),
    vec4<f32>(0.0313, -0.1470, 0.8022, 0.3965),
    vec4<f32>(-0.0967, 0.0137, 0.0, 0.0),
    vec4<f32>(0.0308, -0.1479, 0.7856, 0.4185),
    vec4<f32>(-0.1016, 0.0146, 0.0, 0.0),
    vec4<f32>(0.0303, -0.1479, 0.7681, 0.4399),
    vec4<f32>(-0.1060, 0.0156, 0.0, 0.0),
    vec4<f32>(0.0298, -0.1479, 0.7505, 0.4614),
    vec4<f32>(-0.1104, 0.0166, 0.0, 0.0),
    vec4<f32>(0.0293, -0.1470, 0.7314, 0.4829),
    vec4<f32>(-0.1147, 0.0181, 0.0, 0.0),
    vec4<f32>(0.0288, -0.1460, 0.7119, 0.5049),
    vec4<f32>(-0.1187, 0.0190, 0.0, 0.0),
    vec4<f32>(0.0278, -0.1445, 0.6929, 0.5264),
    vec4<f32>(-0.1226, 0.0200, 0.0, 0.0),
    vec4<f32>(0.0273, -0.1431, 0.6724, 0.5479),
    vec4<f32>(-0.1260, 0.0215, 0.0, 0.0),
    vec4<f32>(0.0264, -0.1411, 0.6528, 0.5693),
    vec4<f32>(-0.1299, 0.0225, 0.0, 0.0),
    vec4<f32>(0.0254, -0.1387, 0.6323, 0.5903),
    vec4<f32>(-0.1328, 0.0234, 0.0, 0.0),
    vec4<f32>(0.0244, -0.1357, 0.6113, 0.6113),
    vec4<f32>(-0.1357, 0.0244, 0.0, 0.0),
    vec4<f32>(0.0234, -0.1328, 0.5903, 0.6323),
    vec4<f32>(-0.1387, 0.0254, 0.0, 0.0),
    vec4<f32>(0.0225, -0.1299, 0.5693, 0.6528),
    vec4<f32>(-0.1411, 0.0264, 0.0, 0.0),
    vec4<f32>(0.0215, -0.1260, 0.5479, 0.6724),
    vec4<f32>(-0.1431, 0.0273, 0.0, 0.0),
    vec4<f32>(0.0200, -0.1226, 0.5264, 0.6929),
    vec4<f32>(-0.1445, 0.0278, 0.0, 0.0),
    vec4<f32>(0.0190, -0.1187, 0.5049, 0.7119),
    vec4<f32>(-0.1460, 0.0288, 0.0, 0.0),
    vec4<f32>(0.0181, -0.1147, 0.4829, 0.7314),
    vec4<f32>(-0.1470, 0.0293, 0.0, 0.0),
    vec4<f32>(0.0166, -0.1104, 0.4614, 0.7505),
    vec4<f32>(-0.1479, 0.0298, 0.0, 0.0),
    vec4<f32>(0.0156, -0.1060, 0.4399, 0.7681),
    vec4<f32>(-0.1479, 0.0303, 0.0, 0.0),
    vec4<f32>(0.0146, -0.1016, 0.4185, 0.7856),
    vec4<f32>(-0.1479, 0.0308, 0.0, 0.0),
    vec4<f32>(0.0137, -0.0967, 0.3965, 0.8022),
    vec4<f32>(-0.1470, 0.0313, 0.0, 0.0),
    vec4<f32>(0.0122, -0.0923, 0.3755, 0.8193),
    vec4<f32>(-0.1460, 0.0313, 0.0, 0.0),
    vec4<f32>(0.0112, -0.0874, 0.3540, 0.8354),
    vec4<f32>(-0.1445, 0.0313, 0.0, 0.0),
    vec4<f32>(0.0103, -0.0825, 0.3330, 0.8506),
    vec4<f32>(-0.1426, 0.0313, 0.0, 0.0),
    vec4<f32>(0.0093, -0.0776, 0.3120, 0.8657),
    vec4<f32>(-0.1401, 0.0308, 0.0, 0.0),
    vec4<f32>(0.0083, -0.0728, 0.2915, 0.8789),
    vec4<f32>(-0.1367, 0.0308, 0.0, 0.0),
    vec4<f32>(0.0073, -0.0679, 0.2710, 0.8926),
    vec4<f32>(-0.1333, 0.0303, 0.0, 0.0),
    vec4<f32>(0.0063, -0.0630, 0.2510, 0.9058),
    vec4<f32>(-0.1294, 0.0293, 0.0, 0.0),
    vec4<f32>(0.0059, -0.0581, 0.2310, 0.9170),
    vec4<f32>(-0.1245, 0.0288, 0.0, 0.0),
    vec4<f32>(0.0049, -0.0537, 0.2119, 0.9282),
    vec4<f32>(-0.1191, 0.0278, 0.0, 0.0),
    vec4<f32>(0.0044, -0.0488, 0.1929, 0.9390),
    vec4<f32>(-0.1138, 0.0264, 0.0, 0.0),
    vec4<f32>(0.0034, -0.0439, 0.1738, 0.9487),
    vec4<f32>(-0.1074, 0.0254, 0.0, 0.0),
    vec4<f32>(0.0029, -0.0396, 0.1558, 0.9575),
    vec4<f32>(-0.1006, 0.0239, 0.0, 0.0),
    vec4<f32>(0.0024, -0.0352, 0.1382, 0.9648),
    vec4<f32>(-0.0928, 0.0225, 0.0, 0.0),
    vec4<f32>(0.0020, -0.0308, 0.1206, 0.9727),
    vec4<f32>(-0.0850, 0.0205, 0.0, 0.0),
    vec4<f32>(0.0015, -0.0264, 0.1040, 0.9785),
    vec4<f32>(-0.0762, 0.0186, 0.0, 0.0),
    vec4<f32>(0.0010, -0.0220, 0.0874, 0.9844),
    vec4<f32>(-0.0674, 0.0166, 0.0, 0.0),
    vec4<f32>(0.0005, -0.0181, 0.0713, 0.9897),
    vec4<f32>(-0.0576, 0.0142, 0.0, 0.0),
    vec4<f32>(0.0005, -0.0142, 0.0562, 0.9932),
    vec4<f32>(-0.0474, 0.0117, 0.0, 0.0),
    vec4<f32>(0.0005, -0.0103, 0.0415, 0.9956),
    vec4<f32>(-0.0361, 0.0088, 0.0, 0.0),
    vec4<f32>(0.0, -0.0068, 0.0269, 0.9985),
    vec4<f32>(-0.0249, 0.0063, 0.0, 0.0),
    vec4<f32>(0.0, -0.0034, 0.0132, 1.0000),
    vec4<f32>(-0.0127, 0.0029, 0.0, 0.0),
);

const COEF_USM: array<vec4<f32>, 128> = array<vec4<f32>, 128>(
    vec4<f32>(0.0, -0.6001, 1.2002, -0.6001),
    vec4<f32>(0.0, 0.0, 0.0, 0.0),
    vec4<f32>(0.0029, -0.6084, 1.1987, -0.5903),
    vec4<f32>(-0.0029, 0.0, 0.0, 0.0),
    vec4<f32>(0.0049, -0.6147, 1.1958, -0.5791),
    vec4<f32>(-0.0068, 0.0005, 0.0, 0.0),
    vec4<f32>(0.0073, -0.6196, 1.1890, -0.5659),
    vec4<f32>(-0.0103, 0.0, 0.0, 0.0),
    vec4<f32>(0.0093, -0.6235, 1.1802, -0.5513),
    vec4<f32>(-0.0151, 0.0, 0.0, 0.0),
    vec4<f32>(0.0112, -0.6265, 1.1699, -0.5352),
    vec4<f32>(-0.0195, 0.0005, 0.0, 0.0),
    vec4<f32>(0.0122, -0.6270, 1.1582, -0.5181),
    vec4<f32>(-0.0259, 0.0005, 0.0, 0.0),
    vec4<f32>(0.0142, -0.6284, 1.1455, -0.5005),
    vec4<f32>(-0.0317, 0.0005, 0.0, 0.0),
    vec4<f32>(0.0156, -0.6265, 1.1274, -0.4790),
    vec4<f32>(-0.0386, 0.0005, 0.0, 0.0),
    vec4<f32>(0.0166, -0.6235, 1.1089, -0.4570),
    vec4<f32>(-0.0454, 0.0010, 0.0, 0.0),
    vec4<f32>(0.0176, -0.6187, 1.0879, -0.4346),
    vec4<f32>(-0.0532, 0.0010, 0.0, 0.0),
    vec4<f32>(0.0181, -0.6138, 1.0659, -0.4102),
    vec4<f32>(-0.0615, 0.0015, 0.0, 0.0),
    vec4<f32>(0.0190, -0.6069, 1.0405, -0.3843),
    vec4<f32>(-0.0698, 0.0015, 0.0, 0.0),
    vec4<f32>(0.0195, -0.6006, 1.0161, -0.3574),
    vec4<f32>(-0.0796, 0.0020, 0.0, 0.0),
    vec4<f32>(0.0200, -0.5928, 0.9893, -0.3286),
    vec4<f32>(-0.0898, 0.0024, 0.0, 0.0),
    vec4<f32>(0.0200, -0.5820, 0.9580, -0.2988),
    vec4<f32>(-0.1001, 0.0029, 0.0, 0.0),
    vec4<f32>(0.0200, -0.5728, 0.9292, -0.2690),
    vec4<f32>(-0.1104, 0.0034, 0.0, 0.0),
    vec4<f32>(0.0200, -0.5620, 0.8975, -0.2368),
    vec4<f32>(-0.1226, 0.0039, 0.0, 0.0),
    vec4<f32>(0.0205, -0.5498, 0.8643, -0.2046),
    vec4<f32>(-0.1343, 0.0044, 0.0, 0.0),
    vec4<f32>(0.0200, -0.5371, 0.8301, -0.1709),
    vec4<f32>(-0.1465, 0.0049, 0.0, 0.0),
    vec4<f32>(0.0195, -0.5239, 0.7944, -0.1367),
    vec4<f32>(-0.1587, 0.0054, 0.0, 0.0),
    vec4<f32>(0.0195, -0.5107, 0.7598, -0.1021),
    vec4<f32>(-0.1724, 0.0059, 0.0, 0.0),
    vec4<f32>(0.0190, -0.4966, 0.7231, -0.0649),
    vec4<f32>(-0.1865, 0.0063, 0.0, 0.0),
    vec4<f32>(0.0186, -0.4819, 0.6846, -0.0288),
    vec4<f32>(-0.1997, 0.0068, 0.0, 0.0),
    vec4<f32>(0.0186, -0.4668, 0.6460, 0.0093),
    vec4<f32>(-0.2144, 0.0073, 0.0, 0.0),
    vec4<f32>(0.0176, -0.4507, 0.6055, 0.0479),
    vec4<f32>(-0.2290, 0.0083, 0.0, 0.0),
    vec4<f32>(0.0171, -0.4370, 0.5693, 0.0859),
    vec4<f32>(-0.2446, 0.0088, 0.0, 0.0),
    vec4<f32>(0.0161, -0.4199, 0.5283, 0.1255),
    vec4<f32>(-0.2598, 0.0098, 0.0, 0.0),
    vec4<f32>(0.0161, -0.4048, 0.4883, 0.1655),
    vec4<f32>(-0.2754, 0.0103, 0.0, 0.0),
    vec4<f32>(0.0151, -0.3887, 0.4497, 0.2041),
    vec4<f32>(-0.2910, 0.0107, 0.0, 0.0),
    vec4<f32>(0.0142, -0.3711, 0.4072, 0.2446),
    vec4<f32>(-0.3066, 0.0117, 0.0, 0.0),
    vec4<f32>(0.0137, -0.3555, 0.3672, 0.2852),
    vec4<f32>(-0.3228, 0.0122, 0.0, 0.0),
    vec4<f32>(0.0132, -0.3394, 0.3262, 0.3262),
    vec4<f32>(-0.3394, 0.0132, 0.0, 0.0),
    vec4<f32>(0.0122, -0.3228, 0.2852, 0.3672),
    vec4<f32>(-0.3555, 0.0137, 0.0, 0.0),
    vec4<f32>(0.0117, -0.3066, 0.2446, 0.4072),
    vec4<f32>(-0.3711, 0.0142, 0.0, 0.0),
    vec4<f32>(0.0107, -0.2910, 0.2041, 0.4497),
    vec4<f32>(-0.3887, 0.0151, 0.0, 0.0),
    vec4<f32>(0.0103, -0.2754, 0.1655, 0.4883),
    vec4<f32>(-0.4048, 0.0161, 0.0, 0.0),
    vec4<f32>(0.0098, -0.2598, 0.1255, 0.5283),
    vec4<f32>(-0.4199, 0.0161, 0.0, 0.0),
    vec4<f32>(0.0088, -0.2446, 0.0859, 0.5693),
    vec4<f32>(-0.4370, 0.0171, 0.0, 0.0),
    vec4<f32>(0.0083, -0.2290, 0.0479, 0.6055),
    vec4<f32>(-0.4507, 0.0176, 0.0, 0.0),
    vec4<f32>(0.0073, -0.2144, 0.0093, 0.6460),
    vec4<f32>(-0.4668, 0.0186, 0.0, 0.0),
    vec4<f32>(0.0068, -0.1997, -0.0288, 0.6846),
    vec4<f32>(-0.4819, 0.0186, 0.0, 0.0),
    vec4<f32>(0.0063, -0.1865, -0.0649, 0.7231),
    vec4<f32>(-0.4966, 0.0190, 0.0, 0.0),
    vec4<f32>(0.0059, -0.1724, -0.1021, 0.7598),
    vec4<f32>(-0.5107, 0.0195, 0.0, 0.0),
    vec4<f32>(0.0054, -0.1587, -0.1367, 0.7944),
    vec4<f32>(-0.5239, 0.0195, 0.0, 0.0),
    vec4<f32>(0.0049, -0.1465, -0.1709, 0.8301),
    vec4<f32>(-0.5371, 0.0200, 0.0, 0.0),
    vec4<f32>(0.0044, -0.1343, -0.2046, 0.8643),
    vec4<f32>(-0.5498, 0.0205, 0.0, 0.0),
    vec4<f32>(0.0039, -0.1226, -0.2368, 0.8975),
    vec4<f32>(-0.5620, 0.0200, 0.0, 0.0),
    vec4<f32>(0.0034, -0.1104, -0.2690, 0.9292),
    vec4<f32>(-0.5728, 0.0200, 0.0, 0.0),
    vec4<f32>(0.0029, -0.1001, -0.2988, 0.9580),
    vec4<f32>(-0.5820, 0.0200, 0.0, 0.0),
    vec4<f32>(0.0024, -0.0898, -0.3286, 0.9893),
    vec4<f32>(-0.5928, 0.0200, 0.0, 0.0),
    vec4<f32>(0.0020, -0.0796, -0.3574, 1.0161),
    vec4<f32>(-0.6006, 0.0195, 0.0, 0.0),
    vec4<f32>(0.0015, -0.0698, -0.3843, 1.0405),
    vec4<f32>(-0.6069, 0.0190, 0.0, 0.0),
    vec4<f32>(0.0015, -0.0615, -0.4102, 1.0659),
    vec4<f32>(-0.6138, 0.0181, 0.0, 0.0),
    vec4<f32>(0.0010, -0.0532, -0.4346, 1.0879),
    vec4<f32>(-0.6187, 0.0176, 0.0, 0.0),
    vec4<f32>(0.0010, -0.0454, -0.4570, 1.1089),
    vec4<f32>(-0.6235, 0.0166, 0.0, 0.0),
    vec4<f32>(0.0005, -0.0386, -0.4790, 1.1274),
    vec4<f32>(-0.6265, 0.0156, 0.0, 0.0),
    vec4<f32>(0.0005, -0.0317, -0.5005, 1.1455),
    vec4<f32>(-0.6284, 0.0142, 0.0, 0.0),
    vec4<f32>(0.0005, -0.0259, -0.5181, 1.1582),
    vec4<f32>(-0.6270, 0.0122, 0.0, 0.0),
    vec4<f32>(0.0005, -0.0195, -0.5352, 1.1699),
    vec4<f32>(-0.6265, 0.0112, 0.0, 0.0),
    vec4<f32>(0.0, -0.0151, -0.5513, 1.1802),
    vec4<f32>(-0.6235, 0.0093, 0.0, 0.0),
    vec4<f32>(0.0, -0.0103, -0.5659, 1.1890),
    vec4<f32>(-0.6196, 0.0073, 0.0, 0.0),
    vec4<f32>(0.0005, -0.0068, -0.5791, 1.1958),
    vec4<f32>(-0.6147, 0.0049, 0.0, 0.0),
    vec4<f32>(0.0, -0.0029, -0.5903, 1.1987),
    vec4<f32>(-0.6084, 0.0029, 0.0, 0.0),
);

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

fn scale_coef(phase: u32, tap: u32) -> f32 {
    let packed = COEF_SCALE[phase * 2u + tap / 4u];
    return packed[tap & 3u];
}

fn usm_coef(phase: u32, tap: u32) -> f32 {
    let packed = COEF_USM[phase * 2u + tap / 4u];
    return packed[tap & 3u];
}

fn pixel_at(pixels: ptr<function, array<vec4<f32>, 36>>, row: u32, col: u32) -> vec4<f32> {
    return (*pixels)[row * 6u + col];
}

fn luma_at(luma: ptr<function, array<f32, 36>>, row: u32, col: u32) -> f32 {
    return (*luma)[row * 6u + col];
}

fn get_y(rgb: vec3<f32>) -> f32 {
    return dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn get_edge_map(
    luma: ptr<function, array<f32, 36>>,
    row: u32,
    col: u32,
) -> vec4<f32> {
    let p00 = luma_at(luma, row, col);
    let p01 = luma_at(luma, row, col + 1u);
    let p02 = luma_at(luma, row, col + 2u);
    let p10 = luma_at(luma, row + 1u, col);
    let p11 = luma_at(luma, row + 1u, col + 1u);
    let p12 = luma_at(luma, row + 1u, col + 2u);
    let p20 = luma_at(luma, row + 2u, col);
    let p21 = luma_at(luma, row + 2u, col + 1u);
    let p22 = luma_at(luma, row + 2u, col + 2u);

    let g0 = abs(p00 + p01 + p02 - p20 - p21 - p22);
    let g45 = abs(p10 + p00 + p01 - p21 - p22 - p12);
    let g90 = abs(p00 + p10 + p20 - p02 - p12 - p22);
    let g135 = abs(p10 + p20 + p21 - p01 - p02 - p12);

    let g0_90_max = max(g0, g90);
    let g0_90_min = min(g0, g90);
    let g45_135_max = max(g45, g135);
    let g45_135_min = min(g45, g135);
    if g0_90_max + g45_135_max == 0.0 {
        return vec4<f32>(0.0);
    }

    let e0_90 = min(g0_90_max / (g0_90_max + g45_135_max), 1.0);
    let e45_135 = 1.0 - e0_90;
    let c0_90 =
        g0_90_max > g0_90_min * DETECT_RATIO &&
        g0_90_max > DETECT_THRESHOLD &&
        g0_90_max > g45_135_min;
    let c45_135 =
        g45_135_max > g45_135_min * DETECT_RATIO &&
        g45_135_max > DETECT_THRESHOLD &&
        g45_135_max > g0_90_min;
    let cg0_90 = g0_90_max == g0;
    let cg45_135 = g45_135_max == g45;

    var fe0_90 = 1.0;
    var fe45_135 = 1.0;
    if c0_90 && c45_135 {
        fe0_90 = e0_90;
        fe45_135 = e45_135;
    }

    var weights = vec4<f32>(0.0);
    if c0_90 {
        if cg0_90 {
            weights.x = fe0_90;
        } else {
            weights.y = fe0_90;
        }
    }
    if c45_135 {
        if cg45_135 {
            weights.z = fe45_135;
        } else {
            weights.w = fe45_135;
        }
    }
    return weights;
}

fn calc_lti(pixels: array<f32, 6>, phase: u32) -> f32 {
    var sel_a = pixels[3];
    var sel_b = pixels[5];
    if phase <= PHASE_COUNT / 2u {
        sel_a = pixels[0];
        sel_b = pixels[2];
    }
    let a_min = min(min(pixels[1], pixels[2]), sel_a);
    let a_max = max(max(pixels[1], pixels[2]), sel_a);
    let b_min = min(min(pixels[3], pixels[4]), sel_b);
    let b_max = max(max(pixels[3], pixels[4]), sel_b);
    let a_contrast = a_max - a_min;
    let b_contrast = b_max - b_min;
    let ratio = max(a_contrast, b_contrast) /
        (min(a_contrast, b_contrast) + LTI_EPSILON);
    return (1.0 - clamp(
        (ratio - MIN_CONTRAST_RATIO) * RATIO_NORM,
        0.0,
        1.0,
    )) * CONTRAST_BOOST;
}

fn eval_poly6(pixels: array<f32, 6>, phase: u32) -> f32 {
    var scaled = 0.0;
    var usm = 0.0;
    for (var tap = 0u; tap < 6u; tap += 1u) {
        scaled += scale_coef(phase, tap) * pixels[tap];
        usm += usm_coef(phase, tap) * pixels[tap];
    }

    let y_scale = 1.0 - clamp((scaled - SHARP_START_Y) * SHARP_SCALE_Y, 0.0, 1.0);
    usm *= y_scale * SHARP_STRENGTH_SCALE + SHARP_STRENGTH_MIN;
    let usm_limit = (y_scale * SHARP_LIMIT_SCALE + SHARP_LIMIT_MIN) * scaled;
    usm = min(usm_limit, max(-usm_limit, usm));
    usm *= calc_lti(pixels, phase);
    return scaled + usm;
}

fn filter_normal(
    luma: ptr<function, array<f32, 36>>,
    phase_x: u32,
    phase_y: u32,
) -> f32 {
    var horizontal = 0.0;
    for (var col = 0u; col < 6u; col += 1u) {
        var vertical = 0.0;
        for (var row = 0u; row < 6u; row += 1u) {
            vertical += luma_at(luma, row, col) * scale_coef(phase_y, row);
        }
        horizontal += vertical * scale_coef(phase_x, col);
    }
    return horizontal;
}

fn add_directional_filters(
    luma: ptr<function, array<f32, 36>>,
    frac_x: f32,
    frac_y: f32,
    phase_x: u32,
    phase_y: u32,
    weights: vec4<f32>,
) -> f32 {
    var filtered = 0.0;

    if weights.x > 0.0 {
        var line: array<f32, 6>;
        for (var row = 0u; row < 6u; row += 1u) {
            line[row] = mix(luma_at(luma, row, 2u), luma_at(luma, row, 3u), frac_x);
        }
        filtered += eval_poly6(line, phase_y) * weights.x;
    }

    if weights.y > 0.0 {
        var line: array<f32, 6>;
        for (var col = 0u; col < 6u; col += 1u) {
            line[col] = mix(luma_at(luma, 2u, col), luma_at(luma, 3u, col), frac_y);
        }
        filtered += eval_poly6(line, phase_x) * weights.y;
    }

    if weights.z > 0.0 {
        var base_phase = 0.5 + 0.5 * (frac_x - frac_y);
        var temporary: array<f32, 7>;
        temporary[1] = mix(luma_at(luma, 2u, 1u), luma_at(luma, 1u, 2u), base_phase);
        temporary[3] = mix(luma_at(luma, 3u, 2u), luma_at(luma, 2u, 3u), base_phase);
        temporary[5] = mix(luma_at(luma, 4u, 3u), luma_at(luma, 3u, 4u), base_phase);

        base_phase -= 0.5;
        var a = luma_at(luma, 2u, 0u);
        var b = luma_at(luma, 3u, 1u);
        var c = luma_at(luma, 4u, 2u);
        var d = luma_at(luma, 5u, 3u);
        if base_phase >= 0.0 {
            a = luma_at(luma, 0u, 2u);
            b = luma_at(luma, 1u, 3u);
            c = luma_at(luma, 2u, 4u);
            d = luma_at(luma, 3u, 5u);
        }
        temporary[0] = mix(luma_at(luma, 1u, 1u), a, abs(base_phase));
        temporary[2] = mix(luma_at(luma, 2u, 2u), b, abs(base_phase));
        temporary[4] = mix(luma_at(luma, 3u, 3u), c, abs(base_phase));
        temporary[6] = mix(luma_at(luma, 4u, 4u), d, abs(base_phase));

        var line: array<f32, 6>;
        var line_phase = frac_x + frac_y;
        var line_start = 0u;
        if line_phase >= 1.0 {
            line_start = 1u;
            line_phase -= 1.0;
        }
        for (var index = 0u; index < 6u; index += 1u) {
            line[index] = temporary[index + line_start];
        }
        let phase = min(u32(line_phase * f32(PHASE_COUNT)), PHASE_COUNT - 1u);
        filtered += eval_poly6(line, phase) * weights.z;
    }

    if weights.w > 0.0 {
        var base_phase = 0.5 * (frac_x + frac_y);
        var temporary: array<f32, 7>;
        temporary[1] = mix(luma_at(luma, 3u, 1u), luma_at(luma, 4u, 2u), base_phase);
        temporary[3] = mix(luma_at(luma, 2u, 2u), luma_at(luma, 3u, 3u), base_phase);
        temporary[5] = mix(luma_at(luma, 1u, 3u), luma_at(luma, 2u, 4u), base_phase);

        base_phase -= 0.5;
        var a = luma_at(luma, 3u, 0u);
        var b = luma_at(luma, 2u, 1u);
        var c = luma_at(luma, 1u, 2u);
        var d = luma_at(luma, 0u, 3u);
        if base_phase >= 0.0 {
            a = luma_at(luma, 5u, 2u);
            b = luma_at(luma, 4u, 3u);
            c = luma_at(luma, 3u, 4u);
            d = luma_at(luma, 2u, 5u);
        }
        temporary[0] = mix(luma_at(luma, 4u, 1u), a, abs(base_phase));
        temporary[2] = mix(luma_at(luma, 3u, 2u), b, abs(base_phase));
        temporary[4] = mix(luma_at(luma, 2u, 3u), c, abs(base_phase));
        temporary[6] = mix(luma_at(luma, 1u, 4u), d, abs(base_phase));

        var line: array<f32, 6>;
        var line_phase = 1.0 + frac_x - frac_y;
        var line_start = 0u;
        if line_phase >= 1.0 {
            line_start = 1u;
            line_phase -= 1.0;
        }
        for (var index = 0u; index < 6u; index += 1u) {
            line[index] = temporary[index + line_start];
        }
        let phase = min(u32(line_phase * f32(PHASE_COUNT)), PHASE_COUNT - 1u);
        filtered += eval_poly6(line, phase) * weights.w;
    }

    return filtered;
}

@fragment
fn fs_nis(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let destination = vec2<u32>(position.xy);
    let scale = params.source_extent / vec2<f32>(params.target_size);
    let source_position =
        params.source_origin + (vec2<f32>(destination) + vec2<f32>(0.5)) * scale - vec2<f32>(0.5);
    let raw_size = vec2<f32>(params.source_size);
    let oriented_size = vec2<f32>(
        dot(abs(params.inverse_x), raw_size),
        dot(abs(params.inverse_y), raw_size),
    );
    if any(source_position < vec2<f32>(-0.5))
        || any(source_position >= oriented_size - vec2<f32>(0.5)) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    let source_floor = vec2<i32>(floor(source_position));
    let fraction = source_position - floor(source_position);

    var pixels: array<vec4<f32>, 36>;
    var luma: array<f32, 36>;
    let maximum = vec2<i32>(params.source_size) - vec2<i32>(1);
    for (var row = 0u; row < 6u; row += 1u) {
        for (var col = 0u; col < 6u; col += 1u) {
            let offset = vec2<i32>(i32(col) - 2, i32(row) - 2);
            let oriented_coord = vec2<f32>(source_floor + offset);
            let raw_position =
                oriented_coord.x * params.inverse_x +
                oriented_coord.y * params.inverse_y +
                params.inverse_offset;
            let coord = clamp(vec2<i32>(round(raw_position)), vec2<i32>(0), maximum);
            let index = row * 6u + col;
            pixels[index] = textureLoad(source_texture, coord, 0);
            luma[index] = get_y(pixels[index].rgb);
        }
    }

    let edge00 = get_edge_map(&luma, 1u, 1u);
    let edge01 = get_edge_map(&luma, 1u, 2u);
    let edge10 = get_edge_map(&luma, 2u, 1u);
    let edge11 = get_edge_map(&luma, 2u, 2u);
    let edge_top = mix(edge00, edge01, fraction.x);
    let edge_bottom = mix(edge10, edge11, fraction.x);
    let weights = mix(edge_top, edge_bottom, fraction.y);

    let phase_x = min(u32(fraction.x * f32(PHASE_COUNT)), PHASE_COUNT - 1u);
    let phase_y = min(u32(fraction.y * f32(PHASE_COUNT)), PHASE_COUNT - 1u);
    let base_weight = 1.0 - dot(weights, vec4<f32>(1.0));
    var output_y = filter_normal(&luma, phase_x, phase_y) * base_weight;
    output_y += add_directional_filters(
        &luma,
        fraction.x,
        fraction.y,
        phase_x,
        phase_y,
        weights,
    );

    let top = mix(pixel_at(&pixels, 2u, 2u), pixel_at(&pixels, 2u, 3u), fraction.x);
    let bottom = mix(pixel_at(&pixels, 3u, 2u), pixel_at(&pixels, 3u, 3u), fraction.x);
    var output = mix(top, bottom, fraction.y);
    let correction = output_y - get_y(output.rgb);
    let corrected_rgb = clamp(
        output.rgb + vec3<f32>(correction),
        vec3<f32>(0.0),
        vec3<f32>(output.a),
    );
    output = vec4<f32>(corrected_rgb, output.a);

    // Source alpha is meaningful for transparent images. Keep it bilinear and never apply the
    // adaptive USM correction to alpha, which would create halos along transparency edges. The
    // source texture is premultiplied, so clamp corrected RGB to alpha as well.
    return output;
}
