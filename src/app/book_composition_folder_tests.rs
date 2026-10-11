use super::super::book_materialization::BookScope;
use super::*;

fn settings() -> Settings {
    let mut settings = Settings::default();
    settings.auto_fullscreen_zip_pdf = true;
    settings.auto_fullscreen_image_folders = true;
    settings.skip_duplicate_images = false;
    settings
}

fn reference(name: &str) -> CompositionPageRef {
    CompositionPageRef::RelativePath { path: name.into() }
}

fn single(name: &str) -> CompositionUnit {
    CompositionUnit::Single {
        page: reference(name),
        placement: SinglePlacement::Center,
        companion: SingleCompanion::Vacant,
    }
}

fn token() -> CompositionToken {
    CompositionToken {
        service_epoch: 7,
        publication_sequence: 3,
    }
}

fn fixture(root: &Path) {
    for name in ["10.jpg", "02.jpg", "03.dng"] {
        std::fs::write(root.join(name), b"physical direct image").unwrap();
    }
}

#[test]
fn g4f_physical_refs_preserve_raw_missing_hidden_and_composed_order() {
    let temp = tempfile::tempdir().unwrap();
    fixture(temp.path());
    let settings = settings();
    let source = prepare_folder_source(temp.path(), &settings).unwrap();
    assert_eq!(
        source.recipe,
        ProjectionRecipe::Book(BookScope::ImageFolder(temp.path().into()))
    );
    assert_eq!(
        source
            .raw_pages
            .iter()
            .map(|raw| raw.page.clone())
            .collect::<Vec<_>>(),
        vec![
            reference("02.jpg"),
            reference("03.dng"),
            reference("10.jpg")
        ]
    );
    let mut record = BookComposition::new(
        ReadingDirection::Rtl,
        vec![
            single("10.jpg"),
            single("missing.jpg"),
            single("03.dng"),
            single("02.jpg"),
        ],
    );
    record.hidden_pages = vec![reference("02.jpg")];
    let rows = project_folder_composition(
        temp.path(),
        &source.raw_pages,
        source.rows,
        settings.file_type_visibility,
        token(),
        Some(&record),
    )
    .unwrap();
    assert_eq!(
        rows.items,
        vec![
            GridItem::Image(temp.path().join("10.jpg")),
            GridItem::Image(temp.path().join("03.dng"))
        ]
    );
    let CompositionProjectionOutcome::Composed(projection) = rows.composition else {
        panic!("composed")
    };
    assert_eq!(
        projection.resolved.page_status(&reference("02.jpg")),
        PageStatus::HiddenByComposition
    );
    assert_eq!(
        projection.resolved.page_status(&reference("missing.jpg")),
        PageStatus::Missing
    );
    assert_eq!(projection.canonical_raw_index_for_projected(0), Some(2));
    assert!(
        matches!(projection.position_for_projected(1).unwrap().identity,
        Some(crate::book_resume_db::PageIdentity::Image { path }) if path.ends_with("/03.dng"))
    );
}

#[test]
fn g4f_off_mixed_and_child_only_keep_production_classification_and_order() {
    let temp = tempfile::tempdir().unwrap();
    fixture(temp.path());
    let mut settings = settings();
    settings.auto_fullscreen_image_folders = false;
    settings.sort_order = crate::settings::SortOrder::FileName;
    let off = prepare_folder_source(temp.path(), &settings).unwrap();
    assert_eq!(off.recipe, ProjectionRecipe::OrdinaryList);
    settings.auto_fullscreen_image_folders = true;
    for name in ["clip.mp4", "sound.mp3"] {
        std::fs::write(temp.path().join(name), b"media").unwrap();
        let mixed = prepare_folder_source(temp.path(), &settings).unwrap();
        assert_eq!(mixed.recipe, ProjectionRecipe::OrdinaryList);
        assert!(
            mixed
                .rows
                .0
                .iter()
                .any(|row| matches!(row, GridItem::Video(_) | GridItem::Audio(_)))
        );
        std::fs::remove_file(temp.path().join(name)).unwrap();
    }
    assert!(matches!(
        prepare_folder_source(temp.path(), &settings)
            .unwrap()
            .recipe,
        ProjectionRecipe::Book(BookScope::ImageFolder(_))
    ));
    let child = tempfile::tempdir().unwrap();
    std::fs::write(child.path().join("child.zip"), b"container").unwrap();
    assert_eq!(
        prepare_folder_source(child.path(), &settings)
            .unwrap()
            .recipe,
        ProjectionRecipe::OrdinaryList
    );
    std::fs::create_dir(child.path().join("subfolder")).unwrap();
    assert_eq!(
        prepare_folder_source(child.path(), &settings)
            .unwrap()
            .recipe,
        ProjectionRecipe::OrdinaryList
    );
}

#[test]
fn g4f_source_manifest_keeps_policy_and_same_name_exclusions_distinct_from_missing() {
    let temp = tempfile::tempdir().unwrap();
    fixture(temp.path());
    std::fs::write(temp.path().join("02.png"), b"duplicate").unwrap();
    let mut settings = settings();
    settings.skip_duplicate_images = true;
    settings.image_ext_priority = vec!["jpg".into(), "png".into()];
    settings
        .file_type_visibility
        .excluded_extensions
        .insert("dng".into());
    let source = prepare_folder_source(temp.path(), &settings).unwrap();
    let statuses = source
        .manifest
        .iter()
        .map(|entry| (entry.page.clone(), entry.availability))
        .collect::<HashMap<_, _>>();
    assert_eq!(
        statuses[&reference("03.dng")],
        ManifestAvailability::ExcludedByPolicy
    );
    assert_eq!(
        statuses[&reference("02.png")],
        ManifestAvailability::FilteredOut
    );
    assert!(matches!(
        statuses[&reference("02.jpg")],
        ManifestAvailability::Present { .. }
    ));
    let record = BookComposition::new(
        ReadingDirection::Ltr,
        vec![
            single("03.dng"),
            single("02.png"),
            single("10.jpg"),
            single("02.jpg"),
        ],
    );
    let rows = project_folder_composition(
        temp.path(),
        &source.raw_pages,
        source.rows,
        settings.file_type_visibility,
        token(),
        Some(&record),
    )
    .unwrap();
    let CompositionProjectionOutcome::Composed(projection) = rows.composition else {
        panic!("composed")
    };
    assert_eq!(
        projection.resolved.page_status(&reference("03.dng")),
        PageStatus::ExcludedByPolicy
    );
    assert_eq!(
        projection.resolved.page_status(&reference("02.png")),
        PageStatus::FilteredOut
    );
}

#[test]
fn g4f_compiled_book_source_stays_outside_folder_composition_adoption() {
    let mut settings = settings();
    let root = settings.books_root_path().join("g4f-existing-compiled");
    settings.auto_fullscreen_zip_pdf = false;
    assert_eq!(
        ProjectionRecipe::for_physical(
            &settings,
            &root,
            &root,
            &[GridItem::Image(root.join("01.jpg"))]
        ),
        ProjectionRecipe::Book(BookScope::CompiledFolder(root.clone()))
    );
    assert_eq!(
        super::super::book_materialization::composition_scope_domain(&BookScope::CompiledFolder(
            root
        ))
        .unwrap(),
        None
    );
}
