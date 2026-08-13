mod ai_job;
mod archive_job;
mod collections;
mod container;
mod heavy_queue;
mod live_favorites;
mod long_job;
mod page_jobs;
mod path_guard;
mod service;
mod thumbnail;
mod video_jump;
mod video_stream;

#[cfg(windows)]
mod pipe;
pub(crate) mod session;
pub(crate) use service::{RemoteServiceControl, RemoteServiceManager, RemoteServiceStatus};

#[cfg(test)]
pub(crate) use container::resolve_remote_effective_params_for_test;
pub(crate) mod ui;

pub(super) const BOOK_SORT_LOCK_REASON: &str =
    "本として表示中は名前順固定です（一覧の並べ替えは使えません）。";
pub(super) const FIXED_LIST_SORT_LOCK_REASON: &str = "この一覧では並び順が固定されています。";
/// Dynamic JSON budget for container and collection entry lists.
///
/// The IPC reader accepts 64 MiB response frames. Capping the repeated list data at
/// 40 MiB leaves 24 MiB for the response envelope, fixed payload fields, and future
/// protocol additions without making ordinary 100,000-entry lists hit the wire limit.
pub(super) const REMOTE_LIST_RESPONSE_BUDGET_BYTES: usize = 40 * 1024 * 1024;
const MAX_REMOTE_AGGREGATE_SIDECAR_PARENT_SCANS: usize = 64;

pub(super) fn serialized_json_len<T: serde::Serialize>(value: &T) -> usize {
    struct ByteCounter(usize);

    impl std::io::Write for ByteCounter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0 = self.0.saturating_add(bytes.len());
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut counter = ByteCounter(0);
    serde_json::to_writer(&mut counter, value)
        .expect("remote IPC response values must serialize to JSON");
    counter.0
}

pub(super) fn response_entry_limit(
    configured_limit: usize,
    returned: usize,
    truncated: bool,
) -> usize {
    if truncated {
        returned
    } else {
        configured_limit
    }
}

/// Remote grid thumbnail provenance shared by physical folders and aggregate lists.
///
/// Sidecar discovery remains owned by `folder_scan::filter_video_image_duplicates`.
/// Aggregate lists only group videos by physical parent and feed one scan per parent
/// into that existing rule; they never remove entries from the aggregate result set.
#[derive(Default)]
pub(super) struct RemoteThumbnailSources {
    video_sidecars: std::collections::HashMap<String, std::path::PathBuf>,
}

impl RemoteThumbnailSources {
    pub(super) fn from_pairs(pairs: &[(std::path::PathBuf, std::path::PathBuf)]) -> Self {
        let mut video_sidecars = std::collections::HashMap::new();
        for (video, image) in pairs {
            video_sidecars.insert(crate::path_key::normalize_keep_drive(video), image.clone());
        }
        Self { video_sidecars }
    }

