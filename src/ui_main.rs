//! メイン画面の UI コンポーネント描画。
//!
//! `App::update()` から呼ばれるメニューバー・ツールバー・アドレスバー・
//! グリッド・進捗オーバーレイ・選択情報オーバーレイの描画メソッドを集約。

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use eframe::egui;

use crate::app::App;
use crate::grid_item::{GridItem, ThumbnailState};
// open_external_player はグリッドからは使わなくなった (動画はフルスクリーン化 →
// インライン再生)。フォルダ系は別途同モジュールから直接呼んでいる箇所がある。

use crate::ui_helpers::{
    PROGRESS_BG_COLOR, PROGRESS_LABEL_COLOR, PROGRESS_NORMAL_COLOR, PROGRESS_UPGRADE_COLOR,
};

// ── ★フィルタのツールバー挙動 (Ctrl/Shift/右クリック) ─────────────────
//
// 通常クリック: そのバケットをトグル
// Ctrl+クリック: solo (そのバケットだけ ON)。同 solo 状態で再クリック → 全 ON (DAW 流)
// Shift+クリック: threshold (そのバケット以上 ON)。同状態で再クリック → 全 ON
// 右クリック: コンテキストメニューから同 3 操作 (こちらは toggle せず常に「set」)

/// アドレスバーの 📌 ボタンが受けたクリック種別 (closure 内で `self` への
/// ミュータブル呼び出しを避けるため、closure を抜けてから dispatch する)。
#[derive(Clone, Copy)]
enum PinButtonClick {
    None,
    /// 左クリック: 選択 item が現在の pin と一致なら解除、不一致なら set。
    Toggle,
    /// 右クリック: 解除。
    Remove,
}

/// 📌 ボタン描画用の状態スナップショット。`render_address_bar` 入口で 1 度算出する。
pub(crate) struct FolderPinButtonState {
    /// ボタンを enable にして良いか (false なら disabled + tooltip 表示)
    pub enabled: bool,
    /// hover 時の tooltip 文字列
    pub tooltip: String,
    /// 現在選択中の item が既に container の pin source と一致しているか
    /// (true: 強調アイコンで「クリックで解除」を示唆)
    pub matches_current_pin: bool,
}

#[derive(Clone, Copy)]
enum RatingFilterOp {
    Toggle,
    Solo,
    /// ★N + 未評価: `rating_filter[0]` と `rating_filter[idx]` だけを ON にする。
    ///
    /// ★5 を Ctrl+クリックするとフォルダまで消えてナビゲーションできなくなる問題への対処。
    /// 「★N の画像をフォルダツリーで探す」ワークフロー向け。ただし `rating_filter` は
    /// コンテナ (Folder/ZIP/PDF) と画像系 (Image/ZipImage/PdfPage) の両方に同じバケットを
    /// 適用するため、副次的に **未評価の通常画像** も表示される (UI 上は意図した挙動として
    /// ラベルを「★N と未評価」としている)。「フォルダだけを残す」ためには
    /// `[bool; 6]` では表現できず kind-aware な別モードが必要で、v0.8.2 以降の検討事項。
    /// idx=0 では意味をなさないので `apply_rating_filter_op` は idx>=1 前提
    /// (idx=0 なら Solo と同値)。
    SoloWithUnrated,
    Threshold,
    AllOn,
}

/// グリッドのセル寸法 (`cell_w`, `cell_h`) を計算する。
///
/// `avail_w <= 0` (chrome が幅を食い切った) は `None` を返してグリッド描画を skip。
/// `MIN_CELL_PX` 下限を強制しないと、`viewport_h / cell_h` が数百〜数千行に暴発して
/// 1 フレームで数千セル描画して UI フリーズする (極端に窓を狭めた時の実害バグ)。
const MIN_CELL_PX: f32 = 32.0;
fn compute_cell_size(avail_w: f32, cols: usize, height_ratio: f32) -> Option<(f32, f32)> {
    if avail_w <= 0.0 {
        return None;
    }
    let cols = cols.max(1);
    let cell_w = (avail_w / cols as f32).floor().max(MIN_CELL_PX);
    let cell_h = (cell_w * height_ratio).round().max(MIN_CELL_PX);
    Some((cell_w, cell_h))
}

fn is_rating_solo(rf: &[bool; 6], idx: usize) -> bool {
    (0..6).all(|i| rf[i] == (i == idx))
}

fn is_rating_threshold(rf: &[bool; 6], idx: usize) -> bool {
    (0..idx).all(|i| !rf[i]) && (idx..6).all(|i| rf[i])
}

/// 現在フィルタが「★N + 未評価」状態か (idx>=1 前提)。
fn is_rating_solo_with_unrated(rf: &[bool; 6], idx: usize) -> bool {
    if idx == 0 {
        return false;
    }
    rf[0] && rf[idx] && (1..6).all(|i| i == idx || !rf[i])
}

fn apply_rating_filter_op(rf: &mut [bool; 6], op: RatingFilterOp, idx: usize) {
    match op {
        RatingFilterOp::Toggle => rf[idx] = !rf[idx],
        RatingFilterOp::Solo => {
            for i in 0..6 {
                rf[i] = i == idx;
            }
        }
        RatingFilterOp::SoloWithUnrated => {
            for i in 0..6 {
                rf[i] = i == 0 || i == idx;
            }
        }
        RatingFilterOp::Threshold => {
            for i in 0..6 {
                rf[i] = i >= idx;
            }
        }
        RatingFilterOp::AllOn => {
            *rf = crate::settings::default_rating_filter();
        }
    }
}

fn rating_button_label(idx: usize) -> String {
    if idx == 0 {
        "なし".to_string()
    } else {
        "★".repeat(idx)
    }
}

fn rating_solo_menu_label(idx: usize) -> String {
    if idx == 0 {
        "未評価のみ表示 (Ctrl+クリック)".to_string()
    } else {
        format!("★{} のみ表示 (Ctrl+クリック)", idx)
    }
}

fn rating_threshold_menu_label(idx: usize) -> String {
    if idx == 0 {
        "すべて表示 (Shift+クリック)".to_string()
    } else {
        format!("★{} 以上を表示 (Shift+クリック)", idx)
    }
}

/// 「★N と未評価」(= ★N + なし) メニュー用ラベル。idx>=1 のみ有効。
/// 「フォルダだけ」ではなく `rating_filter[0]` バケットに入るもの全部 (未評価画像 /
/// 未評価 ZIP 内画像 / 未評価 PDF ページ + フォルダ / ZIP / PDF) が対象なので、
/// 文言は「未評価」に寄せて誤解を避ける。
fn rating_solo_with_unrated_menu_label(idx: usize) -> String {
    format!("★{} と未評価 (Ctrl+Shift+クリック)", idx)
}

fn rating_tooltip(idx: usize) -> String {
    if idx == 0 {
        "未評価を表示 [F6 で解除]\n通常クリック: 切り替え\nCtrl+クリック: これのみ\nShift+クリック: すべて表示"
            .to_string()
    } else {
        format!(
            "★{idx} を表示 [F{idx} で付与]\n通常クリック: 切り替え\nCtrl+クリック: これのみ\nShift+クリック: ★{idx} 以上\nCtrl+Shift+クリック: ★{idx} と未評価"
        )
    }
}

fn counts_as_thumbnail_item(item: &GridItem) -> bool {
    !matches!(item, GridItem::ZipSeparator { .. })
}

fn thumbnail_count_label(items: &[GridItem], visible_indices: &[usize]) -> String {
    let total = items
        .iter()
        .filter(|item| counts_as_thumbnail_item(item))
        .count();
    let visible = visible_indices
        .iter()
        .filter_map(|&idx| items.get(idx))
        .filter(|item| counts_as_thumbnail_item(item))
        .count();
    let width = total.max(1).to_string().len();
    format!("({:>width$}/{})", visible, total, width = width)
}

