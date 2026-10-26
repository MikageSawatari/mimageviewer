// Distribution identity. Rust includes this file; PowerShell/Python read these
// simple string constants. Keep literals on one line and do not add escapes.
#![allow(dead_code)]
pub const VERSION: &str = "0.15.0-miv.1";
pub const BUNDLE_NAME: &str = "mIV Sound.vst3";
pub const SOURCE_ROOT: &str = "vendor/miv-sound";
pub const NOTICES_ROOT: &str = "third_party/miv-sound";
pub const UPSTREAM_TAG: &str = "v0.15.0";
// The public manual stays on its existing snapshot until F3.
pub const DOCS_VERSION: &str = "v0.14.0";
