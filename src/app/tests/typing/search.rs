use super::*;
use crate::keymap::KeyAction;
use crate::ui_pinned_search::PinnedSearchKind as Kind;

#[derive(Clone, Copy, Debug)]
struct SearchBar {
    kind: Kind,
    pinned: bool,
    absent: bool,
}

impl Adapter for SearchBar {
    fn label(&self) -> String {
        format!(
            "{:?} pinned={} absent={}",
            self.kind, self.pinned, self.absent
        )
    }
    fn open(&self, app: &mut App, ctx: &egui::Context) {
        app.open_search_shortcut(ctx, self.kind);
    }
    fn opening_shortcut(&self) -> Option<(egui::Key, KeyAction)> {
        Some(match self.kind {
            Kind::Local => (egui::Key::F, KeyAction::GlobalLocalSearch),
            Kind::Container => (egui::Key::S, KeyAction::GlobalFavSearch),
            Kind::Metadata => (egui::Key::G, KeyAction::GlobalMetadataSearch),
            Kind::Everything => (egui::Key::R, KeyAction::GlobalEverythingSearch),
        })
    }
    fn opening_frame(&self) -> u64 {
        match (self.kind, self.pinned) {
            (Kind::Everything, false) => EVERYTHING_TEMPORARY_OPENING_FRAME,
            (Kind::Everything, true) => EVERYTHING_PINNED_OPENING_FRAME,
            (_, false) => TEMPORARY_SEARCH_OPENING_FRAME,
            (_, true) => PINNED_SEARCH_OPENING_FRAME,
        }
    }
    fn locator(&self) -> Locator {
        Locator::Id(if self.pinned {
            self.kind.input_id()
        } else {
            self.kind.temporary_id()
        })
    }
    fn draft<'a>(&self, app: &'a App) -> &'a str {
        app.search_draft(self.kind)
    }
    fn job(&self) -> Option<&'static str> {
        match self.kind {
            Kind::Container => Some("container"),
            Kind::Metadata => Some("metadata"),
            _ => None,
        }
    }
    fn outcome(&self, query: &str) -> Outcome {
        // Ctrl+G intentionally returns RejectedQuery for 1-char terms and lone
        // 2-char ASCII terms. These are results, never zero-hit successes.
        if self.kind == Kind::Metadata
            && (query.chars().count() < 2 || query.is_ascii() && query.chars().count() < 3)
        {
            Outcome::Rejected
        } else {
            Outcome::Delivered(if self.absent {
                0
            } else if query.starts_with('か') || query.starts_with('k') {
                1
            } else {
                2
            })
        }
    }
    fn adopted(&self, app: &App, query: &str, outcome: Outcome) -> bool {
        match self.kind {
            Kind::Container => matches!(outcome, Outcome::Delivered(count)
                if app.favsearch.last_executed == query && app.favsearch_pending.is_none()
                    && app.favsearch.results_paths.len() == count && app.items.len() == count
                    && !app.sidecar_restore_active()),
            Kind::Metadata => {
                app.global_search.last_executed == query
                    && app.global_search.done
                    && match outcome {
                        Outcome::Rejected => {
                            app.global_search.reject_message.as_deref()
                                == Some(
                                    crate::global_search::RejectReason::TooShort(vec![
                                        query.to_lowercase(),
                                    ])
                                    .as_user_message()
                                    .as_str(),
                                )
                        }
                        Outcome::Delivered(count) => {
                            app.global_search.reject_message.is_none()
                                && app.items_are_global_search_view
                                && app.items.len() == count
                        }
                        Outcome::Started => false,
                    }
            }
            _ => true,
        }
    }
    fn check_idle(&self, app: &App) {
        match self.kind {
            // Ctrl+F is Enter-only. Neither bar submits while
            // typing; searching here would test a different product operation.
            Kind::Local => assert!(
                app.search_pending.is_none() && app.search_filter.is_none(),
                "Ctrl+F must keep its unsent draft"
            ),
            // Everything edits are intentionally ignored by Action::Edited and
            // the temporary bar. Enter/button owns IPC, even without Everything.
            Kind::Everything => assert!(
                app.global_search.pending.is_none() && app.global_search.all_hits.is_empty(),
                "Everything must not query IPC during typing"
            ),
            _ => {}
        }
    }
}

pub(super) fn seed_images(env: &mut AppTestEnv) -> Vec<PathBuf> {
    let folder = env.tmp.path().join("pages");
    std::fs::create_dir_all(&folder).unwrap();
    let paths: Vec<_> = ["alpha.png", "alpine.png", "かな本.png", "koi.png"]
        .into_iter()
        .map(|name| {
            let path = folder.join(name);
            image::RgbImage::new(2, 2).save(&path).unwrap();
            path
        })
        .collect();
    // Publish the physical source through its production owner, including page
    // identities and hydration. Harness::new drives full frames to its adoption.
    env.load_folder(folder);
    paths
}

