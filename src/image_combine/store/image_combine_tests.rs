use super::*;
use crate::image_combine::{CombineFormat, CombineItemSpec, CombineOptions};

pub(crate) fn recipe(root: &Path) -> CombineRecipe {
    CombineRecipe {
        v: 1,
        options: CombineOptions::default(),
        format: CombineFormat::Png,
        items: vec![
            CombineItemSpec {
                composition_identity: None,
                annotation_space: None,
                source: CombineSourceRef::File {
                    path: root.join("first.png"),
                },
                label_override: Some("商品 A".into()),
            },
            CombineItemSpec {
                composition_identity: None,
                annotation_space: None,
                source: CombineSourceRef::ZipEntry {
                    zip_path: root.join("book.zip"),
                    entry_name: "章/2.png".into(),
                },
                label_override: None,
            },
            CombineItemSpec {
                composition_identity: None,
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
    recipe.options.background_color = [12, 34, 56];
    recipe.options.transparent_background = true;
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
fn image_combine_recipe_requires_composition_identity_field_on_every_item() {
    let data = tempfile::tempdir().unwrap();
    let output = data.path().join("combined.png");
    let recipe = recipe(data.path());
    save_recipe(data.path(), &output, &recipe).unwrap();
    let connection = connection(&data.path().join("combine.db"), true).unwrap();
    let key = crate::adjustment_db::normalize_path(&output);
    for missing in 0..recipe.items.len() {
        let mut json = serde_json::to_value(&recipe).unwrap();
        json["items"][missing]
            .as_object_mut()
            .unwrap()
            .remove("composition_identity");
        let json = serde_json::to_string(&json).unwrap();
        connection
            .execute(
                "UPDATE combine_recipes SET recipe = ?1 WHERE output_key = ?2",
                rusqlite::params![json, key],
            )
            .unwrap();
        let error = load_recipe(data.path(), &output).unwrap_err();
        assert!(error.contains("composition_identity"), "{error}");
        let preserved: String = connection
            .query_row(
                "SELECT recipe FROM combine_recipes WHERE output_key = ?1",
                [&key],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved, json);
    }
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
    index.reload = IndexReloadState::Loading;
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
        assert!(index.apply(generation, result).is_none());
    }
    assert!(index.contains_or_unloaded("new"));
    assert!(!index.contains_or_unloaded("old"));
    index.generation = 3;
    index.reload = IndexReloadState::Loading;
    assert_eq!(
        index.apply(3, Err("read failed".into())),
        Some("read failed".into())
    );
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
    index.set_keys_for_test(load_keys(data.path()).unwrap());
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
    assert!(index.reload == IndexReloadState::Stale);
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !index.contains_or_unloaded(&destination_key) {
        assert!(index.poll(data.path(), &ctx).is_none());
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(index.contains_or_unloaded(&destination_key));
}

#[test]
fn image_combine_index_read_failure_preserves_keys_notifies_once_and_does_not_retry() {
    let data = tempfile::tempdir().unwrap();
    let database = data.path().join("combine.db");
    let corrupt_bytes = b"not a SQLite database";
    std::fs::write(&database, corrupt_bytes).unwrap();
    let ctx = egui::Context::default();
    let mut index = CombineIndex::default();
    index.set_keys_for_test(HashSet::from(["preserved".into()]));

    // Run the exact read job inline to avoid scheduling/timing dependencies in this test.
    let mut job = None;
    index.request_reload_with(data.path().to_owned(), ctx.clone(), |worker| {
        job = Some(worker);
        Ok(())
    });
    job.unwrap()();
    let generation = index.generation;
    let error = index
        .poll(data.path(), &ctx)
        .expect("read failure must notify");
    assert!(!error.is_empty());
    assert!(index.contains_or_unloaded("preserved"));
    assert!(!index.contains_or_unloaded("absent"));
    assert_eq!(std::fs::read(database).unwrap(), corrupt_bytes);

    // Even a duplicate failure/success cannot notify again or replace this generation's outcome.
    index
        .tx
        .send((generation, Err("duplicate failure".into())))
        .unwrap();
    index
        .tx
        .send((generation, Ok(HashSet::from(["duplicate".into()]))))
        .unwrap();
    for _ in 0..3 {
        assert!(index.poll(data.path(), &ctx).is_none());
        assert_eq!(
            index.generation, generation,
            "failure must not trigger a retry"
        );
    }
    assert!(index.contains_or_unloaded("preserved"));
    assert!(!index.contains_or_unloaded("duplicate"));
}

#[test]
fn image_combine_index_stale_generation_failure_is_suppressed_before_and_after_latest() {
    let data = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut index = CombineIndex::default();
    index.set_keys_for_test(HashSet::from(["preserved".into()]));
    index.request_reload_with(data.path().to_owned(), ctx.clone(), |_| Ok(()));
    let old_generation = index.generation;
    index.request_reload_with(data.path().to_owned(), ctx.clone(), |_| Ok(()));
    let latest_generation = index.generation;
    index
        .tx
        .send((old_generation, Err("old read failure".into())))
        .unwrap();
    index
        .tx
        .send((latest_generation, Ok(HashSet::from(["latest".into()]))))
        .unwrap();
    index
        .tx
        .send((old_generation, Err("late old read failure".into())))
        .unwrap();
    assert!(index.poll(data.path(), &ctx).is_none());
    assert!(index.contains_or_unloaded("latest"));
    assert!(!index.contains_or_unloaded("preserved"));
    assert_eq!(index.generation, latest_generation);
}

#[test]
fn image_combine_index_injected_worker_start_failure_notifies_latest_only_once() {
    let data = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut index = CombineIndex::default();
    index.set_keys_for_test(HashSet::from(["preserved".into()]));
    index.fail_worker_start_for_test("old spawn failure");
    index.fail_worker_start_for_test("latest spawn failure");
    let generation = index.generation;
    let error = index
        .poll(data.path(), &ctx)
        .expect("spawn failure must notify");
    assert!(error.contains("worker could not start"));
    assert!(error.contains("latest spawn failure"));
    assert!(!error.contains("old spawn failure"));
    assert!(index.contains_or_unloaded("preserved"));
    assert!(!index.contains_or_unloaded("absent"));
    assert!(index.poll(data.path(), &ctx).is_none());
    assert_eq!(index.generation, generation, "spawn failure must not retry");

    // A newer successful reload suppresses an unconsumed failure from the preceding request.
    index.fail_worker_start_for_test("superseded spawn failure");
    index.request_reload_with(data.path().to_owned(), ctx.clone(), |_| Ok(()));
    index
        .tx
        .send((index.generation, Ok(HashSet::from(["latest".into()]))))
        .unwrap();
    assert!(index.poll(data.path(), &ctx).is_none());
    assert!(index.contains_or_unloaded("latest"));
}
