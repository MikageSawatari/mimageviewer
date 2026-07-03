use super::*;
use crate::keymap::{
    BindingConflict, BindingConflictKind, Chord, KeyAction, KeyContext, KeyName, KeyTrigger,
    Keymap, MenuCommandId, MenuCommandOrderSettings, MenuLayoutSettings, TopMenuId,
    menu_command_can_be_hidden, menu_command_spec, menu_commands_for_parent,
    parse_chord_for_action,
};
use crate::ring_shortcut::{
    MouseGestureDirection, RightDragContext, RightDragMode, RingActionId, RingDirection,
    RingShortcutContext, RingShortcutSettings, format_mouse_gesture_pattern,
    mouse_gesture_direction_from_delta,
};
use crate::settings::{
    self, AiFeatureMode, ArchiveFileHandling, CachePolicy, FullscreenFitMode, FullscreenJumpMode,
    Parallelism, ReadingDirection, ReadingFlow, SortOrder, SpreadMode, StartupFolderMode, UiTheme,
};
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashSet};

pub(super) fn page_general(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.label(egui::RichText::new("テーマ").strong());
    ui.add_space(4.0);
    // Standard は旧設定互換のため enum に残っているが、UI では表示しない (System = 追従 or Light)。
    // 保存値が Standard になっていたら Light に寄せる (System に勝手に戻すのは避ける)。
    if state.settings.ui_theme == UiTheme::Standard {
        state.settings.ui_theme = UiTheme::Light;
    }
    ui.radio_value(
        &mut state.settings.ui_theme,
        UiTheme::System,
        "システムに合わせる (Windows のアプリ用色に追従)",
    );
    ui.radio_value(
        &mut state.settings.ui_theme,
        UiTheme::Light,
        "ライト (サムネイル白基調 / フルスクリーン黒)",
    );
    ui.radio_value(
        &mut state.settings.ui_theme,
        UiTheme::Dark,
        "ダーク (全体暗色 / フルスクリーン黒)",
    );
    ui.add_space(12.0);
    ui.label(
        egui::RichText::new(
            "フルスクリーン表示は画像鑑賞のためテーマに関係なく黒背景になります。\n\
             B キーで透過画像の背景色を循環させられます (黒 → 白 → 市松)。",
        )
        .weak(),
    );

    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);
    ui.label(egui::RichText::new("表示時の AI 処理 (アップスケール / ノイズ除去)").strong());
    ui.add_space(4.0);
    for &mode in AiFeatureMode::all() {
        let response = ui.radio_value(
            &mut state.settings.ai_feature_mode,
            mode,
            format!("{} - {}", mode.label(), mode.description()),
        );
        if matches!(mode, AiFeatureMode::HighQuality) {
            response.on_hover_text("GPU 負荷が高く、環境によっては表示が重くなります。");
        }
    }
    ui.label(
        egui::RichText::new(
            "画像を見るときの自動アップスケール / ノイズ除去だけを切り替えます。\n\
             表示が重い、オンボード GPU、低スペック環境では「なし」を推奨します。\n\
             軽量は高速汎用と漫画トーン保持モデルのみを使用し、ノイズ除去は実行しません。\n\
             高画質では写真・イラスト・質感保持モデルとノイズ除去も選択できます。\n\
             消しゴムや補正の被写体マスクなど編集ツールの AI、動画アップスケールは\n\
             この設定の影響を受けません。",
        )
        .weak(),
    );
    if ui
        .link("処理時間の目安を開く")
        .on_hover_text("ブラウザでマニュアルの AI 処理時間表を開きます。")
        .clicked()
    {
        let url = crate::ui_helpers::manual_url("settings.html", Some("ai-processing-time"));
        crate::ui_helpers::open_url(&url);
    }

    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);
    ui.label(egui::RichText::new("ZIP/PDF/対応アーカイブ").strong());
    ui.add_space(4.0);
    ui.radio_value(
        &mut state.settings.auto_fullscreen_zip_pdf,
        false,
        "開いたとき、ページ一覧を表示",
    );
    ui.radio_value(
        &mut state.settings.auto_fullscreen_zip_pdf,
        true,
        "開いたとき、ページをフルスクリーン表示",
    );
    ui.add_enabled_ui(state.settings.auto_fullscreen_zip_pdf, |ui| {
        ui.checkbox(
            &mut state.settings.auto_fullscreen_image_folders,
            "画像のみの通常フォルダも、ページをフルスクリーン表示",
        );
    });
    ui.label(
        egui::RichText::new(
            "ページをフルスクリーン表示する場合、開く位置 (1 ページ目 / 続きから) は\
             「履歴と復元」設定に従います。フルスクリーン中の Enter / Esc で元の一覧へ戻り、\
             Backspace でその本またはフォルダのページ一覧を表示します。外部ファイラや SendTo から\
             開いた本・画像のみの通常フォルダにも適用されます。",
        )
        .weak(),
    );

    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);
    ui.label(egui::RichText::new("画像ビューア").strong());
    ui.add_space(4.0);
    ui.checkbox(
        &mut state.settings.detached_viewer_open_images_in_window,
        "画像/動画を別ウィンドウで開く",
    );
    ui.label(
        egui::RichText::new(
            "画像は開くたびに別ウィンドウを残します。動画は専用の動画ウィンドウを再利用し、F12 で現在の動画だけメイン表示へ一時切替できます。",
        )
        .weak(),
    );
    ui.label(
        egui::RichText::new(
            "※ この設定で開いた別ウィンドウでは、消しゴム・補正レイヤーなどの画像編集機能は利用できません。全体の色調補正やポストフィルタなどの表示調整は利用できます。",
        )
        .weak(),
    );
}

pub(super) fn page_startup_folder(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.label(egui::RichText::new("起動時に開く場所").strong());
    ui.add_space(4.0);

    ui.radio_value(
        &mut state.settings.startup_folder_mode,
        StartupFolderMode::Previous,
        StartupFolderMode::Previous.label(),
    );
    ui.radio_value(
        &mut state.settings.startup_folder_mode,
        StartupFolderMode::Desktop,
        StartupFolderMode::Desktop.label(),
    );
    ui.radio_value(
        &mut state.settings.startup_folder_mode,
        StartupFolderMode::Drives,
        StartupFolderMode::Drives.label(),
    );
    ui.radio_value(
        &mut state.settings.startup_folder_mode,
        StartupFolderMode::ReadingHistory,
        StartupFolderMode::ReadingHistory.label(),
    );
    ui.radio_value(
        &mut state.settings.startup_folder_mode,
        StartupFolderMode::Specific,
        StartupFolderMode::Specific.label(),
    );

    ui.add_space(8.0);
    ui.add_enabled_ui(
        state.settings.startup_folder_mode == StartupFolderMode::Specific,
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("指定フォルダ:");
                let edit_width = (ui.available_width() - 120.0).clamp(180.0, 420.0);
                let mut output = egui::TextEdit::singleline(&mut state.startup_folder_path_input)
                    .desired_width(edit_width)
                    .hint_text("例: D:\\Images")
                    .show(ui);
                let menu_changed = crate::ui_helpers::singleline_text_edit_context_menu(
                    ui,
                    &mut output,
                    &mut state.startup_folder_path_input,
                );
                let response = output.response;
                if response.changed() || menu_changed {
                    let trimmed = state.startup_folder_path_input.trim();
                    state.settings.startup_folder_path = if trimmed.is_empty() {
                        None
                    } else {
                        Some(std::path::PathBuf::from(trimmed))
                    };
                }
                if ui.button("フォルダを開く").clicked()
                    && let Some(dir) = rfd::FileDialog::new().pick_folder()
                {
                    state.startup_folder_path_input = dir.display().to_string();
                    state.settings.startup_folder_path = Some(dir);
                    state.settings.startup_folder_mode = StartupFolderMode::Specific;
                }
            });
        },
    );
    ui.label(
        egui::RichText::new(
            "指定フォルダが開けない場合はデスクトップに、デスクトップも取得できない場合は前回フォルダにフォールバックします。",
        )
        .weak(),
    );
}

pub(super) fn page_explorer_integration(ui: &mut egui::Ui, state: &mut PreferencesState) {
    refresh_send_to_status_if_needed(state);

    ui.label(egui::RichText::new("右クリックメニュー").strong());
    ui.add_space(4.0);
    ui.checkbox(
        &mut state.settings.use_native_shell_context_menu,
        "実ファイル/実フォルダでは Windows 標準の右クリックメニューを使う",
    )
    .on_hover_text(
        "右クリック時の表示だけを切り替えます。ZIP/PDF 内ページなど仮想アイテムは mIV 独自メニューを使います。\
         Ctrl+C/X/V のファイル操作は、この設定に関わらず Windows 標準の動作を使います。",
    );
    ui.add_space(10.0);

    ui.label(
        "Windows の「送る」メニューに mImageViewer を追加します。\n\
         エクスプローラでファイルやフォルダを右クリック → 送る → mImageViewer から開けます。",
    );
    ui.add_space(10.0);

    ui.label(egui::RichText::new("SendTo").strong());
    ui.add_space(4.0);

    match &state.send_to_status {
        Some(Ok(status)) => {
            if status.registered && status.target_matches {
                ui.colored_label(egui::Color32::from_rgb(120, 200, 120), "登録済みです。");
            } else if status.registered {
                ui.colored_label(
                    egui::Color32::from_rgb(230, 190, 90),
                    "登録済みですが、現在の mImageViewer とは別の実行ファイルを指しています。",
                );
            } else {
                ui.label("未登録です。");
            }

            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(format!(
                    "ショートカット: {}",
                    status.shortcut_path.display()
                ))
                .monospace()
                .weak(),
            );
            ui.label(
                egui::RichText::new(format!("登録先: {}", status.expected_target.display()))
                    .monospace()
                    .weak(),
            );
            if let Some(target) = &status.target
                && !status.target_matches
            {
                ui.label(
                    egui::RichText::new(format!("現在のリンク先: {}", target.display()))
                        .monospace()
                        .weak(),
                );
            }
        }
        Some(Err(err)) => {
            ui.colored_label(
                egui::Color32::from_rgb(230, 120, 120),
                format!("状態を確認できませんでした: {err}"),
            );
        }
        None => {}
    }

    ui.add_space(8.0);
    ui.horizontal_wrapped(|ui| {
        if ui.button("SendTo に登録").clicked() {
            state.send_to_status = Some(crate::explorer_integration::register_send_to_shortcut());
            state.send_to_action_message = Some(match &state.send_to_status {
                Some(Ok(_)) => "SendTo に登録しました。".to_string(),
                Some(Err(err)) => format!("登録に失敗しました: {err}"),
                None => String::new(),
            });
        }
        if ui.button("SendTo から削除").clicked() {
            state.send_to_status = Some(crate::explorer_integration::unregister_send_to_shortcut());
            state.send_to_action_message = Some(match &state.send_to_status {
                Some(Ok(_)) => "SendTo から削除しました。".to_string(),
                Some(Err(err)) => format!("削除に失敗しました: {err}"),
                None => String::new(),
            });
        }
        if ui.button("SendTo フォルダを開く").clicked()
            && let Some(Ok(status)) = &state.send_to_status
        {
            open_in_explorer(&status.send_to_dir);
        }
        if ui.button("状態を更新").clicked() {
            refresh_send_to_status(state);
        }
    });

    if let Some(msg) = &state.send_to_action_message {
        ui.add_space(6.0);
        let color = if msg.contains("失敗") {
            egui::Color32::from_rgb(230, 120, 120)
        } else {
            egui::Color32::from_rgb(120, 200, 120)
        };
        ui.colored_label(color, msg);
    }

    ui.add_space(10.0);
    ui.label(
        egui::RichText::new(
            "SendTo には現在起動している mImageViewer の実行ファイルを登録します。\n\
             インストーラ版では launcher、ポータブル版や開発実行では現在の exe を使います。",
        )
        .weak(),
    );
}

fn refresh_send_to_status_if_needed(state: &mut PreferencesState) {
    if state.send_to_status.is_none() {
        refresh_send_to_status(state);
    }
}

fn refresh_send_to_status(state: &mut PreferencesState) {
    state.send_to_status = Some(crate::explorer_integration::send_to_shortcut_status());
}

pub(super) fn page_thumbnail(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;
    ui.checkbox(
        &mut s.thumb_idle_upgrade,
        "アイドル時にキャッシュ由来のサムネイルを高画質化する",
    );
    ui.label(
        "  スクロール停止後、キャッシュ復元 (WebP q=75) のサムネイルを\n  \
         元画像から再デコードして差し替えます。visible 側から順次処理。",
    );

    ui.add_space(12.0);
    ui.label(egui::RichText::new("サムネイル情報ツールチップ").strong());
    ui.label(
        "サムネイル表示で選択中セルの下に表示する内容です。長さなどは必要なときにバックグラウンドで読み込みます。",
    );
    ui.add_space(4.0);
    ui.checkbox(&mut s.thumb_tooltip_show_filename, "ファイル名");
    ui.checkbox(&mut s.thumb_tooltip_show_image_dimensions, "画像解像度");
    ui.checkbox(&mut s.thumb_tooltip_show_video_duration, "長さ");
    ui.checkbox(&mut s.thumb_tooltip_show_kind, "種類");
    ui.checkbox(&mut s.thumb_tooltip_show_file_size, "サイズ");
    ui.checkbox(&mut s.thumb_tooltip_show_modified, "更新日時");
    ui.checkbox(&mut s.thumb_tooltip_show_created, "作成日時");
    ui.checkbox(&mut s.thumb_tooltip_show_video_dimensions, "動画解像度");
    ui.checkbox(&mut s.thumb_tooltip_show_video_codec, "コーデック");
    ui.checkbox(&mut s.thumb_tooltip_show_location, "親フォルダ名");
    ui.checkbox(&mut s.thumb_tooltip_show_full_location, "場所");
    ui.checkbox(
        &mut s.thumb_tooltip_show_reading_history_last_read,
        "読書履歴: 最終閲覧",
    );
    ui.checkbox(
        &mut s.thumb_tooltip_show_reading_history_progress,
        "読書履歴: 既読位置",
    );
}

pub(super) fn page_slideshow(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.horizontal(|ui| {
        ui.label("ページ送り間隔:");
        ui.add(
            egui::Slider::new(&mut state.settings.slideshow_interval_secs, 0.5..=30.0)
                .suffix(" 秒")
                .fixed_decimals(1),
        );
    });

    ui.add_space(8.0);
    ui.label("連結読みスライドショー:");
    ui.horizontal(|ui| {
        ui.label("待機時間:");
        ui.add(
            egui::Slider::new(
                &mut state.settings.slideshow_continuous_wait_secs,
                0.1..=30.0,
            )
            .suffix(" 秒")
            .fixed_decimals(1),
        );
    });
    ui.horizontal(|ui| {
        ui.label("スクロール時間:");
        ui.add(
            egui::Slider::new(
                &mut state.settings.slideshow_continuous_scroll_secs,
                0.0..=5.0,
            )
            .suffix(" 秒")
            .fixed_decimals(1),
        );
    });
    ui.horizontal(|ui| {
        ui.label("1回のスクロール量:");
        ui.add(
            egui::Slider::new(
                &mut state.settings.slideshow_continuous_scroll_percent,
                1..=100,
            )
            .suffix(" %"),
        );
    });

    ui.add_space(8.0);
    ui.label("フォルダの最後まで進んだら:");
    use crate::settings::SlideshowEndAction;
    let action = &mut state.settings.slideshow_end_action;
    ui.radio_value(
        action,
        SlideshowEndAction::LoopFolder,
        SlideshowEndAction::LoopFolder.label(),
    );
    ui.radio_value(
        action,
        SlideshowEndAction::NextFolder,
        SlideshowEndAction::NextFolder.label(),
    );
    ui.radio_value(
        action,
        SlideshowEndAction::Stop,
        SlideshowEndAction::Stop.label(),
    );
    ui.add_space(2.0);
    let hint = egui::Color32::from_gray(140);
    ui.label(
        egui::RichText::new("「次のフォルダへ進む」は、移動先に画像が1枚も無ければ停止します")
            .size(11.0)
            .color(hint),
    );
    ui.label(
        egui::RichText::new("スライドショー中、動画は自動でスキップします")
            .size(11.0)
            .color(hint),
    );

    ui.add_space(6.0);
    ui.label(
        egui::RichText::new("フルスクリーンで S キーまたは ▶ ボタンで開始 / 停止")
            .size(11.0)
            .color(hint),
    );
}

pub(super) fn page_capture(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label("Ctrl+S で保存するキャプチャの形式と保存先を設定します。");
    ui.add_space(8.0);

    egui::ComboBox::from_label("保存形式")
        .selected_text(s.capture_format.label())
        .show_ui(ui, |ui| {
            for format in [
                crate::capture::CaptureFormat::Png,
                crate::capture::CaptureFormat::Jpeg95,
                crate::capture::CaptureFormat::Jpeg85,
                crate::capture::CaptureFormat::Jpeg75,
            ] {
                ui.selectable_value(&mut s.capture_format, format, format.label());
            }
        });

    ui.add_space(8.0);
    ui.label("保存先フォルダ");
    ui.horizontal_wrapped(|ui| {
        let edit_width = (ui.available_width() - 190.0).clamp(180.0, 360.0);
        let mut output = egui::TextEdit::singleline(&mut state.capture_output_dir_input)
            .desired_width(edit_width)
            .hint_text(crate::capture::default_output_dir().display().to_string())
            .show(ui);
        let menu_changed = crate::ui_helpers::singleline_text_edit_context_menu(
            ui,
            &mut output,
            &mut state.capture_output_dir_input,
        );
        let response = output.response;
        if response.changed() || menu_changed {
            let trimmed = state.capture_output_dir_input.trim();
            s.capture_output_dir = if trimmed.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(trimmed))
            };
        }
        if ui.button("既定に戻す").clicked() {
            state.capture_output_dir_input.clear();
            s.capture_output_dir = None;
        }
        if ui.button("フォルダを開く").clicked() {
            let dir = s
                .capture_output_dir
                .clone()
                .unwrap_or_else(crate::capture::default_output_dir);
            crate::capture::open_output_dir_async(dir);
        }
    });

    let effective = s
        .capture_output_dir
        .clone()
        .unwrap_or_else(crate::capture::default_output_dir);
    ui.label(
        egui::RichText::new(format!("実際の保存先: {}", effective.display()))
            .size(11.0)
            .color(egui::Color32::from_gray(140)),
    );
}

pub(super) fn page_operation_behavior(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let settings = &mut state.settings.ring_shortcuts;

    ui.small("右ドラッグ操作中に表示するガイドを切り替えます。オフにしても、割り当てたリングショートカットやマウスジェスチャ自体は実行されます。");
    ui.add_space(8.0);
    ui.checkbox(
        &mut settings.mouse_ring_help_visible,
        "右ドラッグのリングショートカットガイドを表示する",
    );
    ui.checkbox(
        &mut settings.mouse_gesture_help_visible,
        "マウスジェスチャの登録一覧 / 入力中表示を表示する",
    );
}

pub(super) fn page_right_drag_modes(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let settings = &mut state.settings.ring_shortcuts;

    ui.label(egui::RichText::new("マウス右ドラッグ").strong());
    ui.small("右ドラッグの用途を文脈ごとに選びます。未使用の文脈では、右クリックや既存の短押し動作を優先します。");
    egui::Grid::new("right_drag_mode_grid")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            for &context in RightDragContext::all() {
                right_drag_mode_combo(ui, settings, context);
            }
        });
}

pub(super) fn page_ring_shortcut_assignments(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    context: RingShortcutContext,
) {
    ui.small("ゲームパッド X リングと、右ドラッグ mode をリングショートカットにした文脈で使う 8 方向スロットです。");
    ring_shortcut_context_editor(ui, &mut state.settings.ring_shortcuts, context);
}

pub(super) fn page_gamepad_assignments(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    context: RingShortcutContext,
) {
    ui.small("固定ボタンは既定動作で使い、X+方向リングだけ編集できます。");
    ui.add_space(8.0);
    gamepad_layout_preview(ui, context);
    ui.add_space(12.0);
    ui.label(egui::RichText::new("X+方向リング").strong());
    let preview_profile = state.settings.ring_shortcuts.profile(context).clone();
    gamepad_ring_preview(ui, state, &preview_profile, context);
    ui.add_space(8.0);
    ring_shortcut_context_editor_without_preview(ui, &mut state.settings.ring_shortcuts, context);
}

pub(super) fn page_mouse_gesture_bindings(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    context: RightDragContext,
) {
    ui.label(egui::RichText::new(context.label()).strong());
    ui.small("右ドラッグ mode をマウスジェスチャにした文脈で使います。同じ方向の連続入力は 1 stroke に圧縮され、最大 4 stroke まで登録できます。");
    mouse_gesture_context_editor(ui, state, context);
}

pub(super) fn page_mouse_buttons(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.small("戻る/進むボタンは、リングショートカットと同じ単発アクションから選べます。");
    ui.small("割り当てはグリッド、画像フルスクリーン、動画フルスクリーンで別々に保存します。");

    ui.add_space(8.0);
    for &context in RingShortcutContext::all() {
        mouse_button_context_editor(ui, state, context);
    }
}

fn command_filter_controls(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.horizontal_wrapped(|ui| {
        ui.label("検索:");
        ui.label("操作");
        ui.add(
            egui::TextEdit::singleline(&mut state.command_filter)
                .desired_width(220.0)
                .hint_text("操作名 / 説明 / 場所"),
        );
        ui.label("キー");
        ui.add(
            egui::TextEdit::singleline(&mut state.command_key_filter)
                .desired_width(140.0)
                .hint_text("例: F11 / Numpad9"),
        );
        if ui.button("クリア").clicked() {
            state.command_filter.clear();
            state.command_key_filter.clear();
        }
    });
}

pub(super) fn page_command_overview(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.small("現在のキーボード、右ドラッグ、マウスボタン、ゲームパッド X リングの割り当てをまとめて確認します。左端の「編集」で割り当て編集ダイアログを開きます。");
    ui.add_space(8.0);

    let keymap = Keymap::from_settings(&state.settings.keymap);
    let conflicts = keymap.binding_conflicts();
    command_conflict_summary(ui, state, &conflicts);

    ui.add_space(8.0);
    command_filter_controls(ui, state);

    ui.add_space(12.0);
    let conflicted = conflicted_actions(&conflicts);
    let rows = command_overview_rows(state, &keymap, &conflicted);
    egui::Grid::new("operation_command_overview_actions")
        .num_columns(5)
        .spacing([10.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.strong("編集");
            ui.strong("操作");
            ui.strong("場所");
            ui.strong("割り当て");
            ui.strong("状態");
            ui.end_row();

            for row in rows {
                if ui.small_button("編集").clicked() {
                    match row.target {
                        OperationOverviewTarget::Key(action) => {
                            open_operation_assignment_editor(
                                state,
                                OperationAssignmentTarget::Key(action),
                                OperationAssignmentTab::Keyboard,
                            );
                        }
                        OperationOverviewTarget::Ring { context, action } => {
                            open_operation_assignment_editor(
                                state,
                                OperationAssignmentTarget::Ring { context, action },
                                OperationAssignmentTab::RingPad,
                            );
                        }
                    }
                }
                ui.label(row.label).on_hover_text(row.hover);
                ui.label(row.context);
                assignment_summary(ui, &row.assignments);
                if row.status.is_empty() {
                    ui.label(egui::RichText::new("既定").weak());
                } else {
                    let color = if row.conflicted {
                        egui::Color32::from_rgb(220, 120, 80)
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.label(egui::RichText::new(row.status).color(color));
                }
                ui.end_row();
            }
        });
    ui.small("未設定のリング / マウス操作を追加するときは、上部の各タブまたは編集ダイアログから割り当ててください。");
}

#[derive(Clone, Debug)]
enum OperationOverviewTarget {
    Key(KeyAction),
    Ring {
        context: RingShortcutContext,
        action: RingActionId,
    },
}

#[derive(Clone, Debug)]
struct OperationOverviewRow {
    label: String,
    context: String,
    assignments: Vec<(&'static str, Vec<String>)>,
    status: String,
    conflicted: bool,
    hover: String,
    target: OperationOverviewTarget,
}

fn command_overview_rows(
    state: &PreferencesState,
    keymap: &Keymap,
    conflicted: &HashSet<KeyAction>,
) -> Vec<OperationOverviewRow> {
    let mut rows = Vec::new();
    for &action in KeyAction::all()
        .iter()
        .filter(|action| action.is_user_facing())
    {
        let ring_bindings = ring_bindings_for_key_action(action);
        let matches_key_context = operation_keyboard_context_filter_matches(
            state.operation_keyboard_context,
            action.context(),
        );
        let matches_ring_context = ring_bindings.iter().any(|(context, _)| {
            ring_context_matches_key_context(*context, state.operation_keyboard_context)
        });
        if !matches_key_context && !matches_ring_context {
            continue;
        }
        let overridden = state
            .settings
            .keymap
            .override_chord_labels(action)
            .is_some();
        let key_labels = keymap.chord_labels(action);
        if !command_action_matches_filter(action, state.command_filter.trim())
            || !command_key_labels_match_filter(&key_labels, state.command_key_filter.trim())
        {
            continue;
        }
        let is_conflicted = conflicted.contains(&action);
        let mut status = Vec::new();
        if overridden {
            status.push("上書き");
        }
        if is_conflicted {
            status.push("競合");
        }
        let mut assignments = vec![("キー", key_labels)];
        if !ring_bindings.is_empty() {
            let multi_context = ring_bindings.len() > 1;
            let mut ring = Vec::new();
            let mut mouse = Vec::new();
            let mut pad = Vec::new();
            for (context, ring_action) in ring_bindings {
                ring.extend(contextual_assignment_labels(
                    ring_assignment_labels(&state.settings.ring_shortcuts, context, &ring_action),
                    context,
                    multi_context,
                ));
                mouse.extend(contextual_assignment_labels(
                    mouse_assignment_labels(&state.settings.ring_shortcuts, context, &ring_action),
                    context,
                    multi_context,
                ));
                let pad_for_context = gamepad_ring_assignment_labels(
                    &state.settings.ring_shortcuts,
                    context,
                    &ring_action,
                );
                pad.extend(contextual_assignment_labels(
                    pad_for_context,
                    context,
                    multi_context,
                ));
            }
            if !ring.is_empty() || !mouse.is_empty() || !pad.is_empty() {
                assignments.extend([("リング", ring), ("マウス", mouse), ("パッド", pad)]);
                status.push("設定あり");
            }
        }
        rows.push(OperationOverviewRow {
            label: compact_key_action_label(action),
            context: key_action_context_label(action).to_owned(),
            assignments,
            status: status.join(" / "),
            conflicted: is_conflicted,
            hover: action.description().to_owned(),
            target: OperationOverviewTarget::Key(action),
        });
    }

    for &context in RingShortcutContext::all() {
        if !ring_context_matches_key_context(context, state.operation_keyboard_context) {
            continue;
        }
        for action in RingActionId::available_for_context(context)
            .into_iter()
            .filter(|action| *action != RingActionId::None)
        {
            if ring_action_has_key_action(context, &action) {
                continue;
            }
            if !state.command_key_filter.trim().is_empty() {
                continue;
            }
            let label = compact_operation_label(action.label_for_context(context));
            let hover = ring_action_detail_label(&action, context).to_owned();
            if !operation_text_matches_filter(
                &label,
                &hover,
                context.label(),
                state.command_filter.trim(),
            ) {
                continue;
            }
            let ring = ring_assignment_labels(&state.settings.ring_shortcuts, context, &action);
            let mouse = mouse_assignment_labels(&state.settings.ring_shortcuts, context, &action);
            let pad =
                gamepad_ring_assignment_labels(&state.settings.ring_shortcuts, context, &action);
            let has_assignment = !ring.is_empty() || !mouse.is_empty() || !pad.is_empty();
            rows.push(OperationOverviewRow {
                label,
                context: context.label().to_owned(),
                assignments: vec![("リング", ring), ("マウス", mouse), ("パッド", pad)],
                status: if has_assignment {
                    "設定あり".to_owned()
                } else {
                    "未設定".to_owned()
                },
                conflicted: false,
                hover,
                target: OperationOverviewTarget::Ring { context, action },
            });
        }
    }

    rows.sort_by(|a, b| {
        a.context
            .cmp(&b.context)
            .then_with(|| natural_operation_label_cmp(&a.label, &b.label))
            .then_with(|| a.hover.cmp(&b.hover))
    });
    rows
}

fn ring_context_matches_key_context(
    context: RingShortcutContext,
    filter: Option<crate::keymap::KeyContext>,
) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    match (context, filter) {
        (RingShortcutContext::Grid, crate::keymap::KeyContext::Grid) => true,
        (RingShortcutContext::ImageFullscreen, crate::keymap::KeyContext::FsImage) => true,
        (RingShortcutContext::VideoFullscreen, crate::keymap::KeyContext::FsVideo) => true,
        _ => false,
    }
}

fn natural_operation_label_cmp(a: &str, b: &str) -> Ordering {
    crate::ui_helpers::natural_sort_key(a)
        .cmp(&crate::ui_helpers::natural_sort_key(b))
        .then_with(|| a.cmp(b))
}

fn compact_key_action_label(action: KeyAction) -> String {
    let desc = action.description();
    let label = match action {
        KeyAction::GlobalLocalSearch => "現在地フィルタ",
        KeyAction::GlobalFavSearch => "コンテナ検索",
        KeyAction::GlobalMetadataSearch => "アイテム検索",
        KeyAction::GlobalOpenFolder => "フォルダを開く",
        KeyAction::ToggleDetachedViewerMode => "別ウィンドウ",
        KeyAction::HelpShowContextShortcuts => "ショートカット一覧",
        KeyAction::GridSelectAll => "表示中を全チェック",
        _ => desc,
    };
    compact_operation_label(label)
}

fn key_action_context_label(action: KeyAction) -> &'static str {
    operation_keyboard_context_filter_label(operation_keyboard_context_filter_for_context(
        action.context(),
    ))
}

