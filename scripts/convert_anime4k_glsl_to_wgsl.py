#!/usr/bin/env python3
"""Convert Anime4K Upscale CNN x2 variants from mpv GLSL to mImageViewer WGSL."""

from __future__ import annotations

import argparse
import difflib
import re
from dataclasses import dataclass
from pathlib import Path


VARIANTS = ("S", "M", "L", "VL", "UL")
OUTPUT_FILENAMES = {
    "S": "gpu_anime4k_s.wgsl",
    "M": "gpu_anime4k_m.wgsl",
    "L": "gpu_anime4k_l.wgsl",
    "VL": "gpu_anime4k.wgsl",
    "UL": "gpu_anime4k_ul.wgsl",
}
RUST_VARIANTS = {
    "S": "Small",
    "M": "Medium",
    "L": "Large",
    "VL": "VeryLarge",
    "UL": "UltraLarge",
}
SOURCE_INPUT = None


@dataclass(frozen=True)
class Block:
    description: str
    binds: list[str]
    save: str
    defines: list[str]
    body: str


@dataclass(frozen=True)
class GeneratedVariant:
    name: str
    shader: str
    pass_inputs: tuple[tuple[int | None, ...], ...]
    input_binding_count: int

    @property
    def intermediate_count(self) -> int:
        return len(self.pass_inputs) - 1


def required_match(pattern: str, text: str, description: str) -> re.Match[str]:
    match = re.search(pattern, text, re.MULTILINE)
    if match is None:
        raise ValueError(description)
    return match


def parse_blocks(source: str) -> list[Block]:
    chunks = re.split(r"(?=^//!DESC )", source, flags=re.MULTILINE)
    blocks: list[Block] = []
    for chunk in chunks[1:]:
        description = required_match(
            r"^//!DESC (.+)$", chunk, "missing Anime4K pass description"
        ).group(1)
        binds = re.findall(r"^//!BIND (.+)$", chunk, re.MULTILINE)
        save = required_match(
            r"^//!SAVE (.+)$", chunk, f"missing save target in {description}"
        ).group(1)
        defines = re.findall(r"^#define (.+)$", chunk, re.MULTILINE)
        body_match = re.search(r"vec4 hook\(\) \{\n(?P<body>.*?)\n\}", chunk, re.DOTALL)
        if body_match is None:
            raise ValueError(f"missing hook body in {description}")
        blocks.append(Block(description, binds, save, defines, body_match.group("body")))
    if not blocks:
        raise ValueError("no Anime4K passes found")
    return blocks


def resolve_bindings(
    variant: str,
    pass_index: int,
    binds: list[str],
    saved_outputs: dict[str, int],
) -> tuple[int | None, ...]:
    if not binds:
        raise ValueError(f"{variant} pass {pass_index} has no inputs")
    resolved: list[int | None] = []
    for resource in binds:
        if resource == "MAIN":
            resolved.append(SOURCE_INPUT)
            continue
        if resource not in saved_outputs:
            raise ValueError(
                f"{variant} pass {pass_index} binds unknown or future output {resource!r}"
            )
        resolved.append(saved_outputs[resource])
    return tuple(resolved)


