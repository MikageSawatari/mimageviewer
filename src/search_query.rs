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

        let needle = buf.trim().to_lowercase();
        if !needle.is_empty() {
            tokens.push(Token { include, needle });
        }
    }
    tokens
}

/// `hay` がトークン列にマッチするか判定する (内部で小文字化)。
/// - include トークン: hay に含まれなければ不一致
/// - exclude トークン: hay に含まれれば不一致
/// - トークン列が空: 常に一致 (フィルタなしの扱い)
pub fn matches(tokens: &[Token], hay: &str) -> bool {
    if tokens.is_empty() {
        return true;
    }
    let hay_lower = hay.to_lowercase();
    for t in tokens {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn inc(s: &str) -> Token {
        Token {
            include: true,
            needle: s.to_string(),
        }
    }
    fn exc(s: &str) -> Token {
        Token {
            include: false,
            needle: s.to_string(),
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
        assert_eq!(
            parse(r#"-"low quality""#),
            vec![exc("low quality")],
        );
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
}
