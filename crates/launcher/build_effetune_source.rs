//! Approval gate for distribution inputs, shared with build-script regression tests.
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

#[path = "src/bundle_paths.rs"]
#[allow(dead_code)]
mod bundle_paths;

const RECOVER: &str = "Restore the complete approved EffeTune Mixwright v0.11.1 vendor source. Do not regenerate the tracked manifest to accept local changes.";

fn failure(message: impl std::fmt::Display) -> io::Error {
    io::Error::other(format!("EffeTune source approval: {message}. {RECOVER}"))
}

fn parse_manifest(text: &str) -> io::Result<BTreeMap<String, String>> {
    let mut entries = BTreeMap::new();
    let mut case_names = std::collections::BTreeSet::new();
    for line in text
        .lines()
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        let (hash, name) = line
            .split_once("  ")
            .ok_or_else(|| failure("invalid manifest line"))?;
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || name.contains('\\')
            || bundle_paths::relative_name(Path::new(name))? != name
            || !case_names.insert(name.to_lowercase())
        {
            return Err(failure("invalid/duplicate manifest entry"));
        }
        entries.insert(name.to_owned(), hash.to_owned());
    }
    if !entries.contains_key("VERSION") {
        return Err(failure("manifest must include VERSION"));
    }
    Ok(entries)
}

fn hashes(root: &Path) -> io::Result<BTreeMap<String, String>> {
    // Reject links in ancestors as well as in the complete source tree.
    for ancestor in root.ancestors() {
        bundle_paths::checked_metadata(ancestor)?;
    }
    let mut actual = BTreeMap::new();
    for (name, dir, path) in bundle_paths::inventory(root)? {
        if !dir {
            let hash = Sha256::digest(std::fs::read(path)?);
            actual.insert(name, hash.iter().map(|b| format!("{b:02x}")).collect());
        }
    }
    Ok(actual)
}

fn changed_files(
    approved: &BTreeMap<String, String>,
    actual: &BTreeMap<String, String>,
) -> io::Result<Vec<String>> {
    if approved.keys().ne(actual.keys()) {
        return Err(failure("missing or extra source files"));
    }
    Ok(approved
        .iter()
        .filter(|(name, hash)| actual.get(*name) != Some(*hash))
        .map(|(name, _)| name.clone())
        .collect())
}

pub fn validate(workspace: &Path, source: &Path) -> io::Result<()> {
    let manifest_path = workspace.join("third_party/effetune-mixwright/v0.11.1/manifest.sha256");
    println!("cargo:rerun-if-changed={}", manifest_path.display());
    let approved = parse_manifest(&std::fs::read_to_string(manifest_path)?)?;
    let original = workspace.join("vendor/effetune-mixwright");
    let original_hashes = hashes(&original)?;
    if !changed_files(&approved, &original_hashes)?.is_empty() {
        return Err(failure("approved vendor hash mismatch"));
    }
    for (_, _, path) in bundle_paths::inventory(&original)? {
        // Also watch original files when embedding a signed stage.
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-changed={}", original.display());
    let changed = changed_files(&approved, &hashes(source)?)?;
    if changed.is_empty() {
        return Ok(());
    }
    // A signed stage may change only the approved PE's signature fields/blob.
    // PowerShell verifies raw originals, unchanged resources, exact signing-only
    // byte delta, and authorized valid Authenticode. Signing certificates alone
    // never authorize an alternate binary. Arbitrary env sources cannot opt out.
    let validator = workspace.join("scripts/effetune-distribution.ps1");
    println!("cargo:rerun-if-changed={}", validator.display());
    println!(
        "cargo:rerun-if-changed={}",
        workspace.join("scripts/sign-files.ps1").display()
    );
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let output = std::process::Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(validator)
            .arg("-ValidateSource")
            .arg(source)
            .arg("-WorkspaceRoot")
            .arg(workspace)
            .creation_flags(0x08000000)
            .output()?;
        if !output.status.success() {
            return Err(failure(format!(
                "signed stage rejected: {}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    Err(failure(
        "modified signed stages require Windows Authenticode validation",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn approval_rejects_missing_extra_changed_and_duplicate_paths() {
        let hash = "0".repeat(64);
        let manifest = format!("{hash}  VERSION\n{hash}  bundle/file.js\n");
        let approved = parse_manifest(&manifest).unwrap();
        let mut actual = approved.clone();
        actual.remove("bundle/file.js");
        assert!(changed_files(&approved, &actual).is_err());
        actual = approved.clone();
        actual.insert("extra.js".into(), hash.clone());
        assert!(changed_files(&approved, &actual).is_err());
        actual = approved.clone();
        actual.insert("bundle/file.js".into(), "1".repeat(64));
        assert_eq!(
            changed_files(&approved, &actual).unwrap(),
            ["bundle/file.js"]
        );
        assert!(parse_manifest(&format!("{manifest}{hash}  version\n")).is_err());
        assert!(parse_manifest(&format!("{hash}  ../escape\n")).is_err());
    }

    #[test]
    fn approved_vendor_snapshot_matches_all_408_raw_files() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let source = workspace.join("vendor/effetune-mixwright");
        if !source.exists() {
            return;
        }
        let manifest = parse_manifest(
            &std::fs::read_to_string(
                workspace.join("third_party/effetune-mixwright/v0.11.1/manifest.sha256"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.len(), 408);
        assert!(
            changed_files(&manifest, &hashes(&source).unwrap())
                .unwrap()
                .is_empty()
        );
    }
}
