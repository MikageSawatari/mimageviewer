use super::*;
use crate::settings::{
    self, CachePolicy, Parallelism, SortOrder, SpreadMode, ThumbAspect, ToolbarSectionDisplay,
    UiTheme,
};

pub(super) fn page_theme(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.label("背景色テーマ (v0.7.0)");
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
}

pub(super) fn page_thumbnail(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.checkbox(
        &mut state.settings.thumb_idle_upgrade,
        "アイドル時にキャッシュ由来のサムネイルを高画質化する",
    );
    ui.label(
        "  スクロール停止後、キャッシュ復元 (WebP q=75) のサムネイルを\n  \
         元画像から再デコードして差し替えます。visible 側から順次処理。",
    );
}

pub(super) fn page_toolbar(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label(
        "チェックを外した項目はツールバーから隠れます。\n\
         セクション内の全項目を外すとセクション自体が非表示になります。\n\
         上から順に画面のツールバー左端 → 右端に対応します。",
    );
    ui.add_space(6.0);

    // Phase 6.E 以降、フォルダ移動系はアドレス欄と同じ行の「フォルダバー」に集約。
    ui.label(egui::RichText::new("フォルダバー").strong());
    ui.checkbox(&mut s.show_toolbar_folder, "フォルダ入力欄を表示");
    ui.add_enabled(
        s.show_toolbar_folder,
        egui::Checkbox::new(
            &mut s.show_address_bar_history_nav,
            "└ 履歴の戻る/進むボタン (←/→)",
        ),
    )
    .on_hover_text("移動したフォルダの履歴をブラウザのように戻る / 進むできます。");
    ui.add_enabled(
        s.show_toolbar_folder,
        egui::Checkbox::new(&mut s.show_toolbar_parent_button, "└ 親フォルダボタン (⬆)"),
    );
    let mut show_tree_nav = s.show_toolbar_prev_folder || s.show_toolbar_next_folder;
    if ui
        .add_enabled(
            s.show_toolbar_folder,
            egui::Checkbox::new(&mut show_tree_nav, "└ ツリー順の前/次フォルダボタン (▲/▼)"),
        )
        .on_hover_text("Ctrl+↑/↓ と同じく、深さ優先のツリー順で前後のフォルダへ移動します。")
        .changed()
    {
        s.show_toolbar_prev_folder = show_tree_nav;
        s.show_toolbar_next_folder = show_tree_nav;
    }
    ui.add_enabled(
        s.show_toolbar_folder,
        egui::Checkbox::new(
            &mut s.show_address_bar_favorite_button,
            "└ お気に入り追加/設定ボタン (♡/♥)",
        ),
    );
    ui.add_enabled(
        s.show_toolbar_folder,
        egui::Checkbox::new(
            &mut s.show_address_bar_history_menu,
            "└ 最近開いたフォルダ履歴メニュー",
        ),
    );
    // 代表サムネ固定 (📌) ボタンはフォルダバーの一部だが、機能としては独立した
    // 切り替えを提供する (= 自動代表サムネで運用するユーザー向けに非表示にできる)。
    ui.add_enabled(
        s.show_toolbar_folder,
        egui::Checkbox::new(
            &mut s.show_address_bar_folder_pin,
            "└ 代表サムネ固定 (📌) ボタン",
        ),
    )
    .on_hover_text(
        "フォルダバーの 📌 ボタンで、選択中のアイテムを\
         フォルダ / ZIP / PDF の代表サムネに固定できます。\n\
         左クリック: 設定 / 同じ項目で再クリック解除\n\
         右クリック: 解除\n\
         右クリックメニューからも操作できます。",
    );
    // (旧) VST3 ツールバーボタン: v0.9.0 開発中に削除。動画再生中はホバーバーから
    // パネルを開く運用に統一。settings の `show_toolbar_vst3` は legacy フラグとして残るが
    // 動作には影響しない。
    let _ = &mut s.show_toolbar_vst3; // 未使用警告抑制

    // セクションごとの「展開 / プルダウン」表示形式選択 helper。
    // Buttons (= 展開): 既存挙動、横並びの selectable_label 群。
    // Dropdown (= プルダウン): ComboBox 1 個。スペース節約用。
    fn display_radio(ui: &mut egui::Ui, value: &mut ToolbarSectionDisplay) {
        ui.horizontal(|ui| {
            ui.label("表示:");
            for &opt in ToolbarSectionDisplay::all() {
                ui.radio_value(value, opt, opt.label());
            }
        });
    }

    // ── 列 ──
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(2.0);
    ui.label(egui::RichText::new("列").strong());
    display_radio(ui, &mut s.toolbar_cols_display);
    ui.horizontal_wrapped(|ui| {
        for cols in 1..=10usize {
            let mut checked = s.toolbar_cols_items.contains(&cols);
            if ui.checkbox(&mut checked, format!("{cols}")).changed() {
                if checked {
                    s.toolbar_cols_items.push(cols);
                    s.toolbar_cols_items.sort();
                } else {
                    s.toolbar_cols_items.retain(|&c| c != cols);
                }
            }
        }
    });

    // ── 比率 ──
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(2.0);
    ui.label(egui::RichText::new("比率").strong());
    display_radio(ui, &mut s.toolbar_aspect_display);
    ui.horizontal_wrapped(|ui| {
        // 「自動」項目 (常時 7 種より前に表示)。デフォルト ON。
        ui.checkbox(&mut s.toolbar_aspect_auto_visible, "自動");
        for &aspect in ThumbAspect::all() {
            let mut checked = s.toolbar_aspect_items.contains(&aspect);
            if ui.checkbox(&mut checked, aspect.label()).changed() {
                if checked {
                    s.toolbar_aspect_items.push(aspect);
                    let order: Vec<_> = ThumbAspect::all().to_vec();
                    s.toolbar_aspect_items
                        .sort_by_key(|a| order.iter().position(|o| o == a).unwrap_or(usize::MAX));
                } else {
                    s.toolbar_aspect_items.retain(|&a| a != aspect);
                }
            }
        }
    });

    // ── ソート ──
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(2.0);
    ui.label(egui::RichText::new("ソート").strong());
    display_radio(ui, &mut s.toolbar_sort_display);
    ui.horizontal_wrapped(|ui| {
        for &order in SortOrder::all() {
            let mut checked = s.toolbar_sort_items.contains(&order);
            if ui.checkbox(&mut checked, order.short_label()).changed() {
                if checked {
                    s.toolbar_sort_items.push(order);
                    let canonical: Vec<_> = SortOrder::all().to_vec();
                    s.toolbar_sort_items.sort_by_key(|so| {
                        canonical.iter().position(|o| o == so).unwrap_or(usize::MAX)
                    });
                } else {
                    s.toolbar_sort_items.retain(|&so| so != order);
                }
            }
        }
    });

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(2.0);
    ui.checkbox(&mut s.show_toolbar_rating, "レーティング (★ フィルタ)");
    ui.checkbox(&mut s.show_toolbar_favorites, "お気に入り");
    ui.checkbox(&mut s.show_toolbar_tags, "タグ");
}

