//! グリッド選択の一括エクスポート ダイアログ (Ctrl+E)。
//!
//! 書き出しは専用 worker、保存済み編集の合成は製本と共通。
//! ダイアログの値の所有者は BatchExportOptions だけ。

use eframe::egui;

use crate::app::App;
use crate::export_batch::*;

#[derive(Default)]
struct BatchDialogAction {
    start: bool,
    cancel: bool,
    pick_folder: bool,
}

/// Enter is a candidate until the widgets have processed this frame's input.
struct EnterOwner {
    before: Option<egui::Id>,
    popup: bool,
    operated: bool,
    gained: bool,
    text_submitted: bool,
}

impl EnterOwner {
    fn new(ctx: &egui::Context) -> Self {
        Self {
            before: ctx.memory(|m| m.focused()),
            popup: egui::Popup::is_any_open(ctx),
            operated: false,
            gained: false,
            text_submitted: false,
        }
    }
    fn widget(&mut self, response: &egui::Response) {
        self.operated |= response.clicked() || response.changed();
        self.gained |= response.gained_focus();
    }
    fn text(&mut self, response: &egui::Response) {
        self.operated |= response.clicked();
        self.gained |= response.gained_focus();
        self.text_submitted |= self.before == Some(response.id) && response.lost_focus();
    }
    fn submit(&self, ctx: &egui::Context, enter: bool) -> bool {
        enter
            && !self.operated
            && !self.gained
            && !self.popup
            && !egui::Popup::is_any_open(ctx)
            && (self.text_submitted
                || (self.before.is_none() && ctx.memory(|m| m.focused()).is_none()))
    }
}

const DIALOG_MARGIN: f32 = 20.0;

fn draw_batch_window(
    ctx: &egui::Context,
    state: &mut ExportBatchDialogState,
    enter: bool,
) -> BatchDialogAction {
    let mut action = BatchDialogAction::default();
    let mut owner = EnterOwner::new(ctx);
    let mut open = true;
    let viewport = ctx.content_rect();
    let width = 520.0_f32.min((viewport.width() - 56.0).max(1.0));
    egui::Window::new("一括エクスポート")
        .drag_area(egui::WindowDrag::Anywhere)
        .collapsible(false)
        .resizable(false)
        .default_pos(viewport.min + egui::vec2(DIALOG_MARGIN, DIALOG_MARGIN))
        .default_width(width)
        // Give the scroll body's parent the screen budget instead of Window's
        // 420pt default. Non-resizable Resize and auto_shrink retain natural height.
        .default_height((viewport.height() - 2.0 * DIALOG_MARGIN).max(1.0))
        .min_width(1.0)
        .max_width(width)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(width);
            draw_batch_heading(ui, state);
            let footer = batch_footer_height(ui, state.error.as_deref());
            let body = super::startup_dialog_scroll_body(ui, "batch_export_body", footer, |ui| {
                draw_batch_body(ui, state, &mut owner, &mut action);
            });
            #[cfg(test)]
            ctx.data_mut(|data| {
                data.insert_temp(
                    egui::Id::unique("batch_export_layout"),
                    crate::test_ui::scroll_layout::ScrollLayout::from_output(&body),
                );
            });
            #[cfg(not(test))]
            let _ = body;
            draw_batch_actions(ui, state, &mut owner, &mut action);
        });
    action.cancel |= !open;
    action.start |= owner.submit(ctx, enter) && state.can_start();
    action
}

fn draw_batch_heading(ui: &mut egui::Ui, state: &ExportBatchDialogState) {
    if state.preparing() {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(format!(
                "編集内容を読んでいます… {} / {} 件",
                state.items.len(),
                state.total,
            ));
        });
    } else {
        ui.label(format!(
            "{} 件を書き出します (対象外 {} 件)",
            state.items.len(),
            state.skipped,
        ));
    }
}

