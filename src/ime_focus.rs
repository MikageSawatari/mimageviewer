//! IME composition focus policy for egui text fields.

use std::time::{Duration, Instant};

const IME_STATE_ID: &str = "miv_ime_focus_state";
const IME_FOCUS_RETRY_ID: &str = "miv_ime_focus_retry";
const IME_EVENT_GRACE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, Debug, Default)]
struct ImeFocusState {
    composing: bool,
    last_event_at: Option<Instant>,
}

fn ime_state_id(viewport_id: egui::ViewportId) -> egui::Id {
    egui::Id::new((IME_STATE_ID, viewport_id))
}

fn focus_retry_id(viewport_id: egui::ViewportId, widget_id: egui::Id) -> egui::Id {
    egui::Id::new((IME_FOCUS_RETRY_ID, viewport_id, widget_id))
}

fn ime_input_active(ctx: &egui::Context) -> bool {
    let ime_events: Vec<egui::ImeEvent> = ctx.input(|input| {
        input
            .events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Ime(event) => Some(event.clone()),
                _ => None,
            })
            .collect()
    });
    let now = Instant::now();
    let viewport_id = ctx.viewport_id();
    ctx.data_mut(|data| {
        let id = ime_state_id(viewport_id);
        let mut state = data.get_temp::<ImeFocusState>(id).unwrap_or_default();
        for event in ime_events {
            state.last_event_at = Some(now);
            match event {
                egui::ImeEvent::Enabled => state.composing = true,
                egui::ImeEvent::Preedit(text) => state.composing = !text.is_empty(),
                egui::ImeEvent::Commit(_) | egui::ImeEvent::Disabled => {
                    state.composing = false;
                }
            }
        }
        data.insert_temp(id, state);
        state.composing
            || state
                .last_event_at
                .is_some_and(|at| now.saturating_duration_since(at) < IME_EVENT_GRACE)
    })
}

fn ime_focus_key_pressed(ctx: &egui::Context) -> bool {
    ctx.input(|input| {
        input.events.iter().any(|event| {
            matches!(
                event,
                egui::Event::Key {
                    key: egui::Key::Enter | egui::Key::Escape | egui::Key::Tab,
                    pressed: true,
                    ..
                }
            )
        })
    })
}

/// IME の Enter / Escape / Tab による `TextEdit` のフォーカス離脱を防止・復帰する。
///
/// `focus_request` は、呼び出し側が初回フォーカス等に使う request latch があれば渡す。
/// 復帰時は同 latch を次 frame 用に立て直す。latch を持たない欄では widget ID ごとの
/// 一時 latch をこのモジュールが所有する。IME 中でもマウスクリック等、対象キーの event を
/// 伴わない正当なフォーカス移動は復帰しない。IME 非入力時の通常の Tab traversal も維持する。
///
/// # mImageViewer の IME 対応範囲
///
/// コード上 TSF は使用していない。IMM32 を使う経路が 2 つある。
///
/// - main window と fullscreen viewport は winit / egui を通り、`Ime::Preedit` /
///   `Ime::Commit` / `Ime::Disabled` 相当の `egui::ImeEvent` を受ける。
/// - winit 管理外の独立 HWND である native video window は
///   `video/native_window.rs` で `WM_IME_STARTCOMPOSITION` /
///   `WM_IME_COMPOSITION` / `WM_IME_ENDCOMPOSITION` / `WM_IME_SETCONTEXT` を直接処理する。
///   `GCS_COMPSTR` と `GCS_RESULTSTR` を読み、`ISC_SHOWUICOMPOSITIONWINDOW` を落として OS の
///   composition window を抑止する。候補位置は `video/native_window_host.rs` が
///   `ImmSetCompositionWindow` / `ImmSetCandidateWindow` で設定する。
///
/// native 経路は `GCS_COMPATTR` / `GCS_COMPCLAUSE` を読まないため変換対象 clause を区別せず、
/// egui へ渡す preedit 文字列にも caret range がない。`IMR_RECONVERTSTRING` による再変換も
/// 実装していない。main window の message loop は winit が所有するため、TSF 対応は
/// winit / egui 上流の責務になる。所有する 1 HWND だけ TSF 化すると同一アプリ内で IME 挙動が
/// 二重化する一方、本アプリの文字入力は folder path、tag、bookmark name 等の補助用途であるため
/// 採用しない。ただし、このような focus / key routing の不具合は本アプリ側の責務であり、
/// IMM32 の範囲で修正する。「完全な IME には TSF が必要」を未修正の理由にしてはならない。
pub(crate) fn restore_focus_for_ime_key(
    ctx: &egui::Context,
    response: &egui::Response,
    mut focus_request: Option<&mut bool>,
) -> bool {
    let viewport_id = ctx.viewport_id();
    let retry_id = focus_retry_id(viewport_id, response.id);
    let retry_requested = match focus_request.as_deref_mut() {
        Some(request) => std::mem::take(request),
        None => ctx
            .data_mut(|data| data.remove_temp::<bool>(retry_id))
            .unwrap_or(false),
    };
    if retry_requested {
        response.request_focus();
    }

    let ime_active = ime_input_active(ctx);
    if ctx.memory(|memory| memory.has_focus(response.id)) {
        ctx.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    tab: ime_active,
                    horizontal_arrows: true,
                    vertical_arrows: true,
                    ..Default::default()
                },
            );
        });
    }

    if !response.lost_focus() || !ime_active {
        return false;
    }
    if !ime_focus_key_pressed(ctx) {
        return false;
    }

    response.request_focus();
    if let Some(request) = focus_request {
        *request = true;
    } else {
        ctx.data_mut(|data| data.insert_temp(retry_id, true));
    }
    ctx.request_repaint();
    true
}

