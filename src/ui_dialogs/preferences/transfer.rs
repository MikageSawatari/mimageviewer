//! One PreferencesState-owned file job. It edits only the existing draft.

use super::*;
use crate::settings_transfer::{ImportReport, TransferIssue};

pub(super) const SETTINGS_TRANSFER_LABEL: &str = "設定の持ち運び";

pub(super) enum PreferencesTransferJob {
    Idle,
    Importing(mpsc::Receiver<Result<crate::settings_transfer::ParsedPreferences, String>>),
    Exporting(mpsc::Receiver<Result<(usize, Vec<TransferIssue>), String>>),
}

impl PreferencesTransferJob {
    pub(super) fn is_busy(&self) -> bool {
        !matches!(self, Self::Idle)
    }
}

pub(super) enum PreferencesTransferFeedback {
    Imported(ImportReport),
    Exported {
        item_count: usize,
        issues: Vec<TransferIssue>,
    },
    Failed(String),
}

#[derive(Clone, Copy)]
pub(super) enum PreferencesTransferAction {
    Export,
    Import,
}

impl PreferencesState {
    pub(super) fn transfer_ready(&self) -> bool {
        !self.transfer_job.is_busy()
            && self.ui_font_apply_ready()
            && self.ui_font_import_rx.is_none()
            && self.creative_lut_import_rx.is_none()
    }

    pub(super) fn start_preferences_import(&mut self, path: PathBuf, ctx: &egui::Context) {
        if !self.transfer_ready() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        match std::thread::Builder::new()
            .name("preferences-import".into())
            .spawn(move || {
                let result = crate::settings_transfer::read_preferences(&path);
                let _ = tx.send(result);
                ctx.request_repaint();
            }) {
            Ok(_) => {
                self.transfer_feedback = None;
                self.transfer_job = PreferencesTransferJob::Importing(rx);
            }
            Err(error) => {
                self.transfer_failed(format!("読み込み処理を開始できませんでした: {error}"))
            }
        }
    }

    pub(super) fn start_preferences_export(&mut self, path: PathBuf, ctx: &egui::Context) {
        if !self.transfer_ready() {
            return;
        }
        // Capture just portable fields; never clone plugin states/user data for the worker.
        let snapshot = crate::settings_transfer::capture_export(&self.settings);
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        match std::thread::Builder::new()
            .name("preferences-export".into())
            .spawn(move || {
                let result = snapshot.export().and_then(|exported| {
                    crate::settings_transfer::write_preferences(&path, &exported.json)?;
                    Ok((exported.item_count, exported.issues))
                });
                let _ = tx.send(result);
                ctx.request_repaint();
            }) {
            Ok(_) => {
                self.transfer_feedback = None;
                self.transfer_job = PreferencesTransferJob::Exporting(rx);
            }
            Err(error) => {
                self.transfer_failed(format!("書き出し処理を開始できませんでした: {error}"))
            }
        }
    }

    fn transfer_failed(&mut self, message: String) {
        crate::logger::log(format!("[preferences-transfer] {message}"));
        self.transfer_feedback = Some(PreferencesTransferFeedback::Failed(message));
    }

