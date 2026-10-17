//! Pure modal rendering; the process operation owns all actions and I/O.
use crate::image_folder_zip_job::Summary;
use std::path::Path;

#[derive(Clone, Copy)]
pub(crate) enum Phase<'a> {
    GuideConfirm(&'a Summary),
    Preparing,
    Waiting,
    Confirm(&'a Summary),
    Copying {
        images: usize,
        total: usize,
        bytes: u64,
    },
    Saving,
    Adopting,
    Result {
        message: &'a str,
        success: bool,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Action {
    Convert,
    Cancel,
    Close,
}

/// The same process modal blocks pointer/focus in every displayed viewport,
/// including a sibling that does not own the conversion's dialog.
pub(crate) fn block_background(ctx: &egui::Context) {
    let _ = egui::Modal::new(egui::Id::unique("image_folder_zip_input_barrier"))
        .frame(egui::Frame::NONE)
        .backdrop_color(egui::Color32::TRANSPARENT)
        .show(ctx, |_| {});
}

pub(crate) fn draw(
    ctx: &egui::Context,
    source: &Path,
    destination: &Path,
    phase: Phase<'_>,
    can_cancel: bool,
    escape: bool,
) -> Option<Action> {
    block_background(ctx);
    let rect = ctx.content_rect();
    let width = 600.0_f32.min((rect.width() - 40.0).max(180.0));
    let height = 400.0_f32.min((rect.height() - 70.0).max(120.0));
    let mut action = None;
    let mut open = true;
    let result = matches!(phase, Phase::Result { .. });
    let notice = matches!(phase, Phase::GuideConfirm(_));
    egui::Window::new("画像フォルダを ZIP に変換").drag_area(egui::WindowDrag::Anywhere)
        .id(egui::Id::unique("image_folder_zip_modal"))
        .order(egui::Order::Foreground)
        .open(&mut open).collapsible(false).resizable(true)
        .default_pos(rect.min+egui::vec2(20.0,20.0))
        .default_size(egui::vec2(width,height)).max_size(egui::vec2(width,height))
        .show(ctx, |ui| {
            ui.style_mut().spacing.scroll.floating_allocated_width = ui.spacing().scroll.bar_inner_margin + ui.spacing().scroll.bar_width;
            let action_height = ui.spacing().interact_size.y + ui.spacing().item_spacing.y * 2.0;
            let body_height = (ui.available_height()-action_height).max(40.0);
            egui::ScrollArea::vertical().scroll_source(egui::scroll_area::ScrollSource { drag: egui::scroll_area::DragScroll::Always, ..Default::default() }).auto_shrink([false,true]).max_height(body_height).show(ui, |ui| {
                ui.label("元のフォルダは残します。画像を再圧縮せずに保存します。");
                ui.label(egui::RichText::new(source.display().to_string()).strong());
                ui.label("作成先:");
                ui.label(destination.display().to_string());
                ui.separator();
                match phase {
                    Phase::Preparing => { ui.spinner(); ui.label("画像と作成先を確認しています。"); },
                    Phase::Waiting => { ui.spinner(); ui.label("先に始めた保存処理の完了を待っています。"); },
                    Phase::GuideConfirm(summary) | Phase::Confirm(summary) => {
                        if notice {ui.label("画像だけのフォルダでは本の構成を作れません。ZIP に変換してから行ってください。");}
                        ui.label(format!("画像 {} 枚、合計 {:.1} MB",summary.images,summary.bytes as f64/1_000_000.0));
                        ui.label(format!("登録・編集など {} 件を引き継ぎます。",summary.rows));
                        ui.label("表示設定で隠している画像も全て含めます。画像以外のファイルは元のフォルダに残します。");
                        if summary.auxiliary > 0 { ui.label(format!("画像以外 {} 件は ZIP に入れません。",summary.auxiliary)); }
                        ui.label("画像のタグはまとめて ZIP 本体のタグにします。コレクションは元の画像を指したままです。");
                    },
                    Phase::Copying { images,total,bytes } => {
                        ui.add(egui::ProgressBar::new(images as f32 / total.max(1) as f32).show_percentage());
                        ui.label(format!("画像 {images}/{total} 枚、{:.1} MB を保存しました。",bytes as f64/1_000_000.0));
                    },
                    Phase::Saving => { ui.spinner(); ui.label("登録と編集の保存を完了しています。ここからは取り消せません。"); },
                    Phase::Adopting => { ui.spinner(); ui.label("保存した内容を表示に反映しています。"); },
                    Phase::Result {message,success} => {
                        ui.colored_label(if success {ui.visuals().text_color()} else {ui.visuals().warn_fg_color},message);
                    },
                }
            });
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                if notice || matches!(phase,Phase::Confirm(_)) {
                    if ui.button("ZIP に変換").clicked() { action = Some(Action::Convert); }
                }
                if result {
                    if ui.button("閉じる").clicked() { action=Some(Action::Close); }
                } else if ui.add_enabled(can_cancel,egui::Button::new("取消")).clicked() { action=Some(Action::Cancel); }
            });
        });
    if !open || escape {
        if result {
            action = Some(Action::Close);
        } else if can_cancel {
            action = Some(Action::Cancel);
        }
    }
    action
}

#[doc(hidden)]
pub fn draw_snapshot(ctx: &egui::Context, phase: &str) {
    let summary = Summary {
        images: 250,
        bytes: 350_000_000,
        auxiliary: 3,
        rows: 1200,
    };
    let phase = match phase {
        "notice" => Phase::GuideConfirm(&summary),
        "confirm" => Phase::Confirm(&summary),
        "copying" => Phase::Copying {
            images: 100,
            total: 250,
            bytes: 140_000_000,
        },
        "saving" => Phase::Saving,
        "waiting" => Phase::Waiting,
        "adopting" => Phase::Adopting,
        "error" => Phase::Result {
            message: "ZIP を作成できませんでした。元のフォルダは残っています。詳しくはログを確認してください。",
            success: false,
        },
        "result" => Phase::Result {
            message: "ZIP を作成しました。元のフォルダは残っています。",
            success: true,
        },
        _ => Phase::Preparing,
    };
    let _ = draw(
        ctx,
        Path::new("C:/画像/長い名前のフォルダ/保存しておきたい写真集"),
        Path::new("C:/画像/長い名前のフォルダ/保存しておきたい写真集.zip"),
        phase,
        !matches!(phase, Phase::Saving | Phase::Adopting),
        false,
    );
}
