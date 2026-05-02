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
    /// AI 推論バックエンド (DirectML / TensorRT) の選択と TRT pack 管理
    AiBackend,
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
    /// v0.8.1: レーティング XMP 書き込み設定
    Rating,
    /// 起動時 / 定期的なバージョン更新確認
    UpdateCheck,
    /// 動画再生 (HW デコード等)
    Video,
    /// VST3 プラグイン設定 (= 有効化 + チェーン編集)
    Vst3,
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
            Self::AiBackend => "AI バックエンド",
            Self::Cache => "キャッシュ",
            Self::Folder => "フォルダ",
            Self::DuplicateFiles => "同名ファイル",
            Self::ExifDisplay => "EXIF表示",
            Self::SpreadMode => "見開き表示",
            Self::SusiePlugins => "Susie プラグイン",
            Self::IndexerSpeed => "自動インデクサ速度",
            Self::TrayResidency => "タスクトレイ常駐",
            Self::Rating => "レーティング",
            Self::UpdateCheck => "更新確認",
            Self::Video => "動画再生",
            Self::Vst3 => "VST3 プラグイン",
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
            PreferencesPage::AiBackend,
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
    // v0.8.1: レーティングの XMP 書き込み (opt-in)
    TreeCategory {
        label: "レーティング",
        page: Some(PreferencesPage::Rating),
        children: &[],
    },
    // バージョン更新確認 (起動時 + 24h 周期)
    TreeCategory {
        label: "更新確認",
        page: Some(PreferencesPage::UpdateCheck),
        children: &[],
    },
    // 動画再生 (HW デコード等)
    TreeCategory {
        label: "動画再生",
        page: Some(PreferencesPage::Video),
        children: &[],
    },
    // VST3 プラグイン設定 (= 動画再生時の音声プラグイン処理)
    TreeCategory {
        label: "VST3 プラグイン",
        page: Some(PreferencesPage::Vst3),
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

    // ── AI バックエンド ページ用のキャッシュ ────────────────────
    /// プライマリ GPU のベンダー (NVIDIA でなければ TRT は disabled に)
    pub gpu_vendor: Option<crate::gpu_info::GpuVendor>,
    /// TRT ワーカープールが現在 attach されているか (Phase 3、ホットリロード)。
    /// `true` ならアップスケール/デノイズはワーカー経由 TRT で動いている。
    pub trt_worker_active: bool,
    /// 起動時にフォールバックが起きた場合の理由 (UI バナー表示用)
    pub current_runtime_fallback_reason: Option<String>,
    /// TRT pack インストール済みか
    pub trt_pack_installed: bool,
    /// TRT pack の総ディスク使用量 (MiB)
    pub trt_pack_size_mib: u64,
    /// TRT engine cache の総ディスク使用量 (MiB)
    pub trt_engine_cache_size_mib: u64,
    /// 「TensorRT パックをダウンロード」ボタンが押されたか。
    /// 環境設定の Apply/Cancel 後に App 側で読み取って TRT install dialog を開く。
    pub start_trt_install_requested: bool,
    /// エンジンキャッシュ削除の確認ダイアログ表示中フラグ (Codex P3-2)。
    pub trt_cache_delete_confirm_open: bool,
    /// 「TensorRT パックを削除」ボタンで実行された pack 全体削除をリクエスト。
    /// dialog 内では state 更新だけ行い、実際の worker pool detach + ファイル削除は
    /// App 側で Preferences ウィンドウ closure 抜けた後に処理する
    /// (= worker pool が DLL を握ったままだと remove_dir_all が失敗するため)。
    pub uninstall_trt_pack_requested: bool,

    // ── VST3 プラグイン編集 ────────────────────────────────────────
    /// 環境設定を開いた時点でスキャンされていた VST3 プラグイン候補のスナップショット。
    /// VST3 ページ内でスキャン / 再スキャンで更新する。Apply 後に App 側に反映する。
    #[cfg(windows)]
    pub vst3_discovered: Vec<crate::video::dsp::DiscoveredPlugin>,
    /// VST3 scan/probe worker の完了通知。bridge subprocess で bus probe するため
    /// UI thread では直接走らせない。
    #[cfg(windows)]
    pub vst3_scan_rx:
        Option<std::sync::mpsc::Receiver<Result<Vec<crate::video::dsp::DiscoveredPlugin>, String>>>,
    #[cfg(windows)]
    pub vst3_scan_in_progress: bool,
    #[cfg(windows)]
    pub vst3_scan_error: Option<String>,
    /// VST3 ページ内のフィルタ文字列。
    pub vst3_filter: String,
    /// 音声入力を持たない plugin (= Instrument / MIDI FX 等) も候補一覧に表示する。
    #[cfg(windows)]
    pub vst3_show_unusable: bool,
    /// 現在 auto-bypass されているスロットのスナップショット (= 名前, latency_ms)。
    /// VST3 ページ下部の赤字警告表示用。`show_preferences_dialog` の頭で
    /// `dsp_bridge` から毎フレーム refresh される (= ON/OFF が即座に反映)。
    #[cfg(windows)]
    pub vst3_auto_bypassed: Vec<(String, f64)>,
}

