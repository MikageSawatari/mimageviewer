use std::path::PathBuf;

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        println!("cargo:rustc-env=MIV_LIBAVIF_BUILD_ID=not linked (Windows only)");
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/libavif");
    let recovery = "Recovery: bash scripts/setup-libavif.sh (or bash scripts/bootstrap-vendor.sh)";
    let version_path = root.join("VERSION");
    println!("cargo:rerun-if-changed={}", version_path.display());
    let version = std::fs::read_to_string(version_path)
        .unwrap_or_else(|error| panic!("Cannot read vendor/libavif/VERSION: {error}\n{recovery}"));
    assert_eq!(
        version.trim(),
        "libavif=1.4.2 dav1d=1.5.3 libyuv=644251f252a84bf8ce91ff0aca86a9b16b069ab8",
        "Unexpected libavif build version\n{recovery}"
    );
    println!("cargo:rustc-env=MIV_LIBAVIF_BUILD_ID={}", version.trim());
    for file in [
        "include/avif/avif.h",
        "lib/avif.lib",
        "lib/dav1d.lib",
        "lib/yuv.lib",
    ] {
        let path = root.join(file);
        assert!(path.is_file(), "Missing {}\n{recovery}", path.display());
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-changed=shim/miv_avif.c");
    cc::Build::new()
        .include(root.join("include"))
        .define("AVIF_STATIC", None)
        .static_crt(true)
        .file("shim/miv_avif.c")
        .compile("miv_avif");
    println!(
        "cargo:rustc-link-search=native={}",
        root.join("lib").display()
    );
    for name in ["avif", "dav1d", "yuv"] {
        println!("cargo:rustc-link-lib=static={name}");
    }
}