fn batch_footer_height(ui: &egui::Ui, error: Option<&str>) -> f32 {
    let error_height = error.map_or(0.0, |error| {
        egui::WidgetText::from(error)
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Wrap),
                ui.available_width(),
                egui::TextStyle::Body,
            )
            .size()
            .y
            + ui.spacing().item_spacing.y
    });
    super::startup_dialog_footer_height(ui, &["書き出す", "キャンセル"], 8.0) + error_height
}

fn draw_batch_body(
    ui: &mut egui::Ui,
    state: &mut ExportBatchDialogState,
    owner: &mut EnterOwner,
    action: &mut BatchDialogAction,
) {
    ui.strong("出力先とファイル名");
    draw_destination(ui, &mut state.options.destination, owner, action);
    draw_filename(ui, state, owner);
    draw_overwrite(ui, &mut state.options.overwrite, owner);
    ui.separator();
    ui.strong("形式");
    draw_format(ui, &mut state.options.format, owner);
    draw_metadata_options(ui, &mut state.options, owner);
    ui.separator();
    ui.strong("サイズ");
    draw_resize(ui, &mut state.options.resize, owner);
    // All popups must consume their own wheel before the parent body.
    protect_popup_wheel(ui.ctx(), owner.popup);
}

fn draw_destination(
    ui: &mut egui::Ui,
    destination: &mut BatchDestination,
    owner: &mut EnterOwner,
    action: &mut BatchDialogAction,
) {
    let folder = matches!(destination, BatchDestination::Folder { .. });
    let response = ui.radio(folder, "フォルダ");
    if response.clicked() && !folder {
        *destination = BatchDestination::Folder {
            path: Default::default(),
            keep_structure: false,
        };
    }
    owner.widget(&response);
    if let BatchDestination::Folder {
        path,
        keep_structure,
    } = destination
    {
        let path_label = ui.label("保存先");
        ui.horizontal(|ui| {
            let mut path_text = path.display().to_string();
            let edit_width = (ui.available_width() - 90.0).max(1.0);
            let mut output = crate::ime_focus::show_singleline(ui, &mut path_text, None, |edit| {
                edit.desired_width(edit_width).hint_text("保存先フォルダ")
            });
            output.response.response = output.response.response.labelled_by(path_label.id);
            crate::ui_helpers::singleline_text_edit_context_menu(ui, &mut output, &mut path_text);
            owner.text(&output.response);
            if output.response.changed() {
                *path = std::path::PathBuf::from(path_text);
            }
            let response = ui.button("変更...");
            action.pick_folder |= response.clicked();
            owner.widget(&response);
        });
        owner.widget(&ui.checkbox(keep_structure, "元のフォルダ構造を保つ"));
    }
    let same = matches!(destination, BatchDestination::SameAsSource { .. });
    let response = ui.radio(same, "元と同じフォルダ");
    if response.clicked() && !same {
        *destination = BatchDestination::SameAsSource {
            subfolder: String::new(),
        };
    }
    owner.widget(&response);
    if let BatchDestination::SameAsSource { subfolder } = destination {
        let label = ui.label("サブフォルダ (空なら元のフォルダの直下)");
        let mut output = crate::ime_focus::show_singleline(ui, subfolder, None, |edit| {
            edit.desired_width(f32::INFINITY)
        });
        output.response.response = output.response.response.labelled_by(label.id);
        crate::ui_helpers::singleline_text_edit_context_menu(ui, &mut output, subfolder);
        owner.text(&output.response);
    }
}

