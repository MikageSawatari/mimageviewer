//! お気に入り追加ダイアログと、画面・AI が共有するお気に入りの確定処理。
//!
//! お気に入りに現在のフォルダを追加する際に、表示名を入力させるための
//! 小さなモーダルダイアログ。`App` の `show_fav_add_dialog` フラグが
//! true のときだけ描画される。
//!
//! `update()` から `self.show_fav_add_dialog_window(ctx)` で呼ばれる。

#![allow(unused_imports)]

use std::path::PathBuf;

use eframe::egui;

use crate::app::App;
use crate::settings::{FavoriteAddError, MAX_FAVORITES};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct FavoriteAddFlags {
    pub(crate) item_index: bool,
    pub(crate) container_index: bool,
    pub(crate) thumbs: bool,
    pub(crate) similar: bool,
}

impl App {
    /// Commit a favorite through the same path used by the add dialog.
    /// Physical-folder admission belongs to the caller; this does not inspect the target.
    pub(crate) fn commit_favorite_add(
        &mut self,
        name: String,
        target: PathBuf,
        flags: FavoriteAddFlags,
    ) -> Result<uuid::Uuid, FavoriteAddError> {
        let target = crate::path_key::normalize_input_path(&target);
        let name = name.trim().to_string();
        self.settings.try_add_favorite(name, target)?;
        // 追加した最後のエントリに自動インデックスフラグを反映
        let similar_feature_enabled = self.similar_feature_capability().is_enabled();
        let (newly_on_structure, newly_on_similar, fav_id, fav_path) =
            if let Some(last) = self.settings.favorites.last_mut() {
                last.auto_index_structure = flags.container_index;
                last.auto_index_metadata = flags.item_index;
                last.auto_index_thumbs = flags.thumbs;
                if similar_feature_enabled {
                    last.auto_index_similar = flags.similar;
                }
                (
                    last.auto_index_structure,
                    last.auto_index_similar,
                    last.id,
                    last.path.clone(),
                )
            } else {
                (false, false, uuid::Uuid::nil(), PathBuf::new())
            };
        self.settings.save();
        // The ordinary metadata watcher remains live while the optional similar
        // service is paused. Do not route the hidden saved flag through its
        // configure/purge path.
        if similar_feature_enabled {
            self.apply_favorite_similar_index_change(&fav_path, newly_on_similar);
        } else {
            self.sync_shared_favorite_indexers();
        }
        // 名前索引: 新規追加 + structure=true なら bulk を起動。
        // apply_favorite_name_index_change に一本化して、cancel/progress
        // 管理 (name_bulk_handles) も揃える。
        if newly_on_structure {
            self.apply_favorite_name_index_change(fav_id, &fav_path, true);
        }
        // cache_creator_checked は favorites と同じ長さを保つ
        self.cc.checked.resize(self.settings.favorites.len(), false);
        Ok(fav_id)
    }

    /// Apply only supplied index flags, in the favorites editor's commit order.
    pub(crate) fn commit_favorite_index_flags(
        &mut self,
        id: uuid::Uuid,
        item_index: Option<bool>,
        container_index: Option<bool>,
    ) -> bool {
        let Some(favorite) = self.settings.favorites.iter_mut().find(|fav| fav.id == id) else {
            return false;
        };
        let container_changed =
            container_index.is_some_and(|value| value != favorite.auto_index_structure);
        let item_changed = item_index.is_some_and(|value| value != favorite.auto_index_metadata);
        if !container_changed && !item_changed {
            return false;
        }
        if let Some(value) = container_index {
            favorite.auto_index_structure = value;
        }
        if let Some(value) = item_index {
            favorite.auto_index_metadata = value;
        }
        let path = favorite.path.clone();
        let container_on = favorite.auto_index_structure;
        let item_on = favorite.auto_index_metadata;
        // Helpers consume the complete updated settings snapshot, including both flags.
        if container_changed {
            self.apply_favorite_name_index_change(id, &path, container_on);
        }
        if item_changed {
            self.apply_favorite_meta_index_change(id, item_on);
        }
        self.settings.save();
        true
    }

    pub(crate) fn show_fav_add_dialog_window(&mut self, ctx: &egui::Context) {
        if !self.show_fav_add_dialog {
            return;
        }

        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let dialog_pos = ctx.content_rect().min + egui::vec2(80.0, 60.0);
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);

