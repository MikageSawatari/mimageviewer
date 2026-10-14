use std::collections::HashSet;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use musicvideo_core::{PROJECT_VERSION, Project};

pub fn ensure_project_extension(mut path: PathBuf) -> PathBuf {
    let has_expected_suffix = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.to_ascii_lowercase().ends_with(".mivmv.json"));
    if has_expected_suffix {
        return path;
    }
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        path.set_extension("mivmv.json");
    } else if let Some(name) = path.file_name() {
        let mut name = name.to_os_string();
        name.push(".mivmv.json");
        path.set_file_name(name);
    }
    path
}

pub fn save_project(path: &Path, project: &Project) -> Result<(), String> {
    let stored = project_for_storage(path, project)?;
    let json = serde_json::to_string_pretty(&stored)
        .map_err(|error| format!("プロジェクトを JSON に変換できません: {error}"))?;
    let path = absolute_lexical(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| "プロジェクトの保存先フォルダーを特定できません".to_owned())?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|error| {
        format!(
            "プロジェクトの一時ファイルを作成できません ({}): {error}",
            parent.display()
        )
    })?;
    temporary
        .write_all(format!("{json}\n").as_bytes())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| format!("プロジェクトを書き込めません ({}): {error}", path.display()))?;
    temporary.persist(&path).map_err(|error| {
        format!(
            "プロジェクトを保存先へ置けません ({}): {}",
            path.display(),
            error.error
        )
    })?;
    Ok(())
}

pub fn load_project(path: &Path) -> Result<Project, String> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("プロジェクトを開けません ({}): {error}", path.display()))?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("プロジェクト JSON を読めません: {error}"))?;
    let version = value
        .get("version")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| "プロジェクトに version がありません".to_owned())?;
    if version != u64::from(PROJECT_VERSION) {
        return Err(format!(
            "未対応のプロジェクト version {version} です (対応: {PROJECT_VERSION})"
        ));
    }
    let mut project: Project = serde_json::from_value(value)
        .map_err(|error| format!("プロジェクトの内容を読めません: {error}"))?;
    let mut image_ids = HashSet::new();
    if let Some(duplicate) = project
        .images
        .iter()
        .find(|image| !image_ids.insert(image.id.clone()))
    {
        return Err(format!("画像素材 ID「{}」が重複しています", duplicate.id.0));
    }
    resolve_project_paths(path, &mut project)?;
    Ok(project)
}

#[cfg(test)]
pub fn project_json(path: &Path, project: &Project) -> Result<String, String> {
    let stored = project_for_storage(path, project)?;
    serde_json::to_string_pretty(&stored)
        .map_err(|error| format!("プロジェクトを JSON に変換できません: {error}"))
}

fn project_for_storage(path: &Path, project: &Project) -> Result<Project, String> {
    let base = project_base(path)?;
    let mut stored = project.clone();
    if let Some(audio) = &mut stored.audio {
        audio.path = portable_path(&base, &audio.path)?;
    }
    for image in &mut stored.images {
        image.path = portable_path(&base, &image.path)?;
    }
    Ok(stored)
}

fn resolve_project_paths(path: &Path, project: &mut Project) -> Result<(), String> {
    let base = project_base(path)?;
    if let Some(audio) = &mut project.audio {
        audio.path = resolve_path(&base, &audio.path);
    }
    for image in &mut project.images {
        image.path = resolve_path(&base, &image.path);
    }
    Ok(())
}

fn project_base(project_path: &Path) -> Result<PathBuf, String> {
    let absolute = absolute_lexical(project_path)?;
    absolute
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "プロジェクトファイルの親フォルダーを特定できません".to_owned())
}

fn portable_path(base: &Path, asset: &Path) -> Result<PathBuf, String> {
    let absolute = absolute_lexical(asset)?;
    Ok(relative_path(base, &absolute).unwrap_or(absolute))
}

fn resolve_path(base: &Path, stored: &Path) -> PathBuf {
    if stored.is_absolute() {
        normalize_lexical(stored)
    } else {
        normalize_lexical(&base.join(stored))
    }
}