fn draw_filename(ui: &mut egui::Ui, state: &mut ExportBatchDialogState, owner: &mut EnterOwner) {
    let label = ui.label("ファイル名");
    let mut output =
        crate::ime_focus::show_singleline(ui, &mut state.options.template, None, |edit| {
            edit.desired_width(f32::INFINITY)
                .hint_text("ファイル名テンプレート")
        });
    output.response.response = output.response.response.labelled_by(label.id);
    crate::ui_helpers::singleline_text_edit_context_menu(
        ui,
        &mut output,
        &mut state.options.template,
    );
    owner.text(&output.response);
    if !state.initial_focus_done {
        output.response.request_focus();
        state.initial_focus_done = true;
        owner.gained = true;
    }
    ui.small(TEMPLATE_PLACEHOLDER_HINT);
    if let Some(preview) = state.preview_name() {
        ui.add(egui::Label::new(format!("例: {preview}")).wrap());
    }
    if state.options.template.contains("<date>") || state.options.template.contains("<time>") {
        ui.small("<date> / <time> は書き出し時に決まります");
    }
}

fn draw_overwrite(ui: &mut egui::Ui, policy: &mut OverwritePolicy, owner: &mut EnterOwner) {
    ui.label("同じ名前があるとき");
    ui.horizontal_wrapped(|ui| {
        for (value, label) in [
            (OverwritePolicy::Rename, "連番"),
            (OverwritePolicy::Skip, "スキップ"),
            (OverwritePolicy::Replace, "上書き"),
        ] {
            owner.widget(&ui.radio_value(policy, value, label));
        }
    });
    match policy {
        OverwritePolicy::Rename => {
            ui.small("同名のファイルは上書きせず、末尾に連番を付けます。");
        }
        OverwritePolicy::Skip => {
            ui.small("既にあるファイルは読み込まずにスキップします。");
        }
        OverwritePolicy::Replace => {
            ui.add(
                egui::Label::new(
                    "上書きしたファイルは元に戻せません。今回の元ファイルは上書きしません。",
                )
                .wrap(),
            );
        }
    }
}

fn draw_format(ui: &mut egui::Ui, format: &mut BatchFormat, owner: &mut EnterOwner) {
    let combo = egui::ComboBox::from_label("ファイル形式")
        .selected_text(format_label(format.kind))
        .popup_style(crate::os_theme::dark_popup_style(ui.ctx()))
        .show_ui(ui, |ui| {
            for value in [
                BatchFormatKind::Png,
                BatchFormatKind::Jpeg,
                BatchFormatKind::Webp,
                BatchFormatKind::SameAsSource,
            ] {
                owner.widget(&ui.selectable_value(&mut format.kind, value, format_label(value)));
            }
        });
    owner.widget(&combo.response);
    if matches!(
        format.kind,
        BatchFormatKind::Jpeg | BatchFormatKind::SameAsSource
    ) {
        ui.horizontal(|ui| {
            let label = ui.label("JPEG 品質");
            owner.widget(
                &ui.add(egui::DragValue::new(&mut format.jpeg_quality).range(1..=100))
                    .labelled_by(label.id),
            );
        });
    }
    if matches!(
        format.kind,
        BatchFormatKind::Webp | BatchFormatKind::SameAsSource
    ) {
        owner.widget(&ui.checkbox(&mut format.webp_lossless, "WebP 可逆圧縮"));
        if !format.webp_lossless {
            ui.horizontal(|ui| {
                let label = ui.label("WebP 品質");
                owner.widget(
                    &ui.add(egui::DragValue::new(&mut format.webp_quality).range(1..=100))
                        .labelled_by(label.id),
                );
            });
        }
    }
    if format.kind == BatchFormatKind::SameAsSource {
        ui.horizontal(|ui| {
            ui.label("対応しない元形式の代わり");
            for (value, label) in [(SameFallback::Jpeg, "JPEG"), (SameFallback::Png, "PNG")] {
                owner.widget(&ui.radio_value(&mut format.same_fallback, value, label));
            }
        });
    }
}