fn ring_action_detail_label(action: &RingActionId, context: RingShortcutContext) -> &'static str {
    match action {
        RingActionId::GridSelectAll => "表示中のチェック可能な項目をすべてチェックする",
        RingActionId::ImageSpreadShiftLeft => {
            "見開きを画面左方向へ1ページずらす。右→左の見開きでは前/次の意味を反転します"
        }
        RingActionId::ImageSpreadShiftRight => {
            "見開きを画面右方向へ1ページずらす。右→左の見開きでは前/次の意味を反転します"
        }
        RingActionId::ImageSpreadShiftPrev => {
            "見開きを前のページ方向へ1ページずらす。右→左の見開きでも前/次の意味を反転しません"
        }
        RingActionId::ImageSpreadShiftNext => {
            "見開きを次のページ方向へ1ページずらす。右→左の見開きでも前/次の意味を反転しません"
        }
        RingActionId::ImageZoomMode => {
            "全画面ズームモードを切り替えます。Zキー長押し時の照準表示はスキップし、現在のカーソル位置でズーム状態へ入ります"
        }
        RingActionId::MinimizeWindow => "現在操作中のウィンドウを最小化する",
        _ => action.label_for_context(context),
    }
}

fn ring_bindings_for_key_action(action: KeyAction) -> Vec<(RingShortcutContext, RingActionId)> {
    let mut out = Vec::new();
    if let Some(slot) = action.favorite_slot_number()
        && let Some(ring_action) = RingActionId::favorite_slot_action(slot)
    {
        push_ring_binding_if_available(&mut out, RingShortcutContext::Grid, ring_action.clone());
        push_ring_binding_if_available(
            &mut out,
            RingShortcutContext::ImageFullscreen,
            ring_action.clone(),
        );
        push_ring_binding_if_available(&mut out, RingShortcutContext::VideoFullscreen, ring_action);
        return out;
    }
    if let Some(letter) = action.drive_letter()
        && let Some(ring_action) = RingActionId::drive_action(letter)
    {
        push_ring_binding_if_available(&mut out, RingShortcutContext::Grid, ring_action.clone());
        push_ring_binding_if_available(
            &mut out,
            RingShortcutContext::ImageFullscreen,
            ring_action.clone(),
        );
        push_ring_binding_if_available(&mut out, RingShortcutContext::VideoFullscreen, ring_action);
        return out;
    }
    if let Some(ring_action) = location_ring_action_for_key_action(action) {
        push_ring_binding_if_available(&mut out, RingShortcutContext::Grid, ring_action.clone());
        push_ring_binding_if_available(
            &mut out,
            RingShortcutContext::ImageFullscreen,
            ring_action.clone(),
        );
        push_ring_binding_if_available(&mut out, RingShortcutContext::VideoFullscreen, ring_action);
        return out;
    }

    let context = match action.context() {
        crate::keymap::KeyContext::Grid => RingShortcutContext::Grid,
        crate::keymap::KeyContext::FsImage => RingShortcutContext::ImageFullscreen,
        crate::keymap::KeyContext::FsVideo => RingShortcutContext::VideoFullscreen,
        _ => return out,
    };
    let ring_action = match action {
        KeyAction::GridParentFolder => RingActionId::GridParentFolder,
        KeyAction::GridTreeFolderPrev => RingActionId::TreeFolderPrev,
        KeyAction::GridTreeFolderNext => RingActionId::TreeFolderNext,
        KeyAction::GridSiblingFolderPrev => RingActionId::SiblingFolderPrev,
        KeyAction::GridSiblingFolderNext => RingActionId::SiblingFolderNext,
        KeyAction::GridToggleCheck => RingActionId::GridToggleCheck,
        KeyAction::GridSelectAll => RingActionId::GridSelectAll,
        KeyAction::GridToggleDetailsView => RingActionId::GridToggleDetails,
        KeyAction::GridToggleMaximize => RingActionId::ToggleMaximize,
        KeyAction::GridColumnCount1 => RingActionId::GridColumnCount1,
        KeyAction::GridColumnCount2 => RingActionId::GridColumnCount2,
        KeyAction::GridColumnCount3 => RingActionId::GridColumnCount3,
        KeyAction::GridColumnCount4 => RingActionId::GridColumnCount4,
        KeyAction::GridColumnCount5 => RingActionId::GridColumnCount5,
        KeyAction::GridColumnCount6 => RingActionId::GridColumnCount6,
        KeyAction::GridColumnCount7 => RingActionId::GridColumnCount7,
        KeyAction::GridColumnCount8 => RingActionId::GridColumnCount8,
        KeyAction::GridColumnCount9 => RingActionId::GridColumnCount9,
        KeyAction::GridColumnCount10 => RingActionId::GridColumnCount10,
        KeyAction::FsClose => RingActionId::CloseFullscreen,
        KeyAction::FsToggleMetadata => RingActionId::ImageToggleMetadata,
        KeyAction::FsToggleWindowMode => RingActionId::ToggleWindowMode,
        KeyAction::FsSpreadShiftLeft => RingActionId::ImageSpreadShiftLeft,
        KeyAction::FsSpreadShiftRight => RingActionId::ImageSpreadShiftRight,
        KeyAction::FsSpreadShiftPrev => RingActionId::ImageSpreadShiftPrev,
        KeyAction::FsSpreadShiftNext => RingActionId::ImageSpreadShiftNext,
        KeyAction::FsRotateCw => RingActionId::ImageRotateRight,
        KeyAction::FsRotateCcw => RingActionId::ImageRotateLeft,
        KeyAction::FsCapture => RingActionId::ImageCapture,
        KeyAction::FsSlideshow => RingActionId::ImageSlideshow,
        KeyAction::FsZoomMode => RingActionId::ImageZoomMode,
        KeyAction::FsPixelGrid => RingActionId::ImagePixelGrid,
        KeyAction::FsBgCycle => RingActionId::ImageBackgroundCycle,
        KeyAction::FsCompareToggle => RingActionId::ImageComparePin,
        KeyAction::VideoCapture => RingActionId::VideoCapture,
        KeyAction::VideoMute => RingActionId::VideoMute,
        KeyAction::VideoLoop => RingActionId::VideoLoop,
        KeyAction::VideoBookmark => RingActionId::VideoBookmark,
        KeyAction::VideoMarkerPrev => RingActionId::VideoMarkerPrev,
        KeyAction::VideoMarkerNext => RingActionId::VideoMarkerNext,
        KeyAction::VideoTileMode => RingActionId::VideoTileMode,
        KeyAction::VideoExternalPlayer => RingActionId::VideoExternalPlayer,
        KeyAction::VideoCloseFullscreen => RingActionId::CloseFullscreen,
        _ => return out,
    };
    push_ring_binding_if_available(&mut out, context, ring_action);
    out
}

fn ring_binding_for_key_action(action: KeyAction) -> Option<(RingShortcutContext, RingActionId)> {
    ring_bindings_for_key_action(action).into_iter().next()
}

fn push_ring_binding_if_available(
    out: &mut Vec<(RingShortcutContext, RingActionId)>,
    context: RingShortcutContext,
    action: RingActionId,
) {
    if RingActionId::available_for_context(context).contains(&action) {
        out.push((context, action));
    }
}

fn location_ring_action_for_key_action(action: KeyAction) -> Option<RingActionId> {
    Some(match action {
        KeyAction::GridOpenLocationDriveList => RingActionId::OpenLocationDriveList,
        KeyAction::GridOpenLocationReadingHistory => RingActionId::OpenLocationReadingHistory,
        KeyAction::GridOpenLocationRating1 => RingActionId::OpenLocationRating1,
        KeyAction::GridOpenLocationRating2 => RingActionId::OpenLocationRating2,
        KeyAction::GridOpenLocationRating3 => RingActionId::OpenLocationRating3,
        KeyAction::GridOpenLocationRating4 => RingActionId::OpenLocationRating4,
        KeyAction::GridOpenLocationRating5 => RingActionId::OpenLocationRating5,
        KeyAction::GridOpenLocationBooksRoot => RingActionId::OpenLocationBooksRoot,
        KeyAction::GridOpenLocationDesktop => RingActionId::OpenLocationDesktop,
        KeyAction::GridOpenLocationPictures => RingActionId::OpenLocationPictures,
        KeyAction::GridOpenLocationDownloads => RingActionId::OpenLocationDownloads,
        _ => return None,
    })
}

fn ring_action_has_key_action(context: RingShortcutContext, action: &RingActionId) -> bool {
    KeyAction::all()
        .iter()
        .copied()
        .filter(|action| action.is_user_facing())
        .flat_map(ring_bindings_for_key_action)
        .any(|(binding_context, binding_action)| {
            binding_context == context && binding_action == *action
        })
}

fn contextual_assignment_labels(
    labels: Vec<String>,
    context: RingShortcutContext,
    include_context: bool,
) -> Vec<String> {
    if include_context {
        labels
            .into_iter()
            .map(|label| format!("{}: {label}", context.label()))
            .collect()
    } else {
        labels
    }
}

fn compact_operation_label(label: &str) -> String {
    let mut compact = label
        .replace("選択中またはチェック済みの", "")
        .replace("選択中またはチェック済み画像の", "")
        .replace("選択中またはチェック済み画像に", "")
        .replace("現在の表示画像を", "")
        .replace("現在の動画フレームを", "")
        .replace("現在の動画を", "")
        .replace("現在の再生位置を", "")
        .replace("現在の画像または動画に", "")
        .replace("現在の画像または動画の", "")
        .replace("現在のフォルダまたはZIP/PDF本体に", "")
        .replace("現在のフォルダまたはZIP/PDF本体の", "")
        .replace("現在の画像の", "")
        .replace("現在の画像を", "")
        .replace("現在の項目を", "")
        .replace("現在のページを", "")
        .replace("現在ページの", "")
        .replace("現在ページに", "")
        .replace("選択中の画像を", "")
        .replace("選択中の項目を", "")
        .replace("選択中の", "")
        .replace("表示中の", "");
    for suffix in [
        "を切り替える",
        "に切り替える",
        "を表示する",
        "を開始または終了する",
        "を開始または確定する",
        "する",
    ] {
        if compact.ends_with(suffix) {
            let len = compact.len() - suffix.len();
            compact.truncate(len);
            break;
        }
    }
    let compact = compact.trim();
    const COMMAND_OVERVIEW_LABEL_MAX_CHARS: usize = 34;
    if compact.chars().count() <= COMMAND_OVERVIEW_LABEL_MAX_CHARS {
        compact.to_string()
    } else {
        let mut out: String = compact
            .chars()
            .take(COMMAND_OVERVIEW_LABEL_MAX_CHARS - 1)
            .collect();
        out.push('…');
        out
    }
}

fn assignment_summary(ui: &mut egui::Ui, groups: &[(&str, Vec<String>)]) {
    if groups.iter().all(|(_, values)| values.is_empty()) {
        ui.label(egui::RichText::new("なし").weak());
        return;
    }

    ui.vertical(|ui| {
        for (label, values) in groups {
            for value in values {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(format!("{label}:")).weak());
                    ui.monospace(value);
                });
            }
        }
    });
}

pub(super) fn page_command_settings(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    _ime_active: bool,
) {
    ui.small("キーボード操作の割り当てを編集します。競合や予約キーへの割り当ては警告として表示しますが、保存は禁止しません。");
    ui.small("Esc / Enter / 修飾なし矢印など、文脈依存が強い固定操作は現在の対象外です。");
    ui.add_space(8.0);

    let keymap = Keymap::from_settings(&state.settings.keymap);
    let conflicts = keymap.binding_conflicts();
    command_conflict_summary(ui, state, &conflicts);

    ui.add_space(10.0);
    command_filter_controls(ui, state);
    ui.horizontal(|ui| {
        if ui.button("すべて既定に戻す").clicked() {
            state.settings.keymap.overrides.clear();
            state.command_edit_loaded_for = None;
            state.command_capture_slot = None;
            state.command_edit_error = None;
            state.command_edit_notice = None;
        }
    });

    ui.add_space(8.0);
    keyboard_chord_picker(ui, state, &keymap);

    ui.add_space(8.0);
    command_list(ui, state, &keymap, &conflicts);
}

pub(super) fn draw_operation_assignment_editor_dialog(
    ctx: &egui::Context,
    state: &mut PreferencesState,
    ime_active: bool,
) {
    let Some(editor) = state.operation_assignment_editor.clone() else {
        return;
    };

    let mut open = true;
    let title = operation_assignment_editor_title(&editor.target);
    let safe_rect = ctx.content_rect().shrink2(egui::vec2(32.0, 48.0));
    let safe_size = safe_rect.size().max(egui::vec2(360.0, 280.0));
    let default_size = egui::vec2(680.0, 620.0).min(safe_size);
    let min_size = egui::vec2(520.0, 360.0).min(safe_size);
    egui::Window::new(title)
        .open(&mut open)
        .resizable(true)
        .collapsible(false)
        .default_size(default_size)
        .min_size(min_size)
        .max_size(safe_size)
        .constrain_to(safe_rect)
        .show(ctx, |ui| {
            operation_assignment_editor_header(ui, &editor.target);
            ui.add_space(6.0);
            operation_assignment_editor_tabs(ui, state, &editor);
            ui.separator();
            ui.add_space(6.0);

            let chord_keyboard_editor = matches!(
                (&editor.target, editor.tab),
                (
                    OperationAssignmentTarget::Chord(_),
                    OperationAssignmentTab::Keyboard
                )
            );
            if chord_keyboard_editor {
                draw_operation_assignment_editor_body(ui, state, &editor, ime_active);
            } else {
                let available_h = ui.available_height().max(120.0);
                egui::ScrollArea::vertical()
                    .id_salt("operation_assignment_editor_body")
                    .max_height(available_h)
                    .show(ui, |ui| {
                        draw_operation_assignment_editor_body(ui, state, &editor, ime_active);
                    });
            }
        });

    if !open {
        state.operation_assignment_editor = None;
        state.command_editor_source_chord = None;
        state.command_capture_slot = None;
    }
}

pub(super) fn draw_mouse_gesture_recorder_dialog(
    ctx: &egui::Context,
    state: &mut PreferencesState,
) {
    let Some(mut recorder) = state.operation_mouse_gesture_recorder.clone() else {
        return;
    };

    let mut open = true;
    let mut add_pattern = false;
    let mut close_requested = false;
    let title = if recorder.replace_index.is_some() {
        format!("マウスジェスチャ再記録: {}", recorder.context.label())
    } else {
        format!("マウスジェスチャ追加: {}", recorder.context.label())
    };
    egui::Window::new(title)
        .open(&mut open)
        .resizable(true)
        .movable(false)
        .collapsible(false)
        .default_size([460.0, 360.0])
        .min_width(420.0)
        .show(ctx, |ui| {
            ui.small("下の枠内で右ボタンを押しながらマウスを動かして、ジェスチャを記録します。上下左右の最大 4 stroke まで登録できます。");
            ui.add_space(8.0);

            let desired = egui::vec2(ui.available_width().max(320.0), 190.0);
            let (rect, response) = ui.allocate_exact_size(desired, egui::Sense::click_and_drag());
            ui.painter()
                .rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
            ui.painter().rect_stroke(
                rect,
                4.0,
                egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "右ドラッグで記録",
                egui::TextStyle::Body.resolve(ui.style()),
                ui.visuals().weak_text_color(),
            );

            let secondary_down =
                ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary));
            let pointer_pos = ui.input(|i| i.pointer.interact_pos());
            if secondary_down
                && response.hovered()
                && !recorder.recording
                && let Some(pos) = pointer_pos
            {
                recorder.recording = true;
                recorder.pattern.clear();
                recorder.points.clear();
                recorder.points.push(pos);
                recorder.error = None;
            }
            if recorder.recording {
                if secondary_down {
                    if let Some(pos) = pointer_pos {
                        update_mouse_gesture_recording(&mut recorder, pos);
                    }
                    ctx.request_repaint();
                } else {
                    recorder.recording = false;
                    if recorder.pattern.is_empty() {
                        recorder.error = Some("ジェスチャが短すぎます。もう一度右ドラッグしてください。".to_owned());
                    }
                }
            }

            if recorder.points.len() >= 2 {
                for pair in recorder.points.windows(2) {
                    ui.painter().line_segment(
                        [pair[0], pair[1]],
                        egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                    );
                }
            }

            ui.add_space(8.0);
            let pattern_label = if recorder.pattern.is_empty() {
                "未記録".to_owned()
            } else {
                format_mouse_gesture_pattern(&recorder.pattern)
            };
            ui.horizontal(|ui| {
                ui.label("記録:");
                ui.monospace(pattern_label);
            });
            if let Some(error) = &recorder.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            if let Some(conflict) = mouse_gesture_duplicate_label(
                &state.settings.ring_shortcuts,
                recorder.context,
                &recorder.pattern,
            )
            {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!("同じジェスチャが既にあります: {conflict}"),
                );
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let action_label = if recorder.replace_index.is_some() {
                    "更新"
                } else {
                    "追加"
                };
                if ui
                    .add_enabled(!recorder.pattern.is_empty(), egui::Button::new(action_label))
                    .clicked()
                {
                    add_pattern = true;
                }
                if ui.button("やり直し").clicked() {
                    recorder.pattern.clear();
                    recorder.points.clear();
                    recorder.recording = false;
                    recorder.error = None;
                }
                if ui.button("閉じる").clicked() {
                    close_requested = true;
                }
            });
        });

    if add_pattern && !recorder.pattern.is_empty() {
        let profile = state
            .settings
            .ring_shortcuts
            .mouse_gesture_profile_mut(recorder.context);
        let binding = crate::ring_shortcut::MouseGestureBinding::new(
            recorder.pattern.clone(),
            recorder.action.clone(),
        );
        let idx = if let Some(idx) = recorder.replace_index {
            if let Some(slot) = profile.bindings.get_mut(idx) {
                *slot = binding;
                idx
            } else {
                profile.bindings.push(binding);
                profile.bindings.len() - 1
            }
        } else {
            profile.bindings.push(binding);
            profile.bindings.len() - 1
        };
        state.operation_mouse_gesture_inputs.insert(
            (recorder.context, idx),
            format_mouse_gesture_pattern(&recorder.pattern),
        );
        state.operation_mouse_gesture_context = recorder.context;
        state.operation_mouse_gesture_recorder = None;
    } else if open && !close_requested {
        state.operation_mouse_gesture_recorder = Some(recorder);
    } else {
        state.operation_mouse_gesture_recorder = None;
    }
}

fn update_mouse_gesture_recording(recorder: &mut OperationMouseGestureRecorder, pos: egui::Pos2) {
    let Some(&last) = recorder.points.last() else {
        recorder.points.push(pos);
        return;
    };
    let delta = pos - last;
    let Some(direction) = mouse_gesture_direction_from_delta(delta) else {
        return;
    };
    if recorder.pattern.last().copied() != Some(direction) {
        if recorder.pattern.len() >= crate::ring_shortcut::MOUSE_GESTURE_MAX_STROKES {
            recorder.error = Some("最大 4 stroke までです。".to_owned());
        } else {
            recorder.pattern.push(direction);
            recorder.error = None;
        }
    }
    recorder.points.push(pos);
}

fn mouse_gesture_duplicate_label(
    settings: &RingShortcutSettings,
    context: RightDragContext,
    pattern: &[MouseGestureDirection],
) -> Option<String> {
    if pattern.is_empty() {
        return None;
    }
    let profile = settings.mouse_gesture_profile(context);
    profile
        .bindings
        .iter()
        .find(|binding| binding.pattern == pattern)
        .map(|binding| {
            binding
                .action
                .label_for_context(context.gesture_action_context())
                .to_owned()
        })
}

fn operation_assignment_editor_title(target: &OperationAssignmentTarget) -> String {
    match target {
        OperationAssignmentTarget::Key(action) => {
            format!("操作割り当て編集: {}", compact_key_action_label(*action))
        }
        OperationAssignmentTarget::Chord(chord) => {
            format!("キーから割り当て: {}", chord.display_name())
        }
        OperationAssignmentTarget::Ring { context, action } => {
            format!(
                "操作割り当て編集: {} / {}",
                context.label(),
                action.label_for_context(*context)
            )
        }
        OperationAssignmentTarget::RingSlot { context, direction } => {
            format!(
                "ゲームパッド X+{} 割り当て: {}",
                direction_short_label(*direction),
                context.label()
            )
        }
        OperationAssignmentTarget::MouseButton { context, slot } => {
            format!("マウス{}割り当て: {}", slot.label(), context.label())
        }
        OperationAssignmentTarget::MouseGesture { context, index } => {
            format!("マウスジェスチャ編集: {} / #{}", context.label(), index + 1)
        }
    }
}

fn operation_assignment_editor_header(ui: &mut egui::Ui, target: &OperationAssignmentTarget) {
    match target {
        OperationAssignmentTarget::Key(action) => {
            ui.label(egui::RichText::new(compact_key_action_label(*action)).strong());
            ui.small(action.description());
            if let Some((context, ring_action)) = ring_binding_for_key_action(*action) {
                ui.small(format!(
                    "この操作は {} のリング / マウス / パッドにも割り当てられます: {}",
                    context.label(),
                    ring_action.label_for_context(context)
                ));
            }
        }
        OperationAssignmentTarget::Chord(chord) => {
            ui.label(
                egui::RichText::new(format!("キー: {}", chord.display_name()))
                    .strong()
                    .size(14.0),
            );
            ui.small(
                "場所ごとの割り当て状態を確認し、割り当てたい場所を選んでから操作を選びます。",
            );
        }
        OperationAssignmentTarget::Ring { context, action } => {
            ui.label(egui::RichText::new(action.label_for_context(*context)).strong());
            ui.small(ring_action_detail_label(action, *context));
        }
        OperationAssignmentTarget::RingSlot { context, direction } => {
            ui.label(
                egui::RichText::new(format!("{} / X+{}", context.label(), direction.label()))
                    .strong(),
            );
            ui.small("この方向に実行するリングショートカットを選びます。");
        }
        OperationAssignmentTarget::MouseButton { context, slot } => {
            ui.label(
                egui::RichText::new(format!("{} / {}", context.label(), slot.label())).strong(),
            );
            ui.small("物理ボタン単体に実行する一発アクションを選びます。");
        }
        OperationAssignmentTarget::MouseGesture { context, index } => {
            ui.label(
                egui::RichText::new(format!("{} / ジェスチャ #{}", context.label(), index + 1))
                    .strong(),
            );
            ui.small("このジェスチャの実行アクション、再記録、削除を行います。");
        }
    }
}

fn operation_assignment_editor_tabs(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    editor: &OperationAssignmentEditor,
) {
    ui.horizontal_wrapped(|ui| {
        for tab in [
            OperationAssignmentTab::Keyboard,
            OperationAssignmentTab::RingPad,
            OperationAssignmentTab::MouseButtons,
            OperationAssignmentTab::MouseGesture,
        ] {
            let enabled = operation_assignment_tab_enabled(&editor.target, tab);
            let selected = editor.tab == tab;
            if ui
                .add_enabled(enabled, egui::Button::selectable(selected, tab.label()))
                .clicked()
                && let Some(current) = state.operation_assignment_editor.as_mut()
            {
                current.tab = tab;
            }
        }
    });
}

fn operation_assignment_tab_enabled(
    target: &OperationAssignmentTarget,
    tab: OperationAssignmentTab,
) -> bool {
    match target {
        OperationAssignmentTarget::Key(action) => {
            tab == OperationAssignmentTab::Keyboard
                || ring_binding_for_key_action(*action).is_some()
        }
        OperationAssignmentTarget::Chord(_) => tab == OperationAssignmentTab::Keyboard,
        OperationAssignmentTarget::Ring { .. } | OperationAssignmentTarget::RingSlot { .. } => {
            tab != OperationAssignmentTab::Keyboard
        }
        OperationAssignmentTarget::MouseButton { .. } => {
            tab == OperationAssignmentTab::MouseButtons
        }
        OperationAssignmentTarget::MouseGesture { .. } => {
            tab == OperationAssignmentTab::MouseGesture
        }
    }
}

fn draw_operation_assignment_editor_body(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    editor: &OperationAssignmentEditor,
    ime_active: bool,
) {
    match (&editor.target, editor.tab) {
        (OperationAssignmentTarget::Key(action), OperationAssignmentTab::Keyboard) => {
            state.operation_keyboard_context = Some(action.context());
            let keymap = Keymap::from_settings(&state.settings.keymap);
            let conflicts = keymap.binding_conflicts();
            command_editor_for_action(ui, state, &keymap, &conflicts, ime_active, *action);
        }
        (OperationAssignmentTarget::Key(action), OperationAssignmentTab::RingPad) => {
            if let Some((context, ring_action)) = ring_binding_for_key_action(*action) {
                state.operation_ring_context = context;
                ring_action_slot_editor(ui, state, context, ring_action);
            }
        }
        (OperationAssignmentTarget::Key(action), OperationAssignmentTab::MouseButtons) => {
            if let Some((context, ring_action)) = ring_binding_for_key_action(*action) {
                ring_action_mouse_button_editor(ui, state, context, ring_action);
            }
        }
        (OperationAssignmentTarget::Key(action), OperationAssignmentTab::MouseGesture) => {
            if let Some((context, ring_action)) = ring_binding_for_key_action(*action) {
                let right_drag_context = right_drag_context_for_ring_context(context);
                state.operation_mouse_gesture_context = right_drag_context;
                ring_action_mouse_gesture_editor(ui, state, right_drag_context, ring_action);
            }
        }
        (OperationAssignmentTarget::Chord(chord), OperationAssignmentTab::Keyboard) => {
            let keymap = Keymap::from_settings(&state.settings.keymap);
            command_editor_source_chord_section(ui, state, &keymap, *chord);
        }
        (OperationAssignmentTarget::Ring { context, action }, OperationAssignmentTab::RingPad) => {
            state.operation_ring_context = *context;
            ring_action_slot_editor(ui, state, *context, action.clone());
        }
        (
            OperationAssignmentTarget::RingSlot { context, direction },
            OperationAssignmentTab::RingPad,
        ) => {
            state.operation_ring_context = *context;
            ring_slot_assignment_editor(ui, state, *context, *direction);
        }
        (
            OperationAssignmentTarget::MouseButton { context, slot },
            OperationAssignmentTab::MouseButtons,
        ) => {
            mouse_button_assignment_editor(ui, state, *context, *slot);
        }
        (
            OperationAssignmentTarget::MouseGesture { context, index },
            OperationAssignmentTab::MouseGesture,
        ) => {
            state.operation_mouse_gesture_context = *context;
            mouse_gesture_assignment_editor(ui, state, *context, *index);
        }
        (
            OperationAssignmentTarget::Ring { context, action },
            OperationAssignmentTab::MouseButtons,
        ) => {
            ring_action_mouse_button_editor(ui, state, *context, action.clone());
        }
        (
            OperationAssignmentTarget::RingSlot { context, .. },
            OperationAssignmentTab::MouseButtons,
        ) => {
            mouse_button_context_editor(ui, state, *context);
        }
        (
            OperationAssignmentTarget::Ring { context, action },
            OperationAssignmentTab::MouseGesture,
        ) => {
            let right_drag_context = right_drag_context_for_ring_context(*context);
            state.operation_mouse_gesture_context = right_drag_context;
            ring_action_mouse_gesture_editor(ui, state, right_drag_context, action.clone());
        }
        (
            OperationAssignmentTarget::RingSlot { context, .. },
            OperationAssignmentTab::MouseGesture,
        ) => {
            let right_drag_context = right_drag_context_for_ring_context(*context);
            state.operation_mouse_gesture_context = right_drag_context;
            page_mouse_gesture_bindings(ui, state, right_drag_context);
        }
        _ => {
            ui.small("この操作ではこの割り当て種別を編集できません。");
        }
    }
}

