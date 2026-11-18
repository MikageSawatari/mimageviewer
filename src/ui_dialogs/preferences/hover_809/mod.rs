//! §1.809 opt-in measurement tools only. RawInput is the sole input source.
//! No eframe App::logic/ui, native event loop, desktop queries, surface or present.

use super::{PreferencesState, hover_perf};
use egui::epaint::{ClippedPrimitive, ImageDelta, Primitive};
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

mod gpu_tests;

const RADIUS_PIXELS: f32 = 40.0;

pub(super) const GEOMETRIES: [([u32; 2], f32); 4] = [
    ([2560, 1440], 1.0),
    ([2560, 1440], 1.5),
    ([3840, 2160], 1.0),
    ([3840, 2160], 1.5),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scene {
    Preferences,
    Keyboard,
    Chrome,
    Thumbnails,
    ChromeOnly,
    ThumbnailsOnly,
}

impl Scene {
    pub(super) const ALL: [Self; 4] = [
        Self::Preferences,
        Self::Keyboard,
        Self::Chrome,
        Self::Thumbnails,
    ];
    pub(super) const COMPONENTS: [Self; 2] = [Self::ChromeOnly, Self::ThumbnailsOnly];

    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Preferences => "preferences_tree_full",
            Self::Keyboard => "keyboard_full",
            Self::Chrome => "menu_toolbar_full",
            Self::Thumbnails => "thumbnails_full",
            Self::ChromeOnly => "menu_toolbar_only",
            Self::ThumbnailsOnly => "thumbnails_only",
        }
    }

    fn paints_chrome(self) -> bool {
        match self {
            Self::ThumbnailsOnly => false,
            Self::Preferences
            | Self::Keyboard
            | Self::Chrome
            | Self::Thumbnails
            | Self::ChromeOnly => true,
        }
    }

    fn paints_grid(self) -> bool {
        match self {
            Self::ChromeOnly => false,
            Self::Preferences
            | Self::Keyboard
            | Self::Chrome
            | Self::Thumbnails
            | Self::ThumbnailsOnly => true,
        }
    }
}

pub(super) struct Capture {
    pub jobs: Vec<ClippedPrimitive>,
    /// Ordered full/partial updates from Context creation through this exact frame.
    pub texture_sets: Vec<(egui::TextureId, ImageDelta)>,
    pub source: Value,
}

struct Fixture {
    // The existing fixture owns App, workers, override, directory and global lock in drop order.
    env: crate::app::tests::phase_c_support::AppTestEnv,
    state: PreferencesState,
    ctx: egui::Context,
    scene: Scene,
    size: [u32; 2],
    ppp: f32,
    texture_sets: Vec<(egui::TextureId, ImageDelta)>,
}