pub(super) fn page_slideshow(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.horizontal(|ui| {
        ui.label("切り替え間隔:");
        ui.add(
            egui::Slider::new(&mut state.settings.slideshow_interval_secs, 0.5..=30.0)
                .suffix(" 秒")
                .fixed_decimals(1),
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
    ui.horizontal(|ui| {
        let response = ui.add(
            egui::TextEdit::singleline(&mut state.capture_output_dir_input)
                .desired_width(360.0)
                .hint_text(crate::capture::default_output_dir().display().to_string()),
        );
        if response.changed() {
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
    ui.label(egui::RichText::new("AI 処理のスキップしきい値").strong());
    ui.add_space(4.0);
    ui.label("画像の幅または高さがしきい値以上の場合、AI 処理をスキップします。");
    ui.add_space(4.0);

    let skip_options = [512, 1024, 2048];

    ui.horizontal(|ui| {
        ui.label("アップスケール:");
        for &px in &skip_options {
            ui.radio_value(&mut s.ai_upscale_skip_px, px, format!("{px} px"));
        }
    });
    ui.horizontal(|ui| {
        ui.label("ノイズ除去:");
        for &px in &skip_options {
            ui.radio_value(&mut s.ai_denoise_skip_px, px, format!("{px} px"));
        }
    });
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

        ui.add_space(12.0);
        ui.separator();
        ui.add_space(8.0);

        ui.label(egui::RichText::new("レジューム再生").strong());
        ui.add_space(4.0);
        ui.checkbox(
            &mut s.video_grid_open_starts_from_beginning,
            "一覧から開いたときは最初から再生する",
        )
        .on_hover_text(
            "ON: サムネイル一覧からダブルクリック / Enter で開いた動画は、保存済み再生位置があっても先頭から再生します。\n\
         ホイール / ↑↓ などフルスクリーン中の動画移動では、従来どおり保存済み位置から再開します。",
        );
        ui.add_space(6.0);
        let count = s.video_resume_positions.len();
        ui.label(format!(
            "現在 {count} 件の動画について再生位置を記憶しています。\n\
         3 秒以上再生・かつ末尾 5 秒以内に到達していない場合のみ保存されます。"
        ));
        if count > 0 && ui.button("すべての再生位置をクリア").clicked() {
            s.video_resume_positions.clear();
        }
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
        ui.add(
            egui::TextEdit::singleline(&mut state.vst3_filter)
                .hint_text("プラグイン名…")
                .desired_width(f32::INFINITY),
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
    ui.label(
        egui::RichText::new(
            "OFF 中はバックアップの書き込みも既存ファイルの読み込みも行いません\n\
             (既存の mimageviewer.dat は削除されず残ります)。",
        )
        .size(11.0)
        .color(egui::Color32::from_gray(150)),
    );
}

pub(super) fn page_duplicate_files(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;
    ui.checkbox(
        &mut s.skip_zip_if_folder_exists,
        "同名の ZIP/PDF/7z/LZH ファイルとフォルダがある場合、アーカイブ側をスキップ",
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
        "フルスクリーンで画像を開いたときの初期表示モード。\n数字キー 1-5 でも切り替えできます。",
    );
    ui.add_space(4.0);
    egui::ComboBox::from_label("デフォルトの表示モード")
        .selected_text(s.default_spread_mode.label())
        .show_ui(ui, |ui| {
            for &mode in SpreadMode::all() {
                ui.selectable_value(&mut s.default_spread_mode, mode, mode.label());
            }
        });

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);
    ui.checkbox(
        &mut s.auto_fullscreen_zip_pdf,
        "ZIP/PDF を開いたら 1 ページ目をフルスクリーンで表示",
    );
    ui.label(
        egui::RichText::new(
            "一覧から ZIP/PDF を Enter / ダブルクリックで開いたとき、ページ一覧を経由せず\n\
             1 ページ目を直接フルスクリーンで開きます。フルスクリーン中の Enter / Esc で\n\
             元の一覧へ戻り、Backspace でそのファイルのページ一覧を表示します。",
        )
        .weak(),
    );
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