/// ★フィルタのボタン 1 個を描画し、状態が変わったら true を返す。
/// `enabled = false` の間はクリックを無視し、見た目も disabled スタイルで描画する。
///
/// `add_enabled_ui` で外側からまとめてしまうと、`horizontal_wrapped` 内では scope が
/// 残り幅だけの狭い子 UI を作るのでレイアウトが崩れる。そのため enabled は呼び出し側
/// から各ボタンに直接渡す。`context_menu` は egui 上、disabled でも開いてしまうので
/// `if enabled` で明示ガードする (`resp.clicked()` 側は `add_enabled` が消費するので
/// 二重ガードは belt-and-suspenders)。
fn draw_rating_filter_button(
    ui: &mut egui::Ui,
    rf: &mut [bool; 6],
    idx: usize,
    enabled: bool,
) -> bool {
    let sel = rf[idx];
    let resp = ui
        .add_enabled(
            enabled,
            egui::Button::selectable(sel, rating_button_label(idx)),
        )
        .on_hover_text(rating_tooltip(idx));
    let mut changed = false;
    if enabled && resp.clicked() {
        let mods = ui.input(|i| i.modifiers);
        // Windows 専用ビルドなので mods.command は ctrl と同値 (egui 内で alias)。
        // 既存コード (src/ui_main.rs:992 の Ctrl+クリック選択等) と合わせて ctrl のみを見る。
        // 優先順位: Ctrl+Shift > Ctrl > Shift > 通常。
        let op = if mods.ctrl && mods.shift && idx >= 1 {
            // ★N + 未評価 (= `rating_filter[0]` も ON)。idx=0 では意味を成さないので
            // 除外 (下の Ctrl 単独に落ちる)。
            if is_rating_solo_with_unrated(rf, idx) {
                RatingFilterOp::AllOn
            } else {
                RatingFilterOp::SoloWithUnrated
            }
        } else if mods.ctrl {
            if is_rating_solo(rf, idx) {
                RatingFilterOp::AllOn
            } else {
                RatingFilterOp::Solo
            }
        } else if mods.shift {
            if is_rating_threshold(rf, idx) {
                RatingFilterOp::AllOn
            } else {
                RatingFilterOp::Threshold
            }
        } else {
            RatingFilterOp::Toggle
        };
        apply_rating_filter_op(rf, op, idx);
        changed = true;
    }
    // 右クリックメニューは常に「set」(toggle せず) なので op を直接渡す。
    if enabled {
        resp.context_menu(|ui| {
            if ui.button(rating_solo_menu_label(idx)).clicked() {
                apply_rating_filter_op(rf, RatingFilterOp::Solo, idx);
                changed = true;
                ui.close();
            }
            if ui.button(rating_threshold_menu_label(idx)).clicked() {
                apply_rating_filter_op(rf, RatingFilterOp::Threshold, idx);
                changed = true;
                ui.close();
            }
            if idx >= 1
                && ui
                    .button(rating_solo_with_unrated_menu_label(idx))
                    .clicked()
            {
                apply_rating_filter_op(rf, RatingFilterOp::SoloWithUnrated, idx);
                changed = true;
                ui.close();
            }
            ui.separator();
            if ui.button("すべて表示").clicked() {
                apply_rating_filter_op(rf, RatingFilterOp::AllOn, idx);
                changed = true;
                ui.close();
            }
        });
    }
    changed
}

impl App {
    // ── メニューバー ─────────────────────────────────────────────────

