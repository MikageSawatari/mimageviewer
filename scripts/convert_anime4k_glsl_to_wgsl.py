#!/usr/bin/env python3
'Convert Anime4K Upscale CNN x2 VL from mpv GLSL to mImageViewer WGSL.'

from __future__ import annotations

import argparse
import re
from dataclasses import dataclass
from pathlib import Path


EXPECTED_SAVES = [
    "conv2d_tf", "conv2d_tf1",
    "conv2d_1_tf", "conv2d_1_tf1",
    "conv2d_2_tf", "conv2d_2_tf1",
    "conv2d_3_tf", "conv2d_3_tf1",
    "conv2d_4_tf", "conv2d_4_tf1",
    "conv2d_5_tf", "conv2d_5_tf1",
    "conv2d_6_tf", "conv2d_6_tf1",
    "conv2d_last_tf", "conv2d_last_tf1", "conv2d_last_tf2",
]


@dataclass(frozen=True)
class Block:
    description: str
    binds: list[str]
    save: str
    defines: list[str]
    body: str


def parse_blocks(source: str) -> list[Block]:
    chunks = re.split(r"(?=^//!DESC )", source, flags=re.MULTILINE)
    blocks: list[Block] = []
    for chunk in chunks[1:]:
        description = re.search(r"^//!DESC (.+)$", chunk, re.MULTILINE).group(1)
        binds = re.findall(r"^//!BIND (.+)$", chunk, re.MULTILINE)
        save = re.search(r"^//!SAVE (.+)$", chunk, re.MULTILINE).group(1)
        defines = re.findall(r"^#define (.+)$", chunk, re.MULTILINE)
        body_match = re.search(r"vec4 hook\(\) \{\n(?P<body>.*?)\n\}", chunk, re.DOTALL)
        if body_match is None:
            raise ValueError(f"missing hook body in {description}")
        blocks.append(Block(description, binds, save, defines, body_match.group("body")))
    return blocks


def wgsl_load(texture_index: int, negative: bool, offset: str = "vec2<i32>(0, 0)") -> str:
    loaded = f"load_{texture_index}(coord, {offset})"
    if negative:
        loaded = f"-{loaded}"
    return f"max({loaded}, vec4<f32>(0.0))"


def integer_offset(value: str) -> str:
    number = float(value)
    if not number.is_integer():
        raise ValueError(f"non-integral convolution offset: {value}")
    return str(int(number))


def convert_block(block: Block, index: int) -> str:
    macros: dict[str, tuple[int, bool, str]] = {}
    for define in block.defines:
        go = re.fullmatch(
            r"(go_\d+)\(x_off, y_off\) \(max\(\(?(-)?\((.+?)_texOff"
            r"\(vec2\(x_off, y_off\)\)\)\)?, 0\.0\)\)", define,
        )
        if go:
            name, minus, resource = go.groups()
            macros[name] = (block.binds.index(resource), bool(minus), "call")
            continue
        raw_go = re.fullmatch(
            r"(go_\d+)\(x_off, y_off\) \((.+?)_texOff"
            r"\(vec2\(x_off, y_off\)\)\)", define,
        )
        if raw_go:
            name, resource = raw_go.groups()
            macros[name] = (block.binds.index(resource), False, "raw_call")
            continue
        g_value = re.fullmatch(
            r"(g_\d+) \(max\(\(?(-)?\((.+?)_tex\([^)]*\)\)\)?, 0\.0\)\)", define,
        )
        if g_value:
            name, minus, resource = g_value.groups()
            macros[name] = (block.binds.index(resource), bool(minus), "value")
            continue
        raise ValueError(f"unsupported macro in pass {index}: {define}")

    body = block.body
    value_lines: list[str] = []
    for name, (texture_index, negative, kind) in sorted(
        macros.items(), key=lambda pair: int(pair[0].split("_")[1])
    ):
        if kind == "value":
            value_lines.append(f"    let {name} = {wgsl_load(texture_index, negative)};")
            continue
        pattern = rf"{re.escape(name)}\(([-+0-9.eE]+), ([-+0-9.eE]+)\)"

        def replace_call(match: re.Match[str]) -> str:
            offset = (
                f"vec2<i32>({integer_offset(match.group(1))}, "
                f"{integer_offset(match.group(2))})"
            )
            if kind == "raw_call":
                return f"load_{texture_index}(coord, {offset})"
            return wgsl_load(texture_index, negative, offset)

        body = re.sub(pattern, replace_call, body)

    body = body.replace("vec4 result =", "var result: vec4<f32> =")
    body = body.replace("mat4(", "mat4x4<f32>(").replace("vec4(", "vec4<f32>(")
    body = "\n".join("    " + line.strip() for line in body.splitlines())
    return "\n".join([
        f"// {block.description}",
        f"// Inputs: {', '.join(block.binds)}; output: {block.save}.",
        "@fragment",
        f"fn fs_anime4k_{index}(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {{",
        "    let coord = vec2<i32>(i32(position.x), i32(position.y));",
        *value_lines,
        body,
        "}",
    ])