pub(crate) fn show_singleline<'text>(
    ui: &mut egui::Ui,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::widgets::text_edit::TextEditOutput {
    let output = configure(egui::TextEdit::singleline(text)).show(ui);
    let _ = restore_focus_for_ime_key(ui.ctx(), &output.response, focus_request);
    output
}

pub(crate) fn add_singleline<'text>(
    ui: &mut egui::Ui,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::Response {
    let response = ui.add(configure(egui::TextEdit::singleline(text)));
    let _ = restore_focus_for_ime_key(ui.ctx(), &response, focus_request);
    response
}

pub(crate) fn add_sized_singleline<'text>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::Response {
    let response = ui.add_sized(size, configure(egui::TextEdit::singleline(text)));
    let _ = restore_focus_for_ime_key(ui.ctx(), &response, focus_request);
    response
}

pub(crate) fn add_enabled_singleline<'text>(
    ui: &mut egui::Ui,
    enabled: bool,
    text: &'text mut dyn egui::TextBuffer,
    focus_request: Option<&mut bool>,
    configure: impl FnOnce(egui::TextEdit<'text>) -> egui::TextEdit<'text>,
) -> egui::Response {
    let response = ui.add_enabled(enabled, configure(egui::TextEdit::singleline(text)));
    if enabled {
        let _ = restore_focus_for_ime_key(ui.ctx(), &response, focus_request);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn key_event(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    fn draw_text_fields(
        ctx: &egui::Context,
        first_id: egui::Id,
        second_id: egui::Id,
        first: &mut String,
        second: &mut String,
        first_focus_request: &mut bool,
    ) {
        egui::CentralPanel::default().show(ctx, |ui| {
            let _ = show_singleline(ui, first, Some(first_focus_request), |edit| {
                edit.id(first_id)
            });
            let _ = show_singleline(ui, second, None, |edit| edit.id(second_id));
        });
    }

    #[test]
    fn ime_tab_restores_the_editing_field_focus() {
        let ctx = egui::Context::default();
        let first_id = egui::Id::new("ime_tab_first");
        let second_id = egui::Id::new("ime_tab_second");
        let mut first = String::new();
        let mut second = String::new();
        let mut focus_request = true;
        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });

        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
                ],
                ..Default::default()
            },
            |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        );
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(egui::Key::Tab)],
                ..Default::default()
            },
            |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        );

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
        assert!(ctx.wants_keyboard_input());
    }

    #[test]
    fn tab_without_ime_keeps_normal_text_field_traversal() {
        let ctx = egui::Context::default();
        let first_id = egui::Id::new("plain_tab_first");
        let second_id = egui::Id::new("plain_tab_second");
        let mut first = String::new();
        let mut second = String::new();
        let mut focus_request = true;
        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });
        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(egui::Key::Tab)],
                ..Default::default()
            },
            |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        );

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(second_id));
    }

    #[test]
    fn text_cursor_arrow_keys_keep_the_editing_field_focus() {
        let ctx = egui::Context::default();
        let first_id = egui::Id::new("arrow_first");
        let second_id = egui::Id::new("arrow_second");
        let mut first = String::from("abc");
        let mut second = String::new();
        let mut focus_request = true;
        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });
        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });
        let _ = ctx.run(
            egui::RawInput {
                events: vec![key_event(egui::Key::ArrowDown)],
                ..Default::default()
            },
            |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        );

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
    }

    #[test]
    fn ime_activity_without_a_focus_key_does_not_undo_focus_change() {
        let ctx = egui::Context::default();
        let first_id = egui::Id::new("ime_click_first");
        let second_id = egui::Id::new("ime_click_second");
        let mut first = String::new();
        let mut second = String::new();
        let mut focus_request = true;
        let _ = ctx.run(Default::default(), |ctx| {
            draw_text_fields(
                ctx,
                first_id,
                second_id,
                &mut first,
                &mut second,
                &mut focus_request,
            );
        });
        ctx.memory_mut(|memory| memory.request_focus(second_id));
        let _ = ctx.run(
            egui::RawInput {
                events: vec![
                    egui::Event::Ime(egui::ImeEvent::Enabled),
                    egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
                ],
                ..Default::default()
            },
            |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            },
        );

        assert_eq!(ctx.memory(|memory| memory.focused()), Some(second_id));
    }

    #[test]
    fn ime_enter_and_escape_restore_the_editing_field_focus() {
        for key in [egui::Key::Enter, egui::Key::Escape] {
            let ctx = egui::Context::default();
            let first_id = egui::Id::new(("ime_focus_key_first", key));
            let second_id = egui::Id::new(("ime_focus_key_second", key));
            let mut first = String::new();
            let mut second = String::new();
            let mut focus_request = true;
            let _ = ctx.run(Default::default(), |ctx| {
                draw_text_fields(
                    ctx,
                    first_id,
                    second_id,
                    &mut first,
                    &mut second,
                    &mut focus_request,
                );
            });
            let terminal_ime_event = if key == egui::Key::Enter {
                egui::Event::Ime(egui::ImeEvent::Commit("あ".to_owned()))
            } else {
                egui::Event::Ime(egui::ImeEvent::Disabled)
            };
            let _ = ctx.run(
                egui::RawInput {
                    events: vec![
                        egui::Event::Ime(egui::ImeEvent::Enabled),
                        egui::Event::Ime(egui::ImeEvent::Preedit("あ".to_owned())),
                        terminal_ime_event,
                        key_event(key),
                    ],
                    ..Default::default()
                },
                |ctx| {
                    draw_text_fields(
                        ctx,
                        first_id,
                        second_id,
                        &mut first,
                        &mut second,
                        &mut focus_request,
                    );
                },
            );

            assert_eq!(ctx.memory(|memory| memory.focused()), Some(first_id));
        }
    }

    struct RawTextEditExemption {
        path: &'static str,
        line_anchor: &'static str,
        expected_occurrences: usize,
        reason: &'static str,
    }

    const RAW_TEXT_EDIT_EXEMPTIONS: &[RawTextEditExemption] = &[
        RawTextEditExemption {
            path: "src/ime_focus.rs",
            line_anchor: "let output = configure(",
            expected_occurrences: 1,
            reason: "共有 helper 自身が raw TextEdit を構築する境界",
        },
        RawTextEditExemption {
            path: "src/ime_focus.rs",
            line_anchor: "let response = ui.add(configure(",
            expected_occurrences: 1,
            reason: "共有 helper 自身が raw TextEdit を構築する境界",
        },
        RawTextEditExemption {
            path: "src/ime_focus.rs",
            line_anchor: "let response = ui.add_sized(size, configure(",
            expected_occurrences: 1,
            reason: "共有 helper 自身が raw TextEdit を構築する境界",
        },
        RawTextEditExemption {
            path: "src/ime_focus.rs",
            line_anchor: "let response = ui.add_enabled(enabled, configure(",
            expected_occurrences: 1,
            reason: "共有 helper 自身が raw TextEdit を構築する境界",
        },
        RawTextEditExemption {
            path: "src/egui_focus_policy.rs",
            line_anchor: "singleline(first)",
            expected_occurrences: 1,
            reason: "通常 Tab traversal 自体を検証する低レベル test fixture",
        },
        RawTextEditExemption {
            path: "src/egui_focus_policy.rs",
            line_anchor: "singleline(second)",
            expected_occurrences: 1,
            reason: "通常 Tab traversal 自体を検証する低レベル test fixture",
        },
        RawTextEditExemption {
            path: "src/keymap.rs",
            line_anchor: "singleline(&mut text)",
            expected_occurrences: 4,
            reason: "keymap の TextEdit 入力抑止を直接検証する低レベル test fixture",
        },
        RawTextEditExemption {
            path: "src/keymap.rs",
            line_anchor: "singleline(&mut first)",
            expected_occurrences: 2,
            reason: "keymap と通常 Tab traversal の境界を直接検証する test fixture",
        },
        RawTextEditExemption {
            path: "src/keymap.rs",
            line_anchor: "singleline(&mut second)",
            expected_occurrences: 2,
            reason: "keymap と通常 Tab traversal の境界を直接検証する test fixture",
        },
        RawTextEditExemption {
            path: "src/ui_main.rs",
            line_anchor: "singleline(&mut self.color_filter.hex_input)",
            expected_occurrences: 1,
            reason: "RRGGBB 専用の ASCII 制約入力で日本語自由入力ではない",
        },
        RawTextEditExemption {
            path: "src/ui_main.rs",
            line_anchor: "singleline(&mut self.address)",
            expected_occurrences: 1,
            reason: "snapshot 表示中だけ描く disabled 読み取り専用欄",
        },
        RawTextEditExemption {
            path: "src/ui_text.rs",
            line_anchor: "multiline(&mut t.text)",
            expected_occurrences: 1,
            reason: "Enter 改行と caret 操作を所有する注釈本文の複数行 editor",
        },
        RawTextEditExemption {
            path: "src/ui_dialogs/cache_manager.rs",
            line_anchor: "singleline(&mut days_str)",
            expected_occurrences: 1,
            reason: "u32 日数だけを受け付ける数値入力",
        },
        RawTextEditExemption {
            path: "src/ui_dialogs/preferences/pages.rs",
            line_anchor: "singleline(&mut state.command_key_filter)",
            expected_occurrences: 1,
            reason: "F11 等のキー記法だけを検索する ASCII 入力",
        },
        RawTextEditExemption {
            path: "src/ui_dialogs/preferences/pages.rs",
            line_anchor: "singleline(input).hint_text(if idx == 0",
            expected_occurrences: 1,
            reason: "Ctrl+F 等の key chord grammar 専用入力",
        },
        RawTextEditExemption {
            path: "src/ui_dialogs/preferences/pages.rs",
            line_anchor: concat!(".text_edit_", "singleline(&mut state.exif_add_tag_input)"),
            expected_occurrences: 1,
            reason: "EXIF の ASCII 内部 tag 名専用入力",
        },
        RawTextEditExemption {
            path: "src/video/native_presenter/overlay_draw.rs",
            line_anchor: "multiline(&mut state.textarea)",
            expected_occurrences: 1,
            reason: "時刻付き行の paste と独自 focus lifecycle を持つ複数行 editor",
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
             crate::ime_focus の helper を使ってください。意図的に対象外とする場合は、\
             ime_focus.rs の RAW_TEXT_EDIT_EXEMPTIONS に短い理由付きで追加してください。",
            violations.join("\n")
        );
    }
}
