//! 一覧が空になったときの、型付きの理由。
//!
//! 「表示するファイルがありません」は本当に 0 件だったときの文言だが、読み込みに
//! 失敗したときにも同じ文が出ていた。利用者からは「PDF が開けたり開けなかったり
//! する」としか見えず、ログにも失敗した事実が 1 行しか残らない (§1.68)。
//!
//! 空にした経路が理由を残し、grid の中央表示とログがそれを読む。**原因が分からない
//! 不具合は、直す前に無言の早期 return へ型付きの理由を足す** (CLAUDE.md
//! 「バグ修正の一般原則」)。理由を足すこと自体が修正ではないので、これが入った版で
//! ログを待つ。

/// 空になった理由。列挙のワーカーを持つ経路 (PDF / 書庫) が対象。
///
/// 本当に 0 件だった場合はここへ来ない。**理由が付かない空は「本当に空」**という
/// 対応を保つため、成功して 0 件になった経路では設定しないこと。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EmptyItemsReason {
    /// PDF のページ列挙ワーカーが、結果を返さないまま切れた。
    PdfWorkerLost,
    /// PDF のページ列挙が失敗した。`detail` はワーカー由来の生メッセージ。
    PdfEnumerateFailed { detail: String },
    /// PDF にパスワードが必要で、仮に並べていたページを取り下げた。
    PdfPasswordRequired,
    /// 書庫の列挙ワーカーが、結果を返さないまま切れた。
    ZipWorkerLost,
    /// 書庫の列挙が失敗した。
    ZipEnumerateFailed { detail: String },
}

impl EmptyItemsReason {
    /// 一覧の中央に出す文。
    ///
    /// 内部用語を出さない (CLAUDE.md「マニュアル・製品ページの記述方針」)。
    /// ワーカー由来の生メッセージは英語なのでここには載せず、ログにだけ残す。
    pub(crate) fn message(&self) -> &'static str {
        match self {
            Self::PdfWorkerLost | Self::ZipWorkerLost => {
                "読み込みが中断されました。開き直してください"
            }
            Self::PdfEnumerateFailed { .. } => "この PDF を読み込めませんでした",
            Self::PdfPasswordRequired => "この PDF を開くにはパスワードが必要です",
            Self::ZipEnumerateFailed { .. } => "この書庫を読み込めませんでした",
        }
    }

    /// ログ 1 行。利用者から送られたログはここを起点に切り分ける。
    pub(crate) fn log_line(&self) -> String {
        match self {
            Self::PdfWorkerLost => "empty items: pdf enumerate worker disconnected".to_string(),
            Self::PdfEnumerateFailed { detail } => {
                format!("empty items: pdf enumerate failed: {detail}")
            }
            Self::PdfPasswordRequired => {
                "empty items: pdf password required, placeholder pages withdrawn".to_string()
            }
            Self::ZipWorkerLost => "empty items: zip enumerate worker disconnected".to_string(),
            Self::ZipEnumerateFailed { detail } => {
                format!("empty items: zip enumerate failed: {detail}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空表示に出す文へ、ワーカー由来の生メッセージを混ぜない。
    #[test]
    fn the_raw_worker_detail_stays_out_of_the_visible_message() {
        let reason = EmptyItemsReason::PdfEnumerateFailed {
            detail: "PdfiumLibraryInternalError(Unknown)".to_string(),
        };
        assert!(!reason.message().contains("Pdfium"));
        assert!(
            reason
                .log_line()
                .contains("PdfiumLibraryInternalError(Unknown)")
        );
    }

    /// 中断と失敗は別の文にする。前者は開き直せば直る可能性があり、
    /// 次の行動が違う。
    #[test]
    fn an_interrupted_read_reads_differently_from_a_failed_one() {
        assert_ne!(
            EmptyItemsReason::PdfWorkerLost.message(),
            EmptyItemsReason::PdfEnumerateFailed {
                detail: String::new()
            }
            .message()
        );
    }
}
