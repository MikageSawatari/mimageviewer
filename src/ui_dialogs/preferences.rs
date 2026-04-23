//! 統合環境設定ダイアログ。
//!
//! 左側にツリー、右側に設定パネルを配置した環境設定ウィンドウ。
//! OK / キャンセルで一時コピーを確定 or 破棄する。

use eframe::egui;
use std::collections::HashSet;

use crate::app::App;
use crate::settings::{
    self, CachePolicy, Parallelism, Settings, SortOrder, SpreadMode, ThumbAspect, UiTheme,
};

// ── ページ列挙 ──────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PreferencesPage {
    Theme,
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
    SusiePlugins,
    /// v0.8.0: 自動インデクサ速度プロファイル
    IndexerSpeed,
    /// v0.9: タスクトレイ常駐 / 常駐中 pause 設定
    TrayResidency,
}

impl PreferencesPage {
    fn label(self) -> &'static str {
        match self {
            Self::Theme => "テーマ",
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
            Self::SusiePlugins => "Susie プラグイン",
            Self::IndexerSpeed => "自動インデクサ速度",
            Self::TrayResidency => "タスクトレイ常駐",
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
        children: &[
            PreferencesPage::Theme,
            PreferencesPage::Thumbnail,
            PreferencesPage::Toolbar,
            PreferencesPage::Slideshow,
        ],
    },
    TreeCategory {
        label: "パフォーマンス",
        page: None,
        children: &[
            PreferencesPage::Parallelism,
            PreferencesPage::Prefetch,
            PreferencesPage::GpuMemory,
        ],
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
        children: &[
            PreferencesPage::DuplicateFiles,
            PreferencesPage::ExifDisplay,
        ],
    },
    TreeCategory {
        label: "見開き表示",
        page: Some(PreferencesPage::SpreadMode),
        children: &[],
    },
    TreeCategory {
        label: "Susie プラグイン",
        page: Some(PreferencesPage::SusiePlugins),
        children: &[],
    },
    // 全文検索インデクサ (Ctrl+F / Ctrl+G 用) の速度プロファイル
    TreeCategory {
        label: "全文検索インデクサ",
        page: Some(PreferencesPage::IndexerSpeed),
        children: &[],
    },
    // v0.9: タスクトレイ常駐
    TreeCategory {
        label: "タスクトレイ常駐",
        page: Some(PreferencesPage::TrayResidency),
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
    /// EXIF タグ設定で折りたたみ中のグループ。`HashSet` に入っているものが折りたたみ。
    pub exif_collapsed_groups: HashSet<crate::exif_reader::TagGroup>,
    /// カスタム追加直後に「自動スクロールして見せる」タグ名 (1 フレームだけ持つ)。
    pub exif_scroll_to_added: Option<String>,

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
            exif_collapsed_groups: HashSet::new(),
            exif_scroll_to_added: None,
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
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);

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
                let left_rect =
                    egui::Rect::from_min_size(outer_rect.min, egui::vec2(tree_width, main_height));
                let right_rect = egui::Rect::from_min_size(
                    egui::pos2(outer_rect.min.x + tree_width + 8.0, outer_rect.min.y),
                    egui::vec2(
                        (outer_rect.width() - tree_width - 8.0).max(100.0),
                        main_height,
                    ),
                );

                // 左ツリー
                let mut left_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(left_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
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
                let mut right_ui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(right_rect)
                        .layout(egui::Layout::top_down(egui::Align::Min)),
                );
                egui::ScrollArea::vertical()
                    .id_salt("pref_panel")
                    .max_height(main_height)
                    .show(&mut right_ui, |ui| {
                        ui.set_min_width(400.0);
                        draw_page(ui, state, enter_pressed);
                    });

                // 全体の高さを確保
                ui.allocate_space(egui::vec2(available.x, main_height));

                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                // Esc でキャンセル (IME 変換中はスキップ)
                if escape_pressed {
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

                let old_susie = (
                    self.settings.susie_enabled,
                    self.settings.susie_allow_parallel,
                );

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
                    self.reload_current_folder_preserving_override();
                }
                if old_exif != self.settings.exif_hidden_tags {
                    self.exif_cache.clear();
                }

                let new_susie = (
                    self.settings.susie_enabled,
                    self.settings.susie_allow_parallel,
                );
                if old_susie != new_susie {
                    crate::susie_loader::reload(
                        self.settings.susie_enabled,
                        self.settings.susie_allow_parallel,
                    );
                    // 対応拡張子が変わる可能性があるので現在のフォルダを再読み込み
                    self.reload_current_folder_preserving_override();
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
            let resp =
                ui.selectable_label(is_cat_selected, egui::RichText::new(header_text).strong());
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

fn draw_page(ui: &mut egui::Ui, state: &mut PreferencesState, enter_pressed: bool) {
    ui.heading(state.selected.label());
    ui.add_space(8.0);

    match state.selected {
        PreferencesPage::Theme => page_theme(ui, state),
        PreferencesPage::Thumbnail => page_thumbnail(ui, state),
        PreferencesPage::Toolbar => page_toolbar(ui, state),
        PreferencesPage::Slideshow => page_slideshow(ui, state),
        PreferencesPage::Parallelism => page_parallelism(ui, state),
        PreferencesPage::Prefetch => page_prefetch(ui, state),
        PreferencesPage::GpuMemory => page_gpu_memory(ui, state),
        PreferencesPage::Cache => page_cache(ui, state),
        PreferencesPage::Folder => page_folder(ui, state),
        PreferencesPage::DuplicateFiles => page_duplicate_files(ui, state),
        PreferencesPage::ExifDisplay => page_exif_display(ui, state, enter_pressed),
        PreferencesPage::SpreadMode => page_spread_mode(ui, state),
        PreferencesPage::SusiePlugins => page_susie_plugins(ui, state),
        PreferencesPage::IndexerSpeed => page_indexer_speed(ui, state),
        PreferencesPage::TrayResidency => page_tray_residency(ui, state),
    }
}

// ── 個別ページ実装 ──────────────────────────────────────────────

fn page_theme(ui: &mut egui::Ui, state: &mut PreferencesState) {
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
    ui.checkbox(&mut s.show_toolbar_tags, "タグ");
    ui.checkbox(&mut s.show_toolbar_folder, "フォルダ (アドレスバー)");
    ui.checkbox(
        &mut s.show_toolbar_parent_button,
        "上のフォルダへ (⬆ ボタン)",
    );
    ui.checkbox(&mut s.show_toolbar_rating, "レーティング (★ フィルタ)");

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
        egui::RichText::new("フルスクリーンで S キーまたは ▶ ボタンで開始 / 停止")
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

/// v0.8.0: 自動インデクサの速度プロファイル設定ページ。
///
/// `IndexerSpeedProfile` は `GlobalIoSemaphore` の permit 数を決める。
/// 値の変更は **次回起動時に反映** される (ランタイム差し替えは `sync_with_favorites`
/// でも反映されないので現状は再起動が必要)。
fn page_indexer_speed(ui: &mut egui::Ui, state: &mut PreferencesState) {
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
fn page_tray_residency(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label(
        "閉じるボタンを押したときのアプリ終了挙動と、常駐中のインデックス処理を制御します。",
    );
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

fn page_duplicate_files(ui: &mut egui::Ui, state: &mut PreferencesState) {
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

fn page_exif_display(ui: &mut egui::Ui, state: &mut PreferencesState, enter_pressed: bool) {
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

fn page_spread_mode(ui: &mut egui::Ui, state: &mut PreferencesState) {
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
}

fn page_susie_plugins(ui: &mut egui::Ui, state: &mut PreferencesState) {
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