def parse_topology(
    variant: str, blocks: list[Block]
) -> tuple[tuple[tuple[int | None, ...], ...], int]:
    if len(blocks) < 2:
        raise ValueError(f"{variant} needs convolution passes and a final resolve pass")
    convolution_blocks = blocks[:-1]
    resolve_block = blocks[-1]
    saved_outputs: dict[str, int] = {}
    pass_inputs: list[tuple[int | None, ...]] = []

    for pass_index, block in enumerate(convolution_blocks):
        if block.save == "MAIN":
            raise ValueError(f"{variant} convolution pass {pass_index} overwrites MAIN")
        if block.save in saved_outputs:
            raise ValueError(f"{variant} has duplicate save target {block.save!r}")
        inputs = resolve_bindings(variant, pass_index, block.binds, saved_outputs)
        has_source = SOURCE_INPUT in inputs
        if has_source and any(value is not SOURCE_INPUT for value in inputs):
            raise ValueError(
                f"{variant} convolution pass {pass_index} mixes source and intermediate coordinates"
            )
        pass_inputs.append(inputs)
        saved_outputs[block.save] = pass_index

    if resolve_block.save != "MAIN" or "Depth-to-Space" not in resolve_block.description:
        raise ValueError(f"{variant} has an unexpected final resolve pass")
    resolve_inputs = resolve_bindings(
        variant, len(convolution_blocks), resolve_block.binds, saved_outputs
    )
    if not resolve_inputs or resolve_inputs[0] is not SOURCE_INPUT:
        raise ValueError(f"{variant} final resolve must bind MAIN first")
    if any(value is SOURCE_INPUT for value in resolve_inputs[1:]):
        raise ValueError(f"{variant} final resolve binds MAIN more than once")
    correction_count = len(resolve_inputs) - 1
    if correction_count not in (1, 3):
        raise ValueError(
            f"{variant} final resolve has unsupported correction count {correction_count}"
        )
    pass_inputs.append(resolve_inputs)
    input_binding_count = max(len(inputs) for inputs in pass_inputs)
    return tuple(pass_inputs), input_binding_count


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
            r"\(vec2\(x_off, y_off\)\)\)\)?, 0\.0\)\)",
            define,
        )
        if go:
            name, minus, resource = go.groups()
            macros[name] = (block.binds.index(resource), bool(minus), "call")
            continue
        raw_go = re.fullmatch(
            r"(go_\d+)\(x_off, y_off\) \((.+?)_texOff"
            r"\(vec2\(x_off, y_off\)\)\)",
            define,
        )
        if raw_go:
            name, resource = raw_go.groups()
            macros[name] = (block.binds.index(resource), False, "raw_call")
            continue
        g_value = re.fullmatch(
            r"(g_\d+) \(max\(\(?(-)?\((.+?)_tex\([^)]*\)\)\)?, 0\.0\)\)",
            define,
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
    return "\n".join(
        [
            f"// {block.description}",
            f"// Inputs: {', '.join(block.binds)}; output: {block.save}.",
            "@fragment",
            f"fn fs_anime4k_{index}(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {{",
            "    let coord = vec2<i32>(i32(position.x), i32(position.y));",
            *value_lines,
            body,
            "}",
        ]
    )


def shader_prelude(license_text: str, variant: str, input_binding_count: int) -> str:
    texture_declarations = "\n".join(
        f"@group(0) @binding({index}) var input_{index}: texture_2d<f32>;"
        for index in range(input_binding_count)
    )
    load_functions = "\n\n".join(
        f"""fn load_{index}(coord: vec2<i32>, offset: vec2<i32>) -> vec4<f32> {{
    let maximum = vec2<i32>(params.input_size) - vec2<i32>(1, 1);
    let source_coord = clamp(coord + offset + params.input_origin, vec2<i32>(0, 0), maximum);
    return textureLoad(input_{index}, source_coord, 0);
}}"""
        for index in range(input_binding_count)
    )
    return f"""{license_text.rstrip()}

// Generated by scripts/convert_anime4k_glsl_to_wgsl.py.
// Source: Anime4K_Upscale_CNN_x2_{variant}.glsl (Anime4K v3.2).
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
@group(0) @binding({input_binding_count}) var<uniform> params: Anime4kParams;

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
"""


def final_resolve(correction_count: int) -> str:
    if correction_count == 3:
        return r"""
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
"""
    if correction_count != 1:
        raise ValueError(f"unsupported correction texture count: {correction_count}")
    return r"""
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
    let correction = vector_component(textureLoad(input_1, local_coord, 0), channel);
    return vec3<f32>(correction);
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
"""


def convert_source(source: str, variant: str) -> GeneratedVariant:
    if variant not in VARIANTS:
        raise ValueError(f"unknown Anime4K variant: {variant}")
    license_text = source.split("//!DESC", maxsplit=1)[0]
    if license_text == source:
        raise ValueError(f"{variant} source has no Anime4K pass markers")
    blocks = parse_blocks(source)
    pass_inputs, input_binding_count = parse_topology(variant, blocks)
    convolution_blocks = blocks[:-1]
    correction_count = len(pass_inputs[-1]) - 1

    generated = [shader_prelude(license_text, variant, input_binding_count)]
    generated.extend(convert_block(block, index) for index, block in enumerate(convolution_blocks))
    generated.append(final_resolve(correction_count))
    shader = "\n\n".join(generated).rstrip() + "\n"
    return GeneratedVariant(variant, shader, pass_inputs, input_binding_count)


def load_variants(source_dir: Path) -> list[GeneratedVariant]:
    generated: list[GeneratedVariant] = []
    for variant in VARIANTS:
        source_path = source_dir / f"Anime4K_Upscale_CNN_x2_{variant}.glsl"
        source = source_path.read_text(encoding="utf-8")
        generated.append(convert_source(source, variant))
    return generated


def rust_input(value: int | None) -> str:
    if value is SOURCE_INPUT:
        return "Anime4kPassInput::Source"
    return f"Anime4kPassInput::Intermediate({value})"


def render_rust_topology(variants: list[GeneratedVariant]) -> str:
    lines = [
        "// Generated by scripts/convert_anime4k_glsl_to_wgsl.py.",
        "// Do not edit this file directly.",
        "",
        "const GENERATED_ANIME4K_VARIANTS: &[Anime4kVariantData] = &[",
    ]
    for generated in variants:
        lines.extend(
            [
                "    Anime4kVariantData {",
                f"        variant: Anime4kVariant::{RUST_VARIANTS[generated.name]},",
                f'        label: "Anime4K x2 {generated.name}",',
                f'        shader: include_str!("{OUTPUT_FILENAMES[generated.name]}"),',
                f"        input_binding_count: {generated.input_binding_count},",
                "        pass_inputs: &[",
            ]
        )
        for inputs in generated.pass_inputs:
            rendered = ", ".join(rust_input(value) for value in inputs)
            lines.append(f"            &[{rendered}],")
        lines.extend(["        ],", "    },"])
    lines.extend(["];", ""])
    return "\n".join(lines)


def compare_or_write(path: Path, generated: str, check: bool) -> None:
    if not check:
        path.write_text(generated, encoding="utf-8", newline="\n")
        return
    existing = path.read_text(encoding="utf-8")
    if existing == generated:
        return
    difference = "".join(
        difflib.unified_diff(
            existing.splitlines(keepends=True),
            generated.splitlines(keepends=True),
            fromfile=str(path),
            tofile=f"generated:{path.name}",
            n=3,
        )
    )
    raise ValueError(f"generated Anime4K output is stale: {path}\n{difference}")


def main() -> None:
    repo_root = Path(__file__).resolve().parents[1]
    default_source_dir = (
        repo_root.parent / "mimageviewer_testdata_upscale" / "shaders" / "anime4k"
    )
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-dir", type=Path, default=default_source_dir)
    parser.add_argument("--output-dir", type=Path, default=repo_root / "src")
    parser.add_argument("--rust-output", type=Path)
    parser.add_argument(
        "--check",
        action="store_true",
        help="fail if committed WGSL or Rust topology differs from generated output",
    )
    args = parser.parse_args()

    variants = load_variants(args.source_dir)
    for generated in variants:
        compare_or_write(
            args.output_dir / OUTPUT_FILENAMES[generated.name],
            generated.shader,
            args.check,
        )
    rust_output = args.rust_output or args.output_dir / "gpu_anime4k_generated.rs"
    compare_or_write(rust_output, render_rust_topology(variants), args.check)


if __name__ == "__main__":
    main()
