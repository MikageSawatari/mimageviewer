"""Check workspace GUI consumers' declared MSRV against the vendored egui stack.

This checks declarations even when the installed compiler is newer than every
dependency, so cargo check alone would accept an incorrectly low package MSRV.
"""

from pathlib import Path
import tomllib


def version(value):
    return tuple(map(int, value.split(".")))


def main():
    root = Path(__file__).resolve().parent.parent
    workspace = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))
    inherited = workspace["workspace"]["package"]["rust-version"]
    gui = {"egui", "eframe", "egui-winit", "egui-wgpu"}
    required = max(
        version(tomllib.loads((root / "vendor" / name / "Cargo.toml").read_text(encoding="utf-8"))["package"]["rust-version"])
        for name in gui
    )
    checked = 0
    for member in [".", *workspace["workspace"]["members"]]:
        manifest = root / member / "Cargo.toml"
        package = tomllib.loads(manifest.read_text(encoding="utf-8"))
        dependencies = set(package.get("dependencies", {}))
        dependencies.update(package.get("dev-dependencies", {}))
        if not dependencies.intersection(gui):
            continue
        declared = package["package"]["rust-version"]
        effective = inherited if isinstance(declared, dict) and declared.get("workspace") else declared
        assert version(effective) >= required, (
            f"{member}: declared Rust {effective}, vendored GUI dependencies require {required}"
        )
        checked += 1
        print(f"{package['package']['name']}: Rust {effective}")
    assert checked > 0, "no GUI consumer selected"
    print(f"{checked} GUI consumer MSRV declarations passed")


if __name__ == "__main__":
    main()
