//! YouTube コメント等のチャプター行 (`hh:mm:ss タイトル` / `mm:ss タイトル`) を
//! `(pts_secs, title)` のリストにパースする純関数モジュール。
//!
//! 一括ブックマーク登録ダイアログから呼ばれる。1 行 1 件、空行は skip。
//! YouTube からのコピペで時刻部分が `[0:13](https://...)` のように markdown
//! リンクになっていても拾えるようにする。

/// 1 行から `(pts_secs, title)` を抽出。
///
/// - 行頭の空白・装飾 (markdown link の `[` や箇条書きの `-` `*` `・`) を許容。
/// - 時刻は `H:MM:SS` (1 桁 hour も可) / `M:SS` / `MM:SS` を受け付ける。
///   秒・分は 2 桁を想定するが、`5:3` (= 5 分 3 秒) のような片側 1 桁も許容する
///   (= ペースト元の表記揺れ吸収を優先)。
/// - 時刻の後の空白を消費してから、残りをタイトルとして trim する。
///   タイトルは空でも OK (時刻だけ並べた場合に対応)。
/// - `]` `)` などの markdown link 末尾装飾はタイトル先頭から取り除く。
pub fn parse_chapter_line(line: &str) -> Option<(f64, String)> {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    // 行頭装飾を 1 文字ずつ落とす (markdown link / 箇条書き / 全角の中黒など)。
    let after_lead = strip_leading_decorations(trimmed);

    let mut chars = after_lead.char_indices().peekable();
    // 時刻部分の終端を探す。連続する数字・コロン・小数点 (`.mmm` ミリ秒) を拾う。
    // 小数点は最終秒セグメントだけ許す前提だが、loop ではゆるく受け、
    // `parse_time_token` で「. が複数 or h/m に . が混じる」ケースを reject する。
    let time_start = chars.peek().map(|(i, _)| *i)?;
    let mut time_end = time_start;
    let mut saw_digit = false;
    let mut saw_colon = false;
    while let Some(&(i, c)) = chars.peek() {
        if c.is_ascii_digit() || c == ':' || c == '.' {
            if c == ':' {
                saw_colon = true;
            } else if c.is_ascii_digit() {
                saw_digit = true;
            }
            time_end = i + c.len_utf8();
            chars.next();
        } else {
            break;
        }
    }
    if !saw_digit || !saw_colon {
        return None;
    }
    let time_str = &after_lead[time_start..time_end];
    let pts_secs = parse_time_token(time_str)?;

    // 時刻直後の閉じ装飾 (markdown link の `](url)` 部分) を skip してタイトルへ。
    let rest = &after_lead[time_end..];
    let title = extract_title_from_rest(rest);
    Some((pts_secs, title))
}

/// 行頭の markdown link 開始 (`[`) や箇条書き記号 (`-`, `*`, `・`, `•`, タブ, 全角空白)
/// を取り除く。時刻トークンに到達した時点で止める。
fn strip_leading_decorations(s: &str) -> &str {
    let mut idx = 0;
    for (i, c) in s.char_indices() {
        match c {
            '[' | '-' | '*' | '・' | '•' | '\u{3000}' | '\t' | ' ' => {
                idx = i + c.len_utf8();
            }
            _ => return &s[idx..],
        }
    }
    &s[idx..]
}