fn draw_metadata_options(
    ui: &mut egui::Ui,
    options: &mut BatchExportOptions,
    owner: &mut EnterOwner,
) {
    owner.widget(&ui.checkbox(
        &mut options.keep_metadata,
        "元の情報 (EXIF・XMP・AI プロンプト・色) を残す",
    ));
    let note = if options.keep_metadata {
        concat!(
            "形式が元と同じ画像だけ、元の情報を残します。",
            "元の情報に縮小画像が含まれていそうなときは、書き出しの後で知らせます ",
            "(隠した部分が縮小画像に残ることがあります)。",
        )
    } else {
        "選んだ形式で再エンコードします。元の EXIF / AI プロンプトは引き継ぎません。"
    };
    ui.add(egui::Label::new(note).wrap());
    owner.widget(&ui.checkbox(&mut options.keep_file_time, "元の更新日時にする"));
}

fn draw_batch_actions(
    ui: &mut egui::Ui,
    state: &ExportBatchDialogState,
    owner: &mut EnterOwner,
    action: &mut BatchDialogAction,
) {
    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        let response = ui.add_enabled(state.can_start(), egui::Button::new("書き出す"));
        action.start |= response.clicked();
        owner.widget(&response);
        let response = ui.button("キャンセル");
        action.cancel |= response.clicked();
        owner.widget(&response);
    });
    if let Some(error) = &state.error {
        ui.add(
            egui::Label::new(egui::RichText::new(error).color(ui.visuals().error_fg_color)).wrap(),
        );
    }
}

#[cfg(test)]
mod tests;

fn format_label(kind: BatchFormatKind) -> &'static str {
    match kind {
        BatchFormatKind::Png => "PNG",
        BatchFormatKind::Jpeg => "JPEG",
        BatchFormatKind::Webp => "WebP",
        BatchFormatKind::SameAsSource => "元と同じ形式",
    }
}

fn draw_resize(ui: &mut egui::Ui, resize: &mut BatchResize, owner: &mut EnterOwner) {
    let mut index = match resize.mode {
        BatchResizeMode::None => 0,
        BatchResizeMode::Percent(_) => 1,
        BatchResizeMode::LongEdge(_) => 2,
        BatchResizeMode::ShortEdge(_) => 3,
        BatchResizeMode::Width(_) => 4,
        BatchResizeMode::Height(_) => 5,
        BatchResizeMode::Fit { .. } => 6,
    };
    const LABELS: [&str; 7] = [
        "そのまま",
        "割合",
        "長辺",
        "短辺",
        "幅",
        "高さ",
        "枠に収める",
    ];
    let old = index;
    let combo = egui::ComboBox::from_label("リサイズ")
        .selected_text(LABELS[index])
        .popup_style(crate::os_theme::dark_popup_style(ui.ctx()))
        .show_ui(ui, |ui| {
            for (n, label) in LABELS.into_iter().enumerate() {
                owner.widget(&ui.selectable_value(&mut index, n, label));
            }
        });
    owner.widget(&combo.response);
    if old != index {
        resize.mode = match index {
            0 => BatchResizeMode::None,
            1 => BatchResizeMode::Percent(50),
            2 => BatchResizeMode::LongEdge(2048),
            3 => BatchResizeMode::ShortEdge(2048),
            4 => BatchResizeMode::Width(2048),
            5 => BatchResizeMode::Height(2048),
            _ => BatchResizeMode::Fit {
                width: 2048,
                height: 2048,
            },
        };
    }
    ui.horizontal(|ui| match &mut resize.mode {
        BatchResizeMode::None => {}
        BatchResizeMode::Percent(value) => {
            let label = ui.label("割合");
            owner.widget(
                &ui.add(egui::DragValue::new(value).range(1..=400).suffix("%"))
                    .labelled_by(label.id),
            );
        }
        BatchResizeMode::LongEdge(value)
        | BatchResizeMode::ShortEdge(value)
        | BatchResizeMode::Width(value)
        | BatchResizeMode::Height(value) => {
            let label = ui.label(LABELS[index]);
            owner.widget(
                &ui.add(egui::DragValue::new(value).range(256..=16384).suffix("px"))
                    .labelled_by(label.id),
            );
        }
        BatchResizeMode::Fit { width, height } => {
            let width_label = ui.label("幅");
            owner.widget(
                &ui.add(egui::DragValue::new(width).range(256..=16384).suffix("px"))
                    .labelled_by(width_label.id),
            );
            let height_label = ui.label("高さ");
            owner.widget(
                &ui.add(egui::DragValue::new(height).range(256..=16384).suffix("px"))
                    .labelled_by(height_label.id),
            );
        }
    });
    owner.widget(&ui.checkbox(&mut resize.allow_upscale, "拡大する"));
    let label = |filter| match filter {
        ResizeFilter::Lanczos3 => "Lanczos (既定)",
        ResizeFilter::CatmullRom => "バイキュービック",
        ResizeFilter::Bilinear => "バイリニア",
        ResizeFilter::Nearest => "最近傍 (ドット絵向け)",
    };
    let combo = egui::ComboBox::from_label("方式")
        .selected_text(label(resize.filter))
        .popup_style(crate::os_theme::dark_popup_style(ui.ctx()))
        .show_ui(ui, |ui| {
            for value in [
                ResizeFilter::Lanczos3,
                ResizeFilter::CatmullRom,
                ResizeFilter::Bilinear,
                ResizeFilter::Nearest,
            ] {
                owner.widget(&ui.selectable_value(&mut resize.filter, value, label(value)));
            }
        });
    owner.widget(&combo.response);
}