    /// メニューバーを描画し、ナビゲーション先とソート変更の有無を返す。
    pub(crate) fn render_menubar(&mut self, ctx: &egui::Context) -> (Option<PathBuf>, bool) {
        let mut fav_nav: Option<PathBuf> = None;
        let mut settings_changed = false;
        let mut sort_changed = false;
        let selected_video_path =
            self.selected
                .and_then(|idx| self.items.get(idx))
                .and_then(|item| match item {
                    GridItem::Video(path) => Some(path.clone()),
                    _ => None,
                });

        egui::TopBottomPanel::top("menubar").show(ctx, |ui| {
            egui::MenuBar::new().ui(ui, |ui| {
                ui.menu_button("ファイル", |ui| {
                    if ui.button("フォルダを開く…").clicked() {
                        // 既に現在フォルダが設定されていれば初期値として補完
                        self.open_folder_input = self
                            .current_folder
                            .as_ref()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_default();
                        self.show_open_folder_dialog = true;
                        ui.close();
                    }
                    if ui.button("メタデータ検索 (Ctrl+F)").clicked() {
                        // 相互排他は open_local_metadata_search 内で (Ctrl+S/Ctrl+G を閉じる)
                        self.open_local_metadata_search();
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("終了").clicked() {
                        // トレイ常駐設定 ON のときでも [×] ではなく明示終了なので、
                        // `shutdown_requested` を立てて `maybe_intercept_close` を通す。
                        self.shutdown_requested
                            .store(true, std::sync::atomic::Ordering::SeqCst);
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });

                ui.menu_button("お気に入り", |ui| {
                    // このフォルダを追加 (クリック時は名称入力ダイアログを開く)
                    let can_add = self.current_folder.is_some();
                    if ui
                        .add_enabled(can_add, egui::Button::new("このフォルダを追加…"))
                        .clicked()
                    {
                        if let Some(ref folder) = self.current_folder.clone() {
                            // 既定の名前はフォルダ名から補完
                            let default_name = folder
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();
                            self.fav_add_name_input = default_name;
                            self.fav_add_target = Some(folder.clone());
                            self.show_fav_add_dialog = true;
                        }
                        ui.close();
                    }

                    // 編集
                    if ui.button("編集").clicked() {
                        self.show_favorites_editor = true;
                        ui.close();
                    }

                    // 名前で検索 (Ctrl+S)
                    if ui.button("名前で検索 (Ctrl+S)").clicked() {
                        self.open_favsearch();
                        ui.close();
                    }

                    // メタデータ検索 (Ctrl+G)
                    if ui.button("メタデータ検索 (Ctrl+G)").clicked() {
                        // 相互排他は toggle_global_search 内で
                        self.toggle_global_search();
                        ui.close();
                    }

                    // 区切り線
                    ui.separator();

                    // 登録済みお気に入り一覧
                    if self.settings.favorites.is_empty() {
                        ui.label(egui::RichText::new("（未登録）").weak());
                    } else {
                        let favorites = self.settings.favorites.clone();
                        for fav in &favorites {
                            if ui.button(&fav.name).clicked() {
                                fav_nav = Some(fav.path.clone());
                                ui.close();
                            }
                        }
                    }
                });

                ui.menu_button("動画", |ui| {
                    let can_apply_to_selected = selected_video_path.is_some();
                    if ui
                        .add_enabled(
                            can_apply_to_selected,
                            egui::Button::new("この動画をアップスケール登録…"),
                        )
                        .clicked()
                    {
                        if let Some(path) = selected_video_path.clone() {
                            self.request_video_upscale(path);
                        }
                        ui.close();
                    }
                    if ui
                        .add_enabled(
                            can_apply_to_selected,
                            egui::Button::new("この動画のアップスケールを削除"),
                        )
                        .clicked()
                    {
                        if let Some(path) = selected_video_path.clone() {
                            self.request_video_upscale_artifact_delete(path);
                        }
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("アップスケールタスク表示").clicked() {
                        self.show_video_upscale_tasks = true;
                        ui.close();
                    }
                });

                // タグメニュー (docs/tag-feature.md §4.2)
                ui.menu_button("タグ", |ui| {
                    if ui.button("タグを編集…").clicked() {
                        self.open_tag_editor();
                        ui.close();
                    }
                    ui.separator();
                    let selection_count = self.tag_target_path_count();
                    let has_target = selection_count > 0;
                    if ui
                        .add_enabled(
                            has_target,
                            egui::Button::new(format!(
                                "選択中のファイルから mIV タグをクリア ({selection_count})"
                            )),
                        )
                        .on_hover_text(
                            "`#` で始まる dc:subject 要素のみ削除します。\n\
                             他ソフトで付けたタグ (#なし) は触りません。",
                        )
                        .clicked()
                    {
                        self.request_tag_clear_for_selection();
                        ui.close();
                    }
                    ui.separator();
                    if self.settings.tags.is_empty() {
                        ui.label(egui::RichText::new("（タグが未登録）").weak());
                    } else {
                        let tags_snapshot = self.settings.tags.clone();
                        for tag in &tags_snapshot {
                            let label = format!("#{}", tag.name);
                            let btn = egui::Button::new(label);
                            if ui.add_enabled(has_target, btn).clicked() {
                                self.request_tag_toggle_for_selection(&tag.name);
                                ui.close();
                            }
                        }
                    }
                });

                ui.menu_button("設定", |ui| {
                    ui.menu_button("サムネイル列数", |ui| {
                        for cols in crate::settings::MIN_GRID_COLS..=crate::settings::MAX_GRID_COLS
                        {
                            let checked = self.settings.grid_cols == cols;
                            let prefix = if checked { "✓ " } else { "  " };
                            if ui.button(format!("{prefix}{cols} 列")).clicked() {
                                self.settings.grid_cols = cols;
                                settings_changed = true;
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("サムネイル比率", |ui| {
                        for &aspect in crate::settings::ThumbAspect::all() {
                            let checked = self.settings.thumb_aspect == aspect;
                            let prefix = if checked { "✓ " } else { "  " };
                            if ui.button(format!("{prefix}{}", aspect.label())).clicked() {
                                self.settings.thumb_aspect = aspect;
                                settings_changed = true;
                                ui.close();
                            }
                        }
                    });
                    ui.menu_button("ソート順", |ui| {
                        for &order in crate::settings::SortOrder::all() {
                            let checked = self.settings.sort_order == order;
                            let prefix = if checked { "✓ " } else { "  " };
                            if ui.button(format!("{prefix}{}", order.label())).clicked() {
                                self.settings.sort_order = order;
                                sort_changed = true;
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    if ui.button("サムネイルキャッシュ管理").clicked() {
                        let cache_dir = crate::catalog::default_cache_dir();
                        // cache_stats は数千フォルダで秒級になるのでワーカーに回す。
                        // ダイアログは「取得中...」表示で開き、poll 完了時に stats が埋まる。
                        self.cache_manager_stats = None;
                        self.cache_manager_tile_bytes = None;
                        self.cache_manager_result = None;
                        if self.cache_maint_pending.is_none() {
                            self.cache_maint_pending = Some(crate::cache_maintenance::spawn(
                                crate::cache_maintenance::CacheMaintTask::Stats,
                                cache_dir,
                                self.video_tile_cache.clone(),
                            ));
                        }
                        self.show_cache_manager = true;
                        ui.close();
                    }
                    if ui.button("変換済みアーカイブキャッシュ管理").clicked() {
                        self.open_archive_cache_manager();
                        ui.close();
                    }
                    if ui.button("サムネイル画質…").clicked() {
                        self.open_thumb_quality_dialog(ctx);
                        ui.close();
                    }
                    if ui.button("統計…").clicked() {
                        self.show_stats_dialog = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("回転情報をリセット…").clicked() {
                        self.show_rotation_reset_confirm = true;
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("環境設定…").clicked() {
                        self.show_preferences = true;
                        ui.close();
                    }
                    // VST3 関連の設定は環境設定→VST3 プラグインページに集約。
                    // 専用メニューは重複なので持たない (= ユーザー要望 2026-04)。
                    // 動画再生中はホバーバー / ツールバーの VST ボタンから
                    // プレイバックパネルを開く運用。
                });

                ui.menu_button("ヘルプ", |ui| {
                    if ui.button("ヘルプサイトを開く").clicked() {
                        let url = format!(
                            "https://www.mikage.to/mimageviewer/manual/index.html?version={}",
                            env!("CARGO_PKG_VERSION"),
                        );
                        crate::ui_helpers::open_url(&url);
                        ui.close();
                    }
                    ui.separator();
                    if ui.button("ログフォルダを開く").clicked() {
                        let dir = crate::data_dir::logs_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        crate::ui_helpers::open_external_player(&dir);
                        ui.close();
                    }
                    ui.separator();
                    let checking = self.update_check_pending.is_some();
                    if ui
                        .add_enabled(
                            !checking,
                            egui::Button::new(if checking {
                                "更新を確認中…"
                            } else {
                                "更新を確認…"
                            }),
                        )
                        .clicked()
                    {
                        self.kick_update_check(true);
                        ui.close();
                    }
                    if ui.button("バージョン情報").clicked() {
                        self.show_about_dialog = true;
                        ui.close();
                    }
                });

                // メニュー項目の右側に新バージョン通知バッジを表示する。
                // 押すと更新ダイアログを開き、リリースページへの誘導 / skip 操作を行える。
                if self.should_show_update_badge() {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let tag = self
                            .update_info
                            .as_ref()
                            .map(|i| i.latest_tag.clone())
                            .unwrap_or_default();
                        let resp = ui
                            .add(
                                egui::Button::new(
                                    egui::RichText::new(format!("🔔 新バージョン {tag}"))
                                        .color(egui::Color32::from_rgb(100, 170, 100))
                                        .size(12.0),
                                )
                                .fill(egui::Color32::from_rgb(40, 70, 40)),
                            )
                            .on_hover_text(
                                "新しいバージョンがリリースされています。\n\
                                     クリックで詳細を表示します。",
                            );
                        if resp.clicked() {
                            self.show_update_dialog = true;
                        }
                    });
                }
            });
        });

        if settings_changed {
            self.settings.save();
        }
        if sort_changed {
            self.settings.save();
            if let Some(path) = self.current_folder.clone() {
                // スクロール履歴を捨てて先頭から再ロード
                self.folder_history.remove(&path);
                self.load_folder(path);
            }
        }

        (fav_nav, sort_changed)
    }

    // ── 進捗バー ─────────────────────────────────────────────────────

    /// 進捗バーオーバーレイ（左下フローティング）を描画する。
    pub(crate) fn render_progress_overlay(&self, ctx: &egui::Context) {
        let ((cur_normal, peak_normal), (cur_upgrade, peak_upgrade)) = self.progress_snapshot();
        if peak_normal == 0 && peak_upgrade == 0 {
            return;
        }

        egui::Area::new("progress_overlay".into())
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(8.0, -8.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(PROGRESS_BG_COLOR)
                    .show(ui, |ui| {
                        if peak_normal > 0 {
                            let done = peak_normal.saturating_sub(cur_normal);
                            let progress = done as f32 / peak_normal as f32;
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("先読み    ")
                                        .monospace()
                                        .color(PROGRESS_LABEL_COLOR),
                                );
                                ui.add(
                                    egui::ProgressBar::new(progress)
                                        .desired_width(220.0)
                                        .fill(PROGRESS_NORMAL_COLOR)
                                        .text(
                                            egui::RichText::new(format!(
                                                "{} / {}",
                                                done, peak_normal
                                            ))
                                            .color(egui::Color32::BLACK),
                                        ),
                                );
                            });
                        }
                        if peak_upgrade > 0 {
                            let done = peak_upgrade.saturating_sub(cur_upgrade);
                            let progress = done as f32 / peak_upgrade as f32;
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new("高画質化  ")
                                        .monospace()
                                        .color(PROGRESS_LABEL_COLOR),
                                );
                                ui.add(
                                    egui::ProgressBar::new(progress)
                                        .desired_width(220.0)
                                        .fill(PROGRESS_UPGRADE_COLOR)
                                        .text(
                                            egui::RichText::new(format!(
                                                "{} / {}",
                                                done, peak_upgrade
                                            ))
                                            .color(egui::Color32::BLACK),
                                        ),
                                );
                            });
                        }
                    });
            });
        // 進行中は毎フレーム再描画してバーをスムーズに更新
        ctx.request_repaint();
    }

    // ── ツールバー ───────────────────────────────────────────────────

    /// ツールバーを描画し、お気に入りナビゲーション先を返す。
    /// ソート変更があった場合はフォルダの再ロードも行う。
    pub(crate) fn render_toolbar(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        // Vec を先にクローンして borrow checker の制約を回避
        let tb_cols = self.settings.toolbar_cols_items.clone();
        let tb_aspects = self.settings.toolbar_aspect_items.clone();
        let tb_sorts = self.settings.toolbar_sort_items.clone();
        let show_cols = !tb_cols.is_empty();
        let show_aspect = !tb_aspects.is_empty();
        let show_sort = !tb_sorts.is_empty();
        let show_favs = self.settings.show_toolbar_favorites;
        let show_parent = self.settings.show_toolbar_parent_button;
        let show_prev_folder = self.settings.show_toolbar_prev_folder;
        let show_next_folder = self.settings.show_toolbar_next_folder;
        let show_rating = self.settings.show_toolbar_rating;
        let any_toolbar_section = show_cols
            || show_aspect
            || show_sort
            || show_favs
            || show_parent
            || show_prev_folder
            || show_next_folder
            || show_rating;

        if !any_toolbar_section {
            return None;
        }

        let mut toolbar_fav_nav: Option<PathBuf> = None;
        let mut toolbar_sort_changed = false;
        let mut toolbar_parent_nav = false;
        let mut toolbar_prev_folder_nav = false;
        let mut toolbar_next_folder_nav = false;
        let mut toolbar_rating_changed = false;
        let mut toolbar_tag_click: Option<String> = None;

        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                let mut first_section = true;
                if show_parent {
                    let has_parent = self
                        .current_folder
                        .as_ref()
                        .and_then(|p| p.parent())
                        .is_some();
                    if ui
                        .add_enabled(has_parent, egui::Button::new("⬆"))
                        .on_hover_text("上のフォルダへ [BS]")
                        .clicked()
                    {
                        toolbar_parent_nav = true;
                    }
                    first_section = false;
                }
                // Phase 5.8: 前のフォルダ / 次のフォルダ ボタン (= Ctrl+↑↓ と等価)。
                // 「上」と区別するため、塗りつぶし三角 ▲▼ を使う (= 親フォルダボタン
                // の輪郭三角 ⬆ とは形が違う)。
                if show_prev_folder {
                    let has_current = self.current_folder.is_some();
                    if ui
                        .add_enabled(has_current, egui::Button::new("▲"))
                        .on_hover_text("前のフォルダへ [Ctrl+↑]")
                        .clicked()
                    {
                        toolbar_prev_folder_nav = true;
                    }
                    first_section = false;
                }
                if show_next_folder {
                    let has_current = self.current_folder.is_some();
                    if ui
                        .add_enabled(has_current, egui::Button::new("▼"))
                        .on_hover_text("次のフォルダへ [Ctrl+↓]")
                        .clicked()
                    {
                        toolbar_next_folder_nav = true;
                    }
                    first_section = false;
                }
                // ツールバー VST ボタンは v0.9.0 開発中に削除 (= ユーザー要望 2026-04
                // 「ツールバーの VST ボタンも不要になったので削除」)。
                // VST3 プラグインのプレイバックパネルは動画再生中にホバーバー側の
                // VST ボタンから開く (フルスクリーンビューポート内で完結)。
                // 通常表示中はパネルを開く手段は無く、設定変更は環境設定→
                // VST3 プラグイン から行う運用。
                if show_cols {
                    if !first_section {
                        ui.separator();
                    }
                    ui.label("列:");
                    for &cols in &tb_cols {
                        let selected = self.settings.grid_cols == cols;
                        if ui.selectable_label(selected, format!(" {cols} ")).clicked() {
                            self.settings.grid_cols = cols;
                            self.settings.save();
                        }
                    }
                    first_section = false;
                }
                if show_aspect {
                    if !first_section {
                        ui.separator();
                    }
                    ui.label("比率:");
                    for &aspect in &tb_aspects {
                        let selected = self.settings.thumb_aspect == aspect;
                        if ui.selectable_label(selected, aspect.label()).clicked() {
                            self.settings.thumb_aspect = aspect;
                            self.settings.save();
                        }
                    }
                    first_section = false;
                }
                if show_sort {
                    if !first_section {
                        ui.separator();
                    }
                    ui.label("ソート:");
                    for &order in &tb_sorts {
                        let selected = self.settings.sort_order == order;
                        if ui.selectable_label(selected, order.short_label()).clicked() && !selected
                        {
                            self.settings.sort_order = order;
                            self.settings.save();
                            toolbar_sort_changed = true;
                        }
                    }
                    first_section = false;
                }
                if show_rating {
                    if !first_section {
                        ui.separator();
                    }
                    // Ctrl+G の集約ビュー (= 検索結果のフォルダ一覧) では★フィルタを
                    // 反映できない (ヒット件数と filter の二重集計が必要で実装コスト大)。
                    // ドリルイン後は file list + サブフォルダ件数の両方に反映するので
                    // enable に戻す。
                    let aggregated_search = self.global_search.active
                        && matches!(
                            self.global_search.view,
                            crate::global_search_ui::GlobalSearchView::Aggregated
                        );
                    // hover ヒントは disable 中の widget では拾われにくいので
                    // (egui の sense)、有効な「★:」ラベル側に乗せる。
                    let star_label = ui.label("★:");
                    if aggregated_search {
                        star_label.on_hover_text(
                            "検索結果のコンテナ一覧では★フィルタは適用できません。\nコンテナを開くと有効になります。",
                        );
                    }
                    // ★ボタン群を `add_enabled_ui` でまとめると、その scope が「残り幅」
                    // だけの狭い子 UI を作るので `horizontal_wrapped` の wrap が子 UI 内で
                    // 起きてしまい、★★ 以降が右端の縦帯に積まれて崩れる。enabled は各
                    // ボタン側に渡し、親の wrap に直接乗せて次の row に流させる。
                    for idx in 0..6 {
                        if draw_rating_filter_button(
                            ui,
                            &mut self.settings.rating_filter,
                            idx,
                            !aggregated_search,
                        ) {
                            toolbar_rating_changed = true;
                        }
                    }
                    // ★フィルタ一時解除中: コンテナ自身の★で開いた結果として filter が
                    // 全 ON に書き換わっている状態を示すバッジ。クリックで即復元。
                    if self.rating_filter_suppressed_at.is_some() {
                        let resp = ui
                            .small_button(
                                egui::RichText::new("★一時解除中")
                                    .color(egui::Color32::from_rgb(200, 140, 40)),
                            )
                            .on_hover_text("コンテナ自身の★で開いたため一時解除中です。\n親へ戻るか、このバッジをクリックで復元。");
                        if resp.clicked() && self.restore_rating_filter_suppression() {
                            toolbar_rating_changed = true;
                        }
                    }
                    first_section = false;
                }
                if show_favs {
                    if !first_section {
                        ui.separator();
                    }
                    ui.label("お気に入り:");
                    if self.settings.favorites.is_empty() {
                        ui.label(egui::RichText::new("(未登録)").weak());
                    } else {
                        // 現在のフォルダと一致するお気に入りをハイライト
                        let current = self.current_folder.clone();
                        for fav in &self.settings.favorites {
                            let selected =
                                current.as_ref().map(|c| c == &fav.path).unwrap_or(false);
                            if ui
                                .selectable_label(selected, &fav.name)
                                .on_hover_text(fav.path.to_string_lossy())
                                .clicked()
                            {
                                toolbar_fav_nav = Some(fav.path.clone());
                            }
                        }
                    }
                    first_section = false;
                }

                // タグセクション (docs/tag-feature.md §4.3)
                if self.settings.show_toolbar_tags && !self.settings.tags.is_empty() {
                    if !first_section {
                        ui.separator();
                    }
                    ui.label("タグ:");
                    let has_target = self.tag_target_path_count() > 0;
                    let tags_snapshot: Vec<_> = self
                        .settings
                        .tags
                        .iter()
                        .map(|t| t.name.clone())
                        .collect();
                    for name in tags_snapshot {
                        let label = format!("#{name}");
                        let resp = ui.add_enabled(has_target, egui::Button::new(label));
                        if resp.clicked() {
                            toolbar_tag_click = Some(name);
                        }
                    }
                }
            });
            ui.add_space(2.0);
        });

        // 親フォルダへ移動
        if toolbar_parent_nav {
            if let Some(ref cur) = self.current_folder.clone() {
                if let Some(parent) = cur.parent() {
                    self.select_after_load = cur
                        .file_name()
                        .and_then(|n| n.to_str())
                        .map(|s| s.to_string());
                    return Some(parent.to_path_buf());
                }
            }
        }

        // Phase 5.8: 前 / 次フォルダ ボタンは Ctrl+↑↓ と同じ DFS をキック。
        // start_folder_nav は in-flight の連打もまとめてくれる。
        if toolbar_prev_folder_nav {
            if let Some(cur) = self.current_folder.clone() {
                self.start_folder_nav(cur, false, crate::app::FolderNavMode::Grid);
            }
        }
        if toolbar_next_folder_nav {
            if let Some(cur) = self.current_folder.clone() {
                self.start_folder_nav(cur, true, crate::app::FolderNavMode::Grid);
            }
        }

        // ツールバーのソート変更は borrow の関係で遅延実行
        if toolbar_sort_changed {
            if let Some(path) = self.current_folder.clone() {
                self.folder_history.remove(&path);
                self.load_folder(path);
            }
        }

        // レーティングフィルタ変更: 設定を保存して visible_indices を再計算。
        // selected が filter から外れた場合の処理は `rebuild_visible_indices` が
        // 直近の visible idx にリダイレクト (旧コードは None にクリアしていた)。
        if toolbar_rating_changed {
            // ユーザーによる明示的な filter 操作 → suppression anchor を破棄する
            // (ユーザー意思を尊重して、BS しても以前の filter は復元しない)。
            self.drop_rating_filter_suppression_on_user_edit();
            self.settings.save();
            // Ctrl+G 合成ビュー (drilled / aggregated) ではバッジ件数が
            // build_drilled_items 側で rating_filter を使って再計算されるので
            // items 自体を作り直す。実体ビュー (Ctrl+G から開いた PDF/ZIP/Folder)
            // では合成 items に置き換えてしまわないよう visible_indices だけ
            // 再計算する (Codex P2)。
            if self.global_search.active && self.items_are_global_search_view {
                self.rebuild_items_from_global_search();
            } else {
                self.rebuild_visible_indices();
            }
        }

        // ツールバーのタグ項目クリック
        if let Some(name) = toolbar_tag_click {
            self.request_tag_toggle_for_selection(&name);
        }

        // (旧) VST3 プラグイン管理ボタンの click handler はツールバーボタン削除に伴い撤去。

        toolbar_fav_nav
    }