/// 時刻トークン直後の残り文字列からタイトルを取り出す。
///
/// 形式:
/// - `](url) タイトル` (markdown link): URL 終端の `)` を見つけて、その後ろを title に。
/// - `] タイトル` (bracket だけ、URL なし): `]` を 1 文字 skip して残りを title に。
/// - 通常の空白区切り: trim だけ。
///
/// URL 終端の判定 (Codex P2/P3 2026-05-24):
/// - markdown link 内の URL は仕様上 `(...)` をエスケープせず含めることがあるので、
///   **括弧バランス** で判定する。`after_open` は最初の `(` の直後から始まるので、
///   仮想的に depth=0 でスキャンし、`(` で +1 / `)` で -1。depth が -1 になった
///   `)` が外側の link を閉じる箇所。
/// - これで以下を全て正しく扱える:
///   - `(https://e.com/foo(bar)) Title`        → URL=`...foo(bar)`, title=`Title`
///   - `(https://e.com/?t=13) Track (Live)`    → URL=`...?t=13`, title=`Track (Live)`
///   - `(https://e.com/?t=13)メインテーマ`     → URL=`...?t=13`, title=`メインテーマ`
///     (Codex P3: `)` 直後にスペースなしで日本語タイトルが続く形に対応)
///   - `(https://e.com/foo)`                    → URL=`...foo`, title=``
/// - 閉じ括弧が見つからない (壊れたリンク) ときは rest をそのまま title 候補に使う。
fn extract_title_from_rest(rest: &str) -> String {
    let trimmed = if let Some(stripped) = rest.strip_prefix(']') {
        if let Some(after_open) = stripped.strip_prefix('(') {
            if let Some(end_idx) = find_url_close_paren(after_open) {
                &after_open[end_idx + 1..]
            } else {
                // 閉じ括弧がない (壊れたリンク): rest をそのまま使って復旧。
                stripped
            }
        } else {
            // `] Title` 形式 (URL なし): `]` だけ skip。
            stripped
        }
    } else {
        rest
    };
    let mut t = trimmed.trim().to_string();
    // `- タイトル` のような余分なセパレータを行頭から落とす。
    // `:` / `：` (全角コロン) を含めるかどうか: 全角コロンは日本語コンテンツの
    // 内容マーカーであることが多い (例: 「：序章」) ので strip 対象から外し、
    // ASCII `:` だけは table-of-contents の区切りで使われるので残す。
    while let Some(c) = t.chars().next() {
        if matches!(c, '-' | '–' | '—' | '|' | '/' | ':') {
            let new_start = c.len_utf8();
            t = t[new_start..].trim_start().to_string();
        } else {
            break;
        }
    }
    t
}

