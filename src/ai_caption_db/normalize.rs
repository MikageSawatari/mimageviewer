//! Mechanical cleanup shared by new saves and the one-time stored-row cleanup.

const MIN_COLLAPSED_RUN_SCALARS: usize = 30;

pub(super) fn fields(
    caption: &str,
    text: &str,
    keywords: &[String],
) -> (String, String, Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    let keywords = keywords
        .iter()
        .filter_map(|word| {
            let word = repetitions(word).trim().to_owned();
            seen.insert(word.clone()).then_some(word)
        })
        .collect();
    (
        repetitions(caption).trim_end().to_owned(),
        repetitions(text).trim_end().to_owned(),
        keywords,
    )
}

/// Collapse adjacent copies of a 2..=64 Unicode-scalar unit when at least
/// three copies span at least MIN_COLLAPSED_RUN_SCALARS scalars in total.
/// Uniform units are excluded so a one-character run
/// cannot masquerade as a two/four-character unit. No Unicode normalization,
/// case folding, line splitting, or semantic comparison is performed.
fn repetitions(value: &str) -> String {
    let mut scalars: Vec<char> = value.chars().collect();
    loop {
        let mut output = Vec::with_capacity(scalars.len());
        let mut start = 0;
        let mut changed = false;
        while start < scalars.len() {
            let max_unit = 64.min((scalars.len() - start) / 3);
            let repeated = (2..=max_unit).find_map(|length| {
                let unit = &scalars[start..start + length];
                if unit.iter().all(|scalar| *scalar == unit[0])
                    || scalars[start + length..start + 2 * length] != *unit
                    || scalars[start + 2 * length..start + 3 * length] != *unit
                {
                    return None;
                }
                let mut end = start + 3 * length;
                while end + length <= scalars.len() && scalars[end..end + length] == *unit {
                    end += length;
                }
                (end - start >= MIN_COLLAPSED_RUN_SCALARS).then_some((length, end))
            });
            if let Some((length, end)) = repeated {
                output.extend_from_slice(&scalars[start..start + length]);
                start = end;
                changed = true;
            } else {
                output.push(scalars[start]);
                start += 1;
            }
        }
        if !changed {
            return scalars.into_iter().collect();
        }
        // A removed run can bring its neighbors together. Revisit the smaller
        // value so repeated normalization (including a later startup) is inert.
        scalars = output;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_repetition_collapses_only_three_or_more_bounded_nonuniform_units() {
        for (input, expected) in [
            ("", ""),
            ("あいうえお", "あいうえお"),
            ("はいはい", "はいはい"),
            ("はいはいはい", "はいはいはい"),
            ("前はいはいはいはい後", "前はいはいはいはい後"),
            ("ドンドンドン", "ドンドンドン"),
            ("1.1.1.", "1.1.1."),
            ("https://example.com/a/a/a/", "https://example.com/a/a/a/"),
            ("|---|---|---|", "|---|---|---|"),
            ("あああああああああ", "あああああああああ"),
            ("!!!!!!!!!!!!", "!!!!!!!!!!!!"),
            ("あいうあいうあいうあい", "あいうあいうあいうあい"),
            (
                "一行目\n二行目\n一行目\n二行目\n一行目\n二行目\n",
                "一行目\n二行目\n一行目\n二行目\n一行目\n二行目\n",
            ),
            ("e\u{301}e\u{301}e\u{301}", "e\u{301}e\u{301}e\u{301}"),
            (
                "👩\u{200d}💻👩\u{200d}💻👩\u{200d}💻",
                "👩\u{200d}💻👩\u{200d}💻👩\u{200d}💻",
            ),
            ("か\u{3099}がか\u{3099}が", "か\u{3099}がか\u{3099}が"),
        ] {
            assert_eq!(repetitions(input), expected, "input: {input:?}");
            assert_eq!(repetitions(&repetitions(input)), expected);
        }
        for (length, collapse) in [(2, false), (10, true), (64, true), (65, false)] {
            let unit: String = (0..length)
                .map(|index| char::from_u32(0x4e00 + index).unwrap())
                .collect();
            let input = unit.repeat(3);
            let expected = if collapse { unit } else { input.clone() };
            assert_eq!(repetitions(&input), expected);
            assert_eq!(repetitions(&expected), expected);
        }
    }

    #[test]
    fn scalar_repetition_threshold_is_thirty_scalars() {
        // 29 is prime, so it cannot be the length of a complete qualifying run.
        // Cover a 29-scalar input: 28 repeated scalars plus an incomplete unit.
        let below = "ab".repeat(14) + "a";
        assert_eq!(below.chars().count(), 29);
        assert_eq!(repetitions(&below), below);
        for (unit, count) in [("abcdefghij", 3), ("ab", 15)] {
            let input = unit.repeat(count);
            assert_eq!(input.chars().count(), 30);
            assert_eq!(repetitions(&input), unit);
            assert_eq!(repetitions(&repetitions(&input)), unit);
        }
        let separated = "海辺で朝を見て、夕方は海辺へ戻る";
        assert_eq!(repetitions(separated), separated);
        let uniform = "は".repeat(40);
        assert_eq!(repetitions(&uniform), uniform);
    }

    #[test]
    fn unicode_long_runs_preserve_scalar_sequences() {
        for (unit, count) in [("e\u{301}", 15), ("👩\u{200d}💻", 10)] {
            let input = unit.repeat(count);
            assert_eq!(input.chars().count(), 30);
            assert_eq!(repetitions(&input), unit);
            assert_eq!(repetitions(&repetitions(&input)), unit);
        }
    }

    #[test]
    fn newly_adjacent_qualifying_runs_are_rescanned_to_a_fixed_point() {
        let unit = "abCDEFGHIJ";
        let input = format!("{}{}CDEFGHIJ", unit.repeat(2), "ab".repeat(15));
        // Collapsing the inner run creates three adjacent 10-scalar units.
        assert_eq!(repetitions(&input), unit);
        assert_eq!(repetitions(&repetitions(&input)), unit);
    }

    #[test]
    fn caption_fields_trim_ends_and_deduplicate_keywords_after_repetition_cleanup() {
        let mut keywords = [" 海海海 ", "海", "海辺海辺海辺", "海辺", "Ａ", "A", "", " "]
            .map(str::to_owned)
            .to_vec();
        keywords.insert(2, format!(" {} ", "海辺".repeat(15)));
        let result = fields(
            &format!("  {} \n", "青い空".repeat(10)),
            &"上\n下\n".repeat(8),
            &keywords,
        );
        // One-scalar repetitions stay intact, including in keywords.
        assert_eq!(result.0, "  青い空");
        assert_eq!(result.1, "上\n下");
        assert_eq!(
            result.2,
            ["海海海", "海", "海辺", "海辺海辺海辺", "Ａ", "A", ""]
        );
        assert_eq!(fields(&result.0, &result.1, &result.2), result);
    }

    #[test]
    fn keyword_deduplication_preserves_order_spelling_and_distinct_contained_words() {
        let keywords = [
            " 海 ",
            "海辺",
            "海",
            "Ａ",
            "A",
            " ｶﾀｶﾅ",
            "カタカナ",
            "A",
            "",
            " ",
        ]
        .map(str::to_owned);
        assert_eq!(
            fields("", "", &keywords).2,
            ["海", "海辺", "Ａ", "A", "ｶﾀｶﾅ", "カタカナ", ""]
        );
    }

    #[test]
    fn reported_caption_repetitions_collapse_and_keep_incomplete_final_units() {
        for (unit, count) in [("じゅん ", 8), ("あおば\n", 8), ("市場の一日\n朝四時\n", 3)]
        {
            let repeated = unit.repeat(count);
            let (caption, text, keywords) = fields(&repeated, &repeated, &[repeated.clone()]);
            assert_eq!(caption, unit.trim_end());
            assert_eq!(text, unit.trim_end());
            assert_eq!(keywords, [unit.trim().to_owned()]);
        }
        for (unit, count, partial, expected) in [
            ("じゅん ", 8, "じゅ", "じゅん じゅ"),
            ("あおば\n", 8, "あお", "あおば\nあお"),
            (
                "市場の一日\n朝四時\n",
                3,
                "市場の一",
                "市場の一日\n朝四時\n市場の一",
            ),
        ] {
            let input = unit.repeat(count) + partial;
            let (caption, text, _) = fields(&input, &input, &[]);
            assert_eq!(caption, expected);
            assert_eq!(text, expected);
        }
    }
}
