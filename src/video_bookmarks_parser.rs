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
    // 時刻部分の終端を探す。連続する数字とコロンを拾う。
    let time_start = chars.peek().map(|(i, _)| *i)?;
    let mut time_end = time_start;
    let mut saw_digit = false;
    let mut saw_colon = false;
    while let Some(&(i, c)) = chars.peek() {
        if c.is_ascii_digit() || c == ':' {
            if c == ':' {
                saw_colon = true;
            } else {
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
/// - `](url) タイトル` 形式: `]`...次の空白までを skip した残り。
/// - 通常の空白区切り: trim だけ。
fn extract_title_from_rest(rest: &str) -> String {
    let trimmed = if let Some(stripped) = rest.strip_prefix(']') {
        // markdown link: `](https://...) title` を想定。
        // `)` の後ろまでを取り除く。`)` が無ければ rest 全部を skip して空タイトル扱い。
        if let Some(paren_end) = stripped.find(')') {
            &stripped[paren_end + 1..]
        } else {
            ""
        }
    } else {
        rest
    };
    let mut t = trimmed.trim().to_string();
    // `- タイトル` のような余分なセパレータを行頭から落とす。
    while let Some(c) = t.chars().next() {
        if matches!(c, '-' | '–' | '—' | '|' | '/' | ':' | '：') {
            let new_start = c.len_utf8();
            t = t[new_start..].trim_start().to_string();
        } else {
            break;
        }
    }
    t
}

/// `H:M:S` / `M:S` 形式を秒に変換。失敗時 None。
fn parse_time_token(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split(':').collect();
    if !(2..=3).contains(&parts.len()) {
        return None;
    }
    let mut nums: Vec<u64> = Vec::with_capacity(parts.len());
    for p in &parts {
        if p.is_empty() {
            return None;
        }
        let n: u64 = p.parse().ok()?;
        nums.push(n);
    }
    let (h, m, s) = match nums.len() {
        2 => (0u64, nums[0], nums[1]),
        3 => (nums[0], nums[1], nums[2]),
        _ => unreachable!(),
    };
    // 分・秒は 60 未満であるべきだが、表記揺れを許容するため緩く check。
    // 明らかに無効 (= 1000 秒以上の分秒) なら弾く。
    if m >= 1000 || s >= 1000 {
        return None;
    }
    Some(h as f64 * 3600.0 + m as f64 * 60.0 + s as f64)
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
}