/// markdown link の URL 部分 (= 最初の `(` の直後から始まる文字列) を受け取り、
/// **外側の `)` を閉じる箇所** の byte index を返す。
///
/// 括弧バランスで判定: depth=0 でスキャンし、`(` で +1 / `)` で -1。
/// depth が -1 になった `)` の位置が「`](url)` の閉じカッコ」。
///
/// 例:
/// - `https://e.com/foo)`: depth が `)` で -1 → 17 を返す
/// - `https://e.com/foo(bar)) X`: `(` で +1, `)` で 0, `)` で -1 → 22 を返す
/// - `https://e.com/?t=13)メインテーマ`: 初回 `)` で -1 → 19 を返す
///
/// URL 内に閉じすぎ (`)` が `(` より多い) になった瞬間、それを終端と認定する
/// 設計なので、URL は仕様上 paren を balance させる前提となるが、現実の URL も
/// (バランス済みの方が普通なので) この前提でほぼ問題ない。
///
/// 閉じ括弧が見つからない (壊れたリンク) なら None。
fn find_url_close_paren(s: &str) -> Option<usize> {
    let mut depth: i32 = 0;
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => {
                if depth == 0 {
                    return Some(i);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// `H:M:S[.mmm]` / `M:S[.mmm]` 形式を秒に変換。失敗時 None。
///
/// 小数点は **最終 (秒) セグメントだけ** 許す:
/// - OK: `0:13.245` / `1:00:08.500` / `0:13` (小数なし)
/// - NG: `1.5:30` (分に小数) / `0:13.245.5` (二重小数) / `0:.5` (秒の整数部欠落)
fn parse_time_token(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    // 時・分の整数パース。空文字や `.` を含む文字列は弾く。
    let parse_u64 = |p: &str| -> Option<u64> {
        if p.is_empty() {
            return None;
        }
        p.parse().ok()
    };
    let (h, m, sec_part) = match parts.len() {
        2 => (0u64, parse_u64(parts[0])?, parts[1]),
        3 => (parse_u64(parts[0])?, parse_u64(parts[1])?, parts[2]),
        _ => unreachable!(),
    };
    // 秒セグメントは `SS` または `SS.mmm` を受ける。`.` は最大 1 つ。
    // 整数部・小数部はそれぞれ必須 (空 NG)。
    if sec_part.is_empty() {
        return None;
    }
    let (sec_int_str, frac_secs) = match sec_part.split_once('.') {
        Some((int_str, frac_str)) => {
            if int_str.is_empty() || frac_str.is_empty() || frac_str.contains('.') {
                return None;
            }
            // 小数部は ASCII digit のみ許可 (parse::<f64> は 'e' / '+' / '_' などを
            // 受けてしまうので明示弾く)。
            if !frac_str.chars().all(|c| c.is_ascii_digit()) {
                return None;
            }
            let frac: f64 = format!("0.{frac_str}").parse().ok()?;
            (int_str, frac)
        }
        None => (sec_part, 0.0),
    };
    let s_int: u64 = parse_u64(sec_int_str)?;
    // 分・秒は 60 未満であるべきだが、表記揺れを許容するため緩く check。
    // 明らかに無効 (= 1000 秒以上の分秒) なら弾く。
    if m >= 1000 || s_int >= 1000 {
        return None;
    }
    Some(h as f64 * 3600.0 + m as f64 * 60.0 + s_int as f64 + frac_secs)
}

/// 1 件分のパース結果。
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEntry {
    pub pts_secs: f64,
    pub title: String,
}

/// テキスト全体をパース。空行は skip、不正行は `error_lines` に行番号 (1-based) で
/// 追加する。dialog の preview / エラー表示用。
pub fn parse_chapter_text(text: &str) -> (Vec<ParsedEntry>, Vec<usize>) {
    let mut entries = Vec::new();
    let mut error_lines = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match parse_chapter_line(line) {
            Some((pts_secs, title)) => entries.push(ParsedEntry { pts_secs, title }),
            None => error_lines.push(i + 1),
        }
    }
    (entries, error_lines)
}

/// `(pts_secs, title)` のリストを `parse_chapter_text` が解釈できる行フォーマット
/// (`mm:ss タイトル` / `h:mm:ss タイトル`) へ整形する。クリップボードエクスポート用。
///
/// - `seconds_only=true`: 秒は **floor** (= 切り捨て) で整数化。動画コメント欄等の
///   自動リンク化パーサがミリ秒表記を timestamp として認識しないので、互換性優先のときに使う。
///   切り捨てを選ぶのは「実際の発生位置より遅れない (= リンクを踏んで本当の頭出し位置を
///   過ぎてしまうことがない)」ためで、四捨五入だと最大 500ms 遅れる可能性がある。
/// - `seconds_only=false`: 小数第 3 位 (= ms 精度) まで保持。mIV 内のラウンドトリップ用。
/// - 1 時間未満は `mm:ss`、1 時間以上は `h:mm:ss` (UI のヒント記法と揃える)。
/// - タイトルが `None` または空白のみのときは時刻だけの行になる。
///
/// 各行は `\n` 区切り、末尾改行なし。
pub fn format_chapter_lines(entries: &[(f64, Option<String>)], seconds_only: bool) -> String {
    let mut out = String::new();
    for (i, (pts_secs, title)) in entries.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format_time_token(*pts_secs, seconds_only));
        if let Some(t) = title {
            let t = t.trim();
            if !t.is_empty() {
                out.push(' ');
                out.push_str(t);
            }
        }
    }
    out
}

