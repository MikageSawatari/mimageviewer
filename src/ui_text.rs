//! テキスト注釈 (comic) のフルスクリーン編集モード。
//!
//! 消しゴム ([`crate::ui_erase`]) / 隠蔽 ([`crate::ui_conceal`]) と同系列の「4 つ目の
//! 編集モード」。Ctrl+T で入退場する (D2、モード名「テキスト」)。注釈ロジック自体は
//! egui 非依存の `comic-core` に置き、本モジュールは mIV 側の UI / 入力 / 永続化配線
//! だけを担う。
//!
//! ## Inc 3a (このコミット) のスコープ
//!
//! モード骨格のみ:
//! - `enter_text_mode` / `reset_text_mode`: 見開き → Single ピボット、`comic_docs`
//!   (page_path_key 別の作業セット) のロード、退場時に comic.db + サイドカーへ保存。
//! - `handle_text_keys`: Esc / Ctrl+T で退場 (選択中なら先に選択解除)。
//! - `draw_text_panel`: 最小パネル (タイトル / オブジェクト数 / 閉じる)。
//!
//! 座標逆写像 (D8、回転下の編集)・オブジェクト選択 / 移動・種別ごとの編集 UI・IME 安全な
//! テキスト入力は後続インクリメント (Inc 3b 以降 / Inc 4)。
//!
//! ## 作業セットと表示の共有
//!
//! 編集中は `comic_docs[key]` を直接 in-place 編集する。表示パイプライン
//! (`ensure_comic_composite_texture`) も同じ `comic_docs` を読むので、編集が即座に
//! 最前面オーバーレイに反映される (Inc 3b で編集時に `comic_generation` を bump して
//! 再ベイクさせる)。退場時に `save_comic_objects` で comic.db + サイドカーへ確定保存する。

use crate::app::App;
use crate::ui_fullscreen::{FsKeyAction, SpreadPair};

/// パネル幅 (conceal と同じ流儀)。
const PANEL_W: f32 = 220.0;
const PANEL_MARGIN_X: f32 = 16.0;
const PANEL_MARGIN_Y: f32 = 60.0;

impl App {
    // ── モード入退場 ────────────────────────────────────────────────

    /// テキスト編集モードに入る。見開き中は左ページへ Single ピボットする
    /// (消しゴム / 隠蔽の enter と同じ作法)。`comic_docs` に作業セットをロードする。
    pub(crate) fn enter_text_mode(&mut self, fs_idx: usize) {
        // 見開き → Single ピボット
        let spread_pair = match self.resolve_spread_pair(fs_idx) {
            SpreadPair::Double { left, right } => Some((left, right)),
            SpreadPair::Single => None,
        };
        let target_idx = spread_pair.map(|(l, _)| l).unwrap_or(fs_idx);

        // page_path_key を持たないアイテム (Folder / Video 等) では入らない。
        let Some(key) = self.page_path_key(target_idx) else {
            return;
        };

        if let Some(pair) = spread_pair {
            self.text_spread_ctx = Some(crate::app::EraseSpreadCtx {
                saved_mode: self.spread_mode,
                pair,
            });
            self.spread_mode = crate::settings::SpreadMode::Single;
            self.fullscreen_idx = Some(target_idx);
            self.fs_zoom = 1.0;
            self.fs_pan = egui::Vec2::ZERO;
        }

        self.text_mode = true;
        self.text_selected = None;
        self.clear_meta_undo();
        // 作業セット = comic.db の注釈をメモリへロード (表示と共有)。
        self.ensure_comic_doc_loaded(&key);

        let obj_count = self.comic_docs.get(&key).map(Vec::len).unwrap_or(0);
        crate::logger::log(format!("text: enter mode, key={key}, objects={obj_count}"));
    }

    /// テキスト編集モードを抜ける。作業セットを comic.db + サイドカーへ保存してから
    /// 状態をクリアし、見開きから入っていた場合は spread を復元する。
    pub(crate) fn reset_text_mode(&mut self) {
        let restore_idx = self.fullscreen_idx;
        let was_text_mode = self.text_mode;

        // 保存は state mutation の前 (page_path_key は fullscreen_idx を要するため)。
        if was_text_mode {
            if let Some(idx) = restore_idx {
                if let Some(key) = self.page_path_key(idx) {
                    let objs = self.comic_docs.get(&key).cloned().unwrap_or_default();
                    self.save_comic_objects(idx, &key, &objs);
                }
            }
        }

        self.text_mode = false;
        self.text_selected = None;
        if was_text_mode {
            self.clear_meta_undo();
        }

        // 見開きから入っていた場合は spread_mode と表示ページを復元
        if let Some(ctx) = self.text_spread_ctx.take() {
            self.spread_mode = ctx.saved_mode;
            self.fullscreen_idx = Some(ctx.pair.0);
            self.fs_zoom = 1.0;
            self.fs_pan = egui::Vec2::ZERO;
        }
        crate::logger::log("text: reset mode".to_string());
    }

