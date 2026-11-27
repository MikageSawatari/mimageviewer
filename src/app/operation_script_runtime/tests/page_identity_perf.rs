//! Opt-in full stage-2b identity and unchanged r11 event-identity measurements.
//! Synthetic in-memory rows only: no images/archives are opened or rendered.

use super::*;
use std::{hint::black_box, mem::size_of, path::Path, time::Instant};

use crate::app::SpreadPresentationSource;
use crate::app::book_materialization::{BookScope, ProjectionRecipe};
use crate::book_composition::{
    BookComposition, CanonicalRawPage, CompositionPageRef, CompositionProjectionOutcome,
    CompositionToken, ManifestAvailability, ManifestPage, PageDomain, project_composition,
};
use crate::settings::SpreadMode;

const SAMPLES: usize = 257;
const WARMUP: usize = 16;
const NAME_LEN: usize = 200;

pub(super) fn prepare_rows(app: &mut App, count: usize, zip: bool) {
    // Reuse the sole fixture: setup_app holds the process-wide data-dir mutex.
    app.book_composition = Default::default();
    app.spread_shift = Default::default();
    app.spread_step = None;
    let root = PathBuf::from("C:/page-identity-perf");
    let archive = root.join("book.zip");
    app.current_folder = Some(if zip { archive.clone() } else { root.clone() });
    app.projection_recipe = ProjectionRecipe::Book(if zip {
        BookScope::Archive {
            logical_root: archive.clone(),
            prefix: Vec::new(),
        }
    } else {
        BookScope::ImageFolder(root.clone())
    });
    app.items = (0..count)
        .map(|index| {
            let suffix = format!("{index:05}.jpg");
            let prefix_len = if zip {
                NAME_LEN - suffix.len()
            } else {
                NAME_LEN - root.as_os_str().len() - 1 - suffix.len()
            };
            let name = format!("{}{suffix}", "x".repeat(prefix_len));
            if zip {
                assert_eq!(name.len(), NAME_LEN);
                GridItem::ZipImage {
                    zip_path: archive.clone(),
                    entry_name: name,
                }
            } else {
                let path = root.join(name);
                assert_eq!(path.as_os_str().len(), NAME_LEN);
                GridItem::Image(path)
            }
        })
        .collect();
    app.visible_indices = (0..count).collect();
    app.details_order = app.visible_indices.clone();
    app.settings.grid_view_mode = crate::settings::GridViewMode::Thumbnail;
    app.fullscreen_idx = Some(count / 2);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.items_generation += 1;
    app.viewer_navigation_caches.invalidate();
    assert_eq!(app.current_reader_order().len(), count);
}

pub(super) fn install_zip_composition(app: &mut App) {
    let raw = app
        .items
        .iter()
        .enumerate()
        .map(|(raw_index, item)| {
            let GridItem::ZipImage { entry_name, .. } = item else {
                panic!("ZIP measurement fixture");
            };
            CanonicalRawPage {
                page: CompositionPageRef::ArchiveEntry {
                    entry_name: entry_name.clone(),
                },
                raw_index,
            }
        })
        .collect::<Vec<_>>();
    let manifest = raw
        .iter()
        .map(|row| ManifestPage {
            page: row.page.clone(),
            availability: ManifestAvailability::Present {
                item_index: row.raw_index,
            },
        })
        .collect::<Vec<_>>();
    let book = BookComposition::new(
        crate::book_composition::ReadingDirection::Ltr,
        raw.iter().map(|row| row.page.clone()).collect(),
    );
    let prepared = project_composition(
        crate::book_scope::BookScopeKey::archive(app.current_folder.as_ref().unwrap(), &[])
            .unwrap(),
        &PageDomain::ArchiveLevel {
            prefix: String::new(),
        },
        CompositionToken {
            service_epoch: 1,
            publication_sequence: 1,
        },
        Some(&book),
        &raw,
        &manifest,
        app.items.len(),
    )
    .unwrap();
    assert!(
        prepared
            .output_to_input
            .iter()
            .copied()
            .eq(0..app.items.len())
    );
    let CompositionProjectionOutcome::Composed(projection) = prepared.outcome else {
        panic!("expected composed ZIP fixture: {:?}", prepared.outcome);
    };
    app.install_composition_for_test(projection);
}

fn path_bytes(path: &Path) -> usize {
    path.as_os_str().as_encoded_bytes().len()
}

fn item_heap_bytes(item: &ScriptItemIdentity) -> usize {
    match item {
        ScriptItemIdentity::File(_, path) | ScriptItemIdentity::PdfPage(path, _) => {
            path_bytes(path)
        }
        ScriptItemIdentity::ZipImage(path, text) | ScriptItemIdentity::ZipDirectory(path, text) => {
            path_bytes(path) + text.capacity()
        }
        ScriptItemIdentity::Stack(text, path) => text.capacity() + path_bytes(path),
    }
}

