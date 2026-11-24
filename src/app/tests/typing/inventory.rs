//! Every ime_focus single-line call belongs to exactly one test or explained exemption.
use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

struct Entry {
    path: &'static str,
    line_anchor: &'static str,
    expected_occurrences: usize,
    test: Option<&'static str>,
    reason: Option<&'static str>,
}

const fn tested(
    path: &'static str,
    line_anchor: &'static str,
    expected_occurrences: usize,
    test: &'static str,
) -> Entry {
    Entry {
        path,
        line_anchor,
        expected_occurrences,
        test: Some(test),
        reason: None,
    }
}
const fn exempt(
    path: &'static str,
    line_anchor: &'static str,
    expected_occurrences: usize,
    reason: &'static str,
) -> Entry {
    Entry {
        path,
        line_anchor,
        expected_occurrences,
        test: None,
        reason: Some(reason),
    }
}

// The shared history wrapper draws Local/Container/Metadata/Everything and the
// address field. Their separate adapters exercise its callers through App frames.
const INPUT_TESTS: &[Entry] = &[
    tested(
        "src/ui_history_field.rs",
        "show_singleline(ui, text,",
        1,
        "typing_history_popup_edit_keeps_field",
    ),
    tested(
        "src/ui_main.rs",
        "&mut self.tag_view.query",
        1,
        "typing_ctrl_t_ascii",
    ),
    tested(
        "src/ui_dialogs/tag_apply.rs",
        "&mut self.tag_apply_input",
        1,
        "typing_tag_apply_suggestions_ascii",
    ),
    tested(
        "src/ui_dialogs/collections.rs",
        "&mut self.collection_ui.manager_new_name",
        1,
        "typing_collection_creation_name",
    ),
    tested(
        "src/ui_dialogs/smart_folder_editor.rs",
        "&mut draft.filter.name_contains",
        1,
        "typing_smart_folder_rule_draft",
    ),
    tested(
        "src/ui_dialogs/smart_folder_editor.rs",
        "&mut rule.filter.name_contains",
        1,
        "typing_smart_folder_editor_rule",
    ),
    tested(
        "src/ui_main.rs",
        "&mut self.facet_name_input",
        1,
        "typing_facet_name_ascii",
    ),
    tested(
        "src/ui_main.rs",
        "&mut self.facet_tag_search_query",
        1,
        "typing_facet_tag_suggestions_ascii",
    ),
];