fn command_editor_source_chord_section(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    keymap: &Keymap,
    chord: Chord,
) {
    let chord_label = chord.display_name();
    ui.label(
        egui::RichText::new(format!("キー: {chord_label}"))
            .strong()
            .size(14.0),
    );
    ui.small("場所ごとの現在の割り当てです。割り当てたい場所を選ぶと、下に候補が表示されます。");

    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.label("場所:");
            let selected =
                operation_keyboard_context_filter_label(state.operation_keyboard_context);
            egui::ComboBox::from_id_salt((
                "command_editor_source_chord_context",
                chord_label.clone(),
            ))
            .selected_text(selected)
            .width(190.0)
            .show_ui(ui, |ui| {
                for context in key_assignment_context_filters() {
                    let label = operation_keyboard_context_filter_label(context);
                    ui.selectable_value(&mut state.operation_keyboard_context, context, label);
                }
            });
        });

        let matches = actions_for_chord(keymap, chord, state.operation_keyboard_context);
        ui.horizontal_wrapped(|ui| {
            ui.label("現在の割り当て:");
            if matches.is_empty() {
                ui.label(egui::RichText::new("割り当てなし").weak());
            } else {
                for action in matches {
                    let label = if state.operation_keyboard_context.is_none() {
                        format!(
                            "{}: {}",
                            key_action_context_label(action),
                            compact_key_action_label(action)
                        )
                    } else {
                        compact_key_action_label(action)
                    };
                    ui.label(label).on_hover_text(action.description());
                }
            }
        });
    });

    ui.add_space(8.0);
    let filter = state.command_filter.trim().to_string();
    let key_filter = state.command_key_filter.trim().to_string();
    egui::Frame::group(ui.style()).show(ui, |ui| {
        let heading = match state.operation_keyboard_context {
            Some(context) => format!(
                "{} に割り当てる操作を選ぶ",
                operation_keyboard_context_filter_label(Some(context))
            ),
            None => "割り当てる操作を選ぶ".to_string(),
        };
        ui.label(egui::RichText::new(heading).strong());
        ui.add_space(4.0);
        let candidate_height = ui.available_height().max(180.0);
        egui::ScrollArea::vertical()
            .id_salt(("command_editor_chord_assign_list", chord_label))
            .max_height(candidate_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                egui::Grid::new(("command_editor_chord_assign_grid", chord.display_name()))
                    .num_columns(4)
                    .spacing([8.0, 3.0])
                    .striped(true)
                    .show(ui, |ui| {
                        ui.strong("割り当て");
                        ui.strong("操作");
                        ui.strong("現在のキー");
                        ui.strong("状態");
                        ui.end_row();

                        for &action in KeyAction::all()
                            .iter()
                            .filter(|action| action.is_user_facing())
                        {
                            if !operation_keyboard_context_filter_matches(
                                state.operation_keyboard_context,
                                action.context(),
                            ) {
                                continue;
                            }
                            let labels = keymap.chord_labels(action);
                            if !command_action_matches_filter(action, &filter)
                                || !command_key_labels_match_filter(&labels, &key_filter)
                            {
                                continue;
                            }
                            if ui.small_button("割り当て").clicked() {
                                assign_chord_to_action_editor(state, keymap, action, chord);
                            }
                            ui.label(compact_key_action_label(action))
                                .on_hover_text(action.description());
                            assignment_values(ui, &labels);
                            ui.label(chord_assignment_candidate_status(keymap, action, chord))
                                .on_hover_text(
                                    "空きあり: 未使用のキー欄へ自動入力します。\n置換が必要: 編集ダイアログで置き換える行を選びます。",
                                );
                            ui.end_row();
                        }
                    });
            });
    });
}

fn key_assignment_context_filters() -> [Option<KeyContext>; 8] {
    [
        None,
        Some(KeyContext::Global),
        Some(KeyContext::Grid),
        Some(KeyContext::FsCommon),
        Some(KeyContext::Rating),
        Some(KeyContext::FsImage),
        Some(KeyContext::FsVideo),
        Some(KeyContext::Erase),
    ]
}

fn assign_chord_to_action_editor(
    state: &mut PreferencesState,
    keymap: &Keymap,
    action: KeyAction,
    chord: Chord,
) {
    open_command_editor_dialog(state, action, Some(chord));
    ensure_command_editor_loaded(state, keymap, action);
    let label = chord.display_name();
    if let Some(slot) = state
        .command_chord_inputs
        .iter()
        .position(|input| command_input_matches_chord(action, input, chord))
    {
        state.command_edit_notice = Some(format!(
            "{label} はすでにキー {} に入っています。必要なら「適用して閉じる」で保存します。",
            slot + 1
        ));
        state.command_edit_error = None;
        return;
    }
    let Some(slot) = state
        .command_chord_inputs
        .iter()
        .position(|input| input.trim().is_empty())
    else {
        state.command_edit_notice = Some(format!(
            "空きキー欄がありません。置き換えたい行の「このキーに置換」を押すと {label} に差し替えます。"
        ));
        state.command_edit_error = None;
        return;
    };
    state.command_chord_inputs[slot] = label;
    state.command_edit_notice = Some(format!(
        "キー {} に {} を入れました。「適用して閉じる」で保存します。",
        slot + 1,
        state.command_chord_inputs[slot]
    ));
    state.command_edit_error = None;
}

fn operation_assignment_editor_context(
    target: &OperationAssignmentTarget,
) -> Option<RingShortcutContext> {
    match target {
        OperationAssignmentTarget::Ring { context, .. }
        | OperationAssignmentTarget::RingSlot { context, .. } => Some(*context),
        OperationAssignmentTarget::MouseButton { context, .. } => Some(*context),
        OperationAssignmentTarget::MouseGesture { context, .. } => {
            Some(context.gesture_action_context())
        }
        OperationAssignmentTarget::Key(_) | OperationAssignmentTarget::Chord(_) => None,
    }
}

fn open_operation_assignment_editor(
    state: &mut PreferencesState,
    target: OperationAssignmentTarget,
    tab: OperationAssignmentTab,
) {
    if let OperationAssignmentTarget::Key(action) = &target {
        state.operation_keyboard_context =
            operation_keyboard_context_filter_for_context(action.context());
    }
    if let OperationAssignmentTarget::Chord(chord) = &target {
        state.command_editor_source_chord = Some(*chord);
    }
    if let Some(context) = operation_assignment_editor_context(&target) {
        state.operation_ring_context = context;
        state.operation_mouse_gesture_context = right_drag_context_for_ring_context(context);
    }
    if let OperationAssignmentTarget::MouseGesture { context, .. } = &target {
        state.operation_mouse_gesture_context = *context;
    }
    state.operation_assignment_editor = Some(OperationAssignmentEditor { target, tab });
}

fn close_assignment_editors(state: &mut PreferencesState) {
    state.operation_assignment_editor = None;
    state.command_editor_source_chord = None;
    state.command_capture_slot = None;
    state.command_edit_notice = None;
}

fn keyboard_chord_picker(ui: &mut egui::Ui, state: &mut PreferencesState, keymap: &Keymap) {
    ui.group(|ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label(egui::RichText::new("キーボード図").strong());
            ui.checkbox(&mut state.operation_keyboard_ctrl, "Ctrl");
            ui.checkbox(&mut state.operation_keyboard_shift, "Shift");
            ui.checkbox(&mut state.operation_keyboard_alt, "Alt");
            ui.small(
                "ホバーで割り当て状況を表示。クリックすると選択中コマンドの空きキー欄へ入ります。",
            );
        });
        ui.add_space(6.0);

        ui.vertical(|ui| {
            ui.small("メインキー");
            let rows = keyboard_picker_main_rows();
            draw_keyboard_picker_rows(ui, state, keymap, &rows);

            ui.add_space(8.0);
            ui.separator();
            ui.add_space(8.0);

            ui.horizontal_top(|ui| {
                ui.vertical(|ui| {
                    ui.small("ナビゲーション");
                    let rows = keyboard_picker_navigation_rows();
                    draw_keyboard_picker_rows(ui, state, keymap, &rows);
                });
                ui.add_space(18.0);
                ui.vertical(|ui| {
                    ui.small("テンキー");
                    let rows = keyboard_picker_numpad_rows();
                    draw_keyboard_picker_rows(ui, state, keymap, &rows);
                });
            });
        });
    });
}

fn draw_keyboard_picker_rows(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    keymap: &Keymap,
    rows: &[&'static [KeyboardPickerCell]],
) {
    for row in rows {
        ui.horizontal(|ui| {
            for &cell in *row {
                match cell {
                    KeyboardPickerCell::Key(key, label) => {
                        let chord = Chord::new(
                            state.operation_keyboard_ctrl,
                            state.operation_keyboard_shift,
                            state.operation_keyboard_alt,
                            key,
                        );
                        let label = label.unwrap_or_else(|| key.display_name());
                        let width = keyboard_picker_cell_width(label, key);
                        let response =
                            ui.add_sized([width, 24.0], egui::Button::new(label).small());
                        let clicked = response.clicked();
                        response.on_hover_text(keyboard_chord_tooltip(
                            keymap,
                            chord,
                            state.operation_keyboard_context,
                        ));
                        if clicked {
                            assign_keyboard_picker_chord(state, keymap, chord);
                        }
                    }
                    KeyboardPickerCell::Disabled(label) => {
                        let width = keyboard_picker_label_width(label);
                        ui.add_enabled_ui(false, |ui| {
                            ui.add_sized([width, 24.0], egui::Button::new(label).small())
                        })
                        .inner
                        .on_hover_text("現在は割り当て対象外です。");
                    }
                    KeyboardPickerCell::Spacer(width) => {
                        ui.add_space(width);
                    }
                }
            }
        });
    }
}

#[derive(Clone, Copy)]
enum KeyboardPickerCell {
    Key(KeyName, Option<&'static str>),
    Disabled(&'static str),
    Spacer(f32),
}

fn keyboard_picker_main_rows() -> [&'static [KeyboardPickerCell]; 6] {
    use KeyboardPickerCell::Key;
    const EXTENDED_FUNCTION: &[KeyboardPickerCell] = &[
        Key(KeyName::F13, None),
        Key(KeyName::F14, None),
        Key(KeyName::F15, None),
        Key(KeyName::F16, None),
        Key(KeyName::F17, None),
        Key(KeyName::F18, None),
        Key(KeyName::F19, None),
        Key(KeyName::F20, None),
        Key(KeyName::F21, None),
        Key(KeyName::F22, None),
        Key(KeyName::F23, None),
        Key(KeyName::F24, None),
    ];
    const FUNCTION: &[KeyboardPickerCell] = &[
        Key(KeyName::F1, None),
        Key(KeyName::F2, None),
        Key(KeyName::F3, None),
        Key(KeyName::F4, None),
        Key(KeyName::F5, None),
        Key(KeyName::F6, None),
        Key(KeyName::F7, None),
        Key(KeyName::F8, None),
        Key(KeyName::F9, None),
        Key(KeyName::F10, None),
        Key(KeyName::F11, None),
        Key(KeyName::F12, None),
    ];
    const NUMBER: &[KeyboardPickerCell] = &[
        Key(KeyName::Esc, None),
        Key(KeyName::Num1, None),
        Key(KeyName::Num2, None),
        Key(KeyName::Num3, None),
        Key(KeyName::Num4, None),
        Key(KeyName::Num5, None),
        Key(KeyName::Num6, None),
        Key(KeyName::Num7, None),
        Key(KeyName::Num8, None),
        Key(KeyName::Num9, None),
        Key(KeyName::Num0, None),
        Key(KeyName::Minus, None),
        Key(KeyName::JisCaret, Some("^")),
        Key(KeyName::IntlYen, Some("￥")),
        Key(KeyName::Backspace, None),
    ];
    const QWERTY: &[KeyboardPickerCell] = &[
        Key(KeyName::Tab, None),
        Key(KeyName::Q, None),
        Key(KeyName::W, None),
        Key(KeyName::E, None),
        Key(KeyName::R, None),
        Key(KeyName::T, None),
        Key(KeyName::Y, None),
        Key(KeyName::U, None),
        Key(KeyName::I, None),
        Key(KeyName::O, None),
        Key(KeyName::P, None),
        Key(KeyName::JisAt, Some("@")),
        Key(KeyName::OpenBracket, None),
    ];
    const HOME: &[KeyboardPickerCell] = &[
        Key(KeyName::A, None),
        Key(KeyName::S, None),
        Key(KeyName::D, None),
        Key(KeyName::F, None),
        Key(KeyName::G, None),
        Key(KeyName::H, None),
        Key(KeyName::J, None),
        Key(KeyName::K, None),
        Key(KeyName::L, None),
        Key(KeyName::Semicolon, None),
        Key(KeyName::Colon, None),
        Key(KeyName::CloseBracket, None),
        Key(KeyName::Enter, None),
    ];
    const BOTTOM: &[KeyboardPickerCell] = &[
        Key(KeyName::Z, None),
        Key(KeyName::X, None),
        Key(KeyName::C, None),
        Key(KeyName::V, None),
        Key(KeyName::B, None),
        Key(KeyName::N, None),
        Key(KeyName::M, None),
        Key(KeyName::Comma, None),
        Key(KeyName::Period, None),
        Key(KeyName::Slash, None),
        Key(KeyName::IntlRo, Some("＼")),
        Key(KeyName::Space, None),
    ];
    [EXTENDED_FUNCTION, FUNCTION, NUMBER, QWERTY, HOME, BOTTOM]
}

fn keyboard_picker_navigation_rows() -> [&'static [KeyboardPickerCell]; 2] {
    use KeyboardPickerCell::{Disabled, Key, Spacer};
    const NAV_TOP: &[KeyboardPickerCell] = &[
        Disabled("Insert"),
        Key(KeyName::Home, None),
        Key(KeyName::PageUp, None),
        Spacer(48.0),
        Spacer(48.0),
        Key(KeyName::Up, None),
    ];
    const NAV_BOTTOM: &[KeyboardPickerCell] = &[
        Key(KeyName::Delete, None),
        Key(KeyName::End, None),
        Key(KeyName::PageDown, None),
        Spacer(48.0),
        Key(KeyName::Left, None),
        Key(KeyName::Down, None),
        Key(KeyName::Right, None),
    ];
    [NAV_TOP, NAV_BOTTOM]
}

fn keyboard_picker_numpad_rows() -> [&'static [KeyboardPickerCell]; 5] {
    use KeyboardPickerCell::{Disabled, Key, Spacer};
    const NUMPAD_TOP: &[KeyboardPickerCell] = &[
        Disabled("NumLock"),
        Key(KeyName::NumpadDivide, Some("Num/")),
        Key(KeyName::NumpadMultiply, Some("Num*")),
        Key(KeyName::NumpadSubtract, Some("Num-")),
    ];
    const NUMPAD_789: &[KeyboardPickerCell] = &[
        Key(KeyName::Numpad7, Some("Num7")),
        Key(KeyName::Numpad8, Some("Num8")),
        Key(KeyName::Numpad9, Some("Num9")),
        Key(KeyName::NumpadAdd, Some("Num+")),
    ];
    const NUMPAD_456: &[KeyboardPickerCell] = &[
        Key(KeyName::Numpad4, Some("Num4")),
        Key(KeyName::Numpad5, Some("Num5")),
        Key(KeyName::Numpad6, Some("Num6")),
        Key(KeyName::NumpadEnter, Some("Enter")),
    ];
    const NUMPAD_123: &[KeyboardPickerCell] = &[
        Key(KeyName::Numpad1, Some("Num1")),
        Key(KeyName::Numpad2, Some("Num2")),
        Key(KeyName::Numpad3, Some("Num3")),
        Spacer(62.0),
    ];
    const NUMPAD_0: &[KeyboardPickerCell] = &[
        Key(KeyName::Numpad0, Some("Num0")),
        Key(KeyName::NumpadDecimal, Some("Num.")),
        Spacer(44.0),
        Spacer(62.0),
    ];
    [NUMPAD_TOP, NUMPAD_789, NUMPAD_456, NUMPAD_123, NUMPAD_0]
}

fn keyboard_picker_cell_width(label: &str, key: KeyName) -> f32 {
    match key {
        KeyName::Backspace => 76.0,
        KeyName::Enter => 68.0,
        KeyName::NumpadEnter => 62.0,
        KeyName::Space => 132.0,
        KeyName::PageUp | KeyName::PageDown => 62.0,
        _ => keyboard_picker_label_width(label),
    }
}

fn keyboard_picker_label_width(label: &str) -> f32 {
    match label {
        "Insert" | "Delete" | "PageUp" | "PageDown" | "NumLock" => 62.0,
        _ => 44.0,
    }
}

fn keyboard_chord_tooltip(
    keymap: &Keymap,
    chord: Chord,
    context_filter: Option<crate::keymap::KeyContext>,
) -> String {
    let mut lines = vec![chord.display_name()];
    let mut matches = Vec::new();
    for &action in KeyAction::all()
        .iter()
        .filter(|action| action.is_user_facing())
    {
        if !operation_keyboard_context_filter_matches(context_filter, action.context()) {
            continue;
        }
        if keymap.effective_chords(action).contains(&chord) {
            matches.push(format!(
                "{} / {}",
                key_action_context_label(action),
                action.description()
            ));
        }
    }
    if matches.is_empty() {
        lines.push("割り当てなし".to_string());
    } else {
        lines.extend(matches);
    }
    lines.join("\n")
}

fn assign_keyboard_picker_chord(state: &mut PreferencesState, keymap: &Keymap, chord: Chord) {
    let matches = actions_for_chord(keymap, chord, state.operation_keyboard_context);
    state.command_editor_source_chord = Some(chord);
    state.command_selected = None;
    state.command_edit_loaded_for = None;
    state.command_capture_slot = None;
    state.command_edit_error = None;
    state.command_edit_notice = None;
    open_operation_assignment_editor(
        state,
        OperationAssignmentTarget::Chord(chord),
        OperationAssignmentTab::Keyboard,
    );
    state.command_edit_error = if matches.is_empty() {
        Some("このキーに割り当てるコマンドを下の一覧から選んでください。".to_string())
    } else {
        None
    };
}

fn actions_for_chord(
    keymap: &Keymap,
    chord: Chord,
    context_filter: Option<crate::keymap::KeyContext>,
) -> Vec<KeyAction> {
    KeyAction::all()
        .iter()
        .copied()
        .filter(|action| action.is_user_facing())
        .filter(|action| {
            operation_keyboard_context_filter_matches(context_filter, action.context())
                && keymap.effective_chords(*action).contains(&chord)
        })
        .collect()
}

fn open_command_editor_dialog(
    state: &mut PreferencesState,
    action: KeyAction,
    source_chord: Option<Chord>,
) {
    state.command_editor_source_chord = source_chord;
    open_operation_assignment_editor(
        state,
        OperationAssignmentTarget::Key(action),
        OperationAssignmentTab::Keyboard,
    );
}

fn push_unique_label(labels: &mut Vec<String>, label: String) {
    if !labels.contains(&label) {
        labels.push(label);
    }
}

fn right_drag_context_for_ring_context(context: RingShortcutContext) -> RightDragContext {
    match context {
        RingShortcutContext::Grid => RightDragContext::Grid,
        RingShortcutContext::ImageFullscreen => RightDragContext::ImageFullscreen,
        RingShortcutContext::VideoFullscreen => RightDragContext::VideoFullscreen,
    }
}

fn ring_assignment_labels(
    settings: &RingShortcutSettings,
    context: RingShortcutContext,
    action: &RingActionId,
) -> Vec<String> {
    let right_drag_context = right_drag_context_for_ring_context(context);
    if settings.right_drag_mode(right_drag_context) != RightDragMode::RingShortcut {
        return Vec::new();
    }
    settings
        .profile(context)
        .slots
        .iter()
        .enumerate()
        .filter_map(|(idx, slot)| {
            (slot == action).then(|| {
                RingDirection::all()
                    .get(idx)
                    .map(|direction| format!("右ドラッグ {}", direction.label()))
            })?
        })
        .collect()
}

fn gamepad_ring_assignment_labels(
    settings: &RingShortcutSettings,
    context: RingShortcutContext,
    action: &RingActionId,
) -> Vec<String> {
    settings
        .profile(context)
        .slots
        .iter()
        .enumerate()
        .filter_map(|(idx, slot)| {
            (slot == action).then(|| {
                RingDirection::all()
                    .get(idx)
                    .map(|direction| format!("X+{}", direction.label()))
            })?
        })
        .collect()
}

fn mouse_assignment_labels(
    settings: &RingShortcutSettings,
    context: RingShortcutContext,
    action: &RingActionId,
) -> Vec<String> {
    let mut labels = Vec::new();
    let buttons = settings.mouse_button_profile(context);
    if buttons.back == *action {
        push_unique_label(&mut labels, "戻るボタン".to_string());
    }
    if buttons.forward == *action {
        push_unique_label(&mut labels, "進むボタン".to_string());
    }
    if buttons.middle == *action {
        push_unique_label(&mut labels, "ホイールクリック".to_string());
    }

    for &right_drag_context in RightDragContext::all() {
        if right_drag_context.gesture_action_context() != context {
            continue;
        }
        let profile = settings.mouse_gesture_profile(right_drag_context);
        for binding in &profile.bindings {
            if binding.action == *action {
                push_unique_label(
                    &mut labels,
                    format!(
                        "{} {}",
                        right_drag_context.label(),
                        format_mouse_gesture_pattern(&binding.pattern)
                    ),
                );
            }
        }
    }
    labels
}

fn command_conflict_summary(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    conflicts: &[BindingConflict],
) {
    if conflicts.is_empty() {
        ui.label(
            egui::RichText::new("競合している割り当てはありません。")
                .size(11.0)
                .weak(),
        );
        return;
    }

    ui.group(|ui| {
        ui.label(
            egui::RichText::new(format!("競合している割り当て: {} 件", conflicts.len()))
                .strong()
                .color(egui::Color32::from_rgb(220, 150, 80)),
        );
        ui.small("上から処理される側が優先される場合があります。必要に応じて片方を別キーに変更するか、割り当てを解除してください。");

        egui::Grid::new("command_conflicts")
            .num_columns(4)
            .spacing([8.0, 3.0])
            .striped(true)
            .show(ui, |ui| {
                ui.strong("キー");
                ui.strong("種類");
                ui.strong("操作");
                ui.strong("相手");
                ui.end_row();

                for conflict in conflicts {
                    ui.monospace(conflict.chord.display_name());
                    ui.label(binding_conflict_kind_label(conflict.kind))
                        .on_hover_text(binding_conflict_kind_help(conflict.kind));
                    if ui
                        .button(compact_key_action_label(conflict.action))
                        .on_hover_text(conflict.action.description())
                        .clicked()
                    {
                        open_command_editor_dialog(state, conflict.action, Some(conflict.chord));
                    }
                    if let Some(other) = conflict.other_action {
                        if ui
                            .button(compact_key_action_label(other))
                            .on_hover_text(other.description())
                            .clicked()
                        {
                            open_command_editor_dialog(state, other, Some(conflict.chord));
                        }
                    } else {
                        ui.label(conflict.reserved_name.unwrap_or("固定キー"));
                    }
                    ui.end_row();
                }
            });
    });
}

fn command_list(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    keymap: &Keymap,
    conflicts: &[BindingConflict],
) {
    let filter = state.command_filter.trim().to_string();
    let key_filter = state.command_key_filter.trim().to_string();
    let conflicted = conflicted_actions(conflicts);
    ui.label(egui::RichText::new("コマンド一覧").strong());

    egui::Grid::new("command_settings_actions")
        .num_columns(5)
        .spacing([8.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.strong("編集");
            ui.strong("操作");
            ui.strong("キー");
            ui.strong("場所");
            ui.strong("状態");
            ui.end_row();

            for &action in KeyAction::all()
                .iter()
                .filter(|action| action.is_user_facing())
            {
                if !operation_keyboard_context_filter_matches(
                    state.operation_keyboard_context,
                    action.context(),
                ) {
                    continue;
                }
                let labels = keymap.chord_labels(action);
                if !command_action_matches_filter(action, &filter)
                    || !command_key_labels_match_filter(&labels, &key_filter)
                {
                    continue;
                }
                let selected = state.command_selected == Some(action);
                if ui
                    .selectable_label(selected, if selected { "選択中" } else { "編集" })
                    .clicked()
                {
                    open_command_editor_dialog(state, action, None);
                }

                ui.label(compact_key_action_label(action))
                    .on_hover_text(action.description());

                assignment_values(ui, &labels);

                ui.label(key_action_context_label(action));

                let overridden = state
                    .settings
                    .keymap
                    .override_chord_labels(action)
                    .is_some();
                let mut status = Vec::new();
                if overridden {
                    status.push("上書き");
                }
                if conflicted.contains(&action) {
                    status.push("競合");
                }
                if status.is_empty() {
                    ui.label(egui::RichText::new("既定").weak());
                } else {
                    let color = if conflicted.contains(&action) {
                        egui::Color32::from_rgb(220, 120, 80)
                    } else {
                        ui.visuals().text_color()
                    };
                    ui.label(egui::RichText::new(status.join(" / ")).color(color));
                }
                ui.end_row();
            }
        });
}

fn command_editor_for_action(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    keymap: &Keymap,
    conflicts: &[BindingConflict],
    ime_active: bool,
    action: KeyAction,
) {
    ui.label(egui::RichText::new("割り当て編集").strong());
    ensure_command_editor_loaded(state, keymap, action);
    if let Some(slot) = state.command_capture_slot
        && let Some(result) = poll_command_chord_capture(ui.ctx(), action, ime_active)
    {
        match result {
            Ok(label) if slot < state.command_chord_inputs.len() => {
                state.command_chord_inputs[slot] = label;
                state.command_edit_error = None;
                state.command_edit_notice = Some(format!(
                    "キー {} に {} を入力しました。「適用して閉じる」で保存します。",
                    slot + 1,
                    state.command_chord_inputs[slot]
                ));
            }
            Ok(_) => {}
            Err(message) => {
                state.command_edit_error = Some(message);
                state.command_edit_notice = None;
            }
        }
        state.command_capture_slot = None;
    }

    let title = compact_key_action_label(action);
    ui.label(egui::RichText::new(&title).strong());
    if title != action.description() {
        ui.label(action.description());
    }
    ui.small(format!(
        "{} / {}",
        key_action_context_label(action),
        key_trigger_label(action.trigger())
    ));

    let effective_labels = keymap.chord_labels(action);
    ui.small("現在有効なキー:");
    assignment_summary(ui, &[("キー", effective_labels)]);

    let assignment_state = match state.settings.keymap.override_chord_labels(action) {
        Some(labels) if labels.is_empty() => "状態: キー割り当て解除を保存済み",
        Some(_) => "状態: 上書き設定を使用中",
        None => "状態: 既定キーを使用中",
    };
    ui.small(assignment_state);
    if let Some(source_chord) = state.command_editor_source_chord {
        ui.small(format!(
            "選択中のキー: {}。空欄に追加するか、任意の行をこのキーに置換できます。",
            source_chord.display_name()
        ));
    }

    ui.add_space(8.0);
    let modifier_hold = action.trigger() == KeyTrigger::ModifierHold;
    if modifier_hold {
        modifier_hold_command_editor(ui, state);
    } else {
        let preview_conflicts = preview_command_editor_conflicts(state, action, conflicts);
        for idx in 0..state.command_chord_inputs.len() {
            ui.horizontal(|ui| {
                ui.add_sized(
                    [48.0, ui.spacing().interact_size.y],
                    egui::Label::new(format!("キー {}", idx + 1)),
                );
                {
                    let input = &mut state.command_chord_inputs[idx];
                    ui.add_sized(
                        [command_chord_input_width(), ui.spacing().interact_size.y],
                        egui::TextEdit::singleline(input).hint_text(if idx == 0 {
                            "例: Ctrl+F / F13 / none"
                        } else {
                            ""
                        }),
                    );
                }
                let capture_active = state.command_capture_slot == Some(idx);
                let capture_label = if capture_active {
                    "入力待ち..."
                } else {
                    "押して入力"
                };
                if ui
                    .add(egui::Button::new(capture_label).small())
                    .on_hover_text("次に押したキーをこの欄へ入れます。Esc でキャンセルします")
                    .clicked()
                {
                    state.command_capture_slot = Some(idx);
                    state.command_edit_error = None;
                    state.command_edit_notice = None;
                }
                if ui
                    .add(egui::Button::new("解除").small())
                    .on_hover_text("この行のキー割り当てだけを解除します")
                    .clicked()
                {
                    state.command_chord_inputs[idx].clear();
                    if state.command_capture_slot == Some(idx) {
                        state.command_capture_slot = None;
                    }
                    state.command_edit_error = None;
                    state.command_edit_notice =
                        Some(format!("キー {} の割り当てを解除しました。", idx + 1));
                }
                if let Some(source_chord) = state.command_editor_source_chord {
                    let source_label = source_chord.display_name();
                    let already_this_row = command_input_matches_chord(
                        action,
                        state.command_chord_inputs[idx].as_str(),
                        source_chord,
                    );
                    if already_this_row {
                        ui.label(egui::RichText::new("選択中").weak());
                    } else if ui
                        .add(egui::Button::new("このキーに置換").small())
                        .on_hover_text(format!(
                            "キー {} を {} に置き換えます",
                            idx + 1,
                            source_label
                        ))
                        .clicked()
                    {
                        state.command_chord_inputs[idx] = source_label;
                        state.command_capture_slot = None;
                        state.command_edit_error = None;
                        state.command_edit_notice = Some(format!(
                            "キー {} を {} に置き換えました。「適用して閉じる」で保存します。",
                            idx + 1,
                            state.command_chord_inputs[idx]
                        ));
                    }
                }
            });
            if let Some(label) = command_slot_conflict_label(
                &preview_conflicts,
                action,
                state.command_chord_inputs[idx].as_str(),
            ) {
                ui.horizontal(|ui| {
                    ui.add_space(56.0);
                    ui.colored_label(egui::Color32::from_rgb(220, 150, 80), label)
                        .on_hover_text("このキーを同時に使う可能性がある操作があります。必要に応じて片方を変更するか解除してください。");
                });
            }
        }
        ui.small("空欄または none の行は保存時に割り当てから外れます。すべて空欄にすると、この操作のキー割り当ては解除されます。");
    }

    if let Some(error) = &state.command_edit_error {
        ui.colored_label(egui::Color32::from_rgb(220, 90, 80), error);
    }
    if let Some(notice) = &state.command_edit_notice {
        ui.colored_label(egui::Color32::from_rgb(80, 130, 180), notice);
    }

    ui.horizontal(|ui| {
        if ui.button("適用して閉じる").clicked() {
            apply_command_editor(state, action);
            if state.command_edit_error.is_none() {
                close_assignment_editors(state);
            }
        }
        if ui.button("既定に戻す").clicked() {
            state.settings.keymap.remove_override(action);
            state.command_edit_loaded_for = None;
            state.command_capture_slot = None;
            state.command_edit_error = None;
            close_assignment_editors(state);
        }
        if ui.button("閉じる").clicked() {
            close_assignment_editors(state);
        }
    });

    let related: Vec<BindingConflict> = conflicts
        .iter()
        .copied()
        .filter(|conflict| conflict.action == action || conflict.other_action == Some(action))
        .collect();
    if !related.is_empty() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("このコマンドの競合")
                .strong()
                .color(egui::Color32::from_rgb(220, 150, 80)),
        );
        for conflict in related {
            ui.horizontal(|ui| {
                ui.monospace(conflict.chord.display_name());
                ui.label(binding_conflict_kind_label(conflict.kind))
                    .on_hover_text(binding_conflict_kind_help(conflict.kind));
                let other = if conflict.action == action {
                    conflict.other_action
                } else {
                    Some(conflict.action)
                };
                if let Some(other) = other {
                    if ui
                        .button(compact_key_action_label(other))
                        .on_hover_text(other.description())
                        .clicked()
                    {
                        select_command_action(state, other);
                    }
                } else if let Some(name) = conflict.reserved_name {
                    ui.label(name);
                }
            });
        }
    }
}

fn command_chord_input_width() -> f32 {
    300.0
}

fn chord_assignment_candidate_status(keymap: &Keymap, action: KeyAction, chord: Chord) -> String {
    if keymap.effective_chords(action).contains(&chord) {
        return "割り当て済み".to_string();
    }
    let labels = keymap.chord_labels(action);
    if labels.len() >= 3 {
        "置換が必要".to_string()
    } else {
        "空きあり".to_string()
    }
}

fn command_input_matches_chord(action: KeyAction, input: &str, chord: Chord) -> bool {
    parse_chord_for_action(action, input.trim())
        .ok()
        .flatten()
        .is_some_and(|parsed| parsed == chord)
}

fn preview_command_editor_conflicts(
    state: &PreferencesState,
    action: KeyAction,
    fallback: &[BindingConflict],
) -> Vec<BindingConflict> {
    let Ok(chords) = parse_command_chord_inputs_for_editor(action, &state.command_chord_inputs)
    else {
        return fallback.to_vec();
    };
    let mut settings = state.settings.keymap.clone();
    if chords.is_empty() {
        settings.disable_action(action);
    } else {
        settings.set_override_chords(action, chords);
    }
    Keymap::from_settings(&settings).binding_conflicts()
}

fn command_slot_conflict_label(
    conflicts: &[BindingConflict],
    action: KeyAction,
    input: &str,
) -> Option<String> {
    let chord = parse_chord_for_action(action, input.trim())
        .ok()
        .flatten()?;
    let mut peers = Vec::new();
    for conflict in conflicts
        .iter()
        .filter(|conflict| conflict.chord == chord)
        .filter(|conflict| conflict.action == action || conflict.other_action == Some(action))
    {
        let peer = if conflict.action == action {
            conflict
                .other_action
                .map(compact_key_action_label)
                .or_else(|| conflict.reserved_name.map(str::to_string))
        } else {
            Some(compact_key_action_label(conflict.action))
        };
        if let Some(peer) = peer
            && !peers.contains(&peer)
        {
            peers.push(peer);
        }
    }
    if peers.is_empty() {
        None
    } else {
        Some(format!("「{}」と競合", peers.join("、")))
    }
}

fn modifier_hold_command_editor(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.label("長押しに使う修飾キー:");
    ui.horizontal_wrapped(|ui| {
        for label in ["Ctrl", "Shift", "Alt"] {
            let selected = modifier_hold_editor_choice(&state.command_chord_inputs) == Some(label);
            if ui.selectable_label(selected, label).clicked() {
                set_single_command_chord_input(state, label);
            }
        }
        let disabled = modifier_hold_editor_choice(&state.command_chord_inputs).is_none();
        if ui.selectable_label(disabled, "割り当て解除").clicked() {
            state.command_chord_inputs = std::array::from_fn(|_| String::new());
            state.command_capture_slot = None;
            state.command_edit_error = None;
        }
    });
    ui.small("Ctrl / Shift / Alt のいずれか、または割り当て解除を選んでから「適用して閉じる」を押します。");
}

fn modifier_hold_editor_choice(inputs: &[String; 3]) -> Option<&'static str> {
    let mut labels = inputs.iter().map(|s| s.trim()).filter(|s| !s.is_empty());
    let first = labels.next()?;
    if labels.next().is_some() || first.eq_ignore_ascii_case("none") {
        return None;
    }
    if first.eq_ignore_ascii_case("ctrl") || first.eq_ignore_ascii_case("control") {
        Some("Ctrl")
    } else if first.eq_ignore_ascii_case("shift") {
        Some("Shift")
    } else if first.eq_ignore_ascii_case("alt") {
        Some("Alt")
    } else {
        None
    }
}