fn protect_popup_wheel(ctx: &egui::Context, was_open: bool) {
    if was_open || egui::Popup::is_any_open(ctx) {
        crate::wheel_input::consume_popup_wheel_input(ctx);
    }
}

pub(crate) fn draw_progress_window(
    ctx: &egui::Context,
    pending: &crate::export_dialog::ExportPending,
) -> (bool, bool) {
    let mut open = true;
    let mut close = false;
    let mut cancel = false;
    let warning = pending.end.is_some() && !pending.thumbnail_notices.is_empty();
    let viewport = ctx.content_rect();
    let width = 380.0_f32.min((viewport.width() - 56.0).max(1.0));
    egui::Window::new("エクスポート進行状況")
        .drag_area(egui::WindowDrag::Anywhere)
        .collapsible(false)
        .resizable(false)
        .default_pos(viewport.min + egui::vec2(DIALOG_MARGIN, DIALOG_MARGIN))
        .default_width(width)
        .default_height((viewport.height() - 2.0 * DIALOG_MARGIN).max(1.0))
        .min_width(1.0)
        .max_width(width)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(width);
            if warning {
                draw_thumbnail_warning(ui, pending);
            } else {
                draw_running_progress(ui, pending);
            }
            ui.add_space(8.0);
            if pending.end.is_some() {
                close |= ui.button("閉じる").clicked();
            } else if pending.cancel_requested {
                ui.add_enabled(false, egui::Button::new("キャンセル中..."));
            } else {
                cancel |= ui.button("キャンセル").clicked();
            }
        });
    if !open {
        if pending.end.is_some() {
            close = true;
        } else {
            cancel = true;
        }
    }
    (close, cancel)
}

fn draw_thumbnail_warning(ui: &mut egui::Ui, pending: &crate::export_dialog::ExportPending) {
    let heading = format!(
        "元の縮小画像が残っている可能性があります ({} 件)",
        pending.thumbnail_notices.len(),
    );
    ui.add(
        egui::Label::new(
            egui::RichText::new(heading)
                .strong()
                .color(ui.visuals().warn_fg_color),
        )
        .wrap(),
    );
    ui.add(egui::Label::new(&pending.last_message).wrap());
    let footer = super::startup_dialog_footer_height(ui, &["閉じる"], 8.0);
    let body = super::startup_dialog_scroll_body(ui, "batch_export_warning_body", footer, |ui| {
        draw_thumbnail_warning_body(ui, pending);
    });
    #[cfg(test)]
    ui.ctx().data_mut(|data| {
        data.insert_temp(
            egui::Id::unique("batch_export_warning_layout"),
            crate::test_ui::scroll_layout::ScrollLayout::from_output(&body),
        );
    });
    #[cfg(not(test))]
    let _ = body;
}