    // ── アドレスバー ─────────────────────────────────────────────────

    /// アドレスバーを描画し、Enter で確定されたパスを返す。
    pub(crate) fn render_address_bar(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        if !self.settings.show_toolbar_folder {
            self.address_has_focus = false;
            return None;
        }

        let enter_pressed = self.dialog_enter_pressed(ctx);
        // 現在表示中フォルダ / ZIP / PDF のコンテナレーティングを取得。
        // 0 のときは非表示、1〜5 のときは★バッジをアドレス欄の右端に表示する。
        let folder_rating = self.current_folder_rating();
        let thumbnail_count = (!self.global_search.active
            && !self.favsearch.active
            && !self.items_are_global_search_view
            && self.search_filter.is_none()
            && self.search_pending.is_none())
        .then(|| thumbnail_count_label(&self.items, &self.visible_indices));
        // 📌 (代表サムネ固定) ボタンの表示判定 + 状態をあらかじめ計算する。
        // closure 内で `self` のミュータブル借用が衝突しないように外で確定しておく。
        let pin_button_info = self.compute_folder_pin_button_state();
        egui::TopBottomPanel::top("address_bar")
            .show(ctx, |ui| -> Option<PathBuf> {
                ui.add_space(3.0);
                let mut result = None;
                let mut pin_click = PinButtonClick::None;
                ui.horizontal(|ui| {
                    ui.label("フォルダ:");
                    // ★バッジは右寄せで先に配置し、残り幅を TextEdit が埋める。
                    // right_to_left レイアウトで ★ → TextEdit の順に追加すると、
                    // TextEdit は available width いっぱいに広がる。
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if folder_rating >= 1 && folder_rating <= 5 {
                            let stars = "★".repeat(folder_rating as usize);
                            ui.label(
                                egui::RichText::new(format!("📁{stars}"))
                                    .color(egui::Color32::from_rgb(130, 170, 220))
                                    .strong(),
                            )
                            .on_hover_text(
                                "このフォルダ / ZIP / PDF のレーティング [Shift+F1〜F6]",
                            );
                            ui.add_space(4.0);
                        }
                        // 📌 (代表サムネ固定): right_to_left なので 📁★ より左 (= 入力欄寄り) に置く。
                        if let Some(info) = pin_button_info.as_ref() {
                            let label = if info.matches_current_pin {
                                egui::RichText::new("📌")
                                    .color(egui::Color32::from_rgb(230, 180, 90))
                                    .strong()
                            } else {
                                egui::RichText::new("📌")
                            };
                            let btn = egui::Button::new(label).frame(false);
                            let resp = ui.add_enabled(info.enabled, btn);
                            let resp = resp.on_hover_text(info.tooltip.as_str());
                            if info.enabled {
                                if resp.clicked() {
                                    pin_click = PinButtonClick::Toggle;
                                } else if resp.secondary_clicked() {
                                    pin_click = PinButtonClick::Remove;
                                }
                            }
                            ui.add_space(4.0);
                        }
                        if let Some(count) = thumbnail_count.as_ref() {
                            ui.label(
                                egui::RichText::new(count.as_str())
                                    .size(11.0)
                                    .monospace()
                                    .color(egui::Color32::from_gray(140)),
                            )
                            .on_hover_text("表示中のサムネイル数 / 全サムネイル数");
                            ui.add_space(4.0);
                        }
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut self.address)
                                    .desired_width(f32::INFINITY),
                            );
                            self.address_has_focus = resp.has_focus();
                            if resp.lost_focus() && enter_pressed {
                                let p = PathBuf::from(&self.address);
                                if let Some(resolved) =
                                    crate::folder_tree::resolve_openable_path(&p)
                                {
                                    result = Some(resolved);
                                }
                            }
                        });
                    });
                });
                ui.add_space(3.0);
                // pin ボタンクリックは closure 抜けてから処理する (App ミュータブル借用が必要)
                match pin_click {
                    PinButtonClick::Toggle => self.toggle_folder_pin_from_selection(),
                    PinButtonClick::Remove => self.remove_folder_pin_for_current_container(),
                    PinButtonClick::None => {}
                }
                result
            })
            .inner
    }

    // ── 検索バー ─────────────────────────────────────────────────────

    /// メタデータ検索バーを描画する。
    pub(crate) fn render_search_bar(&mut self, ctx: &egui::Context) {
        if !self.show_search_bar {
            return;
        }

        // Enter は **raw** で読む。`dialog_enter_pressed` は IME 変換直後 300ms の
        // グレース中も false を返すため、日本語で「おはよう[Enter]」と確定兼送信した
        // ケースで検索が走らず、代わりにグリッドの Enter ショートカット (フルスクリーン)
        // が走ってしまう。ここでは `response.lost_focus()` が Tab / クリック外しでも
        // true になる性質を raw Enter との AND で打ち消し、"Enter でフォーカスを失った"
        // ときだけ execute_search を呼ぶ。search_query は IME Commit で既に確定済み。
        let raw_enter_pressed = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed = self.dialog_escape_pressed(ctx);
        egui::TopBottomPanel::top("search_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label("検索:");
                let response = ui.add_sized(
                    [320.0, 20.0],
                    egui::TextEdit::singleline(&mut self.search_query)
                        .hint_text(r#"このフォルダ内のメタデータ (AND / -除外 / "…")"#),
                );

                // フォーカスリクエスト
                if self.search_focus_request {
                    self.search_focus_request = false;
                    response.request_focus();
                }

                // フォーカス状態を追跡
                self.search_has_focus = response.has_focus();

                // Enter で検索実行 (IME 変換確定の Enter も同じ扱い)
                if response.lost_focus() && raw_enter_pressed {
                    self.execute_search();
                    // フォーカスを外してカーソルキーでグリッド操作できるようにする
                    response.surrender_focus();
                    self.search_has_focus = false;
                }

                // × ボタン
                if ui.small_button("×").on_hover_text("検索を閉じる").clicked() {
                    self.cancel_pending_folder_nav();
                    self.show_search_bar = false;
                    self.search_query.clear();
                    self.search_filter = None;
                    self.search_has_focus = false;
                    self.cancel_search_pending();
                    self.rebuild_visible_indices();
                }

                // ── 検索対象ドロップダウン (§19.7) ──
                let current =
                    crate::global_search_ui::TargetChoice::from_target(&self.search_target);
                let mut next = current;
                egui::ComboBox::from_id_salt("ctrl_f_search_target")
                    .selected_text(current.label())
                    .width(160.0)
                    .show_ui(ui, |ui| {
                        for &choice in crate::global_search_ui::TARGET_CHOICES {
                            ui.selectable_value(&mut next, choice, choice.label());
                        }
                    });
                if next != current {
                    self.search_target = next.to_target();
                    // クエリが空でなければ即再検索
                    if !self.search_query.trim().is_empty() {
                        self.execute_search();
                    }
                }

                if crate::ui_helpers::or_mode_checkbox(ui, &mut self.search_or_mode)
                    && !self.search_query.trim().is_empty()
                {
                    self.execute_search();
                }

                // Esc で検索解除（ダイアログが開いていない場合のみ。IME 変換中もスキップ）
                if !self.any_dialog_open() && escape_pressed {
                    self.cancel_pending_folder_nav();
                    self.show_search_bar = false;
                    self.search_query.clear();
                    self.search_filter = None;
                    self.search_has_focus = false;
                    self.cancel_search_pending();
                    self.rebuild_visible_indices();
                }

                // 検索中インジケータ or マッチ件数 (separator の後に表示)
                if self.search_pending.is_some() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("検索中...")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(180, 180, 80)),
                    );
                } else if let Some(ref filter) = self.search_filter {
                    ui.separator();
                    let image_count = filter
                        .iter()
                        .filter(|&&i| {
                            matches!(
                                self.items.get(i),
                                Some(crate::grid_item::GridItem::Image(_))
                            )
                        })
                        .count();
                    let total_images = self
                        .items
                        .iter()
                        .filter(|it| matches!(it, crate::grid_item::GridItem::Image(_)))
                        .count();
                    ui.label(
                        egui::RichText::new(format!("{image_count}/{total_images} 件"))
                            .size(11.0)
                            .color(egui::Color32::from_gray(140)),
                    );
                }
            });
            ui.add_space(2.0);
        });
    }

    /// お気に入り検索バー (ツールバー直下の 2 行目) を描画する。
    /// `favsearch.active` が true のときだけ表示される。
    pub(crate) fn render_favsearch_bar(&mut self, ctx: &egui::Context) {
        if !self.favsearch.active {
            return;
        }
        // Ctrl+F 側と同じく raw Enter で判定する (IME 変換確定の Enter も送信扱い)。
        // `response.lost_focus()` と AND することで Tab / クリック外しの誤発火は弾ける。
        let raw_enter_pressed = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed = self.dialog_escape_pressed(ctx);

        let mut close_requested = false;
        let mut query_changed = false;

        egui::TopBottomPanel::top("favsearch_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label("検索:");
                let response = ui.add_sized(
                    [320.0, 20.0],
                    egui::TextEdit::singleline(&mut self.favsearch.query).hint_text(
                        r#"お気に入り配下のフォルダ/ZIP/PDF/動画名 (AND / -除外 / "…")"#,
                    ),
                );

                if self.favsearch.focus_request {
                    self.favsearch.focus_request = false;
                    response.request_focus();
                }
                self.favsearch.has_focus = response.has_focus();

                // 入力が変わるたびに即座に検索を再実行 (小規模 DB 前提)
                if response.changed() {
                    query_changed = true;
                }
                // Enter で確定的に再実行 (IME 変換確定の Enter も同じ扱い)
                if response.lost_focus() && raw_enter_pressed {
                    query_changed = true;
                }

                if ui.small_button("×").on_hover_text("検索を閉じる").clicked() {
                    close_requested = true;
                }

                // ── お気に入り絞り込みドロップダウン (§19.7) ──
                // `auto_index_structure=true` のお気に入りのみ候補に出す (名前索引の対象と一致)。
                {
                    let current = self.favsearch.favorite_filter;
                    let label_for = |opt: Option<uuid::Uuid>| -> String {
                        match opt {
                            None => "すべて".to_string(),
                            Some(id) => self
                                .settings
                                .favorite_by_id(id)
                                .map(|f| f.name.clone())
                                .unwrap_or_else(|| "(削除済)".to_string()),
                        }
                    };
                    let mut next = current;
                    egui::ComboBox::from_id_salt("favsearch_fav")
                        .selected_text(label_for(current))
                        .width(140.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut next, None, "すべて");
                            for fav in &self.settings.favorites {
                                if !fav.auto_index_structure {
                                    continue;
                                }
                                ui.selectable_value(&mut next, Some(fav.id), &fav.name);
                            }
                        });
                    if next != current {
                        self.favsearch.favorite_filter = next;
                        // ドロップダウン変更は即再実行。クエリが空なら execute_favsearch が早期 return する。
                        query_changed = true;
                        // last_executed と現 query が一致していても再実行するよう last_executed を空に倒す。
                        self.favsearch.last_executed.clear();
                    }
                }

                if crate::ui_helpers::or_mode_checkbox(ui, &mut self.favsearch.or_mode) {
                    query_changed = true;
                    self.favsearch.last_executed.clear();
                }

                if self.favsearch_pending.is_some() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new("検索中...")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(180, 180, 80)),
                    );
                } else if self.favsearch.on_results_grid() {
                    ui.separator();
                    ui.label(
                        egui::RichText::new(format!("{} 件", self.favsearch.results_paths.len()))
                            .size(11.0)
                            .color(egui::Color32::from_gray(140)),
                    );
                }
            });
            ui.add_space(2.0);
        });

        // Esc で閉じる (IME 変換中はスキップ、他のダイアログが開いていないときのみ)。
        // テキストボックスが Esc で focus を失ってから本チェックに達するため、
        // has_focus は要求せず active のみで判定する (Ctrl+F の検索バーと同じ挙動)。
        if !self.any_dialog_open() && escape_pressed {
            close_requested = true;
        }

        if close_requested {
            self.close_favsearch();
            return;
        }
        if query_changed && self.favsearch.query != self.favsearch.last_executed {
            self.execute_favsearch();
        }
    }

    // ── セルインタラクション ─────────────────────────────────────────

    /// グリッドセルのクリック・ダブルクリック・右クリックを処理する。
    /// ダブルクリックでフォルダに入る場合はそのパスを返す。
    fn handle_cell_interaction(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        cell_rect: egui::Rect,
        idx: usize,
    ) -> Option<PathBuf> {
        let response = ui.interact(cell_rect, ui.id().with(idx), egui::Sense::click());
        let mut nav = None;
        if response.clicked() {
            let ctrl = ctx.input(|i| i.modifiers.ctrl);
            let shift = ctx.input(|i| i.modifiers.shift);
            if shift {
                // Shift+クリック: 前回選択位置から現在位置までを範囲チェック
                if let Some(prev_sel) = self.selected {
                    let vi = &self.visible_indices;
                    let prev_pos = vi.iter().position(|&i| i == prev_sel).unwrap_or(0);
                    let cur_pos = vi.iter().position(|&i| i == idx).unwrap_or(0);
                    let (start, end) = if prev_pos <= cur_pos {
                        (prev_pos, cur_pos)
                    } else {
                        (cur_pos, prev_pos)
                    };
                    for vp in start..=end {
                        if let Some(&vidx) = vi.get(vp) {
                            if self.items.get(vidx).is_some_and(|it| it.is_checkable()) {
                                self.checked.insert(vidx);
                            }
                        }
                    }
                }
            } else if ctrl {
                // Ctrl+クリック: チェック ON/OFF トグル + 選択移動。
                // 初回 Ctrl+クリック (checked が空) のときは直前のカーソル位置も checked に
                // 入れる (エクスプローラ流「A 通常クリック → B Ctrl+クリックで A+B が選択」)。
                if self.checked.is_empty() {
                    if let Some(prev_sel) = self.selected {
                        if prev_sel != idx
                            && self.idx_visible(prev_sel)
                            && self.items.get(prev_sel).is_some_and(|it| it.is_checkable())
                        {
                            self.checked.insert(prev_sel);
                        }
                    }
                }
                if self.items.get(idx).is_some_and(|it| it.is_checkable()) {
                    if self.checked.contains(&idx) {
                        self.checked.remove(&idx);
                    } else {
                        self.checked.insert(idx);
                    }
                }
            }
            self.selected = Some(idx);
            self.update_last_selected_image();
        }
        if response.double_clicked() {
            match self.items.get(idx) {
                Some(GridItem::Folder(p)) => {
                    // Ctrl+G 絞り込みビューでは「ヒットを含む子フォルダ」を Folder として
                    // 並べているので、通常の load_folder ではなく絞り込みをさらに 1 段潜る
                    // 経路に流す (docs §10.3 [3] 絞り込みビュー)。
                    if self.global_search.active
                        && matches!(
                            self.global_search.view,
                            crate::global_search_ui::GlobalSearchView::DrilledInto { .. }
                        )
                    {
                        self.drill_into_subfolder(p.clone());
                    } else {
                        let p = p.clone();
                        self.maybe_suppress_rating_filter_for_opened_container(idx);
                        nav = Some(p);
                    }
                }
                Some(GridItem::ZipFile(p)) | Some(GridItem::PdfFile(p)) => {
                    // Folder 分岐とは global_search drill-in 判定が違うためここは別のまま。
                    let p = p.clone();
                    self.maybe_suppress_rating_filter_for_opened_container(idx);
                    nav = Some(p);
                }
                Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::ZipSeparator { .. })
                | Some(GridItem::PdfPage { .. })
                | Some(GridItem::Video(_)) => {
                    // 動画も画像と同じくフルスクリーン化 → VideoPlayer がインライン再生する。
                    // 外部プレイヤーで開きたい場合はフルスクリーン中の Shift+Enter または
                    // 右クリックメニューから (近日対応予定)。
                    // Phase 7.J: グリッドから明示的に開いたケースなので、
                    // 「一覧から開いたときだけ再生する」設定でも再生開始する。
                    self.bump_input_seq_for_item("grid_double_click", idx);
                    if matches!(self.items.get(idx), Some(GridItem::Video(_))) {
                        // Prevent the second click of the grid double-click from
                        // reaching the newly-opened fullscreen video and toggling
                        // playback back to paused.
                        self.fs_suppress_primary_until_release = true;
                        self.fs_focus_regained_at = Some(std::time::Instant::now());
                    }
                    self.fs_open_intent_from_grid = true;
                    self.open_fullscreen(idx);
                }
                Some(GridItem::ConvertibleArchive { path, format }) => {
                    let pf = path.clone();
                    let fmt = *format;
                    self.maybe_suppress_rating_filter_for_opened_container(idx);
                    if let Some(cached) = self.try_archive_cache_lookup(&pf) {
                        self.open_archive_via_cache(pf, cached);
                    } else {
                        self.request_archive_convert(pf, fmt);
                    }
                }
                Some(GridItem::SearchContainer { path, kind, .. }) => {
                    // Ctrl+G 結果ビューのコンテナ: ダブルクリックで drill-down view に遷移
                    // (docs §10.3 [3] 絞り込みビュー)
                    let p = path.clone();
                    let is_zip = matches!(kind, crate::grid_item::SearchContainerKind::Zip);
                    // ★コンテナを開いた時の中身空表示対策 (Codex P2)
                    self.maybe_suppress_rating_filter_for_opened_container_path(&p);
                    self.drill_into_container(p, is_zip);
                }
                None => {}
            }
        }
        // 右クリック → コンテキストメニュー
        if response.secondary_clicked() {
            self.selected = Some(idx);
            self.update_last_selected_image();
            self.context_menu_idx = Some(idx);
            self.context_menu_pos = ctx.input(|i| i.pointer.interact_pos().unwrap_or_default());
        }
        nav
    }

    // ── サムネイルグリッド ───────────────────────────────────────────

    /// サムネイルグリッドを描画し、フォルダナビゲーション先を返す。
    pub(crate) fn render_grid(&mut self, ctx: &egui::Context) -> Option<PathBuf> {
        let scroll_to = self.scroll_to_selected;
        self.scroll_to_selected = false;

        egui::CentralPanel::default()
            .show(ctx, |ui| -> Option<PathBuf> {
                let global_searching =
                    self.items_are_global_search_view && self.global_search.is_searching();
                if self.items.is_empty() {
                    // ZIP / PDF 非同期列挙中は「読み込み中…」にして待ち状態を明示する。
                    // BS や Ctrl+↑↓ はこの間でも受理され、load_folder 側で pending が
                    // Drop されて worker が cancel する。
                    let loading = self.zip_enumerate_pending.is_some()
                        || self.pdf_enumerate_pending.is_some();
                    let msg = if global_searching {
                        "検索中"
                    } else if loading {
                        "読み込み中…"
                    } else if self.current_folder.is_some() {
                        "表示するファイルがありません"
                    } else {
                        "フォルダを入力して Enter キーを押してください"
                    };
                    let r = ui.centered_and_justified(|ui| ui.label(msg));
                    // 空フォルダでも右クリックでフォルダ操作可能にする
                    if r.inner.secondary_clicked() {
                        if self.current_folder.is_some() {
                            self.context_menu_idx = Some(usize::MAX); // 特殊値: フォルダ操作
                            self.context_menu_pos =
                                ctx.input(|i| i.pointer.interact_pos().unwrap_or_default());
                        }
                    }
                    return None;
                }

                if self.visible_indices.is_empty() {
                    ui.centered_and_justified(|ui| {
                        ui.label(if global_searching {
                            "検索中"
                        } else {
                            "検索結果なし"
                        });
                    });
                    return None;
                }

                let cols = self.settings.grid_cols.max(1);
                let avail_w = ui.available_width();
                let height_ratio = self.settings.thumb_aspect.height_ratio();
                let Some((cell_w, cell_h)) = compute_cell_size(avail_w, cols, height_ratio) else {
                    return None;
                };

                // ウィンドウリサイズやアスペクト比変更でセルサイズが変わった場合スナップし直す
                if (cell_w - self.last_cell_size).abs() > 0.5
                    || (cell_h - self.last_cell_h).abs() > 0.5
                {
                    self.scroll_offset_y = (self.scroll_offset_y / cell_h).round() * cell_h;
                    self.last_cell_size = cell_w;
                    self.last_cell_h = cell_h;
                }

                if scroll_to {
                    self.apply_scroll_to_selected(cols, cell_h);
                }

                let total_rows = self.visible_indices.len().div_ceil(cols);
                let natural_h = total_rows as f32 * cell_h;

                // egui 内部の max offset = total_h - viewport_h が行境界に揃うよう、
                // total_h を拡張する。これにより egui と自前の行スナップが一致し振動を防ぐ。
                // 拡張量は最大 cell_h 未満（端数の補正のみ）。
                let total_h = if natural_h <= self.last_viewport_h {
                    natural_h
                } else {
                    let raw_max = natural_h - self.last_viewport_h;
                    let snapped_max = (raw_max / cell_h).ceil() * cell_h;
                    snapped_max + self.last_viewport_h
                };

                let max_offset = if total_h <= self.last_viewport_h {
                    0.0
                } else {
                    total_h - self.last_viewport_h
                };
                self.scroll_offset_y = self.scroll_offset_y.clamp(0.0, max_offset);

                let mut nav: Option<PathBuf> = None;

                // egui にスクロールを管理させず、自前の offset を毎フレーム注入する。
                // ただしスクロールバードラッグ時は egui 側のオフセットを読み戻す。
                let scroll_output = egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .vertical_scroll_offset(self.scroll_offset_y)
                    .show_viewport(ui, |ui, viewport| {
                        // ビューポート高さを記録（次フレームのスクロール計算に使う）
                        self.last_viewport_h = viewport.height();

                        let (content_rect, _) = ui.allocate_exact_size(
                            egui::vec2(avail_w, total_h),
                            egui::Sense::hover(),
                        );

                        let first_row = (viewport.min.y / cell_h) as usize;
                        let last_row = ((viewport.max.y / cell_h) as usize + 2).min(total_rows);

                        // Phase 2b ワーカーへ現在の可視先頭/終端アイテムを通知
                        let vis_first_idx = self
                            .visible_indices
                            .get(first_row * cols)
                            .copied()
                            .unwrap_or(0);
                        self.scroll_hint.store(vis_first_idx, Ordering::Relaxed);
                        // 可視範囲の終端 (exclusive)。先読みの forward 側距離計算に使う。
                        // last_row は exclusive、可視セルの最後の位置は (last_row*cols - 1) だが
                        // 末尾の行は半分しか埋まっていない場合があるので visible_indices.len() で clamp。
                        let last_pos_inclusive = (last_row * cols)
                            .saturating_sub(1)
                            .min(self.visible_indices.len().saturating_sub(1));
                        let vis_end_idx = self
                            .visible_indices
                            .get(last_pos_inclusive)
                            .copied()
                            .map(|i| i + 1)
                            .unwrap_or(vis_first_idx);
                        self.visible_end_shared
                            .store(vis_end_idx, Ordering::Relaxed);

                        for row in first_row..last_row {
                            for col in 0..cols {
                                let vis_pos = row * cols + col;
                                if vis_pos >= self.visible_indices.len() {
                                    break;
                                }
                                let idx = self.visible_indices[vis_pos];

                                let cell_rect = egui::Rect::from_min_size(
                                    content_rect.min
                                        + egui::vec2(col as f32 * cell_w, row as f32 * cell_h),
                                    egui::vec2(cell_w, cell_h),
                                );

                                if let Some(n) =
                                    self.handle_cell_interaction(ui, ctx, cell_rect, idx)
                                {
                                    nav = Some(n);
                                }
                                // handle_cell_interaction 内で同期的に items が差し替わる
                                // 経路がある (SearchContainer ダブルクリック →
                                // drill_into_container、Ctrl+G 絞り込み中の Folder
                                // ダブルクリック → drill_into_subfolder)。以降の
                                // self.items[idx] / self.thumbnails[idx] は stale idx で
                                // out-of-bounds panic するので、境界を再チェックして
                                // 残りの列/行を抜ける (panic.log の ui_main.rs:1026
                                // "len is 0 but index is 102" を回避)。
                                if idx >= self.items.len() || idx >= self.thumbnails.len() {
                                    break;
                                }

                                let rot = self.get_rotation(idx);
                                let has_page_override =
                                    self.adjustment_page_params.contains_key(&idx);
                                let has_mask = self.mask_pages.contains(&idx);
                                let rating = self.get_rating(idx);
                                // 可視セルは同期適用 (~3ms/枚)。先読み分は背後の
                                // process_thumb_adjust_budget が逐次処理する。
                                // ドラッグ中は両経路ともスキップして生サムネ表示に戻す
                                // (70 枚毎フレーム再生成は ~200ms のフリーズになるため)。
                                if !self.adjustment_dragging {
                                    self.maybe_apply_thumb_adjustment(ctx, idx);
                                }
                                let adjusted_tex = if self.adjustment_dragging {
                                    None
                                } else {
                                    self.thumb_adjust_tex.get(&idx)
                                };
                                let tags = self.cell_tag_list(idx).to_vec();
                                let filter_match = self.folder_rating_match(idx);
                                let filter_match_count = filter_match.map(|(c, _)| c);
                                // 📌 バッジ (金色) — ユーザーが Pin 操作した対象アイテムの
                                // 目印。「現在表示中のコンテナの pin source = この item」
                                // (= ユーザーがこのアイテムを選択して P / 📌 を押した) のとき
                                // のみ出す。
                                //
                                // **「コンテナ自身が pin 済み」の表示は出さない** (= ユーザーから
                                // 「pin で表示されているサムネ」と「auto-pick で選ばれたサムネ」を
                                // 区別させないことで、「badge = 自分が Pin 操作した対象」を 1 対 1
                                // で対応させる)。
                                let has_pin = if let Some(cur) = self.current_folder.as_ref() {
                                    let cur_key = crate::path_key::normalize_keep_drive(cur);
                                    self.folder_pin_map.get(&cur_key).is_some_and(|pin_src| {
                                        crate::folder_thumb_pins::source_from_grid_item(
                                            cur,
                                            &self.items[idx],
                                        )
                                        .as_ref()
                                            == Some(pin_src)
                                    })
                                } else {
                                    false
                                };
                                crate::app::draw_cell(
                                    ui,
                                    cell_rect,
                                    self.selected == Some(idx),
                                    self.checked.contains(&idx),
                                    has_page_override,
                                    has_mask,
                                    rating,
                                    &self.items[idx],
                                    &self.thumbnails[idx],
                                    rot,
                                    adjusted_tex,
                                    &tags,
                                    filter_match_count,
                                    has_pin,
                                );
                                // 小さい右下バッジに限らずセル全体をホバー領域にして
                                // ★内訳 tooltip を出す。
                                if let Some((_total, per_star)) = filter_match {
                                    let hover_id = egui::Id::new(("folder_rating_badge", idx));
                                    let resp =
                                        ui.interact(cell_rect, hover_id, egui::Sense::hover());
                                    if resp.hovered() {
                                        resp.on_hover_ui_at_pointer(|ui| {
                                            ui.vertical(|ui| {
                                                ui.label("マッチする子孫ファイル");
                                                for s in (1..=5usize).rev() {
                                                    let c = per_star[s - 1];
                                                    if c > 0 {
                                                        ui.label(format!(
                                                            "{} : {} 件",
                                                            "★".repeat(s),
                                                            c
                                                        ));
                                                    }
                                                }
                                            });
                                        });
                                    }
                                }

                                // 選択中セルの矩形を記録 (オーバーレイ配置用)
                                if self.selected == Some(idx) {
                                    self.selected_cell_rect = Some(cell_rect);
                                }
                            }
                        }

                        // グリッドの空白部分で右クリック → フォルダメニュー。
                        // Sense::click() だと左クリックも消費するので、ポインタ位置を
                        // 直接チェックする。`ctx.input` はグローバルなので、ツールバーの
                        // ★フィルタボタン等への右クリックまで拾ってグリッドの右クリック
                        // メニューが同時に開いてしまう不具合があった。`ui_contains_pointer`
                        // で CentralPanel 範囲内に限定する。
                        let bg_right_clicked = ui.ui_contains_pointer()
                            && ctx.input(|i| i.pointer.secondary_clicked());
                        if bg_right_clicked && self.context_menu_idx.is_none() {
                            if self.current_folder.is_some() {
                                self.context_menu_idx = Some(usize::MAX);
                                self.context_menu_pos =
                                    ctx.input(|i| i.pointer.interact_pos().unwrap_or_default());
                            }
                        }
                    });

                // スクロールバードラッグによるオフセット変化を読み戻す。
                // egui が内部で管理するオフセットと自前オフセットを同期させる。
                // ただし行スナップによる端数差分で毎フレーム振動するのを防ぐため、
                // 1 行分 (cell_h) 以上ずれた場合のみ同期する。
                let egui_offset = scroll_output.state.offset.y;
                if (egui_offset - self.scroll_offset_y).abs() > cell_h * 0.5 {
                    self.scroll_offset_y = (egui_offset / cell_h).round() * cell_h;
                }

                // 右上フィードバックトースト (Q / Ctrl+Backspace / F7/F8 / レーティング等)
                // show_feedback_toast でセットされたテキストをグリッド画面でも描画する。
                // フルスクリーン側は render_fullscreen_viewport が別途呼ぶ。
                let full_rect = ui.max_rect();
                self.draw_feedback_toast(ui, full_rect, ctx);

                nav
            })
            .inner
    }

    // ── 選択情報オーバーレイ ─────────────────────────────────────────

    /// 選択中アイテムの情報をセル直下に表示する。
    pub(crate) fn render_selection_info(&self, ctx: &egui::Context) {
        // フルスクリーン中は出さない (独自のホバーヘッダーを持つため)。
        if self.fullscreen_idx.is_some() {
            return;
        }

        let (Some(idx), Some(cell_rect)) = (self.selected, self.selected_cell_rect) else {
            return;
        };

        // ZipSeparator はスキップ
        if matches!(
            self.items.get(idx),
            Some(GridItem::ZipSeparator { .. }) | None
        ) {
            return;
        }

        let name = self
            .items
            .get(idx)
            .map(|it| it.name().to_string())
            .unwrap_or_default();
        // 元画像のピクセル寸法 (ThumbnailState::Loaded.source_dims から取得)
        let dims_str = match self.thumbnails.get(idx) {
            Some(ThumbnailState::Loaded {
                source_dims: Some((w, h)),
                ..
            }) => Some(format!("{} × {}", w, h)),
            _ => None,
        };
        let text = match dims_str {
            Some(d) => format!("{}   {}", d, name),
            None => name,
        };

        // セル幅で配置: セルの左下を基点、セル幅に合わせる
        let cell_w = cell_rect.width();
        let area_pos = cell_rect.left_bottom() + egui::vec2(0.0, 4.0);

        egui::Area::new("selection_info".into())
            .order(egui::Order::Middle)
            .fixed_pos(area_pos)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 25, 35, 230))
                    .show(ui, |ui| {
                        let inner_width = (cell_w - 12.0).max(40.0);
                        ui.set_min_width(inner_width);
                        ui.set_max_width(inner_width);
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(text)
                                    .color(egui::Color32::WHITE)
                                    .monospace(),
                            )
                            .truncate(),
                        );
                    });
            });
    }
}

