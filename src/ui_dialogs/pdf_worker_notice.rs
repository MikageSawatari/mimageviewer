//! PDF の読み込み準備に失敗したことを知らせる persistent notice。

use crate::app::App;
use crate::pdf_loader::PdfWorkerNotice;
use eframe::egui;

impl App {
    /// PDF loader の one-shot notice を update loop から poll し、UI state へ転写する。
    /// poll 自体は pool を初期化しない (PDF に触っていない利用者には出ない)。
    pub(crate) fn poll_pdf_worker_notice(&mut self) {
        if self.pdf_worker_notice.is_none() {
            self.pdf_worker_notice = crate::pdf_loader::take_worker_notice();
        }
    }

    /// 時間では消えない notice window。閉じるまで同じ理由を保持する。
    pub(crate) fn show_pdf_worker_notice_dialog(&mut self, ctx: &egui::Context) {
        let Some(notice) = self.pdf_worker_notice.as_ref() else {
            return;
        };
        let body = notice_text(notice);
        let content = ctx.content_rect();
        let default_pos = egui::pos2(content.max.x - 440.0 - 16.0, content.min.y + 56.0);
        let mut close_clicked = false;

        egui::Window::new("PDF の準備を開始できませんでした")
            .id(egui::Id::new("pdf_worker_notice"))
            .default_pos(default_pos)
            .default_width(420.0)
            .resizable(true)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.label(body);
                ui.separator();
                if ui.button("閉じる").clicked() {
                    close_clicked = true;
                }
            });

        if close_clicked {
            self.pdf_worker_notice = None;
        }
    }
}

/// 利用者向けの文面。内部用語 (ワーカー / プロセス / プール) を出さず、
/// 「何ができないか」「何は使えるか」「次に何をするか」の順で書く。
/// 失敗は `OnceLock` で確定するので、**復帰手段は再起動だけ**である点を必ず含める。
fn notice_text(notice: &PdfWorkerNotice) -> String {
    let mut lines: Vec<String> = Vec::new();
    lines.push(
        "PDF を読み込む準備ができなかったため、新しく PDF を開いたり、PDF のページを作ったり\
         できません。"
            .to_string(),
    );
    lines.push(String::new());
    lines.push(
        "画像・動画・アーカイブの表示と、すでに読み込み済みの PDF ページはそのまま利用できます。"
            .to_string(),
    );
    lines.push(String::new());
    lines.push(
        "アプリを再起動すると回復する場合があります。繰り返し起きる場合は、セキュリティソフトが \
         mImageViewer の動作を妨げていないか確認してください。"
            .to_string(),
    );
    lines.push(String::new());
    lines.push(format!(
        "詳細 (不具合報告用): 準備できた数 {}/{} (必要 {})",
        notice.ready_workers, notice.requested_workers, notice.minimum_workers
    ));
    lines.push(format!("最後のエラー: {}", notice.last_error));
    lines.push(format!("ログ: {}", notice.logs_dir.display()));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::notice_text;
    use crate::pdf_loader::PdfWorkerNotice;
    use std::path::PathBuf;

    fn sample() -> PdfWorkerNotice {
        PdfWorkerNotice {
            ready_workers: 2,
            requested_workers: 5,
            minimum_workers: 3,
            last_error: "readiness timed out".to_string(),
            logs_dir: PathBuf::from(r"C:\data\logs"),
        }
    }

    #[test]
    fn notice_includes_last_startup_error_and_log_directory() {
        let text = notice_text(&sample());

        assert!(text.contains("2/5"));
        assert!(text.contains("readiness timed out"));
        assert!(text.contains(r"C:\data\logs"));
        assert!(text.contains("画像・動画・アーカイブ"));
        assert!(text.contains("読み込み済みの PDF ページ"));
    }

    /// 失敗は OnceLock で確定するので、再起動以外に復帰手段が無い。文面から落とさない。
    #[test]
    fn notice_tells_the_user_that_restarting_is_the_way_back() {
        assert!(notice_text(&sample()).contains("再起動"));
    }

    /// 利用者向け文面に内部用語を出さない (CLAUDE.md「マニュアル・製品ページの記述方針」)。
    #[test]
    fn notice_avoids_internal_concurrency_vocabulary() {
        let text = notice_text(&sample());
        for term in ["ワーカー", "プロセス", "プール", "スレッド", "PDFium"] {
            assert!(!text.contains(term), "内部用語が文面に出ている: {term}");
        }
    }

    /// 行頭に折り返し由来の空白を残さない (Rust の行継続を書き損ねると混入する)。
    #[test]
    fn notice_lines_have_no_leading_whitespace() {
        for line in notice_text(&sample()).lines() {
            assert_eq!(line, line.trim_start(), "行頭に空白が残っている: {line:?}");
        }
    }
}
