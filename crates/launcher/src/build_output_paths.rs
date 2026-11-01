//! Locate launcher inputs from Cargo's actual build-script output directory.
//! Ambient CARGO_TARGET_DIR is deliberately not consulted: Cargo [env] can
//! override it for child processes without moving Cargo's own build outputs.

use std::path::{Component, Path, PathBuf};

pub fn profile_dir_from_out_dir(out_dir: &Path) -> Result<&Path, &'static str> {
    if !out_dir.is_absolute()
        || out_dir
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err("OUT_DIR must be an absolute path without parent components");
    }
    if out_dir.file_name() != Some("out".as_ref()) {
        return Err("OUT_DIR must end with /build/mimageviewer-launcher-<hash>/out");
    }
    let package_dir = out_dir.parent().ok_or("OUT_DIR has no package directory")?;
    let hash = package_dir
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("mimageviewer-launcher-"))
        .ok_or("OUT_DIR package directory is not mimageviewer-launcher-<hash>")?;
    if hash.is_empty() || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("OUT_DIR launcher package hash is missing or invalid");
    }
    let build_dir = package_dir
        .parent()
        .ok_or("OUT_DIR has no build directory")?;
    if build_dir.file_name() != Some("build".as_ref()) {
        return Err("OUT_DIR package directory must be immediately inside build");
    }
    let profile_dir = build_dir
        .parent()
        .ok_or("OUT_DIR has no profile directory")?;
    if profile_dir.file_name().is_none() {
        return Err("OUT_DIR has no named profile directory");
    }
    Ok(profile_dir)
}

pub fn release_inputs_dir_from_out_dir(out_dir: &Path) -> Result<PathBuf, &'static str> {
    let profile_dir = profile_dir_from_out_dir(out_dir)?;
    // The launcher, including its debug-profile tests, embeds release companions.
    // Preserve that prerequisite while taking the target root only from OUT_DIR.
    let target_dir = profile_dir
        .parent()
        .ok_or("OUT_DIR has no target directory")?;
    Ok(target_dir.join("release"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        std::env::current_dir().unwrap().join("fake-launcher-build")
    }

    fn out_dir(target: &Path, profile: &str) -> PathBuf {
        target
            .join(profile)
            .join("build/mimageviewer-launcher-1234abcd/out")
    }

    #[test]
    fn derives_actual_release_profile_and_inputs() {
        let target = root().join("target");
        let out = out_dir(&target, "release");
        assert_eq!(
            profile_dir_from_out_dir(&out).unwrap(),
            target.join("release")
        );
        assert_eq!(
            release_inputs_dir_from_out_dir(&out).unwrap(),
            target.join("release")
        );
    }

    #[test]
    fn alternate_target_is_taken_from_out_dir_without_default_guessing() {
        let target = root().join("custom-output");
        let out = out_dir(&target, "release");
        assert_eq!(
            release_inputs_dir_from_out_dir(&out).unwrap(),
            target.join("release")
        );
    }

    #[test]
    fn debug_and_custom_profiles_keep_release_companion_prerequisite() {
        let target = root().join("custom-output");
        for profile in ["debug", "dev-runtime"] {
            let out = out_dir(&target, profile);
            assert_eq!(
                profile_dir_from_out_dir(&out).unwrap(),
                target.join(profile)
            );
            assert_eq!(
                release_inputs_dir_from_out_dir(&out).unwrap(),
                target.join("release")
            );
        }
    }

    #[test]
    fn cross_target_profile_retains_the_actual_triple_directory() {
        let target = root().join("target/x86_64-pc-windows-msvc");
        assert_eq!(
            release_inputs_dir_from_out_dir(&out_dir(&target, "release")).unwrap(),
            target.join("release")
        );
    }

    #[test]
    fn malformed_out_dir_is_rejected_without_a_fallback() {
        let target = root().join("target");
        for out in [
            PathBuf::from("target/release/build/mimageviewer-launcher-1234abcd/out"),
            target.join("release/build/mimageviewer-launcher-1234abcd/other"),
            target.join("release/other/mimageviewer-launcher-1234abcd/out"),
            target.join("release/build/other-package-1234abcd/out"),
            target.join("release/build/mimageviewer-launcher-/out"),
            target.join("release/build/mimageviewer-launcher-not-a-hash/out"),
            target.join("release/../release/build/mimageviewer-launcher-1234abcd/out"),
        ] {
            assert!(
                release_inputs_dir_from_out_dir(&out).is_err(),
                "accepted {}",
                out.display()
            );
        }
    }
}