#[cfg(test)]
mod rating_filter_op_tests {
    use super::*;

    #[test]
    fn thumbnail_count_label_pads_visible_to_total_digits() {
        let items: Vec<GridItem> = (0..100)
            .map(|i| GridItem::Image(PathBuf::from(format!("img_{i}.jpg"))))
            .collect();
        let visible_indices: Vec<usize> = (0..20).collect();

        assert_eq!(thumbnail_count_label(&items, &visible_indices), "( 20/100)");
    }

    #[test]
    fn thumbnail_count_label_ignores_zip_separators() {
        let items = vec![
            GridItem::Image(PathBuf::from("a.jpg")),
            GridItem::ZipSeparator {
                dir_display: "chapter".to_string(),
            },
            GridItem::Image(PathBuf::from("b.jpg")),
        ];
        let visible_indices = vec![0, 1];

        assert_eq!(thumbnail_count_label(&items, &visible_indices), "(1/2)");
    }

    #[test]
    fn is_solo_detects_single_on_bucket() {
        let mut rf = [false; 6];
        rf[3] = true;
        assert!(is_rating_solo(&rf, 3));
        assert!(!is_rating_solo(&rf, 2));
        // 全 ON は solo ではない
        assert!(!is_rating_solo(&[true; 6], 3));
        // 全 OFF も solo ではない
        assert!(!is_rating_solo(&[false; 6], 3));
    }