fn approx_owned_heap_bytes(identity: &ScriptPageIdentity) -> usize {
    // Retained per-identity heap estimate, NOT allocator instrumentation or total
    // allocated bytes. Vec/String capacities are counted; paths use encoded byte
    // lengths. Excludes allocator headers/rounding, PathBuf spare capacity, temporary
    // allocations, stack storage, and the composition Arc's already-owned backing.
    let ScriptLocation::Physical { path, prefix } = &identity.location else {
        panic!("measurement assumes a physical book location");
    };
    assert!(matches!(
        identity.scalars.presentation_source,
        SpreadPresentationSource::Other
    ));
    assert!(identity.scalars.browser_output_root.is_none());
    assert!(identity.scalars.extracted_surface.is_none());
    path.as_deref().map_or(0, path_bytes)
        + prefix.capacity() * size_of::<String>()
        + prefix.iter().map(String::capacity).sum::<usize>()
        + identity.reader_order.capacity() * size_of::<ScriptItemIdentity>()
        + identity
            .reader_order
            .iter()
            .map(item_heap_bytes)
            .sum::<usize>()
        + identity
            .scalars
            .item
            .iter()
            .map(item_heap_bytes)
            .sum::<usize>()
        + identity
            .scalars
            .pairing_origins
            .0
            .iter()
            .map(item_heap_bytes)
            .sum::<usize>()
        + identity
            .scalars
            .pairing_origins
            .1
            .iter()
            .map(item_heap_bytes)
            .sum::<usize>()
}

fn measure(app: &App, kind: &str, source: &str, mode: &str) {
    for _ in 0..WARMUP {
        drop(black_box(black_box(app).operation_script_page_identity()));
    }
    let mut construction = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        let identity = black_box(black_box(app).operation_script_page_identity());
        let elapsed = start.elapsed().as_secs_f64() * 1_000_000.0;
        // Destruction and sample bookkeeping are outside the construction timer.
        drop(identity);
        construction.push(elapsed);
    }
    // Two independent calls clone the reader items independently, but intentionally
    // share an adopted composition Arc, exactly as repeated frames would do.
    let left = black_box(app.operation_script_page_identity());
    let right = black_box(app.operation_script_page_identity());
    assert_eq!(left.reader_order.len(), app.items.len());
    assert_eq!(left, right);
    assert_eq!(
        matches!(left.unit_source, ScriptUnitSourceInputs::Composed(_)),
        source == "composed"
    );
    let approx_bytes = approx_owned_heap_bytes(&left);
    for _ in 0..WARMUP {
        assert!(black_box(black_box(&left) == black_box(&right)));
    }
    let mut equality = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        let equal = black_box(black_box(&left) == black_box(&right));
        let elapsed = start.elapsed().as_secs_f64() * 1_000_000.0;
        assert!(equal);
        equality.push(elapsed);
    }
    construction.sort_by(f64::total_cmp);
    equality.sort_by(f64::total_cmp);
    println!(
        "page_identity N={} kind={kind} mode={mode} median_us={:.3} p95_us={:.3} approx_bytes={approx_bytes} eq_us={:.3} source={source} samples={SAMPLES} warmup={WARMUP} timing=construction_only eq_p95_us={:.3} bytes=owned_heap_estimate",
        app.items.len(),
        construction[SAMPLES / 2],
        construction[(SAMPLES * 95 - 1) / 100],
        equality[SAMPLES / 2],
        equality[(SAMPLES * 95 - 1) / 100],
    );
    if app.items.len() == 50_000 {
        let mut events = OperationScriptEvents::default();
        events.observe(app.collect_operation_script_event_identity(ScriptInputSurface::Viewer));
        let mut samples = Vec::with_capacity(SAMPLES);
        for sample in 0..WARMUP + SAMPLES {
            let start = Instant::now();
            let changed = black_box(events.observe(black_box(
                app.collect_operation_script_event_identity(ScriptInputSurface::Viewer),
            )));
            let elapsed = start.elapsed().as_secs_f64() * 1_000_000.0;
            assert!(changed.is_empty());
            if sample >= WARMUP {
                samples.push(elapsed);
            }
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "event_identity_unchanged N=50000 kind={kind} mode={mode} source={source} construction_compare_median_us={:.3} p95_us={:.3} samples={SAMPLES} warmup={WARMUP} reader_order=excluded composition=strong_arc",
            samples[SAMPLES / 2],
            samples[(SAMPLES * 95 - 1) / 100]
        );
    }
}

#[test]
#[ignore = "release-only page identity measurement; run with --release --ignored --nocapture"]
fn perf_operation_script_page_identity() {
    if cfg!(debug_assertions) {
        eprintln!(
            "page_identity: debug timings are meaningless; rerun with --release --ignored --nocapture"
        );
        return;
    }
    println!(
        "page_identity_info names=200_ascii samples={SAMPLES} warmup={WARMUP} timing=construction_only_drop_excluded eq=independent_identities_shared_composition_arc bytes=retained_owned_heap_estimate excludes=allocator_overhead,path_spare_capacity,transients,stack,shared_composition"
    );
    println!(
        "page_identity_skip kind=image source=composed reason=image_folder_composition_adoption_is_unsupported_in_composition_scope_domain"
    );
    // Exactly one setup_app for all cases: creating a nested fixture deadlocks its
    // process-wide data-dir mutex. Neither timing loop creates a fixture or does I/O.
    let mut env = setup_app();
    for count in [1_000, 10_000, 50_000] {
        for (zip, kind) in [(true, "zip"), (false, "image")] {
            prepare_rows(&mut env, count, zip);
            for source in ["automatic", "composed"] {
                if source == "composed" {
                    if !zip {
                        continue;
                    }
                    install_zip_composition(&mut env);
                }
                for (mode, label) in [(SpreadMode::Single, "single"), (SpreadMode::Ltr, "spread")] {
                    env.spread_mode = mode;
                    measure(&env, kind, source, label);
                }
            }
        }
    }
}
