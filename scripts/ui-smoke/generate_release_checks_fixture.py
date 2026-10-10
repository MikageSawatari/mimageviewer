#!/usr/bin/env python3
"""Create release-check media and overlays; never launch mImageViewer or seed DBs.

CLI writes only a fresh descendant of this repository's target/ui-smoke/runs.
The allowed_root argument is an API seam for isolated temporary-directory tests,
not a command-line escape hatch. App seed requirements are a recipe, not evidence
that caches, pins or indexes have already been prepared.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import stat
import struct
import subprocess
import sys
import zlib
from pathlib import Path

from generate_rating_sort_fixture import png


RECIPE_VERSION = "release-checks-v3"
SCHEMA_VERSION = 1
WIDTH, HEIGHT = 884, 444
STATIC_COUNT = 16
STATIC_REQUESTED_PX_TARGET = 374
STATIC_REQUESTED_PX_TOLERANCE = 8
STATIC_THUMB_ASPECT = "Portrait3x4"
FOLDER_COUNT, IMAGES_PER_FOLDER = 512, 4
TRAY_FOLDER_COUNT, TRAY_IMAGES_PER_FOLDER = 600, 1
TRAY_WIDTH, TRAY_HEIGHT = 400, 300
TRAY_GRID_COLS = 10
NAVIGATION_COUNT = 6
SEARCH_TERM = "mivreleasefixtureorchid"
SEARCH_HIT_NUMBERS = (1, 3, 6)
DEFAULT_ALLOWED_ROOT = Path(__file__).resolve().parents[2] / "target" / "ui-smoke" / "runs"
MANY_NAMES = ("cached", "uncached")
TRAY_NAMES = ("tray-while", "tray-after")
CASE_PATHS = {
    "static-foreground": "static",
    "static-background": "static",
    "video-pin-background": "video-pin",
    "tray-close-while-loading": "tray-while",
    "tray-close-after-loading": "tray-after",
    "perf-cached": "cached",
    "perf-uncached": "uncached",
}


def reject_reparse(path: Path) -> None:
    """Inspect lexical ancestors before resolve() can hide a junction/symlink."""
    for candidate in (path, *path.parents):
        try:
            info = candidate.lstat()
        except FileNotFoundError:
            continue
        if stat.S_ISLNK(info.st_mode) or getattr(info, "st_file_attributes", 0) & 0x400:
            raise ValueError(f"refusing reparse point: {candidate}")


def isolated_path(path: Path, allowed_root: Path) -> Path:
    path, allowed_root = Path(path), Path(allowed_root)
    if ".." in path.parts or ".." in allowed_root.parts:
        raise ValueError("parent traversal is not allowed")
    path = Path(os.path.abspath(path))
    allowed_root = Path(os.path.abspath(allowed_root))
    reject_reparse(path)
    reject_reparse(allowed_root)
    resolved, root = path.resolve(), allowed_root.resolve()
    if resolved == root or not resolved.is_relative_to(root):
        raise ValueError(f"fixture must be a strict descendant of {root}")
    return resolved


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def fixture_png(identity: int, prompt: str | None = None, *,
                width: int = WIDTH, height: int = HEIGHT) -> bytes:
    # Reuse the existing PNG encoder. Identity lives in pixels, not merely text:
    # every generated image differs even after a decoder discards metadata.
    if not 0 < identity < 1 << 24:
        raise ValueError("image identity must fit a nonzero 24-bit RGB value")
    color = ((identity >> 16) & 255, (identity >> 8) & 255, identity & 255)
    encoded = png(color, width=width, height=height)
    if prompt is not None:
        payload = b"parameters\0" + (
            prompt + "\nNegative prompt: synthetic noise\n"
            f"Steps: 20, Sampler: Euler, CFG scale: 7, Seed: 1, Size: {width}x{height}"
        ).encode("ascii")
        kind = b"tEXt"
        chunk = struct.pack(">I", len(payload)) + kind + payload
        chunk += struct.pack(">I", zlib.crc32(kind + payload) & 0xFFFFFFFF)
        encoded = encoded[:33] + chunk + encoded[33:]
    return encoded


def settings_overlay(start_path: Path, search_root: Path, *, tray_case: bool = False,
                     seed_cache: bool = False, static_case: bool = False) -> dict:
    overlay = {
        "startup_folder_mode": "specific",
        "startup_folder_path": str(start_path),
        "startup_window_state": "normal",
        "window_maximized": False,
        "window_size": [1328.0, 900.0],
        "grid_view_mode": "Thumbnail",
        "grid_cols": 7,
        # §1.198: the taller 3:4 cell requests ~374px at 150% DPI here.
        # A 16:9 cell with the same width requests only ~281px, missing the
        # integer-decode regression. Actual request evidence remains mandatory.
        "thumb_aspect": STATIC_THUMB_ASPECT if static_case else "Landscape16x9",
        "thumb_aspect_auto": False,
        "sort_order": "FileName",
        "minimize_to_tray_on_close": True,
        "always_on_top": False,
        "auto_fullscreen_image_folders": False,
        "remember_favorite_view_state": False,
        # The stopped seed profile persists Always. Override it explicitly in
        # runtime cases instead of inheriting the preparation policy.
        "cache_policy": "Auto",
        "keymap": {"overrides": [], "legacy_ini_migration_done": True,
                   "legacy_ini_backup": None},
        "favorites": [{
            "id": "64289cf0-6b51-46c7-93ba-721c921cc001",
            "name": "Generated release metadata",
            "path": str(search_root),
            "auto_index_structure": True,
            "auto_index_metadata": True,
            "auto_index_thumbs": False,
            "auto_index_similar": False,
        }],
    }
    if tray_case:
        overlay["cache_policy"] = "Off"
        overlay["grid_cols"] = TRAY_GRID_COLS
    elif seed_cache:
        # Normal saving producer, with the released Always policy, prepares small
        # generated PNGs reliably. Runtime non-tray cases retain default Auto.
        overlay["cache_policy"] = "Always"
    return overlay


def static_recipe() -> dict:
    return {"width": WIDTH, "height": HEIGHT, "window_size": [1328, 900],
            "grid_cols": 7, "thumb_aspect": STATIC_THUMB_ASPECT, "thumb_aspect_auto": False,
            "requested_px_hint": STATIC_REQUESTED_PX_TARGET,
            "requested_px_target": STATIC_REQUESTED_PX_TARGET,
            "requested_px_tolerance": STATIC_REQUESTED_PX_TOLERANCE,
            "requested_px_must_be_observed": True}


def tray_recipe() -> dict:
    # A's isolated hardware control reproduced both tray regressions with small,
    # fast-decoding images. Large slow decodes do not build the same backlog.
    return {"width": TRAY_WIDTH, "height": TRAY_HEIGHT,
            "subtrees": len(TRAY_NAMES), "folders_per_subtree": TRAY_FOLDER_COUNT,
            "images_per_folder": TRAY_IMAGES_PER_FOLDER, "grid_cols": TRAY_GRID_COLS}


def ffmpeg_arguments(ffmpeg: Path, video: Path) -> list[str]:
    return [str(ffmpeg), "-hide_banner", "-loglevel", "error", "-nostdin", "-n",
            "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=10:duration=2",
            "-map", "0:v:0", "-an", "-c:v", "mpeg4", "-q:v", "5",
            "-threads", "1", "-flags:v", "+bitexact", "-fflags", "+bitexact",
            "-map_metadata", "-1", "-metadata", "creation_time=1970-01-01T00:00:00Z",
            "-pix_fmt", "yuv420p", str(video)]


def generate(output: Path, ffmpeg: Path, *, allowed_root: Path = DEFAULT_ALLOWED_ROOT,
             run_command=None) -> dict:
    output = isolated_path(output, allowed_root)
    if output.exists() and (not output.is_dir() or any(output.iterdir())):
        raise ValueError(f"refusing non-empty fixture output: {output}")
    ffmpeg = Path(ffmpeg).resolve(strict=True)
    if not ffmpeg.is_file():
        raise ValueError("FFmpeg must be an existing executable file")
    # All refusal checks precede any output creation. No replacement or cleanup:
    # incomplete media remains evidence, and a rerun must use a fresh run folder.
    output.mkdir(parents=True, exist_ok=True)
    files = []
    paths = {name: str(output / name)
             for name in ("static", *MANY_NAMES, *TRAY_NAMES, "video-pin", "search")}
    identity = 0

    def image(path: Path, prompt: str | None = None, *,
              width: int = WIDTH, height: int = HEIGHT) -> None:
        nonlocal identity
        identity += 1
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(fixture_png(identity, prompt, width=width, height=height))
        files.append({"path": str(path), "relative_path": path.relative_to(output).as_posix(),
                      "kind": "png", "size_bytes": path.stat().st_size,
                      "sha256": sha256(path), "width": width, "height": height,
                      "pixel_identity": identity})

    for number in range(1, STATIC_COUNT + 1):
        image(Path(paths["static"]) / f"static-{number:03d}.png")
    for name in MANY_NAMES:
        for folder in range(1, FOLDER_COUNT + 1):
            for number in range(1, IMAGES_PER_FOLDER + 1):
                image(Path(paths[name]) / f"folder-{folder:04d}" / f"image-{number:02d}.png")
    for name in TRAY_NAMES:
        for folder in range(1, TRAY_FOLDER_COUNT + 1):
            for number in range(1, TRAY_IMAGES_PER_FOLDER + 1):
                image(Path(paths[name]) / f"folder-{folder:04d}" / f"image-{number:02d}.png",
                      width=TRAY_WIDTH, height=TRAY_HEIGHT)

    perf = {}
    for mode in ("cached", "uncached"):
        navigation = output / f"perf-{mode}-navigation"
        paths[f"perf-{mode}-navigation"] = str(navigation)
        order = []
        for folder in range(1, NAVIGATION_COUNT + 1):
            child = navigation / f"navigation-{folder:02d}"
            order.append(str(child))
            for number in range(1, IMAGES_PER_FOLDER + 1):
                image(child / f"image-{number:02d}.png")
        perf[mode] = {"many_folders": paths[mode], "start_folder": order[0],
                      "navigation_order": order, "ctrl_down_count": 5,
                      "cache_prepared": False}
    paths["navigation"] = paths["perf-cached-navigation"]
    hits, non_hits = [], []
    for number in range(1, 7):
        path = Path(paths["search"]) / f"metadata-{number:02d}.png"
        hit = number in SEARCH_HIT_NUMBERS
        image(path, f"generated {SEARCH_TERM if hit else 'neutral'} illustration")
        (hits if hit else non_hits).append(str(path))

    child = Path(paths["video-pin"]) / "video-child"
    child.mkdir(parents=True)
    video = child / "pin-source.mp4"
    paths.update({"video-pin-child": str(child), "video-pin-source": str(video)})
    command = ffmpeg_arguments(ffmpeg, video)
    runner = run_command or subprocess.run
    runner(command, check=True, timeout=120,
           creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    if not video.is_file() or video.stat().st_size == 0:
        raise ValueError("FFmpeg did not produce a non-empty video")
    files.append({"path": str(video), "relative_path": video.relative_to(output).as_posix(),
                  "kind": "video", "size_bytes": video.stat().st_size,
                  "sha256": sha256(video), "width": 640, "height": 360,
                  "duration_seconds": 2, "fps": 10, "audio_streams": 0})

    overlays = {}
    overlay_values = {}
    for case, name in CASE_PATHS.items():
        overlay_values[case] = settings_overlay(Path(paths[name]), Path(paths["search"]),
                                               tray_case=case.startswith("tray-"),
                                               static_case=case.startswith("static-"))
    for seed, name in (("seed-static", "static"), ("seed-perf-cached", "cached"),
                       ("seed-video-pin", "video-pin-child"), ("seed-search", "search")):
        overlay_values[seed] = settings_overlay(Path(paths[name]), Path(paths["search"]),
                                               seed_cache=seed != "seed-search",
                                               static_case=seed == "seed-static")
    for case, value in overlay_values.items():
        path = output / "overlays" / f"{case}.json"
        path.parent.mkdir(exist_ok=True)
        path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")
        overlays[case] = str(path)
        files.append({"path": str(path), "relative_path": path.relative_to(output).as_posix(),
                      "kind": "settings-overlay", "size_bytes": path.stat().st_size,
                      "sha256": sha256(path)})

    manifest = {
        "schema_version": SCHEMA_VERSION, "recipe_version": RECIPE_VERSION,
        "fixture_root": str(output), "seed_state": "not_prepared",
        "paths": paths, "overlays": overlays, "files": files,
        "counts": {"static_images": STATIC_COUNT, "many_subtrees": len(MANY_NAMES),
                   "folders_per_many_subtree": FOLDER_COUNT,
                   "images_per_many_folder": IMAGES_PER_FOLDER,
                   "tray_subtrees": len(TRAY_NAMES),
                   "folders_per_tray_subtree": TRAY_FOLDER_COUNT,
                   "images_per_tray_folder": TRAY_IMAGES_PER_FOLDER,
                   "navigation_subtrees": 2, "navigation_folders_per_subtree": NAVIGATION_COUNT,
                   "search_images": 6, "search_hits": len(hits),
                   "png_images": identity, "videos": 1},
        "static": static_recipe(),
        "tray": tray_recipe(),
        "search": {"root": paths["search"], "term": SEARCH_TERM,
                   "metadata_key": "parameters", "expected_hits": hits,
                   "expected_non_hits": non_hits, "index_settle_timeout_seconds": 180},
        "perf": perf,
        "video_pin": {"parent": paths["video-pin"], "child": str(child),
                      "source": str(video), "seek_seconds": 0.5,
                      "playback_required": False, "restart_required": True},
        "media_generation": {"ffmpeg_path": str(ffmpeg), "ffmpeg_sha256": sha256(ffmpeg),
                             "ffmpeg_arguments": command[1:]},
        "seed_requirements": {
            "write_owner": "normal_app_apis", "app_launch_requires_interactive_approval": True,
            "profile_copy_requires_clean_process_exit": True,
            "same_absolute_paths_after_restore": True,
            "cache": {"prepare": [paths["static"], paths["cached"],
                                   *perf["cached"]["navigation_order"]],
                      "read_after_restart": True,
                      "must_remain_unread": [paths["uncached"], paths["tray-while"],
                                             paths["tray-after"],
                                             paths["perf-uncached-navigation"]]},
            "pin": {"normal_video_pin_api": True, "source": str(video),
                    "catalog_saved": True, "cached_parent_tile_after_restart": True},
            "index": {"roots": [paths["search"]], "auto_index_thumbs": False,
                      "initial_scan_settled": True, "exact_expected_hit_set": hits},
            "tray": {"cache_policy": "Off", "close_boundary_cache_absent": True,
                     "post_close_source_finalized_backlog_correlation": True},
        },
    }
    (output / "fixture-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n",
                                                   encoding="utf-8")
    return manifest


def verify_fixture(output: Path, *, allowed_root: Path = DEFAULT_ALLOWED_ROOT) -> dict:
    """Verify media/overlay integrity only; no inferred App seed success."""
    output = isolated_path(output, allowed_root)
    manifest_path = output / "fixture-manifest.json"
    reject_reparse(manifest_path)
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if (manifest.get("schema_version") != SCHEMA_VERSION or
            manifest.get("recipe_version") != RECIPE_VERSION or
            manifest.get("fixture_root") != str(output) or
            manifest.get("seed_state") != "not_prepared"):
        raise ValueError("manifest version, root or preparation state is invalid")
    expected_counts = {
        "static_images": STATIC_COUNT, "many_subtrees": len(MANY_NAMES),
        "folders_per_many_subtree": FOLDER_COUNT, "images_per_many_folder": IMAGES_PER_FOLDER,
        "tray_subtrees": len(TRAY_NAMES), "folders_per_tray_subtree": TRAY_FOLDER_COUNT,
        "images_per_tray_folder": TRAY_IMAGES_PER_FOLDER,
        "navigation_subtrees": 2, "navigation_folders_per_subtree": NAVIGATION_COUNT,
        "search_images": 6, "search_hits": len(SEARCH_HIT_NUMBERS),
        "png_images": STATIC_COUNT + len(MANY_NAMES) * FOLDER_COUNT * IMAGES_PER_FOLDER
                      + len(TRAY_NAMES) * TRAY_FOLDER_COUNT * TRAY_IMAGES_PER_FOLDER
                      + 2 * NAVIGATION_COUNT * IMAGES_PER_FOLDER + 6,
        "videos": 1,
    }
    required_overlays = {*CASE_PATHS, "seed-static", "seed-perf-cached", "seed-video-pin", "seed-search"}
    if manifest.get("counts") != expected_counts or set(manifest["overlays"]) != required_overlays:
        raise ValueError("fixture counts or required overlays are incomplete")
    if manifest.get("tray") != tray_recipe():
        raise ValueError("tray reproduction shape is invalid")
    if (manifest["static"].get("requested_px_target") != STATIC_REQUESTED_PX_TARGET or
            manifest["static"].get("requested_px_tolerance") != STATIC_REQUESTED_PX_TOLERANCE or
            manifest["static"].get("requested_px_must_be_observed") is not True):
        raise ValueError("static request observation range is invalid")
    if manifest["static"] != static_recipe():
        raise ValueError("static regression recipe is invalid")
    expected_paths = {name: str(output / name)
                      for name in ("static", *MANY_NAMES, *TRAY_NAMES, "video-pin", "search")}
    expected_paths.update({f"perf-{mode}-navigation": str(output / f"perf-{mode}-navigation")
                           for mode in ("cached", "uncached")})
    expected_paths.update({
        "navigation": str(output / "perf-cached-navigation"),
        "video-pin-child": str(output / "video-pin" / "video-child"),
        "video-pin-source": str(output / "video-pin" / "video-child" / "pin-source.mp4"),
    })
    if manifest["paths"] != expected_paths:
        raise ValueError("fixture paths do not match the isolated recipe")
    expected_files = {output / "static" / f"static-{number:03d}.png"
                      for number in range(1, STATIC_COUNT + 1)}
    expected_files.update(output / name / f"folder-{folder:04d}" / f"image-{number:02d}.png"
                          for name in MANY_NAMES for folder in range(1, FOLDER_COUNT + 1)
                          for number in range(1, IMAGES_PER_FOLDER + 1))
    expected_files.update(output / name / f"folder-{folder:04d}" / f"image-{number:02d}.png"
                          for name in TRAY_NAMES for folder in range(1, TRAY_FOLDER_COUNT + 1)
                          for number in range(1, TRAY_IMAGES_PER_FOLDER + 1))
    expected_files.update(output / f"perf-{mode}-navigation" / f"navigation-{folder:02d}"
                          / f"image-{number:02d}.png" for mode in ("cached", "uncached")
                          for folder in range(1, NAVIGATION_COUNT + 1)
                          for number in range(1, IMAGES_PER_FOLDER + 1))
    expected_files.update(output / "search" / f"metadata-{number:02d}.png" for number in range(1, 7))
    expected_files.update(output / "overlays" / f"{case}.json" for case in required_overlays)
    expected_files.add(output / "video-pin" / "video-child" / "pin-source.mp4")
    seen = set()
    for entry in manifest["files"]:
        path = isolated_path(Path(entry["path"]), output)
        if entry["relative_path"] != path.relative_to(output).as_posix() or path in seen:
            raise ValueError("manifest file address is invalid or duplicated")
        seen.add(path)
        if path.stat().st_size != entry["size_bytes"] or sha256(path) != entry["sha256"]:
            raise ValueError(f"fixture content changed: {path}")
        if path.suffix == ".png":
            dimensions = ((TRAY_WIDTH, TRAY_HEIGHT)
                          if path.relative_to(output).parts[0] in TRAY_NAMES else (WIDTH, HEIGHT))
            if entry.get("kind") != "png" or (entry.get("width"), entry.get("height")) != dimensions:
                raise ValueError(f"fixture PNG dimensions are invalid: {path}")
    if seen != expected_files:
        raise ValueError("fixture file inventory is incomplete or unexpected")
    for name, path in manifest["paths"].items():
        isolated_path(Path(path), output).resolve(strict=True)
    expected_hits = [str(output / "search" / f"metadata-{number:02d}.png")
                     for number in SEARCH_HIT_NUMBERS]
    if (manifest["search"]["term"] != SEARCH_TERM or
            manifest["search"]["root"] != expected_paths["search"] or
            manifest["search"]["expected_hits"] != expected_hits or
            manifest["seed_requirements"]["index"]["exact_expected_hit_set"] != expected_hits or
            manifest["seed_requirements"]["index"]["roots"] != [expected_paths["search"]] or
            manifest["seed_requirements"]["index"]["auto_index_thumbs"] is not False):
        raise ValueError("search expectations are invalid")
    for mode in ("cached", "uncached"):
        order = [str(output / f"perf-{mode}-navigation" / f"navigation-{folder:02d}")
                 for folder in range(1, NAVIGATION_COUNT + 1)]
        if manifest["perf"][mode] != {
                "many_folders": expected_paths[mode], "start_folder": order[0],
                "navigation_order": order, "ctrl_down_count": 5, "cache_prepared": False}:
            raise ValueError("perf preparation recipe is invalid")
    if manifest["seed_requirements"]["cache"] != {
            "prepare": [expected_paths["static"], expected_paths["cached"],
                        *manifest["perf"]["cached"]["navigation_order"]],
            "read_after_restart": True,
            "must_remain_unread": [expected_paths["uncached"], expected_paths["tray-while"],
                                   expected_paths["tray-after"],
                                   expected_paths["perf-uncached-navigation"]]}:
        raise ValueError("cache seed isolation requirements are invalid")
    if manifest["seed_requirements"]["tray"] != {
            "cache_policy": "Off", "close_boundary_cache_absent": True,
            "post_close_source_finalized_backlog_correlation": True}:
        raise ValueError("tray seed observation requirements are invalid")
    for case, path in manifest["overlays"].items():
        if case in CASE_PATHS:
            expected = settings_overlay(Path(manifest["paths"][CASE_PATHS[case]]),
                                        Path(manifest["paths"]["search"]),
                                        tray_case=case.startswith("tray-"),
                                        static_case=case.startswith("static-"))
        elif case in ("seed-static", "seed-perf-cached", "seed-video-pin", "seed-search"):
            name = {"seed-static": "static", "seed-perf-cached": "cached",
                    "seed-video-pin": "video-pin-child", "seed-search": "search"}[case]
            expected = settings_overlay(Path(manifest["paths"][name]),
                                        Path(manifest["paths"]["search"]),
                                        seed_cache=case != "seed-search",
                                        static_case=case == "seed-static")
        else:
            raise ValueError(f"unknown overlay: {case}")
        overlay_path = isolated_path(Path(path), output)
        if overlay_path not in seen or json.loads(overlay_path.read_text(encoding="utf-8")) != expected:
            raise ValueError(f"invalid settings overlay: {case}")
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--ffmpeg", type=Path, help="exact FFmpeg executable path")
    parser.add_argument("--verify", action="store_true", help="verify media, not App seed state")
    args = parser.parse_args()
    try:
        if args.verify:
            verify_fixture(args.output)
        else:
            if args.ffmpeg is None:
                parser.error("--ffmpeg is required for media generation")
            generate(args.output, args.ffmpeg)
    except (OSError, ValueError, subprocess.SubprocessError, KeyError, TypeError) as error:
        print(f"release fixture unavailable: {error}", file=sys.stderr)
        return 2
    print(str(args.output.resolve() / "fixture-manifest.json"))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