impl PreferencesState {
    pub(crate) fn from_settings(
        s: &Settings,
        ai_runtime: Option<&crate::ai::runtime::AiRuntime>,
    ) -> Self {
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

        // AI バックエンドページ用の情報を 1 回だけ取得 (環境設定ダイアログを開いた時点)
        let gpu_vendor = crate::gpu_info::query_primary_gpu_vendor();
        let (current_runtime_fallback_reason, trt_worker_active) = match ai_runtime {
            Some(rt) => (
                rt.active_backend().fallback_reason.clone(),
                rt.has_worker_pool(),
            ),
            None => (None, false),
        };
        let trt_pack_installed = crate::ai::tensorrt_pack::is_pack_installed();
        let trt_pack_size_mib = if trt_pack_installed {
            dir_size_bytes(&crate::ai::tensorrt_pack::pack_dir()) / (1024 * 1024)
        } else {
            0
        };
        let trt_engine_cache_size_mib =
            dir_size_bytes(&crate::ai::tensorrt_pack::engine_cache_dir()) / (1024 * 1024);

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
            gpu_vendor,
            trt_worker_active,
            current_runtime_fallback_reason,
            trt_pack_installed,
            trt_pack_size_mib,
            trt_engine_cache_size_mib,
            start_trt_install_requested: false,
            trt_cache_delete_confirm_open: false,
            uninstall_trt_pack_requested: false,
            #[cfg(windows)]
            vst3_discovered: Vec::new(),
            #[cfg(windows)]
            vst3_scan_rx: None,
            #[cfg(windows)]
            vst3_scan_in_progress: false,
            #[cfg(windows)]
            vst3_scan_error: None,
            vst3_filter: String::new(),
            #[cfg(windows)]
            vst3_show_unusable: false,
            #[cfg(windows)]
            vst3_auto_bypassed: Vec::new(),
        }
    }
}

/// 指定ディレクトリ配下のファイル合計サイズを返す。エラーや不在は 0。
/// AI バックエンドページの "X MiB を解放" 表示等で使う。
fn dir_size_bytes(dir: &std::path::Path) -> u64 {
    fn walk(p: &std::path::Path) -> u64 {
        let Ok(meta) = std::fs::metadata(p) else {
            return 0;
        };
        if meta.is_file() {
            return meta.len();
        }
        let mut total = 0u64;
        if let Ok(entries) = std::fs::read_dir(p) {
            for e in entries.flatten() {
                total = total.saturating_add(walk(&e.path()));
            }
        }
        total
    }
    walk(dir)
}

// ── メインダイアログ ────────────────────────────────────────────

impl App {
    pub(crate) fn show_preferences_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_preferences {
            return;
        }

        // 初回: 一時コピーを作成
        if self.pref_state.is_none() {
            #[cfg_attr(not(windows), allow(unused_mut))]
            let mut new_state =
                PreferencesState::from_settings(&self.settings, self.ai_runtime.as_deref());
            // 既にスキャン済みの VST3 プラグイン候補を引き継ぐ (= 再スキャン不要で表示)
            #[cfg(windows)]
            {
                new_state.vst3_discovered = self.vst3_discovered.clone();
            }
            self.pref_state = Some(new_state);
        }

