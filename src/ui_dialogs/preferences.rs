//! 統合環境設定ダイアログ。
//!
//! 左側にツリー、右側に設定パネルを配置した環境設定ウィンドウ。
//! OK / キャンセルで一時コピーを確定 or 破棄する。

use eframe::egui;
use std::collections::HashSet;

use crate::app::App;
use crate::settings::{self, CachePolicy, Parallelism, Settings, SortOrder, SpreadMode, ThumbAspect};

// ── ページ列挙 ──────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PreferencesPage {
    Thumbnail,
    Toolbar,
    Slideshow,
    Parallelism,
    Prefetch,
    GpuMemory,
    Cache,
    Folder,
    DuplicateFiles,
    ExifDisplay,
    SpreadMode,
}

impl PreferencesPage {
    fn label(self) -> &'static str {
        match self {
            Self::Thumbnail => "サムネイル",
            Self::Toolbar => "ツールバー",
            Self::Slideshow => "スライドショー",
            Self::Parallelism => "並列読み込み",
            Self::Prefetch => "先読み",
            Self::GpuMemory => "GPUメモリ",
            Self::Cache => "キャッシュ",
            Self::Folder => "フォルダ",
            Self::DuplicateFiles => "同名ファイル",
            Self::ExifDisplay => "EXIF表示",
            Self::SpreadMode => "見開き表示",
        }
    }
}

// ── ツリーカテゴリ定義 ──────────────────────────────────────────

struct TreeCategory {
    label: &'static str,
    /// カテゴリ自体がページを持つ場合の直接ページ
    page: Option<PreferencesPage>,
    /// 子ページ（空ならリーフカテゴリ）
    children: &'static [PreferencesPage],
}

const TREE: &[TreeCategory] = &[
    TreeCategory {
        label: "表示",
        page: None,
        children: &[PreferencesPage::Thumbnail, PreferencesPage::Toolbar, PreferencesPage::Slideshow],
    },
    TreeCategory {
        label: "パフォーマンス",
        page: None,
        children: &[PreferencesPage::Parallelism, PreferencesPage::Prefetch, PreferencesPage::GpuMemory],
    },
    TreeCategory {
        label: "キャッシュ",
        page: Some(PreferencesPage::Cache),
        children: &[],
    },
    TreeCategory {
        label: "フォルダ",
        page: Some(PreferencesPage::Folder),
        children: &[],
    },
    TreeCategory {
        label: "ファイル処理",
        page: None,
        children: &[PreferencesPage::DuplicateFiles, PreferencesPage::ExifDisplay],
    },
    TreeCategory {
        label: "見開き表示",
        page: Some(PreferencesPage::SpreadMode),
        children: &[],
    },
];

// ── 一時編集状態 ────────────────────────────────────────────────

pub(crate) struct PreferencesState {
    /// 編集用の Settings 一時コピー
    pub settings: Settings,
    /// 現在選択中のページ
    pub selected: PreferencesPage,
    /// 展開中のカテゴリラベル
    pub expanded: HashSet<&'static str>,

    // ページ固有の一時状態
    pub manual_threads: usize,
    pub exif_add_tag_input: String,

    // 初回に1度だけ取得するキャッシュ値
    pub auto_thread_count: usize,
    pub vram_mib: Option<u64>,
}

impl PreferencesState {
    fn from_settings(s: &Settings) -> Self {
        let manual_threads = match &s.parallelism {
            Parallelism::Manual(n) => *n,
            Parallelism::Auto => s.parallelism.thread_count(),
        };
        let auto_thread_count = {
            let cores = std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(2);
            (cores / 2).max(1)
        };
        let mut expanded = HashSet::new();
        for cat in TREE {
            if !cat.children.is_empty() {
                expanded.insert(cat.label);
            }
        }
        Self {
            settings: s.clone(),
            selected: PreferencesPage::Thumbnail,
            expanded,
            manual_threads,
            exif_add_tag_input: String::new(),
            auto_thread_count,
            vram_mib: crate::gpu_info::query_vram_summary_mib(),
        }
    }
}

