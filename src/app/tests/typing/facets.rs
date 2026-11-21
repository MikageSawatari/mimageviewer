use super::*;

// Candidate values from the same clicked-field path as address/dialogs.
// New surfaces MUST be measured by typing_opening_readiness_measurement on the
// shared runner before these become accepted measured constants. No local cargo.
const FACET_NAME_OPENING_FRAME: u64 = 2;
const FACET_TAG_OPENING_FRAME: u64 = 2;

#[derive(Clone, Copy)]
enum Facet {
    Name,
    Tags,
}

impl Adapter for Facet {
    fn label(&self) -> String {
        match self {
            Self::Name => "facet name filter",
            Self::Tags => "facet tag suggestions",
        }
        .into()
    }
    fn open(&self, _app: &mut App, _ctx: &egui::Context) {}
    fn opening_button(&self) -> Option<&'static str> {
        matches!(self, Self::Tags).then_some("facet-tag-menu")
    }
    fn opens_focused(&self) -> bool {
        false
    }
    fn opening_frame(&self) -> u64 {
        match self {
            Self::Name => FACET_NAME_OPENING_FRAME,
            Self::Tags => FACET_TAG_OPENING_FRAME,
        }
    }
    fn locator(&self) -> Locator {
        Locator::Site(
            "src/ui_main.rs",
            match self {
                Self::Name => "&mut self.facet_name_input",
                Self::Tags => "&mut self.facet_tag_search_query",
            },
        )
    }
    fn draft<'a>(&self, app: &'a App) -> &'a str {
        match self {
            Self::Name => &app.facet_name_input,
            Self::Tags => &app.facet_tag_search_query,
        }
    }
    fn job(&self) -> Option<&'static str> {
        Some(match self {
            Self::Name => "facet-name",
            Self::Tags => "facet-tag",
        })
    }
    fn outcome(&self, query: &str) -> Outcome {
        Outcome::Delivered(if query.starts_with('r') {
            0
        } else if query.starts_with('k') || query.starts_with('か') {
            1
        } else {
            2
        })
    }
    fn adopted(&self, app: &App, query: &str, outcome: Outcome) -> bool {
        let Outcome::Delivered(count) = outcome else {
            return false;
        };
        match self {
            Self::Name => {
                app.settings.facet_filter.name_query == query
                    && app.facet_name_debounce_deadline.is_none()
                    && app.facet_name_cache_generation == Some(app.items_generation)
                    && app.facet_name_cache.len() == app.items.len()
                    && app.visible_indices.len() == count
            }
            Self::Tags => app
                .facet_tag_suggestion_cache
                .as_ref()
                .is_some_and(|(key, rows)| {
                    *key == crate::tags_db::normalize_tag_key(query) && rows.len() == count
                }),
        }
    }
    fn defers_results_during_ime(&self) -> bool {
        matches!(self, Self::Name)
    }
    fn ime_deferred_deadline(&self, app: &App) -> Option<Instant> {
        matches!(self, Self::Name)
            .then_some(app.facet_name_debounce_deadline)
            .flatten()
    }
    fn check_ime_deferred(&self, app: &App, query: &str) {
        assert!(matches!(self, Self::Name));
        // facet_name_filter.rs: poll_facet_name_debounce renews 150ms throughout
        // composition. These tests start empty, so no preedit may become applied.
        assert!(app.settings.facet_filter.name_query.is_empty());
        assert_eq!(app.visible_indices.len(), 4);
        assert!(app.facet_name_cache_generation.is_none());
        assert!(
            !probe::jobs()
                .iter()
                .any(|job| job.field == "facet-name-cache" && job.outcome == Outcome::Started),
            "normalization worker must not start for an uncommitted preedit"
        );
        assert!(!probe::jobs().iter().any(|job| job.field == "facet-name"
            && job.query == query
            && matches!(job.outcome, Outcome::Delivered(_))));
    }
}

fn harness(facet: Facet, measure: bool) -> Harness<Facet> {
    let mut env = fixture();
    env.settings.show_toolbar_facet_filter = true;
    env.settings.facet_filter = Default::default();
    env.settings.toolbar_facet_filter_items = vec![match facet {
        Facet::Name => crate::settings::ToolbarFacetFilterItem::NameFilter,
        Facet::Tags => crate::settings::ToolbarFacetFilterItem::Tags,
    }];
    let paths = super::search::seed_images(&mut env);
    if matches!(facet, Facet::Tags) {
        let mut db = crate::tags_db::TagsDb::open_at(&env.tmp.path().join("tags.db")).unwrap();
        for (path, tag) in paths.iter().zip(["alpha", "alpine", "かな本", "koi"]) {
            db.set_item_tags(&crate::tags_db::item_key_for_path(path), [tag], "typing")
                .unwrap();
        }
        env.tags_db = Some(db);
    }
    Harness::new_with_measurement(env, facet, paths, measure)
}