fn draw_thumbnail_warning_body(ui: &mut egui::Ui, pending: &crate::export_dialog::ExportPending) {
    let description = concat!(
        "元の情報を残して書き出した次のファイルに、元の絵の縮小画像が含まれている可能性があります。",
        "消しゴムや隠蔽で隠した部分が、縮小画像に残っていることがあります。",
        "公開する前に確認するか、『元の情報を残す』をオフにして書き出し直してください。",
    );
    ui.add(egui::Label::new(description).wrap());
    for notice in &pending.thumbnail_notices {
        let mut kinds = Vec::new();
        if notice.kinds.xmp {
            kinds.push("XMP の縮小画像");
        }
        if notice.kinds.xmp_uninspectable {
            kinds.push("XMP (調べられませんでした)");
        }
        if notice.kinds.maker_note {
            kinds.push("メーカー独自の情報");
        }
        let name = notice
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        if ui
            .add(egui::Button::new(format!("{name}: {}", kinds.join(" / "))).wrap())
            .clicked()
        {
            crate::capture::reveal_path_async(notice.path.clone());
        }
    }
    draw_export_failures(ui, pending);
}

fn draw_export_failures(ui: &mut egui::Ui, pending: &crate::export_dialog::ExportPending) {
    for error in &pending.errors {
        ui.add(egui::Label::new(format!("{}: {}", error.label, error.message)).wrap());
    }
}

fn draw_running_progress(ui: &mut egui::Ui, pending: &crate::export_dialog::ExportPending) {
    ui.add(egui::Label::new(&pending.last_message).wrap());
    let progress = if pending.total == 0 {
        0.0
    } else {
        pending.done as f32 / pending.total as f32
    };
    ui.add(
        egui::ProgressBar::new(progress)
            .show_percentage()
            .text(format!("{} / {}", pending.done, pending.total)),
    );
    ui.label(format!(
        "成功 {} / スキップ {} / 失敗 {}",
        pending.successes.len(),
        pending.skipped.len(),
        pending.errors.len(),
    ));
    if !pending.thumbnail_notices.is_empty() {
        ui.add(
            egui::Label::new(format!(
                "縮小画像の警告 {} 件 (終了後に一覧を出します)",
                pending.thumbnail_notices.len(),
            ))
            .wrap(),
        );
    }
    if !pending.errors.is_empty() {
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("{} 件のエラー", pending.errors.len()),
        );
        egui::ScrollArea::vertical()
            .max_height(120.0)
            .auto_shrink([false, true])
            .show(ui, |ui| draw_export_failures(ui, pending));
    }
}

/// まだ編集内容を読んでいない書き出し対象。
///
/// 一覧 index を持つ通常のページと、集約セルを展開したスタックメンバー (index を
/// 持たない実ファイル) の 2 種類。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExportBatchTarget {
    /// 一覧の項目。**index だけでなくページキーで身元を持つ。**
    ///
    /// 準備は複数フレームにまたがるので、その間に外部監視の再読込などで一覧が並び替わる
    /// ことがある。index だけを覚えていると、残りの対象が別のファイルを指し、選んでいない
    /// ファイルが出力されたり、選んだファイルが落ちたりする (v3.5.0 レビュー R04)。
    Item {
        idx: usize,
        page_key: String,
    },
    StackMember(std::path::PathBuf),
}

