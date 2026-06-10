//! フルスクリーン表示のレンダリング。
//!
//! `App::update()` から呼ばれる `render_fullscreen_viewport()` を実装する。
//! 元は `update()` 内にインラインで書かれていた ~460 行を独立メソッドに切り出したもの。
//!
//! # ⚠️ 動画 UI の責任分担に注意
//!
//! - **動画フルスクリーン中の HUD・音量スライダ・seek bar・ミュート・top bar・
//!   メタデータパネル・ノーマライズボタン等の playback controls は、このファイルではなく
//!   [`crate::video::native_presenter`] (egui overlay) が描画する**
//! - このファイルが現役で担当する動画関連は:
//!   - 動画の **エラー表示** と **「動画を準備中…」スピナー**
//!     ([`crate::App::draw_video_hud`])
//! - 動画 UI を変更したい場合は必ず [docs/video-architecture.md] と
//!   [src/video/native_presenter/mod.rs] を読むこと
//!
//! 過去にこのファイル内 (line 6800 付近) に動画 HUD コードが書かれていた経緯があるが、
//! v0.9.0 で native presenter に移行済。新規追加 / 移植時に「ui_fullscreen.rs を見て
//! しまう」誤認を避けるため明示。

use eframe::egui;
use std::sync::Arc;

use crate::app::App;
use crate::fs_animation::FsCacheEntry;
use crate::grid_item::{GridItem, ThumbnailState};
use crate::keymap::KeyAction;
use crate::pdf_loader::PdfPageContentType;
use crate::settings::{FullscreenFitMode, ReadingDirection, ReadingFlow, SpreadMode};
use crate::ui_helpers::{HoverTipExt, open_external_player};

#[derive(Clone, Copy)]
pub(crate) struct FullscreenCursorState {
    last_activity: Option<std::time::Instant>,
    hidden: bool,
}

pub(crate) mod draw_icons;
use self::draw_icons::*;

// ── 定数 ────────────────────────────────────────────────────────────────

/// メタデータパネルの最大幅
const METADATA_PANEL_WIDTH: f32 = 380.0;
/// ホバー時トップバーの高さ
pub(crate) const TOP_BAR_HEIGHT: f32 = 44.0;
/// 静止画フルスクリーン下端のページシークバーの高さ。下端まで伸びる左右パネル
/// (補正パネル / メタデータパネル) はこの分だけ下端を空けてシークバーと重ならない
/// ようにする (`draw_fullscreen_seek_overlay` の panel_rect と同じ高さ)。
pub(crate) const FS_SEEK_BAR_HEIGHT: f32 = 38.0;
/// ホイール感度（raw_scroll_delta の除数）
const WHEEL_SENSITIVITY: f32 = 30.0;

fn should_handle_fullscreen_wheel(
    cursor_in_panel: bool,
    in_video_tile: bool,
    ctrl_held: bool,
    modal_for_keys: bool,
    // 表示モード / フィットのポップアップメニュー表示中は、メニュー上でも画像上でも
    // ホイールを背後のページ送り・連結スクロール・ズームへ流さない (modal と同様に全抑制)。
    popup_open: bool,
) -> bool {
    !modal_for_keys
        && !popup_open
        && (!in_video_tile || !ctrl_held)
        && (ctrl_held || !cursor_in_panel)
}

fn should_zoom_fullscreen_wheel(ctrl_held: bool, overlay_edit_mode: bool) -> bool {
    ctrl_held || overlay_edit_mode
}

/// 中ボタンドラッグズーム: 縦 N px で倍率 2 倍/半分になる感度 (v0.8.1)。
/// 100 px で 2 倍 (= 上へ 200 px で 4 倍)。ホイール 1 ノッチ ≈ 10% と比べて粗めだが、
/// 縦フル (1080 px) ストロークすれば 2^10 ≈ 1000 倍まで届くので十分。
const MIDDLE_DRAG_UNIT_PX: f32 = 100.0;
/// 中ボタン押下から「ドラッグ開始」とみなす最小移動量。
/// この距離以下ならズームは触らない (クリックのみとの区別 / 暴発防止)。
const MIDDLE_DRAG_THRESHOLD_PX: f32 = 4.0;
/// ズーム倍率の下限
const ZOOM_MIN: f32 = 0.1;
/// ズーム倍率の上限
const ZOOM_MAX: f32 = 50.0;
/// 連結読み: ホイール 1 ノッチ相当として扱う raw scroll delta。
const CONTINUOUS_READING_WHEEL_REFERENCE_DELTA: f32 = 120.0;
/// 連結読み: PageUp/PageDown で進む画面長の割合。
const VERTICAL_READING_PAGE_SCROLL_FRAC: f32 = 0.85;
/// 連結読み: 同時に画面に入るページ数の上限。
const VERTICAL_READING_MAX_VISIBLE_PAGES: usize = 16;
/// 連結読み: 可視範囲の前後に保持する先読みページ数。
const VERTICAL_READING_PREFETCH_PAD: usize = 2;
/// 連結読み: fs_cache に残すページ数上限。
const VERTICAL_READING_MAX_CACHE_PAGES: usize = 20;
/// 連結読み: fs_cache に残すテクスチャの推定総ピクセル数上限。
const VERTICAL_READING_MAX_CACHE_TEXELS: usize = 320_000_000;
/// 連結読み: final composite / comic 合成を新規生成するページ数の 1 フレーム上限。
const VERTICAL_READING_PROCESSED_UPLOADS_PER_FRAME: usize = 1;
/// ズームが 1.0 とみなせるしきい値
const ZOOM_NEAR_ONE: f32 = 1.001;
/// 回転・パンがゼロとみなせるしきい値
const TRANSFORM_EPSILON: f32 = 0.001;
/// パンがゼロとみなせるしきい値（length_sq）
const PAN_EPSILON_SQ: f32 = 0.25;
/// バー内ボタンのサイズ
const BAR_BUTTON_SIZE: f32 = 32.0;
/// バー内ボタンの上下マージン
const BAR_BUTTON_MARGIN: f32 = 6.0;
/// バー内ボタン間の隙間
const BAR_BUTTON_GAP: f32 = 4.0;
/// チェックマーク円の半径
const CHECKMARK_RADIUS: f32 = 18.0;
/// 透過画像背景の市松 1 タイルサイズ (px)
const CHECKER_TILE_PX: f32 = 16.0;
/// ピクセル境界グリッドを表示し始める 1 画像ピクセルあたりの画面ピクセル数。
const PIXEL_GRID_MIN_CELL_PHYSICAL_PX: f32 = 8.0;
/// safety: 異常な変換で大量線分を積まないための上限。
const PIXEL_GRID_MAX_LINES: usize = 5000;

#[derive(Clone, Copy)]
enum ComparePreparedTextureKind {
    Pinned,
    Current,
    Diff,
}

struct ExportDialogTarget {
    source: crate::export_dialog::ExportSource,
    source_label: String,
    original_format: crate::save_with_metadata::SrcFormat,
    source_dir: std::path::PathBuf,
    basename: String,
    pixels: crate::export_dialog::ExportPixels,
}

#[cfg(windows)]
struct NativeFocusClaim {
    foreground_hwnd: usize,
    post_foreground_hwnd: usize,
    target_hwnd: usize,
    set_foreground_ok: bool,
    attach_thread_input_ok: bool,
    set_active_ok: bool,
    set_focus_ok: bool,
}

#[cfg(windows)]
struct NativeFocusTarget {
    foreground_hwnd: usize,
    target_hwnd: usize,
}

#[cfg(windows)]
fn native_window_under_cursor_focus_target() -> NativeFocusTarget {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::{
        GA_ROOT, GetAncestor, GetCursorPos, GetForegroundWindow, WindowFromPoint,
    };

    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err() {
            return NativeFocusTarget {
                foreground_hwnd: GetForegroundWindow().0 as usize,
                target_hwnd: 0,
            };
        }
        let hovered = WindowFromPoint(pt);
        let foreground = GetForegroundWindow();
        if hovered.0.is_null() {
            return NativeFocusTarget {
                foreground_hwnd: foreground.0 as usize,
                target_hwnd: 0,
            };
        }
        let root = GetAncestor(hovered, GA_ROOT);
        let target = if root.0.is_null() { hovered } else { root };
        NativeFocusTarget {
            foreground_hwnd: foreground.0 as usize,
            target_hwnd: target.0 as usize,
        }
    }
}

#[cfg(windows)]
fn claim_native_window_focus(target_hwnd: usize) -> NativeFocusClaim {
    let report = crate::video::native_window::claim_foreground(target_hwnd as u64);
    NativeFocusClaim {
        foreground_hwnd: report.foreground_hwnd as usize,
        post_foreground_hwnd: report.post_foreground_hwnd as usize,
        target_hwnd: report.target_hwnd as usize,
        set_foreground_ok: report.set_foreground_ok,
        attach_thread_input_ok: report.attach_thread_input_ok,
        set_active_ok: report.set_active_ok,
        set_focus_ok: report.set_focus_ok,
    }
}

#[cfg(not(windows))]
struct NativeFocusClaim {
    foreground_hwnd: usize,
    post_foreground_hwnd: usize,
    target_hwnd: usize,
    set_foreground_ok: bool,
    attach_thread_input_ok: bool,
    set_active_ok: bool,
    set_focus_ok: bool,
}

#[cfg(not(windows))]
struct NativeFocusTarget {
    foreground_hwnd: usize,
    target_hwnd: usize,
}

#[cfg(not(windows))]
fn native_window_under_cursor_focus_target() -> NativeFocusTarget {
    NativeFocusTarget {
        foreground_hwnd: 0,
        target_hwnd: 0,
    }
}

#[cfg(not(windows))]
fn claim_native_window_focus(target_hwnd: usize) -> NativeFocusClaim {
    NativeFocusClaim {
        foreground_hwnd: 0,
        post_foreground_hwnd: 0,
        target_hwnd,
        set_foreground_ok: false,
        attach_thread_input_ok: false,
        set_active_ok: false,
        set_focus_ok: false,
    }
}

#[cfg(windows)]
fn current_foreground_hwnd() -> usize {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

    unsafe { GetForegroundWindow().0 as usize }
}

#[cfg(not(windows))]
fn current_foreground_hwnd() -> usize {
    0
}

#[cfg(windows)]
fn original_preview_shortcut_held(_ctx: &egui::Context) -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_RCONTROL};

    unsafe { GetAsyncKeyState(VK_RCONTROL.0 as i32) < 0 }
}

#[cfg(not(windows))]
fn original_preview_shortcut_held(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.key_down(egui::Key::Num0))
}

/// 物理的な Ctrl キー押下を OS から直接読む。フルスクリーンビューポートでは
/// `ctx.input(|i| i.modifiers.ctrl)` がキーフォーカス不在で stale (常に false) になり得る
/// ため、Ctrl 依存の挙動 (ソースプレビュー / 補正レイヤー境界筆の通常筆切替) はこれを使う。
#[cfg(windows)]
pub(crate) fn ctrl_held_via_os() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL};

    unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) < 0 }
}

#[cfg(not(windows))]
pub(crate) fn ctrl_held_via_os() -> bool {
    false
}

#[cfg(windows)]
fn shift_held_via_os() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_SHIFT};

    unsafe { GetAsyncKeyState(VK_SHIFT.0 as i32) < 0 }
}

#[cfg(not(windows))]
fn shift_held_via_os() -> bool {
    false
}

/// 静止画フルスクリーンで Enter キーをフルスクリーン解除トリガーとして消費すべきか
/// 判定する純関数 (副作用ゼロ)。
///
/// 設計意図: 「グリッドで Enter で開く → フルスクリーンで Enter で抜ける」のトグル
/// 動作を成立させる。Esc も同じ位置に居るので、Enter は Esc の **追加** 選択肢。
///
/// 除外条件 (= false を返す):
/// - 動画モード: Enter は `handle_video_input` で「再生/一時停止」として既に消費中
/// - IME 変換中: Enter は IME 確定キーなので奪わない
/// - フルスクリーン context menu 表示中: メニュー側の Enter 選択操作を優先
/// - グリッド Enter で open した直後の押下: `suppress_until_release` が立っている間は
///   抑止 (= 同フレーム内に残った Enter event を拾って即 close する事故を防ぐ)。
///   詳細は `fs_suppress_enter_close_until_release` フィールドの doc 参照。
///
/// サブモード (補正レイヤー/消しゴム/隠蔽/export crop) は caller 側で早期 return
/// 済みなのでここでは判定しない (= caller の責任)。
pub(crate) fn should_close_fullscreen_on_enter(
    is_video_item: bool,
    ime_active: bool,
    context_menu_open: bool,
    suppress_until_release: bool,
) -> bool {
    !is_video_item && !ime_active && !context_menu_open && !suppress_until_release
}

/// `local_adjust_mode` 中にどのテクスチャ経路を採用するかを表す純粋な決定型。
///
/// `resolve_fs_processed_texture` の補正レイヤー分岐で副作用 (OS API キー読み・
/// HashMap アクセス・worker spawn) を呼ぶ前の **判定だけ** を切り出すために用意した。
/// これにより A-3 saga で 3 回手戻りした「Ctrl+Shift vs `preview_to_selected_layer`
/// vs Ctrl のみ」の優先順位が unit test できる。
///
/// 経路 → 副作用 (caller がやる):
/// - `ShowSource`: `resolve_local_adjust_source_texture(ctx, idx)`
/// - `BypassLayer{layer_idx}`: `maybe_start_local_adjust_layer_bypass_preview` +
///   `current_local_adjust_layer_bypass_texture` → fallback to source
/// - `PrefixPreview{layer_count}`: `maybe_start_local_adjust_prefix_preview` +
///   `current_local_adjust_prefix_preview_texture` → fallback to source
/// - `FullComposite`: `current_local_adjust_texture` → fallback to source
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalAdjustPreviewAction {
    ShowSource,
    BypassLayer { layer_idx: usize },
    PrefixPreview { layer_count: usize },
    FullComposite,
}

/// 補正レイヤー描画経路の純粋な決定ロジック。
///
/// 副作用ゼロ。OS API (`ctrl_held_via_os` / `shift_held_via_os`) や App 状態の
/// 取り出しは caller の責任で、ここでは bool / Option / usize しか見ない。
///
/// 入力:
/// - `fs_prev_focused`: フルスクリーンが OS フォーカスを持っているか。false の場合
///   ctrl/shift の OS 読みは不安定なので 0 扱いにする (= ここで一括処理)
/// - `ctrl_held` / `shift_held`: OS から読んだ修飾キー状態
/// - `show_source_toggle`: パネルの「元画像を表示」トグル (= App::local_adjust_show_source)
/// - `preview_to_selected_layer_toggle`: パネルの「選択レイヤーまでプレビュー」
/// - `has_any_layer`: 当該ページに 1 つ以上レイヤーがあるか
/// - `selected_layer_idx`: 選択中レイヤーの idx (= App::selected_local_adjust_layer_idx)
/// - `total_layers`: ページのレイヤー数
pub(crate) fn decide_local_adjust_preview_action(
    fs_prev_focused: bool,
    ctrl_held: bool,
    shift_held: bool,
    show_source_toggle: bool,
    preview_to_selected_layer_toggle: bool,
    has_any_layer: bool,
    selected_layer_idx: Option<usize>,
    total_layers: usize,
) -> LocalAdjustPreviewAction {
    // フルスクリーン非フォーカス時は OS キー読みを採用しない (= 別アプリの
    // Ctrl 押下を誤検知しないため)。`original_preview_active` と同じガード。
    let (ctrl, shift) = if fs_prev_focused {
        (ctrl_held, shift_held)
    } else {
        (false, false)
    };
    let adjust_panel_active = has_any_layer;
    // Ctrl+Shift bypass はパネルにレイヤーがある時だけ意味を持つ (= バイパスする
    // 対象が無いと無意味)。adjust_panel_active=false なら bypass は不発で、
    // 下の ShowSource ゲートに落ちる。
    let modifier_bypass_active = adjust_panel_active && ctrl && shift;

    if !modifier_bypass_active && (ctrl || show_source_toggle) {
        return LocalAdjustPreviewAction::ShowSource;
    }

    let preview_requested = preview_to_selected_layer_toggle || modifier_bypass_active;
    if preview_requested && total_layers > 0 {
        if let Some(layer_idx) = selected_layer_idx {
            if modifier_bypass_active {
                let bypass_layer_idx = layer_idx.min(total_layers - 1);
                return LocalAdjustPreviewAction::BypassLayer {
                    layer_idx: bypass_layer_idx,
                };
            } else {
                // 「選択レイヤーまでプレビュー」: 先頭から N+1 枚を適用
                let layer_count = layer_idx.min(total_layers - 1) + 1;
                if layer_count < total_layers {
                    return LocalAdjustPreviewAction::PrefixPreview { layer_count };
                }
                // layer_count == total_layers → 通常の FullComposite と同じ結果なので
                // 専用の prefix preview worker を起動する意味がない (= fall through)
            }
        }
    }

    LocalAdjustPreviewAction::FullComposite
}

/// メインビューポート経由のフルスクリーンキー処理 (`handle_fullscreen_root_key_input`)
/// を起動するかどうかを判定する「プローブ」。押されたキーがこの集合に無いフレームは
/// 実ハンドラ (`handle_fs_key_input` → 動画は `handle_video_input`) を一切呼ばない。
///
/// ⚠️ 再発防止: フルスクリーンのショートカットキーを追加・変更したら **必ずここにも
/// 追加する**。漏らすと、専用フルスクリーン viewport ではなくメインウィンドウに
/// フォーカスがある経路 (in-window 動画再生など) でそのキーだけ無反応になる。
/// 2026-05 に perf オーバーレイを P→F へ移した際、プローブへの F 追加漏れで実害が出た。
fn is_fullscreen_shortcut_probe_key(key: egui::Key) -> bool {
    matches!(
        key,
        egui::Key::ArrowLeft
            | egui::Key::ArrowRight
            | egui::Key::ArrowUp
            | egui::Key::ArrowDown
            | egui::Key::Home
            | egui::Key::End
            | egui::Key::PageUp
            | egui::Key::PageDown
            | egui::Key::Num0
            | egui::Key::Num1
            | egui::Key::Num2
            | egui::Key::Num3
            | egui::Key::Num4
            | egui::Key::Num5
            | egui::Key::Num6
            | egui::Key::Num7
            | egui::Key::Num8
            | egui::Key::Num9
            | egui::Key::W
            | egui::Key::Enter
            | egui::Key::Escape
            | egui::Key::Backspace
            | egui::Key::Space
            | egui::Key::S
            | egui::Key::M
            | egui::Key::L
            | egui::Key::J
            | egui::Key::K
            | egui::Key::B
            | egui::Key::C
            | egui::Key::D
            | egui::Key::P
            | egui::Key::F
            | egui::Key::G
            | egui::Key::T
            | egui::Key::I
            | egui::Key::X
            | egui::Key::Z
            | egui::Key::Y
            | egui::Key::R
            | egui::Key::F1
            | egui::Key::F2
            | egui::Key::F3
            | egui::Key::F4
            | egui::Key::F5
            | egui::Key::F6
            | egui::Key::F11
    )
}

fn fullscreen_shortcut_event_summary(ctx: &egui::Context) -> Option<String> {
    let parts = ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|event| {
                if let egui::Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } = event
                    && is_fullscreen_shortcut_probe_key(*key)
                {
                    Some(format!(
                        "{:?}:{}{}:{:?}",
                        key,
                        if *pressed { "down" } else { "up" },
                        if *repeat { ":repeat" } else { "" },
                        modifiers
                    ))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    });
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(","))
    }
}

/// egui のキーを native 動画キーイベントの仮想キーコードへ変換する。起動直後の
/// 黒 backdrop 表示中 (presenter HWND 未確定) に egui 経由で来たキーを
/// `handle_native_video_key_event` へ渡すために使う。
///
/// ⚠️ 上の `is_fullscreen_shortcut_probe_key` と対で更新すること。動画フルスクリーンの
/// ショートカットを追加・変更したら、プローブとこの変換表の両方に足す必要がある。
#[cfg(windows)]
fn native_video_vk_from_egui_key(key: egui::Key) -> Option<u32> {
    Some(match key {
        egui::Key::Enter => 0x0D,
        egui::Key::Escape => 0x1B,
        egui::Key::Space => 0x20,
        egui::Key::End => 0x23,
        egui::Key::Home => 0x24,
        egui::Key::ArrowLeft => 0x25,
        egui::Key::ArrowUp => 0x26,
        egui::Key::ArrowRight => 0x27,
        egui::Key::ArrowDown => 0x28,
        egui::Key::B => 0x42,
        egui::Key::C => 0x43,
        egui::Key::F => 0x46,
        egui::Key::J => 0x4A,
        egui::Key::K => 0x4B,
        egui::Key::L => 0x4C,
        egui::Key::M => 0x4D,
        egui::Key::P => 0x50,
        egui::Key::S => 0x53,
        egui::Key::W => 0x57,
        egui::Key::X => 0x58,
        egui::Key::F1 => 0x70,
        egui::Key::F2 => 0x71,
        egui::Key::F3 => 0x72,
        egui::Key::F4 => 0x73,
        egui::Key::F5 => 0x74,
        egui::Key::F6 => 0x75,
        egui::Key::F11 => 0x7A,
        _ => return None,
    })
}

#[cfg(windows)]
fn native_video_key_events_from_ctx(
    ctx: &egui::Context,
) -> Vec<crate::video::native_window::NativeVideoKeyEvent> {
    ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|event| {
                if let egui::Event::Key {
                    key,
                    pressed: true,
                    repeat,
                    modifiers,
                    ..
                } = event
                {
                    native_video_vk_from_egui_key(*key).map(|virtual_key| {
                        crate::video::native_window::NativeVideoKeyEvent {
                            virtual_key,
                            shift: modifiers.shift,
                            ctrl: modifiers.ctrl,
                            alt: modifiers.alt,
                            repeat: *repeat,
                        }
                    })
                } else {
                    None
                }
            })
            .collect()
    })
}

/// 補正ショートカット (U/T/N) のスコープ。どの層を書き換えるかを表す。
/// 解決 (`App::resolve_adjust_scope`) と書き込み (`App::write_params_for_scope`) は
/// App 側にメソッドとして実装され、ここには enum 定義とラベルだけ置く。
#[derive(Clone, Copy)]
pub(crate) enum AdjustScope {
    PageOverride,
    FavoriteDefault(uuid::Uuid),
    Global,
}

impl AdjustScope {
    /// トースト表示用ラベル。
    #[inline]
    pub(crate) fn label(self) -> &'static str {
        match self {
            AdjustScope::PageOverride => "個別",
            AdjustScope::FavoriteDefault(_) => "お気に入り",
            AdjustScope::Global => "標準",
        }
    }
}

/// 見開き描画時に書き出されるページ矩形レイアウト。
/// ルーペ描画がカーソル位置からどちらのページかを判定し、UV サンプリングに使う。
#[derive(Clone, Copy)]
pub struct FsSpreadLayout {
    pub left_idx: usize,
    pub left_rect: egui::Rect,
    pub right_idx: usize,
    pub right_rect: egui::Rect,
}

#[derive(Clone, Copy, Debug)]
struct VerticalReadingPage {
    idx: usize,
    rect: egui::Rect,
}

#[derive(Clone, Debug)]
struct VerticalReadingSeparator {
    text: String,
    rect: egui::Rect,
}

#[derive(Clone, Debug)]
struct ContinuousReadingUnitSpec {
    anchor_idx: usize,
    pages: Vec<usize>,
    separator_text: Option<String>,
}

impl ContinuousReadingUnitSpec {
    fn pages(anchor_idx: usize, pages: Vec<usize>) -> Self {
        Self {
            anchor_idx,
            pages,
            separator_text: None,
        }
    }

    fn separator(anchor_idx: usize, text: String) -> Self {
        Self {
            anchor_idx,
            pages: Vec::new(),
            separator_text: Some(text),
        }
    }

    fn contains_idx(&self, idx: usize) -> bool {
        self.anchor_idx == idx || self.pages.contains(&idx)
    }
}

#[derive(Clone, Debug)]
struct ContinuousReadingPageSize {
    idx: usize,
    width: f32,
    height: f32,
}

#[derive(Clone, Debug)]
struct ContinuousReadingUnitSize {
    pages: Vec<ContinuousReadingPageSize>,
    width: f32,
    height: f32,
    page_gap: f32,
}

fn continuous_spread_fit_width(
    page_count: usize,
    base_width: f32,
    spread_mode: SpreadMode,
    flow: ReadingFlow,
    fit_mode: FullscreenFitMode,
    spread_gap: f32,
) -> (f32, f32) {
    if page_count == 1
        && spread_mode.is_spread()
        && flow.is_vertical()
        && matches!(fit_mode, FullscreenFitMode::Width)
    {
        ((base_width * 2.0).max(1.0), spread_gap.max(0.0))
    } else {
        (
            base_width.max(1.0),
            spread_gap.max(0.0) * page_count.saturating_sub(1) as f32,
        )
    }
}

fn continuous_reading_page_rects(
    unit_rect: egui::Rect,
    size: &ContinuousReadingUnitSize,
) -> Vec<(usize, egui::Rect)> {
    let mut rects = Vec::with_capacity(size.pages.len());
    let mut x = unit_rect.min.x;
    for page in &size.pages {
        let rect = if size.pages.len() == 1 {
            egui::Rect::from_center_size(unit_rect.center(), egui::vec2(page.width, page.height))
        } else {
            let rect = egui::Rect::from_min_size(
                egui::pos2(x, unit_rect.min.y),
                egui::vec2(page.width, page.height),
            );
            x += page.width + size.page_gap;
            rect
        };
        rects.push((page.idx, rect));
    }
    rects
}

fn continuous_separator_base_size(_flow: ReadingFlow, fallback: egui::Vec2) -> egui::Vec2 {
    egui::vec2(fallback.x.max(1.0), fallback.y.max(1.0))
}

fn nearest_continuous_page_unit_size(
    units: &[ContinuousReadingUnitSpec],
    sizes: &[ContinuousReadingUnitSize],
    pos: usize,
) -> Option<(f32, f32)> {
    for prev in (0..pos).rev() {
        if units.get(prev).is_some_and(|unit| !unit.pages.is_empty()) {
            let size = sizes.get(prev)?;
            return Some((size.width, size.height));
        }
    }
    for next in pos + 1..units.len() {
        if units.get(next).is_some_and(|unit| !unit.pages.is_empty()) {
            let size = sizes.get(next)?;
            return Some((size.width, size.height));
        }
    }
    None
}

fn apply_continuous_separator_unit_sizes(
    units: &[ContinuousReadingUnitSpec],
    sizes: &mut [ContinuousReadingUnitSize],
) {
    for pos in 0..units.len().min(sizes.len()) {
        if units[pos].separator_text.is_none() {
            continue;
        }
        if let Some((width, height)) = nearest_continuous_page_unit_size(units, sizes, pos) {
            sizes[pos].width = width.max(1.0);
            sizes[pos].height = height.max(1.0);
        }
    }
}

/// 透過背景の描画スタイル。B キーで 3 モードを循環する。
///
/// フルスクリーンのビューポート背景は `ui_fullscreen.rs` で `Color32::BLACK` に
/// ハードコードされており、テーマ設定 (Light/Dark/System) に関係なく常に黒。
/// そのため B キー循環は「黒 (= ビューポート既定) → 白 → 市松」のテーマ非依存
/// 3 モードとした。以前はテーマの反対色を計算していたが、Light テーマ時に
/// `反対色 = 黒 = ビューポート既定` となり 2 モード連続で視覚変化なしになるバグが
/// あったため撤去 (v0.7.0 フィードバック)。
pub(crate) enum FsBgStyle<'a> {
    /// 塗らない (0 = ビューポート既定 / 常に黒地)
    Default,
    /// 単色で塗りつぶす (1 = 白)
    Solid(egui::Color32),
    /// 市松パターン (2)。テクスチャは Wrap=Repeat で作成済みであること。
    Checker(&'a egui::TextureHandle),
}

/// B キーで選択されたモードから描画スタイルを構築する。
///
/// - mode = 0: `Default` (塗らない — ビューポート既定の黒が透けて見える)
/// - mode = 1: `Solid(WHITE)`
/// - mode = 2: `Checker` (中間グレー市松)
pub(crate) fn transparent_bg_style<'a>(
    mode: u8,
    checker: Option<&'a egui::TextureHandle>,
) -> FsBgStyle<'a> {
    match mode {
        1 => FsBgStyle::Solid(egui::Color32::WHITE),
        2 => match checker {
            Some(t) => FsBgStyle::Checker(t),
            None => FsBgStyle::Default,
        },
        _ => FsBgStyle::Default,
    }
}

/// B キー循環で使うモードのトーストラベル。
pub(crate) fn transparent_bg_toast(mode: u8) -> &'static str {
    match mode {
        1 => "[背景: 白]",
        2 => "[背景: 市松]",
        _ => "[背景: 黒]",
    }
}

/// `rect` 内に透過背景を描画する。画像テクスチャを描く**直前**に呼ぶこと。
pub(crate) fn paint_transparent_bg(
    painter: &egui::Painter,
    rect: egui::Rect,
    style: &FsBgStyle<'_>,
) {
    match style {
        FsBgStyle::Default => {}
        FsBgStyle::Solid(color) => {
            painter.rect_filled(rect, 0.0, *color);
        }
        FsBgStyle::Checker(tex) => {
            // テクスチャは Wrap=Repeat で 16×16 の市松。
            // rect 全域をカバーするよう UV を rect_size / tile_px で指定する。
            let uv_max = egui::pos2(
                rect.width() / CHECKER_TILE_PX,
                rect.height() / CHECKER_TILE_PX,
            );
            let uv_rect = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), uv_max);
            painter.image(tex.id(), rect, uv_rect, egui::Color32::WHITE);
        }
    }
}
/// チェックマーク円のマージン（画面端からの距離）
const CHECKMARK_MARGIN: f32 = 16.0;
/// フィードバックトースト表示時間（秒）。短い確認系トーストの既定値。
/// 複数行の案内文は `show_feedback_toast_with_duration` で長めを指定する。
pub(crate) const FEEDBACK_TOAST_DURATION: f32 = 1.2;
/// 境界ヒント（最初/最後の項目に達した案内）の表示時間（秒）
const BOUNDARY_HINT_DURATION: f32 = 2.5;
/// 画像・動画フォルダが見つからない旨のヒント表示時間（秒）。メッセージが長く
/// ユーザーがフルスクリーンを維持するか Esc で抜けるか判断する時間が要るため、
/// 境界ヒントより長めに取る。
const NO_IMAGE_FOLDER_HINT_DURATION: f32 = 4.0;

/// J/K でマーカー間ジャンプするときの「現在位置とみなす許容幅」(秒)。
/// 現在位置とほぼ同じマーカーをスキップして次のマーカーへ進めるための余裕。
const NAV_MARKER_EPSILON: f64 = 0.5;

/// フルスクリーンで上部ホバーバーが表示される画面上端からの距離 (ピクセル)。
/// `draw_fs_hover_bar` の hover 判定と、`fs_ui_is_clean` のクリーン判定で共有する。
const TOP_BAR_HOVER_Y: f32 = 60.0;
// CURSOR_HIDE_IDLE_SECS は `crate::video::native_presenter::CURSOR_HIDE_IDLE_SECS`
// に集約している (eframe 経路と D3D11 native 経路の両方で同じ閾値を使うため)。

/// 動画のチャプター・ブックマーク・ピンを 1 本の Vec に集約するための種別タグ。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum NavMarkerKind {
    Chapter,
    Bookmark,
    Pin,
}

/// 動画再生時のジャンプ可能マーカー (チャプター開始 / ブックマーク / ピン)。
/// シークバー描画と J/K ジャンプの両方から使う。
#[derive(Clone, Debug)]
pub(crate) struct NavMarker {
    pub pts: f64,
    pub kind: NavMarkerKind,
    pub title: Option<String>,
}

/// フルスクリーン中央のヒントオーバーレイ。
#[derive(Copy, Clone)]
pub(crate) enum FsBoundaryHint {
    /// 最初/最後の項目に到達 (at_end: true=末尾, false=先頭)。
    Edge {
        at_end: bool,
        at: std::time::Instant,
    },
    /// Ctrl+↑↓ で画像・動画のある次 (forward=true) / 前 (forward=false) のフォルダが
    /// skip_limit 以内に見つからなかった。
    NoImageFolder {
        forward: bool,
        at: std::time::Instant,
    },
    /// Ctrl+PageUp/PageDown の兄弟限定移動で、同じ親に前後の兄弟が無い。
    NoSiblingFolder {
        forward: bool,
        at: std::time::Instant,
    },
    /// Ctrl+G 絞り込みビューで、これ以上進める検索結果が無い
    /// (forward=true: 末端, forward=false: 先頭)。
    SearchEnd {
        forward: bool,
        at: std::time::Instant,
    },
    /// Ctrl+F active / 検索結果一覧など、現在の scope では Ctrl+↑↓ が移動を
    /// 開始しないことを知らせる。
    NavNoOp {
        reason: FsNavNoOpReason,
        at: std::time::Instant,
    },
}

#[derive(Copy, Clone)]
pub(crate) enum FsNavNoOpReason {
    LocalFilterActive,
    SearchResultList,
    SearchSiblingUnsupported,
}

impl FsBoundaryHint {
    pub(crate) fn started_at(&self) -> std::time::Instant {
        match self {
            FsBoundaryHint::Edge { at, .. }
            | FsBoundaryHint::NoImageFolder { at, .. }
            | FsBoundaryHint::NoSiblingFolder { at, .. }
            | FsBoundaryHint::SearchEnd { at, .. }
            | FsBoundaryHint::NavNoOp { at, .. } => *at,
        }
    }
}

// ── 見開きペアリング ──────────────────────────────────────────────────────

/// 見開き表示のペア解決結果。
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum SpreadPair {
    /// 単独表示（1ページ表示 / 横長画像 / 表紙 / 末尾余り）
    Single,
    /// 見開き表示: left=画面左に表示するidx, right=画面右に表示するidx
    Double { left: usize, right: usize },
}

impl App {
    /// 分析モードを解除し、関連する状態をリセットする。
    pub(crate) fn reset_analysis_mode(&mut self) {
        let restore_idx = self.fullscreen_idx;
        self.analysis_mode = false;
        // post-filter バイパスを解除 (消しゴム or 隠蔽加工モード中ならそちらが保持する)
        if self.post_filter_bypassed && !self.is_overlay_edit_mode_active() {
            let needs_post_filter_restore = restore_idx
                .map(|idx| {
                    self.effective_params(idx).post_filter != crate::adjustment::PostFilter::None
                })
                .unwrap_or(false);
            self.post_filter_bypassed = false;
            if needs_post_filter_restore {
                if let Some(idx) = restore_idx {
                    self.clear_adjustment_render_caches_for_bypass(idx);
                }
            }
        }
        self.analysis_hover_color = None;
        self.analysis_pinned_color = None;
        self.analysis_grayscale = false;
        self.analysis_mosaic_grid = false;
        self.analysis_filter_mag = 0;
        self.analysis_guide_drag = None;
    }

    /// 分析モード ON 時に post-filter を一時バイパスする。Z キーハンドラから呼ぶ。
    pub(crate) fn enter_analysis_mode_bypass(&mut self) {
        if !self.post_filter_bypassed {
            self.post_filter_bypassed = true;
            if let Some(idx) = self.fullscreen_idx {
                if self.effective_params(idx).post_filter != crate::adjustment::PostFilter::None {
                    self.clear_adjustment_render_caches_for_bypass(idx);
                }
            }
        }
    }

    /// 分析モードを ON/OFF トグルする。**Z キーとホバーバー分析ボタンの共通経路** (Codex P1:
    /// ボタンが `analysis_mode` を直接反転して副作用 = ズーム/パン引き継ぎ・post-filter
    /// bypass の enter/exit・補正パネル排他 を飛ばしていた退行を防ぐ)。
    pub(crate) fn toggle_analysis_mode(&mut self) {
        if self.analysis_mode {
            // 分析→通常: ズーム/パンを引き継ぐ
            self.fs_zoom = self.analysis_zoom;
            self.fs_pan = self.analysis_pan;
            self.reset_analysis_mode();
        } else {
            // 通常→分析: ズーム/パンを引き継ぐ
            self.analysis_zoom = self.fs_zoom;
            self.analysis_pan = self.fs_pan;
            self.analysis_mode = true;
            self.enter_analysis_mode_bypass();
            // 補正パネルと排他
            self.adjustment_mode = false;
            self.local_adjust_mode = false;
            self.local_adjust_add_layer_dialog_open = false;
            self.local_adjust_change_mask_dialog_open = false;
            self.local_adjust_effect_picker_dialog_open = false;
        }
    }

    /// `idx` に対する「現在表示できる最良の既存テクスチャ」を Arc::clone で取り出す。
    /// 優先順: final composite cache → edit cache → fs_cache (Static / Animated 現フレーム)
    /// → サムネ (`include_thumb=true` のときのみ)。
    ///
    /// `prepare_fullscreen_state` の高解像度 tex 解決と `current_fs_tex_for_holdover`
    /// が同じチェーンを 2 回書いていた重複を集約する。ここでは既存 cache の lookup
    /// だけを行い、MI-GAN や隠蔽合成の新規生成は走らせない。生成を伴う通常描画は
    /// `resolve_fs_processed_texture` を使う。
    pub(crate) fn resolve_fs_display_tex(
        &self,
        idx: usize,
        include_thumb: bool,
    ) -> Option<egui::TextureHandle> {
        // comic 注釈は最前面 (D1)。holdover / display-tex 解決でも最優先で拾う。
        if let Some(tex) = self.current_comic_composite_texture(idx) {
            return Some(tex);
        }
        if let Some(tex) = self.current_final_composite_texture(idx) {
            return Some(tex);
        }
        if let Some(tex) = self.current_edit_result_texture(idx) {
            return Some(tex);
        }
        if let Some(entry) = self.conceal_cache.get(&idx) {
            if entry.generation == self.conceal_generation {
                return Some(entry.texture.clone());
            }
        }
        if let Some(tex) = self.current_local_adjust_texture(idx) {
            return Some(tex);
        }
        if let Some(tex) = self.current_erase_result_texture(idx) {
            return Some(tex);
        }
        match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { tex, .. }) => return Some(tex.clone()),
            Some(FsCacheEntry::Animated {
                frames,
                current_frame,
                ..
            }) => {
                if let Some((h, _)) = frames.get(*current_frame) {
                    return Some(h.clone());
                }
            }
            Some(FsCacheEntry::Video { .. }) => {
                // 動画は native presenter が独立 HWND に描画するため、
                // ここから取り出せる egui TextureHandle はない。サムネイルへ
                // フォールバックする。
            }
            _ => {}
        }
        if include_thumb {
            if let Some(crate::grid_item::ThumbnailState::Loaded { tex, .. }) =
                self.thumbnails.get(idx)
            {
                return Some(tex.clone());
            }
        }
        None
    }

    /// 現在 fullscreen 表示中の画像 (Static) が透過 (alpha<255) を含むか。
    /// 透過がある画像でのみ B キーの背景切替が意味を持つ (不透明画像は背景が画像の裏に
    /// 隠れて見た目が変わらない)。Static 以外 (アニメ / 動画 / 未ロード) は判定せず `true` を
    /// 返し、従来どおり切替を許可する (誤って無効化しないため)。
    fn fs_image_has_alpha(&self, idx: usize) -> bool {
        match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { pixels, .. }) => pixels.pixels.iter().any(|p| p.a() < 255),
            _ => true,
        }
    }

    /// 右 Ctrl ホールド中だけ、mIV 側の派生表示 (補正 / AI / 消しゴム補完) を
    /// 迂回して raw decode の元画像テクスチャを選ぶ。
    ///
    /// フォーカスは `ctx.input(|i| i.viewport().focused)` ではなく
    /// `self.fs_prev_focused` で確認する: この関数の呼び出し元
    /// `prepare_fullscreen_state` は `show_viewport_immediate` の **外側** で
    /// 呼ばれているので、ここでの ctx はメインビューポートのもの = フルスクリーンが
    /// OS フォーカスを持っている間は常に `Some(false)` を返してしまう。
    /// `fs_prev_focused` はフルスクリーン viewport closure 内で毎フレーム更新される
    /// ので、フルスクリーン側の前フレーム focus 状態を反映する。`GetAsyncKeyState`
    /// 単体だと動画/アニメ駆動の repaint 中に他アプリの右Ctrl を拾うリスクがあるので、
    /// 必ずフォーカス gate と AND させる。
    ///
    /// Shift 検出も `ctx.input(|i| i.modifiers.shift)` ではなく OS API を使う。
    /// `prepare_fullscreen_state` は `show_viewport_immediate` の外側で呼ばれるため、
    /// ここでの ctx はメインビューポートのもの。フルスクリーンが OS フォーカスを
    /// 持っている間、メイン ctx の modifier event は届かない
    /// (= `i.modifiers.shift` が常に false)。右Ctrl 検出と同じ理由。
    fn original_preview_active(&self, ctx: &egui::Context, idx: usize) -> bool {
        if !self.fs_prev_focused {
            return false;
        }
        if self.any_modal_dialog_open_for_fullscreen_keys() {
            return false;
        }
        if !original_preview_shortcut_held(ctx) {
            return false;
        }
        if self.local_adjust_mode && shift_held_via_os() {
            return false;
        }
        matches!(
            self.items.get(idx),
            Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::PdfPage { .. })
        )
    }

    fn resolve_original_preview_tex(&self, idx: usize) -> Option<egui::TextureHandle> {
        match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { tex, .. }) => Some(tex.clone()),
            Some(FsCacheEntry::Animated {
                frames,
                current_frame,
                ..
            }) => frames.get(*current_frame).map(|(h, _)| h.clone()),
            _ => None,
        }
    }

    /// 編集プレビューの下地になる source 解像度レイヤを解決する。
    fn resolve_fs_pre_overlay_texture(&self, idx: usize) -> Option<egui::TextureHandle> {
        match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { tex, .. }) => Some(tex.clone()),
            Some(FsCacheEntry::Animated {
                frames,
                current_frame,
                ..
            }) => frames.get(*current_frame).map(|(h, _)| h.clone()),
            _ => None,
        }
    }

    /// 補正レイヤー直前の source 解像度入力テクスチャを解決する。
    /// 優先順: erase result → raw fs。
    fn resolve_local_adjust_source_texture(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
    ) -> Option<egui::TextureHandle> {
        if let Some(tex) = self.ensure_erase_result_texture(ctx, idx) {
            return Some(tex);
        }
        if self.mask_pages.contains(&idx) {
            return None;
        }
        self.resolve_fs_pre_overlay_texture(idx)
    }

    /// フルスクリーン描画で使う最終表示テクスチャを解決する共通入口。
    ///
    /// 単ページ / 見開き / ルーペの全てがここを通ることで、加工レイヤを追加した
    /// ときの横展開漏れを防ぐ。通常表示は edit result に AdjustParams / AI /
    /// post_filter を最終段で適用した final composite を使う。
    ///
    /// 注意: 本関数は `prepare_fullscreen_state` 経由でメインビューポート ctx が
    /// 渡される経路がある。Modifier 状態は `ctx.input(|i| i.modifiers)` ではなく
    /// 必ず `*_held_via_os()` 系で取ること。
    fn resolve_fs_processed_texture(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        original_preview_active: bool,
    ) -> Option<egui::TextureHandle> {
        let is_video = matches!(self.items.get(idx), Some(GridItem::Video(_)));
        if is_video {
            return None;
        }
        // Z 分析モードは AI / 補正 / 注釈 / 隠蔽 / 消しゴム / 局所補正をすべてバイパスして
        // **raw 元画像**を表示する (右 Ctrl の original preview と同じ経路)。分析パネルの色取得・
        // ヒストグラム・グレースケール/拡大鏡オーバーレイは raw fs_cache を読むので、表示も raw に
        // 揃えることで「クリックした見た目の色 = 分析値」が一致する (Codex 指摘 + ユーザー要望)。
        if original_preview_active || self.analysis_mode {
            return self.resolve_original_preview_tex(idx);
        }

        if self.erase_mode {
            if !self.erase_preview_active {
                return self.ensure_erase_base_texture(ctx, idx);
            }
            let preview_tex = self
                .erase_preview_cache
                .get(&idx)
                .map(|e| e.texture.clone());
            return preview_tex
                .or_else(|| self.resolve_fs_pre_overlay_texture(idx))
                .or_else(|| self.ensure_erase_base_texture(ctx, idx));
        }

        if self.local_adjust_mode {
            // 判定は副作用ゼロの `decide_local_adjust_preview_action` に集約 (= unit
            // test できる)。caller である本関数は OS API 読み・cache lookup・worker
            // spawn・source texture フォールバックの責務だけを持つ。
            //
            // `prepare_fullscreen_state` 経由では ctx がメインビューポートのものになり、
            // フルスクリーンが OS フォーカスを持つ間は modifier event が届かないため、
            // `original_preview_active` と同じく OS キー状態を見る (fs_prev_focused
            // ガードは decide 関数内に集約済み)。
            let total_layers = self
                .local_adjust_page_layers
                .get(&idx)
                .map(Vec::len)
                .unwrap_or(0);
            let action = decide_local_adjust_preview_action(
                self.fs_prev_focused,
                ctrl_held_via_os(),
                shift_held_via_os(),
                self.local_adjust_show_source,
                self.local_adjust_preview_to_selected_layer,
                total_layers > 0,
                self.selected_local_adjust_layer_idx(idx),
                total_layers,
            );
            match action {
                LocalAdjustPreviewAction::ShowSource => {
                    return self.resolve_local_adjust_source_texture(ctx, idx);
                }
                LocalAdjustPreviewAction::BypassLayer { layer_idx } => {
                    self.maybe_start_local_adjust_layer_bypass_preview(idx, layer_idx);
                    if let Some(tex) =
                        self.current_local_adjust_layer_bypass_texture(idx, layer_idx)
                    {
                        return Some(tex);
                    }
                    return self.resolve_local_adjust_source_texture(ctx, idx);
                }
                LocalAdjustPreviewAction::PrefixPreview { layer_count } => {
                    self.maybe_start_local_adjust_prefix_preview(idx, layer_count);
                    if let Some(tex) =
                        self.current_local_adjust_prefix_preview_texture(idx, layer_count)
                    {
                        return Some(tex);
                    }
                    return self.resolve_local_adjust_source_texture(ctx, idx);
                }
                LocalAdjustPreviewAction::FullComposite => {
                    if let Some(local_adjust_tex) = self.current_local_adjust_texture(idx) {
                        return Some(local_adjust_tex);
                    }
                    return self.resolve_local_adjust_source_texture(ctx, idx);
                }
            }
        }

        if self.conceal_mode && !self.conceal_preview_active {
            if let Some(local_adjust_tex) = self.current_local_adjust_texture(idx) {
                return Some(local_adjust_tex);
            }
            if let Some(erase_result_tex) = self.ensure_erase_result_texture(ctx, idx) {
                return Some(erase_result_tex);
            }
            return self.resolve_fs_pre_overlay_texture(idx);
        }

        // comic (テキスト注釈) は最前面 = パイプライン最終段 (D1)。注釈が無ければ
        // None なので素の final composite にフォールバック (非注釈画像はゼロ
        // オーバーヘッド・退行なし)。
        if let Some(comic) = self.ensure_comic_composite_texture(ctx, idx) {
            return Some(comic);
        }
        self.ensure_final_composite_texture(ctx, idx)
            .or_else(|| self.resolve_fs_pre_overlay_texture(idx))
    }

    /// Ctrl+↑↓ ナビ発火直前に `fs_holdover_tex` を仕込むためのヘルパ。
    /// `resolve_fs_display_tex` を `include_thumb=true` で呼んで「最良の表示物」を取る。
    pub(crate) fn current_fs_tex_for_holdover(&self, fs_idx: usize) -> Option<egui::TextureHandle> {
        self.resolve_fs_display_tex(fs_idx, true)
    }

    /// `fs_nav_locked_gen.is_some()` の薄いラッパー。
    /// 入力ハンドラ・描画パスから「現在 nav ロック中か」を簡潔に問い合わせるため。
    pub(crate) fn fs_nav_is_locked(&self) -> bool {
        self.fs_nav_locked_gen.is_some()
    }

    /// nav ロックと holdover を強制解除する。`poll_fs_nav_lock` の通常解除条件
    /// (items_generation 進行 + 新ページの tex 用意) に到達しないケース
    /// (DFS が境界に当たって path=None / 画像・動画フォルダに着地できず !hit_image_folder /
    /// ユーザーが Esc でフルスクリーンを抜けて DFS をキャンセル) で明示的に呼ぶ。
    /// これをやらないと `fs_nav_locked_gen` が永続化して以降の Ctrl+↑↓ がすべて
    /// 無視される (Codex P1)。
    pub(crate) fn release_fs_nav_lock(&mut self) {
        self.fs_nav_locked_gen = None;
        self.fs_holdover_tex = None;
    }

    /// Ctrl+↑↓ ナビ発火直前に `fs_holdover_tex` を仕込み、`items_generation` を
    /// ロック取得時点で記録する。ナビによる items 入れ替えで fs_cache が drop されても、
    /// ロック解除まで holdover Arc を Render パスから参照することで画面が真っ白に
    /// なるのを防ぐ。`items_generation` のスナップショットは `poll_fs_nav_lock` の
    /// 「items が入れ替わる前にロックを解除しない」判定に使う。
    pub(crate) fn capture_fs_nav_holdover(&mut self, fs_idx: usize) {
        self.fs_holdover_tex = self.current_fs_tex_for_holdover(fs_idx);
        self.fs_nav_locked_gen = Some(self.items_generation);
    }

    /// 毎フレーム呼び出され、ナビロックの解除条件を満たしたら lock を解除する。
    /// 解除条件: ① items が入れ替わって `items_generation` が進んだ
    /// (= `install_new_items` で新フォルダの items が導入された) かつ
    /// ② 現フルスクリーン idx に対して thumbnails が `Loaded` か
    /// fs_cache に Static / Animated エントリが入った状態。
    /// 「items が進む前」(= 旧ページがロード済み判定で誤って解除される) のを
    /// items_generation チェックで防ぐ。
    pub(crate) fn poll_fs_nav_lock(&mut self) {
        let Some(locked_gen) = self.fs_nav_locked_gen else {
            return;
        };
        // items_generation が進んでいない = まだ items 入れ替えが起きていないので
        // 旧 fs_idx のテクスチャが残っているのは当然。ここで解除すると holdover が
        // 失われて「ファイル名のみ表示」のフラッシュが出るので保留する。
        if self.items_generation <= locked_gen {
            return;
        }
        let Some(idx) = self.fullscreen_idx else {
            // 通常は `apply_folder_nav_result` 内で close_fullscreen → open_fullscreen が
            // 同フレームで連続実行されるので fs_idx は Some に戻る。例外は **PDF/ZIP の
            // async enumerate 待ち**: PDF メタキャッシュ hit で `try_apply_pdf_meta_cache`
            // が placeholder grid を install (= items_generation++) するが、
            // `reopen_fullscreen_after_folder_nav_load` は `pdf_enumerate_pending` が
            // 残っているため "enumerate_defer" で抜け、fullscreen_idx は None のまま
            // 数フレーム経過する。この window で holdover を解放してしまうと、
            // `keep_fullscreen_viewport_alive` (viewport mode) と
            // `render_embedded_fs_nav_holdover` (in-window mode) の defer 描画から
            // 直前ページ画像が消えて真っ黒のフラッシュになる。
            // `fs_nav_after_pdf_enumerate` が立っている間はユーザーがフルスクリーン
            // 継続を意図しているので、解除を保留して deferred reopen 完了まで待つ。
            if self.fs_nav_after_pdf_enumerate.is_some() {
                return;
            }
            // 上記以外で fullscreen_idx = None = ユーザーが Esc 等で抜けた。
            self.fs_nav_locked_gen = None;
            self.fs_holdover_tex = None;
            return;
        };
        let has_full = matches!(
            self.fs_cache.get(&idx),
            Some(FsCacheEntry::Static { .. })
                | Some(FsCacheEntry::Animated { .. })
                | Some(FsCacheEntry::Video { .. })
        );
        let has_thumb = matches!(
            self.thumbnails.get(idx),
            Some(crate::grid_item::ThumbnailState::Loaded { .. })
        );
        if has_full || has_thumb {
            self.fs_nav_locked_gen = None;
            self.fs_holdover_tex = None;
        }
    }

    /// フルスクリーン通常モードのズーム/パンが有効なら返す。
    /// 閾値以下なら None (描画側で無変換パスに流れるよう明示するため)。
    pub(crate) fn fs_zoom_pan(&self) -> Option<(f32, egui::Vec2)> {
        if self.fs_zoom > ZOOM_NEAR_ONE || self.fs_pan.length_sq() > PAN_EPSILON_SQ {
            Some((self.fs_zoom, self.fs_pan))
        } else {
            None
        }
    }

    /// 中ボタンドラッグズームを処理する。
    ///
    /// ホイール押し込み + 上下ドラッグで fs_zoom / analysis_zoom を連続的に変える。
    /// 分析モード中は analysis_zoom/pan、それ以外は fs_zoom/pan を書き換える
    /// (どちらに書き込むかはドラッグ開始時点の `analysis_mode` で決まる)。
    /// 戻り値は「このフレームで中ボタンがアクティブに使われていた」か。true なら
    /// 呼び出し側はこの後の左クリック/右クリックの解釈をスキップしてよい。
    fn handle_middle_drag_zoom(&mut self, ctx: &egui::Context, full_rect: egui::Rect) -> bool {
        let (is_down, is_pressed, is_released, current_pos) = ctx.input(|i| {
            (
                i.pointer.button_down(egui::PointerButton::Middle),
                i.pointer.button_pressed(egui::PointerButton::Middle),
                i.pointer.button_released(egui::PointerButton::Middle),
                i.pointer.interact_pos(),
            )
        });

        // 押下開始フレーム: 現在のズーム/パン/ピボットをスナップショット。
        // 同フレームで既にドラッグ中なら無視 (重複開始を防ぐ)。
        if is_pressed && self.fs_middle_zoom_drag.is_none() {
            if let Some(pos) = current_pos {
                // 分析モードでは画像エリアが右パネル分左にずれるので中心が違う
                let rect_center = if self.analysis_mode {
                    analysis_image_rect(full_rect).center()
                } else {
                    full_rect.center()
                };
                let (start_zoom, start_pan) = if self.analysis_mode {
                    (self.analysis_zoom, self.analysis_pan)
                } else {
                    (self.fs_zoom, self.fs_pan)
                };
                self.fs_middle_zoom_drag = Some(MiddleZoomDrag {
                    pivot: pos,
                    start_zoom,
                    start_pan,
                    rect_center,
                    is_analysis: self.analysis_mode,
                });
            }
        }

        // ドラッグ中: 押しっぱなしの間、毎フレーム pivot からの差分で新しいズームを計算。
        if is_down {
            if let (Some(drag), Some(pos)) = (self.fs_middle_zoom_drag.clone(), current_pos) {
                let dy = pos.y - drag.pivot.y;
                if dy.abs() < MIDDLE_DRAG_THRESHOLD_PX {
                    // しきい値以下: ズーム変更なし (クリック暴発防止)
                    return true;
                }
                // 上方向 (dy < 0) で拡大、下方向で縮小
                let factor = 2.0_f32.powf(-dy / MIDDLE_DRAG_UNIT_PX);
                let new_zoom = (drag.start_zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
                // pivot 位置が画面上で動かないように pan を補正。ホイールズームと違い
                // 差分累積ではなく毎回 start 値基準で計算することで、dy が元に戻れば
                // pan も完全に元へ戻る (累積誤差が発生しない)。
                let new_pan = zoom_preserve_pivot(
                    drag.pivot,
                    drag.rect_center,
                    drag.start_pan,
                    drag.start_zoom,
                    new_zoom,
                );
                // 書き戻し先はドラッグ開始時の is_analysis で固定
                // (途中でモードが切り替わっても書き先がブレないように)
                if drag.is_analysis {
                    self.analysis_zoom = new_zoom;
                    self.analysis_pan = new_pan;
                } else {
                    self.fs_zoom = new_zoom;
                    self.fs_pan = new_pan;
                }
                return true;
            }
        }

        // リリース: PDF のときだけ新倍率で再レンダリング要求 (ドラッグ中は発行しない)。
        if is_released {
            if let Some(drag) = self.fs_middle_zoom_drag.take() {
                let final_zoom = if drag.is_analysis {
                    self.analysis_zoom
                } else {
                    self.fs_zoom
                };
                if (final_zoom - drag.start_zoom).abs() > f32::EPSILON {
                    self.maybe_rerender_pdf(final_zoom);
                }
                return true;
            }
        }

        false
    }

    /// Overlay editing tools share Photoshop-style temporary pan: hold Space and
    /// drag with the primary button. `can_start` should be false over tool panels,
    /// but an already-started pan continues if the pointer crosses a panel.
    pub(crate) fn handle_overlay_space_pan_drag(
        &mut self,
        ctx: &egui::Context,
        space_held: bool,
        can_start: bool,
        primary_pressed: bool,
        primary_down: bool,
        primary_released: bool,
        pointer_pos: Option<egui::Pos2>,
    ) -> bool {
        let active = self.fs_pan_drag_start.is_some();
        if !space_held {
            if active {
                self.fs_pan_drag_start = None;
            }
            return false;
        }
        if !active && !can_start {
            return false;
        }

        if primary_pressed && !active {
            if let Some(pos) = pointer_pos {
                self.fs_pan_drag_start = Some((pos, self.fs_pan));
            }
        } else if primary_down
            && let (Some((start_pos, start_pan)), Some(pos)) = (self.fs_pan_drag_start, pointer_pos)
        {
            self.fs_pan = start_pan + (pos - start_pos);
        }
        if primary_released {
            self.fs_pan_drag_start = None;
        }
        ctx.set_cursor_icon(if primary_down {
            egui::CursorIcon::Grabbing
        } else {
            egui::CursorIcon::Grab
        });
        true
    }

    /// ホイールによるマウス位置固定ズームを適用する。ズームが変化したら true を返す。
    fn apply_wheel_zoom(
        zoom: &mut f32,
        pan: &mut egui::Vec2,
        wheel_y: f32,
        mouse: Option<egui::Pos2>,
        rect_center: egui::Pos2,
    ) -> bool {
        let factor = 1.1_f32.powf(wheel_y / WHEEL_SENSITIVITY);
        let old_zoom = *zoom;
        *zoom = (old_zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        if let Some(mouse) = mouse {
            *pan = zoom_preserve_pivot(mouse, rect_center, *pan, old_zoom, *zoom);
        }
        *zoom != old_zoom
    }

    /// 現在のフルスクリーン画像が PDF ページなら、指定ズームで再レンダリングを要求する。
    fn maybe_rerender_pdf(&mut self, zoom: f32) {
        if let Some(idx) = self.fullscreen_idx {
            if matches!(self.items.get(idx), Some(GridItem::PdfPage { .. })) {
                self.request_pdf_rerender(idx, zoom);
            }
        }
    }

    pub(crate) fn apply_gamepad_fullscreen_zoom(&mut self, amount: f32, dt: f32) -> bool {
        if self.analysis_mode || amount.abs() < 0.01 || dt <= 0.0 {
            return false;
        }
        let old_zoom = self.fs_zoom;
        let factor = 2.0_f32.powf(amount * dt);
        self.fs_zoom = (self.fs_zoom * factor).clamp(ZOOM_MIN, ZOOM_MAX);
        if (self.fs_zoom - old_zoom).abs() <= f32::EPSILON {
            return false;
        }
        self.maybe_rerender_pdf(self.fs_zoom);
        true
    }

    pub(crate) fn apply_gamepad_fullscreen_pan(&mut self, delta: egui::Vec2) -> bool {
        if self.analysis_mode || delta.length_sq() <= f32::EPSILON || self.fs_zoom <= ZOOM_NEAR_ONE
        {
            return false;
        }
        self.fs_pan += delta;
        true
    }
}

/// ピボット点 (`mouse`) が画面上で動かないように、zoom 変化に合わせて pan を補正した
/// 新しい pan を返す。`apply_wheel_zoom` と `handle_middle_drag_zoom` が共用する。
///
/// 式: new_pan = base_pan + (mouse - (rect_center + base_pan)) * (1 - new_zoom / base_zoom)
fn zoom_preserve_pivot(
    mouse: egui::Pos2,
    rect_center: egui::Pos2,
    base_pan: egui::Vec2,
    base_zoom: f32,
    new_zoom: f32,
) -> egui::Vec2 {
    let center = rect_center + base_pan;
    let cx = mouse.x - center.x;
    let cy = mouse.y - center.y;
    let ratio = new_zoom / base_zoom;
    egui::vec2(
        base_pan.x + cx * (1.0 - ratio),
        base_pan.y + cy * (1.0 - ratio),
    )
}

fn should_draw_fs_pixel_grid(
    pixel_grid_enabled: bool,
    using_full_texture: bool,
    zoom_pan: Option<(f32, egui::Vec2)>,
) -> bool {
    pixel_grid_enabled
        && using_full_texture
        && zoom_pan.is_some_and(|(zoom, _)| zoom > ZOOM_NEAR_ONE)
}

/// 中ボタンドラッグズームのスナップショット状態 (v0.8.1)。
/// ドラッグ開始フレームで固定し、以降はここからの差分でズームを計算する。
#[derive(Clone)]
pub(crate) struct MiddleZoomDrag {
    /// ドラッグ開始時のカーソル位置 (このピボットが画面上で動かないように pan を補正)
    pub pivot: egui::Pos2,
    /// 開始時の zoom (fs_zoom または analysis_zoom のいずれか)
    pub start_zoom: f32,
    /// 開始時の pan
    pub start_pan: egui::Vec2,
    /// ピボット計算に使う画像エリアの中心 (通常モードは full_rect.center()、
    /// 分析モードは analysis_image_rect.center())
    pub rect_center: egui::Pos2,
    /// ドラッグ開始時に分析モードだったか (途中でモード切替されても書き戻し先を固定)
    pub is_analysis: bool,
}

/// 分析モード時の画像表示領域（パネル分を右側に確保した残り）を返す。
fn analysis_image_rect(full_rect: egui::Rect) -> egui::Rect {
    let panel_w = 360.0_f32.clamp(full_rect.width() * 0.20, full_rect.width() * 0.35);
    egui::Rect::from_min_max(
        full_rect.min,
        egui::pos2(full_rect.max.x - panel_w, full_rect.max.y),
    )
}

/// 補正パネル (adjustment panel) のオーバーレイ矩形を返す。
///
/// 設計幅 `LEFT_PANEL_WIDTH` (260px) のまま画像に重ねる。以前は
/// `full_rect.width() * 0.3` で縮小していたが、in-window 表示の狭い窓では
/// 設計幅を下回り、中の文字が折り返し・重なって崩れた (窓幅は min_inner_size で
/// 640 以上に保証されるため縮小は不要)。
///
/// 描画 (`draw_adjustment_panel`) と当たり判定 (ホバー閉じ / ホイール /
/// クリック抑制) は必ずこの関数の rect を使うこと。幅がずれると「見えているのに
/// 枠外扱い」になり、パネルが閉じる・操作がページ送りに化ける。
fn adjustment_panel_rect(full_rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(full_rect.min.x, full_rect.min.y + TOP_BAR_HEIGHT),
        egui::pos2(
            full_rect.min.x + crate::ui_adjustment_panel::LEFT_PANEL_WIDTH,
            // 下端のページシークバーと重ならないよう、元の下端マージンとシークバー高さの
            // 大きい方だけ下端を空ける。
            full_rect.max.y
                - crate::ui_adjustment_panel::LEFT_PANEL_BOTTOM_MARGIN.max(FS_SEEK_BAR_HEIGHT),
        ),
    )
}

/// VST3 コンパクト表示モード時の動画表示領域 (= 右上 1/4 = 幅・高さ各 1/2)。
/// 残った左下 3/4 は黒背景のままなのでプラグイン GUI ウィンドウを置きやすい。
fn vst3_compact_image_rect(full_rect: egui::Rect) -> egui::Rect {
    let half_w = full_rect.width() * 0.5;
    let half_h = full_rect.height() * 0.5;
    egui::Rect::from_min_max(
        egui::pos2(full_rect.max.x - half_w, full_rect.min.y),
        egui::pos2(full_rect.max.x, full_rect.min.y + half_h),
    )
}

/// ナビゲーション可能アイテムのインデックスリストを作成する。
/// `adjacent_navigable_idx` と同じフィルタ条件。
fn build_nav_indices(items: &[GridItem], visible_indices: &[usize]) -> Vec<usize> {
    visible_indices
        .iter()
        .copied()
        .filter(|&i| {
            matches!(
                items.get(i),
                Some(GridItem::Image(_))
                    | Some(GridItem::Video(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::ZipSeparator { .. })
                    | Some(GridItem::PdfPage { .. })
            )
        })
        .collect()
}

fn build_image_reading_indices(items: &[GridItem], visible_indices: &[usize]) -> Vec<usize> {
    visible_indices
        .iter()
        .copied()
        .filter(|&i| {
            matches!(
                items.get(i),
                Some(GridItem::Image(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::PdfPage { .. })
            )
        })
        .collect()
}

fn count_seek_overlay_non_image_items(items: &[GridItem], nav_indices: &[usize]) -> (usize, usize) {
    nav_indices
        .iter()
        .fold((0usize, 0usize), |(videos, others), &idx| {
            match items.get(idx) {
                Some(GridItem::Video(_)) => (videos + 1, others),
                Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::PdfPage { .. })
                | Some(GridItem::ZipSeparator { .. }) => (videos, others),
                Some(_) | None => (videos, others + 1),
            }
        })
}

fn vertical_reading_offsets(heights: &[f32], gap: f32, current_pos: usize) -> Vec<f32> {
    if heights.is_empty() || current_pos >= heights.len() {
        return Vec::new();
    }
    let mut offsets = vec![0.0; heights.len()];
    for pos in current_pos + 1..heights.len() {
        offsets[pos] = offsets[pos - 1] + (heights[pos - 1] + heights[pos]) * 0.5 + gap.max(0.0);
    }
    for pos in (0..current_pos).rev() {
        offsets[pos] = offsets[pos + 1] - (heights[pos] + heights[pos + 1]) * 0.5 - gap.max(0.0);
    }
    offsets
}

fn clamp_vertical_reading_scroll(
    scroll: f32,
    offsets: &[f32],
    heights: &[f32],
    viewport_h: f32,
) -> f32 {
    if offsets.is_empty() || heights.is_empty() || offsets.len() != heights.len() {
        return 0.0;
    }
    let center_y = viewport_h * 0.5;
    let min_scroll = center_y + offsets[0] - heights[0] * 0.5;
    let last = offsets.len() - 1;
    let max_scroll = center_y + offsets[last] + heights[last] * 0.5 - viewport_h;
    if min_scroll <= max_scroll {
        scroll.clamp(min_scroll, max_scroll)
    } else {
        (min_scroll + max_scroll) * 0.5
    }
}

fn vertical_reading_visible_positions(
    offsets: &[f32],
    heights: &[f32],
    scroll: f32,
    viewport_h: f32,
) -> Vec<usize> {
    if offsets.len() != heights.len() {
        return Vec::new();
    }
    let center_y = viewport_h * 0.5;
    offsets
        .iter()
        .zip(heights.iter())
        .enumerate()
        .filter_map(|(pos, (&offset, &height))| {
            let page_center_y = center_y - scroll + offset;
            let top = page_center_y - height * 0.5;
            let bottom = page_center_y + height * 0.5;
            (bottom >= 0.0 && top <= viewport_h).then_some(pos)
        })
        .collect()
}

fn vertical_reading_nearest_position(offsets: &[f32], scroll: f32) -> Option<usize> {
    offsets
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            (*a - scroll)
                .abs()
                .partial_cmp(&(*b - scroll).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(pos, _)| pos)
}

fn vertical_reading_reanchor_scroll(scroll: f32, old_offsets: &[f32], new_pos: usize) -> f32 {
    old_offsets
        .get(new_pos)
        .map(|offset| scroll - *offset)
        .unwrap_or(scroll)
}

/// 見開きの左右ページの content bbox (各ページ正規化座標 0..1) を combined 空間
/// (幅 `left_w + right_w` × 高さ `combined_h`、左ページが x∈[0,left_w]、右ページが
/// x∈[left_w, left_w+right_w]) に写像し、union を取って返す。両方 `None` のときは
/// `None` (= 余白カット無効)。bbox 無しのページは全域 (余白を切らない) 扱い。
/// 返り値 `(x0, y0, x1, y1)` は combined 空間の矩形。
pub(crate) fn spread_content_union(
    margin_left: Option<egui::Rect>,
    margin_right: Option<egui::Rect>,
    left_w: f32,
    right_w: f32,
    combined_h: f32,
) -> Option<(f32, f32, f32, f32)> {
    if margin_left.is_none() && margin_right.is_none() {
        return None;
    }
    let combined_w = left_w + right_w;
    let (lx0, ly0, lx1, ly1) = match margin_left {
        Some(b) => (
            b.min.x * left_w,
            b.min.y * combined_h,
            b.max.x * left_w,
            b.max.y * combined_h,
        ),
        None => (0.0, 0.0, left_w, combined_h),
    };
    let (rx0, ry0, rx1, ry1) = match margin_right {
        Some(b) => (
            left_w + b.min.x * right_w,
            b.min.y * combined_h,
            left_w + b.max.x * right_w,
            b.max.y * combined_h,
        ),
        None => (left_w, 0.0, combined_w, combined_h),
    };
    Some((lx0.min(rx0), ly0.min(ry0), lx1.max(rx1), ly1.max(ry1)))
}

/// 指定インデックスの画像が横長（幅>高さ）かを判定する。
/// テクスチャサイズが不明な場合は false（縦長として扱う）。
fn is_landscape(
    idx: usize,
    fs_cache: &std::collections::HashMap<usize, FsCacheEntry>,
    thumbnails: &[ThumbnailState],
) -> bool {
    // フルサイズキャッシュから判定
    if let Some(entry) = fs_cache.get(&idx) {
        match entry {
            FsCacheEntry::Static { tex, .. } => {
                let s = tex.size_vec2();
                return s.x > s.y;
            }
            FsCacheEntry::Animated { frames, .. } => {
                if let Some((tex, _)) = frames.first() {
                    let s = tex.size_vec2();
                    return s.x > s.y;
                }
            }
            FsCacheEntry::Video { player, .. } => {
                if let Some(info) = player.info() {
                    return info.width > info.height;
                }
            }
            FsCacheEntry::Failed => {}
        }
    }
    // サムネイルから判定
    if let Some(ThumbnailState::Loaded { tex, .. }) = thumbnails.get(idx) {
        let s = tex.size_vec2();
        return s.x > s.y;
    }
    false
}

// ── フルスクリーン状態の中間構造体 ──────────────────────────────────────

/// フルスクリーン描画 1 フレーム分の事前計算済み状態。
struct FsFrameState {
    is_video: bool,
    separator_text: Option<String>,
    original_preview_active: bool,
    tex: Option<egui::TextureHandle>,
    thumb_tex: Option<egui::TextureHandle>,
    /// 上部ホバーバー左側に表示するパス文字列。
    /// 通常画像は `<folder>\<filename>`、ZIP 内画像は `<archive-path> > <entry>`、
    /// PDF ページは `<pdf-path> > Page N` の形で事前に整形して格納する。
    /// 変換済みアーカイブ閲覧中は `archive_source_override` を用いて元の
    /// RAR/7z/LZH のパスを表示する (キャッシュ ZIP のパスは見せない)。
    location_display: String,
    image_dims: Option<(u32, u32)>,
    image_file_size: Option<u64>,
    /// 原寸が GPU テクスチャ上限 (MAX_TEXTURE_DIM=8192) を超えていて、
    /// 表示は縮小版を使っているとき true。ホバーバーに⚠マーカーを出すのに使う。
    image_downscaled: bool,
    is_loading: bool,
    /// 起動時 VST3 チェーンロードが終わるまで動画開始を待っている状態。
    vst3_waiting_for_video: bool,
    fs_load_failed: bool,
    /// PDF ページのコンテンツ種別 (非 PDF なら None)
    pdf_content_type: Option<PdfPageContentType>,
}

/// フルスクリーンのキー入力結果。
#[derive(Default)]
pub(crate) struct FsKeyAction {
    /// Esc / Enter / 右クリック相当。`handle_fullscreen_close_request` を通す
    /// (= モードB のコンテナページなら L1、それ以外は 1 段 close)。
    pub(crate) close: bool,
    /// BS = 階層を 1 段だけ戻す。コンテナページなら `close_fullscreen` を直接呼んで
    /// L2 ページ一覧へ (設定で分岐しない)。
    pub(crate) close_to_page_list: bool,
    pub(crate) nav_delta: i32,
    pub(crate) ctrl_nav: Option<i32>,
    pub(crate) sibling_nav: Option<i32>,
    /// Home/End などの絶対ジャンプ先 item index
    pub(crate) jump_to: Option<usize>,
}

struct FsSeekInfo {
    image_indices: Vec<usize>,
    current_pos: usize,
    media_count: usize,
    video_count: usize,
    other_count: usize,
}

impl App {
    fn fullscreen_viewport_id(&self) -> egui::ViewportId {
        egui::ViewportId::from_hash_of(("fullscreen_viewer", self.fs_viewport_generation))
    }

    fn fullscreen_seek_info(&self, fs_idx: usize) -> Option<FsSeekInfo> {
        let display_order = self.current_grid_order();
        let image_indices = build_image_reading_indices(&self.items, display_order);
        let current_pos = image_indices.iter().position(|&idx| idx == fs_idx)?;
        let nav_indices = build_nav_indices(&self.items, display_order);
        let (video_count, other_count) =
            count_seek_overlay_non_image_items(&self.items, &nav_indices);
        let media_count = image_indices.len() + video_count + other_count;
        Some(FsSeekInfo {
            image_indices,
            current_pos,
            media_count,
            video_count,
            other_count,
        })
    }

    fn fullscreen_mixed_media_summary(info: &FsSeekInfo) -> String {
        let mut parts = Vec::new();
        if !info.image_indices.is_empty() {
            parts.push(format!("画像 {} ファイル", info.image_indices.len()));
        }
        if info.video_count > 0 {
            parts.push(format!("動画 {} ファイル", info.video_count));
        }
        if info.other_count > 0 {
            parts.push(format!("その他 {} 件", info.other_count));
        }
        parts.join("、")
    }

    fn seek_to_continuous_page(&mut self, ctx: &egui::Context, target_idx: usize) {
        self.fullscreen_idx = Some(target_idx);
        self.selected = Some(target_idx);
        self.scroll_to_selected = true;
        self.fs_vertical_scroll = 0.0;
        self.update_last_selected_image();
        self.record_book_resume(target_idx);
        ctx.request_repaint();
    }

    fn draw_fullscreen_seek_overlay(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        fs_idx: usize,
    ) -> Option<usize> {
        self.fs_seek_overlay_visible = false;
        let Some(info) = self.fullscreen_seek_info(fs_idx) else {
            self.fs_seek_drag_active = false;
            return None;
        };
        // 分析モード中は対象画像に集中するため、下端のページシークバーを出さない
        // (分析パネルの手描き content が下端にあり、clip しないままシークバーへはみ出す
        // 問題も併せて解消される)。
        if self.any_dialog_open() || self.is_overlay_edit_mode_active() || self.analysis_mode {
            self.fs_seek_drag_active = false;
            return None;
        }

        let primary_down = ctx.input(|i| i.pointer.primary_down());
        if !primary_down {
            self.fs_seek_drag_active = false;
        }
        const SEEK_HOVER_HEIGHT: f32 = 78.0;
        const SEEK_BAR_HEIGHT: f32 = FS_SEEK_BAR_HEIGHT;

        let bottom_band = egui::Rect::from_min_max(
            egui::pos2(full_rect.left(), full_rect.bottom() - SEEK_HOVER_HEIGHT),
            full_rect.right_bottom(),
        );
        let bottom_hover = ctx.input(|i| {
            i.pointer
                .hover_pos()
                .is_some_and(|pos| bottom_band.contains(pos))
        });
        if !bottom_hover && !self.fs_seek_drag_active {
            return None;
        }

        let panel_rect = egui::Rect::from_min_max(
            egui::pos2(full_rect.left(), full_rect.bottom() - SEEK_BAR_HEIGHT),
            full_rect.right_bottom(),
        )
        .intersect(full_rect);
        if panel_rect.width() < 160.0 {
            return None;
        }

        self.fs_seek_overlay_visible = true;
        let painter = ui.painter();
        painter.rect_filled(
            panel_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(8, 10, 14, 224),
        );
        painter.hline(
            panel_rect.x_range(),
            panel_rect.top(),
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 42),
            ),
        );

        let all_nav_items_are_images =
            info.media_count == info.image_indices.len() && !info.image_indices.is_empty();
        if !all_nav_items_are_images {
            let summary = Self::fullscreen_mixed_media_summary(&info);
            painter.text(
                panel_rect.center(),
                egui::Align2::CENTER_CENTER,
                summary,
                egui::FontId::proportional(14.0),
                egui::Color32::from_rgb(235, 238, 242),
            );
            return None;
        }

        let total = info.image_indices.len();
        let is_rtl = self.reading_direction == ReadingDirection::Rtl;
        let inner = panel_rect.shrink2(egui::vec2(12.0, 7.0));
        let font = egui::FontId::monospace(13.0);
        let sample_label = format!("{}/{}", total, total);
        let sample_galley = painter.layout_no_wrap(
            sample_label,
            font.clone(),
            egui::Color32::from_rgb(242, 244, 247),
        );
        let label_width = (sample_galley.size().x + 18.0)
            .max(64.0)
            .min((inner.width() * 0.32).max(64.0));
        let gap = 12.0;
        let (label_rect, track_rect) = if is_rtl {
            let label_rect =
                egui::Rect::from_min_size(inner.min, egui::vec2(label_width, inner.height()));
            let track_rect = egui::Rect::from_min_max(
                egui::pos2(label_rect.right() + gap, inner.center().y - 4.0),
                egui::pos2(inner.right(), inner.center().y + 4.0),
            );
            (label_rect, track_rect)
        } else {
            let label_rect = egui::Rect::from_min_max(
                egui::pos2(inner.right() - label_width, inner.top()),
                inner.right_bottom(),
            );
            let track_rect = egui::Rect::from_min_max(
                egui::pos2(inner.left(), inner.center().y - 4.0),
                egui::pos2(label_rect.left() - gap, inner.center().y + 4.0),
            );
            (label_rect, track_rect)
        };
        if track_rect.width() < 48.0 {
            return None;
        }

        let mut display_pos = info.current_pos.min(total - 1);
        let mut target = None;
        let hit_rect = track_rect.expand2(egui::vec2(0.0, 14.0));
        let response = ui.interact(
            hit_rect,
            ui.make_persistent_id("fullscreen_seek_track"),
            egui::Sense::click_and_drag(),
        );
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }

        let seek_pointer = if response.dragged() || response.clicked() {
            response.interact_pointer_pos()
        } else {
            None
        };
        if response.dragged() {
            self.fs_seek_drag_active = true;
        }
        if let Some(pointer_pos) = seek_pointer {
            let raw_fraction =
                ((pointer_pos.x - track_rect.left()) / track_rect.width()).clamp(0.0, 1.0);
            let fraction = if is_rtl {
                1.0 - raw_fraction
            } else {
                raw_fraction
            };
            let pos = if total <= 1 {
                0
            } else {
                (fraction * (total - 1) as f32).round() as usize
            }
            .min(total - 1);
            display_pos = pos;
            let target_idx = info.image_indices[pos];
            if target_idx != fs_idx || self.continuous_reading_active_for_idx(fs_idx) {
                if self.continuous_reading_active_for_idx(fs_idx) {
                    self.seek_to_continuous_page(ctx, target_idx);
                } else {
                    target = Some(target_idx);
                }
            }
        }

        painter.rect_filled(
            track_rect,
            4.0,
            egui::Color32::from_rgba_unmultiplied(92, 98, 110, 170),
        );
        let fraction = if total <= 1 {
            0.0
        } else {
            display_pos as f32 / (total - 1) as f32
        };
        let knob_x = if is_rtl {
            track_rect.right() - track_rect.width() * fraction
        } else {
            track_rect.left() + track_rect.width() * fraction
        };
        let filled_rect = if is_rtl {
            egui::Rect::from_min_max(
                egui::pos2(knob_x, track_rect.top()),
                track_rect.right_bottom(),
            )
        } else {
            egui::Rect::from_min_max(track_rect.min, egui::pos2(knob_x, track_rect.bottom()))
        };
        painter.rect_filled(
            filled_rect,
            4.0,
            egui::Color32::from_rgba_unmultiplied(112, 174, 255, 230),
        );
        painter.circle_filled(
            egui::pos2(knob_x, track_rect.center().y),
            6.0,
            egui::Color32::from_rgb(232, 240, 255),
        );
        painter.circle_stroke(
            egui::pos2(knob_x, track_rect.center().y),
            6.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(10, 16, 26, 180)),
        );

        let label = format!("{}/{}", display_pos + 1, total);
        painter.text(
            label_rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            font,
            egui::Color32::from_rgb(242, 244, 247),
        );
        if !primary_down && self.fs_seek_drag_active {
            self.fs_seek_drag_active = false;
        }
        target
    }

    /// フルスクリーンビューポートのライフサイクル後始末を行う。
    ///
    /// - フルスクリーンが現在アクティブ (`fullscreen_idx.is_some()`) なら何もしない
    ///   (描画は `render_fullscreen_viewport` が担当)。
    /// - PDF 列挙待ちの遷移中は holdover を表示してちらつきを防ぐ。
    /// - フルスクリーン終了直後の 1 フレームだけ `Visible(false)` を送って hidden に落とす。
    /// - それ以外のアイドル時は何もしない。以前は「入場時のちらつき防止」のため毎フレーム
    ///   `show_viewport_immediate(...with_visible(false), ...)` を呼んでいたが、hidden viewport の
    ///   常時維持が `fullscreen_viewport_ms` の主要候補だった (2026-05-10 perf log で
    ///   非アクティブ時に 30-70ms/frame を計測、内訳分割は同改修で追加) ため、呼ばない方針に
    ///   変更した。代償として `close_fullscreen` 後の再入場時に 1x1 → フルサイズの DWM 遷移
    ///   フラッシュが毎回出る (`fs_viewport_recreate_after_hide` で generation が進み新しい
    ///   ViewportId になるため)。
    pub(crate) fn keep_fullscreen_viewport_alive(&mut self, ctx: &egui::Context) {
        if self.fullscreen_idx.is_some() {
            return; // アクティブなときは render_fullscreen_viewport が担当
        }
        let fs_id = self.fullscreen_viewport_id();

        // Ctrl+↑↓ で PDF フォルダを挟む遷移中は fullscreen_idx が None のまま
        // PDF enumerate 完了を待つ。この間ビューポートを隠すとその下のグリッドが
        // 見えてちらつくので維持しつつ、ナビロックの holdover (= 直前ページの
        // テクスチャ) があればそれを表示して「黒画面で待たされる」体感を緩和する。
        if self.fs_viewport_shown && self.fs_nav_after_pdf_enumerate.is_some() {
            let fs_builder = self.build_fullscreen_viewport_builder();
            let mut cancel = false;
            // holdover を中央フィットで描画する用のテクスチャ参照をクロージャ前に外出し。
            let holdover = self.fs_holdover_tex.clone();
            ctx.show_viewport_immediate(fs_id, fs_builder, |ctx, _class| {
                // 列挙が重い / ワーカー異常停止などで待ちが長くなったときに
                // ユーザーが黒画面に閉じ込められないよう、Esc とウィンドウ
                // クローズ要求を受け付けて保留中の遷移をキャンセルする。
                if ctx.input(|i| i.viewport().close_requested())
                    || ctx.input(|i| i.key_pressed(egui::Key::Escape))
                {
                    cancel = true;
                }
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        if let Some(handle) = holdover.as_ref() {
                            // 中央 contain フィット (= はみ出さないアスペクト維持)。
                            let avail = ui.available_size();
                            let tex_size = handle.size_vec2();
                            if tex_size.x > 0.0
                                && tex_size.y > 0.0
                                && avail.x > 0.0
                                && avail.y > 0.0
                            {
                                let scale = (avail.x / tex_size.x).min(avail.y / tex_size.y);
                                let w = tex_size.x * scale;
                                let h = tex_size.y * scale;
                                let img_rect = egui::Rect::from_center_size(
                                    ui.max_rect().center(),
                                    egui::vec2(w, h),
                                );
                                ui.painter().image(
                                    handle.id(),
                                    img_rect,
                                    egui::Rect::from_min_max(
                                        egui::pos2(0.0, 0.0),
                                        egui::pos2(1.0, 1.0),
                                    ),
                                    egui::Color32::WHITE,
                                );
                            }
                        }
                    });
            });
            if cancel {
                // 保留中の「列挙後にフルスクリーン復帰」意図を破棄。
                // poll_pdf_enumerate 完了時のフルスクリーン再オープンが抑止され、
                // 次フレーム以降はこの関数の非アクティブ経路でビューポートが
                // 隠される (グリッドへ戻る)。
                self.fs_nav_after_pdf_enumerate = None;
                // ZIP defer の場合 `items_generation` がまだ進んでいないため、
                // `poll_fs_nav_lock` の解放経路に乗らず lock/holdover が居座る。
                // 明示 release で確実に状態をクリーンにする (embedded 用ヘルパと対称、
                // Codex 第 3 ラウンド P2)。
                self.release_fs_nav_lock();
                ctx.request_repaint();
            }
            return;
        }

        // フルスクリーン非アクティブ時の hidden viewport 維持コストを排除する。
        // 詳細は関数 doc を参照。
        if !self.fs_viewport_shown {
            return;
        }
        // ここに来るのは close_fullscreen 直後の 1 フレーム。
        // show_viewport_immediate を 1 回呼んで viewport を alive にし、
        // ViewportBuilder::with_visible(false) は「initial」可視性しか制御しないため、
        // 一度表示済みのビューポートを隠すには明示的に Visible(false) を送る必要がある。
        // 送信直前に DWM トランジションを無効化して Win11 のフェードアウトを抑止する。
        let fs_builder = self.build_fullscreen_viewport_builder().with_visible(false);
        ctx.show_viewport_immediate(fs_id, fs_builder, |_ctx, _class| {});
        crate::dwm_transitions::disable_transitions_for_thread_windows();
        ctx.send_viewport_cmd_to(fs_id, egui::ViewportCommand::Visible(false));
        self.fs_viewport_shown = false;
        if self.fs_viewport_recreate_after_hide {
            self.fs_viewport_generation = self.fs_viewport_generation.wrapping_add(1);
            self.fs_viewport_recreate_after_hide = false;
        }
    }

    /// in-window 静止画モード (`native_video_in_window_active`) で PDF/ZIP の
    /// async enumerate 待ち中に、メインウィンドウの `CentralPanel` に黒地 + holdover を
    /// 描画する。viewport モード側 `keep_fullscreen_viewport_alive` の PDF defer
    /// ブランチ (line 1095- area) と対称な役割。これがないと:
    ///   - `apply_folder_nav_result` の `close_fullscreen` で `fullscreen_idx = None`
    ///   - `reopen_fullscreen_after_folder_nav_load` が `pdf_enumerate_pending.is_some()`
    ///     を理由に "enumerate_defer" で抜ける
    ///   - 数フレーム間 `embedded_fs_active` が false になり、`render_grid` が走って
    ///     メインウィンドウの白い CentralPanel (ライトテーマ既定) が露出する
    /// という流れで「黒背景の画像が一瞬消えて白背景が見える」フラッシュが発生する。
    ///
    /// Esc / ウィンドウクローズ要求で deferred reopen を破棄して fullscreen を抜ける
    /// (viewport モードの defer 分岐と同じ挙動)。
    #[cfg(windows)]
    fn render_embedded_fs_nav_holdover(&mut self, ctx: &egui::Context) {
        let holdover = self.fs_holdover_tex.clone();
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        let escape_pressed = !self.ime_input_active()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        let cancel = close_requested || escape_pressed;

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                if let Some(handle) = holdover.as_ref() {
                    // 中央 contain フィット (= はみ出さないアスペクト維持)。
                    let avail = ui.available_size();
                    let tex_size = handle.size_vec2();
                    if tex_size.x > 0.0 && tex_size.y > 0.0 && avail.x > 0.0 && avail.y > 0.0 {
                        let scale = (avail.x / tex_size.x).min(avail.y / tex_size.y);
                        let w = tex_size.x * scale;
                        let h = tex_size.y * scale;
                        let img_rect =
                            egui::Rect::from_center_size(ui.max_rect().center(), egui::vec2(w, h));
                        ui.painter().image(
                            handle.id(),
                            img_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                }
            });

        if cancel {
            // 保留中の「列挙後にフルスクリーン復帰」意図を破棄してグリッドへ戻す。
            self.fs_nav_after_pdf_enumerate = None;
            self.release_fs_nav_lock();
            ctx.request_repaint();
        } else {
            // enumerate worker は別スレッドで完了し repaint を要求しないため、
            // defer 中は明示的に次フレームを起こす (= viewport モードと同じ)。
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// in-window 静止画から専用 fullscreen viewport へ切り替える直後、
    /// OS が新 viewport を前面表示するまで main 側の通常グリッド描画を抑止する。
    #[cfg(windows)]
    pub(crate) fn still_fullscreen_viewport_enter_suppressed(&mut self) -> bool {
        let Some(until) = self.still_fullscreen_viewport_enter_suppress_until else {
            return false;
        };

        if self.fullscreen_idx.is_none() || self.native_video_in_window_active {
            self.still_fullscreen_viewport_enter_suppress_until = None;
            if !self.fs_nav_is_locked() {
                self.fs_holdover_tex = None;
            }
            return false;
        }

        if std::time::Instant::now() <= until {
            return true;
        }

        self.still_fullscreen_viewport_enter_suppress_until = None;
        if !self.fs_nav_is_locked() {
            self.fs_holdover_tex = None;
        }
        false
    }

    /// `still_fullscreen_viewport_enter_suppressed` 中に main viewport へ描く黒地。
    /// 可能なら直前画像を中央 contain で残し、専用 viewport が前面に出るまで
    /// 背面のグリッドが見えないようにする。
    #[cfg(windows)]
    pub(crate) fn render_still_fullscreen_viewport_enter_holdover(&mut self, ctx: &egui::Context) {
        let holdover = self.fs_holdover_tex.clone();
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(egui::Color32::BLACK))
            .show(ctx, |ui| {
                if let Some(handle) = holdover.as_ref() {
                    let avail = ui.available_size();
                    let tex_size = handle.size_vec2();
                    if tex_size.x > 0.0 && tex_size.y > 0.0 && avail.x > 0.0 && avail.y > 0.0 {
                        let scale = (avail.x / tex_size.x).min(avail.y / tex_size.y);
                        let w = tex_size.x * scale;
                        let h = tex_size.y * scale;
                        let img_rect =
                            egui::Rect::from_center_size(ui.max_rect().center(), egui::vec2(w, h));
                        ui.painter().image(
                            handle.id(),
                            img_rect,
                            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                    }
                }
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }

    pub(crate) fn render_fullscreen_viewport(&mut self, ctx: &egui::Context) {
        let Some(fs_idx) = self.fullscreen_idx else {
            // in-window 静止画モードで PDF/ZIP enumerate defer 中:
            // grid (= 白 CentralPanel) が露出するのを防ぐため、メインウィンドウに
            // 直接黒地 + holdover を描く。詳細は `render_embedded_fs_nav_holdover`
            // の doc を参照。viewport モードでは `keep_fullscreen_viewport_alive` が
            // 別 viewport で同じ役割を担うので、ここで二重に描かないよう
            // `native_video_in_window_active` で gate する。
            #[cfg(windows)]
            if self.native_video_in_window_active && self.fs_nav_after_pdf_enumerate.is_some() {
                self.render_embedded_fs_nav_holdover(ctx);
            }
            return;
        };

        // ── pending の PDF 再レンダリング結果を取り込む ──
        // show_viewport_immediate 内では &mut self が使えるので、
        // メインの update() を待たずにここで直接 poll する。
        self.poll_prefetch(ctx);

        // ── 状態の事前計算 ──
        // 動画フルスクリーンの黒 backdrop は **presenter HWND が未確定の起動中だけ**
        // 出す純粋な「起動カバー」。fresh open で presenter が立ち上がるまでの黒画面を
        // 隠すのが唯一の役目で、popup HWND が確定したら隠して viewport を破棄させる
        // (presenter popup 自身が黒背景 DComp visual を持つので backdrop は不要)。
        //   - in-window モード: presenter child が main のクライアント領域に直接
        //     描画するので backdrop は出さない (出すと別 top-level window が
        //     foreground を奪い画面全体が黒くなる)。
        //   - Plan B のウィンドウ / 全画面トグル: popup は常に HWND 確定済み
        //     (`hwnd_ready`) なので backdrop は一切出さない。これにより backdrop の
        //     破棄・再生成 (= 白フラッシュ / 黒被り) がトグルで起きない。
        #[cfg(windows)]
        if self.native_video_backdrop_target_for_fs(fs_idx) {
            let hwnd_ready = self.native_video_presenter_hwnd_for_fs(fs_idx).is_some();
            let startup_cover = !self.native_video_in_window_active
                && !hwnd_ready
                && self.native_video_presenter_pending_for_fs(fs_idx);
            if startup_cover {
                self.show_native_video_black_backdrop(ctx, fs_idx);
            } else {
                self.hide_native_video_black_backdrop_if_shown(ctx);
            }
            return;
        }

        self.advance_animation(ctx, fs_idx);
        // 見開きペアを 1 回だけ解決し、以降のフレーム処理で再利用する
        // (resolve_spread_pair は get_nav_indices 内で Vec<usize> をクローンするため、
        //  毎フレーム 3〜4 回呼ばれるのを避ける)
        let spread_pair = self.resolve_spread_pair(fs_idx);
        let is_spread_double = matches!(spread_pair, SpreadPair::Double { .. });
        // 見開きパートナーの事前読み込み + アニメーション進行
        if let SpreadPair::Double { left, right } = spread_pair {
            let partner = if left == fs_idx { right } else { left };
            self.advance_animation(ctx, partner);
            if !self.fs_cache.contains_key(&partner) && !self.fs_pending.contains_key(&partner) {
                self.start_fs_load(partner);
            }
        }
        let state = self.prepare_fullscreen_state(ctx, fs_idx);

        let mut close_fs = false;
        let mut close_to_page_list = false;
        let mut nav_delta: i32 = 0;
        let mut ctrl_nav: Option<i32> = None;
        let mut sibling_nav: Option<i32> = None;
        let mut jump_to: Option<usize> = None;
        // 境界ヒント即時消去のため、フレーム先頭の状態を捕捉する。
        // handle_fs_navigation 実行後に、ヒントが同じ start_time のまま残って
        // いれば「このフレームで再設定されていない」= 打ち切ってよい、と判定する。
        let hint_start_before = self.fs_boundary_hint.map(|h| h.started_at());
        let mut had_user_input_in_frame = false;
        let prev_foreground_hwnd = self.fs_prev_foreground_hwnd;

        // ── ビューポート構築 ──
        // in-window モード中は静止画を専用 viewport ではなくメインウィンドウの
        // egui ctx に直接描画する (embedded)。本関数冒頭で動画は early-return
        // 済みなので、ここに来る fs_idx は常に非動画。
        #[cfg(windows)]
        let embedded = self.fullscreen_embedded_still_active();
        #[cfg(not(windows))]
        let embedded = false;
        let main_ctx = ctx;
        #[cfg(windows)]
        let hide_viewport_after_embedded_paint = embedded && self.fs_viewport_shown;
        let mut fs_builder = self.build_fullscreen_viewport_builder();
        let fs_id = self.fullscreen_viewport_id();
        let need_show = !self.fs_viewport_shown;
        if need_show && !embedded {
            // 新規 viewport は hidden で作り、DWM transition 抑止属性を当ててから
            // Visible(true) にする。初期 white client と最大化アニメーションの露出を
            // 動画 backdrop と同じ手順で避ける。
            fs_builder = fs_builder.with_visible(false);
        }
        let fs_viewport_t0 = std::time::Instant::now();
        let mut fs_setup_ms = 0.0_f64;
        let mut fs_input_ms = 0.0_f64;
        let mut fs_media_ms = 0.0_f64;
        let mut fs_overlay_ms = 0.0_f64;
        let mut fs_hud_ms = 0.0_f64;
        let mut fs_panels_ms = 0.0_f64;
        let mut fs_hover_bar_ms = 0.0_f64;
        let mut fs_central_ms = 0.0_f64;
        let mut fs_vst_manager_ms = 0.0_f64;
        let mut fs_closure_ms = 0.0_f64;
        let fs_state_is_video = state.is_video;
        // 動画は native presenter が独立 HWND に描画するので、egui 側 viewport は
        // 黒 backdrop のみ。ここで GPU 経路かどうかを区別する必要は無い。
        let fs_state_gpu_video = false;

        {
            let mut render_fs_body = |ctx: &egui::Context, embedded: bool| {
                let closure_t0 = std::time::Instant::now();
                let setup_t0 = std::time::Instant::now();
                // フルスクリーンビューポート内のイベントで IME 状態を更新する
                // (メインビューポートとは別のイベントキューなのでここで呼ぶ必要がある)。
                // embedded のときは ctx = main ctx で、IME 状態は update() 冒頭で
                // 既に更新済みなので二重処理しない。
                if !embedded {
                    self.update_ime_state(ctx);
                }
                // IME 変換確定の Enter が multiline 本文欄 (セリフ) に改行として入るのを防ぐ
                // (egui の既知挙動。実機 FB 2026-06-06)。変換確定自体は Ime::Commit が行うので、
                // IME がアクティブ (変換中 or 直近 300ms に Ime イベント = Windows の別フレーム
                // 配信吸収) の間は raw な Key::Enter を除去する。非変換時の Enter は通るので
                // 意図的な改行は可能。テキスト注釈モードの本文編集中のみに限定する。
                if self.text_mode && self.ime_input_active() {
                    ctx.input_mut(|i| {
                        i.events.retain(|e| {
                            !matches!(
                                e,
                                egui::Event::Key {
                                    key: egui::Key::Enter,
                                    ..
                                }
                            )
                        });
                    });
                }
                if need_show && !embedded {
                    // embedded のときは専用 viewport を作らないので Visible/Focus は
                    // 送らない (main ウィンドウは既に表示・フォーカス済み)。
                    #[cfg(windows)]
                    crate::dwm_transitions::disable_transitions_for_thread_windows();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(true));
                }

                // 他アプリからフォーカスが戻ってきた瞬間を記録。
                // この直後のクリックはナビ目的ではなく「フォーカスを戻すだけのクリック」
                // とみなし、アプリ側の処理を抑制する（ページ送り・パン開始など）。
                let focused_now = ctx.input(|i| i.viewport().focused).unwrap_or(true);
                if focused_now && !self.fs_prev_focused {
                    self.fs_focus_regained_at = Some(std::time::Instant::now());
                }
                self.fs_prev_focused = focused_now;

                // カーソル自動非表示用のアクティビティ検出。マウス移動 / クリック /
                // ホイールがあれば「活動中」とみなしタイマをリセットする。キー操作は
                // カーソルを再表示しない。
                // `pointer.velocity()` ではなく `delta()` を使う (velocity は静止後も
                // 慣性を残し、3 秒タイマがいつまでも進まない誤動作になるため)。
                // `open_fullscreen` で Some 初期化されるが、想定外の入場経路で None の
                // まま到達した場合も safety net として今フレームで起動する。
                if self.cursor_last_activity.is_none() {
                    self.cursor_last_activity = Some(std::time::Instant::now());
                }
                let cursor_active = ctx.input(|i| {
                    i.pointer.delta() != egui::Vec2::ZERO
                        || i.pointer.any_pressed()
                        || i.pointer.any_click()
                        || i.smooth_scroll_delta != egui::Vec2::ZERO
                });
                if cursor_active {
                    self.cursor_last_activity = Some(std::time::Instant::now());
                    if self.cursor_hidden {
                        ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(true));
                    }
                    self.cursor_hidden = false;
                }

                // event consume される前に捕捉 (handle_fs_key_input が矢印等を
                // 消費するとイベントが見えなくなるため)。マウス移動は操作と見なさない。
                if self.fs_boundary_hint.is_some() {
                    had_user_input_in_frame = ctx.input(|i| {
                        i.events.iter().any(|e| {
                            matches!(
                                e,
                                egui::Event::Key { pressed: true, .. }
                                    | egui::Event::PointerButton { pressed: true, .. }
                                    | egui::Event::MouseWheel { .. }
                            )
                        })
                    });
                }

                if !embedded && ctx.input(|i| i.viewport().close_requested()) {
                    // embedded のときの close_requested は main ウィンドウの × =
                    // アプリ終了要求。フルスクリーン解除ではないので拾わない。
                    close_fs = true;
                }
                fs_setup_ms = setup_t0.elapsed().as_secs_f64() * 1000.0;

                let central_t0 = std::time::Instant::now();
                egui::CentralPanel::default()
                    .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                    .show(ctx, |ui| {
                        let full_rect = ui.max_rect();
                        let input_t0 = std::time::Instant::now();

                        // ── キー入力 ──
                        if let Some(keys) = fullscreen_shortcut_event_summary(ctx) {
                            let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
                            crate::logger::log(format!(
                                "[fs-key] source=fullscreen focused={} foreground=0x{:x} keys={}",
                                focused,
                                current_foreground_hwnd(),
                                keys
                            ));
                        }
                        let key_action = self.handle_fs_key_input(ctx, fs_idx, is_spread_double);
                        if key_action.close { close_fs = true; }
                        if key_action.close_to_page_list { close_to_page_list = true; }
                        nav_delta = key_action.nav_delta;
                        ctrl_nav = key_action.ctrl_nav;
                        sibling_nav = key_action.sibling_nav;
                        jump_to = key_action.jump_to;
                        // perf: キー起因のナビはここで input_seq を進める
                        if nav_delta != 0 {
                            self.bump_input_seq("fs_key", Some(&format!("delta={nav_delta}")));
                        } else if ctrl_nav.is_some() {
                            self.bump_input_seq("fs_ctrl_nav", None);
                        } else if sibling_nav.is_some() {
                            self.bump_input_seq("fs_sibling_nav", None);
                        } else if key_action.close {
                            self.bump_input_seq("fs_close_key", None);
                        }

                        // ── ホイール & クリック ──
                        let (wheel_nav, click_close) = self.handle_fs_wheel_and_click(
                            ui,
                            ctx,
                            full_rect,
                            &state,
                            is_spread_double,
                            prev_foreground_hwnd,
                        );
                        if wheel_nav != 0 { nav_delta = wheel_nav; }
                        if click_close { close_fs = true; }
                        // perf: ホイール/クリック起因のナビ
                        if wheel_nav != 0 {
                            self.bump_input_seq("fs_wheel", Some(&format!("delta={wheel_nav}")));
                        } else if click_close {
                            self.bump_input_seq("fs_close_click", None);
                        }
                        // ホイール/キーで nav_delta が確定済みなら、
                        // ホバーバーのボタンホバーで上書きされないよう保護
                        let nav_locked = nav_delta != 0;
                        fs_input_ms = input_t0.elapsed().as_secs_f64() * 1000.0;

                        // ── 分析/補正モード: 見開き中は無効 ──
                        // 分析モードは画像エリアを左側に制限する（右パネルと重ならないよう）。
                        // 補正モードは左パネルを画像の上にオーバーレイする（画像位置は移動しない）。
                        let compare_wipe_active = matches!(
                            self.compare_view_mode,
                            crate::app::CompareViewMode::Wipe { .. }
                        );
                        // 360 度パノラマビューモード中は分析 / 補正 / 比較を抑止する
                        // (= 上バーの 360 / × / window のみ、機能制限モード)。
                        let panorama_mode_active_now = self.is_panorama_mode_active(fs_idx);
                        let analysis_active = self.analysis_mode
                            && !is_spread_double
                            && !panorama_mode_active_now;
                        // 補正パネルは見開き Double でも使えるようにする (左右独立補正 + コピー)。
                        // 編集対象 (画面上の左/右) は `adjust_spread_target` で切替。
                        let adjustment_active = self.adjustment_mode
                            && !compare_wipe_active
                            && !panorama_mode_active_now;
                        // VST3 動画コンパクト表示モード: 動画のときだけ右上 1/4 に縮小し、
                        // 残った左下 3/4 をプラグイン GUI 用に空ける。動画でない (画像/PDF)
                        // ときは無視する (= プラグインで分析するのは動画なので)。
                        let vst3_compact_active =
                            cfg!(windows) && state.is_video && self.settings.vst3_enabled
                                && self.settings.vst3_gui_visible
                                && self.settings.vst3_video_compact;
                        let image_rect = if analysis_active {
                            analysis_image_rect(full_rect)
                        } else if vst3_compact_active {
                            vst3_compact_image_rect(full_rect)
                        } else {
                            full_rect
                        };

                        // ── 画像 / 動画 / セパレータ描画 ──
                        let media_t0 = std::time::Instant::now();
                        if self.continuous_reading_active_for_idx(fs_idx) {
                            self.draw_fs_continuous_reading(
                                ui,
                                ctx,
                                image_rect,
                                fs_idx,
                                state.original_preview_active,
                            );
                        } else if let Some(sep) = state.separator_text.as_ref() {
                            Self::draw_fs_separator(ui, image_rect, sep);
                        } else {
                            match spread_pair {
                                SpreadPair::Single => {
                                    // ── 360 度パノラマビュー (最優先で試行) ──
                                    // active なら通常パス (compare / draw_fs_image / rotation /
                                    // zoom / pan) は完全にスキップする。準備中なら false が
                                    // 返り、通常パスで equirect が平らに表示される
                                    // (= 「数フレ平らな表示 → 360 描画開始」UX、docs §4.2)。
                                    let panorama_painted = if self.is_panorama_mode_active(fs_idx) {
                                        self.try_paint_panorama(ui, ctx, image_rect, fs_idx)
                                    } else {
                                        false
                                    };
                                    if panorama_painted {
                                        self.fs_spread_layout = None;
                                    } else {

                                    let fs_rotation = self.get_rotation(fs_idx);
                                    let zp = if analysis_active {
                                        Some((self.analysis_zoom, self.analysis_pan))
                                    } else {
                                        self.fs_zoom_pan()
                                    };
                                    let free_rot = if analysis_active { 0.0 } else { self.fs_free_rotation };
                                    // 前フレームと異なる (idx, テクスチャ) の最初の描画で paint を emit。
                                    // seq はエントリ自身の `load_seq` を使う (self.input_seq だと
                                    // paint 時点で別操作に更新されていて load→ready→paint の相関が崩れる)。
                                    if crate::perf::is_enabled()
                                        && let Some(tex) = state.tex.as_ref()
                                    {
                                        let cur_id = tex.id();
                                        let prev = self.fs_painted_last;
                                        let is_new = !matches!(
                                            prev,
                                            Some((prev_idx, prev_id, _)) if prev_idx == fs_idx && prev_id == cur_id
                                        );
                                        if is_new {
                                            let key = self.perf_item_key(fs_idx);
                                            let entry_seq = self
                                                .fs_cache
                                                .get(&fs_idx)
                                                .map(|e| e.load_seq())
                                                .unwrap_or(0);
                                            crate::perf::event(
                                                "fs",
                                                "paint",
                                                key.as_deref(),
                                                entry_seq,
                                                &[("idx", serde_json::Value::from(fs_idx))],
                                            );
                                            self.fs_painted_last = Some((fs_idx, cur_id, entry_seq));
                                        }
                                    }
                                    let compare_mode = self.compare_view_mode;
                                    let compare_requested = !matches!(
                                        compare_mode,
                                        crate::app::CompareViewMode::Off
                                    );
                                    // 入力ハンドラ (キー / ホイール / クリック / gamepad) と
                                    // 同一述語で判定し、描画と入力の食い違いを構造的に防ぐ。
                                    let continuous_reading_active =
                                        self.continuous_reading_active_for_idx(fs_idx);
                                    if continuous_reading_active {
                                        self.draw_fs_continuous_reading(
                                            ui,
                                            ctx,
                                            image_rect,
                                            fs_idx,
                                            state.original_preview_active,
                                        );
                                    } else if compare_requested {
                                        self.ensure_compare_prepared_pair(ctx, fs_idx);
                                        if self.draw_compare_prepared_mode(
                                            ui,
                                            ctx,
                                            image_rect,
                                            compare_mode,
                                            zp,
                                        ) {
                                            // 比較表示側で描画済み。
                                        } else if matches!(
                                            compare_mode,
                                            crate::app::CompareViewMode::PinnedNormal
                                        ) {
                                            let compare_tex =
                                                self.ensure_compare_pinned_texture(ctx);
                                            if let Some(tex) = compare_tex.as_ref() {
                                                let bg_style = self.fs_bg_style(ctx);
                                                Self::draw_compare_pinned_image(
                                                    ui, image_rect, tex, zp, &bg_style, None,
                                                );
                                            }
                                        } else {
                                            let fallback_compare_tex = if matches!(
                                                compare_mode,
                                                crate::app::CompareViewMode::Wipe { .. }
                                            ) {
                                                self.ensure_compare_pinned_texture(ctx)
                                            } else {
                                                None
                                            };
                                            let pixel_grid_enabled =
                                                self.fs_pixel_grid_enabled && !analysis_active;
                                            // 余白カットフィット用 bbox (設定 OFF なら None)。bg_style が
                                            // self を借用する前に算出しておく。
                                            let fit_mode = if analysis_active {
                                                FullscreenFitMode::Page
                                            } else {
                                                self.effective_fullscreen_fit_mode()
                                            };
                                            let content_bbox = self.fs_margin_bbox(fs_idx);
                                            let bg_style = self.fs_bg_style(ctx);
                                            Self::draw_fs_image(
                                                ui,
                                                image_rect,
                                                state.tex.as_ref(),
                                                state.thumb_tex.as_ref(),
                                                state.is_video,
                                                state.vst3_waiting_for_video,
                                                state.fs_load_failed,
                                                fs_rotation,
                                                zp,
                                                free_rot,
                                                &bg_style,
                                                &state.location_display,
                                                pixel_grid_enabled,
                                                fit_mode,
                                                content_bbox,
                                            );
                                            if let crate::app::CompareViewMode::Wipe { fraction } =
                                                compare_mode
                                            {
                                                if let Some(tex) = fallback_compare_tex.as_ref() {
                                                    // 線 / clip はフィット後の実表示画像矩形基準に
                                                    // して、画像の切り替え位置と一致させる。
                                                    let ref_rect = Self::compare_image_draw_rect(
                                                        image_rect,
                                                        tex.size(),
                                                        zp,
                                                    )
                                                    .unwrap_or(image_rect);
                                                    let wipe_x = ref_rect.left()
                                                        + ref_rect.width()
                                                            * fraction.clamp(0.05, 0.95);
                                                    let clip = egui::Rect::from_min_max(
                                                        ref_rect.min,
                                                        egui::pos2(wipe_x, ref_rect.max.y),
                                                    );
                                                    Self::draw_compare_pinned_image(
                                                        ui,
                                                        image_rect,
                                                        tex,
                                                        zp,
                                                        &bg_style,
                                                        Some(clip),
                                                    );
                                                    Self::draw_compare_wipe_line(
                                                        ui, ref_rect, fraction,
                                                    );
                                                }
                                            }
                                        }
                                    } else {
                                        let pixel_grid_enabled =
                                            self.fs_pixel_grid_enabled && !analysis_active;
                                        // 余白カットフィット用 bbox (設定 OFF なら None)。bg_style が
                                        // self を借用する前に算出しておく。
                                        let fit_mode = if analysis_active {
                                            FullscreenFitMode::Page
                                        } else {
                                            self.effective_fullscreen_fit_mode()
                                        };
                                        let content_bbox = self.fs_margin_bbox(fs_idx);
                                        let bg_style = self.fs_bg_style(ctx);
                                        Self::draw_fs_image(
                                            ui, image_rect,
                                            state.tex.as_ref(), state.thumb_tex.as_ref(),
                                            state.is_video, state.vst3_waiting_for_video,
                                            state.fs_load_failed, fs_rotation, zp,
                                            free_rot, &bg_style, &state.location_display,
                                            pixel_grid_enabled,
                                            fit_mode,
                                            content_bbox,
                                        );
                                    }
                                    // 単一表示時は見開きレイアウトキャッシュを破棄
                                    self.fs_spread_layout = None;
                                    } // else (= !panorama_painted) ブロック終端
                                }
                                SpreadPair::Double { left, right } => {
                                    let compare_mode = self.compare_view_mode;
                                    let zoom_pan = self.fs_zoom_pan();
                                    let compare_requested = !matches!(
                                        compare_mode,
                                        crate::app::CompareViewMode::Off
                                    );
                                    if compare_requested {
                                        self.ensure_compare_prepared_pair(ctx, fs_idx);
                                    }
                                    if compare_requested
                                        && self.draw_compare_prepared_mode(
                                            ui,
                                            ctx,
                                            image_rect,
                                            compare_mode,
                                            zoom_pan,
                                        )
                                    {
                                        self.fs_spread_layout = None;
                                    } else if matches!(
                                        compare_mode,
                                        crate::app::CompareViewMode::PinnedNormal
                                    ) {
                                        let compare_tex = self.ensure_compare_pinned_texture(ctx);
                                        if let Some(tex) = compare_tex.as_ref() {
                                            let bg_style = self.fs_bg_style(ctx);
                                            Self::draw_compare_pinned_image(
                                                ui, image_rect, tex, zoom_pan, &bg_style, None,
                                            );
                                            self.fs_spread_layout = None;
                                        }
                                    } else {
                                        let fallback_compare_tex = if matches!(
                                            compare_mode,
                                            crate::app::CompareViewMode::Wipe { .. }
                                        ) {
                                            self.ensure_compare_pinned_texture(ctx)
                                        } else {
                                            None
                                        };
                                        self.draw_fs_spread(
                                            ui,
                                            ctx,
                                            image_rect,
                                            left,
                                            right,
                                            state.original_preview_active,
                                        );
                                        if let crate::app::CompareViewMode::Wipe { fraction } =
                                            compare_mode
                                        {
                                            if let Some(tex) = fallback_compare_tex.as_ref() {
                                                let bg_style = self.fs_bg_style(ctx);
                                                // 線 / clip はフィット後の実表示画像矩形基準にして
                                                // 切り替え位置と一致させる (single 側と同じ)。
                                                let ref_rect = Self::compare_image_draw_rect(
                                                    image_rect,
                                                    tex.size(),
                                                    zoom_pan,
                                                )
                                                .unwrap_or(image_rect);
                                                let wipe_x = ref_rect.left()
                                                    + ref_rect.width()
                                                        * fraction.clamp(0.05, 0.95);
                                                let clip = egui::Rect::from_min_max(
                                                    ref_rect.min,
                                                    egui::pos2(wipe_x, ref_rect.max.y),
                                                );
                                                Self::draw_compare_pinned_image(
                                                    ui,
                                                    image_rect,
                                                    tex,
                                                    zoom_pan,
                                                    &bg_style,
                                                    Some(clip),
                                                );
                                                Self::draw_compare_wipe_line(
                                                    ui, ref_rect, fraction,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        fs_media_ms = media_t0.elapsed().as_secs_f64() * 1000.0;

                        // ── 消しゴムモード: マスク塗り＋オーバーレイ描画 ──
                        // `is_spread_double` はキー入力ハンドラより前 (フレーム冒頭) で
                        // 計算されるので、見開き中に [E] を押した最初のフレームだけは
                        // `is_spread_double = true` のまま `erase_mode = true` になる
                        // フレーム単位の遷移期間が存在する。この 1 フレームでは見開き
                        // レンダ + 消しゴム overlay を併発させずスキップして、次フレームに
                        // 単一ページ表示で消しゴムを描画する。
                        if self.erase_mode && !is_spread_double && !state.original_preview_active {
                            let zp = self.fs_zoom_pan();
                            self.handle_erase_paint(ctx, image_rect, zp);
                            self.draw_erase_overlay(ui, ctx, image_rect, zp);
                            ctx.request_repaint();
                        } else if self.erase_mode {
                            // 遷移フレームでも次フレームを必ず描画させる (request_repaint)
                            ctx.request_repaint();
                        }

                        // ── 隠蔽加工モード: マスク塗り + オーバーレイ描画 ──
                        // 消しゴム同様、見開き中は 1 フレーム遷移期間で is_spread_double が
                        // true のまま conceal_mode = true になりうるので、その 1 フレームは
                        // overlay 描画をスキップして単一ページ表示で出す次フレームに描画する。
                        if self.conceal_mode
                            && !is_spread_double
                            && !state.original_preview_active
                        {
                            let zp = self.fs_zoom_pan();
                            self.handle_conceal_paint(ctx, image_rect, zp);
                            self.draw_conceal_overlay(ui, ctx, image_rect, zp);
                            ctx.request_repaint();
                        } else if self.conceal_mode {
                            ctx.request_repaint();
                        }

                        // ── テキスト注釈モード: キャンバス選択/移動 + パネル描画 ──
                        // (Inc 3b 座標逆写像 + 選択 / 3c ドラッグ移動 / 3d IME 編集)。
                        if self.text_mode
                            && !is_spread_double
                            && !state.original_preview_active
                        {
                            let zp = self.fs_zoom_pan();
                            // 子ダイアログ表示中はキャンバスのポインタ操作を止める (Codex P2:
                            // ダイアログ上のクリック/ドラッグが背面オブジェクトの選択/移動/削除に
                            // 漏れるのを防ぐ)。ダイアログ自体の描画は draw_text_overlay で行う。
                            if !self.text_subdialog_open() {
                                self.handle_text_canvas_input(ctx, image_rect, zp);
                            }
                            self.draw_text_overlay(ui, ctx, image_rect, zp);
                            ctx.request_repaint();
                        } else if self.text_mode {
                            ctx.request_repaint();
                        }

                        if self.local_adjust_mode
                            && !is_spread_double
                            && !state.original_preview_active
                        {
                            let zp = self.fs_zoom_pan();
                            self.handle_local_adjust_canvas_input(ctx, full_rect, image_rect, zp);
                            self.draw_local_adjust_canvas_overlay(ui, image_rect, zp);
                        } else if self.local_adjust_mode {
                            ctx.request_repaint();
                        }

                        if let Some((w, h)) = state.image_dims {
                            let image_size = [w as usize, h as usize];
                            let has_crop = self.export_crop_for_idx(fs_idx, image_size).is_some();
                            // crop はパイプライン最後段。crop の枠 / 暗転は「切り取りツール」
                            // または「素の通常表示」でのみ出す。各ツールは自分の手前までの
                            // 状態を表示する作りなので、crop より手前のツール (消しゴム /
                            // 補正レイヤー / 隠蔽加工) や 補正 / 分析モード中は出さない。
                            let in_earlier_tool = self.erase_mode
                                || self.local_adjust_mode
                                || self.conceal_mode
                                || self.adjustment_mode
                                || self.analysis_mode;
                            if !is_spread_double
                                && !state.original_preview_active
                                && (self.export_crop_mode || (has_crop && !in_earlier_tool))
                            {
                                // crop overlay は full_rect ではなく、実際に表示されている
                                // 画像のフィット矩形 (レターボックス + zoom/pan 反映) に
                                // 写像する (消しゴム / 隠蔽の image_layout と同じ作法)。
                                // 既知の制限: 回転 / 自由回転は未対応 (消しゴム / 隠蔽の
                                // overlay と同じく source 座標で写像する)。保存は source 方位の
                                // composite に crop を適用するので正しいが、回転表示中は
                                // overlay の向きが表示とズレる (アプリ全体の overlay 共通制限)。
                                let crop_image_rect = {
                                    let display_size = egui::vec2(w as f32, h as f32);
                                    let fit_scale = (full_rect.width() / display_size.x)
                                        .min(full_rect.height() / display_size.y);
                                    let (total_scale, center) = match self.fs_zoom_pan() {
                                        Some((zoom, pan)) => {
                                            (fit_scale * zoom, full_rect.center() + pan)
                                        }
                                        None => (fit_scale, full_rect.center()),
                                    };
                                    egui::Rect::from_center_size(
                                        center,
                                        display_size * total_scale,
                                    )
                                };
                                let pointer_allowed = !self.any_dialog_open()
                                    && !ctx.input(|i| {
                                        i.pointer.hover_pos().is_some_and(|p| {
                                            self.export_crop_panel_rect(full_rect).contains(p)
                                        })
                                    });
                                let used = self.draw_export_crop_overlay(
                                    ui,
                                    crop_image_rect,
                                    fs_idx,
                                    image_size,
                                    pointer_allowed,
                                );
                                if self.export_crop_mode || used {
                                    ctx.request_repaint();
                                }
                            } else if self.export_crop_mode {
                                ctx.request_repaint();
                            }
                        } else if self.export_crop_mode {
                            ctx.request_repaint();
                        }

                        let overlay_t0 = std::time::Instant::now();
                        // ── ルーペ (Shift ホールド / M トグル) ──
                        // 見開き・分析・補正モードでは内部で早期 return する。
                        // 消しゴムモードのマスクオーバーレイより上に載せる (最新状態を拡大)。
                        self.draw_fs_loupe_if_active(
                            ui, ctx, full_rect, fs_idx,
                            state.tex.as_ref(), state.thumb_tex.as_ref(),
                            is_spread_double,
                        );

                        // ── 透過背景インジケータ (B キー変更直後のみフェード表示) ──
                        self.draw_fs_transparent_bg_indicator(ui, full_rect);
                        self.draw_original_preview_indicator(ui, full_rect, state.original_preview_active);
                        self.sync_slideshow_anchor_for_frame(ctx, fs_idx, &state);
                        self.draw_slideshow_progress_indicator(ui, full_rect, ctx);
                        if !state.is_video {
                            self.draw_compare_pin_indicator(ui, full_rect, ctx);
                        }
                        if !state.is_video
                            && let Some(seek_target) =
                                self.draw_fullscreen_seek_overlay(ui, ctx, full_rect, fs_idx)
                        {
                            jump_to = Some(seek_target);
                        }
                        fs_overlay_ms = overlay_t0.elapsed().as_secs_f64() * 1000.0;

                        // ── 動画 (エラー / 「準備中」のみ) ──
                        // Playback HUD / seek bar / Perf graph 等は native presenter の
                        // egui overlay 側 (`src/video/native_presenter/overlay_draw.rs`)
                        // が描画する。ここでは egui main window 側に出すべき
                        // 軽量インジケータだけ担当する。
                        let hud_t0 = std::time::Instant::now();
                        if state.is_video {
                            self.draw_video_hud(ui, full_rect, fs_idx);
                        }
                        fs_hud_ms = hud_t0.elapsed().as_secs_f64() * 1000.0;

                        // ── チェックマーク ──
                        if self.checked.contains(&fs_idx) {
                            draw_fs_checkmark(ui, full_rect);
                        }

                        // ── 高解像度読込中インジケーター ──
                        let has_any_tex = state.tex.is_some() || state.thumb_tex.is_some();
                        let pdf_rerendering = self.fs_pending.contains_key(&fs_idx);
                        if state.is_loading && has_any_tex && !pdf_rerendering {
                            ui.painter().text(
                                image_rect.min + egui::vec2(16.0, 16.0),
                                egui::Align2::LEFT_TOP,
                                "高解像度 読込中...",
                                egui::FontId::proportional(14.0),
                                egui::Color32::from_rgba_unmultiplied(220, 220, 220, 180),
                            );
                        }

                        // ── PDF 再レンダリング進捗 ──
                        if self.fs_pending.contains_key(&fs_idx) {
                            let label = if matches!(self.items.get(fs_idx), Some(GridItem::PdfPage { .. })) {
                                "PDF 再レンダリング中..."
                            } else {
                                "読込中..."
                            };
                            let font = egui::FontId::proportional(13.0);
                            let pos = egui::pos2(image_rect.min.x + 12.0, image_rect.max.y - 12.0);
                            let galley = ui.painter().layout_no_wrap(
                                label.to_string(), font.clone(), egui::Color32::WHITE,
                            );
                            let text_rect = egui::Align2::LEFT_BOTTOM
                                .anchor_size(pos, galley.size());
                            let bg = text_rect.expand(4.0);
                            ui.painter().rect_filled(
                                bg, 4.0,
                                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200),
                            );
                            ui.painter().galley(text_rect.min, galley, egui::Color32::WHITE);
                        }

                        // ── 分析パネル（分析モード時、見開き中は無効）──
                        let panels_t0 = std::time::Instant::now();
                        if analysis_active {
                            let pixels = match self.fs_cache.get(&fs_idx) {
                                Some(FsCacheEntry::Static { pixels, .. }) => {
                                    Some(std::sync::Arc::clone(pixels))
                                }
                                _ => None,
                            };
                            let close_analysis = self.draw_analysis_panel(
                                ui, ctx, full_rect, image_rect, pixels.as_deref(),
                            );
                            if close_analysis {
                                // × も Z / ホバーボタンと同じ経路 (ズーム/パン引き継ぎ込み) に
                                // 揃える。analysis_mode は true なので OFF 方向にトグルされる。
                                self.toggle_analysis_mode();
                            }
                        } else if state.is_video {
                            // 動画は native presenter (独立 HWND) が描画とオーバーレイを担うため、
                            // 通常 eframe ビューポート側ではパネルを描画しない。タイル再オープン
                            // 要求 (S キーの遷移) のリトライだけはここで処理する。
                            #[cfg(windows)]
                            {
                                if (self.video_tile_mode_active || self.video_tile_reopen_pending)
                                    && self.video_tile_state.is_none()
                                {
                                    let now = std::time::Instant::now();
                                    let deadline =
                                        *self.video_tile_reopen_deadline.get_or_insert_with(|| {
                                            now + std::time::Duration::from_secs(3)
                                        });
                                    if now >= deadline {
                                        self.video_tile_reopen_pending = false;
                                        self.video_tile_reopen_deadline = None;
                                    } else {
                                        let screen = self.video_tile_layout_size(fs_idx, ctx);
                                        self.video_tile_state =
                                            self.build_video_tile_state_for(fs_idx, screen);
                                        if self.video_tile_state.is_some() {
                                            self.video_tile_mode_active = true;
                                            self.video_tile_reopen_pending = false;
                                            self.video_tile_reopen_deadline = None;
                                        } else {
                                            let retry = std::time::Duration::from_millis(80);
                                            let wait =
                                                deadline.saturating_duration_since(now).min(retry);
                                            ctx.request_repaint_after(wait);
                                        }
                                    }
                                }
                            }
                        } else if self.local_adjust_mode
                            && !compare_wipe_active
                            && !panorama_mode_active_now
                        {
                            self.draw_local_adjust_panel(ctx, full_rect);
                        } else if self.export_crop_mode
                            && !compare_wipe_active
                            && !panorama_mode_active_now
                        {
                            let image_size =
                                state.image_dims.map(|(w, h)| [w as usize, h as usize]);
                            self.draw_export_crop_panel(ctx, full_rect, fs_idx, image_size);
                        } else if adjustment_active {
                            // ── オーバーレイモード: 左パネル + 右パネル 同時表示 ──
                            // 描画と当たり判定で同じ rect を使う (adjustment_panel_rect 参照)。
                            let panel_rect = adjustment_panel_rect(full_rect);
                            self.draw_adjustment_panel(ui, panel_rect, state.image_dims);
                            // 右側にメタデータパネルも同時表示（show_metadata_panel の状態に関係なく）
                            if !is_spread_double {
                                self.draw_metadata_panel_forced(ui, ctx, full_rect);
                            }
                        } else if panorama_mode_active_now {
                            // 360 モード中はメタデータ / 補正 / 分析パネルを全て抑止
                            // (docs/panorama-360-view-plan.md フィードバック対応)。
                            // 上部ホバーバーの 360 / × / window だけが表示される。
                            //
                            // Phase 2a: NeedsUserConfirmation バナー (§3.6.2 / §3.6.4)。
                            // 大画像 (>200 MP) で確認待ちのときだけ描画。
                            self.draw_pano_confirmation_banner(ui, ctx, full_rect, fs_idx);
                        } else if !is_spread_double
                            && !compare_wipe_active
                            && !self.is_overlay_edit_mode_active()
                        {
                            // ── メタデータパネル（通常モード：TABキー固定 or 右端ホバー）──
                            // 消しゴム / 隠蔽加工モード中は自前パネルとの競合 + 編集集中度
                            // 低下を避けるためメタデータ右パネル全体を抑制する。
                            let right_panel_visible =
                                self.draw_metadata_panel(ui, ctx, full_rect);
                            let _ = right_panel_visible;
                        }
                        fs_panels_ms = panels_t0.elapsed().as_secs_f64() * 1000.0;

                        // ── ホバーバー ──
                        let hover_bar_t0 = std::time::Instant::now();
                        let mut bar_rotate_cw = false;
                        let mut bar_rotate_ccw = false;
                        let spread_before = self.spread_mode;
                        let reading_flow_before = self.reading_flow;
                        let reading_direction_before = self.reading_direction;
                        // AI 処理情報を計算（ホバーバーのファイル情報に表示）
                        let ai_info_model_name: String;
                        let ai_upscale_info = if self.ai_upscale_enabled || self.ai_denoise_model.is_some() {
                            ai_info_model_name = self.ai_model_label(fs_idx, false);
                            // 処理後のサイズ
                            if let Some(tex) = self.current_final_composite_texture(fs_idx) {
                                let s = tex.size_vec2();
                                Some((ai_info_model_name.as_str(), s.x as u32, s.y as u32))
                            } else if self.ai_upscale_enabled {
                                if let Some((w, h)) = state.image_dims {
                                    if crate::ai::upscale::should_process(w, h, self.settings.ai_upscale_skip_px) {
                                        Some((ai_info_model_name.as_str(), w * 4, h * 4))
                                    } else {
                                        None
                                    }
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        // 消しゴム / 隠蔽加工モード中は上部バーを抑制 (自前パネルと競合させない)。
                        if !self.is_overlay_edit_mode_active() {
                            let saved_nav = nav_delta;
                            let has_page_override = self.adjustment_page_params.contains_key(&fs_idx);
                            // Phase 6: 動画モードかどうかを上ホバーバーに通知する。
                            // 動画なら video_meta (duration, bitrate) と画像 dims (= 動画解像度) を
                            // 動画用 info text 構築のために流す。
                            let is_video_mode = state.is_video;
                            let mut video_dims: Option<(u32, u32)> = None;
                            let mut video_meta: Option<(f64, i64)> = None;
                            #[cfg(windows)]
                            if is_video_mode {
                                if let Some(crate::fs_animation::FsCacheEntry::Video {
                                    player, ..
                                }) = self.fs_cache.get(&fs_idx)
                                {
                                    if let Some(info) = player.info() {
                                        video_dims = Some((info.width, info.height));
                                        video_meta =
                                            Some((info.duration_secs, info.bit_rate_bps));
                                    }
                                }
                            }
                            let display_dims = if is_video_mode {
                                video_dims
                            } else {
                                state.image_dims
                            };
                            #[cfg(windows)]
                            let tile_active = self.video_tile_mode_active;
                            #[cfg(not(windows))]
                            let tile_active = false;
                            let mut tile_pressed = false;
                            // VST ボタン: 動画モード + VST3 機能有効のときだけ表示
                            let show_vst3_button =
                                cfg!(windows) && is_video_mode && self.settings.vst3_enabled;
                            let vst3_panel_open = self.show_vst3_manager;
                            let mut vst3_pressed = false;
                            let mut copy_capture_pressed = false;
                            let mut window_mode_pressed = false;
                            // ウィンドウ / 全画面 切り替えボタン: 静止画フルスクリーン
                            // (= 非動画、Windows) のときだけ出す。動画は native HUD 側に
                            // 専用トグルがある。
                            let show_window_toggle = cfg!(windows) && !is_video_mode;
                            let slideshow_was_playing = self.slideshow_playing;
                            // 360 度パノラマビュー: 検出 + アクティブ状態を計算
                            // (docs/panorama-360-view-plan.md §5.3)。is_panorama_mode_active
                            // は state Some + detect Some を要求するが、ボタン表示は
                            // detect Some だけで十分 (= 360 ボタン押下で初期化可能)。
                            // 非対応画像でもボタンは disabled で常に表示する。
                            let panorama_trigger = self.detect_panorama(fs_idx);
                            let panorama_active = self.panorama_state.is_some()
                                && panorama_trigger.is_some();
                            // panorama_mode_active=true なら他のボタン (info / analysis /
                            // spread / 補正 / rotate / capture / VST / play / tile) は全て隠す。
                            let panorama_mode_active = panorama_active;
                            let mut panorama_pressed = false;
                            let fit_mode = self.effective_fullscreen_fit_mode();
                            let mut fit_mode_choice: Option<FullscreenFitMode> = None;
                            let mut bar_analysis_pressed = false;
                            Self::draw_fs_hover_bar(
                                ui, ctx, full_rect,
                                &state.location_display,
                                display_dims, state.image_file_size,
                                state.image_downscaled,
                                &mut close_fs, &mut nav_delta,
                                &mut self.show_metadata_panel,
                                false,
                                &mut self.slideshow_playing,
                                &mut self.settings.slideshow_interval_secs,
                                &mut bar_rotate_cw, &mut bar_rotate_ccw,
                                self.analysis_mode,
                                &mut bar_analysis_pressed,
                                panorama_trigger,
                                panorama_active,
                                panorama_mode_active,
                                &mut panorama_pressed,
                                &mut self.spread_mode,
                                &mut self.reading_flow,
                                &mut self.reading_direction,
                                &mut self.spread_popup_open,
                                is_spread_double,
                                ai_upscale_info,
                                &mut self.adjustment_mode,
                                &mut self.local_adjust_mode,
                                has_page_override,
                                fit_mode,
                                &mut self.fit_popup_open,
                                &mut fit_mode_choice,
                                state.pdf_content_type,
                                is_video_mode,
                                video_meta,
                                tile_active,
                                &mut tile_pressed,
                                show_vst3_button,
                                vst3_panel_open,
                                &mut vst3_pressed,
                                &mut copy_capture_pressed,
                                show_window_toggle,
                                embedded,
                                &mut window_mode_pressed,
                                self.cursor_hidden,
                            );
                            if copy_capture_pressed {
                                self.copy_image_capture_to_clipboard(ctx, fs_idx);
                            }
                            // 分析ボタン押下は Z キーと同じ経路へ合流 (副作用込み、Codex P1)。
                            if bar_analysis_pressed && !is_spread_double {
                                self.toggle_analysis_mode();
                            }
                            // ズーム/フィットモード: ツールバーボタンはメニューから選択
                            // (0 キーは従来どおり循環)。
                            if let Some(mode) = fit_mode_choice {
                                self.set_fullscreen_fit_mode_for_current(ctx, fs_idx, mode);
                            }
                            // 360 度パノラマビュー: トグル
                            if panorama_pressed {
                                self.toggle_panorama_mode(fs_idx);
                            }
                            // ウィンドウ / 全画面 切り替えボタンが押された。
                            #[cfg(windows)]
                            if window_mode_pressed {
                                self.toggle_still_window_mode();
                                // 描画先 (embedded ⇔ 専用 viewport) の切替は次フレームの
                                // render_fullscreen_viewport で起きる。静止画は
                                // handle_fs_repaint が自発 repaint しないので、ルート
                                // ビューポートの次フレームを明示要求する。さもないと
                                // モード切替が次の入力まで反映されない (Codex P2)。
                                ctx.request_repaint_of(egui::ViewportId::ROOT);
                            }
                            if !slideshow_was_playing && self.slideshow_playing {
                                self.schedule_next_slideshow_from_now();
                            }
                            // ▦ タイルボタンが押されたら toggle_video_tile_mode に dispatch
                            #[cfg(windows)]
                            if tile_pressed {
                                let screen = self.video_tile_layout_size(fs_idx, ctx);
                                self.toggle_video_tile_mode(fs_idx, screen);
                            }
                            // VST ボタンが押されたら「管理パネル + 全プラグイン GUI」を一斉トグル。
                            // workspace 全体を「VST3 を見る」「VST3 を畳む」の 2 状態に切り替える。
                            // Compact mode is remembered, but only applied while VST GUIs are visible.
                            #[cfg(windows)]
                            if vst3_pressed {
                                let opening = !self.show_vst3_manager;
                                self.show_vst3_manager = opening;
                                std::sync::Arc::clone(&self.dsp_bridge)
                                    .set_all_guis_visible_async(opening);
                                self.settings.vst3_gui_visible = opening;
                                self.settings.save();
                            }
                            #[cfg(not(windows))]
                            if vst3_pressed {
                                self.show_vst3_manager = !self.show_vst3_manager;
                            }
                            // ホイール/キーで確定した nav_delta を保護
                            if nav_locked { nav_delta = saved_nav; }
                        }
                        fs_hover_bar_ms = hover_bar_t0.elapsed().as_secs_f64() * 1000.0;
                        if bar_rotate_cw { self.rotate_image_cw(fs_idx); }
                        if bar_rotate_ccw { self.rotate_image_ccw(fs_idx); }

                        // ── フルスクリーン用コンテキストメニュー ──
                        if self.show_fs_context_menu(ctx) {
                            close_fs = true;
                        }

                        // ── フルスクリーン左下ステータス表示 ──
                        if self.ai_upscale_enabled || self.ai_denoise_model.is_some() {
                            self.draw_fs_ai_status(ui, fs_idx);
                        }

                        // ── 右上フィードバックトースト ──
                        self.draw_feedback_toast(ui, full_rect, ctx);

                        // ── スタンプ埋め込み worker の進行表示 (中央「読み込み中」) ──
                        self.draw_stamp_embed_overlay(ui, full_rect, ctx);

                        // ── 注釈ベイク worker の進行表示 (中央「テキスト処理中…」) ──
                        self.draw_comic_bake_overlay(ui, full_rect, ctx);

                        // 動画ブックマーク名編集ダイアログは native presenter overlay の
                        // 中で描画される (= `native_presenter/overlay_draw.rs::draw_native_*`)。
                        // eframe ビューポートからは描画しない。

                        // ── 中央の境界ヒント (最初/最後の項目です…) ──
                        self.draw_boundary_hint(ui, full_rect, ctx);

                        // ── スロット保存ダイアログ ──
                        self.draw_slot_save_dialog(ctx);
                        self.draw_export_dialog(ctx);
                        self.draw_export_progress_dialog(ctx);

                        let spread_changed_from_direction =
                            self.reading_direction != reading_direction_before
                                && self.sync_spread_mode_from_reading_direction();
                        // ホバーバーのポップアップからモードが変更された場合
                        if self.spread_mode != spread_before {
                            if !spread_changed_from_direction {
                                self.update_reading_direction_from_spread_mode(self.spread_mode);
                            }
                            self.persist_current_spread_mode();
                            if !self.reading_flow.is_paged() {
                                self.reset_continuous_reading_transform();
                                self.disable_non_paged_fullscreen_modes(fs_idx);
                            }
                            if self.spread_mode.is_spread() && self.analysis_mode {
                                self.reset_analysis_mode();
                            }
                            self.adjust_spread_target = crate::app::AdjustSpreadTarget::Left;
                            if !spread_changed_from_direction {
                                self.normalize_spread_position(ctx);
                            }
                        }
                        let reading_flow_changed = self.reading_flow != reading_flow_before;
                        if reading_flow_changed || self.reading_direction != reading_direction_before
                        {
                            self.reset_continuous_reading_transform();
                            if reading_flow_changed {
                                self.set_default_fullscreen_fit_for_flow(
                                    ctx,
                                    fs_idx,
                                    self.reading_flow,
                                );
                            }
                            if !self.reading_flow.is_paged() {
                                self.disable_non_paged_fullscreen_modes(fs_idx);
                            }
                            self.persist_current_reading_flow();
                            ctx.request_repaint();
                        }
                    });
                fs_central_ms = central_t0.elapsed().as_secs_f64() * 1000.0;

                // パネル / HUD が全て非表示で 3 秒以上アイドルならカーソルを隠す。
                // 動画 native presenter 経路は `update_cursor_icon` で
                // `CursorIcon::None` → `SetCursor(None)` に解決する (= 完全非表示)。
                // 静止画 (egui) 経路は winit が `CursorIcon::None` を Windows API で
                // 正しく非表示処理する。VST3 manager 等のダイアログ表示中は
                // `any_dialog_open()` で `fs_ui_is_clean` が false を返すため抑制される。
                //
                // 状態機械:
                // - マウス操作あり / UI 表示中: `cursor_last_activity = Some(now)`,
                //   `cursor_hidden = false` (idle タイマをリセット)。これにより
                //   一時停止 (HUD 表示中) の間にタイマが古くなり、再開直後に即座に
                //   カーソルが消える事故を防ぐ。
                // - clean かつ idle >= 3 秒、または `cursor_hidden` が立っている:
                //   `CursorIcon::None` を毎フレーム適用 (egui は frame 跨ぎで sticky に
                //   ならないため)、`cursor_hidden = true` をセット。
                {
                    let full_rect = ctx.content_rect();
                    let is_video = state.is_video;
                    let clean = self.fs_ui_is_clean(ctx, full_rect, is_video);
                    if !clean {
                        // UI が出ている間はタイマを today に戻して countdown を停止。
                        self.cursor_last_activity = Some(std::time::Instant::now());
                        if self.cursor_hidden {
                            ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(true));
                        }
                        self.cursor_hidden = false;
                    }
                    let idle = self
                        .cursor_last_activity
                        .map(|t| t.elapsed().as_secs_f32())
                        .unwrap_or(0.0);
                    let threshold = crate::video::native_presenter::CURSOR_HIDE_IDLE_SECS;
                    if clean && (idle >= threshold || self.cursor_hidden) {
                        if !self.cursor_hidden {
                            ctx.send_viewport_cmd(egui::ViewportCommand::CursorVisible(false));
                        }
                        ctx.set_cursor_icon(egui::CursorIcon::None);
                        self.cursor_hidden = true;
                    } else if clean {
                        // カウントダウン中: 残時間後に再描画予約してきっかり 3 秒で隠す。
                        let remain = (threshold - idle).max(0.05);
                        ctx.request_repaint_after(std::time::Duration::from_secs_f32(remain));
                    }
                }

                // ── VST3 プラグイン管理ウィンドウ + チェーンエディタ (フルスクリーン中も表示) ──
                // egui::Window はビューポート単位で z-order が独立しているので、
                // フルスクリーンビューポート内で `show_vst3_manager` を呼ぶことで
                // 動画の手前に管理パネルを描画できる。動画分析中に動画を見ながら
                // プラグインを追加・順序入れ替え・バイパス切替・GUI 表示できる。
                #[cfg(windows)]
                {
                    let vst_t0 = std::time::Instant::now();
                    self.show_vst3_manager(ctx);
                    self.vst3_pump_gui_signals();
                    fs_vst_manager_ms = vst_t0.elapsed().as_secs_f64() * 1000.0;
                }

                // 編集用追加パック取得ダイアログ (フルスクリーン中も表示)。テキスト注釈 /
                // 補正の編集導線 (オノマトペ追加・被写体マスク) からフルスクリーン中に
                // 起動されるので、ここで描かないと背後 (メインビューポート) に隠れて
                // 「DL ボタンを押しても何も起きない」状態になる (実機 FB 2026-06-06)。
                // メイン update 側は fullscreen 中スキップするので二重描画にはならない。
                self.show_editing_addon_dialog(ctx);

                self.fs_prev_foreground_hwnd = current_foreground_hwnd();
                fs_closure_ms = closure_t0.elapsed().as_secs_f64() * 1000.0;
            };
            if embedded {
                // in-window 静止画: メインウィンドウの egui ctx に直接描画する。
                render_fs_body(main_ctx, true);
                #[cfg(windows)]
                if hide_viewport_after_embedded_paint {
                    // 先に main 側へ画像を描いてから古い専用 viewport を隠す。
                    // これで背面の一覧が DWM に露出する隙間を作らない。
                    self.hide_native_video_black_backdrop_if_shown(main_ctx);
                }
            } else {
                // 従来: 専用フルスクリーン viewport を出してそこに描画する。
                main_ctx.show_viewport_immediate(fs_id, fs_builder, |vp_ctx, _class| {
                    render_fs_body(vp_ctx, false);
                });
            }
        }
        let fs_viewport_ms = fs_viewport_t0.elapsed().as_secs_f64() * 1000.0;
        if crate::perf::is_enabled() && fs_viewport_ms > 8.0 {
            let fs_outer_ms = (fs_viewport_ms - fs_closure_ms).max(0.0);
            let fs_closure_tracked_ms = fs_setup_ms + fs_central_ms + fs_vst_manager_ms;
            let fs_closure_unaccounted_ms = (fs_closure_ms - fs_closure_tracked_ms).max(0.0);
            let fs_central_tracked_ms = fs_input_ms
                + fs_media_ms
                + fs_overlay_ms
                + fs_hud_ms
                + fs_panels_ms
                + fs_hover_bar_ms;
            let fs_central_unaccounted_ms = (fs_central_ms - fs_central_tracked_ms).max(0.0);
            let (video_playing, video_pending_frames, video_state, video_seq) =
                match self.fs_cache.get(&fs_idx) {
                    Some(FsCacheEntry::Video { player, .. }) => (
                        player.is_playing(),
                        player.pending_frames() as i64,
                        player.engine_state_code() as i64,
                        player.displayed_frame_seq() as i64,
                    ),
                    _ => (false, -1, -1, -1),
                };
            crate::perf::event(
                "ui",
                "fs_viewport_breakdown",
                None,
                self.input_seq,
                &[
                    ("idx", serde_json::Value::from(fs_idx as i64)),
                    ("total_ms", serde_json::Value::from(fs_viewport_ms)),
                    ("outer_ms", serde_json::Value::from(fs_outer_ms)),
                    ("closure_ms", serde_json::Value::from(fs_closure_ms)),
                    (
                        "closure_unaccounted_ms",
                        serde_json::Value::from(fs_closure_unaccounted_ms),
                    ),
                    (
                        "central_unaccounted_ms",
                        serde_json::Value::from(fs_central_unaccounted_ms),
                    ),
                    ("setup_ms", serde_json::Value::from(fs_setup_ms)),
                    ("central_ms", serde_json::Value::from(fs_central_ms)),
                    ("input_ms", serde_json::Value::from(fs_input_ms)),
                    ("media_ms", serde_json::Value::from(fs_media_ms)),
                    ("overlay_ms", serde_json::Value::from(fs_overlay_ms)),
                    ("hud_ms", serde_json::Value::from(fs_hud_ms)),
                    ("panels_ms", serde_json::Value::from(fs_panels_ms)),
                    ("hover_bar_ms", serde_json::Value::from(fs_hover_bar_ms)),
                    ("vst_manager_ms", serde_json::Value::from(fs_vst_manager_ms)),
                    ("is_video", serde_json::Value::from(fs_state_is_video)),
                    ("gpu_video", serde_json::Value::from(fs_state_gpu_video)),
                    ("video_playing", serde_json::Value::from(video_playing)),
                    (
                        "video_pending_frames",
                        serde_json::Value::from(video_pending_frames),
                    ),
                    ("video_state", serde_json::Value::from(video_state)),
                    ("video_seq", serde_json::Value::from(video_seq)),
                    (
                        "vst3_manager",
                        serde_json::Value::from(self.show_vst3_manager),
                    ),
                ],
            );
        }

        if !embedded {
            // embedded のときは専用 viewport を作っていないので shown フラグは
            // 立てない (close 後の viewport 後始末も走らせない)。
            self.fs_viewport_shown = true;
        }

        // ── ナビゲーション & スライドショー処理 ──
        self.handle_fs_navigation(
            ctx,
            close_fs,
            close_to_page_list,
            ctrl_nav,
            sibling_nav,
            nav_delta,
            jump_to,
            fs_idx,
        );

        // hint_start_before と一致 = このフレームで再設定されていない
        // (= 境界でない方向への移動、別キー入力、等)。操作があれば即消去。
        // 再設定されていた場合 (= 引き続き境界に突き当たった) はそのまま残す。
        if had_user_input_in_frame {
            let hint_now = self.fs_boundary_hint.map(|h| h.started_at());
            if hint_now.is_some() && hint_now == hint_start_before {
                self.fs_boundary_hint = None;
            }
        }
        self.handle_fs_repaint(ctx, fs_idx, state.is_video);
    }

    // ── 状態準備ヘルパー ────────────────────────────────────────────────

    /// アニメーションフレームを進める（メインコンテキストの時刻を使う）。
    fn advance_animation(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
        if is_video {
            return;
        }
        let now = ctx.input(|i| i.time);
        if let Some(FsCacheEntry::Animated {
            frames,
            current_frame,
            next_frame_at,
            ..
        }) = self.fs_cache.get_mut(&fs_idx)
        {
            if now >= *next_frame_at && !frames.is_empty() {
                *current_frame = (*current_frame + 1) % frames.len();
                let delay = frames[*current_frame].1.max(0.02);
                *next_frame_at = now + delay;
            }
        }
    }

    /// 上部ホバーバー・読込中プレースホルダ共通で使う、`idx` 位置の表示用パス文字列。
    /// 通常は `<folder>\<filename>`、ZIP/PDF 内は `<archive> > <entry>` / `<pdf> > Page N`。
    fn location_display_for(&self, idx: usize) -> String {
        let item = self.items.get(idx);
        let filename = item.map(|i| i.name().to_string()).unwrap_or_default();
        let base_folder = self
            .effective_folder()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        compute_location_display(item, &base_folder, &filename)
    }

    /// 見開きの各ページ用: 読込中ブランチが走りそうなときだけパスを計算する。
    /// steady state (fs_cache or thumbnail hit) では空文字列を返し、呼び出し先の
    /// `draw_centered_elided_label` は空なら描画をスキップする。
    fn location_display_for_loading(&self, idx: usize) -> String {
        let has_display_tex = matches!(
            self.fs_cache.get(&idx),
            Some(FsCacheEntry::Static { .. })
                | Some(FsCacheEntry::Animated { .. })
                | Some(FsCacheEntry::Video { .. })
        ) || matches!(
            self.thumbnails.get(idx),
            Some(ThumbnailState::Loaded { .. })
        );
        if has_display_tex {
            String::new()
        } else {
            self.location_display_for(idx)
        }
    }

    /// フルスクリーン描画に必要な状態を事前計算する。
    fn prepare_fullscreen_state(&mut self, ctx: &egui::Context, fs_idx: usize) -> FsFrameState {
        let is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
        let separator_text = match self.items.get(fs_idx) {
            Some(GridItem::ZipSeparator { dir_display }) => Some(dir_display.clone()),
            _ => None,
        };
        let is_separator = separator_text.is_some();
        let original_preview_active = self.original_preview_active(ctx, fs_idx);

        let tex = self.resolve_fs_processed_texture(ctx, fs_idx, original_preview_active);

        let fs_load_failed = matches!(self.fs_cache.get(&fs_idx), Some(FsCacheEntry::Failed));

        let thumb_tex = match self.thumbnails.get(fs_idx) {
            Some(ThumbnailState::Loaded { tex, .. }) => Some(tex.clone()),
            _ => None,
        }
        .or_else(|| {
            // ナビ ロック中で新ページのサムネ未準備のときは旧ページのテクスチャを
            // 流用して「ファイル名だけが切り替わる」状態を回避する。サムネが
            // Loaded になった瞬間 `poll_fs_nav_lock` がロックを解除し holdover が
            // 解放される。
            if self.fs_nav_is_locked() {
                self.fs_holdover_tex.clone()
            } else {
                None
            }
        });

        let mut location_display = self.location_display_for(fs_idx);
        // 動画の場合は decode 経路 (HW/SW) と GPU パスを末尾に追記。
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            if let Some(info) = player.info() {
                let hw = if info.hw_decode_active { "HW" } else { "SW" };
                let mut tags = vec![hw.to_string()];
                #[cfg(windows)]
                {
                    if info.gpu_path_active {
                        tags.push("GPU".into());
                    }
                }
                location_display = format!("{}  [{}]", location_display, tags.join("/"));
            }
        }
        // image_dims は常に元画像のサイズを表示する（AI アップスケール後のサイズではない）。
        // AI テクスチャが選ばれている場合でも、元画像のサイズを使う。
        // GPU 上限超過で worker が clamp した画像は `source_dims` に原寸が入っており、
        // ホバー表示はそれを使う。clamp が発動していたら後段で警告マーカーを出す。
        // fs_cache が未到達でも `fs_early_dims` (ヘッダ解析結果) にヒントがあれば使う。
        let (image_dims, image_downscaled): (Option<(u32, u32)>, bool) = {
            match self.fs_cache.get(&fs_idx) {
                Some(FsCacheEntry::Static {
                    tex, source_dims, ..
                }) => {
                    let tex_size = tex.size_vec2();
                    match source_dims {
                        Some([sw, sh]) => {
                            let clamped = (*sw, *sh) != (tex_size.x as usize, tex_size.y as usize);
                            (Some((*sw as u32, *sh as u32)), clamped)
                        }
                        None => (Some((tex_size.x as u32, tex_size.y as u32)), false),
                    }
                }
                Some(FsCacheEntry::Animated {
                    frames,
                    current_frame,
                    ..
                }) => {
                    let dims = frames.get(*current_frame).map(|(h, _)| {
                        let s = h.size_vec2();
                        (s.x as u32, s.y as u32)
                    });
                    (dims, false)
                }
                Some(FsCacheEntry::Video { player, .. }) => {
                    if let Some(info) = player.info() {
                        (Some((info.width, info.height)), false)
                    } else {
                        (None, false)
                    }
                }
                _ => {
                    // まだ fs_cache が埋まっていない段階: ヘッダ解析済みなら原寸ヒントを使う。
                    // 原寸が GPU 上限超なら、本デコード完了後に clamp が発動することが
                    // 事前に判るのでこの時点で⚠ダウンスケール警告を出してよい。
                    if let Some([sw, sh]) = self.fs_early_dims.get(&fs_idx).copied() {
                        let will_clamp =
                            sw > crate::app::MAX_TEXTURE_DIM || sh > crate::app::MAX_TEXTURE_DIM;
                        (Some((sw as u32, sh as u32)), will_clamp)
                    } else {
                        // 最後の頼り: フォールバックサムネイル/テクスチャから寸法を取得。
                        let dims = tex.as_ref().map(|t| {
                            let s = t.size_vec2();
                            (s.x as u32, s.y as u32)
                        });
                        (dims, false)
                    }
                }
            }
        };
        let image_file_size: Option<u64> = self
            .image_metas
            .get(fs_idx)
            .and_then(|m| m.map(|(_, sz)| sz.max(0) as u64));
        let is_loading =
            !is_video && !is_separator && !fs_load_failed && !self.fs_cache.contains_key(&fs_idx);
        #[cfg(windows)]
        let vst3_waiting_for_video = is_video
            && self.vst3_deferred_video_open == Some(fs_idx)
            && self.vst3_startup_load.is_some();
        #[cfg(not(windows))]
        let vst3_waiting_for_video = false;
        #[cfg(windows)]
        if vst3_waiting_for_video {
            location_display = self.vst3_startup_progress_text();
        }

        let pdf_content_type = match self.items.get(fs_idx) {
            Some(GridItem::PdfPage { content_type, .. }) => *content_type,
            _ => None,
        };

        FsFrameState {
            is_video,
            separator_text,
            original_preview_active,
            tex,
            thumb_tex,
            location_display,
            image_dims,
            image_file_size,
            image_downscaled,
            is_loading,
            vst3_waiting_for_video,
            fs_load_failed,
            pdf_content_type,
        }
    }

    /// フルスクリーンビューポートの ViewportBuilder を構築する。
    #[cfg(windows)]
    fn native_video_presenter_hwnd_for_fs(&self, fs_idx: usize) -> Option<u64> {
        if let Some((hwnd, _)) = self.pending_native_video_output_hwnds_for_fs(fs_idx) {
            return Some(hwnd);
        }
        match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                let hwnd = player.native_presenter_hwnd();
                (hwnd != 0).then_some(hwnd)
            }
            _ => None,
        }
    }

    #[cfg(windows)]
    fn native_video_backdrop_target_for_fs(&self, fs_idx: usize) -> bool {
        matches!(self.items.get(fs_idx), Some(GridItem::Video(_)))
    }

    /// in-window モードで静止画 (= 非動画) フルスクリーンを表示中かどうか。
    ///
    /// true のとき `render_fullscreen_viewport` は専用 viewport ではなく
    /// メインウィンドウの egui ctx に直接 CentralPanel を描く (embedded)。
    /// 動画は native presenter (独立 / 子 HWND) 経路なのでここでは除外する。
    /// これにより in-window 動画 ⇔ 静止画をホイールで往復しても画面モードが
    /// 全画面↔ウィンドウで切り替わらず、一貫して main ウィンドウ内に収まる。
    ///
    /// 判定はフルスクリーンで表示しうる静止画系アイテム
    /// (Image / ZipImage / PdfPage / ZipSeparator) の **明示的な許可リスト**で行う。
    /// 「非 Video なら何でも embedded」にすると、`fullscreen_idx` が範囲外や
    /// コンテナ (Folder/ZipFile/PdfFile) を指す異常時に embedded 扱いになり、
    /// `update()` がグリッド描画を抑止して黒画面に閉じ込められる恐れがあるため。
    #[cfg(windows)]
    pub(crate) fn fullscreen_embedded_still_active(&self) -> bool {
        self.native_video_in_window_active
            && self.fullscreen_idx.is_some_and(|idx| {
                matches!(
                    self.items.get(idx),
                    Some(
                        GridItem::Image(_)
                            | GridItem::ZipImage { .. }
                            | GridItem::PdfPage { .. }
                            | GridItem::ZipSeparator { .. }
                    )
                )
            })
    }

    #[cfg(windows)]
    fn native_video_presenter_pending_for_fs(&self, fs_idx: usize) -> bool {
        if self.pending_native_video_output_active_for_fs(fs_idx) {
            let Some((hwnd, _)) = self.pending_native_video_output_hwnds_for_fs(fs_idx) else {
                return true;
            };
            return self.native_video_front_synced_hwnd != hwnd;
        }
        match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                if player.native_presenter_pending() {
                    return true;
                }
                let hwnd = player.native_presenter_hwnd();
                hwnd != 0 && self.native_video_front_synced_hwnd != hwnd
            }
            _ => false,
        }
    }

    /// 動画フルスクリーンの黒 backdrop viewport が生きていれば、DWM フェードを
    /// 抑止しつつ 1 フレームで隠して破棄させる。`fs_viewport_shown` が false の
    /// ときは **何もしない** — 死んでいる viewport を `show_viewport_immediate` で
    /// 復活させてしまうと、`need_show=false` 経路で可視のまま再生成され、白い
    /// クラス背景がフラッシュするため (Plan B トグルで実害、2026-05-22)。
    /// `keep_fullscreen_viewport_alive` の cleanup と同じ手順。
    #[cfg(windows)]
    fn hide_native_video_black_backdrop_if_shown(&mut self, ctx: &egui::Context) {
        if !self.fs_viewport_shown {
            return;
        }
        let fs_id = self.fullscreen_viewport_id();
        let fs_builder = self.build_fullscreen_viewport_builder().with_visible(false);
        ctx.show_viewport_immediate(fs_id, fs_builder, |_ctx, _class| {});
        crate::dwm_transitions::disable_transitions_for_thread_windows();
        ctx.send_viewport_cmd_to(fs_id, egui::ViewportCommand::Visible(false));
        self.fs_viewport_shown = false;
    }

    #[cfg(windows)]
    fn show_native_video_black_backdrop(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let fs_id = self.fullscreen_viewport_id();
        let need_show = !self.fs_viewport_shown;
        let fs_builder = self.build_fullscreen_viewport_builder_with_transparency(false);
        let fs_builder = if need_show {
            // Create the fullscreen backdrop HWND hidden first. The DWM
            // transition flag can only be applied after the HWND exists, so a
            // visible initial create can animate before the attribute lands.
            fs_builder.with_visible(false)
        } else {
            fs_builder
        };
        let expected_physical_rect = self.fullscreen_backdrop_physical_rect();
        let mut close_fs = false;
        ctx.show_viewport_immediate(fs_id, fs_builder, |ctx, _class| {
            // Visible な fullscreen viewport なので、native 動画の黒 backdrop 中も
            // IME 状態だけは通常 viewport と同じ入口で更新する。
            self.update_ime_state(ctx);
            if need_show {
                crate::dwm_transitions::disable_transitions_for_thread_windows();
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            if let Some(keys) = fullscreen_shortcut_event_summary(ctx) {
                let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
                crate::logger::log(format!(
                    "[fs-key] source=native-backdrop focused={} foreground=0x{:x} keys={}",
                    focused,
                    current_foreground_hwnd(),
                    keys
                ));
            }
            // Codex 2周目 P1: ノーマライズスキャン中なら handle_native_video_key_event の
            // ESC は cancel ルートに乗る。直後の `consume_key` で ESC を取るとフォールバック側で
            // close_fs = true になり、cancel + close が同フレームで走ってしまうので、
            // この呼び出し前後でスキャン状態の変化 (= ESC で cancel された) を検出して
            // close 判定をスキップする。
            let normalize_active_before = self.normalize_state.is_some();
            if !self.ime_input_active() {
                for key in native_video_key_events_from_ctx(ctx) {
                    self.handle_native_video_key_event(ctx, fs_idx, key);
                }
            }
            let normalize_cancelled_this_frame =
                normalize_active_before && self.normalize_state.is_none();
            let close_requested = ctx.input(|i| i.viewport().close_requested());
            let escape_pressed = !self.ime_input_active()
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
            if (close_requested || escape_pressed) && !normalize_cancelled_this_frame {
                close_fs = true;
            }
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(egui::Color32::BLACK))
                .show(ctx, |_ui| {});
        });
        if self.native_video_presenter_hwnd_for_fs(fs_idx).is_none()
            && let (Some(main_hwnd), Some(expected)) = (self.main_hwnd, expected_physical_rect)
        {
            let raised = crate::dwm_transitions::raise_visible_thread_window_matching_rect(
                windows::Win32::Foundation::HWND(main_hwnd as *mut _),
                expected,
            );
            if need_show {
                crate::logger::log(format!(
                    "[native-video] raised fullscreen backdrop hwnd=0x{:x} main=0x{:x}",
                    raised.map(|hwnd| hwnd.0 as usize).unwrap_or(0),
                    main_hwnd as usize
                ));
            }
        }
        self.fs_viewport_shown = true;
        if close_fs {
            self.handle_fullscreen_close_request();
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn build_fullscreen_viewport_builder(&self) -> egui::ViewportBuilder {
        self.build_fullscreen_viewport_builder_with_transparency(true)
    }

    #[cfg(windows)]
    fn fullscreen_backdrop_physical_rect(&self) -> Option<windows::Win32::Foundation::RECT> {
        let center = self.last_outer_rect.map(|r| r.center())?;
        let ppp = self.last_pixels_per_point;
        let rect = crate::monitor::get_monitor_logical_rect_at(center.x * ppp, center.y * ppp)?;
        Some(windows::Win32::Foundation::RECT {
            left: (rect.min.x * ppp).round() as i32,
            top: (rect.min.y * ppp).round() as i32,
            right: (rect.max.x * ppp).round() as i32,
            bottom: (rect.max.y * ppp).round() as i32,
        })
    }

    fn build_fullscreen_viewport_builder_with_transparency(
        &self,
        transparent: bool,
    ) -> egui::ViewportBuilder {
        let center = self.last_outer_rect.map(|r| r.center());
        let ppp = self.last_pixels_per_point;

        let monitor_rect =
            center.and_then(|c| crate::monitor::get_monitor_logical_rect_at(c.x * ppp, c.y * ppp));

        let b = egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(transparent)
            .with_taskbar(false);
        match monitor_rect {
            Some(rect) => b
                .with_position(rect.min)
                .with_inner_size([rect.width(), rect.height()]),
            None => b.with_fullscreen(true),
        }
    }

    // ── 見開きペアリング ────────────────────────────────────────────────

    /// build_nav_indices の結果をキャッシュして返す。
    fn get_nav_indices(&mut self) -> Vec<usize> {
        if let Some(ref cached) = self.cached_nav_indices {
            return cached.clone();
        }
        let nav = build_nav_indices(&self.items, self.current_grid_order());
        self.cached_nav_indices = Some(nav.clone());
        nav
    }

    /// 現在の見開きモードとインデックスからペア表示を解決する。
    pub(crate) fn resolve_spread_pair(&mut self, idx: usize) -> SpreadPair {
        if !self.spread_mode.is_spread() {
            return SpreadPair::Single;
        }

        let nav = self.get_nav_indices();
        let Some(pos) = nav.iter().position(|&i| i == idx) else {
            return SpreadPair::Single;
        };

        // ペアリング開始位置: cover (表紙あり) なら 1 (先頭ページ単独 + 1 ページずれ)。
        // 「1 ページずらし」(Ctrl+←/→) は cover/非cover を切り替えて表現するので、ここは
        // mode の has_cover だけを見ればよい (専用の位相ステータスは持たない)。
        let pair_start = if self.spread_mode.has_cover() { 1 } else { 0 };

        // pair_start=1 のとき pos=0 は相方が居ないので常に単独 (= あぶれた先頭ページ)
        if pair_start == 1 && pos == 0 {
            return SpreadPair::Single;
        }

        // 横長画像は単独
        if is_landscape(idx, &self.fs_cache, &self.thumbnails) {
            return SpreadPair::Single;
        }

        // ペア内の位置を計算 (0-indexed from pair_start)
        let relative = pos - pair_start;
        let is_first_of_pair = relative % 2 == 0;

        // ペア相手の pos を決定
        let partner_pos = if is_first_of_pair { pos + 1 } else { pos - 1 };

        // パートナーが存在しない or 横長の場合は単独
        let partner_idx = match nav.get(partner_pos) {
            Some(&pidx) => pidx,
            None => return SpreadPair::Single,
        };
        if is_landscape(partner_idx, &self.fs_cache, &self.thumbnails) {
            return SpreadPair::Single;
        }

        // 小さい pos のインデックスと大きい pos のインデックス
        let (small_idx, large_idx) = if is_first_of_pair {
            (idx, partner_idx)
        } else {
            (partner_idx, idx)
        };

        // LTR: 左=小, 右=大  /  RTL: 左=大, 右=小
        if self.spread_mode.is_rtl() {
            SpreadPair::Double {
                left: large_idx,
                right: small_idx,
            }
        } else {
            SpreadPair::Double {
                left: small_idx,
                right: large_idx,
            }
        }
    }

    /// 現在画面に出ている見開きレイアウトを優先してペアを返す。
    ///
    /// Ctrl+S / Ctrl+E は「表示中のものを保存する」操作なので、直近の描画で確定した
    /// `fs_spread_layout` が現在 idx を含む場合はそちらを正とする。レイアウトが無い
    /// フレームでは通常のペアリング規則へフォールバックする。
    pub(crate) fn resolve_visible_spread_pair(&mut self, idx: usize) -> SpreadPair {
        if self.spread_mode.is_spread()
            && let Some(layout) = self.fs_spread_layout
            && (layout.left_idx == idx || layout.right_idx == idx)
        {
            return SpreadPair::Double {
                left: layout.left_idx,
                right: layout.right_idx,
            };
        }
        self.resolve_spread_pair(idx)
    }

    /// 見開きモードでの nav_delta を計算する。
    /// 見開き表示中は 2 ページ送り、Single 表示 (横長等) や非見開きは 1 ページ送り。
    /// 「1 ページずらし」は Ctrl+←/→ ([`Self::compute_spread_offset_nudge`]) が担当する
    /// ので、ここでは扱わない。
    pub(crate) fn spread_nav_delta(&mut self, base_delta: i32) -> i32 {
        if !self.spread_mode.is_spread() {
            return base_delta;
        }
        let fs_idx = match self.fullscreen_idx {
            Some(i) => i,
            None => return base_delta,
        };
        // 現在の表示が Single（横長等）なら1ページ送り
        match self.resolve_spread_pair(fs_idx) {
            SpreadPair::Single => base_delta,
            SpreadPair::Double { .. } => base_delta * 2,
        }
    }

    /// Ctrl+←/→ の「見開き 1 ページずらし」。現在ページ (`fs_idx`) を読み順で `dir`
    /// (+1=前方 / -1=後方) に 1 つ動かし、その新ページがペアの先頭になるよう
    /// **見開きモード (cover/非cover)** を決めて返す。専用の位相ステータスを持たず
    /// `spread_mode` を直接切り替えるので、ホバーバー/ポップアップ/数字キーと表示状態が
    /// 一致し、`spread_db` でフォルダ単位に永続化される。
    ///
    /// 戻り値 `(new_idx, new_mode)`。移動先が範囲外なら `None` (= 端で no-op)。
    /// 純粋ロジック (副作用なし) なのでユニットテスト可能。
    pub(crate) fn compute_spread_offset_nudge(
        &mut self,
        fs_idx: usize,
        dir: i32,
    ) -> Option<(usize, crate::settings::SpreadMode)> {
        use crate::settings::SpreadMode;
        let nav = self.get_nav_indices();
        let pos = nav.iter().position(|&i| i == fs_idx)?;
        let new_pos = pos as i32 + dir;
        if new_pos < 0 || new_pos as usize >= nav.len() {
            return None;
        }
        let new_pos = new_pos as usize;
        let new_idx = nav[new_pos];
        // new_pos がペアの先頭 (pair_start = new_pos % 2) になるよう、現在の読み方向を保った
        // まま cover/非cover を選ぶ (cover = pair_start 1 = 先頭ページ単独 + 1 ページずれ)。
        let want_cover = new_pos % 2 == 1;
        let new_mode = match (self.spread_mode.is_rtl(), want_cover) {
            (false, false) => SpreadMode::Ltr,
            (false, true) => SpreadMode::LtrCover,
            (true, false) => SpreadMode::Rtl,
            (true, true) => SpreadMode::RtlCover,
        };
        Some((new_idx, new_mode))
    }

    pub(crate) fn fullscreen_cursor_state(&self) -> FullscreenCursorState {
        FullscreenCursorState {
            last_activity: self.cursor_last_activity,
            hidden: self.cursor_hidden,
        }
    }

    pub(crate) fn restore_fullscreen_cursor_state(
        &mut self,
        ctx: &egui::Context,
        state: FullscreenCursorState,
    ) {
        self.cursor_last_activity = state.last_activity;
        self.cursor_hidden = state.hidden;
        if state.hidden {
            ctx.send_viewport_cmd_to(
                self.fullscreen_viewport_id(),
                egui::ViewportCommand::CursorVisible(false),
            );
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
    }

    /// 見開きモード切替後、fullscreen_idx をペアの先頭に正規化する。
    pub(crate) fn normalize_spread_position(&mut self, ctx: &egui::Context) {
        if !self.spread_mode.is_spread() {
            return;
        }
        let Some(idx) = self.fullscreen_idx else {
            return;
        };
        let nav = self.get_nav_indices();
        let Some(pos) = nav.iter().position(|&i| i == idx) else {
            return;
        };

        let pair_start = if self.spread_mode.has_cover() { 1 } else { 0 };
        if pos < pair_start {
            return; // 表紙位置 (先頭ページ単独)
        }
        let relative = pos - pair_start;
        if relative % 2 != 0 {
            // ペアの2番目にいるので1番目に戻す
            let new_idx = nav[pos - 1];
            let cursor_state = self.fullscreen_cursor_state();
            self.open_fullscreen(new_idx);
            self.restore_fullscreen_cursor_state(ctx, cursor_state);
            self.selected = Some(new_idx);
        }
    }

    // ── キー入力 ────────────────────────────────────────────────────────

    /// フルスクリーン表示中にメインビューポートへ届いたキーを、フルスクリーン操作として処理する。
    ///
    /// VST editor の owner 切り替えや cross-process focus handoff のタイミングによっては、
    /// マウスイベントは fullscreen viewport に届く一方で、キーだけ main viewport に届くことがある。
    /// main 側の通常ショートカットは fullscreen 中にブロックされるため、ここで同じ key handler に通す。
    pub(crate) fn handle_fullscreen_root_key_input(&mut self, ctx: &egui::Context) -> bool {
        let Some(fs_idx) = self.fullscreen_idx else {
            return false;
        };
        // in-window 静止画 (embedded) では本体 (render_fullscreen_viewport) の
        // handle_fs_key_input が同じ main ctx 上で直接キーを処理する。ここでも
        // 処理するとナビが二重発火するので委譲する。true を返して
        // handle_keyboard / handle_clipboard_shortcuts (= グリッド用) を抑止する。
        #[cfg(windows)]
        if self.fullscreen_embedded_still_active() {
            return true;
        }
        let Some(keys) = fullscreen_shortcut_event_summary(ctx) else {
            return false;
        };

        let root_focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        crate::logger::log(format!(
            "[fs-key] source=root focused={} foreground=0x{:x} keys={}",
            root_focused,
            current_foreground_hwnd(),
            keys
        ));

        let spread_pair = self.resolve_spread_pair(fs_idx);
        let is_spread_double = matches!(spread_pair, SpreadPair::Double { .. });
        let key_action = self.handle_fs_key_input(ctx, fs_idx, is_spread_double);

        if key_action.nav_delta != 0 {
            self.bump_input_seq(
                "fs_root_key",
                Some(&format!("delta={}", key_action.nav_delta)),
            );
        } else if key_action.ctrl_nav.is_some() {
            self.bump_input_seq("fs_root_ctrl_nav", None);
        } else if key_action.sibling_nav.is_some() {
            self.bump_input_seq("fs_root_sibling_nav", None);
        } else if key_action.close || key_action.close_to_page_list {
            self.bump_input_seq("fs_root_close_key", None);
        }

        self.handle_fs_navigation(
            ctx,
            key_action.close,
            key_action.close_to_page_list,
            key_action.ctrl_nav,
            key_action.sibling_nav,
            key_action.nav_delta,
            key_action.jump_to,
            fs_idx,
        );
        true
    }

    /// フルスクリーンのキー入力を処理し、アクションを返す。
    fn handle_fs_key_input(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        is_spread_double: bool,
    ) -> FsKeyAction {
        // マウスドライバ / AHK 経由で積まれた進む/戻る pending は **early-return より前に
        // drain** する。フォーカス無し / モーダル表示中で早期 return すると pending が
        // 次フレームに持ち越されて誤発火するため (Codex P2)。ブロック中は count を捨てる。
        let (browser_back_count, browser_forward_count) = crate::take_pending_mouse_nav();

        let has_focus = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        let mut action = FsKeyAction {
            close: false,
            close_to_page_list: false,
            nav_delta: 0,
            ctrl_nav: None,
            sibling_nav: None,
            jump_to: None,
        };

        if !has_focus {
            return action;
        }
        // モーダルダイアログ表示中はキー入力を奪わない
        // (テキスト入力やダイアログ内の Enter/Esc 処理を優先)
        if self.any_modal_dialog_open_for_fullscreen_keys() {
            return action;
        }

        if Self::consume_pipeline_debug_shortcut(ctx) {
            self.start_pipeline_debug_export(ctx, fs_idx);
            return action;
        }

        // 消しゴムモード中は専用ショートカットのみ有効にし、通常のフルスクリーンショートカット
        // (矢印ナビ、R/L 回転、I メタデータ等) を無効化する。
        if self.erase_mode {
            return self.handle_erase_keys(ctx, fs_idx);
        }

        // 隠蔽加工モード中: 専用ショートカット (S/B/L/I/V/H/R/O、D/F、Ctrl+Z、Delete、
        // 矢印移動、Esc / Ctrl+M) に切り替える。通常のフルスクリーンショートカットは無効化。
        if self.text_mode {
            return self.handle_text_keys(ctx, fs_idx);
        }

        if self.conceal_mode {
            return self.handle_conceal_keys(ctx, fs_idx);
        }

        if self.local_adjust_mode {
            // 多角形作成中の Ctrl+Z は頂点戻し。ただし consume_key は matches_logically 判定の
            // ため Modifiers::CTRL 指定でも Ctrl+Shift+Z (やり直し) を吸ってしまう
            // (undo_ops.rs handle_meta_undo_keys の注記と同じ罠)。そこでイベントの修飾子を
            // matches_exact(CTRL) で完全一致判定し、Ctrl 単独の Z だけを消費する。Ctrl+Shift+Z は
            // 残して下の handle_meta_undo_keys で redo へ流す。FS viewport で stale になる
            // i.modifiers の状態読みは使わない。
            if self.local_adjust_mask_tool == crate::app::LocalAdjustMaskTool::Polygon
                && !self.local_adjust_mask_lasso_points.is_empty()
                && ctx.input_mut(|i| {
                    let before = i.events.len();
                    i.events.retain(|e| {
                        !matches!(
                            e,
                            egui::Event::Key {
                                key: egui::Key::Z,
                                pressed: true,
                                modifiers,
                                ..
                            } if modifiers.matches_exact(egui::Modifiers::CTRL)
                        )
                    });
                    i.events.len() != before
                })
            {
                self.local_adjust_mask_lasso_points.pop();
                self.show_feedback_toast("多角形: 頂点を戻しました".to_string());
                return action;
            }
            self.handle_meta_undo_keys(ctx);
            if self.keymap.consume_action(ctx, KeyAction::LaShowSource) {
                self.local_adjust_show_source = !self.local_adjust_show_source;
                self.show_feedback_toast(if self.local_adjust_show_source {
                    "補正レイヤー: 元画像表示 ON".to_string()
                } else {
                    "補正レイヤー: 元画像表示 OFF".to_string()
                });
            }
            if self.keymap.consume_action(ctx, KeyAction::LaShowMask) {
                self.local_adjust_show_mask = !self.local_adjust_show_mask;
                self.show_feedback_toast(if self.local_adjust_show_mask {
                    "補正レイヤー: マスク表示 ON".to_string()
                } else {
                    "補正レイヤー: マスク表示 OFF".to_string()
                });
            }
            if self.keymap.consume_action(ctx, KeyAction::LaPaintAdd) {
                self.local_adjust_mask_paint_add = true;
                self.show_feedback_toast("手動マスク: 描画".to_string());
            }
            if self.keymap.consume_action(ctx, KeyAction::LaPaintErase) {
                self.local_adjust_mask_paint_add = false;
                self.show_feedback_toast("手動マスク: 消去".to_string());
            }
            if self.keymap.consume_action(ctx, KeyAction::LaToolBrush) {
                self.set_local_adjust_mask_tool_from_shortcut(
                    crate::app::LocalAdjustMaskTool::Brush,
                );
            }
            if self.keymap.consume_action(ctx, KeyAction::LaToolEdgeBrush) {
                self.set_local_adjust_mask_tool_from_shortcut(
                    crate::app::LocalAdjustMaskTool::EdgeBrush,
                );
            }
            if self.keymap.consume_action(ctx, KeyAction::LaToolGapFill) {
                self.set_local_adjust_mask_tool_from_shortcut(
                    crate::app::LocalAdjustMaskTool::GapFillBrush,
                );
            }
            if self.keymap.consume_action(ctx, KeyAction::LaToolLasso) {
                self.set_local_adjust_mask_tool_from_shortcut(
                    crate::app::LocalAdjustMaskTool::Lasso,
                );
            }
            if self.keymap.consume_action(ctx, KeyAction::LaToolPolygon) {
                self.set_local_adjust_mask_tool_from_shortcut(
                    crate::app::LocalAdjustMaskTool::Polygon,
                );
            }
            if self.keymap.consume_action(ctx, KeyAction::LaToolSelect) {
                self.set_local_adjust_mask_tool_from_shortcut(
                    crate::app::LocalAdjustMaskTool::Select,
                );
            }
            if self.keymap.consume_action(ctx, KeyAction::LaToolLine) {
                self.set_local_adjust_mask_tool_from_shortcut(
                    crate::app::LocalAdjustMaskTool::Line,
                );
            }
            if self.keymap.consume_action(ctx, KeyAction::LaToolVLine) {
                self.set_local_adjust_mask_tool_from_shortcut(
                    crate::app::LocalAdjustMaskTool::VertLine,
                );
            }
            if self.keymap.consume_action(ctx, KeyAction::LaToolHLine) {
                self.set_local_adjust_mask_tool_from_shortcut(
                    crate::app::LocalAdjustMaskTool::HorizLine,
                );
            }
            if self.keymap.consume_action(ctx, KeyAction::LaToolRect) {
                self.set_local_adjust_mask_tool_from_shortcut(
                    crate::app::LocalAdjustMaskTool::Rect,
                );
            }
            if self.keymap.consume_action(ctx, KeyAction::LaToolEllipse) {
                self.set_local_adjust_mask_tool_from_shortcut(
                    crate::app::LocalAdjustMaskTool::Ellipse,
                );
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
                && self.commit_local_adjust_polygon_from_shortcut(fs_idx)
            {
                self.show_feedback_toast("多角形マスクを確定しました".to_string());
                ctx.request_repaint();
                return action;
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete))
                && self.delete_selected_local_adjust_shape_from_shortcut(fs_idx)
            {
                self.show_feedback_toast("図形マスクを削除しました".to_string());
                ctx.request_repaint();
                return action;
            }
            // 図形が選択されているときだけ矢印 (nudge) / ブラケット (rotate) を consume する。
            // 未選択時に consume すると、補正パネルでフォーカス中のスライダー等へ矢印が
            // 届かなくなり微調整できなくなる (ラボ復旧時の回帰防止)。
            if self.local_adjust_selected_shape.is_some() {
                // 高速化/15度スナップの修飾キーは OS 直読み (FS viewport の stale 回避)。
                // 矢印/ブラケットの consume_key は NONE/CTRL 両方を拾うので step/snap だけ
                // OS 状態で決めれば、Ctrl/Shift を離した後の残留も起きない。
                let ctrl_held = ctrl_held_via_os();
                let step = if ctrl_held {
                    crate::ui_adjustment_panel::LOCAL_ADJUST_NUDGE_PIXELS_FAST
                } else {
                    crate::ui_adjustment_panel::LOCAL_ADJUST_NUDGE_PIXELS
                };
                let (mut dx, mut dy) = (0.0_f32, 0.0_f32);
                ctx.input_mut(|i| {
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
                        || i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowLeft)
                    {
                        dx -= step;
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
                        || i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowRight)
                    {
                        dx += step;
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                        || i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowUp)
                    {
                        dy -= step;
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                        || i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowDown)
                    {
                        dy += step;
                    }
                });
                if self.nudge_selected_local_adjust_shape_from_shortcut(fs_idx, dx, dy) {
                    ctx.request_repaint();
                    return action;
                }
                let rotate_step = if ctrl_held {
                    crate::ui_adjustment_panel::LOCAL_ADJUST_ROTATE_DEG_STEP_FAST
                } else {
                    crate::ui_adjustment_panel::LOCAL_ADJUST_ROTATE_DEG_STEP
                };
                let mut rotate_deg = 0.0_f32;
                ctx.input_mut(|i| {
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::OpenBracket)
                        || i.consume_key(egui::Modifiers::CTRL, egui::Key::OpenBracket)
                    {
                        rotate_deg -= rotate_step;
                    }
                    if i.consume_key(egui::Modifiers::NONE, egui::Key::CloseBracket)
                        || i.consume_key(egui::Modifiers::CTRL, egui::Key::CloseBracket)
                    {
                        rotate_deg += rotate_step;
                    }
                });
                if self.rotate_selected_local_adjust_shape_from_shortcut(
                    fs_idx,
                    rotate_deg.to_radians(),
                    shift_held_via_os(),
                ) {
                    ctx.request_repaint();
                    return action;
                }
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                if self.cancel_local_adjust_canvas_edit_from_shortcut() {
                    self.show_feedback_toast("編集中の図形操作を解除しました".to_string());
                    ctx.request_repaint();
                } else {
                    self.local_adjust_mode = false;
                    self.local_adjust_add_layer_dialog_open = false;
                    self.local_adjust_change_mask_dialog_open = false;
                    self.local_adjust_effect_picker_dialog_open = false;
                }
            }
            return action;
        }

        if self.export_crop_mode {
            return self.handle_export_crop_keys(ctx, fs_idx);
        }

        // 動画フルスクリーン中は専用キーマップ (Space=play/pause、Enter=play/pause、
        // Shift+Enter=外部プレイヤー、←→=シーク、↑↓=音量、M=mute、L=loop) を
        // 画像系のキー処理より先に走らせる。
        // 動画 HUD 2 段化リデザイン (Phase 1): Space は動画モードでは play/pause トグルに
        // 変更し、`handle_video_input` 側で consume する。画像モードでは従来通り画像選択トグル。
        // フルスクリーン用コンテキストメニュー表示中は奪わない (= メニュー側の Enter
        // 選択操作を優先、Codex Phase 5.1 P2 反映)。
        let is_video_fs = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)))
            && self.fs_context_menu_idx.is_none();
        let video_horizontal_arrow_key = is_video_fs
            && ctx.input(|i| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::ArrowLeft | egui::Key::ArrowRight,
                            pressed: true,
                            ..
                        }
                    )
                })
            });
        let video_shift_vertical_key = is_video_fs
            && ctx.input(|i| {
                i.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::ArrowUp | egui::Key::ArrowDown,
                            pressed: true,
                            modifiers,
                            ..
                        } if modifiers.shift
                    )
                })
            });
        if is_video_fs {
            let video_path = if let Some(GridItem::Video(p)) = self.items.get(fs_idx) {
                Some(p.clone())
            } else {
                None
            };
            self.handle_video_input(ctx, fs_idx, video_path.as_deref());
        }

        // ナビゲーションキーは input_mut で消費して、パネル内ウィジェット（スライダー等）に
        // 奪われないようにする
        let esc = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
        // 静止画フルスクリーンでは Enter も Esc と同等に「フルスクリーン解除」トリガー。
        // グリッドで Enter (Double click 相当) → 開く、フルスクリーンで Enter → 戻る、
        // のトグル動作を成立させる。判定は副作用ゼロの
        // `should_close_fullscreen_on_enter` に集約 (= unit test 可能)。
        //
        // suppress ガード解除: Enter が現在押下されていなければ
        // `fs_suppress_enter_close_until_release` を false にリセット (= 次の新規押下から
        // close が有効化される)。「Enter で open → 同フレームで close」を防ぐ仕組み。
        let enter_currently_down = ctx.input(|i| i.key_down(egui::Key::Enter));
        if !enter_currently_down {
            self.fs_suppress_enter_close_until_release = false;
        }
        let is_video_item = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
        let enter_consume_ok = should_close_fullscreen_on_enter(
            is_video_item,
            self.ime_input_active(),
            self.fs_context_menu_idx.is_some(),
            self.fs_suppress_enter_close_until_release,
        );
        let enter_close = !esc
            && enter_consume_ok
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let esc = esc || enter_close;
        // 左右キーは上下と分離して処理（RTL 反転のため）
        let ctrl_d = self.keymap.consume_action(ctx, KeyAction::FsCtrlNavNext);
        let ctrl_u = self.keymap.consume_action(ctx, KeyAction::FsCtrlNavPrev);
        // Ctrl+←/→: 見開き「1 ページずらし」(応急補正)。Single モードでは 1 ページ移動に
        // フォールバックする。動画は Ctrl+←/→ が 30 秒シークなので画像 (= !is_video_fs) のみ消費。
        let ctrl_left = !is_video_fs
            && self
                .keymap
                .consume_action(ctx, KeyAction::FsSpreadShiftLeft);
        let ctrl_right = !is_video_fs
            && self
                .keymap
                .consume_action(ctx, KeyAction::FsSpreadShiftRight);
        let ctrl_page_down = self.keymap.consume_action(ctx, KeyAction::FsSiblingNext);
        let ctrl_page_up = self.keymap.consume_action(ctx, KeyAction::FsSiblingPrev);
        // PageUp/Down のスクロール用 consume も、実際に連続描画している条件
        // (continuous_reading_active_for_idx) に揃える。reading_flow だけで判定すると、
        // 非対応アイテム/解析/比較中に PageUp/Down を消費しておきながら無反応 (デッドキー) になる。
        let continuous_mode_for_page_keys = self.continuous_reading_active_for_idx(fs_idx);
        let page_down = continuous_mode_for_page_keys
            && self
                .keymap
                .consume_action(ctx, KeyAction::FsContinuousScrollForward);
        let page_up = continuous_mode_for_page_keys
            && self
                .keymap
                .consume_action(ctx, KeyAction::FsContinuousScrollBack);
        // マウス戻る/進む (Extra1/Extra2 = native XButton) を Ctrl+↑/↓ と等価に扱う。
        let mouse_back = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra1));
        let mouse_forward = ctx.input(|i| i.pointer.button_pressed(egui::PointerButton::Extra2));
        // WM_APPCOMMAND / VK_BROWSER_BACK/FORWARD 経路 (上で関数頭で drain 済み) を消費。
        // 詳細は main.rs の `install_mouse_nav_hook` 参照。
        let browser_back = browser_back_count > 0;
        let browser_forward = browser_forward_count > 0;
        let arrow_right = ctx.input_mut(|i| {
            !video_horizontal_arrow_key
                && (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)
                    || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowRight))
        });
        let arrow_left = ctx.input_mut(|i| {
            !video_horizontal_arrow_key
                && (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)
                    || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowLeft))
        });
        let arrow_down = ctx.input_mut(|i| {
            !video_shift_vertical_key
                && (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)
                    || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowDown))
        });
        let arrow_up = ctx.input_mut(|i| {
            !video_shift_vertical_key
                && (i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)
                    || i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowUp))
        });
        let key_home = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Home));
        let key_end = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::End));
        let key_i = self.keymap.consume_action(ctx, KeyAction::FsToggleMetadata);
        // Space: スライドショー関連 (変数名の紛らわしさ回避のため key_space)。
        // 動画モードでは `handle_video_input` 側で play/pause として消費するため、ここでは
        // 画像系処理に流さない。**`is_video_fs` ではなく純粋な「現在アイテムが Video か」で
        // gate する** こと: `is_video_fs` は context menu 表示中に false になるため、それで
        // gate すると context menu open の動画で Space → 画像系チェックトグルへ流出する
        // (Codex Phase 1 P2 指摘)。
        let current_item_is_video_for_space =
            matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
        let key_space = !current_item_is_video_for_space
            && self.keymap.consume_action(ctx, KeyAction::FsSpaceCheck);
        let key_ctrl_s_capture = !is_video_fs
            && self.fs_context_menu_idx.is_none()
            && self.keymap.consume_action(ctx, KeyAction::FsCapture);
        let key_ctrl_e_export = !is_video_fs
            && self.fs_context_menu_idx.is_none()
            && self.keymap.consume_action(ctx, KeyAction::FsExport);
        let key_compare_x = !is_video_fs
            && self.fs_context_menu_idx.is_none()
            && self.keymap.consume_action(ctx, KeyAction::FsCompareToggle);
        let key_compare_alt_c = !is_video_fs
            && self.fs_context_menu_idx.is_none()
            && self.keymap.consume_action(ctx, KeyAction::FsCompareDiff);
        let key_compare_shift_c = !is_video_fs
            && self.fs_context_menu_idx.is_none()
            && self.keymap.consume_action(ctx, KeyAction::FsCompareWipe);
        let key_compare_c = !is_video_fs
            && self.fs_context_menu_idx.is_none()
            && !key_compare_alt_c
            && !key_compare_shift_c
            && self.keymap.consume_action(ctx, KeyAction::FsCompareCycle);
        // S: スライドショー 再生/停止 (旧 P キー、左手で押しやすいよう S に移行)
        let key_s = self.keymap.consume_action(ctx, KeyAction::FsSlideshow);
        let key_r = self.keymap.consume_action(ctx, KeyAction::FsRotateCw);
        let key_l = self.keymap.consume_action(ctx, KeyAction::FsRotateCcw);
        let key_z = self.keymap.consume_action(ctx, KeyAction::FsAnalysis);
        // V: 360 度パノラマビューワーモード トグル (docs/panorama-360-view-plan.md)。
        // 消しゴムモード中は ui_erase 側が V (vertical line tool) を先に consume するので、
        // ここで奪っても消しゴム中は届かない (= mode-scoped 共存)。
        let key_v_panorama = self.keymap.consume_action(ctx, KeyAction::FsPanorama);
        let key_g = self.keymap.consume_action(ctx, KeyAction::FsPixelGrid);
        let key_m = self
            .keymap
            .consume_action(ctx, KeyAction::FsLoupeLockToggle);
        let key_e = self.keymap.consume_action(ctx, KeyAction::FsEraseMode);
        // B: 透過画像の背景サイクル。消しゴムモードでは ui_erase が B (筆ツール) を既に消費している。
        let key_b_bg = self.keymap.consume_action(ctx, KeyAction::FsBgCycle);
        // 360 モード中は他モード切替系のキーを抑止 (= フィードバック反映の「機能制限モード」)。
        // - 抑止対象: Z (分析) / S (スライドショー) / E (消しゴム) / M (ルーペ) / B (bg cycle)
        //   / I (メタデータ) / C 系 (比較)
        // - 抑止しない: V (= 360 を抜ける手段)、Esc / 矢印 / Wheel / F1-F5 / BS (= ナビ・レーティング)
        let pano_active_now = self.is_panorama_mode_active(fs_idx);
        let continuous_reading = !self.reading_flow.is_paged();
        let key_z = key_z && !pano_active_now;
        let key_s = key_s && !pano_active_now;
        let key_e = key_e && !pano_active_now;
        let key_m = key_m && !pano_active_now;
        let key_b_bg = key_b_bg && !pano_active_now;
        let key_g = key_g && !pano_active_now;
        let key_i = key_i && !pano_active_now;
        let key_compare_x = key_compare_x && !pano_active_now;
        let key_compare_alt_c = key_compare_alt_c && !pano_active_now;
        let key_compare_shift_c = key_compare_shift_c && !pano_active_now;
        let key_compare_c = key_compare_c && !pano_active_now;
        let key_z = key_z && !continuous_reading;
        let key_e = key_e && !continuous_reading;
        let key_v_panorama = key_v_panorama && !continuous_reading;
        let key_compare_x = key_compare_x && !continuous_reading;
        let key_compare_alt_c = key_compare_alt_c && !continuous_reading;
        let key_compare_shift_c = key_compare_shift_c && !continuous_reading;
        let key_compare_c = key_compare_c && !continuous_reading;
        // P: 現在表示中アイテムを親コンテナの代表サムネに固定 / 解除。
        // 動画フルスクリーンの P は handle_video_input が先に「現在フレームをピン留め」として
        // consume するため、ここでは静止画系アイテムだけを対象にする。
        let current_item_is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
        let key_p_pin = !current_item_is_video && self.keymap.consume_action(ctx, KeyAction::FsPin);

        // コンテナ★ (既定: Shift+F1〜F6): 開いている画像が属するコンテナ
        // (フォルダ / ZIP / PDF) にレーティング / 解除。
        // current_folder がそのまま親コンテナなので、そちらに書き込めば一覧画面で★絞り込みできる。
        let container_rating_key = self.keymap.consume_rating_action(ctx, true);
        if let Some(stars) = container_rating_key
            && self.set_current_folder_rating(stars)
        {
            self.show_container_rating_toast(stars);
        }

        // レーティング 1〜5 / 解除 (既定: F1〜F6)
        let rating_key = self.keymap.consume_rating_action(ctx, false);
        if let Some(stars) = rating_key {
            // Undo 用にフルスクリーン現在ページの before/after を 1 件分積む。
            let before = self.rating_cache.get(&fs_idx).copied().unwrap_or(0);
            if before != stars {
                let summary = if stars == 0 {
                    "★解除".to_string()
                } else {
                    format!("★{stars}")
                };
                self.capture_rating_undo(vec![(fs_idx, before, stars)], summary);
            }
            self.set_rating(fs_idx, stars);
            // レーティング変更でフィルタ境界を跨ぐ可能性があるので visible_indices 再計算。
            self.rebuild_visible_indices();
            if stars == 0 {
                self.show_feedback_toast("[★解除]".to_string());
            } else {
                self.show_feedback_toast(format!("[{}]", "★".repeat(stars as usize)));
            }
        }
        if key_p_pin {
            self.toggle_folder_pin_for_idx(fs_idx);
        }

        // F7/F8: 消しゴムマスクスロット 1/2 をフルスクリーン表示のまま現ページに適用
        // (消しゴムモードに入らず、1 キーで inpaint までを一気に実行)
        // F9/F10: 隠蔽マスクスロット 1/2 を現ページに適用
        // Shift+F7/F8/F9/F10: 適用済みマスクを削除
        let delete_erase_mask = self
            .keymap
            .consume_action(ctx, KeyAction::FsDeleteEraseMask);
        let delete_conceal_mask = self
            .keymap
            .consume_action(ctx, KeyAction::FsDeleteConcealMask);
        let key_f7 = self.keymap.consume_action(ctx, KeyAction::FsApplyErase1);
        let key_f8 = self.keymap.consume_action(ctx, KeyAction::FsApplyErase2);
        let key_f9 = self.keymap.consume_action(ctx, KeyAction::FsApplyConceal1);
        let key_f10 = self.keymap.consume_action(ctx, KeyAction::FsApplyConceal2);
        if delete_erase_mask {
            self.delete_erase_mask_in_viewing_mode();
        }
        if delete_conceal_mask {
            self.delete_conceal_mask_in_viewing_mode();
        }
        if key_f7 {
            self.apply_slot_in_viewing_mode(ctx, 1);
        }
        if key_f8 {
            self.apply_slot_in_viewing_mode(ctx, 2);
        }
        if key_f9 {
            self.apply_conceal_slot_in_viewing_mode(1);
        }
        if key_f10 {
            self.apply_conceal_slot_in_viewing_mode(2);
        }

        // F11: ウィンドウ表示 ⇔ 全画面表示 トグル (ホバーバー × の左の
        // 「ウィンドウ / 全画面 切り替え」ボタンと同じ動作)。
        //
        // 消しゴムモード中は本関数冒頭の `if self.erase_mode { return ... }` で
        // 早期 return するため自動的に無効化される (ホバーバーのトグルボタン自体も
        // 消しゴム中は非表示で挙動を揃えている。erase_mask_texture が ctx-bound で
        // viewport 切替時に invalidate されるのを避けるため意図的)。
        //
        // 動画アイテム上では skip する。動画は native presenter が
        // `handle_native_video_key_event` の 0x7A arm で F11 を直接拾い
        // `toggle_video_window_mode()` (presenter rebuild を伴う) を呼ぶ。
        // 起動直後の black backdrop / コンテキストメニュー表示中などで egui に
        // F11 が漏れて来た場合に still 用 toggle が走ると、設定だけ flip して
        // 動画 presenter のモードと乖離するため。is_video_fs は
        // fs_context_menu_idx.is_none() を含むので使えない (= 純粋な item 種別 check)。
        //
        // consume_key は repeat 込み + matches_logically で余分な Shift も拾うため、
        // 厳格に「修飾なし・非 repeat」だけ抜き出す custom event filter を使う。
        #[cfg(windows)]
        {
            let current_is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
            if !current_is_video {
                let f11_pressed = ctx.input_mut(|i| {
                    let mut found = false;
                    i.events.retain(|e| {
                        let consume = matches!(
                            e,
                            egui::Event::Key {
                                key: egui::Key::F11,
                                pressed: true,
                                repeat: false,
                                modifiers,
                                ..
                            } if modifiers.is_none()
                        );
                        if consume {
                            found = true;
                        }
                        !consume
                    });
                    found
                });
                if f11_pressed {
                    self.toggle_still_window_mode();
                    // 描画先 (embedded ⇔ 専用 viewport) の切替は次フレームの
                    // render_fullscreen_viewport で起きる。ホバーバーボタンと
                    // 同じく ROOT ビューポートの再描画を明示要求する。
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            }
        }

        // V キー (VST3 プラグイン GUI トグル) は撤去した。理由は app.rs 同箇所参照。
        // フルスクリーン中はホバーバーの "VST" ボタンから管理パネルを開く運用。

        // 消しゴムモード中は ui_erase が先に Ctrl+Z を吸収する。
        self.handle_meta_undo_keys(ctx);

        // ページ構成 (1-5) / 連結方式 (6) / 横方向 (7) 切替
        let key_1 = self.keymap.consume_action(ctx, KeyAction::FsSpreadSingle);
        let key_2 = self.keymap.consume_action(ctx, KeyAction::FsSpreadLtr);
        let key_3 = self.keymap.consume_action(ctx, KeyAction::FsSpreadLtrCover);
        let key_4 = self.keymap.consume_action(ctx, KeyAction::FsSpreadRtl);
        let key_5 = self.keymap.consume_action(ctx, KeyAction::FsSpreadRtlCover);
        let key_6 = self
            .keymap
            .consume_action(ctx, KeyAction::FsReadingFlowCycle);
        let key_7 = self
            .keymap
            .consume_action(ctx, KeyAction::FsReadingDirectionToggle);
        let key_0 = self.keymap.consume_action(ctx, KeyAction::FsFitModeCycle);

        // U / Shift+U / Alt+U: AI アップスケールモデル サイクル (次 / 前 / なしリセット)
        // 注意: egui の consume_key は matches_logically で判定されるため、Modifiers::NONE が
        // Shift/Alt を伴う入力まで吸収する。具体的な修飾子から先に consume する必要がある。
        let key_u_alt = self.keymap.consume_action(ctx, KeyAction::FsAiModelReset);
        let key_u_shift = self.keymap.consume_action(ctx, KeyAction::FsAiModelPrev);
        let key_u = self.keymap.consume_action(ctx, KeyAction::FsAiModelNext);
        // N キー: AI デノイズサイクル
        let key_n = self.keymap.consume_action(ctx, KeyAction::FsDenoiseCycle);
        // T / Shift+T / Alt+T: ポストフィルタ (レトロ系) サイクル (次 / 前 / なしリセット)
        // P はグリッド / 動画フルスクリーンのピン留めに統一する。F は動画の FPS/Perf 表示に
        // 使っているため、ポストフィルタは T (Tone / posT filter) に割り当てる。
        // 同様に Alt+T → Shift+T → T の順で consume (matches_logically 対策)。
        let key_t_alt = self
            .keymap
            .consume_action(ctx, KeyAction::FsPostFilterReset);
        let key_t_shift = self.keymap.consume_action(ctx, KeyAction::FsPostFilterPrev);
        let key_t = self.keymap.consume_action(ctx, KeyAction::FsPostFilterNext);

        // Ctrl+数字キー: 保存スロットからロード
        // (Shift+数字はキー配列によって記号化され egui::Key::Num1 等にマッチしないため CTRL を採用)
        let slot_keys: [bool; 10] = [
            self.keymap.consume_action(ctx, KeyAction::FsAdjustSlot1),
            self.keymap.consume_action(ctx, KeyAction::FsAdjustSlot2),
            self.keymap.consume_action(ctx, KeyAction::FsAdjustSlot3),
            self.keymap.consume_action(ctx, KeyAction::FsAdjustSlot4),
            self.keymap.consume_action(ctx, KeyAction::FsAdjustSlot5),
            self.keymap.consume_action(ctx, KeyAction::FsAdjustSlot6),
            self.keymap.consume_action(ctx, KeyAction::FsAdjustSlot7),
            self.keymap.consume_action(ctx, KeyAction::FsAdjustSlot8),
            self.keymap.consume_action(ctx, KeyAction::FsAdjustSlot9),
            self.keymap.consume_action(ctx, KeyAction::FsAdjustSlot10),
        ];

        // Ctrl+Backspace / Q: 現在ページの個別補正設定を解除 (標準値に戻す)
        // Q は片手で押しやすいショートカット (補正パネルでの操作中に素早く元に戻したい用途)
        let clear_page_key = self.keymap.consume_action(ctx, KeyAction::FsClearAdjust);

        // 表示モード切替 + フィードバック表示
        let new_spread = if key_1 {
            Some(SpreadMode::Single)
        } else if key_2 {
            Some(SpreadMode::Ltr)
        } else if key_3 {
            Some(SpreadMode::LtrCover)
        } else if key_4 {
            Some(SpreadMode::Rtl)
        } else if key_5 {
            Some(SpreadMode::RtlCover)
        } else {
            None
        };

        if let Some(mode) = new_spread {
            if mode != self.spread_mode {
                self.spread_mode = mode;
                self.update_reading_direction_from_spread_mode(mode);
                self.spread_popup_open = false;
                self.adjust_spread_target = crate::app::AdjustSpreadTarget::Left;
                if !self.reading_flow.is_paged() {
                    self.reset_continuous_reading_transform();
                    self.disable_non_paged_fullscreen_modes(fs_idx);
                }
                // DB に保存
                self.persist_current_spread_mode();
                self.persist_current_reading_flow();
                // 分析モードを解除 (post-filter バイパスも戻す)
                if mode.is_spread() && self.analysis_mode {
                    self.reset_analysis_mode();
                }
                // ページ位置を正規化
                self.normalize_spread_position(ctx);
            }
            // フィードバック表示
            let key_num = if key_1 {
                1
            } else if key_2 {
                2
            } else if key_3 {
                3
            } else if key_4 {
                4
            } else {
                5
            };
            self.show_feedback_toast(format!("[{}:{}]", key_num, mode.label()));
        }
        // 表示操作の数字キーは、連結読みで画面中央に来る ZIP 区切りページ上でも有効にする。
        // セパレータは画像処理の対象ではないが、0/6/7 はフォルダ単位の表示設定なので
        // レンダラと同じ連結読み対応判定 (画像 / ZIP画像 / PDFページ / ZipSeparator) を使う。
        let display_mode_keys_supported = self.continuous_reading_supported_idx(fs_idx);
        if key_6 && display_mode_keys_supported {
            let flow = self.reading_flow.next();
            self.set_reading_flow_for_fullscreen(ctx, fs_idx, flow);
            self.show_feedback_toast(format!("[6:{}]", flow.label()));
        }
        if key_7 && display_mode_keys_supported {
            let direction = self.reading_direction.next();
            self.set_reading_direction_for_fullscreen(ctx, fs_idx, direction);
            self.show_feedback_toast(format!("[7:{}]", direction.label()));
        }
        if key_0 && display_mode_keys_supported {
            self.cycle_fullscreen_fit_mode(ctx, fs_idx);
            self.show_feedback_toast(format!(
                "[0:{}]",
                self.effective_fullscreen_fit_mode().label()
            ));
        }

        // U キー: AI アップスケールモデルをサイクル
        // 現在効いているスコープ (個別 > お気に入り標準 > 標準) を書き換える。
        if (key_u || key_u_shift || key_u_alt) && self.reading_flow.is_paged() {
            let mut params = self.effective_params(fs_idx).clone();
            let items =
                crate::adjustment::upscale_menu_items_for_mode(self.settings.ai_feature_mode);
            let cur = items
                .iter()
                .position(|(_, k)| match (k, params.upscale_model.as_deref()) {
                    (None, None) => true,
                    (Some(a), Some(b)) => *a == b,
                    _ => false,
                });
            // 制限モードでは保存済みモデルを潰さない:
            //   - Disabled は「なし」のみ (= items.len() <= 1) で循環の意味が無い。
            //   - 保存済みモデルがこの AI モードのメニューに無い (cur=None) ときは、
            //     そのモデルは隠れて保持されているだけ。U で None / 別モデルに上書きすると
            //     full モードへ戻したときに選択が失われるため、変更せず維持する。
            if items.len() <= 1 || cur.is_none() {
                self.show_feedback_toast(format!(
                    "[U:アップスケール]\nこの AI モード ({}) では変更しません (保存済み選択を維持)",
                    self.settings.ai_feature_mode.label()
                ));
            } else {
                let scope = self.resolve_adjust_scope(fs_idx);
                let cur = cur.unwrap_or(0);
                let next = if key_u_alt {
                    0
                } else if key_u_shift {
                    (cur + items.len() - 1) % items.len()
                } else {
                    (cur + 1) % items.len()
                };
                let (label, key) = items[next];
                params.upscale_model = key.map(|s| s.to_string());
                // 切替先がアップスケール **有効** で、かつ画像サイズが
                // `ai_upscale_skip_px` 閾値以上なら処理がスキップされる。
                // 「切り替えたのに見た目が変わらない」違和感を避けるため
                // トーストに 2 行目で明示する。
                let mut toast = format!("[U:{}アップスケール {}]", scope.label(), label);
                if key.is_some()
                    && let Some(crate::fs_animation::FsCacheEntry::Static { pixels, .. }) =
                        self.fs_cache.get(&fs_idx)
                {
                    let w = pixels.size[0] as u32;
                    let h = pixels.size[1] as u32;
                    let threshold = self.settings.ai_upscale_skip_px;
                    if !crate::ai::upscale::should_process(w, h, threshold) {
                        toast.push_str(&format!(
                            "\n(解像度が高いためアップスケール処理無効: {w}×{h} ≥ {threshold}px)"
                        ));
                    }
                }
                self.show_feedback_toast(toast);
                self.capture_adjust_full(format!("AI アップスケール: {label}"), |app| {
                    app.write_params_for_scope(fs_idx, scope, params);
                    app.clear_all_adjustment_and_ai_caches(fs_idx);
                });
            }
        }

        // N キー: AI デノイズをトグル
        if key_n && self.reading_flow.is_paged() {
            if !self.settings.ai_feature_mode.allows_denoise() {
                self.show_feedback_toast(format!(
                    "[N:デノイズ無効]\nAI 機能: {}",
                    self.settings.ai_feature_mode.label()
                ));
            } else {
                let scope = self.resolve_adjust_scope(fs_idx);
                let mut params = self.effective_params(fs_idx).clone();
                if params.denoise_model.is_some() {
                    params.denoise_model = None;
                    self.show_feedback_toast(format!("[N:{}デノイズ OFF]", scope.label()));
                } else {
                    params.denoise_model =
                        Some(crate::ai::ModelKind::DenoiseRealplksr.as_str().to_string());
                    self.show_feedback_toast(format!("[N:{}デノイズ ON]", scope.label()));
                }
                self.capture_adjust_full("AI デノイズの切替".to_string(), |app| {
                    app.write_params_for_scope(fs_idx, scope, params);
                    app.clear_all_adjustment_and_ai_caches(fs_idx);
                });
            }
        }

        // T / Shift+T / Alt+T: ポストフィルタの次/前/なしへ切替。
        // AI 再実行は発生させないため色調キャッシュのみクリア。
        if (key_t || key_t_shift || key_t_alt) && self.reading_flow.is_paged() {
            let scope = self.resolve_adjust_scope(fs_idx);
            let mut params = self.effective_params(fs_idx).clone();
            let all = crate::adjustment::PostFilter::ALL;
            let cur = all
                .iter()
                .position(|f| *f == params.post_filter)
                .unwrap_or(0);
            let next_idx = if key_t_alt {
                0
            } else if key_t_shift {
                (cur + all.len() - 1) % all.len()
            } else {
                (cur + 1) % all.len()
            };
            let next = all[next_idx];
            params.post_filter = next;
            self.show_feedback_toast(format!("[T: {} / {}]", scope.label(), next.display_label()));
            self.capture_adjust_full(
                format!("ポストフィルタ: {}", next.display_label()),
                |app| {
                    app.write_params_for_scope(fs_idx, scope, params);
                    match scope {
                        AdjustScope::PageOverride => app.clear_adjustment_caches(fs_idx),
                        AdjustScope::FavoriteDefault(_) | AdjustScope::Global => {}
                    }
                },
            );
        }

        // Ctrl+数字キー: 保存スロットを現在ページに適用 (= ページ個別化)
        for (slot_idx, &pressed) in slot_keys.iter().enumerate() {
            if pressed && self.reading_flow.is_paged() {
                self.capture_adjust_full(
                    format!(
                        "スロット{}を適用",
                        crate::adjustment::slot_key_label(slot_idx)
                    ),
                    |app| app.apply_slot_to_current_page(slot_idx),
                );
            }
        }

        // Ctrl+Backspace: 個別設定があれば解除、なければフィードバックのみ
        if clear_page_key && self.reading_flow.is_paged() {
            if self.adjustment_page_params.contains_key(&fs_idx) {
                self.capture_adjust_full("個別設定の解除".to_string(), |app| {
                    app.clear_page_params(fs_idx)
                });
                self.show_feedback_toast("[個別設定を解除]".to_string());
            } else {
                self.show_feedback_toast("[個別設定なし]".to_string());
            }
        }

        if key_compare_x {
            self.toggle_compare_pin_from_current(ctx, fs_idx);
        }
        if key_compare_c {
            self.toggle_compare_pinned_view(ctx, fs_idx);
        }
        if key_compare_shift_c {
            self.toggle_compare_wipe_mode(ctx, fs_idx);
        }
        if key_compare_alt_c {
            self.toggle_compare_diff_mode(ctx, fs_idx);
        }

        if esc && self.compare_view_mode.is_overlay() {
            self.compare_view_mode = crate::app::CompareViewMode::Off;
            self.compare_wipe_dragging = false;
            self.show_feedback_toast("[比較: Normal]".to_string());
        } else if esc {
            action.close = true;
        }
        // BS = 階層を 1 段だけ戻す。ZIP/PDF ページを見ているとき (current_folder がコンテナ)
        // は、設定に関係なく常にコンテナのページ一覧 (L2) へ戻る (= close_fullscreen を直接呼ぶ。
        // current_folder は ZIP/PDF のままなので L2 が出る)。通常フォルダ内画像では、グリッド
        // 側の BS ハンドラが別ビューポート中は抑止されるため、ここで一覧へ戻す。Ctrl+BS
        // (個別補正解除) は Modifiers::NONE 限定なので影響しない。
        let viewing_container_page = self
            .current_folder
            .as_deref()
            .is_some_and(crate::folder_tree::is_virtual_folder);
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)) {
            if viewing_container_page {
                action.close_to_page_list = true;
            } else {
                action.close = true;
            }
        }
        // 見開きダブル表示中は I/Z/R/L を無効化
        if key_i && !is_spread_double {
            self.show_metadata_panel = !self.show_metadata_panel;
            self.metadata_panel_hover_active = false;
        }
        // V: 360 度パノラマビューモード トグル。
        // 検出済み (= 360 ボタンが有効な状態) のときだけ反応する。非対応画像で
        // V を押しても no-op (= 一般的なキーマップ慣例)。
        if key_v_panorama && !is_spread_double && self.detect_panorama(fs_idx).is_some() {
            self.toggle_panorama_mode(fs_idx);
        }
        if key_z && !is_spread_double {
            self.toggle_analysis_mode();
        }
        if self.analysis_mode && !is_spread_double {
            if key_g {
                self.analysis_grayscale = !self.analysis_grayscale;
            }
            if key_m {
                self.analysis_mosaic_grid = !self.analysis_mosaic_grid;
                if self.analysis_mosaic_grid {
                    self.analysis_guide_drag = None;
                }
            }
        } else if key_g && !is_video_fs {
            self.fs_pixel_grid_enabled = !self.fs_pixel_grid_enabled;
            self.show_feedback_toast(if self.fs_pixel_grid_enabled {
                "[ピクセルグリッド ON]".to_string()
            } else {
                "[ピクセルグリッド OFF]".to_string()
            });
        } else if key_m && !self.adjustment_mode {
            // M: ルーペ表示のトグル (分析モード外でのみ。分析モードでは既存のモザイクグリッド操作)
            self.fs_loupe_locked = !self.fs_loupe_locked;
            self.show_feedback_toast(if self.fs_loupe_locked {
                "[ルーペ ON]".to_string()
            } else {
                "[ルーペ OFF]".to_string()
            });
        }

        // B: 透過画像の背景サイクル (分析・補正・動画モード外)。
        // 消しゴムモードは別ブランチ (handle_erase_keys) で処理済みのためここには来ない。
        // 通常: 黒 → 白 → 市松 の 3 モード循環。
        // AI アップスケール有効時 (composite-first): 市松は出力にパターンが焼き込まれて崩れるので
        // 黒/白の 2 モード循環に制限する。デノイズのみの場合はアルファ保持パスを通るので市松 OK。
        if key_b_bg && !self.analysis_mode && !self.adjustment_mode {
            // 背景切替は透過 (alpha) のある画像でのみ意味を持つ。見開きはどちらかに透過が
            // あれば許可。RGB のみの不透明画像では切替しても見た目が変わらないので無効化し案内する。
            let idxs: Vec<usize> = match self.resolve_spread_pair(fs_idx) {
                SpreadPair::Double { left, right } => vec![left, right],
                SpreadPair::Single => vec![fs_idx],
            };
            if !idxs.iter().any(|&i| self.fs_image_has_alpha(i)) {
                self.show_feedback_toast(
                    "透過画像ではないため背景は切り替えできません".to_string(),
                );
            } else {
                let modulo: u8 = if self.ai_upscale_enabled { 2 } else { 3 };
                self.fs_transparent_bg_mode = (self.fs_transparent_bg_mode + 1) % modulo;
                self.fs_transparent_bg_indicator_until =
                    Some(std::time::Instant::now() + std::time::Duration::from_millis(1200));
                let label = transparent_bg_toast(self.fs_transparent_bg_mode);
                self.show_feedback_toast(label.to_string());
                // AI アップスケール (composite-first) では表示物が背景別の (idx,bg) 結果。
                // adjustment_cache 等は idx のみキーで背景非依存に残るため、背景を変えても旧背景の
                // 派生結果が表示され続け固着する。色補正変更時と同じ無効化を行い、新背景の
                // (idx,bg) から表示を作り直させる (v1.0.0 安定性: Issue C)。
                if self.ai_upscale_enabled {
                    for &i in &idxs {
                        self.clear_adjustment_caches(i);
                    }
                }
            }
        }

        // E: 消しゴムモード切り替え (分析・補正中は無効)。
        // 見開き中の起動は `enter_erase_mode` が一時的に Single に切り替えて
        // 左ページを編集対象にする (Apply / Cancel で見開き状態に戻る)。
        if key_e && !self.analysis_mode && !self.adjustment_mode {
            if self.erase_mode {
                // 2回目のE: inpaint実行
                self.execute_erase_inpaint(ctx, fs_idx);
            } else {
                // 1回目のE: マスクモード開始
                self.enter_erase_mode(fs_idx);
            }
        }

        // Ctrl+M: 隠蔽加工モード入退場 (分析・補正中は無効)。Phase 1。
        // 見開き / 動画 / モーダル状態は `enter_conceal_mode` 側で適切に分岐する。
        // (Conceal モード中の Ctrl+M / Esc は本関数冒頭の早期 return で処理済み。)
        let key_ctrl_m = self.keymap.consume_action(ctx, KeyAction::FsConcealMode);
        if key_ctrl_m
            && !self.analysis_mode
            && !self.adjustment_mode
            && !self.erase_mode
            && self.reading_flow.is_paged()
            && !is_video_fs
        {
            self.enter_conceal_mode(fs_idx);
        }

        // Ctrl+T: テキスト注釈モード入場 (分析・補正・消しゴム・隠蔽・動画中は無効)。
        // テキストモード中の Ctrl+T / Esc は本関数冒頭の早期 return で処理済み。
        let key_ctrl_t = self.keymap.consume_action(ctx, KeyAction::FsTextMode);
        if key_ctrl_t
            && !self.analysis_mode
            && !self.adjustment_mode
            && !self.erase_mode
            && !self.conceal_mode
            && self.reading_flow.is_paged()
            && !is_video_fs
        {
            self.enter_text_mode(fs_idx);
        }

        if key_ctrl_s_capture {
            self.save_image_capture_to_file(ctx, fs_idx);
        }
        if key_ctrl_e_export {
            self.open_export_dialog_for_current(ctx, fs_idx);
        }

        // S: スライドショー開始/停止トグル (旧 P、左手で押しやすいよう S へ移行)
        // 開始時のみ、現在ページが画像系アイテム (Image/ZipImage/PdfPage) かを確認する。
        // ZipSeparator など非画像アイテム上では開始させない (停止操作は常に許可)。
        if key_s {
            if self.slideshow_playing {
                self.slideshow_playing = false;
            } else if matches!(
                self.items.get(fs_idx),
                Some(GridItem::Image(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::PdfPage { .. })
            ) {
                self.slideshow_playing = true;
                self.schedule_next_slideshow_from_now();
            }
        }

        // Space: スライドショー中→停止、停止中→画像をチェック。
        // 動画モードでは `current_item_is_video_for_space` で gate されているため
        // `key_space` は常に false (= ここには到達しない)。Video アームを残しておくと
        // 「context menu open 中の動画で Space → チェックトグル」の脱出口になり得るので
        // **Video / ZipImage の動画系派生を含めない** (Codex Phase 1 P2 指摘)。
        // (ZipImage / PdfPage は静止画扱いのため従来通りチェック可能。)
        if key_space {
            if self.slideshow_playing {
                self.slideshow_playing = false;
            } else {
                match self.items.get(fs_idx) {
                    Some(GridItem::Image(_))
                    | Some(GridItem::ZipImage { .. })
                    | Some(GridItem::PdfPage { .. }) => {
                        if self.checked.contains(&fs_idx) {
                            self.checked.remove(&fs_idx);
                        } else {
                            self.checked.insert(fs_idx);
                        }
                    }
                    _ => {}
                }
            }
        }
        if key_r && !is_spread_double {
            self.rotate_image_cw(fs_idx);
        }
        if key_l && !is_spread_double {
            self.rotate_image_ccw(fs_idx);
        }

        // ── ナビゲーション ──
        // RTL モードでは左右キーの意味を反転
        let rtl = self.spread_mode.is_rtl();
        // 連続読みのスクロール/ナビ分岐は、レンダラが実際に連続描画している条件と一致させる。
        // これで縦/横読み中に解析(Z)・比較(X/C)・オーバーレイ編集・動画/非対応アイテムへ
        // 入ったとき、↑↓ がスクロールへ吸われてナビもスクロールもしない (デッド入力) のを防ぎ、
        // フォールバック描画 (単ページ) に対して通常のページ送りが効くようにする。
        let continuous_active = self.continuous_reading_active_for_idx(fs_idx);
        let vertical_reading = continuous_active && self.reading_flow.is_vertical();
        let horizontal_reading = continuous_active && self.reading_flow.is_horizontal();
        if vertical_reading || horizontal_reading {
            let mut scroll_delta = 0.0;
            let key_step = self.continuous_reading_key_step_px(ctx);
            if vertical_reading {
                if arrow_down && !ctrl_d {
                    scroll_delta += key_step;
                }
                if arrow_up && !ctrl_u {
                    scroll_delta -= key_step;
                }
            } else {
                let axis_rtl = self.reading_direction == ReadingDirection::Rtl;
                if ((arrow_right && !axis_rtl) || (arrow_left && axis_rtl)) && !ctrl_d {
                    scroll_delta += key_step;
                }
                if ((arrow_left && !axis_rtl) || (arrow_right && axis_rtl)) && !ctrl_u {
                    scroll_delta -= key_step;
                }
            }
            let viewport = ctx.content_rect();
            let page_step = if horizontal_reading {
                viewport.width()
            } else {
                viewport.height()
            } * VERTICAL_READING_PAGE_SCROLL_FRAC;
            if page_down {
                scroll_delta += page_step;
            }
            if page_up {
                scroll_delta -= page_step;
            }
            self.scroll_vertical_reading_by(ctx, scroll_delta);
        }
        let nav_next = if vertical_reading {
            (arrow_right && !rtl) || (arrow_left && rtl)
        } else if horizontal_reading {
            arrow_down
        } else {
            (arrow_right && !rtl) || (arrow_left && rtl) || arrow_down
        };
        let nav_prev = if vertical_reading {
            (arrow_left && !rtl) || (arrow_right && rtl)
        } else if horizontal_reading {
            arrow_up
        } else {
            (arrow_left && !rtl) || (arrow_right && rtl) || arrow_up
        };

        // 矢印キーでのフォルダ内移動はスライドショーを止めない (ホイール / クリックと統一)。
        // 一部スキップしつつスライドショーを継続できる。フォルダをまたぐ Ctrl+↑↓ や
        // S / Space / Esc は従来どおり停止する。
        if nav_next && !ctrl_d {
            action.nav_delta = self.spread_nav_delta(1);
        }
        if nav_prev && !ctrl_u {
            action.nav_delta = self.spread_nav_delta(-1);
        }
        // Ctrl+←/→: 見開きモードでは「1 ページずらし」(現在ページを軸に見開きを 1 ページ
        // ぶんずらす)、Single モードでは 1 ページ移動。RTL は左右の意味を反転 (plain 矢印と同じ)。
        let ctrl_nudge_next = (ctrl_right && !rtl) || (ctrl_left && rtl);
        let ctrl_nudge_prev = (ctrl_left && !rtl) || (ctrl_right && rtl);
        if ctrl_nudge_next || ctrl_nudge_prev {
            let dir = if ctrl_nudge_next { 1 } else { -1 };
            if self.spread_mode.is_spread() {
                if let Some((new_idx, new_mode)) = self.compute_spread_offset_nudge(fs_idx, dir) {
                    // 見開きモード (cover/非cover) を直接切り替えて 1 ページずらす。
                    // フォルダ単位で永続化 (spread_db)。new_idx は新モードでペア先頭なので
                    // normalize は no-op。
                    self.spread_mode = new_mode;
                    self.update_reading_direction_from_spread_mode(new_mode);
                    self.persist_current_spread_mode();
                    self.persist_current_reading_flow();
                    self.adjust_spread_target = crate::app::AdjustSpreadTarget::Left;
                    action.jump_to = Some(new_idx);
                    self.show_feedback_toast("見開きを1ページずらしました".to_string());
                }
            } else {
                // Single モード: 1 ページ移動にフォールバック (ユーザー要望)
                action.nav_delta = dir;
            }
        }
        if ctrl_d || mouse_forward || browser_forward {
            action.ctrl_nav = Some(1);
        }
        if ctrl_u || mouse_back || browser_back {
            action.ctrl_nav = Some(-1);
        }
        if ctrl_page_down {
            action.sibling_nav = Some(1);
        }
        if ctrl_page_up {
            action.sibling_nav = Some(-1);
        }

        if key_home {
            let display_order = self.current_grid_order().to_vec();
            if let Some(first) =
                crate::ui_helpers::boundary_navigable_idx(&self.items, &display_order, false)
            {
                if first != fs_idx {
                    // Home もフォルダ内移動なのでスライドショーは止めない。
                    action.jump_to = Some(first);
                } else {
                    self.fs_boundary_hint = Some(FsBoundaryHint::Edge {
                        at_end: false,
                        at: std::time::Instant::now(),
                    });
                }
            }
        }
        if key_end {
            let display_order = self.current_grid_order().to_vec();
            if let Some(last) =
                crate::ui_helpers::boundary_navigable_idx(&self.items, &display_order, true)
            {
                if last != fs_idx {
                    // End もフォルダ内移動なのでスライドショーは止めない。
                    action.jump_to = Some(last);
                } else {
                    self.fs_boundary_hint = Some(FsBoundaryHint::Edge {
                        at_end: true,
                        at: std::time::Instant::now(),
                    });
                }
            }
        }

        action
    }

    // ── ホイール & クリック ──────────────────────────────────────────────

    fn handle_compare_wipe_drag(&mut self, ctx: &egui::Context, image_rect: egui::Rect) -> bool {
        let crate::app::CompareViewMode::Wipe { fraction } = self.compare_view_mode else {
            self.compare_wipe_dragging = false;
            return false;
        };
        if image_rect.width() <= 1.0 {
            return false;
        }

        let (primary_pressed, primary_down, primary_released, pointer_pos) = ctx.input(|i| {
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.primary_released(),
                i.pointer.interact_pos().or_else(|| i.pointer.hover_pos()),
            )
        });
        if primary_released {
            let was_dragging = self.compare_wipe_dragging;
            self.compare_wipe_dragging = false;
            return was_dragging;
        }

        let line_x = image_rect.left() + image_rect.width() * fraction.clamp(0.05, 0.95);
        let hit = pointer_pos
            .map(|p| image_rect.contains(p) && (p.x - line_x).abs() <= 14.0)
            .unwrap_or(false);
        if hit {
            ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        if primary_pressed && hit {
            self.compare_wipe_dragging = true;
        }
        if self.compare_wipe_dragging && primary_down {
            if let Some(pos) = pointer_pos {
                let new_fraction =
                    ((pos.x - image_rect.left()) / image_rect.width()).clamp(0.05, 0.95);
                self.compare_view_mode = crate::app::CompareViewMode::Wipe {
                    fraction: new_fraction,
                };
                ctx.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
                ctx.request_repaint();
            }
            return true;
        }
        false
    }

    /// ホイールとクリックを処理し、(nav_delta, close) を返す。
    fn handle_fs_wheel_and_click(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        state: &FsFrameState,
        is_spread_double: bool,
        prev_foreground_hwnd: usize,
    ) -> (i32, bool) {
        let mut nav_delta = 0i32;
        let mut close = false;

        // レンダラが連続読みを描画しているか。クリックのページジャンプ抑制と、連続読み中の
        // デッドな pan-drag (fs_pan に書いても縦/横描画は fs_vertical_scroll しか見ないため
        // has_transform だけ汚す) の抑制に使う。キー側の continuous_active と同一述語。
        let continuous_active = self
            .fullscreen_idx
            .is_some_and(|idx| self.continuous_reading_active_for_idx(idx));

        // VST editor windows are bridge-process native windows. In the
        // cross-process owner-popup case, egui can still report this viewport as
        // focused while Win32 sends keyboard input to the VST editor. Check the
        // actual foreground HWND when the user clicks back into fullscreen.
        let (fullscreen_primary_event, viewport_focused) = ctx.input(|i| {
            let focused = i.viewport().focused.unwrap_or(true);
            let primary_event = i.pointer.primary_down()
                || i.pointer.primary_pressed()
                || i.pointer.primary_released();
            let in_fullscreen = i
                .pointer
                .interact_pos()
                .map(|p| full_rect.contains(p))
                .unwrap_or(false);
            (primary_event && in_fullscreen, focused)
        });
        if fullscreen_primary_event {
            let target = native_window_under_cursor_focus_target();
            let previous_foreign_foreground = prev_foreground_hwnd != 0
                && target.target_hwnd != 0
                && prev_foreground_hwnd != target.target_hwnd;
            let current_foreign_foreground = target.foreground_hwnd != 0
                && target.target_hwnd != 0
                && target.foreground_hwnd != target.target_hwnd;
            let vst_gui_visible =
                cfg!(windows) && self.settings.vst3_enabled && self.settings.vst3_gui_visible;
            let focus_restore_click = previous_foreign_foreground || vst_gui_visible;
            let should_claim_native_focus =
                focus_restore_click && (previous_foreign_foreground || current_foreign_foreground);
            let claim_debounced = self
                .fs_last_native_focus_claim_at
                .map(|t| t.elapsed() < std::time::Duration::from_millis(100))
                .unwrap_or(false);
            let focus = if should_claim_native_focus && !claim_debounced {
                self.fs_last_native_focus_claim_at = Some(std::time::Instant::now());
                claim_native_window_focus(target.target_hwnd)
            } else {
                NativeFocusClaim {
                    foreground_hwnd: target.foreground_hwnd,
                    post_foreground_hwnd: target.foreground_hwnd,
                    target_hwnd: target.target_hwnd,
                    set_foreground_ok: false,
                    attach_thread_input_ok: false,
                    set_active_ok: false,
                    set_focus_ok: false,
                }
            };
            if previous_foreign_foreground
                || current_foreign_foreground
                || should_claim_native_focus
                || claim_debounced
            {
                crate::logger::log(format!(
                    "[fs-focus] prev_foreground=0x{:x} foreground=0x{:x} post_foreground=0x{:x} fullscreen=0x{:x} prev_foreign={} current_foreign={} vst_gui_visible={} viewport_focused={} suppress={} native_claim={} claim_debounced={} set_foreground={} attach_thread_input={} set_active={} set_focus={}",
                    prev_foreground_hwnd,
                    focus.foreground_hwnd,
                    focus.post_foreground_hwnd,
                    focus.target_hwnd,
                    previous_foreign_foreground,
                    current_foreign_foreground,
                    vst_gui_visible,
                    viewport_focused,
                    focus_restore_click,
                    should_claim_native_focus && !claim_debounced,
                    claim_debounced,
                    focus.set_foreground_ok,
                    focus.attach_thread_input_ok,
                    focus.set_active_ok,
                    focus.set_focus_ok
                ));
            }
            if focus_restore_click {
                if should_claim_native_focus && !claim_debounced {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                self.fs_focus_regained_at = Some(std::time::Instant::now());
                self.fs_suppress_primary_until_release = true;
                return (0, false);
            }
        }

        let compare_wipe_active = matches!(
            self.compare_view_mode,
            crate::app::CompareViewMode::Wipe { .. }
        );

        // ── ホイール ──
        // パネル領域内ではホイールナビゲーションを抑制
        let panel_w = METADATA_PANEL_WIDTH.min(full_rect.width() * 0.5);
        let panel_left = full_rect.max.x - panel_w;
        let hover_threshold = full_rect.max.x - full_rect.width() * 0.25;
        let has_right_panel = self.show_metadata_panel || self.metadata_panel_hover_active;
        // 当たり判定は描画と同じ rect を使う (adjustment_panel_rect 参照)。
        let left_panel_right = adjustment_panel_rect(full_rect).max.x;
        // When the OS cursor is hidden, egui still exposes the last hover position.
        // Treat that position as stale and block passive hover side effects until a
        // real input event revives the cursor.
        let passive_hover_enabled = !self.cursor_hidden;
        let cursor_in_panel = passive_hover_enabled
            && ctx.input(|i| {
                i.pointer
                    .hover_pos()
                    .map(|p| {
                        let in_right = !compare_wipe_active
                            && p.x > panel_left
                            && p.y >= 60.0
                            && (has_right_panel || p.x > hover_threshold);
                        let in_left = !compare_wipe_active
                            && self.adjustment_mode
                            && p.x < left_panel_right
                            && p.y >= 60.0;
                        let in_erase_panel =
                            self.erase_mode && self.erase_panel_rect(full_rect).contains(p);
                        let in_local_adjust_panel = self.local_adjust_mode
                            && (self.local_adjust_panel_rect(full_rect).contains(p)
                                || self.local_adjust_tool_panel_rect(full_rect).contains(p));
                        let in_conceal_panel =
                            self.conceal_mode && self.conceal_panel_rect(full_rect).contains(p);
                        let in_export_crop_panel = self.export_crop_mode
                            && self.export_crop_panel_rect(full_rect).contains(p);
                        // テキスト注釈モードの左 (一覧) / 右 (詳細) パネル。一覧 ScrollArea を
                        // ホイールでスクロールしたいので、パネル上のホイールはズームに使わない
                        // (実機 FB 2026-06-07)。text mode では image_rect == full_rect なので
                        // (analysis/vst3_compact 以外。1992 行) text_panel_rect(full_rect) が
                        // 実際のパネル矩形と一致する。
                        let in_text_panel = self.text_mode
                            && (self.text_panel_rect(full_rect).contains(p)
                                || self.text_detail_panel_rect(full_rect).contains(p));
                        in_right
                            || in_left
                            || in_erase_panel
                            || in_local_adjust_panel
                            || in_conceal_panel
                            || in_export_crop_panel
                            || in_text_panel
                    })
                    .unwrap_or(false)
            });

        // 左端・上端・右端のホバーでオーバーレイ（上バー＋左パネル＋右パネル）を同時表示/非表示
        // 消しゴムモード中は自前のパネルを左端に描いているためエッジ発火を抑制する。
        // 加えて、消しゴムモードに入る前から adjustment_mode が立っていると、消しゴムパネルが
        // 左端を占有している間 edge_hover が常に true 扱いになり off へ遷移できないので、
        // 強制的に落とす。
        if compare_wipe_active {
            if self.adjustment_mode && !self.adjustment_dragging {
                self.adjustment_mode = false;
            }
        } else if self.is_overlay_edit_mode_active() {
            // 消しゴム / 補正レイヤー / 隠蔽加工モード中は補正パネルを強制 OFF
            // (編集中に色補正バーが画面端ホバーで開かないように)。
            self.adjustment_mode = false;
        } else {
            let edge_hover = passive_hover_enabled
                && ctx.input(|i| {
                    i.pointer
                        .hover_pos()
                        .map(|p| {
                            p.y < 60.0  // 上端
                    || p.x < full_rect.min.x + full_rect.width() * 0.05  // 左端5%
                    || p.x > full_rect.max.x - full_rect.width() * 0.05 // 右端5%
                        })
                        .unwrap_or(false)
                });
            if edge_hover && !self.analysis_mode && self.reading_flow.is_paged() {
                self.adjustment_mode = true;
            } else if !cursor_in_panel
                && !edge_hover
                && self.adjustment_mode
                && !self.adjustment_dragging
            {
                self.adjustment_mode = false;
            }
        }

        // Wipe のドラッグ基準は、白線 / clip / シェーダー合成境界と同じ「フィット後の実表示
        // 画像矩形」にする。full_rect (黒帯込み) で取ると線・切り替え位置とドラッグがズレる。
        // base 矩形と zoom_pan は描画側 (image_rect / zp) と一致させる。
        let (compare_base_rect, compare_zoom_pan) = if self.analysis_mode && !is_spread_double {
            (
                analysis_image_rect(full_rect),
                Some((self.analysis_zoom, self.analysis_pan)),
            )
        } else {
            (full_rect, self.fs_zoom_pan())
        };
        // pair が準備中 (Shift+C 直後、worker 完了前) は pinned スロットの元サイズで
        // フィット矩形を作る。compare_image_draw_rect はアスペクト比だけで矩形が決まるので
        // target_size でも source_size でも同じ実表示画像矩形になり、描画 fallback と一致する。
        let compare_target_size = self
            .compare_prepared_pair
            .as_ref()
            .map(|pair| pair.target_size)
            .or_else(|| {
                self.pinned_compare_slot
                    .as_ref()
                    .map(|slot| slot.source_size)
            });
        let compare_drag_rect = compare_target_size
            .and_then(|size| {
                Self::compare_image_draw_rect(compare_base_rect, size, compare_zoom_pan)
            })
            .unwrap_or(compare_base_rect);
        if !cursor_in_panel && self.handle_compare_wipe_drag(ctx, compare_drag_rect) {
            return (0, false);
        }

        // ── 中ボタン (ホイール押し込み) ドラッグでズーム ──
        // キーボードを使わず右手のマウスだけで拡大縮小したい用途向け。
        // 分析モード中でも同じ操作感で動かす (書き戻し先はドラッグ開始時の mode で固定)。
        // パネル上で「開始」された中ボタンは無視して下流に流すが、既にドラッグが
        // 走っているときはカーソルがパネルを通過しても継続させる (UX のブレ防止)。
        if !cursor_in_panel || self.fs_middle_zoom_drag.is_some() {
            self.handle_middle_drag_zoom(ctx, full_rect);
        }

        // 動画タイルモード中でも Ctrl なし Wheel は前後アイテム移動に使う。
        // Ctrl+Wheel はタイル overlay 側の列数切替に渡すため、ここでは消費しない。
        #[cfg(windows)]
        let in_video_tile = self.video_tile_mode_active;
        #[cfg(not(windows))]
        let in_video_tile = false;
        let wheel_y = ctx.input(|i| i.raw_scroll_delta.y);
        let ctrl_held = ctx.input(|i| i.modifiers.ctrl);
        // modal ダイアログ (Ctrl+E など) が開いている間は wheel 由来のページ送りを
        // 抑止する。state.source 等が snapshot 時点で固定されているので、idx だけ
        // 移動すると間違った画像が export される (Codex review CONFIRMED)。
        let modal_for_keys = self.any_modal_dialog_open_for_fullscreen_keys();
        let handle_wheel_here = should_handle_fullscreen_wheel(
            cursor_in_panel,
            in_video_tile,
            ctrl_held,
            modal_for_keys,
            self.spread_popup_open || self.fit_popup_open,
        );
        if wheel_y.abs() > 0.5 && handle_wheel_here {
            ctx.input_mut(|i| {
                i.raw_scroll_delta = egui::Vec2::ZERO;
                i.smooth_scroll_delta = egui::Vec2::ZERO;
                i.events
                    .retain(|e| !matches!(e, egui::Event::MouseWheel { .. }));
            });
            if self.analysis_mode {
                // 分析モード: ホイールでズーム
                let mouse = ctx.input(|i| i.pointer.hover_pos());
                let image_rect = analysis_image_rect(full_rect);
                let changed = Self::apply_wheel_zoom(
                    &mut self.analysis_zoom,
                    &mut self.analysis_pan,
                    wheel_y,
                    mouse,
                    image_rect.center(),
                );
                if changed {
                    self.maybe_rerender_pdf(self.analysis_zoom);
                }
            } else if self.handle_panorama_wheel_if_active(ctx, wheel_y, ctrl_held) {
                // 360 度パノラマビュー: ホイールを FOV 調整に転用 (Ctrl 有無に関わらず)。
                // 2026-05 ユーザー要望: 拡大縮小のつもりでホイールを回して画像が切り替
                // わる事故を避けるため、360 ON 時はホイール全部 (= 修飾キー無視) を
                // FOV 操作に振る。前後ナビは矢印キーで行う。
            } else if !ctrl_held && continuous_active {
                let delta = self.continuous_reading_wheel_delta_px(ctx, wheel_y);
                self.scroll_vertical_reading_by(ctx, delta);
            } else {
                if should_zoom_fullscreen_wheel(ctrl_held, self.is_overlay_edit_mode_active()) {
                    // 通常モード: Ctrl+ホイールでズーム。
                    // 消しゴム / 隠蔽加工モード中は画像上の修飾なしホイールも
                    // ズームへ割り当てる。パネル上は cursor_in_panel でここへ来ないため
                    // パネルスクロールを維持できる。
                    let mouse = ctx.input(|i| i.pointer.hover_pos());
                    let changed = Self::apply_wheel_zoom(
                        &mut self.fs_zoom,
                        &mut self.fs_pan,
                        wheel_y,
                        mouse,
                        full_rect.center(),
                    );
                    if changed {
                        self.maybe_rerender_pdf(self.fs_zoom);
                    }
                } else {
                    let base = if wheel_y < 0.0 { 1 } else { -1 };
                    nav_delta = self.spread_nav_delta(base);
                }
            }
        }

        // ── クリック & ドラッグ ──
        let fs_response = ui.interact(
            full_rect,
            egui::Id::new("fs_click"),
            egui::Sense::click_and_drag(),
        );
        // ── フォーカス復帰クリックの抑制 ──
        // 他アプリから戻ってきた直後に押された左ボタンはナビ目的ではないとみなし、
        // 押下〜離すまでの全期間にわたってアプリ側の左クリック処理を無効化する。
        // グレースを跨いだ正常クリックには影響しないよう、状態ベースで追跡する。
        const FOCUS_RESTORE_GRACE: std::time::Duration = std::time::Duration::from_millis(300);
        let (primary_down, primary_released) =
            ctx.input(|i| (i.pointer.primary_down(), i.pointer.primary_released()));
        if let Some(t) = self.fs_focus_regained_at {
            if t.elapsed() >= FOCUS_RESTORE_GRACE {
                self.fs_focus_regained_at = None;
            } else if !self.fs_suppress_primary_until_release && primary_down {
                self.fs_suppress_primary_until_release = true;
                self.fs_focus_regained_at = None;
            }
        }
        // フレーム冒頭の状態をスナップ。release フレームでは primary_released と
        // fs_response.clicked() が同一フレームで立つので、フラグのクリアを先に
        // 行うと抑制対象のクリックがそのままナビを走らせてしまう。判定は
        // スナップショットで行い、クリアは左クリック分岐が終わった後に回す。
        let suppress_this_frame = self.fs_suppress_primary_until_release;
        if suppress_this_frame {
            // フォーカス復帰クリック: 左ボタン経由の分岐をすべてスキップ。
            // 右クリック処理 (下の secondary ブロック) は別系統なので走らせる
        } else if self.is_overlay_edit_mode_active() {
            // 消しゴム / 隠蔽加工モード: 左クリック/ドラッグはマスク塗りに使うためナビ無効化
        } else if self.analysis_mode {
            // 分析モード: 左クリックでのナビを無効化（パン用のドラッグは analysis_panel 側）
            // ダブルクリックでズームリセット
            if fs_response.double_clicked() {
                self.analysis_zoom = 1.0;
                self.analysis_pan = egui::Vec2::ZERO;
                self.maybe_rerender_pdf(1.0);
            }
            // 右クリックは analysis_panel 側で処理
        } else if self.handle_panorama_drag_if_active(ctx, full_rect, &fs_response) {
            // 360 度パノラマビュー: 左ドラッグ → yaw/pitch、ダブルクリック → reset (§5.2)。
            // 通常モードの zoom/pan/rotation ドラッグはスキップ。
        } else {
            // ── 通常モード: ドラッグ操作 ──
            let mods = ctx.input(|i| i.modifiers);
            let primary_pressed = fs_response.drag_started_by(egui::PointerButton::Primary);
            let primary_down = fs_response.dragged_by(egui::PointerButton::Primary);
            let primary_released = fs_response.drag_stopped_by(egui::PointerButton::Primary);
            let pointer_pos = ctx.input(|i| i.pointer.hover_pos());

            // 見開き 2 ページ表示中はフリー回転が描画に反映されないため、Ctrl+ドラッグ回転を無効化する
            if mods.ctrl && !is_spread_double && self.reading_flow.is_paged() {
                // Ctrl+ドラッグ → 回転
                if primary_pressed {
                    if let Some(pos) = pointer_pos {
                        self.fs_rotation_drag_start = Some((pos, self.fs_free_rotation));
                    }
                } else if primary_down {
                    if let Some((start_pos, start_rot)) = self.fs_rotation_drag_start {
                        if let Some(pos) = pointer_pos {
                            let center = full_rect.center() + self.fs_pan;
                            let start_angle =
                                (start_pos.y - center.y).atan2(start_pos.x - center.x);
                            let cur_angle = (pos.y - center.y).atan2(pos.x - center.x);
                            self.fs_free_rotation = start_rot + (cur_angle - start_angle);
                        }
                    }
                }
            } else if continuous_active && primary_down && !cursor_in_panel {
                let pointer_delta = ctx.input(|i| i.pointer.delta());
                let scroll_delta = if self.reading_flow.is_horizontal() {
                    if self.reading_direction == ReadingDirection::Rtl {
                        pointer_delta.x
                    } else {
                        -pointer_delta.x
                    }
                } else {
                    -pointer_delta.y
                };
                self.scroll_vertical_reading_by(ctx, scroll_delta);
            } else if !continuous_active
                && (self.fullscreen_fit_allows_drag_pan()
                    || self.fs_zoom > ZOOM_NEAR_ONE
                    || self.fs_free_rotation.abs() > TRANSFORM_EPSILON)
            {
                // ズームまたは回転中: ドラッグでパン (連続読み中は fs_zoom が自動インフレ
                // されるが、その pan は描画に反映されず has_transform を汚すだけなので除外。
                // 連続読みのスクロールはホイール / 矢印 / gamepad で行う)
                if primary_pressed {
                    if let Some(pos) = pointer_pos {
                        self.fs_pan_drag_start = Some((pos, self.fs_pan));
                    }
                } else if primary_down {
                    if let Some((start_pos, start_pan)) = self.fs_pan_drag_start {
                        if let Some(pos) = pointer_pos {
                            self.fs_pan = start_pan + (pos - start_pos);
                        }
                    }
                }
            }
            if primary_released {
                self.fs_pan_drag_start = None;
                self.fs_rotation_drag_start = None;
            }

            // ダブルクリック → ズーム/パン/回転リセット
            let has_transform = self.fs_zoom > ZOOM_NEAR_ONE
                || self.fs_free_rotation.abs() > TRANSFORM_EPSILON
                || self.fs_pan.length_sq() > PAN_EPSILON_SQ;
            if fs_response.double_clicked() && has_transform {
                self.fs_zoom = 1.0;
                self.fs_pan = egui::Vec2::ZERO;
                self.fs_free_rotation = 0.0;
                self.maybe_rerender_pdf(1.0);
            } else if !has_transform && self.fs_context_menu_idx.is_none() {
                // 変形なし: 従来の動画/画像クリック動作（コンテキストメニュー表示中は無効）
                let was_dragging = fs_response.dragged() && fs_response.drag_delta().length() > 3.0;
                if !was_dragging {
                    if state.is_video {
                        // 動画: クリックで再生/一時停止トグル (一般的な動画プレイヤー慣例)。
                        // 外部プレイヤーで開きたい場合は Shift+Enter。
                        // ただし下部 HUD バーの矩形内のクリックは play/pause / シーク /
                        // 音量等のウィジェット側で処理させたいので除外する。
                        // Phase 5.4 追加: 左ジャンプパネル / 右メタ情報パネルの上で
                        // クリックしたら toggle_play しない (Codex P5.4 M1 反映)。
                        // Phase 5.5 追加: タイルモード中はオーバーレイのタイルクリック
                        // で seek + close を行うため、background catch-all は完全抑止
                        // (Codex P5.5 H1 反映)。
                        let tile_active = self.video_tile_mode_active;
                        let pos_opt = fs_response.interact_pointer_pos();
                        // 旧 egui HUD は撤去済 (native presenter overlay が代替)。
                        // egui main window 側で intercept すべき HUD 矩形は無いので常に false。
                        let in_hud = false;
                        let in_video_panel = pos_opt
                            .map(|p| {
                                let left_thresh = full_rect.min.x + full_rect.width() * 0.25;
                                let right_thresh = full_rect.max.x - full_rect.width() * 0.25;
                                (p.x < left_thresh || p.x > right_thresh)
                                    && p.y >= full_rect.min.y + 44.0
                            })
                            .unwrap_or(false);
                        if fs_response.clicked()
                            && !tile_active
                            && !in_hud
                            && !in_video_panel
                            && let Some(idx) = self.fullscreen_idx
                            && let Some(p) = self.fs_video_player(idx)
                        {
                            p.toggle_play();
                        }
                    } else if fs_response.clicked() {
                        // ポップアップ表示中はクリックでのページ送りを抑制
                        let any_popup = self.spread_popup_open || self.fit_popup_open;
                        if !any_popup {
                            if let Some(pos) = fs_response.interact_pointer_pos() {
                                let panel_threshold = full_rect.max.x - full_rect.width() * 0.25;
                                let in_right_panel = pos.y >= 60.0
                                    && (self.show_metadata_panel
                                        || self.metadata_panel_hover_active
                                        || pos.x > panel_threshold)
                                    && pos.x
                                        > full_rect.max.x
                                            - METADATA_PANEL_WIDTH.min(full_rect.width() * 0.5);
                                let in_left_panel = self.adjustment_mode
                                    && pos.x < adjustment_panel_rect(full_rect).max.x
                                    && pos.y >= 60.0;
                                // 連続読み中はクリックでのページジャンプを抑制する。連続読みは
                                // 連続スクロール表示なので、左半分/右半分クリックで別ファイルへ
                                // 飛ぶのはモデルに反する (特に fs_zoom が一瞬インフレされず
                                // has_transform=false になった隙のクリックで誤爆する)。
                                if !in_right_panel && !in_left_panel && !continuous_active {
                                    let base = if pos.x > full_rect.center().x { 1 } else { -1 };
                                    nav_delta = self.spread_nav_delta(base);
                                }
                            }
                        }
                    }
                }
            }
        }
        // 抑制対象クリックの release フレームでここに来るので、左クリック分岐の
        // スキップが終わったこのタイミングでフラグを落とす。
        if suppress_this_frame && primary_released {
            self.fs_suppress_primary_until_release = false;
        }
        // 分析モード中は右クリックを色固定に使うため、終了トリガーにしない
        // コンテキストメニュー表示中は右クリック処理をスキップ
        // 表示モード / フィットのポップアップ表示中も、右クリックはメニューを閉じる
        // 用途 (popup 側の外クリック判定) に専念させ、フルスクリーン終了 / コンテキスト
        // メニューを誤発火させない。handle_fs_wheel_and_click は hover bar 描画より前に
        // 走るため、ここで読む popup 状態はまだ閉じられていない。
        if !self.analysis_mode
            && !self.is_overlay_edit_mode_active()
            && self.fs_context_menu_idx.is_none()
            && !self.spread_popup_open
            && !self.fit_popup_open
        {
            let secondary_down = ctx.input(|i| i.pointer.secondary_down());
            let secondary_released = ctx.input(|i| i.pointer.secondary_released());
            let secondary_pos = ctx.input(|i| i.pointer.interact_pos().unwrap_or_default());
            // 旧 egui HUD は撤去済 (native presenter overlay が右クリックも独自処理)。
            // egui main window 側に右クリックを吸収すべき HUD 矩形は無い。

            if secondary_down && self.fs_secondary_press_start.is_none() {
                // 押下開始を記録
                self.fs_secondary_press_start = Some((std::time::Instant::now(), secondary_pos));
            }

            if let Some((start_time, start_pos)) = self.fs_secondary_press_start {
                let elapsed = start_time.elapsed();
                let current_pos = ctx.input(|i| i.pointer.interact_pos().unwrap_or(start_pos));
                let moved = current_pos.distance(start_pos);

                if !secondary_released
                    && elapsed >= std::time::Duration::from_millis(400)
                    && moved < 20.0
                {
                    // 長押ししきい値超過 → 押下中にコンテキストメニューを即表示
                    self.fs_context_menu_idx = self.fullscreen_idx;
                    self.fs_context_menu_pos = current_pos;
                    self.fs_secondary_press_start = None;
                } else if secondary_released {
                    if moved < 20.0 && elapsed < std::time::Duration::from_millis(400) {
                        // 短押し → 従来通り閉じる
                        close = true;
                    }
                    self.fs_secondary_press_start = None;
                } else if moved >= 20.0 {
                    // マウスが動きすぎた → キャンセル
                    self.fs_secondary_press_start = None;
                }
            }
        }

        (nav_delta, close)
    }

    // ── ナビゲーション & スライドショー ─────────────────────────────────

    /// スライドショーを 1 ステップ進める。動画はスキップ ([adjacent_slideshow_idx])。
    /// フォルダ末尾に到達したら `slideshow_end_action` 設定に従って
    /// ループ / 次フォルダ / 停止する。
    fn advance_slideshow(&mut self, ctx: &egui::Context, cur: usize) {
        let slide_delta = self.spread_nav_delta(1);
        let display_order = self.current_grid_order().to_vec();
        if let Some(idx) =
            crate::ui_helpers::adjacent_slideshow_idx(&self.items, &display_order, cur, slide_delta)
        {
            // フォルダ内の次の静止画系アイテムへ前進。
            self.slideshow_anchor_idx = None;
            self.open_fullscreen_from_slideshow_navigation(ctx, idx);
            self.selected = Some(idx);
            self.scroll_to_selected = true;
            return;
        }
        // 末尾到達: 設定に従う。
        match self.settings.slideshow_end_action {
            crate::settings::SlideshowEndAction::Stop => {
                self.slideshow_playing = false;
                self.slideshow_anchor_idx = None;
            }
            crate::settings::SlideshowEndAction::LoopFolder => {
                self.loop_slideshow_to_first(ctx);
            }
            crate::settings::SlideshowEndAction::NextFolder => {
                // 次フォルダへ。検索コンテキスト等で発火できなければループにフォールバック。
                if !self.try_start_slideshow_next_folder(cur) {
                    self.loop_slideshow_to_first(ctx);
                }
            }
        }
    }

    /// フォルダ内の先頭の静止画系アイテム (Video / ZipSeparator 除外) へ折り返す。
    /// 静止画系が一つも無ければスライドショーを停止する。
    fn loop_slideshow_to_first(&mut self, ctx: &egui::Context) {
        let display_order = self.current_grid_order().to_vec();
        if let Some(idx) = crate::ui_helpers::first_slideshow_still_idx(&self.items, &display_order)
        {
            self.slideshow_anchor_idx = None;
            self.open_fullscreen_from_slideshow_navigation(ctx, idx);
            self.selected = Some(idx);
            self.scroll_to_selected = true;
        } else {
            self.slideshow_playing = false;
            self.slideshow_anchor_idx = None;
        }
    }

    /// スライドショーの「次フォルダへ進む」を発火する。発火できたら true。
    ///
    /// 手動 Ctrl+↓ と同じ非同期 skip-walk 経路 (`FolderNavMode::SlideshowNext`) を使うが、
    /// 判定述語は静止画ありに限定される ([spawn_folder_nav] 側で選択)。発火時に
    /// `slideshow_playing=false` にして in-flight 中のタイマー/sync 再入を止め、
    /// `capture_fs_nav_holdover` で Ctrl+↑↓ と同じ nav ロックを取得する。復帰は
    /// `FolderNavMode::SlideshowNext` 由来で reopen 側が行う。
    ///
    /// 次フォルダ概念が無い (検索ビュー / お気に入り検索 / Ctrl+F 中) か、現在フォルダが
    /// 取れない場合は発火せず false を返す (呼び出し側でループにフォールバック)。
    fn try_start_slideshow_next_folder(&mut self, fs_idx: usize) -> bool {
        if self.fs_nav_is_locked() {
            // 既に nav 進行中: 二重発火しない (が、ループフォールバックもしない)。
            return true;
        }
        // ★固定 中は snapshot 内の次 playable image-like entry に遷移して slideshow 継続 (= §4.6)。
        if self.is_snapshot_active() {
            self.capture_fs_nav_holdover(fs_idx);
            return self.snapshot_advance_for_slideshow(/*forward=*/ true);
        }
        if self.global_search.active || self.favsearch.active || self.show_search_bar {
            return false;
        }
        // ネスト ZIP の本の中: 手動 Ctrl+↓ (#4) と同じく次の兄弟本へ進み、スライドショーを
        // 継続する (レビュー P3: ここで start_folder_nav に流すと残りの兄弟本をスキップして
        // ZIP ごと脱出してしまう)。端 (最後の本) では従来どおり下の共通経路に落ちて ZIP を
        // 抜け、次の実フォルダへ進む。holdover / 端での lock 残留対策は
        // zip_nav_sibling_fullscreen 側に集約されている。
        if self.zip_nav.as_ref().is_some_and(|n| !n.at_root()) {
            if self.zip_nav_sibling_fullscreen(fs_idx, true) {
                // 移動先の本に画像が無くフルスクリーンが閉じた場合 (sibling 内で
                // slideshow_playing=false 済み) はスケジュールしない。
                if self.fullscreen_idx.is_some() {
                    self.slideshow_playing = true;
                    self.schedule_next_slideshow_from_now();
                }
                return true;
            }
            // 端 (兄弟なし): fall through して ZIP を抜けて次フォルダへ。
        }
        let Some(folder) = self.current_folder.clone() else {
            return false;
        };
        self.slideshow_playing = false;
        self.slideshow_anchor_idx = None;
        self.capture_fs_nav_holdover(fs_idx);
        self.start_folder_nav(folder, true, crate::app::FolderNavMode::SlideshowNext);
        true
    }

    fn slideshow_interval_duration(&self) -> std::time::Duration {
        let secs = self.settings.slideshow_interval_secs;
        let secs = if secs.is_finite() {
            secs.clamp(0.5, 30.0)
        } else {
            3.0
        };
        std::time::Duration::from_secs_f32(secs)
    }

    pub(crate) fn schedule_next_slideshow_from_now(&mut self) {
        self.slideshow_next_at = std::time::Instant::now() + self.slideshow_interval_duration();
        self.slideshow_anchor_idx = self.fullscreen_idx;
    }

    fn current_slideshow_frame_ready(&self, fs_idx: usize, state: &FsFrameState) -> bool {
        if state.separator_text.is_some() {
            return true;
        }
        // 動画は ready 扱いにして必ずタイマーを進める。動画フレームは `state.tex` を
        // 持たず、サムネが未生成だと永久に「未 ready」で止まりうるため。タイマーが
        // 回れば advance_slideshow が adjacent_slideshow_idx で動画を飛ばして次へ送る
        // (= 動画到達でスライドショーが固まらない)。
        if state.is_video {
            return true;
        }
        let has_own_thumb = matches!(
            self.thumbnails.get(fs_idx),
            Some(ThumbnailState::Loaded { .. })
        );
        state.tex.is_some() || has_own_thumb || state.fs_load_failed
    }

    fn sync_slideshow_anchor_for_frame(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        state: &FsFrameState,
    ) {
        if !self.slideshow_playing {
            return;
        }
        // 動画でスライドショーを止めない (ユーザー設定: 動画はスキップして継続)。
        // 自動送り (advance_slideshow) は adjacent_slideshow_idx で動画を飛ばすので、
        // 動画に居るのは手動ナビで来た場合のみ。1 間隔だけ表示して次の静止画へ送る。
        let ready = self.current_slideshow_frame_ready(fs_idx, state);
        if self.slideshow_anchor_idx == Some(fs_idx) {
            if !ready {
                self.slideshow_anchor_idx = None;
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            return;
        }
        if ready {
            self.schedule_next_slideshow_from_now();
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    pub(crate) fn open_fullscreen_from_fs_navigation(&mut self, ctx: &egui::Context, idx: usize) {
        #[cfg(windows)]
        if self.try_start_video_tile_fast_swap(ctx, idx) {
            return;
        }
        #[cfg(windows)]
        if self.try_start_native_video_fast_swap(ctx, idx, None, false) {
            return;
        }

        #[cfg(windows)]
        let restore_video_tile = self.video_tile_mode_active;

        #[cfg(windows)]
        if restore_video_tile {
            self.video_tile_state = None;
            self.video_tile_swap_pending = None;
        } else {
            self.cancel_stale_video_tile_reopen(self.fullscreen_idx, "fs-navigation");
        }

        let cursor_state = self.fullscreen_cursor_state();
        self.open_fullscreen(idx);
        // `open_fullscreen` resets cursor idleness for a new fullscreen entry.
        // Fullscreen-internal navigation should keep the mouse cursor state continuous;
        // keyboard page turns must not revive a hidden cursor, while pointer navigation
        // has already marked the cursor active before reaching this helper.
        self.restore_fullscreen_cursor_state(ctx, cursor_state);

        #[cfg(windows)]
        {
            if restore_video_tile && matches!(self.items.get(idx), Some(GridItem::Video(_))) {
                self.video_tile_mode_active = true;
                self.video_tile_reopen_pending = true;
                self.video_tile_reopen_deadline =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                ctx.request_repaint();
            } else if restore_video_tile {
                self.video_tile_mode_active = false;
                self.video_tile_reopen_pending = false;
                self.video_tile_reopen_deadline = None;
            }
        }
    }

    fn open_fullscreen_from_slideshow_navigation(&mut self, ctx: &egui::Context, idx: usize) {
        // Timer-driven slideshow advances are fullscreen-internal navigation too, so
        // use the same cursor-state carry path as keyboard/mouse page turns.
        self.open_fullscreen_from_fs_navigation(ctx, idx);
    }

    pub(crate) fn handle_fullscreen_ctrl_nav_context(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        forward: bool,
        native_toast: bool,
    ) {
        if self.fs_nav_is_locked() {
            return;
        }
        // 手動 Ctrl+↑↓ (フォルダ移動操作) はスライドショーを止める。成功時は後続の
        // close_fullscreen でも落ちるが、境界 / 画像なし / 検索ビュー no-op など
        // フォルダが変わらないケースでも明示停止して「Ctrl+↑↓ で停止」の一貫した
        // 挙動にする (フォルダ内移動の矢印/ホイール/クリックは継続のまま)。
        // SlideshowNext 自動送りはこの関数を経由せず start_folder_nav を直接呼ぶので
        // ここでは止まらない。
        self.slideshow_playing = false;
        self.slideshow_anchor_idx = None;
        // Cross-scope ナビ (Ctrl+↑↓: 通常のフォルダ移動、Favsearch、Ctrl+G drilled into) が
        // 始まる時点で、video swap 由来の deferred nav delta は別フォルダ / 別検索スコープ
        // / 非動画アイテムで誤発火しうるので破棄する。`capture_fs_nav_holdover` を経由しない
        // 経路 (Ctrl+G DrilledInto の global_search_ctrl_nav_fullscreen など) も含めて
        // 確実にカバーするため、Ctrl+↑↓ ハンドラの入口で一括で消す
        // (Codex 第 8/9 P2 指摘)。
        #[cfg(windows)]
        {
            self.native_video_deferred_nav_delta = None;
        }

        // ★固定 中は snapshot 内 entry を巡回する (= §4.6)。
        // Folder/Image/Video 混合 entry 全部対象。
        if self.is_snapshot_active() {
            self.capture_fs_nav_holdover(fs_idx);
            let _ = self.snapshot_navigate(
                ctx, forward, /*page_only=*/ false, /*resume_slideshow=*/ false,
            );
            return;
        }

        if self.global_search.active {
            if self.global_search.drill.is_some() {
                self.global_search_ctrl_nav_fullscreen(ctx, forward);
            } else {
                // 一覧ビューはグリッドに実画像が並ぶので通常のフルスクリーン前後移動が
                // 使える。フォルダ横断 (Ctrl+↑↓) の概念は無いので no-op ヒントを出す。
                self.show_fullscreen_nav_noop(ctx, FsNavNoOpReason::SearchResultList, native_toast);
            }
            return;
        }

        if self.favsearch.active {
            let Some(root) = self.favsearch.nav_stack.first().cloned() else {
                self.show_fullscreen_nav_noop(ctx, FsNavNoOpReason::SearchResultList, native_toast);
                return;
            };
            let Some(current) = self.favsearch.nav_stack.last().cloned() else {
                return;
            };
            self.capture_fs_nav_holdover(fs_idx);
            self.start_folder_nav(
                current,
                forward,
                crate::app::FolderNavMode::Favsearch {
                    root,
                    fullscreen: true,
                },
            );
            return;
        }

        if self.show_search_bar {
            self.cancel_pending_folder_nav();
            self.show_fullscreen_nav_noop(ctx, FsNavNoOpReason::LocalFilterActive, native_toast);
            return;
        }

        // ネスト ZIP の本の中: Ctrl+↑↓ で兄弟本へ移り、その本の先頭画像を開く (#4)。
        // ルート (本一覧) では下の current_folder 分岐へ流して ZIP を抜ける (BS と対称)。
        // holdover は移動確定後に zip_nav_sibling_fullscreen 内で取る (端で lock が残るのを防ぐ)。
        // 端 (兄弟なし = false) では何もしない (読書中に ZIP を抜けない)。
        if self.zip_nav.as_ref().is_some_and(|n| !n.at_root()) {
            let _ = self.zip_nav_sibling_fullscreen(fs_idx, forward);
            return;
        }

        if let Some(cur) = self.current_folder.clone() {
            self.capture_fs_nav_holdover(fs_idx);
            self.start_folder_nav(cur, forward, crate::app::FolderNavMode::Fullscreen);
        }
    }

    pub(crate) fn handle_fullscreen_sibling_nav_context(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        forward: bool,
        native_toast: bool,
    ) {
        if self.fs_nav_is_locked() {
            return;
        }
        self.slideshow_playing = false;
        self.slideshow_anchor_idx = None;
        #[cfg(windows)]
        {
            self.native_video_deferred_nav_delta = None;
        }

        // ★固定 中は snapshot 内の playable image-like entry のみを巡回 (= §4.6)。
        // Ctrl+PageUp/Down は Folder entry を skip して直接 image/video へ。
        if self.is_snapshot_active() {
            self.capture_fs_nav_holdover(fs_idx);
            let _ = self.snapshot_navigate(
                ctx, forward, /*page_only=*/ true, /*resume_slideshow=*/ false,
            );
            let _ = native_toast;
            return;
        }

        if self.global_search.active || self.favsearch.active {
            self.cancel_pending_folder_nav();
            self.show_fullscreen_nav_noop(
                ctx,
                FsNavNoOpReason::SearchSiblingUnsupported,
                native_toast,
            );
            return;
        }

        if self.show_search_bar {
            self.cancel_pending_folder_nav();
            self.show_fullscreen_nav_noop(ctx, FsNavNoOpReason::LocalFilterActive, native_toast);
            return;
        }

        if let Some(cur) = self.current_folder.clone() {
            self.capture_fs_nav_holdover(fs_idx);
            self.start_folder_nav(cur, forward, crate::app::FolderNavMode::SiblingFullscreen);
        }
    }

    fn show_fullscreen_nav_noop(
        &mut self,
        ctx: &egui::Context,
        reason: FsNavNoOpReason,
        native_toast: bool,
    ) {
        #[cfg(windows)]
        if native_toast {
            self.show_native_video_overlay_toast(Self::nav_noop_title(reason).to_string(), true);
            self.request_native_video_hud_repaint(ctx);
            return;
        }

        #[cfg(not(windows))]
        let _ = (ctx, native_toast);

        self.fs_boundary_hint = Some(FsBoundaryHint::NavNoOp {
            reason,
            at: std::time::Instant::now(),
        });
    }

    pub(crate) fn nav_noop_title(reason: FsNavNoOpReason) -> &'static str {
        match reason {
            FsNavNoOpReason::LocalFilterActive => "Ctrl+F検索中はフォルダ移動しません",
            FsNavNoOpReason::SearchResultList => "検索結果を開いてからCtrl+↑↓で移動できます",
            FsNavNoOpReason::SearchSiblingUnsupported => "検索中は兄弟フォルダ移動しません",
        }
    }

    /// フルスクリーン終了・ナビゲーション・スライドショーを処理する。
    pub(crate) fn handle_fs_navigation(
        &mut self,
        ctx: &egui::Context,
        close_fs: bool,
        close_to_page_list: bool,
        ctrl_nav: Option<i32>,
        sibling_nav: Option<i32>,
        nav_delta: i32,
        jump_to: Option<usize>,
        fs_idx: usize,
    ) {
        if close_fs {
            self.handle_fullscreen_close_request();
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            // keep_fullscreen_viewport_alive の cleanup フレーム (Visible(false) 送信) を保証。
            // 修正後の keep_alive はアイドル時ゼロコスト早期 return するため、偶発的な
            // input/focus repaint に頼らず明示的に次フレームを起こす。
            ctx.request_repaint();
        } else if close_to_page_list {
            // BS: 階層を 1 段戻す = コンテナのページ一覧 (L2) へ。close_fullscreen は
            // current_folder=ZIP/PDF のまま閉じるので L2 が出る (設定で分岐しない)。
            self.close_fullscreen();
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
        // fast-swap (動画タイル / native 動画) が進行中なら、swap 機構側が
        // 表示遷移を完結させるので、handle_fs_navigation 経由の通常 nav 経路は
        // 二重発火を避けるため早期 return する。
        #[cfg(windows)]
        if !close_fs
            && !close_to_page_list
            && (self.video_tile_swap_pending.is_some()
                || self.native_video_fast_swap_pending.is_some())
        {
            return;
        }
        // Ctrl+↑↓ はフルスクリーンを保ったまま現在コンテキストの前後へ飛び、
        // 移動先の先頭 image-like を開く。self.selected も合わせて更新するので、
        // ここからフルスクリーンを閉じたときグリッド側のカーソルが最後に観た項目に残る。
        //
        // 実装上: `navigate_folder_with_skip` は DFS + `read_dir` で UI スレッドを
        // ブロックし得るので (深い階層だと 100ms 級)、ここでは発火だけ行い、
        // 実際の close_fullscreen / load_folder / open_fullscreen は
        // `apply_folder_nav_result` (FolderNavMode::Fullscreen ブランチ) に任せる。
        if let Some(delta) = ctrl_nav {
            self.handle_fullscreen_ctrl_nav_context(ctx, fs_idx, delta > 0, false);
        } else if let Some(delta) = sibling_nav {
            self.handle_fullscreen_sibling_nav_context(ctx, fs_idx, delta > 0, false);
        } else if !close_fs && !close_to_page_list {
            // close (Esc) / close_to_page_list (BS) は終端アクション。閉じた後に同フレームの
            // wheel 由来 nav_delta 等で別項目を開き直さないようガードする。
            if let Some(new_idx) = jump_to {
                self.open_fullscreen_from_fs_navigation(ctx, new_idx);
                self.selected = Some(new_idx);
                self.scroll_to_selected = true;
                self.update_last_selected_image();
            } else if nav_delta != 0 {
                let display_order = self.current_grid_order().to_vec();
                if let Some(new_idx) = crate::ui_helpers::adjacent_navigable_idx(
                    &self.items,
                    &display_order,
                    fs_idx,
                    nav_delta,
                ) {
                    self.open_fullscreen_from_fs_navigation(ctx, new_idx);
                    self.selected = Some(new_idx);
                    self.scroll_to_selected = true;
                    self.update_last_selected_image();
                } else {
                    // 境界到達: 中央にヒントを出す (nav_delta > 0 なら末尾)
                    self.fs_boundary_hint = Some(FsBoundaryHint::Edge {
                        at_end: nav_delta > 0,
                        at: std::time::Instant::now(),
                    });
                    crate::logger::log(format!(
                        "[NAV] adjacent_navigable_idx returned None: fs_idx={fs_idx}, delta={nav_delta}, items={}, visible={}",
                        self.items.len(),
                        display_order.len()
                    ));
                }
            }
        }

        // ── スライドショー タイマー ──
        // フォルダ nav 進行中 (= fs_nav ロック保持中) はタイマーを止める。手動 Ctrl+↑↓ の
        // in-flight 中に旧フォルダで誤って advance するのを防ぐ (SlideshowNext は発火時に
        // slideshow_playing=false にしているのでそもそも入らない)。手動 Ctrl+↑↓ が
        // フォルダを変えた場合は close_fullscreen が slideshow_playing=false にする。
        if self.slideshow_playing && !close_fs && !self.fs_nav_is_locked() {
            let now = std::time::Instant::now();
            let anchored = self
                .fullscreen_idx
                .is_some_and(|idx| self.slideshow_anchor_idx == Some(idx));
            if !anchored {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            } else {
                if now >= self.slideshow_next_at {
                    if let Some(cur) = self.fullscreen_idx {
                        self.advance_slideshow(ctx, cur);
                    }
                    // 前進 / 折り返し / 次フォルダ発火 (async) いずれも次フレームで反映する。
                    ctx.request_repaint();
                }
                if self.slideshow_playing {
                    let remaining = self.slideshow_next_at.saturating_duration_since(now);
                    ctx.request_repaint_after(remaining.min(std::time::Duration::from_millis(100)));
                }
            }
        }
    }

    /// フルスクリーンの再描画リクエストを管理する。
    fn handle_fs_repaint(&self, ctx: &egui::Context, fs_idx: usize, is_video: bool) {
        // 高解像度読み込み完了まで、またはPDF再レンダリング中は毎フレーム再描画
        let image_loading = !is_video
            && self
                .fullscreen_idx
                .map(|i| !self.fs_cache.contains_key(&i))
                .unwrap_or(false);
        let pdf_rerendering = self.fs_pending.contains_key(&fs_idx);
        if image_loading || pdf_rerendering {
            ctx.request_repaint();
        }

        // 右クリック長押し検出中: しきい値チェックのため再描画をリクエスト
        if let Some((start_time, _)) = self.fs_secondary_press_start {
            let remaining =
                std::time::Duration::from_millis(400).saturating_sub(start_time.elapsed());
            if remaining.is_zero() {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(remaining);
            }
        }

        // アニメーション: 次フレームの時刻まで待ってから再描画
        if !is_video {
            if let Some(FsCacheEntry::Animated { next_frame_at, .. }) = self.fs_cache.get(&fs_idx) {
                let delay = (next_frame_at - ctx.input(|i| i.time)).max(0.0);
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(delay));
            }
        }
    }

    // ── フルスクリーン描画ヘルパー ──────────────────────────────────────

    /// ZIP セパレータの章タイトル画面を描画する。
    fn draw_fs_separator(ui: &mut egui::Ui, full_rect: egui::Rect, sep: &str) {
        let title_size = (full_rect.height() * 0.12).clamp(48.0, 120.0);
        let sub_size = (full_rect.height() * 0.030).clamp(20.0, 36.0);

        ui.painter().rect_filled(
            egui::Rect::from_center_size(
                full_rect.center(),
                egui::vec2(full_rect.width() * 0.85, title_size * 2.2),
            ),
            16.0,
            egui::Color32::from_rgba_unmultiplied(30, 45, 80, 180),
        );
        ui.painter().text(
            full_rect.center(),
            egui::Align2::CENTER_CENTER,
            sep,
            egui::FontId::proportional(title_size),
            egui::Color32::WHITE,
        );
        ui.painter().text(
            egui::pos2(full_rect.center().x, full_rect.max.y - 48.0),
            egui::Align2::CENTER_BOTTOM,
            "── 作品の区切り ──",
            egui::FontId::proportional(sub_size),
            egui::Color32::from_rgb(150, 180, 220),
        );
    }

    /// 連結読み中の ZIP セパレータを、画像ページ間の軽い区切り帯として描画する。
    fn draw_fs_continuous_separator(
        painter: &egui::Painter,
        rect: egui::Rect,
        sep: &str,
        flow: crate::settings::ReadingFlow,
    ) {
        let inset = if flow.is_horizontal() {
            egui::vec2(8.0, 18.0)
        } else {
            egui::vec2(18.0, 8.0)
        };
        let band = rect.shrink2(inset);
        if band.width() < 32.0 || band.height() < 32.0 {
            return;
        }

        painter.rect_filled(
            band,
            8.0,
            egui::Color32::from_rgba_unmultiplied(30, 45, 80, 180),
        );
        painter.rect_stroke(
            band,
            8.0,
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(135, 165, 215, 130),
            ),
            egui::StrokeKind::Outside,
        );

        let title_size = if flow.is_horizontal() {
            (band.width() * 0.11).clamp(18.0, 32.0)
        } else {
            (band.height() * 0.28).clamp(20.0, 44.0)
        };
        let sub_size = (title_size * 0.52).clamp(12.0, 20.0);
        let show_sub = band.height() >= 72.0 && band.width() >= 120.0;
        let line_gap = (title_size * 0.22).clamp(8.0, 14.0);
        let title_y = if show_sub {
            band.center().y - (title_size + line_gap + sub_size) * 0.5
        } else {
            band.center().y - title_size * 0.5
        };
        crate::ui_helpers::draw_centered_elided_label(
            painter,
            band,
            sep,
            title_size,
            egui::Color32::WHITE,
            title_y,
            18.0,
        );

        if show_sub {
            crate::ui_helpers::draw_centered_elided_label(
                painter,
                band,
                "作品の区切り",
                sub_size,
                egui::Color32::from_rgb(150, 180, 220),
                title_y + title_size + line_gap,
                18.0,
            );
        }
    }

    /// フルスクリーンの画像 / 動画 / 読込中 / 失敗 表示を描画する。
    /// zoom/pan が Some のとき分析モードのズーム/パンを適用する。
    /// `bg_style` が Default 以外のとき、画像 rect の直下に透過背景を塗る。
    ///
    /// 動画は native presenter が独立 HWND に直接描画するので、ここでは
    /// 余白カットフィット用の中身 bbox を取得 (キャッシュ付き)。設定 OFF / 余白なし /
    /// pixels 未取得 (アニメ等) なら None。補正前後で余白はほぼ不変なので raw fs_cache の
    /// pixels から検出する。
    pub(crate) fn fs_margin_bbox(&mut self, idx: usize) -> Option<egui::Rect> {
        if !matches!(
            self.effective_fullscreen_fit_mode(),
            FullscreenFitMode::MarginFit
        ) {
            return None;
        }
        if let Some(cached) = self.fs_margin_bbox_cache.get(&idx) {
            return *cached;
        }
        let bbox = match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { pixels, .. }) => crate::margin_fit::detect_content_bbox(
                &**pixels,
                crate::margin_fit::DEFAULT_TOLERANCE,
            ),
            _ => None,
        };
        self.fs_margin_bbox_cache.insert(idx, bbox);
        bbox
    }

    /// 余白カット検出の診断を logger に出力する (デバッグ用、ボタン ON 時に呼ぶ)。
    /// 検出された各連結成分の面積・位置と、各辺を決めている成分をログに出すので、
    /// 「右が詰まらない原因 (小さな点/汚れ/本文が端まで届く)」やしきい値の目安が分かる。
    pub(crate) fn log_margin_fit_diag(&self, idx: usize) {
        let pixels = match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { pixels, .. }) => pixels.clone(),
            _ => {
                crate::logger::log(
                    "[margin-fit diag] pixels 未取得 (再デコード待ち)。少し待って再度トグル"
                        .to_string(),
                );
                return;
            }
        };
        let diag = crate::margin_fit::diagnose(&pixels, crate::margin_fit::DEFAULT_TOLERANCE);
        let bbox_s = match diag.bbox {
            Some(r) => format!(
                "[{:.3},{:.3} .. {:.3},{:.3}]",
                r.min.x, r.min.y, r.max.x, r.max.y
            ),
            None => "None (通常フィット)".to_string(),
        };
        crate::logger::log(format!(
            "[margin-fit diag] idx={} orig={}x{} downscaled={}x{} margin_luma={} border_frac={:.2} min_area={} tol={} 成分数={} 最終bbox={}",
            idx,
            pixels.size[0],
            pixels.size[1],
            diag.downscaled.0,
            diag.downscaled.1,
            diag.margin_luma,
            diag.border_margin_frac,
            diag.min_area,
            diag.tol,
            diag.components.len(),
            bbox_s
        ));
        // 実際にカットされる成分 (= 最終 bbox の外へはみ出すもの) を上位5件だけ要約。原因
        // 切り分け (番号/ゴミが切られているか・面積いくつか) に一番効くのでこれだけ残す。
        // 詳しい全成分ダンプが要るときは margin_fit::diagnose の結果をここで展開すればよい。
        // bbox=None (通常フィット) のときは何も切られない。
        if let Some(bb) = diag.bbox {
            let eps = 1e-4;
            let mut cut: Vec<&crate::margin_fit::DiagComponent> = diag
                .components
                .iter()
                .filter(|c| {
                    c.rect.min.x < bb.min.x - eps
                        || c.rect.max.x > bb.max.x + eps
                        || c.rect.min.y < bb.min.y - eps
                        || c.rect.max.y > bb.max.y + eps
                })
                .collect();
            cut.sort_by(|a, b| b.area.cmp(&a.area));
            let shown: Vec<String> = cut
                .iter()
                .take(5)
                .map(|c| {
                    format!(
                        "area={}@({:.3},{:.3}){}",
                        c.area,
                        (c.rect.min.x + c.rect.max.x) * 0.5,
                        (c.rect.min.y + c.rect.max.y) * 0.5,
                        if c.kept { "KEEP" } else { "" }
                    )
                })
                .collect();
            crate::logger::log(format!(
                "[margin-fit diag] カット対象 {}件 上位5: {}",
                cut.len(),
                shown.join(" / ")
            ));
        }
    }

    /// 静止画 / アニメーション / サムネイル / プレースホルダーだけを扱う。
    #[allow(clippy::too_many_arguments)]
    fn draw_fs_image(
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        tex: Option<&egui::TextureHandle>,
        thumb_tex: Option<&egui::TextureHandle>,
        is_video: bool,
        vst3_waiting_for_video: bool,
        fs_load_failed: bool,
        rotation: crate::rotation_db::Rotation,
        zoom_pan: Option<(f32, egui::Vec2)>,
        free_rotation_rad: f32,
        bg_style: &FsBgStyle<'_>,
        // 読込中プレースホルダ直下に出す対象パス (`location_display_for` 参照)。
        // 空ならラベル描画をスキップ。
        location_display: &str,
        pixel_grid_enabled: bool,
        fit_mode: FullscreenFitMode,
        // 余白カットフィット用の中身 bbox (正規化 0..1)。Some & rotation なしのとき適用。
        content_bbox: Option<egui::Rect>,
    ) {
        let using_full_texture = tex.is_some();
        let display_tex = tex.or(thumb_tex);
        if let Some(handle) = display_tex {
            let tex_size = handle.size_vec2();
            let display_size = match rotation {
                crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
                    egui::vec2(tex_size.y, tex_size.x)
                }
                _ => tex_size,
            };
            let margin_bbox = content_bbox
                .filter(|_| rotation.is_none() && free_rotation_rad.abs() <= TRANSFORM_EPSILON);
            let page_fit =
                || (full_rect.width() / display_size.x).min(full_rect.height() / display_size.y);
            let (fit_scale, content_center_px) = match fit_mode {
                FullscreenFitMode::MarginFit if margin_bbox.is_some() => {
                    let bbox = margin_bbox.unwrap();
                    let bbox_w = (bbox.width() * display_size.x).max(1.0);
                    let bbox_h = (bbox.height() * display_size.y).max(1.0);
                    let s = (full_rect.width() / bbox_w).min(full_rect.height() / bbox_h);
                    let cpx = egui::vec2(
                        (bbox.center().x - 0.5) * display_size.x,
                        (bbox.center().y - 0.5) * display_size.y,
                    );
                    (s, cpx)
                }
                FullscreenFitMode::Width => (full_rect.width() / display_size.x, egui::Vec2::ZERO),
                FullscreenFitMode::Height => {
                    (full_rect.height() / display_size.y, egui::Vec2::ZERO)
                }
                FullscreenFitMode::Original => (1.0, egui::Vec2::ZERO),
                _ => (page_fit(), egui::Vec2::ZERO),
            };
            let (total_scale, base_center) = match zoom_pan {
                Some((zoom, pan)) => (fit_scale * zoom, full_rect.center() + pan),
                None => (fit_scale, full_rect.center()),
            };
            // 中身 bbox の中心をウィンドウ中心へ寄せる (content_center_px=0 なら従来どおり)。
            let center = base_center - content_center_px * total_scale;
            let img_rect = egui::Rect::from_center_size(center, display_size * total_scale);
            let needs_clip = zoom_pan.is_some()
                || free_rotation_rad.abs() > TRANSFORM_EPSILON
                || !matches!(fit_mode, FullscreenFitMode::Page);
            let painter = if needs_clip {
                ui.painter().with_clip_rect(full_rect)
            } else {
                ui.painter().clone()
            };
            // 透過画像用背景 (B キーで切替)。回転時は img_rect が回転前の bbox になるため
            // 視覚的ズレを避けて rotation が None のときのみ適用する。
            if rotation.is_none() && free_rotation_rad.abs() <= TRANSFORM_EPSILON {
                paint_transparent_bg(&painter, img_rect, bg_style);
            }
            if rotation.is_none() && free_rotation_rad.abs() <= TRANSFORM_EPSILON {
                painter.image(
                    handle.id(),
                    img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                crate::app::draw_rotated_image_ex(
                    &painter,
                    handle.id(),
                    img_rect,
                    rotation,
                    free_rotation_rad,
                    center,
                );
            }
            if should_draw_fs_pixel_grid(pixel_grid_enabled, using_full_texture, zoom_pan) {
                Self::draw_fs_pixel_grid(
                    &painter,
                    full_rect,
                    img_rect,
                    tex_size,
                    rotation,
                    free_rotation_rad,
                    center,
                    total_scale,
                    ui.ctx().pixels_per_point(),
                );
            }
        } else if fs_load_failed {
            ui.painter().text(
                full_rect.center(),
                egui::Align2::CENTER_CENTER,
                "読込失敗",
                egui::FontId::proportional(32.0),
                egui::Color32::from_rgb(255, 140, 140),
            );
            ui.painter().text(
                full_rect.center() + egui::vec2(0.0, 40.0),
                egui::Align2::CENTER_CENTER,
                "このファイルはデコードできませんでした",
                egui::FontId::proportional(16.0),
                egui::Color32::from_gray(180),
            );
        } else {
            let painter = ui.painter();
            let loading_label = if vst3_waiting_for_video {
                "VST3 プラグインを初期化中..."
            } else if is_video {
                "動画サムネイル 読込中..."
            } else {
                "読込中..."
            };
            painter.text(
                full_rect.center(),
                egui::Align2::CENTER_CENTER,
                loading_label,
                egui::FontId::proportional(24.0),
                egui::Color32::from_gray(180),
            );
            crate::ui_helpers::draw_centered_elided_label(
                painter,
                full_rect,
                location_display,
                14.0,
                egui::Color32::from_gray(170),
                full_rect.center().y + 22.0,
                20.0,
            );
        }
    }

    pub(crate) fn scroll_vertical_reading_by(&mut self, ctx: &egui::Context, delta: f32) {
        if delta.abs() <= 0.5 {
            return;
        }
        self.fs_vertical_scroll += delta;
        ctx.request_repaint();
    }

    fn continuous_reading_viewport_len_for_flow(&self, ctx: &egui::Context) -> f32 {
        let viewport = ctx.content_rect();
        if self.reading_flow.is_horizontal() {
            viewport.width().max(1.0)
        } else {
            viewport.height().max(1.0)
        }
    }

    fn continuous_reading_key_step_px(&self, ctx: &egui::Context) -> f32 {
        self.continuous_reading_viewport_len_for_flow(ctx)
            * (self
                .settings
                .continuous_reading_key_scroll_percent
                .clamp(1, 100) as f32
                / 100.0)
    }

    fn continuous_reading_wheel_delta_px(&self, ctx: &egui::Context, wheel_y: f32) -> f32 {
        let notches = wheel_y / CONTINUOUS_READING_WHEEL_REFERENCE_DELTA;
        -notches
            * self.continuous_reading_viewport_len_for_flow(ctx)
            * (self
                .settings
                .continuous_reading_wheel_scroll_percent
                .clamp(1, 100) as f32
                / 100.0)
    }

    pub(crate) fn continuous_reading_gamepad_speed_px_per_sec(&self, ctx: &egui::Context) -> f32 {
        self.continuous_reading_viewport_len_for_flow(ctx)
            * (self
                .settings
                .continuous_reading_gamepad_scroll_percent_per_sec
                .clamp(10, 300) as f32
                / 100.0)
    }

    pub(crate) fn scroll_vertical_reading_step(&mut self, ctx: &egui::Context, direction: f32) {
        self.scroll_vertical_reading_by(ctx, direction * self.continuous_reading_key_step_px(ctx));
    }

    pub(crate) fn persist_current_spread_mode(&self) {
        // ネスト ZIP は本 (zip_path + 階層) ごとに独立記憶。通常は current_folder。
        if let (Some(db), Some(key)) = (&self.spread_db, self.spread_container_key()) {
            let _ = db.set(
                &key,
                self.spread_mode,
                self.settings.default_spread_mode,
                self.settings.default_reading_flow,
                self.settings.default_reading_direction,
            );
        }
    }

    pub(crate) fn persist_current_reading_flow(&self) {
        if let (Some(db), Some(key)) = (&self.spread_db, self.spread_container_key()) {
            let _ = db.set_flow(
                &key,
                self.reading_flow,
                self.reading_direction,
                self.settings.default_spread_mode,
                self.settings.default_reading_flow,
                self.settings.default_reading_direction,
            );
        }
    }

    pub(crate) fn reset_continuous_reading_transform(&mut self) {
        self.fs_vertical_scroll = 0.0;
        self.fs_pan = egui::Vec2::ZERO;
        self.fs_free_rotation = 0.0;
        self.fs_vertical_cache_keep_set.clear();
    }

    fn reset_fullscreen_fit_transform(&mut self, fs_idx: usize) {
        self.fs_zoom = 1.0;
        self.fs_pan = egui::Vec2::ZERO;
        self.fs_free_rotation = 0.0;
        self.maybe_rerender_pdf(1.0);
        if matches!(
            self.settings.fullscreen_fit_mode,
            FullscreenFitMode::MarginFit
        ) {
            self.log_margin_fit_diag(fs_idx);
        }
    }

    fn effective_fullscreen_fit_mode(&self) -> FullscreenFitMode {
        self.settings
            .fullscreen_fit_mode
            .effective_for_flow(self.reading_flow)
    }

    fn fullscreen_fit_allows_drag_pan(&self) -> bool {
        matches!(
            self.effective_fullscreen_fit_mode(),
            FullscreenFitMode::Width | FullscreenFitMode::Height | FullscreenFitMode::Original
        )
    }

    fn set_fullscreen_fit_mode_for_current(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        mode: FullscreenFitMode,
    ) {
        let mode = mode.effective_for_flow(self.reading_flow);
        if self.settings.fullscreen_fit_mode == mode
            && self.settings.margin_fit_enabled == matches!(mode, FullscreenFitMode::MarginFit)
        {
            return;
        }
        self.settings.fullscreen_fit_mode = mode;
        self.settings.margin_fit_enabled = matches!(mode, FullscreenFitMode::MarginFit);
        self.fs_margin_bbox_cache.clear();
        self.reset_fullscreen_fit_transform(fs_idx);
        self.settings.save();
        ctx.request_repaint();
    }

    fn cycle_fullscreen_fit_mode(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let next = self
            .effective_fullscreen_fit_mode()
            .next_for_flow(self.reading_flow);
        self.set_fullscreen_fit_mode_for_current(ctx, fs_idx, next);
    }

    fn set_default_fullscreen_fit_for_flow(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        flow: ReadingFlow,
    ) {
        let mode = FullscreenFitMode::default_for_flow(flow);
        self.settings.fullscreen_fit_mode = mode;
        self.settings.margin_fit_enabled = false;
        self.fs_margin_bbox_cache.clear();
        self.reset_fullscreen_fit_transform(fs_idx);
        self.settings.save();
        ctx.request_repaint();
    }

    pub(crate) fn disable_non_paged_fullscreen_modes(&mut self, fs_idx: usize) {
        self.compare_view_mode = crate::app::CompareViewMode::Off;
        if self.is_panorama_mode_active(fs_idx) {
            self.toggle_panorama_mode(fs_idx);
        }
        if self.analysis_mode {
            self.reset_analysis_mode();
        }
        self.adjustment_mode = false;
        if self.erase_mode {
            self.reset_erase_mode();
        }
        if self.conceal_mode {
            self.reset_conceal_mode();
        }
        if self.text_mode {
            self.text_spread_ctx = None;
            self.reset_text_mode();
        }
        if self.export_crop_mode {
            self.export_crop_spread_ctx = None;
            self.reset_export_crop_mode();
        }
        self.local_adjust_mode = false;
        self.local_adjust_add_layer_dialog_open = false;
        self.local_adjust_change_mask_dialog_open = false;
        self.local_adjust_change_mask_keep_manual_override = true;
        self.local_adjust_effect_picker_dialog_open = false;
        self.local_adjust_canvas_drag = None;
        self.local_adjust_mask_brush_stroke = None;
        self.local_adjust_mask_lasso_points.clear();
        self.local_adjust_selected_shape = None;
    }

    pub(crate) fn set_reading_flow_for_fullscreen(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        flow: ReadingFlow,
    ) {
        if flow == self.reading_flow {
            return;
        }
        self.reading_flow = flow;
        self.reset_continuous_reading_transform();
        self.set_default_fullscreen_fit_for_flow(ctx, fs_idx, flow);
        if !flow.is_paged() {
            self.disable_non_paged_fullscreen_modes(fs_idx);
        }
        self.persist_current_reading_flow();
        ctx.request_repaint();
    }

    pub(crate) fn update_reading_direction_from_spread_mode(&mut self, mode: SpreadMode) {
        if mode.is_rtl() {
            self.reading_direction = ReadingDirection::Rtl;
        } else if matches!(mode, SpreadMode::Ltr | SpreadMode::LtrCover) {
            self.reading_direction = ReadingDirection::Ltr;
        }
    }

    pub(crate) fn sync_spread_mode_from_reading_direction(&mut self) -> bool {
        let new_mode = self
            .spread_mode
            .with_reading_direction(self.reading_direction);
        if new_mode == self.spread_mode {
            return false;
        }
        self.spread_mode = new_mode;
        self.adjust_spread_target = crate::app::AdjustSpreadTarget::Left;
        true
    }

    pub(crate) fn set_reading_direction_for_fullscreen(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        direction: ReadingDirection,
    ) {
        let direction_changed = direction != self.reading_direction;
        self.reading_direction = direction;
        let spread_changed = self.sync_spread_mode_from_reading_direction();
        if !direction_changed && !spread_changed {
            return;
        }
        self.reset_continuous_reading_transform();
        if !self.reading_flow.is_paged() {
            self.disable_non_paged_fullscreen_modes(fs_idx);
        }
        if spread_changed {
            self.persist_current_spread_mode();
        }
        self.persist_current_reading_flow();
        ctx.request_repaint();
    }

    pub(crate) fn vertical_reading_supported_idx(&self, idx: usize) -> bool {
        matches!(
            self.items.get(idx),
            Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::PdfPage { .. })
        )
    }

    fn continuous_reading_supported_idx(&self, idx: usize) -> bool {
        self.vertical_reading_supported_idx(idx)
            || matches!(self.items.get(idx), Some(GridItem::ZipSeparator { .. }))
    }

    /// レンダラが連続読み (縦/横スクロール) を実際に描画する条件と同一の述語。
    ///
    /// 入力ハンドラ (キー / ホイール / クリック / gamepad) は「連続読みとして扱うか」を
    /// この 1 箇所で判定し、レンダリングと入力の食い違いを防ぐ。例:
    /// - 縦/横読み中に Z(解析) や X/C(比較) へ入るとレンダラは単ページにフォールバックする
    ///   のに、入力側が `reading_flow` だけ見て ↑↓ をスクロールへ吸い、ナビもスクロールも
    ///   しない (デッド入力) 状態になる。
    /// - 動画や非対応アイテム上で連結方式が誤適用される。
    ///
    /// 描画側 (`continuous_reading_active`) の判定とこの述語を揃えること。
    pub(crate) fn continuous_reading_active_for_idx(&self, idx: usize) -> bool {
        !self.reading_flow.is_paged()
            && !self.analysis_mode
            && !matches!(self.items.get(idx), Some(GridItem::Video(_)))
            && self.continuous_reading_supported_idx(idx)
            && matches!(self.compare_view_mode, crate::app::CompareViewMode::Off)
            && !self.is_overlay_edit_mode_active()
            && !self.is_panorama_mode_active(idx)
    }

    fn continuous_reading_units_and_pos(
        &self,
        idx: usize,
    ) -> Option<(Vec<ContinuousReadingUnitSpec>, usize)> {
        let display_order = self.current_grid_order().to_vec();
        let image_indices = build_image_reading_indices(&self.items, &display_order);
        let mut image_units = Vec::new();

        if self.spread_mode.is_spread() {
            let pair_start = if self.spread_mode.has_cover() { 1 } else { 0 };
            let mut pos = 0usize;
            while pos < image_indices.len() {
                let current = image_indices[pos];
                if (pair_start == 1 && pos == 0)
                    || is_landscape(current, &self.fs_cache, &self.thumbnails)
                {
                    image_units.push(ContinuousReadingUnitSpec::pages(current, vec![current]));
                    pos += 1;
                    continue;
                }

                if (pos - pair_start) % 2 != 0 {
                    image_units.push(ContinuousReadingUnitSpec::pages(current, vec![current]));
                    pos += 1;
                    continue;
                }

                let Some(&partner) = image_indices.get(pos + 1) else {
                    image_units.push(ContinuousReadingUnitSpec::pages(current, vec![current]));
                    pos += 1;
                    continue;
                };
                if is_landscape(partner, &self.fs_cache, &self.thumbnails) {
                    image_units.push(ContinuousReadingUnitSpec::pages(current, vec![current]));
                    pos += 1;
                    continue;
                }

                let pages = if self.spread_mode.is_rtl() {
                    vec![partner, current]
                } else {
                    vec![current, partner]
                };
                image_units.push(ContinuousReadingUnitSpec::pages(current, pages));
                pos += 2;
            }
        } else {
            image_units.extend(
                image_indices
                    .iter()
                    .copied()
                    .map(|idx| ContinuousReadingUnitSpec::pages(idx, vec![idx])),
            );
        }

        let mut unit_by_page = std::collections::HashMap::new();
        for (unit_pos, unit) in image_units.iter().enumerate() {
            for &page_idx in &unit.pages {
                unit_by_page.insert(page_idx, unit_pos);
            }
        }

        let mut inserted_image_units = std::collections::HashSet::new();
        let mut units = Vec::new();
        for &visible_idx in &display_order {
            match self.items.get(visible_idx) {
                Some(GridItem::ZipSeparator { dir_display }) => {
                    units.push(ContinuousReadingUnitSpec::separator(
                        visible_idx,
                        dir_display.clone(),
                    ));
                }
                Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::PdfPage { .. }) => {
                    let Some(&unit_pos) = unit_by_page.get(&visible_idx) else {
                        continue;
                    };
                    if inserted_image_units.insert(unit_pos) {
                        units.push(image_units[unit_pos].clone());
                    }
                }
                _ => {}
            }
        }

        let pos = units.iter().position(|unit| unit.contains_idx(idx))?;
        Some((units, pos))
    }

    fn vertical_reading_base_size(&mut self, idx: usize, fallback: egui::Vec2) -> egui::Vec2 {
        let rotation = self.get_rotation(idx);
        let raw = match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static {
                tex, source_dims, ..
            }) => match source_dims {
                Some([w, h]) => Some(egui::vec2(*w as f32, *h as f32)),
                None => Some(tex.size_vec2()),
            },
            Some(FsCacheEntry::Animated {
                frames,
                current_frame,
                ..
            }) => frames.get(*current_frame).map(|(tex, _)| tex.size_vec2()),
            _ => None,
        }
        .or_else(|| {
            self.fs_early_dims
                .get(&idx)
                .map(|[w, h]| egui::vec2(*w as f32, *h as f32))
        })
        .or_else(|| match self.thumbnails.get(idx) {
            Some(ThumbnailState::Loaded {
                tex, source_dims, ..
            }) => source_dims
                .map(|(w, h)| egui::vec2(w as f32, h as f32))
                .or_else(|| Some(tex.size_vec2())),
            _ => None,
        })
        .unwrap_or(fallback);

        let size = match rotation {
            crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
                egui::vec2(raw.y, raw.x)
            }
            _ => raw,
        };
        if size.x > 0.0 && size.y > 0.0 {
            size
        } else {
            egui::vec2(fallback.x.max(1.0), fallback.y.max(1.0))
        }
    }

    fn continuous_unit_base_size(
        &mut self,
        unit: &ContinuousReadingUnitSpec,
        flow: crate::settings::ReadingFlow,
        fallback: egui::Vec2,
    ) -> ContinuousReadingUnitSize {
        if unit.separator_text.is_some() {
            let size = continuous_separator_base_size(flow, fallback);
            return ContinuousReadingUnitSize {
                width: size.x,
                height: size.y,
                page_gap: 0.0,
                pages: Vec::new(),
            };
        }

        let mut page_bases = unit
            .pages
            .iter()
            .copied()
            .map(|idx| {
                let base = self.vertical_reading_base_size(idx, fallback);
                ContinuousReadingPageSize {
                    idx,
                    width: base.x.max(1.0),
                    height: base.y.max(1.0),
                }
            })
            .collect::<Vec<_>>();

        if page_bases.len() <= 1 {
            let page = page_bases.pop().unwrap_or(ContinuousReadingPageSize {
                idx: unit.anchor_idx,
                width: fallback.x.max(1.0),
                height: fallback.y.max(1.0),
            });
            return ContinuousReadingUnitSize {
                width: page.width,
                height: page.height,
                page_gap: 0.0,
                pages: vec![page],
            };
        }

        let combined_h = page_bases
            .iter()
            .map(|page| page.height)
            .fold(1.0_f32, f32::max);
        let mut combined_w = 0.0;
        for page in &mut page_bases {
            page.width *= combined_h / page.height.max(1.0);
            page.height = combined_h;
            combined_w += page.width;
        }
        ContinuousReadingUnitSize {
            pages: page_bases,
            width: combined_w.max(1.0),
            height: combined_h.max(1.0),
            page_gap: 0.0,
        }
    }

    fn continuous_unit_size_for_flow(
        &mut self,
        unit: &ContinuousReadingUnitSpec,
        flow: crate::settings::ReadingFlow,
        image_rect: egui::Rect,
        zoom: f32,
        fallback: egui::Vec2,
    ) -> ContinuousReadingUnitSize {
        let mut base = self.continuous_unit_base_size(unit, flow, fallback);
        let fit_mode = self.effective_fullscreen_fit_mode();
        let spread_gap = self.settings.spread_page_gap_px.min(200) as f32;
        let page_gap = if base.pages.len() > 1 {
            spread_gap
        } else {
            0.0
        };
        let (fit_width, fit_gap_total) = continuous_spread_fit_width(
            base.pages.len(),
            base.width,
            self.spread_mode,
            flow,
            fit_mode,
            spread_gap,
        );
        let target_w = (image_rect.width() * zoom - fit_gap_total).max(1.0);
        let target_h = (image_rect.height() * zoom).max(1.0);
        let scale = match fit_mode {
            FullscreenFitMode::Width => target_w / fit_width.max(1.0),
            FullscreenFitMode::Height => target_h / base.height.max(1.0),
            FullscreenFitMode::Original => zoom,
            FullscreenFitMode::Page => {
                (target_w / base.width.max(1.0)).min(target_h / base.height.max(1.0))
            }
            FullscreenFitMode::MarginFit => {
                if flow.is_horizontal() {
                    target_h / base.height.max(1.0)
                } else {
                    target_w / fit_width.max(1.0)
                }
            }
        };
        base.width = (fit_width * scale + fit_gap_total).max(1.0);
        base.height = (base.height * scale).max(1.0);
        base.page_gap = page_gap;
        for page in &mut base.pages {
            page.width = (page.width * scale).max(1.0);
            page.height = (page.height * scale).max(1.0);
        }
        base
    }

    fn vertical_reading_loaded_texels(&self, idx: usize) -> usize {
        match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { tex, .. }) => {
                let s = tex.size_vec2();
                (s.x.max(1.0) as usize).saturating_mul(s.y.max(1.0) as usize)
            }
            Some(FsCacheEntry::Animated { frames, .. }) => frames
                .iter()
                .map(|(tex, _)| {
                    let s = tex.size_vec2();
                    (s.x.max(1.0) as usize).saturating_mul(s.y.max(1.0) as usize)
                })
                .sum(),
            _ => 0,
        }
    }

    fn continuous_visible_page_count(
        units: &[ContinuousReadingUnitSpec],
        visible_positions: &[usize],
    ) -> usize {
        visible_positions
            .iter()
            .filter_map(|&pos| units.get(pos))
            .map(|unit| unit.pages.len())
            .sum()
    }

    fn continuous_reading_layout(
        &mut self,
        ctx: &egui::Context,
        image_rect: egui::Rect,
        units: &[ContinuousReadingUnitSpec],
        current_pos: usize,
    ) -> Option<(
        Vec<VerticalReadingPage>,
        Vec<VerticalReadingSeparator>,
        Vec<usize>,
        Vec<f32>,
    )> {
        if units.is_empty() || current_pos >= units.len() {
            return None;
        }
        let flow = self.reading_flow;
        let fallback = egui::vec2(image_rect.width().max(1.0), image_rect.height().max(1.0));
        let viewport_len = if flow.is_horizontal() {
            image_rect.width().max(1.0)
        } else {
            image_rect.height().max(1.0)
        };
        let mut zoom = self.fs_zoom.clamp(ZOOM_MIN, ZOOM_MAX);
        let mut sizes = Vec::new();
        let mut offsets = Vec::new();
        let mut visible_positions = Vec::new();

        for _ in 0..8 {
            sizes.clear();
            for unit in units {
                sizes.push(
                    self.continuous_unit_size_for_flow(unit, flow, image_rect, zoom, fallback),
                );
            }
            apply_continuous_separator_unit_sizes(units, &mut sizes);
            let extents = sizes
                .iter()
                .map(|s| {
                    if flow.is_horizontal() {
                        s.width
                    } else {
                        s.height
                    }
                })
                .collect::<Vec<_>>();
            offsets = vertical_reading_offsets(
                &extents,
                self.settings.continuous_reading_gap_px.min(200) as f32,
                current_pos,
            );
            self.fs_vertical_scroll = clamp_vertical_reading_scroll(
                self.fs_vertical_scroll,
                &offsets,
                &extents,
                viewport_len,
            );
            visible_positions = vertical_reading_visible_positions(
                &offsets,
                &extents,
                self.fs_vertical_scroll,
                viewport_len,
            );
            let visible_pages = Self::continuous_visible_page_count(units, &visible_positions);
            if visible_pages <= VERTICAL_READING_MAX_VISIBLE_PAGES || zoom >= ZOOM_MAX {
                break;
            }
            let factor =
                ((visible_pages as f32 / VERTICAL_READING_MAX_VISIBLE_PAGES as f32).sqrt() * 1.05)
                    .max(1.05);
            zoom = (zoom * factor).min(ZOOM_MAX);
        }

        if zoom > self.fs_zoom + TRANSFORM_EPSILON {
            self.fs_zoom = zoom;
            ctx.request_repaint();
        }

        while Self::continuous_visible_page_count(units, &visible_positions)
            > VERTICAL_READING_MAX_VISIBLE_PAGES
        {
            let Some((remove_idx, _)) =
                visible_positions
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| {
                        (offsets[**a] - self.fs_vertical_scroll)
                            .abs()
                            .partial_cmp(&(offsets[**b] - self.fs_vertical_scroll).abs())
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
            else {
                break;
            };
            visible_positions.remove(remove_idx);
        }

        let mut pages = Vec::new();
        let mut separators = Vec::new();
        for list_pos in visible_positions.iter().copied() {
            let Some(size) = sizes.get(list_pos) else {
                continue;
            };
            let unit_center = if flow.is_horizontal() {
                let sign = if self.reading_direction == crate::settings::ReadingDirection::Rtl {
                    -1.0
                } else {
                    1.0
                };
                egui::pos2(
                    image_rect.center().x + sign * (offsets[list_pos] - self.fs_vertical_scroll),
                    image_rect.center().y + self.fs_pan.y,
                )
            } else {
                egui::pos2(
                    image_rect.center().x + self.fs_pan.x,
                    image_rect.center().y - self.fs_vertical_scroll + offsets[list_pos],
                )
            };
            let unit_rect =
                egui::Rect::from_center_size(unit_center, egui::vec2(size.width, size.height));
            if let Some(text) = units[list_pos].separator_text.as_ref() {
                separators.push(VerticalReadingSeparator {
                    text: text.clone(),
                    rect: unit_rect,
                });
                continue;
            }
            for (idx, rect) in continuous_reading_page_rects(unit_rect, size) {
                pages.push(VerticalReadingPage { idx, rect });
            }
        }

        Some((pages, separators, visible_positions, offsets))
    }

    fn update_continuous_reading_prefetch_window(
        &mut self,
        units: &[ContinuousReadingUnitSpec],
        visible_positions: &[usize],
    ) {
        if units.is_empty() {
            return;
        }
        let current_pos = self
            .fullscreen_idx
            .and_then(|idx| units.iter().position(|unit| unit.contains_idx(idx)))
            .unwrap_or_else(|| visible_positions.first().copied().unwrap_or(0))
            .min(units.len() - 1);
        let mut visible_positions = visible_positions
            .iter()
            .copied()
            .filter(|&pos| pos < units.len())
            .collect::<Vec<_>>();
        if visible_positions.is_empty() {
            visible_positions.push(current_pos);
        }
        visible_positions.sort_unstable();
        visible_positions.dedup();

        let first_visible = visible_positions
            .first()
            .copied()
            .unwrap_or(current_pos)
            .min(units.len() - 1);
        let last_visible = visible_positions
            .last()
            .copied()
            .unwrap_or(current_pos)
            .min(units.len() - 1);
        let keep_start = first_visible.saturating_sub(VERTICAL_READING_PREFETCH_PAD);
        let keep_end = (last_visible + VERTICAL_READING_PREFETCH_PAD).min(units.len() - 1);
        let center_pos = (first_visible + last_visible) as f32 * 0.5;
        let visible_set = visible_positions
            .iter()
            .copied()
            .collect::<std::collections::HashSet<_>>();

        let mut candidates = (keep_start..=keep_end).collect::<Vec<_>>();
        candidates.sort_by(|a, b| {
            let a_visible = visible_set.contains(a);
            let b_visible = visible_set.contains(b);
            b_visible.cmp(&a_visible).then_with(|| {
                (*a as f32 - center_pos)
                    .abs()
                    .partial_cmp(&(*b as f32 - center_pos).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });

        let mut keep_positions = Vec::new();
        let mut keep_page_count = 0usize;
        for pos in candidates {
            let page_count = units[pos].pages.len();
            if visible_set.contains(&pos)
                || keep_page_count + page_count <= VERTICAL_READING_MAX_CACHE_PAGES
            {
                keep_page_count += page_count;
                keep_positions.push(pos);
            }
        }
        keep_positions.sort_unstable();

        let mut keep_set = std::collections::HashSet::new();
        for &pos in &keep_positions {
            keep_set.extend(units[pos].pages.iter().copied());
        }
        if keep_set.is_empty() {
            keep_set.extend(units[current_pos].pages.iter().copied());
        }
        if let Some(current_idx) = self.fullscreen_idx {
            if units[current_pos].pages.contains(&current_idx) {
                keep_set.insert(current_idx);
            }
        }

        let mut loaded_texels: usize = keep_set
            .iter()
            .map(|&idx| self.vertical_reading_loaded_texels(idx))
            .sum();
        if loaded_texels > VERTICAL_READING_MAX_CACHE_TEXELS {
            let mut removable = keep_positions
                .iter()
                .copied()
                .filter(|pos| !visible_set.contains(pos))
                .collect::<Vec<_>>();
            removable.sort_by(|a, b| {
                (*b as f32 - center_pos)
                    .abs()
                    .partial_cmp(&(*a as f32 - center_pos).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            for pos in removable {
                for &idx in &units[pos].pages {
                    if keep_set.remove(&idx) {
                        loaded_texels =
                            loaded_texels.saturating_sub(self.vertical_reading_loaded_texels(idx));
                    }
                }
                if loaded_texels <= VERTICAL_READING_MAX_CACHE_TEXELS {
                    break;
                }
            }
        }

        let mut keep_positions_for_load = keep_positions
            .iter()
            .copied()
            .filter(|&pos| units[pos].pages.iter().any(|idx| keep_set.contains(idx)))
            .collect::<Vec<_>>();
        if keep_positions_for_load.is_empty() {
            keep_positions_for_load.push(current_pos);
        }
        keep_positions_for_load.sort_unstable();
        keep_positions_for_load.dedup();

        self.fs_vertical_cache_keep_set = keep_set.clone();
        self.evict_final_pipeline_cache_for_keep_set(&keep_set);
        self.evict_adjustment_cache_for_keep_set(&keep_set);

        self.save_all_video_resume_positions();
        self.fs_cache
            .retain(|k, v| keep_set.contains(k) || matches!(v, FsCacheEntry::Video { .. }));

        let current_idx = self.fullscreen_idx.unwrap_or(units[current_pos].anchor_idx);
        let current_loading =
            keep_set.contains(&current_idx) && !self.fs_cache.contains_key(&current_idx);
        let to_cancel = self
            .fs_pending
            .keys()
            .filter(|&&k| {
                if k == current_idx {
                    return false;
                }
                current_loading || !keep_set.contains(&k)
            })
            .copied()
            .collect::<Vec<_>>();
        for idx in to_cancel {
            if let Some((cancel, _, _)) = self.fs_pending.remove(&idx) {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            self.fs_early_dims.remove(&idx);
        }
        if current_loading {
            if !self.fs_pending.contains_key(&current_idx) {
                self.start_fs_load(current_idx);
            }
            return;
        }

        let mut targets = Vec::new();
        for pos in keep_positions_for_load {
            for &idx in &units[pos].pages {
                if keep_set.contains(&idx) {
                    targets.push((pos, idx));
                }
            }
        }
        targets.sort_by(|a, b| {
            let a_current = a.1 != current_idx;
            let b_current = b.1 != current_idx;
            a_current.cmp(&b_current).then_with(|| {
                (a.0 as f32 - current_pos as f32)
                    .abs()
                    .partial_cmp(&(b.0 as f32 - current_pos as f32).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });
        for (_, idx) in targets {
            if keep_set.contains(&idx)
                && !self.fs_cache.contains_key(&idx)
                && !self.fs_pending.contains_key(&idx)
            {
                self.start_fs_load(idx);
            }
        }
    }

    fn vertical_reading_processed_texture_cached(&self, idx: usize) -> bool {
        if self.comic_pages.contains(&idx) {
            self.current_comic_composite_texture(idx).is_some()
        } else {
            self.current_final_composite_texture(idx).is_some()
        }
    }

    fn vertical_reading_process_indices(
        &self,
        pages: &[VerticalReadingPage],
        current_idx: usize,
        image_rect: egui::Rect,
    ) -> std::collections::HashSet<usize> {
        let mut candidates = pages.iter().collect::<Vec<_>>();
        let horizontal = self.reading_flow.is_horizontal();
        let center = image_rect.center();
        candidates.sort_by(|a, b| {
            let a_current = a.idx != current_idx;
            let b_current = b.idx != current_idx;
            let a_dist = if horizontal {
                (a.rect.center().x - center.x).abs()
            } else {
                (a.rect.center().y - center.y).abs()
            };
            let b_dist = if horizontal {
                (b.rect.center().x - center.x).abs()
            } else {
                (b.rect.center().y - center.y).abs()
            };
            a_current.cmp(&b_current).then_with(|| {
                a_dist
                    .partial_cmp(&b_dist)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
        });

        let mut process = std::collections::HashSet::new();
        for page in candidates {
            if page.idx != current_idx && self.vertical_reading_processed_texture_cached(page.idx) {
                continue;
            }
            process.insert(page.idx);
            if process.len() >= VERTICAL_READING_PROCESSED_UPLOADS_PER_FRAME {
                break;
            }
        }
        process
    }

    fn draw_fs_continuous_reading(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        image_rect: egui::Rect,
        fs_idx: usize,
        original_preview_active: bool,
    ) {
        let Some((units, current_pos)) = self.continuous_reading_units_and_pos(fs_idx) else {
            self.fs_spread_layout = None;
            return;
        };
        let Some((pages, separators, visible_positions, offsets)) =
            self.continuous_reading_layout(ctx, image_rect, &units, current_pos)
        else {
            self.fs_spread_layout = None;
            return;
        };
        self.update_continuous_reading_prefetch_window(&units, &visible_positions);

        let bg_tex = if self.fs_transparent_bg_mode == 2 {
            self.ensure_checker_texture(ctx);
            self.fs_checker_texture.clone()
        } else {
            None
        };
        let bg_style = transparent_bg_style(self.fs_transparent_bg_mode, bg_tex.as_ref());
        let painter = ui.painter().with_clip_rect(image_rect);
        let holdover_for_locked = if self.fs_nav_is_locked() {
            self.fs_holdover_tex.clone()
        } else {
            None
        };

        let process_indices = if original_preview_active {
            std::collections::HashSet::new()
        } else {
            self.vertical_reading_process_indices(
                &pages,
                self.fullscreen_idx.unwrap_or(fs_idx),
                image_rect,
            )
        };
        let has_deferred_processed = !original_preview_active
            && pages.iter().any(|page| {
                !process_indices.contains(&page.idx)
                    && !self.vertical_reading_processed_texture_cached(page.idx)
            });

        let mut any_loading = false;
        for page in pages {
            self.advance_animation(ctx, page.idx);
            let rotation = self.get_rotation(page.idx);
            let location = self.location_display_for_loading(page.idx);
            let display_tex = if original_preview_active {
                self.resolve_original_preview_tex(page.idx)
                    .or_else(|| self.resolve_fs_display_tex(page.idx, true))
            } else if process_indices.contains(&page.idx) {
                // 連結読みでも単ページ/見開きと同じ final pipeline を使う。ただし
                // 新規 GPU upload は中央付近から 1 フレームずつ進め、スクロール中の
                // 大量同期生成を避ける。
                self.resolve_fs_processed_texture(ctx, page.idx, false)
            } else {
                self.resolve_fs_display_tex(page.idx, true)
            };
            if matches!(self.fs_cache.get(&page.idx), Some(FsCacheEntry::Failed)) {
                painter.text(
                    page.rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "読込失敗",
                    egui::FontId::proportional(18.0),
                    egui::Color32::from_rgb(255, 140, 140),
                );
                continue;
            }
            if !self.fs_cache.contains_key(&page.idx) {
                any_loading = true;
            }
            Self::draw_fs_spread_page(
                &painter,
                page.rect,
                page.idx,
                rotation,
                &self.thumbnails,
                &bg_style,
                &location,
                holdover_for_locked.as_ref(),
                display_tex.as_ref(),
            );
        }
        for separator in separators {
            Self::draw_fs_continuous_separator(
                &painter,
                separator.rect,
                &separator.text,
                self.reading_flow,
            );
        }

        if let Some(new_pos) = vertical_reading_nearest_position(&offsets, self.fs_vertical_scroll)
            && new_pos != current_pos
            && let Some(unit) = units.get(new_pos)
        {
            let new_idx = unit.anchor_idx;
            self.fs_vertical_scroll =
                vertical_reading_reanchor_scroll(self.fs_vertical_scroll, &offsets, new_pos);
            self.fullscreen_idx = Some(new_idx);
            self.selected = Some(new_idx);
            self.scroll_to_selected = true;
            self.update_last_selected_image();
            self.record_book_resume(new_idx);
            ctx.request_repaint();
        }

        self.fs_spread_layout = None;
        if any_loading || has_deferred_processed {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_fs_pixel_grid(
        painter: &egui::Painter,
        full_rect: egui::Rect,
        img_rect: egui::Rect,
        tex_size: egui::Vec2,
        rotation: crate::rotation_db::Rotation,
        free_rotation_rad: f32,
        center: egui::Pos2,
        image_px_to_point: f32,
        pixels_per_point: f32,
    ) {
        if image_px_to_point * pixels_per_point < PIXEL_GRID_MIN_CELL_PHYSICAL_PX {
            return;
        }
        let tex_w = tex_size.x.round().max(1.0) as usize;
        let tex_h = tex_size.y.round().max(1.0) as usize;
        if tex_w == 0 || tex_h == 0 {
            return;
        }

        let uvs = match rotation {
            crate::rotation_db::Rotation::None => [
                egui::pos2(0.0, 0.0),
                egui::pos2(1.0, 0.0),
                egui::pos2(1.0, 1.0),
                egui::pos2(0.0, 1.0),
            ],
            crate::rotation_db::Rotation::Cw90 => [
                egui::pos2(0.0, 1.0),
                egui::pos2(0.0, 0.0),
                egui::pos2(1.0, 0.0),
                egui::pos2(1.0, 1.0),
            ],
            crate::rotation_db::Rotation::Cw180 => [
                egui::pos2(1.0, 1.0),
                egui::pos2(0.0, 1.0),
                egui::pos2(0.0, 0.0),
                egui::pos2(1.0, 0.0),
            ],
            crate::rotation_db::Rotation::Cw270 => [
                egui::pos2(1.0, 0.0),
                egui::pos2(1.0, 1.0),
                egui::pos2(0.0, 1.0),
                egui::pos2(0.0, 0.0),
            ],
        };
        let mut positions = [
            img_rect.left_top(),
            img_rect.right_top(),
            img_rect.right_bottom(),
            img_rect.left_bottom(),
        ];
        if free_rotation_rad.abs() > TRANSFORM_EPSILON {
            let (sin_r, cos_r) = free_rotation_rad.sin_cos();
            for p in &mut positions {
                let dx = p.x - center.x;
                let dy = p.y - center.y;
                p.x = center.x + dx * cos_r - dy * sin_r;
                p.y = center.y + dx * sin_r + dy * cos_r;
            }
        }

        let pos_for_uv = |u: f32, v: f32| -> egui::Pos2 {
            for (idx, uv) in uvs.iter().enumerate() {
                if (uv.x - u).abs() < f32::EPSILON && (uv.y - v).abs() < f32::EPSILON {
                    return positions[idx];
                }
            }
            positions[0]
        };
        let p00 = pos_for_uv(0.0, 0.0);
        let p10 = pos_for_uv(1.0, 0.0);
        let p01 = pos_for_uv(0.0, 1.0);
        let axis_u = p10 - p00;
        let axis_v = p01 - p00;
        let det = axis_u.x * axis_v.y - axis_u.y * axis_v.x;
        if det.abs() < 1e-5 {
            return;
        }
        let screen_from_uv = |u: f32, v: f32| -> egui::Pos2 { p00 + axis_u * u + axis_v * v };
        let uv_from_screen = |p: egui::Pos2| -> egui::Vec2 {
            let d = p - p00;
            egui::vec2(
                (d.x * axis_v.y - d.y * axis_v.x) / det,
                (axis_u.x * d.y - axis_u.y * d.x) / det,
            )
        };

        let corners = [
            full_rect.left_top(),
            full_rect.right_top(),
            full_rect.right_bottom(),
            full_rect.left_bottom(),
        ];
        let mut min_u = f32::INFINITY;
        let mut max_u = f32::NEG_INFINITY;
        let mut min_v = f32::INFINITY;
        let mut max_v = f32::NEG_INFINITY;
        for corner in corners {
            let uv = uv_from_screen(corner);
            min_u = min_u.min(uv.x);
            max_u = max_u.max(uv.x);
            min_v = min_v.min(uv.y);
            max_v = max_v.max(uv.y);
        }
        min_u = min_u.clamp(0.0, 1.0);
        max_u = max_u.clamp(0.0, 1.0);
        min_v = min_v.clamp(0.0, 1.0);
        max_v = max_v.clamp(0.0, 1.0);
        if min_u > max_u || min_v > max_v {
            return;
        }

        let x0 = ((min_u * tex_w as f32).floor() as isize - 1).max(0) as usize;
        let x1 = ((max_u * tex_w as f32).ceil() as usize + 1).min(tex_w);
        let y0 = ((min_v * tex_h as f32).floor() as isize - 1).max(0) as usize;
        let y1 = ((max_v * tex_h as f32).ceil() as usize + 1).min(tex_h);
        let line_count = x1.saturating_sub(x0) + 1 + y1.saturating_sub(y0) + 1;
        if line_count > PIXEL_GRID_MAX_LINES {
            return;
        }

        let stroke_width = (1.0 / pixels_per_point.max(1.0)).clamp(0.5, 1.0);
        let line = egui::Stroke::new(
            stroke_width,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 120),
        );
        for x in x0..=x1 {
            let u = x as f32 / tex_w as f32;
            let a = screen_from_uv(u, min_v);
            let b = screen_from_uv(u, max_v);
            painter.line_segment([a, b], line);
        }
        for y in y0..=y1 {
            let v = y as f32 / tex_h as f32;
            let a = screen_from_uv(min_u, v);
            let b = screen_from_uv(max_u, v);
            painter.line_segment([a, b], line);
        }
    }

    fn ensure_compare_pinned_texture(
        &mut self,
        ctx: &egui::Context,
    ) -> Option<egui::TextureHandle> {
        let slot = self.pinned_compare_slot.as_mut()?;
        if slot.texture.is_none() {
            let tex = ctx.load_texture(
                format!(
                    "compare_pin_{}_{}x{}",
                    slot.source_idx, slot.source_size[0], slot.source_size[1]
                ),
                slot.pixels.as_ref().clone(),
                egui::TextureOptions::LINEAR,
            );
            slot.texture = Some(tex);
        }
        slot.texture.clone()
    }

    fn compare_prepared_pair_matches(&self, fs_idx: usize) -> bool {
        let Some(slot) = self.pinned_compare_slot.as_ref() else {
            return false;
        };
        self.compare_prepared_pair.as_ref().is_some_and(|pair| {
            pair.current_idx == fs_idx && pair.pinned_source_idx == slot.source_idx
        })
    }

    fn ensure_compare_prepared_pair(&mut self, ctx: &egui::Context, fs_idx: usize) -> bool {
        let Some(slot) = self.pinned_compare_slot.as_ref() else {
            return false;
        };
        if self.compare_prepared_pair_matches(fs_idx) {
            return true;
        }
        if self
            .compare_prepare_pending
            .as_ref()
            .is_some_and(|pending| {
                pending.current_idx == fs_idx && pending.pinned_source_idx == slot.source_idx
            })
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return false;
        }

        let pinned_source_idx = slot.source_idx;
        let pinned_size = slot.source_size;
        let pinned_pixels = Arc::clone(&slot.pixels);
        let current_work = match self.prepare_capture_pixel_work(ctx, fs_idx) {
            Ok(work) => work,
            Err(err) => {
                self.compare_view_mode = crate::app::CompareViewMode::Off;
                self.compare_wipe_dragging = false;
                self.show_feedback_toast(err);
                return false;
            }
        };
        self.compare_prepared_pair = None;
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("compare-prepare".into())
            .spawn(move || {
                let result = crate::capture::run_pixel_work(current_work).and_then(
                    |(_basename, width, height, current_rgba)| {
                        let pinned_source_rgba =
                            crate::capture::color_image_to_rgba(pinned_pixels.as_ref());
                        let pinned_rgba = crate::capture::align_rgba_to_canvas_lanczos(
                            pinned_size[0] as u32,
                            pinned_size[1] as u32,
                            &pinned_source_rgba,
                            width,
                            height,
                        )?;
                        let diff_rgba = crate::capture::diff_rgba_color(
                            width,
                            height,
                            &pinned_rgba,
                            &current_rgba,
                        )?;
                        Ok(crate::app::ComparePrepareResult {
                            current_idx: fs_idx,
                            pinned_source_idx,
                            width,
                            height,
                            pinned_rgba,
                            current_rgba,
                            diff_rgba,
                        })
                    },
                );
                let _ = tx.send(result);
            });

        match thread {
            Ok(_) => {
                self.compare_prepare_pending = Some(crate::app::ComparePreparePending {
                    current_idx: fs_idx,
                    pinned_source_idx,
                    rx,
                });
                self.show_feedback_toast("比較表示を準備中".to_string());
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(err) => {
                self.show_feedback_toast(format!("比較 worker を開始できません: {err}"));
            }
        }
        false
    }

    fn ensure_compare_prepared_texture(
        &mut self,
        ctx: &egui::Context,
        kind: ComparePreparedTextureKind,
    ) -> Option<egui::TextureHandle> {
        let pair = self.compare_prepared_pair.as_mut()?;
        let key = pair.key;
        let target_size = pair.target_size;
        let (slot, pixels, label) = match kind {
            ComparePreparedTextureKind::Pinned => (
                &mut pair.pinned_texture,
                Arc::clone(&pair.pinned_pixels),
                "pinned",
            ),
            ComparePreparedTextureKind::Current => (
                &mut pair.current_texture,
                Arc::clone(&pair.current_pixels),
                "current",
            ),
            ComparePreparedTextureKind::Diff => (
                &mut pair.diff_texture,
                Arc::clone(&pair.diff_pixels),
                "diff",
            ),
        };
        if slot.is_none() {
            let tex = ctx.load_texture(
                format!(
                    "compare_prepared_{}_{}_{}x{}",
                    label, key, target_size[0], target_size[1]
                ),
                pixels.as_ref().clone(),
                egui::TextureOptions::LINEAR,
            );
            *slot = Some(tex);
        }
        slot.clone()
    }

    fn compare_image_draw_rect(
        full_rect: egui::Rect,
        image_size: [usize; 2],
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> Option<egui::Rect> {
        let tex_size = egui::vec2(image_size[0] as f32, image_size[1] as f32);
        if tex_size.x <= 0.0
            || tex_size.y <= 0.0
            || full_rect.width() <= 0.0
            || full_rect.height() <= 0.0
        {
            return None;
        }
        let fit_scale = (full_rect.width() / tex_size.x).min(full_rect.height() / tex_size.y);
        let (total_scale, center) = match zoom_pan {
            Some((zoom, pan)) => (fit_scale * zoom, full_rect.center() + pan),
            None => (fit_scale, full_rect.center()),
        };
        Some(egui::Rect::from_center_size(center, tex_size * total_scale))
    }

    #[cfg(windows)]
    fn compare_shader_shape(
        &self,
        image_rect: egui::Rect,
        pair: &crate::app::ComparePreparedPair,
        mode: crate::compare_wgpu::CompareShaderMode,
        wipe_fraction: f32,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> Option<(egui::Rect, egui::Shape)> {
        let target_format = self.wgpu_render_state.as_ref()?.target_format;
        let draw_rect = Self::compare_image_draw_rect(image_rect, pair.target_size, zoom_pan)?;
        let callback = crate::compare_wgpu::CompareShaderCallback {
            key: pair.key,
            width: pair.target_size[0] as u32,
            height: pair.target_size[1] as u32,
            pinned_rgba: Arc::clone(&pair.pinned_rgba),
            current_rgba: Arc::clone(&pair.current_rgba),
            mode,
            wipe_fraction,
            target_format,
        };
        Some((
            draw_rect,
            egui::Shape::Callback(egui_wgpu::Callback::new_paint_callback(draw_rect, callback)),
        ))
    }

    #[cfg(not(windows))]
    fn compare_shader_shape(
        &self,
        _image_rect: egui::Rect,
        _pair: &crate::app::ComparePreparedPair,
        _mode: crate::compare_wgpu::CompareShaderMode,
        _wipe_fraction: f32,
        _zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> Option<(egui::Rect, egui::Shape)> {
        None
    }

    /// 360 度パノラマビューがアクティブで左ドラッグ / ダブルクリックを処理した場合 true。
    /// 通常モードの click/drag 処理 (zoom/pan/rotation) はこの呼び出しが true を返したら
    /// スキップする (docs/panorama-360-view-plan.md §5.2)。
    ///
    /// - 左ドラッグ → yaw/pitch (sens = fov_y / viewport_h)
    /// - ダブルクリック → reset (GPano hint or 0)
    /// - **通常 Wheel / 矢印 / Esc は奪わない** (= 既存ナビ動作維持)
    fn handle_panorama_drag_if_active(
        &mut self,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        fs_response: &egui::Response,
    ) -> bool {
        let Some(fs_idx) = self.fullscreen_idx else {
            return false;
        };
        if !self.is_panorama_mode_active(fs_idx) {
            return false;
        }
        let viewport_h = full_rect.height().max(1.0);
        if let Some(pano) = self.panorama_state.as_mut() {
            // 左ドラッグ → yaw/pitch (「掴んで引っ張る」感覚、Google Street View 流)。
            // 画像がドラッグ方向と同じ向きに動くようにする。
            //
            // WGSL の射影 (panorama_wgpu.rs §3.3) を踏まえた符号:
            // - 右ドラッグ (dx>0) → yaw 増 → 視点は左を向く → 画像が右に流れる ✓
            // - 下ドラッグ (dy>0) → pitch 増 → 視点は上を向く → 画像が下に流れる ✓
            //   (pitch=π/2 で空を直視、pitch=-π/2 で床を直視、WGSL の `cam_dir.z=-1` 規約)
            //
            // ⚠ `Response::drag_delta()` は egui 0.33 で **フレーム間デルタ**
            //    (= "since last frame", egui::Response::drag_delta 行コメント参照)。
            //    **累積差分ではない** ので毎フレ加算で OK。累積版が必要なら
            //    `total_drag_delta()`。Codex P1 第 18 ラウンドで「累積」と誤指摘あり、
            //    egui ソース (src/response.rs 該当行) で fetch 確認済み。
            let primary_down = fs_response.dragged_by(egui::PointerButton::Primary);
            if primary_down {
                let delta = fs_response.drag_delta();
                if delta.length_sq() > 0.0 {
                    let sens = pano.fov_y / viewport_h;
                    pano.yaw += delta.x * sens;
                    // yaw を [-π, π] に wrap
                    let two_pi = std::f32::consts::TAU;
                    while pano.yaw > std::f32::consts::PI {
                        pano.yaw -= two_pi;
                    }
                    while pano.yaw < -std::f32::consts::PI {
                        pano.yaw += two_pi;
                    }
                    // pitch クランプ (極を直視させない)
                    pano.pitch = (pano.pitch + delta.y * sens)
                        .clamp(-crate::panorama::PITCH_LIMIT, crate::panorama::PITCH_LIMIT);
                    // pose が NaN / Inf に化けないことを保証 (Codex P2 第 5、2026-05)
                    pano.sanitize();
                    ctx.request_repaint();
                }
            }
            // ダブルクリック → 初期視点に戻す (GPano hint or 0)
            if fs_response.double_clicked() {
                pano.reset();
                ctx.request_repaint();
            }
        }
        true
    }

    /// 360 度パノラマビューがアクティブで Wheel を処理した場合 true。
    ///
    /// 2026-05 ユーザー要望反映: 360 モード中は **Ctrl 有無に関わらず Wheel を FOV
    /// 操作に転用**する (= 拡大縮小のつもりで画像送りを誤発火する事故を回避)。
    /// 前後ナビは矢印 / BS キーで行う。
    ///
    /// 元設計 (Ctrl+Wheel のみ FOV、通常 Wheel は前後送り維持) は実機 UX で問題が
    /// 出たため取り下げた。`_ctrl_held` 引数は呼び出し側のシグネチャ互換のため残置。
    fn handle_panorama_wheel_if_active(
        &mut self,
        ctx: &egui::Context,
        wheel_y: f32,
        _ctrl_held: bool,
    ) -> bool {
        // 2026-05 ユーザー要望: 360 モード中は **ホイール (修飾なし) も FOV 操作に転用**。
        // 元設計 (通常 Wheel は前後送り維持、Ctrl+Wheel のみ FOV) はユーザーが
        // 拡大縮小のつもりでホイールを回して画像が切り替わる事故が多発したため変更。
        // Ctrl 有無に関わらず、360 ON 時は同じ操作を入れる。
        if wheel_y.abs() < 0.5 {
            return false;
        }
        let Some(fs_idx) = self.fullscreen_idx else {
            return false;
        };
        if !self.is_panorama_mode_active(fs_idx) {
            return false;
        }
        if let Some(pano) = self.panorama_state.as_mut() {
            // FOV = fov * exp(-wheel * 0.0015)。ホイール 1 ノッチ ≈ 50px で約 7% 変化。
            let factor = (-wheel_y * 0.0015).exp();
            pano.fov_y =
                (pano.fov_y * factor).clamp(crate::panorama::FOV_MIN, crate::panorama::FOV_MAX);
            // pose が NaN / Inf に化けないことを保証 (Codex P2 第 5、2026-05)
            pano.sanitize();
            ctx.request_repaint();
        }
        true
    }

    /// 360 度パノラマビューを emit する (docs/panorama-360-view-plan.md §4.2)。
    /// `pano_uploaded` が `(source_key, cache_key)` 一致でアップロード済みのときだけ
    /// `Shape::Callback` を出して true を返す。準備中 (未アップロード or stale) なら
    /// false を返し、呼び出し側は通常パス (`draw_fs_image`) にフォールバックして
    /// 平面で equirect を表示する (= 「数フレ平らな equirect → 360 描画開始」UX)。
    ///
    /// 360 自身が yaw/pitch/fov を持つので、rotation_db / zoom / pan / free_rotation /
    /// spread は完全に無視する (呼び出し側で関連ブロックをスキップする責任あり)。
    #[cfg(windows)]
    fn try_paint_panorama(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        image_rect: egui::Rect,
        fs_idx: usize,
    ) -> bool {
        // 1. アップロード経路を起動 (stale なら今フレで同期 upload)。
        //    実測で 40-110 ms かかり得るが、Phase 1 では UI スレッド許容 (§4.1.1)。
        self.ensure_pano_upload(ctx, fs_idx);
        // 2. 今フレの CallbackResources に Arc を載せる (毎フレ必須)。
        self.sync_pano_callback_resources();

        let Some(render_state) = self.wgpu_render_state.as_ref() else {
            return false;
        };
        let target_format = render_state.target_format;
        let Some(resolution) = self.resolve_pano_source(ctx, fs_idx) else {
            return false;
        };
        let uploaded_ready = self
            .pano_uploaded
            .as_ref()
            .map(|u| u.source_key == resolution.source_key && u.cache_key == resolution.cache_key)
            .unwrap_or(false);
        if !uploaded_ready {
            return false;
        }
        let Some(pano) = self.panorama_state.as_ref() else {
            return false;
        };
        let aspect = if image_rect.height() > 0.0 {
            image_rect.width() / image_rect.height()
        } else {
            1.0
        };
        // Phase 1.5 部分 FOV equirect: GPano `CroppedArea*` 宣言から UV 変換を計算。
        // フル equirect / 宣言なしの画像は IDENTITY (= 恒等変換) になる。
        let uv_transform = self.compute_pano_uv_transform(fs_idx);
        // `resolution.source_key` を callback に渡すが、後段の status indicator でも
        // 参照するので clone する。
        let callback = crate::panorama_wgpu::PanoramaShaderCallback {
            source_key: resolution.source_key.clone(),
            cache_key: resolution.cache_key,
            yaw: pano.yaw,
            pitch: pano.pitch,
            fov_y: pano.fov_y,
            aspect,
            uv_transform,
            target_format,
        };
        let shape = egui::Shape::Callback(egui_wgpu::Callback::new_paint_callback(
            image_rect, callback,
        ));
        ui.painter().add(shape);

        // Phase 2a settle overlay (docs/panorama-360-view-plan.md §4.6.3 step 6/7)
        // 同フレ後段で 8K base の上にフェードイン alpha blend で描画。
        let pose = (pano.yaw, pano.pitch, pano.fov_y);
        let now_cache_key = resolution.cache_key;
        // 現フレの viewport ピクセルサイズ。settle render の出力 aspect 計算と stale check
        // に使う (Codex P1 第 2、2026-05)。
        let viewport_size: (u32, u32) = (
            image_rect.width().round().max(1.0) as u32,
            image_rect.height().round().max(1.0) as u32,
        );
        // **`last_viewport_size` の直接代入ではなく `note_state` 経由**で更新する
        // (Codex P3 第 3 ラウンド、2026-05): viewport が変わったときに進行中の settle
        // render を cancel + `settle_since` reset する。直接代入だと差分検出が走らず、
        // 古い viewport の overlay が完成しても描画時の stale guard で捨てられるだけで、
        // settle 再起動が次フレ以降にずれる。
        if let Some(refinement) = self.pano_refinement.as_mut() {
            refinement.note_state(pose, now_cache_key, Some(viewport_size));
        }
        // **当フレ最新の viewport を反映してから settle spawn 判定**を行う
        // (Codex P1 第 5 ラウンド、2026-05): App::update で動かしていた頃は前フレの
        // viewport で spawn → 直後 note_state でキャンセル、という無駄サイクルが入っていた。
        // 本メソッドは paint 中なので最新値で判定できる。
        self.try_spawn_settle_render(ctx, fs_idx, &resolution);
        let overlay_payload = self.pano_refinement.as_ref().and_then(|r| {
            if r.overlay_ok_for(
                pose,
                now_cache_key,
                Some(viewport_size),
                Some(target_format),
            ) {
                let alpha = match r.overlay_fade_start {
                    Some(start) => {
                        let elapsed = start.elapsed().as_secs_f32();
                        (elapsed / 0.15).clamp(0.0, 1.0)
                    }
                    None => 1.0,
                };
                let overlay = r.overlay.as_ref()?;
                let gpu = std::sync::Arc::clone(&overlay.gpu);
                Some((alpha, gpu, r.source_key.clone()))
            } else {
                None
            }
        });
        let overlay_drawn = overlay_payload.is_some();
        let overlay_alpha = overlay_payload.as_ref().map(|(a, _, _)| *a).unwrap_or(0.0);
        if let Some((alpha, gpu, source_key)) = overlay_payload {
            let overlay_callback = crate::panorama_wgpu::SettleOverlayCallback {
                source_key,
                cache_key: now_cache_key,
                pose,
                alpha,
                target_format,
                gpu,
            };
            ui.painter().add(egui::Shape::Callback(
                egui_wgpu::Callback::new_paint_callback(image_rect, overlay_callback),
            ));
            // フェードイン中は repaint をリクエスト (alpha 1.0 になるまで毎フレ)
            if alpha < 1.0 {
                ctx.request_repaint();
            }
        }
        // 高画質モードの status インジケータ (下部中央)。
        // ユーザーが「いま settle が動いているか」を視覚的に判定できるように。
        self.draw_pano_status_indicator(
            ui,
            ctx,
            image_rect,
            fs_idx,
            &resolution,
            overlay_drawn,
            overlay_alpha,
        );
        true
    }

    /// 360 度パノラマビュー Phase 2a: 高画質モードの状態を画面下部中央に小さく表示する。
    /// settle が動いているか / 待機中か / OFF か をユーザーが目で確認できるようにする。
    ///
    /// 加えて、現在の state に応じてモード切替ボタンを 1 つだけバッジ右側に出す
    /// (CLAUDE 議論 2026-05):
    /// - `BaseOnly`: `[高画質に切替]` ボタン (= SettleApproved 化 + worker spawn)
    /// - `SettleReady` / `SettleApproved` (= settle 経路 active): `[8K 軽量に切替]`
    ///   ボタン (= BaseOnly 化 + worker cancel + HighResSource drop)
    /// - `NeedsUserConfirmation` (= バナー表示中) / `policy_enabled=false`
    ///   (= AI / 補正中で settle 不能) / state 未設定: ボタンなし
    ///
    /// 「高画質に切替」を押しても `pano_session_approved_max_pixels` は **bump しない**。
    /// = この 1 枚だけ高画質化、次の新画像 (> 200 MP) は再びバナーで確認する。
    /// session-wide 承認はバナーのチェックボックスに限定するという設計判断。
    #[cfg(windows)]
    fn draw_pano_status_indicator(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        image_rect: egui::Rect,
        fs_idx: usize,
        resolution: &crate::panorama::PanoSourceResolution,
        overlay_drawn: bool,
        overlay_alpha: f32,
    ) {
        use crate::panorama::PanoramaQualityState;
        let high_res = self.pano_high_res_source.get(&resolution.source_key);
        let high_res_loaded = high_res.is_some();
        let policy_enabled = resolution.settle_policy.is_enabled();
        let state = self.pano_quality_state.get(&resolution.source_key).cloned();
        let state_allows_settle = matches!(
            state,
            Some(PanoramaQualityState::SettleReady) | Some(PanoramaQualityState::SettleApproved)
        );
        let rendering = self
            .pano_refinement
            .as_ref()
            .map(|r| r.rendering.is_some())
            .unwrap_or(false);
        // 源解像度 (フル RGBA があるならその dim、無ければ fs_cache source_dims)
        let src_dims: Option<(u32, u32)> = if let Some(hr) = high_res {
            Some(hr.dims())
        } else if let Some(crate::fs_animation::FsCacheEntry::Static {
            source_dims: Some([w, h]),
            ..
        }) = self.fs_cache.get(&fs_idx)
        {
            Some((*w as u32, *h as u32))
        } else {
            None
        };

        // ステータステキストと色を決定。
        // 色: 緑=ON / 黄=描画中 or フェードイン / 灰=待機 or OFF
        let green = egui::Color32::from_rgb(120, 220, 130);
        let yellow = egui::Color32::from_rgb(255, 210, 110);
        let gray = egui::Color32::from_rgb(180, 180, 180);
        let (mark, label, color) = if overlay_drawn {
            if overlay_alpha >= 0.999 {
                ("●", "高画質 ON".to_string(), green)
            } else {
                (
                    "●",
                    format!("高画質 適用中… {:>3.0}%", overlay_alpha * 100.0),
                    yellow,
                )
            }
        } else if rendering {
            ("●", "高画質 描画中…".to_string(), yellow)
        } else if state_allows_settle && policy_enabled && high_res_loaded {
            ("○", "高画質 待機中 (静止 500ms で発動)".to_string(), gray)
        } else if state_allows_settle && policy_enabled && !high_res_loaded {
            ("●", "高画質 ロード中…".to_string(), yellow)
        } else if matches!(state, Some(PanoramaQualityState::BaseOnly)) {
            ("○", "最大 8K 表示 (軽量)".to_string(), gray)
        } else if matches!(
            state,
            Some(PanoramaQualityState::NeedsUserConfirmation { .. })
        ) {
            (
                "⚠",
                "高画質モード 確認待ち (バナー参照)".to_string(),
                yellow,
            )
        } else if !policy_enabled {
            // settle_policy が Disabled になっている主な理由を推定
            // (AI 機能 / post_filter / auto_mode / transient)
            let reason = self.pano_settle_disabled_reason(fs_idx, resolution.source_kind);
            ("○", format!("高画質 OFF ({reason})"), gray)
        } else {
            ("○", "8K 表示".to_string(), gray)
        };

        // 源解像度補足 (例: " (11968×5984)")
        let dims_str = match src_dims {
            Some((w, h)) => format!(" ({w}×{h})"),
            None => String::new(),
        };
        let full_text = format!("{mark} {label}{dims_str}");

        // 切替ボタン情報を決定 (action / 表示文言 / ホバー)
        #[derive(Clone, Copy)]
        enum ToggleAction {
            ToHighQuality,
            ToBaseOnly,
        }
        // BaseOnly での [高画質に切替] は以下の AND 条件を満たすときだけ表示する
        // (Codex P1/P2 第 4 ラウンド、2026-05):
        //  - `is_plain_image` (= `GridItem::Image`)。ZIP/PDF/Video/Folder は
        //    `start_pano_high_res_load` が即 return するので、押下しても worker が
        //    spawn されず "高画質 ロード中…" で永久 stall する。
        //    `maybe_update_pano_quality_state_from_static` でも非通常画像は
        //    BaseOnly 固定にされている。
        //  - `!high_res_failed` (= 前回 decode 失敗履歴がない)。`start_pano_high_res_load`
        //    が failed エントリで即 return するため、同じ stall を引き起こす。
        //  - `policy_enabled` (= AI / post_filter / auto_mode で settle_policy が
        //    Disabled でない)。Disabled 下で SettleApproved に倒しても settle は動かず、
        //    巨大 RGBA だけ確保される無駄を避ける。設定を解除すれば次フレで
        //    自動的にボタンが現れる。
        let is_plain_image = matches!(self.items.get(fs_idx), Some(GridItem::Image(_)));
        let high_res_failed = self.pano_high_res_failed.contains(&resolution.source_key);
        let can_switch_to_hq = is_plain_image && !high_res_failed && policy_enabled;
        let toggle_info: Option<(ToggleAction, &'static str, String)> = if matches!(
            state,
            Some(PanoramaQualityState::BaseOnly)
        ) && can_switch_to_hq
        {
            // BaseOnly → SettleApproved。ホバーに RAM 想定量を出す。
            let hover = match src_dims {
                Some((w, h)) => {
                    let est_gb = (w as f64) * (h as f64) * 4.0 * 2.0 / 1.0e9;
                    format!(
                        "高画質モードに切り替えます。\nフル RGBA をメモリに保持するため約 {:.1} GB の RAM を使います。",
                        est_gb
                    )
                }
                None => {
                    "高画質モードに切り替えます。\nフル RGBA をメモリに保持します。".to_string()
                }
            };
            Some((ToggleAction::ToHighQuality, "高画質に切替", hover))
        } else if state_allows_settle && policy_enabled {
            // SettleReady/SettleApproved → BaseOnly。
            // ホバーに「解放される RAM の見込み」を出す (= 現在保持中の HighResSource
            // dims、無ければ src_dims から推定)。
            let hover_dims = high_res.map(|hr| hr.dims()).or(src_dims);
            let hover = match hover_dims {
                Some((w, h)) => {
                    let est_gb = (w as f64) * (h as f64) * 4.0 * 2.0 / 1.0e9;
                    format!(
                        "8K 表示に切り替えます (高画質モード OFF)。\nフル RGBA メモリ約 {:.1} GB を解放します。",
                        est_gb
                    )
                }
                None => {
                    "8K 表示に切り替えます (高画質モード OFF)。\nフル RGBA メモリを解放します。"
                        .to_string()
                }
            };
            Some((ToggleAction::ToBaseOnly, "8K 軽量に切替", hover))
        } else {
            None
        };

        // バッジ矩形 (下部中央、半透明背景)。
        // テキスト幅を `Galley` から測り、ボタンぶんの幅も足してから pill_rect を決める。
        let font_id = egui::FontId::proportional(13.0);
        let galley = ui
            .painter()
            .layout_no_wrap(full_text.clone(), font_id.clone(), color);
        let text_size = galley.size();
        let padding = egui::vec2(12.0, 6.0);
        let button_width = 130.0_f32;
        let button_height = 24.0_f32;
        let button_gap = 10.0_f32;
        let extra_w = if toggle_info.is_some() {
            button_gap + button_width
        } else {
            0.0
        };
        let pill_size = egui::vec2(
            text_size.x + extra_w + padding.x * 2.0,
            text_size.y.max(button_height) + padding.y * 2.0,
        );
        let pill_center = egui::pos2(image_rect.center().x, image_rect.bottom() - 28.0);
        let pill_rect = egui::Rect::from_center_size(pill_center, pill_size);
        ui.painter().rect_filled(
            pill_rect,
            egui::CornerRadius::same(6),
            egui::Color32::from_black_alpha(180),
        );
        ui.painter().rect_stroke(
            pill_rect,
            egui::CornerRadius::same(6),
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(40)),
            egui::epaint::StrokeKind::Outside,
        );
        // テキストは左寄せ (button があれば右余白にボタンを置くため)
        let text_pos = egui::pos2(
            pill_rect.left() + padding.x,
            pill_rect.center().y - text_size.y * 0.5,
        );
        ui.painter().galley(text_pos, galley, color);

        // 切替ボタン (該当 state のみ)
        if let Some((action, btn_label, hover)) = toggle_info {
            let btn_rect = egui::Rect::from_min_size(
                egui::pos2(
                    pill_rect.left() + padding.x + text_size.x + button_gap,
                    pill_rect.center().y - button_height * 0.5,
                ),
                egui::vec2(button_width, button_height),
            );
            let mut clicked = false;
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(btn_rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |child_ui| {
                    let resp = child_ui
                        .add_sized(
                            btn_rect.size(),
                            egui::Button::new(egui::RichText::new(btn_label).size(12.0)),
                        )
                        .on_hover_text(hover);
                    if resp.clicked() {
                        clicked = true;
                    }
                },
            );
            if clicked {
                match action {
                    ToggleAction::ToHighQuality => {
                        // BaseOnly → SettleApproved。high-res worker を kick。
                        // **`pano_session_approved_max_pixels` は bump しない**。
                        // session-wide 承認はバナーのチェックボックスに限定するため。
                        let source_key = resolution.source_key.clone();
                        let cache_key = resolution.cache_key;
                        self.pano_quality_state
                            .insert(source_key, PanoramaQualityState::SettleApproved);
                        self.start_pano_high_res_load(fs_idx, cache_key);
                        ctx.request_repaint();
                    }
                    ToggleAction::ToBaseOnly => {
                        // SettleApproved/SettleReady → BaseOnly。worker を全 cancel し、
                        // ロード済み HighResSource を drop (= フル RGBA 1-2 GB を即解放)。
                        // 進行中の settle render も cancel + callback_resources 除去。
                        let source_key = resolution.source_key.clone();
                        self.pano_quality_state
                            .insert(source_key.clone(), PanoramaQualityState::BaseOnly);
                        for (_, req) in self.pano_high_res_pending.drain() {
                            req.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                        }
                        self.pano_high_res_source.remove(&source_key);
                        self.clear_pano_refinement();
                        ctx.request_repaint();
                    }
                }
            }
        }
    }

    #[cfg(not(windows))]
    fn draw_pano_status_indicator(
        &mut self,
        _ui: &mut egui::Ui,
        _ctx: &egui::Context,
        _image_rect: egui::Rect,
        _fs_idx: usize,
        _resolution: &crate::panorama::PanoSourceResolution,
        _overlay_drawn: bool,
        _overlay_alpha: f32,
    ) {
    }

    /// `compute_settle_policy` が Disabled を返すときの主因を 1 単語で返す
    /// (status インジケータ表示用)。
    fn pano_settle_disabled_reason(&self, fs_idx: usize, source_kind: u16) -> &'static str {
        // AI が **実効的に** この画像に適用される場合のみ "AI 機能 ON" 扱い。
        // 設定 ON でもサイズ閾値超でスキップされる画像は AI 由来ではないので、
        // 後段の post_filter / auto_mode / 補正 transient 等の理由を先に出す。
        if self.ai_will_apply_to(fs_idx) {
            return "AI 機能 ON";
        }
        if source_kind == crate::panorama::SOURCE_KIND_AI
            || source_kind == crate::panorama::SOURCE_KIND_AI_ADJUST
        {
            return "AI 由来 cache";
        }
        let params = self.effective_params(fs_idx);
        if params.auto_mode.is_some() {
            return "自動補正 ON";
        }
        if params.post_filter != crate::adjustment::PostFilter::None {
            return "ポストフィルタ ON";
        }
        if params.smart_sharpen != 0 {
            return "シャープ化 ON";
        }
        if source_kind == crate::panorama::SOURCE_KIND_FS && !params.is_color_identity() {
            return "補正適用待ち";
        }
        "不明"
    }

    #[cfg(not(windows))]
    fn try_paint_panorama(
        &mut self,
        _ui: &mut egui::Ui,
        _ctx: &egui::Context,
        _image_rect: egui::Rect,
        _fs_idx: usize,
    ) -> bool {
        false
    }

    /// 360 度パノラマビュー Phase 2a: NeedsUserConfirmation バナー
    /// (docs/panorama-360-view-plan.md §3.6.2 / §3.6.4)。
    ///
    /// 表示位置: フルスクリーン上部 (横幅は中央寄せ)、動画 HUD と同じ階層。
    /// 内容: 解像度 / MP / 想定 RAM 消費の数値表示 + 「フル解像度(高画質)」/
    /// 「最大 8K(軽量)」/ 「今後も高画質モードで開く」チェックボックス。
    /// state が NeedsUserConfirmation 以外なら何も描画せず即 return。
    fn draw_pano_confirmation_banner(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        fs_idx: usize,
    ) {
        // 対象画像の state 取得
        let Some(source_key) = self.metadata_cache_key(fs_idx) else {
            return;
        };
        let state = self.pano_quality_state.get(&source_key).cloned();
        let Some(crate::panorama::PanoramaQualityState::NeedsUserConfirmation {
            source_pixels,
            est_ram_gb,
        }) = state
        else {
            return;
        };
        // 源解像度を引き直す (バナーに表示する W×H 用)。
        let dims = self
            .fs_cache
            .get(&fs_idx)
            .and_then(|e| match e {
                crate::fs_animation::FsCacheEntry::Static { source_dims, .. } => *source_dims,
                _ => None,
            })
            .unwrap_or([0, 0]);
        let mp = source_pixels as f64 / 1_000_000.0;
        let header = format!(
            "⚠ 大きな 360° 画像です ({}×{}、約 {:.0} MP)",
            dims[0], dims[1], mp
        );
        let body = format!(
            "高品質モードで表示するには 約 {:.1} GB のメモリを使います。",
            est_ram_gb
        );

        // バナーの矩形 (上部中央、横幅 720px or 画面の 80%、最大)
        let banner_w = (full_rect.width() * 0.80).min(720.0).max(420.0);
        let center_x = full_rect.center().x;
        let top_y = full_rect.top() + 56.0; // 上部ホバーバーの少し下
        let banner_rect = egui::Rect::from_min_size(
            egui::pos2(center_x - banner_w * 0.5, top_y),
            egui::vec2(banner_w, 120.0),
        );

        // 半透明背景
        ui.painter().rect_filled(
            banner_rect.expand(6.0),
            egui::CornerRadius::same(8),
            egui::Color32::from_black_alpha(220),
        );
        ui.painter().rect_stroke(
            banner_rect.expand(6.0),
            egui::CornerRadius::same(8),
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(80)),
            egui::epaint::StrokeKind::Outside,
        );

        // 上部のテキスト 2 行
        ui.painter().text(
            egui::pos2(banner_rect.center().x, banner_rect.top() + 10.0),
            egui::Align2::CENTER_TOP,
            &header,
            egui::FontId::proportional(15.0),
            egui::Color32::from_rgb(255, 220, 120),
        );
        ui.painter().text(
            egui::pos2(banner_rect.center().x, banner_rect.top() + 34.0),
            egui::Align2::CENTER_TOP,
            &body,
            egui::FontId::proportional(13.0),
            egui::Color32::from_white_alpha(230),
        );

        // ボタン領域 (UI として allocate)
        let button_area_top = banner_rect.top() + 60.0;
        let button_area = egui::Rect::from_min_size(
            egui::pos2(banner_rect.left() + 16.0, button_area_top),
            egui::vec2(banner_rect.width() - 32.0, 50.0),
        );
        let mut clicked_hq = false;
        let mut clicked_base = false;
        // チェックボックス状態は self に専用フィールドを置かず、buttons の戻りで切り替える
        // 簡素化のため、毎フレ true 固定で扱い「今後も高品質」は default OFF / クリックで適用。
        // 実装ロード上は、HQ ボタン押下時に「自動承認モード」をユーザーが選ぶ別ボタンで対応。
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(button_area)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |child_ui| {
                if child_ui
                    .add_sized(
                        egui::vec2(200.0, 32.0),
                        egui::Button::new(
                            egui::RichText::new("フル解像度(高画質)").size(14.0).strong(),
                        )
                        .fill(egui::Color32::from_rgb(120, 180, 80)),
                    )
                    .on_hover_text(
                        "フル RGBA をメモリに保持し、静止時に画面解像度ぶんだけ\nCPU で再サンプリングして高画質オーバーレイを表示します。",
                    )
                    .clicked()
                {
                    clicked_hq = true;
                }
                child_ui.add_space(8.0);
                if child_ui
                    .add_sized(
                        egui::vec2(160.0, 32.0),
                        egui::Button::new(egui::RichText::new("最大 8K(軽量)").size(14.0)),
                    )
                    .on_hover_text(
                        "8K に縮小した表示のみ使用します。メモリ消費は抑えられますが、\n拡大時の精細さは下がります (= 通常画面と同じ品質)。",
                    )
                    .clicked()
                {
                    clicked_base = true;
                }
                child_ui.add_space(12.0);
                // 「今後も高画質モードで開く」: バナーごとの local state を持たない
                // (= 毎フレ false 起点で描画) ため、self の単純フィールドで管理。
                //
                // バナー背景が暗色 (`from_black_alpha(220)`) なので、egui デフォルトの
                // チェックボックスラベル色 (薄いグレー) では読みづらい。本文テキスト
                // (`from_white_alpha(230)`) と揃えて明示的に色とサイズを指定する。
                child_ui.checkbox(
                    &mut self.pano_banner_remember_session,
                    egui::RichText::new("今後も高画質モードで開く")
                        .color(egui::Color32::from_white_alpha(230))
                        .size(13.0),
                );
            },
        );

        if clicked_hq {
            // 状態を SettleApproved にして worker spawn (resolution で cache_key を取り直す)
            self.pano_quality_state.insert(
                source_key.clone(),
                crate::panorama::PanoramaQualityState::SettleApproved,
            );
            if self.pano_banner_remember_session {
                // **`.max()`** で **monotonically increasing** に保つ (Codex P1 第 5 ラウンド、
                // 2026-05): 直前に 350 MP を承認 (stored=350) → 220 MP を承認 (= source_pixels)
                // のときに stored=220 に下がると、次の 280 MP は 220×1.25=275 を超えて
                // バナー再表示になる。max を取れば stored=350 のまま保持できる。
                self.pano_session_approved_max_pixels =
                    self.pano_session_approved_max_pixels.max(source_pixels);
            }
            if let Some(resolution) = self.resolve_pano_source(ctx, fs_idx) {
                self.start_pano_high_res_load(fs_idx, resolution.cache_key);
            }
            ctx.request_repaint();
        }
        if clicked_base {
            self.pano_quality_state
                .insert(source_key, crate::panorama::PanoramaQualityState::BaseOnly);
            ctx.request_repaint();
        }
    }

    fn draw_compare_prepared_mode(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        image_rect: egui::Rect,
        mode: crate::app::CompareViewMode,
        zoom_pan: Option<(f32, egui::Vec2)>,
    ) -> bool {
        if self.compare_prepared_pair.is_none() {
            return false;
        }

        let shader_shape = self
            .compare_prepared_pair
            .as_ref()
            .and_then(|pair| match mode {
                crate::app::CompareViewMode::Wipe { fraction } => self.compare_shader_shape(
                    image_rect,
                    pair,
                    crate::compare_wgpu::CompareShaderMode::Wipe,
                    fraction,
                    zoom_pan,
                ),
                crate::app::CompareViewMode::Diff => self.compare_shader_shape(
                    image_rect,
                    pair,
                    crate::compare_wgpu::CompareShaderMode::Diff,
                    0.5,
                    zoom_pan,
                ),
                _ => None,
            });
        if let Some((draw_rect, shape)) = shader_shape {
            let bg_style = self.fs_bg_style(ctx);
            paint_transparent_bg(ui.painter(), draw_rect, &bg_style);
            ui.painter().add(shape);
            if let crate::app::CompareViewMode::Wipe { fraction } = mode {
                // 白線はシェーダーの合成境界 (draw_rect = フィット後の実表示画像矩形) に揃える。
                Self::draw_compare_wipe_line(ui, draw_rect, fraction);
            }
            return true;
        }

        match mode {
            crate::app::CompareViewMode::PinnedNormal => {
                let tex =
                    self.ensure_compare_prepared_texture(ctx, ComparePreparedTextureKind::Pinned);
                let Some(tex) = tex.as_ref() else {
                    return false;
                };
                let bg_style = self.fs_bg_style(ctx);
                Self::draw_compare_pinned_image(ui, image_rect, tex, zoom_pan, &bg_style, None);
                true
            }
            crate::app::CompareViewMode::Wipe { fraction } => {
                let current =
                    self.ensure_compare_prepared_texture(ctx, ComparePreparedTextureKind::Current);
                let pinned =
                    self.ensure_compare_prepared_texture(ctx, ComparePreparedTextureKind::Pinned);
                let (Some(current), Some(pinned)) = (current.as_ref(), pinned.as_ref()) else {
                    return false;
                };
                let bg_style = self.fs_bg_style(ctx);
                Self::draw_compare_pinned_image(ui, image_rect, current, zoom_pan, &bg_style, None);
                // 線 / clip はフィット後の実表示画像矩形基準にして、画像の切り替え位置と一致させる。
                let ref_rect = Self::compare_image_draw_rect(image_rect, current.size(), zoom_pan)
                    .unwrap_or(image_rect);
                let wipe_x = ref_rect.left() + ref_rect.width() * fraction.clamp(0.05, 0.95);
                let clip =
                    egui::Rect::from_min_max(ref_rect.min, egui::pos2(wipe_x, ref_rect.max.y));
                Self::draw_compare_pinned_image(
                    ui,
                    image_rect,
                    pinned,
                    zoom_pan,
                    &bg_style,
                    Some(clip),
                );
                Self::draw_compare_wipe_line(ui, ref_rect, fraction);
                true
            }
            crate::app::CompareViewMode::Diff => {
                let tex =
                    self.ensure_compare_prepared_texture(ctx, ComparePreparedTextureKind::Diff);
                let Some(tex) = tex.as_ref() else {
                    return false;
                };
                let bg_style = self.fs_bg_style(ctx);
                Self::draw_compare_pinned_image(ui, image_rect, tex, zoom_pan, &bg_style, None);
                true
            }
            crate::app::CompareViewMode::Off => false,
        }
    }

    fn draw_compare_pinned_image(
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        tex: &egui::TextureHandle,
        zoom_pan: Option<(f32, egui::Vec2)>,
        bg_style: &FsBgStyle<'_>,
        clip_rect: Option<egui::Rect>,
    ) {
        let Some(img_rect) =
            Self::compare_image_draw_rect(full_rect, [tex.size()[0], tex.size()[1]], zoom_pan)
        else {
            return;
        };
        let clip = clip_rect
            .map(|r| r.intersect(full_rect))
            .unwrap_or(full_rect);
        let painter = ui.painter().with_clip_rect(clip);
        paint_transparent_bg(&painter, img_rect, bg_style);
        painter.image(
            tex.id(),
            img_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    }

    fn draw_compare_wipe_line(ui: &mut egui::Ui, image_rect: egui::Rect, fraction: f32) {
        let x = image_rect.left() + image_rect.width() * fraction.clamp(0.05, 0.95);
        ui.painter().line_segment(
            [
                egui::pos2(x, image_rect.top()),
                egui::pos2(x, image_rect.bottom()),
            ],
            egui::Stroke::new(2.0, egui::Color32::from_white_alpha(150)),
        );
    }

    fn draw_compare_pin_indicator(
        &mut self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        ctx: &egui::Context,
    ) {
        let Some(display_name) = self
            .pinned_compare_slot
            .as_ref()
            .map(|slot| slot.display_name.clone())
        else {
            return;
        };
        let tex = self.ensure_compare_pinned_texture(ctx);

        let max_label_chars = 28usize;
        let label = if display_name.chars().count() > max_label_chars {
            let mut s: String = display_name.chars().take(max_label_chars).collect();
            s.push('…');
            format!("比較中: {s}")
        } else {
            format!("比較中: {display_name}")
        };
        let font = egui::FontId::proportional(13.0);
        let galley = ui
            .painter()
            .layout_no_wrap(label, font, egui::Color32::WHITE);
        let thumb_size = egui::vec2(72.0, 54.0);
        let width = (thumb_size.x + 10.0 + galley.size().x).min(full_rect.width() - 32.0);
        let panel_size = egui::vec2(width + 16.0, thumb_size.y + 16.0);
        let panel_rect = egui::Rect::from_min_size(
            egui::pos2(
                full_rect.max.x - panel_size.x - 18.0,
                full_rect.max.y - panel_size.y - 18.0,
            ),
            panel_size,
        );
        ui.painter().rect_filled(
            panel_rect,
            4.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 185),
        );
        let thumb_rect =
            egui::Rect::from_min_size(panel_rect.min + egui::vec2(8.0, 8.0), thumb_size);
        ui.painter().rect_filled(
            thumb_rect,
            2.0,
            egui::Color32::from_rgba_unmultiplied(35, 35, 35, 230),
        );
        if let Some(tex) = tex.as_ref() {
            let tex_size = tex.size_vec2();
            if tex_size.x > 0.0 && tex_size.y > 0.0 {
                let scale = (thumb_rect.width() / tex_size.x).min(thumb_rect.height() / tex_size.y);
                let draw_rect = egui::Rect::from_center_size(thumb_rect.center(), tex_size * scale);
                ui.painter().image(
                    tex.id(),
                    draw_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            }
        }
        let text_pos = egui::pos2(
            thumb_rect.max.x + 10.0,
            panel_rect.center().y - galley.size().y * 0.5,
        );
        ui.painter().galley(text_pos, galley, egui::Color32::WHITE);
    }

    /// 16×16 の市松テクスチャを lazy 生成する。
    /// B キーで「市松」モードになったとき初めて呼ばれる。
    pub(crate) fn ensure_checker_texture(&mut self, ctx: &egui::Context) {
        if self.fs_checker_texture.is_some() {
            return;
        }
        // 16×16 の 2 値グレー。タイルは 8×8 の 2 色。
        // Photoshop / GIMP 標準に近い中間グレーで、目に邪魔にならない配色。
        let mut rgba = Vec::with_capacity(16 * 16 * 4);
        for y in 0..16 {
            for x in 0..16 {
                let cell = ((x / 8) + (y / 8)) % 2;
                let v: u8 = if cell == 0 { 224 } else { 176 };
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        let ci = egui::ColorImage::from_rgba_unmultiplied([16, 16], &rgba);
        let options = egui::TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            minification: egui::TextureFilter::Nearest,
            wrap_mode: egui::TextureWrapMode::Repeat,
            mipmap_mode: None,
        };
        let tex = ctx.load_texture("fs_transparent_checker", ci, options);
        self.fs_checker_texture = Some(tex);
    }

    /// 現在の `fs_transparent_bg_mode` から描画スタイルを返す。
    /// 市松モードのときはテクスチャを lazy 生成する。
    fn fs_bg_style<'a>(&'a mut self, ctx: &egui::Context) -> FsBgStyle<'a> {
        if self.fs_transparent_bg_mode == 2 {
            self.ensure_checker_texture(ctx);
        }
        transparent_bg_style(
            self.fs_transparent_bg_mode,
            self.fs_checker_texture.as_ref(),
        )
    }

    /// 透過背景モードが Default 以外のとき、画面右上に現在モードを示す。
    /// モード変更直後 (`fs_transparent_bg_indicator_until` 有効) のみ表示。
    fn draw_fs_transparent_bg_indicator(&mut self, ui: &egui::Ui, full_rect: egui::Rect) {
        let Some(until) = self.fs_transparent_bg_indicator_until else {
            return;
        };
        let now = std::time::Instant::now();
        if now >= until {
            self.fs_transparent_bg_indicator_until = None;
            return;
        }
        // フェードアウト: 最後 400ms で alpha を下げる
        let remaining = until.saturating_duration_since(now);
        let alpha_f = (remaining.as_millis().min(400) as f32) / 400.0;
        let alpha = (alpha_f * 220.0) as u8;
        // 3 モード循環 (テーマ非依存): ビューポート既定の黒 (0) / 白 (1) / 市松 (2)。
        let label = match self.fs_transparent_bg_mode {
            1 => "背景: 白",
            2 => "背景: 市松",
            _ => "背景: 黒",
        };
        let painter = ui.painter();
        let font = egui::FontId::proportional(14.0);
        let galley = painter.layout_no_wrap(
            label.to_string(),
            font,
            egui::Color32::from_white_alpha(alpha),
        );
        let pos = egui::pos2(
            full_rect.max.x - galley.size().x - 16.0,
            full_rect.min.y + 12.0,
        );
        let bg = egui::Rect::from_min_size(pos, galley.size()).expand(6.0);
        painter.rect_filled(
            bg,
            4.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, alpha.saturating_sub(40)),
        );
        painter.galley(pos, galley, egui::Color32::from_white_alpha(alpha));
        ui.ctx().request_repaint(); // フェードを継続
    }

    fn draw_original_preview_indicator(&self, ui: &egui::Ui, full_rect: egui::Rect, active: bool) {
        if !active {
            return;
        }
        let label = if cfg!(windows) {
            "元画像表示中: 右Ctrl"
        } else {
            "元画像表示中: 0"
        };
        let painter = ui.painter();
        let font = egui::FontId::proportional(14.0);
        let galley = painter.layout_no_wrap(label.to_string(), font, egui::Color32::WHITE);
        let pos = egui::pos2(full_rect.min.x + 16.0, full_rect.min.y + 12.0);
        let bg = egui::Rect::from_min_size(pos, galley.size()).expand(6.0);
        painter.rect_filled(bg, 4.0, egui::Color32::from_rgba_unmultiplied(0, 0, 0, 190));
        painter.galley(pos, galley, egui::Color32::WHITE);
        ui.ctx().request_repaint();
    }

    fn draw_slideshow_progress_indicator(
        &self,
        ui: &egui::Ui,
        full_rect: egui::Rect,
        ctx: &egui::Context,
    ) {
        if !self.slideshow_playing {
            return;
        }

        let now = std::time::Instant::now();
        let interval = self.slideshow_interval_duration();
        let interval_secs = interval.as_secs_f32().max(0.001);
        let anchored = self
            .fullscreen_idx
            .is_some_and(|idx| self.slideshow_anchor_idx == Some(idx));
        let progress = if anchored {
            let remaining = self.slideshow_next_at.saturating_duration_since(now);
            ((interval_secs - remaining.as_secs_f32()) / interval_secs).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let painter = ui.painter();
        let center = egui::pos2(full_rect.max.x - 22.0, full_rect.min.y + 22.0);
        let radius = 8.0;
        painter.circle_stroke(
            center,
            radius,
            egui::Stroke::new(1.2, egui::Color32::from_white_alpha(42)),
        );

        if progress > 0.0 {
            let start = -std::f32::consts::FRAC_PI_2;
            let sweep = std::f32::consts::TAU * progress;
            let segments = ((36.0 * progress).ceil() as usize).clamp(2, 36);
            let mut points = Vec::with_capacity(segments + 1);
            for i in 0..=segments {
                let t = i as f32 / segments as f32;
                let angle = start + sweep * t;
                points.push(egui::pos2(
                    center.x + radius * angle.cos(),
                    center.y + radius * angle.sin(),
                ));
            }
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(2.0, egui::Color32::from_white_alpha(120)),
            ));
        }

        let repaint = if anchored {
            self.slideshow_next_at
                .saturating_duration_since(now)
                .min(std::time::Duration::from_millis(100))
        } else {
            std::time::Duration::from_millis(100)
        };
        ctx.request_repaint_after(repaint);
    }

    /// ルーペ (局所拡大) 描画。
    ///
    /// 有効条件:
    /// - `fs_loupe_locked` が true (M キーでトグル) か、keymap のルーペ保持修飾キー押下中
    /// - ビューポートにフォーカスがある
    /// - 分析モード・補正モードに入っていない
    /// - カーソルが `full_rect` 内
    /// - 画像が回転 0 / 任意回転なし (回転時は UV 逆変換が複雑なため v0.7.0 では非対応)
    /// - 現在 Single (非見開き) 表示で、テクスチャが存在する
    ///
    /// 見開き時は現状は未対応 (v0.7.0 のスコープ外)。
    pub(crate) fn draw_fs_loupe_if_active(
        &mut self,
        ui: &egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        fs_idx: usize,
        tex: Option<&egui::TextureHandle>,
        thumb_tex: Option<&egui::TextureHandle>,
        spread_double: bool,
    ) {
        if self.is_panorama_mode_active(fs_idx) {
            return;
        }
        if self.analysis_mode || self.adjustment_mode {
            return;
        }
        let (hover, focused) =
            ctx.input(|i| (i.pointer.hover_pos(), i.viewport().focused.unwrap_or(true)));
        let loupe_hold = self
            .keymap
            .modifier_held_action(ctx, KeyAction::FsLoupeHold);
        if !focused {
            return;
        }
        if !self.fs_loupe_locked && !loupe_hold {
            return;
        }
        let Some(cursor) = hover else { return };
        if !full_rect.contains(cursor) {
            return;
        }

        // ── 対象のページ矩形 + テクスチャを決定 ───────────────────────
        // Single / Spread で分岐。見開きはカーソル直下のページを選ぶ。
        let (img_rect, handle_owned, idx_for_rot) = if spread_double {
            let Some(layout) = self.fs_spread_layout else {
                return;
            };
            let (page_idx, page_rect) = if layout.left_rect.contains(cursor) {
                (layout.left_idx, layout.left_rect)
            } else if layout.right_rect.contains(cursor) {
                (layout.right_idx, layout.right_rect)
            } else {
                return;
            };
            // 見開き時はページ rect がそのまま image rect (draw_fs_spread が高さ統一で
            // アスペクトをぴったり合わせた矩形を組むため、letterbox は発生しない)。
            // ルーペも通常描画と同じ加工済みレイヤを参照する。
            let page_original_preview_active = self.original_preview_active(ctx, page_idx);
            let page_tex =
                self.resolve_fs_processed_texture(ctx, page_idx, page_original_preview_active);
            let page_thumb: Option<egui::TextureHandle> = match self.thumbnails.get(page_idx) {
                Some(ThumbnailState::Loaded { tex, .. }) => Some(tex.clone()),
                _ => None,
            };
            let Some(handle) = page_tex.or(page_thumb) else {
                return;
            };
            (page_rect, handle, page_idx)
        } else {
            let Some(handle) = tex.or(thumb_tex) else {
                return;
            };
            let tex_size = handle.size_vec2();
            if tex_size.x <= 0.0 || tex_size.y <= 0.0 {
                return;
            }
            let fit_scale = (full_rect.width() / tex_size.x).min(full_rect.height() / tex_size.y);
            let (total_scale, img_center) = match self.fs_zoom_pan() {
                Some((zoom, pan)) => (fit_scale * zoom, full_rect.center() + pan),
                None => (fit_scale, full_rect.center()),
            };
            let size = tex_size * total_scale;
            let rect = egui::Rect::from_center_size(img_center, size);
            (rect, handle.clone(), fs_idx)
        };

        // 回転 / 任意回転時は UV 逆変換が複雑なためルーペ非対応
        let rotation = self.get_rotation(idx_for_rot);
        if !rotation.is_none() || self.fs_free_rotation.abs() > TRANSFORM_EPSILON {
            return;
        }

        let tex_size = handle_owned.size_vec2();
        if tex_size.x <= 0.0 || tex_size.y <= 0.0 {
            return;
        }
        if !img_rect.contains(cursor) {
            return;
        }

        // 画面 px → テクスチャ px の変換倍率 (見開きでも単一でも共通のスカラー)
        let total_scale = img_rect.width() / tex_size.x;
        let uv_center = egui::vec2(
            (cursor.x - img_rect.min.x) / img_rect.width(),
            (cursor.y - img_rect.min.y) / img_rect.height(),
        );

        // ルーペパラメータ (将来設定化)
        const LOUPE_SIZE: f32 = 300.0;
        const LOUPE_ZOOM: f32 = 3.0;
        const LOUPE_OFFSET: f32 = 40.0;

        // サンプル UV: LOUPE_SIZE / LOUPE_ZOOM をテクスチャピクセル単位に変換
        let sample_px_half = LOUPE_SIZE * 0.5 / (total_scale * LOUPE_ZOOM);
        let half_uv = egui::vec2(sample_px_half / tex_size.x, sample_px_half / tex_size.y);
        let uv_min = egui::pos2(
            (uv_center.x - half_uv.x).clamp(0.0, 1.0),
            (uv_center.y - half_uv.y).clamp(0.0, 1.0),
        );
        let uv_max = egui::pos2(
            (uv_center.x + half_uv.x).clamp(0.0, 1.0),
            (uv_center.y + half_uv.y).clamp(0.0, 1.0),
        );
        let uv_rect = egui::Rect::from_min_max(uv_min, uv_max);

        // ポップアップ位置: カーソル右下 → はみ出すなら反転
        let mut popup_pos = cursor + egui::vec2(LOUPE_OFFSET, LOUPE_OFFSET);
        if popup_pos.x + LOUPE_SIZE > full_rect.max.x {
            popup_pos.x = cursor.x - LOUPE_OFFSET - LOUPE_SIZE;
        }
        if popup_pos.y + LOUPE_SIZE > full_rect.max.y {
            popup_pos.y = cursor.y - LOUPE_OFFSET - LOUPE_SIZE;
        }
        // それでも画面外に出る場合は full_rect の内側に寄せる
        popup_pos.x = popup_pos.x.max(full_rect.min.x + 4.0);
        popup_pos.y = popup_pos.y.max(full_rect.min.y + 4.0);
        let loupe_rect = egui::Rect::from_min_size(popup_pos, egui::vec2(LOUPE_SIZE, LOUPE_SIZE));

        let painter = ui.painter();
        // 背景 (黒で囲う) + 画像 + 枠線
        painter.rect_filled(loupe_rect.expand(3.0), 4.0, egui::Color32::BLACK);
        painter.image(handle_owned.id(), loupe_rect, uv_rect, egui::Color32::WHITE);
        painter.rect_stroke(
            loupe_rect,
            2.0,
            egui::Stroke::new(2.0, egui::Color32::WHITE),
            egui::StrokeKind::Outside,
        );
        // ルーペ保持中は再描画を継続 (キー離したら止める)
        if loupe_hold {
            ctx.request_repaint();
        }
    }

    /// 見開きモードの2ページ描画。
    /// 2枚の画像を中央に配置し、設定されたページ間隔だけ黒背景を見せる。
    fn draw_fs_spread(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        image_rect: egui::Rect,
        left_idx: usize,
        right_idx: usize,
        original_preview_active: bool,
    ) {
        let zoom_pan = self.fs_zoom_pan();
        let fit_mode = self.effective_fullscreen_fit_mode();
        let left_rot = self.get_rotation(left_idx);
        let right_rot = self.get_rotation(right_idx);
        // 余白カットフィット (見開き): 各ページの content bbox を取得し、後で左右セットの
        // union をフィットさせる。回転ページは対象外 (single 同様)。`fs_margin_bbox` は
        // &mut self なので bg_style の借用より前に算出しておく。
        let margin_left = if left_rot.is_none() {
            self.fs_margin_bbox(left_idx)
        } else {
            None
        };
        let margin_right = if right_rot.is_none() {
            self.fs_margin_bbox(right_idx)
        } else {
            None
        };
        // 各ページが読込中ブランチに落ちたときに出すパス。steady state では空文字列になり
        // `draw_centered_elided_label` が描画をスキップするので無駄な String 化を避ける。
        let left_location = self.location_display_for_loading(left_idx);
        let right_location = self.location_display_for_loading(right_idx);
        // 透過背景スタイル (bg_style はテクスチャ借用を含むため左右描画の前後で寿命に注意)
        // fs_bg_style は &mut self を要求するため先に解決してから以降は shared borrow に切り替える。
        // 透過画像が見開きの片方だけの場合もあるので両ページに同じ bg を適用する。
        let bg_tex = if self.fs_transparent_bg_mode == 2 {
            self.ensure_checker_texture(ctx);
            self.fs_checker_texture.clone()
        } else {
            None
        };
        let bg_style = transparent_bg_style(self.fs_transparent_bg_mode, bg_tex.as_ref());

        // 各ページの表示サイズを計算して、全体をフィットさせる
        // 片方だけフルサイズだとアスペクト比の微小差でレイアウトがジャンプするため、
        // 両方フルサイズが揃うまではサムネイルサイズに統一する
        let both_in_fs_cache =
            self.fs_cache.contains_key(&left_idx) && self.fs_cache.contains_key(&right_idx);
        let (left_size, right_size) = if both_in_fs_cache {
            (
                Self::get_display_size(left_idx, left_rot, &self.fs_cache, &self.thumbnails),
                Self::get_display_size(right_idx, right_rot, &self.fs_cache, &self.thumbnails),
            )
        } else {
            // サムネイルのみ使用（fs_cache を空マップとして渡す）
            let empty = std::collections::HashMap::new();
            (
                Self::get_display_size(left_idx, left_rot, &empty, &self.thumbnails),
                Self::get_display_size(right_idx, right_rot, &empty, &self.thumbnails),
            )
        };

        // ズーム/パンが有効な場合は image_rect でクリップする
        // (ズーム時にページが image_rect 外へはみ出して他の UI を覆わないようにするため)
        let painter = if zoom_pan.is_some() || !matches!(fit_mode, FullscreenFitMode::Page) {
            ui.painter().with_clip_rect(image_rect)
        } else {
            ui.painter().clone()
        };

        if let (Some(ls), Some(rs)) = (left_size, right_size) {
            let spread_gap = self.settings.spread_page_gap_px.min(200) as f32;
            // 両ページの高さを揃える（高い方に合わせる）
            let combined_h = ls.y.max(rs.y);
            let left_w = ls.x * (combined_h / ls.y);
            let right_w = rs.x * (combined_h / rs.y);

            let combined_w = left_w + right_w;

            // 余白カットフィット: 左右ページの content 矩形を combined 空間に写像して union を
            // 取り、セット全体の外周余白を詰める (回転なし & 設定 ON & どちらかに bbox あり)。
            let content = if matches!(fit_mode, FullscreenFitMode::MarginFit) {
                spread_content_union(margin_left, margin_right, left_w, right_w, combined_h)
            } else {
                None
            };

            // フィット基準: content (余白カット) があればその幅高さ、無ければ combined 全体。
            let (fit_w, fit_h, content_center) = match content {
                Some((cx0, cy0, cx1, cy1)) => (
                    (cx1 - cx0).max(1.0),
                    (cy1 - cy0).max(1.0),
                    egui::vec2((cx0 + cx1) * 0.5, (cy0 + cy1) * 0.5),
                ),
                None => (
                    combined_w,
                    combined_h,
                    egui::vec2(combined_w * 0.5, combined_h * 0.5),
                ),
            };
            let page_fit = || {
                ((image_rect.width() - spread_gap).max(1.0) / combined_w)
                    .min(image_rect.height() / combined_h)
            };
            let fit_scale = match fit_mode {
                FullscreenFitMode::MarginFit if content.is_some() => {
                    ((image_rect.width() - spread_gap).max(1.0) / fit_w)
                        .min(image_rect.height() / fit_h)
                }
                FullscreenFitMode::Width => (image_rect.width() - spread_gap).max(1.0) / combined_w,
                FullscreenFitMode::Height => image_rect.height() / combined_h,
                FullscreenFitMode::Original => 1.0,
                _ => page_fit(),
            };

            let (total_scale, base_center) = match zoom_pan {
                Some((zoom, pan)) => (fit_scale * zoom, image_rect.center() + pan),
                None => (fit_scale, image_rect.center()),
            };
            // content (= 余白カット後の中身) の中心を画面中心へ寄せる。
            // content_center=combined 中心のとき (余白カット無効) は従来どおり 0 ずれ。
            let combined_center = egui::vec2(combined_w * 0.5, combined_h * 0.5);
            let center = base_center - (content_center - combined_center) * total_scale;

            let scaled_lw = left_w * total_scale;
            let scaled_rw = right_w * total_scale;
            let scaled_h = combined_h * total_scale;

            // 全体を中央に配置
            let total_w = scaled_lw + spread_gap + scaled_rw;
            let start_x = center.x - total_w * 0.5;
            let start_y = center.y - scaled_h * 0.5;

            let left_rect = egui::Rect::from_min_size(
                egui::pos2(start_x, start_y),
                egui::vec2(scaled_lw, scaled_h),
            );
            let right_rect = egui::Rect::from_min_size(
                egui::pos2(start_x + scaled_lw + spread_gap, start_y),
                egui::vec2(scaled_rw, scaled_h),
            );

            // ナビ ロック中は旧ページのテクスチャを左右両方の最終フォールバックとして
            // 使い、「ファイル名のみ表示」状態を回避する。両ページに同じ holdover が
            // 出るのは束の間 (poll_fs_nav_lock がサムネ Loaded で解除する) なので許容。
            let holdover_for_locked = if self.fs_nav_is_locked() {
                self.fs_holdover_tex.clone()
            } else {
                None
            };
            let left_display_tex =
                self.resolve_fs_processed_texture(ctx, left_idx, original_preview_active);
            let right_display_tex =
                self.resolve_fs_processed_texture(ctx, right_idx, original_preview_active);
            for (rect, idx, rot, location, display_tex) in [
                (
                    left_rect,
                    left_idx,
                    left_rot,
                    &left_location,
                    left_display_tex.as_ref(),
                ),
                (
                    right_rect,
                    right_idx,
                    right_rot,
                    &right_location,
                    right_display_tex.as_ref(),
                ),
            ] {
                Self::draw_fs_spread_page(
                    &painter,
                    rect,
                    idx,
                    rot,
                    &self.thumbnails,
                    &bg_style,
                    location,
                    holdover_for_locked.as_ref(),
                    display_tex,
                );
            }

            // ルーペが参照するレイアウトを記録 (両ページのサイズが既知のときのみ信頼できる)
            self.fs_spread_layout = Some(FsSpreadLayout {
                left_idx,
                left_rect,
                right_idx,
                right_rect,
            });
        } else {
            // サイズ不明の場合は均等分割フォールバック
            // (ズーム/パンはサイズが分かってからでないと正しく計算できないため適用しない)
            let spread_gap = self.settings.spread_page_gap_px.min(200) as f32;
            let half_w = (image_rect.width() - spread_gap).max(2.0) / 2.0;
            let left_rect =
                egui::Rect::from_min_size(image_rect.min, egui::vec2(half_w, image_rect.height()));
            let right_rect = egui::Rect::from_min_size(
                egui::pos2(image_rect.min.x + half_w + spread_gap, image_rect.min.y),
                egui::vec2(half_w, image_rect.height()),
            );
            // フォールバック分岐でも nav ロック中の holdover を渡す (上のパス参照)。
            let holdover_for_locked = if self.fs_nav_is_locked() {
                self.fs_holdover_tex.clone()
            } else {
                None
            };
            let left_display_tex =
                self.resolve_fs_processed_texture(ctx, left_idx, original_preview_active);
            let right_display_tex =
                self.resolve_fs_processed_texture(ctx, right_idx, original_preview_active);
            for (rect, idx, rot, location, display_tex) in [
                (
                    left_rect,
                    left_idx,
                    left_rot,
                    &left_location,
                    left_display_tex.as_ref(),
                ),
                (
                    right_rect,
                    right_idx,
                    right_rot,
                    &right_location,
                    right_display_tex.as_ref(),
                ),
            ] {
                Self::draw_fs_spread_page(
                    &painter,
                    rect,
                    idx,
                    rot,
                    &self.thumbnails,
                    &bg_style,
                    location,
                    holdover_for_locked.as_ref(),
                    display_tex,
                );
            }
            // フォールバック分岐: サイズ未確定でアスペクト比が崩れる可能性があるため、
            // ルーペ用レイアウトには書かない (ルーペは非見開きパスのロジックで描画しない)。
            self.fs_spread_layout = None;
        }
    }

    /// テクスチャの表示サイズ（回転考慮）を返す。テクスチャ未取得なら None。
    fn get_display_size(
        idx: usize,
        rotation: crate::rotation_db::Rotation,
        fs_cache: &std::collections::HashMap<usize, FsCacheEntry>,
        thumbnails: &[ThumbnailState],
    ) -> Option<egui::Vec2> {
        let tex = match fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { tex, .. }) => Some(tex.size_vec2()),
            Some(FsCacheEntry::Animated {
                frames,
                current_frame,
                ..
            }) => frames.get(*current_frame).map(|(h, _)| h.size_vec2()),
            _ => None,
        };
        let size = tex.or_else(|| {
            if let Some(ThumbnailState::Loaded { tex, .. }) = thumbnails.get(idx) {
                Some(tex.size_vec2())
            } else {
                None
            }
        })?;
        Some(match rotation {
            crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
                egui::vec2(size.y, size.x)
            }
            _ => size,
        })
    }

    /// 見開きモードの1ページ分を指定領域に描画。
    /// `painter` は呼び出し側でクリップ済みのものを渡すことで、ズーム時のはみ出しを防ぐ。
    /// `location_display` は draw_fs_image と同じで、空なら読込中ラベル描画をスキップ。
    /// `display_tex` は `resolve_fs_processed_texture` で解決済みの加工済みテクスチャ。
    /// ここでは thumbnail → holdover の最終フォールバックだけを担当する。
    #[allow(clippy::too_many_arguments)]
    fn draw_fs_spread_page(
        painter: &egui::Painter,
        rect: egui::Rect,
        idx: usize,
        rotation: crate::rotation_db::Rotation,
        thumbnails: &[ThumbnailState],
        bg_style: &FsBgStyle<'_>,
        location_display: &str,
        holdover_tex: Option<&egui::TextureHandle>,
        display_tex: Option<&egui::TextureHandle>,
    ) {
        let thumb_tex = match thumbnails.get(idx) {
            Some(ThumbnailState::Loaded { tex, .. }) => Some(tex.clone()),
            _ => None,
        };
        let display_tex = display_tex.or(thumb_tex.as_ref()).or(holdover_tex);

        if let Some(handle) = display_tex {
            let tex_size = handle.size_vec2();
            let display_size = match rotation {
                crate::rotation_db::Rotation::Cw90 | crate::rotation_db::Rotation::Cw270 => {
                    egui::vec2(tex_size.y, tex_size.x)
                }
                _ => tex_size,
            };
            let fit_scale = (rect.width() / display_size.x).min(rect.height() / display_size.y);
            let img_rect = egui::Rect::from_center_size(rect.center(), display_size * fit_scale);
            // 回転中は bbox のズレを避けて背景を適用しない
            if rotation.is_none() {
                paint_transparent_bg(painter, img_rect, bg_style);
                painter.image(
                    handle.id(),
                    img_rect,
                    egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    egui::Color32::WHITE,
                );
            } else {
                crate::app::draw_rotated_image(painter, handle.id(), img_rect, rotation);
            }
        } else {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "読込中...",
                egui::FontId::proportional(18.0),
                egui::Color32::from_gray(150),
            );
            crate::ui_helpers::draw_centered_elided_label(
                painter,
                rect,
                location_display,
                12.0,
                egui::Color32::from_gray(150),
                rect.center().y + 18.0,
                12.0,
            );
        }
    }

    /// フルスクリーン UI が「クリーンな状態」(= 上部バー / 左右パネル / HUD / モーダルが
    /// 何も出ていない) か判定する。`true` かつアイドル時間が `CURSOR_HIDE_IDLE_SECS` を
    /// 超えたらマウスカーソルを `CursorIcon::None` で非表示にする。
    fn fs_ui_is_clean(&self, ctx: &egui::Context, full_rect: egui::Rect, _is_video: bool) -> bool {
        let pointer = ctx.input(|i| i.pointer.hover_pos());
        // Once the cursor is hidden, the last hover position is stale until a real input
        // event arrives. Do not let that passive position keep hover UI "visible" and
        // immediately revive the OS cursor after slideshow advances.
        let passive_hover_enabled = !self.cursor_hidden;
        let in_top = passive_hover_enabled && pointer.is_some_and(|p| p.y < TOP_BAR_HOVER_Y);
        let in_right = passive_hover_enabled
            && pointer.is_some_and(|p| p.x > full_rect.max.x - full_rect.width() * 0.25);
        // 動画再生中の HUD / speed popup は native presenter overlay 側で管理されるため
        // egui main window のカーソル可視判定からは除外する (= 旧 egui HUD は撤去済)。
        !in_top
            && !in_right
            && !self.show_metadata_panel
            && !self.metadata_panel_hover_active
            && !self.adjustment_mode
            && !self.is_overlay_edit_mode_active()
            && !self.analysis_mode
            && !self.spread_popup_open
            && !self.fit_popup_open
            && self.fs_context_menu_idx.is_none()
            && !self.any_dialog_open()
    }

    /// フルスクリーンのホバー時トップバーを描画する。
    #[allow(clippy::too_many_arguments)]
    /// 上部ホバーバーを描画する。`location_display` は左側に表示するパス文字列
    /// (`FsFrameState::location_display`)。通常は `<folder>\<filename>`、ZIP/PDF
    /// 内は `<archive-path> > <entry>`。
    fn draw_fs_hover_bar(
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        full_rect: egui::Rect,
        location_display: &str,
        image_dims: Option<(u32, u32)>,
        image_file_size: Option<u64>,
        // 原寸が GPU 上限超で表示が縮小版のとき true。dims のあとに⚠マーカーを出す。
        image_downscaled: bool,
        close_fs: &mut bool,
        nav_delta: &mut i32,
        show_info: &mut bool,
        force_show: bool,
        slideshow_playing: &mut bool,
        _slideshow_interval: &mut f32,
        rotate_cw: &mut bool,
        rotate_ccw: &mut bool,
        // 分析ボタン: 表示状態 (active) は値で受け、押下は out フラグで返す。`analysis_mode` を
        // 直接反転すると Z キー経路の副作用 (ズーム/パン引き継ぎ・bypass enter/exit・補正排他) を
        // 飛ばすため、呼び出し側で `toggle_analysis_mode()` に合流させる (Codex P1)。
        analysis_active: bool,
        analysis_pressed: &mut bool,
        // 360 度パノラマビュー (docs/panorama-360-view-plan.md §5.3):
        // - trigger Some(Auto/Hint) のとき球体アイコンを表示。None なら disabled 表示。
        // - panorama_active=true なら強調背景。
        // - panorama_mode_active=true (= state Some + detect Some) なら他のボタンを
        //   全て隠して 360 / × / window_toggle のみに絞る (= 360 モード中は機能制限)。
        // - クリックされたら panorama_pressed = true を返す (caller が toggle 経路を呼ぶ)。
        panorama_trigger: Option<crate::panorama::PanoramaTrigger>,
        panorama_active: bool,
        panorama_mode_active: bool,
        panorama_pressed: &mut bool,
        spread_mode: &mut SpreadMode,
        reading_flow: &mut ReadingFlow,
        reading_direction: &mut ReadingDirection,
        spread_popup_open: &mut bool,
        is_spread_double: bool,
        // AI アップスケール後のサイズとモデル名（表示用）。動画モードでは無視される。
        ai_upscale_info: Option<(&str, u32, u32)>,
        // 画像補正パネル表示トグル
        adjustment_mode: &mut bool,
        local_adjust_mode: &mut bool,
        // 現在ページに個別補正が適用されているか (ボタン点灯用)
        has_page_override: bool,
        // ズーム/フィットモード (画像のみ)。fit_mode = 現在の実効モード。
        // ボタンクリックで fit_popup_open をトグルし、メニュー項目選択で
        // fit_mode_choice に選択モードを返す ([0] キーの循環とは別系統)。
        fit_mode: FullscreenFitMode,
        fit_popup_open: &mut bool,
        fit_mode_choice: &mut Option<FullscreenFitMode>,
        // PDF ページのコンテンツ種別 (非 PDF なら None)
        pdf_content_type: Option<PdfPageContentType>,
        // Phase 6: 動画モードか。true なら画像専用ボタンを隠し、▦ タイルボタンに
        // 切替、右側情報も動画情報に差し替える。
        is_video: bool,
        // 動画情報 (= info() の値抜粋、is_video=true のときのみ有効)。
        // (duration_secs, bit_rate_bps)
        video_meta: Option<(f64, i64)>,
        // ▦ タイルボタンの状態 + 押下フラグ。
        tile_active: bool,
        tile_pressed: &mut bool,
        // VST3 プラグイン管理ボタン (動画モード + vst3_enabled のときのみ表示)。
        // active = 管理パネルが既に開いている。pressed = クリックされた。
        show_vst3_button: bool,
        vst3_panel_open: bool,
        vst3_pressed: &mut bool,
        copy_capture_pressed: &mut bool,
        // ウィンドウ / 全画面 切り替えボタン (× の左)。show=表示するか、
        // in_window=現在 in-window 表示中か、pressed=クリックされたか。
        show_window_toggle: bool,
        in_window_mode: bool,
        window_mode_pressed: &mut bool,
        cursor_hidden: bool,
    ) {
        let hover_in_top = ctx.input(|i| {
            !cursor_hidden
                && i.pointer
                    .hover_pos()
                    .map(|p| p.y < TOP_BAR_HOVER_Y)
                    .unwrap_or(false)
        });
        // adjustment_mode がオンならオーバーレイとして常に表示
        if !hover_in_top
            && !force_show
            && !*spread_popup_open
            && !*fit_popup_open
            && !*adjustment_mode
        {
            return;
        }

        let bar_rect =
            egui::Rect::from_min_size(full_rect.min, egui::vec2(full_rect.width(), TOP_BAR_HEIGHT));
        ui.painter().rect_filled(
            bar_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200),
        );
        ui.painter().line_segment(
            [
                egui::pos2(bar_rect.min.x, bar_rect.max.y),
                egui::pos2(bar_rect.max.x, bar_rect.max.y),
            ],
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 60),
            ),
        );

        // ── ボタン群（右端から左に並べる）──
        let mut next_x = bar_rect.max.x - BAR_BUTTON_SIZE - BAR_BUTTON_MARGIN;

        // × 閉じるボタン
        // 360 モード中はクリックで「360 モード OFF」(= toggle_panorama_mode で
        // panorama_state を None に) に転用 (2026-05 ユーザー要望)。
        // フルスクリーン全体を閉じたい場合は Esc を使う。
        let close_resp = draw_bar_button(
            ui,
            next_x,
            bar_rect.min.y + BAR_BUTTON_MARGIN,
            "fs_close_btn",
            |hovered| {
                if hovered {
                    egui::Color32::from_rgba_unmultiplied(220, 50, 50, 230)
                } else {
                    egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
                }
            },
            false, // active 状態なし
            |p, c, r| draw_close_icon(p, c, r),
        );
        let close_tooltip = if panorama_active {
            "360° モードを抜ける (フルスクリーンを閉じるには Esc)"
        } else {
            "閉じる [Esc]"
        };
        let close_resp = close_resp.hover_tip_dark(close_tooltip);
        if close_resp.clicked() {
            if panorama_active {
                // 360 モード中は × → 360 解除 (toggle_panorama_mode が OFF 経路に入る)。
                *panorama_pressed = true;
            } else {
                *close_fs = true;
            }
        }
        if close_resp.hovered() {
            *nav_delta = 0;
        }
        next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;

        // ⊞ ウィンドウ / 全画面 切り替えボタン (× の左)。
        // native 動画 HUD のトグルボタンと同じ役割を、静止画フルスクリーンの
        // egui ホバーバーに置く。
        if show_window_toggle {
            let wm_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_window_mode_btn",
                |hovered| bar_button_bg(hovered, in_window_mode),
                in_window_mode,
                |p, c, r| draw_window_toggle_icon(p, c, r),
            );
            let wm_resp = wm_resp.hover_tip_dark(if in_window_mode {
                "全画面表示に切り替え [F11]"
            } else {
                "ウィンドウ内表示に切り替え [F11]"
            });
            if wm_resp.clicked() {
                *window_mode_pressed = true;
            }
            if wm_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // 📷 キャプチャコピー (画像のみ)。Ctrl+S のファイル保存と同じ snapshot 経路を使う。
        // 360 モード中は機能制限のため非表示 (docs/panorama-360-view-plan.md フィードバック対応)。
        if !is_video && !panorama_mode_active {
            let camera_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_capture_copy_btn",
                |hovered| bar_button_bg(hovered, false),
                false,
                |p, c, r| draw_camera_icon(p, c, r),
            );
            let camera_resp = camera_resp
                .hover_tip_dark("クリック: クリップボードにコピー\nCtrl+S: ファイル保存");
            if camera_resp.clicked() {
                *copy_capture_pressed = true;
            }
            if camera_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // VST ボタン: 動画モード + VST3 機能 ON のときだけ表示。
        // クリックで管理パネルを開く / 閉じる。management panel は egui::Window で
        // フルスクリーンビューポート内に描画されるので動画の手前に出る。
        // 360 モード中は非表示 (= 360 は画像専用なのでここに来ない想定だが二重保護)。
        if show_vst3_button && !panorama_mode_active {
            let vst_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_vst3_btn",
                |hovered| bar_button_bg(hovered, vst3_panel_open),
                vst3_panel_open,
                |p, c, _r| draw_vst_text_label(p, c),
            );
            let vst_resp = vst_resp.hover_tip_dark(if vst3_panel_open {
                "VST3 プラグイン管理を閉じる"
            } else {
                "VST3 プラグイン管理を開く"
            });
            if vst_resp.clicked() {
                *vst3_pressed = true;
            }
            if vst_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // ▶/⏸ スライドショーボタン (画像モード) または ▦ タイルボタン (動画モード)
        // 360 モード中は非表示。
        if panorama_mode_active {
            // skip
        } else if is_video {
            let tile_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_video_tile_btn",
                |hovered| bar_button_bg(hovered, tile_active),
                tile_active,
                |p, c, r| draw_tile_grid_icon(p, c, r),
            );
            let tile_resp = tile_resp.hover_tip_dark(if tile_active {
                "タイルモード解除 [S]"
            } else {
                "タイルモード [S]"
            });
            if tile_resp.clicked() {
                *tile_pressed = true;
            }
            if tile_resp.hovered() {
                *nav_delta = 0;
            }
        } else {
            let play_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_play_btn",
                |hovered| {
                    if *slideshow_playing {
                        egui::Color32::from_rgba_unmultiplied(60, 180, 60, 200)
                    } else if hovered {
                        egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
                    } else {
                        egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
                    }
                },
                false,
                |p, c, r| {
                    if *slideshow_playing {
                        draw_pause_icon(p, c, r);
                    } else {
                        draw_play_triangle(p, c, r);
                    }
                },
            );
            let play_resp = if *slideshow_playing {
                play_resp.hover_tip_dark("スライドショー停止")
            } else {
                play_resp.hover_tip_dark("スライドショー")
            };
            if play_resp.clicked() {
                *slideshow_playing = !*slideshow_playing;
            }
            if play_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // ↷ 右回転 / ↶ 左回転ボタン (画像のみ — 動画では意味を持たないため非表示)
        // 360 モード中は非表示 (360 ビューは独自の yaw を持つため rotation_db は適用しない)。
        if !is_video && !panorama_mode_active {
            let rcw_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_rcw_btn",
                |hovered| bar_button_bg(hovered, false),
                false,
                |p, c, r| draw_rotate_icon(p, c, r, true),
            );
            let rcw_resp = rcw_resp.hover_tip_dark("右回転 [R]");
            if rcw_resp.clicked() {
                *rotate_cw = true;
            }
            if rcw_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;

            let rccw_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_rccw_btn",
                |hovered| bar_button_bg(hovered, false),
                false,
                |p, c, r| draw_rotate_icon(p, c, r, false),
            );
            let rccw_resp = rccw_resp.hover_tip_dark("左回転 [L]");
            if rccw_resp.clicked() {
                *rotate_ccw = true;
            }
            if rccw_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // ℹ Info ボタン (360 モード中は非表示)
        if !panorama_mode_active {
            let info_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_info_btn",
                |hovered| bar_button_bg(hovered, *show_info),
                *show_info,
                |p, c, r| draw_info_icon(p, c, r),
            );
            let info_resp = info_resp.hover_tip_dark("メタデータ [I / Tab]");
            if info_resp.clicked() {
                *show_info = !*show_info;
            }
            if info_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // 🔬 分析ボタン（見開きダブル中は非表示。動画では意味を持たないため非表示）
        // 360 モード中も非表示。
        if !is_spread_double && !is_video && !panorama_mode_active && reading_flow.is_paged() {
            let analysis_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_analysis_btn",
                |hovered| bar_button_bg(hovered, analysis_active),
                analysis_active,
                |p, c, r| draw_analysis_icon(p, c, r),
            );
            let analysis_resp = analysis_resp.hover_tip_dark("分析ツール [Z]");
            if analysis_resp.clicked() {
                *analysis_pressed = true;
            }
            if analysis_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // 360 度パノラマビューボタン (docs/panorama-360-view-plan.md §5.3 / §6.1):
        // - 360 モード OFF + 360 対応画像: ボタン表示 + クリックで ON
        // - 360 モード OFF + 非対応画像: disabled 表示
        // - **360 モード ON 時はボタンを隠す** (× が 360 解除を兼ねるため、
        //   2026-05 ユーザー要望)
        // 元設計 (常時表示 + 強調背景) はユーザーフィードバックで取り下げ。
        if !is_video && !is_spread_double && !panorama_active && reading_flow.is_paged() {
            let tooltip = match panorama_trigger {
                Some(crate::panorama::PanoramaTrigger::Auto) => "360° 画像 (XMP 検出) [V]",
                Some(crate::panorama::PanoramaTrigger::Hint) => {
                    "360° ビューワーで開く (アスペクト比から推定) [V]"
                }
                None => "360° 画像ではありません",
            };
            let is_enabled = panorama_trigger.is_some();
            let pano_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_panorama_btn",
                |hovered| {
                    if is_enabled {
                        bar_button_bg(hovered, panorama_active)
                    } else {
                        // disabled: 押せないので hover でも色を変えない
                        egui::Color32::from_rgba_unmultiplied(60, 60, 60, 160)
                    }
                },
                panorama_active,
                |p, c, r| {
                    if is_enabled {
                        draw_panorama_icon(p, c, r);
                    } else {
                        draw_panorama_icon_disabled(p, c, r);
                    }
                },
            );
            let pano_resp = pano_resp.hover_tip_dark(tooltip);
            if is_enabled && pano_resp.clicked() {
                *panorama_pressed = true;
            }
            if pano_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // 📖 表示モードボタン (画像のみ。動画では非表示)
        // 360 モード中も非表示 (360 は強制 Single)。
        let spread_active = spread_mode.is_spread() || !reading_flow.is_paged();
        let sm = *spread_mode;
        let mut spread_resp_rect = egui::Rect::NOTHING;
        if !is_video && !panorama_mode_active {
            let spread_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_spread_btn",
                |hovered| bar_button_bg(hovered, spread_active),
                spread_active,
                |p, c, r| draw_spread_icon(p, c, r, sm),
            );
            let spread_resp =
                spread_resp.hover_tip_dark("表示モード [1-5] / 連結方式 [6] / 横方向 [7]");
            spread_resp_rect = spread_resp.rect;
            if spread_resp.clicked() {
                *spread_popup_open = !*spread_popup_open;
                *fit_popup_open = false;
            }
            if spread_resp.hovered() {
                *nav_delta = 0;
            }
        } else if *spread_popup_open {
            // 動画モードに切り替わったときは popup を閉じる (表示モードは画像のみ)
            *spread_popup_open = false;
        }

        // 表示モードポップアップ (画像のみ)
        if *spread_popup_open && !is_video {
            let popup_x = next_x;
            let popup_y = bar_rect.max.y + 4.0;
            let popup_w = 230.0_f32;
            let popup_h =
                (SpreadMode::all().len() + ReadingFlow::all().len() + 2) as f32 * 36.0 + 92.0;
            let popup_rect = egui::Rect::from_min_size(
                egui::pos2(popup_x, popup_y),
                egui::vec2(popup_w, popup_h),
            );

            // 背景
            ui.painter().rect_filled(
                popup_rect,
                6.0,
                egui::Color32::from_rgba_unmultiplied(30, 30, 30, 240),
            );
            ui.painter().rect_stroke(
                popup_rect,
                6.0,
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(100, 100, 100, 180),
                ),
                egui::StrokeKind::Outside,
            );

            let mut item_y = popup_rect.min.y + 4.0;
            ui.painter().text(
                egui::pos2(popup_rect.min.x + 12.0, item_y + 16.0),
                egui::Align2::LEFT_CENTER,
                "ページ構成",
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(150),
            );
            item_y += 28.0;
            for &mode in SpreadMode::all() {
                let item_rect = egui::Rect::from_min_size(
                    egui::pos2(popup_rect.min.x + 4.0, item_y),
                    egui::vec2(popup_w - 8.0, 32.0),
                );
                let item_resp = ui.interact(
                    item_rect,
                    egui::Id::new(format!("spread_popup_{}", mode.to_int())),
                    egui::Sense::click(),
                );
                let is_current = *spread_mode == mode;
                let bg = if is_current {
                    egui::Color32::from_rgba_unmultiplied(80, 140, 220, 200)
                } else if item_resp.hovered() {
                    egui::Color32::from_rgba_unmultiplied(80, 80, 80, 200)
                } else {
                    egui::Color32::TRANSPARENT
                };
                ui.painter().rect_filled(item_rect, 4.0, bg);

                // アイコン (左側)
                let icon_center = egui::pos2(item_rect.min.x + 20.0, item_rect.center().y);
                draw_spread_icon(ui.painter(), icon_center, 7.0, mode);

                // ラベル (右側)
                ui.painter().text(
                    egui::pos2(item_rect.min.x + 44.0, item_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    mode.label(),
                    egui::FontId::proportional(13.0),
                    egui::Color32::from_gray(220),
                );

                let shortcut_label = format!("[{}]", mode.to_int() + 1);
                ui.painter().text(
                    egui::pos2(item_rect.max.x - 8.0, item_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    shortcut_label,
                    egui::FontId::proportional(11.0),
                    egui::Color32::from_gray(140),
                );

                if item_resp.clicked() {
                    *spread_mode = mode;
                    if mode.is_rtl() {
                        *reading_direction = ReadingDirection::Rtl;
                    } else if matches!(mode, SpreadMode::Ltr | SpreadMode::LtrCover) {
                        *reading_direction = ReadingDirection::Ltr;
                    }
                    *spread_popup_open = false;
                }
                item_y += 36.0;
            }

            ui.painter().text(
                egui::pos2(popup_rect.min.x + 12.0, item_y + 16.0),
                egui::Align2::LEFT_CENTER,
                "連結方式",
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(150),
            );
            item_y += 28.0;
            for &flow in ReadingFlow::all() {
                let item_rect = egui::Rect::from_min_size(
                    egui::pos2(popup_rect.min.x + 4.0, item_y),
                    egui::vec2(popup_w - 8.0, 32.0),
                );
                let item_resp = ui.interact(
                    item_rect,
                    egui::Id::new(format!("reading_flow_popup_{}", flow.to_int())),
                    egui::Sense::click(),
                );
                let is_current = *reading_flow == flow;
                let bg = if is_current {
                    egui::Color32::from_rgba_unmultiplied(80, 140, 220, 200)
                } else if item_resp.hovered() {
                    egui::Color32::from_rgba_unmultiplied(80, 80, 80, 200)
                } else {
                    egui::Color32::TRANSPARENT
                };
                ui.painter().rect_filled(item_rect, 4.0, bg);
                ui.painter().text(
                    egui::pos2(item_rect.min.x + 16.0, item_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    flow.label(),
                    egui::FontId::proportional(13.0),
                    egui::Color32::from_gray(220),
                );
                ui.painter().text(
                    egui::pos2(item_rect.max.x - 8.0, item_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    if flow == ReadingFlow::Paged {
                        "[6]"
                    } else {
                        "[6循環]"
                    },
                    egui::FontId::proportional(11.0),
                    egui::Color32::from_gray(140),
                );
                if item_resp.clicked() {
                    *reading_flow = flow;
                    *spread_popup_open = false;
                }
                item_y += 36.0;
            }

            ui.painter().text(
                egui::pos2(popup_rect.min.x + 12.0, item_y + 16.0),
                egui::Align2::LEFT_CENTER,
                "横方向",
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(150),
            );
            item_y += 28.0;
            for &direction in &[ReadingDirection::Ltr, ReadingDirection::Rtl] {
                let item_rect = egui::Rect::from_min_size(
                    egui::pos2(popup_rect.min.x + 4.0, item_y),
                    egui::vec2(popup_w - 8.0, 32.0),
                );
                let item_resp = ui.interact(
                    item_rect,
                    egui::Id::new(format!("reading_direction_popup_{}", direction.to_int())),
                    egui::Sense::click(),
                );
                let is_current = *reading_direction == direction;
                let bg = if is_current {
                    egui::Color32::from_rgba_unmultiplied(80, 140, 220, 200)
                } else if item_resp.hovered() {
                    egui::Color32::from_rgba_unmultiplied(80, 80, 80, 200)
                } else {
                    egui::Color32::TRANSPARENT
                };
                ui.painter().rect_filled(item_rect, 4.0, bg);
                ui.painter().text(
                    egui::pos2(item_rect.min.x + 16.0, item_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    direction.label(),
                    egui::FontId::proportional(13.0),
                    egui::Color32::from_gray(220),
                );
                ui.painter().text(
                    egui::pos2(item_rect.max.x - 8.0, item_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    "[7]",
                    egui::FontId::proportional(11.0),
                    egui::Color32::from_gray(140),
                );
                if item_resp.clicked() {
                    *reading_direction = direction;
                    *spread_popup_open = false;
                }
                item_y += 36.0;
            }

            // ポップアップ外クリックで閉じる
            let pointer_pos = ctx.input(|i| i.pointer.press_origin());
            if let Some(pos) = pointer_pos {
                if !popup_rect.contains(pos) && !spread_resp_rect.contains(pos) {
                    if ctx.input(|i| i.pointer.any_pressed()) {
                        *spread_popup_open = false;
                    }
                }
            }
        }

        if !is_video {
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // ズーム/フィットボタン (画像のみ、見開きボタンの左)。クリックでメニューを開く。
        let fit_btn_x = next_x;
        let mut fit_resp_rect = egui::Rect::NOTHING;
        if !is_video && !panorama_mode_active {
            let fit_active = !matches!(fit_mode, FullscreenFitMode::Page);
            let mf_resp = draw_bar_button(
                ui,
                next_x,
                bar_rect.min.y + BAR_BUTTON_MARGIN,
                "fs_margin_fit_btn",
                |hovered| bar_button_bg(hovered, fit_active),
                fit_active,
                draw_margin_fit_icon,
            );
            let fit_tip = if !reading_flow.is_paged() {
                format!(
                    "ズーム/フィット: {} [クリックで選択 / 0で循環]\n連結モードでは余白カットフィットをスキップ",
                    fit_mode.label()
                )
            } else {
                format!(
                    "ズーム/フィット: {} [クリックで選択 / 0で循環]",
                    fit_mode.label()
                )
            };
            let mf_resp = mf_resp.hover_tip_dark(fit_tip);
            fit_resp_rect = mf_resp.rect;
            if mf_resp.clicked() {
                *fit_popup_open = !*fit_popup_open;
                *spread_popup_open = false;
            }
            if mf_resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        } else if *fit_popup_open {
            // 動画 / 360 モードに切り替わったら閉じる (フィットは画像のみ)
            *fit_popup_open = false;
        }

        // ズーム/フィットポップアップ (画像のみ)。現在モードを青でハイライト。
        if *fit_popup_open && !is_video && !panorama_mode_active {
            let modes = FullscreenFitMode::selectable_for_flow(*reading_flow);
            let header_h = 28.0_f32;
            let item_h = 36.0_f32;
            let popup_w = 264.0_f32;
            let popup_h = header_h + modes.len() as f32 * item_h + 8.0;
            let popup_rect = egui::Rect::from_min_size(
                egui::pos2(fit_btn_x, bar_rect.max.y + 4.0),
                egui::vec2(popup_w, popup_h),
            );

            ui.painter().rect_filled(
                popup_rect,
                6.0,
                egui::Color32::from_rgba_unmultiplied(30, 30, 30, 240),
            );
            ui.painter().rect_stroke(
                popup_rect,
                6.0,
                egui::Stroke::new(
                    1.0,
                    egui::Color32::from_rgba_unmultiplied(100, 100, 100, 180),
                ),
                egui::StrokeKind::Outside,
            );

            let mut item_y = popup_rect.min.y + 4.0;
            ui.painter().text(
                egui::pos2(popup_rect.min.x + 12.0, item_y + 16.0),
                egui::Align2::LEFT_CENTER,
                "ズーム/フィット  [0で循環]",
                egui::FontId::proportional(11.0),
                egui::Color32::from_gray(150),
            );
            item_y += header_h;
            for &mode in modes {
                let item_rect = egui::Rect::from_min_size(
                    egui::pos2(popup_rect.min.x + 4.0, item_y),
                    egui::vec2(popup_w - 8.0, 32.0),
                );
                let item_resp = ui.interact(
                    item_rect,
                    egui::Id::new(format!("fit_popup_{mode:?}")),
                    egui::Sense::click(),
                );
                let is_current = fit_mode == mode;
                let bg = if is_current {
                    egui::Color32::from_rgba_unmultiplied(80, 140, 220, 200)
                } else if item_resp.hovered() {
                    egui::Color32::from_rgba_unmultiplied(80, 80, 80, 200)
                } else {
                    egui::Color32::TRANSPARENT
                };
                ui.painter().rect_filled(item_rect, 4.0, bg);

                ui.painter().text(
                    egui::pos2(item_rect.min.x + 16.0, item_rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    mode.label(),
                    egui::FontId::proportional(13.0),
                    egui::Color32::from_gray(220),
                );

                if item_resp.clicked() {
                    *fit_mode_choice = Some(mode);
                    *fit_popup_open = false;
                }
                item_y += item_h;
            }

            // ポップアップ外クリックで閉じる
            if let Some(pos) = ctx.input(|i| i.pointer.press_origin()) {
                if !popup_rect.contains(pos)
                    && !fit_resp_rect.contains(pos)
                    && ctx.input(|i| i.pointer.any_pressed())
                {
                    *fit_popup_open = false;
                }
            }
        }

        // 🎨 画像補正パネルトグルボタン (動画では非表示)
        // 補正ボタン (360 モード中は非表示)
        if !is_video && !panorama_mode_active && reading_flow.is_paged() {
            let btn_rect = egui::Rect::from_min_size(
                egui::pos2(next_x, bar_rect.min.y + BAR_BUTTON_MARGIN),
                egui::vec2(BAR_BUTTON_SIZE, BAR_BUTTON_SIZE),
            );
            let resp = ui.interact(
                btn_rect,
                egui::Id::new("fs_adjust_btn"),
                egui::Sense::click(),
            );
            let bg = if *adjustment_mode {
                egui::Color32::from_rgba_unmultiplied(80, 140, 220, 220)
            } else if has_page_override {
                // 個別設定が効いているときは薄い警告色でヒント
                egui::Color32::from_rgba_unmultiplied(120, 100, 60, 200)
            } else if resp.hovered() {
                egui::Color32::from_rgba_unmultiplied(100, 100, 100, 200)
            } else {
                egui::Color32::from_rgba_unmultiplied(70, 70, 70, 200)
            };
            ui.painter().rect_filled(btn_rect, 4.0, bg);
            ui.painter().text(
                btn_rect.center(),
                egui::Align2::CENTER_CENTER,
                "🎨",
                egui::FontId::proportional(16.0),
                egui::Color32::WHITE,
            );
            let tooltip = if has_page_override {
                "画像補正 (このページは個別設定あり)"
            } else {
                "画像補正"
            };
            let resp = resp.hover_tip_dark(tooltip);
            if resp.clicked() {
                let opening = !*adjustment_mode;
                *adjustment_mode = opening;
                if opening {
                    *local_adjust_mode = false;
                }
            }
            if resp.hovered() {
                *nav_delta = 0;
            }
            next_x -= BAR_BUTTON_SIZE + BAR_BUTTON_GAP;
        }

        // ── 左側: フォルダ + ファイル名 (または archive > entry) ──
        // 右側のボタン / 情報テキストと衝突しないように幅制限して右端を切る。
        // Phase 6: 動画モードでは AI アップスケール / PDF 情報を渡さず、
        // 動画専用の info text に切り替える。
        let info_text = if is_video {
            build_info_text_video(image_dims, image_file_size, video_meta)
        } else {
            build_info_text(
                image_dims,
                image_file_size,
                image_downscaled,
                ai_upscale_info,
                pdf_content_type,
            )
        };
        if !location_display.is_empty() {
            let info_w = if info_text.is_empty() {
                0.0
            } else {
                ui.painter()
                    .layout_no_wrap(
                        info_text.clone(),
                        egui::FontId::proportional(15.0),
                        egui::Color32::WHITE,
                    )
                    .size()
                    .x
            };
            let max_x = next_x - 12.0 - info_w;
            let avail_width = (max_x - (bar_rect.min.x + 12.0)).max(40.0);
            // パス文字列は 1 行に切り詰める (溢れは末尾を省略)。折り返すと上バー
            // (TOP_BAR_HEIGHT=44px) を超えて下の補正パネルに食い込むため
            // (狭い in-window 表示で顕著)。
            let mut job = egui::text::LayoutJob::single_section(
                location_display.to_string(),
                egui::TextFormat {
                    font_id: egui::FontId::proportional(13.0),
                    color: egui::Color32::from_gray(200),
                    ..Default::default()
                },
            );
            job.wrap = egui::text::TextWrapping {
                max_width: avail_width,
                max_rows: 1,
                ..Default::default()
            };
            let galley = ui.painter().layout_job(job);
            let text_y = bar_rect.center().y - galley.size().y * 0.5;
            ui.painter().galley(
                egui::pos2(bar_rect.min.x + 12.0, text_y),
                galley,
                egui::Color32::from_gray(200),
            );
        }

        // ── 右側: 画像サイズ / 動画情報 / ファイルサイズ ──
        if is_video {
            if !info_text.is_empty() {
                ui.painter().text(
                    egui::pos2(next_x - 12.0, bar_rect.center().y),
                    egui::Align2::RIGHT_CENTER,
                    info_text,
                    egui::FontId::proportional(15.0),
                    egui::Color32::WHITE,
                );
            }
        } else {
            draw_fs_bar_info_text(
                ui,
                bar_rect,
                egui::pos2(next_x - 12.0, bar_rect.center().y),
                image_dims,
                image_file_size,
                image_downscaled,
                ai_upscale_info,
                pdf_content_type,
            );
        }
    }
}

// ── フルスクリーン AI ステータスオーバーレイ ────────────────────────────

impl App {
    /// 現在有効な AI 処理のモデル名を結合して返す。
    /// `show_auto_prefix` が true の場合、自動選択時に「自動: 」プレフィックスを付ける。
    fn ai_model_label(&self, fs_idx: usize, show_auto_prefix: bool) -> String {
        let mut labels = Vec::new();
        if let Some(denoise_kind) = self.ai_denoise_model {
            labels.push(denoise_kind.display_label().to_string());
        }
        if self.ai_upscale_enabled {
            let upscale_label = match self.ai_upscale_model_override {
                Some(k) => k.display_label().to_string(),
                None => self
                    .ai_classify_cache
                    .get(&fs_idx)
                    .map(|c| {
                        if show_auto_prefix {
                            format!("自動: {}", c.display_label())
                        } else {
                            c.display_label().to_string()
                        }
                    })
                    .unwrap_or_else(|| "自動".to_string()),
            };
            labels.push(upscale_label);
        }
        labels.join(" + ")
    }

    /// フルスクリーン左下に AI 処理ステータスを表示する。
    fn draw_fs_ai_status(&mut self, ui: &mut egui::Ui, fs_idx: usize) {
        let is_upscaling = self
            .final_ai_pending
            .keys()
            .any(|key| key.edit_key.idx == fs_idx);
        let is_upscaled = self
            .final_ai_cache
            .keys()
            .any(|key| key.edit_key.idx == fs_idx);
        let is_loading = self.fs_pending.contains_key(&fs_idx);
        let any_busy = is_loading || is_upscaling || !self.final_ai_pending.is_empty();

        let mut lines: Vec<(String, egui::Color32)> = Vec::new();

        if is_loading {
            lines.push(("読込中...".to_string(), egui::Color32::from_gray(210)));
        }

        if is_upscaling {
            let label = self.ai_model_label(fs_idx, true);
            lines.push((
                format!("AI 処理中 ({})", label),
                egui::Color32::from_rgb(255, 200, 80),
            ));
        } else if is_upscaled {
            let label = self.ai_model_label(fs_idx, false);
            lines.push((
                format!("AI 処理完了 ({})", label),
                egui::Color32::from_rgb(80, 220, 80),
            ));
        }

        if self.erase_base_cache.contains_key(&fs_idx) && !self.erase_mode {
            lines.push((
                "消去補完済み".to_string(),
                egui::Color32::from_rgb(180, 140, 255),
            ));
        }

        // Pipeline P1 refactor で旧 ai_upscale_pending を参照する経路が無効化されて
        // 進捗バーが常時非表示になっていた。新パイプライン版 `final_ai_prefetch_progress`
        // が done/total を計算して、先読み中だけバーを出す。
        let prefetch_progress: Option<(usize, usize)> = self.final_ai_prefetch_progress(fs_idx);

        if lines.is_empty() && prefetch_progress.is_none() {
            self.ai_status_done_at = None;
            return;
        }

        // 全処理完了後の自動非表示: 完了から 1 秒フル表示、続く 1 秒でフェードアウト。
        const FADE_START_SECS: f32 = 1.0;
        const FADE_DURATION_SECS: f32 = 1.0;
        if any_busy {
            self.ai_status_done_at = None;
        } else {
            let done_at = *self
                .ai_status_done_at
                .get_or_insert_with(std::time::Instant::now);
            if done_at.elapsed().as_secs_f32() > FADE_START_SECS + FADE_DURATION_SECS {
                return;
            }
        }

        let alpha = if let Some(done_at) = self.ai_status_done_at {
            let elapsed = done_at.elapsed().as_secs_f32();
            if elapsed < FADE_START_SECS {
                1.0
            } else {
                (1.0 - (elapsed - FADE_START_SECS) / FADE_DURATION_SECS).clamp(0.0, 1.0)
            }
        } else {
            1.0
        };

        // Area の available width が 0 のまま描画されるとラベルが 1 文字幅で
        // 縦に折り返される。min_width で横方向を確保する。
        const MIN_WIDTH: f32 = 260.0;
        const BAR_WIDTH: f32 = 180.0;
        const FONT_SIZE: f32 = 13.0;

        let ctx = ui.ctx().clone();
        let bottom_offset = if self.fs_seek_overlay_visible {
            -58.0
        } else {
            -12.0
        };
        egui::Area::new("fs_ai_status_overlay".into())
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(12.0, bottom_offset))
            .show(&ctx, |ui| {
                ui.set_opacity(alpha);
                ui.set_min_width(MIN_WIDTH);
                egui::Frame::popup(ui.style())
                    .fill(crate::ui_helpers::PROGRESS_BG_COLOR)
                    .show(ui, |ui| {
                        for (text, color) in &lines {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(text).color(*color).size(FONT_SIZE),
                                )
                                .wrap_mode(egui::TextWrapMode::Extend),
                            );
                        }
                        if let Some((done, total)) = prefetch_progress {
                            ui.horizontal(|ui| {
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new("先読み AI")
                                            .monospace()
                                            .color(crate::ui_helpers::PROGRESS_LABEL_COLOR),
                                    )
                                    .wrap_mode(egui::TextWrapMode::Extend),
                                );
                                ui.add(
                                    egui::ProgressBar::new(done as f32 / total as f32)
                                        .desired_width(BAR_WIDTH)
                                        .fill(crate::ui_helpers::PROGRESS_UPGRADE_COLOR)
                                        .text(
                                            egui::RichText::new(format!("{} / {}", done, total))
                                                .color(egui::Color32::BLACK),
                                        ),
                                );
                            });
                        }
                    });
            });

        // フェードアウト中のみ毎フレーム再描画。処理中の進捗更新は
        // poll_final_ai 側が完了時に repaint を要求するのでここでの busy-loop repaint は不要。
        if self.ai_status_done_at.is_some() {
            ctx.request_repaint();
        }
    }
}

impl App {
    /// スタンプ埋め込み worker (R2-6) の進行を画面中央に表示する。worker が完了していれば
    /// `poll_stamp_embed` がここで適用し、まだ処理中なら中央に「スタンプ読み込み中…」を出して
    /// 再描画を要求する (UI は固まらない)。
    pub(crate) fn draw_stamp_embed_overlay(
        &mut self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        ctx: &egui::Context,
    ) {
        // 完了チェック + stale 破棄。まだ処理中なら true。
        if !self.poll_stamp_embed() {
            return;
        }
        let text = "スタンプ読み込み中…";
        let font = egui::FontId::proportional(20.0);
        let galley =
            ui.painter()
                .layout_no_wrap(text.to_string(), font.clone(), egui::Color32::WHITE);
        let padding = egui::vec2(28.0, 18.0);
        let box_size = galley.size() + padding * 2.0;
        let rect = egui::Rect::from_center_size(full_rect.center(), box_size);
        ui.painter().rect_filled(
            rect,
            10.0,
            egui::Color32::from_rgba_unmultiplied(20, 20, 20, 230),
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            font,
            egui::Color32::WHITE,
        );
        // worker 完了を取りこぼさないよう毎フレーム再描画。
        ctx.request_repaint();
    }

    /// 注釈ベイク worker (B) の完了を取り込み (GPU upload + `comic_cache` 挿入) し、現在表示中の
    /// ページのベイクが進行中なら `true` を返す (トースト表示用)。`try_recv` は `&self` なので
    /// iter 中に取り出し、完了/切断した idx を集めてから mutable 処理する。
    pub(crate) fn poll_comic_bake(&mut self, ctx: &egui::Context) -> bool {
        if self.comic_bake_pending.is_empty() {
            return false;
        }
        let mut ready: Vec<(usize, crate::app::ComicBakeResult)> = Vec::new();
        let mut dropped: Vec<usize> = Vec::new();
        for (&i, p) in self.comic_bake_pending.iter() {
            match p.rx.try_recv() {
                Ok(r) => ready.push((i, r)),
                Err(std::sync::mpsc::TryRecvError::Empty) => {}
                Err(std::sync::mpsc::TryRecvError::Disconnected) => dropped.push(i),
            }
        }
        for i in dropped {
            self.comic_bake_pending.remove(&i);
        }
        for (i, r) in ready {
            let Some(p) = self.comic_bake_pending.remove(&i) else {
                continue;
            };
            // フォルダ差し替え / 削除で items 世代が進んでいたら、この idx は別画像を指す可能性が
            // あるので結果を捨てる (idx 使い回しによる誤 upload / 誤キャッシュ防止、Codex P1)。
            // comic_gen / base は読み出し時 (ensure) の ptr_eq 照合でも守られるが、ここで弾けば
            // 無駄な GPU upload と stale エントリ保持も避けられる。
            if p.items_gen != self.items_generation {
                continue;
            }
            let composed = std::sync::Arc::new(r.pixels);
            let t_up = std::time::Instant::now();
            let upload = crate::app::clamp_for_gpu(&composed).into_owned();
            let texture = ctx.load_texture(
                format!(
                    "comic_{i}_{}_{}_{}",
                    std::sync::Arc::as_ptr(&p.base) as usize,
                    p.comic_gen,
                    p.preview_scale
                ),
                upload,
                egui::TextureOptions::LINEAR,
            );
            let upload_ms = t_up.elapsed().as_secs_f64() * 1000.0;
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "fs",
                    "comic_composite_build",
                    None,
                    0,
                    &[
                        ("ms", (r.bake_ms + r.composite_ms + upload_ms).into()),
                        ("bake_ms", r.bake_ms.into()),
                        ("composite_ms", r.composite_ms.into()),
                        ("upload_ms", upload_ms.into()),
                        ("w", (r.w as u64).into()),
                        ("h", (r.h as u64).into()),
                        ("objs", (r.objs as u64).into()),
                        ("preview_scale", (p.preview_scale as u64).into()),
                        ("idx", (i as u64).into()),
                        ("worker", true.into()),
                    ],
                );
            }
            self.comic_cache.insert(
                i,
                crate::app::ComicCacheEntry {
                    pixels: composed,
                    texture,
                    base: p.base,
                    comic_gen: p.comic_gen,
                    dims: p.dims,
                    preview_scale: p.preview_scale,
                },
            );
            ctx.request_repaint();
        }
        // 表示中ページのベイクが 150ms 以上続いていればトースト対象 (軽い注釈の高速ベイクで
        // 一瞬チラつかせない)。完了の取り込み自体は上で常に行う (ensure 側が毎フレーム
        // request_repaint するので、トースト非表示でも poll は回り続ける)。
        self.fullscreen_idx.is_some_and(|fi| {
            self.comic_bake_pending
                .get(&fi)
                .is_some_and(|p| p.started.elapsed().as_millis() >= 150)
        })
    }

    /// 注釈ベイク worker の進行表示 (中央「テキスト処理中…」)。完了取り込み + 進行判定は
    /// `poll_comic_bake` が行い、進行中のときだけトーストを描く (stamp embed と同流儀)。
    pub(crate) fn draw_comic_bake_overlay(
        &mut self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        ctx: &egui::Context,
    ) {
        if !self.poll_comic_bake(ctx) {
            return;
        }
        let text = "テキスト処理中…";
        let font = egui::FontId::proportional(20.0);
        let galley =
            ui.painter()
                .layout_no_wrap(text.to_string(), font.clone(), egui::Color32::WHITE);
        let padding = egui::vec2(28.0, 18.0);
        let box_size = galley.size() + padding * 2.0;
        let rect = egui::Rect::from_center_size(full_rect.center(), box_size);
        ui.painter().rect_filled(
            rect,
            10.0,
            egui::Color32::from_rgba_unmultiplied(20, 20, 20, 230),
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            font,
            egui::Color32::WHITE,
        );
        // worker 完了を取りこぼさないよう毎フレーム再描画。
        ctx.request_repaint();
    }

    /// 右上にフィードバックトーストを描画する。
    /// フルスクリーン側 (`render_fullscreen_viewport`) とグリッド側 (`render_grid`) の
    /// 両方から呼ばれる。どちらで描画しても同じ見た目になるよう、描画先 `ui` と
    /// トーストの基準矩形 `full_rect` を呼び出し側が渡す。
    pub(crate) fn draw_feedback_toast(
        &mut self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        ctx: &egui::Context,
    ) {
        let Some((ref text, start_time, duration)) = self.fs_feedback_toast else {
            return;
        };
        let elapsed = start_time.elapsed().as_secs_f32();
        if elapsed > duration {
            self.fs_feedback_toast = None;
            self.fs_feedback_toast_reveal_path = None;
            return;
        }

        // フェードアウト (最後の0.3秒)
        let alpha = if elapsed > duration - 0.3 {
            ((duration - elapsed) / 0.3).clamp(0.0, 1.0)
        } else {
            1.0
        };
        let alpha_u8 = (alpha * 220.0) as u8;

        let font = egui::FontId::proportional(18.0);
        let galley = ui
            .painter()
            .layout_no_wrap(text.clone(), font.clone(), egui::Color32::WHITE);
        let text_size = galley.size();
        let padding = egui::vec2(16.0, 10.0);
        let toast_size = text_size + padding * 2.0;

        let toast_rect = egui::Rect::from_min_size(
            egui::pos2(
                full_rect.max.x - toast_size.x - 20.0,
                full_rect.min.y + 60.0,
            ),
            toast_size,
        );

        ui.painter().rect_filled(
            toast_rect,
            8.0,
            egui::Color32::from_rgba_unmultiplied(30, 30, 30, alpha_u8),
        );
        ui.painter().text(
            toast_rect.center(),
            egui::Align2::CENTER_CENTER,
            text,
            font,
            egui::Color32::from_rgba_unmultiplied(255, 255, 255, (alpha * 255.0) as u8),
        );

        if let Some(path) = self.fs_feedback_toast_reveal_path.clone() {
            let resp = ui
                .interact(
                    toast_rect,
                    egui::Id::new("feedback_toast_reveal_capture"),
                    egui::Sense::click(),
                )
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .hover_tip_dark("クリックで保存ファイルを表示");
            if resp.clicked() {
                crate::capture::reveal_path_async(path);
                self.fs_feedback_toast = None;
                self.fs_feedback_toast_reveal_path = None;
            }
        }

        // フェードアウト中は 30fps で再描画
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }

    /// 画面中央に境界ヒント (最初/最後の項目です… / 次のフォルダが見つかりません…) を描画する。
    fn draw_boundary_hint(
        &mut self,
        ui: &mut egui::Ui,
        full_rect: egui::Rect,
        ctx: &egui::Context,
    ) {
        let Some(hint) = self.fs_boundary_hint else {
            return;
        };
        let start_time = hint.started_at();
        let duration = match hint {
            FsBoundaryHint::Edge { .. } => BOUNDARY_HINT_DURATION,
            FsBoundaryHint::NoImageFolder { .. } => NO_IMAGE_FOLDER_HINT_DURATION,
            FsBoundaryHint::NoSiblingFolder { .. } => NO_IMAGE_FOLDER_HINT_DURATION,
            FsBoundaryHint::SearchEnd { .. } => NO_IMAGE_FOLDER_HINT_DURATION,
            FsBoundaryHint::NavNoOp { .. } => BOUNDARY_HINT_DURATION,
        };
        let elapsed = start_time.elapsed().as_secs_f32();
        if elapsed > duration {
            self.fs_boundary_hint = None;
            return;
        }

        let alpha = if elapsed > duration - 0.4 {
            ((duration - elapsed) / 0.4).clamp(0.0, 1.0)
        } else {
            1.0
        };

        let (title, body_lines): (&str, Vec<&str>) = match hint {
            FsBoundaryHint::Edge { at_end: true, .. } => (
                "最後の項目です",
                vec!["[Home] 最初に戻る", "[Ctrl]+[↓] ツリー順で次へ"],
            ),
            FsBoundaryHint::Edge { at_end: false, .. } => (
                "最初の項目です",
                vec!["[End] 最後に移動", "[Ctrl]+[↑] ツリー順で前へ"],
            ),
            FsBoundaryHint::NoImageFolder { forward: true, .. } => (
                "次のフォルダに画像・動画が見つかりません",
                vec![
                    "[Esc] でサムネイル一覧に戻り",
                    "[Ctrl]+[↓] で空フォルダを越えて移動できます",
                ],
            ),
            FsBoundaryHint::NoImageFolder { forward: false, .. } => (
                "前のフォルダに画像・動画が見つかりません",
                vec![
                    "[Esc] でサムネイル一覧に戻り",
                    "[Ctrl]+[↑] で空フォルダを越えて移動できます",
                ],
            ),
            FsBoundaryHint::NoSiblingFolder { forward: true, .. } => (
                "次の兄弟フォルダはありません",
                vec!["[Alt]+[↑] 親フォルダへ", "[Ctrl]+[↓] ツリー順で次へ"],
            ),
            FsBoundaryHint::NoSiblingFolder { forward: false, .. } => (
                "前の兄弟フォルダはありません",
                vec!["[Alt]+[↑] 親フォルダへ", "[Ctrl]+[↑] ツリー順で前へ"],
            ),
            FsBoundaryHint::SearchEnd { forward: true, .. } => (
                "最後の検索結果です",
                vec!["[Esc] で検索を閉じると", "通常のフォルダ移動に戻ります"],
            ),
            FsBoundaryHint::SearchEnd { forward: false, .. } => (
                "最初の検索結果です",
                vec!["[Esc] で検索を閉じると", "通常のフォルダ移動に戻ります"],
            ),
            FsBoundaryHint::NavNoOp {
                reason: FsNavNoOpReason::LocalFilterActive,
                ..
            } => (
                Self::nav_noop_title(FsNavNoOpReason::LocalFilterActive),
                vec!["現在の一覧フィルタを維持します"],
            ),
            FsBoundaryHint::NavNoOp {
                reason: FsNavNoOpReason::SearchResultList,
                ..
            } => (
                Self::nav_noop_title(FsNavNoOpReason::SearchResultList),
                vec!["結果を開くと検索スコープ内で移動できます"],
            ),
            FsBoundaryHint::NavNoOp {
                reason: FsNavNoOpReason::SearchSiblingUnsupported,
                ..
            } => (
                Self::nav_noop_title(FsNavNoOpReason::SearchSiblingUnsupported),
                vec!["検索を閉じると通常フォルダで移動できます"],
            ),
        };

        let title_font = egui::FontId::proportional(32.0);
        let body_font = egui::FontId::proportional(22.0);
        let white = egui::Color32::from_rgba_unmultiplied(255, 255, 255, (alpha * 255.0) as u8);
        let accent = egui::Color32::from_rgba_unmultiplied(255, 220, 120, (alpha * 255.0) as u8);

        let painter = ui.painter();
        let title_galley = painter.layout_no_wrap(title.to_string(), title_font.clone(), white);
        let body_galleys: Vec<_> = body_lines
            .iter()
            .map(|s| painter.layout_no_wrap(s.to_string(), body_font.clone(), white))
            .collect();

        let line_gap = 10.0;
        let padding = egui::vec2(32.0, 24.0);
        let content_w = body_galleys
            .iter()
            .map(|g| g.size().x)
            .fold(title_galley.size().x, f32::max);
        let body_h: f32 = body_galleys.iter().map(|g| g.size().y).sum::<f32>()
            + line_gap * (body_galleys.len().saturating_sub(1) as f32);
        let content_h = title_galley.size().y + line_gap * 1.5 + body_h;
        let box_size = egui::vec2(content_w, content_h) + padding * 2.0;

        let center = full_rect.center();
        let box_rect = egui::Rect::from_center_size(center, box_size);

        let bg_alpha = (alpha * 210.0) as u8;
        painter.rect_filled(
            box_rect,
            12.0,
            egui::Color32::from_rgba_unmultiplied(20, 20, 20, bg_alpha),
        );
        painter.rect_stroke(
            box_rect,
            12.0,
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(200, 200, 200, (alpha * 120.0) as u8),
            ),
            egui::StrokeKind::Outside,
        );

        let mut y = box_rect.min.y + padding.y;
        painter.text(
            egui::pos2(center.x, y),
            egui::Align2::CENTER_TOP,
            title,
            title_font,
            accent,
        );
        y += title_galley.size().y + line_gap * 1.5;
        for (line, galley) in body_lines.iter().zip(body_galleys.iter()) {
            painter.text(
                egui::pos2(center.x, y),
                egui::Align2::CENTER_TOP,
                *line,
                body_font.clone(),
                white,
            );
            y += galley.size().y + line_gap;
        }

        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
}

// ===========================================================================
// 動画インライン再生用 UI ヘルパー
// ===========================================================================

impl App {
    /// 現在 `fs_idx` のキャッシュエントリが動画なら `&VideoPlayer` を返す。
    /// HUD 各ウィジェットの click handler / 上位の toggle_play で使う。
    pub(crate) fn fs_video_player(&self, fs_idx: usize) -> Option<&crate::video::VideoPlayer> {
        match self.fs_cache.get(&fs_idx)? {
            FsCacheEntry::Video { player, .. } => Some(player),
            _ => None,
        }
    }

    pub(crate) fn step_video_frame(&mut self, ctx: &egui::Context, fs_idx: usize, direction: i32) {
        if let Some(player) = self.fs_video_player(fs_idx) {
            player.step_frame(direction);
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    pub(crate) fn copy_video_frame_to_clipboard(&mut self, fs_idx: usize) {
        let Some((path, target_secs)) = self.fs_video_player(fs_idx).and_then(|player| {
            if player.error().is_some() || player.info().is_none() {
                None
            } else {
                Some((player.path().clone(), player.screenshot_target_secs()))
            }
        }) else {
            self.show_feedback_toast("動画フレームをコピーできません".to_string());
            return;
        };

        self.show_feedback_toast("動画フレームをクリップボードへコピー中".to_string());
        let clipboard_seq = crate::ui_dialogs::context_menu::reserve_clipboard_write_sequence();
        std::thread::Builder::new()
            .name("video-frame-copy".into())
            .spawn(move || match crate::video::screenshot::capture_frame(&path, target_secs) {
                Ok(frame) => {
                    crate::logger::log(format!(
                        "video frame copied to clipboard: target={:.3}s decoded_target={:.3}s {}x{} {}",
                        target_secs,
                        frame.target_secs,
                        frame.width,
                        frame.height,
                        path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                    ));
                    crate::ui_dialogs::context_menu::copy_rgba_image_to_clipboard_async_seq(
                        frame.width,
                        frame.height,
                        frame.rgba,
                        clipboard_seq,
                    );
                }
                Err(err) => {
                    crate::logger::log(format!(
                        "video frame copy failed: target={target_secs:.3}s {}: {err}",
                        path.display()
                    ));
                }
            })
            .ok();
    }

    pub(crate) fn save_video_frame_to_file(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.capture_pending.is_some() {
            self.show_feedback_toast("キャプチャ保存中です".to_string());
            return;
        }

        let Some((path, target_secs)) = self.fs_video_player(fs_idx).and_then(|player| {
            if player.error().is_some() || player.info().is_none() {
                None
            } else {
                Some((player.path().clone(), player.screenshot_target_secs()))
            }
        }) else {
            self.show_feedback_toast("動画フレームを保存できません".to_string());
            return;
        };

        let output_dir = self.capture_output_dir_path();
        let format = self.settings.capture_format;
        let basename = crate::capture::basename_for_path(&path);
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_path = path.clone();
        let thread = std::thread::Builder::new()
            .name("video-frame-save".into())
            .spawn(move || {
                let result = match crate::video::screenshot::capture_frame(&worker_path, target_secs)
                {
                    Ok(frame) => {
                        crate::logger::log(format!(
                            "video frame captured for save: target={:.3}s decoded_target={:.3}s {}x{} {}",
                            target_secs,
                            frame.target_secs,
                            frame.width,
                            frame.height,
                            worker_path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                        ));
                        crate::capture::save_rgba_unique(
                            &output_dir,
                            &basename,
                            format,
                            frame.width,
                            frame.height,
                            &frame.rgba,
                        )
                    }
                    Err(err) => Err(format!("動画フレーム取得に失敗しました: {err}")),
                };
                let _ = tx.send(result);
            });

        match thread {
            Ok(_) => {
                self.capture_pending = Some(crate::app::CapturePending { rx });
                self.show_feedback_toast(format!("動画フレームを保存中 ({})", format.label()));
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(err) => {
                self.show_feedback_toast(format!("保存 worker を開始できません: {err}"));
            }
        }
    }

    pub(crate) fn toggle_compare_pin_from_current(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.compare_pin_pending.is_some() {
            self.show_feedback_toast("比較画像を準備中です".to_string());
            return;
        }

        if self
            .pinned_compare_slot
            .as_ref()
            .is_some_and(|slot| slot.source_idx == fs_idx)
        {
            self.pinned_compare_slot = None;
            self.compare_view_mode = crate::app::CompareViewMode::Off;
            self.compare_pin_load_pending = None;
            self.compare_prepare_pending = None;
            self.compare_prepared_pair = None;
            self.compare_wipe_dragging = false;
            self.show_feedback_toast("比較画像を解除しました".to_string());
            ctx.request_repaint();
            return;
        }

        let work = match self.prepare_capture_pixel_work(ctx, fs_idx) {
            Ok(work) => work,
            Err(err) => {
                self.show_feedback_toast(err);
                return;
            }
        };
        self.start_compare_pin_work(ctx, fs_idx, work);
    }

    pub(crate) fn start_compare_pin_single(&mut self, ctx: &egui::Context, idx: usize) {
        let work = match self.prepare_capture_pixel_job(ctx, idx) {
            Ok(job) => crate::capture::CapturePixelWork::Single(job),
            Err(err) => {
                self.show_feedback_toast(err);
                return;
            }
        };
        self.start_compare_pin_work(ctx, idx, work);
    }

    fn start_compare_pin_work(
        &mut self,
        ctx: &egui::Context,
        source_idx: usize,
        work: crate::capture::CapturePixelWork,
    ) {
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("compare-pin".into())
            .spawn(move || {
                let result =
                    crate::capture::run_pixel_work(work).map(|(basename, width, height, rgba)| {
                        crate::app::ComparePinResult {
                            basename,
                            width,
                            height,
                            rgba,
                        }
                    });
                let _ = tx.send(result);
            });

        match thread {
            Ok(_) => {
                self.compare_pin_load_pending = None;
                self.compare_prepare_pending = None;
                self.compare_prepared_pair = None;
                self.compare_pin_pending = Some(crate::app::ComparePinPending { source_idx, rx });
                self.show_feedback_toast("比較画像を準備中".to_string());
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(err) => {
                self.show_feedback_toast(format!("比較 worker を開始できません: {err}"));
            }
        }
    }

    pub(crate) fn toggle_compare_pin_from_grid_selection(&mut self, ctx: &egui::Context) {
        let Some(idx) = self.selected else {
            self.show_feedback_toast("比較画像にする画像を選択してください".to_string());
            return;
        };
        if !matches!(
            self.items.get(idx),
            Some(GridItem::Image(_))
                | Some(GridItem::ZipImage { .. })
                | Some(GridItem::PdfPage { .. })
        ) {
            self.show_feedback_toast("このアイテムは比較画像に設定できません".to_string());
            return;
        }

        if self.compare_pin_pending.is_some() {
            self.show_feedback_toast("比較画像を準備中です".to_string());
            return;
        }

        if self
            .pinned_compare_slot
            .as_ref()
            .is_some_and(|slot| slot.source_idx == idx)
        {
            self.toggle_compare_pin_from_current(ctx, idx);
            return;
        }

        let Some(source_key) = self.metadata_cache_key(idx) else {
            self.show_feedback_toast("このアイテムは比較画像に設定できません".to_string());
            return;
        };

        match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { .. }) | Some(FsCacheEntry::Animated { .. }) => {
                self.start_compare_pin_single(ctx, idx);
            }
            Some(FsCacheEntry::Failed) => {
                self.show_feedback_toast("比較画像を読み込めませんでした".to_string());
            }
            _ => {
                if self.compare_pin_pending.is_some() {
                    self.show_feedback_toast("比較画像を準備中です".to_string());
                    return;
                }
                self.compare_pin_load_pending = Some(crate::app::ComparePinLoadPending {
                    source_idx: idx,
                    source_key,
                });
                if !self.fs_pending.contains_key(&idx) {
                    self.start_fs_load(idx);
                }
                self.show_feedback_toast("比較画像を読み込み中".to_string());
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }

    pub(crate) fn toggle_compare_pinned_view(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.pinned_compare_slot.is_none() {
            self.show_feedback_toast("比較画像が未設定です。X で設定してください".to_string());
            return;
        }
        self.compare_view_mode = match self.compare_view_mode {
            crate::app::CompareViewMode::PinnedNormal => crate::app::CompareViewMode::Off,
            crate::app::CompareViewMode::Off => crate::app::CompareViewMode::PinnedNormal,
            crate::app::CompareViewMode::Wipe { .. } => crate::app::CompareViewMode::Off,
            crate::app::CompareViewMode::Diff => crate::app::CompareViewMode::Off,
        };
        self.compare_wipe_dragging = false;
        if matches!(
            self.compare_view_mode,
            crate::app::CompareViewMode::PinnedNormal
        ) {
            self.ensure_compare_prepared_pair(ctx, fs_idx);
        }
        let label = match self.compare_view_mode {
            crate::app::CompareViewMode::PinnedNormal => "[比較: ピン表示]",
            _ => "[比較: 現在表示]",
        };
        self.show_feedback_toast(label.to_string());
    }

    pub(crate) fn toggle_compare_wipe_mode(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.pinned_compare_slot.is_none() {
            self.show_feedback_toast("比較画像が未設定です。X で設定してください".to_string());
            return;
        }
        self.compare_view_mode = match self.compare_view_mode {
            crate::app::CompareViewMode::Wipe { .. } => crate::app::CompareViewMode::Off,
            _ => crate::app::CompareViewMode::Wipe { fraction: 0.5 },
        };
        self.compare_wipe_dragging = false;
        if matches!(
            self.compare_view_mode,
            crate::app::CompareViewMode::Wipe { .. }
        ) {
            self.ensure_compare_prepared_pair(ctx, fs_idx);
        }
        let label = match self.compare_view_mode {
            crate::app::CompareViewMode::Wipe { .. } => "[比較: Wipe]",
            _ => "[比較: Normal]",
        };
        self.show_feedback_toast(label.to_string());
    }

    pub(crate) fn toggle_compare_diff_mode(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.pinned_compare_slot.is_none() {
            self.show_feedback_toast("比較画像が未設定です。X で設定してください".to_string());
            return;
        }
        self.compare_view_mode = match self.compare_view_mode {
            crate::app::CompareViewMode::Diff => crate::app::CompareViewMode::Off,
            _ => crate::app::CompareViewMode::Diff,
        };
        self.compare_wipe_dragging = false;
        if matches!(self.compare_view_mode, crate::app::CompareViewMode::Diff) {
            self.ensure_compare_prepared_pair(ctx, fs_idx);
        }
        let label = match self.compare_view_mode {
            crate::app::CompareViewMode::Diff => "[比較: Diff]",
            _ => "[比較: Normal]",
        };
        self.show_feedback_toast(label.to_string());
    }

    pub(crate) fn save_image_capture_to_file(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.capture_pending.is_some() {
            self.show_feedback_toast("キャプチャ保存中です".to_string());
            return;
        }

        let work = match self.prepare_capture_pixel_work(ctx, fs_idx) {
            Ok(work) => work,
            Err(err) => {
                self.show_feedback_toast(err);
                return;
            }
        };
        let format = self.settings.capture_format;
        let output_dir = self.capture_output_dir_path();
        let jpeg_matte =
            crate::capture::JpegMatte::from_fs_transparent_bg_mode(self.fs_transparent_bg_mode);
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("image-capture-save".into())
            .spawn(move || {
                let result = crate::capture::run_pixel_work(work).and_then(
                    |(basename, width, height, rgba)| {
                        crate::capture::save_rgba_unique_with_matte(
                            &output_dir,
                            &basename,
                            format,
                            jpeg_matte,
                            width,
                            height,
                            &rgba,
                        )
                    },
                );
                let _ = tx.send(result);
            });

        match thread {
            Ok(_) => {
                self.capture_pending = Some(crate::app::CapturePending { rx });
                self.show_feedback_toast(format!("キャプチャを保存中 ({})", format.label()));
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(err) => {
                self.show_feedback_toast(format!("保存 worker を開始できません: {err}"));
            }
        }
    }

    pub(crate) fn copy_image_capture_to_clipboard(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let work = match self.prepare_capture_pixel_work(ctx, fs_idx) {
            Ok(work) => work,
            Err(err) => {
                self.show_feedback_toast(err);
                return;
            }
        };
        let clipboard_seq = crate::ui_dialogs::context_menu::reserve_clipboard_write_sequence();
        std::thread::Builder::new()
            .name("image-capture-clipboard".into())
            .spawn(move || match crate::capture::run_pixel_work(work) {
                Ok((_basename, width, height, rgba)) => {
                    crate::ui_dialogs::context_menu::copy_rgba_image_to_clipboard_async_seq(
                        width,
                        height,
                        rgba,
                        clipboard_seq,
                    );
                }
                Err(err) => {
                    crate::logger::log(format!("image capture clipboard failed: {err}"));
                }
            })
            .ok();
        self.show_feedback_toast("キャプチャをクリップボードへコピー中".to_string());
    }

    fn prepare_capture_pixel_work(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
    ) -> Result<crate::capture::CapturePixelWork, String> {
        match self.resolve_visible_spread_pair(idx) {
            SpreadPair::Single => self
                .prepare_capture_pixel_job(ctx, idx)
                .map(crate::capture::CapturePixelWork::Single),
            SpreadPair::Double { left, right } => {
                let left_job = self.prepare_capture_pixel_job(ctx, left)?;
                let right_job = self.prepare_capture_pixel_job(ctx, right)?;
                let basename = crate::capture::basename_from_text(&format!(
                    "{}_{}",
                    left_job.basename, right_job.basename
                ));
                Ok(crate::capture::CapturePixelWork::Spread {
                    basename,
                    left: left_job,
                    right: right_job,
                })
            }
        }
    }

    fn prepare_capture_pixel_job(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
    ) -> Result<crate::capture::CapturePixelJob, String> {
        let basename = self
            .capture_basename_for_idx(idx)
            .ok_or_else(|| "このアイテムはキャプチャ保存できません".to_string())?;
        // テキスト注釈 (comic) があれば最終 composite に焼き込んでから保存/コピー/比較する。
        // 注釈が無ければ素の final composite。Ctrl+E export (export_page_pixels_for_idx) と
        // 同じ経路にして「表示どおり保存/コピー/比較」する (Codex 監査 P0)。
        let pixels = match self.comic_composited_pixels_for_export(ctx, idx) {
            Some(p) => p,
            None => self
                .ensure_final_composite_pixels(ctx, idx)
                .ok_or_else(|| "最終合成の完了後に再実行してください".to_string())?,
        };
        let size = pixels.size;
        let mut job = crate::capture::CapturePixelJob::already_adjusted(basename, pixels);
        // crop は表示パイプラインでは適用しないので、最終段でここで切り出す。
        if let Some(rect) = self.export_crop_rect_for_pixels(idx, size) {
            job = job.with_crop(rect);
        }
        Ok(job)
    }

    #[allow(dead_code)] // Legacy export variant path; final composite is used in v1.1.0 P1.
    fn capture_job_with_conceal(
        &self,
        idx: usize,
        job: crate::capture::CapturePixelJob,
    ) -> crate::capture::CapturePixelJob {
        let [w, h] = job.source.size;
        let mut job = if let Some(mask) = self.conceal_composite_mask_for_export(idx, w, h) {
            job.with_conceal(Arc::new(mask), self.current_conceal_preset_from_settings())
        } else {
            job
        };
        if let Some(crop) = self.export_crop_for_idx(idx, [w, h]) {
            job = job.with_crop(crop.rect);
        }
        job
    }

    fn capture_basename_for_idx(&self, idx: usize) -> Option<String> {
        let item = self.items.get(idx)?;
        match item {
            GridItem::Image(path) => Some(crate::capture::basename_for_path(path)),
            GridItem::ZipImage {
                zip_path,
                entry_name,
            } => {
                let zip = crate::capture::basename_for_path(zip_path);
                let entry_name = crate::zip_loader::entry_basename(entry_name);
                let entry = std::path::Path::new(entry_name)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(crate::capture::basename_from_text)
                    .unwrap_or_else(|| "entry".to_string());
                Some(format!("{zip}_{entry}"))
            }
            GridItem::PdfPage {
                pdf_path, page_num, ..
            } => {
                let pdf = crate::capture::basename_for_path(pdf_path);
                Some(format!("{pdf}_p{:04}", page_num + 1))
            }
            _ => None,
        }
    }

    pub(crate) fn open_export_dialog_for_current(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.export_pending.is_some() {
            self.show_feedback_toast("エクスポート中です".to_string());
            return;
        }
        if self.is_overlay_edit_mode_active() || self.adjustment_mode || self.analysis_mode {
            self.show_feedback_toast("編集モードを閉じてからエクスポートしてください".to_string());
            return;
        }
        let target = match self.prepare_export_dialog_target(ctx, fs_idx) {
            Ok(target) => target,
            Err(err) => {
                self.show_feedback_toast(err);
                return;
            }
        };

        let output_dir = self
            .settings
            .export_last_directory
            .clone()
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| target.source_dir.clone());
        let original_selection = self.settings.export_batch_selection;
        let mut selection = original_selection;
        let has_conceal_mask = target.pixels.has_conceal_mask();
        for i in 1..selection.len() {
            if !has_conceal_mask || self.settings.conceal_presets[i - 1].is_none() {
                selection[i] = false;
            }
        }
        if !selection.iter().any(|&v| v) {
            selection[0] = true;
        }
        let original_include_metadata = self.settings.export_embed_metadata;

        self.export_dialog = Some(crate::export_dialog::ExportDialogState {
            source: target.source,
            source_label: target.source_label,
            original_format: target.original_format.clone(),
            output_format: crate::export_dialog::ExportFormat::from_source(
                &target.original_format,
                self.settings.export_fallback_format,
            ),
            scale: self.settings.export_default_scale,
            basename: crate::capture::basename_from_text(&format!("{}_edited", target.basename)),
            output_dir_text: output_dir.display().to_string(),
            source_dir: target.source_dir,
            include_metadata: original_include_metadata,
            selection,
            has_conceal_mask,
            pixels: target.pixels,
            original_selection,
            original_include_metadata,
            initial_focus_done: false,
            error: None,
        });
        ctx.request_repaint();
    }

    fn prepare_export_dialog_target(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) -> Result<ExportDialogTarget, String> {
        match self.resolve_visible_spread_pair(fs_idx) {
            SpreadPair::Single => self.prepare_single_export_dialog_target(ctx, fs_idx),
            SpreadPair::Double { left, right } => {
                self.prepare_spread_export_dialog_target(ctx, left, right)
            }
        }
    }

    fn prepare_single_export_dialog_target(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
    ) -> Result<ExportDialogTarget, String> {
        self.ensure_export_erase_ready(ctx, &[idx])?;
        self.ensure_export_local_adjust_ready(ctx, &[idx])?;
        let (source, source_label, original_format, source_dir, basename) =
            self.export_source_info_for_idx(idx)?;
        let pixels = self.export_page_pixels_for_idx(ctx, idx)?;
        Ok(ExportDialogTarget {
            source,
            source_label,
            original_format,
            source_dir,
            basename,
            pixels: crate::export_dialog::ExportPixels::Single(pixels),
        })
    }

    fn prepare_spread_export_dialog_target(
        &mut self,
        ctx: &egui::Context,
        left_idx: usize,
        right_idx: usize,
    ) -> Result<ExportDialogTarget, String> {
        self.ensure_export_erase_ready(ctx, &[left_idx, right_idx])?;
        self.ensure_export_local_adjust_ready(ctx, &[left_idx, right_idx])?;
        let (_, left_label, _, left_dir, left_basename) =
            self.export_source_info_for_idx(left_idx)?;
        let (_, right_label, _, _, right_basename) = self.export_source_info_for_idx(right_idx)?;
        let left = self.export_page_pixels_for_idx(ctx, left_idx)?;
        let right = self.export_page_pixels_for_idx(ctx, right_idx)?;
        let basename =
            crate::capture::basename_from_text(&format!("{left_basename}_{right_basename}"));
        Ok(ExportDialogTarget {
            source: crate::export_dialog::ExportSource::RenderedSpread,
            source_label: format!("見開き: {left_label} / {right_label}"),
            original_format: crate::save_with_metadata::SrcFormat::Other("spread".to_string()),
            source_dir: left_dir,
            basename,
            pixels: crate::export_dialog::ExportPixels::Spread { left, right },
        })
    }

    fn ensure_export_erase_ready(
        &mut self,
        ctx: &egui::Context,
        indices: &[usize],
    ) -> Result<(), String> {
        // 消しゴム commit (MI-GAN inpaint) が進行中だと erase_result_cache がまだ
        // 空で、resolve_export_base_pixels が pre-erase の adjustment_cache / fs_cache
        // へフォールバックしてしまい「消したつもりの結果が反映されない export」になる。
        if self.erase_inpaint_pending.keys().any(|k| {
            indices.contains(&k.idx) && matches!(k.kind, crate::ui_erase::EraseInpaintKind::Commit)
        }) {
            return Err("消しゴム補完の完了後にエクスポートしてください".to_string());
        }

        // 保存済みマスクがあるのに erase_result_cache が空のままだと、export は
        // pre-erase pixels を書き出してしまう。ensure_erase_result_texture をここで
        // 呼んで commit を再投入し、結果が揃うまで Ctrl+E を保留する。
        for &idx in indices {
            if self.mask_pages.contains(&idx) {
                let _ = self.ensure_erase_result_texture(ctx, idx);
                if self.current_erase_result_pixels(idx).is_none() {
                    return Err(
                        "消しゴム補完の準備中です。少し待ってから Ctrl+E を再実行してください"
                            .to_string(),
                    );
                }
            }
        }
        Ok(())
    }

    fn ensure_export_local_adjust_ready(
        &mut self,
        ctx: &egui::Context,
        indices: &[usize],
    ) -> Result<(), String> {
        for &idx in indices {
            if self.has_active_local_adjust_layers(idx) {
                self.maybe_start_local_adjust_render(idx);
                if self.current_local_adjust_pixels(idx).is_none() {
                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                    return Err(
                        "補正レイヤーの反映中です。少し待ってから Ctrl+E を再実行してください"
                            .to_string(),
                    );
                }
            }
        }
        Ok(())
    }

    fn export_page_pixels_for_idx(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
    ) -> Result<crate::export_dialog::ExportPagePixels, String> {
        // 注釈 (comic) があれば最終 composite に焼き込んでから export する (最前面 D1)。
        // 注釈が無ければ素の final composite。フルスクリーン Ctrl+E は conceal_mask=None
        // なので、ここで焼いた注釈が worker の conceal preset 合成に潰されない (Inc 7)。
        let base_pixels = match self.comic_composited_pixels_for_export(ctx, idx) {
            Some(p) => p,
            None => self
                .ensure_final_composite_pixels(ctx, idx)
                .ok_or_else(|| "最終合成の完了後にエクスポートしてください".to_string())?,
        };
        // crop は表示には反映しないので、export の最終段で base_pixels (= final
        // composite。AI アップスケールで source とサイズが違いうる) に対して切り出す。
        let crop = self.export_crop_rect_for_pixels(idx, base_pixels.size);
        Ok(crate::export_dialog::ExportPagePixels {
            base_pixels,
            conceal_mask: None,
            crop,
        })
    }

    pub(crate) fn draw_export_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.export_dialog.take() else {
            return;
        };

        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let preset_slots = self.settings.conceal_presets.clone();
        let has_conceal_mask = state.has_conceal_mask;
        for i in 1..state.selection.len() {
            if !has_conceal_mask || preset_slots[i - 1].is_none() {
                state.selection[i] = false;
            }
        }
        let selected_count = state.selection.iter().filter(|&&v| v).count();
        let basename_ok = !crate::capture::basename_from_text(&state.basename)
            .trim()
            .is_empty();
        let can_start =
            selected_count > 0 && basename_ok && !state.output_dir_text.trim().is_empty();
        let mut open = true;
        let mut canceled = false;
        let mut start = false;
        let mut pick_folder = false;

        // CLAUDE.md「ダイアログ (egui::Window)」: anchor() はドラッグ不可になるため
        // 必ず default_pos() を使う。
        let default_pos = ctx.content_rect().center() - egui::vec2(230.0, 200.0);
        egui::Window::new("エクスポート")
            .collapsible(false)
            .resizable(false)
            .default_pos(default_pos)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_min_width(460.0);
                ui.label(&state.source_label);
                ui.add_space(6.0);
                ui.label("ファイル名");
                let mut basename_output = egui::TextEdit::singleline(&mut state.basename)
                    .desired_width(f32::INFINITY)
                    .hint_text("出力ファイル名")
                    .show(ui);
                crate::ui_helpers::singleline_text_edit_context_menu(
                    ui,
                    &mut basename_output,
                    &mut state.basename,
                );
                let basename_resp = basename_output.response;
                // 初回フレームのみ basename にフォーカス。毎フレーム request_focus
                // すると他フィールドへフォーカス移動が直ちに巻き戻される
                // (Codex review CONFIRMED)。
                if !state.initial_focus_done {
                    basename_resp.request_focus();
                    state.initial_focus_done = true;
                }
                ui.add_space(6.0);
                ui.label("保存先");
                ui.horizontal(|ui| {
                    let buttons_width = 144.0;
                    let edit_width = (ui.available_width() - buttons_width).max(180.0);
                    let mut output_dir_output =
                        egui::TextEdit::singleline(&mut state.output_dir_text)
                            .desired_width(edit_width)
                            .hint_text("保存先フォルダ")
                            .show(ui);
                    crate::ui_helpers::singleline_text_edit_context_menu(
                        ui,
                        &mut output_dir_output,
                        &mut state.output_dir_text,
                    );
                    if ui.button("変更...").clicked() {
                        pick_folder = true;
                    }
                    if ui
                        .button("元の場所")
                        .on_hover_text("元ファイルのあるフォルダを保存先にします")
                        .clicked()
                    {
                        state.reset_output_dir_to_source_dir();
                    }
                });
                ui.add_space(6.0);
                egui::ComboBox::from_label("形式")
                    .selected_text(state.output_format.label())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut state.output_format,
                            crate::export_dialog::ExportFormat::Jpeg95,
                            "JPEG 95",
                        );
                        ui.selectable_value(
                            &mut state.output_format,
                            crate::export_dialog::ExportFormat::Png,
                            "PNG",
                        );
                        if !matches!(
                            state.original_format,
                            crate::save_with_metadata::SrcFormat::Other(_)
                        ) {
                            ui.selectable_value(
                                &mut state.output_format,
                                crate::export_dialog::ExportFormat::Webp,
                                "WebP",
                            );
                        }
                    });

                let metadata_possible = state.original_format.supports_metadata_writeback()
                    && state.original_format == state.output_format.to_src_format();
                // state.include_metadata はユーザーの意図 (チェックボックス操作の結果)
                // を保持する。format 不一致でも値は force-flip しない — 形式を JPEG→PNG→
                // JPEG と切り替えても元のチェック状態が戻る (Codex review CONFIRMED)。
                // 実際の保存時の判定は run_export が `original_format == output_format`
                // を AND して行う。
                ui.add_enabled(
                    metadata_possible,
                    egui::Checkbox::new(&mut state.include_metadata, "AI プロンプト / EXIF を保持"),
                );
                if !metadata_possible {
                    ui.small("形式変換、PDF、見開き合成ではメタデータ保持は無効です");
                }

                ui.add_space(6.0);
                ui.label("出力サイズ");
                let base_size = state.pixels.render_size();
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    for scale in crate::export_dialog::ExportScale::FIXED {
                        let [w, h] = scale.scaled_size(base_size);
                        ui.radio_value(
                            &mut state.scale,
                            scale,
                            format!("{} ({}×{})", scale.label(), w, h),
                        );
                    }
                    // 長辺 px 指定 (AI アップスケールの巨大サイズを一定上限へ収める用途)。
                    let mut long_edge_px = match state.scale {
                        crate::export_dialog::ExportScale::LongEdge(px) => px,
                        _ => crate::export_dialog::ExportScale::DEFAULT_LONG_EDGE,
                    };
                    let is_long_edge =
                        matches!(state.scale, crate::export_dialog::ExportScale::LongEdge(_));
                    let [lw, lh] = crate::export_dialog::ExportScale::LongEdge(long_edge_px)
                        .scaled_size(base_size);
                    ui.horizontal(|ui| {
                        if ui
                            .radio(is_long_edge, format!("長辺指定 ({lw}×{lh})"))
                            .clicked()
                        {
                            state.scale = crate::export_dialog::ExportScale::LongEdge(long_edge_px);
                        }
                        let resp = ui.add(
                            egui::DragValue::new(&mut long_edge_px)
                                .range(
                                    crate::export_dialog::ExportScale::LONG_EDGE_MIN
                                        ..=crate::export_dialog::ExportScale::LONG_EDGE_MAX,
                                )
                                .suffix("px"),
                        );
                        if resp.changed() {
                            state.scale = crate::export_dialog::ExportScale::LongEdge(long_edge_px);
                        }
                    });
                });

                ui.separator();
                ui.label("出力するバリエーション");
                ui.checkbox(&mut state.selection[0], "現在の設定 (_0)");
                for (slot_idx, slot) in preset_slots.iter().enumerate() {
                    let enabled = has_conceal_mask && slot.is_some();
                    let label = match slot {
                        Some(preset) if !preset.name.trim().is_empty() => {
                            format!(
                                "プリセット{}: {} (_{})",
                                slot_idx + 1,
                                preset.name,
                                slot_idx + 1
                            )
                        }
                        Some(_) => format!("プリセット{} (_{})", slot_idx + 1, slot_idx + 1),
                        None => format!("プリセット{}: 空", slot_idx + 1),
                    };
                    ui.add_enabled_ui(enabled, |ui| {
                        ui.checkbox(&mut state.selection[slot_idx + 1], label);
                    });
                }
                if !has_conceal_mask {
                    ui.small("プリセット出力は隠蔽マスクがある画像で有効です");
                }

                if let Some(err) = &state.error {
                    ui.add_space(6.0);
                    ui.colored_label(egui::Color32::from_rgb(255, 140, 140), err);
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(can_start, egui::Button::new("保存"))
                        .clicked()
                    {
                        start = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        canceled = true;
                    }
                    ui.small(format!("{selected_count} 件"));
                });
            });

        if pick_folder {
            let start_dir = std::path::PathBuf::from(state.output_dir_text.trim());
            if let Some(dir) = rfd::FileDialog::new()
                .set_directory(start_dir)
                .pick_folder()
            {
                state.output_dir_text = dir.display().to_string();
            }
        }

        if escape_pressed || !open || canceled {
            self.export_dialog = None;
            return;
        }

        if enter_pressed && can_start {
            start = true;
        }
        if start {
            match self.start_export_from_dialog(ctx, state.clone()) {
                Ok(()) => return,
                Err(err) => {
                    state.error = Some(err);
                    self.export_dialog = Some(state);
                    return;
                }
            }
        }

        self.export_dialog = Some(state);
    }

    pub(crate) fn draw_export_progress_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.export_pending.as_mut() else {
            return;
        };
        let mut dismiss_finished = false;
        let mut request_cancel = false;
        let progress = if pending.total == 0 {
            0.0
        } else {
            pending.done as f32 / pending.total as f32
        };

        // CLAUDE.md「ダイアログ (egui::Window)」: anchor() はドラッグ不可になるため
        // 必ず default_pos() を使う。
        let progress_default_pos = ctx.content_rect().center() - egui::vec2(190.0, 100.0);
        egui::Window::new("エクスポート進行状況")
            .collapsible(false)
            .resizable(false)
            .default_pos(progress_default_pos)
            .show(ctx, |ui| {
                ui.set_min_width(380.0);
                ui.label(&pending.last_message);
                ui.add(
                    egui::ProgressBar::new(progress)
                        .show_percentage()
                        .text(format!("{} / {}", pending.done, pending.total)),
                );
                if !pending.errors.is_empty() {
                    ui.add_space(8.0);
                    ui.colored_label(
                        egui::Color32::from_rgb(255, 140, 140),
                        format!("{} 件のエラー", pending.errors.len()),
                    );
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .show(ui, |ui| {
                            for err in &pending.errors {
                                ui.small(format!("{}: {}", err.label, err.message));
                            }
                        });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if pending.finished {
                        if ui.button("閉じる").clicked() {
                            dismiss_finished = true;
                        }
                    } else if pending.cancel_requested {
                        ui.add_enabled(false, egui::Button::new("キャンセル中..."));
                    } else if ui.button("キャンセル").clicked() {
                        request_cancel = true;
                    }
                });
            });

        if request_cancel {
            pending.cancel_requested = true;
            pending
                .cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
            pending.last_message = "キャンセル中...".to_string();
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if dismiss_finished {
            self.export_pending = None;
        }
    }

    pub(crate) fn poll_export_pending(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.export_pending.as_mut() else {
            return;
        };
        let mut clear_pending = false;
        let mut toast: Option<String> = None;
        let mut reveal_path: Option<std::path::PathBuf> = None;
        // batch 完了時に 1 度だけフォルダ refresh を発火するため、success path を集約。
        // 旧版は Completed ごとに note_exported_file_for_folder_refresh を呼んで
        // N エントリで N 回 load_folder が走り、UI が thrash していた
        // (Codex review CONFIRMED)。
        let mut refresh_after_done = false;

        loop {
            match pending.rx.try_recv() {
                Ok(crate::export_dialog::ExportEvent::Started { label }) => {
                    pending.last_message = format!("{label} を保存中");
                }
                Ok(crate::export_dialog::ExportEvent::Completed(success)) => {
                    pending.done = pending.done.saturating_add(1);
                    pending.last_message = format!("保存しました: {}", success.label);
                    pending.successes.push(success);
                }
                Ok(crate::export_dialog::ExportEvent::Failed(err)) => {
                    pending.done = pending.done.saturating_add(1);
                    pending.last_message = format!("保存に失敗: {}", err.label);
                    pending.errors.push(err);
                }
                Ok(crate::export_dialog::ExportEvent::Cancelled) => {
                    toast = Some("エクスポートをキャンセルしました".to_string());
                    // キャンセル時も既に書き出されたファイルがあれば refresh
                    if !pending.successes.is_empty() {
                        refresh_after_done = true;
                    }
                    clear_pending = true;
                    break;
                }
                Ok(crate::export_dialog::ExportEvent::AllDone) => {
                    pending.finished = true;
                    // batch 完了時にまとめて 1 回 refresh
                    if !pending.successes.is_empty() {
                        refresh_after_done = true;
                    }
                    if pending.errors.is_empty() {
                        if pending.successes.len() == 1 {
                            reveal_path = pending.successes.first().map(|s| s.path.clone());
                        } else {
                            toast = Some(format!(
                                "{} 件をエクスポートしました",
                                pending.successes.len()
                            ));
                        }
                        clear_pending = true;
                    } else {
                        pending.last_message = format!(
                            "完了: 成功 {} / 失敗 {}",
                            pending.successes.len(),
                            pending.errors.len()
                        );
                    }
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // AllDone を既に受け取り済み (= worker は正常終了) の場合は、
                    // ここで「中断されました」toast を出すと AllDone エラー集計の
                    // 進捗ダイアログを 1 フレで上書きしてしまう。
                    // finished が立っていればダイアログ表示を維持する。
                    if !pending.finished {
                        toast = Some("エクスポート worker が中断されました".to_string());
                        clear_pending = true;
                    }
                    break;
                }
            }
        }

        // 完了 (AllDone / Cancelled) で書き出されたファイルの parent をまとめて 1 件
        // pending に積む。複数 success path はすべて同じ output_dir なので、最初の
        // 成功 path だけを渡せば十分。
        let first_success_path = if refresh_after_done {
            self.export_pending
                .as_ref()
                .and_then(|p| p.successes.first().map(|s| s.path.clone()))
        } else {
            None
        };

        if clear_pending {
            self.export_pending = None;
        } else if self.export_pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if let Some(path) = first_success_path {
            self.note_exported_file_for_folder_refresh(&path);
        }
        if let Some(path) = reveal_path {
            self.show_capture_saved_toast(path);
        }
        if let Some(message) = toast {
            self.show_feedback_toast(message);
        }
    }

    fn start_export_from_dialog(
        &mut self,
        ctx: &egui::Context,
        state: crate::export_dialog::ExportDialogState,
    ) -> Result<(), String> {
        if self.export_pending.is_some() {
            return Err("エクスポート中です".to_string());
        }
        // ダイアログを閉じる前に fullscreen を抜けていた場合は保護的に reject
        // (Codex review CONFIRMED — close_fullscreen は dialog を残しがち)。
        if self.fullscreen_idx.is_none() {
            return Err("フルスクリーン表示中に実行してください".to_string());
        }
        let output_dir = std::path::PathBuf::from(state.output_dir_text.trim());
        if output_dir.as_os_str().is_empty() {
            return Err("保存先フォルダを指定してください".to_string());
        }
        let basename = crate::capture::basename_from_text(&state.basename);

        // ダイアログを開いた瞬間に snapshot した pixels / conceal mask を使う。
        // ここで再 resolve すると animated GIF の current_frame が進んで違うフレーム
        // が export される (Codex review CONFIRMED)。
        let pixels = state.pixels.clone();
        let has_conceal_mask = pixels.has_conceal_mask();
        let mut planned: Vec<(String, u8, Option<crate::conceal::ConcealPreset>)> = Vec::new();
        if state.selection[0] {
            let preset = has_conceal_mask.then(|| self.current_conceal_preset_from_settings());
            planned.push(("現在の設定".to_string(), 0, preset));
        }
        for slot_idx in 0..4 {
            if state.selection[slot_idx + 1] {
                if !has_conceal_mask {
                    continue;
                }
                let Some(preset) = self.settings.conceal_presets[slot_idx].clone() else {
                    continue;
                };
                let label = if preset.name.trim().is_empty() {
                    format!("プリセット{}", slot_idx + 1)
                } else {
                    format!("プリセット{}: {}", slot_idx + 1, preset.name)
                };
                planned.push((label, (slot_idx + 1) as u8, Some(preset)));
            }
        }
        if planned.is_empty() {
            return Err("出力するバリエーションを選んでください".to_string());
        }

        let suffixes: Vec<u8> = planned.iter().map(|(_, suffix, _)| *suffix).collect();
        let resolved_basename = crate::export_dialog::resolve_session_basename(
            &output_dir,
            &basename,
            state.output_format.extension(),
            &suffixes,
        )?;

        let mut entries = Vec::with_capacity(planned.len());
        for (label, suffix, preset) in planned {
            entries.push(crate::export_dialog::ExportEntry {
                label,
                suffix,
                conceal_preset: preset,
            });
        }

        // 永続化する selection は **ユーザーが実際にダイアログで触った値** を残す。
        // 環境要因 (has_conceal_mask=false / preset slot 空) で planned から落ちた
        // エントリは「ユーザーが unchecked にした」と区別したいので、original_selection
        // を底辺にして「現在も checked」な slot だけ反映する (Codex review CONFIRMED)。
        let mut next_selection = state.original_selection;
        // user が今のセッションで checked にした slot は反映する
        for i in 0..state.selection.len() {
            if state.selection[i] {
                next_selection[i] = true;
            }
        }
        // user が明示的に unchecked にした slot を反映 (= state.selection が false で
        // かつ環境要因で disable されていなかった slot のみ)。
        for i in 0..state.selection.len() {
            let env_disabled =
                i > 0 && (!has_conceal_mask || self.settings.conceal_presets[i - 1].is_none());
            if !state.selection[i] && !env_disabled {
                next_selection[i] = false;
            }
        }
        // metadata は元値ベースで、ユーザーが意図的に切り替えた場合のみ反映する。
        // format 切替の force-flip を考慮し、metadata_possible=true のときの値だけを
        // 信頼する。
        let metadata_possible = state.original_format.supports_metadata_writeback()
            && state.original_format == state.output_format.to_src_format();
        let persisted_metadata = if metadata_possible {
            state.include_metadata
        } else {
            state.original_include_metadata
        };
        self.settings.export_embed_metadata = persisted_metadata;
        self.settings.export_last_directory = Some(output_dir.clone());
        self.settings.export_batch_selection = next_selection;
        self.settings.export_default_scale = state.scale;
        if matches!(
            state.original_format,
            crate::save_with_metadata::SrcFormat::Other(_)
        ) && let Some(fallback) = state.output_format.fallback_format()
        {
            self.settings.export_fallback_format = fallback;
        }
        self.settings.save();

        // export 中の中間ファイル増分による多重 load_folder を防ぐため、batch 完了
        // (AllDone) の単一トリガで refresh する。中間 Completed イベントでの
        // note_exported_file_for_folder_refresh は呼ばない (Codex review CONFIRMED)。
        let effective_include_metadata = state.include_metadata && metadata_possible;
        let request = crate::export_dialog::ExportRequest {
            source: state.source,
            original_format: state.original_format,
            output_format: state.output_format,
            output_dir,
            basename: resolved_basename,
            pixels,
            scale: state.scale,
            entries,
            include_metadata: effective_include_metadata,
        };
        let pending = crate::export_dialog::spawn_export_worker(request)?;
        self.export_pending = Some(pending);
        self.export_dialog = None;
        self.show_feedback_toast("エクスポートを開始しました".to_string());
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        Ok(())
    }

    fn export_source_info_for_idx(
        &self,
        idx: usize,
    ) -> Result<
        (
            crate::export_dialog::ExportSource,
            String,
            crate::save_with_metadata::SrcFormat,
            std::path::PathBuf,
            String,
        ),
        String,
    > {
        match self.items.get(idx) {
            Some(GridItem::Image(path)) => {
                let dir = path
                    .parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                let format = crate::save_with_metadata::SrcFormat::from_path(path);
                let basename = crate::capture::basename_for_path(path);
                Ok((
                    crate::export_dialog::ExportSource::File { path: path.clone() },
                    format!("元画像: {}", path.display()),
                    format,
                    dir,
                    basename,
                ))
            }
            Some(GridItem::ZipImage {
                zip_path,
                entry_name,
            }) => {
                let dir = zip_path
                    .parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                let ext = std::path::Path::new(entry_name)
                    .extension()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                let format = crate::save_with_metadata::SrcFormat::from_ext(ext);
                let zip = crate::capture::basename_for_path(zip_path);
                let entry_name_base = crate::zip_loader::entry_basename(entry_name);
                let entry = std::path::Path::new(entry_name_base)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(crate::capture::basename_from_text)
                    .unwrap_or_else(|| "entry".to_string());
                Ok((
                    crate::export_dialog::ExportSource::ZipEntry {
                        zip_path: zip_path.clone(),
                        entry_name: entry_name.clone(),
                    },
                    format!("ZIP: {} > {}", zip_path.display(), entry_name),
                    format,
                    dir,
                    format!("{zip}_{entry}"),
                ))
            }
            Some(GridItem::PdfPage {
                pdf_path, page_num, ..
            }) => {
                let dir = pdf_path
                    .parent()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
                let pdf = crate::capture::basename_for_path(pdf_path);
                Ok((
                    crate::export_dialog::ExportSource::PdfPage,
                    format!("PDF: {} / page {}", pdf_path.display(), page_num + 1),
                    crate::save_with_metadata::SrcFormat::Other("pdf".to_string()),
                    dir,
                    format!("{pdf}_p{:04}", page_num + 1),
                ))
            }
            _ => Err("このアイテムはエクスポートできません".to_string()),
        }
    }

    #[allow(dead_code)] // Legacy export variant path; final composite is used in v1.1.0 P1.
    fn resolve_export_base_pixels(&self, idx: usize) -> Result<Arc<egui::ColorImage>, String> {
        if let Some(pixels) = self.current_local_adjust_pixels(idx) {
            return Ok(pixels);
        }
        if self.mask_pages.contains(&idx) && self.current_erase_result_pixels(idx).is_none() {
            return Err("消しゴム補完の完了後にエクスポートしてください".to_string());
        }
        if self.has_active_local_adjust_layers(idx) {
            return Err("補正レイヤーの反映完了後にエクスポートしてください".to_string());
        }

        if let Some(pixels) = self.current_erase_result_pixels(idx) {
            return Ok(pixels);
        }
        if !self.post_filter_bypassed
            && let Some(FsCacheEntry::Static { pixels, .. }) = self.adjustment_cache.get(&idx)
        {
            return Ok(Arc::clone(pixels));
        }

        let bg = self.effective_upscale_bg_mode();
        if self.ai_will_apply_to(idx)
            && let Some(FsCacheEntry::Static { pixels, .. }) = self.ai_upscale_cache.get(&(idx, bg))
        {
            return Ok(Arc::clone(pixels));
        }

        match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { pixels, .. }) => Ok(Arc::clone(pixels)),
            Some(FsCacheEntry::Animated {
                frame_pixels,
                current_frame,
                ..
            }) => frame_pixels
                .get(*current_frame)
                .cloned()
                .ok_or_else(|| "アニメーションフレームを取得できません".to_string()),
            _ => Err("画像の読み込み完了後にエクスポートしてください".to_string()),
        }
    }

    #[allow(dead_code)] // Legacy export variant path; final composite is used in v1.1.0 P1.
    fn conceal_composite_mask_for_export(
        &self,
        idx: usize,
        w: usize,
        h: usize,
    ) -> Option<Vec<bool>> {
        if !self.conceal_pages.contains(&idx) {
            return None;
        }
        let key = self.page_path_key(idx)?;
        let db = self.conceal_db.as_ref()?;
        let (mut bitmap, shapes) = db.get_full(&key, w, h)?;
        crate::mask_db::rasterize_shapes_into(&mut bitmap, &shapes, w, h);
        if bitmap.iter().any(|&b| b) {
            Some(bitmap)
        } else {
            None
        }
    }

    pub(crate) fn current_conceal_preset_from_settings(&self) -> crate::conceal::ConcealPreset {
        crate::conceal::ConcealPreset {
            name: "現在の設定".to_string(),
            conceal_type: self.settings.conceal_type,
            mosaic_tile_mode: self.settings.conceal_mosaic_tile_mode,
            mosaic_boundary: self.settings.conceal_mosaic_boundary,
            fill_opacity_percent: self.settings.conceal_fill_opacity_percent,
            fill_edge: self.settings.conceal_fill_edge,
            blur_radius_px: self.settings.conceal_blur_radius_px,
            blur_mode: self.settings.conceal_blur_mode,
            blur_feather: self.settings.conceal_blur_feather,
        }
    }

    /// 動画のチャプター開始 / ブックマーク / ピンを 1 本の Vec に集約し pts 昇順で返す。
    /// シークバー描画 (マーカー縦線) と J/K ジャンプの両方が同じソースを使う。
    pub(crate) fn collect_video_nav_markers(&mut self, fs_idx: usize) -> Vec<NavMarker> {
        self.ensure_fullscreen_video_marker_cache(fs_idx);
        let path = match self.fs_video_player(fs_idx) {
            Some(p) => p.path().clone(),
            None => return Vec::new(),
        };
        let mut markers: Vec<NavMarker> = Vec::new();
        if let Some(info) = self.fs_video_player(fs_idx).and_then(|p| p.info()) {
            for c in &info.chapters {
                markers.push(NavMarker {
                    pts: c.start_secs,
                    kind: NavMarkerKind::Chapter,
                    title: c.title.clone(),
                });
            }
        }
        let (pin_pts, bookmarks) = self.fullscreen_video_marker_snapshot(fs_idx, &path);
        for bookmark in bookmarks {
            markers.push(NavMarker {
                pts: bookmark.pts_secs,
                kind: NavMarkerKind::Bookmark,
                title: bookmark.title,
            });
        }
        if let Some(pts) = pin_pts {
            markers.push(NavMarker {
                pts,
                kind: NavMarkerKind::Pin,
                title: None,
            });
        }
        markers.sort_by(|a, b| {
            a.pts
                .partial_cmp(&b.pts)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        markers
    }

    /// 動画再生時のキー入力を処理する。フルスクリーン中で `state.is_video` のときに
    /// 1 フレームに 1 回呼ぶ。
    pub(crate) fn handle_video_input(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        video_path: Option<&std::path::Path>,
    ) {
        // IME 変換中はショートカットを発火させない
        if self.ime_input_active() {
            return;
        }

        // F11: ウィンドウ / 全画面 切り替え (HUD トグルボタンと同じ動作)。
        //
        // 動画フルスクリーンでも main mIV ウィンドウにフォーカスがあるケースが
        // 存在する: in-window 動画モード、起動直後の focus handoff、
        // フルスクリーン用コンテキストメニュー閉鎖直後など。これらでは F11 が
        // native HWND ではなく egui の root viewport に届く (Codex P1 2026-05)。
        //
        // 純粋な item 種別 check で動画と判定したフレームで F11 を捕まえ、
        // 仮想 VK 0x7A を作って native HWND 経路と同じ
        // `handle_native_video_key_event` の 0x7A arm に流す。これにより
        // normalize scan ガード、`toggle_video_window_mode` (presenter rebuild
        // request) など全てのロジックを一本化できる。
        //
        // consume_key は repeat 込み + matches_logically で余分な Shift も拾うため、
        // 厳格に「修飾なし・非 repeat」だけ抜き出す custom event filter を使う。
        #[cfg(windows)]
        {
            let current_is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
            if current_is_video {
                let f11_pressed = ctx.input_mut(|i| {
                    let mut found = false;
                    i.events.retain(|e| {
                        let consume = matches!(
                            e,
                            egui::Event::Key {
                                key: egui::Key::F11,
                                pressed: true,
                                repeat: false,
                                modifiers,
                                ..
                            } if modifiers.is_none()
                        );
                        if consume {
                            found = true;
                        }
                        !consume
                    });
                    found
                });
                if f11_pressed {
                    let synthetic = crate::video::native_window::NativeVideoKeyEvent {
                        virtual_key: 0x7A,
                        shift: false,
                        ctrl: false,
                        alt: false,
                        repeat: false,
                    };
                    self.handle_native_video_key_event(ctx, fs_idx, synthetic);
                }
            }
        }

        // 動画モードのキー処理: 動画 HUD 2 段化リデザイン (Phase 1) で Space を再生/停止
        // トグルに追加。Enter / Shift+Enter は既存どおり (Enter = 再生/停止、Shift+Enter = 外部
        // プレイヤー)。egui の `consume_key` は修飾子マッチが厳密 (Caps Lock + Shift などで
        // 取りこぼす) ので、`modifiers.shift` を見た fallback も併用する。
        let shift_enter = self
            .keymap
            .consume_action(ctx, KeyAction::VideoExternalPlayer);
        if shift_enter {
            crate::logger::log("video Shift+Enter pressed → external player".to_string());
        }
        // 再生 / 一時停止トグル。Shift+Enter は上で先に取っているので残らない。
        let play_pause = self.keymap.consume_action(ctx, KeyAction::VideoPlayPause);
        // Phase 7.H シーク粒度: ←→=5 秒、Shift+←→=1 秒、Ctrl+←→=30 秒。
        // タイル中は seek せずカーソル移動に切り替える。Ctrl 併用時だけ 1 行分移動。
        // ↑↓ は consume せず後段の image arrow_up/down (= 前後ファイル) に流す
        // (= マウスホイールと整合)。Shift+↑↓ だけ動画モードで音量に使う。
        let tile_active_for_keyboard = self.video_tile_mode_active;
        let tile_left_ctrl = if tile_active_for_keyboard {
            ctx.input_mut(|i| {
                if i.consume_key(
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                    egui::Key::ArrowLeft,
                ) {
                    Some(true)
                } else if i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowLeft) {
                    Some(true)
                } else if i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowLeft) {
                    Some(false)
                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft) {
                    Some(false)
                } else {
                    None
                }
            })
        } else {
            None
        };
        let tile_right_ctrl = if tile_active_for_keyboard {
            ctx.input_mut(|i| {
                if i.consume_key(
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                    egui::Key::ArrowRight,
                ) {
                    Some(true)
                } else if i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowRight) {
                    Some(true)
                } else if i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowRight) {
                    Some(false)
                } else if i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight) {
                    Some(false)
                } else {
                    None
                }
            })
        } else {
            None
        };
        let tile_left = tile_left_ctrl.is_some();
        let ctrl_shift_left = tile_left_ctrl.is_none()
            && tile_right_ctrl.is_none()
            && ctx.input_mut(|i| {
                i.consume_key(
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                    egui::Key::ArrowLeft,
                )
            });
        let ctrl_shift_right = tile_left_ctrl.is_none()
            && tile_right_ctrl.is_none()
            && ctx.input_mut(|i| {
                i.consume_key(
                    egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                    egui::Key::ArrowRight,
                )
            });
        let frame_step_key = ctrl_shift_left || ctrl_shift_right;
        let ctrl_shift_held_now = ctx.input(|i| i.modifiers.ctrl && i.modifiers.shift);
        let shift_left = !frame_step_key
            && !ctrl_shift_held_now
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowLeft));
        let shift_right = !frame_step_key
            && !ctrl_shift_held_now
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::SHIFT, egui::Key::ArrowRight));
        let ctrl_left = !frame_step_key
            && !ctrl_shift_held_now
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowLeft));
        let ctrl_right = !frame_step_key
            && !ctrl_shift_held_now
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::ArrowRight));
        let left = !frame_step_key
            && !ctrl_shift_held_now
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft));
        let right = !frame_step_key
            && !ctrl_shift_held_now
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight));
        let shift_up = self.keymap.consume_action(ctx, KeyAction::VideoVolumeUp);
        let shift_down = self.keymap.consume_action(ctx, KeyAction::VideoVolumeDown);
        // ↑↓ プレーンは consume しない (= image handler が file navigation に使う)。
        let mute_key = self.keymap.consume_action(ctx, KeyAction::VideoMute);
        let loop_key = self.keymap.consume_action(ctx, KeyAction::VideoLoop);
        // Phase 5.4.1: B キーで現在位置にブックマーク追加 (動画モード限定)。
        // 画像モードの B (透過背景循環) とは handle_video_input 先行 consume で分離。
        let bookmark_key = self.keymap.consume_action(ctx, KeyAction::VideoBookmark);
        let save_frame_key = self.keymap.consume_action(ctx, KeyAction::VideoCapture);
        // Phase 5.5: S キーでタイルモード トグル (動画モード限定)。画像モードの
        // S (スライドショー) とは handle_video_input 先行 consume で分離する。
        let tile_key = self.keymap.consume_action(ctx, KeyAction::VideoTileMode);
        // F キーでフレームレート / Perf オーバーレイのトグル (動画モード限定)。
        // 以前 P を使っていたが、P は「現在フレームをピン留め」に再割り当てしたので
        // 移動した (F = Frames / FPS の mnemonic)。画像モードの F は未使用なので
        // 競合しない。
        let perf_key = self.keymap.consume_action(ctx, KeyAction::VideoPerfOverlay);
        // P キーで現在再生位置をピン留め (動画モード限定)。グリッドモードの P
        // (folder_thumb_pin toggle) と統一した「P = Pin」の mnemonic。画像モードの
        // ポストフィルタは T に移動済み。
        let pin_key = self.keymap.consume_action(ctx, KeyAction::VideoPin);
        // 比較ビューは静止画 / ZIP / PDF 限定。動画では passthrough させず silent no-op として消費する。
        let compare_x = self
            .keymap
            .consume_action(ctx, KeyAction::VideoCompareToggle);
        let compare_alt_c = self.keymap.consume_action(ctx, KeyAction::VideoCompareDiff);
        let compare_shift_c = self.keymap.consume_action(ctx, KeyAction::VideoCompareWipe);
        let compare_c = self
            .keymap
            .consume_action(ctx, KeyAction::VideoCompareCycle);
        // W キー: 頭出し (= seek to 0 + play)。左手で押しやすく、画像モードでも未使用。
        let rewind_key = self.keymap.consume_action(ctx, KeyAction::VideoSeekStart);
        // J/K: チャプター・ブックマーク・ピンを 1 本のマーカー列にまとめて前後ジャンプ。
        // 矢印キーは既に固定秒数シークに使っているので別キー。J=前、K=次。
        let prev_marker_key = self.keymap.consume_action(ctx, KeyAction::VideoMarkerPrev);
        let next_marker_key = self.keymap.consume_action(ctx, KeyAction::VideoMarkerNext);
        // タイルモード中は ESC でも閉じれるようにする (= 一般的な「全画面モード解除」)。
        // ただし ESC は元々フルスクリーン全体を閉じるキーなので、タイルモード中だけ
        // 横取りする。
        let escape_for_tile = self.video_tile_mode_active
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

        if shift_enter {
            if let Some(p) = video_path {
                open_external_player(p);
            }
            return;
        }
        if play_pause && self.video_tile_mode_active {
            self.play_selected_video_tile(ctx, fs_idx);
            return;
        }
        if let Some(ctrl) = tile_left_ctrl.or(tile_right_ctrl) {
            self.handle_video_tile_cursor_key(ctx, fs_idx, ctrl, tile_left);
            return;
        }
        if save_frame_key {
            self.save_video_frame_to_file(ctx, fs_idx);
            return;
        }
        if compare_x || compare_alt_c || compare_shift_c || compare_c {
            return;
        }

        // 先に現在の音量だけ取り出す (player borrow を短く保つ)
        let cur_volume = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.volume(),
            _ => return,
        };
        // Phase 7.H: 音量は Shift+↑↓ 限定 (= dB fader key step)。プレーン ↑↓ はファイル移動。
        let new_vol = if shift_up {
            Some(crate::settings::step_video_volume_by_fader_key_step(
                cur_volume, 1,
            ))
        } else if shift_down {
            Some(crate::settings::step_video_volume_by_fader_key_step(
                cur_volume, -1,
            ))
        } else {
            None
        };

        // player に作用させる (借用はこの if-let のスコープ内で完結)
        let mut seek_outcome: Option<crate::video::RelativeSeekOutcome> = None;
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            if play_pause {
                player.toggle_play();
            }
            if ctrl_shift_left {
                player.step_frame(-1);
            }
            if ctrl_shift_right {
                player.step_frame(1);
            }
            // Phase 7.H シーク粒度:
            //   ←→ = 5 秒 (デフォルト、動画プレイヤー慣例)
            //   Shift+←→ = 1 秒 (細かい、フレーム単位調整に近い)
            //   Ctrl+←→ = 30 秒 (大きい、長い動画の早送り用)
            if left {
                seek_outcome = Some(player.seek_relative(-5.0));
            }
            if right {
                seek_outcome = Some(player.seek_relative(5.0));
            }
            if shift_left {
                seek_outcome = Some(player.seek_relative(-1.0));
            }
            if shift_right {
                seek_outcome = Some(player.seek_relative(1.0));
            }
            if ctrl_left {
                seek_outcome = Some(player.seek_relative(-30.0));
            }
            if ctrl_right {
                seek_outcome = Some(player.seek_relative(30.0));
            }
            if let Some(v) = new_vol {
                player.set_volume(v);
            }
        }
        // 先頭 / 末尾に達してシークが発行されなかった場合は境界トーストで通知する
        // (player 借用が終わってから self を触る)。
        match seek_outcome {
            Some(crate::video::RelativeSeekOutcome::AtStart) => {
                self.show_feedback_toast("動画先頭です".to_string());
            }
            Some(crate::video::RelativeSeekOutcome::AtEnd) => {
                self.show_feedback_toast("動画末尾です".to_string());
            }
            Some(crate::video::RelativeSeekOutcome::Seeked) | None => {}
        }

        // 設定への反映 (player 借用は終わっているので self.settings を書き換え可能)。
        // Phase 8 (Codex P3-1): 即時 save 化。グリッド列数や動画タイル列数が
        // 即時保存されるのと挙動を揃え、強制終了時の loss を防ぐ。
        if let Some(v) = new_vol {
            self.settings.video_volume = v;
            self.settings.save();
        }
        if mute_key {
            self.toggle_video_session_mute_for_fs_idx(fs_idx);
        }
        if loop_key {
            // cycle_native_video_loop_common は app/native_video.rs (cfg(windows)) に
            // 定義されているため、非 Windows ビルドではリンクできない。
            // mimageviewer の動画機能自体が Windows 限定 (`pub mod video;` が cfg(windows))
            // なので非 Windows では何もしない。
            #[cfg(windows)]
            self.cycle_native_video_loop_common(ctx, fs_idx);
            #[cfg(not(windows))]
            let _ = (ctx, fs_idx);
        }

        // Phase 5.5: S キーでタイルモード トグル。画面サイズは native presenter の
        // 実クライアントサイズ優先で取得 (取得不可なら content rect にフォールバック)。
        // toggle 内で fs_cache / video_tile_state を借用するので、player 借用後に呼ぶ。
        if tile_key {
            let screen = self.video_tile_layout_size(fs_idx, ctx);
            self.toggle_video_tile_mode(fs_idx, screen);
        }
        if perf_key {
            self.video_perf_overlay_visible = !self.video_perf_overlay_visible;
            // perf overlay は native presenter が描画する。フラグを反転するだけでなく、
            // presenter 側の AtomicBool にも伝搬しないと表示が切り替わらない。native
            // HWND 経由の handle_native_video_key_event は伝搬しているが、こちらの
            // egui 経路 (= main ウィンドウにフォーカスがある in-window モード等) でも
            // 同じ更新を行う必要がある。
            #[cfg(windows)]
            if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                player.set_native_perf_overlay_visible(self.video_perf_overlay_visible);
            }
        }
        if pin_key {
            // P キー: タイルモード中は選択中タイル、それ以外は現在再生位置を
            // ピン留め (= HUD の 📌 ボタンと同等)。
            #[cfg(windows)]
            {
                if self.video_tile_mode_active {
                    self.handle_native_video_set_tile_pin_command(ctx, fs_idx);
                } else {
                    // 現在 PTS を `set_native_video_pin` に渡す (内部で seek thumbnail を
                    // request + nearest 取得 + WebP encode + video_pins DB に書き込み)。
                    // 既に同位置のピンがあれば SQL の ON CONFLICT で pin_pts/thumb_webp を
                    // 上書きするだけなので idempotent。
                    let target = self
                        .fs_video_player(fs_idx)
                        .map(|p| p.position())
                        .unwrap_or(0.0);
                    self.handle_native_video_set_pin_command(ctx, fs_idx, target);
                }
            }
            #[cfg(not(windows))]
            let _ = (ctx, fs_idx);
        }
        if rewind_key && let Some(p) = self.fs_video_player(fs_idx) {
            // `seek(0.0)` は内部で `apply_command(Play)` を発行し autoplay intent を
            // 立てるので、追加 `toggle_play()` は不要 (Codex P2-1 2026-05-17)。
            p.seek(0.0);
        }

        // 何らかの動画ショートカット入力があれば HUD のフェードタイマをリセット
        // (= HUD を再表示)。マウス活動と同様の扱い。
        // J/K: マーカー (チャプター/ブックマーク/ピン) 間の前後ジャンプ。
        // 現在再生位置 ± epsilon を境にした最近接探索で「現在マーカーで足踏み」を防ぐ。
        // J で前のマーカーが見つからないときは動画先頭 (0.0) へ seek (= 既に先頭に
        // 居る場合だけ何もしない、閾値は ALREADY_AT_START_TOL)。K のときは何もしない。
        const ALREADY_AT_START_TOL: f64 = 0.05;
        if prev_marker_key || next_marker_key {
            let markers = self.collect_video_nav_markers(fs_idx);
            let current = self
                .fs_video_player(fs_idx)
                .map(|p| p.position())
                .unwrap_or(0.0);
            let target: Option<NavMarker> = if next_marker_key {
                markers
                    .iter()
                    .find(|m| m.pts > current + NAV_MARKER_EPSILON)
                    .cloned()
            } else {
                markers
                    .iter()
                    .rev()
                    .find(|m| m.pts < current - NAV_MARKER_EPSILON)
                    .cloned()
            };
            match target {
                Some(m) => {
                    if let Some(p) = self.fs_video_player(fs_idx) {
                        p.seek(m.pts);
                    }
                    // CH/BM ループ中ならマーカージャンプ後に loop_target を更新
                    #[cfg(windows)]
                    self.apply_loop_mode_to_player(fs_idx);
                    let direction = if next_marker_key { "次の" } else { "前の" };
                    let kind_label = match m.kind {
                        NavMarkerKind::Chapter => "チャプター",
                        NavMarkerKind::Bookmark => "ブックマーク",
                        NavMarkerKind::Pin => "ピン",
                    };
                    let toast = match (m.kind, m.title.as_deref()) {
                        (NavMarkerKind::Chapter, Some(t)) | (NavMarkerKind::Bookmark, Some(t))
                            if !t.is_empty() =>
                        {
                            format!(
                                "{} {}{}: {}",
                                crate::ui_helpers::format_hms(m.pts),
                                direction,
                                kind_label,
                                t
                            )
                        }
                        _ => format!(
                            "{} {}{}",
                            crate::ui_helpers::format_hms(m.pts),
                            direction,
                            kind_label
                        ),
                    };
                    self.show_feedback_toast(toast);
                }
                None if !next_marker_key && current > ALREADY_AT_START_TOL => {
                    // J キーで前のマーカーが見つからない (= 最初のマーカー手前または空) かつ
                    // 既に先頭に居なければ動画先頭へ seek。
                    if let Some(p) = self.fs_video_player(fs_idx) {
                        p.seek(0.0);
                    }
                    #[cfg(windows)]
                    self.apply_loop_mode_to_player(fs_idx);
                    self.show_feedback_toast(format!(
                        "{} 動画先頭",
                        crate::ui_helpers::format_hms(0.0)
                    ));
                }
                None => {
                    // K キーでマーカーが無い (= 末尾以降) ケース、
                    // または J キーで既に先頭にいるケースは何もしない。
                }
            }
        }
        // ESC は タイルモード中のみキャッチして close。フルスクリーン解脱は呼び出し側
        // (handle_image_keys 後段) の通常 ESC で扱う。
        if escape_for_tile {
            self.close_video_tile_mode();
        }

        // Phase 5.4.1: ブックマーク追加。現在位置 + 動画パスを取得して DB に挿入。
        // 借用衝突を避けるため `if let Some(player)` 短いスコープで pos / path を抜く。
        // Codex P5.4 M2 反映: Loading / エラー状態では追加させない (= 0.0s に
        // ゴミブックマークが入るのを防ぐ)。info() が来ている = duration / has_audio が
        // 確定 = 1 フレーム以上は decoder が動いている前提。
        if bookmark_key {
            let snapshot = match self.fs_cache.get(&fs_idx) {
                Some(FsCacheEntry::Video { player, .. }) => {
                    if player.error().is_some() || player.info().is_none() {
                        None
                    } else {
                        Some((player.path().clone(), player.position()))
                    }
                }
                _ => None,
            };
            if let (Some((path, pts)), Some(db)) = (snapshot, self.video_bookmark_db.as_ref()) {
                if let Err(e) = db.add(&path, pts, None, &[]) {
                    crate::logger::log(format!("video bookmark add failed: {e}"));
                } else {
                    crate::logger::log(format!(
                        "video bookmark added: pts={pts:.2}s {}",
                        path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                    ));
                    self.refresh_fullscreen_video_marker_cache(fs_idx);
                    #[cfg(windows)]
                    self.sync_native_video_timeline_markers(fs_idx);
                }
            }
        }
    }

    /// 動画再生のエラー インジケータを描画する。
    /// playback controls (HUD, seek bar, volume, etc.) と「動画を準備中…」スピナーは
    /// **native presenter overlay** (`src/video/native_presenter/overlay_draw.rs` の
    /// `draw_native_center_status`) が描画する (= native window が egui main の上に
    /// 乗るので、ここで描画しても見えない)。本関数では egui main window 側に
    /// 直接出すべきエラー文言だけを担当する。
    ///
    /// (旧挙動として `!has_texture` で「動画を準備中...」を egui に描いていたが、
    /// native presenter が上に乗ると見えなかったので削除。進捗 HUD の本物は
    /// native presenter 側で `build_preparing_message` を使って描く。)
    pub(crate) fn draw_video_hud(&self, ui: &mut egui::Ui, full_rect: egui::Rect, fs_idx: usize) {
        let has_error = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.error().map(|s| s.to_string()),
            _ => return,
        };

        if let Some(err) = has_error {
            let painter = ui.painter();
            let galley = painter.layout_no_wrap(
                format!("動画を再生できません: {err}"),
                egui::FontId::proportional(20.0),
                egui::Color32::from_rgb(255, 120, 120),
            );
            let pos = full_rect.center() - galley.size() / 2.0;
            let bg_rect = egui::Rect::from_min_size(pos, galley.size()).expand(12.0);
            painter.rect_filled(
                bg_rect,
                6.0,
                egui::Color32::from_rgba_unmultiplied(0, 0, 0, 200),
            );
            painter.galley(pos, galley, egui::Color32::from_rgb(255, 120, 120));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid_item::GridItem;
    use std::path::PathBuf;

    #[test]
    fn pixel_grid_requires_user_zoom_above_one() {
        assert!(!should_draw_fs_pixel_grid(true, true, None));
        assert!(!should_draw_fs_pixel_grid(
            true,
            true,
            Some((1.0, egui::vec2(24.0, 12.0)))
        ));
        assert!(!should_draw_fs_pixel_grid(
            true,
            true,
            Some((ZOOM_NEAR_ONE, egui::Vec2::ZERO))
        ));
        assert!(should_draw_fs_pixel_grid(
            true,
            true,
            Some((ZOOM_NEAR_ONE + 0.01, egui::Vec2::ZERO))
        ));
    }

    #[test]
    fn pixel_grid_respects_toggle_and_full_texture_gate() {
        let zoomed = Some((2.0, egui::Vec2::ZERO));

        assert!(!should_draw_fs_pixel_grid(false, true, zoomed));
        assert!(!should_draw_fs_pixel_grid(true, false, zoomed));
        assert!(should_draw_fs_pixel_grid(true, true, zoomed));
    }

    #[test]
    fn ctrl_wheel_is_handled_even_over_panels() {
        assert!(should_handle_fullscreen_wheel(
            true, false, true, false, false
        ));
        assert!(!should_handle_fullscreen_wheel(
            true, false, false, false, false
        ));
        assert!(should_handle_fullscreen_wheel(
            false, false, false, false, false
        ));
        assert!(!should_handle_fullscreen_wheel(
            false, false, true, true, false
        ));
        // 表示モード / フィットのポップアップ表示中は Ctrl+ホイールでも抑制する。
        assert!(!should_handle_fullscreen_wheel(
            false, false, true, false, true
        ));
    }

    #[test]
    fn compare_wipe_reference_rect_is_letterbox_fitted_not_full_rect() {
        // 横長ウィンドウ + 縦長画像 → 左右に黒帯。比較 Wipe の白線 / clip / ドラッグ /
        // シェーダー合成境界は、この「フィット後の実表示画像矩形」を基準にしなければ
        // ならない。full_rect (黒帯込み) と混在させると、同じ fraction でも線の位置と
        // 切り替わり位置がズレる (本バグの本質)。
        let full = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1000.0, 800.0));
        let fitted = App::compare_image_draw_rect(full, [400, 800], None).unwrap();
        // 高さフィット (800/800=1.0)、幅 400 → 中央寄せで左右に黒帯。
        assert!((fitted.height() - 800.0).abs() < 0.5);
        assert!((fitted.width() - 400.0).abs() < 0.5);
        assert!(fitted.width() < full.width()); // = full_rect とは別物
        assert!((fitted.center().x - full.center().x).abs() < 0.5); // 中央寄せ
        // 同一 fraction でも基準が違えば線の x がズレる。
        let f = 0.25_f32;
        let x_fitted = fitted.left() + fitted.width() * f;
        let x_full = full.left() + full.width() * f;
        assert!((x_fitted - x_full).abs() > 100.0);
    }

    #[test]
    fn fullscreen_wheel_zoom_includes_overlay_edit_modes() {
        assert!(should_zoom_fullscreen_wheel(true, false));
        assert!(should_zoom_fullscreen_wheel(false, true));
        assert!(!should_zoom_fullscreen_wheel(false, false));
    }

    #[test]
    fn seek_overlay_counts_zip_separator_as_non_media() {
        let zip = PathBuf::from(r"C:\books\comic.zip");
        let items = vec![
            GridItem::ZipSeparator {
                dir_display: "(root)".into(),
            },
            GridItem::ZipImage {
                zip_path: zip.clone(),
                entry_name: "chapter1/page01.jpg".into(),
            },
            GridItem::ZipSeparator {
                dir_display: "chapter2".into(),
            },
            GridItem::ZipImage {
                zip_path: zip,
                entry_name: "chapter2/page01.jpg".into(),
            },
        ];
        let visible_indices = vec![0, 1, 2, 3];
        let nav_indices = build_nav_indices(&items, &visible_indices);

        assert_eq!(
            build_image_reading_indices(&items, &visible_indices),
            vec![1, 3]
        );
        assert_eq!(
            count_seek_overlay_non_image_items(&items, &nav_indices),
            (0, 0)
        );
    }

    #[test]
    fn seek_overlay_counts_video_as_mixed_media() {
        let items = vec![
            GridItem::Image(PathBuf::from(r"C:\books\page01.jpg")),
            GridItem::Video(PathBuf::from(r"C:\books\bonus.mp4")),
        ];
        let visible_indices = vec![0, 1];
        let nav_indices = build_nav_indices(&items, &visible_indices);

        assert_eq!(
            build_image_reading_indices(&items, &visible_indices),
            vec![0]
        );
        assert_eq!(
            count_seek_overlay_non_image_items(&items, &nav_indices),
            (1, 0)
        );
    }

    #[test]
    fn vertical_reading_offsets_center_current_page() {
        let offsets = vertical_reading_offsets(&[100.0, 100.0, 100.0], 10.0, 1);
        assert_eq!(offsets, vec![-110.0, 0.0, 110.0]);
    }

    #[test]
    fn vertical_reading_visible_positions_follow_scroll() {
        let heights = [100.0, 100.0, 100.0];
        let offsets = vertical_reading_offsets(&heights, 10.0, 1);

        assert_eq!(
            vertical_reading_visible_positions(&offsets, &heights, 0.0, 100.0),
            vec![1]
        );
        assert_eq!(
            vertical_reading_visible_positions(&offsets, &heights, 110.0, 100.0),
            vec![2]
        );
        assert_eq!(vertical_reading_nearest_position(&offsets, 110.0), Some(2));
    }

    #[test]
    fn vertical_reading_scroll_clamps_to_book_edges() {
        let heights = [100.0, 100.0];
        let offsets = vertical_reading_offsets(&heights, 10.0, 0);

        assert_eq!(
            clamp_vertical_reading_scroll(-500.0, &offsets, &heights, 100.0),
            0.0
        );
        assert_eq!(
            clamp_vertical_reading_scroll(500.0, &offsets, &heights, 100.0),
            110.0
        );
    }

    #[test]
    fn vertical_reading_reanchor_keeps_visual_position() {
        let heights = [100.0, 100.0, 100.0];
        let offsets = vertical_reading_offsets(&heights, 10.0, 1);

        assert_eq!(vertical_reading_reanchor_scroll(110.0, &offsets, 2), 0.0);
        assert_eq!(vertical_reading_reanchor_scroll(-110.0, &offsets, 0), 0.0);
    }

    #[test]
    fn vertical_spread_cover_uses_virtual_two_page_width_for_width_fit() {
        assert_eq!(
            continuous_spread_fit_width(
                1,
                100.0,
                SpreadMode::RtlCover,
                ReadingFlow::Vertical,
                FullscreenFitMode::Width,
                4.0,
            ),
            (200.0, 4.0)
        );
        assert_eq!(
            continuous_spread_fit_width(
                1,
                100.0,
                SpreadMode::RtlCover,
                ReadingFlow::Vertical,
                FullscreenFitMode::Height,
                4.0,
            ),
            (100.0, 0.0)
        );
        assert_eq!(
            continuous_spread_fit_width(
                1,
                100.0,
                SpreadMode::Single,
                ReadingFlow::Vertical,
                FullscreenFitMode::Width,
                4.0,
            ),
            (100.0, 0.0)
        );
    }

    #[test]
    fn continuous_single_page_rect_is_centered_inside_virtual_unit() {
        let unit_rect =
            egui::Rect::from_center_size(egui::pos2(100.0, 50.0), egui::vec2(200.0, 100.0));
        let size = ContinuousReadingUnitSize {
            pages: vec![ContinuousReadingPageSize {
                idx: 42,
                width: 96.0,
                height: 100.0,
            }],
            width: 200.0,
            height: 100.0,
            page_gap: 0.0,
        };
        let rects = continuous_reading_page_rects(unit_rect, &size);
        assert_eq!(rects.len(), 1);
        assert_eq!(rects[0].0, 42);
        assert!((rects[0].1.center().x - unit_rect.center().x).abs() < 0.001);
        assert!((rects[0].1.width() - 96.0).abs() < 0.001);
    }

    #[test]
    fn continuous_separator_unit_does_not_count_as_page() {
        let units = vec![
            ContinuousReadingUnitSpec::separator(10, "chapter".to_owned()),
            ContinuousReadingUnitSpec::pages(11, vec![11, 12]),
        ];

        assert!(units[0].contains_idx(10));
        assert!(units[0].pages.is_empty());
        assert_eq!(App::continuous_visible_page_count(&units, &[0, 1]), 2);
    }

    #[test]
    fn continuous_separator_unit_copies_neighbor_page_size() {
        let units = vec![
            ContinuousReadingUnitSpec::pages(1, vec![1]),
            ContinuousReadingUnitSpec::separator(2, "chapter".to_owned()),
            ContinuousReadingUnitSpec::pages(3, vec![3]),
        ];
        let mut sizes = vec![
            ContinuousReadingUnitSize {
                pages: vec![ContinuousReadingPageSize {
                    idx: 1,
                    width: 320.0,
                    height: 480.0,
                }],
                width: 320.0,
                height: 480.0,
                page_gap: 0.0,
            },
            ContinuousReadingUnitSize {
                pages: Vec::new(),
                width: 80.0,
                height: 120.0,
                page_gap: 0.0,
            },
            ContinuousReadingUnitSize {
                pages: vec![ContinuousReadingPageSize {
                    idx: 3,
                    width: 500.0,
                    height: 700.0,
                }],
                width: 500.0,
                height: 700.0,
                page_gap: 0.0,
            },
        ];

        apply_continuous_separator_unit_sizes(&units, &mut sizes);

        assert_eq!(sizes[1].width, 320.0);
        assert_eq!(sizes[1].height, 480.0);
    }

    #[test]
    fn leading_continuous_separator_unit_copies_next_page_size() {
        let units = vec![
            ContinuousReadingUnitSpec::separator(1, "root".to_owned()),
            ContinuousReadingUnitSpec::pages(2, vec![2]),
        ];
        let mut sizes = vec![
            ContinuousReadingUnitSize {
                pages: Vec::new(),
                width: 80.0,
                height: 120.0,
                page_gap: 0.0,
            },
            ContinuousReadingUnitSize {
                pages: vec![ContinuousReadingPageSize {
                    idx: 2,
                    width: 640.0,
                    height: 360.0,
                }],
                width: 640.0,
                height: 360.0,
                page_gap: 0.0,
            },
        ];

        apply_continuous_separator_unit_sizes(&units, &mut sizes);

        assert_eq!(sizes[0].width, 640.0);
        assert_eq!(sizes[0].height, 360.0);
    }

    // ── decide_local_adjust_preview_action unit tests ─────────────────────
    //
    // A-3 saga (3 iterations of fixes) で発生した「修飾キー判定 + 経路選択」の
    // 退行を符号化する。OS API (`ctrl_held_via_os`) と App 状態は caller が
    // 取り出して渡す形にしてあるので、ここでは pure logic だけテストできる。

    fn act(
        focus: bool,
        ctrl: bool,
        shift: bool,
        show_source: bool,
        preview_to_selected: bool,
        has_any: bool,
        selected: Option<usize>,
        total: usize,
    ) -> LocalAdjustPreviewAction {
        decide_local_adjust_preview_action(
            focus,
            ctrl,
            shift,
            show_source,
            preview_to_selected,
            has_any,
            selected,
            total,
        )
    }

    /// 修飾キー無し・パネル状態無し → FullComposite (= 通常表示)
    #[test]
    fn decide_action_default_is_full_composite() {
        assert_eq!(
            act(true, false, false, false, false, true, Some(0), 3),
            LocalAdjustPreviewAction::FullComposite
        );
    }

    /// Ctrl のみ押下 → 元画像表示 (= 一時的に補正前を見るパス)
    #[test]
    fn decide_action_ctrl_only_shows_source() {
        assert_eq!(
            act(true, true, false, false, false, true, Some(0), 3),
            LocalAdjustPreviewAction::ShowSource
        );
    }

    /// 「元画像を表示」トグル ON → ShowSource (= panel checkbox)
    #[test]
    fn decide_action_show_source_toggle_shows_source() {
        assert_eq!(
            act(true, false, false, true, false, true, Some(0), 3),
            LocalAdjustPreviewAction::ShowSource
        );
    }

    /// Ctrl+Shift + パネルにレイヤーあり + 選択あり → BypassLayer (= ラボ仕様)
    #[test]
    fn decide_action_ctrl_shift_with_layers_uses_bypass() {
        assert_eq!(
            act(true, true, true, false, false, true, Some(1), 3),
            LocalAdjustPreviewAction::BypassLayer { layer_idx: 1 }
        );
    }

    /// Ctrl+Shift だがパネルにレイヤー無し → ShowSource (= bypass 対象が無いので
    /// Ctrl-only 経路に落ちる)
    #[test]
    fn decide_action_ctrl_shift_without_layers_falls_to_source() {
        assert_eq!(
            act(true, true, true, false, false, false, None, 0),
            LocalAdjustPreviewAction::ShowSource
        );
    }

    /// 「選択レイヤーまでプレビュー」ON + 選択は最終レイヤーより手前 → PrefixPreview
    #[test]
    fn decide_action_preview_toggle_with_intermediate_selection_uses_prefix() {
        assert_eq!(
            act(true, false, false, false, true, true, Some(1), 3),
            LocalAdjustPreviewAction::PrefixPreview { layer_count: 2 }
        );
    }

    /// 「選択レイヤーまでプレビュー」ON + 選択は最後のレイヤー → FullComposite
    /// (= prefix=total と FullComposite は同じ結果なので worker 起動を避ける)
    #[test]
    fn decide_action_preview_toggle_at_last_layer_falls_to_full() {
        assert_eq!(
            act(true, false, false, false, true, true, Some(2), 3),
            LocalAdjustPreviewAction::FullComposite
        );
    }

    /// `selected_layer_idx = None` のときは preview 経路に進まない → FullComposite
    #[test]
    fn decide_action_no_selection_falls_to_full_composite() {
        assert_eq!(
            act(true, false, false, false, true, true, None, 3),
            LocalAdjustPreviewAction::FullComposite
        );
    }

    /// フルスクリーン非フォーカス: 別アプリの Ctrl 押下を誤検知しないために
    /// ctrl_held / shift_held を 0 扱いし、Ctrl が来ても FullComposite のまま
    #[test]
    fn decide_action_ignores_modifiers_when_not_focused() {
        assert_eq!(
            act(false, true, true, false, false, true, Some(1), 3),
            LocalAdjustPreviewAction::FullComposite,
            "non-focused fullscreen must not pick up other app's Ctrl+Shift"
        );
    }

    /// 非フォーカスでも `show_source_toggle` (= app state) は効く
    /// (= OS API 経由でないので別アプリの状態を読まない)
    #[test]
    fn decide_action_show_source_toggle_works_when_not_focused() {
        assert_eq!(
            act(false, false, false, true, false, true, Some(0), 3),
            LocalAdjustPreviewAction::ShowSource
        );
    }

    /// Ctrl+Shift で選択 idx が total を超えていたら clamp される
    /// (= 古い selected idx が leftover していても panic しない)
    #[test]
    fn decide_action_bypass_clamps_overflow_layer_idx() {
        assert_eq!(
            act(true, true, true, false, false, true, Some(99), 3),
            LocalAdjustPreviewAction::BypassLayer { layer_idx: 2 }
        );
    }

    /// total_layers = 0 で preview/bypass 経路は不発になり FullComposite
    /// (= ページに何も無い、ただ Ctrl+Shift 押されただけ)
    #[test]
    fn decide_action_zero_layers_never_picks_preview() {
        // Ctrl+Shift だが has_any=false → modifier_bypass_active=false → ShowSource (Ctrl 経路)
        assert_eq!(
            act(true, true, true, false, false, false, None, 0),
            LocalAdjustPreviewAction::ShowSource
        );
        // preview_to_selected ON で total=0 → FullComposite (preview 経路に入らない)
        assert_eq!(
            act(true, false, false, false, true, false, None, 0),
            LocalAdjustPreviewAction::FullComposite
        );
    }

    /// P7-3a: `show_source_toggle` と `preview_to_selected_layer_toggle` が**両方 ON**
    /// なら、ShowSource が PrefixPreview より優先される。
    ///
    /// 根拠: `decide_local_adjust_preview_action` のコードを読むと、最初の if で
    /// `show_source_toggle` を見て早期 return するため、後段の preview_requested 判定
    /// に到達しない構造になっている。これは UX として「元画像表示はパネルで
    /// 明示的に有効化したもの」だから「選択レイヤーまでプレビュー」よりも強い
    /// ユーザー意図、という設計。
    ///
    /// 退行: もし将来この優先順位が反転すると、ユーザーが「元画像表示」を ON に
    /// したまま選択レイヤーを切り替えると、急に PrefixPreview に切り替わって
    /// 「元画像が見えない」と困惑する。
    #[test]
    fn decide_action_show_source_toggle_beats_preview_to_selected_layer() {
        assert_eq!(
            act(true, false, false, true, true, true, Some(1), 3),
            LocalAdjustPreviewAction::ShowSource,
            "show_source トグル ON は preview_to_selected_layer ON より優先"
        );
    }

    /// P7-3b: Ctrl+Shift modifier bypass は `show_source_toggle` の状態より優先される。
    ///
    /// 根拠: 最初の if 条件 `!modifier_bypass_active && (ctrl || show_source_toggle)`
    /// で `modifier_bypass_active=true` なら全体が false で抜けない → BypassLayer 経路へ。
    ///
    /// UX 意図: パネル UX で「元画像表示」を ON にしたまま「特定レイヤーだけ一時的に
    /// 抜いた絵を見たい」ときに Ctrl+Shift を押すと bypass が効く (= toggle を一度
    /// off にしなくて済む)。
    #[test]
    fn decide_action_ctrl_shift_bypass_beats_show_source_toggle() {
        assert_eq!(
            act(true, true, true, true, false, true, Some(1), 3),
            LocalAdjustPreviewAction::BypassLayer { layer_idx: 1 },
            "Ctrl+Shift bypass は show_source トグル ON より優先 (= modifier の方が一時操作で勝つ)"
        );
    }

    // ── should_close_fullscreen_on_enter unit tests ──────────────────────
    //
    // Enter キーを Esc と同等に「フルスクリーン解除」トリガーとして使うかを判定する
    // 純関数の境界条件を符号化する。

    /// 通常の静止画フルスクリーン中: Enter で解除する (= Esc 相当)。
    #[test]
    fn enter_closes_fs_for_still_image_in_normal_state() {
        assert!(should_close_fullscreen_on_enter(false, false, false, false));
    }

    /// 動画モードでは Enter は「再生/停止」なので消費しない。
    #[test]
    fn enter_does_not_close_fs_for_video() {
        assert!(!should_close_fullscreen_on_enter(true, false, false, false));
    }

    /// IME 変換中の Enter は IME 確定キーなので奪わない (= 日本語入力中の事故防止)。
    #[test]
    fn enter_does_not_close_fs_during_ime_composition() {
        assert!(!should_close_fullscreen_on_enter(false, true, false, false));
    }

    /// フルスクリーン context menu 表示中の Enter はメニュー選択操作なので奪わない。
    #[test]
    fn enter_does_not_close_fs_when_context_menu_open() {
        assert!(!should_close_fullscreen_on_enter(false, false, true, false));
    }

    /// 同時に複数の除外条件が成立しても (= 動画 + IME + menu)、いずれかが立てば
    /// 解除しない。
    #[test]
    fn enter_close_combined_exclusions() {
        assert!(!should_close_fullscreen_on_enter(true, true, true, true));
    }

    /// グリッドの Enter で open した直後のフレームでは `suppress_until_release` が
    /// true で、Enter event がまだ input queue に残っていても close しない
    /// (= 「一瞬開いてすぐ閉じる」ユーザー報告に対する退行ガード)。
    #[test]
    fn enter_does_not_close_fs_while_suppressed_after_grid_open() {
        assert!(!should_close_fullscreen_on_enter(false, false, false, true));
    }

    /// P7-3c: `preview_to_selected_layer_toggle` は **フルスクリーン非フォーカス時でも効く**。
    ///
    /// 根拠: focus は OS API (`*_held_via_os`) で読む修飾キーの誤検知を防ぐためだけの
    /// ガードで、App state である toggle 系には適用されない。`fs_prev_focused=false`
    /// でも `preview_to_selected_layer_toggle=true` なら PrefixPreview に進む。
    ///
    /// 回帰防止: `decide_local_adjust_preview_action` 内で focus による early return を
    /// 全体に適用するような誤った refactor が入ると、別アプリにフォーカスが移った
    /// 瞬間に panel toggle が無視されてしまう。
    #[test]
    fn decide_action_preview_to_selected_layer_works_when_not_focused() {
        assert_eq!(
            act(false, false, false, false, true, true, Some(1), 3),
            LocalAdjustPreviewAction::PrefixPreview { layer_count: 2 },
            "panel toggle 系は OS focus に依存せず効く (= 修飾キーの focus guard とは別)"
        );
    }

    #[test]
    fn location_display_regular_image_joins_folder_and_filename() {
        let out = compute_location_display(
            Some(&GridItem::Image(PathBuf::from(r"C:\photos\2024\img.jpg"))),
            r"C:\photos\2024",
            "img.jpg",
        );
        assert_eq!(out, r"C:\photos\2024\img.jpg");
    }

    #[test]
    fn location_display_regular_image_handles_trailing_separator() {
        // ドライブルート直下 "C:\" のような末尾 '\' ケース
        let out = compute_location_display(
            Some(&GridItem::Image(PathBuf::from(r"C:\img.jpg"))),
            r"C:\",
            "img.jpg",
        );
        assert_eq!(out, r"C:\img.jpg");
    }

    #[test]
    fn location_display_zip_image_uses_arrow_separator() {
        let out = compute_location_display(
            Some(&GridItem::ZipImage {
                zip_path: PathBuf::from(r"C:\archives\book.zip"),
                entry_name: "ch01/page01.jpg".to_string(),
            }),
            r"C:\archives\book.zip",
            "page01.jpg",
        );
        assert_eq!(out, r"C:\archives\book.zip > ch01/page01.jpg");
    }

    #[test]
    fn location_display_pdf_page_shows_page_number() {
        let out = compute_location_display(
            Some(&GridItem::PdfPage {
                pdf_path: PathBuf::from(r"C:\docs\manual.pdf"),
                page_num: 4, // 0-indexed なので表示は "Page 5"
                content_type: None,
            }),
            r"C:\docs\manual.pdf",
            "Page 5",
        );
        assert_eq!(out, r"C:\docs\manual.pdf > Page 5");
    }

    /// 変換済み RAR/7z/LZH を閲覧中は `effective_folder()` が元アーカイブのパスを
    /// 返す想定。`base_folder` にその値が渡ってくるので、キャッシュ ZIP のパス
    /// ではなく元 RAR/7z/LZH が表示される。
    #[test]
    fn location_display_uses_override_path_for_converted_archives() {
        let out = compute_location_display(
            Some(&GridItem::ZipImage {
                // zip_path はキャッシュ ZIP 側 (UI 表示には使わない)
                zip_path: PathBuf::from(r"C:\AppData\cache\abc\book.zip"),
                entry_name: "page01.jpg".to_string(),
            }),
            // base_folder は effective_folder() → 元 LZH
            r"C:\downloads\book.lzh",
            "page01.jpg",
        );
        assert_eq!(out, r"C:\downloads\book.lzh > page01.jpg");
    }

    #[test]
    fn location_display_empty_base_falls_back_to_filename() {
        let out = compute_location_display(
            Some(&GridItem::Image(PathBuf::from("img.jpg"))),
            "",
            "img.jpg",
        );
        assert_eq!(out, "img.jpg");
    }

    #[test]
    fn location_display_empty_base_zip_falls_back_to_entry() {
        let out = compute_location_display(
            Some(&GridItem::ZipImage {
                zip_path: PathBuf::from("book.zip"),
                entry_name: "page01.jpg".to_string(),
            }),
            "",
            "page01.jpg",
        );
        assert_eq!(out, "page01.jpg");
    }

    // ── build_info_text: ダウンスケール警告 ─────────────────────────

    #[test]
    fn info_text_dims_only_no_marker_when_not_downscaled() {
        let s = build_info_text(Some((1920, 1080)), None, false, None, None);
        assert_eq!(s, "1920 × 1080");
    }

    #[test]
    fn info_text_appends_downscale_marker_when_flag_set() {
        let s = build_info_text(Some((7168, 9216)), None, true, None, None);
        assert!(s.starts_with("7168 × 9216"));
        assert!(
            s.contains("ダウンスケール表示中"),
            "warning marker present: {s:?}"
        );
    }

    #[test]
    fn info_text_marker_comes_after_ai_info() {
        // AI 情報 "(漫画 ...)" の内側に警告が混入しないこと。
        let s = build_info_text(
            Some((7168, 9216)),
            None,
            true,
            Some(("漫画", 28672, 36864)),
            None,
        );
        let ai_end = s.find(')').expect("AI closing paren exists");
        let marker_pos = s.find("ダウンスケール").expect("marker present");
        assert!(
            marker_pos > ai_end,
            "marker should come after AI info: {s:?}"
        );
    }

    #[test]
    fn info_text_no_marker_when_dims_missing() {
        // dims が None のとき (まだロード中) は downscaled=true でも警告を出さない。
        let s = build_info_text(None, Some(1_234_567), true, None, None);
        assert!(
            !s.contains("ダウンスケール"),
            "no marker without dims: {s:?}"
        );
    }
}