        egui::Window::new("お気に入りに追加")
            .drag_area(egui::WindowDrag::Anywhere)
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(420.0);

                if let Some(ref target) = self.fav_add_target {
                    ui.label("次のフォルダをお気に入りに追加します:");
                    ui.add_space(2.0);
                    ui.label(
                        egui::RichText::new(target.to_string_lossy())
                            .monospace()
                            .weak(),
                    );
                    ui.add_space(8.0);
                    let field_label = ui.label("表示名 (ツールバーやメニューに表示される名前):");
                    ui.add_space(2.0);

                    let resp = crate::ime_focus::add_singleline(
                        ui,
                        &mut self.fav_add_name_input,
                        None,
                        |edit| edit.desired_width(f32::INFINITY),
                    )
                    .labelled_by(field_label.id);
                    // 初回フォーカス
                    if !resp.has_focus()
                        && ctx.input(|i| i.focused)
                        && !ui.memory(|m| m.focused().is_some())
                    {
                        resp.request_focus();
                    }
                    // Enter で決定 (IME 変換中は dialog_enter_pressed が false を返す)
                    if resp.lost_focus() && enter_pressed {
                        apply = true;
                    }

                    ui.add_space(6.0);
                    ui.label(
                        egui::RichText::new(format!(
                            "登録数: {} / {}",
                            self.settings.favorites.len(),
                            MAX_FAVORITES
                        ))
                        .weak(),
                    );
                    if let Some(error) = self.fav_add_error.as_deref() {
                        ui.add_space(4.0);
                        ui.colored_label(egui::Color32::from_rgb(220, 80, 80), error);
                    }

                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new(
                            "索引化 (あとで編集可能):\n\
                             チェックした項目はこの場で 1 回全走査し、以降は\n\
                             ファイルの変更監視で自動更新します。\n\
                             終了中の変更を起動時に確認するかは設定で選べます。",
                        )
                        .weak(),
                    );
                    ui.add_space(2.0);
                    ui.checkbox(
                        &mut self.fav_add_auto_index_structure,
                        "コンテナ索引 (フォルダ・ZIP・PDF を Ctrl+S で名前検索)",
                    );
                    ui.checkbox(
                        &mut self.fav_add_auto_index_metadata,
                        "アイテム索引 (画像・PDF・動画をファイル名 / タグ / EXIF 等で Ctrl+G 検索)",
                    );
                    if favorite_add_similar_control_visible(self.similar_feature_capability()) {
                        ui.checkbox(
                            &mut self.fav_add_auto_index_similar,
                            "別バージョン索引 (表示中の画像と同じ絵の別バージョンを検索)",
                        );
                    }
                    // サムネイルは I/O が重い (GB 規模) ため自動化から外し、手動バルクのみ
                    // (「お気に入り」ダイアログからサムネ一括作成ボタンで起動)
                } else {
                    ui.label("追加対象のフォルダが不明です。");
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let can_apply =
                        self.fav_add_target.is_some() && !self.fav_add_name_input.trim().is_empty();
                    if ui
                        .add_enabled(can_apply, egui::Button::new("  追加  "))
                        .clicked()
                    {
                        apply = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });

        if apply {
            self.fav_add_error = None;
            if let Some(target) = self.fav_add_target.clone() {
                let flags = FavoriteAddFlags {
                    item_index: self.fav_add_auto_index_metadata,
                    container_index: self.fav_add_auto_index_structure,
                    thumbs: self.fav_add_auto_index_thumbs,
                    similar: self.fav_add_auto_index_similar,
                };
                match self.commit_favorite_add(self.fav_add_name_input.clone(), target, flags) {
                    Ok(_) => {
                        self.show_fav_add_dialog = false;
                        self.fav_add_name_input.clear();
                        self.fav_add_target = None;
                        self.fav_add_error = None;
                        // 次回デフォルト値は false リセット (design doc §8.2 の "環境設定のデフォルトから取る" は v1.x で検討)
                        self.fav_add_auto_index_structure = false;
                        self.fav_add_auto_index_metadata = false;
                        self.fav_add_auto_index_thumbs = false;
                        self.fav_add_auto_index_similar = false;
                    }
                    Err(FavoriteAddError::Duplicate) => {
                        self.fav_add_error =
                            Some("このフォルダはすでにお気に入りに登録されています。".to_string());
                    }
                    Err(FavoriteAddError::LimitReached { max }) => {
                        self.fav_add_error = Some(format!(
                            "お気に入りは最大 {max} 件まで登録できます。不要な項目を削除してから追加してください。"
                        ));
                    }
                }
            } else {
                self.fav_add_error = Some("追加対象のフォルダが不明です。".to_string());
            }
        } else if cancel || !open || escape_pressed {
            self.show_fav_add_dialog = false;
            self.fav_add_name_input.clear();
            self.fav_add_target = None;
            self.fav_add_error = None;
            self.fav_add_auto_index_structure = false;
            self.fav_add_auto_index_metadata = false;
            self.fav_add_auto_index_thumbs = false;
            self.fav_add_auto_index_similar = false;
        }
    }
}

