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
//!
//! ## 結合モード (`MatchMode`)
//!
//! 検索 UI の「□OR」チェックで切り替える (docs/archive/search-metadata/search-expansion-design.md §20)。
//! - `MatchMode::And` (既定): include トークンを **すべて** 含むものがマッチ
//! - `MatchMode::Or`: include トークンを **1 つ以上** 含むものがマッチ
//!
//! **NOT トークンは常に AND** (OR モードでも同じ)。
//! 例: `klee #klee -sleep -nsfw` を OR モードで評価すると
//! `(klee OR #klee) AND (NOT sleep) AND (NOT nsfw)` と解釈される。

/// include トークン群の結合方法。NOT トークンは常に AND なのでこの enum に依らない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MatchMode {
    /// 既定。include トークンを AND で結合する。
    #[default]
    And,
    /// include トークンを OR で結合する (NOT は AND のまま)。
    Or,
}

impl From<bool> for MatchMode {
    /// UI チェックボックスの `or_mode: bool` → `MatchMode` 変換。`true` で `Or`。
    fn from(or_mode: bool) -> Self {
        if or_mode {
            MatchMode::Or
        } else {
            MatchMode::And
        }
    }
}

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

/// `hay` がトークン列にマッチするか判定する (内部で小文字化、AND モード既定)。
/// - include トークン: hay に含まれなければ不一致
/// - exclude トークン: hay に含まれれば不一致
/// - トークン列が空: 常に一致 (フィルタなしの扱い)
///
/// `is_tag` フラグ付きトークンも通常のキーワード扱いで hay に対して substring 判定する
/// (`#原神` を探しに行く)。tags フィールドは bigram tokenize されて per-source テキストに
/// 結合されるため、post-filter でも自然に一致する。
pub fn matches(tokens: &[Token], hay: &str) -> bool {
    matches_with_mode(tokens, hay, MatchMode::And)
}

/// `matches` の結合モード指定版 (docs §20)。
/// - `MatchMode::And`: include は全部含む必要あり
/// - `MatchMode::Or`: include は 1 つでも含めば OK (exclude は常に AND)
///
/// include が 0 個 + exclude のみ + OR モードの場合、「exclude を含まない」だけで一致扱い
/// にする (AND モードと同じ振る舞い、NOT-only は UI 側で拒否される)。
pub fn matches_with_mode(tokens: &[Token], hay: &str, mode: MatchMode) -> bool {
    let hay_lower = hay.to_lowercase();
    matches_lowercased_with_mode(tokens, &hay_lower, mode)
}