    // ── キー入力 ────────────────────────────────────────────────────

    /// テキストモード中のキー処理。`ui_fullscreen` のキーハンドラ冒頭で
    /// `if self.text_mode { return self.handle_text_keys(...) }` として委譲される。
    ///
    /// Inc 3a: Esc / Ctrl+T で退場 (選択中なら先に選択解除)。テキストフィールドの
    /// IME 安全な Enter/Escape ゲートは編集 UI を入れる Inc 3d で追加する。
    pub(crate) fn handle_text_keys(&mut self, ctx: &egui::Context, _fs_idx: usize) -> FsKeyAction {
        let action = FsKeyAction::default();

        let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        if esc {
            if self.text_selected.is_some() {
                self.text_selected = None;
                return action;
            }
            self.reset_text_mode();
            return action;
        }

        // Ctrl+T: 再押下で退場。
        let ctrl_t = ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::T));
        if ctrl_t {
            self.reset_text_mode();
            return action;
        }

        action
    }

    // ── パネル描画 ──────────────────────────────────────────────────

    /// テキストモードのパネル領域 (クリック吸収判定用)。
    pub(crate) fn text_panel_rect(&self, full_rect: egui::Rect) -> egui::Rect {
        let pos = egui::pos2(
            full_rect.min.x + PANEL_MARGIN_X,
            full_rect.min.y + PANEL_MARGIN_Y,
        );
        egui::Rect::from_min_size(pos, egui::vec2(PANEL_W + 8.0, 180.0))
    }

    /// テキストモードのオーバーレイ描画 (Inc 3a はパネルのみ)。フルスクリーン
    /// ビューポートの描画シーケンスから毎フレーム呼ばれる。
    pub(crate) fn draw_text_overlay(&mut self, ctx: &egui::Context, full_rect: egui::Rect) {
        self.draw_text_panel(ctx, full_rect);
    }

    /// テキストモードの最小パネル。`egui::Area` + `Frame::popup` + クリック吸収 sink。
    /// Inc 3a はタイトル / オブジェクト数 / 閉じるボタンのみ。ツールパレットや
    /// セリフ / 本体 / しっぽ等の編集 UI は Inc 4 で移植する。
    fn draw_text_panel(&mut self, ctx: &egui::Context, full_rect: egui::Rect) {
        if !self.text_mode {
            return;
        }
        // クロージャ内で self を二重借用しないよう、必要な値を先に取り出す。
        let obj_count = self
            .fullscreen_idx
            .and_then(|idx| self.page_path_key(idx))
            .and_then(|k| self.comic_docs.get(&k).map(Vec::len))
            .unwrap_or(0);
        let panel_pos = egui::pos2(
            full_rect.min.x + PANEL_MARGIN_X,
            full_rect.min.y + PANEL_MARGIN_Y,
        );
        let sink_rect = self.text_panel_rect(full_rect);

        let mut close = false;
        egui::Area::new(egui::Id::new("text_panel"))
            .fixed_pos(panel_pos)
            .order(egui::Order::Foreground)
            .interactable(true)
            .show(ctx, |ui| {
                // パネル領域のクリック / ドラッグを吸収して背後のキャンバスへ漏らさない。
                ui.interact(
                    sink_rect,
                    egui::Id::new("text_panel_click_sink"),
                    egui::Sense::click_and_drag(),
                );
                egui::Frame::popup(ui.style())
                    .fill(egui::Color32::from_rgba_unmultiplied(20, 20, 20, 230))
                    .stroke(egui::Stroke::new(
                        1.0,
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 40),
                    ))
                    .corner_radius(6.0)
                    .show(ui, |ui| {
                        ui.set_min_width(PANEL_W);
                        ui.set_max_width(PANEL_W);
                        ui.horizontal(|ui| {
                            ui.strong("テキスト注釈");
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    // × は U+00D7 (multiplication sign、Yu Gothic に存在)。
                                    if ui.button("×").on_hover_text("閉じる (Esc)").clicked() {
                                        close = true;
                                    }
                                },
                            );
                        });
                        ui.separator();
                        ui.label(format!("オブジェクト: {obj_count}"));
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new("Ctrl+T / Esc で終了")
                                .small()
                                .color(egui::Color32::from_gray(170)),
                        );
                    });
            });

        if close {
            self.reset_text_mode();
        }
    }
}