    #[test]
    fn is_threshold_detects_idx_and_above() {
        let rf = [false, false, false, true, true, true];
        assert!(is_rating_threshold(&rf, 3));
        assert!(!is_rating_threshold(&rf, 2));
        assert!(!is_rating_threshold(&rf, 4));
        // idx=0 のとき threshold は全 ON と等価
        assert!(is_rating_threshold(&[true; 6], 0));
        assert!(!is_rating_threshold(&[false; 6], 0));
    }

    #[test]
    fn apply_toggle_flips_single_bucket() {
        let mut rf = [true; 6];
        apply_rating_filter_op(&mut rf, RatingFilterOp::Toggle, 2);
        assert_eq!(rf, [true, true, false, true, true, true]);
        apply_rating_filter_op(&mut rf, RatingFilterOp::Toggle, 2);
        assert_eq!(rf, [true; 6]);
    }

    #[test]
    fn apply_solo_sets_only_target_on() {
        let mut rf = [true; 6];
        apply_rating_filter_op(&mut rf, RatingFilterOp::Solo, 3);
        assert_eq!(rf, [false, false, false, true, false, false]);
    }

    #[test]
    fn apply_threshold_sets_idx_and_above() {
        let mut rf = [false; 6];
        apply_rating_filter_op(&mut rf, RatingFilterOp::Threshold, 3);
        assert_eq!(rf, [false, false, false, true, true, true]);
        // idx=0 は全 ON と等価
        apply_rating_filter_op(&mut rf, RatingFilterOp::Threshold, 0);
        assert_eq!(rf, [true; 6]);
    }