// ── メインダイアログ ────────────────────────────────────────────

impl App {
    pub(crate) fn show_preferences_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_preferences {
            return;
        }

        // 初回: 一時コピーを作成
        if self.pref_state.is_none() {
            self.pref_state = Some(PreferencesState::from_settings(&self.settings));
        }

        let mut open = true;
        let mut apply = false;
        let mut cancel = false;

        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);

        egui::Window::new("環境設定")
            .open(&mut open)
            .resizable(true)
            .collapsible(false)
            .default_pos(dialog_pos)
            .default_size([720.0, 520.0])
            .show(ctx, |ui| {
                let state = self.pref_state.as_mut().unwrap();

                // ── メインエリア: 左ツリー + 右パネル ──
                let available = ui.available_size();
                let bottom_height = 36.0;
                let main_height = (available.y - bottom_height - 12.0).max(200.0);
                let tree_width = 180.0;

                // StripBuilder の代わりに手動で左右分割
                // 左ツリーを child_ui で配置し、残りを右パネルにする
                let outer_rect = ui.available_rect_before_wrap();
                let left_rect = egui::Rect::from_min_size(
                    outer_rect.min,
                    egui::vec2(tree_width, main_height),
                );
                let right_rect = egui::Rect::from_min_size(
                    egui::pos2(outer_rect.min.x + tree_width + 8.0, outer_rect.min.y),
                    egui::vec2(
                        (outer_rect.width() - tree_width - 8.0).max(100.0),
                        main_height,
                    ),
                );

                // 左ツリー
                let mut left_ui = ui.new_child(egui::UiBuilder::new()
                    .max_rect(left_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)));
                egui::ScrollArea::vertical()
                    .id_salt("pref_tree")
                    .max_height(main_height)
                    .show(&mut left_ui, |ui| {
                        ui.set_min_width(tree_width - 12.0);
                        draw_tree(ui, state);
                    });

                // 区切り線
                let sep_x = outer_rect.min.x + tree_width + 3.0;
                ui.painter().vline(
                    sep_x,
                    outer_rect.min.y..=outer_rect.min.y + main_height,
                    ui.visuals().widgets.noninteractive.bg_stroke,
                );

                // 右パネル
                let mut right_ui = ui.new_child(egui::UiBuilder::new()
                    .max_rect(right_rect)
                    .layout(egui::Layout::top_down(egui::Align::Min)));
                egui::ScrollArea::vertical()
                    .id_salt("pref_panel")
                    .max_height(main_height)
                    .show(&mut right_ui, |ui| {
                        ui.set_min_width(400.0);
                        draw_page(ui, state);
                    });

                // 全体の高さを確保
                ui.allocate_space(egui::vec2(available.x, main_height));

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // Esc でキャンセル
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    cancel = true;
                }

                ui.horizontal(|ui| {
                    if ui.button("  OK  ").clicked() {
                        apply = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });

        if apply {
            if let Some(state) = self.pref_state.take() {
                let old_dup = (
                    self.settings.skip_zip_if_folder_exists,
                    self.settings.skip_image_if_video_exists,
                    self.settings.skip_duplicate_images,
                    self.settings.image_ext_priority.clone(),
                );
                let old_exif = self.settings.exif_hidden_tags.clone();

                self.settings = state.settings;
                self.settings.save();

                // 同名ファイル設定が変更された場合はフォルダを再読み込み
                let new_dup = (
                    self.settings.skip_zip_if_folder_exists,
                    self.settings.skip_image_if_video_exists,
                    self.settings.skip_duplicate_images,
                    self.settings.image_ext_priority.clone(),
                );
                if old_dup != new_dup {
                    if let Some(folder) = self.current_folder.clone() {
                        self.load_folder(folder);
                    }
                }
                if old_exif != self.settings.exif_hidden_tags {
                    self.exif_cache.clear();
                }
            }
            self.show_preferences = false;
        } else if cancel || !open {
            self.pref_state = None;
            self.show_preferences = false;
        }
    }
}