impl Fixture {
    fn new(scene: Scene, size: [u32; 2], ppp: f32) -> Self {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        let mut env = crate::app::tests::phase_c_support::setup_app();
        let folder = env.tmp.path().join("synthetic-images");
        std::fs::create_dir(&folder).unwrap();
        env.current_folder = Some(folder.clone());
        env.settings.grid_cols = 12;
        // No files, decode requests, directory scans or pending thumbnail work are needed.
        env.items = (0..96)
            .map(|i| crate::grid_item::GridItem::Image(folder.join(format!("image-{i:03}.png"))))
            .collect();
        env.thumbnails = (0..96)
            .map(|i| {
                let pixels = (0..64 * 48)
                    .map(|p| {
                        egui::Color32::from_rgb(
                            (p % 64 * 4) as u8,
                            (p / 64 * 5) as u8,
                            (i * 17) as u8,
                        )
                    })
                    .collect();
                let tex = ctx.load_texture(
                    format!("hover809-{i}"),
                    egui::ColorImage::new([64, 48], pixels),
                    egui::TextureOptions::LINEAR,
                );
                crate::grid_item::ThumbnailState::loaded_raster(
                    tex,
                    crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
                    false,
                    64,
                    crate::source_coverage::SourceCoverage::FullSource,
                    Some((64, 48)),
                    None,
                )
            })
            .collect();
        env.visible_indices = (0..96).collect();
        env.details_order = env.visible_indices.clone();
        env.image_metas = vec![Some((0, 64 * 48 * 4)); 96];
        env.show_preferences = scene == Scene::Preferences;
        env.show_operation_customize = scene == Scene::Keyboard;
        env.ensure_book_list_loaded();
        let deadline = Instant::now() + Duration::from_secs(10);
        while env.book_op_pending.is_some() {
            env.poll_book_op_pending(&ctx);
            assert!(
                Instant::now() < deadline,
                "fixture empty bookshelf must settle"
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(env.book_shelf.snapshot.is_some());
        Self {
            env,
            state: hover_perf::state(false),
            ctx,
            scene,
            size,
            ppp,
            texture_sets: Vec::new(),
        }
    }

    fn frame(
        &mut self,
        time: f64,
        hz: u32,
        pointer: Option<egui::Pos2>,
    ) -> (
        Value,
        Vec<ClippedPrimitive>,
        Vec<egui::epaint::ClippedShape>,
    ) {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(
                    self.size[0] as f32 / self.ppp,
                    self.size[1] as f32 / self.ppp,
                ),
            )),
            time: Some(time),
            predicted_dt: 1.0 / hz as f32,
            events: pointer.map(egui::Event::PointerMoved).into_iter().collect(),
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(self.ppp);
        let start = Instant::now();
        let mut chrome_bottom = 0.0;
        let mut output = self.ctx.run_ui(input, |ui| {
            if self.scene.paints_chrome() {
                let (nav, changed) = self.env.render_menubar(ui);
                assert!(nav.is_none() && !changed);
            }
            if matches!(self.scene, Scene::Preferences | Scene::Keyboard) {
                hover_perf::draw_809(ui.ctx(), &mut self.state, self.scene == Scene::Keyboard);
            }
            if self.scene.paints_chrome() {
                let (nav, address) = self.env.render_toolbar(ui);
                assert!(nav.is_none() && address.is_none());
                chrome_bottom = ui.available_rect_before_wrap().top();
            }
            if self.scene.paints_grid() {
                assert!(self.env.render_grid(ui).is_none());
            }
        });
        let run_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(output.viewport_output.len(), 1, "single ROOT fixture only");
        assert_eq!(output.pixels_per_point, self.ppp);
        let delay = output.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
        let texture_set_count = output.textures_delta.set.len();
        let texture_free_count = output.textures_delta.free.len();
        // Handles are retained by the fixture; do not replay a capture with retired textures.
        assert_eq!(texture_free_count, 0);
        for (id, deltas) in output.textures_delta.set.drain() {
            // IDs are independent; preserve the ordered patches within each ID/frame.
            self.texture_sets
                .extend(deltas.into_iter().map(|delta| (id, delta)));
        }
        let shapes = std::mem::take(&mut output.shapes);
        let tess_shapes = shapes.clone();
        let start = Instant::now();
        let jobs = self.ctx.tessellate(tess_shapes, output.pixels_per_point);
        let tessellate_ms = start.elapsed().as_secs_f64() * 1000.0;
        let (vertices, indices, callbacks) = geometry_counts(&jobs);
        let mut hovered = self
            .ctx
            .interaction_snapshot(|s| s.hovered.iter().map(|id| id.value()).collect::<Vec<_>>());
        hovered.sort_unstable();
        let actual_pointer = self.ctx.input(|i| i.pointer.hover_pos());
        let sample = json!({"synthetic_time_s": time, "pointer": actual_pointer.map(|p| [p.x,p.y]), "pointer_move_event": pointer.is_some(),
            "chrome_bottom_points":chrome_bottom,
            "run_ms": run_ms, "tessellate_ms": tessellate_ms, "cpu_ms": run_ms + tessellate_ms,
            "passes": output.platform_output.num_completed_passes,
            "repaint_requested": delay != Duration::MAX,
            "repaint_immediate": delay.is_zero(),
            "repaint_delay_ms": (delay != Duration::MAX).then(|| delay.as_secs_f64()*1000.0),
            "textures_delta_empty": texture_set_count == 0 && texture_free_count == 0,
            "texture_sets": texture_set_count, "texture_frees": texture_free_count,
            "vertices": vertices, "indices": indices, "draw_calls": jobs.len(), "callbacks": callbacks,
            "hovered_widget_ids": hovered});
        output.drop_without_applying_deltas();
        (sample, jobs, shapes)
    }