    pub(super) fn for_remote_entries(
        settings: &crate::settings::Settings,
        entries: &[mimageviewer_ipc::RemoteEntry],
    ) -> Self {
        if !settings.skip_image_if_video_exists || !settings.video_thumb_use_sidecar_image {
            return Self::default();
        }

        let videos = entries
            .iter()
            .filter(|entry| entry.kind == mimageviewer_ipc::RemoteEntryKind::Video)
            .map(|entry| std::path::PathBuf::from(&entry.path))
            .collect::<Vec<_>>();
        if videos.is_empty() {
            return Self::default();
        }

        let requested = videos
            .iter()
            .map(|path| crate::path_key::normalize_keep_drive(path))
            .collect::<std::collections::HashSet<_>>();
        let mut parent_keys = std::collections::HashSet::new();
        let mut parents = Vec::new();
        for video in &videos {
            if let Some(parent) = video.parent() {
                let key = crate::path_key::normalize_keep_drive(parent);
                if parent_keys.insert(key) {
                    parents.push(parent.to_path_buf());
                }
            }
        }

        let skipped_parents = parents
            .len()
            .saturating_sub(MAX_REMOTE_AGGREGATE_SIDECAR_PARENT_SCANS);
        if skipped_parents > 0 {
            crate::logger::log(format!(
                "remote_ipc: aggregate sidecar parent scan capped limit={} scanned={} skipped_parents={skipped_parents}",
                MAX_REMOTE_AGGREGATE_SIDECAR_PARENT_SCANS,
                MAX_REMOTE_AGGREGATE_SIDECAR_PARENT_SCANS,
            ));
        }

        // 64 parents bounds synchronous read_dir latency while covering clustered
        // aggregate results; later parents safely fall through to the Shell thumbnail.
        // With V requested videos and capped parents p, this is expected
        // O(V + sum(E_p + S_p)) time and O(V + max(E_p + S_p)) working memory.
        let mut sources = Self::default();
        for parent in parents
            .into_iter()
            .take(MAX_REMOTE_AGGREGATE_SIDECAR_PARENT_SCANS)
        {
            let mut scan =
                match crate::app::folder_scan::scan_directory_with_settings(&parent, settings) {
                    Ok(scan) => scan,
                    Err(error) => {
                        crate::logger::log(format!(
                            "remote_ipc: aggregate sidecar scan failed parent={} error={error}",
                            parent.display()
                        ));
                        continue;
                    }
                };
            let found =
                crate::app::folder_scan::filter_video_image_duplicates(&mut scan.all_media, true);
            for (video, image) in found.sidecars {
                let key = crate::path_key::normalize_keep_drive(&video);
                if requested.contains(&key) {
                    sources.video_sidecars.insert(key, image);
                }
            }
        }
        sources
    }

    pub(super) fn source_address(
        &self,
        path: &std::path::Path,
        kind: mimageviewer_ipc::RemoteEntryKind,
    ) -> Option<mimageviewer_ipc::RemoteAddress> {
        if kind != mimageviewer_ipc::RemoteEntryKind::Video {
            return None;
        }
        let source = self
            .video_sidecars
            .get(&crate::path_key::normalize_keep_drive(path))?;
        let logical = path_guard::resolve_existing(source.to_string_lossy().as_ref())
            .map(|resolved| resolved.logical)
            .unwrap_or_else(|_| source.clone());
        Some(mimageviewer_ipc::RemoteAddress::file(
            logical.to_string_lossy().into_owned(),
        ))
    }

    pub(super) fn populate_remote_entries(&self, entries: &mut [mimageviewer_ipc::RemoteEntry]) {
        for entry in entries {
            entry.thumbnail_address =
                self.source_address(std::path::Path::new(&entry.path), entry.kind);
        }
    }
}

#[cfg(test)]
mod remote_thumbnail_source_tests {
    use super::*;

    fn entry(path: &std::path::Path) -> mimageviewer_ipc::RemoteEntry {
        mimageviewer_ipc::RemoteEntry {
            path: path.to_string_lossy().into_owned(),
            thumbnail_address: None,
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            kind: mimageviewer_ipc::RemoteEntryKind::Video,
            detail: None,
            progress_current: None,
            progress_total: None,
            rating: None,
        }
    }

    fn create_video_and_sidecar(root: &std::path::Path, index: usize) -> std::path::PathBuf {
        let parent = root.join(format!("parent-{index:03}"));
        std::fs::create_dir(&parent).unwrap();
        let video = parent.join("clip.mp4");
        std::fs::write(&video, b"video").unwrap();
        std::fs::write(parent.join("clip.jpg"), b"sidecar").unwrap();
        video
    }

