mod help_build;

fn main() {
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let manual = manifest.join("../../htdocs/mimageviewer/manual");
    println!("cargo:rerun-if-changed={}", manual.display());
    println!("cargo:rerun-if-changed=help_build.rs");
    let pages =
        help_build::load_manual(&manual).unwrap_or_else(|error| panic!("bundled help: {error}"));
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(output.join("bundled_help.rs"), help_build::generate(&pages))
        .expect("write bundled help");
}