    fn capture(&self, jobs: &[ClippedPrimitive], source: Value) -> Capture {
        Capture {
            jobs: jobs.to_vec(),
            texture_sets: self.texture_sets.clone(),
            source,
        }
    }

    fn warmup(&mut self) -> (egui::Pos2, Vec<ClippedPrimitive>, Vec<(String, egui::Rect)>) {
        let mut shapes = Vec::new();
        for n in 0..40 {
            shapes = self.frame(n as f64 / 60.0, 60, None).2;
        }
        let center = match self.scene {
            Scene::Preferences => {
                let labels = ["全体設定"];
                let rect = labels
                    .iter()
                    .find_map(|label| hover_perf::text_rect(&shapes, label))
                    .or_else(|| hover_perf::text_rect(&shapes, "▶ 表示"))
                    .expect("preferences tree label visible");
                rect.center() + egui::vec2(0.0, 55.0)
            }
            Scene::Keyboard => {
                let interactive = self.ctx.interactive_rects_last_pass();
                let g = text_widget_rect(&shapes, &interactive, "G").expect("keyboard G visible");
                let h = text_widget_rect(&shapes, &interactive, "H").expect("keyboard H visible");
                // A circle about one narrow key can run almost entirely through
                // gaps. Use the middle of two real caps in the main keyboard.
                egui::pos2((g.center().x + h.center().x) * 0.5, g.center().y)
            }
            // The circle can extend below the bars; hit evidence uses actual chrome bounds.
            Scene::Chrome | Scene::ChromeOnly => egui::pos2(180.0, 45.0),
            Scene::Thumbnails | Scene::ThumbnailsOnly => {
                egui::pos2(self.size[0] as f32 / self.ppp * 0.5, 200.0)
            }
        };
        let mut previous = Vec::new();
        let mut chrome_bottom = 0.0;
        for n in 40..120 {
            let (sample, jobs, painted) = self.frame(
                n as f64 / 60.0,
                60,
                (n == 40).then_some(center + egui::vec2(RADIUS_PIXELS / self.ppp, 0.0)),
            );
            previous = jobs;
            shapes = painted;
            chrome_bottom = sample["chrome_bottom_points"].as_f64().unwrap() as f32;
        }
        let targets = self.target_rects(&previous, &shapes, chrome_bottom);
        (center, previous, targets)
    }

    fn target_rects(
        &self,
        jobs: &[ClippedPrimitive],
        shapes: &[egui::epaint::ClippedShape],
        chrome_bottom: f32,
    ) -> Vec<(String, egui::Rect)> {
        let interactive = self.ctx.interactive_rects_last_pass();
        match self.scene {
            Scene::Thumbnails | Scene::ThumbnailsOnly => jobs
                .iter()
                .filter_map(|job| {
                    let Primitive::Mesh(mesh) = &job.primitive else {
                        return None;
                    };
                    if mesh.texture_id == egui::TextureId::Managed(0) {
                        return None;
                    }
                    let rect = mesh
                        .vertices
                        .iter()
                        .fold(egui::Rect::NOTHING, |r, v| {
                            r.union(egui::Rect::from_min_max(v.pos, v.pos))
                        })
                        .intersect(job.clip_rect);
                    rect.is_positive()
                        .then(|| (format!("image_{:?}", mesh.texture_id), rect))
                })
                .collect(),
            Scene::Chrome | Scene::ChromeOnly => interactive
                .into_iter()
                .enumerate()
                .filter(|(_, r)| {
                    r.min.y >= 0.0
                        && r.max.y <= chrome_bottom
                        && r.width() > 3.0
                        && r.height() < 100.0
                })
                .map(|(i, r)| (format!("bar_control_{i}"), r))
                .collect(),
            Scene::Preferences | Scene::Keyboard => {
                let labels: Vec<String> = if self.scene == Scene::Keyboard {
                    (1..=24)
                        .map(|n| format!("F{n}"))
                        .chain(('A'..='Z').map(|c| c.to_string()))
                        .collect()
                } else {
                    super::TREE
                        .iter()
                        .map(|c| {
                            if c.children.is_empty() {
                                c.label.to_owned()
                            } else {
                                format!("▶ {}", c.label)
                            }
                        })
                        .collect()
                };
                labels
                    .into_iter()
                    .filter_map(|label| {
                        text_widget_rect(shapes, &interactive, &label).map(|r| (label, r))
                    })
                    .collect()
            }
        }
    }
}