// ── ツリー描画 ──────────────────────────────────────────────────

fn draw_tree(ui: &mut egui::Ui, state: &mut PreferencesState) {
    for cat in TREE {
        if cat.children.is_empty() {
            // リーフカテゴリ（直接ページを持つ）
            if let Some(page) = cat.page {
                let selected = state.selected == page;
                if ui.selectable_label(selected, cat.label).clicked() {
                    state.selected = page;
                }
            }
        } else {
            // 子を持つカテゴリ
            let is_expanded = state.expanded.contains(cat.label);
            let icon = if is_expanded { "▼ " } else { "▶ " };
            let header_text = format!("{}{}", icon, cat.label);

            // カテゴリヘッダ: クリックで展開/折り畳み
            // カテゴリ自体がページを持つ場合は選択もする
            let is_cat_selected = cat.page.is_some_and(|p| state.selected == p);
            let resp = ui.selectable_label(is_cat_selected, egui::RichText::new(header_text).strong());
            if resp.clicked() {
                if is_expanded {
                    state.expanded.remove(cat.label);
                } else {
                    state.expanded.insert(cat.label);
                }
                if let Some(page) = cat.page {
                    state.selected = page;
                }
            }

            // 子ページ
            if is_expanded {
                for &child in cat.children {
                    let selected = state.selected == child;
                    let text = format!("    {}", child.label());
                    if ui.selectable_label(selected, text).clicked() {
                        state.selected = child;
                    }
                }
            }
        }
    }
}

// ── 右パネル ページ描画 ─────────────────────────────────────────

fn draw_page(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.heading(state.selected.label());
    ui.add_space(8.0);

    match state.selected {
        PreferencesPage::Thumbnail => page_thumbnail(ui, state),
        PreferencesPage::Toolbar => page_toolbar(ui, state),
        PreferencesPage::Slideshow => page_slideshow(ui, state),
        PreferencesPage::Parallelism => page_parallelism(ui, state),
        PreferencesPage::Prefetch => page_prefetch(ui, state),
        PreferencesPage::GpuMemory => page_gpu_memory(ui, state),
        PreferencesPage::Cache => page_cache(ui, state),
        PreferencesPage::Folder => page_folder(ui, state),
        PreferencesPage::DuplicateFiles => page_duplicate_files(ui, state),
        PreferencesPage::ExifDisplay => page_exif_display(ui, state),
        PreferencesPage::SpreadMode => page_spread_mode(ui, state),
    }
}

// ── 個別ページ実装 ──────────────────────────────────────────────

fn page_thumbnail(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.checkbox(
        &mut state.settings.thumb_idle_upgrade,
        "アイドル時にキャッシュ由来のサムネイルを高画質化する",
    );
    ui.label(
        "  スクロール停止後、キャッシュ復元 (WebP q=75) のサムネイルを\n  \
         元画像から再デコードして差し替えます。visible 側から順次処理。",
    );
}

