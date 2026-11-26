//! One integration test crate; run individual modules with cargo test and the full suite with nextest.

mod common;
#[path = "support/logical_bounds.rs"]
mod logical_bounds;
#[path = "support/gpu_test_group.rs"]
mod test_gpu_group;
#[path = "support/ui_harness.rs"]
mod test_ui;

// Expand at crate root so each `mod name;` resolves beside integration.rs.
// One input owns both the actual modules and the inventory checked below.
macro_rules! integration_modules {
    ($($name:ident),* $(,)?) => {
        $(mod $name;)*
        const INTEGRATION_MODULES: &[&str] = &[$(stringify!($name)),*];
    };
}

// Keep the test files in alphabetical order. Shared support is declared only above.
integration_modules!(
    archive_conversion_failure_diagnostics,
    book_composition_editor_snapshots,
    catalog_integration,
    conceal_db_integration,
    export_integration,
    fts_tantivy_compat,
    fts_tantivy_integrity,
    grid_sort_integration,
    local_adjust_core_parity,
    mask_db_migration,
    search_metadata_e2e,
    search_name_e2e,
    settings_integration,
    sidecar_import,
    sidecar_shape_migration,
    susie_integration,
    test_ai_upscale,
    test_archive_convert,
    test_tile_blend,
    test_zip_enumerate,
    ui_snapshot,
    zip_integration,
);

#[test]
fn every_integration_test_file_is_declared_once() {
    use std::collections::{BTreeMap, BTreeSet};

    let test_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let mut files = BTreeSet::new();
    let mut directory_entrypoints = BTreeSet::new();
    for entry in std::fs::read_dir(&test_dir).expect("read tests directory") {
        let entry = entry.expect("read tests directory entry");
        let path = entry.path();
        let file_type = entry.file_type().expect("read test entry type");
        if file_type.is_dir() && path.join("main.rs").is_file() {
            directory_entrypoints.insert(path.join("main.rs"));
        }
        if file_type.is_file()
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

    let mut counts = BTreeMap::new();
    for &name in INTEGRATION_MODULES {
        *counts.entry(name.to_owned()).or_insert(0usize) += 1;
    }
    let declared: BTreeSet<_> = counts.keys().cloned().collect();
    let undeclared: Vec<_> = files.difference(&declared).collect();
    let without_file: Vec<_> = declared.difference(&files).collect();
    let duplicates: Vec<_> = counts.iter().filter(|(_, count)| **count > 1).collect();
    assert!(
        undeclared.is_empty()
            && without_file.is_empty()
            && duplicates.is_empty()
            && directory_entrypoints.is_empty(),
        "integration module declarations must match tests/*.rs exactly once: \
         files without declarations={undeclared:?}; declarations without files={without_file:?}; \
         duplicate declarations (name, count)={duplicates:?}; \
         unsupported tests/<name>/main.rs entrypoints={directory_entrypoints:?}. \
         Put directory-style tests in tests/<name>.rs and register them in integration."
    );
}