fn set_single_command_chord_input(state: &mut PreferencesState, label: &str) {
    state.command_chord_inputs = std::array::from_fn(|idx| {
        if idx == 0 {
            label.to_string()
        } else {
            String::new()
        }
    });
    state.command_capture_slot = None;
    state.command_edit_error = None;
}

fn assignment_values(ui: &mut egui::Ui, values: &[String]) {
    if values.is_empty() {
        ui.label(egui::RichText::new("未設定").weak());
    } else {
        ui.vertical(|ui| {
            for value in values {
                ui.monospace(value);
            }
        });
    }
}

fn select_command_action(state: &mut PreferencesState, action: KeyAction) {
    state.command_selected = Some(action);
    state.command_edit_loaded_for = None;
    state.command_capture_slot = None;
    state.command_edit_error = None;
    state.command_edit_notice = None;
}

fn ensure_command_editor_loaded(state: &mut PreferencesState, keymap: &Keymap, action: KeyAction) {
    if state.command_edit_loaded_for == Some(action) {
        return;
    }
    let labels = state
        .settings
        .keymap
        .override_chord_labels(action)
        .unwrap_or_else(|| keymap.chord_labels(action));
    state.command_chord_inputs =
        std::array::from_fn(|idx| labels.get(idx).cloned().unwrap_or_default());
    state.command_edit_loaded_for = Some(action);
    state.command_capture_slot = None;
    state.command_edit_error = None;
    state.command_edit_notice = None;
}

fn apply_command_editor(state: &mut PreferencesState, action: KeyAction) {
    let deduped = match parse_command_chord_inputs_for_editor(action, &state.command_chord_inputs) {
        Ok(chords) => chords,
        Err(message) => {
            state.command_edit_error = Some(message);
            return;
        }
    };
    if deduped.is_empty() {
        state.settings.keymap.disable_action(action);
    } else {
        state.settings.keymap.set_override_chords(action, deduped);
    }
    state.command_edit_loaded_for = None;
    state.command_capture_slot = None;
    state.command_edit_error = None;
    state.command_edit_notice = Some("キー割り当てを保存しました。".to_string());
}

fn parse_command_chord_inputs_for_editor(
    action: KeyAction,
    inputs: &[String; 3],
) -> Result<Vec<Chord>, String> {
    let mut deduped = Vec::new();
    for input in inputs
        .iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case("none"))
    {
        match parse_chord_for_action(action, &input) {
            Ok(Some(chord)) => {
                if !deduped.contains(&chord) {
                    deduped.push(chord);
                }
            }
            Ok(None) => {}
            Err(err) => {
                return Err(format!("{input}: {err}"));
            }
        }
    }
    Ok(deduped.into_iter().take(3).collect())
}

fn poll_command_chord_capture(
    ctx: &egui::Context,
    action: KeyAction,
    ime_active: bool,
) -> Option<Result<String, String>> {
    if ime_active {
        return None;
    }
    #[cfg(windows)]
    if crate::key_input::is_frame_active() {
        let mut result = None;
        crate::key_input::consume_key_down(false, |edge| {
            if is_win32_modifier_key(edge.virtual_key) {
                return false;
            }
            let Some(name) = KeyName::from_win32(edge.virtual_key, edge.scan_code, edge.extended)
            else {
                result = Some(Err(format!(
                    "このキーは割り当て対象外です: VK=0x{:02X}, scan=0x{:02X}",
                    edge.virtual_key, edge.scan_code
                )));
                return true;
            };
            if name == KeyName::Esc {
                result = Some(Err("キー入力待ちをキャンセルしました。".to_string()));
                return true;
            }
            let chord = Chord::new(edge.ctrl, edge.shift, edge.alt, name);
            let label = chord.display_name();
            result = Some(parse_chord_for_action(action, &label).map(|_| label));
            true
        });
        return result;
    }
    ctx.input_mut(|i| {
        let mut result = None;
        i.events.retain(|event| {
            if result.is_some() {
                return true;
            }
            let egui::Event::Key {
                key,
                pressed,
                repeat,
                modifiers,
                ..
            } = event
            else {
                return true;
            };
            if !*pressed || *repeat {
                return true;
            }
            if *key == egui::Key::Escape {
                result = Some(Err("キー入力待ちをキャンセルしました。".to_string()));
                return false;
            }
            let Some(name) = KeyName::from_egui(*key) else {
                result = Some(Err(format!("このキーは割り当て対象外です: {key:?}")));
                return false;
            };
            let chord = Chord::new(modifiers.ctrl, modifiers.shift, modifiers.alt, name);
            let label = chord.display_name();
            result = Some(parse_chord_for_action(action, &label).map(|_| label));
            false
        });
        result
    })
}

#[cfg(windows)]
fn is_win32_modifier_key(virtual_key: u32) -> bool {
    matches!(virtual_key, 0x10 | 0x11 | 0x12 | 0xA0..=0xA5)
}

fn command_action_matches_filter(action: KeyAction, filter: &str) -> bool {
    operation_text_matches_filter(
        &compact_key_action_label(action),
        action.description(),
        key_action_context_label(action),
        filter,
    )
}

fn operation_text_matches_filter(
    label: &str,
    description: &str,
    context: &str,
    filter: &str,
) -> bool {
    let filter = filter.trim().to_ascii_lowercase();
    if filter.is_empty() {
        return true;
    }
    label.to_ascii_lowercase().contains(&filter)
        || description.to_ascii_lowercase().contains(&filter)
        || context.to_ascii_lowercase().contains(&filter)
}

fn command_key_labels_match_filter(labels: &[String], filter: &str) -> bool {
    let filter = filter.trim().to_ascii_lowercase();
    if filter.is_empty() {
        return true;
    }
    labels
        .iter()
        .any(|label| label.to_ascii_lowercase().contains(&filter))
}

fn conflicted_actions(conflicts: &[BindingConflict]) -> HashSet<KeyAction> {
    let mut actions = HashSet::new();
    for conflict in conflicts {
        actions.insert(conflict.action);
        if let Some(other) = conflict.other_action {
            actions.insert(other);
        }
    }
    actions
}

fn binding_conflict_kind_label(kind: BindingConflictKind) -> &'static str {
    match kind {
        BindingConflictKind::Hard => "同じ文脈",
        BindingConflictKind::ActiveOverlap => "同時有効",
        BindingConflictKind::TriggerMismatch => "種類違い",
        BindingConflictKind::Reserved => "固定キー",
    }
}

fn binding_conflict_kind_help(kind: BindingConflictKind) -> &'static str {
    match kind {
        BindingConflictKind::Hard => {
            "同じ文脈の操作に同じキーが割り当てられています。同時には使い分けできないため、処理順で先に判定された操作が優先されます。"
        }
        BindingConflictKind::ActiveOverlap => {
            "別の文脈の操作ですが、同じ画面で同時に有効になる可能性があります。必要に応じて片方を別キーに変更してください。"
        }
        BindingConflictKind::TriggerMismatch => {
            "同じキーが、押下・長押し・修飾キー長押しなど別の種類の入力に割り当てられています。意図しない反応になる場合があります。"
        }
        BindingConflictKind::Reserved => {
            "Esc / Enter / 修飾なし矢印など、固定操作として扱うキーへの割り当てです。固定操作が優先される場合があります。"
        }
    }
}

fn key_trigger_label(trigger: KeyTrigger) -> &'static str {
    match trigger {
        KeyTrigger::Press => "押下",
        KeyTrigger::ModifierHold => "修飾キー長押し",
        KeyTrigger::KeyHold => "キー長押し",
    }
}

pub(super) fn page_menu_layout(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.small("メニューバーの上位メニューと固定項目の表示順を変更します。登録済みお気に入り、タグ一覧、更新確認など状態で変わる項目は固定位置に残ります。");
    ui.add_space(8.0);

    let layout_snapshot = state.settings.menu_layout.clone();
    let top_order = menu_layout_top_order(&layout_snapshot);
    let hidden = menu_layout_hidden_set(&layout_snapshot);
    let mut edit: Option<MenuLayoutEdit> = None;

    ui.horizontal(|ui| {
        if ui.button("既定に戻す").clicked() {
            edit = Some(MenuLayoutEdit::Reset);
        }
        if ui.button("すべて表示").clicked() {
            edit = Some(MenuLayoutEdit::ShowAll);
        }
    });

    ui.add_space(8.0);

    for (top_index, &top) in top_order.iter().enumerate() {
        let command_order = menu_layout_command_order(&layout_snapshot, top);
        let visible_count = command_order
            .iter()
            .filter(|id| !hidden.contains(id))
            .count();
        let mut top_visible = visible_count > 0;

        ui.separator();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(top_index > 0, egui::Button::new("↑").small())
                .on_hover_text("上へ")
                .clicked()
            {
                edit = Some(MenuLayoutEdit::MoveTop(top_index, -1));
            }
            if ui
                .add_enabled(
                    top_index + 1 < top_order.len(),
                    egui::Button::new("↓").small(),
                )
                .on_hover_text("下へ")
                .clicked()
            {
                edit = Some(MenuLayoutEdit::MoveTop(top_index, 1));
            }
            if ui.checkbox(&mut top_visible, top.label()).changed() {
                edit = Some(MenuLayoutEdit::SetTopVisible(top, top_visible));
            }
            ui.label(
                egui::RichText::new(format!("{visible_count} / {}", command_order.len()))
                    .weak()
                    .size(11.0),
            );
        });

        egui::CollapsingHeader::new(format!("{} の項目", top.label()))
            .id_salt(("menu_layout_top", top))
            .default_open(top_visible)
            .show(ui, |ui| {
                egui::Grid::new(("menu_layout_commands", top))
                    .num_columns(4)
                    .spacing([8.0, 4.0])
                    .striped(true)
                    .show(ui, |ui| {
                        for (command_index, &command) in command_order.iter().enumerate() {
                            let mut command_visible = !hidden.contains(&command);
                            let can_hide = menu_command_can_be_hidden(command);
                            let checkbox = egui::Checkbox::new(&mut command_visible, "");
                            if ui
                                .add_enabled(can_hide, checkbox)
                                .on_hover_text(if can_hide {
                                    "表示"
                                } else {
                                    "環境設定への入口なので非表示にできません"
                                })
                                .changed()
                            {
                                edit = Some(MenuLayoutEdit::SetCommandVisible(
                                    command,
                                    command_visible,
                                ));
                            }

                            let label = menu_command_spec(command)
                                .map(|spec| spec.label)
                                .unwrap_or(command.stable_name());
                            ui.label(label);

                            if ui
                                .add_enabled(command_index > 0, egui::Button::new("↑").small())
                                .on_hover_text("上へ")
                                .clicked()
                            {
                                edit = Some(MenuLayoutEdit::MoveCommand(top, command_index, -1));
                            }
                            if ui
                                .add_enabled(
                                    command_index + 1 < command_order.len(),
                                    egui::Button::new("↓").small(),
                                )
                                .on_hover_text("下へ")
                                .clicked()
                            {
                                edit = Some(MenuLayoutEdit::MoveCommand(top, command_index, 1));
                            }
                            ui.end_row();
                        }
                    });
            });
    }

    if let Some(edit) = edit {
        apply_menu_layout_edit(&mut state.settings.menu_layout, edit);
    }
}

enum MenuLayoutEdit {
    Reset,
    ShowAll,
    MoveTop(usize, i32),
    SetTopVisible(TopMenuId, bool),
    MoveCommand(TopMenuId, usize, i32),
    SetCommandVisible(MenuCommandId, bool),
}

fn apply_menu_layout_edit(layout: &mut MenuLayoutSettings, edit: MenuLayoutEdit) {
    match edit {
        MenuLayoutEdit::Reset => {
            *layout = MenuLayoutSettings::default();
        }
        MenuLayoutEdit::ShowAll => {
            layout.hidden_commands.clear();
        }
        MenuLayoutEdit::MoveTop(index, delta) => {
            let mut order = menu_layout_top_order(layout);
            if move_index(&mut order, index, delta) {
                write_menu_layout_top_order(layout, &order);
            }
        }
        MenuLayoutEdit::SetTopVisible(top, visible) => {
            let mut hidden = menu_layout_hidden_set(layout);
            for spec in menu_commands_for_parent(top) {
                if visible {
                    hidden.remove(&spec.id);
                } else if menu_command_can_be_hidden(spec.id) {
                    hidden.insert(spec.id);
                }
            }
            write_menu_layout_hidden(layout, &hidden);
        }
        MenuLayoutEdit::MoveCommand(parent, index, delta) => {
            let mut order = menu_layout_command_order(layout, parent);
            if move_index(&mut order, index, delta) {
                write_menu_layout_command_order(layout, parent, &order);
            }
        }
        MenuLayoutEdit::SetCommandVisible(command, visible) => {
            let mut hidden = menu_layout_hidden_set(layout);
            if visible {
                hidden.remove(&command);
            } else if menu_command_can_be_hidden(command) {
                hidden.insert(command);
            }
            write_menu_layout_hidden(layout, &hidden);
        }
    }
}

fn move_index<T>(items: &mut [T], index: usize, delta: i32) -> bool {
    let Some(target) = (index as i32).checked_add(delta) else {
        return false;
    };
    if target < 0 || target as usize >= items.len() {
        return false;
    }
    items.swap(index, target as usize);
    true
}

fn menu_layout_top_order(layout: &MenuLayoutSettings) -> Vec<TopMenuId> {
    let mut out = Vec::with_capacity(TopMenuId::ALL.len());
    for name in &layout.top_menu_order {
        if let Some(id) = TopMenuId::parse_stable_name(name)
            && !out.contains(&id)
        {
            out.push(id);
        }
    }
    for &id in TopMenuId::ALL {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

fn write_menu_layout_top_order(layout: &mut MenuLayoutSettings, order: &[TopMenuId]) {
    layout.top_menu_order = order
        .iter()
        .map(|id| id.stable_name().to_string())
        .collect();
}

fn menu_layout_command_order(layout: &MenuLayoutSettings, parent: TopMenuId) -> Vec<MenuCommandId> {
    let mut out = Vec::new();
    for order in &layout.command_order {
        if TopMenuId::parse_stable_name(&order.parent) != Some(parent) {
            continue;
        }
        for name in &order.commands {
            let Some(id) = MenuCommandId::parse_stable_name(name) else {
                continue;
            };
            if out.contains(&id) {
                continue;
            }
            if menu_command_spec(id).is_some_and(|spec| spec.parent == parent) {
                out.push(id);
            }
        }
    }
    for spec in menu_commands_for_parent(parent) {
        if !out.contains(&spec.id) {
            out.push(spec.id);
        }
    }
    out
}

fn write_menu_layout_command_order(
    layout: &mut MenuLayoutSettings,
    parent: TopMenuId,
    order: &[MenuCommandId],
) {
    layout
        .command_order
        .retain(|entry| TopMenuId::parse_stable_name(&entry.parent) != Some(parent));
    layout.command_order.push(MenuCommandOrderSettings {
        parent: parent.stable_name().to_string(),
        commands: order
            .iter()
            .map(|id| id.stable_name().to_string())
            .collect(),
    });
}

fn menu_layout_hidden_set(layout: &MenuLayoutSettings) -> BTreeSet<MenuCommandId> {
    layout
        .hidden_commands
        .iter()
        .filter_map(|name| MenuCommandId::parse_stable_name(name))
        .filter(|id| menu_command_can_be_hidden(*id))
        .collect()
}

fn write_menu_layout_hidden(layout: &mut MenuLayoutSettings, hidden: &BTreeSet<MenuCommandId>) {
    layout.hidden_commands = MenuCommandId::ALL
        .iter()
        .copied()
        .filter(|id| menu_command_can_be_hidden(*id))
        .filter(|id| hidden.contains(id))
        .map(|id| id.stable_name().to_string())
        .collect();
}

fn right_drag_mode_combo(
    ui: &mut egui::Ui,
    settings: &mut RingShortcutSettings,
    context: RightDragContext,
) {
    ui.label(context.label());
    let mut mode = settings.right_drag_mode(context);
    if context.ring_context().is_none() && mode == RightDragMode::RingShortcut {
        mode = RightDragMode::Disabled;
    }
    egui::ComboBox::from_id_salt(("right_drag_mode", context))
        .width(220.0)
        .selected_text(mode.label())
        .show_ui(ui, |ui| {
            ui.selectable_value(
                &mut mode,
                RightDragMode::Disabled,
                RightDragMode::Disabled.label(),
            );
            if context.ring_context().is_some() {
                ui.selectable_value(
                    &mut mode,
                    RightDragMode::RingShortcut,
                    RightDragMode::RingShortcut.label(),
                );
            } else {
                ui.add_enabled(false, egui::Label::new(RightDragMode::RingShortcut.label()))
                    .on_hover_text(
                        "編集モードのリング割り当ては未対応です。ジェスチャを使ってください。",
                    );
            }
            ui.selectable_value(
                &mut mode,
                RightDragMode::MouseGesture,
                RightDragMode::MouseGesture.label(),
            );
        });
    settings.set_right_drag_mode(context, mode);
    ui.end_row();
}
fn mouse_button_context_editor(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    context: RingShortcutContext,
) {
    ui.separator();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(context.label()).strong());
        if ui.button("既定に戻す").clicked() {
            state
                .settings
                .ring_shortcuts
                .reset_mouse_button_profile(context);
        }
    });

    state
        .settings
        .ring_shortcuts
        .mouse_button_profile_mut(context)
        .sanitize(context);
    egui::Grid::new(("mouse_button_bindings", context))
        .num_columns(3)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            mouse_button_row(ui, state, context, MouseButtonSlot::Back);
            mouse_button_row(ui, state, context, MouseButtonSlot::Forward);
            mouse_button_row(ui, state, context, MouseButtonSlot::Middle);
        });
}

fn mouse_button_row(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    context: RingShortcutContext,
    slot: MouseButtonSlot,
) {
    let action = state
        .settings
        .ring_shortcuts
        .mouse_button_profile(context)
        .action(slot);
    if ui.small_button("編集").clicked() {
        open_operation_assignment_editor(
            state,
            OperationAssignmentTarget::MouseButton { context, slot },
            OperationAssignmentTab::MouseButtons,
        );
    }
    ui.label(slot.label());
    ui.label(action.label_for_context(context))
        .on_hover_text(ring_action_detail_label(&action, context));
    ui.end_row();
}

fn mouse_button_assignment_editor(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    context: RingShortcutContext,
    slot: MouseButtonSlot,
) {
    let mut close_requested = false;
    ui.group(|ui| {
        ui.label(egui::RichText::new(format!("{} / {}", context.label(), slot.label())).strong());
        ui.small("なしを選ぶとこのボタン単体では何もしません。");
        ui.add_space(6.0);

        let available = RingActionId::available_for_mouse_button_context(context);
        let profile = state
            .settings
            .ring_shortcuts
            .mouse_button_profile_mut(context);
        profile.sanitize(context);
        let value = match slot {
            MouseButtonSlot::Back => &mut profile.back,
            MouseButtonSlot::Forward => &mut profile.forward,
            MouseButtonSlot::Middle => &mut profile.middle,
        };
        egui::ComboBox::from_id_salt(("mouse_button_assignment_editor", context, slot))
            .width(300.0)
            .selected_text(value.label_for_context(context))
            .show_ui(ui, |ui| {
                for action in &available {
                    ui.selectable_value(value, action.clone(), action.label_for_context(context))
                        .on_hover_text(ring_action_detail_label(action, context));
                }
            });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("割り当て解除").clicked() {
                *value = RingActionId::None;
            }
            if ui.button("閉じる").clicked() {
                close_requested = true;
            }
        });
    });
    if close_requested {
        close_assignment_editors(state);
    }
}

fn mouse_gesture_context_editor(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    context: RightDragContext,
) {
    let mut open_recorder = false;
    ui.separator();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(context.label()).strong());
        if ui.button("ジェスチャを追加").clicked() {
            open_recorder = true;
        }
        if ui.button("既定に戻す").clicked() {
            state
                .settings
                .ring_shortcuts
                .reset_mouse_gesture_profile(context);
            state
                .operation_mouse_gesture_inputs
                .retain(|(ctx, _), _| *ctx != context);
        }
    });

    let action_context = context.gesture_action_context();
    let rows = {
        let profile = state
            .settings
            .ring_shortcuts
            .mouse_gesture_profile_mut(context);
        profile.sanitize(context);
        profile.bindings.clone()
    };
    state
        .operation_mouse_gesture_inputs
        .retain(|(ctx, idx), _| *ctx != context || *idx < rows.len());

    egui::Grid::new(("mouse_gesture_bindings", context))
        .num_columns(4)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label(egui::RichText::new("編集").strong());
            ui.label(egui::RichText::new("ジェスチャ").strong());
            ui.label(egui::RichText::new("操作").strong());
            ui.label("");
            ui.end_row();

            for (idx, binding) in rows.iter().enumerate() {
                if ui.small_button("編集").clicked() {
                    open_operation_assignment_editor(
                        state,
                        OperationAssignmentTarget::MouseGesture {
                            context,
                            index: idx,
                        },
                        OperationAssignmentTab::MouseGesture,
                    );
                }
                ui.monospace(format_mouse_gesture_pattern(&binding.pattern));
                ui.label(binding.action.label_for_context(action_context))
                    .on_hover_text(ring_action_detail_label(&binding.action, action_context));
                ui.label(egui::RichText::new("再記録は編集から").size(11.0).weak());
                ui.end_row();
            }
        });

    let profile = state.settings.ring_shortcuts.mouse_gesture_profile(context);
    let mut seen = BTreeSet::new();
    let mut duplicates = Vec::new();
    for binding in &profile.bindings {
        let label = format_mouse_gesture_pattern(&binding.pattern);
        if !seen.insert(label.clone()) {
            duplicates.push(label);
        }
    }
    if !duplicates.is_empty() {
        duplicates.sort();
        duplicates.dedup();
        ui.colored_label(
            ui.visuals().error_fg_color,
            format!("同じジェスチャが重複しています: {}", duplicates.join(" / ")),
        );
    }
    if profile.bindings.is_empty() {
        ui.small("登録済みジェスチャはありません。右ドラッグ mode をマウスジェスチャにしても、この文脈では何も実行しません。");
    }

    if open_recorder {
        let action_context = context.gesture_action_context();
        let action = RingActionId::available_for_context(action_context)
            .into_iter()
            .find(|action| *action != RingActionId::None)
            .unwrap_or(RingActionId::None);
        state.operation_mouse_gesture_recorder = Some(OperationMouseGestureRecorder {
            context,
            action,
            replace_index: None,
            pattern: Vec::new(),
            points: Vec::new(),
            recording: false,
            error: None,
        });
    }
}

