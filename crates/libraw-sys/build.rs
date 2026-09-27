use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vendor/libraw");
    let makefile = root.join("Makefile.msvc");
    let zlib = root.join("zlib");
    if !makefile.is_file() || !zlib.join("zlib.h").is_file() {
        panic!(
            "\n+------------------------------------------------------------\n+LibRaw 0.22.2 source is missing from vendor/libraw/.\n+Recovery: bash scripts/setup-libraw.sh\n+Or restore all vendor assets: bash scripts/bootstrap-vendor.sh\n+------------------------------------------------------------"
        );
    }
    println!("cargo:rerun-if-changed={}", makefile.display());
    println!("cargo:rerun-if-changed={}", root.join("VERSION").display());
    println!("cargo:rerun-if-changed=shim/miv_libraw.cpp");
    println!("cargo:rerun-if-changed=shim/miv_libraw.h");
    println!("cargo:rerun-if-changed={}", root.join("libraw").display());
    println!("cargo:rerun-if-changed={}", root.join("src").display());
    println!("cargo:rerun-if-changed={}", zlib.display());
    let version = std::fs::read_to_string(root.join("VERSION")).expect("LibRaw VERSION");
    assert_eq!(version.trim(), "0.22.2", "Unexpected LibRaw source version");

    let mut z = cc::Build::new();
    z.include(&zlib);
    for name in [
        "adler32", "compress", "crc32", "deflate", "gzclose", "gzlib", "gzread", "gzwrite",
        "infback", "inffast", "inflate", "inftrees", "trees", "uncompr", "zutil",
    ] {
        let file = zlib.join(format!("{name}.c"));
        println!("cargo:rerun-if-changed={}", file.display());
        z.file(file);
    }
    z.compile("miv_zlib");

    let content = std::fs::read_to_string(&makefile).expect("read Makefile.msvc");
    let object_block = content
        .split_once("LIB_OBJECTS=")
        .and_then(|(_, rest)| rest.split_once("DLL_OBJECTS="))
        .map(|(block, _)| block)
        .expect("Makefile.msvc LIB_OBJECTS block");
    let objects: Vec<String> = object_block
        .split_whitespace()
        .filter_map(|token| {
            let token = token.trim_end_matches('\\').replace('\\', "/");
            token.ends_with("_st.obj").then_some(token)
        })
        .collect();
    assert!(objects.len() > 70, "LibRaw source list unexpectedly short");
    let rules: HashMap<String, String> = content
        .lines()
        .filter_map(|line| line.split_once(':'))
        .filter_map(|(target, source)| {
            let target = target.trim().replace('\\', "/");
            let source = source.trim().replace('\\', "/");
            (target.ends_with("_st.obj") && source.ends_with(".cpp")).then_some((target, source))
        })
        .collect();
    let mut cpp = cc::Build::new();
    cpp.cpp(true)
        .include(&root)
        .include(&zlib)
        .define("LIBRAW_NODLL", None)
        .define("LIBRAW_BUILDLIB", None)
        .define("USE_ZLIB", None)
        .define("USE_JPEG", None)
        .flag_if_supported("/EHsc");
    let jpeg_include = std::env::var("DEP_TURBOJPEG_INCLUDE")
        .expect("turbojpeg-sys must provide libjpeg-turbo headers");
    for path in jpeg_include.split(',') {
        cpp.include(Path::new(path));
    }
    let jpeg_header_dir = Path::new(jpeg_include.split(',').next().expect("JPEG include path"));
    let jpeg_lib_dir = jpeg_header_dir
        .parent()
        .expect("JPEG install root")
        .join("lib");
    assert!(jpeg_lib_dir.join("turbojpeg-static.lib").is_file());
    for object in objects {
        let source = rules
            .get(&object)
            .unwrap_or_else(|| panic!("No Makefile.msvc source rule for {object}"));
        let file = root.join(source);
        assert!(file.is_file(), "LibRaw source missing: {}", file.display());
        println!("cargo:rerun-if-changed={}", file.display());
        cpp.file(file);
    }
    cpp.file("shim/miv_libraw.cpp");
    cpp.compile("miv_libraw");
    // turbojpeg-static contains the libjpeg API as well as TurboJPEG. Reuse
    // this one archive; linking jpeg-static too would duplicate JPEG objects.
    println!("cargo:rustc-link-search=native={}", jpeg_lib_dir.display());
    println!("cargo:rustc-link-lib=static=turbojpeg-static");
}