const EXEMPTIONS: &[Entry] = &[
    exempt(
        "src/ui_dialogs/export_batch/watermark.rs",
        "&mut options.text",
        1,
        "透かしの文字。書き出す操作時だけ使用する",
    ),
    exempt(
        "src/ui_dialogs/export_batch/watermark.rs",
        "&mut path_text",
        1,
        "ロゴのパス。入力終了時だけ存在確認する",
    ),
    exempt(
        "src/ui_dialogs/preferences/browser_integration.rs",
        "input,",
        1,
        "ブラウザ閲覧の保存先。入力中は検査せず、適用時に worker で確定する設定欄",
    ),
    exempt(
        "src/app/list_order_worker_tests.rs",
        "&mut String::new()",
        1,
        "一覧移動の入力所有を検査する低レベル fixture",
    ),
    exempt(
        "src/app/main_drop_tests.rs",
        "show_singleline(ui, &mut text,",
        2,
        "ドロップと入力所有の境界を検査する fixture",
    ),
    exempt(
        "src/app/pinned_search/history_tests.rs",
        "app.search_draft_mut(field.kind())",
        1,
        "履歴 helper の低レベル fixture",
    ),
    exempt(
        "src/app/tests/section1768/extended.rs",
        "&mut self.text",
        2,
        "パネル寿命と入力所有の fixture",
    ),
    exempt(
        "src/app/tests.rs",
        "add_singleline(ui, title,",
        1,
        "しおり欄の低レベル fixture",
    ),
    exempt(
        "src/remote_ipc/ui.rs",
        "input,",
        1,
        "Remote のパスワード欄。入力中は送信しない",
    ),
    exempt(
        "src/search_request.rs",
        "show_singleline(ui, query,",
        1,
        "検索語注入と選択範囲の低レベル fixture",
    ),
    exempt(
        "src/search_request.rs",
        "show_singleline(ui, &mut query,",
        1,
        "検索語注入とフォーカスの低レベル fixture",
    ),
    exempt(
        "src/test_script.rs",
        "text,",
        1,
        "診断スクリプトのテスト用入力欄",
    ),
    exempt(
        "src/ui_adjustment_panel/shape_undo_investigation_tests.rs",
        "&mut text,",
        1,
        "図形Undoの調査 fixture",
    ),
    exempt(
        "src/ui_adjustment_panel.rs",
        "effect_query,",
        1,
        "補正パネル内の効果名フィルタ。検索結果の採用を伴わない",
    ),
    exempt(
        "src/ui_adjustment_panel.rs",
        "add_singleline(ui, title,",
        1,
        "しおり名。確定時だけ保存する",
    ),
    exempt(
        "src/ui_adjustment_panel.rs",
        "&mut name_input",
        1,
        "補正プリセット名。確定時だけ保存する",
    ),
    exempt(
        "src/ui_dialogs/archive_convert.rs",
        "&mut state.password",
        1,
        "書庫パスワード。確定時だけ処理する",
    ),
    exempt(
        "src/ui_dialogs/collections.rs",
        "add_singleline(ui, input,",
        1,
        "改名の下書き。重複名の判定と保存は改名操作時",
    ),
    exempt(
        "src/ui_dialogs/collections.rs",
        "add_singleline(ui, &mut draft,",
        1,
        "名前モーダル。保存操作時だけactorへ送る",
    ),
    exempt(
        "src/ui_dialogs/export_batch.rs",
        "&mut state.options.template",
        1,
        "書き出し名テンプレートの設定欄",
    ),
    exempt(
        "src/ui_dialogs/export_batch.rs",
        "&mut path_text",
        1,
        "書き出し先。確定時に使用する",
    ),
    exempt(
        "src/ui_dialogs/export_batch.rs",
        "subfolder",
        1,
        "一括出力のサブフォルダ名。書き出す操作時だけ使用する",
    ),
    exempt(
        "src/ui_dialogs/fav_add.rs",
        "&mut self.fav_add_name_input",
        1,
        "お気に入り表示名。確定時に保存する",
    ),
    exempt(
        "src/ui_dialogs/favorites_editor.rs",
        "&mut self.settings.favorites[i].name",
        1,
        "お気に入り管理の表示名。候補検索はしない",
    ),
    exempt(
        "src/ui_dialogs/open_folder.rs",
        "&mut self.open_folder_input",
        1,
        "場所を開くダイアログ。確定時だけ移動する",
    ),
    exempt(
        "src/ui_dialogs/pdf_password.rs",
        "&mut self.pdf_password_input",
        1,
        "PDFパスワード。確定時だけ使用する",
    ),
    exempt(
        "src/ui_dialogs/preferences/name_colors.rs",
        "&mut hex",
        1,
        "設定画面の色コード欄",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut state.ui_font_filter",
        1,
        "設定画面のフォント一覧フィルタ",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut state.startup_folder_path_input",
        1,
        "設定画面の起動場所",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut tool.name",
        1,
        "設定画面の外部ツール表示名",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut state.external_tool_executable_input",
        1,
        "設定画面の外部ツール実行ファイル",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut tool.arguments",
        1,
        "設定画面の外部ツール引数",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut state.external_tool_working_directory_input",
        1,
        "設定画面の外部ツール作業場所",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut state.clipboard_capture_output_dir_input",
        1,
        "設定画面の取り込み先",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut state.capture_output_dir_input",
        1,
        "設定画面のキャプチャ先",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut state.command_filter",
        1,
        "設定画面の操作一覧フィルタ",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut state.book_root_input",
        1,
        "設定画面の製本ルート",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut entry.name",
        1,
        "設定画面のLUT表示名",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut state.vst3_filter",
        1,
        "設定画面のVST一覧フィルタ",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut destination.name",
        1,
        "設定画面の保存先表示名",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "add_singleline(ui, &mut path,",
        1,
        "設定画面の保存先パス",
    ),
    exempt(
        "src/ui_dialogs/preferences/pages.rs",
        "&mut state.file_type_extension_input",
        1,
        "設定画面の拡張子入力",
    ),
    exempt(
        "src/ui_dialogs/preferences.rs",
        "&mut state.search_query",
        1,
        "設定項目内のフィルタ。アプリ一覧とは別の設定UI",
    ),
    exempt(
        "src/ui_dialogs/settings_restore.rs",
        "add_singleline(ui, label,",
        1,
        "設定バックアップの表示名",
    ),
    exempt(
        "src/ui_dialogs/smart_folder_editor.rs",
        "add_singleline(ui, &mut name,",
        1,
        "新規名。重複表示の更新は次フレーム、確定まで検索しない",
    ),
    exempt(
        "src/ui_dialogs/smart_folder_editor.rs",
        "&mut definition.name",
        1,
        "管理画面の表示名。確定まで検索しない",
    ),
    exempt(
        "src/ui_dialogs/tag_editor.rs",
        "&mut self.tag_editor_draft[i].name",
        1,
        "ピン留めタグの改名下書き。候補は出さず確定時に反映",
    ),
    exempt(
        "src/ui_fullscreen/in_page_scroll/handler_tests.rs",
        "add_singleline(ui, &mut text,",
        2,
        "ページ内スクロールの入力所有 fixture",
    ),
    exempt(
        "src/ui_fullscreen.rs",
        "&mut state.basename",
        1,
        "画像書き出し名。確定時に使用する",
    ),
    exempt(
        "src/ui_fullscreen.rs",
        "&mut state.output_dir_text",
        1,
        "画像書き出し先。確定時に使用する",
    ),
    exempt(
        "src/ui_fullscreen.rs",
        "add_singleline(ui, &mut text,",
        2,
        "フルスクリーン入力所有の低レベル fixture",
    ),
    exempt(
        "src/ui_helpers.rs",
        "&mut self.text",
        1,
        "右クリックメニューの低レベル fixture",
    ),
    exempt(
        "src/ui_main.rs",
        "view.new_name",
        1,
        "本の管理の新規名。確定時に作成する",
    ),
    exempt(
        "src/ui_main.rs",
        "edit.id(name_id)",
        1,
        "本の管理の改名欄。確定時に反映する",
    ),
    exempt(
        "src/ui_main.rs",
        "&mut path,",
        1,
        "アドレス欄レイアウトのsnapshot fixture",
    ),
    exempt(
        "src/ui_metadata_panel.rs",
        "input,",
        1,
        "情報パネルのしおり名。確定時に保存する",
    ),
    exempt(
        "src/ui_text.rs",
        "&mut self.text_font_dialog_sample",
        1,
        "文字注釈のフォント見本欄",
    ),
    exempt(
        "src/ui_text.rs",
        "&mut self.text_font_dialog_filter",
        1,
        "文字注釈のフォント一覧フィルタ",
    ),
    exempt(
        "src/ui_text.rs",
        "&mut filter_local",
        1,
        "文字注釈の素材一覧フィルタ",
    ),
    exempt(
        "src/ui_text.rs",
        "&mut w.name_plate.name.text",
        1,
        "文字注釈の名前札。編集プレビュー専用",
    ),
    exempt(
        "src/ui_text.rs",
        "add_singleline(ui, name_input,",
        1,
        "文字注釈のプリセット名",
    ),
    exempt(
        "src/video/native_presenter/overlay_draw.rs",
        "&mut state.title",
        1,
        "native動画のしおり名。headless Appには描画されない",
    ),
    exempt(
        "src/video/native_presenter/overlay_draw.rs",
        "input,",
        1,
        "native動画のタグ入力。別のnative overlayでApp単体では描画されない",
    ),
];

