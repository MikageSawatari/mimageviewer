use super::*;
use crate::settings_transfer::{ImportReport, TransferIssue};

pub(crate) enum PreferencesTransferFeedback {
    Imported(ImportReport),
    Exported {
        item_count: usize,
        issues: Vec<TransferIssue>,
    },
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PreferencesTransferAction {
    Export,
    Import,
}

/// A single request owns explanation, file work, and its completion notification.
/// It never holds a PreferencesState or mutates the live settings/database.
pub(crate) enum PreferencesTransferState {
    Explanation {
        action: PreferencesTransferAction,
        feedback: Option<PreferencesTransferFeedback>,
    },
    Importing(mpsc::Receiver<Result<crate::settings_transfer::ParsedPreferences, String>>),
    Exporting(mpsc::Receiver<Result<(usize, Vec<TransferIssue>), String>>),
    Exported {
        item_count: usize,
        issues: Vec<TransferIssue>,
    },
}

impl PreferencesTransferState {
    fn is_busy(&self) -> bool {
        matches!(self, Self::Importing(_) | Self::Exporting(_))
    }
    fn failed(action: PreferencesTransferAction, message: String) -> Self {
        crate::logger::log(format!("[preferences-transfer] {message}"));
        Self::Explanation {
            action,
            feedback: Some(PreferencesTransferFeedback::Failed(message)),
        }
    }
}

impl App {
    pub(crate) fn preferences_transfer_dialog_open(&self) -> bool {
        self.show_settings_restore && self.settings_restore_state.preferences_transfer.is_some()
    }

    pub(crate) fn preferences_transfer_busy(&self) -> bool {
        self.settings_restore_state
            .preferences_transfer
            .as_ref()
            .is_some_and(|state| state.is_busy())
    }

    pub(crate) fn start_preferences_transfer(&mut self, action: PreferencesTransferAction) {
        let state = &mut self.settings_restore_state;
        if state.preferences_transfer.is_none() {
            state.preferences_transfer = Some(Box::new(PreferencesTransferState::Explanation {
                action,
                feedback: None,
            }));
        }
    }

    fn preferences_transfer_path_selected(&mut self, path: Option<PathBuf>, ctx: &egui::Context) {
        let Some(path) = path else {
            return;
        };
        let state = &mut self.settings_restore_state;
        let Some(PreferencesTransferState::Explanation { action, .. }) =
            state.preferences_transfer.as_deref()
        else {
            return;
        };
        let action = *action;
        let repaint = ctx.clone();
        match action {
            PreferencesTransferAction::Import => {
                let (tx, rx) = mpsc::channel();
                match std::thread::Builder::new()
                    .name("preferences-import".into())
                    .spawn(move || {
                        let _ = tx.send(crate::settings_transfer::read_preferences(&path));
                        repaint.request_repaint();
                    }) {
                    Ok(_) => {
                        state.preferences_transfer =
                            Some(Box::new(PreferencesTransferState::Importing(rx)))
                    }
                    Err(error) => {
                        state.preferences_transfer =
                            Some(Box::new(PreferencesTransferState::failed(
                                action,
                                format!("読み込み処理を開始できませんでした: {error}"),
                            )))
                    }
                }
            }
            PreferencesTransferAction::Export => {
                // Capture committed settings only, without draft, paths or user data.
                let snapshot =
                    crate::settings_transfer::capture_export(&self.settings.preferences_snapshot());
                let (tx, rx) = mpsc::channel();
                match std::thread::Builder::new()
                    .name("preferences-export".into())
                    .spawn(move || {
                        let result = snapshot.export().and_then(|exported| {
                            crate::settings_transfer::write_preferences(&path, &exported.json)?;
                            Ok((exported.item_count, exported.issues))
                        });
                        let _ = tx.send(result);
                        repaint.request_repaint();
                    }) {
                    Ok(_) => {
                        state.preferences_transfer =
                            Some(Box::new(PreferencesTransferState::Exporting(rx)))
                    }
                    Err(error) => {
                        state.preferences_transfer =
                            Some(Box::new(PreferencesTransferState::failed(
                                action,
                                format!("書き出し処理を開始できませんでした: {error}"),
                            )))
                    }
                }
            }
        }
    }

    fn poll_preferences_transfer(&mut self) {
        let state = &mut self.settings_restore_state;
        let mut imported = None;
        let next = match state.preferences_transfer.as_deref() {
            Some(PreferencesTransferState::Importing(rx)) => match rx.try_recv() {
                Ok(Ok(parsed)) => {
                    imported = Some(parsed);
                    None
                }
                Ok(Err(error)) => Some(PreferencesTransferState::failed(
                    PreferencesTransferAction::Import,
                    format!("ファイルを取り込めませんでした: {error}"),
                )),
                Err(mpsc::TryRecvError::Disconnected) => Some(PreferencesTransferState::failed(
                    PreferencesTransferAction::Import,
                    "読み込み処理が応答せず終了しました。".into(),
                )),
                Err(mpsc::TryRecvError::Empty) => None,
            },
            Some(PreferencesTransferState::Exporting(rx)) => match rx.try_recv() {
                Ok(Ok((item_count, issues))) => {
                    Some(PreferencesTransferState::Exported { item_count, issues })
                }
                Ok(Err(error)) => Some(PreferencesTransferState::failed(
                    PreferencesTransferAction::Export,
                    format!("ファイルへ書き出せませんでした: {error}"),
                )),
                Err(mpsc::TryRecvError::Disconnected) => Some(PreferencesTransferState::failed(
                    PreferencesTransferAction::Export,
                    "書き出し処理が応答せず終了しました。".into(),
                )),
                Err(mpsc::TryRecvError::Empty) => None,
            },
            _ => None,
        };
        if let Some(next) = next {
            state.preferences_transfer = Some(Box::new(next));
        }
        if let Some(parsed) = imported {
            // Accept the worker result at this one boundary. An independently opened
            // preferences window owns its draft; importing must never replace it.
            if self.show_preferences {
                state.preferences_transfer = Some(Box::new(PreferencesTransferState::failed(
                    PreferencesTransferAction::Import,
                    "環境設定が開かれたため、取り込みを中止しました。環境設定を閉じてから行ってください。".into(),
                )));
                return;
            }
            state.preferences_transfer = None;
            self.show_settings_restore = false;
            self.pref_state = None;
            self.ensure_preferences_state();
            let draft = self
                .pref_state
                .as_mut()
                .expect("initialized preferences draft");
            let report = parsed.apply_to(&mut draft.settings);
            draft.transfer_feedback = Some(PreferencesTransferFeedback::Imported(report));
            self.open_preferences_page(PreferencesPage::General);
        }
    }

