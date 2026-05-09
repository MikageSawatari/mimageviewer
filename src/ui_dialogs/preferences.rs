//! 統合環境設定ダイアログ。
//!
//! 左側にツリー、右側に設定パネルを配置した環境設定ウィンドウ。
//! OK / キャンセルで一時コピーを確定 or 破棄する。

use eframe::egui;
use std::collections::HashSet;

use crate::app::App;
use crate::settings::{Parallelism, Settings};

mod pages;
use self::pages::*;

#[cfg(windows)]
pub enum Vst3ScanMessage {
    Progress {
        done: usize,
        total: usize,
        path: String,
    },
    Finished(Result<Vec<crate::video::dsp::DiscoveredPlugin>, String>),
}

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
    pub vst3_scan_rx: Option<std::sync::mpsc::Receiver<Vst3ScanMessage>>,
    #[cfg(windows)]
    pub vst3_scan_in_progress: bool,
    #[cfg(windows)]
    pub vst3_scan_error: Option<String>,
    #[cfg(windows)]
    pub vst3_scan_done: usize,
    #[cfg(windows)]
    pub vst3_scan_total: usize,
    #[cfg(windows)]
    pub vst3_scan_current: String,
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
            #[cfg(windows)]
            vst3_scan_done: 0,
            #[cfg(windows)]
            vst3_scan_total: 0,
            #[cfg(windows)]
            vst3_scan_current: String::new(),
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

// 個別ページ実装は `preferences/pages.rs` に分離。
