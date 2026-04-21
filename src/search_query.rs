//! 検索クエリの共通パーサ。
//!
//! メタデータ検索 (Ctrl+F) とお気に入り検索 (Ctrl+S) の両方で共用する。
//!
//! 構文:
//! - スペース区切り = すべてのトークンを含むものがマッチ (AND)
//! - 先頭 `-` = そのトークンを含まないものがマッチ (NOT)
//! - `"..."` = クォートで囲むと中のスペースも含めて 1 トークンとして扱う
//! - `-"..."` = NOT + クォートの組み合わせも可
//! - 閉じクォートが無い場合はそのまま末尾までを 1 トークンとして扱う (寛容パース)
//!
//! トークンは `needle` を小文字化して保持する。照合は `matches` に生の hay を渡せば内部で小文字化される。

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// true: そのトークンを含むものだけ残す。false: 含むものを除外する。
    pub include: bool,
    /// 小文字化された照合対象文字列。空になるトークンは parse で捨てる。
    pub needle: String,
    /// true の場合は「タグ検索トークン」。`#タグ名` プレフィックスで入力された場合に立つ。
    /// needle には `#` 込みで入る (例: "#原神")。
    /// タグ検索トークンは all_text_norm に対する substring 検索ではなく、
    /// fts_meta.db の tags カラム (スペース区切り) に対する**完全一致**で判定される。
    pub is_tag: bool,
}

/// クエリ文字列を正負トークン列に分解する。空白のみ、または `-` 単体は無視する。
pub fn parse(query: &str) -> Vec<Token> {
    let chars: Vec<char> = query.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        // 先頭の空白をスキップ
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() {
            break;
        }

        // NOT プレフィックス ( `-X` で X が空白でない場合のみ )
        let mut include = true;
        if chars[i] == '-' {
            match chars.get(i + 1) {
                Some(&c) if !c.is_whitespace() => {
                    include = false;
                    i += 1;
                }
                _ => {
                    // 裸の `-` はノイズとしてスキップ
                    i += 1;
                    continue;
                }
            }
        }

        let mut buf = String::new();
        if i < chars.len() && chars[i] == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                buf.push(chars[i]);
                i += 1;
            }
            if i < chars.len() {
                i += 1;
            }
        } else {
            while i < chars.len() && !chars[i].is_whitespace() {
                buf.push(chars[i]);
                i += 1;
            }
        }

        let raw = buf.trim();
        // `#タグ名` プレフィックス判定: 先頭 `#` で、`#` 以降に 1 文字以上あるもの。
        // `#` 単体や `##...` は通常キーワード扱いにする (ユーザの意図が曖昧なため)。
        let is_tag = raw.starts_with('#') && raw.chars().count() >= 2 && !raw.starts_with("##");
        let needle = raw.to_lowercase();
        if !needle.is_empty() && needle != "-" {
            tokens.push(Token {
                include,
                needle,
                is_tag,
            });
        }
    }
    tokens
}

/// タグトークンと通常トークンを分割するヘルパ。
pub fn split_tokens<'a>(tokens: &'a [Token]) -> (Vec<&'a Token>, Vec<&'a Token>) {
    let (tags, keywords): (Vec<&Token>, Vec<&Token>) =
        tokens.iter().partition(|t| t.is_tag);
    (tags, keywords)
}

/// 指定 doc のタグ列 (スペース区切り、すべて小文字化前提) がタグトークンに一致するか判定。
/// - include タグトークン: tags に完全一致する要素があれば合致
/// - exclude タグトークン: tags に完全一致する要素があれば不一致
pub fn matches_tags(tag_tokens: &[&Token], tags_space_sep: &str) -> bool {
    if tag_tokens.is_empty() {
        return true;
    }
    let hay: Vec<&str> = tags_space_sep.split_whitespace().collect();
    for t in tag_tokens {
        let hit = hay.iter().any(|h| h.eq_ignore_ascii_case(&t.needle));
        if t.include && !hit {
            return false;
        }
        if !t.include && hit {
            return false;
        }
    }
    true
}

/// `hay` がトークン列にマッチするか判定する (内部で小文字化)。
/// - include トークン: hay に含まれなければ不一致
/// - exclude トークン: hay に含まれれば不一致
/// - タグトークン (`is_tag`): ここでは無視される (`matches_tags` で別途判定すべし)
/// - トークン列が空: 常に一致 (フィルタなしの扱い)
pub fn matches(tokens: &[Token], hay: &str) -> bool {
    if tokens.is_empty() {
        return true;
    }
    let hay_lower = hay.to_lowercase();
    for t in tokens {
        if t.is_tag {
            continue; // タグは matches_tags 側で判定する
        }
        if t.include {
            if !hay_lower.contains(&t.needle) {
                return false;
            }
        } else if hay_lower.contains(&t.needle) {
            return false;
        }
    }
    true
}