    pub(crate) fn show_preferences_transfer_dialog(&mut self, ctx: &egui::Context) {
        self.poll_preferences_transfer();
        let Some(transfer) = self.settings_restore_state.preferences_transfer.as_deref() else {
            return;
        };
        let busy = transfer.is_busy();
        egui::Popup::close_all(ctx);
        let enter = !busy && self.dialog_enter_pressed(ctx);
        let escape = !busy && self.dialog_escape_pressed(ctx);
        let mut choose_file = None;
        let mut close = false;
        egui::Modal::new(egui::Id::new("preferences_transfer_dialog")).show(ctx, |ui| {
            ui.set_width(420.0);
            match transfer {
                PreferencesTransferState::Explanation { action, feedback } => {
                    render_transfer_explanation(ui, *action, false, feedback.as_ref());
                    ui.horizontal(|ui| {
                        let label = match action {
                            PreferencesTransferAction::Export => "書き出す",
                            PreferencesTransferAction::Import => "ファイルを選ぶ",
                        };
                        if ui.button(label).clicked() || enter {
                            choose_file = Some(*action);
                        }
                        if ui.button("キャンセル").clicked() || escape {
                            close = true;
                        }
                    });
                }
                PreferencesTransferState::Importing(_) => {
                    render_transfer_explanation(ui, PreferencesTransferAction::Import, true, None)
                }
                PreferencesTransferState::Exporting(_) => {
                    render_transfer_explanation(ui, PreferencesTransferAction::Export, true, None)
                }
                PreferencesTransferState::Exported { item_count, issues } => {
                    ui.heading("環境設定の書き出し");
                    render_transfer_feedback(
                        ui,
                        Some(&PreferencesTransferFeedback::Exported {
                            item_count: *item_count,
                            issues: issues.clone(),
                        }),
                    );
                    if ui.button("閉じる").clicked() || enter || escape {
                        close = true;
                    }
                }
            }
        });
        if close {
            self.settings_restore_state.preferences_transfer = None;
        } else if let Some(action) = choose_file {
            let dialog = rfd::FileDialog::new().add_filter("mIV preferences", &["json"]);
            let path = match action {
                PreferencesTransferAction::Import => dialog.pick_file(),
                PreferencesTransferAction::Export => dialog
                    .set_file_name("preferences.mivprefs.json")
                    .save_file()
                    .map(|mut path| {
                        if path.extension().is_none() {
                            path.set_extension("json");
                        }
                        path
                    }),
            };
            self.preferences_transfer_path_selected(path, ctx);
        }
    }

    #[cfg(test)]
    pub(crate) fn hold_preferences_transfer_for_test(
        &mut self,
    ) -> mpsc::Sender<Result<crate::settings_transfer::ParsedPreferences, String>> {
        let (tx, rx) = mpsc::channel();
        self.settings_restore_state.preferences_transfer =
            Some(Box::new(PreferencesTransferState::Importing(rx)));
        tx
    }
}

pub(super) fn render_transfer_explanation(
    ui: &mut egui::Ui,
    action: PreferencesTransferAction,
    busy: bool,
    feedback: Option<&PreferencesTransferFeedback>,
) {
    ui.heading(match action {
        PreferencesTransferAction::Export => "環境設定の書き出し",
        PreferencesTransferAction::Import => "環境設定の取り込み",
    });
    match action {
        PreferencesTransferAction::Export => {
            ui.label("現在の確定済みの環境設定から、移行できる設定をファイルに保存します。");
            ui.small("環境設定画面で変更中の未確定の設定は含みません。");
        }
        PreferencesTransferAction::Import => {
            ui.label("設定ファイルを読み込み、環境設定画面で内容を確認します。");
            ui.small("OK で保存します。キャンセルで今回の変更を取り消します。");
        }
    }
    ui.label("「この時点に戻す」と異なり、持ち運べる環境設定だけが対象です。");
    ui.label("★・タグ・画像の編集・本棚・お気に入り・履歴・読書位置などの利用データは含みません。");
    ui.label("PC 固有の保存先やパス、操作カスタマイズ、通信設定も対象外です。");
    if action == PreferencesTransferAction::Import {
        ui.small("対象外の設定や利用データは、取り込み前の内容を保持します。");
    }
    ui.small("操作カスタマイズは、設定メニューの専用画面から書き出せます。");
    if busy {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(match action {
                PreferencesTransferAction::Export => "設定ファイルを書き出しています…",
                PreferencesTransferAction::Import => "設定ファイルを読み込んでいます…",
            });
        });
        ui.small("ファイル処理の完了後に操作できます。");
    }
    render_transfer_feedback(ui, feedback);
}