fn collect_sources(dir: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("source directory") {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_dir() {
            collect_sources(&entry.path(), files);
        } else if entry.path().extension().is_some_and(|ext| ext == "rs") {
            files.push(entry.path());
        }
    }
}

/// Blank comments and string literals without moving byte/line positions. In
/// particular, code examples in raw strings must not become real call sites.
fn code_only(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut result = bytes.to_vec();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i] == b'\'' {
            // Char literals can contain a quote or comment delimiter. Lifetimes
            // such as 'text have no closing apostrophe at the next char boundary.
            let mut end = i + 1;
            if bytes.get(end) == Some(&b'\\') {
                end += 1;
                if bytes.get(end) == Some(&b'u') && bytes.get(end + 1) == Some(&b'{') {
                    end += 2;
                    while end < bytes.len() && bytes[end] != b'}' {
                        end += 1;
                    }
                    end += usize::from(end < bytes.len());
                } else {
                    end = (end + 1).min(bytes.len());
                }
            } else if end < bytes.len() {
                end += source[end..].chars().next().unwrap().len_utf8();
            }
            if bytes.get(end) == Some(&b'\'') {
                i = end + 1;
            } else {
                i += 1;
                continue;
            }
        } else {
            let raw_start = if bytes[i..].starts_with(b"br") {
                i + 2
            } else if bytes[i] == b'r' {
                i + 1
            } else {
                i
            };
            let mut quote = raw_start;
            while quote < bytes.len() && bytes[quote] == b'#' {
                quote += 1;
            }
            if raw_start > i && quote < bytes.len() && bytes[quote] == b'"' {
                let hashes = quote - raw_start;
                i = quote + 1;
                while i < bytes.len() {
                    if bytes[i] == b'"'
                        && bytes
                            .get(i + 1..i + 1 + hashes)
                            .is_some_and(|tail| tail.iter().all(|b| *b == b'#'))
                    {
                        i += hashes + 1;
                        break;
                    }
                    i += 1;
                }
            } else if bytes[i] == b'"' {
                i += 1;
                while i < bytes.len() {
                    if bytes[i] == b'\\' {
                        i = (i + 2).min(bytes.len());
                    } else if bytes[i] == b'"' {
                        i += 1;
                        break;
                    } else {
                        i += 1;
                    }
                }
            } else {
                i += 1;
                continue;
            }
        }
        for byte in &mut result[start..i] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    String::from_utf8(result).expect("blanked source remains UTF-8")
}