fn mouse_gesture_assignment_editor(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    context: RightDragContext,
    index: usize,
) {
    let action_context = context.gesture_action_context();
    let Some(binding) = state
        .settings
        .ring_shortcuts
        .mouse_gesture_profile(context)
        .bindings
        .get(index)
        .cloned()
    else {
        ui.small("このジェスチャは削除済みです。");
        if ui.button("閉じる").clicked() {
            close_assignment_editors(state);
        }
        return;
    };

    let mut action = binding.action.clone();
    let mut close_requested = false;
    let mut delete_requested = false;
    let mut rerecord_requested = false;
    ui.group(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "{} / {}",
                context.label(),
                format_mouse_gesture_pattern(&binding.pattern)
            ))
            .strong(),
        );
        ui.small("操作を変更するか、実際の右ドラッグでジェスチャを再記録します。");
        ui.add_space(6.0);

        egui::ComboBox::from_id_salt(("mouse_gesture_assignment_editor", context, index))
            .width(300.0)
            .selected_text(action.label_for_context(action_context))
            .show_ui(ui, |ui| {
                for candidate in RingActionId::available_for_context(action_context) {
                    ui.selectable_value(
                        &mut action,
                        candidate.clone(),
                        candidate.label_for_context(action_context),
                    )
                    .on_hover_text(ring_action_detail_label(&candidate, action_context));
                }
            });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("再記録").clicked() {
                rerecord_requested = true;
            }
            if ui.button("削除").clicked() {
                delete_requested = true;
            }
            if ui.button("閉じる").clicked() {
                close_requested = true;
            }
        });
    });

    if action != binding.action
        && let Some(slot) = state
            .settings
            .ring_shortcuts
            .mouse_gesture_profile_mut(context)
            .bindings
            .get_mut(index)
    {
        slot.action = action.clone();
    }
    if rerecord_requested {
        state.operation_mouse_gesture_recorder = Some(OperationMouseGestureRecorder {
            context,
            action: action.clone(),
            replace_index: Some(index),
            pattern: Vec::new(),
            points: Vec::new(),
            recording: false,
            error: None,
        });
    }
    if delete_requested {
        if index
            < state
                .settings
                .ring_shortcuts
                .mouse_gesture_profile(context)
                .bindings
                .len()
        {
            state
                .settings
                .ring_shortcuts
                .mouse_gesture_profile_mut(context)
                .bindings
                .remove(index);
        }
        state
            .operation_mouse_gesture_inputs
            .retain(|(ctx, _), _| *ctx != context);
        close_assignment_editors(state);
    } else if close_requested {
        close_assignment_editors(state);
    }
}

fn ring_action_slot_editor(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    context: RingShortcutContext,
    action: RingActionId,
) {
    ui.label(egui::RichText::new("リング / X+方向").strong());
    ui.small("チェックした方向にこの操作を割り当てます。既に別の操作が入っている方向をチェックすると置き換えます。");
    ui.add_space(6.0);

    let profile = state.settings.ring_shortcuts.profile_mut(context);
    profile.sanitize(context);
    egui::Grid::new(("ring_action_slot_editor", context, action.as_str()))
        .num_columns(4)
        .spacing([8.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            for &direction in RingDirection::all() {
                let idx = direction.slot_index();
                let current = profile
                    .slots
                    .get(idx)
                    .cloned()
                    .unwrap_or(RingActionId::None);
                let mut checked = current == action;
                if ui.checkbox(&mut checked, direction.label()).changed() {
                    profile.slots[idx] = if checked {
                        action.clone()
                    } else {
                        RingActionId::None
                    };
                }
                ui.label(current.label_for_context(context))
                    .on_hover_text(current.as_str());
                if idx % 2 == 1 {
                    ui.end_row();
                }
            }
        });
}

fn ring_action_mouse_button_editor(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    context: RingShortcutContext,
    action: RingActionId,
) {
    ui.label(egui::RichText::new("マウスボタン").strong());
    ui.small(
        "物理ボタンへこの操作を割り当てます。チェックを外すと、そのボタンは未割り当てになります。",
    );
    ui.add_space(6.0);

    if !action.is_valid_for_mouse_button_context(context) {
        ui.small("この操作はこの画面のマウスボタンには割り当てられません。");
        return;
    }

    let profile = state
        .settings
        .ring_shortcuts
        .mouse_button_profile_mut(context);
    let mut back = profile.back == action;
    if ui.checkbox(&mut back, "戻るボタン").changed() {
        profile.back = if back {
            action.clone()
        } else {
            RingActionId::None
        };
    }
    let mut forward = profile.forward == action;
    if ui.checkbox(&mut forward, "進むボタン").changed() {
        profile.forward = if forward {
            action.clone()
        } else {
            RingActionId::None
        };
    }
    let mut middle = profile.middle == action;
    if ui.checkbox(&mut middle, "ホイールクリック").changed() {
        profile.middle = if middle { action } else { RingActionId::None };
    }
}

fn ring_action_mouse_gesture_editor(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    context: RightDragContext,
    action: RingActionId,
) {
    ui.label(egui::RichText::new("マウスジェスチャ").strong());
    ui.small(
        "この操作へ登録済みのジェスチャだけを表示します。追加は実際に右ドラッグして記録します。",
    );
    ui.add_space(6.0);

    if ui.button("ジェスチャを追加").clicked() {
        state.operation_mouse_gesture_recorder = Some(OperationMouseGestureRecorder {
            context,
            action: action.clone(),
            replace_index: None,
            pattern: Vec::new(),
            points: Vec::new(),
            recording: false,
            error: None,
        });
    }

    let profile = state
        .settings
        .ring_shortcuts
        .mouse_gesture_profile_mut(context);
    profile.sanitize(context);
    let mut remove_idx = None;
    egui::Grid::new(("ring_action_mouse_gesture_editor", context, action.as_str()))
        .num_columns(3)
        .spacing([8.0, 4.0])
        .striped(true)
        .show(ui, |ui| {
            ui.strong("ジェスチャ");
            ui.strong("操作");
            ui.label("");
            ui.end_row();
            for (idx, binding) in profile.bindings.iter().enumerate() {
                if binding.action != action {
                    continue;
                }
                ui.monospace(format_mouse_gesture_pattern(&binding.pattern));
                ui.label(
                    binding
                        .action
                        .label_for_context(context.gesture_action_context()),
                );
                if ui.small_button("削除").clicked() {
                    remove_idx = Some(idx);
                }
                ui.end_row();
            }
        });
    if let Some(idx) = remove_idx {
        profile.bindings.remove(idx);
        state
            .operation_mouse_gesture_inputs
            .retain(|(ctx, _), _| *ctx != context);
    }
    if !profile
        .bindings
        .iter()
        .any(|binding| binding.action == action)
    {
        ui.small("この操作へ登録済みのジェスチャはありません。");
    }
}

fn ring_slot_assignment_editor(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    context: RingShortcutContext,
    direction: RingDirection,
) {
    let mut close_requested = false;
    ui.group(|ui| {
        ui.label(
            egui::RichText::new(format!("X+{} / {}", direction.label(), context.label()))
                .strong(),
        );
        ui.small("この方向に割り当てる操作を選びます。右ドラッグをリングショートカットにしている文脈では、同じ方向の右ドラッグにも使われます。");
        ui.add_space(6.0);

        let available = RingActionId::available_for_context(context);
        let profile = state.settings.ring_shortcuts.profile_mut(context);
        profile.sanitize(context);
        let idx = direction.slot_index();
        egui::ComboBox::from_id_salt(("ring_slot_assignment_editor", context, direction))
            .width(280.0)
            .selected_text(profile.slots[idx].label_for_context(context))
            .show_ui(ui, |ui| {
                for action in &available {
                    ui.selectable_value(
                        &mut profile.slots[idx],
                        action.clone(),
                        action.label_for_context(context),
                    )
                    .on_hover_text(ring_action_detail_label(action, context));
                }
            });
        ui.add_space(8.0);
        if ui.button("閉じる").clicked() {
            close_requested = true;
        }
    });
    if close_requested {
        close_assignment_editors(state);
    }
}

fn gamepad_layout_preview(ui: &mut egui::Ui, context: RingShortcutContext) {
    ui.group(|ui| {
        ui.label(egui::RichText::new("ゲームパッド固定ボタン").strong());
        ui.small("固定ボタンは既定動作で使い、下の X+方向リングだけ編集できます。");
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                gamepad_fixed_button_label(
                    ui,
                    "LT",
                    gamepad_left_trigger_label(context),
                    gamepad_trigger_tooltip(context, false),
                );
                gamepad_fixed_button_label(
                    ui,
                    "LB",
                    "前フォルダ",
                    "前のフォルダへ移動します。",
                );
                ui.add_space(4.0);
                ui.label(egui::RichText::new("方向 / 左スティック").strong().size(11.0));
                gamepad_dpad_preview(ui, context);
            });

            ui.add_space(16.0);
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    gamepad_fixed_button_label(
                        ui,
                        "Select",
                        gamepad_select_label(context),
                        gamepad_select_tooltip(context),
                    );
                    gamepad_fixed_button_label(
                        ui,
                        "Start",
                        "お気に入り",
                        "お気に入りの移動パネルを開きます。",
                    );
                });
                ui.add_space(8.0);
                ui.add_sized(
                    [204.0, 88.0],
                    egui::Label::new(
                        egui::RichText::new("X 単体\nピッカーパネル\n\nX+方向は下のリング設定")
                            .size(11.0),
                    )
                    .wrap()
                    .selectable(false),
                )
                .on_hover_text("X を方向入力なしで離すとピッカーパネルを開きます。X+方向リングは下の 8 方向スロットで編集します。");
            });

            ui.add_space(16.0);
            ui.vertical(|ui| {
                gamepad_fixed_button_label(
                    ui,
                    "RT",
                    gamepad_right_trigger_label(context),
                    gamepad_trigger_tooltip(context, true),
                );
                gamepad_fixed_button_label(
                    ui,
                    "RB",
                    "次フォルダ",
                    "次のフォルダへ移動します。",
                );
                ui.add_space(4.0);
                egui::Grid::new(("gamepad_face_buttons", context))
                    .num_columns(3)
                    .spacing([4.0, 4.0])
                    .show(ui, |ui| {
                        ui.label("");
                        gamepad_fixed_button_label(
                            ui,
                            "Y",
                            gamepad_y_label(context),
                            gamepad_y_tooltip(context),
                        );
                        ui.end_row();
                        gamepad_fixed_button_label(
                            ui,
                            "X",
                            "ピッカー",
                            "X 単体でピッカーパネルを開きます。X を押しながら方向入力すると、下の X+方向リングを実行します。",
                        );
                        gamepad_fixed_button_label(
                            ui,
                            "B",
                            gamepad_b_label(context),
                            gamepad_b_tooltip(context),
                        );
                        gamepad_fixed_button_label(
                            ui,
                            "A",
                            gamepad_a_label(context),
                            gamepad_a_tooltip(context),
                        );
                        ui.end_row();
                    });
                ui.add_space(4.0);
                gamepad_button_label(
                    ui,
                    "右スティック",
                    "ズーム",
                    gamepad_right_stick_tooltip(context),
                );
            });
        });
    });
}

fn gamepad_fixed_button_label(ui: &mut egui::Ui, button: &str, default_label: &str, tooltip: &str) {
    let hover = format!("{tooltip}\n\n既定: {default_label}");
    gamepad_button_label(ui, button, default_label, &hover);
}

fn gamepad_button_label(ui: &mut egui::Ui, button: &str, label: &str, tooltip: &str) {
    let text = format!("{button}\n{label}");
    ui.add_sized(
        [84.0, 40.0],
        egui::Label::new(egui::RichText::new(text).size(11.0))
            .wrap()
            .selectable(false),
    )
    .on_hover_text(tooltip);
}

fn gamepad_dpad_preview(ui: &mut egui::Ui, context: RingShortcutContext) {
    egui::Grid::new(("gamepad_dpad_preview", context))
        .num_columns(3)
        .spacing([4.0, 4.0])
        .show(ui, |ui| {
            ui.label("");
            gamepad_button_label(ui, "↑", "上", gamepad_direction_tooltip(context, "上"));
            ui.end_row();
            gamepad_button_label(ui, "←", "左", gamepad_direction_tooltip(context, "左"));
            ui.label(egui::RichText::new("移動").size(11.0).weak());
            gamepad_button_label(ui, "→", "右", gamepad_direction_tooltip(context, "右"));
            ui.end_row();
            ui.label("");
            gamepad_button_label(ui, "↓", "下", gamepad_direction_tooltip(context, "下"));
            ui.end_row();
        });
}

fn gamepad_ring_preview(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    profile: &crate::ring_shortcut::RingShortcutProfile,
    context: RingShortcutContext,
) {
    egui::Grid::new(("gamepad_ring_preview", context))
        .num_columns(3)
        .spacing([4.0, 4.0])
        .show(ui, |ui| {
            const CELLS: [Option<RingDirection>; 9] = [
                Some(RingDirection::UpLeft),
                Some(RingDirection::Up),
                Some(RingDirection::UpRight),
                Some(RingDirection::Left),
                None,
                Some(RingDirection::Right),
                Some(RingDirection::DownLeft),
                Some(RingDirection::Down),
                Some(RingDirection::DownRight),
            ];
            for (idx, cell) in CELLS.iter().enumerate() {
                match cell {
                    Some(direction) => {
                        let action = profile
                            .slots
                            .get(direction.slot_index())
                            .unwrap_or(&RingActionId::None);
                        let label = action.label_for_context(context);
                        let text = format!("X+{}\n{}", direction_short_label(*direction), label);
                        if ui
                            .add_sized(
                                [104.0, 42.0],
                                egui::Button::new(egui::RichText::new(text).size(11.0)).wrap(),
                            )
                            .on_hover_text(format!(
                                "{}\n{}",
                                direction.label(),
                                ring_action_detail_label(action, context)
                            ))
                            .clicked()
                        {
                            open_operation_assignment_editor(
                                state,
                                OperationAssignmentTarget::RingSlot {
                                    context,
                                    direction: *direction,
                                },
                                OperationAssignmentTab::RingPad,
                            );
                        }
                    }
                    None => {
                        ui.add_sized(
                            [104.0, 42.0],
                            egui::Label::new(egui::RichText::new("X 単体\nピッカー").size(11.0))
                                .wrap()
                                .selectable(false),
                        )
                        .on_hover_text("X を方向入力なしで離すとピッカーパネルを開きます。");
                    }
                }
                if (idx + 1) % 3 == 0 {
                    ui.end_row();
                }
            }
        });
}

fn direction_short_label(direction: RingDirection) -> &'static str {
    match direction {
        RingDirection::Up => "↑",
        RingDirection::UpRight => "↗",
        RingDirection::Right => "→",
        RingDirection::DownRight => "↘",
        RingDirection::Down => "↓",
        RingDirection::DownLeft => "↙",
        RingDirection::Left => "←",
        RingDirection::UpLeft => "↖",
    }
}

fn gamepad_direction_tooltip(context: RingShortcutContext, direction: &str) -> &'static str {
    match context {
        RingShortcutContext::Grid => {
            "サムネイル一覧ではカーソルを移動します。X を押しながら入力すると X+方向リングになります。"
        }
        RingShortcutContext::ImageFullscreen => {
            "画像フルスクリーンでは前後移動や連続読書スクロールに使います。X を押しながら入力すると X+方向リングになります。"
        }
        RingShortcutContext::VideoFullscreen => match direction {
            "左" | "右" => {
                "動画フルスクリーンではシークに使います。X を押しながら入力すると X+方向リングになります。"
            }
            _ => {
                "動画フルスクリーンでは前後ファイル移動に使います。X を押しながら入力すると X+方向リングになります。"
            }
        },
    }
}

fn gamepad_left_trigger_label(context: RingShortcutContext) -> &'static str {
    match context {
        RingShortcutContext::Grid => "スクロール",
        RingShortcutContext::ImageFullscreen => "ズームアウト",
        RingShortcutContext::VideoFullscreen => "左シーク",
    }
}

fn gamepad_right_trigger_label(context: RingShortcutContext) -> &'static str {
    match context {
        RingShortcutContext::Grid => "スクロール",
        RingShortcutContext::ImageFullscreen => "ズームイン",
        RingShortcutContext::VideoFullscreen => "右シーク",
    }
}

fn gamepad_trigger_tooltip(context: RingShortcutContext, right: bool) -> &'static str {
    match context {
        RingShortcutContext::Grid => "サムネイル一覧ではスクロールに使います。",
        RingShortcutContext::ImageFullscreen => "画像フルスクリーンではズームに使います。",
        RingShortcutContext::VideoFullscreen if right => {
            "動画フルスクリーンでは右方向シークに使います。"
        }
        RingShortcutContext::VideoFullscreen => "動画フルスクリーンでは左方向シークに使います。",
    }
}

fn gamepad_select_label(context: RingShortcutContext) -> &'static str {
    match context {
        RingShortcutContext::Grid => "場所パネル",
        RingShortcutContext::ImageFullscreen => "見開き切替",
        RingShortcutContext::VideoFullscreen => "マーカー移動",
    }
}

fn gamepad_select_tooltip(context: RingShortcutContext) -> &'static str {
    match context {
        RingShortcutContext::Grid => "場所移動パネルを開きます。",
        RingShortcutContext::ImageFullscreen => "見開き表示を切り替えます。",
        RingShortcutContext::VideoFullscreen => "ブックマーク / チャプターの移動パネルを開きます。",
    }
}

fn gamepad_y_label(context: RingShortcutContext) -> &'static str {
    match context {
        RingShortcutContext::Grid => "フォルダツリー",
        RingShortcutContext::ImageFullscreen => "補助操作",
        RingShortcutContext::VideoFullscreen => "タイルモード",
    }
}

fn gamepad_y_tooltip(context: RingShortcutContext) -> &'static str {
    match context {
        RingShortcutContext::Grid => {
            "フォルダツリーを表示 / 非表示にします。Y+方向でツリー操作もできます。"
        }
        RingShortcutContext::ImageFullscreen => {
            "Y+上下で先頭 / 末尾へ移動、Y+左右で見開きの左右位置を調整します。"
        }
        RingShortcutContext::VideoFullscreen => {
            "動画ではタイルモードを切り替えます。Y+左右で前後マーカー移動、Y+上下で通常の上下操作を行います。"
        }
    }
}

fn gamepad_a_label(context: RingShortcutContext) -> &'static str {
    match context {
        RingShortcutContext::Grid => "決定 / 開く",
        RingShortcutContext::ImageFullscreen => "次の画像",
        RingShortcutContext::VideoFullscreen => "再生 / 一時停止",
    }
}

fn gamepad_a_tooltip(context: RingShortcutContext) -> &'static str {
    match context {
        RingShortcutContext::Grid => "選択中の項目を開きます。",
        RingShortcutContext::ImageFullscreen => "次の画像へ移動します。",
        RingShortcutContext::VideoFullscreen => {
            "再生 / 一時停止または決定操作として Enter 相当を送ります。"
        }
    }
}

fn gamepad_b_label(context: RingShortcutContext) -> &'static str {
    match context {
        RingShortcutContext::Grid => "戻る",
        RingShortcutContext::ImageFullscreen => "閉じる",
        RingShortcutContext::VideoFullscreen => "閉じる",
    }
}

fn gamepad_b_tooltip(context: RingShortcutContext) -> &'static str {
    match context {
        RingShortcutContext::Grid => "親フォルダへ戻ります。",
        RingShortcutContext::ImageFullscreen => "フルスクリーンを閉じます。",
        RingShortcutContext::VideoFullscreen => "動画フルスクリーンを閉じます。",
    }
}

fn gamepad_right_stick_tooltip(context: RingShortcutContext) -> &'static str {
    match context {
        RingShortcutContext::Grid => "サムネイル一覧では未使用です。",
        RingShortcutContext::ImageFullscreen => "画像フルスクリーンではズームに使います。",
        RingShortcutContext::VideoFullscreen => "動画フルスクリーンでは未使用です。",
    }
}

fn ring_shortcut_context_editor(
    ui: &mut egui::Ui,
    settings: &mut RingShortcutSettings,
    context: RingShortcutContext,
) {
    ring_shortcut_context_editor_impl(ui, settings, context, true);
}

fn ring_shortcut_context_editor_without_preview(
    ui: &mut egui::Ui,
    settings: &mut RingShortcutSettings,
    context: RingShortcutContext,
) {
    ring_shortcut_context_editor_impl(ui, settings, context, false);
}

fn ring_shortcut_context_editor_impl(
    ui: &mut egui::Ui,
    settings: &mut RingShortcutSettings,
    context: RingShortcutContext,
    show_preview: bool,
) {
    ui.separator();
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(context.label()).strong());
        if ui.button("既定に戻す").clicked() {
            settings.reset_profile(context);
        }
    });

    let available = RingActionId::available_for_context(context);
    ui.horizontal_top(|ui| {
        {
            let profile = settings.profile_mut(context);
            profile.sanitize(context);
            egui::Grid::new(("ring_shortcut_slots", context))
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    for &direction in RingDirection::all() {
                        let idx = direction.slot_index();
                        ui.label(direction.label());
                        let selected = profile.slots[idx].label_for_context(context);
                        egui::ComboBox::from_id_salt(("ring_shortcut_slot", context, direction))
                            .width(190.0)
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                for action in &available {
                                    ui.selectable_value(
                                        &mut profile.slots[idx],
                                        action.clone(),
                                        action.label_for_context(context),
                                    );
                                }
                            });
                        ui.end_row();
                    }
                });
        }

        if show_preview {
            ui.add_space(16.0);
            let preview_profile = settings.profile(context).clone();
            ring_shortcut_preview(ui, &preview_profile, context);
        }
    });
}

fn ring_shortcut_preview(
    ui: &mut egui::Ui,
    profile: &crate::ring_shortcut::RingShortcutProfile,
    context: RingShortcutContext,
) {
    ui.vertical(|ui| {
        ui.label(egui::RichText::new("プレビュー").strong());
        egui::Grid::new(("ring_shortcut_preview", context))
            .num_columns(3)
            .spacing([4.0, 4.0])
            .show(ui, |ui| {
                const CELLS: [Option<RingDirection>; 9] = [
                    Some(RingDirection::UpLeft),
                    Some(RingDirection::Up),
                    Some(RingDirection::UpRight),
                    Some(RingDirection::Left),
                    None,
                    Some(RingDirection::Right),
                    Some(RingDirection::DownLeft),
                    Some(RingDirection::Down),
                    Some(RingDirection::DownRight),
                ];

                for (i, cell) in CELLS.iter().enumerate() {
                    match cell {
                        Some(direction) => {
                            let idx = direction.slot_index();
                            let action = profile
                                .slots
                                .get(idx)
                                .unwrap_or(&RingActionId::None)
                                .label_for_context(context);
                            let text = format!("{}\n{}", direction.label(), action);
                            ui.add_sized(
                                [96.0, 42.0],
                                egui::Label::new(egui::RichText::new(text).size(11.0)).wrap(),
                            );
                        }
                        None => {
                            ui.add_sized(
                                [96.0, 42.0],
                                egui::Label::new(egui::RichText::new("中央\n取消").size(11.0))
                                    .wrap(),
                            );
                        }
                    }
                    if (i + 1) % 3 == 0 {
                        ui.end_row();
                    }
                }
            });
    });
}

pub(super) fn page_book(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label("製本した本（本棚）の保存先フォルダを設定します。");
    ui.add_space(8.0);

    ui.label("本棚の保存先");
    ui.horizontal_wrapped(|ui| {
        let edit_width = (ui.available_width() - 190.0).clamp(180.0, 360.0);
        let mut output = egui::TextEdit::singleline(&mut state.book_root_input)
            .desired_width(edit_width)
            .hint_text(crate::books::default_books_root().display().to_string())
            .show(ui);
        let menu_changed = crate::ui_helpers::singleline_text_edit_context_menu(
            ui,
            &mut output,
            &mut state.book_root_input,
        );
        let response = output.response;
        if response.changed() || menu_changed {
            let trimmed = state.book_root_input.trim();
            s.book_root = if trimmed.is_empty() {
                None
            } else {
                Some(std::path::PathBuf::from(trimmed))
            };
        }
        if ui.button("既定に戻す").clicked() {
            state.book_root_input.clear();
            s.book_root = None;
        }
        if ui.button("フォルダを開く").clicked() {
            let dir = s
                .book_root
                .clone()
                .unwrap_or_else(crate::books::default_books_root);
            crate::capture::open_output_dir_async(dir);
        }
    });

    let effective = s
        .book_root
        .clone()
        .unwrap_or_else(crate::books::default_books_root);
    ui.label(
        egui::RichText::new(format!("本棚: {}", effective.display()))
            .size(11.0)
            .color(egui::Color32::from_gray(140)),
    );
    ui.label(
        egui::RichText::new(
            "保存先を変更しても既存の本は移動しません。元の場所に通常のフォルダとして残ります。",
        )
        .size(11.0)
        .color(egui::Color32::from_gray(140)),
    );
}

pub(super) fn page_parallelism(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;
    let is_auto = s.parallelism == Parallelism::Auto;

    let mut current_auto = is_auto;
    if ui
        .radio(
            current_auto,
            format!(
                "自動（CPUコア数の半分: {} スレッド）",
                state.auto_thread_count
            ),
        )
        .clicked()
    {
        s.parallelism = Parallelism::Auto;
        current_auto = true;
    }

    ui.horizontal(|ui| {
        if ui.radio(!current_auto, "手動").clicked() {
            s.parallelism = Parallelism::Manual(state.manual_threads);
        }
        ui.add_enabled(
            !current_auto,
            egui::DragValue::new(&mut state.manual_threads)
                .range(1..=64)
                .suffix(" スレッド"),
        );
        if !current_auto {
            s.parallelism = Parallelism::Manual(state.manual_threads);
        }
    });
}