fn text_widget_rect(
    shapes: &[egui::epaint::ClippedShape],
    interactive: &[egui::Rect],
    label: &str,
) -> Option<egui::Rect> {
    let text = hover_perf::text_rect(shapes, label)?;
    interactive
        .iter()
        .copied()
        .filter(|r| r.contains(text.center()) && r.width() < 250.0 && r.height() < 60.0)
        .min_by(|a, b| a.area().total_cmp(&b.area()))
}

pub(super) fn geometry_counts(jobs: &[ClippedPrimitive]) -> (usize, usize, usize) {
    jobs.iter()
        .fold((0, 0, 0), |(v, i, c), job| match &job.primitive {
            Primitive::Mesh(mesh) => (v + mesh.vertices.len(), i + mesh.indices.len(), c),
            Primitive::Callback(_) => (v, i, c + 1),
        })
}

// Exact ordered values, not a hash. Float bits include signed zero and NaN payloads.
fn identical(a: &[ClippedPrimitive], b: &[ClippedPrimitive]) -> bool {
    let pos = |p: egui::Pos2, q: egui::Pos2| {
        p.x.to_bits() == q.x.to_bits() && p.y.to_bits() == q.y.to_bits()
    };
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            pos(a.clip_rect.min, b.clip_rect.min)
                && pos(a.clip_rect.max, b.clip_rect.max)
                && match (&a.primitive, &b.primitive) {
                    (Primitive::Mesh(a), Primitive::Mesh(b)) => {
                        a.texture_id == b.texture_id
                            && a.indices == b.indices
                            && a.vertices.len() == b.vertices.len()
                            && a.vertices.iter().zip(&b.vertices).all(|(a, b)| {
                                pos(a.pos, b.pos) && pos(a.uv, b.uv) && a.color == b.color
                            })
                    }
                    _ => false, // Callback identity is never proof of equal rendered content.
                }
        })
}

pub(super) fn distribution(mut values: Vec<f64>) -> Value {
    assert!(!values.is_empty());
    values.sort_by(f64::total_cmp);
    let q = |q: f64| {
        let r = (values.len() - 1) as f64 * q;
        let lo = r.floor() as usize;
        let hi = r.ceil() as usize;
        values[lo] + (values[hi] - values[lo]) * r.fract()
    };
    json!({"median":q(0.5),"p90":q(0.9),"max":values.last().unwrap()})
}