/// Flatten Rust use trees before resolving names. Braces belong to the tree,
/// never to an alias. Tokenize `as` separately from identifiers such as `basic`.
fn use_leaves(
    tokens: &[&str],
    cursor: &mut usize,
    mut path: Vec<String>,
    leaves: &mut Vec<(Vec<String>, String)>,
) {
    while tokens.get(*cursor) == Some(&"::") {
        *cursor += 1;
    }
    loop {
        let token = *tokens.get(*cursor).expect("incomplete use tree");
        if token == "{" {
            *cursor += 1;
            while tokens.get(*cursor) != Some(&"}") {
                use_leaves(tokens, cursor, path.clone(), leaves);
                match tokens.get(*cursor) {
                    Some(&",") => *cursor += 1,
                    Some(&"}") => {}
                    other => panic!("unsupported use tree near {other:?}: {tokens:?}"),
                }
            }
            *cursor += 1;
            return;
        }
        assert!(
            token == "*"
                || token
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_'),
            "unsupported use token: {token}"
        );
        path.push(token.to_owned());
        *cursor += 1;
        if tokens.get(*cursor) == Some(&"::") {
            *cursor += 1;
        } else {
            break;
        }
    }
    let alias = if tokens.get(*cursor) == Some(&"as") {
        *cursor += 1;
        let alias = tokens
            .get(*cursor)
            .expect("missing import alias")
            .to_string();
        *cursor += 1;
        Some(alias)
    } else {
        None
    };
    let last = path.last().expect("use leaf").clone();
    if last == "self" && path.len() > 1 {
        path.pop();
    }
    let alias = alias.unwrap_or_else(|| path.last().unwrap().clone());
    leaves.push((path, alias));
}