pub(super) fn render_transfer_feedback(
    ui: &mut egui::Ui,
    feedback: Option<&PreferencesTransferFeedback>,
) {
    match feedback {
        Some(PreferencesTransferFeedback::Failed(message)) => {
            ui.colored_label(ui.visuals().error_fg_color, message);
        }
        Some(PreferencesTransferFeedback::Imported(report)) => {
            if report.accepted_count == 0 {
                ui.label("取り込める項目がありませんでした。");
            } else {
                ui.label(format!(
                    "{} 項目を読み込み、{} 項目を変更しました。",
                    report.accepted_count,
                    report.changed_fields.len()
                ));
                ui.label("OK で保存します。キャンセルで今回の変更を取り消します。");
            }
            if !report.changed_fields.is_empty() {
                ui.collapsing("変更した項目", |ui| {
                    for label in &report.changed_fields {
                        ui.label(label);
                    }
                });
            }
            render_issues(ui, &report.issues);
            if report.unknown_count > 0 {
                ui.small(format!(
                    "対象外または未対応の項目を {} 件、読み飛ばしました。",
                    report.unknown_count
                ));
            }
        }
        Some(PreferencesTransferFeedback::Exported { item_count, issues }) => {
            ui.label(format!("{item_count} 項目をファイルに書き出しました。"));
            render_issues(ui, issues);
        }
        None => {}
    }
}

