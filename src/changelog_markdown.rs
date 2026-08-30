//! 更新履歴 (GitHub release body) の Markdown サブセット描画。
//!
//! egui には組み込み Markdown レンダラが無いため、リリースノートで実際に使う
//! 最小サブセットだけを自前でパースして描画する:
//!
//! - `### 見出し` / `## 見出し`
//! - `- 箇条書き` / `* 箇条書き` (先頭空白で 1 段ぶら下げを深くする)
//! - `**強調**` / `` `インラインコード` `` / `<kbd>キー</kbd>`
//! - 空行は段落の区切り
//!
//! バージョン更新ダイアログ ([crate::ui_dialogs] の `update_notice`) から呼ばれる。
//! UI から切り出してあるので `egui_kittest` でスナップショットテストできる
//! ([tests/ui_snapshot.rs] の `changelog_markdown_*`)。

/// 本文テキストのフォントサイズ (px)。
const BODY_TEXT_SIZE: f32 = 12.5;

/// インライン要素 1 つ分。`render_inline_wrapped` が `horizontal_wrapped` で並べる。
#[derive(Debug, PartialEq)]
enum Seg {
    /// 通常テキスト (`bold` が true なら `**強調**` 由来)
    Text { text: String, bold: bool },
    /// `` `inline code` ``
    Code(String),
    /// `<kbd>キー</kbd>` — キーキャップ風のチップで描く
    Kbd(String),
}

/// 1 行分のテキストを `**強調**` / `` `コード` `` / `<kbd>キー</kbd>` で分割する。
///
/// 完全な Markdown パーサではなく、リリースノートで実際に出てくる記法だけを扱う。
/// 閉じが見つからない `**` / `` ` `` / `<kbd>` はそのまま文字として残す
/// (壊れた表示にしない)。release body は 8KB で打ち切られるため、強調スパンの
/// 途中で切れて閉じ `**` を失うケースが実際に起こりうる。
fn parse_inline(input: &str) -> Vec<Seg> {
    let mut segs: Vec<Seg> = Vec::new();
    let mut buf = String::new();
    let mut bold = false;
    let mut s = input;

    loop {
        let p_bold = s.find("**");
        let p_code = s.find('`');
        let p_kbd = s.find("<kbd>");
        let next = [p_bold, p_code, p_kbd].into_iter().flatten().min();
        let Some(pos) = next else {
            buf.push_str(s);
            break;
        };
        buf.push_str(&s[..pos]);

        if Some(pos) == p_bold {
            // 強調中なら閉じ `**`。そうでなければ、対になる `**` が後続にある場合のみ
            // 開始扱いにする (対が無い `**` はリテラルとして残す)。
            if bold || s[pos + 2..].contains("**") {
                if !buf.is_empty() {
                    segs.push(Seg::Text {
                        text: std::mem::take(&mut buf),
                        bold,
                    });
                }
                bold = !bold;
            } else {
                buf.push_str("**");
            }
            s = &s[pos + 2..];
        } else if Some(pos) == p_code {
            if let Some(end) = s[pos + 1..].find('`') {
                if !buf.is_empty() {
                    segs.push(Seg::Text {
                        text: std::mem::take(&mut buf),
                        bold,
                    });
                }
                segs.push(Seg::Code(s[pos + 1..pos + 1 + end].to_string()));
                s = &s[pos + 1 + end + 1..];
            } else {
                // 閉じバッククォートが無い: リテラル扱いで前進
                buf.push('`');
                s = &s[pos + 1..];
            }
        } else {
            // `<kbd>`
            if let Some(end) = s[pos + 5..].find("</kbd>") {
                if !buf.is_empty() {
                    segs.push(Seg::Text {
                        text: std::mem::take(&mut buf),
                        bold,
                    });
                }
                segs.push(Seg::Kbd(s[pos + 5..pos + 5 + end].to_string()));
                s = &s[pos + 5 + end + 6..];
            } else {
                buf.push_str("<kbd>");
                s = &s[pos + 5..];
            }
        }
    }
    if !buf.is_empty() {
        segs.push(Seg::Text { text: buf, bold });
    }
    segs
}

