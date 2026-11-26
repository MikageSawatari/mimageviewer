use super::*;
use crate::image_combine::store::{load_recipe, save_recipe};
use crate::image_combine::{
    CombineFormat, CombineItemSpec, CombineOptions, CombineRecipe, CombineSourceRef,
};

fn recipe(source: &Path, label: &str) -> CombineRecipe {
    CombineRecipe {
        v: 1,
        options: CombineOptions::default(),
        format: CombineFormat::Png,
        items: vec![CombineItemSpec {
            composition_identity: None,
            annotation_space: None,
            source: CombineSourceRef::File {
                path: source.to_owned(),
            },
            label_override: Some(label.into()),
        }],
    }
}

#[test]
fn image_combine_store_descriptor_matches_comic_and_effects_only_follow_changes() {
    let comic = STORES.iter().find(|s| s.file == "comic.db").unwrap();
    let combine = STORES.iter().find(|s| s.file == "combine.db").unwrap();
    assert!(combine.unique);
    assert!(combine.rename_generic);
    assert_eq!(combine.book_policy, comic.book_policy);
    assert_eq!(combine.copies_with_book_page, comic.copies_with_book_page);
    let mut effect = StoreMutationEffects::default();
    effect.record_completed(combine);
    effect.record_rows_changed(combine, 0);
    effect.record_rows_changed(comic, 1);
    assert!(!effect.combine_index_stale());
    let mut changed = StoreMutationEffects::default();
    changed.record_rows_changed(combine, 1);
    effect.merge(changed);
    assert!(effect.combine_index_stale());
}

#[test]
fn image_combine_store_exact_parent_rename_and_purge_preserve_recipe_source_paths() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let old_parent = root.path().join("pictures_%");
    let new_parent = root.path().join("renamed");
    let output = old_parent.join("joined.png");
    let moved = old_parent.join("moved.png");
    let sibling = old_parent.join("second.png");
    let adjacent = root.path().join("pictures_%extra/keep.png");
    let original = recipe(&old_parent.join("source.png"), "unchanged source");
    for path in [&output, &sibling, &adjacent] {
        save_recipe(&data, path, &original).unwrap();
    }
    let renamed = run_at(&data, &output, &moved);
    assert!(renamed.errors.is_empty(), "{:?}", renamed.errors);
    assert!(renamed.store_mutations.combine_index_stale());
    assert!(load_recipe(&data, &output).unwrap().is_none());
    assert_eq!(load_recipe(&data, &moved).unwrap(), Some(original.clone()));
    let report = run_at_with_shape(&data, &old_parent, &new_parent, true);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(report.store_mutations.combine_index_stale());
    let moved = new_parent.join("moved.png");
    assert_eq!(load_recipe(&data, &moved).unwrap(), Some(original.clone()));
    assert_eq!(
        load_recipe(&data, &new_parent.join("second.png")).unwrap(),
        Some(original.clone())
    );
    assert_eq!(
        load_recipe(&data, &adjacent).unwrap(),
        Some(original.clone())
    );
    let purge = purge_removed_scopes_at(
        &data,
        &[crate::delete_worker::DeleteSourceScope::Exact(
            moved.clone(),
        )],
        &[],
    );
    assert!(purge.errors.is_empty(), "{:?}", purge.errors);
    assert!(purge.store_mutations.combine_index_stale());
    assert!(load_recipe(&data, &moved).unwrap().is_none());
    assert!(
        load_recipe(&data, &new_parent.join("second.png"))
            .unwrap()
            .is_some()
    );
    let purge = purge_removed_scopes_at(
        &data,
        &[crate::delete_worker::DeleteSourceScope::Tree(new_parent)],
        &[],
    );
    assert!(purge.errors.is_empty(), "{:?}", purge.errors);
    assert!(purge.store_mutations.combine_index_stale());
    assert_eq!(load_recipe(&data, &adjacent).unwrap(), Some(original));
}

#[test]
fn image_combine_store_destination_wins_and_repeat_copy_has_no_index_effect() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source.png");
    let destination = root.path().join("destination.png");
    let copied = root.path().join("copied.png");
    save_recipe(root.path(), &source, &recipe(&source, "old")).unwrap();
    let winner = recipe(&source, "destination wins");
    save_recipe(root.path(), &destination, &winner).unwrap();
    let report = run_at(root.path(), &source, &destination);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(report.store_mutations.combine_index_stale());
    assert!(load_recipe(root.path(), &source).unwrap().is_none());
    assert_eq!(
        load_recipe(root.path(), &destination).unwrap(),
        Some(winner.clone())
    );
    let mappings = [StoreCopyPathMapping::exact(&destination, &copied)];
    let copied_report = copy_stores_at(root.path(), &mappings);
    assert!(
        copied_report.errors.is_empty(),
        "{:?}",
        copied_report.errors
    );
    assert!(copied_report.store_mutations.combine_index_stale());
    assert_eq!(
        load_recipe(root.path(), &copied).unwrap(),
        Some(winner.clone())
    );
    let repeated = copy_stores_at(root.path(), &mappings);
    assert!(repeated.errors.is_empty(), "{:?}", repeated.errors);
    assert!(!repeated.store_mutations.combine_index_stale());
    assert_eq!(load_recipe(root.path(), &copied).unwrap(), Some(winner));
}

#[test]
fn image_combine_store_prefix_collision_deletion_invalidates_index() {
    let root = tempfile::tempdir().unwrap();
    let old_parent = root.path().join("old");
    let new_parent = root.path().join("new");
    let source = old_parent.join("combined.png");
    let destination = new_parent.join("combined.png");
    save_recipe(root.path(), &source, &recipe(&source, "old")).unwrap();
    let winner = recipe(&source, "destination wins");
    save_recipe(root.path(), &destination, &winner).unwrap();
    let report = run_at_with_shape(root.path(), &old_parent, &new_parent, true);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert!(report.store_mutations.combine_index_stale());
    assert!(load_recipe(root.path(), &source).unwrap().is_none());
    assert_eq!(
        load_recipe(root.path(), &destination).unwrap(),
        Some(winner)
    );
}