/// `decide_partial` の戻り値。追加情報 (XMP 等) を取得する必要があるかを示す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartialResult {
    /// 現在の hay のみで結果が確定した。追加情報を読む必要はない。
    Decided(bool),
    /// `include` トークンが hay に未発見、または `exclude` トークンが hay に
    /// 未発見 (= 追加情報に混入している可能性あり) のため、追加情報が必要。
    NeedsMore,
}

/// hay_so_far だけで確定判定できるかを返す。Ctrl+F メタデータ検索で、
/// 高コストな XMP 読み込みを「避けられる時は避ける」ための事前判定に使う。
///
/// 戻り値は以下 3 種類:
/// - `Decided(false)`: `exclude` トークンが hay_so_far に**含まれている** 。
///   追加情報に関わらず不一致確定なので XMP を読む必要なし。
/// - `Decided(true)`: 全 `include` トークンが hay_so_far に含まれ、かつ `exclude`
///   トークンが**1 つも無い** (クエリに `-X` が無い)。追加情報が増えても
///   結果は変わらないので XMP を読む必要なし。
/// - `NeedsMore`: 上記以外 — include が欠けているか、exclude が存在して未確認。
///   追加情報で結果が覆り得るので、追加情報を読んでから `matches` で再判定する。
pub fn decide_partial(tokens: &[Token], hay_so_far: &str) -> PartialResult {
    if tokens.is_empty() {
        return PartialResult::Decided(true);
    }
    let hay_lower = hay_so_far.to_lowercase();
    let mut any_include_missing = false;
    let mut has_exclude = false;
    for t in tokens {
        if t.include {
            if !hay_lower.contains(&t.needle) {
                any_include_missing = true;
            }
        } else {
            has_exclude = true;
            if hay_lower.contains(&t.needle) {
                return PartialResult::Decided(false);
            }
        }
    }
    if any_include_missing || has_exclude {
        // include が足りない場合は追加情報で補える可能性あり。
        // exclude が存在する場合は追加情報にも含まれていないかを確認する必要あり。
        PartialResult::NeedsMore
    } else {
        PartialResult::Decided(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inc(s: &str) -> Token {
        Token {
            include: true,
            needle: s.to_string(),
            is_tag: false,
        }
    }
    fn exc(s: &str) -> Token {
        Token {
            include: false,
            needle: s.to_string(),
            is_tag: false,
        }
    }
    fn tag(s: &str) -> Token {
        Token {
            include: true,
            needle: s.to_string(),
            is_tag: true,
        }
    }
    fn not_tag(s: &str) -> Token {
        Token {
            include: false,
            needle: s.to_string(),
            is_tag: true,
        }
    }

    #[test]
    fn parse_empty() {
        assert!(parse("").is_empty());
        assert!(parse("   ").is_empty());
    }

    #[test]
    fn parse_single() {
        assert_eq!(parse("hello"), vec![inc("hello")]);
    }

    #[test]
    fn parse_and_lowercases() {
        assert_eq!(parse("Hello WORLD"), vec![inc("hello"), inc("world")]);
    }

    #[test]
    fn parse_not() {
        assert_eq!(parse("foo -bar"), vec![inc("foo"), exc("bar")]);
    }

    #[test]
    fn parse_quoted_phrase() {
        assert_eq!(
            parse(r#"foo "hello world" bar"#),
            vec![inc("foo"), inc("hello world"), inc("bar")],
        );
    }

    #[test]
    fn parse_quoted_not() {
        assert_eq!(parse(r#"-"low quality""#), vec![exc("low quality")],);
    }

    #[test]
    fn parse_unterminated_quote() {
        // 閉じクォート無しは末尾までを 1 トークン
        assert_eq!(parse(r#""abc def"#), vec![inc("abc def")]);
    }

    #[test]
    fn parse_lone_dash_ignored() {
        // 裸の `-` はトークンにならない
        assert_eq!(parse("foo - bar"), vec![inc("foo"), inc("bar")]);
    }

    #[test]
    fn parse_dash_inside_word_kept() {
        // 単語中の `-` は NOT にならない (例: "jean-claude")
        assert_eq!(parse("jean-claude"), vec![inc("jean-claude")]);
    }

    #[test]
    fn matches_and() {
        let t = parse("foo bar");
        assert!(matches(&t, "foo xxx bar"));
        assert!(matches(&t, "barfoo"));
        assert!(!matches(&t, "foo only"));
        assert!(!matches(&t, "bar only"));
    }

    #[test]
    fn matches_not() {
        let t = parse("foo -bar");
        assert!(matches(&t, "foo alone"));
        assert!(!matches(&t, "foo bar together"));
    }

    #[test]
    fn matches_not_only() {
        // NOT-only query: bar を含まないものが全部一致
        let t = parse("-bar");
        assert!(matches(&t, "anything"));
        assert!(!matches(&t, "has bar in it"));
    }

    #[test]
    fn matches_phrase() {
        let t = parse(r#""hello world""#);
        assert!(matches(&t, "say hello world to me"));
        assert!(!matches(&t, "hello and world are apart"));
    }

    #[test]
    fn matches_empty_tokens() {
        // トークン 0 個は常にマッチ
        assert!(matches(&[], "anything"));
    }

    // ---- decide_partial ----

    #[test]
    fn decide_partial_all_includes_no_excludes() {
        // 全 include が hay にあり exclude なし → Decided(true)
        let t = parse("foo bar");
        assert_eq!(
            decide_partial(&t, "foo and bar"),
            PartialResult::Decided(true)
        );
    }

    #[test]
    fn decide_partial_include_missing() {
        // include が hay に欠けている → 追加情報で補えるかもしれないので NeedsMore
        let t = parse("foo bar");
        assert_eq!(decide_partial(&t, "only foo"), PartialResult::NeedsMore);
    }

    #[test]
    fn decide_partial_exclude_hit() {
        // exclude が hay に存在する時点で不一致確定 → Decided(false)
        let t = parse("foo -bad");
        assert_eq!(
            decide_partial(&t, "foo has bad"),
            PartialResult::Decided(false)
        );
    }

    #[test]
    fn decide_partial_exclude_not_found_yet() {
        // exclude が未発見でも、追加情報に入っているかを検証する必要あり → NeedsMore
        let t = parse("foo -bad");
        assert_eq!(decide_partial(&t, "foo is clean"), PartialResult::NeedsMore);
    }

    #[test]
    fn decide_partial_exclude_only_missing_in_hay() {
        // "-bad" のみの場合、hay に bad が無くても追加情報確認が必要 → NeedsMore
        let t = parse("-bad");
        assert_eq!(decide_partial(&t, "anything"), PartialResult::NeedsMore);
    }

    #[test]
    fn decide_partial_empty_tokens() {
        // トークン 0 個は常に Decided(true) (追加情報を読む必要なし)
        assert_eq!(
            decide_partial(&[], "anything"),
            PartialResult::Decided(true)
        );
    }

    #[test]
    fn decide_partial_exclude_hit_short_circuits_missing_include() {
        // exclude が見つかれば、他の include が欠けていても Decided(false) を返す
        // (追加情報を読む必要なし)
        let t = parse("missing -bad");
        assert_eq!(
            decide_partial(&t, "text with bad here"),
            PartialResult::Decided(false),
        );
    }

    // ---- タグ構文 (docs/tag-feature.md) ----

    #[test]
    fn parse_tag_prefix() {
        let tokens = parse("#原神");
        assert_eq!(tokens, vec![tag("#原神")]);
    }

    #[test]
    fn parse_tag_with_keyword() {
        let tokens = parse("#原神 写真");
        assert_eq!(tokens, vec![tag("#原神"), inc("写真")]);
    }

    #[test]
    fn parse_tag_exclude() {
        let tokens = parse("-#原神");
        assert_eq!(tokens, vec![not_tag("#原神")]);
    }

    #[test]
    fn parse_hash_alone_is_keyword() {
        // 単独 `#` はキーワード扱い (プレフィックスとして使えない)
        let tokens = parse("#");
        assert_eq!(tokens, vec![inc("#")]);
    }

    #[test]
    fn parse_double_hash_is_keyword() {
        // `##foo` は曖昧なのでキーワード扱い
        let tokens = parse("##foo");
        assert_eq!(tokens, vec![inc("##foo")]);
    }

    #[test]
    fn matches_tags_include_hit() {
        let tokens = parse("#原神");
        let (tags, _) = split_tokens(&tokens);
        assert!(matches_tags(&tags, "#原神 #風景"));
    }

    #[test]
    fn matches_tags_include_miss() {
        let tokens = parse("#ドール");
        let (tags, _) = split_tokens(&tokens);
        assert!(!matches_tags(&tags, "#原神 #風景"));
    }

    #[test]
    fn matches_tags_exclude() {
        let tokens = parse("-#原神");
        let (tags, _) = split_tokens(&tokens);
        assert!(!matches_tags(&tags, "#原神 #風景"));
        assert!(matches_tags(&tags, "#ドール"));
    }

    #[test]
    fn matches_tags_and_logic() {
        let tokens = parse("#原神 #風景");
        let (tags, _) = split_tokens(&tokens);
        assert!(matches_tags(&tags, "#原神 #風景 #その他"));
        assert!(!matches_tags(&tags, "#原神"));
        assert!(!matches_tags(&tags, "#風景"));
    }

    #[test]
    fn matches_tags_substring_does_not_match() {
        // オプション A: タグは substring でなく完全一致でのみヒット
        let tokens = parse("#原");
        let (tags, _) = split_tokens(&tokens);
        assert!(!matches_tags(&tags, "#原神"));
    }
}