/// `format_chapter_lines` の時刻成形ヘルパー。負値は 0 にクランプ。
fn format_time_token(pts_secs: f64, seconds_only: bool) -> String {
    let secs = if pts_secs.is_finite() && pts_secs > 0.0 {
        pts_secs
    } else {
        0.0
    };
    if seconds_only {
        let total = secs.floor() as u64;
        let h = total / 3600;
        let m = (total % 3600) / 60;
        let s = total % 60;
        if h > 0 {
            format!("{h}:{m:02}:{s:02}")
        } else {
            format!("{m}:{s:02}")
        }
    } else {
        // 小数第 3 位までを保持。SQLite に入った double を全て小数で吐いてしまうと冗長
        // (例: `0:13.000`) なので、ミリ秒が 0 のとき小数部を省略する。
        let total_ms = (secs * 1000.0).round() as u64;
        let h = total_ms / 3_600_000;
        let m = (total_ms % 3_600_000) / 60_000;
        let s_int = (total_ms % 60_000) / 1000;
        let ms = total_ms % 1000;
        let body = if h > 0 {
            format!("{h}:{m:02}:{s_int:02}")
        } else {
            format!("{m}:{s_int:02}")
        };
        if ms == 0 {
            body
        } else {
            format!("{body}.{ms:03}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mm_ss() {
        let (pts, title) = parse_chapter_line("0:13 メインテーマ").unwrap();
        assert!((pts - 13.0).abs() < 1e-9);
        assert_eq!(title, "メインテーマ");
    }

    #[test]
    fn parses_h_mm_ss() {
        let (pts, title) = parse_chapter_line("1:00:08 魏々たる丹砂").unwrap();
        assert!((pts - 3608.0).abs() < 1e-9);
        assert_eq!(title, "魏々たる丹砂");
    }

    #[test]
    fn parses_hh_mm_ss() {
        let (pts, _) = parse_chapter_line("12:34:56 something").unwrap();
        assert!((pts - (12.0 * 3600.0 + 34.0 * 60.0 + 56.0)).abs() < 1e-9);
    }

    #[test]
    fn double_space_between_time_and_title_ok() {
        let (pts, title) = parse_chapter_line("12:45  希望の航路").unwrap();
        assert!((pts - 765.0).abs() < 1e-9);
        assert_eq!(title, "希望の航路");
    }

    #[test]
    fn handles_markdown_link_time() {
        let (pts, title) =
            parse_chapter_line("[0:13](https://example.com/?t=13) メインテーマ").unwrap();
        assert!((pts - 13.0).abs() < 1e-9);
        assert_eq!(title, "メインテーマ");
    }

    #[test]
    fn handles_bracket_only_time() {
        // Codex C12: `[mm:ss] Title` (markdown link ではなく単純な bracket)
        let (pts, title) = parse_chapter_line("[12:34] チャプター名").unwrap();
        assert!((pts - 754.0).abs() < 1e-9);
        assert_eq!(title, "チャプター名");
    }

    #[test]
    fn handles_bracket_only_time_h_mm_ss() {
        let (pts, title) = parse_chapter_line("[1:00:08] 魏々たる丹砂").unwrap();
        assert!((pts - 3608.0).abs() < 1e-9);
        assert_eq!(title, "魏々たる丹砂");
    }

    #[test]
    fn handles_markdown_link_with_paren_in_url() {
        // Codex C11: URL に括弧が含まれていても title が削れない。
        let (pts, title) =
            parse_chapter_line("[0:13](https://example.com/foo(bar)) メインテーマ").unwrap();
        assert!((pts - 13.0).abs() < 1e-9);
        assert_eq!(title, "メインテーマ");
    }

    #[test]
    fn handles_markdown_link_with_query_paren() {
        let (pts, title) =
            parse_chapter_line("[2:13](https://example.com/?t=133&s=(a)b) Track").unwrap();
        assert!((pts - 133.0).abs() < 1e-9);
        assert_eq!(title, "Track");
    }

    #[test]
    fn preserves_parens_in_title_text() {
        // Codex P2 2026-05-24: URL の `)` とタイトルの `)` を区別するため
        // 「空白前の `)`」を URL 終端と判定する。タイトル末尾の (Live) が壊れない。
        let (pts, title) =
            parse_chapter_line("[0:13](https://example.com/?t=13) Track (Live)").unwrap();
        assert!((pts - 13.0).abs() < 1e-9);
        assert_eq!(title, "Track (Live)");
    }

    #[test]
    fn preserves_parens_in_title_with_url_paren() {
        // URL とタイトル両方に括弧があるケース
        let (pts, title) =
            parse_chapter_line("[5:00](https://example.com/path(v2)) Acoustic (Live Mix)").unwrap();
        assert!((pts - 300.0).abs() < 1e-9);
        assert_eq!(title, "Acoustic (Live Mix)");
    }

    #[test]
    fn markdown_link_no_title_no_trailing_space() {
        // URL の `)` の直後に文字列末尾が来る場合 (タイトル無し)
        let (pts, title) = parse_chapter_line("[0:13](https://example.com/foo)").unwrap();
        assert!((pts - 13.0).abs() < 1e-9);
        assert_eq!(title, "");
    }

    #[test]
    fn markdown_link_immediately_followed_by_japanese_title() {
        // Codex P3 2026-05-24: `)` 直後にスペースなしで日本語タイトルが続く形 (実例として
        // 日本語コメント由来でよく見る)。「) 直後が空白」判定だと壊れる。
        let (pts, title) =
            parse_chapter_line("[0:13](https://example.com/?t=13)メインテーマ").unwrap();
        assert!((pts - 13.0).abs() < 1e-9);
        assert_eq!(title, "メインテーマ");
    }

    #[test]
    fn markdown_link_japanese_title_with_url_inner_paren() {
        // URL 内括弧 + 直後に日本語タイトル
        let (pts, title) =
            parse_chapter_line("[12:34](https://example.com/foo(bar))チャプター").unwrap();
        assert!((pts - 754.0).abs() < 1e-9);
        assert_eq!(title, "チャプター");
    }

    #[test]
    fn preserves_fullwidth_colon_in_title() {
        // Codex C12 / レビューでの懸念: 全角コロンは内容マーカーとして残す。
        let (pts, title) = parse_chapter_line("0:00 ：序章「目覚め」").unwrap();
        assert!((pts - 0.0).abs() < 1e-9);
        assert_eq!(title, "：序章「目覚め」");
    }

    #[test]
    fn handles_bullet_prefix() {
        let (pts, title) = parse_chapter_line("- 5:00 Track 5").unwrap();
        assert!((pts - 300.0).abs() < 1e-9);
        assert_eq!(title, "Track 5");
    }

    #[test]
    fn handles_separator_after_time() {
        let (pts, title) = parse_chapter_line("0:00 - Intro").unwrap();
        assert!((pts - 0.0).abs() < 1e-9);
        assert_eq!(title, "Intro");
    }

    #[test]
    fn empty_line_returns_none() {
        assert!(parse_chapter_line("").is_none());
        assert!(parse_chapter_line("   ").is_none());
    }

    #[test]
    fn no_colon_returns_none() {
        assert!(parse_chapter_line("just text").is_none());
    }

    #[test]
    fn title_can_be_empty() {
        let (pts, title) = parse_chapter_line("0:00").unwrap();
        assert!((pts - 0.0).abs() < 1e-9);
        assert_eq!(title, "");
    }

    #[test]
    fn full_youtube_block_parses_in_order() {
        let block = "0:13 メインテーマ\n\
                     2:13 希望に満ちるアナザーデイ\n\
                     1:00:08 魏々たる丹砂\n\
                     1:12:32 エンディング\n";
        let (entries, errors) = parse_chapter_text(block);
        assert!(
            errors.is_empty(),
            "no parse errors expected, got {errors:?}"
        );
        assert_eq!(entries.len(), 4);
        assert!((entries[0].pts_secs - 13.0).abs() < 1e-9);
        assert_eq!(entries[0].title, "メインテーマ");
        assert!((entries[2].pts_secs - 3608.0).abs() < 1e-9);
        assert_eq!(entries[3].title, "エンディング");
    }

    #[test]
    fn parse_text_reports_error_line_numbers() {
        let block = "0:00 Intro\n\
                     not a chapter\n\
                     1:00 Track\n";
        let (entries, errors) = parse_chapter_text(block);
        assert_eq!(entries.len(), 2);
        assert_eq!(errors, vec![2]);
    }

    #[test]
    fn parse_text_skips_blank_lines() {
        let block = "\n0:00 A\n\n\n1:00 B\n\n";
        let (entries, errors) = parse_chapter_text(block);
        assert!(errors.is_empty());
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn rejects_huge_minutes() {
        assert!(parse_chapter_line("9999:00 broken").is_none());
    }

    #[test]
    fn user_provided_real_data_parses() {
        // ユーザー提示のリスト全 28 件を網羅 (見つけにくい trailing space や全角空白も含む)。
        let block = "\
0:13 メインテーマ
2:13 希望に満ちるアナザーデイ
4:17 アカツキワイナリー
7:08 彷徨する輝き
10:27 遥かなる憂慮
12:45  希望の航路
15:53 落葉風波
17:51 異郷の櫻
19:25 華散る夢
21:44 空を翔ける不羈
25:50 失っていない記憶
28:47 満開の焔硝
30:42 妄念と執念
33:26 凪いだ心
35:19 無垢の歌
37:00 勝利に向けて
38:49 燼滅の舞
42:05 消滅した記憶
44:24 祈望と故郷の魂
46:38  烈火のごとく
51:30 神女劈観
54:10 野に響く鶴の音
57:33 真意を綴りし虹章
1:00:08 魏々たる丹砂
1:02:06 華やかな灯火、星々の如く
1:06:33 キャラバン宿駅
1:08:23 スメールシティ
1:10:41 スメールシティ
1:12:32 エンディング
";
        let (entries, errors) = parse_chapter_text(block);
        assert!(errors.is_empty(), "parse errors: {errors:?}");
        assert_eq!(entries.len(), 29);
        // 順序が壊れていないか (秒数が単調増加)
        for w in entries.windows(2) {
            assert!(w[0].pts_secs < w[1].pts_secs, "non-monotonic at {w:?}");
        }
        // 末尾要素は 1:12:32 = 4352 秒
        assert!((entries.last().unwrap().pts_secs - 4352.0).abs() < 1e-9);
        assert_eq!(entries.last().unwrap().title, "エンディング");
        // trailing space を含む 1:00:08 のタイトルは trim されている
        let entry_3608 = entries
            .iter()
            .find(|e| (e.pts_secs - 3608.0).abs() < 1e-9)
            .unwrap();
        assert_eq!(entry_3608.title, "魏々たる丹砂");
    }

    #[test]
    fn format_seconds_only_under_one_hour() {
        let s = format_chapter_lines(
            &[
                (13.0, Some("メインテーマ".to_string())),
                (133.0, Some("希望".to_string())),
            ],
            true,
        );
        assert_eq!(s, "0:13 メインテーマ\n2:13 希望");
    }

    #[test]
    fn format_seconds_only_over_one_hour() {
        let s = format_chapter_lines(&[(3608.0, Some("魏々たる丹砂".to_string()))], true);
        assert_eq!(s, "1:00:08 魏々たる丹砂");
    }

    #[test]
    fn format_seconds_only_floors_milliseconds() {
        // 13.245 / 13.999 / 60.5 はすべて秒単位 floor で 13 / 13 / 60 になる。
        let s = format_chapter_lines(
            &[
                (13.245, Some("a".to_string())),
                (13.999, Some("b".to_string())),
                (60.5, Some("c".to_string())),
            ],
            true,
        );
        assert_eq!(s, "0:13 a\n0:13 b\n1:00 c");
    }

    #[test]
    fn format_precise_preserves_milliseconds() {
        let s = format_chapter_lines(
            &[
                (13.245, Some("a".to_string())),
                (60.5, Some("b".to_string())),
            ],
            false,
        );
        assert_eq!(s, "0:13.245 a\n1:00.500 b");
    }

    #[test]
    fn format_precise_drops_zero_milliseconds() {
        // ミリ秒が 0 のときは小数部を省略 (= 整数秒と区別がつくが冗長な `.000` を避ける)。
        let s = format_chapter_lines(&[(13.0, Some("a".to_string()))], false);
        assert_eq!(s, "0:13 a");
    }

    #[test]
    fn format_no_title_emits_time_only() {
        let s = format_chapter_lines(&[(13.0, None), (60.0, Some(String::new()))], true);
        assert_eq!(s, "0:13\n1:00");
    }

    #[test]
    fn format_whitespace_title_trimmed_to_empty() {
        // タイトルが空白のみのときは時刻だけの行 (= ` 後ろに無駄な空白を残さない)。
        let s = format_chapter_lines(&[(13.0, Some("  ".to_string()))], true);
        assert_eq!(s, "0:13");
    }

    #[test]
    fn format_empty_input_returns_empty_string() {
        assert_eq!(format_chapter_lines(&[], true), "");
        assert_eq!(format_chapter_lines(&[], false), "");
    }

    #[test]
    fn format_negative_or_nonfinite_clamped_to_zero() {
        let s = format_chapter_lines(&[(-5.0, Some("neg".to_string())), (f64::NAN, None)], true);
        assert_eq!(s, "0:00 neg\n0:00");
    }

    #[test]
    fn format_round_trips_through_parser_seconds_only() {
        // 秒単位で整形 → パースで元の秒に戻る (タイトルも保持)。
        let entries = vec![
            (13.0, Some("メインテーマ".to_string())),
            (133.0, Some("希望".to_string())),
            (3608.0, Some("魏々たる丹砂".to_string())),
        ];
        let text = format_chapter_lines(&entries, true);
        let (parsed, errors) = parse_chapter_text(&text);
        assert!(errors.is_empty(), "round-trip errors: {errors:?}");
        assert_eq!(parsed.len(), entries.len());
        for (e, p) in entries.iter().zip(parsed.iter()) {
            assert!((e.0 - p.pts_secs).abs() < 1e-9);
            assert_eq!(e.1.as_deref().unwrap_or(""), p.title);
        }
    }

    // --- 小数秒対応 (Codex P2 2026-05-24) ---

    #[test]
    fn parses_mm_ss_with_milliseconds() {
        let (pts, title) = parse_chapter_line("0:13.245 a").unwrap();
        assert!((pts - 13.245).abs() < 1e-9);
        assert_eq!(title, "a");
    }

    #[test]
    fn parses_h_mm_ss_with_milliseconds() {
        let (pts, title) = parse_chapter_line("1:00:08.500 タイトル").unwrap();
        assert!((pts - 3608.5).abs() < 1e-9);
        assert_eq!(title, "タイトル");
    }

    #[test]
    fn parses_mm_ss_with_milliseconds_no_title() {
        let (pts, title) = parse_chapter_line("0:13.245").unwrap();
        assert!((pts - 13.245).abs() < 1e-9);
        assert_eq!(title, "");
    }

    #[test]
    fn parses_markdown_link_with_fractional_seconds() {
        let (pts, title) =
            parse_chapter_line("[0:13.245](https://example.com/?t=13) メインテーマ").unwrap();
        assert!((pts - 13.245).abs() < 1e-9);
        assert_eq!(title, "メインテーマ");
    }

    #[test]
    fn rejects_decimal_in_minutes_segment() {
        // 分セグメントへの `.` 混入は数字でも reject (`1.5:30` のような不正形式)。
        assert!(parse_chapter_line("1.5:30 something").is_none());
    }

    #[test]
    fn rejects_decimal_in_hours_segment() {
        assert!(parse_chapter_line("1.5:00:30 something").is_none());
    }

    #[test]
    fn rejects_double_decimal_in_seconds() {
        // 秒セグメントに `.` が 2 つ以上あるのは reject。
        assert!(parse_chapter_line("0:13.245.5 something").is_none());
    }

    #[test]
    fn rejects_empty_seconds_integer_part() {
        // `0:.5` のような秒の整数部欠落は reject。
        assert!(parse_chapter_line("0:.5 something").is_none());
    }

    #[test]
    fn rejects_empty_seconds_fractional_part() {
        // `0:13.` (小数点だけ) は reject。
        assert!(parse_chapter_line("0:13. something").is_none());
    }

    #[test]
    fn format_round_trips_through_parser_precise() {
        // 精密モードで整形 → パースで元の pts_secs に戻る (タイトルも保持)。
        let entries = vec![
            (13.0, Some("整数秒".to_string())), // ms=0 → 小数部省略
            (13.245, Some("ミリ秒精度".to_string())),
            (60.5, Some("ハーフ秒".to_string())),
            (3608.001, Some("時間越え 1ms".to_string())),
        ];
        let text = format_chapter_lines(&entries, false);
        let (parsed, errors) = parse_chapter_text(&text);
        assert!(errors.is_empty(), "precise round-trip errors: {errors:?}");
        assert_eq!(parsed.len(), entries.len());
        for (e, p) in entries.iter().zip(parsed.iter()) {
            // 整形時に小数第 3 位へ round しているので、許容誤差は 0.5ms 程度。
            assert!(
                (e.0 - p.pts_secs).abs() < 0.001,
                "expected {} got {}",
                e.0,
                p.pts_secs
            );
            assert_eq!(e.1.as_deref().unwrap_or(""), p.title);
        }
    }
}