fn favorite_add_similar_control_visible(
    capability: crate::similar_index::SimilarFeatureCapability,
) -> bool {
    capability.is_enabled()
}

#[cfg(test)]
mod similar_control_visibility_tests {
    use super::favorite_add_similar_control_visible;
    use crate::similar_index::SimilarFeatureCapability;

    #[test]
    fn similar_control_visibility_follows_the_shared_capability() {
        assert!(!favorite_add_similar_control_visible(
            SimilarFeatureCapability::Paused
        ));
        assert!(favorite_add_similar_control_visible(
            SimilarFeatureCapability::Enabled
        ));
        assert!(favorite_add_similar_control_visible(
            crate::similar_index::PRODUCT_SIMILAR_FEATURE_CAPABILITY
        ));
    }
}

#[cfg(test)]
mod favorite_commit_tests {
    use super::*;

    fn setup_app() -> crate::app::AppTestEnvForTest {
        let mut app = crate::app::setup_app_for_test();
        // State/commit coverage only: indexer integration has its own manager fixtures.
        // Keep these commits from starting scan workers or touching any physical root.
        app.name_index_manager = None;
        app.search_index_db = None;
        app.indexer_init = crate::app::IndexerInit::NotStarted;
        app.similar_index = None;
        app.settings.favorites.clear();
        app.cc.checked.clear();
        app
    }

    #[test]
    fn fav_add_commit_trims_normalizes_sets_flags_and_aligns_checked() {
        let mut app = setup_app();
        let first = app.tmp.path().join("first");
        app.settings
            .try_add_favorite("First".into(), first)
            .unwrap();
        app.cc.checked = vec![true];
        let target = PathBuf::from("c:/Pictures/Favorite");
        let id = app
            .commit_favorite_add(
                "  Favorite\t ".into(),
                target.clone(),
                FavoriteAddFlags {
                    item_index: true,
                    container_index: true,
                    thumbs: true,
                    similar: true,
                },
            )
            .unwrap();

        let favorite = app.settings.favorites.last().unwrap();
        assert_eq!(favorite.id, id);
        assert_ne!(id, uuid::Uuid::nil());
        assert_eq!(favorite.name, "Favorite");
        assert_eq!(
            favorite.path,
            crate::path_key::normalize_input_path(&target)
        );
        assert!(favorite.auto_index_metadata);
        assert!(favorite.auto_index_structure);
        assert!(favorite.auto_index_thumbs);
        assert!(
            !favorite.auto_index_similar,
            "paused service keeps its default flag"
        );
        assert_eq!(app.cc.checked, vec![true, false]);
        assert!(app.name_index_manager.is_none());
        assert!(app.similar_index.is_none());
    }

    #[test]
    fn fav_add_commit_duplicate_and_limit_leave_entries_and_checked_unchanged() {
        let mut app = setup_app();
        let target = app.tmp.path().join("favorite");
        app.commit_favorite_add("First".into(), target.clone(), FavoriteAddFlags::default())
            .unwrap();
        app.cc.checked[0] = true;
        let first_id = app.settings.favorites[0].id;
        assert_eq!(
            app.commit_favorite_add(
                "Duplicate".into(),
                target.clone(),
                FavoriteAddFlags::default()
            ),
            Err(FavoriteAddError::Duplicate)
        );
        assert_eq!(app.settings.favorites.len(), 1);
        assert_eq!(app.settings.favorites[0].id, first_id);
        assert_eq!(app.settings.favorites[0].name, "First");
        assert_eq!(app.cc.checked, vec![true]);

        for index in 1..MAX_FAVORITES {
            let path = app.tmp.path().join(format!("favorite-{index}"));
            app.settings
                .try_add_favorite(index.to_string(), path)
                .unwrap();
        }
        let checked_before = app.cc.checked.clone();
        let overflow = app.tmp.path().join("overflow");
        assert_eq!(
            app.commit_favorite_add("Overflow".into(), overflow, FavoriteAddFlags::default()),
            Err(FavoriteAddError::LimitReached { max: MAX_FAVORITES })
        );
        assert_eq!(app.settings.favorites.len(), MAX_FAVORITES);
        assert_eq!(app.cc.checked, checked_before);
        // Preserve Settings' duplicate-before-limit precedence.
        assert_eq!(
            app.commit_favorite_add("Duplicate".into(), target, FavoriteAddFlags::default()),
            Err(FavoriteAddError::Duplicate)
        );
    }

