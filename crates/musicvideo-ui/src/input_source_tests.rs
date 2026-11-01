use std::path::{Path, PathBuf};

struct RawTextEditExemption {
    path: &'static str,
    line_anchor: &'static str,
    expected_occurrences: usize,
    reason: &'static str,
}

const RAW_TEXT_EDIT_EXEMPTIONS: &[RawTextEditExemption] = &[
    RawTextEditExemption {
        path: "src/lib.rs",
        line_anchor: "multiline(editor)",
        expected_occurrences: 1,
        reason: "歌詞固有の LyricsTextBuffer / widget ID / native Undo の描画境界を保つ",
    },
    RawTextEditExemption {
        path: "src/lib.rs",
        line_anchor: "singleline(&mut text).id(text_id)",
        expected_occurrences: 2,
        reason: "focused_egui_text_edit_keeps_ctrl_z_for_its_own_undo の低レベル native Undo fixture",
    },
];

fn collect_rust_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src directory") {
        let entry = entry.expect("read src entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_sources(&path, files);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            files.push(path);
        }
    }
}

#[test]
fn raw_text_edits_are_ime_aware_or_explicitly_exempt() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src_dir = manifest_dir.join("src");
    let mut files = Vec::new();
    collect_rust_sources(&src_dir, &mut files);
    files.sort();

    let raw_patterns = [
        ["TextEdit::", "singleline("].concat(),
        ["TextEdit::", "multiline("].concat(),
        [".text_edit_", "singleline("].concat(),
        [".text_edit_", "multiline("].concat(),
    ];
    let mut exemption_matches = vec![0usize; RAW_TEXT_EDIT_EXEMPTIONS.len()];
    let mut violations = Vec::new();

    for path in files {
        let relative = path
            .strip_prefix(&manifest_dir)
            .expect("source under manifest directory")
            .to_string_lossy()
            .replace('\\', "/");
        let source = std::fs::read_to_string(&path).expect("read Rust source as UTF-8");
        let lines: Vec<&str> = source.lines().collect();
        for (line_idx, line) in lines.iter().enumerate() {
            if !raw_patterns.iter().any(|pattern| line.contains(pattern)) {
                continue;
            }
            let matches: Vec<usize> = RAW_TEXT_EDIT_EXEMPTIONS
                .iter()
                .enumerate()
                .filter_map(|(idx, exemption)| {
                    (exemption.path == relative
                        && line.contains(exemption.line_anchor)
                        && !exemption.reason.trim().is_empty())
                    .then_some(idx)
                })
                .collect();
            match matches.as_slice() {
                [idx] => exemption_matches[*idx] += 1,
                [] => violations.push(format!("{relative}:{}: {}", line_idx + 1, line.trim())),
                _ => violations.push(format!(
                    "{relative}:{}: 複数の exemption に一致: {}",
                    line_idx + 1,
                    line.trim()
                )),
            }
        }
    }

    for (idx, exemption) in RAW_TEXT_EDIT_EXEMPTIONS.iter().enumerate() {
        assert_eq!(
            exemption_matches[idx], exemption.expected_occurrences,
            "IME raw TextEdit exemption が現ソースと一致しません: {} / {} ({})",
            exemption.path, exemption.line_anchor, exemption.reason
        );
    }
    assert!(
        violations.is_empty(),
        "IME focus policy を通らない raw TextEdit があります:\n{}\n自由入力欄は \
         egui_ime の helper を使ってください。意図的に対象外とする場合は、\
         input_source_tests.rs の RAW_TEXT_EDIT_EXEMPTIONS に短い理由付きで追加してください。",
        violations.join("\n")
    );
}
