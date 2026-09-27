//! The single list of camera RAW formats handled by LibRaw.

use std::path::Path;

pub const RAW_EXTENSIONS: &[&str] = &[
    "dng", "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef", "ptx",
    "rwl", "iiq", "crw", "srw",
];

pub fn is_raw_ext(extension: &str) -> bool {
    RAW_EXTENSIONS
        .iter()
        .any(|candidate| extension.eq_ignore_ascii_case(candidate))
}

pub fn is_raw_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(is_raw_ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_and_wic_extension_lists_partition_folder_support() {
        use crate::{folder_tree::SUPPORTED_EXTENSIONS, wic_decoder::WIC_SUPPORTED_EXTENSIONS};

        assert_eq!(RAW_EXTENSIONS.len(), 17);
        for &extension in RAW_EXTENSIONS {
            assert!(SUPPORTED_EXTENSIONS.contains(&extension), "{extension}");
            assert!(
                !WIC_SUPPORTED_EXTENSIONS.contains(&extension),
                "{extension}"
            );
            assert!(is_raw_ext(&extension.to_ascii_uppercase()));
        }
        for &extension in WIC_SUPPORTED_EXTENSIONS {
            assert!(SUPPORTED_EXTENSIONS.contains(&extension), "{extension}");
            assert!(!is_raw_ext(extension), "{extension}");
        }
        for &extension in SUPPORTED_EXTENSIONS {
            assert_eq!(
                is_raw_ext(extension),
                RAW_EXTENSIONS.contains(&extension),
                "{extension}"
            );
        }
    }
}