pub struct ExportBatchDialogState {
    /// 編集内容を読み終えた書き出し対象。読んだ時点の絵で固定されるので、
    /// ダイアログを開いている間にページを編集しても出力は変わらない。
    pub items: Vec<crate::export_batch::BatchExportItem>,
    /// まだ読んでいない対象。**1 フレームで全部読まない。**
    ///
    /// 隠蔽 / 消しゴムのマスクは圧縮 payload の展開を伴い、補正レイヤーと注釈フォントも
    /// 読み込みが要る。選択件数ぶんまとめて読むと、押してからダイアログが出るまで UI が
    /// 止まり、キャンセルもできない (v3.5.0 レビュー F04)。フレームあたりの予算で進める。
    pub pending: std::collections::VecDeque<ExportBatchTarget>,
    /// 対象総数 (進捗表示用)。`items.len() + pending.len()` は失敗分だけずれる。
    pub total: usize,
    /// 選択に含まれていたが書き出せなかった件数 (フォルダ / 動画 / 音声 / 書庫本体)。
    pub skipped: usize,
    pub options: BatchExportOptions,
    pub initial_focus_done: bool,
    pub error: Option<String>,
}

impl ExportBatchDialogState {
    pub(crate) fn preparing(&self) -> bool {
        !self.pending.is_empty()
    }

    fn can_start(&self) -> bool {
        !self.preparing()
            && !self.items.is_empty()
            && !self.options.template.trim().is_empty()
            && match &self.options.destination {
                BatchDestination::Folder { path, .. } => !path.to_string_lossy().trim().is_empty(),
                BatchDestination::SameAsSource { .. } => true,
            }
    }

    fn preview_name(&self) -> Option<String> {
        let item = self.items.first()?;
        Some(format!(
            "{}.{}",
            preview_item_stem(&self.options.template, item, 1),
            self.options.format.resolve(&item.source_ext).extension()
        ))
    }
}

impl App {
    /// グリッド選択を一括エクスポートするダイアログを開く。対象が 1 件も無ければ
    /// 開かずに理由を出す。
    pub(crate) fn open_export_batch_dialog(&mut self, ctx: &egui::Context) {
        if self.export_pending.is_some() {
            self.show_feedback_warning("エクスポート中です".to_string());
            return;
        }
        if self.export_batch_dialog.is_some() {
            return;
        }
        if self.grid_selection_indices().is_empty() {
            self.show_feedback_warning("書き出す画像・ページを選択してください".to_string());
            return;
        }
        // 対象を決めるのは安い判定だけ。編集内容はフレーム予算で読み進める (F04)。
        let (pending, skipped) = self.grid_batch_export_targets();
        if pending.is_empty() {
            self.show_feedback_warning("選択の中に書き出せる画像・ページがありません".to_string());
            return;
        }
        let options = self.batch_export_initial_options();
        let total = pending.len();
        self.export_batch_dialog = Some(ExportBatchDialogState {
            items: Vec::with_capacity(total),
            pending,
            total,
            skipped,
            options,
            initial_focus_done: false,
            error: None,
        });
        ctx.request_repaint();
    }

    /// ダイアログを開かずに準備を最後まで回す (テスト用)。**production と同じ 2 つの
    /// 関数を回すだけ**で、判定は複製しない。
    #[cfg(test)]
    pub(crate) fn prepare_all_batch_export_items_for_test(
        &mut self,
    ) -> (Vec<crate::export_batch::BatchExportItem>, usize) {
        let (pending, skipped) = self.grid_batch_export_targets();
        let total = pending.len();
        let mut state = ExportBatchDialogState {
            items: Vec::new(),
            pending,
            total,
            skipped,
            options: BatchExportOptions::default(),
            initial_focus_done: true,
            error: None,
        };
        while state.preparing() {
            self.advance_export_batch_preparation(&mut state);
        }
        (state.items, state.skipped)
    }

    #[cfg(test)]
    pub(crate) fn advance_export_batch_preparation_for_test(
        &mut self,
        state: &mut ExportBatchDialogState,
    ) {
        self.advance_export_batch_preparation(state);
    }