fn page_toolbar(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label(
        "チェックを外した項目はツールバーから隠れます。\n\
         セクション内の全項目を外すとセクション自体が非表示になります。",
    );
    ui.add_space(6.0);

    ui.checkbox(&mut s.show_toolbar_favorites, "お気に入り");
    ui.checkbox(&mut s.show_toolbar_folder, "フォルダ (アドレスバー)");
    ui.checkbox(&mut s.show_toolbar_parent_button, "上のフォルダへ (⬆ ボタン)");

    // ── 列 ──
    ui.add_space(6.0);
    ui.separator();
    ui.add_space(2.0);
    ui.label(egui::RichText::new("列").strong());
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
    ui.horizontal_wrapped(|ui| {
        for &aspect in ThumbAspect::all() {
            let mut checked = s.toolbar_aspect_items.contains(&aspect);
            if ui.checkbox(&mut checked, aspect.label()).changed() {
                if checked {
                    s.toolbar_aspect_items.push(aspect);
                    let order: Vec<_> = ThumbAspect::all().to_vec();
                    s.toolbar_aspect_items.sort_by_key(|a| {
                        order.iter().position(|o| o == a).unwrap_or(usize::MAX)
                    });
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
}

fn page_slideshow(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.horizontal(|ui| {
        ui.label("切り替え間隔:");
        ui.add(
            egui::Slider::new(&mut state.settings.slideshow_interval_secs, 0.5..=30.0)
                .suffix(" 秒")
                .fixed_decimals(1),
        );
    });
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new("フルスクリーンで Space キーまたは ▶ ボタンで開始")
            .size(11.0)
            .color(egui::Color32::from_gray(140)),
    );
}

fn page_parallelism(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;
    let is_auto = s.parallelism == Parallelism::Auto;

    let mut current_auto = is_auto;
    if ui
        .radio(
            current_auto,
            format!("自動（CPUコア数の半分: {} スレッド）", state.auto_thread_count),
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

fn page_prefetch(ui: &mut egui::Ui, state: &mut PreferencesState) {
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

fn page_gpu_memory(ui: &mut egui::Ui, state: &mut PreferencesState) {
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

fn page_cache(ui: &mut egui::Ui, state: &mut PreferencesState) {
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

fn page_folder(ui: &mut egui::Ui, state: &mut PreferencesState) {
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
    ui.label("フォルダの代表画像を探すとき、サブフォルダを何階層まで探索するか。\n0 にすると直接の子ファイルのみ使用します。");
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
                .range(1..=10usize)
                .suffix(" 回"),
        );
    });
}

fn page_duplicate_files(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;
    ui.checkbox(
        &mut s.skip_zip_if_folder_exists,
        "同名の ZIP ファイルとフォルダがある場合、ZIP をスキップ",
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

fn page_exif_display(ui: &mut egui::Ui, state: &mut PreferencesState) {
    ui.label("非表示にする EXIF タグ名:");
    ui.add_space(4.0);

    let mut to_remove: Option<usize> = None;
    let avail_w = ui.available_width();

    egui::ScrollArea::vertical()
        .max_height(300.0)
        .id_salt("exif_tags_scroll")
        .show(ui, |ui| {
            ui.set_min_width(avail_w);
            for (i, tag) in state.settings.exif_hidden_tags.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.set_min_width(avail_w - 8.0);
                    ui.label(tag);
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if ui.small_button("×").clicked() {
                                to_remove = Some(i);
                            }
                        },
                    );
                });
            }
        });

    if let Some(idx) = to_remove {
        state.settings.exif_hidden_tags.remove(idx);
    }

    ui.add_space(8.0);
    ui.separator();
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label("追加:");
        let response = ui.text_edit_singleline(&mut state.exif_add_tag_input);
        if (ui.button("追加").clicked()
            || response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter)))
            && !state.exif_add_tag_input.trim().is_empty()
        {
            let tag = state.exif_add_tag_input.trim().to_string();
            if !state.settings.exif_hidden_tags.contains(&tag) {
                state.settings.exif_hidden_tags.push(tag);
            }
            state.exif_add_tag_input.clear();
        }
    });

    ui.add_space(4.0);
    if ui.button("デフォルトに戻す").clicked() {
        state.settings.exif_hidden_tags = settings::default_exif_hidden_tags();
    }
}

fn page_spread_mode(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label("フルスクリーンで画像を開いたときの初期表示モード。\n数字キー 5-9 でも切り替えできます。");
    ui.add_space(4.0);
    egui::ComboBox::from_label("デフォルトの表示モード")
        .selected_text(s.default_spread_mode.label())
        .show_ui(ui, |ui| {
            for &mode in SpreadMode::all() {
                ui.selectable_value(&mut s.default_spread_mode, mode, mode.label());
            }
        });
}