    #[test]
    fn fav_add_commit_preserves_legacy_empty_name_admission() {
        let mut app = setup_app();
        let target = app.tmp.path().join("empty-name");
        app.commit_favorite_add(" \t ".into(), target, FavoriteAddFlags::default())
            .unwrap();
        assert_eq!(app.settings.favorites[0].name, "");
    }

    #[test]
    fn fav_add_dialog_button_commits_and_resets_through_the_shared_path() {
        use egui_kittest::kittest::Queryable;
        let mut app = setup_app();
        let target = app.tmp.path().join("dialog-favorite");
        std::fs::create_dir(&target).unwrap();
        app.settings.book_root = Some(app.tmp.path().join("books"));
        app.fav_add_target = Some(target.clone());
        app.fav_add_name_input = "  Dialog Favorite  ".into();
        app.fav_add_auto_index_structure = true;
        app.fav_add_auto_index_metadata = true;
        app.show_fav_add_dialog = true;
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(900.0, 700.0))
            .build_ui_state(
                |ui, app: &mut crate::app::AppTestEnvForTest| {
                    app.show_fav_add_dialog_window(ui.ctx())
                },
                app,
            );
        harness.run();
        harness.get_by_label("  追加  ").click();
        harness.run();
        let app = harness.state();
        let favorite = &app.settings.favorites[0];
        assert_eq!(favorite.name, "Dialog Favorite");
        assert_eq!(
            favorite.path,
            crate::path_key::normalize_input_path(&target)
        );
        assert!(favorite.auto_index_metadata && favorite.auto_index_structure);
        assert_eq!(app.cc.checked, [false]);
        assert!(!app.show_fav_add_dialog);
        assert!(app.fav_add_target.is_none());
        assert!(app.fav_add_name_input.is_empty());
        assert!(!app.fav_add_auto_index_metadata && !app.fav_add_auto_index_structure);
    }

    #[test]
    fn favorite_index_commit_changes_only_supplied_flags_and_preserves_other_state() {
        let mut app = setup_app();
        let target = app.tmp.path().join("favorite");
        let id = app
            .commit_favorite_add(
                "Favorite".into(),
                target.clone(),
                FavoriteAddFlags::default(),
            )
            .unwrap();
        app.settings.favorites[0].auto_index_thumbs = true;
        app.settings.favorites[0].auto_index_similar = true;
        app.cc.checked[0] = true;

        assert!(app.commit_favorite_index_flags(id, Some(true), Some(true)));
        assert!(app.commit_favorite_index_flags(id, Some(false), None));
        let favorite = &app.settings.favorites[0];
        assert_eq!(favorite.id, id);
        assert_eq!(favorite.name, "Favorite");
        assert_eq!(favorite.path, target);
        assert!(!favorite.auto_index_metadata);
        assert!(favorite.auto_index_structure);
        assert!(favorite.auto_index_thumbs);
        assert!(favorite.auto_index_similar);
        assert_eq!(app.cc.checked, vec![true]);
        assert!(!app.commit_favorite_index_flags(id, Some(false), Some(true)));
        assert!(!app.commit_favorite_index_flags(id, None, None));
        assert!(!app.commit_favorite_index_flags(uuid::Uuid::new_v4(), Some(true), Some(false)));
        assert!(app.commit_favorite_index_flags(id, None, Some(false)));
        assert!(!app.settings.favorites[0].auto_index_structure);
        assert!(app.settings.favorites[0].auto_index_similar);
        assert!(app.settings.favorites[0].auto_index_thumbs);
    }
}