def shader_prelude(license_text: str) -> str:
    texture_declarations = "\n".join(
        f"@group(0) @binding({index}) var input_{index}: texture_2d<f32>;"
        for index in range(14)
    )
    load_functions = "\n\n".join(
        f'''fn load_{index}(coord: vec2<i32>, offset: vec2<i32>) -> vec4<f32> {{
    let maximum = vec2<i32>(params.input_size) - vec2<i32>(1, 1);
    let source_coord = clamp(coord + offset + params.input_origin, vec2<i32>(0, 0), maximum);
    return textureLoad(input_{index}, source_coord, 0);
}}'''
        for index in range(14)
    )
    return f'''{license_text.rstrip()}

// Generated by scripts/convert_anime4k_glsl_to_wgsl.py.
// Source: Anime4K_Upscale_CNN_x2_VL.glsl (Anime4K v3.2).
// Do not edit this file directly.

struct Anime4kParams {{
    output_size: vec2<u32>,
    input_size: vec2<u32>,
    input_origin: vec2<i32>,
    process_origin: vec2<i32>,
    source_size: vec2<u32>,
    process_size: vec2<u32>,
    source_region: vec4<f32>,
}}

{texture_declarations}
@group(0) @binding(14) var<uniform> params: Anime4kParams;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {{
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}}

{load_functions}
'''


def final_resolve() -> str:
    return r'''
fn source_sample(coord: vec2<i32>) -> vec4<f32> {
    let maximum = vec2<i32>(params.source_size) - vec2<i32>(1, 1);
    return textureLoad(input_0, clamp(coord, vec2<i32>(0, 0), maximum), 0);
}

fn bilinear_source(position: vec2<f32>) -> vec4<f32> {
    let base = vec2<i32>(floor(position));
    let fraction = fract(position);
    let top = mix(source_sample(base), source_sample(base + vec2<i32>(1, 0)), fraction.x);
    let bottom = mix(
        source_sample(base + vec2<i32>(0, 1)),
        source_sample(base + vec2<i32>(1, 1)),
        fraction.x,
    );
    return mix(top, bottom, fraction.y);
}

fn vector_component(value: vec4<f32>, index: u32) -> f32 {
    switch index {
        case 0u: { return value.x; }
        case 1u: { return value.y; }
        case 2u: { return value.z; }
        default: { return value.w; }
    }
}

fn correction_at(lattice_coord: vec2<i32>) -> vec3<f32> {
    let lattice_maximum = vec2<i32>(params.source_size * 2u) - vec2<i32>(1, 1);
    let lattice = clamp(lattice_coord, vec2<i32>(0, 0), lattice_maximum);
    let source_coord = lattice / 2;
    let local_maximum = vec2<i32>(params.process_size) - vec2<i32>(1, 1);
    let local_coord = clamp(
        source_coord - params.process_origin,
        vec2<i32>(0, 0),
        local_maximum,
    );
    let channel = u32((lattice.y & 1) * 2 + (lattice.x & 1));
    return vec3<f32>(
        vector_component(textureLoad(input_1, local_coord, 0), channel),
        vector_component(textureLoad(input_2, local_coord, 0), channel),
        vector_component(textureLoad(input_3, local_coord, 0), channel),
    );
}

fn bilinear_correction(position: vec2<f32>) -> vec3<f32> {
    let base = vec2<i32>(floor(position));
    let fraction = fract(position);
    let top = mix(correction_at(base), correction_at(base + vec2<i32>(1, 0)), fraction.x);
    let bottom = mix(
        correction_at(base + vec2<i32>(0, 1)),
        correction_at(base + vec2<i32>(1, 1)),
        fraction.x,
    );
    return mix(top, bottom, fraction.y);
}

// Resolve the fixed x2 correction lattice directly into the requested visible target.
@fragment
fn fs_anime4k_resolve(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let target_position = position.xy;
    let source_position = params.source_region.xy
        + target_position * params.source_region.zw / vec2<f32>(params.output_size)
        - vec2<f32>(0.5, 0.5);
    let base = bilinear_source(source_position);
    let correction_position = 2.0 * (source_position + vec2<f32>(0.5, 0.5))
        - vec2<f32>(0.5, 0.5);
    let corrected_rgb = clamp(
        base.rgb + bilinear_correction(correction_position),
        vec3<f32>(0.0),
        vec3<f32>(base.a),
    );
    return vec4<f32>(corrected_rgb, base.a);
}
'''


def main() -> None:
    repo_root = Path(__file__).resolve().parents[1]
    default_source = (
        repo_root.parent / "mimageviewer_testdata_upscale" / "shaders" / "anime4k"
        / "Anime4K_Upscale_CNN_x2_VL.glsl"
    )
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, default=default_source)
    parser.add_argument("--output", type=Path, default=repo_root / "src" / "gpu_anime4k.wgsl")
    args = parser.parse_args()

    source = args.source.read_text(encoding="utf-8")
    license_text = source.split("//!DESC", maxsplit=1)[0]
    blocks = parse_blocks(source)
    if len(blocks) != 18:
        raise ValueError(f"expected 17 convolution blocks plus resolve, found {len(blocks)}")
    convolution_blocks = blocks[:-1]
    saves = [block.save for block in convolution_blocks]
    if saves != EXPECTED_SAVES:
        raise ValueError(f"unexpected Anime4K pass topology: {saves!r}")
    if blocks[-1].save != "MAIN" or "Depth-to-Space" not in blocks[-1].description:
        raise ValueError("unexpected Anime4K final resolve block")

    generated = [shader_prelude(license_text)]
    generated.extend(convert_block(block, index) for index, block in enumerate(convolution_blocks))
    generated.append(final_resolve())
    args.output.write_text("\n\n".join(generated).rstrip() + "\n", encoding="utf-8", newline="\n")


if __name__ == "__main__":
    main()