    #[test]
    fn aggregate_sidecar_scan_covers_every_parent_below_the_limit() {
        let temp = tempfile::tempdir().unwrap();
        let videos = (0..3)
            .map(|index| create_video_and_sidecar(temp.path(), index))
            .collect::<Vec<_>>();
        let entries = videos.iter().map(|path| entry(path)).collect::<Vec<_>>();

        let sources = RemoteThumbnailSources::for_remote_entries(
            &crate::settings::Settings::default(),
            &entries,
        );

        for video in videos {
            assert!(
                sources
                    .source_address(&video, mimageviewer_ipc::RemoteEntryKind::Video)
                    .is_some()
            );
        }
    }

    #[test]
    fn aggregate_sidecar_scan_caps_parents_and_leaves_the_rest_for_shell() {
        let temp = tempfile::tempdir().unwrap();
        let videos = (0..=MAX_REMOTE_AGGREGATE_SIDECAR_PARENT_SCANS)
            .map(|index| create_video_and_sidecar(temp.path(), index))
            .collect::<Vec<_>>();
        let entries = videos.iter().map(|path| entry(path)).collect::<Vec<_>>();

        let sources = RemoteThumbnailSources::for_remote_entries(
            &crate::settings::Settings::default(),
            &entries,
        );

        for video in videos
            .iter()
            .take(MAX_REMOTE_AGGREGATE_SIDECAR_PARENT_SCANS)
        {
            assert!(
                sources
                    .source_address(video, mimageviewer_ipc::RemoteEntryKind::Video)
                    .is_some()
            );
        }
        assert!(
            sources
                .source_address(
                    &videos[MAX_REMOTE_AGGREGATE_SIDECAR_PARENT_SCANS],
                    mimageviewer_ipc::RemoteEntryKind::Video,
                )
                .is_none()
        );
    }
}

pub(super) fn sort_order_wire_value(order: crate::settings::SortOrder) -> String {
    serde_json::to_value(order)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .expect("SortOrder serializes as a string")
}

pub(super) fn parse_sort_order_wire(
    value: &str,
) -> Result<crate::settings::SortOrder, &'static str> {
    serde_json::from_value(serde_json::Value::String(value.to_owned()))
        .map_err(|_| "並び順が正しくありません")
}

pub(super) fn remote_grid_sort_state(
    selected: crate::settings::SortOrder,
    locked_reason: Option<&str>,
) -> mimageviewer_ipc::RemoteGridSortState {
    mimageviewer_ipc::RemoteGridSortState {
        selected: sort_order_wire_value(selected),
        options: crate::settings::SortOrder::all()
            .iter()
            .copied()
            .map(|order| mimageviewer_ipc::RemoteGridSortOption {
                value: sort_order_wire_value(order),
                label: order.label().to_owned(),
                short_label: order.short_label().to_owned(),
            })
            .collect(),
        locked_reason: locked_reason.map(str::to_owned),
    }
}

pub(super) fn post_filter_wire_value(filter: crate::adjustment::PostFilter) -> String {
    serde_json::to_value(filter)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .expect("PostFilter serializes as a string")
}

pub(super) fn parse_post_filter_wire(
    value: &str,
) -> Result<crate::adjustment::PostFilter, &'static str> {
    serde_json::from_value(serde_json::Value::String(value.to_owned()))
        .map_err(|_| "ポストフィルタの選択値が不正です")
}

pub(super) fn remote_post_filter_state(
    selected: crate::adjustment::PostFilter,
) -> mimageviewer_ipc::RemotePostFilterState {
    mimageviewer_ipc::RemotePostFilterState {
        selected: post_filter_wire_value(selected),
        groups: crate::adjustment::POST_FILTER_GROUPS
            .iter()
            .map(|group| mimageviewer_ipc::RemotePostFilterGroup {
                label: group.label.to_owned(),
                options: group
                    .filters
                    .iter()
                    .copied()
                    .map(|filter| mimageviewer_ipc::RemotePostFilterOption {
                        value: post_filter_wire_value(filter),
                        label: filter.display_label().to_owned(),
                        rewrites_pixels: filter.rewrites_pixels(),
                    })
                    .collect(),
            })
            .collect(),
    }
}