pub(super) fn page_prefetch(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label(egui::RichText::new("フルサイズ画像の先読み").strong());
    ui.add_space(4.0);
    ui.label("フルサイズ表示時に前後の画像を先読みする枚数（各最大 50 枚）。");
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("後方（前の画像）:");
        ui.add(
            egui::DragValue::new(&mut s.prefetch_back)
                .range(0..=50usize)
                .suffix(" 枚"),
        );
    });
    ui.horizontal(|ui| {
        ui.label("前方（次の画像）:");
        ui.add(
            egui::DragValue::new(&mut s.prefetch_forward)
                .range(0..=50usize)
                .suffix(" 枚"),
        );
    });

    ui.add_space(12.0);
    ui.label(egui::RichText::new("サムネイルの先読み").strong());
    ui.add_space(4.0);
    ui.label(
        "サムネイルグリッドで現在位置の前後に何ページ分を GPU に保持するか。\n\
         範囲外はメモリから破棄され、スクロールで戻ると再読み込みされます。",
    );
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("後方（前のページ）:");
        ui.add(
            egui::DragValue::new(&mut s.thumb_prev_pages)
                .range(0..=20u32)
                .suffix(" ページ"),
        );
    });
    ui.horizontal(|ui| {
        ui.label("前方（次のページ）:");
        ui.add(
            egui::DragValue::new(&mut s.thumb_next_pages)
                .range(0..=20u32)
                .suffix(" ページ"),
        );
    });

    ui.add_space(12.0);
    ui.label(egui::RichText::new("AI アップスケールの先読み").strong());
    ui.add_space(4.0);
    ui.label("フルスクリーン表示時に AI アップスケール結果を前後の画像に先読みする枚数。");
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("後方（前の画像）:");
        ui.add(
            egui::DragValue::new(&mut s.ai_upscale_prefetch_back)
                .range(0..=10usize)
                .suffix(" 枚"),
        );
    });
    ui.horizontal(|ui| {
        ui.label("前方（次の画像）:");
        ui.add(
            egui::DragValue::new(&mut s.ai_upscale_prefetch_forward)
                .range(0..=10usize)
                .suffix(" 枚"),
        );
    });

    ui.add_space(12.0);
    ui.label(egui::RichText::new("AI アップスケール結果の保持").strong());
    ui.add_space(4.0);
    ui.label(
        "フルスクリーンを閉じた後も、完了済みの AI アップスケール / ノイズ除去結果を\n\
         メモリに残す上限です。どちらかを 0 にすると保持しません。",
    );
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("最大枚数:");
        ui.add(
            egui::DragValue::new(&mut s.retained_final_ai_cache_max_entries)
                .range(
                    settings::RETAINED_FINAL_AI_CACHE_MAX_ENTRIES_MIN
                        ..=settings::RETAINED_FINAL_AI_CACHE_MAX_ENTRIES_MAX,
                )
                .suffix(" 枚"),
        );
    });
    ui.horizontal(|ui| {
        ui.label("最大メモリ:");
        ui.add(
            egui::DragValue::new(&mut s.retained_final_ai_cache_max_mib)
                .range(
                    settings::RETAINED_FINAL_AI_CACHE_MAX_MIB_MIN
                        ..=settings::RETAINED_FINAL_AI_CACHE_MAX_MIB_MAX,
                )
                .suffix(" MB"),
        );
    });

    ui.add_space(12.0);
    ui.label(egui::RichText::new("AI 処理のサイズ上限").strong());
    ui.add_space(4.0);
    ui.label("長辺と短辺がどちらも上限未満の画像のみ AI 処理を実行します (上限以上はスキップ)。");
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("アップスケール:");
        if let Some(new_limit) =
            ai_size_limit_combo(ui, "pref_ai_upscale_size_limit", s.ai_upscale_limit())
        {
            s.ai_upscale_size_limit = Some(new_limit);
        }
    });
    ui.horizontal(|ui| {
        ui.label("ノイズ除去:");
        if let Some(new_limit) =
            ai_size_limit_combo(ui, "pref_ai_denoise_size_limit", s.ai_denoise_limit())
        {
            s.ai_denoise_size_limit = Some(new_limit);
        }
    });
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new(
            "アップスケールは最終結果を長辺 8192px 以下へ直接組み立てますが、上限が大きい\n\
             ほどタイル数・合成バッファ・処理時間が増えます。メモリが不足する環境では\n\
             小さい上限を選んでください。",
        )
        .size(11.0)
        .weak(),
    );
}

/// AI 処理サイズ上限の候補 (長辺, 短辺, 表示ラベル)。
/// 判定は `ai::upscale::should_process_rect` (長辺・短辺とも未満なら処理) を参照。
///
/// ⚠ 長辺は `crate::app::MAX_TEXTURE_DIM` (8192) 以下に保つこと。render-to-target
/// 後も最終合成バッファは GPU テクスチャ上限内に収める。
/// 6144 / 8192 クラスは結合見開きスキャンなど大判向けの高負荷オプション。
const AI_SIZE_LIMIT_OPTIONS: [(u32, u32, &str); 9] = [
    (512, 512, "512 x 512 未満"),
    (1024, 1024, "1024 x 1024 未満"),
    (2048, 1024, "2048 x 1024 未満"),
    (2048, 2048, "2048 x 2048 未満"),
    (4096, 2048, "4096 x 2048 未満"),
    (4096, 4096, "4096 x 4096 未満 (高負荷)"),
    (6144, 4096, "6144 x 4096 未満 (超高負荷)"),
    (8192, 4096, "8192 x 4096 未満 (超高負荷)"),
    (8192, 8192, "8192 x 8192 未満 (超高負荷)"),
];

/// 「長辺 x 短辺 未満」候補のコンボボックスを描画し、選択が変わったら Some を返す。
/// 現在値が候補に無い場合 (旧設定の読み替え値等) は現在値をそのまま表示する。
fn ai_size_limit_combo(
    ui: &mut egui::Ui,
    id: &str,
    current: crate::ai::upscale::AiProcessSizeLimit,
) -> Option<crate::ai::upscale::AiProcessSizeLimit> {
    let selected_label = AI_SIZE_LIMIT_OPTIONS
        .iter()
        .find(|(long, short, _)| *long == current.long_edge_px && *short == current.short_edge_px)
        .map(|(_, _, label)| (*label).to_string())
        .unwrap_or_else(|| format!("{} 未満", current.label()));
    let mut picked = None;
    egui::ComboBox::from_id_salt(id)
        .selected_text(selected_label)
        .width(200.0)
        .show_ui(ui, |ui| {
            for (long, short, label) in AI_SIZE_LIMIT_OPTIONS {
                let is_sel = long == current.long_edge_px && short == current.short_edge_px;
                if ui.selectable_label(is_sel, label).clicked() && !is_sel {
                    picked = Some(crate::ai::upscale::AiProcessSizeLimit {
                        long_edge_px: long,
                        short_edge_px: short,
                    });
                }
            }
        });
    picked
}

pub(super) fn page_gpu_memory(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    let vram_label = match state.vram_mib {
        Some(mib) if mib >= 1024 => format!("{:.1} GiB", mib as f64 / 1024.0),
        Some(mib) => format!("{} MiB", mib),
        None => "取得失敗 (4 GiB 仮定)".to_string(),
    };
    ui.label(format!(
        "サムネイル GPU メモリ上限 (安全ネット):\n\
         超過時は先読み範囲を自動的に縮小します。\n\
         検出した GPU の VRAM: {vram_label}",
    ));

    ui.horizontal(|ui| {
        ui.label("上限:");
        ui.add(
            egui::Slider::new(&mut s.thumb_vram_cap_percent, 0..=100u32)
                .step_by(5.0)
                .suffix(" %"),
        );
    });

    let pct = s.thumb_vram_cap_percent;
    let text = if pct == 0 {
        "  ↑ 0% = 無制限 (推奨しない)".to_string()
    } else {
        let cap_mib = crate::gpu_info::vram_cap_from_percent(pct) / (1024 * 1024);
        format!(
            "  ↑ VRAM の {}% = 約 {} MiB を上限とします (推奨: 50%)",
            pct, cap_mib
        )
    };
    ui.label(text);
}

/// AI 推論バックエンド (DirectML / TensorRT) の選択ページ。
///
/// Phase 3 アーキテクチャ:
/// - メイン: 常に DirectML
/// - TensorRT 有効化: 別プロセスのワーカーが起動して、Upscale/Denoise を担当
/// - 切り替えはホットリロードでアプリ再起動不要
pub(super) fn page_ai_backend(ui: &mut egui::Ui, state: &mut PreferencesState) {
    use crate::ai::AiBackend;
    use crate::gpu_info::GpuVendor;

    ui.label(egui::RichText::new("AI 推論バックエンド").strong());
    ui.add_space(4.0);
    ui.label(
        "AI アップスケール / ノイズ除去 / 消しゴム機能で使う実行環境を選択します。\n\
         TensorRT は NVIDIA GPU 専用ですが、DirectML より大幅に高速 \
         (アップスケール 1.5-2.7x、ノイズ除去 2.6-2.8x)。",
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "※「高速汎用」モデルは TensorRT を選択しても DirectML で動作します \
             (このモデルでは DirectML が最速のため)。",
        )
        .size(12.0)
        .color(egui::Color32::from_rgb(170, 170, 170)),
    );
    ui.add_space(8.0);

    // 検出 GPU の表示
    let vendor_label = match state.gpu_vendor {
        Some(GpuVendor::Nvidia) => "NVIDIA (TensorRT 利用可能)",
        Some(GpuVendor::Amd) => "AMD (DirectML のみ)",
        Some(GpuVendor::Intel) => "Intel (DirectML のみ)",
        Some(GpuVendor::Other(_)) => "その他 (DirectML のみ)",
        None => "GPU 検出失敗 (DirectML のみ)",
    };
    ui.label(format!("検出 GPU: {vendor_label}"));
    ui.add_space(8.0);

    // 起動時にフォールバックが起きた場合のバナー
    if let Some(reason) = &state.current_runtime_fallback_reason {
        ui.colored_label(egui::Color32::from_rgb(220, 160, 50), format!("⚠ {reason}"));
        ui.add_space(8.0);
    }

    // バックエンド選択
    let nvidia = matches!(state.gpu_vendor, Some(GpuVendor::Nvidia));
    let current_choice = state
        .settings
        .ai_backend
        .as_deref()
        .and_then(AiBackend::from_str)
        .unwrap_or_default();

    ui.label("バックエンド:");
    let mut new_choice = current_choice;
    ui.horizontal(|ui| {
        ui.radio_value(
            &mut new_choice,
            AiBackend::DirectMl,
            "DirectML (デフォルト)",
        );
        let trt_resp = ui.add_enabled(
            nvidia,
            egui::RadioButton::new(new_choice == AiBackend::TensorRt, "TensorRT"),
        );
        if trt_resp.clicked() && nvidia {
            new_choice = AiBackend::TensorRt;
        }
        if !nvidia {
            trt_resp.on_hover_text("NVIDIA GPU が検出されていません");
        }
    });
    if new_choice != current_choice {
        state.settings.ai_backend = Some(new_choice.as_str().to_string());
    }
    ui.add_space(8.0);

    // 現在の動作状態 (ホットリロード対応 = 再起動不要)
    let current_label = if state.trt_worker_active {
        "TensorRT (ワーカー稼働中)"
    } else {
        "DirectML"
    };
    // TensorRT を選択しているが pack が未インストールの場合、OK 押下時に
    // worker spawn 失敗 → エラー通知が出る代わりに、UI 上で「実際には
    // DirectML で動作する」ことを明示する (= apply_ai_backend_change 側でも
    // spawn 試行を skip する仕様と整合)。
    let trt_unavailable = new_choice == AiBackend::TensorRt && !state.trt_pack_installed;
    let pending_change = if trt_unavailable {
        false
    } else {
        match new_choice {
            AiBackend::TensorRt => !state.trt_worker_active,
            AiBackend::DirectMl | AiBackend::Cpu => state.trt_worker_active,
        }
    };
    ui.horizontal(|ui| {
        if trt_unavailable {
            ui.label("現在動作中: DirectML");
            ui.colored_label(
                egui::Color32::from_rgb(220, 160, 50),
                "(パックが未インストールのため TensorRT は使用されません)",
            );
        } else {
            ui.label(format!("現在動作中: {current_label}"));
            if pending_change {
                ui.colored_label(
                    egui::Color32::from_rgb(120, 180, 220),
                    "(OK を押すと反映、再起動不要)",
                );
            }
        }
    });
    ui.add_space(8.0);

    // TensorRT 詳細
    if new_choice == AiBackend::TensorRt {
        ui.separator();
        ui.add_space(8.0);
        ui.label(egui::RichText::new("TensorRT 設定").strong());
        ui.add_space(4.0);

        if !state.trt_pack_installed {
            // pack 未インストール
            ui.colored_label(
                egui::Color32::from_rgb(220, 100, 100),
                "❌ TensorRT パックが未インストールです",
            );
            ui.add_space(4.0);
            ui.label(
                "TensorRT 高速化パック (約 1.97 GB) を GitHub からダウンロードします。\n\
                 完了後、次回起動時に TensorRT が自動で有効になります。",
            );
            ui.add_space(8.0);
            if ui
                .button("TensorRT パックをダウンロード (即時実行)")
                .on_hover_text(
                    "押すとすぐダウンロードを開始します。\n\
                     OK / キャンセルとは無関係です。\n\n\
                     GitHub Releases から CUDA / cuDNN / TensorRT runtime と \
                     事前ビルド済み AI エンジンをダウンロードします (約 1.97 GB)。\n\
                     対応 GPU: RTX 30 / 40 / 50 シリーズ。",
                )
                .clicked()
            {
                state.start_trt_install_requested = true;
            }
        } else {
            // pack インストール済み
            ui.colored_label(
                egui::Color32::from_rgb(100, 200, 100),
                format!("✓ パック展開済み ({} MiB)", state.trt_pack_size_mib),
            );
            ui.add_space(8.0);

            // パック削除 / 再 DL 管理
            // 削除ボタンは pack 全体 (DLL + エンジンキャッシュ + INSTALL_OK) を消し、
            // 再起動なしでこのダイアログ上で「ダウンロード」フローへ戻る。
            // 削除/DL は OK/キャンセルとは独立な即時実行 (ファイル操作のため)。
            ui.label(format!(
                "エンジンキャッシュ: {} MiB",
                state.trt_engine_cache_size_mib
            ));
            ui.label(
                egui::RichText::new(
                    "(ドライバ更新後等にエラーが出る場合や再 DL を試したい場合は\n\
                     パックを削除すると、次のステップでダウンロードに戻れます)",
                )
                .small(),
            );
            ui.add_space(4.0);
            if ui
                .button("TensorRT パックを削除 (即時実行)")
                .on_hover_text(
                    "押すとすぐ実行されます。\n\
                     OK / キャンセルとは無関係です。",
                )
                .clicked()
            {
                state.trt_cache_delete_confirm_open = true;
            }
            // 確認ダイアログ
            if state.trt_cache_delete_confirm_open {
                let mut do_delete = false;
                let mut do_cancel = false;
                let mut window_open = state.trt_cache_delete_confirm_open;
                let pack_total_mib = state.trt_pack_size_mib + state.trt_engine_cache_size_mib;
                egui::Window::new("TensorRT パック削除の確認")
                    .collapsible(false)
                    .resizable(false)
                    .open(&mut window_open)
                    .show(ui.ctx(), |ui| {
                        ui.label(format!(
                            "TensorRT パック ({} MiB) を削除します。",
                            pack_total_mib
                        ));
                        ui.label(
                            "削除後は同じダイアログで「TensorRT パックをダウンロード」\n\
                             ボタンが表示されます。再 DL に 5〜15 分かかります。",
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("削除する").clicked() {
                                do_delete = true;
                            }
                            if ui.button("キャンセル").clicked() {
                                do_cancel = true;
                            }
                        });
                    });
                // 確認ダイアログの × でも閉じれるよう、open フラグを反映する。
                // (`.open(&mut state.x.clone())` は clone 経由で書き戻しが届かないバグ)
                state.trt_cache_delete_confirm_open = window_open;
                if do_delete {
                    // 実際の削除は App 側に委譲。理由:
                    // - TRT worker pool が DLL を握ったままだと remove_dir_all が失敗する
                    //   ため、先に worker pool を detach する必要がある (= AiRuntime API)
                    // - ai_backend = TensorRT のままだと AI 機能呼び出し時に再 attach で
                    //   失敗 (= 削除直後の DLL 不在) → エラーダイアログが出る
                    // → App 側で「detach → 削除 → ai_backend を DirectML に切替 → save」を
                    //   一括処理する
                    state.uninstall_trt_pack_requested = true;
                    // dialog 上の表示も即時切替 (= 「ダウンロード」分岐へ)
                    state.trt_pack_installed = false;
                    state.trt_pack_size_mib = 0;
                    state.trt_engine_cache_size_mib = 0;
                    // 「現在動作中」表記も DirectML に同期 (= App 側で detach するので
                    // 実際の worker pool は止まる。state は dialog 開いた時のスナップ
                    // ショットなので、ここで明示的に false にしないと表記が古いまま)。
                    state.trt_worker_active = false;
                    state.current_runtime_fallback_reason = None;
                    // 設定 UI 上のラジオボタンも DirectML に同期
                    // (= state.settings は draft、live settings は App 側で切替)
                    state.settings.ai_backend =
                        Some(crate::ai::AiBackend::DirectMl.as_str().to_string());
                    state.trt_cache_delete_confirm_open = false;
                } else if do_cancel {
                    state.trt_cache_delete_confirm_open = false;
                }
            }
        }
    }
}

/// 編集用追加パック (オノマトペ向けフォント + 被写体分離モデル) の管理ページ。
///
/// 表示は `from_settings` で取ったスナップショット (`state.editing_addon_*`) を使う。
/// 実際の DL / 削除 / フォルダオープンはボタンでリクエストフラグを立て、
/// `show_preferences_dialog` の末尾 (closure 抜けた後) で App 側が処理する
/// (TensorRT パック管理と同じパターン)。
pub(super) fn page_editing_addon(ui: &mut egui::Ui, state: &mut PreferencesState) {
    use crate::editing_addon::AddonStatus;

    ui.label(egui::RichText::new("編集用追加ファイル").strong());
    ui.add_space(4.0);
    ui.label(
        "吹き出し・テキスト・オノマトペ機能と、補正レイヤーの被写体分離 (人物などの\n\
         切り抜き) で使う、追加フォントと AI モデルをまとめて管理します。\n\
         基本的なテキスト編集はこのパックが無くてもシステムフォントで利用できます。",
    );
    ui.add_space(8.0);

    match state.editing_addon_status.clone() {
        AddonStatus::Valid { version } => {
            ui.colored_label(
                egui::Color32::from_rgb(100, 200, 100),
                format!("✓ 導入済み (バージョン {version})"),
            );
            ui.add_space(8.0);
            ui.label(format!(
                "ディスク使用量: 約 {} MiB",
                state.editing_addon_size_mib
            ));
            ui.label(format!(
                "含まれるフォント: {} 書体",
                state.editing_addon_font_count
            ));
            if !state.editing_addon_subject_model.is_empty() {
                ui.label(format!(
                    "被写体分離モデル: {}",
                    state.editing_addon_subject_model
                ));
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if ui
                    .button("更新を確認・再ダウンロード")
                    .on_hover_text(
                        "配布一覧から最新の編集用追加パックを取得して導入し直します。\n\
                         OK / キャンセルとは無関係に、すぐにダウンロードフローへ進みます。",
                    )
                    .clicked()
                {
                    state.start_editing_addon_install_requested = true;
                }
                if ui
                    .button("インストール先を開く")
                    .on_hover_text("追加ファイルの保存先フォルダを Explorer で開きます。")
                    .clicked()
                {
                    state.open_editing_addon_folder_requested = true;
                }
            });
            ui.add_space(8.0);
            if ui
                .button("削除")
                .on_hover_text(
                    "追加フォントと AI モデルを削除します。\n\
                     ユーザー追加フォントや保存済み編集データは消えません。",
                )
                .clicked()
            {
                state.editing_addon_delete_confirm_open = true;
            }

            // 削除確認ダイアログ
            if state.editing_addon_delete_confirm_open {
                let mut do_delete = false;
                let mut do_cancel = false;
                let mut window_open = state.editing_addon_delete_confirm_open;
                egui::Window::new("編集用追加ファイル削除の確認")
                    .collapsible(false)
                    .resizable(false)
                    .open(&mut window_open)
                    .show(ui.ctx(), |ui| {
                        ui.label(format!(
                            "編集用追加ファイル (約 {} MiB) を削除します。",
                            state.editing_addon_size_mib
                        ));
                        ui.label(
                            "削除してもユーザー追加フォントや保存済みの編集データは\n\
                             消えません。再び必要になったらこのページから\n\
                             再ダウンロードできます。",
                        );
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.button("削除する").clicked() {
                                do_delete = true;
                            }
                            if ui.button("キャンセル").clicked() {
                                do_cancel = true;
                            }
                        });
                    });
                // × ボタンでも閉じれるよう open フラグを書き戻す。
                state.editing_addon_delete_confirm_open = window_open;
                if do_delete {
                    state.uninstall_editing_addon_requested = true;
                    // 表示も即「未導入」へ同期 (= App 側で実際に削除される)。
                    state.editing_addon_status = AddonStatus::Missing;
                    state.editing_addon_size_mib = 0;
                    state.editing_addon_font_count = 0;
                    state.editing_addon_subject_model.clear();
                    state.editing_addon_delete_confirm_open = false;
                } else if do_cancel {
                    state.editing_addon_delete_confirm_open = false;
                }
            }
        }
        AddonStatus::Missing => {
            ui.colored_label(egui::Color32::from_rgb(200, 160, 80), "未導入です");
            ui.add_space(6.0);
            ui.label(
                "初めて編集機能へ入ったときにも確認ダイアログが出ますが、ここから\n\
                 先にダウンロードしておくこともできます (約 550 MB)。",
            );
            ui.add_space(10.0);
            if ui
                .button("ダウンロード")
                .on_hover_text(
                    "オノマトペ向けフォントと被写体分離モデルをダウンロードします。\n\
                     OK / キャンセルとは無関係に、すぐにダウンロードフローへ進みます。",
                )
                .clicked()
            {
                state.start_editing_addon_install_requested = true;
            }
            ui.add_space(8.0);
            if ui
                .button("インストール先を開く")
                .on_hover_text("追加ファイルの保存先フォルダを Explorer で開きます。")
                .clicked()
            {
                state.open_editing_addon_folder_requested = true;
            }
        }
        AddonStatus::Corrupt(msg) => {
            ui.colored_label(
                egui::Color32::from_rgb(220, 100, 100),
                "⚠ 導入データが壊れています",
            );
            ui.add_space(4.0);
            ui.label(msg);
            ui.add_space(10.0);
            ui.label("再ダウンロードすると修復できます。");
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("再ダウンロード").clicked() {
                    state.start_editing_addon_install_requested = true;
                }
                if ui.button("インストール先を開く").clicked() {
                    state.open_editing_addon_folder_requested = true;
                }
            });
        }
    }
}

pub(super) fn page_cache(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label(
        "サムネイルキャッシュをいつ生成するかを指定します。\n\
         Off にしても既存のキャッシュは引き続き読み込まれます。",
    );
    ui.add_space(8.0);

    ui.label(egui::RichText::new("モード").strong());
    ui.add_space(4.0);
    for policy in [CachePolicy::Off, CachePolicy::Auto, CachePolicy::Always] {
        if ui.radio(s.cache_policy == policy, policy.label()).clicked() {
            s.cache_policy = policy;
        }
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    let auto_active = s.cache_policy == CachePolicy::Auto;

    ui.add_enabled_ui(auto_active, |ui| {
        ui.label(egui::RichText::new("Auto モードのしきい値").strong());
        ui.add_space(4.0);

        ui.label("時間しきい値 (decode + display の合計がこれ以上ならキャッシュ):");
        ui.add(
            egui::Slider::new(&mut s.cache_threshold_ms, 10..=100)
                .step_by(5.0)
                .suffix(" ms"),
        );
        ui.label("  小さいほど多くキャッシュ。25 ms 推奨。");

        ui.add_space(8.0);

        ui.label("サイズしきい値 (このサイズ以上は無条件キャッシュ):");
        let mut size_mb = (s.cache_size_threshold_bytes as f64) / 1_000_000.0;
        if ui
            .add(
                egui::Slider::new(&mut size_mb, 0.5..=10.0)
                    .step_by(0.5)
                    .suffix(" MB"),
            )
            .changed()
        {
            s.cache_size_threshold_bytes = (size_mb * 1_000_000.0) as u64;
        }
        ui.label("  2 MB 推奨。これ以上の重い画像が確実にキャッシュされます。");

        ui.add_space(8.0);

        ui.checkbox(
            &mut s.cache_webp_always,
            "既存 .webp は常にキャッシュ (処理が重いため推奨)",
        );
        ui.checkbox(
            &mut s.cache_pdf_always,
            "PDF ページは常にキャッシュ (処理が重いため推奨)",
        );
        ui.checkbox(
            &mut s.cache_zip_always,
            "ZIP 内画像は常にキャッシュ (処理が重いため推奨)",
        );
    });

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(6.0);

    ui.label(egui::RichText::new("変換済みアーカイブキャッシュ").strong());
    ui.add_space(4.0);
    ui.label(
        "RAR / 7z / LZH から作成した ZIP キャッシュの容量上限です。\n\
         上限を超えた場合、次回の変換完了後に最終アクセスが古いものから削除します。",
    );
    ui.add_space(6.0);
    ui.label(egui::RichText::new("RAR / 7z / LZH の処理").strong());
    let mut archive_handling = s.archive_file_handling_resolved();
    for &handling in ArchiveFileHandling::all_user_visible() {
        if ui
            .radio_value(&mut archive_handling, handling, handling.label())
            .on_hover_text(handling.description())
            .changed()
        {
            s.set_archive_file_handling(archive_handling);
        }
    }
    ui.label(
        egui::RichText::new(
            "「無視する」では、一覧・フォルダ移動・ZIP 内の入れ子変換提案で RAR / 7z / LZH を扱いません。",
        )
        .small()
        .weak(),
    );
    ui.add_space(8.0);
    let mut archive_limit_enabled = s.archive_cache_max_bytes > 0;
    const ARCHIVE_CACHE_LIMIT_BYTES_PER_MB: u64 = 1_000_000;
    const DEFAULT_ARCHIVE_CACHE_LIMIT_MB: u64 = 20_000;
    if ui
        .checkbox(&mut archive_limit_enabled, "容量上限を有効にする")
        .changed()
    {
        s.archive_cache_max_bytes = if archive_limit_enabled {
            DEFAULT_ARCHIVE_CACHE_LIMIT_MB * ARCHIVE_CACHE_LIMIT_BYTES_PER_MB
        } else {
            0
        };
    }
    if archive_limit_enabled {
        let mut limit_mb = (s
            .archive_cache_max_bytes
            .saturating_add(ARCHIVE_CACHE_LIMIT_BYTES_PER_MB - 1)
            / ARCHIVE_CACHE_LIMIT_BYTES_PER_MB)
            .max(1);
        if ui
            .horizontal(|ui| {
                ui.label("上限:");
                ui.add(
                    egui::DragValue::new(&mut limit_mb)
                        .range(1..=1_000_000u64)
                        .speed(100.0)
                        .suffix(" MB"),
                )
            })
            .inner
            .changed()
        {
            s.archive_cache_max_bytes = limit_mb.saturating_mul(ARCHIVE_CACHE_LIMIT_BYTES_PER_MB);
        }
        ui.label(format!(
            "現在の上限: {} MB",
            crate::ui_helpers::format_count(limit_mb)
        ));
    } else {
        ui.label("現在の上限: 無制限");
    }
}

/// v0.8.0: 自動インデクサの速度プロファイル設定ページ。
///
/// `IndexerSpeedProfile` は `GlobalIoSemaphore` の permit 数を決める。
/// 値の変更は **次回起動時に反映** される (ランタイム差し替えは `sync_with_favorites`
/// でも反映されないので現状は再起動が必要)。
pub(super) fn page_indexer_speed(ui: &mut egui::Ui, state: &mut PreferencesState) {
    use crate::settings::IndexerSpeedProfile;
    let s = &mut state.settings;

    ui.label(
        "Ctrl+G グローバルメタ検索用のバックグラウンドインデクサの速度を設定します。\n\
         I/O 同時実行数 (permit 数) を切り替えて、UI 応答性とインデックス速度のバランスを調整します。",
    );
    ui.add_space(8.0);
    ui.label(egui::RichText::new("速度プロファイル").strong());
    ui.add_space(4.0);

    for profile in IndexerSpeedProfile::all() {
        let selected = s.indexer_speed_profile == *profile;
        if ui.radio(selected, profile.label()).clicked() {
            s.indexer_speed_profile = *profile;
        }
    }

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(
            "※ 速度プロファイルの変更は次回起動時に反映されます。\n\
             ※ PDF ワーカー / サムネイル読み込みとも I/O を共有するため、\n  \
             High にするとインデックス中に通常操作がもたつく可能性があります。\n\
             ※ 未使用で放置してもアクティブなインデクサが無ければ I/O は消費しません。\n\
             ※ 索引化の対象はお気に入りごとに選べます (「お気に入り > 編集」ダイアログ)。",
        )
        .weak()
        .size(11.0),
    );
}

