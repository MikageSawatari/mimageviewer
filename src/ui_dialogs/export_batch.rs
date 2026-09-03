//! グリッド選択の一括エクスポート ダイアログ (Ctrl+E)。
//!
//! 書き出しそのものは [`crate::export_batch`] の worker が行い、合成は製本と同じ
//! [`crate::books::write_composited_page`] を通る。ここは出力先・ファイル名
//! テンプレート・形式・サイズを決めるだけ。

use eframe::egui;

use crate::app::App;

pub struct ExportBatchDialogState {
    /// ダイアログを開いた瞬間に固定した書き出し対象。編集内容のスナップショットを
    /// 含むので、開いている間にページを編集しても押した瞬間の絵が出る。
    pub items: Vec<crate::export_batch::BatchExportItem>,
    /// 選択に含まれていたが書き出せなかった件数 (フォルダ / 動画 / 音声 / 書庫本体)。
    pub skipped: usize,
    pub output_dir_text: String,
    pub template: String,
    pub format: crate::capture::CaptureFormat,
    pub scale: crate::export_dialog::ExportScale,
    pub initial_focus_done: bool,
    pub error: Option<String>,
}

impl ExportBatchDialogState {
    fn output_dir(&self) -> std::path::PathBuf {
        std::path::PathBuf::from(self.output_dir_text.trim())
    }

    /// 先頭 1 件の出力ファイル名。テンプレートを打っている最中の答え合わせに出す。
    fn preview_name(&self) -> Option<String> {
        let item = self.items.first()?;
        Some(format!(
            "{}.{}",
            crate::export_batch::resolve_item_stem(&self.template, item, 1),
            self.format.extension()
        ))
    }
}

impl App {
    /// グリッド選択を一括エクスポートするダイアログを開く。対象が 1 件も無ければ
    /// 開かずに理由を出す。
    pub(crate) fn open_export_batch_dialog(&mut self, ctx: &egui::Context) {
        if self.export_pending.is_some() {
            self.show_feedback_toast("エクスポート中です".to_string());
            return;
        }
        if self.export_batch_dialog.is_some() {
            return;
        }
        if self.grid_selection_indices().is_empty() {
            self.show_feedback_toast("書き出す画像・ページを選択してください".to_string());
            return;
        }
        let (items, skipped) = self.grid_batch_export_items(ctx);
        if items.is_empty() {
            self.show_feedback_toast("選択の中に書き出せる画像・ページがありません".to_string());
            return;
        }
        let output_dir_text = self
            .settings
            .export_batch_directory
            .clone()
            .filter(|path| path.is_dir())
            .or_else(|| self.settings.export_last_directory.clone())
            .or_else(|| self.current_folder.clone())
            .map(|path| path.display().to_string())
            .unwrap_or_default();
        self.export_batch_dialog = Some(ExportBatchDialogState {
            items,
            skipped,
            output_dir_text,
            template: self.settings.export_batch_template.clone(),
            format: self.settings.export_batch_format,
            scale: self.settings.export_batch_scale,
            initial_focus_done: false,
            error: None,
        });
        ctx.request_repaint();
    }

    pub(crate) fn draw_export_batch_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.export_batch_dialog.take() else {
            return;
        };

        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let can_start = !state.items.is_empty()
            && !state.output_dir_text.trim().is_empty()
            && !state.template.trim().is_empty();
        let mut open = true;
        let mut canceled = false;
        let mut start = false;
        let mut pick_folder = false;

        // CLAUDE.md「ダイアログ (egui::Window)」: anchor() はドラッグ不可になるため
        // 必ず default_pos() を使う。
        let default_pos = ctx.content_rect().center() - egui::vec2(230.0, 190.0);
        egui::Window::new("一括エクスポート")
            .collapsible(false)
            .resizable(false)
            .default_pos(default_pos)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(460.0);
                ui.label(format!("{} 件を書き出します", state.items.len()));
                if state.skipped > 0 {
                    ui.small(format!(
                        "対象外の {} 件は除外しました (フォルダ・動画・音声・書庫そのもの)",
                        state.skipped
                    ));
                }

                ui.add_space(6.0);
                ui.label("ファイル名");
                let mut template_output =
                    crate::ime_focus::show_singleline(ui, &mut state.template, None, |edit| {
                        edit.desired_width(f32::INFINITY)
                            .hint_text("ファイル名テンプレート")
                    });
                crate::ui_helpers::singleline_text_edit_context_menu(
                    ui,
                    &mut template_output,
                    &mut state.template,
                );
                // 初回フレームのみフォーカスを寄せる。毎フレーム request_focus すると
                // 他フィールドへ移れない (単ページ側と同じ理由)。
                if !state.initial_focus_done {
                    template_output.response.request_focus();
                    state.initial_focus_done = true;
                }
                ui.small(crate::export_batch::TEMPLATE_PLACEHOLDER_HINT);
                if let Some(preview) = state.preview_name() {
                    ui.small(format!("例: {preview}"));
                }

                ui.add_space(6.0);
                ui.label("保存先");
                ui.horizontal(|ui| {
                    let buttons_width = 84.0;
                    let edit_width = (ui.available_width() - buttons_width).max(180.0);
                    let mut output_dir_output = crate::ime_focus::show_singleline(
                        ui,
                        &mut state.output_dir_text,
                        None,
                        |edit| edit.desired_width(edit_width).hint_text("保存先フォルダ"),
                    );
                    crate::ui_helpers::singleline_text_edit_context_menu(
                        ui,
                        &mut output_dir_output,
                        &mut state.output_dir_text,
                    );
                    if ui.button("変更...").clicked() {
                        pick_folder = true;
                    }
                });