pub(super) fn normalize_remote_view_trim_state(
    value: &serde_json::Value,
) -> Result<crate::view_trim::ViewTrimBookState, &'static str> {
    let mut state: crate::view_trim::ViewTrimBookState =
        serde_json::from_value(value.clone()).map_err(|_| "表示トリム設定が正しくありません")?;
    state.apply_mode = crate::view_trim::view_trim_base_apply_mode(state.apply_mode);
    state.book_settings = state.book_settings.clamped();
    Ok(state)
}

pub(super) enum RemoteSortSettingsSource {
    Live,
    #[cfg(test)]
    Snapshot(crate::settings::SortOrder),
}

impl RemoteSortSettingsSource {
    pub(super) fn load(&self) -> Result<crate::settings::SortOrder, String> {
        match self {
            Self::Live => crate::settings_db::with_db_result(|db| db.load_sort_order())
                .map_err(|error| error.to_string()),
            #[cfg(test)]
            Self::Snapshot(order) => Ok(*order),
        }
    }
}

pub(crate) fn remote_adjustment_values(
    params: &crate::adjustment::AdjustParams,
) -> mimageviewer_ipc::RemoteAdjustmentValues {
    mimageviewer_ipc::RemoteAdjustmentValues {
        brightness: params.brightness,
        contrast: params.contrast,
        gamma: params.gamma,
        saturation: params.saturation,
        temperature: params.temperature,
        black_point: params.black_point,
        white_point: params.white_point,
        midtone: params.midtone,
        auto_mode: params.auto_mode.map(|mode| match mode {
            crate::adjustment::AutoMode::Auto => mimageviewer_ipc::RemoteAutoMode::Auto,
            crate::adjustment::AutoMode::MangaCleanup => {
                mimageviewer_ipc::RemoteAutoMode::MangaCleanup
            }
        }),
        colorize: remote_colorize_params(&params.colorize),
        post_filter: Some(post_filter_wire_value(params.post_filter)),
        ai: Some(mimageviewer_ipc::RemoteAiAdjustmentValues {
            upscale_model: params.upscale_model.clone(),
            denoise_model: params.denoise_model.clone(),
        }),
    }
}

pub(crate) fn remote_colorize_params(
    params: &crate::colorize::ColorizeParams,
) -> mimageviewer_ipc::RemoteColorizeParams {
    mimageviewer_ipc::RemoteColorizeParams {
        mode: match params.mode {
            crate::colorize::ColorizeMode::Disabled => {
                mimageviewer_ipc::RemoteColorizeMode::Disabled
            }
            crate::colorize::ColorizeMode::MonochromeOnly => {
                mimageviewer_ipc::RemoteColorizeMode::MonochromeOnly
            }
            crate::colorize::ColorizeMode::AllImages => {
                mimageviewer_ipc::RemoteColorizeMode::AllImages
            }
        },
        mono_tolerance: params.mono_tolerance,
        palette: match params.palette {
            crate::colorize::ColorizePalette::Legacy4Color => {
                mimageviewer_ipc::RemoteColorizePalette::Legacy4Color
            }
            crate::colorize::ColorizePalette::LegacySkin => {
                mimageviewer_ipc::RemoteColorizePalette::LegacySkin
            }
            crate::colorize::ColorizePalette::Custom => {
                mimageviewer_ipc::RemoteColorizePalette::Custom
            }
        },
        control_points: params
            .control_points
            .iter()
            .map(|point| mimageviewer_ipc::RemoteColorizeControlPoint {
                color: point.color,
                strength: point.strength,
            })
            .collect(),
        luminance_weight: params.luminance_weight,
        density_normalization_strength: params.density_normalization_strength,
        tone_method: match params.tone_method {
            crate::colorize::ToneDensityMethod::Off => {
                mimageviewer_ipc::RemoteToneDensityMethod::Off
            }
            crate::colorize::ToneDensityMethod::Fast => {
                mimageviewer_ipc::RemoteToneDensityMethod::Fast
            }
            crate::colorize::ToneDensityMethod::LocalMean => {
                mimageviewer_ipc::RemoteToneDensityMethod::LocalMean
            }
            crate::colorize::ToneDensityMethod::Gaussian => {
                mimageviewer_ipc::RemoteToneDensityMethod::Gaussian
            }
        },
        tone_radius: params.tone_radius,
        tone_strength: params.tone_strength,
    }
}

