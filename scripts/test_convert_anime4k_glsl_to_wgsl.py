#!/usr/bin/env python3
"""Golden and topology tests for the Anime4K GLSL converter."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path


SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
SOURCE_DIR = (
    REPO_ROOT.parent
    / "mimageviewer_testdata_upscale"
    / "shaders"
    / "anime4k"
)
sys.path.insert(0, str(SCRIPT_DIR))

from convert_anime4k_glsl_to_wgsl import (  # noqa: E402
    OUTPUT_FILENAMES,
    VARIANTS,
    convert_source,
    load_variants,
    render_rust_topology,
)


class Anime4kConverterTests(unittest.TestCase):
    def source(self, variant: str) -> str:
        return (
            SOURCE_DIR / f"Anime4K_Upscale_CNN_x2_{variant}.glsl"
        ).read_text(encoding="utf-8")

    def test_regenerated_vl_matches_committed_shader_golden(self) -> None:
        generated = convert_source(self.source("VL"), "VL").shader
        committed = (REPO_ROOT / "src" / "gpu_anime4k.wgsl").read_text(
            encoding="utf-8"
        )
        self.assertEqual(generated, committed)

    def test_all_variants_emit_expected_topology_and_own_license(self) -> None:
        expected = {
            "S": (5, 2),
            "M": (9, 7),
            "L": (10, 4),
            "VL": (18, 14),
            "UL": (25, 15),
        }
        for variant in VARIANTS:
            with self.subTest(variant=variant):
                source = self.source(variant)
                generated = convert_source(source, variant)
                pass_count, binding_count = expected[variant]
                self.assertEqual(len(generated.pass_inputs), pass_count)
                self.assertEqual(generated.input_binding_count, binding_count)
                license_text = source.split("//!DESC", maxsplit=1)[0].rstrip()
                self.assertTrue(generated.shader.startswith(license_text))
                self.assertIn(
                    f"Source: Anime4K_Upscale_CNN_x2_{variant}.glsl",
                    generated.shader,
                )

    def test_all_committed_outputs_match_the_converter(self) -> None:
        variants = load_variants(SOURCE_DIR)
        for generated in variants:
            with self.subTest(variant=generated.name):
                committed = (
                    REPO_ROOT / "src" / OUTPUT_FILENAMES[generated.name]
                ).read_text(encoding="utf-8")
                self.assertEqual(generated.shader, committed)
        committed_rust = (
            REPO_ROOT / "src" / "gpu_anime4k_generated.rs"
        ).read_text(encoding="utf-8")
        self.assertEqual(render_rust_topology(variants), committed_rust)

    def test_unknown_binding_is_an_error(self) -> None:
        source = self.source("S").replace(
            "//!BIND MAIN", "//!BIND missing_output", 1
        )
        with self.assertRaisesRegex(ValueError, "unknown or future output"):
            convert_source(source, "S")


if __name__ == "__main__":
    unittest.main()
