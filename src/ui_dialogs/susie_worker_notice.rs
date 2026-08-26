//! Susie プラグインでの読み込みを打ち切ったことを知らせる persistent notice。
//!
//! 枠が 1 つ減っただけでは出さない。残りの枠が処理を続けられる間、利用者にできる
//! ことは無い。発行は `susie_loader` 側で**最後の枠が諦めたとき**だけに絞ってある。

use crate::app::App;
use crate::susie_loader::SusieWorkerNotice;
use eframe::egui;

impl App {
    /// susie_loader の one-shot notice を update loop から poll し、UI state へ転写する。
    /// poll 自体はプールを初期化しない (Susie を使っていない利用者には出ない)。
    pub(crate) fn poll_susie_worker_notice(&mut self) {
        if self.susie_worker_notice.is_none() {
            self.susie_worker_notice = crate::susie_loader::take_worker_notice();
        }
    }

    /// 時間では消えない notice window。閉じるまで同じ理由を保持する。
    pub(crate) fn show_susie_worker_notice_dialog(&mut self, ctx: &egui::Context) {
        let Some(notice) = self.susie_worker_notice.as_ref() else {
            return;
        };
        let body = notice_text(notice);
        let content = ctx.content_rect();
        let default_pos = egui::pos2(content.max.x - 440.0 - 16.0, content.min.y + 56.0);
        let mut close_clicked = false;

        egui::Window::new("Susie プラグインでの読み込みを打ち切りました")
            .id(egui::Id::new("susie_worker_notice"))
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
            self.susie_worker_notice = None;
        }
    }
}

/// 利用者向けの文面。内部用語 (ワーカー / プロセス / プール) を出さず、
/// 「何ができないか」「何は使えるか」「次に何をするか」の順で書く。
///
/// **復帰手段はアプリの再起動ではない。**環境設定から読み込み直せるので、そちらを
/// 案内する。再起動を案内すると、開いている一覧や表示位置を捨てさせることになる。
fn notice_text(notice: &SusieWorkerNotice) -> String {
    let health = &notice.health;
    let mut lines: Vec<String> = Vec::new();
    lines.push(
        "Susie プラグインが必要な形式の画像を、これ以上開けません。同じ問題が繰り返し起きたため、\
         読み込みを打ち切りました。"
            .to_string(),
    );
    lines.push(String::new());
    lines.push("本体が対応している画像・動画・アーカイブの表示には影響しません。".to_string());
    lines.push(String::new());
    lines.push(
        "環境設定の「Susie プラグイン」ページで「⟳ プラグインを再読み込み」を押すと、もう一度\
         使えるようになります。特定の画像で毎回起きる場合は、その画像を開かないようにするか、\
         原因のプラグインを外してください。"
            .to_string(),
    );
    lines.push(String::new());
    lines.push(format!(
        "詳細 (不具合報告用): 読み込み枠 {}/{}、作り直し {} 回、開けなかった画像 {} 件",
        health.live_workers, health.started_workers, health.restarts, health.crashing_subjects
    ));
    if let Some(reason) = &health.last_failure {
        lines.push(format!("最後のエラー: {reason}"));
    }
    lines.push(format!("ログ: {}", notice.logs_dir.display()));
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::notice_text;
    use crate::susie_loader::{SusieWorkerHealth, SusieWorkerNotice};
    use std::path::PathBuf;

    fn sample() -> SusieWorkerNotice {
        SusieWorkerNotice {
            health: SusieWorkerHealth {
                started_workers: 3,
                live_workers: 0,
                restarts: 5,
                gave_up_workers: 3,
                crashing_subjects: 2,
                last_failure: Some("unexpected end of file".to_string()),
            },
            logs_dir: PathBuf::from(r"C:\data\logs"),
        }
    }

    #[test]
    fn notice_includes_the_counts_the_last_error_and_the_log_directory() {
        let text = notice_text(&sample());

        assert!(text.contains("0/3"), "{text}");
        assert!(text.contains("作り直し 5 回"), "{text}");
        assert!(text.contains("開けなかった画像 2 件"), "{text}");
        assert!(text.contains("unexpected end of file"), "{text}");
        assert!(text.contains(r"C:\data\logs"), "{text}");
    }

    /// 復帰手段は再読み込みであって再起動ではない。案内を取り違えない。
    #[test]
    fn notice_points_at_the_reload_button_not_at_restarting_the_app() {
        let text = notice_text(&sample());
        assert!(text.contains("プラグインを再読み込み"), "{text}");
        assert!(!text.contains("アプリを再起動"), "{text}");
    }

    /// 何が使えるかを必ず書く。読めなくなったのは Susie 形式だけである。
    #[test]
    fn notice_says_what_still_works() {
        assert!(notice_text(&sample()).contains("画像・動画・アーカイブ"));
    }

    /// 利用者向け文面に内部用語を出さない (CLAUDE.md「マニュアル・製品ページの記述方針」)。
    #[test]
    fn notice_avoids_internal_concurrency_vocabulary() {
        let text = notice_text(&sample());
        for term in ["ワーカー", "プロセス", "プール", "スレッド", "ディスパッチ"]
        {
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

    /// 失敗理由が取れていなくても文面が成立する。
    #[test]
    fn notice_without_a_recorded_error_still_reads_as_a_complete_message() {
        let mut notice = sample();
        notice.health.last_failure = None;
        let text = notice_text(&notice);
        assert!(!text.contains("最後のエラー"), "{text}");
        assert!(text.contains("読み込み枠 0/3"), "{text}");
    }
}
