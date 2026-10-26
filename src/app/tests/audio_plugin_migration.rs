use super::*;

fn pending(
    app: &mut App,
) -> mpsc::Sender<Result<crate::effetune::migration::MigrationOutcome, String>> {
    let (migration, tx) = crate::effetune::startup::StartupMigration::pending_for_test();
    app.dsp_bridge = crate::video::dsp::DspBridge::new_waiting_for_migration(migration.gate());
    app.effetune = crate::effetune::EffetuneController::new_with_startup_gate(migration.gate());
    app.audio_plugin_migration = migration;
    tx
}

#[test]
fn pending_migration_blocks_startup_toolbar_rebuild_scan_and_remote_bridge_consumers() {
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut env = setup_app_for_test();
    let app = &mut env.app;
    let _tx = pending(app);
    app.settings.vst3_enabled = true;
    let generation = app.dsp_bridge.current_chain_rebuild_gen();
    let remote = app.remote_clockless_audio_processing(1.0);
    let local = app.local_audio_dsp_chain();
    app.kick_off_vst3_startup_load();
    app.kick_off_vst3_chain_rebuild();
    assert_eq!(app.dsp_bridge.current_chain_rebuild_gen(), generation);
    assert!(!app.vst3_init_kicked);
    assert!(app.vst3_startup_load.is_none());
    assert!(app.media_startup_load_pending());
    assert!(!app.effetune_toolbar_available());
    app.effetune_toolbar_click();
    app.effetune.startup(None, None);
    assert!(!app.effetune.startup_pending());
    // Even direct background consumers cannot bypass the one App-owned gate.
    assert!(app.dsp_bridge.enable().unwrap_err().contains("引き継ぎ"));
    assert!(!local.user.as_ref().unwrap().is_enabled());
    let sound = crate::video::dsp::DspBridge::new_for_miv_sound_with_startup_gate(
        env.tmp.path().join("blocked-sound"),
        app.audio_plugin_migration.gate(),
    );
    assert!(sound.enable().unwrap_err().contains("引き継ぎ"));
    let _ = remote;
    app.open_preferences_page(crate::ui_dialogs::preferences::PreferencesPage::Vst3);
    let mut harness = crate::test_ui::builder()
        .with_size(egui::vec2(1100.0, 850.0))
        .with_context_setup(crate::ui_fonts::configure_fonts)
        .build_ui_state(|ui, app| app.show_preferences_dialog(ui.ctx()), env);
    harness.run();
    assert!(
        harness
            .get_by_label("プラグインをスキャン")
            .accesskit_node()
            .is_disabled()
    );
}

#[test]
fn failed_migration_notifies_once_and_allows_only_user_vst_for_this_process() {
    let mut env = setup_app_for_test();
    let app = &mut env.app;
    let tx = pending(app);
    tx.send(Err("fixture: exclusive source lock busy".into()))
        .unwrap();
    let ctx = egui::Context::default();
    app.poll_audio_plugin_migration(&ctx);
    assert!(app.audio_plugin_migration.gate().user_vst_allowed());
    assert!(!app.audio_plugin_migration.gate().sound_allowed());
    assert!(!app.media_startup_load_pending());
    assert!(matches!(
        app.effetune.runtime,
        crate::effetune::EffetuneRuntime::Failed(_)
    ));
    assert_eq!(
        app.fs_feedback_toast.as_ref().unwrap().0,
        crate::effetune::startup::FAILURE_NOTICE
    );
    let recorded = app.fs_feedback_toast.as_ref().unwrap().1;
    app.poll_audio_plugin_migration(&ctx);
    assert_eq!(app.fs_feedback_toast.as_ref().unwrap().1, recorded);
    assert!(!app.effetune_toolbar_available());
}

#[test]
fn terminal_continuation_uses_latest_settings_after_pending_changes() {
    let mut env = setup_app_for_test();
    let app = &mut env.app;
    let tx = pending(app);
    app.settings.vst3_enabled = true;
    app.kick_off_vst3_chain_rebuild();
    assert!(!app.vst3_init_kicked);
    // A configured chain removed while waiting must not load stale startup data.
    app.settings.vst3_plugins.push(
        serde_json::from_value(serde_json::json!({"path":"never-load-migration-fixture.vst3"}))
            .unwrap(),
    );
    app.settings.vst3_plugins.clear();
    tx.send(Ok(
        crate::effetune::migration::MigrationOutcome::NotApplicable,
    ))
    .unwrap();
    app.poll_audio_plugin_migration(&egui::Context::default());
    assert!(app.vst3_init_kicked);
    assert!(app.vst3_startup_load.is_none());
    assert!(app.audio_plugin_migration.gate().sound_allowed());
}

#[test]
fn migration_terminal_without_downstream_loader_resumes_video_and_audio_after_sidecar_barrier() {
    for audio in [false, true] {
        for succeeded in [false, true] {
            let mut env = setup_app_for_test();
            let path = env.tmp.path().join(if audio {
                "unavailable.wav"
            } else {
                "unavailable.mp4"
            });
            let folder = env.tmp.path().join("sidecar-barrier");
            let app = &mut env.app;
            let tx = pending(app);
            app.settings.vst3_enabled = false;
            app.effetune.runtime = crate::effetune::EffetuneRuntime::Unavailable(
                crate::effetune::UnavailableReason::CpuUnsupported,
            );
            app.items = vec![if audio {
                GridItem::Audio(path)
            } else {
                GridItem::Video(path)
            }];
            app.fullscreen_idx = Some(0);
            app.vst3_deferred_media_open = Some(0);
            app.activate_sidecar_restore_modal_for_test(folder);
            tx.send(if succeeded {
                Ok(crate::effetune::migration::MigrationOutcome::NotApplicable)
            } else {
                Err("lock busy fixture".into())
            })
            .unwrap();
            let ctx = egui::Context::default();
            app.poll_audio_plugin_migration(&ctx);
            assert!(app.audio_plugin_migration.waiting());
            assert_eq!(app.vst3_deferred_media_open, Some(0));
            app.sidecar_restore = None; // The existing recovery owner settled.
            app.poll_audio_plugin_migration(&ctx);
            assert!(!app.media_startup_load_pending());
            assert_eq!(
                app.vst3_deferred_media_open, None,
                "no later loader will deliver another completion"
            );
            app.poll_audio_plugin_migration(&ctx);
            assert_eq!(app.vst3_deferred_media_open, None);
        }
    }
}