        // 毎フレーム refresh: auto-bypass されたスロットを取得して PreferencesState に反映。
        // VST3 ページ下部の赤字警告表示用。最新状態を見せたいので毎フレーム更新する
        // (= ユーザーが手動で再 ON した瞬間に警告が消える等、即時反映)。
        #[cfg(windows)]
        if let Some(state) = self.pref_state.as_mut() {
            let sample_rate = self.dsp_bridge.sample_rate();
            state.vst3_auto_bypassed = self
                .dsp_bridge
                .slots()
                .into_iter()
                .filter(|s| s.auto_bypassed_for_latency && s.bypass)
                .map(|s| {
                    let ms = if sample_rate > 0 {
                        s.latency_samples as f64 / sample_rate as f64 * 1000.0
                    } else {
                        0.0
                    };
                    (s.plugin_name.unwrap_or_else(|| "(不明)".to_string()), ms)
                })
                .collect();
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
                        // (note: 「TRT 全エンジンビルド」ボタンのフラグは下のブロックで処理する)
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });

        if apply {
            if let Some(mut state) = self.pref_state.take() {
                let old_dup = (
                    self.settings.skip_zip_if_folder_exists,
                    self.settings.skip_image_if_video_exists,
                    self.settings.skip_duplicate_images,
                    self.settings.image_ext_priority.clone(),
                    self.settings.video_thumb_use_sidecar_image,
                );
                let old_exif = self.settings.exif_hidden_tags.clone();

                let old_susie = (
                    self.settings.susie_enabled,
                    self.settings.susie_allow_parallel,
                );

                let old_pause_minimized = self.settings.pause_indexer_while_minimized;

                // AI バックエンド設定変更を検出してホットリロードトリガに使う
                let old_ai_backend = self.settings.ai_backend.clone();
                let new_ai_backend = state.settings.ai_backend.clone();

                // VST3 enable 状態 + チェーン構成の変化を検出してホットリロード。
                let old_vst3_enabled = self.settings.vst3_enabled;
                let new_vst3_enabled = state.settings.vst3_enabled;
                #[cfg(windows)]
                let old_vst3_chain: Vec<String> = self
                    .settings
                    .vst3_plugins
                    .iter()
                    .map(|e| e.path.clone())
                    .collect();
                #[cfg(windows)]
                let new_vst3_chain: Vec<String> = state
                    .settings
                    .vst3_plugins
                    .iter()
                    .map(|e| e.path.clone())
                    .collect();
                // VST3 ページで再スキャンした候補を App 側に反映
                #[cfg(windows)]
                let new_vst3_discovered = state.vst3_discovered.clone();

                // ダイアログを開いた時点の `state.settings` は self.settings の snapshot。
                // 開いている間に他ダイアログ (お気に入り編集 / タグ編集 / 補正プリセット /
                // 開いたアプリ履歴等) や runtime (ツールバー選択 / ウィンドウ移動 / レーティング
                // フィルタ) が変えたフィールドは state.settings 側に反映されないため、そのまま
                // self.settings = state.settings を実行するとそれらの変更が消える。
                // 対策: 環境設定が管理しないフィールドを self.settings の最新値で state に
                // 移送してから全体差し替えする。新しく「環境設定 UI から触らない」フィールドを
                // Settings に追加した場合はここにも追記が必要。
                state
                    .settings
                    .overwrite_non_preferences_from(&mut self.settings);

                self.settings = state.settings;
                self.settings.save();

                // 「常駐中はインデックス更新を一時停止する」が変わったらトレイの checkmark も
                // 同期する (お気に入り編集ダイアログと同じチェックボックス項目への二重経路)。
                if old_pause_minimized != self.settings.pause_indexer_while_minimized {
                    self.sync_tray_pause_check();
                }

                // AI バックエンド変更のホットリロード処理 (Phase 3)
                if old_ai_backend != new_ai_backend {
                    self.apply_ai_backend_change(new_ai_backend.as_deref());
                }

                // 同名ファイル設定が変更された場合はフォルダを再読み込み
                // (Phase 5.3: video_thumb_use_sidecar_image も含む — 切り替え時に
                // video_thumb_overrides を作り直すため)
                let new_dup = (
                    self.settings.skip_zip_if_folder_exists,
                    self.settings.skip_image_if_video_exists,
                    self.settings.skip_duplicate_images,
                    self.settings.image_ext_priority.clone(),
                    self.settings.video_thumb_use_sidecar_image,
                );
                if old_dup != new_dup {
                    self.reload_current_folder_preserving_override();
                }
                if old_exif != self.settings.exif_hidden_tags {
                    self.exif_cache.clear();
                }

                // VST3 enable 状態 / チェーン構成の変化反映
                #[cfg(windows)]
                {
                    self.vst3_discovered = new_vst3_discovered;
                    let chain_changed = old_vst3_chain != new_vst3_chain;
                    if old_vst3_enabled != new_vst3_enabled {
                        if new_vst3_enabled {
                            self.kick_off_vst3_chain_rebuild();
                        } else {
                            // VST3 OFF へのトグル: bridge teardown 前に内部状態と
                            // ウィンドウ位置を保存 (= 次回 ON 時の復元用)。
                            let states = self.snapshot_vst3_states_into_settings();
                            let positions = self.snapshot_vst3_window_positions_into_settings();
                            if states > 0 || positions > 0 {
                                self.settings.save();
                            }
                            self.dsp_bridge.disable();
                        }
                    } else if chain_changed && new_vst3_enabled {
                        self.kick_off_vst3_chain_rebuild();
                    }
                }
                #[cfg(not(windows))]
                let _ = (old_vst3_enabled, new_vst3_enabled);

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

        // 「TensorRT パックをダウンロード」ボタンが押されていたら、環境設定ダイアログを
        // 閉じて TRT install dialog を開く (= ユーザーは設定変更を保存しなくてもインストール
        // フローへ進める。完了後の TensorRT 有効化は再起動 + AiBackend 設定で行う想定)。
        if let Some(ps) = self.pref_state.as_mut() {
            if ps.start_trt_install_requested {
                ps.start_trt_install_requested = false;
                self.pref_state = None;
                self.show_preferences = false;
                let target_sm = crate::gpu_info::query_primary_gpu_sm();
                self.trt_install_state = Some(
                    crate::ui_dialogs::trt_install::TrtInstallState::new(target_sm),
                );
            }
        }

        // 「TensorRT パックを削除」ボタンが確定されていたら、worker pool を停止 →
        // ファイル削除 → live settings を DirectML に切替 → save をまとめて実行。
        if let Some(ps) = self.pref_state.as_mut() {
            if ps.uninstall_trt_pack_requested {
                ps.uninstall_trt_pack_requested = false;
                self.uninstall_trt_pack_now();
            }
        }
    }

    /// TRT パック削除フロー本体。
    /// - 走行中の worker pool を detach (= 子プロセス停止 + DLL ハンドル解放、UI thread)
    /// - live `settings.ai_backend` を DirectML に切替して save (UI thread、瞬時)
    /// - `tensorrt/` と `tensorrt-engines/` を **背景 thread で** 削除 (= 多 GB の I/O で
    ///   UI thread をブロックしないため Codex P2 指摘)。削除完了は logger に出力するのみ
    ///   (UI 状態は呼び出し時点で既に「削除済」相当に同期されている)。
    pub(crate) fn uninstall_trt_pack_now(&mut self) {
        // 1. worker pool 停止 (DLL ハンドル解放)。これは速い (ms オーダー) ので UI thread で OK。
        if let Some(runtime) = self.ai_runtime.as_ref() {
            if runtime.has_worker_pool() {
                crate::logger::log(
                    "[AI] TRT パック削除のため worker pool を停止します".to_string(),
                );
                runtime.detach_worker_pool();
            }
        }

        // 2. live settings を DirectML に固定 → save (UI thread、瞬時)
        let was_trt =
            self.settings.ai_backend.as_deref() == Some(crate::ai::AiBackend::TensorRt.as_str());
        if was_trt {
            self.settings.ai_backend = Some(crate::ai::AiBackend::DirectMl.as_str().to_string());
            self.settings.save();
            crate::logger::log(
                "[AI] AI バックエンドを DirectML に切替しました (TRT パック削除後)".to_string(),
            );
        }

        // 3. ファイル削除は背景 thread に逃がす。
        //    pack ~2 GB + engine cache 数百 MB の remove_dir_all は秒〜十数秒かかり、
        //    UI を凍らせるため (CLAUDE.md: UI thread 同期 I/O 禁止)。
        //    削除完了前に再 install を要求した場合、install 側の atomic rename + INSTALL_OK
        //    最終書き込みパターンで上書きできるので race は実害なし。
        let pack_dir = crate::ai::tensorrt_pack::pack_dir();
        let engine_cache_dir = crate::ai::tensorrt_pack::engine_cache_dir();
        std::thread::Builder::new()
            .name("trt-pack-uninstall".to_string())
            .spawn(move || {
                for dir in [pack_dir, engine_cache_dir] {
                    match std::fs::remove_dir_all(&dir) {
                        Ok(()) => {
                            crate::logger::log(format!(
                                "[AI] TensorRT を削除しました: {}",
                                dir.display()
                            ));
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            crate::logger::log(format!(
                                "[AI] TensorRT 削除に失敗: {} ({e})",
                                dir.display()
                            ));
                        }
                    }
                }
            })
            .ok();
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
        PreferencesPage::AiBackend => page_ai_backend(ui, state),
        PreferencesPage::Cache => page_cache(ui, state),
        PreferencesPage::Folder => page_folder(ui, state),
        PreferencesPage::DuplicateFiles => page_duplicate_files(ui, state),
        PreferencesPage::ExifDisplay => page_exif_display(ui, state, enter_pressed),
        PreferencesPage::SpreadMode => page_spread_mode(ui, state),
        PreferencesPage::SusiePlugins => page_susie_plugins(ui, state),
        PreferencesPage::IndexerSpeed => page_indexer_speed(ui, state),
        PreferencesPage::TrayResidency => page_tray_residency(ui, state),
        PreferencesPage::Rating => page_rating(ui, state),
        PreferencesPage::UpdateCheck => page_update_check(ui, state),
        PreferencesPage::Video => page_video(ui, state),
        PreferencesPage::Vst3 => page_vst3(ui, state),
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
         セクション内の全項目を外すとセクション自体が非表示になります。\n\
         上から順に画面のツールバー左端 → 右端に対応します。",
    );
    ui.add_space(6.0);

    // Phase 6.E: 設定の表示順を画面上のツールバーの左→右の並びに揃える。
    // 画面の順:
    //   上のフォルダ ⬆ → 前 ▲ → 次 ▼ → 列 → 比率 → ソート → ★ → お気に入り
    //   → タグ → (フォルダアドレスバー、別位置)

    ui.checkbox(
        &mut s.show_toolbar_parent_button,
        "上のフォルダへ (⬆ ボタン)",
    );
    ui.checkbox(
        &mut s.show_toolbar_prev_folder,
        "前のフォルダへ (▲ ボタン、Ctrl+↑ と同じ動作)",
    );
    ui.checkbox(
        &mut s.show_toolbar_next_folder,
        "次のフォルダへ (▼ ボタン、Ctrl+↓ と同じ動作)",
    );
    // (旧) VST3 ツールバーボタン: v0.9.0 開発中に削除。動画再生中はホバーバーから
    // パネルを開く運用に統一。settings の `show_toolbar_vst3` は legacy フラグとして残るが
    // 動作には影響しない。
    let _ = &mut s.show_toolbar_vst3; // 未使用警告抑制

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

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(2.0);
    ui.checkbox(&mut s.show_toolbar_rating, "レーティング (★ フィルタ)");
    ui.checkbox(&mut s.show_toolbar_favorites, "お気に入り");
    ui.checkbox(&mut s.show_toolbar_tags, "タグ");

    ui.add_space(6.0);
    ui.separator();
    ui.add_space(2.0);
    // フォルダ (アドレスバー) は他のツールバーセクションと別位置 (= ツールバー
    // とは別のアドレスバー帯) に出るので、最後にまとめて表示。
    ui.checkbox(
        &mut s.show_toolbar_folder,
        "フォルダ (アドレスバー、別の場所に表示)",
    );
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

/// AI 推論バックエンド (DirectML / TensorRT) の選択ページ。
///
/// Phase 3 アーキテクチャ:
/// - メイン: 常に DirectML
/// - TensorRT 有効化: 別プロセスのワーカーが起動して、Upscale/Denoise を担当
/// - 切り替えはホットリロードでアプリ再起動不要
fn page_ai_backend(ui: &mut egui::Ui, state: &mut PreferencesState) {
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
fn page_rating(ui: &mut egui::Ui, state: &mut PreferencesState) {
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
fn page_update_check(ui: &mut egui::Ui, state: &mut PreferencesState) {
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

fn page_video(ui: &mut egui::Ui, state: &mut PreferencesState) {
    let s = &mut state.settings;

    ui.label(egui::RichText::new("ハードウェアデコード").strong());
    ui.add_space(4.0);
    ui.label(
        "GPU の動画デコード機能 (Direct3D 11) を使って HEVC / 4K 動画の CPU 負荷を下げます。\n\
         ドライバ非対応や初期化失敗の場合は自動的に CPU デコードに切り替わります。",
    );
    ui.add_space(6.0);
    ui.checkbox(&mut s.video_hw_decode, "ハードウェアデコードを有効にする")
        .on_hover_text(
            "ON: 対応コーデックは GPU でデコード (失敗時は CPU に自動フォールバック)。\n\
         OFF (既定): 常に CPU でデコード。\n\
         切り替え後は次に開く動画から反映されます。",
        );

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);

    ui.label(egui::RichText::new("再生").strong());
    ui.add_space(4.0);
    // Phase 7.J: 自動再生 3 モード (Off / OnlyFromGrid / Always)。
    // 旧 video_autoplay (bool) も内部で migration 用に保持。
    ui.label("自動再生:");
    use crate::settings::VideoAutoplayMode;
    for &mode in VideoAutoplayMode::all() {
        if ui
            .radio_value(&mut s.video_autoplay_mode, mode, mode.label())
            .changed()
        {
            // 旧 bool 設定は新モードと矛盾しないように同期しておく
            // (例: ユーザーが新 UI で Always を選んだら video_autoplay=true、それ以外は
            // false。古いコードを誤読したときの不整合を防ぐ)。
            s.video_autoplay = matches!(mode, VideoAutoplayMode::Always);
        }
    }
    ui.add_space(4.0);
    ui.checkbox(&mut s.video_loop, "終端まで再生したら最初から繰り返す");
    ui.checkbox(&mut s.video_start_muted, "起動直後はミュートで開始");

    ui.add_space(8.0);
    let mut vol_pct = (s.video_volume * 100.0).round() as i32;
    if ui
        .add(
            egui::Slider::new(&mut vol_pct, 0..=100)
                .text("既定音量 (%)")
                .clamping(egui::SliderClamping::Always),
        )
        .changed()
    {
        s.video_volume = (vol_pct as f64 / 100.0).clamp(0.0, 1.0);
    }

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);

    ui.label(egui::RichText::new("再生位置の記憶").strong());
    ui.add_space(4.0);
    let count = s.video_resume_positions.len();
    ui.label(format!(
        "現在 {count} 件の動画について再生位置を記憶しています。\n\
         3 秒以上再生・かつ末尾 5 秒以内に到達していない場合のみ保存されます。"
    ));
    if count > 0 && ui.button("すべての再生位置をクリア").clicked() {
        s.video_resume_positions.clear();
    }

    ui.add_space(12.0);
    ui.separator();
    ui.add_space(8.0);

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

    // VST3 プラグイン処理は専用ページ "VST3 プラグイン" に分離した (= ユーザー要望
    // 「環境設定の中に新しい項目」)。動画タブには出さない。
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
fn page_vst3(ui: &mut egui::Ui, state: &mut PreferencesState) {
    use crate::settings::Vst3PluginEntry;

    const MAX_CHAIN_LEN: usize = 10;

    if let Some(rx) = state.vst3_scan_rx.as_ref() {
        match rx.try_recv() {
            Ok(Ok(found)) => {
                state.vst3_discovered = found;
                state.vst3_scan_rx = None;
                state.vst3_scan_in_progress = false;
                state.vst3_scan_error = None;
            }
            Ok(Err(err)) => {
                state.vst3_scan_rx = None;
                state.vst3_scan_in_progress = false;
                state.vst3_scan_error = Some(err);
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                state.vst3_scan_rx = None;
                state.vst3_scan_in_progress = false;
                state.vst3_scan_error = Some("VST3 scan worker が終了しました".to_string());
            }
        }
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
            "スキャン中..."
        } else if state.vst3_discovered.is_empty() {
            "プラグインをスキャン"
        } else {
            "再スキャン"
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
            if let Err(e) = std::thread::Builder::new()
                .name("vst3-scan-probe".into())
                .spawn(move || {
                    let roots = crate::video::dsp::default_vst3_paths();
                    let result = crate::video::dsp::scan_with_audio_probe(&roots);
                    let _ = tx.send(result);
                })
            {
                state.vst3_scan_rx = None;
                state.vst3_scan_in_progress = false;
                state.vst3_scan_error = Some(format!("scan worker 起動失敗: {e}"));
            }
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
fn page_vst3(_ui: &mut egui::Ui, _state: &mut PreferencesState) {}

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