/// 非公開の AI / filter 値を保った完全パラメータへ、remote 編集値を重ねる。
pub(crate) fn apply_remote_adjustment_values(
    mut params: crate::adjustment::AdjustParams,
    values: &mimageviewer_ipc::RemoteAdjustmentValues,
) -> Result<crate::adjustment::AdjustParams, &'static str> {
    let finite = [
        values.brightness,
        values.contrast,
        values.gamma,
        values.saturation,
        values.temperature,
        values.midtone,
    ]
    .into_iter()
    .all(f32::is_finite);
    if !finite
        || !(-100.0..=100.0).contains(&values.brightness)
        || !(-100.0..=100.0).contains(&values.contrast)
        || !(0.2..=5.0).contains(&values.gamma)
        || !(-100.0..=100.0).contains(&values.saturation)
        || !(-100.0..=100.0).contains(&values.temperature)
        || !(0.1..=10.0).contains(&values.midtone)
        || values.black_point > 254
        || values.white_point < 1
    {
        return Err("画像補正値が範囲外です");
    }
    let colorize = &values.colorize;
    if !(1..=64).contains(&colorize.mono_tolerance)
        || colorize.luminance_weight > 100
        || colorize.density_normalization_strength > 100
        || !colorize.tone_radius.is_finite()
        || !(0.1..=4.0).contains(&colorize.tone_radius)
        || colorize.tone_strength > 100
        || !(2..=10).contains(&colorize.control_points.len())
        || colorize
            .control_points
            .iter()
            .any(|point| !point.strength.is_finite() || !(0.0..=10.0).contains(&point.strength))
    {
        return Err("カラー化設定が範囲外です");
    }
    params.brightness = values.brightness;
    params.contrast = values.contrast;
    params.gamma = values.gamma;
    params.saturation = values.saturation;
    params.temperature = values.temperature;
    params.black_point = values.black_point;
    params.white_point = values.white_point;
    params.midtone = values.midtone;
    params.auto_mode = values.auto_mode.map(|mode| match mode {
        mimageviewer_ipc::RemoteAutoMode::Auto => crate::adjustment::AutoMode::Auto,
        mimageviewer_ipc::RemoteAutoMode::MangaCleanup => crate::adjustment::AutoMode::MangaCleanup,
    });
    params.colorize = crate::colorize::ColorizeParams {
        mode: match colorize.mode {
            mimageviewer_ipc::RemoteColorizeMode::Disabled => {
                crate::colorize::ColorizeMode::Disabled
            }
            mimageviewer_ipc::RemoteColorizeMode::MonochromeOnly => {
                crate::colorize::ColorizeMode::MonochromeOnly
            }
            mimageviewer_ipc::RemoteColorizeMode::AllImages => {
                crate::colorize::ColorizeMode::AllImages
            }
        },
        mono_tolerance: colorize.mono_tolerance,
        palette: match colorize.palette {
            mimageviewer_ipc::RemoteColorizePalette::Legacy4Color => {
                crate::colorize::ColorizePalette::Legacy4Color
            }
            mimageviewer_ipc::RemoteColorizePalette::LegacySkin => {
                crate::colorize::ColorizePalette::LegacySkin
            }
            mimageviewer_ipc::RemoteColorizePalette::Custom => {
                crate::colorize::ColorizePalette::Custom
            }
        },
        control_points: colorize
            .control_points
            .iter()
            .map(|point| crate::colorize::ColorizeControlPoint {
                color: point.color,
                strength: point.strength,
            })
            .collect(),
        luminance_weight: colorize.luminance_weight,
        density_normalization_strength: colorize.density_normalization_strength,
        tone_method: match colorize.tone_method {
            mimageviewer_ipc::RemoteToneDensityMethod::Off => {
                crate::colorize::ToneDensityMethod::Off
            }
            mimageviewer_ipc::RemoteToneDensityMethod::Fast => {
                crate::colorize::ToneDensityMethod::Fast
            }
            mimageviewer_ipc::RemoteToneDensityMethod::LocalMean => {
                crate::colorize::ToneDensityMethod::LocalMean
            }
            mimageviewer_ipc::RemoteToneDensityMethod::Gaussian => {
                crate::colorize::ToneDensityMethod::Gaussian
            }
        },
        tone_radius: colorize.tone_radius,
        tone_strength: colorize.tone_strength,
    };
    if let Some(value) = values.post_filter.as_deref() {
        params.post_filter = parse_post_filter_wire(value)?;
    }
    if let Some(ai) = values.ai.as_ref() {
        if let Some(key) = ai.upscale_model.as_deref()
            && key != "auto"
            && !crate::ai::ModelKind::from_str(key)
                .is_some_and(|model| crate::ai::ModelKind::upscale_models().contains(&model))
        {
            return Err("AI アップスケールの選択値が不正です");
        }
        if let Some(key) = ai.denoise_model.as_deref()
            && !crate::ai::ModelKind::from_str(key)
                .is_some_and(|model| crate::ai::ModelKind::denoise_models().contains(&model))
        {
            return Err("AI デノイズの選択値が不正です");
        }
        params.upscale_model = ai.upscale_model.clone();
        params.denoise_model = ai.denoise_model.clone();
    }
    Ok(params)
}