fn render_issues(ui: &mut egui::Ui, issues: &[TransferIssue]) {
    if !issues.is_empty() {
        ui.collapsing(
            format!("読み飛ばした項目 ({} 件)", issues.len()),
            |ui| {
                for issue in issues {
                    ui.label(format!("{}: {}", issue.field, issue.reason));
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};

    fn begin_import(app: &mut App, path: PathBuf, ctx: &egui::Context) {
        app.show_settings_restore = true;
        app.start_preferences_transfer(PreferencesTransferAction::Import);
        app.preferences_transfer_path_selected(Some(path), ctx);
    }

    #[test]
    fn preferences_transfer_worker_roundtrip_real_ok_db_reopen_and_cancel() {
        use crate::reading_history_db::{
            ReadingHistoryDb, ReadingHistoryEntry, ReadingHistoryKind,
        };
        use crate::settings::{FileOrganizeDestination, TextContrast, UiTheme};
        let mut app = crate::app::setup_app_for_test();
        let source_dir = tempfile::tempdir().unwrap();
        let source_db = crate::settings_db::SettingsDb::create_new(source_dir.path()).unwrap();
        let mut source = app.settings.clone();
        source.ui_theme = UiTheme::Dark;
        source.text_contrast = TextContrast::Strong;
        source.slideshow_interval_secs = 17.0;
        source.file_organize_destinations = vec![FileOrganizeDestination::from_path(
            source_dir.path().join("source-private"),
        )];
        source.favorites = vec![crate::settings::FavoriteEntry::new(
            "source private name".into(),
            source_dir.path().join("favorite"),
        )];
        source.remote_service_enabled = true;
        source_db.save_full(&source).unwrap();
        let source = source_db.load_into_settings().unwrap();
        let source_before = serde_json::to_value(&source).unwrap();
        let path = source_dir.path().join("preferences.mivprefs.json");
        let ctx = egui::Context::default();
        let original = app.settings.clone();
        app.settings = source.clone();
        app.show_settings_restore = true;
        app.start_preferences_transfer(PreferencesTransferAction::Export);
        app.preferences_transfer_path_selected(Some(path.clone()), &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.preferences_transfer_busy() {
            app.poll_preferences_transfer();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(matches!(
            app.settings_restore_state.preferences_transfer.as_deref(),
            Some(PreferencesTransferState::Exported { .. })
        ));
        app.settings_restore_state.preferences_transfer = None;
        app.settings = original;
        let json = std::fs::read_to_string(&path).unwrap();
        assert!(!json.contains("source-private"));
        assert!(!json.contains("source private name"));
        assert_eq!(
            serde_json::to_value(source_db.load_into_settings().unwrap()).unwrap(),
            source_before
        );

        app.settings.ui_theme = UiTheme::Light;
        app.settings.text_contrast = TextContrast::Standard;
        app.settings.slideshow_interval_secs = 3.0;
        app.settings.video_playback_speed = 1.25;
        app.settings.file_organize_destinations = vec![FileOrganizeDestination::from_path(
            app.tmp.path().join("target-only"),
        )];
        app.settings.favorites = vec![crate::settings::FavoriteEntry::new(
            "target favorite".into(),
            app.tmp.path().join("favorite"),
        )];
        app.settings.tags = vec![crate::settings::TagDef::new("target tag definition".into())];
        app.settings
            .video_resume_positions
            .insert("target.mp4".into(), 12.5);
        assert!(app.settings.save_checked());
        let target_db = crate::settings_db::SettingsDb::open(app.tmp.path()).unwrap();
        app.settings = target_db.load_into_settings().unwrap();
        crate::settings::apply_load_time_migrations(&mut app.settings);
        let before = app.settings.clone();
        let ratings =
            crate::rating_db::RatingDb::open_at(app.tmp.path().join("ratings.db")).unwrap();
        ratings.set("target-only", 4).unwrap();
        let history = ReadingHistoryDb::open_at(app.tmp.path().join("reading_history.db")).unwrap();
        history
            .upsert(
                ReadingHistoryEntry::new(
                    app.tmp.path().join("target.zip"),
                    ReadingHistoryKind::Zip,
                    None,
                    "target history".into(),
                    Some(3),
                    Some(10),
                ),
                1000,
            )
            .unwrap();
        let history_before = format!("{:?}", history.list_recent(1000).unwrap());
        let mut tags = crate::tags_db::TagsDb::open_at(&app.tmp.path().join("tags.db")).unwrap();
        tags.set_item_tags("target-only", ["target personal tag"], "manual")
            .unwrap();
        let tags_before = tags.display_tags_for_item("target-only");
        let collection_path = app.tmp.path().join("collection.db");
        let runtime =
            crate::collection_store::CollectionStoreRuntime::start_at(collection_path.clone())
                .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if matches!(
                runtime.try_recv_event(),
                Some(crate::collection_store::CollectionRuntimeEvent::Ready(_))
            ) {
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        runtime
            .client()
            .create_collection("target collection".into())
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
            .unwrap();
        runtime.shutdown_and_join();
        let collection_before = std::fs::read(&collection_path).unwrap();
        // These are independent stores; neither the worker nor OK may replace them.
        let sentinels = [
            "page_edits/target.dat",
            "cache/target.webp",
            "books/target/book.json",
        ];
        for name in sentinels {
            std::fs::create_dir_all(app.tmp.path().join(name).parent().unwrap()).unwrap();
            std::fs::write(
                app.tmp.path().join(name),
                format!("target user data: {name}"),
            )
            .unwrap();
        }

        app.show_settings_restore = true;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1100.0, 850.0))
            .build_state(
                |ctx, app| {
                    app.show_settings_restore_dialog(ctx);
                    app.show_preferences_dialog(ctx);
                },
                app,
            );
        harness.run();
        begin_import(harness.state_mut(), path.clone(), &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while harness.state().preferences_transfer_busy() {
            harness.run_steps(2);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(harness.state().settings.ui_theme, UiTheme::Light);
        assert_eq!(
            harness
                .state()
                .pref_state
                .as_ref()
                .unwrap()
                .settings
                .ui_theme,
            UiTheme::Dark
        );
        assert_eq!(
            target_db.load_into_settings().unwrap().ui_theme,
            UiTheme::Light
        );
        harness.get_by_label("  OK  ").click();
        harness.run();
        assert!(!harness.state().show_preferences);
        assert!(harness.state().pref_state.is_none());
        let mut persisted = target_db.load_into_settings().unwrap();
        crate::settings::apply_load_time_migrations(&mut persisted);
        assert_eq!(persisted.ui_theme, UiTheme::Dark);
        assert_eq!(persisted.text_contrast, TextContrast::Strong);
        assert_eq!(persisted.slideshow_interval_secs, 17.0);
        crate::settings_transfer::assert_excluded_unchanged(&before, &persisted);
        assert_eq!(ratings.get("target-only"), 4);
        assert_eq!(tags.display_tags_for_item("target-only"), tags_before);
        assert_eq!(std::fs::read(&collection_path).unwrap(), collection_before);
        assert_eq!(
            format!("{:?}", history.list_recent(1000).unwrap()),
            history_before
        );
        for name in sentinels {
            assert_eq!(
                std::fs::read_to_string(harness.state().tmp.path().join(name)).unwrap(),
                format!("target user data: {name}")
            );
        }

        harness
            .state_mut()
            .open_preferences_page(PreferencesPage::General);
        harness.run();
        assert_eq!(
            harness
                .state()
                .pref_state
                .as_ref()
                .unwrap()
                .settings
                .ui_theme,
            UiTheme::Dark
        );
        harness.get_by_label("キャンセル").click();
        harness.run();
        assert!(!harness.state().show_preferences);
        let cancel_file = source_dir.path().join("cancel.json");
        std::fs::write(&cancel_file, r#"{"format":"mimageviewer.preferences","format_version":1,"preferences":{"ui_theme":"Light"}}"#).unwrap();
        begin_import(harness.state_mut(), cancel_file, &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while harness.state().preferences_transfer_busy() {
            harness.run_steps(2);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        harness.get_by_label("キャンセル").click();
        harness.run();
        harness.run();
        harness.get_by_label("破棄して閉じる").click();
        harness.run();
        assert!(harness.state().pref_state.is_none());
        assert_eq!(harness.state().settings.ui_theme, UiTheme::Dark);
        let mut after_cancel = target_db.load_into_settings().unwrap();
        crate::settings::apply_load_time_migrations(&mut after_cancel);
        assert_eq!(
            serde_json::to_value(after_cancel).unwrap(),
            serde_json::to_value(persisted).unwrap()
        );
    }

    #[test]
    fn preferences_transfer_does_not_enable_network_in_draft_or_real_ok() {
        // These assertions intentionally do not use the policy's generated preservation helper.
        let mut app = crate::app::setup_app_for_test();
        app.settings.ui_theme = crate::settings::UiTheme::Light;
        app.settings.remote_service_enabled = false;
        app.settings.remote_video_streaming_enabled = false;
        app.settings.update_check_enabled = false;
        assert!(app.settings.save_checked());
        let db = crate::settings_db::SettingsDb::open(app.tmp.path()).unwrap();
        let path = app.tmp.path().join("network-enable-attempt.json");
        std::fs::write(
            &path,
            r#"{"format":"mimageviewer.preferences","format_version":1,"preferences":{"ui_theme":"Dark","remote_service_enabled":true,"remote_video_streaming_enabled":true,"update_check_enabled":true}}"#,
        )
        .unwrap();
        app.show_settings_restore = true;
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1100.0, 850.0))
            .build_state(
                |ctx, app| {
                    app.show_settings_restore_dialog(ctx);
                    app.show_preferences_dialog(ctx);
                },
                app,
            );
        harness.run();
        begin_import(harness.state_mut(), path, &egui::Context::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while harness.state().preferences_transfer_busy() {
            harness.run_steps(2);
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let draft = &harness.state().pref_state.as_ref().unwrap().settings;
        assert_eq!(draft.ui_theme, crate::settings::UiTheme::Dark);
        assert!(!draft.remote_service_enabled);
        assert!(!draft.remote_video_streaming_enabled);
        assert!(!draft.update_check_enabled);
        assert_eq!(
            harness.state().settings.ui_theme,
            crate::settings::UiTheme::Light
        );

        harness.get_by_label("  OK  ").click();
        harness.run();
        assert!(!harness.state().show_preferences);
        assert_eq!(
            harness.state().settings.ui_theme,
            crate::settings::UiTheme::Dark
        );
        assert!(!harness.state().settings.remote_service_enabled);
        assert!(!harness.state().settings.remote_video_streaming_enabled);
        assert!(!harness.state().settings.update_check_enabled);
        let saved = db.load_into_settings().unwrap();
        assert_eq!(saved.ui_theme, crate::settings::UiTheme::Dark);
        assert!(!saved.remote_service_enabled);
        assert!(!saved.remote_video_streaming_enabled);
        assert!(!saved.update_check_enabled);
    }

    fn restore_harness() -> Harness<'static, crate::app::AppTestEnvForTest> {
        let mut app = crate::app::setup_app_for_test();
        app.show_settings_restore = true;
        Harness::builder()
            .with_size(egui::vec2(1100.0, 850.0))
            .build_state(
                |ctx, app| {
                    app.show_settings_restore_dialog(ctx);
                    app.show_preferences_dialog(ctx);
                },
                app,
            )
    }

    #[test]
    fn preferences_transfer_entries_disabled_in_both_window_open_orders() {
        use egui_kittest::kittest::NodeT;
        for preferences_first in [true, false] {
            let mut app = crate::app::setup_app_for_test();
            if preferences_first {
                app.open_preferences_page(PreferencesPage::General);
                app.open_settings_restore_dialog();
            } else {
                app.open_settings_restore_dialog();
                app.open_preferences_page(PreferencesPage::General);
            }
            let mut harness = Harness::builder()
                .with_size(egui::vec2(1200.0, 900.0))
                .build_state(
                    |ctx, app| {
                        app.show_settings_restore_dialog(ctx);
                        app.show_preferences_dialog(ctx);
                    },
                    app,
                );
            harness.run();
            assert!(harness.state().show_preferences);
            assert!(harness.state().show_settings_restore);
            // Existing restoration actions keep their enabled state. Only the new entries stop.
            assert!(
                !harness
                    .get_by_label("設定を完全リセット…")
                    .accesskit_node()
                    .is_disabled()
            );
            for label in ["環境設定を書き出し…", "環境設定を取り込み…"] {
                assert!(harness.get_by_label(label).accesskit_node().is_disabled());
                harness.get_by_label(label).click();
                harness.run();
                assert!(
                    harness
                        .state()
                        .settings_restore_state
                        .preferences_transfer
                        .is_none()
                );
            }
            harness.state_mut().show_preferences = false;
            harness.state_mut().pref_state = None;
            harness.run();
            for label in ["環境設定を書き出し…", "環境設定を取り込み…"] {
                assert!(!harness.get_by_label(label).accesskit_node().is_disabled());
            }
        }
    }

    #[test]
    fn preferences_transfer_completion_preserves_an_independently_opened_draft() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.ui_theme = crate::settings::UiTheme::Light;
        assert!(app.settings.save_checked());
        let db = crate::settings_db::SettingsDb::open(app.tmp.path()).unwrap();
        let saved_before = serde_json::to_value(db.load_into_settings().unwrap()).unwrap();
        app.show_settings_restore = true;
        let tx = app.hold_preferences_transfer_for_test();
        // A separate entry can display preferences while the worker is running.
        app.show_preferences = true;
        app.ensure_preferences_state();
        app.pref_state.as_mut().unwrap().settings.ui_theme = crate::settings::UiTheme::Dark;
        app.pref_state.as_mut().unwrap().search_query = "edited search".into();
        let draft_ptr = app.pref_state.as_deref().unwrap() as *const PreferencesState;
        let draft_before =
            serde_json::to_value(&app.pref_state.as_ref().unwrap().settings).unwrap();
        let live_before = serde_json::to_value(&app.settings).unwrap();
        let generation = crate::settings::save_generation();
        tx.send(crate::settings_transfer::parse_preferences(
            r#"{"format":"mimageviewer.preferences","format_version":1,"preferences":{"slideshow_interval_secs":9.0}}"#,
        )).unwrap();
        app.poll_preferences_transfer();
        assert!(!app.preferences_transfer_busy());
        assert!(app.show_preferences);
        assert!(app.show_settings_restore);
        assert_eq!(
            app.pref_state.as_deref().unwrap() as *const PreferencesState,
            draft_ptr
        );
        assert_eq!(
            serde_json::to_value(&app.pref_state.as_ref().unwrap().settings).unwrap(),
            draft_before
        );
        assert_eq!(
            app.pref_state.as_ref().unwrap().search_query,
            "edited search"
        );
        assert_eq!(serde_json::to_value(&app.settings).unwrap(), live_before);
        assert_eq!(crate::settings::save_generation(), generation);
        assert_eq!(
            serde_json::to_value(db.load_into_settings().unwrap()).unwrap(),
            saved_before
        );
        match app.settings_restore_state.preferences_transfer.as_deref() {
            Some(PreferencesTransferState::Explanation {
                feedback: Some(PreferencesTransferFeedback::Failed(message)),
                ..
            }) => assert!(message.contains("環境設定を閉じてから行ってください")),
            _ => panic!("import must terminate with a notification"),
        }
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1100.0, 850.0))
            .build_state(
                |ctx, app| {
                    app.show_preferences_dialog(ctx);
                    app.show_settings_restore_dialog(ctx);
                },
                app,
            );
        harness.run();
        assert!(harness.query_by_label("環境設定が開かれたため、取り込みを中止しました。環境設定を閉じてから行ってください。").is_some());
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert!(
            harness
                .state()
                .settings_restore_state
                .preferences_transfer
                .is_none()
        );
        assert!(harness.state().show_preferences);
        assert!(!harness.state().show_preferences_discard_confirm);
        assert_eq!(
            harness.state().pref_state.as_deref().unwrap() as *const PreferencesState,
            draft_ptr
        );
        assert_eq!(
            harness.state().pref_state.as_ref().unwrap().search_query,
            "edited search"
        );
        assert_eq!(
            serde_json::to_value(&harness.state().pref_state.as_ref().unwrap().settings).unwrap(),
            draft_before
        );
        assert_eq!(
            serde_json::to_value(&harness.state().settings).unwrap(),
            live_before
        );
        assert_eq!(
            serde_json::to_value(db.load_into_settings().unwrap()).unwrap(),
            saved_before
        );
        assert_eq!(crate::settings::save_generation(), generation);
    }

    #[test]
    fn preferences_transfer_modal_preserves_operation_key_capture_and_resumes_after_close() {
        use crate::keymap::KeyAction;
        #[cfg(windows)]
        let _input = crate::key_input::lock_test_input();
        for (phase, key) in [
            ("explanation", egui::Key::Escape),
            ("explanation", egui::Key::A),
            ("exported", egui::Key::Enter),
            ("exported", egui::Key::Escape),
            ("busy", egui::Key::Enter),
            ("busy", egui::Key::Escape),
        ] {
            let mut app = crate::app::setup_app_for_test();
            app.show_operation_customize = true;
            app.open_settings_restore_dialog();
            let mut harness = Harness::builder()
                .with_size(egui::vec2(1200.0, 950.0))
                .build_state(
                    |ctx, app| {
                        // App::update draws these dialogs in this order.
                        app.show_preferences_dialog(ctx);
                        app.show_operation_customize_dialog(ctx);
                        app.show_settings_restore_dialog(ctx);
                    },
                    app,
                );
            harness.run();
            let action = KeyAction::GridToggleStackMode;
            let state = harness
                .state_mut()
                .operation_customize_state
                .as_mut()
                .unwrap();
            state.operation_assignment_editor = Some(OperationAssignmentEditor {
                target: OperationAssignmentTarget::Key(action),
                tab: OperationAssignmentTab::Keyboard,
            });
            state.command_edit_loaded_for = Some(action);
            state.command_chord_inputs = ["Ctrl+Q".into(), String::new(), String::new()];
            state.command_capture_slot = Some(1);
            state.command_edit_notice = Some("existing notice".into());
            let inputs = state.command_chord_inputs.clone();
            let notice = state.command_edit_notice.clone();
            let editor = state.operation_assignment_editor.clone();
            let live_before = serde_json::to_value(&harness.state().settings).unwrap();
            let generation = crate::settings::save_generation();
            let _worker = match phase {
                "explanation" => {
                    harness
                        .state_mut()
                        .start_preferences_transfer(PreferencesTransferAction::Import);
                    None
                }
                "exported" => {
                    harness
                        .state_mut()
                        .settings_restore_state
                        .preferences_transfer =
                        Some(Box::new(PreferencesTransferState::Exported {
                            item_count: 1,
                            issues: vec![],
                        }));
                    None
                }
                "busy" => Some(harness.state_mut().hold_preferences_transfer_for_test()),
                _ => unreachable!(),
            };
            harness.run_steps(2);
            harness.key_press(key);
            harness.run_steps(2);
            let state = harness.state().operation_customize_state.as_ref().unwrap();
            assert_eq!(state.command_capture_slot, Some(1), "{phase}: {key:?}");
            assert_eq!(state.command_chord_inputs, inputs, "{phase}: {key:?}");
            assert_eq!(state.command_edit_loaded_for, Some(action));
            assert_eq!(state.command_edit_notice, notice);
            assert_eq!(state.command_edit_error, None);
            assert_eq!(state.operation_assignment_editor, editor);
            assert!(harness.state().show_operation_customize);
            assert_eq!(
                serde_json::to_value(&harness.state().settings).unwrap(),
                live_before
            );
            assert_eq!(crate::settings::save_generation(), generation);
            if phase == "busy" || key == egui::Key::A {
                assert!(harness.state().preferences_transfer_dialog_open());
                harness
                    .state_mut()
                    .settings_restore_state
                    .preferences_transfer = None;
            } else {
                assert!(!harness.state().preferences_transfer_dialog_open());
            }
            // Ordinary restoration remains open: it must not block assignment capture.
            assert!(harness.state().show_settings_restore);
            harness.run_steps(2);
            harness.key_press(egui::Key::A);
            harness.run_steps(2);
            let state = harness.state().operation_customize_state.as_ref().unwrap();
            assert_eq!(state.command_capture_slot, None);
            assert_eq!(state.command_chord_inputs[1], "A");
        }
    }

    #[test]
    fn preferences_transfer_export_notification_enter_preserves_background_search() {
        let mut app = crate::app::setup_app_for_test();
        app.show_settings_restore = true;
        app.settings_restore_state.preferences_transfer =
            Some(Box::new(PreferencesTransferState::Exported {
                item_count: 1,
                issues: vec![],
            }));
        app.show_preferences = true;
        app.ensure_preferences_state();
        let state = app.pref_state.as_mut().unwrap();
        state.search_query = "動画".into();
        state.showing_results = true;
        let selected = state.selected;
        let before = serde_json::to_value(&state.settings).unwrap();
        let generation = crate::settings::save_generation();
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1100.0, 850.0))
            .build_state(
                |ctx, app| {
                    app.show_preferences_dialog(ctx);
                    app.show_settings_restore_dialog(ctx);
                },
                app,
            );
        harness.run();
        harness.key_press(egui::Key::Enter);
        harness.run();
        assert!(
            harness
                .state()
                .settings_restore_state
                .preferences_transfer
                .is_none()
        );
        assert!(harness.state().show_preferences);
        let state = harness.state().pref_state.as_ref().unwrap();
        assert_eq!(state.search_query, "動画");
        assert!(state.showing_results);
        assert_eq!(state.selected, selected);
        assert_eq!(serde_json::to_value(&state.settings).unwrap(), before);
        assert_eq!(crate::settings::save_generation(), generation);
    }

    #[test]
    fn preferences_transfer_entry_explanation_and_file_cancel_keep_live_db_and_draft_absent() {
        for (entry, action, confirmation) in [
            (
                "環境設定を書き出し…",
                PreferencesTransferAction::Export,
                "書き出す",
            ),
            (
                "環境設定を取り込み…",
                PreferencesTransferAction::Import,
                "ファイルを選ぶ",
            ),
        ] {
            let mut harness = restore_harness();
            let before = serde_json::to_value(&harness.state().settings).unwrap();
            let generation = crate::settings::save_generation();
            harness.run();
            harness.get_by_label(entry).click();
            harness.run();
            assert!(harness.query_by_label(confirmation).is_some());
            assert!(
                matches!(harness.state().settings_restore_state.preferences_transfer.as_deref(), Some(PreferencesTransferState::Explanation { action: actual, .. }) if *actual == action)
            );
            assert!(!harness.state().preferences_transfer_busy());
            harness
                .state_mut()
                .preferences_transfer_path_selected(None, &egui::Context::default());
            harness.run();
            assert!(harness.query_by_label(confirmation).is_some());
            assert!(harness.state().pref_state.is_none());
            assert_eq!(
                serde_json::to_value(&harness.state().settings).unwrap(),
                before
            );
            assert_eq!(crate::settings::save_generation(), generation);
            // Only the child explanation is dismissed; the restore entry remains open.
            harness.get_by_label("キャンセル").click();
            harness.run();
            assert!(
                harness
                    .state()
                    .settings_restore_state
                    .preferences_transfer
                    .is_none()
            );
            assert!(harness.state().show_settings_restore);
            harness.get_by_label(entry).click();
            harness.run();
            assert!(harness.query_by_label(confirmation).is_some());
            harness.key_press(egui::Key::Escape);
            harness.run();
            assert!(
                harness
                    .state()
                    .settings_restore_state
                    .preferences_transfer
                    .is_none()
            );
            assert!(harness.state().show_settings_restore);
            assert!(harness.state().pref_state.is_none());
        }
    }

    #[test]
    fn preferences_transfer_broken_future_and_disconnected_import_never_create_draft() {
        for text in [
            "broken JSON",
            r#"{"format":"mimageviewer.preferences","format_version":999,"preferences":{"ui_theme":"Dark"}}"#,
        ] {
            let mut app = crate::app::setup_app_for_test();
            let before = serde_json::to_value(&app.settings).unwrap();
            let path = app.tmp.path().join("invalid.json");
            std::fs::write(&path, text).unwrap();
            begin_import(&mut app, path, &egui::Context::default());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while app.preferences_transfer_busy() {
                app.poll_preferences_transfer();
                assert!(std::time::Instant::now() < deadline);
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            assert!(matches!(
                app.settings_restore_state.preferences_transfer.as_deref(),
                Some(PreferencesTransferState::Explanation {
                    feedback: Some(PreferencesTransferFeedback::Failed(_)),
                    ..
                })
            ));
            assert!(app.pref_state.is_none());
            assert!(!app.show_preferences);
            assert!(app.show_settings_restore);
            assert_eq!(serde_json::to_value(&app.settings).unwrap(), before);
            let tx = app.hold_preferences_transfer_for_test();
            drop(tx);
            app.poll_preferences_transfer();
            assert!(!app.preferences_transfer_busy());
            assert!(app.pref_state.is_none());
        }
    }

    #[test]
    fn preferences_transfer_busy_single_owner_blocks_entry_cancel_close_and_escape() {
        let mut harness = restore_harness();
        harness.run();
        let tx = harness.state_mut().hold_preferences_transfer_for_test();
        let generation = crate::settings::save_generation();
        harness.run_steps(2);
        // The production restoration window and its other actions cannot acquire input.
        for label in ["環境設定を書き出し…", "環境設定を取り込み…", "Close window"]
        {
            harness.get_by_label(label).click();
            harness.run_steps(2);
            assert!(harness.state().preferences_transfer_busy());
            assert!(harness.state().show_settings_restore);
        }
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        assert!(harness.state().show_settings_restore);
        assert!(harness.state().preferences_transfer_busy());
        harness
            .state_mut()
            .start_preferences_transfer(PreferencesTransferAction::Export);
        harness.state_mut().preferences_transfer_path_selected(
            Some(PathBuf::from("never-read")),
            &egui::Context::default(),
        );
        assert!(harness.state().preferences_transfer_busy());
        assert_eq!(crate::settings::save_generation(), generation);
        tx.send(Err("test completed".into())).unwrap();
        harness.run();
        assert!(!harness.state().preferences_transfer_busy());
        harness.get_by_label("キャンセル").click();
        harness.run();
        assert!(
            harness
                .state()
                .settings_restore_state
                .preferences_transfer
                .is_none()
        );
        assert!(harness.state().show_settings_restore);
    }

    #[test]
    fn preferences_transfer_dropped_owner_cannot_apply_to_a_new_request() {
        let mut app = crate::app::setup_app_for_test();
        let tx = app.hold_preferences_transfer_for_test();
        app.settings_restore_state.preferences_transfer = None;
        app.start_preferences_transfer(PreferencesTransferAction::Import);
        assert!(tx.send(crate::settings_transfer::parse_preferences(r#"{"format":"mimageviewer.preferences","format_version":1,"preferences":{"ui_theme":"Dark"}}"#)).is_err());
        app.poll_preferences_transfer();
        assert!(!app.preferences_transfer_busy());
        assert!(app.pref_state.is_none());
    }

    #[test]
    fn preferences_transfer_export_uses_committed_values_and_reports_failure() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.ui_theme = crate::settings::UiTheme::Light;
        app.settings.default_spread_mode = crate::settings::SpreadMode::all()[0];
        let standard_spread = serde_json::to_value(app.settings.default_spread_mode).unwrap();
        let mut favorite = crate::settings::FavoriteViewState::from_settings(&app.settings);
        favorite.default_spread_mode = crate::settings::SpreadMode::all()[1];
        app.settings
            .apply_favorite_view_overlay(uuid::Uuid::new_v4(), &favorite);
        app.ensure_preferences_state();
        app.pref_state.as_mut().unwrap().settings.ui_theme = crate::settings::UiTheme::Dark;
        let path = app.tmp.path().join("committed.json");
        app.start_preferences_transfer(PreferencesTransferAction::Export);
        app.preferences_transfer_path_selected(Some(path.clone()), &egui::Context::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.preferences_transfer_busy() {
            app.poll_preferences_transfer();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let json: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(json["preferences"]["ui_theme"], "Light");
        assert_eq!(json["preferences"]["default_spread_mode"], standard_spread);
        assert_eq!(
            app.settings.default_spread_mode,
            favorite.default_spread_mode
        );
        assert_eq!(
            app.pref_state.as_ref().unwrap().settings.ui_theme,
            crate::settings::UiTheme::Dark
        );
        assert!(matches!(
            app.settings_restore_state.preferences_transfer.as_deref(),
            Some(PreferencesTransferState::Exported { .. })
        ));
        app.settings_restore_state.preferences_transfer = None;
        app.start_preferences_transfer(PreferencesTransferAction::Export);
        let failed_path = app.tmp.path().join("missing-parent/out.json");
        app.preferences_transfer_path_selected(Some(failed_path), &egui::Context::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while app.preferences_transfer_busy() {
            app.poll_preferences_transfer();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(matches!(
            app.settings_restore_state.preferences_transfer.as_deref(),
            Some(PreferencesTransferState::Explanation {
                feedback: Some(PreferencesTransferFeedback::Failed(_)),
                ..
            })
        ));
    }

    #[test]
    fn preferences_transfer_explanation_ime_enter_escape_does_not_select_or_close() {
        let mut app = crate::app::setup_app_for_test();
        app.show_settings_restore = true;
        app.start_preferences_transfer(PreferencesTransferAction::Import);
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        for key in [egui::Key::Enter, egui::Key::Escape] {
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![
                        egui::Event::Ime(egui::ImeEvent::Enabled),
                        egui::Event::Ime(egui::ImeEvent::Preedit("移行".into())),
                        egui::Event::Key {
                            key,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    ..Default::default()
                },
                |ctx| app.show_settings_restore_dialog(ctx),
            );
            assert!(matches!(
                app.settings_restore_state.preferences_transfer.as_deref(),
                Some(PreferencesTransferState::Explanation { .. })
            ));
            assert!(app.show_settings_restore);
            assert!(app.pref_state.is_none());
        }
    }

    #[test]
    fn preferences_transfer_busy_background_menu_and_toolbar_pointer_handlers() {
        use crate::settings::{SortOrder, ToolbarSectionDisplay};
        for command in [
            crate::keymap::MenuCommandId::SettingsRestoreSettings,
            crate::keymap::MenuCommandId::SettingsOperationCustomize,
            crate::keymap::MenuCommandId::SettingsPreferences,
        ] {
            let mut app = crate::app::setup_app_for_test();
            let folder = app.tmp.path().join("toolbar-folder");
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join("item.jpg"), b"image").unwrap();
            app.settings.sort_order = SortOrder::DateDesc;
            app.settings.show_toolbar_sort = true;
            app.settings.toolbar_sort_items = vec![SortOrder::RatingDesc];
            app.settings.toolbar_sort_display = ToolbarSectionDisplay::Buttons;
            app.load_folder_with_scan(folder, None);
            let mut fonts_ready = false;
            let mut harness = Harness::builder()
                .with_size(egui::vec2(1700.0, 500.0))
                .build_state(
                    move |ctx, app| {
                        if !fonts_ready {
                            crate::ui_fonts::configure_fonts(ctx);
                            fonts_ready = true;
                            ctx.request_repaint();
                            return;
                        }
                        app.render_menubar(ctx);
                        app.render_toolbar(ctx);
                        app.show_settings_restore_dialog(ctx);
                    },
                    app,
                );
            harness.run();
            let menu_header_pos = harness
                .get_all_by_label("設定")
                .next()
                .unwrap()
                .rect()
                .center();
            harness.get_all_by_label("設定").next().unwrap().click();
            harness.run();
            let label = harness.state().keymap.menu_command_label(command);
            assert!(harness.query_by_label(&label).is_some());
            let toolbar_pos = harness.get_by_label("評価↓").rect().center();
            let (tx, rx) = mpsc::channel();
            harness.state_mut().show_settings_restore = true;
            harness
                .state_mut()
                .settings_restore_state
                .preferences_transfer = Some(Box::new(PreferencesTransferState::Exporting(rx)));
            harness.run_steps(2); // The production busy spinner intentionally keeps repainting.
            assert!(harness.state().common_modal_dialog_open());
            assert_eq!(
                harness.state().modal_dialog_block_reason(),
                Some("settings_restore")
            );
            // Click the actual menu header and toolbar through the production
            // preferences modal, with no transfer-specific guards in either handler.
            for pos in [menu_header_pos, toolbar_pos] {
                harness.hover_at(pos);
                for pressed in [true, false] {
                    harness.event(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    });
                    harness.step();
                }
            }
            harness.run_steps(2);
            assert!(
                harness.query_by_label(&label).is_none(),
                "busy modal must prevent reopening the actual settings menu"
            );
            assert!(harness.state().show_settings_restore);
            assert!(!harness.state().show_operation_customize);
            assert!(!harness.state().show_preferences);
            assert_eq!(harness.state().settings.sort_order, SortOrder::DateDesc);
            // The same toolbar path remains usable after completion.
            tx.send(Ok((128, Vec::new()))).unwrap();
            harness.state_mut().poll_preferences_transfer();
            assert!(harness.state().common_modal_dialog_open());
            harness
                .state_mut()
                .settings_restore_state
                .preferences_transfer = None;
            harness.state_mut().show_settings_restore = false;
            harness.run_steps(2); // Retire the previous pass's egui modal layer.
            harness.run();
            harness.get_by_label("評価↓").click();
            harness.run();
            assert_eq!(harness.state().settings.sort_order, SortOrder::RatingDesc);
            harness.hover_at(menu_header_pos);
            for pressed in [true, false] {
                harness.event(egui::Event::PointerButton {
                    pos: menu_header_pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                });
                harness.step();
            }
            harness.run();
            harness.get_by_label(&label).click();
            harness.run();
            match command {
                crate::keymap::MenuCommandId::SettingsRestoreSettings => {
                    assert!(harness.state().show_settings_restore)
                }
                crate::keymap::MenuCommandId::SettingsOperationCustomize => {
                    assert!(harness.state().show_operation_customize)
                }
                crate::keymap::MenuCommandId::SettingsPreferences => {
                    assert!(harness.state().show_preferences)
                }
                _ => unreachable!(),
            }
        }
    }
}