                ui.add_space(6.0);
                egui::ComboBox::from_label("形式")
                    .selected_text(state.format.label())
                    .show_ui(ui, |ui| {
                        for format in crate::capture::CaptureFormat::ALL {
                            ui.selectable_value(&mut state.format, format, format.label());
                        }
                    });

                ui.add_space(6.0);
                ui.label("出力サイズ");
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    for scale in crate::export_dialog::ExportScale::FIXED {
                        ui.radio_value(&mut state.scale, scale, scale.label());
                    }
                    // 元のサイズが 1 件ずつ違うので、単ページ側のような実寸表示は出さない。
                    let mut long_edge_px = match state.scale {
                        crate::export_dialog::ExportScale::LongEdge(px) => px,
                        _ => crate::export_dialog::ExportScale::DEFAULT_LONG_EDGE,
                    };
                    let is_long_edge =
                        matches!(state.scale, crate::export_dialog::ExportScale::LongEdge(_));
                    ui.horizontal(|ui| {
                        if ui.radio(is_long_edge, "長辺指定").clicked() {
                            state.scale = crate::export_dialog::ExportScale::LongEdge(long_edge_px);
                        }
                        let resp = ui.add(
                            egui::DragValue::new(&mut long_edge_px)
                                .range(
                                    crate::export_dialog::ExportScale::LONG_EDGE_MIN
                                        ..=crate::export_dialog::ExportScale::LONG_EDGE_MAX,
                                )
                                .suffix("px"),
                        );
                        if resp.changed() {
                            state.scale = crate::export_dialog::ExportScale::LongEdge(long_edge_px);
                        }
                    });
                });

                ui.add_space(6.0);
                ui.small(
                    "選んだ形式で再エンコードします。元の EXIF / AI プロンプトは引き継ぎません。",
                );
                ui.small("同名のファイルは上書きせず、末尾に連番を付けます。");

                if let Some(err) = &state.error {
                    ui.add_space(6.0);
                    ui.colored_label(egui::Color32::from_rgb(255, 140, 140), err);
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(can_start, egui::Button::new("書き出す"))
                        .clicked()
                    {
                        start = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        canceled = true;
                    }
                });
            });

        if pick_folder {
            let start_dir = state.output_dir();
            if let Some(dir) = rfd::FileDialog::new()
                .set_directory(start_dir)
                .pick_folder()
            {
                state.output_dir_text = dir.display().to_string();
            }
        }

        if escape_pressed || !open || canceled {
            self.export_batch_dialog = None;
            return;
        }

        if enter_pressed && can_start {
            start = true;
        }
        if start {
            match self.start_batch_export(&mut state) {
                // 成功時 / 復帰不能時はどちらも start_batch_export がダイアログを畳む。
                Ok(()) => return,
                Err(err) => {
                    state.error = Some(err);
                    self.export_batch_dialog = Some(state);
                    return;
                }
            }
        }

        self.export_batch_dialog = Some(state);
    }

    /// 入力の検証で弾いたときは `Err` を返し、呼び出し元がユーザーの入力を保ったまま
    /// ダイアログへ戻す。開始できた場合とスレッドを作れなかった場合はここで畳む。
    fn start_batch_export(&mut self, state: &mut ExportBatchDialogState) -> Result<(), String> {
        if self.export_pending.is_some() {
            return Err("エクスポート中です".to_string());
        }
        let output_dir = state.output_dir();
        if output_dir.as_os_str().is_empty() {
            return Err("保存先フォルダを指定してください".to_string());
        }

        self.settings.export_batch_directory = Some(output_dir.clone());
        self.settings.export_batch_template = state.template.clone();
        self.settings.export_batch_format = state.format;
        self.settings.export_batch_scale = state.scale;
        self.settings.save();

        if state.items.is_empty() {
            return Err("書き出す画像がありません".to_string());
        }

        let template = state.template.clone();
        let scale = state.scale;
        let format = state.format;
        // 入力の検証はここまでで済ませてから items を渡す。以降の失敗はスレッドを
        // 作れなかった場合だけで、そのときスナップショットは戻せない (closure ごと
        // 落ちる)。復帰不能なのでダイアログは閉じ、理由だけ出す。
        let mut items = std::mem::take(&mut state.items);
        // 形式はダイアログで選ぶので、スナップショット時の既定を上書きする。
        for item in &mut items {
            item.edits.format = format;
        }
        let request = crate::export_batch::BatchExportRequest {
            output_dir,
            template,
            scale,
            items,
            // 合成が消しゴム / AI 拡大を回し得るので、worker 終端までローカル AI 利用中に
            // 見せる (v3.5.0 レビュー F09)。
            local_ai_activity: self.local_ai_activity_lease(),
        };
        match crate::export_batch::spawn_batch_export_worker(request) {
            Ok(pending) => {
                self.export_pending = Some(pending);
                self.export_batch_dialog = None;
                Ok(())
            }
            Err(err) => {
                self.export_batch_dialog = None;
                self.show_feedback_toast(err);
                Ok(())
            }
        }
    }
}