    #[test]
    fn apply_all_on_matches_default() {
        let mut rf = [false; 6];
        apply_rating_filter_op(&mut rf, RatingFilterOp::AllOn, 0);
        assert_eq!(rf, crate::settings::default_rating_filter());
    }

    /// Ctrl+Shift+★N の挙動: ★N と「なし」だけ ON、残りはすべて OFF。
    /// 未評価コンテナ (= フォルダ / 未評価 ZIP / 未評価 PDF) と未評価画像の両方が残るので
    /// ★N 画像をフォルダツリーから探す用途向け。
    #[test]
    fn apply_solo_with_unrated_keeps_none_bucket_on() {
        let mut rf = [true; 6];
        apply_rating_filter_op(&mut rf, RatingFilterOp::SoloWithUnrated, 5);
        assert_eq!(rf, [true, false, false, false, false, true]);
    }

    /// `is_rating_solo_with_unrated` は Ctrl+Shift 状態の検出用。
    /// トグル 2 回目の Ctrl+Shift クリックで AllOn に戻るための述語。
    #[test]
    fn is_solo_with_unrated_detects_none_plus_target() {
        let rf = [true, false, false, false, false, true];
        assert!(is_rating_solo_with_unrated(&rf, 5));
        assert!(!is_rating_solo_with_unrated(&rf, 4));
        // idx=0 は定義外 (常に false)
        assert!(!is_rating_solo_with_unrated(&rf, 0));
        // なし が OFF なら false
        let rf_no_none = [false, false, false, false, false, true];
        assert!(!is_rating_solo_with_unrated(&rf_no_none, 5));
        // 2 星バケツ以上 ON も false
        let rf_two_stars = [true, false, false, true, false, true];
        assert!(!is_rating_solo_with_unrated(&rf_two_stars, 5));
    }