/// 小文字化済みの `hay_lower` に対する照合。
pub fn matches_lowercased_with_mode(tokens: &[Token], hay_lower: &str, mode: MatchMode) -> bool {
    if tokens.is_empty() {
        return true;
    }
    let mut any_include = false;
    let mut include_hit = false;
    for t in tokens {
        if t.include {
            any_include = true;
            let hit = hay_lower.contains(&t.needle);
            match mode {
                MatchMode::And => {
                    if !hit {
                        return false;
                    }
                }
                MatchMode::Or => {
                    if hit {
                        include_hit = true;
                    }
                }
            }
        } else if hay_lower.contains(&t.needle) {
            return false;
        }
    }
    match mode {
        MatchMode::And => true,
        // OR モード: include が 1 つもなければ「フィルタなし + exclude 通過」 = true
        MatchMode::Or => !any_include || include_hit,
    }
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

/// hay_so_far だけで確定判定できるかを返す (AND モード既定、後方互換)。
pub fn decide_partial(tokens: &[Token], hay_so_far: &str) -> PartialResult {
    decide_partial_with_mode(tokens, hay_so_far, MatchMode::And)
}

/// hay_so_far だけで確定判定できるかを返す。Ctrl+F メタデータ検索で、
/// 高コストな XMP 読み込みを「避けられる時は避ける」ための事前判定に使う。
///
/// 戻り値は以下 3 種類:
/// - `Decided(false)`: exclude トークンが hay_so_far に**含まれている** 。
///   追加情報に関わらず不一致確定なので XMP を読む必要なし (AND/OR 共通)。
/// - `Decided(true)`:
///   - AND: 全 include が hay_so_far にあり、exclude が 1 つも存在しない。
///   - OR: 少なくとも 1 つの include が hay_so_far にあり、exclude が 1 つも存在しない。
/// - `NeedsMore`: 追加情報で結果が覆り得る。
///   - AND: include が欠けている、または exclude が存在して未確認。
///   - OR: include が 1 つもヒットしていない (追加情報で見つかるかも)、または exclude 未確認。
pub fn decide_partial_with_mode(
    tokens: &[Token],
    hay_so_far: &str,
    mode: MatchMode,
) -> PartialResult {
    if tokens.is_empty() {
        return PartialResult::Decided(true);
    }
    let hay_lower = hay_so_far.to_lowercase();
    let mut has_include = false;
    let mut any_include_missing = false;
    let mut include_hit = false;
    let mut has_exclude = false;
    for t in tokens {
        if t.include {
            has_include = true;
            if hay_lower.contains(&t.needle) {
                include_hit = true;
            } else {
                any_include_missing = true;
            }
        } else {
            has_exclude = true;
            if hay_lower.contains(&t.needle) {
                return PartialResult::Decided(false);
            }
        }
    }
    match mode {
        MatchMode::And => {
            if any_include_missing || has_exclude {
                PartialResult::NeedsMore
            } else {
                PartialResult::Decided(true)
            }
        }
        MatchMode::Or => {
            // OR: include が 1 つでも見つかれば、あとは exclude 次第。
            // exclude が無ければ Decided(true)、あれば追加情報で混入を確認する必要あり。
            if has_include && !include_hit {
                // どの include もまだ見つからない → 追加情報に含まれているかも。
                return PartialResult::NeedsMore;
            }
            if has_exclude {
                PartialResult::NeedsMore
            } else {
                PartialResult::Decided(true)
            }
        }
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

    // ---- タグ構文 (docs/archive/search-metadata/tag-feature.md) ----

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
    fn parse_tag_flag_set() {
        // `#原神` は is_tag=true、needle は `#` 込み
        let tokens = parse("#原神");
        assert_eq!(tokens.len(), 1);
        assert!(tokens[0].is_tag);
        assert_eq!(tokens[0].needle, "#原神");
    }

    #[test]
    fn parse_double_hash_is_keyword() {
        // `##foo` は is_tag=false (ユーザ意図が曖昧なので通常キーワード扱い)
        let tokens = parse("##foo");
        assert_eq!(tokens.len(), 1);
        assert!(!tokens[0].is_tag);
    }

    // ---- OR モード (docs §20) ----

    #[test]
    fn or_mode_matches_any_include() {
        // OR: いずれかの include が含まれれば一致
        let t = parse("klee #klee");
        assert!(matches_with_mode(&t, "this is klee art", MatchMode::Or));
        assert!(matches_with_mode(&t, "#klee is here", MatchMode::Or));
        assert!(!matches_with_mode(&t, "unrelated text", MatchMode::Or));
    }

    #[test]
    fn or_mode_with_excludes_still_and() {
        // OR: include は OR でも、exclude は AND (常に除外)
        let t = parse("klee #klee -sleep -nsfw");
        assert!(matches_with_mode(&t, "klee portrait", MatchMode::Or));
        assert!(!matches_with_mode(&t, "klee is sleep", MatchMode::Or));
        assert!(!matches_with_mode(&t, "#klee nsfw", MatchMode::Or));
    }

    #[test]
    fn or_mode_none_match_fails() {
        // OR: include が 1 つも含まれなければ不一致
        let t = parse("foo bar");
        assert!(!matches_with_mode(
            &t,
            "neither token present",
            MatchMode::Or
        ));
    }

    #[test]
    fn or_mode_single_include_ok() {
        // OR でも include が 1 個なら AND と挙動が同じ
        let t = parse("klee");
        assert!(matches_with_mode(&t, "this is klee", MatchMode::Or));
        assert!(!matches_with_mode(&t, "unrelated", MatchMode::Or));
    }

    #[test]
    fn or_mode_only_excludes_matches_any() {
        // OR で exclude only (UI では NOT-only を弾く前提) は AND と同じく
        // exclude を含まないものに一致する。
        let t = parse("-bad");
        assert!(matches_with_mode(&t, "anything", MatchMode::Or));
        assert!(!matches_with_mode(&t, "has bad", MatchMode::Or));
    }

    #[test]
    fn matches_default_is_and() {
        // 既定のショートハンド `matches` は AND 挙動
        let t = parse("foo bar");
        assert!(matches(&t, "foo and bar"));
        assert!(!matches(&t, "only foo"));
    }

    #[test]
    fn lowercased_matcher_preserves_filename_query_rules() {
        assert!(matches_lowercased_with_mode(
            &parse(""),
            "anything.jpg",
            MatchMode::And
        ));
        assert!(matches_lowercased_with_mode(
            &parse("PHOTO"),
            "summer_photo.jpg",
            MatchMode::And
        ));
        assert!(matches_lowercased_with_mode(
            &parse("summer photo"),
            "summer_photo.jpg",
            MatchMode::And
        ));
        assert!(!matches_lowercased_with_mode(
            &parse("summer -draft"),
            "summer_draft.jpg",
            MatchMode::And
        ));
        assert!(matches_lowercased_with_mode(
            &parse("summer -draft"),
            "summer_final.jpg",
            MatchMode::And
        ));
    }

    #[test]
    fn decide_partial_or_mode_any_include_hit() {
        // OR: hay_so_far に include が 1 つでもあれば Decided(true) (exclude 無し)
        let t = parse("foo bar");
        assert_eq!(
            decide_partial_with_mode(&t, "only foo here", MatchMode::Or),
            PartialResult::Decided(true)
        );
    }

    #[test]
    fn decide_partial_or_mode_no_include_yet() {
        // OR: include が 1 つも見つかっていない → 追加情報で見つかるかも
        let t = parse("foo bar");
        assert_eq!(
            decide_partial_with_mode(&t, "unrelated", MatchMode::Or),
            PartialResult::NeedsMore
        );
    }

    #[test]
    fn decide_partial_or_mode_exclude_hit_short_circuit() {
        // OR でも exclude が hay にあれば Decided(false)
        let t = parse("foo bar -bad");
        assert_eq!(
            decide_partial_with_mode(&t, "foo here but bad", MatchMode::Or),
            PartialResult::Decided(false)
        );
    }

    #[test]
    fn decide_partial_or_mode_include_hit_but_exclude_unchecked() {
        // OR: include 見つけた、exclude は hay に未存在 → 追加情報に exclude がないか要確認
        let t = parse("foo -bad");
        assert_eq!(
            decide_partial_with_mode(&t, "foo is here", MatchMode::Or),
            PartialResult::NeedsMore
        );
    }
}
