fn main() {
    // Export undecorated names so the worker's GetProcAddress finds them.
    // See plugin.def for why.
    println!("cargo:rerun-if-changed=plugin.def");
    let def = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("plugin.def");
    println!("cargo:rustc-cdylib-link-arg=/DEF:{}", def.display());
}