#[cfg(test)]
mod adjustment_value_tests {
    use super::*;

    #[test]
    fn remote_colorize_default_matches_core_default() {
        assert_eq!(
            remote_colorize_params(&crate::colorize::ColorizeParams::default()),
            mimageviewer_ipc::RemoteColorizeParams::default()
        );
    }

    #[test]
    fn remote_overlay_applies_colorize_and_preserves_hidden_adjustment_fields() {
        let base = crate::adjustment::AdjustParams {
            upscale_model: Some("auto".to_owned()),
            denoise_model: Some("denoise_realplksr".to_owned()),
            post_filter: crate::adjustment::PostFilter::CrtFull,
            smart_sharpen: 42,
            ..Default::default()
        };
        let mut values = remote_adjustment_values(&base);
        values.brightness = 10.0;
        values.contrast = -5.0;
        values.gamma = 1.2;
        values.saturation = 7.0;
        values.temperature = 3.0;
        values.black_point = 4;
        values.white_point = 247;
        values.midtone = 0.8;
        values.auto_mode = Some(mimageviewer_ipc::RemoteAutoMode::MangaCleanup);
        values.post_filter = Some("sepia".to_owned());
        values.colorize.mode = mimageviewer_ipc::RemoteColorizeMode::AllImages;
        values.colorize.palette = mimageviewer_ipc::RemoteColorizePalette::Custom;
        values.colorize.control_points = vec![
            mimageviewer_ipc::RemoteColorizeControlPoint {
                color: [1, 2, 3],
                strength: 2.5,
            },
            mimageviewer_ipc::RemoteColorizeControlPoint {
                color: [240, 230, 220],
                strength: 0.75,
            },
        ];

        let applied = apply_remote_adjustment_values(base.clone(), &values).unwrap();

        assert_eq!(
            applied.upscale_model,
            values.ai.as_ref().unwrap().upscale_model
        );
        assert_eq!(
            applied.denoise_model,
            values.ai.as_ref().unwrap().denoise_model
        );
        assert_eq!(remote_colorize_params(&applied.colorize), values.colorize);
        assert_eq!(applied.post_filter, crate::adjustment::PostFilter::Sepia);
        assert_eq!(applied.smart_sharpen, base.smart_sharpen);
        assert_eq!(remote_adjustment_values(&applied), values);
    }

