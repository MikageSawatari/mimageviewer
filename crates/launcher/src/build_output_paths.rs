//! Locate profile directories and launcher inputs from Cargo's actual OUT_DIR.
//! Ambient CARGO_TARGET_DIR is deliberately not consulted: Cargo [env] can
//! override it for child processes without moving Cargo's own build outputs.

use std::path::{Component, Path, PathBuf};

pub fn profile_dir_from_out_dir<'a>(out_dir: &'a Path, package: &str) -> Result<&'a Path, String> {
    let invalid_layout = || {
        format!(
            "OUT_DIR must be an absolute path without parent components matching \
             <profile_dir>/build/{package}/<nonempty ASCII hex hash>/out; \
             Cargo's build-dir layout may have changed"
        )
    };
    if !out_dir.is_absolute()
        || out_dir
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(invalid_layout());
    }
    if out_dir.file_name() != Some("out".as_ref()) {
        return Err(invalid_layout());
    }
    let hash_dir = out_dir.parent().ok_or_else(invalid_layout)?;
    let hash = hash_dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(invalid_layout)?;
    if hash.is_empty() || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(invalid_layout());
    }
    let package_dir = hash_dir.parent().ok_or_else(invalid_layout)?;
    if package_dir.file_name() != Some(package.as_ref()) {
        return Err(invalid_layout());
    }
    let build_dir = package_dir.parent().ok_or_else(invalid_layout)?;
    if build_dir.file_name() != Some("build".as_ref()) {
        return Err(invalid_layout());
    }
    let profile_dir = build_dir.parent().ok_or_else(invalid_layout)?;
    if profile_dir.file_name().is_none() {
        return Err(invalid_layout());
    }
    Ok(profile_dir)
}

// The main package's build script shares only the profile-directory helper.
#[allow(dead_code)]
pub fn release_inputs_dir_from_out_dir(out_dir: &Path, package: &str) -> Result<PathBuf, String> {
    let profile_dir = profile_dir_from_out_dir(out_dir, package)?;
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
            .join("build/mimageviewer-launcher/1234abcd/out")
    }

    #[test]
    fn derives_actual_release_profile_and_inputs() {
        let target = root().join("target");
        let out = out_dir(&target, "release");
        assert_eq!(
            profile_dir_from_out_dir(&out, "mimageviewer-launcher").unwrap(),
            target.join("release")
        );
        assert_eq!(
            release_inputs_dir_from_out_dir(&out, "mimageviewer-launcher").unwrap(),
            target.join("release")
        );
    }

    #[test]
    fn alternate_target_is_taken_from_out_dir_without_default_guessing() {
        let target = root().join("custom-output");
        let out = out_dir(&target, "release");
        assert_eq!(
            release_inputs_dir_from_out_dir(&out, "mimageviewer-launcher").unwrap(),
            target.join("release")
        );
    }

    #[test]
    fn debug_and_custom_profiles_keep_release_companion_prerequisite() {
        let target = root().join("custom-output");
        for profile in ["debug", "dev-runtime"] {
            let out = out_dir(&target, profile);
            assert_eq!(
                profile_dir_from_out_dir(&out, "mimageviewer-launcher").unwrap(),
                target.join(profile)
            );
            assert_eq!(
                release_inputs_dir_from_out_dir(&out, "mimageviewer-launcher").unwrap(),
                target.join("release")
            );
        }
    }

    #[test]
    fn cross_target_profile_retains_the_actual_triple_directory() {
        let target = root().join("target/x86_64-pc-windows-msvc");
        assert_eq!(
            release_inputs_dir_from_out_dir(&out_dir(&target, "release"), "mimageviewer-launcher",)
                .unwrap(),
            target.join("release")
        );
    }

    #[test]
    fn main_package_and_nonempty_ascii_hex_hashes_are_supported() {
        let target = root().join("target");
        for profile in ["debug", "dev-runtime", "release"] {
            for hash in ["a", "30505735908c35d5", "ABCDEF0123456789"] {
                let out = target
                    .join(profile)
                    .join("build/mimageviewer")
                    .join(hash)
                    .join("out");
                assert_eq!(
                    profile_dir_from_out_dir(&out, "mimageviewer").unwrap(),
                    target.join(profile)
                );
                assert!(profile_dir_from_out_dir(&out, "mimageviewer-launcher").is_err());
                let launcher_out = out_dir(&target, profile);
                assert!(profile_dir_from_out_dir(&launcher_out, "mimageviewer").is_err());
            }
        }
    }

    #[test]
    fn malformed_out_dir_is_rejected_without_a_fallback() {
        let target = root().join("target");
        for out in [
            PathBuf::from("target/release/build/mimageviewer-launcher/1234abcd/out"),
            target.join("release/build/mimageviewer-launcher/1234abcd/other"),
            target.join("release/other/mimageviewer-launcher/1234abcd/out"),
            target.join("release/build/other-package/1234abcd/out"),
            target.join("release/build/mimageviewer-launcher/out"),
            target.join("release/build/mimageviewer-launcher//out"),
            target.join("release/build/mimageviewer-launcher/not-a-hash/out"),
            target.join("release/build/mimageviewer-launcher/abcdg/out"),
            target.join("release/build/mimageviewer-launcher/\u{ff11}\u{ff12}/out"),
            target.join("release/build/mimageviewer-launcher-1234abcd/out"),
            target.join("release/../release/build/mimageviewer-launcher/1234abcd/out"),
        ] {
            let error = release_inputs_dir_from_out_dir(&out, "mimageviewer-launcher")
                .expect_err(&format!("accepted {}", out.display()));
            assert!(error.contains("<profile_dir>/build/mimageviewer-launcher/"));
            assert!(error.contains("Cargo's build-dir layout may have changed"));
        }
    }
}