fn seed_container(env: &mut AppTestEnv) {
    let root = env.tmp.path().join("favorites");
    let entries: Vec<_> = ["alpha", "alpine", "かな本", "koi"]
        .into_iter()
        .map(|name| {
            let path = root.join(name);
            std::fs::create_dir_all(&path).unwrap();
            crate::search_index_db::IndexEntry {
                display_name: name.into(),
                path,
                kind: crate::search_index_db::IndexKind::Folder,
                mtime: 0,
            }
        })
        .collect();
    let db =
        crate::search_index_db::SearchIndexDb::open_at(&env.tmp.path().join("typing-index.db"))
            .unwrap();
    db.upsert_children(&root, &root, &entries).unwrap();
    env.search_index_db = Some(Arc::new(db));
    let mut favorite = crate::settings::FavoriteEntry::new("typing".into(), root);
    favorite.auto_index_structure = true;
    env.settings.favorites = vec![favorite];
}

fn seed_metadata(env: &mut AppTestEnv, paths: &[PathBuf]) {
    let data = env.tmp.path().join("typing-metadata");
    std::fs::create_dir_all(&data).unwrap();
    let mut favorite = crate::settings::FavoriteEntry::new(
        "typing".into(),
        paths[0].parent().unwrap().to_path_buf(),
    );
    favorite.auto_index_metadata = true;
    // Seed before the manager opens its stores/writer, using the same store layout
    // as IndexerManager's existing test fixture. No indexing supervisor is needed.
    drop(crate::fts_meta::FtsMetaDb::open_at(&data.join("fts_meta.db")).unwrap());
    let index = crate::fts_index::FtsIndex::open_at(&data.join("fts_index")).unwrap();
    let mut writer = index.writer().unwrap();
    for path in paths {
        crate::fts_index::upsert_doc(
            &writer,
            index.fields(),
            &crate::fts_index::IndexDoc {
                path: crate::path_key::normalize_keep_drive(path),
                display_path: path.to_string_lossy().into_owned(),
                container: crate::fts_index::Container::Fs,
                zip_entry: String::new(),
                favorite_id: favorite.id,
                kind: crate::fts_index::IndexKind::Image,
                mtime: 0,
                file_size: std::fs::metadata(path).unwrap().len() as i64,
                norms: crate::ingest_text::PerSourceText {
                    name: crate::search_norm::normalize_for_match(
                        path.file_name().unwrap().to_str().unwrap(),
                    ),
                    ..Default::default()
                },
            },
        )
        .unwrap();
    }
    writer.commit().unwrap();
    drop(writer);
    drop(index);
    env.settings.favorites = vec![favorite];
    env.indexer_init = IndexerInit::Ready(
        crate::indexer_manager::IndexerManager::new_at(
            &data,
            &[],
            crate::settings::IndexerSpeedProfile::default(),
            Arc::clone(&env.activity_gate),
            Vec::new(),
        )
        .expect("open seeded metadata index"),
    );
}

fn harness(kind: Kind, pinned: bool, absent: bool) -> Harness<SearchBar> {
    harness_with_measurement(kind, pinned, absent, false)
}

fn harness_with_measurement(
    kind: Kind,
    pinned: bool,
    absent: bool,
    measure: bool,
) -> Harness<SearchBar> {
    let mut env = fixture();
    let paths = seed_images(&mut env);
    match kind {
        Kind::Local => env.settings.show_toolbar_search_local = pinned,
        Kind::Container => {
            env.settings.show_toolbar_search_container = pinned;
            seed_container(&mut env);
        }
        Kind::Metadata => {
            env.settings.show_toolbar_search_metadata = pinned;
            seed_metadata(&mut env, &paths);
        }
        Kind::Everything => env.settings.show_toolbar_search_everything = pinned,
    }
    Harness::new_with_measurement(
        env,
        SearchBar {
            kind,
            pinned,
            absent,
        },
        paths,
        measure,
    )
}

fn ascii_case(kind: Kind, pinned: bool, absent: bool) {
    harness(kind, pinned, absent).type_text(if absent { "rzz" } else { "alp" });
}

fn ime_case(kind: Kind, pinned: bool) {
    // Keep every nonempty preedit a prefix that has a matching fixture row.
    // The physical Process presses consumed by Windows IME are not sent.
    harness(kind, pinned, false).compose_ime(&["k", "か", "かな"], "かな");
}

#[test]
fn typing_ctrl_f_pinned_ascii() {
    ascii_case(Kind::Local, true, false);
}
#[test]
fn typing_ctrl_f_temporary_ascii() {
    ascii_case(Kind::Local, false, false);
}
#[test]
fn typing_ctrl_f_bars_zero() {
    for pinned in [false, true] {
        ascii_case(Kind::Local, pinned, true);
    }
}
#[test]
fn typing_ctrl_f_bars_ime() {
    for pinned in [false, true] {
        ime_case(Kind::Local, pinned);
    }
}
#[test]
fn typing_ctrl_g_pinned_ascii() {
    ascii_case(Kind::Metadata, true, false);
}
#[test]
fn typing_ctrl_g_temporary_ascii() {
    ascii_case(Kind::Metadata, false, false);
}
#[test]
fn typing_ctrl_g_bars_zero() {
    for pinned in [false, true] {
        ascii_case(Kind::Metadata, pinned, true);
    }
}
#[test]
fn typing_ctrl_g_bars_ime() {
    for pinned in [false, true] {
        ime_case(Kind::Metadata, pinned);
    }
}
#[test]
fn typing_everything_pinned_ascii() {
    ascii_case(Kind::Everything, true, false);
}
#[test]
fn typing_everything_temporary_ascii() {
    ascii_case(Kind::Everything, false, false);
}
#[test]
fn typing_everything_bars_zero() {
    for pinned in [false, true] {
        ascii_case(Kind::Everything, pinned, true);
    }
}
#[test]
fn typing_everything_bars_ime() {
    for pinned in [false, true] {
        ime_case(Kind::Everything, pinned);
    }
}

