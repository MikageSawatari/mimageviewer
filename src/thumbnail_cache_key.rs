//! Inventory retention is separate from exact thumbnail lookup and stamp validation.
use std::collections::HashSet;

/// Keep exact keys, plus resolved EPUB pin generations whose pending identity is
/// still in the inventory. This must never be used to select a row for display.
pub(crate) fn inventory_retains_key(existing: &HashSet<String>, stored: &str) -> bool {
    existing.contains(stored)
        || epub_pin_pending_key(stored).is_some_and(|pending| existing.contains(pending))
}

fn epub_pin_pending_key(stored: &str) -> Option<&str> {
    if !stored.starts_with("folderthumb:") && !stored.starts_with("pdfthumb:") {
        return None;
    }
    // ZIP entry names are literal strings and can imitate all the suffix tokens.
    // Recognize only the first pin delimiter and the producer's PDF leaf kinds.
    let (_, identity) = stored.split_once("#pin:")?;
    let leaf = if let Some(cascade) = identity.strip_prefix("cascade:") {
        let (hash, leaf) = cascade.split_once(':')?;
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return None;
        }
        leaf
    } else {
        identity
    };
    if !leaf.starts_with("pdfpage|") && !leaf.starts_with("pdffile|") {
        return None;
    }
    let (pending, generation) = stored.rsplit_once("|generation:")?;
    if !pending.ends_with("|generation-pending|generation-pending")
        || pending.contains("|generation:")
    {
        return None;
    }
    let (mtime, size) = generation.split_once(':')?;
    for number in [mtime, size] {
        let parsed = number.parse::<i64>().ok()?;
        if parsed.to_string() != number {
            return None;
        }
    }
    Some(pending)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(namespace: &str, leaf: &str) -> String {
        format!("{namespace}child#pin:{leaf}|book.epub|-|2|generation-pending|generation-pending")
    }

    #[test]
    fn retention_is_pure_and_keeps_each_generation_of_the_same_pin() {
        for namespace in ["folderthumb:", "pdfthumb:"] {
            for kind in ["pdfpage", "pdffile"] {
                for leaf in [
                    kind.to_owned(),
                    format!("cascade:{}:{kind}", "a0".repeat(32)),
                ] {
                    let pending = pending(namespace, &leaf);
                    // No ReadTarget/fixed-generation table or filesystem access is needed.
                    let inventory = HashSet::from([pending.clone()]);
                    for generation in [
                        "0:0",
                        "17:4096",
                        "18:8192",
                        "-1:-2",
                        "-9223372036854775808:9223372036854775807",
                    ] {
                        assert!(inventory_retains_key(
                            &inventory,
                            &format!("{pending}|generation:{generation}")
                        ));
                    }
                }
            }
        }
    }

    #[test]
    fn retention_preserves_exact_keys_even_when_not_a_generation_key() {
        for key in [
            "ordinary.jpg",
            "zipdir:dir/#pin:pdfpage|literal|generation-pending|generation-pending|generation:1:2",
            "folderthumb:child#pin:pdfpage|malformed|generation:+1:2",
        ] {
            assert!(inventory_retains_key(&HashSet::from([key.into()]), key));
        }
    }

    #[test]
    fn retention_requires_unchanged_pin_crop_and_policy() {
        let pending = "folderthumb:auto-v2:numeric:d3:child#pin:pdfpage|book.epub|-|2|crop:0,0,0.5,1|generation-pending|generation-pending";
        let stored = format!("{pending}|generation:17:4096");
        assert!(inventory_retains_key(
            &HashSet::from([pending.into()]),
            &stored
        ));
        for changed in [
            pending.replace("book.epub", "other.epub"),
            pending.replace("|-|2|", "|-|3|"),
            pending.replace("crop:0,0,0.5,1", "crop:0,0,1,1"),
            pending.replace("numeric:d3", "name:d2"),
            "folderthumb:auto-v2:numeric:d3:child".into(),
        ] {
            assert!(!inventory_retains_key(&HashSet::from([changed]), &stored));
        }
        assert!(!inventory_retains_key(&HashSet::new(), &stored));
    }

    #[test]
    fn retention_rejects_noncanonical_or_nonterminal_generation_suffixes() {
        let pending = pending("folderthumb:", "pdfpage");
        let inventory = HashSet::from([pending.clone()]);
        // No suffix means an exact inventory key, which must remain retained.
        assert!(inventory_retains_key(&inventory, &pending));
        for suffix in [
            "|generation:1",
            "|generation:1:2:3",
            "|generation:1:2|tail",
            "|generation:9223372036854775808:2",
            "|generation:1:-9223372036854775809",
            "|generation:+1:2",
            "|generation:1:+2",
            "|generation:01:2",
            "|generation:1:02",
            "|generation:-0:2",
            "|generation:-01:2",
            "|generation:1: 2",
            "|generation:1:2\n",
            "|generation:1:2|generation:3:4",
            "|generation:1:2|generation-pending|generation-pending|generation:3:4",
        ] {
            assert!(
                !inventory_retains_key(&inventory, &format!("{pending}{suffix}")),
                "{suffix:?}"
            );
        }
        for invalid_pending in [
            pending.replace(
                "|generation-pending|generation-pending",
                "|generation-pending",
            ),
            format!("{pending}|crop:0,0,1,1"),
        ] {
            assert!(!inventory_retains_key(
                &HashSet::from([invalid_pending.clone()]),
                &format!("{invalid_pending}|generation:1:2")
            ));
        }
    }

    #[test]
    fn retention_rejects_literal_zip_entries_and_invalid_pin_structure() {
        let hash = "ab".repeat(32);
        for literal in [
            "entry|generation:1:2".to_owned(),
            "entry|generation-pending|generation-pending".to_owned(),
            "pdfpage|book.epub|-|2|generation-pending|generation-pending".to_owned(),
            format!("cascade:{hash}:pdffile|book.epub|-|-|generation-pending|generation-pending"),
        ] {
            for stem in [
                format!("zipdir:dir/#pin:{literal}"),
                format!("zipthumb:book.zip#pin:{literal}"),
                format!("archivethumb:book.rar#pin:{literal}"),
                format!("audioart:{literal}"),
                literal.clone(),
                format!("folderthumb:child#pin:zipentry|book.zip|{literal}"),
                format!("pdfthumb:book.pdf#pin:zipentry|book.zip|{literal}"),
                format!("folderthumb:child#pin:cascade:{hash}:zipentry|book.zip|{literal}"),
            ] {
                let stored = format!("{stem}|generation:1:2");
                assert!(
                    !inventory_retains_key(&HashSet::from([stem]), &stored),
                    "{stored}"
                );
                assert!(inventory_retains_key(
                    &HashSet::from([stored.clone()]),
                    &stored
                ));
            }
        }
        for leaf in [
            "image|pdfpage".to_owned(),
            format!("cascade:{}:pdfpage", "a".repeat(63)),
            format!("cascade:{}:pdfpage", "a".repeat(65)),
            format!("cascade:{}:pdfpage", "g".repeat(64)),
            format!("cascade:{hash}:cascade:{hash}:pdfpage"),
            "other#pin:pdfpage".into(),
        ] {
            let stem = pending("folderthumb:", &leaf);
            assert!(
                !inventory_retains_key(
                    &HashSet::from([stem.clone()]),
                    &format!("{stem}|generation:1:2")
                ),
                "{leaf}"
            );
        }
    }
}