    /// 1 フレームぶんの準備を進める。**予算を超えたら次のフレームへ持ち越す。**
    ///
    /// 予算は「1 件は必ず進める。超えたらそこで止める」。1 件が重くても必ず前進するので
    /// 止まらず、軽い件が続くフレームではまとめて片付く。
    fn advance_export_batch_preparation(&mut self, state: &mut ExportBatchDialogState) {
        const BUDGET: std::time::Duration = std::time::Duration::from_millis(6);
        if state.pending.is_empty() {
            return;
        }
        let started = std::time::Instant::now();
        while let Some(target) = state.pending.pop_front() {
            match self.batch_export_item_for_target(&target) {
                Ok(Some(item)) => state.items.push(item),
                Ok(None) => state.skipped += 1,
                Err(err) => {
                    state.skipped += 1;
                    crate::logger::log(format!("batch export skip {target:?}: {err}"));
                }
            }
            if started.elapsed() >= BUDGET {
                break;
            }
        }
    }

    fn batch_export_initial_options(&self) -> BatchExportOptions {
        let mut options = self
            .settings
            .export_batch_options
            .clone()
            .unwrap_or_else(|| {
                BatchExportOptions::from_legacy(
                    std::path::PathBuf::new(),
                    self.settings.export_batch_template.clone(),
                    self.settings.export_batch_format,
                    self.settings.export_batch_scale,
                )
            });
        if let BatchDestination::Folder { path, .. } = &mut options.destination {
            if path.as_os_str().is_empty() {
                *path = self
                    .settings
                    .export_batch_directory
                    .clone()
                    .filter(|p| p.is_dir())
                    .or_else(|| self.settings.export_last_directory.clone())
                    .or_else(|| self.current_folder.clone())
                    .unwrap_or_default();
            }
        }
        options
    }

    pub(crate) fn draw_export_batch_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.export_batch_dialog.take() else {
            return;
        };
        self.advance_export_batch_preparation(&mut state);
        let enter = self.dialog_enter_pressed(ctx);
        let escape = self.dialog_escape_pressed(ctx);
        let action = draw_batch_window(ctx, &mut state, enter);
        if action.pick_folder {
            let directory = match &state.options.destination {
                BatchDestination::Folder { path, .. } => path.clone(),
                _ => Default::default(),
            };
            if let Some(path) = rfd::FileDialog::new()
                .set_directory(directory)
                .pick_folder()
            {
                if let BatchDestination::Folder { path: target, .. } =
                    &mut state.options.destination
                {
                    *target = path;
                }
            }
        }
        if escape || action.cancel {
            return;
        }
        if action.start {
            match self.start_batch_export(&mut state) {
                Ok(()) => return,
                Err(error) => state.error = Some(error),
            }
        }
        if state.preparing() {
            ctx.request_repaint();
        }
        self.export_batch_dialog = Some(state);
    }

    /// 入力の検証で弾いたときは `Err` を返し、呼び出し元がユーザーの入力を保ったまま
    /// ダイアログへ戻す。開始できた場合とスレッドを作れなかった場合はここで畳む。
    fn start_batch_export(&mut self, state: &mut ExportBatchDialogState) -> Result<(), String> {
        if self.export_pending.is_some() {
            return Err("エクスポート中です".to_string());
        }
        if !state.can_start() {
            return Err("保存先・ファイル名と書き出す画像を確認してください".into());
        }
        self.settings.export_batch_options = Some(state.options.clone());
        self.settings.save();
        let items = std::mem::take(&mut state.items);
        let request = crate::export_batch::BatchExportRequest {
            options: state.options.clone(),
            items,
            raw: Some(crate::raw::RawDecodeContext::new(
                std::sync::Arc::clone(&self.raw_develop_executor),
                self.settings.raw_brightness,
            )),
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
                self.show_feedback_error(err);
                Ok(())
            }
        }
    }
}