#[test]
fn typing_ctrl_s_pinned_ascii() {
    ascii_case(Kind::Container, true, false);
}
#[test]
fn typing_ctrl_s_temporary_ascii() {
    ascii_case(Kind::Container, false, false);
}
#[test]
fn typing_ctrl_s_bars_zero() {
    for pinned in [false, true] {
        // Zero hits alone do not trigger the known hydration/focus defect.
        // First exercise its nonempty-result control, then start the zero-hit
        // case from a fresh empty field after that independent fixture drops.
        ascii_case(Kind::Container, pinned, false);
        ascii_case(Kind::Container, pinned, true);
    }
}
#[test]
fn typing_ctrl_s_bars_ime() {
    for pinned in [false, true] {
        ime_case(Kind::Container, pinned);
    }
}

struct TagView {
    absent: bool,
}
impl Adapter for TagView {
    fn label(&self) -> String {
        "Ctrl+T tag view".into()
    }
    fn open(&self, app: &mut App, _ctx: &egui::Context) {
        app.open_tag_view();
    }
    fn opening_shortcut(&self) -> Option<(egui::Key, KeyAction)> {
        Some((egui::Key::T, KeyAction::GridTagView))
    }
    fn opening_frame(&self) -> u64 {
        TAG_VIEW_OPENING_FRAME
    }
    fn locator(&self) -> Locator {
        Locator::Id(App::tag_search_input_id())
    }
    fn draft<'a>(&self, app: &'a App) -> &'a str {
        &app.tag_view.query
    }
    fn job(&self) -> Option<&'static str> {
        Some("tag-view")
    }
    fn outcome(&self, query: &str) -> Outcome {
        Outcome::Delivered(if self.absent {
            0
        } else if query.starts_with('か') || query.starts_with('k') {
            1
        } else {
            2
        })
    }
    fn adopted(&self, app: &App, query: &str, outcome: Outcome) -> bool {
        matches!(outcome, Outcome::Delivered(count) if app.tag_view.last_executed == query
            && app.tag_view_pending.is_none() && app.items_are_tag_view
            && app.tag_view.result_count == count && app.items.len() == count && !app.sidecar_restore_active())
    }
}

fn tag_harness(absent: bool) -> Harness<TagView> {
    tag_harness_with_measurement(absent, false)
}

fn tag_harness_with_measurement(absent: bool, measure: bool) -> Harness<TagView> {
    let mut env = fixture();
    let paths = seed_images(&mut env);
    // The origin's completed load must not prewarm the result rows' identities.
    // First adoption therefore enters new destinations, as in the reported
    // transition, rather than merely showing a subset of the origin's rows.
    let result_root = env.tmp.path().join("tag-results");
    std::fs::create_dir_all(&result_root).unwrap();
    // tag-view's worker opens this fixture's actual tags.db, not an in-memory fake.
    let mut db = crate::tags_db::TagsDb::open_at(&env.tmp.path().join("tags.db")).unwrap();
    for tag in ["alpha", "alpine", "かな本", "koi"] {
        let path = result_root.join(format!("{tag}.png"));
        image::RgbImage::new(2, 2).save(&path).unwrap();
        db.set_item_tags(&crate::tags_db::item_key_for_path(&path), [tag], "typing")
            .unwrap();
    }
    drop(db);
    Harness::new_with_measurement(env, TagView { absent }, paths, measure)
}

pub(super) fn measure_openings(failures: &mut Vec<String>) {
    for kind in Kind::ALL {
        for pinned in [false, true] {
            let h = harness_with_measurement(kind, pinned, false, true);
            if let Some(failure) = h.measured_opening_contract_failure() {
                failures.push(failure);
            }
        }
    }
    let h = tag_harness_with_measurement(false, true);
    if let Some(failure) = h.measured_opening_contract_failure() {
        failures.push(failure);
    }
}

#[test]
fn typing_ctrl_t_ascii() {
    tag_harness(false).type_text("alp");
}
#[test]
fn typing_ctrl_t_zero() {
    // Keep the known-red trigger in this ignored regression as well. The
    // second, independent fixture still verifies zero hits from an empty field.
    tag_harness(false).type_text("alp");
    tag_harness(true).type_text("rzz");
}
#[test]
fn typing_ctrl_t_ime() {
    tag_harness(false).compose_ime(&["k", "か", "かな"], "かな");
}