/// v0.9: タスクトレイ常駐設定ページ。
///
/// お気に入りダイアログにも同じ項目があるが、環境設定側は「全体設定を探したとき
/// にここでも見つかる」ことを意図した冗長配置。両者は `settings` の同じフィールドを
/// 参照するので片方を変更すればもう片方も同期する。
pub(super) fn page_tray_residency(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label("閉じるボタンを押したときのアプリ終了挙動と、常駐中のインデックス処理を制御します。");
    ui.add_space(10.0);

    ui.label(egui::RichText::new("閉じるボタンの挙動").strong());
    ui.add_space(4.0);
    ui.checkbox(
        &mut s.minimize_to_tray_on_close,
        "アプリを閉じる代わりに、タスクトレイに常駐する",
    )
    .on_hover_text(
        "OFF (既定): [×] でプロセス終了。次回起動時にインデックスが再スキャンされます。\n\
         ON: [×] でウィンドウを隠してタスクトレイに常駐。notify-rs でファイル変更を\n\
         追い続けるため、次回開いたときは最新のインデックスがそのまま使えます。\n\
         終了はタスクトレイアイコンを右クリックして「終了」を選んでください。",
    );
    ui.add_space(12.0);

    ui.add_enabled_ui(s.minimize_to_tray_on_close, |ui| {
        ui.label(egui::RichText::new("常駐中のインデックス更新").strong());
        ui.add_space(4.0);
        ui.checkbox(
            &mut s.pause_indexer_while_minimized,
            "常駐中はインデックス更新を一時停止する",
        )
        .on_hover_text(
            "OFF (既定): 常駐中もファイル監視と初回スキャンを続けます。\n\
             ON: 常駐中は全て止め、ウィンドウを開いた瞬間に再開します (溜まっていた\n\
             ファイル変更もそこで順次処理)。\n\n\
             OFF でも常駐中は I/O 並列度が自動で 1 に絞られるので、ゲームや動画再生など\n\
             他アプリの I/O 負荷が気になる場合でも普通は OFF のままで問題ありません。",
        );
    });

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(
            "※ タスクトレイ常駐はトレイアイコンからの「開く / 常駐時のスキャンを一時停止 / 終了」\n  \
             メニューでも操作できます。\n\
             ※ トレイアイコン左クリックでウィンドウが復帰します。",
        )
        .weak()
        .size(11.0),
    );
}

/// レーティング ★ の XMP 書き込み設定ページ。opt-in。
/// ファイル書き換えを伴う機能なので、タグ設定と同じく UI で明示的に ON にしないと動かない。
pub(super) fn page_rating(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label(
        "F1〜F5 で付与する★をファイル内の XMP `xmp:Rating` にも書き込むか設定します。\n\
         ファイル移動後もレーティングを保持したい場合に有効にしてください。",
    );
    ui.add_space(10.0);

    ui.checkbox(
        &mut s.write_rating_to_xmp,
        "レーティングを XMP にも書き込む",
    )
    .on_hover_text(
        "OFF (既定): レーティングはアプリ内データベースだけに保存。ファイルは非破壊。\n\
         ファイルを別フォルダに移動すると★は失われます (アプリはパスで管理するため)。\n\
         ON: ★ を付けるたびにファイル内の XMP `xmp:Rating` も更新します。\n\
         Lightroom / Windows エクスプローラー「評価」列など、XMP 対応ソフトで同じ★が見えます。",
    );

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(6.0);

    ui.label(egui::RichText::new("有効化した場合の注意").strong());
    ui.add_space(4.0);
    ui.label(
        "・対応形式は JPEG / PNG / WebP のみです。\n  \
         それ以外 (RAW / HEIC / AVIF / TIFF / ZIP 内画像 / PDF ページ等) は従来通り\n  \
         アプリ内データベースだけに保存されるため、別フォルダへ移動すると★は失われます。\n\
         ・★ を付け外しするたびにファイル本体が書き換わり、更新日時が新しくなります。\n\
         ・フォルダやコンテナ (ZIP / PDF) 本体の Shift+F1〜F6 による★は、\n  \
         書き込み先が無いため常にアプリ内データベースのみです (この設定と無関係)。",
    );

    ui.add_space(10.0);
    ui.label(
        egui::RichText::new(
            "※ 書き込みに失敗した場合 (読み取り専用・排他ロック等) は画面右下にトーストで通知します。\n\
             その場合も★自体はアプリ内データベースには反映済みです。",
        )
        .weak()
        .size(11.0),
    );
}

/// バージョン更新確認の設定ページ。
/// ON で起動時 + 24 時間ごとに GitHub Releases API を叩いて新バージョンを確認する。
pub(super) fn page_update_check(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label(
        "起動時と 24 時間ごとに新しいバージョンが公開されていないか確認します。\n\
         新バージョンを検出するとメニューバーに通知バッジが表示されます。",
    );
    ui.add_space(10.0);

    ui.checkbox(
        &mut s.update_check_enabled,
        "新バージョンを自動的に確認する",
    )
    .on_hover_text(
        "ON (既定): 起動時と 24 時間ごとに GitHub のリリースページに問い合わせ。\n\
             OFF: 自動確認を行いません。ヘルプメニューの「更新を確認…」で手動確認は可能。",
    );

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new(
            "通信先: api.github.com (HTTPS、未認証)。\n\
             失敗 (オフライン等) は通知せず黙って終了します。\n\
             問い合わせ内容はバージョン情報のみで、ユーザーデータは送信しません。",
        )
        .size(11.0)
        .color(egui::Color32::from_gray(150)),
    );

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(6.0);

    if let Some(ref skipped) = s.update_check_dismissed_version.clone() {
        ui.label(format!("現在「{skipped}」の通知は非表示にしています。"));
        if ui.button("通知を再度有効にする").clicked() {
            s.update_check_dismissed_version = None;
        }
        ui.add_space(8.0);
    }

    ui.label(
        egui::RichText::new(format!("現在のバージョン: v{}", env!("CARGO_PKG_VERSION"))).size(12.0),
    );
    ui.add_space(4.0);
    if ui.button("リリース履歴を開く").clicked() {
        crate::ui_helpers::open_url(crate::update_check::releases_page_url());
    }
}

pub(super) fn page_video(ui: &mut egui::Ui, state: &mut PreferencesState) {
    {
        let s = &mut state.settings;

        ui.label(egui::RichText::new("ハードウェアデコード").strong());
        ui.add_space(4.0);
        ui.label(
            "GPU の動画デコード機能 (Direct3D 11) を使って HEVC / 4K 動画の CPU 負荷を下げます。\n\
         D3D11VA 非対応のコーデックは CPU デコードで再生し、対応コーデックの初期化失敗はエラーとして表示します。",
        );
        ui.add_space(6.0);
        ui.checkbox(&mut s.video_hw_decode, "ハードウェアデコードを有効にする")
            .on_hover_text(
                "ON (既定): 対応コーデックは GPU でデコード。D3D11VA 非対応コーデックは CPU でデコード。\n\
         OFF: 常に CPU でデコード。\n\
         切り替え後は次に開く動画から反映されます。",
            );

        ui.add_space(12.0);
        ui.label(egui::RichText::new("デインターレース").strong());
        ui.add_space(4.0);
        ui.label(
            "インターレース動画の横縞ノイズを、FFmpeg の bwdif フィルタで表示前に補正します。",
        );
        ui.add_space(6.0);
        egui::ComboBox::from_label("デインターレース")
            .selected_text(s.video_deinterlace.label())
            .show_ui(ui, |ui| {
                for &mode in crate::settings::VideoDeinterlaceMode::all() {
                    ui.selectable_value(&mut s.video_deinterlace, mode, mode.label());
                }
            });
        ui.label(
            egui::RichText::new("自動: インターレースとしてデコードされたフレームだけ補正。切り替え後は次に開く動画から反映されます。")
                .small(),
        );

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(egui::RichText::new("再生").strong());
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("ループ再生:");
            let mut current = s.video_loop_mode;
            egui::ComboBox::from_id_salt("video_loop_mode")
                .selected_text(current.label())
                .show_ui(ui, |ui| {
                    for mode in crate::settings::VideoLoopMode::all() {
                        ui.selectable_value(&mut current, *mode, mode.label());
                    }
                });
            if current != s.video_loop_mode {
                s.video_loop_mode = current;
                // 旧 bool 設定も新モードと矛盾しないよう同期 (古いコード誤読対策)。
                s.video_loop = !matches!(current, crate::settings::VideoLoopMode::Off);
            }
        });
        ui.checkbox(&mut s.video_start_muted, "起動直後はミュートで開始");

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            let mut vol_pos = crate::settings::video_volume_linear_to_fader_pos(s.video_volume);
            let response = ui.add(
                egui::Slider::new(&mut vol_pos, 0.0..=1.0)
                    .text("既定音量")
                    .show_value(false)
                    .clamping(egui::SliderClamping::Always),
            );
            if response.changed() {
                s.video_volume = crate::settings::video_volume_fader_pos_to_linear(vol_pos);
            }
            ui.label(crate::settings::format_video_volume_db(s.video_volume));
        });

        // 再生位置レジューム (続き/先頭の切替・位置クリア) は「履歴と復元」ページに集約。
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);
    }

    draw_audio_normalize_cache_controls(ui, state);

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);

    {
        let s = &mut state.settings;
        ui.label(egui::RichText::new("グリッドサムネイル").strong());
        ui.add_space(4.0);
        ui.checkbox(
            &mut s.video_thumb_use_sidecar_image,
            "同名ファイル名の画像があれば動画サムネに優先採用",
        )
        .on_hover_text(
            "例: movie.mp4 の隣に movie.jpg があれば、それをサムネに使う。\n\
         OFF にすると Windows 標準のサムネのみ採用 (= 既定動作)。\n\
         ピン留めしたフレーム (今後実装予定) は本設定に関わらず常に最優先。",
        );
    }

    // VST3 プラグイン処理は専用ページ "VST3 プラグイン" に分離した (= ユーザー要望
    // 「環境設定の中に新しい項目」)。動画タブには出さない。
}

fn draw_audio_normalize_cache_controls(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.add_space(12.0);
    ui.label(egui::RichText::new("音量ノーマライズ測定値").strong());
    ui.add_space(4.0);

    if state.audio_normalize_db_available {
        ui.label(format!(
            "現在 {} 件の動画について音量ノーマライズ測定値を保存しています。",
            state.audio_normalize_entry_count
        ));
        ui.label(
            egui::RichText::new("削除しても、現在再生中に適用済みの音量はその場では変更しません。")
                .small()
                .weak(),
        );
        ui.add_space(4.0);
        if ui
            .add_enabled(
                state.audio_normalize_entry_count > 0,
                egui::Button::new("すべての音量ノーマライズ測定値をクリア"),
            )
            .clicked()
        {
            state.audio_normalize_clear_result = None;
            state.audio_normalize_clear_confirm_open = true;
        }
    } else {
        ui.label(
            egui::RichText::new("音量ノーマライズ測定値 DB を開けませんでした。")
                .small()
                .weak(),
        );
    }

    if let Some(msg) = state.audio_normalize_clear_result.as_deref() {
        ui.add_space(4.0);
        ui.label(egui::RichText::new(msg).small());
    }

    if state.audio_normalize_clear_confirm_open {
        let mut open = true;
        let entry_count = state.audio_normalize_entry_count;
        egui::Window::new("音量ノーマライズ測定値のクリア")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ui.ctx(), |ui| {
                ui.label(format!(
                    "保存済みの音量ノーマライズ測定値 ({entry_count} 件) をすべて削除しますか？"
                ));
                ui.label("次回以降、必要な動画は再スキャンされます。");
                ui.label("この操作は元に戻せません。");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("クリア").clicked() {
                        state.audio_normalize_clear_requested = true;
                        state.audio_normalize_clear_confirm_open = false;
                    }
                    if ui.button("キャンセル").clicked() {
                        state.audio_normalize_clear_confirm_open = false;
                    }
                });
            });
        if !open {
            state.audio_normalize_clear_confirm_open = false;
        }
    }
}

/// VST3 プラグイン専用ページ (= 環境設定 ツリーの "VST3 プラグイン" カテゴリ)。
///
/// - 有効化チェックボックス
/// - 推奨プラグイン候補のスキャン + 検索
/// - チェーン編集 (上限 10、↑↓ 並べ替え、× 削除)
/// - 候補一覧 (= クリックで末尾追加)
///
/// 動画再生中はホバーバー VST ボタンの **プレイバックパネル**で ON/OFF + GUI
/// 表示の運用切替のみ行う設計。
#[cfg(windows)]
pub(super) fn page_vst3(ui: &mut egui::Ui, state: &mut PreferencesState) {
    use crate::settings::Vst3PluginEntry;

    const MAX_CHAIN_LEN: usize = 10;

    let mut scan_finished: Option<Result<Vec<crate::video::dsp::DiscoveredPlugin>, String>> = None;
    let mut scan_disconnected = false;
    if let Some(rx) = state.vst3_scan_rx.as_ref() {
        loop {
            match rx.try_recv() {
                Ok(Vst3ScanMessage::Progress { done, total, path }) => {
                    state.vst3_scan_done = done;
                    state.vst3_scan_total = total;
                    state.vst3_scan_current = path;
                    ui.ctx().request_repaint();
                }
                Ok(Vst3ScanMessage::Finished(result)) => {
                    scan_finished = Some(result);
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_millis(100));
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    scan_disconnected = true;
                    break;
                }
            }
        }
    }
    if let Some(result) = scan_finished {
        match result {
            Ok(found) => {
                state.vst3_scan_done = state.vst3_scan_total;
                state.vst3_discovered = found;
                state.vst3_scan_error = None;
            }
            Err(err) => {
                state.vst3_scan_error = Some(err);
            }
        }
        state.vst3_scan_rx = None;
        state.vst3_scan_in_progress = false;
        state.vst3_scan_current.clear();
    } else if scan_disconnected {
        state.vst3_scan_rx = None;
        state.vst3_scan_in_progress = false;
        state.vst3_scan_current.clear();
        state.vst3_scan_error = Some("VST3 scan worker が終了しました".to_string());
    }

    ui.label(
        "動画音声を VST3 プラグインで加工してから再生します。\n\
         LUFS 測定 (Youlean LM2 等) や EQ (FabFilter Pro-Q 等) で動画の音声を\n\
         リアルタイムに分析・加工できます。",
    );
    ui.add_space(8.0);

    // VST3 bridge host が手に入らない版 (= host exe を同梱しないポータブルビルド) では
    // 機能を利用できない。トグルを無効化し、理由をホバーで示す (= ユーザー要望の挙動)。
    if !crate::video::dsp::vst3_supported() {
        state.settings.vst3_enabled = false;
        let mut off = false;
        ui.add_enabled(
            false,
            egui::Checkbox::new(&mut off, "VST3 プラグイン処理を有効にする"),
        )
        .on_hover_text("ポータブル版では利用できません");
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(
                "この版では VST3 プラグイン機能を利用できません。\
                 インストーラ版または単体 exe 版をご利用ください。",
            )
            .weak()
            .small(),
        );
        return;
    }

    ui.checkbox(
        &mut state.settings.vst3_enabled,
        "VST3 プラグイン処理を有効にする",
    )
    .on_hover_text(
        "ON: 動画再生時に下のチェーンのプラグインを順番に通します。\n\
         OFF (既定): プラグイン処理なし (= 通常再生)。",
    );

    if !state.settings.vst3_enabled {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("(チェーンの編集は VST3 プラグイン処理を ON にすると操作できます)")
                .weak()
                .small(),
        );
        return;
    }

    // ── 自動 OFF された plugin の警告 (= MAX_PDC_LATENCY_SECS=2s 超で auto-bypass) ──
    // 警告は VST3 enabled 時のみ表示。再生中に latency 変化で発火するので、
    // ここに常設しておけばユーザーは設定画面を開いた時に気付ける。
    if !state.vst3_auto_bypassed.is_empty() {
        ui.add_space(8.0);
        let warn_color = egui::Color32::from_rgb(220, 80, 80);
        for (name, ms) in &state.vst3_auto_bypassed {
            ui.label(
                egui::RichText::new(format!(
                    "[!] 「{}」は遅延 {:.1}ms (上限 2000ms 超) のため自動 OFF にしました。\n     プラグイン側で遅延を減らしてから手動で再 ON してください。",
                    name, ms
                ))
                .color(warn_color)
                .strong()
                .small(),
            );
        }
    }

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(6.0);

    // ── チェーン編集 ──
    let chain_len = state.settings.vst3_plugins.len();
    ui.label(
        egui::RichText::new(format!(
            "プラグインチェーン ({chain_len}/{MAX_CHAIN_LEN} 個)"
        ))
        .strong(),
    );
    ui.label(
        egui::RichText::new(
            "上から順に音声を通します。動画再生中はホバーバー VST ボタンのパネル\n\
             から ON/OFF (バイパス) を切り替えできます。",
        )
        .small()
        .weak(),
    );
    ui.add_space(4.0);

    let mut clicked_remove: Option<usize> = None;
    let mut clicked_move_up: Option<usize> = None;
    let mut clicked_move_down: Option<usize> = None;

    if state.settings.vst3_plugins.is_empty() {
        ui.label(egui::RichText::new("(空)").weak());
    } else {
        let total = state.settings.vst3_plugins.len();
        for (idx, entry) in state.settings.vst3_plugins.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{}.", idx + 1)).weak());
                let name = std::path::Path::new(&entry.path)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("(不明)");
                ui.label(egui::RichText::new(name).strong())
                    .on_hover_text(entry.path.as_str());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .small_button("×")
                        .on_hover_text("チェーンから削除")
                        .clicked()
                    {
                        clicked_remove = Some(idx);
                    }
                    let down_enabled = idx + 1 < total;
                    if ui
                        .add_enabled(down_enabled, egui::Button::new("↓").small())
                        .on_hover_text("下へ")
                        .clicked()
                    {
                        clicked_move_down = Some(idx);
                    }
                    let up_enabled = idx > 0;
                    if ui
                        .add_enabled(up_enabled, egui::Button::new("↑").small())
                        .on_hover_text("上へ")
                        .clicked()
                    {
                        clicked_move_up = Some(idx);
                    }
                });
            });
        }
    }
    if let Some(idx) = clicked_remove {
        state.settings.vst3_plugins.remove(idx);
    }
    if let Some(idx) = clicked_move_up {
        if idx > 0 {
            state.settings.vst3_plugins.swap(idx, idx - 1);
        }
    }
    if let Some(idx) = clicked_move_down {
        if idx + 1 < state.settings.vst3_plugins.len() {
            state.settings.vst3_plugins.swap(idx, idx + 1);
        }
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(6.0);

    // ── プラグイン追加 ──
    ui.horizontal(|ui| {
        let scan_label = if state.vst3_scan_in_progress {
            "スキャン中...".to_string()
        } else if state.vst3_discovered.is_empty() {
            "プラグインをスキャン".to_string()
        } else {
            "再スキャン".to_string()
        };
        if ui
            .add_enabled(!state.vst3_scan_in_progress, egui::Button::new(scan_label))
            .on_hover_text(
                "%COMMONPROGRAMFILES%\\VST3\\ 等を再帰走査し、bridge subprocess で audio input/output を確認します",
            )
            .clicked()
        {
            let (tx, rx) = std::sync::mpsc::channel();
            state.vst3_scan_rx = Some(rx);
            state.vst3_scan_in_progress = true;
            state.vst3_scan_error = None;
            state.vst3_scan_done = 0;
            state.vst3_scan_total = 0;
            state.vst3_scan_current = "候補を列挙中...".to_string();
            if let Err(e) = std::thread::Builder::new()
                .name("vst3-scan-probe".into())
                .spawn(move || {
                    let _ = tx.send(Vst3ScanMessage::Progress {
                        done: 0,
                        total: 0,
                        path: "候補を列挙中...".to_string(),
                    });
                    let roots = crate::video::dsp::default_vst3_paths();
                    let progress_tx = tx.clone();
                    let result =
                        crate::video::dsp::scan_with_audio_probe_progress(&roots, |done, total, path| {
                            let label = path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("")
                                .to_string();
                            let _ = progress_tx.send(Vst3ScanMessage::Progress {
                                done,
                                total,
                                path: label,
                            });
                        });
                    let _ = tx.send(Vst3ScanMessage::Finished(result));
                })
            {
                state.vst3_scan_rx = None;
                state.vst3_scan_in_progress = false;
                state.vst3_scan_current.clear();
                state.vst3_scan_error = Some(format!("scan worker 起動失敗: {e}"));
            }
        }
        if state.vst3_scan_in_progress {
            let mut progress = if state.vst3_scan_total > 0 {
                format!(
                    "({}/{})",
                    state.vst3_scan_done, state.vst3_scan_total
                )
            } else {
                "(列挙中)".to_string()
            };
            if !state.vst3_scan_current.is_empty() {
                progress.push(' ');
                progress.push_str(&state.vst3_scan_current);
            }
            ui.label(egui::RichText::new(progress).small().weak());
        }
        if !state.vst3_discovered.is_empty() {
            let hidden_unusable = state
                .vst3_discovered
                .iter()
                .filter(|p| p.hidden_by_default())
                .count();
            let probe_errors = state
                .vst3_discovered
                .iter()
                .filter(|p| p.has_probe_error())
                .count();
            ui.label(
                egui::RichText::new(if hidden_unusable > 0 && !state.vst3_show_unusable {
                    let mut text = format!(
                        "{} 個検出 / 音声入力なし {} 個は非表示",
                        state.vst3_discovered.len(), hidden_unusable
                    );
                    if probe_errors > 0 {
                        text.push_str(&format!(" / error {probe_errors} 個"));
                    }
                    format!("({text})")
                } else if hidden_unusable > 0 {
                    let mut text = format!(
                        "{} 個検出 / 音声入力なし {} 個を含む",
                        state.vst3_discovered.len(), hidden_unusable
                    );
                    if probe_errors > 0 {
                        text.push_str(&format!(" / error {probe_errors} 個"));
                    }
                    format!("({text})")
                } else if probe_errors > 0 {
                    format!("({} 個検出 / error {probe_errors} 個)", state.vst3_discovered.len())
                } else {
                    format!("({} 個検出)", state.vst3_discovered.len())
                })
                .small()
                .weak(),
            );
        }
        let chain_full = state.settings.vst3_plugins.len() >= MAX_CHAIN_LEN;
        if chain_full {
            ui.colored_label(
                egui::Color32::from_rgb(220, 160, 60),
                "上限 (10 個) に達しています",
            );
        }
    });
    if let Some(err) = &state.vst3_scan_error {
        ui.label(
            egui::RichText::new(format!("スキャンに失敗しました: {err}"))
                .small()
                .color(egui::Color32::from_rgb(220, 80, 80)),
        );
    }
    ui.label(
        egui::RichText::new(
            "認証が必要な VST3 は、他の DAW で 1 度起動して認証を済ませてから再スキャンしてください。",
        )
        .small()
        .weak(),
    );

    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("検索:");
        let mut output = egui::TextEdit::singleline(&mut state.vst3_filter)
            .hint_text("プラグイン名…")
            .desired_width(f32::INFINITY)
            .show(ui);
        crate::ui_helpers::singleline_text_edit_context_menu(
            ui,
            &mut output,
            &mut state.vst3_filter,
        );
    });
    if state.vst3_discovered.iter().any(|p| p.hidden_by_default()) {
        ui.checkbox(
            &mut state.vst3_show_unusable,
            "音声入力なしのプラグインも表示",
        )
        .on_hover_text(
            "Instrument / MIDI FX 等、動画音声を入力として受け取れない VST3 も候補に表示します。",
        );
    }
    ui.add_space(2.0);

    if state.vst3_discovered.is_empty() {
        ui.label(
            egui::RichText::new("「プラグインをスキャン」ボタンを押してください。")
                .weak()
                .small(),
        );
        return;
    }

    let chain_full = state.settings.vst3_plugins.len() >= MAX_CHAIN_LEN;
    let filter_lower = state.vst3_filter.to_ascii_lowercase();
    let existing: std::collections::HashSet<String> = state
        .settings
        .vst3_plugins
        .iter()
        .map(|e| e.path.clone())
        .collect();
    let available_height = ui.available_height().max(120.0);
    let mut clicked_add: Option<String> = None;
    egui::ScrollArea::vertical()
        .id_salt("vst3-pref-page-picker-scroll")
        .max_height(available_height)
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for plugin in &state.vst3_discovered {
                if plugin.hidden_by_default() && !state.vst3_show_unusable {
                    continue;
                }
                let path_s = plugin.path.to_string_lossy().to_string();
                let already_in_chain = existing.contains(&path_s);
                if !filter_lower.is_empty()
                    && !plugin
                        .display_name
                        .to_ascii_lowercase()
                        .contains(&filter_lower)
                {
                    continue;
                }
                let base_label = if plugin.has_probe_error() {
                    format!("{}  (error)", plugin.display_name)
                } else if let Some(reason) = plugin.hidden_reason() {
                    format!("{}  ({reason})", plugin.display_name)
                } else {
                    plugin.display_name.clone()
                };
                let label = if already_in_chain {
                    format!("{base_label}  (追加済み)")
                } else {
                    base_label
                };
                let enabled = !already_in_chain && !chain_full && !plugin.has_probe_error();
                let hover = if let Some(err) = &plugin.probe_error {
                    format!(
                        "{path_s}\n\nprobe error: {err}\n\n認証が必要な場合は、他の DAW で認証を済ませてから再スキャンしてください。"
                    )
                } else {
                    path_s.clone()
                };
                let resp = ui
                    .add_enabled(enabled, egui::Button::new(label))
                    .on_hover_text(hover);
                if resp.clicked() {
                    clicked_add = Some(path_s);
                }
            }
        });
    if let Some(path) = clicked_add {
        if state.settings.vst3_plugins.len() < MAX_CHAIN_LEN
            && !state.settings.vst3_plugins.iter().any(|e| e.path == path)
        {
            state.settings.vst3_plugins.push(Vst3PluginEntry {
                path,
                bypass: false,
                state: None,
                user_hidden: false,
                gui_pos: None,
                gui_size: None,
            });
        }
    }
}

#[cfg(not(windows))]
pub(super) fn page_vst3(_ui: &mut egui::Ui, _state: &mut PreferencesState) {}

pub(super) fn page_folder(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label(egui::RichText::new("フォルダサムネイル").strong());
    ui.add_space(4.0);
    ui.label("フォルダの代表画像をどの順序で選ぶか。\n先頭の画像がサムネイルとして表示されます。");
    ui.add_space(4.0);
    egui::ComboBox::from_label("代表画像の選択基準")
        .selected_text(s.folder_thumb_sort.label())
        .show_ui(ui, |ui| {
            for &order in SortOrder::all() {
                ui.selectable_value(&mut s.folder_thumb_sort, order, order.label());
            }
        });

    ui.add_space(12.0);
    ui.label(egui::RichText::new("フォルダサムネイル探索").strong());
    ui.add_space(4.0);
    ui.label(
        "フォルダの代表画像を探すとき、サブフォルダを何階層まで探索するか。\n\
         1 以上ではサブフォルダ内の画像を直接の子ファイルより優先します。0 にすると直接の子ファイルのみ使用します。",
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("サブフォルダ探索階層:");
        ui.add(
            egui::DragValue::new(&mut s.folder_thumb_depth)
                .range(0..=10u32)
                .suffix(" 階層"),
        );
    });

    ui.add_space(12.0);
    ui.label(egui::RichText::new("フォルダ移動").strong());
    ui.add_space(4.0);
    ui.label("Ctrl+↑↓ で移動先フォルダに画像がない場合、自動でスキップする最大回数。");
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label("空フォルダのスキップ上限:");
        ui.add(
            egui::DragValue::new(&mut s.folder_skip_limit)
                .range(1..=30usize)
                .suffix(" 回"),
        );
    });

    ui.add_space(12.0);
    ui.label(egui::RichText::new("設定のバックアップ").strong());
    ui.add_space(4.0);
    ui.label(
        "画像補正・消しゴムマスクの設定をフォルダごとに mimageviewer.dat として\n\
         隠しファイルで保存します。フォルダを丸ごと別ドライブへ移動しても設定が\n\
         保持されるようになります。",
    );
    ui.add_space(4.0);
    ui.checkbox(
        &mut s.sidecar_backup_enabled,
        "フォルダに補正・マスク設定のバックアップを保存する",
    );
    ui.add_space(4.0);
    ui.checkbox(
        &mut s.tag_sidecar_backup_enabled,
        "フォルダにタグのバックアップを保存する",
    );
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "タグのバックアップは既定 OFF です。共有フォルダに整理用タグを残したくない場合は OFF のまま使えます。\n\
             OFF 中は該当バックアップの書き込みも既存ファイルの読み込みも行いません (既存の mimageviewer.dat は削除されず残ります)。",
        )
        .size(11.0)
        .color(egui::Color32::from_gray(150)),
    );

    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);
    ui.label(egui::RichText::new("ファイル名スタック").strong());
    ui.add_space(4.0);
    ui.label(
        "アドレスバーの「スタック」ボタンで、似たファイルを自動で分類して 1 つに畳んで表示します。\n\
         既定では末尾連番・先頭連番・更新時刻 (連写) などを順に判定します。",
    );
    ui.add_space(4.0);
    ui.checkbox(
        &mut s.stack_script_enabled,
        "分類ルールをスクリプト (カスタム) で行う",
    )
    .on_hover_text(
        "OFF: 内蔵の自動分類ルールを使います。\n\
         ON: データフォルダの stack_rules.rhai (無ければ内蔵既定) を使います。",
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .button("スクリプトを開く")
            .on_hover_text("stack_rules.rhai を作成 (無ければ既定で) してエディタで開きます")
            .clicked()
        {
            match crate::filename_stack_script::ensure_user_script_exists() {
                Ok(path) => {
                    let _ = opener::open(&path);
                }
                Err(e) => crate::logger::log(format!("stack script open failed: {e}")),
            }
        }
        if ui
            .button("既定に戻す")
            .on_hover_text("stack_rules.rhai を内蔵の既定スクリプトで上書きします")
            .clicked()
        {
            if let Err(e) = crate::filename_stack_script::reset_user_script() {
                crate::logger::log(format!("stack script reset failed: {e}"));
            }
        }
        if ui.link("書き方をヘルプで見る").clicked() {
            let url = crate::ui_helpers::manual_url("stack.html", None);
            crate::ui_helpers::open_url(&url);
        }
    });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(
            "スクリプトは Rhai 言語で書きます (正規表現も使えます)。書き方や AI への依頼\n\
             テンプレートはヘルプの「スタック表示」ページを参照してください。",
        )
        .size(11.0)
        .color(egui::Color32::from_gray(150)),
    );
}