pub(crate) fn absolute_lexical(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(normalize_lexical(path))
    } else {
        std::env::current_dir()
            .map(|current| normalize_lexical(&current.join(path)))
            .map_err(|error| format!("現在のフォルダーを取得できません: {error}"))
    }
}

fn relative_path(base: &Path, target: &Path) -> Option<PathBuf> {
    let base_components = base.components().collect::<Vec<_>>();
    let target_components = target.components().collect::<Vec<_>>();
    if roots(&base_components) != roots(&target_components) {
        return None;
    }
    let common = base_components
        .iter()
        .zip(&target_components)
        .take_while(|(left, right)| left == right)
        .count();
    let mut relative = PathBuf::new();
    for component in &base_components[common..] {
        if matches!(component, Component::Normal(_)) {
            relative.push("..");
        }
    }
    for component in &target_components[common..] {
        relative.push(component.as_os_str());
    }
    Some(relative)
}

fn roots<'a>(components: &'a [Component<'a>]) -> &'a [Component<'a>] {
    let root_len = components
        .iter()
        .take_while(|component| matches!(component, Component::Prefix(_) | Component::RootDir))
        .count();
    &components[..root_len]
}

fn normalize_lexical(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicvideo_core::{
        Animatable, AudioRef, BandCalibration, BandRange, BandWeighting, ImageId, ImageRef, Interp,
        Keyframe, LayerKind, LyricLine, ModulationBinding, ModulationCurve, ModulationTarget,
        ModulatorSource, Smoothing, Track, default_project, new_band_energy_modulator,
        new_image_layer, new_lyrics_layer,
    };

    #[test]
    fn project_round_trip_preserves_project_and_stores_relative_asset_paths() {
        let temp = tempfile::tempdir().unwrap();
        let project_path = temp.path().join("work").join("example.mivmv.json");
        std::fs::create_dir_all(project_path.parent().unwrap()).unwrap();
        let media = temp.path().join("media");
        let mut project = default_project(12.5);
        project.audio = Some(AudioRef {
            path: media.join("song.wav"),
        });
        project.images.push(ImageRef {
            id: ImageId::new("background"),
            path: media.join("cover.png"),
        });

        save_project(&project_path, &project).unwrap();
        let json = std::fs::read_to_string(&project_path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        let expected_audio = PathBuf::from("..").join("media").join("song.wav");
        let expected_image = PathBuf::from("..").join("media").join("cover.png");
        assert_eq!(value["audio"]["path"].as_str(), expected_audio.to_str());
        assert_eq!(value["images"][0]["path"].as_str(), expected_image.to_str());
        assert_eq!(load_project(&project_path).unwrap(), project);

        project.duration_secs = 20.0;
        save_project(&project_path, &project).unwrap();
        assert_eq!(load_project(&project_path).unwrap(), project);
    }

    #[test]
    fn keyframe_round_trip_preserves_order_interpolation_and_duplicate_times() {
        let temp = tempfile::tempdir().unwrap();
        let project_path = temp.path().join("keyframes.mivmv.json");
        let mut project = default_project(12.0);
        let second_image = ImageId::new("image-1");
        project.layers.push(new_image_layer(&project, second_image));
        let LayerKind::Image { zoom, .. } = &mut project.layers[0].kind else {
            unreachable!()
        };
        *zoom = Animatable::Track(
            Track::new(vec![
                Keyframe {
                    t: 0.0,
                    value: 1.0,
                    interp: Interp::EaseOut,
                },
                Keyframe {
                    t: 6.0,
                    value: 1.2,
                    interp: Interp::EaseInOut,
                },
            ])
            .unwrap(),
        );
        project.layers[0].opacity = Animatable::Track(
            Track::new(vec![
                Keyframe {
                    t: 0.0,
                    value: 1.0,
                    interp: Interp::Linear,
                },
                Keyframe {
                    t: 6.0,
                    value: 1.0,
                    interp: Interp::EaseOut,
                },
                Keyframe {
                    t: 6.0,
                    value: 0.75,
                    interp: Interp::Step,
                },
                Keyframe {
                    t: 8.0,
                    value: 0.0,
                    interp: Interp::EaseInOut,
                },
            ])
            .unwrap(),
        );
        project.layers[4].opacity = Animatable::Track(
            Track::new(vec![
                Keyframe {
                    t: 0.0,
                    value: 0.0,
                    interp: Interp::Linear,
                },
                Keyframe {
                    t: 6.0,
                    value: 0.0,
                    interp: Interp::EaseIn,
                },
                Keyframe {
                    t: 8.0,
                    value: 1.0,
                    interp: Interp::Linear,
                },
            ])
            .unwrap(),
        );

        save_project(&project_path, &project).unwrap();
        let restored = load_project(&project_path).unwrap();
        assert_eq!(restored, project);
        let Animatable::Track(track) = &restored.layers[0].opacity else {
            panic!("opacity track was not restored");
        };
        assert_eq!(
            track
                .keys()
                .iter()
                .map(|key| (key.t, key.interp, key.value))
                .collect::<Vec<_>>(),
            [
                (0.0, Interp::Linear, 1.0),
                (6.0, Interp::EaseOut, 1.0),
                (6.0, Interp::Step, 0.75),
                (8.0, Interp::EaseInOut, 0.0),
            ]
        );
    }

    #[test]
    fn modulator_and_binding_round_trip_without_reordering() {
        let temp = tempfile::tempdir().unwrap();
        let project_path = temp.path().join("modulators.mivmv.json");
        let mut project = default_project(12.0);
        let mut bass = new_band_energy_modulator(&project);
        bass.name = "C1–B1".to_owned();
        bass.source = ModulatorSource::BandOnset {
            range: BandRange::Semitone {
                low_midi: 24,
                high_midi: 35,
            },
            smoothing: Smoothing::Envelope {
                attack_secs: 0.02,
                release_secs: 0.24,
            },
            weighting: BandWeighting::MivDisplay,
            gain_db: 4.5,
            floor: 0.15,
            ceiling: 0.8,
            calibration: BandCalibration::Auto {
                low_percentile: 40.0,
                high_percentile: 98.0,
            },
            threshold: 0.12,
        };
        let id = bass.id;
        project.modulators.push(bass);
        project.layers[1].modulations = vec![
            ModulationBinding {
                target: ModulationTarget::SpectrumRadius,
                modulator: id,
                amount: 80.0,
                curve: ModulationCurve::Squared,
            },
            ModulationBinding {
                target: ModulationTarget::GlowAlpha,
                modulator: id,
                amount: -0.4,
                curve: ModulationCurve::Sqrt,
            },
        ];
        project.layers[0].modulations = vec![ModulationBinding {
            target: ModulationTarget::ImageZoom,
            modulator: id,
            amount: 0.12,
            curve: ModulationCurve::Squared,
        }];

        save_project(&project_path, &project).unwrap();
        let restored = load_project(&project_path).unwrap();
        assert_eq!(restored, project);
        assert_eq!(restored.modulators[0].id, id);
        assert_eq!(
            restored.layers[1].modulations,
            project.layers[1].modulations
        );
        assert_eq!(
            restored.layers[0].modulations,
            project.layers[0].modulations
        );
    }

    #[test]
    fn lyrics_round_trip_preserves_unset_empty_and_duplicate_times() {
        let temp = tempfile::tempdir().unwrap();
        let project_path = temp.path().join("lyrics.mivmv.json");
        let mut project = default_project(12.0);
        let mut layer = new_lyrics_layer(&project);
        let LayerKind::Lyrics(lyrics) = &mut layer.kind else {
            unreachable!()
        };
        lyrics.lines = vec![
            LyricLine {
                text: "未設定".into(),
                time_secs: None,
            },
            LyricLine {
                text: String::new(),
                time_secs: Some(2.0),
            },
            LyricLine {
                text: "同時の前".into(),
                time_secs: Some(4.0),
            },
            LyricLine {
                text: "同時の後".into(),
                time_secs: Some(4.0),
            },
        ];
        project.layers.push(layer);

        save_project(&project_path, &project).unwrap();
        let restored = load_project(&project_path).unwrap();
        assert_eq!(restored, project);
        let LayerKind::Lyrics(lyrics) = &restored.layers.last().unwrap().kind else {
            unreachable!()
        };
        assert_eq!(lyrics.lines[0].time_secs, None);
        assert_eq!(lyrics.lines[1].text, "");
        assert_eq!(lyrics.lines[2].time_secs, lyrics.lines[3].time_secs);
        assert_eq!(lyrics.lines[2].text, "同時の前");
        assert_eq!(lyrics.lines[3].text, "同時の後");
    }

    #[test]
    fn legacy_average_modulator_json_defaults_the_new_fields() {
        let temp = tempfile::tempdir().unwrap();
        let project_path = temp.path().join("legacy-modulator.mivmv.json");
        let mut project = default_project(12.0);
        let mut modulator = new_band_energy_modulator(&project);
        modulator.source = ModulatorSource::BandEnergy {
            range: BandRange::Hz {
                low_hz: 40.0,
                high_hz: 120.0,
            },
            smoothing: Smoothing::Average { factor: 0.65 },
            weighting: BandWeighting::Raw,
            gain_db: 3.0,
            floor: 0.03,
            ceiling: 0.8,
            calibration: BandCalibration::Manual,
            threshold: 0.5,
        };
        project.modulators.push(modulator);
        let mut value = serde_json::to_value(&project).unwrap();
        value["modulators"][0]["source"]["BandEnergy"]
            .as_object_mut()
            .unwrap()
            .remove("threshold");
        value["modulators"][0]["source"]["BandEnergy"]
            .as_object_mut()
            .unwrap()
            .remove("calibration");
        std::fs::write(&project_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();

        let restored = load_project(&project_path).unwrap();
        let ModulatorSource::BandEnergy {
            smoothing,
            calibration,
            threshold,
            ..
        } = restored.modulators[0].source
        else {
            panic!("legacy source changed variant");
        };
        assert_eq!(smoothing, Smoothing::Average { factor: 0.65 });
        assert_eq!(calibration, BandCalibration::Manual);
        assert_eq!(threshold, 0.0);
    }

    #[test]
    fn unknown_version_is_rejected_explicitly() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("future.mivmv.json");
        let future = serde_json::json!({ "version": 99 });
        std::fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
        let error = load_project(&path).unwrap_err();
        assert!(error.contains("未対応のプロジェクト version 99"));
    }

    #[test]
    fn project_extension_is_applied_once() {
        assert_eq!(
            ensure_project_extension(PathBuf::from("sample")),
            PathBuf::from("sample.mivmv.json")
        );
        assert_eq!(
            ensure_project_extension(PathBuf::from("sample.json")),
            PathBuf::from("sample.mivmv.json")
        );
        assert_eq!(
            ensure_project_extension(PathBuf::from("sample.MIVMV.JSON")),
            PathBuf::from("sample.MIVMV.JSON")
        );
    }

    #[test]
    fn duplicate_image_ids_are_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("duplicate.mivmv.json");
        let mut project = default_project(1.0);
        project.images = vec![
            ImageRef {
                id: ImageId::new("same"),
                path: PathBuf::from("first.png"),
            },
            ImageRef {
                id: ImageId::new("same"),
                path: PathBuf::from("second.png"),
            },
        ];
        std::fs::write(&path, serde_json::to_vec(&project).unwrap()).unwrap();
        assert!(load_project(&path).unwrap_err().contains("same"));
    }

    #[test]
    fn project_json_does_not_write_files() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("example.mivmv.json");
        let mut project = default_project(12.5);
        project.output.width = 640;
        project.output.height = 360;
        project.output.fps = 30.0;
        project.layers.truncate(1);
        project.audio = Some(AudioRef {
            path: temp.path().join("media").join("song.wav"),
        });
        project.images.push(ImageRef {
            id: ImageId::new("background"),
            path: temp.path().join("media").join("cover.png"),
        });
        let json = project_json(&path, &project).unwrap();
        println!("{json}");
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["version"], PROJECT_VERSION);
        assert!(!path.exists());
    }
}
