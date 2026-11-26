//! One integration test crate; run individual modules with cargo test and the full suite with nextest.

mod common;
#[path = "support/logical_bounds.rs"]
mod logical_bounds;
#[path = "support/gpu_test_group.rs"]
mod test_gpu_group;
#[path = "support/ui_harness.rs"]
mod test_ui;

// Keep the test files in alphabetical order. Shared support is declared only above.
mod archive_conversion_failure_diagnostics;
mod book_composition_editor_snapshots;
mod catalog_integration;
mod conceal_db_integration;
mod export_integration;
mod fts_tantivy_compat;
mod fts_tantivy_integrity;
mod grid_sort_integration;
mod local_adjust_core_parity;
mod mask_db_migration;
mod search_metadata_e2e;
mod search_name_e2e;
mod settings_integration;
mod sidecar_import;
mod sidecar_shape_migration;
mod susie_integration;
mod test_ai_upscale;
mod test_archive_convert;
mod test_tile_blend;
mod test_zip_enumerate;
mod ui_snapshot;
mod zip_integration;

#[test]
fn every_integration_test_file_is_declared_once() {
    use std::collections::{BTreeMap, BTreeSet};

    let test_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files = BTreeSet::new();
    for entry in std::fs::read_dir(&test_dir).expect("read tests directory") {
        let entry = entry.expect("read tests directory entry");
        let path = entry.path();
        if entry.file_type().expect("read test entry type").is_file()
            && path.extension().is_some_and(|ext| ext == "rs")
            && path
                .file_name()
                .is_some_and(|name| name != "integration.rs")
        {
            files.insert(
                path.file_stem()
                    .and_then(|name| name.to_str())
                    .expect("UTF-8 test file name")
                    .to_owned(),
            );
        }
    }

    // Read declarations from source, including cfg-disabled modules. Attributes
    // can be on a preceding line or on the declaration's own line.
    let declaration = regex::Regex::new(
        r"(?m)^[ \t]*(?:#\[[^\n]*\][ \t]*)*mod[ \t]+([A-Za-z_][A-Za-z0-9_]*)[ \t]*;",
    )
    .unwrap();
    let mut counts = BTreeMap::new();
    for capture in declaration.captures_iter(include_str!("integration.rs")) {
        let name = capture[1].to_owned();
        if matches!(
            name.as_str(),
            "test_gpu_group" | "test_ui" | "logical_bounds" | "common"
        ) {
            continue;
        }
        *counts.entry(name).or_insert(0usize) += 1;
    }
    let declared: BTreeSet<_> = counts.keys().cloned().collect();
    let undeclared: Vec<_> = files.difference(&declared).collect();
    let without_file: Vec<_> = declared.difference(&files).collect();
    let duplicates: Vec<_> = counts.iter().filter(|(_, count)| **count > 1).collect();
    assert!(
        undeclared.is_empty() && without_file.is_empty() && duplicates.is_empty(),
        "integration module declarations must match tests/*.rs exactly once: \
         files without declarations={undeclared:?}; declarations without files={without_file:?}; \
         duplicate declarations (name, count)={duplicates:?}"
    );
}
