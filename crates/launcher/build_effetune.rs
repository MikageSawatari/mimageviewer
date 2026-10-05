use sha2::{Digest, Sha256};
use std::path::Path;

#[path = "src/bundle_paths.rs"]
#[allow(dead_code)]
mod bundle_paths;

#[path = "build_effetune_source.rs"]
mod source_approval;

pub fn generate(workspace: &Path) {
    println!("cargo:rerun-if-env-changed=MIMV_EFFETUNE_DIR");
    println!("cargo:rerun-if-env-changed=MIV_SIGN_SHA1");
    println!("cargo:rerun-if-env-changed=MIV_SIGN_SUBJECT");
    let source = std::env::var_os("MIMV_EFFETUNE_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| workspace.join("vendor/effetune-mixwright"));
    let source = if source.is_absolute() {
        source
    } else {
        workspace.join(source)
    };
    let recover = "Restore vendor/effetune-mixwright/VERSION and the complete EffeTune Mixwright.vst3 bundle (v0.12.0); or run scripts/build-release.ps1 to stage it. MIMV_EFFETUNE_DIR must point to a root containing VERSION and the bundle.";
    source_approval::validate(workspace, &source).unwrap_or_else(|e| panic!("{e}. {recover}"));
    bundle_paths::checked_metadata(&source)
        .unwrap_or_else(|e| panic!("EffeTune source {}: {e}. {recover}", source.display()));
    let version_file = source.join("VERSION");
    bundle_paths::checked_metadata(&version_file)
        .unwrap_or_else(|e| panic!("EffeTune VERSION: {e}. {recover}"));
    let version = std::fs::read_to_string(&version_file)
        .unwrap_or_else(|e| panic!("EffeTune VERSION: {e}. {recover}"));
    let version = version.trim();
    assert!(
        !version.is_empty() && !version.contains(['\n', '\r', '\t']),
        "Invalid EffeTune VERSION. {recover}"
    );
    let bundle = source.join("EffeTune Mixwright.vst3");
    let inventory = bundle_paths::inventory(&bundle)
        .unwrap_or_else(|e| panic!("EffeTune bundle: {e}. {recover}"));
    assert!(!inventory.is_empty(), "Empty EffeTune bundle. {recover}");
    assert!(
        bundle
            .join("Contents/x86_64-win/EffeTune Mixwright.vst3")
            .is_file(),
        "Missing EffeTune plugin PE. {recover}"
    );
    println!("cargo:rerun-if-changed={}", version_file.display());
    // Watching every directory discovers added/removed files as well as modifications.
    println!("cargo:rerun-if-changed={}", bundle.display());
    let mut output = String::from("static EFFETUNE_FILES: &[effetune_bundle::BundleFile] = &[\n");
    let version_hash = crate::sha256_file_hex(&version_file);
    output.push_str(&format!("effetune_bundle::BundleFile {{ name: \"VERSION\", bytes: include_bytes!({:?}), hash: {version_hash:?} }},\n", version_file.to_str().unwrap()));
    let mut manifest = Sha256::new();
    manifest.update(version.as_bytes());
    manifest.update(version_hash.as_bytes());
    for (relative, dir, path) in inventory {
        println!("cargo:rerun-if-changed={}", path.display());
        if dir {
            continue;
        }
        let name = format!("EffeTune Mixwright.vst3/{relative}");
        let hash = crate::sha256_file_hex(&path);
        manifest.update(format!(
            "\n{name}\t{}\t{hash}",
            std::fs::metadata(&path).unwrap().len()
        ));
        output.push_str(&format!("effetune_bundle::BundleFile {{ name: {name:?}, bytes: include_bytes!({:?}), hash: {hash:?} }},\n", path.to_str().unwrap()));
    }
    output.push_str("];\n");
    output.push_str(&format!(
        "const EFFETUNE_MANIFEST: &str = {:?};\n",
        format!("{version}:{}", crate::hex_lower(&manifest.finalize()))
    ));
    output.push_str(&format!("const EFFETUNE_VERSION: &str = {version:?};\n"));
    let out =
        std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("effetune_files.rs");
    std::fs::write(out, output).unwrap();
}
