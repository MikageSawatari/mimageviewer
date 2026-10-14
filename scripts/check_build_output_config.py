"""Reject Cargo output overrides before build wrappers perform any side effects.

Uses the same ancestor/CARGO_HOME config discovery and legacy-file precedence
as Cargo. TOML parsing also covers quoted/dotted keys, inline tables and includes.
No Cargo, compiler, process-control or signing tool is invoked by this check.
"""

import os
from pathlib import Path
import re
import sys

if sys.version_info < (3, 11):
    print("[build-output] Python 3.11 or newer is required to check fixed output paths.")
    sys.exit(1)

import tomllib


OUTPUT_ENV = frozenset({
    "CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR",
    "CARGO_BUILD_TARGET", "CARGO_BUILD_ARTIFACT_DIR", "CARGO_BUILD_OUT_DIR",
})
OUTPUT_OPTIONS = frozenset({
    "--target-dir", "--build-dir", "--target", "--profile", "--artifact-dir",
    "--out-dir", "--manifest-path",
})


class OutputOverride(ValueError):
    pass


def check_rustflags(flags):
    if isinstance(flags, str):
        flags = re.split(r"\s+|\x1f", flags)
    for index, flag in enumerate(flags):
        if (flag.startswith("-o")
                or flag.split("=", 1)[0] in {"--out-dir", "--crate-name"}
                or "extra-filename" in flag
                or (flag.startswith("--emit=") and "=" in flag[7:])
                or (flag == "--emit" and index + 1 < len(flags) and "=" in flags[index + 1])):
            raise OutputOverride("rustflags output option")


def check_config(config, base, seen):
    for key in ("target-dir", "build-dir", "target", "artifact-dir", "out-dir"):
        if key in config.get("build", {}):
            raise OutputOverride("Cargo config build." + key)
    for key, value in config.get("env", {}).items():
        if key.upper() in OUTPUT_ENV:
            raise OutputOverride("Cargo config env." + key.upper())
        if key.upper() in {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS"} or (
                key.upper().startswith("CARGO_TARGET_") and key.upper().endswith("_RUSTFLAGS")):
            check_rustflags(value.get("value", "") if isinstance(value, dict) else value)
    check_rustflags(config.get("build", {}).get("rustflags", []))
    for target in config.get("target", {}).values():
        if isinstance(target, dict):
            check_rustflags(target.get("rustflags", []))
    for include in config.get("include", []):
        path = include["path"] if isinstance(include, dict) else include
        optional = isinstance(include, dict) and include.get("optional", False)
        check_file(base / path, seen, optional=optional)


def check_file(path, seen, optional=False):
    path = path.resolve()
    if path in seen:
        return
    seen.add(path)
    if optional and not path.exists():
        return
    with path.open("rb") as source:
        check_config(tomllib.load(source), path.parent, seen)


def check_output(start, environ, args):
    for key in environ:
        if key.upper() in OUTPUT_ENV:
            # Reject even an override pointing at the usual target directory.
            raise OutputOverride("environment " + key.upper())
        if key.upper() in {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS"} or (
                key.upper().startswith("CARGO_TARGET_") and key.upper().endswith("_RUSTFLAGS")):
            check_rustflags(environ[key])

    seen = set()
    start = Path(start).resolve()
    cargo_home = Path(environ.get("CARGO_HOME") or Path.home() / ".cargo")
    for directory in [start / ".cargo", *(parent / ".cargo" for parent in start.parents), cargo_home]:
        legacy = directory / "config"
        modern = directory / "config.toml"
        if legacy.is_file():
            check_file(legacy, seen)
        elif modern.is_file():
            check_file(modern, seen)

    index = 0
    while index < len(args):
        option, separator, value = args[index].partition("=")
        if option in OUTPUT_OPTIONS:
            raise OutputOverride("Cargo argument " + option)
        if option.startswith("-C"):
            raise OutputOverride("Cargo argument -C (working directory)")
        if option == "-Z" or option.startswith("-Z"):
            # Unstable flags can introduce additional output/layout overrides.
            raise OutputOverride("Cargo argument -Z (unstable output layout)")
        if option == "--config":
            if not separator:
                index += 1
                value = args[index]
            if "=" in value:
                check_config(tomllib.loads(value), start, seen)
            else:
                check_file(start / value, seen)
        index += 1


def main():
    args = sys.argv[1:]
    start = args.pop(0)
    if args[:1] == ["--"]:
        args = args[1:]
    try:
        check_output(start, os.environ, args)
    except (OutputOverride, OSError, ValueError, TypeError, KeyError, IndexError, AttributeError) as error:
        # Do not print values, config contents or paths (which may be non-ASCII).
        reason = str(error) if isinstance(error, OutputOverride) else "Cargo config could not be checked"
        print("[build-output] " + reason + ": fixed output paths are required.")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