    pub(super) fn poll_preferences_transfer(&mut self) {
        match &self.transfer_job {
            PreferencesTransferJob::Idle => {}
            PreferencesTransferJob::Importing(rx) => match rx.try_recv() {
                Ok(result) => {
                    self.transfer_job = PreferencesTransferJob::Idle;
                    match result {
                        Ok(parsed) => {
                            let report = parsed.apply_to(&mut self.settings);
                            self.transfer_feedback =
                                Some(PreferencesTransferFeedback::Imported(report));
                        }
                        Err(error) => {
                            self.transfer_failed(format!("ファイルを取り込めませんでした: {error}"))
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.transfer_job = PreferencesTransferJob::Idle;
                    self.transfer_failed("読み込み処理が応答せず終了しました。".into());
                }
            },
            PreferencesTransferJob::Exporting(rx) => match rx.try_recv() {
                Ok(result) => {
                    self.transfer_job = PreferencesTransferJob::Idle;
                    match result {
                        Ok((item_count, issues)) => {
                            self.transfer_feedback =
                                Some(PreferencesTransferFeedback::Exported { item_count, issues });
                        }
                        Err(error) => {
                            self.transfer_failed(format!("ファイルへ書き出せませんでした: {error}"))
                        }
                    }
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.transfer_job = PreferencesTransferJob::Idle;
                    self.transfer_failed("書き出し処理が応答せず終了しました。".into());
                }
            },
        }
    }
}

pub(super) fn draw_settings_transfer(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let action = render_settings_transfer(
        ui,
        &state.transfer_job,
        state.transfer_ready(),
        state.transfer_feedback.as_ref(),
    );
    match action {
        Some(PreferencesTransferAction::Export) => {
            if let Some(mut path) = rfd::FileDialog::new()
                .add_filter("mIV preferences", &["json"])
                .set_file_name("preferences.mivprefs.json")
                .save_file()
            {
                if path.extension().is_none() {
                    path.set_extension("json");
                }
                state.start_preferences_export(path, ui.ctx());
            }
        }
        Some(PreferencesTransferAction::Import) => {
            if let Some(path) = rfd::FileDialog::new()
                .add_filter("mIV preferences", &["json"])
                .pick_file()
            {
                state.start_preferences_import(path, ui.ctx());
            }
        }
        None => {}
    }
}

pub(super) fn render_settings_transfer(
    ui: &mut egui::Ui,
    job: &PreferencesTransferJob,
    ready: bool,
    feedback: Option<&PreferencesTransferFeedback>,
) -> Option<PreferencesTransferAction> {
    ui.label(egui::RichText::new(SETTINGS_TRANSFER_LABEL).strong());
    ui.label("この画面の移行できる設定をファイルに保存します。保存先や利用データ、操作カスタマイズは含みません。");
    ui.small("書き出しには、この画面で変更した未確定の設定も含みます。");
    ui.small("操作カスタマイズは、設定メニューの専用画面から書き出せます。");
    let mut action = None;
    ui.horizontal_wrapped(|ui| {
        if ui
            .add_enabled(ready, egui::Button::new("書き出し…"))
            .clicked()
        {
            action = Some(PreferencesTransferAction::Export);
        }
        if ui
            .add_enabled(ready, egui::Button::new("取り込み…"))
            .clicked()
        {
            action = Some(PreferencesTransferAction::Import);
        }
    });
    match job {
        PreferencesTransferJob::Importing(_) => {
            ui.label("設定ファイルを読み込んでいます…");
        }
        PreferencesTransferJob::Exporting(_) => {
            ui.label("設定ファイルを書き出しています…");
        }
        PreferencesTransferJob::Idle => {}
    }
    if !ready && !job.is_busy() {
        ui.small("ほかの設定の準備が完了すると利用できます。");
    }
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
    action
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

    fn draft(settings: &Settings) -> PreferencesState {
        super::super::tests::preferences_state_for_test(settings)
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
        let mut source_state = draft(&source);
        source_state.start_preferences_export(path.clone(), &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while source_state.transfer_job.is_busy() {
            source_state.poll_preferences_transfer();
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(matches!(
            source_state.transfer_feedback.as_ref(),
            Some(PreferencesTransferFeedback::Exported { .. })
        ));
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

        app.open_preferences_page(PreferencesPage::General);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1100.0, 850.0))
            .build_state(|ctx, app| app.show_preferences_dialog(ctx), app);
        harness.run();
        harness
            .state_mut()
            .pref_state
            .as_mut()
            .unwrap()
            .start_preferences_import(path.clone(), &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while harness.state().preferences_transfer_busy() {
            harness.run();
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
        let cancel_file = source_dir.path().join("cancel.json");
        std::fs::write(&cancel_file, r#"{"format":"mimageviewer.preferences","format_version":1,"preferences":{"ui_theme":"Light"}}"#).unwrap();
        harness
            .state_mut()
            .pref_state
            .as_mut()
            .unwrap()
            .start_preferences_import(cancel_file, &ctx);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while harness.state().preferences_transfer_busy() {
            harness.run();
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
        app.open_preferences_page(PreferencesPage::General);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1100.0, 850.0))
            .build_state(|ctx, app| app.show_preferences_dialog(ctx), app);
        harness.run();
        harness
            .state_mut()
            .pref_state
            .as_mut()
            .unwrap()
            .start_preferences_import(path, &egui::Context::default());
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while harness.state().preferences_transfer_busy() {
            harness.run();
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

    #[test]
    fn preferences_transfer_busy_keeps_its_draft_and_single_job() {
        let mut app = crate::app::setup_app_for_test();
        app.show_preferences = true;
        let mut state = draft(&app.settings);
        let (tx, rx) = mpsc::channel(); // Hold the job deterministically without filesystem timing.
        state.transfer_job = PreferencesTransferJob::Importing(rx);
        app.pref_state = Some(Box::new(state));
        let ctx = egui::Context::default();
        app.open_preferences_page(PreferencesPage::Folder);
        app.request_close_preferences_dialog();
        assert!(!app.show_settings_restore);
        assert!(!app.show_operation_customize);
        assert!(!app.show_preferences_discard_confirm);
        assert!(app.show_preferences);
        assert!(app.preferences_requested_page.is_none());
        assert!(app.grid_open_from_click_allowed() == false);
        // A second worker cannot replace the first receiver, and close/OK remain disabled.
        let unused_path = app.tmp.path().join("never-open");
        app.pref_state
            .as_mut()
            .unwrap()
            .start_preferences_import(unused_path, &ctx);
        assert!(app.preferences_transfer_busy());
        tx.send(Err("test failure".into())).unwrap();
        app.pref_state.as_mut().unwrap().poll_preferences_transfer();
        assert!(!app.preferences_transfer_busy());
        assert!(app.pref_state.as_ref().unwrap().transfer_ready());
    }

    #[test]
    fn preferences_transfer_whole_file_failure_and_disconnected_job_keep_draft() {
        let mut state = draft(&Settings::default());
        let before = serde_json::to_value(&state.settings).unwrap();
        let (tx, rx) = mpsc::channel();
        state.transfer_job = PreferencesTransferJob::Importing(rx);
        tx.send(crate::settings_transfer::parse_preferences("broken JSON"))
            .unwrap();
        state.poll_preferences_transfer();
        assert!(!state.transfer_job.is_busy());
        assert!(matches!(
            state.transfer_feedback.as_ref(),
            Some(PreferencesTransferFeedback::Failed(_))
        ));
        assert_eq!(serde_json::to_value(&state.settings).unwrap(), before);
        let (tx, rx) = mpsc::channel();
        state.transfer_job = PreferencesTransferJob::Importing(rx);
        drop(tx);
        state.poll_preferences_transfer();
        assert!(!state.transfer_job.is_busy());
        assert_eq!(serde_json::to_value(&state.settings).unwrap(), before);
    }

    #[test]
    fn preferences_transfer_dropped_dialog_cannot_apply_to_new_draft() {
        let mut old = draft(&Settings::default());
        let (tx, rx) = mpsc::channel();
        old.transfer_job = PreferencesTransferJob::Importing(rx);
        drop(old);
        let mut reopened = draft(&Settings::default());
        let before = serde_json::to_value(&reopened.settings).unwrap();
        let result = crate::settings_transfer::parse_preferences(
            r#"{"format":"mimageviewer.preferences","format_version":1,"preferences":{"ui_theme":"Dark"}}"#,
        );
        assert!(tx.send(result).is_err());
        reopened.poll_preferences_transfer();
        assert!(!reopened.transfer_job.is_busy());
        assert_eq!(serde_json::to_value(&reopened.settings).unwrap(), before);
    }

    #[test]
    fn preferences_transfer_ime_confirmation_does_not_apply_or_cancel() {
        let mut app = crate::app::setup_app_for_test();
        app.open_preferences_page(PreferencesPage::General);
        let ctx = egui::Context::default();
        crate::ime_focus::install_ime_input_policy(&ctx);
        let _ = ctx.run(Default::default(), |ctx| app.show_preferences_dialog(ctx));
        let generation = crate::settings::save_generation();
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
                |ctx| app.show_preferences_dialog(ctx),
            );
            assert!(app.show_preferences);
            assert!(!app.show_preferences_discard_confirm);
            assert!(!app.preferences_transfer_busy());
            assert_eq!(crate::settings::save_generation(), generation);
        }
    }

    #[test]
    fn preferences_transfer_busy_ok_cancel_close_and_escape_do_not_close_draft() {
        let mut app = crate::app::setup_app_for_test();
        app.open_preferences_page(PreferencesPage::General);
        let mut harness = Harness::builder()
            .with_size(egui::vec2(1100.0, 850.0))
            .build_state(|ctx, app| app.show_preferences_dialog(ctx), app);
        harness.run();
        let tx = harness.state_mut().hold_preferences_transfer_for_test();
        let generation = crate::settings::save_generation();
        harness.run_steps(2);
        for label in ["  OK  ", "キャンセル", "Close window"] {
            harness.get_by_label(label).click();
            harness.run_steps(2);
            assert!(harness.state().show_preferences);
            assert!(harness.state().preferences_transfer_busy());
            assert!(!harness.state().show_preferences_discard_confirm);
            assert_eq!(crate::settings::save_generation(), generation);
        }
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        assert!(harness.state().show_preferences);
        tx.send(Err("test completed".into())).unwrap();
        harness.run();
        assert!(!harness.state().preferences_transfer_busy());
        harness.get_by_label("キャンセル").click();
        harness.run();
        assert!(harness.state().pref_state.is_none());
    }

    #[test]
    fn preferences_transfer_busy_background_menu_and_toolbar_pointer_handlers() {
        use crate::settings::{SortOrder, ToolbarSectionDisplay};
        for command in [
            crate::keymap::MenuCommandId::SettingsRestoreSettings,
            crate::keymap::MenuCommandId::SettingsOperationCustomize,
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
                        app.show_preferences_dialog(ctx);
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
            let mut state = draft(&harness.state().settings);
            state.transfer_job = PreferencesTransferJob::Exporting(rx);
            harness.state_mut().show_preferences = true;
            harness.state_mut().pref_state = Some(Box::new(state));
            harness.run_steps(2); // The production busy spinner intentionally keeps repainting.
            assert!(harness.state().common_modal_dialog_open());
            assert_eq!(
                harness.state().modal_dialog_block_reason(),
                Some("preferences")
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
            assert!(!harness.state().show_settings_restore);
            assert!(!harness.state().show_operation_customize);
            assert_eq!(harness.state().settings.sort_order, SortOrder::DateDesc);
            // The same toolbar path remains usable after completion.
            tx.send(Ok((128, Vec::new()))).unwrap();
            harness
                .state_mut()
                .pref_state
                .as_mut()
                .unwrap()
                .poll_preferences_transfer();
            // Completion keeps the preferences dialog modal; closing it restores background UI.
            assert!(harness.state().common_modal_dialog_open());
            harness.state_mut().show_preferences = false;
            harness.state_mut().pref_state = None;
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
                _ => unreachable!(),
            }
        }
    }
}