pub(super) fn page_duplicate_files(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;
    ui.checkbox(
        &mut s.skip_zip_if_folder_exists,
        "同名の ZIP/PDF/RAR/7z/LZH ファイルとフォルダがある場合、アーカイブ側をスキップ",
    );
    ui.add_space(4.0);
    ui.checkbox(
        &mut s.skip_image_if_video_exists,
        "同名の動画と画像がある場合、画像をスキップ",
    );
    ui.add_space(4.0);
    ui.checkbox(
        &mut s.skip_duplicate_images,
        "同名の画像が複数拡張子で存在する場合、優先度で選択",
    );

    if s.skip_duplicate_images {
        ui.add_space(4.0);
        ui.indent("ext_priority", |ui| {
            ui.label(
                egui::RichText::new("拡張子の優先度（上が最優先）:")
                    .size(12.0)
                    .color(egui::Color32::from_gray(160)),
            );
            ui.add_space(2.0);

            let mut swap: Option<(usize, usize)> = None;
            let len = state.settings.image_ext_priority.len();

            egui::ScrollArea::vertical()
                .max_height(200.0)
                .id_salt("dup_ext_scroll")
                .show(ui, |ui| {
                    for i in 0..len {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{}.", i + 1))
                                    .size(11.0)
                                    .color(egui::Color32::from_gray(140)),
                            );
                            ui.label(&state.settings.image_ext_priority[i]);
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if i + 1 < len && ui.small_button("▼").clicked() {
                                        swap = Some((i, i + 1));
                                    }
                                    if i > 0 && ui.small_button("▲").clicked() {
                                        swap = Some((i, i - 1));
                                    }
                                },
                            );
                        });
                    }
                });

            if let Some((a, b)) = swap {
                state.settings.image_ext_priority.swap(a, b);
            }

            ui.add_space(4.0);
            if ui.button("デフォルトに戻す").clicked() {
                state.settings.image_ext_priority = settings::default_image_ext_priority();
            }
        });
    }
}

pub(super) fn page_exif_display(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    enter_pressed: bool,
) {
    use crate::exif_reader::TagGroup;

    ui.label(
        "メタデータパネルで非表示にする EXIF タグを選択します。\n\
         チェックを入れたタグは「Image Info」サイドパネルに表示されません。",
    );
    ui.add_space(8.0);

    // 内側で max_height スクロールを作ると外側の pref_panel ScrollArea と
    // 二重スクロールになり、内側のスクロールバーが操作しづらい。外側の単一スクロールに
    // 任せる。
    for &group in TagGroup::ordered() {
        draw_exif_group(ui, state, group);
        ui.add_space(2.0);
    }
    draw_exif_custom_tags(ui, state);

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("カスタム追加:");
        let response = ui.text_edit_singleline(&mut state.exif_add_tag_input);
        if (ui.button("追加").clicked() || (response.lost_focus() && enter_pressed))
            && !state.exif_add_tag_input.trim().is_empty()
        {
            let tag = state.exif_add_tag_input.trim().to_string();
            if !state.settings.exif_hidden_tags.contains(&tag) {
                state.settings.exif_hidden_tags.push(tag.clone());
            }
            // 次フレームで該当行を viewport にスクロールするマーカー
            state.exif_scroll_to_added = Some(tag);
            state.exif_add_tag_input.clear();
        }
    });
    ui.label(
        egui::RichText::new(
            "MakerNote 系などリストに無いタグはここから追加できます (内部名で入力)。",
        )
        .small()
        .color(egui::Color32::from_gray(140)),
    );

    ui.add_space(4.0);
    if ui.button("デフォルトに戻す").clicked() {
        state.settings.exif_hidden_tags = settings::default_exif_hidden_tags();
    }
}

/// 1 グループ分のヘッダー (折りたたみ + 全選択/全解除) と、展開中ならチェックリストを描画する。
fn draw_exif_group(
    ui: &mut egui::Ui,
    state: &mut PreferencesState,
    group: crate::exif_reader::TagGroup,
) {
    use crate::exif_reader;

    // チェックリストに出すタグ一覧 (登録順)
    let tags: Vec<&'static exif_reader::TagInfo> =
        exif_reader::known_tags_in_group(group).collect();
    let total = tags.len();
    let hidden_count = tags
        .iter()
        .filter(|t| state.settings.exif_hidden_tags.iter().any(|h| h == t.name))
        .count();

    let collapsed = state.exif_collapsed_groups.contains(&group);
    let arrow = if collapsed { "▶" } else { "▼" };
    let header = format!(
        "{arrow} {}  ({hidden_count}/{total} 非表示)",
        group.display_name()
    );

    ui.horizontal(|ui| {
        if ui.selectable_label(false, header).clicked() {
            if collapsed {
                state.exif_collapsed_groups.remove(&group);
            } else {
                state.exif_collapsed_groups.insert(group);
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.small_button("全解除").clicked() {
                let names: Vec<&str> = tags.iter().map(|t| t.name).collect();
                state
                    .settings
                    .exif_hidden_tags
                    .retain(|h| !names.iter().any(|n| n == h));
            }
            if ui.small_button("全選択").clicked() {
                for tag in &tags {
                    if !state
                        .settings
                        .exif_hidden_tags
                        .iter()
                        .any(|h| h == tag.name)
                    {
                        state.settings.exif_hidden_tags.push(tag.name.to_string());
                    }
                }
            }
        });
    });

    if collapsed {
        return;
    }

    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 18,
            right: 0,
            top: 0,
            bottom: 4,
        })
        .show(ui, |ui| {
            for tag in &tags {
                let mut hidden = state
                    .settings
                    .exif_hidden_tags
                    .iter()
                    .any(|h| h == tag.name);
                let label = format!("{}  —  {}", tag.name, tag.display);
                if ui.checkbox(&mut hidden, label).changed() {
                    if hidden {
                        if !state
                            .settings
                            .exif_hidden_tags
                            .iter()
                            .any(|h| h == tag.name)
                        {
                            state.settings.exif_hidden_tags.push(tag.name.to_string());
                        }
                    } else {
                        state.settings.exif_hidden_tags.retain(|h| h != tag.name);
                    }
                }
            }
        });
}

/// `TAG_REGISTRY` に無い「カスタム」タグ (ユーザーが手動追加したもの) のリスト + × 削除。
fn draw_exif_custom_tags(ui: &mut egui::Ui, state: &mut PreferencesState) {
    use crate::exif_reader::{self, TagGroup};

    let known: std::collections::HashSet<&'static str> = TagGroup::ordered()
        .iter()
        .flat_map(|&g| exif_reader::known_tags_in_group(g))
        .map(|t| t.name)
        .collect();

    let custom: Vec<String> = state
        .settings
        .exif_hidden_tags
        .iter()
        .filter(|t| !known.contains(t.as_str()))
        .cloned()
        .collect();

    if custom.is_empty() {
        return;
    }

    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(format!("▼ カスタム  ({} 件)", custom.len()))
            .color(egui::Color32::from_gray(180))
            .size(13.0),
    );

    let mut to_remove: Option<String> = None;
    let scroll_target = state.exif_scroll_to_added.clone();
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: 18,
            right: 0,
            top: 0,
            bottom: 4,
        })
        .show(ui, |ui| {
            for tag in &custom {
                let row = ui.horizontal(|ui| {
                    ui.label(tag);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").clicked() {
                            to_remove = Some(tag.clone());
                        }
                    });
                });
                // 直前に追加された行を viewport にスクロールイン
                if scroll_target.as_deref() == Some(tag.as_str()) {
                    row.response.scroll_to_me(Some(egui::Align::Center));
                }
            }
        });

    if let Some(t) = to_remove {
        state.settings.exif_hidden_tags.retain(|x| x != &t);
    }
    // マーカーを 1 フレームで消費
    if state.exif_scroll_to_added.is_some() {
        state.exif_scroll_to_added = None;
    }
}

pub(super) fn page_spread_mode(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label(
        "フルスクリーンで画像を開いたときの初期表示。\n数字キー 1-5 でページ構成、6 で連結方式、7 で横方向、0 でズーム/フィットを切り替えできます。",
    );
    ui.add_space(4.0);
    egui::ComboBox::from_label("デフォルトのページ構成")
        .selected_text(s.default_spread_mode.label())
        .show_ui(ui, |ui| {
            for &mode in SpreadMode::all() {
                ui.selectable_value(&mut s.default_spread_mode, mode, mode.label());
            }
        });
    ui.add_space(8.0);
    egui::ComboBox::from_label("デフォルトの連結方式")
        .selected_text(s.default_reading_flow.label())
        .show_ui(ui, |ui| {
            for &flow in ReadingFlow::all() {
                ui.selectable_value(&mut s.default_reading_flow, flow, flow.label());
            }
        });
    egui::ComboBox::from_label("横連結の方向")
        .selected_text(s.default_reading_direction.label())
        .show_ui(ui, |ui| {
            for &direction in &[ReadingDirection::Ltr, ReadingDirection::Rtl] {
                ui.selectable_value(
                    &mut s.default_reading_direction,
                    direction,
                    direction.label(),
                );
            }
        });
    egui::ComboBox::from_label("ズーム/フィット")
        .selected_text(s.fullscreen_fit_mode.label())
        .show_ui(ui, |ui| {
            for &mode in FullscreenFitMode::all() {
                ui.selectable_value(&mut s.fullscreen_fit_mode, mode, mode.label());
            }
        });
    ui.horizontal(|ui| {
        ui.checkbox(&mut s.fullscreen_fit_no_upscale, "拡大しない");
        ui.checkbox(&mut s.fullscreen_fit_no_downscale, "縮小しない");
    });
    ui.small("自動フィット時の倍率制限。ホイールなどの手動ズームは制限しません。");
    ui.checkbox(
        &mut s.fullscreen_seek_bar_locked,
        "下部ページシークバーを固定表示",
    );
    ui.small("ON のときはフルスクリーン下端にシークバー領域を確保し、画像をその上の領域にフィットします。下部シークバー端の鍵アイコンからも切り替えできます。");
    ui.checkbox(
        &mut s.fullscreen_page_number_overlay,
        "ページ番号を常時表示",
    );
    ui.small("フルスクリーン右下に現在ページ / 総ページ数を小さく表示します。");
    ui.checkbox(
        &mut s.fullscreen_keep_on_app_switch,
        "メインに戻ったらフルスクリーンへ復帰",
    );
    ui.small("ON のときは、Alt+Tab などで mIV のメインウィンドウへ戻っても表示を閉じず、フルスクリーン側へフォーカスを戻します。メインも操作する場合は F12 別ウィンドウを使ってください。");
    ui.horizontal(|ui| {
        ui.label("マウスカーソルを隠すまで");
        ui.add(
            egui::DragValue::new(&mut s.fullscreen_cursor_hide_delay_secs)
                .range(
                    crate::settings::FULLSCREEN_CURSOR_HIDE_DELAY_MIN_SECS
                        ..=crate::settings::FULLSCREEN_CURSOR_HIDE_DELAY_MAX_SECS,
                )
                .speed(0.1)
                .fixed_decimals(1)
                .suffix(" 秒"),
        );
    });
    s.fullscreen_cursor_hide_delay_secs = crate::settings::clamp_fullscreen_cursor_hide_delay_secs(
        s.fullscreen_cursor_hide_delay_secs,
    );
    ui.horizontal(|ui| {
        ui.label("ページジャンプ量");
        egui::ComboBox::from_id_salt("fullscreen_jump_mode")
            .selected_text(s.fullscreen_jump_mode.label())
            .show_ui(ui, |ui| {
                for &mode in FullscreenJumpMode::all() {
                    ui.selectable_value(&mut s.fullscreen_jump_mode, mode, mode.label());
                }
            });
        match s.fullscreen_jump_mode {
            FullscreenJumpMode::Percent => {
                ui.add(
                    egui::DragValue::new(&mut s.fullscreen_jump_percent)
                        .range(
                            crate::settings::FULLSCREEN_JUMP_PERCENT_MIN
                                ..=crate::settings::FULLSCREEN_JUMP_PERCENT_MAX,
                        )
                        .speed(1)
                        .suffix(" %"),
                );
            }
            FullscreenJumpMode::FixedPages => {
                ui.add(
                    egui::DragValue::new(&mut s.fullscreen_fixed_jump_count)
                        .range(
                            crate::settings::FULLSCREEN_FIXED_JUMP_MIN
                                ..=crate::settings::FULLSCREEN_FIXED_JUMP_MAX,
                        )
                        .speed(1)
                        .suffix(" ページ"),
                );
            }
        }
    });
    s.fullscreen_jump_percent = s.fullscreen_jump_percent.clamp(
        crate::settings::FULLSCREEN_JUMP_PERCENT_MIN,
        crate::settings::FULLSCREEN_JUMP_PERCENT_MAX,
    );
    s.fullscreen_fixed_jump_count = s.fullscreen_fixed_jump_count.clamp(
        crate::settings::FULLSCREEN_FIXED_JUMP_MIN,
        crate::settings::FULLSCREEN_FIXED_JUMP_MAX,
    );
    ui.small("画像フルスクリーンの Shift+← / Shift+→ で前後へジャンプする量です。割合は画像・ZIP/PDF ページの総数から計算します。動画フルスクリーンでは Shift+左右は 1 秒シークのままです。");
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.label("見開きのページ間隔");
        ui.add(
            egui::DragValue::new(&mut s.spread_page_gap_px)
                .range(0..=200u32)
                .speed(1)
                .suffix(" px"),
        );
    });
    ui.horizontal(|ui| {
        ui.label("連結読みのページ間隔");
        ui.add(
            egui::DragValue::new(&mut s.continuous_reading_gap_px)
                .range(0..=200u32)
                .speed(1)
                .suffix(" px"),
        );
    });
    ui.add_space(8.0);
    ui.label("連結読みのスクロール量 (画面サイズ基準)");
    ui.horizontal(|ui| {
        ui.label("ホイール 1 ノッチ");
        ui.add(
            egui::DragValue::new(&mut s.continuous_reading_wheel_scroll_percent)
                .range(1..=100u32)
                .speed(1)
                .suffix(" %"),
        );
    });
    ui.horizontal(|ui| {
        ui.label("矢印キー / D-pad 1 回");
        ui.add(
            egui::DragValue::new(&mut s.continuous_reading_key_scroll_percent)
                .range(1..=100u32)
                .speed(1)
                .suffix(" %"),
        );
    });
    ui.horizontal(|ui| {
        ui.label("左スティック最大");
        ui.add(
            egui::DragValue::new(&mut s.continuous_reading_gamepad_scroll_percent_per_sec)
                .range(10..=300u32)
                .speed(1)
                .suffix(" %/秒"),
        );
    });
}

pub(super) fn page_playback_resume(ui: &mut egui::Ui, state: &mut PreferencesState) {
    use crate::settings::ResumeMode;
    let s = &mut state.settings;

    ui.label(
        "動画・音声・ZIP/PDF (本) の位置復元と、読書履歴を管理します。\n\
         一覧から開いたとき / 移動したとき (Ctrl+↑↓ や ↓↑・ホイールでの前後移動) に、前回の\n\
         位置 (続きから) で開くか、最初/先頭から開くかを選べます。保存された位置が無いときは\n\
         自動的に先頭になります。",
    );
    ui.add_space(10.0);

    // 「動画 × 一覧から開く」は互換のため既存 bool (video_grid_open_starts_from_beginning) が
    // 保存先。accessor 経由で ResumeMode として読み書きする。他 3 セルは専用 enum フィールド。
    let mut video_open = s.video_open_resume();
    let mut video_nav = s.video_nav_resume;
    let mut book_open = s.book_open_resume;
    let mut book_nav = s.book_nav_resume;
    let mut music_open = s.music_open_resume;
    let mut music_nav = s.music_nav_resume;

    let combo = |ui: &mut egui::Ui, id: &str, val: &mut ResumeMode| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(val.label())
            .width(96.0)
            .show_ui(ui, |ui| {
                for &m in ResumeMode::all() {
                    ui.selectable_value(val, m, m.label());
                }
            });
    };

    egui::Grid::new("playback_resume_grid")
        .num_columns(3)
        .spacing([24.0, 10.0])
        .show(ui, |ui| {
            ui.label("");
            ui.strong("一覧から開く");
            ui.strong("移動 (↓↑ / ホイール / Ctrl+↑↓)");
            ui.end_row();

            ui.strong("動画");
            combo(ui, "pr_video_open", &mut video_open);
            combo(ui, "pr_video_nav", &mut video_nav);
            ui.end_row();

            ui.strong("ZIP / PDF (本)");
            combo(ui, "pr_book_open", &mut book_open);
            combo(ui, "pr_book_nav", &mut book_nav);
            ui.end_row();

            ui.strong("音声");
            combo(ui, "pr_music_open", &mut music_open);
            combo(ui, "pr_music_nav", &mut music_nav);
            ui.end_row();
        });

    s.set_video_open_resume(video_open);
    s.video_nav_resume = video_nav;
    s.book_open_resume = book_open;
    s.book_nav_resume = book_nav;
    s.music_open_resume = music_open;
    s.music_nav_resume = music_nav;

    ui.add_space(12.0);
    ui.label(
        egui::RichText::new(
            "既定: 動画は「一覧から開く=続きから」「移動=続きから」。\n\
             ZIP/PDF は「一覧から開く=続きから」「移動=先頭から」。\n\
             音声は「一覧から開く=最初から」「移動=最初から」。\n\
             例えば「移動は続き・開いたら先頭」のように、セルごとに自由に組み合わせられます。",
        )
        .size(11.0)
        .color(egui::Color32::from_gray(150)),
    );

    // ── 保存済み位置の管理 (記憶件数の確認 + クリア) ──
    // s (= &mut state.settings) の借用は上の書き戻しで終わるので、以降は state を直接使う。
    ui.add_space(14.0);
    ui.separator();
    ui.add_space(8.0);
    ui.label(egui::RichText::new("保存済み位置の管理").strong());
    ui.add_space(4.0);

    // 動画・音声 (再生位置は settings 内の同じ HashMap を path キーで共有。クリアは OK 適用時に反映)。
    let video_count = state.settings.video_resume_positions.len();
    ui.label(format!(
        "動画・音声の再生位置: {video_count} 件を記憶 (3 秒以上再生・末尾 5 秒以内に未到達のときのみ保存)。"
    ));
    if video_count > 0 && ui.button("動画・音声の再生位置をすべてクリア").clicked()
    {
        state.settings.video_resume_positions.clear();
    }

    ui.add_space(8.0);

    // 本 = フォルダ / ZIP / PDF (読書位置は専用 DB。クリアは App 側で即時実行)。
    let book_count = state.book_resume_entry_count;
    ui.label(format!(
        "本 (フォルダ / ZIP / PDF) の読書位置: {book_count} 件を記憶。"
    ));
    if book_count > 0 && ui.button("本の読書位置をすべてクリア").clicked() {
        state.book_resume_clear_requested = true;
    }
    if let Some(msg) = &state.book_resume_clear_result {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(msg)
                .size(11.0)
                .color(egui::Color32::from_gray(150)),
        );
    }

    ui.add_space(10.0);
    ui.separator();
    ui.add_space(8.0);
    ui.label(egui::RichText::new("読書履歴").strong());
    ui.checkbox(
        &mut state.settings.reading_history_enabled,
        "フルスクリーンで読んだ本を読書履歴に記録する",
    );
    ui.horizontal(|ui| {
        ui.label("保持件数:");
        ui.add(
            egui::DragValue::new(&mut state.settings.reading_history_limit)
                .range(1..=crate::reading_history_db::READING_HISTORY_LIMIT_MAX)
                .speed(10),
        );
        ui.label(format!(
            "/ 最大 {}",
            crate::reading_history_db::READING_HISTORY_LIMIT_MAX
        ));
    });
    let history_count = state.reading_history_entry_count;
    ui.label(format!("読書履歴: {history_count} 件を記憶。"));
    if history_count > 0 && ui.button("読書履歴をすべてクリア").clicked() {
        state.reading_history_clear_requested = true;
    }
    if let Some(msg) = &state.reading_history_clear_result {
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new(msg)
                .size(11.0)
                .color(egui::Color32::from_gray(150)),
        );
    }
}

pub(super) fn page_susie_plugins(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.checkbox(&mut s.susie_enabled, "Susie 画像プラグインを有効にする");
    ui.label(
        egui::RichText::new(
            "OFF にするとプラグインフォルダを読み込まなくなります。\n\
             有効時は `mimageviewer-susie32.exe` (32bit ワーカープロセス) を起動して\n\
             プラグインをロードします。ワーカーが存在しない環境では自動的に無効化されます。",
        )
        .weak(),
    );
    ui.add_space(8.0);

    ui.add_enabled_ui(s.susie_enabled, |ui| {
        ui.checkbox(
            &mut s.susie_allow_parallel,
            "プラグインを並列実行する (推奨: ON)",
        );
        ui.label(
            egui::RichText::new(
                "OFF にするとワーカープロセス数を 1 に固定します。\n\
                 古いプラグインで一時ファイル衝突・INI の同時書き込み等の\n\
                 問題が疑われる場合に切り分け用として OFF にしてください。",
            )
            .weak(),
        );
    });

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);

    let plugin_dir = crate::susie_loader::plugin_dir();
    ui.horizontal(|ui| {
        ui.label("プラグインフォルダ:");
    });
    ui.label(
        egui::RichText::new(plugin_dir.display().to_string())
            .monospace()
            .weak(),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui.button("📁 フォルダを開く").clicked() {
            let _ = crate::susie_loader::ensure_plugin_dir();
            open_in_explorer(&plugin_dir);
        }
        if ui.button("⟳ プラグインを再読み込み").clicked() {
            crate::susie_loader::reload(s.susie_enabled, s.susie_allow_parallel);
        }
    });

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);

    // ロード済みプラグイン一覧 / 診断情報
    // 診断は "編集中の" enabled フラグを見る (OK 前にトグルした直後でも
    // チェックボックス表示と診断パネルが同じ状態を示すようにする)。
    ui.label(egui::RichText::new("ロード済みプラグイン").strong());
    ui.add_space(4.0);
    let status = crate::susie_loader::pool_status(s.susie_enabled);
    let plugins: Vec<crate::susie_loader::PluginInfo> = if matches!(
        status,
        crate::susie_loader::PoolStatus::ReadyWithPlugins { .. }
    ) {
        crate::susie_loader::try_get_pool()
            .map(|pool| pool.plugins().to_vec())
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    crate::ui_susie_diagnostic::render_diagnostic(ui, &status, &plugins);
}

/// 開発者 / 診断ページ。
///
/// 普通の利用者が「再生できない」「重い」等の不具合をサポートに伝えるとき、
/// コマンドライン引数や `%APPDATA%` の手探りなしでログを集められるようにする。
pub(super) fn page_developer(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.label(
        "不具合をサポートに調べてもらうための診断機能です。\n\
         通常の利用では設定する必要はありません。",
    );
    ui.add_space(12.0);

    // ── 診断情報の書き出し ──────────────────────────────────────
    ui.label(egui::RichText::new("診断情報").strong());
    ui.add_space(4.0);
    ui.label(
        "動作ログ・エラーログ・(記録していれば) 性能ログを 1 つの zip に\n\
         まとめてデスクトップに保存します。サポートへはこの zip を送ってください。",
    );
    ui.add_space(6.0);

    if ui.button("ログを zip にする").clicked() {
        // 利用者が明示的に押すボタンなので同期実行で問題ない
        // (ログは通常数 MB、最大でも数十 MB で deflate 圧縮は速い)。
        state.diag_export_result = Some(crate::diagnostics::export_diagnostics_zip());
    }

    match &state.diag_export_result {
        Some(Ok(path)) => {
            ui.add_space(6.0);
            ui.colored_label(
                egui::Color32::from_rgb(120, 200, 120),
                format!("保存しました: {}", path.display()),
            );
            let parent = path.parent().map(|p| p.to_path_buf());
            if let Some(parent) = parent {
                if ui.button("保存先フォルダを開く").clicked() {
                    open_in_explorer(&parent);
                }
            }
        }
        Some(Err(msg)) => {
            ui.add_space(6.0);
            ui.colored_label(
                egui::Color32::from_rgb(230, 120, 120),
                format!("書き出しに失敗しました: {msg}"),
            );
        }
        None => {}
    }

    ui.add_space(8.0);
    ui.label(
        egui::RichText::new("※ zip にはファイル名やフォルダのパスが含まれます。")
            .weak()
            .size(11.0),
    );

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(6.0);

    // ── 性能ログ ────────────────────────────────────────────────
    ui.label(egui::RichText::new("性能ログ").strong());
    ui.add_space(4.0);
    ui.checkbox(
        &mut state.settings.perf_log_enabled,
        "性能ログを記録する (次回起動から有効)",
    )
    .on_hover_text(
        "OFF (既定): 性能ログは記録しません。\n\
         ON: フレーム単位の詳細な性能イベントを記録します。\n\
         「動作が重い」「カクつく」といった不具合をサポートに調べてもらうときだけ\n\
         ON にしてください。ログファイルが大きくなるため、普段は OFF のままで\n\
         問題ありません。変更は次回起動時から反映されます。",
    );

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(6.0);

    ui.label(
        egui::RichText::new(
            "※ 動作ログ・エラーログは常に記録されています (古いものは自動的に\n  \
             整理されるためディスクを圧迫しません)。上の「ログを zip にする」で\n  \
             まとめて取り出せます。",
        )
        .weak()
        .size(11.0),
    );
}

fn open_in_explorer(path: &std::path::Path) {
    // Explorer でフォルダを開く。path が存在しなければ何もしない。
    if !path.exists() {
        return;
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer.exe").arg(path).spawn();
    }
    #[cfg(not(windows))]
    {
        let _ = path;
    }
}

#[cfg(test)]
mod tests {
    use super::{AI_SIZE_LIMIT_OPTIONS, natural_operation_label_cmp};
    use crate::app::MAX_TEXTURE_DIM;

    /// AI サイズ上限プリセットの長辺は GPU テクスチャ上限 (8192) を超えてはならない。
    /// render-to-target 後も最終 AI / composite は `MAX_TEXTURE_DIM` 以下に保つ。
    /// また表示は「長辺 x 短辺」なので short <= long を保つ。
    #[test]
    fn ai_size_limit_presets_stay_within_gpu_texture_limit() {
        let max = MAX_TEXTURE_DIM as u32;
        for (long, short, label) in AI_SIZE_LIMIT_OPTIONS {
            assert!(
                long <= max,
                "preset '{label}' long_edge {long} exceeds MAX_TEXTURE_DIM {max}"
            );
            assert!(
                short <= long,
                "preset '{label}' short_edge {short} exceeds long_edge {long}"
            );
        }
        // 最大プリセットが 8192 x 8192 であること (要望: 8192 まで対応)。
        let largest = AI_SIZE_LIMIT_OPTIONS.last().copied().unwrap();
        assert_eq!((largest.0, largest.1), (max, max));
    }

    #[test]
    fn operation_labels_sort_numbers_naturally() {
        let mut labels = vec![
            "サムネイル列数を10列に",
            "サムネイル列数を1列に",
            "サムネイル列数を2列に",
            "サムネイル列数を9列に",
        ];
        labels.sort_by(|a, b| natural_operation_label_cmp(a, b));
        assert_eq!(
            labels,
            vec![
                "サムネイル列数を1列に",
                "サムネイル列数を2列に",
                "サムネイル列数を9列に",
                "サムネイル列数を10列に",
            ]
        );
    }
}