    #[test]
    fn remote_immediate_overlay_rejects_non_finite_values() {
        let mut values = remote_adjustment_values(&crate::adjustment::AdjustParams::default());
        values.gamma = f32::NAN;
        assert!(apply_remote_adjustment_values(Default::default(), &values).is_err());
    }

    #[test]
    fn missing_remote_ai_values_preserve_saved_models() {
        let base = crate::adjustment::AdjustParams {
            upscale_model: Some("auto".to_owned()),
            denoise_model: Some("denoise_realplksr".to_owned()),
            ..Default::default()
        };
        let mut values = remote_adjustment_values(&base);
        values.ai = None;
        let applied = apply_remote_adjustment_values(base.clone(), &values).unwrap();
        assert_eq!(applied.upscale_model, base.upscale_model);
        assert_eq!(applied.denoise_model, base.denoise_model);
    }

    #[test]
    fn missing_remote_post_filter_value_preserves_saved_filter() {
        let base = crate::adjustment::AdjustParams {
            post_filter: crate::adjustment::PostFilter::CrtFull,
            ..Default::default()
        };
        let mut payload = serde_json::to_value(remote_adjustment_values(&base)).unwrap();
        payload.as_object_mut().unwrap().remove("post_filter");
        let values: mimageviewer_ipc::RemoteAdjustmentValues =
            serde_json::from_value(payload).unwrap();
        assert_eq!(values.post_filter, None);

        let applied = apply_remote_adjustment_values(base.clone(), &values).unwrap();
        assert_eq!(applied.post_filter, base.post_filter);
    }

    #[test]
    fn remote_post_filter_value_rejects_unknown_filter() {
        let mut values = remote_adjustment_values(&crate::adjustment::AdjustParams::default());
        values.post_filter = Some("future_filter".to_owned());
        assert!(apply_remote_adjustment_values(Default::default(), &values).is_err());
    }

    #[test]
    fn remote_post_filter_state_uses_core_groups_labels_and_pixel_metadata() {
        let selected = crate::adjustment::PostFilter::CrtFull;
        let state = remote_post_filter_state(selected);
        assert_eq!(state.selected, post_filter_wire_value(selected));
        assert_eq!(
            state
                .groups
                .iter()
                .map(|group| group.label.as_str())
                .collect::<Vec<_>>(),
            crate::adjustment::POST_FILTER_GROUPS
                .iter()
                .map(|group| group.label)
                .collect::<Vec<_>>()
        );

        let options = state
            .groups
            .iter()
            .flat_map(|group| group.options.iter())
            .collect::<Vec<_>>();
        for option in &options {
            let filter = parse_post_filter_wire(&option.value).unwrap();
            assert_eq!(option.label, filter.display_label());
            assert_eq!(option.rewrites_pixels, filter.rewrites_pixels());
        }
        assert_eq!(
            options
                .iter()
                .filter(|option| !option.rewrites_pixels)
                .map(|option| option.value.as_str())
                .collect::<Vec<_>>(),
            ["none", "nearest", "upscale_sharp", "upscale_anime"]
        );
    }

    #[test]
    fn remote_ai_values_reject_unknown_or_wrong_kind_models() {
        let mut values = remote_adjustment_values(&crate::adjustment::AdjustParams::default());
        values.ai.as_mut().unwrap().upscale_model = Some("denoise_realplksr".to_owned());
        assert!(apply_remote_adjustment_values(Default::default(), &values).is_err());

        values.ai.as_mut().unwrap().upscale_model = Some("realcugan_4x".to_owned());
        values.ai.as_mut().unwrap().denoise_model = Some("realcugan_4x".to_owned());
        assert!(apply_remote_adjustment_values(Default::default(), &values).is_err());
    }