fn assert_cache_adopted(h: &Harness<Facet>) {
    let jobs = probe::jobs();
    let generation = h.env.items_generation.to_string();
    let cache: Vec<_> = jobs
        .iter()
        .filter(|job| job.field == "facet-name-cache" && job.query == generation)
        .collect();
    let started = cache
        .iter()
        .position(|job| job.outcome == Outcome::Started)
        .expect("cache worker started");
    assert!(
        cache
            .iter()
            .skip(started + 1)
            .any(|job| job.outcome == Outcome::Delivered(4)),
        "normalization cache must adopt before the result: {}",
        h.diagnostic()
    );
}

#[test]
fn typing_facet_name_ascii() {
    let mut h = harness(Facet::Name, false);
    h.type_text("alp");
    assert_cache_adopted(&h);
}
#[test]
fn typing_facet_name_zero() {
    let mut h = harness(Facet::Name, false);
    h.type_text("rzz");
    assert_cache_adopted(&h);
}
#[test]
fn typing_facet_name_ime_defers_until_commit() {
    let mut h = harness(Facet::Name, false);
    h.compose_ime(&["k", "か", "かな"], "かな");
    assert_cache_adopted(&h);
}

#[test]
#[expect(deprecated, reason = "Exercise the real Windows IME Enabled event")]
fn typing_facet_name_ime_missing_deadline_rejects_early_adoption() {
    let mut h = harness(Facet::Name, false);
    h.stage = "IME Preedit with synthetic early adoption and missing deadline".into();
    h.composition_start = Some(0);
    h.tick(vec![egui::Event::Ime(egui::ImeEvent::Enabled)]);
    let since = probe::jobs().len();
    h.expected = "alp".into();
    h.tick(vec![egui::Event::Ime(egui::ImeEvent::Preedit {
        text: h.expected.clone(),
        active_range_chars: None,
    })]);
    assert!(h.env.facet_name_debounce_deadline.is_some());

    // Test-only counterexample: preserve a real focused, composing TextEdit,
    // but make App look as if this preedit was already applied and delivered.
    // No production frame phase or producer is replaced/modified.
    h.env.settings.facet_filter.name_query = h.expected.clone();
    h.env.facet_name_tokens = crate::search_query::parse(&h.expected);
    h.env.facet_name_cache = h
        .env
        .items
        .iter()
        .map(|item| item.name().to_lowercase().into_boxed_str())
        .collect();
    h.env.facet_name_cache_generation = Some(h.env.items_generation);
    h.env.visible_indices = vec![0, 1];
    h.env.facet_name_debounce_deadline = None;
    probe::job("facet-name", &h.expected, Outcome::Delivered(2));
    h.assert_field();
    assert!(h.adapter.defers_results_during_ime());
    assert_eq!(
        h.result_progress(since),
        (true, true),
        "ordinary result waiting would accept this counterexample"
    );

    let failure = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| h.wait_result(since)))
        .expect_err("missing deadline must fail instead of accepting early adoption");
    let message = failure
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| failure.downcast_ref::<&str>().copied())
        .expect("string panic payload");
    assert!(
        message.contains("missing IME deferral deadline"),
        "must fail at the deadline check, not an unrelated invariant: {message}"
    );
}
#[test]
fn typing_facet_tag_suggestions_ascii() {
    harness(Facet::Tags, false).type_text("alp");
}
#[test]
fn typing_facet_tag_suggestions_zero() {
    harness(Facet::Tags, false).type_text("rzz");
}
#[test]
fn typing_facet_tag_suggestions_ime() {
    harness(Facet::Tags, false).compose_ime(&["k", "か", "かな"], "かな");
}

pub(super) fn measure_openings(failures: &mut Vec<String>) {
    for facet in [Facet::Name, Facet::Tags] {
        let h = harness(facet, true);
        if let Some(failure) = h.measured_opening_contract_failure() {
            failures.push(failure);
        }
    }
}
