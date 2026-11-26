use super::*;
use crate::image_combine::{CombineFormat, CombineItemSpec, CombineOptions};

pub(crate) fn recipe(root: &Path) -> CombineRecipe {
    CombineRecipe {
        v: 1,
        options: CombineOptions::default(),
        format: CombineFormat::Png,
        items: vec![
            CombineItemSpec {
                annotation_space: None,
                source: CombineSourceRef::File {
                    path: root.join("first.png"),
                },
                label_override: Some("商品 A".into()),
            },
            CombineItemSpec {
                annotation_space: None,
                source: CombineSourceRef::ZipEntry {
                    zip_path: root.join("book.zip"),
                    entry_name: "章/2.png".into(),
                },
                label_override: None,
            },
            CombineItemSpec {
                annotation_space: None,
                source: CombineSourceRef::PdfPage {
                    pdf_path: root.join("book.pdf"),
                    page_num: 3,
                },
                label_override: Some(String::new()),
            },
        ],
    }
}

#[test]
fn image_combine_recipe_roundtrip_upsert_and_key_only_index() {
    let data = tempfile::tempdir().unwrap();
    let output = data.path().join("joined.png");
    let mut recipe = recipe(data.path());
    recipe.items[2].annotation_space = Some([1600, 2400]);
    save_recipe(data.path(), &output, &recipe).unwrap();
    assert_eq!(
        load_recipe(data.path(), &output).unwrap(),
        Some(recipe.clone())
    );
    recipe.items.reverse();
    save_recipe(data.path(), &output, &recipe).unwrap();
    assert_eq!(load_recipe(data.path(), &output).unwrap(), Some(recipe));
    let keys = load_keys(data.path()).unwrap();
    assert_eq!(
        keys,
        HashSet::from([crate::adjustment_db::normalize_path(&output)])
    );
    let connection = connection(&data.path().join("combine.db"), false).unwrap();
    let (count, updated): (i64, i64) = connection
        .query_row(
            "SELECT COUNT(*), MAX(updated_at) FROM combine_recipes",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(count, 1);
    assert!(updated > 0);
}

#[test]
fn image_combine_recipe_annotation_space_defaults_without_migration_and_rejects_zero() {
    let data = tempfile::tempdir().unwrap();
    let mut recipe = recipe(data.path());
    let mut json = serde_json::to_value(&recipe).unwrap();
    for item in json["items"].as_array_mut().unwrap() {
        item.as_object_mut().unwrap().remove("annotation_space");
    }
    let decoded: CombineRecipe = serde_json::from_value(json).unwrap();
    assert_eq!(decoded, recipe);
    recipe.items[0].annotation_space = Some([0, 100]);
    assert!(save_recipe(data.path(), &data.path().join("output.png"), &recipe).is_err());
}

#[test]
fn image_combine_recipe_paths_absolute_and_missing_store_read_does_not_create() {
    let data = tempfile::tempdir().unwrap();
    assert!(
        load_recipe(data.path(), &data.path().join("none.png"))
            .unwrap()
            .is_none()
    );
    assert!(load_keys(data.path()).unwrap().is_empty());
    assert!(!data.path().join("combine.db").exists());
    let mut recipe = recipe(data.path());
    recipe.items[0].source = CombineSourceRef::File {
        path: "relative.png".into(),
    };
    let output = data.path().join("joined.png");
    save_recipe(data.path(), &output, &recipe).unwrap();
    let stored = load_recipe(data.path(), &output).unwrap().unwrap();
    assert!(source_path(&stored.items[0].source).is_absolute());
}

#[test]
fn image_combine_recipe_unknown_version_and_corruption_leave_original_rows() {
    let data = tempfile::tempdir().unwrap();
    initialize_database_at(&data.path().join("combine.db")).unwrap();
    let connection = connection(&data.path().join("combine.db"), true).unwrap();
    let output = data.path().join("joined.png");
    let key = crate::adjustment_db::normalize_path(&output);
    let mut unsupported = recipe(data.path());
    unsupported.v = 99;
    for json in [
        serde_json::to_string(&unsupported).unwrap(),
        "broken JSON".into(),
    ] {
        connection
            .execute(
                "INSERT OR REPLACE INTO combine_recipes VALUES (?1, ?2, 10)",
                rusqlite::params![key, json],
            )
            .unwrap();
        assert!(load_recipe(data.path(), &output).is_err());
        let preserved: String = connection
            .query_row(
                "SELECT recipe FROM combine_recipes WHERE output_key=?1",
                [&key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved, json);
    }
}

#[test]
fn image_combine_index_delayed_old_results_never_replace_latest_or_failed_snapshot() {
    let mut index = CombineIndex::default();
    assert!(index.contains_or_unloaded("anything"));
    index.generation = 2;
    index
        .tx
        .send((2, Ok(HashSet::from(["new".into()]))))
        .unwrap();
    index
        .tx
        .send((1, Ok(HashSet::from(["old".into()]))))
        .unwrap();
    while let Ok((generation, result)) = index.rx.try_recv() {
        index.apply(generation, result);
    }
    assert!(index.contains_or_unloaded("new"));
    assert!(!index.contains_or_unloaded("old"));
    index.apply(2, Err("read failed".into()));
    assert!(index.contains_or_unloaded("new"));
    assert!(!index.contains_or_unloaded("absent"));
}

#[test]
fn image_combine_index_book_copy_effect_reloads_loaded_keys() {
    let data = tempfile::tempdir().unwrap();
    let source = data.path().join("original.png");
    let destination = data.path().join("copy.png");
    save_recipe(data.path(), &source, &recipe(data.path())).unwrap();
    let mut index = CombineIndex::default();
    index.apply(0, load_keys(data.path()));
    let destination_key = crate::adjustment_db::normalize_path(&destination);
    assert!(!index.contains_or_unloaded(&destination_key));
    let guard = crate::rename_key_migration::acquire_store_maintenance(data.path(), &[]).unwrap();
    let report = crate::rename_key_migration::migrate_book_paths_at(
        data.path(),
        &[],
        &[(source, destination)],
        &guard,
    );
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(report.store_mutations.combine_index_stale());
    index.mark_stale();
    assert!(index.stale);
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !index.contains_or_unloaded(&destination_key) {
        index.poll(data.path(), &ctx);
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(index.contains_or_unloaded(&destination_key));
}