    /// Ctrl+Shift+クリック は solo_with_unrated ↔ 全 ON を往復する。
    #[test]
    fn ctrl_shift_click_model_toggles_with_unrated() {
        let mut rf = [true; 6];
        // 初回 Ctrl+Shift+★5 → ★5 + なし だけ
        let op = if is_rating_solo_with_unrated(&rf, 5) {
            RatingFilterOp::AllOn
        } else {
            RatingFilterOp::SoloWithUnrated
        };
        apply_rating_filter_op(&mut rf, op, 5);
        assert!(is_rating_solo_with_unrated(&rf, 5));
        // 同じボタンを Ctrl+Shift+クリック再度 → 全 ON
        let op = if is_rating_solo_with_unrated(&rf, 5) {
            RatingFilterOp::AllOn
        } else {
            RatingFilterOp::SoloWithUnrated
        };
        apply_rating_filter_op(&mut rf, op, 5);
        assert_eq!(rf, crate::settings::default_rating_filter());
    }

    /// click logic のモデル: Ctrl+click は solo ↔ 全 ON を往復する。
    #[test]
    fn ctrl_click_model_solo_and_restore() {
        let mut rf = [true; 6];
        // 既に全 ON で Ctrl+★3 → solo 状態に
        let op = if is_rating_solo(&rf, 3) {
            RatingFilterOp::AllOn
        } else {
            RatingFilterOp::Solo
        };
        apply_rating_filter_op(&mut rf, op, 3);
        assert!(is_rating_solo(&rf, 3));
        // 同じボタンを Ctrl+クリック再度 → 全 ON
        let op = if is_rating_solo(&rf, 3) {
            RatingFilterOp::AllOn
        } else {
            RatingFilterOp::Solo
        };
        apply_rating_filter_op(&mut rf, op, 3);
        assert_eq!(rf, crate::settings::default_rating_filter());
    }

    /// Shift+click は threshold ↔ 全 ON を往復する。
    #[test]
    fn shift_click_model_threshold_and_restore() {
        let mut rf = [true; 6];
        // 全 ON で Shift+★3 → threshold (idx>=3 のみ ON)
        let op = if is_rating_threshold(&rf, 3) {
            RatingFilterOp::AllOn
        } else {
            RatingFilterOp::Threshold
        };
        apply_rating_filter_op(&mut rf, op, 3);
        assert_eq!(rf, [false, false, false, true, true, true]);
        // 同ボタン再度 → 全 ON
        let op = if is_rating_threshold(&rf, 3) {
            RatingFilterOp::AllOn
        } else {
            RatingFilterOp::Threshold
        };
        apply_rating_filter_op(&mut rf, op, 3);
        assert_eq!(rf, crate::settings::default_rating_filter());
    }
}

#[cfg(test)]
mod compute_cell_size_tests {
    use super::*;

    #[test]
    fn returns_none_for_non_positive_width() {
        assert_eq!(compute_cell_size(0.0, 4, 1.0), None);
        assert_eq!(compute_cell_size(-10.0, 4, 1.0), None);
    }

    #[test]
    fn computes_cell_size_for_normal_window() {
        let (w, h) = compute_cell_size(800.0, 4, 1.0).expect("Some");
        assert_eq!(w, 200.0);
        assert_eq!(h, 200.0);
    }

    #[test]
    fn applies_height_ratio_to_cell_h() {
        let (w, h) = compute_cell_size(800.0, 4, 1.5).expect("Some");
        assert_eq!(w, 200.0);
        assert_eq!(h, 300.0);
    }

    /// **回帰テスト** (主目的): 狭幅 window で cell_w が MIN_CELL_PX (32px) 未満になると、
    /// `viewport_h / cell_h` が数千行に暴発して UI が固まるバグの再発検知。
    #[test]
    fn clamps_cell_w_to_min_when_window_too_narrow() {
        let (w, _) = compute_cell_size(100.0, 10, 1.0).expect("Some");
        assert!(w >= MIN_CELL_PX);
        assert_eq!(w, MIN_CELL_PX);
    }

    #[test]
    fn clamps_cell_h_to_min_when_aspect_ratio_extreme() {
        let (_, h) = compute_cell_size(800.0, 4, 0.05).expect("Some");
        assert_eq!(h, MIN_CELL_PX);
    }

    #[test]
    fn cols_zero_falls_back_to_one() {
        let (w, _) = compute_cell_size(800.0, 0, 1.0).expect("Some");
        assert_eq!(w, 800.0);
    }
}