fn call_lines(source: &str) -> Vec<usize> {
    let code = code_only(source);
    let helpers = [
        "show_singleline",
        "add_singleline",
        "add_singleline_sensitive",
        "add_sized_singleline",
    ];
    let mut modules: HashSet<String> = ["ime_focus".to_owned(), "super".to_owned()].into();
    let mut imported = HashSet::new();
    let mut names: BTreeSet<String> = helpers.iter().map(|name| (*name).to_owned()).collect();
    let mut leaves = Vec::new();
    let token_pattern = regex::Regex::new(r"[A-Za-z_][A-Za-z0-9_]*|::|[{},*]").unwrap();
    for import in regex::Regex::new(r"\buse\s+([^;]+);")
        .unwrap()
        .captures_iter(&code)
    {
        let tokens: Vec<_> = token_pattern
            .find_iter(&import[1])
            .map(|m| m.as_str())
            .collect();
        let mut cursor = 0;
        use_leaves(&tokens, &mut cursor, Vec::new(), &mut leaves);
        assert_eq!(cursor, tokens.len(), "unparsed import: {}", &import[1]);
    }
    // Resolve to a fixed point: imports may refer to earlier/later module or
    // function aliases, including re-aliases and nested `super::{...}` trees.
    loop {
        let before = modules.len() + imported.len();
        for (path, alias) in &leaves {
            let last = path.last().unwrap();
            let parent = path.iter().rev().nth(1);
            if last == "ime_focus" || modules.contains(last) {
                modules.insert(alias.clone());
            } else if parent.is_some_and(|parent| modules.contains(parent)) {
                if last == "*" && parent.is_none_or(|parent| parent != "super") {
                    imported.extend(helpers.iter().map(|name| (*name).to_owned()));
                } else if helpers.contains(&last.as_str()) || imported.contains(last) {
                    imported.insert(alias.clone());
                }
            } else if imported.contains(last) {
                imported.insert(alias.clone());
            }
        }
        if modules.len() + imported.len() == before {
            break;
        }
    }
    names.extend(imported.iter().cloned());
    let pattern = format!(
        r"\b(?P<path>(?:[A-Za-z_][A-Za-z0-9_]*\s*::\s*)*)(?P<name>{})\s*\(",
        names.into_iter().collect::<Vec<_>>().join("|")
    );
    regex::Regex::new(&pattern)
        .unwrap()
        .captures_iter(&code)
        .filter_map(|capture| {
            let path = capture.name("path").unwrap().as_str();
            let name = capture.name("name").unwrap().as_str();
            let module = path
                .split("::")
                .filter(|part| !part.trim().is_empty())
                .last()
                .map(str::trim);
            let is_helper = match module {
                Some(module) => modules.contains(module),
                None => imported.contains(name),
            };
            is_helper.then(|| {
                code[..capture.get(0).unwrap().start()]
                    .bytes()
                    .filter(|b| *b == b'\n')
                    .count()
            })
        })
        .collect()
}

#[test]
fn typing_inventory_detects_qualified_imported_and_multiline_helpers() {
    let source = r###"
        use crate::ime_focus::{add_singleline, show_singleline as show, self as focus};
        use crate::ime_focus::add_sized_singleline as sized;
        use crate::ime_focus as repair;
        crate::ime_focus::add_singleline(
            ui, draft, None, configure);
        ime_focus::show_singleline(ui, draft, None, configure);
        super::add_singleline(ui, draft, None, configure);
        add_singleline(ui, draft, None, configure);
        show(ui, draft, None, configure);
        sized(ui, size, draft, None, configure);
        focus::add_singleline_sensitive(ui, draft, None, configure);
        repair::add_singleline(ui, draft, None, configure);
        ui_history_field::show_singleline(ui, draft, None, id, entries, options, configure);
        // ime_focus::add_singleline(ui, draft, None, configure);
        let example = r#"ime_focus::add_singleline(ui, draft, None, configure)"#;
    "###;
    assert_eq!(call_lines(source).len(), 8);
}