/// GitHub release body (Markdown サブセット) を読みやすく描画する。
pub fn render(ui: &mut egui::Ui, body: &str) {
    for raw_line in body.lines() {
        let line = raw_line.trim_end();
        let trimmed = line.trim_start();

        if trimmed.is_empty() {
            ui.add_space(5.0);
            continue;
        }

        // 見出し (`### ` / `## `)。release body にバージョン見出しが残っている場合に効く。
        if let Some(rest) = trimmed
            .strip_prefix("### ")
            .or_else(|| trimmed.strip_prefix("## "))
        {
            ui.add_space(6.0);
            ui.label(egui::RichText::new(rest).size(15.0).strong());
            ui.add_space(3.0);
            continue;
        }

        // 箇条書き (`- ` / `* `)。先頭の空白でぶら下げインデントを 1 段だけ深くする。
        if let Some(rest) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            let nested = line.len() - trimmed.len() >= 2;
            render_bullet(ui, rest, nested);
            ui.add_space(3.0);
            continue;
        }

        // 通常段落。
        render_inline_wrapped(ui, trimmed);
        ui.add_space(3.0);
    }
}

/// 箇条書き 1 項目。bullet マーカーと本文をぶら下げインデントで揃える。
fn render_bullet(ui: &mut egui::Ui, text: &str, nested: bool) {
    ui.horizontal_top(|ui| {
        ui.add_space(if nested { 20.0 } else { 4.0 });
        ui.label(
            egui::RichText::new("•")
                .size(BODY_TEXT_SIZE)
                .color(ui.visuals().weak_text_color()),
        );
        ui.add_space(5.0);
        // vertical 内に置くと残り幅で wrap され、折り返し行が bullet の右に揃う。
        ui.vertical(|ui| {
            render_inline_wrapped(ui, text);
        });
    });
}

/// インラインセグメント列を `horizontal_wrapped` で折り返し描画する。
fn render_inline_wrapped(ui: &mut egui::Ui, text: &str) {
    ui.horizontal_wrapped(|ui| {
        // セグメント境界の空白は文字列側に含めているので widget 間隔は 0 にする。
        ui.spacing_mut().item_spacing.x = 0.0;
        for seg in parse_inline(text) {
            match seg {
                Seg::Text { text, bold } => {
                    if text.is_empty() {
                        continue;
                    }
                    let mut rt = egui::RichText::new(text).size(BODY_TEXT_SIZE);
                    if bold {
                        rt = rt.strong();
                    }
                    ui.label(rt);
                }
                Seg::Code(t) => {
                    ui.label(egui::RichText::new(t).code().size(BODY_TEXT_SIZE - 1.0));
                }
                Seg::Kbd(t) => {
                    kbd_chip(ui, &t);
                }
            }
        }
    });
}