    #[test]
    fn remote_colorize_overlay_rejects_invalid_custom_control_points() {
        let mut values = remote_adjustment_values(&crate::adjustment::AdjustParams::default());
        values.colorize.control_points.truncate(1);
        assert!(apply_remote_adjustment_values(Default::default(), &values).is_err());

        let mut values = remote_adjustment_values(&crate::adjustment::AdjustParams::default());
        values.colorize.control_points[0].strength = f32::INFINITY;
        assert!(apply_remote_adjustment_values(Default::default(), &values).is_err());
    }

    #[test]
    fn remote_view_trim_normalization_clamps_every_book_margin_and_drops_page_mode() {
        let value = serde_json::json!({
            "apply_mode": "page",
            "book_settings": {
                "enabled": true,
                "spread_separate": true,
                "single": { "left": -1.0, "top": 0.8, "right": 0.9, "bottom": -0.2 },
                "spread_linked": { "top": 0.7, "bottom": -0.1, "inner": 0.6, "outer": 0.5 },
                "spread_left": { "left": 0.4, "top": 0.3, "right": -0.4, "bottom": 0.8 },
                "spread_right": { "left": -0.2, "top": 0.5, "right": 0.7, "bottom": 0.6 }
            }
        });
        let normalized = normalize_remote_view_trim_state(&value).unwrap();
        assert_eq!(
            normalized.apply_mode,
            crate::view_trim::ViewTrimApplyMode::None
        );
        for margin in [
            normalized.book_settings.single.left,
            normalized.book_settings.single.top,
            normalized.book_settings.single.right,
            normalized.book_settings.single.bottom,
            normalized.book_settings.spread_linked.top,
            normalized.book_settings.spread_linked.bottom,
            normalized.book_settings.spread_linked.inner,
            normalized.book_settings.spread_linked.outer,
            normalized.book_settings.spread_left.left,
            normalized.book_settings.spread_left.top,
            normalized.book_settings.spread_left.right,
            normalized.book_settings.spread_left.bottom,
            normalized.book_settings.spread_right.left,
            normalized.book_settings.spread_right.top,
            normalized.book_settings.spread_right.right,
            normalized.book_settings.spread_right.bottom,
        ] {
            assert!((0.0..=crate::view_trim::MAX_VIEW_TRIM_MARGIN).contains(&margin));
        }
    }

    #[test]
    fn remote_sort_state_uses_core_values_and_labels() {
        let state = remote_grid_sort_state(crate::settings::SortOrder::DateDesc, None);
        assert_eq!(state.options.len(), crate::settings::SortOrder::all().len());
        for (option, order) in state.options.iter().zip(crate::settings::SortOrder::all()) {
            assert_eq!(option.value, sort_order_wire_value(*order));
            assert_eq!(option.label, order.label());
            assert_eq!(option.short_label, order.short_label());
            assert_eq!(parse_sort_order_wire(&option.value), Ok(*order));
        }
        assert_eq!(
            state.selected,
            sort_order_wire_value(crate::settings::SortOrder::DateDesc)
        );
    }
}

pub(crate) struct RemoteIpcServer {
    #[cfg(windows)]
    _guard: pipe::ServerGuard,
}

impl RemoteIpcServer {
    pub(crate) fn start(settings: crate::settings::Settings) -> Result<Self, String> {
        #[cfg(windows)]
        {
            return pipe::ServerGuard::start(settings).map(|guard| Self { _guard: guard });
        }
        #[cfg(not(windows))]
        {
            let _ = settings;
            Err("リモート接続は Windows の名前付きパイプ専用です".to_owned())
        }
    }

    pub(crate) fn session_handle(&self) -> session::SessionHandle {
        #[cfg(windows)]
        {
            self._guard.session_handle()
        }
        #[cfg(not(windows))]
        {
            unreachable!("remote IPC server is Windows-only")
        }
    }
}