#[test]
fn typing_inventory_detects_grouped_and_chained_aliases() {
    for source in [
        "use crate::{ime_focus as focus}; focus::add_singleline(ui, draft, None, configure);",
        "use crate::ime_focus as focus; use focus::add_singleline as field; field(ui, draft, None, configure);",
        "use super::{ime_focus as focus}; use focus::{show_singleline as field}; field(ui, draft, None, configure);",
        "use super::{self as focus}; use focus::add_singleline as field; field(ui, draft, None, configure);",
        "use field as renamed; use focus::add_singleline as field; use crate::{ime_focus as focus}; renamed(ui, draft, None, configure);",
        "use crate::{ime_focus::{self as focus, add_singleline as field}}; field(ui, draft, None, configure);",
        "use crate::ime_focus as focus; use focus::*; add_singleline(ui, draft, None, configure);",
        "use crate::ime_focus as focus; mod nested { use super::focus as repair; repair::add_singleline(ui, draft, None, configure); }",
        "use crate::ime_focus::add_singleline as field; mod nested { use super::field as renamed; renamed(ui, draft, None, configure); }",
    ] {
        assert_eq!(
            call_lines(source).len(),
            1,
            "missed use tree / alias: {source}"
        );
    }
}

#[test]
fn typing_singleline_helpers_have_exactly_one_test_or_reason() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    collect_sources(&root.join("src"), &mut files);
    files.sort();
    let mut typing_sources = String::new();
    for file in &files {
        if file.starts_with(root.join("src/app/tests/typing")) {
            typing_sources.push_str(&code_only(&std::fs::read_to_string(file).unwrap()));
            typing_sources.push('\n');
        }
    }
    let entries: Vec<_> = INPUT_TESTS.iter().chain(EXEMPTIONS).collect();
    let mut counts = vec![0; entries.len()];
    let mut violations = Vec::new();
    let advice = "新しい欄には src/app/tests/typing/ に入力テストを書いて INPUT_TESTS に登録するか、EXEMPTIONS に具体的な除外理由を足してください。";
    for entry in &entries {
        match (entry.test, entry.reason) {
            (Some(test), None) => {
                assert!(test.starts_with("typing_"));
                let pattern = format!(r"\bfn\s+{}\s*\(", regex::escape(test));
                assert!(
                    regex::Regex::new(&pattern)
                        .unwrap()
                        .is_match(&typing_sources),
                    "missing test {test}: {advice}"
                );
            }
            (None, Some(reason)) => assert!(!reason.trim().is_empty(), "empty reason: {advice}"),
            _ => panic!("inventory entry must have exactly one test or reason: {advice}"),
        }
    }
    for file in files {
        let relative = file
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if relative == "src/ime_focus.rs" {
            continue;
        }
        let source = std::fs::read_to_string(&file).unwrap();
        let lines: Vec<_> = source.lines().collect();
        for line in call_lines(&source) {
            let window = lines
                .iter()
                .skip(line)
                .take(6)
                .copied()
                .collect::<Vec<_>>()
                .join("\n");
            let matching: Vec<_> = entries
                .iter()
                .enumerate()
                .filter_map(|(index, entry)| {
                    (entry.path == relative && window.contains(entry.line_anchor)).then_some(index)
                })
                .collect();
            match matching.as_slice() {
                [index] => counts[*index] += 1,
                _ => violations.push(format!(
                    "{relative}:{} matches={matching:?}: {}",
                    line + 1,
                    lines[line].trim()
                )),
            }
        }
    }
    for (entry, count) in entries.iter().zip(&counts) {
        if *count != entry.expected_occurrences {
            violations.push(format!(
                "{} / {}: expected {}, found {}",
                entry.path, entry.line_anchor, entry.expected_occurrences, count
            ));
        }
    }
    assert!(violations.is_empty(), "{}\n{advice}", violations.join("\n"));
    println!(
        "typing inventory: total={} tested={} exempt={}",
        counts.iter().sum::<usize>(),
        INPUT_TESTS
            .iter()
            .map(|e| e.expected_occurrences)
            .sum::<usize>(),
        EXEMPTIONS
            .iter()
            .map(|e| e.expected_occurrences)
            .sum::<usize>()
    );
}