/// `<kbd>キー</kbd>` をキーキャップ風の枠付きチップで描く。
fn kbd_chip(ui: &mut egui::Ui, text: &str) {
    let dark = ui.visuals().dark_mode;
    let text_color = ui.visuals().strong_text_color();
    let galley = ui.painter().layout_no_wrap(
        text.to_owned(),
        egui::FontId::proportional(11.0),
        text_color,
    );
    let pad = egui::vec2(5.0, 2.5);
    let size = galley.size() + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let (bg, border) = if dark {
        (egui::Color32::from_gray(64), egui::Color32::from_gray(105))
    } else {
        (egui::Color32::from_gray(235), egui::Color32::from_gray(165))
    };
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, bg);
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, border),
        egui::StrokeKind::Inside,
    );
    painter.galley(rect.min + pad, galley, text_color);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// README の最新の更新履歴が、**この描画器で描ける記法だけ**でできていること。
    ///
    /// 更新通知ダイアログは GitHub release の body をここで描く。README の該当節が
    /// そのまま body になるので、対応していない記法 (リンク・画像・表) を書くと
    /// **そのまま文字として出る**。しかも気付くのは公開した後になる。
    ///
    /// これはリリース手順が「公開後に別マシンで目視」としていた確認の代わり
    /// (CLAUDE.md Phase 4)。目視は体裁しか見られず、しかも公開前には試せない。
    /// 閉じ忘れた `**` は描画器が**わざと文字のまま残す**ので、`Seg::Text` に
    /// マーカーが残っていればそれが検出結果になる。
    #[test]
    fn the_newest_changelog_entry_only_uses_markup_this_renderer_handles() {
        let readme = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/README.md"))
            .expect("README.md");
        let mut lines = readme.lines().skip_while(|l| !l.starts_with("## 更新履歴"));
        lines.next();
        let section: Vec<&str> = lines
            .skip_while(|l| !l.starts_with("### v"))
            .skip(1)
            .take_while(|l| !l.starts_with("### v"))
            .filter(|l| !l.trim().is_empty())
            .collect();
        assert!(
            section.len() >= 3,
            "最新の更新履歴が読めていない ({} 行)",
            section.len()
        );

        for line in section {
            let trimmed = line.trim_start();
            let body = trimmed
                .strip_prefix("- ")
                .or_else(|| trimmed.strip_prefix("* "))
                .unwrap_or(trimmed);
            for seg in parse_inline(body) {
                let Seg::Text { text, .. } = seg else {
                    continue;
                };
                for marker in ["**", "`", "<kbd", "</kbd", "](", "!["] {
                    assert!(
                        !text.contains(marker),
                        "描画器が解釈できない記法が残っている: {marker:?} in {text:?}
                         (対応しているのは **強調** / `コード` / <kbd>キー</kbd> と、
                          行頭の `### ` 見出し・`- ` 箇条書きだけ)"
                    );
                }
            }
        }
    }

    fn text(s: &str) -> Seg {
        Seg::Text {
            text: s.to_string(),
            bold: false,
        }
    }
    fn bold(s: &str) -> Seg {
        Seg::Text {
            text: s.to_string(),
            bold: true,
        }
    }

    #[test]
    fn plain_text_single_segment() {
        assert_eq!(parse_inline("ただのテキスト"), vec![text("ただのテキスト")]);
    }

    #[test]
    fn bold_run_splits() {
        assert_eq!(
            parse_inline("前 **強調** 後"),
            vec![text("前 "), bold("強調"), text(" 後"),]
        );
    }

    #[test]
    fn leading_bold_no_empty_segment() {
        // 行頭が `**` のとき空の Text を作らない (changelog の `- **機能名**: ...` 形)
        assert_eq!(
            parse_inline("**機能名**: 説明"),
            vec![bold("機能名"), text(": 説明"),]
        );
    }

    #[test]
    fn inline_code_segment() {
        assert_eq!(
            parse_inline("ファイル `settings.db` を移行"),
            vec![
                text("ファイル "),
                Seg::Code("settings.db".to_string()),
                text(" を移行"),
            ]
        );
    }

    #[test]
    fn kbd_combo() {
        assert_eq!(
            parse_inline("<kbd>Ctrl</kbd>+<kbd>S</kbd> で保存"),
            vec![
                Seg::Kbd("Ctrl".to_string()),
                text("+"),
                Seg::Kbd("S".to_string()),
                text(" で保存"),
            ]
        );
    }

    #[test]
    fn unclosed_markers_kept_literal() {
        // 閉じられていない `` ` `` / `<kbd>` は壊さずリテラルとして残す
        assert_eq!(
            parse_inline("素の `バッククォート"),
            vec![text("素の `バッククォート")]
        );
        assert_eq!(parse_inline("素の <kbd>タグ"), vec![text("素の <kbd>タグ")]);
    }

    #[test]
    fn unclosed_bold_kept_literal() {
        // 対の無い `**` はリテラル扱い。8KB 打ち切りで強調スパンの途中が切れても
        // それ以降が丸ごと bold にならないこと。
        assert_eq!(
            parse_inline("説明 **強調の途中で切断"),
            vec![text("説明 **強調の途中で切断")]
        );
        // 正常な対 + 末尾の余分な `**` は、対だけ強調にして残りはリテラル。
        assert_eq!(
            parse_inline("**機能**: 補足 **"),
            vec![bold("機能"), text(": 補足 **")]
        );
    }

    #[test]
    fn cjk_inside_kbd_is_utf8_safe() {
        // `<kbd>` の中身が CJK でも ASCII タグ境界で切るので panic しない
        assert_eq!(
            parse_inline("<kbd>右Ctrl</kbd> 長押し"),
            vec![Seg::Kbd("右Ctrl".to_string()), text(" 長押し"),]
        );
    }
}