pub(super) fn write_json(env: &str, value: &Value) {
    let relative = PathBuf::from(
        std::env::var_os(env)
            .unwrap_or_else(|| panic!("set {env} to a new tree-relative JSON path")),
    );
    assert!(
        !relative.is_absolute()
            && relative
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .canonicalize()
        .unwrap();
    let path = root.join(relative);
    let parent = path
        .parent()
        .unwrap()
        .canonicalize()
        .expect("JSON parent must exist");
    assert!(parent.starts_with(root));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .expect("new JSON inside this tree");
    use std::io::Write;
    file.write_all(&serde_json::to_vec_pretty(value).unwrap())
        .unwrap();
}

fn measure(
    scene: Scene,
    size: [u32; 2],
    ppp: f32,
    hz: u32,
    moving: bool,
    seconds: f64,
    capture: bool,
) -> (Value, Vec<(&'static str, Capture)>) {
    let mut fixture = Fixture::new(scene, size, ppp);
    let initial = serde_json::to_value(&fixture.state.settings).unwrap();
    let (center, mut previous, targets) = fixture.warmup();
    assert!(
        !targets.is_empty(),
        "scene must expose actual widget/image rectangles"
    );
    let mut visited = std::collections::HashSet::new();
    let mut target_frames = 0;
    let frames = (hz as f64 * seconds).round() as usize;
    let mut samples = Vec::new();
    let mut representatives: Vec<(&'static str, Capture)> = Vec::new();
    let radius = RADIUS_PIXELS / ppp;
    let mut previous_hover = fixture.ctx.interaction_snapshot(|s| {
        let mut ids = s.hovered.iter().map(|id| id.value()).collect::<Vec<_>>();
        ids.sort_unstable();
        json!(ids)
    });
    for n in 0..frames {
        let angle = std::f64::consts::TAU * n as f64 / hz as f64;
        let pointer = moving
            .then(|| center + egui::vec2(angle.cos() as f32 * radius, angle.sin() as f32 * radius));
        let (mut sample, jobs, _) = fixture.frame(2.0 + n as f64 / hz as f64, hz, pointer);
        assert_eq!(sample["passes"], 1, "measured frames require one pass");
        let same = identical(&previous, &jobs);
        let hover_changed = sample["hovered_widget_ids"] != previous_hover;
        previous_hover = sample["hovered_widget_ids"].clone();
        let eligible = same && sample["textures_delta_empty"] == true && sample["callbacks"] == 0;
        sample["frame"] = json!(n);
        sample["identical_primitives"] = json!(same);
        sample["hover_target_changed"] = json!(hover_changed);
        sample["skip_candidate"] = json!(eligible);
        let actual_pointer = fixture.ctx.input(|i| i.pointer.hover_pos()).unwrap();
        let target = targets.iter().position(|(_, r)| r.contains(actual_pointer));
        if let Some(target) = target {
            visited.insert(target);
            target_frames += 1;
        }
        sample["fixture_target"] = json!(target.map(|i| &targets[i].0));
        // Retain every observed class, preferring its first sustained frame.
        // A class occurring only during the initial transition is still real.
        if capture {
            let class = if same { "same" } else { "changed" };
            let existing = representatives
                .iter()
                .position(|(label, _)| *label == class);
            let first_sustained = existing.is_some_and(|i| {
                n >= hz as usize / 4
                    && representatives[i].1.source["frame"].as_u64().unwrap() < u64::from(hz / 4)
            });
            if existing.is_none() || first_sustained {
                let mut source = sample.clone();
                source["scene"] = json!(scene.name());
                source["size_in_pixels"] = json!(size);
                source["pixels_per_point"] = json!(ppp);
                source["hz"] = json!(hz);
                source["moving"] = json!(moving);
                source["radius_pixels"] = json!(RADIUS_PIXELS);
                source["radius_points"] = json!(radius);
                let captured = (class, fixture.capture(&jobs, source));
                if let Some(i) = existing {
                    representatives[i] = captured;
                } else {
                    representatives.push(captured);
                }
            }
        }
        previous = jobs;
        samples.push(sample);
    }
    assert_eq!(
        initial,
        serde_json::to_value(&fixture.state.settings).unwrap()
    );
    if moving {
        assert!(
            target_frames >= frames / 20,
            "circle must pass actual scene targets"
        );
    }
    let count = |field: &str| samples.iter().filter(|s| s[field] == true).count();
    let same = count("identical_primitives");
    let changed = frames - same;
    let eligible = count("skip_candidate");
    let result = json!({"scene":scene.name(),"size_in_pixels":size,"pixels_per_point":ppp,"hz":hz,
        "moving":moving,"seconds":seconds,"frames":frames,"center_points":[center.x,center.y],"radius_pixels":RADIUS_PIXELS,"radius_points":radius,"revolutions_per_second":1,
        "identical_frames":same,"changed_frames":changed,"hover_target_changes":count("hover_target_changed"),
        "identical_ratio":same as f64/frames as f64,"skip_candidate_frames":eligible,"skip_candidate_ratio":eligible as f64/frames as f64,
        "immediate_repaint_frames":count("repaint_immediate"),
        "target_frames":target_frames,"distinct_targets":visited.len(),
        "target_geometry":targets.iter().map(|(label,r)|json!({"label":label,"rect":[r.min.x,r.min.y,r.max.x,r.max.y]})).collect::<Vec<_>>(),
        "vertices":distribution(samples.iter().map(|s|s["vertices"].as_f64().unwrap()).collect()),
        "cpu_ms":distribution(samples.iter().map(|s|s["cpu_ms"].as_f64().unwrap()).collect()),"samples":samples});
    (result, representatives)
}

pub(super) fn representatives(
    scene: Scene,
    size: [u32; 2],
    ppp: f32,
) -> Vec<(&'static str, Capture)> {
    let mut captures = measure(scene, size, ppp, 144, true, 1.0, true).1;
    if !captures.iter().any(|(class, _)| *class == "same") {
        // A includes this stationary control too. Keep a real identical frame
        // available even when sustained movement changes every frame.
        if let Some(same) = measure(scene, size, ppp, 144, false, 1.0, true)
            .1
            .into_iter()
            .find(|(class, _)| *class == "same")
        {
            captures.push(same);
        }
    }
    captures
}

#[test]
#[ignore = "opt-in CPU-only §1.809 measurement; no device, native window or desktop input"]
fn hover_809_identical_frames_cpu_measurement() {
    let seconds = std::env::var("MIV_HOVER_809_SECONDS")
        .map(|s| s.parse::<f64>().unwrap())
        .unwrap_or(5.0);
    assert!(seconds.is_finite() && (1.0..=5.0).contains(&seconds));
    let mut cases = Vec::new();
    for (size, ppp) in GEOMETRIES {
        for scene in Scene::ALL {
            for hz in [60, 144, 240] {
                for moving in [true, false] {
                    let (case, _) = measure(scene, size, ppp, hz, moving, seconds, false);
                    println!(
                        "HOVER809 {} {} ppp={} {}Hz moving={} identical={}/{} changed={} hover_changes={} vertices={}",
                        scene.name(),
                        size[0],
                        ppp,
                        hz,
                        moving,
                        case["identical_frames"],
                        case["frames"],
                        case["changed_frames"],
                        case["hover_target_changes"],
                        case["vertices"]["median"]
                    );
                    cases.push(case);
                }
            }
        }
    }
    let mut capture_preflight = Vec::new();
    if std::env::var("MIV_HOVER_809_CAPTURE_SMOKE").as_deref() == Ok("1") {
        for (size, ppp) in GEOMETRIES {
            for scene in Scene::ALL.into_iter().chain(Scene::COMPONENTS) {
                for (class, capture) in representatives(scene, size, ppp) {
                    let textures: std::collections::HashSet<_> = capture
                        .texture_sets
                        .iter()
                        .filter(|(_, delta)| delta.pos.is_none())
                        .map(|(id, _)| *id)
                        .collect();
                    assert!(!capture.jobs.is_empty());
                    for job in &capture.jobs {
                        let Primitive::Mesh(mesh) = &job.primitive else {
                            panic!("fixture must use ordinary egui meshes, no callbacks")
                        };
                        assert!(
                            textures.contains(&mesh.texture_id),
                            "capture must include each mesh texture's initialization"
                        );
                    }
                    assert_eq!(capture.source["identical_primitives"], class == "same");
                    capture_preflight.push(json!({"class":class,"source_cpu_sample":capture.source,"primitives":capture.jobs.len(),"texture_deltas":capture.texture_sets.len()}));
                }
            }
        }
    }
    write_json(
        "MIV_HOVER_809_JSON",
        &json!({"section":"1.809","measurement":"A","clock":"synthetic RawInput; forced frames even when repaint has no deadline","cpu_timing":"run_ui + tessellate, excludes shape clone, exact comparison, JSON, fixture setup","cases":cases,"capture_preflight":capture_preflight}),
    );
}
