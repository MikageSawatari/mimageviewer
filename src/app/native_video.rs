use super::*;
use crate::keymap::{CommandDisplayRow, CommandScope, FS_VIDEO_ACTIVE_SCOPES, KeyAction};

/// native presenter が転送してきた KeyDown の virtual key が **いま物理的に押下中か**を
/// OS に問い合わせる。placement 切替 (Plan B) で presenter を作り直すと、既に離された
/// F12 の KeyDown が数百 ms 遅れて再配送され (`repeat=false` でも stale)、detached→main→
/// detached の二重トグルになる。`repeat` フラグは信用できない (stale でも false) ため、
/// GetAsyncKeyState の high bit で「今まだ押されているか」を見て stale 再配送を弾く。
#[cfg(windows)]
fn native_video_key_physically_down(
    key: &crate::video::native_window::NativeVideoKeyEvent,
) -> bool {
    // headless な単体テストは合成 KeyDown を送るので実際には物理キーが押されておらず、
    // GetAsyncKeyState が常に false を返して F12 トグルが走らない。この OS 問い合わせは
    // 実機の stale 再配送弾き専用なので、テスト時は「押されている」とみなして production
    // 分岐 (実機でのみ意味を持つ) を迂回する。
    #[cfg(test)]
    {
        let _ = key;
        true
    }
    #[cfg(not(test))]
    {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        // high bit (0x8000) = 現在押下中。呼び出し時点の物理状態を返す。
        unsafe { (GetAsyncKeyState(key.virtual_key as i32) as u16 & 0x8000) != 0 }
    }
}

#[cfg(windows)]
fn main_iconic_video_source_enabled(
    detached_or_switching: bool,
    is_video: bool,
    music_view_active: bool,
) -> bool {
    !detached_or_switching && is_video && !music_view_active
}

/// 動画ピン留めの「ピン位置のフレームをサムネ DB に書き戻す」非同期待ち用。
///
/// ピン留めボタンが押されたが thumb worker キャッシュにそのフレームがまだ無い場合、
/// 1 件だけ pending を保持して `tick_pending_pin_thumb_refresh` が後続フレームで
/// 完了をポーリングし、揃ったところで `VideoPinDb::set_pin` を再呼び出ししてグリッド
/// サムネに反映する (`video_thumb_overrides_dirty_paths` を立て直す)。
#[cfg(windows)]
pub(crate) struct PendingPinThumbRefresh {
    pub(crate) fs_idx: usize,
    pub(crate) path: std::path::PathBuf,
    pub(crate) pts: f64,
    pub(crate) started_at: std::time::Instant,
}

#[cfg(windows)]
enum MarkerThumbSave {
    Pin {
        pts_secs: f64,
        thumb_webp: Vec<u8>,
        cached: VideoMarkerCachedThumbnail,
    },
    Bookmark {
        id: i64,
        thumb_webp: Vec<u8>,
        cached: VideoMarkerCachedThumbnail,
    },
    Chapter {
        pts_secs: f64,
        thumb_webp: Vec<u8>,
        cached: VideoMarkerCachedThumbnail,
    },
}

#[cfg(windows)]
#[derive(Clone, Copy)]
enum MarkerThumbSaveTarget {
    Pin,
    Bookmark { id: i64 },
    Chapter,
}

#[cfg(windows)]
fn encode_native_overlay_thumbnail_webp(
    thumbnail: &crate::video::native_presenter::NativeOverlayThumbnail,
) -> Option<Vec<u8>> {
    let expected_len = thumbnail.width as usize * thumbnail.height as usize * 4;
    if thumbnail.width == 0 || thumbnail.height == 0 || thumbnail.rgba.len() != expected_len {
        return None;
    }
    let encoder = webp::Encoder::from_rgba(&thumbnail.rgba, thumbnail.width, thumbnail.height);
    let webp = encoder.encode(75.0).to_vec();
    (!webp.is_empty()).then_some(webp)
}

#[cfg(windows)]
fn encode_native_overlay_tile_thumbnail_webp(
    thumbnail: &crate::video::native_presenter::NativeOverlayTileThumbnail,
) -> Option<Vec<u8>> {
    let expected_len = thumbnail.width as usize * thumbnail.height as usize * 4;
    if thumbnail.width == 0 || thumbnail.height == 0 || thumbnail.rgba.len() != expected_len {
        return None;
    }
    let encoder =
        webp::Encoder::from_rgba(thumbnail.rgba.as_slice(), thumbnail.width, thumbnail.height);
    let webp = encoder.encode(75.0).to_vec();
    (!webp.is_empty()).then_some(webp)
}

#[cfg(windows)]
struct NativeVideoSourceSwapStarted {
    from_idx: usize,
    target_idx: usize,
    target_path: std::path::PathBuf,
    source_epoch: u64,
    started_at: std::time::Instant,
}

#[cfg(windows)]
pub(crate) struct NativeVideoOpenPending {
    pub(crate) idx: usize,
    pub(crate) path: std::path::PathBuf,
    pub(crate) from_grid: bool,
    pub(crate) autoplay_override: Option<bool>,
    pub(crate) ignore_resume: bool,
    pub(crate) wait_for_detached_host: bool,
    pub(crate) requested_at: std::time::Instant,
    pub(crate) deadline: std::time::Instant,
    pub(crate) input_seq: u64,
    /// ParkedLive mount 中に生成された pending の owner window id。
    /// VST3 deferred open からも parked mount 中に到達するため mounted 専用ではない。
    pub(crate) parked_live_window_id: Option<u64>,
}

#[cfg(windows)]
const NATIVE_VIDEO_NAV_SWAP_DEBOUNCE_MS: u64 = 120;

#[cfg(windows)]
pub(crate) fn video_mtime_secs_for_resume_thumb(path: &std::path::Path) -> i64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(windows)]
pub(crate) struct NativeVideoSourceSwapPending {
    pub(crate) from_idx: usize,
    pub(crate) target_idx: usize,
    pub(crate) target_path: std::path::PathBuf,
    pub(crate) native_output: crate::video::NativeVideoOutput,
    pub(crate) autoplay_override: Option<bool>,
    pub(crate) ignore_resume: bool,
    pub(crate) show_preparing_overlay: bool,
    pub(crate) reason: &'static str,
    pub(crate) requested_at: std::time::Instant,
    pub(crate) deadline: std::time::Instant,
    pub(crate) input_seq: u64,
    pub(crate) history_trigger: crate::app::HistoryTrigger,
    pub(crate) cursor_state: crate::ui_fullscreen::FullscreenCursorState,
    /// ParkedLive poll 中に enqueue された source-swap なら、その owner window id。
    /// Completion が別フレームへずれても通常 `open_fullscreen` 経路へ漏らさないため、
    /// enqueue 時点の事実として焼き込む。
    pub(crate) parked_live_window_id: Option<u64>,
    /// Inc 7: この swap 完了後に「動画→音声モード」を再確立する (音声モードの動画が連続
    /// 再生 EOF で次動画へ送られたケース)。true なら completion の `open_fullscreen` 後に
    /// `enter_video_audio_mode` を呼び、hidden presenter を維持したまま音楽ビューへ戻す。
    pub(crate) audio_mode_after_swap: bool,
}

#[cfg(windows)]
#[derive(Clone)]
pub(crate) struct VideoResumePreviewCacheEntry {
    pub(crate) path_key: String,
    pub(crate) timestamp_ms: i64,
    pub(crate) video_mtime: i64,
    pub(crate) min_tile_w: u32,
    pub(crate) thumbnail: crate::video::native_presenter::NativeOverlayTileThumbnail,
}

#[cfg(windows)]
const VIDEO_RESUME_PREVIEW_SESSION_CACHE_CAP: usize = 8;

/// Norm 操作 (toggle ON / OFF / scan 完了) を 1 セットで適用するヘルパー。
///
/// 3 箇所 (`disable_normalize_globally` / `apply_normalize_gain_db_to_player` /
/// scan 完了パス) を集約することで perf event の漏れを防ぐ。
///
/// ## 実装方針 (= 2026-05-11 修正、Codex 助言)
///
/// **`clear_audio_output_buffer()` を呼ばない**。`set_normalize_gain` だけを呼ぶ。
/// 理由:
/// - 汎用 `clear_audio_output_buffer` は `raw_pending` (= 通常 5 秒分の先読み) を
///   捨てる。Norm では decoder flush しないので、捨てた直後に届く新しい audio frame
///   の audible PTS が master clock から 5 秒先行し、wall-rate cap で追従できず、
///   **A/V offset = −5000ms 級の永続ズレ**が残った (= 過去ログで実測、各 toggle 毎に
///   累積し最終的に −20s に達した)。
/// - clear せずに `set_normalize_gain` だけ呼ぶと、既存 `processed` の最大 ~100ms 分は
///   旧 gain のまま再生されるが、`raw_pending` 経由で新 gain が次の chunk から自然に
///   反映される。100ms 程度の音量ズレは知覚しにくく、A/V offset は飛ばない。
///
/// ## 計装内容
/// - `video.norm_apply_begin`: 直前の master clock pos / 最後に表示した video PTS
/// - `video.norm_apply_end`: 適用後の master clock pos
///
/// 修正前後の比較は `analyze_perf.py av_drift` で `A/V offset` を見ればよい
/// (= 修正前は累積 −20s 級、修正後は ±数十 ms に収まるはず)。
#[cfg(windows)]
pub(super) fn apply_normalize_gain_with_perf(
    player: &crate::video::VideoPlayer,
    fs_idx: usize,
    new_gain_linear: f64,
    new_gain_db: f32,
    reason: &'static str,
) {
    if crate::perf::is_enabled() {
        crate::perf::event(
            "video",
            "norm_apply_begin",
            None,
            0,
            &[
                ("fs_idx", serde_json::Value::from(fs_idx as i64)),
                ("gain_db", serde_json::Value::from(new_gain_db as f64)),
                ("reason", serde_json::Value::from(reason)),
                ("now", serde_json::Value::from(player.position())),
                (
                    "video_pts",
                    serde_json::Value::from(player.last_displayed_pts_secs().unwrap_or(f64::NAN)),
                ),
            ],
        );
    }
    // ⚠️ clear_audio_output_buffer() は呼ばない (上記 doc コメント参照)。
    // set_normalize_gain は atomic store だけで buffer は触らないので、
    // 既存 processed (~100ms) は旧 gain で鳴り続け、その後 raw_pending 経由で
    // 新 gain に切り替わる。A/V offset は連続性を保つ。
    player.set_normalize_gain(new_gain_linear);
    if crate::perf::is_enabled() {
        crate::perf::event(
            "video",
            "norm_apply_end",
            None,
            0,
            &[
                ("fs_idx", serde_json::Value::from(fs_idx as i64)),
                ("now", serde_json::Value::from(player.position())),
            ],
        );
    }
}

impl App {
    #[cfg(windows)]
    pub(crate) fn sync_native_video_grade(&mut self) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        let grade = self.creative_lut_library.video_snapshot(
            &self.settings.creative_luts,
            &self.settings.video_adjustments,
            &self.settings.video_preset_slots,
        );
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_video_grade(grade);
        }
    }

    #[cfg(windows)]
    pub(crate) fn save_video_adjust_slot(&mut self, slot_idx: usize) {
        if slot_idx >= self.settings.video_preset_slots.slots.len() {
            return;
        }

        let existing_name = self.settings.video_preset_slots.slots[slot_idx]
            .as_ref()
            .map(|slot| slot.name.trim().to_string())
            .filter(|name| !name.is_empty());
        let key_label = crate::adjustment::slot_key_label(slot_idx);
        let name = existing_name.unwrap_or_else(|| format!("Slot {key_label}"));
        self.settings.video_preset_slots.slots[slot_idx] =
            Some(crate::creative_lut::VideoPresetSlot {
                name: name.clone(),
                adjustments: self.settings.video_adjustments.clone(),
            });
        self.sync_native_video_grade();
        self.settings.save();
        self.show_feedback_toast(format!("[動画スロット{key_label}: {name} 保存]"));
    }

    #[cfg(windows)]
    pub(crate) fn load_video_adjust_slot(&mut self, slot_idx: usize) {
        if slot_idx >= self.settings.video_preset_slots.slots.len() {
            return;
        }

        let key_label = crate::adjustment::slot_key_label(slot_idx);
        let Some(slot) = self.settings.video_preset_slots.slots[slot_idx].clone() else {
            self.show_feedback_toast(format!("[動画スロット{key_label} は空です]"));
            return;
        };
        self.settings.video_adjustments = slot.adjustments;
        self.settings.video_adjustments.sanitize();
        self.sync_native_video_grade();
        self.settings.save();
        self.show_feedback_toast(format!("[動画スロット{key_label}: {}]", slot.name));
    }

    #[cfg(windows)]
    fn video_mtime_secs(path: &std::path::Path) -> i64 {
        video_mtime_secs_for_resume_thumb(path)
    }

    #[cfg(windows)]
    fn format_navigation_preview_time(secs: f64) -> String {
        let secs = secs.max(0.0).round() as u64;
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        if h > 0 {
            format!("{h}:{m:02}:{s:02}")
        } else {
            format!("{m}:{s:02}")
        }
    }

    #[cfg(windows)]
    fn lookup_video_resume_preview_thumbnail(
        &mut self,
        path: &std::path::Path,
    ) -> Option<crate::video::native_presenter::NativeOverlayTileThumbnail> {
        let key = crate::adjustment_db::normalize_path(path);
        let pts = *self.settings.video_resume_positions.get(&key)?;
        if !pts.is_finite() || pts < super::VIDEO_RESUME_MIN_POSITION_SECS {
            return None;
        }
        let timestamp_ms = (pts * 1000.0).round() as i64;
        let video_mtime = Self::video_mtime_secs(path);
        let min_tile_w = crate::settings::VIDEO_RESUME_PREVIEW_EXTRACT_WIDTH;
        if let Some(pos) = self.video_resume_preview_cache.iter().position(|entry| {
            entry.path_key == key
                && entry.timestamp_ms == timestamp_ms
                && entry.video_mtime == video_mtime
                && entry.min_tile_w == min_tile_w
        }) {
            let entry = self.video_resume_preview_cache.remove(pos)?;
            let thumbnail = entry.thumbnail.clone();
            self.video_resume_preview_cache.push_front(entry);
            return Some(thumbnail);
        }
        let cache = self.video_tile_cache.as_ref()?;
        let (cached_timestamp_ms, webp) =
            cache.lookup_resume_webp(path, video_mtime, min_tile_w)?;
        if cached_timestamp_ms != timestamp_ms {
            return None;
        }
        let (width, height, rgba) = crate::catalog::decode_thumb_to_rgba(&webp)?;
        let thumbnail = crate::video::native_presenter::NativeOverlayTileThumbnail {
            target_secs: pts,
            width,
            height,
            rgba: std::sync::Arc::new(rgba),
        };
        self.video_resume_preview_cache
            .push_front(VideoResumePreviewCacheEntry {
                path_key: key,
                timestamp_ms,
                video_mtime,
                min_tile_w,
                thumbnail: thumbnail.clone(),
            });
        while self.video_resume_preview_cache.len() > VIDEO_RESUME_PREVIEW_SESSION_CACHE_CAP {
            self.video_resume_preview_cache.pop_back();
        }
        Some(thumbnail)
    }

    #[cfg(windows)]
    fn native_video_navigation_preview_for_path(
        &mut self,
        path: &std::path::Path,
    ) -> crate::video::native_presenter::NativeOverlayNavigationPreview {
        let file_name = path
            .file_name()
            .and_then(|s| s.to_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or("video")
            .to_string();
        let thumbnail = self.lookup_video_resume_preview_thumbnail(path);
        let subtitle = if let Some(thumbnail) = thumbnail.as_ref() {
            format!(
                "保存済み位置 {} を表示中 - 再生準備中...",
                Self::format_navigation_preview_time(thumbnail.target_secs)
            )
        } else {
            "プレビュー未保存 - 再生準備中...".to_string()
        };
        crate::video::native_presenter::NativeOverlayNavigationPreview {
            file_name,
            subtitle,
            thumbnail,
        }
    }

    #[cfg(windows)]
    pub(super) fn defer_native_video_open_if_decoder_busy(
        &mut self,
        idx: usize,
        path: &std::path::Path,
        from_grid: bool,
        autoplay_override: Option<bool>,
        ignore_resume: bool,
    ) -> bool {
        if !self.settings.video_hw_decode {
            return false;
        }
        let max_live_video_decode_threads = crate::video::decoder::MAX_LIVE_VIDEO_DECODE_THREADS;
        let live_decoders = crate::video::decoder::LIVE_VIDEO_DECODE_THREADS
            .load(std::sync::atomic::Ordering::Acquire);
        if live_decoders < max_live_video_decode_threads {
            return false;
        }

        let now = std::time::Instant::now();
        self.native_video_open_pending = Some(NativeVideoOpenPending {
            idx,
            path: path.to_path_buf(),
            from_grid,
            autoplay_override,
            ignore_resume,
            wait_for_detached_host: false,
            requested_at: now,
            deadline: now + std::time::Duration::from_secs(10),
            input_seq: self.input_seq,
            parked_live_window_id: self.native_video_parked_live_input_window_id,
        });
        crate::logger::log(format!(
            "[native-video] defer regular open: idx={idx} live_video_decode_threads={live_decoders} max={max_live_video_decode_threads}"
        ));
        if crate::perf::is_enabled() {
            crate::perf::event(
                "native_video",
                "regular_open_deferred",
                None,
                self.input_seq,
                &[
                    ("idx", serde_json::Value::from(idx as i64)),
                    (
                        "live_video_decode_threads",
                        serde_json::Value::from(live_decoders as i64),
                    ),
                    (
                        "max",
                        serde_json::Value::from(max_live_video_decode_threads as i64),
                    ),
                ],
            );
        }
        true
    }

    #[cfg(windows)]
    pub(super) fn defer_native_video_open_until_detached_host(
        &mut self,
        idx: usize,
        path: &std::path::Path,
        from_grid: bool,
        autoplay_override: Option<bool>,
        ignore_resume: bool,
    ) -> bool {
        let now = std::time::Instant::now();
        self.native_video_open_pending = Some(NativeVideoOpenPending {
            idx,
            path: path.to_path_buf(),
            from_grid,
            autoplay_override,
            ignore_resume,
            wait_for_detached_host: true,
            requested_at: now,
            deadline: now + std::time::Duration::from_secs(10),
            input_seq: self.input_seq,
            parked_live_window_id: self.native_video_parked_live_input_window_id,
        });
        crate::logger::log(format!(
            "[native-video] defer regular open until detached host is ready: idx={idx}"
        ));
        true
    }

    #[cfg(windows)]
    pub(super) fn poll_native_video_open_pending(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.native_video_open_pending.as_ref() else {
            return;
        };
        if !Self::parked_source_swap_poll_owner_matches(
            pending.parked_live_window_id,
            self.native_video_parked_live_input_window_id,
        ) {
            return;
        }
        let idx = pending.idx;
        let path = pending.path.clone();
        let from_grid = pending.from_grid;
        let autoplay_override = pending.autoplay_override;
        let ignore_resume = pending.ignore_resume;
        let wait_for_detached_host = pending.wait_for_detached_host;
        let requested_at = pending.requested_at;
        let deadline = pending.deadline;
        let input_seq = pending.input_seq;
        let parked_live_window_id = pending.parked_live_window_id;

        if self.fullscreen_idx != Some(idx)
            || !matches!(self.items.get(idx), Some(GridItem::Video(p)) if p == &path)
            || self.fs_cache.contains_key(&idx)
        {
            self.native_video_open_pending = None;
            return;
        }

        let now = std::time::Instant::now();
        if wait_for_detached_host && !self.detached_viewer_video_host_ready() {
            if now >= deadline {
                self.native_video_open_pending = None;
                crate::logger::log(format!(
                    "[native-video] detached host wait timeout: idx={idx} waited_ms={:.1}",
                    requested_at.elapsed().as_secs_f64() * 1000.0
                ));
                self.show_feedback_toast(
                    "別ウィンドウの準備に失敗したため動画を開けませんでした".to_string(),
                );
                if let Some(window_id) = parked_live_window_id {
                    self.request_parked_live_media_close_after_poll(
                        window_id,
                        "parked_native_open_host_timeout",
                    );
                    ctx.request_repaint();
                    return;
                }
                self.close_fullscreen();
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(
                    deadline
                        .saturating_duration_since(now)
                        .min(std::time::Duration::from_millis(16)),
                );
            }
            return;
        }

        let max_live_video_decode_threads = crate::video::decoder::MAX_LIVE_VIDEO_DECODE_THREADS;
        let live_decoders = crate::video::decoder::LIVE_VIDEO_DECODE_THREADS
            .load(std::sync::atomic::Ordering::Acquire);
        if live_decoders < max_live_video_decode_threads {
            self.native_video_open_pending = None;
            self.fs_open_intent_from_grid = from_grid;
            self.fs_video_open_autoplay_override = autoplay_override;
            self.fs_video_open_ignore_resume_once = ignore_resume;
            crate::logger::log(format!(
                "[native-video] resume deferred regular open: idx={idx} waited_ms={:.1} live_video_decode_threads={live_decoders}",
                requested_at.elapsed().as_secs_f64() * 1000.0
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_video",
                    "regular_open_deferred_start",
                    None,
                    input_seq,
                    &[
                        ("idx", serde_json::Value::from(idx as i64)),
                        (
                            "wait_ms",
                            serde_json::Value::from(requested_at.elapsed().as_secs_f64() * 1000.0),
                        ),
                        (
                            "live_video_decode_threads",
                            serde_json::Value::from(live_decoders as i64),
                        ),
                    ],
                );
            }
            self.start_fs_load(idx);
            ctx.request_repaint();
            return;
        }

        if now >= deadline {
            self.native_video_open_pending = None;
            crate::logger::log(format!(
                "[native-video] deferred regular open timeout: idx={idx} waited_ms={:.1} live_video_decode_threads={live_decoders}",
                requested_at.elapsed().as_secs_f64() * 1000.0
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_video",
                    "regular_open_deferred_timeout",
                    None,
                    input_seq,
                    &[
                        ("idx", serde_json::Value::from(idx as i64)),
                        (
                            "wait_ms",
                            serde_json::Value::from(requested_at.elapsed().as_secs_f64() * 1000.0),
                        ),
                        (
                            "live_video_decode_threads",
                            serde_json::Value::from(live_decoders as i64),
                        ),
                    ],
                );
            }
            self.show_feedback_toast("前の動画デコード終了待ちがタイムアウトしました".to_string());
            if let Some(window_id) = parked_live_window_id {
                self.request_parked_live_media_close_after_poll(
                    window_id,
                    "parked_native_open_decoder_timeout",
                );
                ctx.request_repaint();
                return;
            }
            self.close_fullscreen();
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(
                deadline
                    .saturating_duration_since(now)
                    .min(std::time::Duration::from_millis(100)),
            );
        }
    }

    #[cfg(windows)]
    pub(crate) fn detached_video_host_switch_pending(&self) -> bool {
        self.pending_detached_video_host_switch.is_some()
    }

    #[cfg(windows)]
    fn defer_native_video_switch_until_detached_host(
        &mut self,
        target_presentation: ViewerPresentation,
        activate_on_show: bool,
    ) {
        let now = std::time::Instant::now();
        self.pending_detached_video_host_switch = Some(super::DetachedVideoHostSwitchPending {
            target_presentation,
            activate_on_show,
            requested_at: now,
            deadline: now + std::time::Duration::from_secs(5),
        });
        crate::logger::log(format!(
            "[native-video] defer placement switch until detached host is ready: \
             target={target_presentation:?} activate={activate_on_show}"
        ));
    }

    #[cfg(windows)]
    pub(super) fn poll_detached_video_host_switch_pending(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_detached_video_host_switch else {
            return;
        };
        let Some(idx) = self.fullscreen_idx else {
            self.pending_detached_video_host_switch = None;
            return;
        };
        if !matches!(self.items.get(idx), Some(GridItem::Video(_))) {
            self.pending_detached_video_host_switch = None;
            return;
        }
        if !matches!(
            pending.target_presentation,
            ViewerPresentation::DetachedWindow
        ) {
            self.pending_detached_video_host_switch = None;
            return;
        }

        let now = std::time::Instant::now();
        if self.detached_viewer_video_host_ready() {
            self.pending_detached_video_host_switch = None;
            crate::logger::log(format!(
                "[native-video] resume deferred detached placement switch after {:.1}ms",
                pending.requested_at.elapsed().as_secs_f64() * 1000.0
            ));
            self.switch_native_video_viewer_presentation(
                pending.target_presentation,
                pending.activate_on_show,
            );
            ctx.request_repaint();
        } else if now >= pending.deadline {
            self.pending_detached_video_host_switch = None;
            crate::logger::log(format!(
                "[native-video] detached placement switch host wait timed out after {:.1}ms",
                pending.requested_at.elapsed().as_secs_f64() * 1000.0
            ));
            self.show_feedback_toast(
                "別ウィンドウの準備に失敗したため動画表示を切り替えられませんでした".to_string(),
            );
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(
                pending
                    .deadline
                    .saturating_duration_since(now)
                    .min(std::time::Duration::from_millis(16)),
            );
        }
    }

    /// detached 動画再生中に host HWND が変わったとき、presenter child を現 host へ
    /// 再親付けするための一連の仕組み (host capture race 対策)。detached の egui viewport は
    /// 切替 (main⇔detached) をまたぐと OS window (host HWND) が作り直されることがあり、旧
    /// host の子として残った presenter child (WS_CHILD) が旧 host 破棄で道連れ死 → WM_QUIT →
    /// presenter 終了 → 再生終了 という既存バグを、**常に現在の detached host へ追従させる**
    /// ことで防ぐ。
    ///
    /// detached 動画が現在の presentation か、または切替中の target になっているか。
    /// host resync の適用可否判定に使う。**切替中 (target=DetachedWindow) も含める**のが
    /// 肝で、initial main→detached の switch 進行中に host が変わったケースも取りこぼさない。
    #[cfg(windows)]
    pub(crate) fn detached_video_presentation_active_or_targeted(&self) -> bool {
        let is_video = self
            .fullscreen_idx
            .map(|idx| matches!(self.items.get(idx), Some(GridItem::Video(_))))
            .unwrap_or(false);
        if !is_video {
            return false;
        }
        matches!(self.viewer_presentation, ViewerPresentation::DetachedWindow)
            || self
                .native_video_mode_switch
                .map(|p| matches!(p.target_presentation, ViewerPresentation::DetachedWindow))
                .unwrap_or(false)
    }

    /// Inc 7 (動画→音声モード): この動画の native presenter は hidden のまま保持され、
    /// egui の音楽ビューが表示の責務を持つ。VST ホスト表示中だけは presenter を
    /// intentionally un-hide して使うため、通常の native presenter 経路へ戻す。
    #[cfg(windows)]
    pub(crate) fn video_audio_mode_hides_native_presenter_for(&self, fs_idx: usize) -> bool {
        self.video_audio_mode == Some(fs_idx) && !self.video_audio_vst_active_for(fs_idx)
    }

    #[cfg(windows)]
    pub(crate) fn detached_video_host_resync_reason(
        child_alive: bool,
        child_parent_hwnd: Option<u64>,
        current_host_hwnd: u64,
    ) -> Option<&'static str> {
        if !child_alive {
            Some("child_lost")
        } else if child_parent_hwnd != Some(current_host_hwnd) {
            Some("parent_changed")
        } else {
            None
        }
    }

    #[cfg(windows)]
    fn detached_video_current_host_geometry_settled(&self) -> bool {
        if let Some(window_id) = self.native_video_parked_live_input_window_id {
            // Passive host の HWND を作り直したフレームでは
            // detached_window_hwnd_clear_if_dead がこの latch を false に戻す。保存 placement
            // を新 host へ適用した後のフレームまで presenter rebuild を待つことで、egui の
            // 既定 822x656 host へ一度付け替えるちらつきを避ける。
            self.detached_image_windows
                .iter()
                .find(|window| window.id == window_id)
                .is_some_and(|window| window.initial_placement_applied)
        } else {
            self.detached_viewer_host_geometry_settled
        }
    }

    /// detached host 変更に対する presenter child の再親付けを 1 回試みる。
    /// 戻り値 `true` = 「解決した (再親付け発行 or そもそも不要)」→ 要求フラグを落としてよい。
    /// `false` = 「保留 (mode switch 進行中 / host 未確定などで今は発行できない)」→ フラグを
    /// 残して次フレーム以降に再試行する。旧 host 破棄で presenter が道連れ死する race 窓を
    /// 最小化するため capture 時に即時呼び、取りこぼしを poll で拾う。
    #[cfg(windows)]
    pub(crate) fn try_resync_detached_video_host(&mut self) -> bool {
        if !self.detached_video_presentation_active_or_targeted() {
            return true; // 再親付け対象でない → 要求は破棄してよい
        }
        // Inc 7 (動画→音声モード): presenter は hidden のまま保持され、表示の責務は egui 音楽
        // ビューに移っている。この状態で host resync / SwitchPlacement を走らせると presenter が
        // 再表示され、「video_audio_mode は Some なのに動画が出る」不整合になる。音声モード中の
        // host resync は不要として解決済みにし、正規 exit (Z/Esc/♪) だけが presenter を
        // un-hide / re-place できるようにする。VST ホスト表示中は presenter を意図的に使うため
        // 通常経路へ戻す。
        if self
            .fullscreen_idx
            .is_some_and(|idx| self.video_audio_mode_hides_native_presenter_for(idx))
        {
            return true;
        }
        if self.native_video_mode_switch.is_some() {
            return false; // 切替中は重ねられない。完了後に再試行
        }
        // publish HWND は presenter が今実際に出している committed window を指す。ただし
        // child 自体が生きていることだけでは表示先の正しさを保証できない。tray restore では
        // egui host が新 HWND へ作り直された後も child が旧 host の子として生存し、フレームを
        // present し続けながら新 host は黒、という状態になり得る。現在の registry host と
        // GetParent(child) を比較し、child lost と parent mismatch の両方を再同期対象にする。
        let child_hwnd = self
            .fullscreen_idx
            .and_then(|idx| match self.fs_cache.get(&idx) {
                Some(FsCacheEntry::Video { player, .. }) => Some(player.native_presenter_hwnd()),
                _ => None,
            })
            .unwrap_or(0);
        if child_hwnd == 0 {
            return true; // presenter 未確立 → 追従不要
        }
        let current_host_hwnd = self.detached_viewer_host_hwnd_raw();
        if current_host_hwnd == 0 {
            return false; // host registry の再取得待ち。要求を保持して次フレームに再試行。
        }
        let child_alive = crate::video::native_window::is_window_alive(child_hwnd);
        let child_parent_hwnd = child_alive
            .then(|| crate::video::native_window::window_parent(child_hwnd))
            .flatten();
        let Some(reason) = Self::detached_video_host_resync_reason(
            child_alive,
            child_parent_hwnd,
            current_host_hwnd,
        ) else {
            return true; // child は現在の registry host に正しく接続済み。
        };
        if !self.detached_video_current_host_geometry_settled() {
            return false;
        }
        let queued = self.sync_detached_video_child_presenter_rect();
        if queued {
            crate::logger::log(format!(
                "[native-video] detached child host mismatch -> resync presenter \
                 reason={reason} child=0x{child_hwnd:x} child_parent={} \
                 current_host=0x{current_host_hwnd:x} host_generation={}",
                child_parent_hwnd
                    .map(|parent| format!("0x{parent:x}"))
                    .unwrap_or_else(|| "none".to_string()),
                self.detached_viewer_host_generation
            ));
        }
        queued
    }

    /// 毎フレーム、detached child の実親が現在の registry host と一致するか確認し、
    /// child の破棄または親不一致なら現 host へ再親付けして黒表示を復旧する。
    #[cfg(windows)]
    pub(super) fn poll_detached_video_host_resync(&mut self) {
        // detached 動画でなくなったら再親付け要求を掃除する (次 session へ持ち越さない)。
        if !self.detached_video_presentation_active_or_targeted() {
            self.pending_detached_video_host_resync = false;
            return;
        }
        // 「host 変更フラグ待ち」ではなく毎フレーム child の生存と実親を確認する。
        // registration と旧 host teardown の順序に依存せず、遅延した親不一致も拾う。
        if self.try_resync_detached_video_host() {
            self.pending_detached_video_host_resync = false;
        }
    }

    #[cfg(windows)]
    pub(super) fn defer_native_video_source_swap_until_decoder_free(
        &mut self,
        ctx: &egui::Context,
        target_idx: usize,
        autoplay_override: Option<bool>,
        ignore_resume: bool,
        show_preparing_overlay: bool,
        reason: &'static str,
        history_trigger: crate::app::HistoryTrigger,
    ) -> bool {
        let target_path = match self.items.get(target_idx).cloned() {
            Some(GridItem::Video(path)) => path,
            _ => {
                // pending 中に画像などへ移動した場合は、保持していた native presenter を
                // 解放して通常の fullscreen 遷移に任せる。
                self.native_video_source_swap_pending = None;
                return false;
            }
        };
        let now = std::time::Instant::now();
        let parked_live_window_id = self.native_video_parked_live_input_window_id;
        // Inc 7: 音声モードの動画が連続再生 EOF で次動画へ送られた swap かどうか
        // (`handle_video_audio_mode_continuous_eof` が直前に立てる one-shot)。true なら
        // presenter を hidden のまま再利用し、swap 完了後に音声モードを再確立する。
        let keep_audio_mode = self.source_swap_keep_audio_mode;
        let navigation_preview = if reason == "navigation" {
            Some(self.native_video_navigation_preview_for_path(&target_path))
        } else {
            None
        };

        if reason != "tile" {
            self.cancel_stale_video_tile_reopen(Some(target_idx), "deferred-update");
        }
        if self.native_video_source_swap_pending.is_some() {
            if self.fullscreen_idx != Some(target_idx) {
                self.reset_fs_side_panel_runtime_for_file_change();
            }
            self.fullscreen_idx = Some(target_idx);
            // Inc 7: 進行中の swap が既に音声モード維持 (audio_mode_after_swap=true) なら、
            // 通常ナビによる update でもその intent を維持する。keep_audio_mode(=この update の
            // 要求) だけで上書きすると、音声モードの swap 完了前に別ナビが入ったとき
            // audio_mode_after_swap が false に潰れ、completion が可視動画を開いて元バグ
            // (黒画面 / 前フレーム固着) に戻る (Codex P1)。通常 swap 同士 (両方 false) は不変。
            let keep_audio_after_update = keep_audio_mode
                || self
                    .native_video_source_swap_pending
                    .as_ref()
                    .is_some_and(|p| p.audio_mode_after_swap);
            // 音声モード維持 swap は fullscreen_idx を target へ進めた瞬間から video_audio_mode も
            // target に合わせる (でないと fs_music_view_active(target) が false になり音楽ビューが
            // 一瞬消える、Codex #5)。
            if keep_audio_after_update {
                self.video_audio_mode = Some(target_idx);
            }
            self.refresh_fullscreen_video_marker_cache(target_idx);
            let pending = self
                .native_video_source_swap_pending
                .as_mut()
                .expect("checked is_some above");
            pending.target_idx = target_idx;
            pending.target_path = target_path;
            pending.autoplay_override = autoplay_override;
            pending.ignore_resume = ignore_resume;
            pending.show_preparing_overlay = show_preparing_overlay;
            pending.reason = reason;
            pending.requested_at = now;
            pending.deadline = now + std::time::Duration::from_secs(10);
            pending.input_seq = self.input_seq;
            pending.history_trigger = history_trigger;
            pending.parked_live_window_id = Self::source_swap_owner_after_update(
                parked_live_window_id,
                pending.parked_live_window_id,
            );
            pending.audio_mode_after_swap = keep_audio_after_update;
            pending
                .native_output
                .set_navigation_preview(navigation_preview);
            crate::logger::log(format!(
                "[native-video] update deferred source swap: reason={reason} target_idx={target_idx} parked_live_window_id={:?}",
                pending.parked_live_window_id
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_video",
                    "source_swap_deferred_update",
                    None,
                    self.input_seq,
                    &[
                        ("target_idx", serde_json::Value::from(target_idx as i64)),
                        ("reason", serde_json::Value::from(reason)),
                    ],
                );
            }
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            return true;
        }

        let Some(from_idx) = self.fullscreen_idx else {
            return false;
        };
        if from_idx == target_idx {
            return true;
        }
        if !matches!(self.items.get(from_idx), Some(GridItem::Video(_))) {
            return false;
        }

        self.save_all_video_resume_positions();
        let native_output = match self.fs_cache.get_mut(&from_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                player.pause_audio_output();
                player.set_playing(false);
                player.clear_audio_output_buffer();
                player.take_native_output()
            }
            _ => None,
        };
        let Some(native_output) = native_output else {
            return false;
        };
        native_output.set_navigation_preview(navigation_preview);
        if reason != "tile" {
            self.cancel_stale_video_tile_reopen(Some(from_idx), "deferred-start");
        }

        // 同時 HW decoder 数 1 運用では、旧 decoder の終了を待つ必要がある。
        // ただし NativeVideoOutput はここで App 側に退避し、presenter HWND / DComp
        // tree は生かしたままにする。normal open に落として旧 VideoPlayer ごと落とすと、
        // presenter HWND が消える 150-300ms の穴で背後のアプリや黒画面が見える。
        if from_idx != target_idx {
            self.cleanup_normalize_state_for_fs_idx(from_idx);
            self.fs_cache.remove(&from_idx);
        }
        self.native_video_open_pending = None;
        if self.fullscreen_idx != Some(target_idx) {
            self.reset_fs_side_panel_runtime_for_file_change();
        }
        self.fullscreen_idx = Some(target_idx);
        // Inc 7: 音声モード維持 swap は fullscreen_idx を target へ進めた瞬間から
        // video_audio_mode も target に合わせて音楽ビューを継続表示する (Codex #5)。旧 idx の
        // player は上で remove 済み、新 player は completion で insert されるが、
        // draw_fs_music_view は player 不在でも空状態で描けるので穴は空かない。
        if keep_audio_mode {
            self.video_audio_mode = Some(target_idx);
        }
        self.refresh_fullscreen_video_marker_cache(target_idx);
        self.native_video_source_swap_pending = Some(NativeVideoSourceSwapPending {
            from_idx,
            target_idx,
            target_path,
            native_output,
            autoplay_override,
            ignore_resume,
            show_preparing_overlay,
            reason,
            requested_at: now,
            deadline: now + std::time::Duration::from_secs(10),
            input_seq: self.input_seq,
            history_trigger,
            cursor_state: self.fullscreen_cursor_state(),
            parked_live_window_id,
            audio_mode_after_swap: keep_audio_mode,
        });
        crate::logger::log(format!(
            "[native-video] defer source swap: reason={reason} from_idx={from_idx} -> target_idx={target_idx} parked_live_window_id={parked_live_window_id:?}"
        ));
        if crate::perf::is_enabled() {
            crate::perf::event(
                "native_video",
                "source_swap_deferred",
                None,
                self.input_seq,
                &[
                    ("from_idx", serde_json::Value::from(from_idx as i64)),
                    ("target_idx", serde_json::Value::from(target_idx as i64)),
                    ("reason", serde_json::Value::from(reason)),
                ],
            );
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
        true
    }

    #[cfg(windows)]
    fn drain_native_video_source_swap_pending_events(&mut self, ctx: &egui::Context) {
        // committed 世代は fs_cache ではなく **退避中の native_output** から読む。source
        // swap 中は presenter が pending 側にあり fs_cache の player は native_output を
        // 持たないため、fs_cache 経由だと committed=0 と誤認して旧世代 close を誤受理する
        // (Codex P1)。placement switch 直後に navigation が deferred swap へ入ったケース。
        let Some((fs_idx, mut committed, parked_live_window_id, events)) = self
            .native_video_source_swap_pending
            .as_ref()
            .map(|pending| {
                (
                    self.fullscreen_idx.unwrap_or(pending.target_idx),
                    pending.native_output.committed_generation(),
                    pending.parked_live_window_id,
                    pending.native_output.drain_events(),
                )
            })
        else {
            return;
        };
        for (_epoch, event) in events {
            match event {
                // window close (× / Alt+F4) も退避中 committed で gate する。通常経路の
                // `handle_native_video_window_event` は fs_cache committed を見るため
                // この drain では使えない。
                crate::video::NativeVideoOutputEvent::Window(
                    crate::video::native_window::NativeVideoWindowEvent::CloseRequested {
                        generation,
                    },
                ) => {
                    if !Self::accept_native_video_close_with_committed(
                        committed,
                        generation,
                        "source_swap_window_close",
                    ) {
                        continue;
                    }
                    if let Some(window_id) = parked_live_window_id {
                        if let Some(pending) = self.native_video_source_swap_pending.take() {
                            pending.native_output.set_navigation_preview(None);
                        }
                        crate::logger::log(format!(
                            "[native-video] parked deferred source swap close ignored: \
                             window_id={window_id} fs_idx={fs_idx} source=window_close"
                        ));
                        return;
                    }
                    self.close_fullscreen();
                    return;
                }
                crate::video::NativeVideoOutputEvent::Window(event) => {
                    self.handle_native_video_window_event(ctx, fs_idx, event);
                }
                crate::video::NativeVideoOutputEvent::NavigateItem { delta, .. } => {
                    self.navigate_native_video_fullscreen(ctx, fs_idx, delta);
                }
                crate::video::NativeVideoOutputEvent::CloseFullscreen { generation } => {
                    if !Self::accept_native_video_close_with_committed(
                        committed,
                        generation,
                        "source_swap_close",
                    ) {
                        continue;
                    }
                    if let Some(window_id) = parked_live_window_id {
                        if let Some(pending) = self.native_video_source_swap_pending.take() {
                            pending.native_output.set_navigation_preview(None);
                        }
                        crate::logger::log(format!(
                            "[native-video] parked deferred source swap close ignored: \
                             window_id={window_id} fs_idx={fs_idx} source=close_fullscreen"
                        ));
                        return;
                    }
                    self.close_fullscreen();
                    return;
                }
                crate::video::NativeVideoOutputEvent::PlacementSwitched {
                    request_id,
                    placement,
                    generation,
                } => {
                    // source swap 中の placement switch も committed を追随させる。退避中の
                    // native_output atomic に write-through して、swap 完了で native_output が
                    // fs_cache へ戻ったあとの通常 handler でも正しい committed が見えるようにする。
                    committed = committed.max(generation);
                    if let Some(pending) = self.native_video_source_swap_pending.as_ref() {
                        pending.native_output.bump_committed_generation(generation);
                    }
                    // presentation/session も通常経路と同じロジックで反映する。ここで落とすと
                    // in-flight な F12 switch が native_video_mode_switch を取り残し、swap 完了
                    // 後に viewer_presentation が古いまま divergence する (Codex 再レビュー)。
                    self.apply_native_video_placement_switch_state(
                        request_id, placement, generation,
                    );
                }
                _ => {}
            }
            if self.native_video_source_swap_pending.is_none() {
                return;
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn poll_native_video_source_swap_pending(&mut self, ctx: &egui::Context) {
        let Some(parked_live_window_id) = self
            .native_video_source_swap_pending
            .as_ref()
            .and_then(|pending| pending.parked_live_window_id)
        else {
            self.drain_native_video_source_swap_pending_events(ctx);
            return self.poll_native_video_source_swap_pending_after_owner_gate(ctx);
        };

        if !Self::parked_source_swap_poll_owner_matches(
            Some(parked_live_window_id),
            self.native_video_parked_live_input_window_id,
        ) {
            return;
        }

        self.drain_native_video_source_swap_pending_events(ctx);
        self.poll_native_video_source_swap_pending_after_owner_gate(ctx);
    }

    #[cfg(windows)]
    pub(crate) fn parked_source_swap_poll_owner_matches(
        pending_window_id: Option<u64>,
        current_parked_input_window_id: Option<u64>,
    ) -> bool {
        pending_window_id.is_none() || pending_window_id == current_parked_input_window_id
    }

    /// F12 の入口が、現在 mount 中の context 所有 source-swap を見ているか。
    /// owner=None は mounted/active の共通 stamp なので、active bundle が unmount 中の
    /// root 処理からは所有扱いしない (review-v2.3.0 追補3: 角度B-1)。
    #[cfg(windows)]
    pub(crate) fn current_context_owns_source_swap_pending(&self) -> bool {
        self.native_video_source_swap_pending
            .as_ref()
            .is_some_and(|pending| {
                pending.parked_live_window_id == self.native_video_parked_live_input_window_id
                    && (self.native_video_parked_live_input_window_id.is_some()
                        || !self.active_detached_viewer_context_contains_video())
            })
    }

    #[cfg(windows)]
    pub(crate) fn source_swap_owner_after_update(
        current_parked_window_id: Option<u64>,
        _previous_pending_owner: Option<u64>,
    ) -> Option<u64> {
        current_parked_window_id
    }

    #[cfg(windows)]
    pub(crate) fn pending_owner_after_context_transition(
        pending_owner: Option<u64>,
        from_owner: Option<u64>,
        to_owner: Option<u64>,
    ) -> Option<u64> {
        if pending_owner == from_owner {
            to_owner
        } else {
            pending_owner
        }
    }

    /// App-global pending の mounted/active と ParkedLive の ownership 遷移。
    #[cfg(windows)]
    pub(crate) fn rebind_native_video_pending_owners(
        &mut self,
        from_owner: Option<u64>,
        to_owner: Option<u64>,
        _reason: &'static str,
    ) {
        if let Some(pending) = self.native_video_source_swap_pending.as_mut() {
            pending.parked_live_window_id = Self::pending_owner_after_context_transition(
                pending.parked_live_window_id,
                from_owner,
                to_owner,
            );
        }
        if let Some(pending) = self.native_video_open_pending.as_mut() {
            pending.parked_live_window_id = Self::pending_owner_after_context_transition(
                pending.parked_live_window_id,
                from_owner,
                to_owner,
            );
        }
        if let Some(pending) = self.native_video_fast_swap_pending.as_mut() {
            pending.parked_live_window_id = Self::pending_owner_after_context_transition(
                pending.parked_live_window_id,
                from_owner,
                to_owner,
            );
        }
        if let Some(pending) = self.video_tile_swap_pending.as_mut() {
            pending.parked_live_window_id = Self::pending_owner_after_context_transition(
                pending.parked_live_window_id,
                from_owner,
                to_owner,
            );
        }
        if let Some(pending) = self.media_navigation_pending.as_mut() {
            pending.owner_window_id = Self::pending_owner_after_context_transition(
                pending.owner_window_id,
                from_owner,
                to_owner,
            );
        }
    }

    #[cfg(windows)]
    pub(crate) fn parked_source_swap_pending_belongs_to_window(
        pending_window_id: Option<u64>,
        window_id: u64,
    ) -> bool {
        pending_window_id == Some(window_id)
    }

    #[cfg(windows)]
    pub(crate) fn discard_parked_source_swap_pending_for_window(
        &mut self,
        window_id: u64,
        reason: &'static str,
    ) -> bool {
        let belongs_to_window =
            self.native_video_source_swap_pending
                .as_ref()
                .is_some_and(|pending| {
                    Self::parked_source_swap_pending_belongs_to_window(
                        pending.parked_live_window_id,
                        window_id,
                    )
                });
        if !belongs_to_window {
            return false;
        }
        if let Some(pending) = self.native_video_source_swap_pending.take() {
            pending.native_output.set_navigation_preview(None);
            crate::logger::log(format!(
                "[native-video] parked deferred source swap discarded: \
                 window_id={window_id} reason={reason} target_idx={} pending_reason={}",
                pending.target_idx, pending.reason
            ));
        }
        true
    }

    /// ParkedLive teardown 時に、その窓が所有する App-global pending だけを破棄する。
    /// (review-v2.3.0 追補2 BA-7: parked pending teardown)
    #[cfg(windows)]
    pub(crate) fn discard_parked_native_video_pending_for_window(
        &mut self,
        window_id: u64,
        reason: &'static str,
    ) -> bool {
        let mut discarded = self.discard_parked_source_swap_pending_for_window(window_id, reason);
        if self
            .native_video_open_pending
            .as_ref()
            .is_some_and(|pending| pending.parked_live_window_id == Some(window_id))
        {
            self.native_video_open_pending = None;
            discarded = true;
        }
        let discarded_fast = self
            .native_video_fast_swap_pending
            .as_ref()
            .is_some_and(|pending| pending.parked_live_window_id == Some(window_id));
        if discarded_fast {
            self.native_video_fast_swap_pending = None;
            discarded = true;
        }
        let discarded_tile = self
            .video_tile_swap_pending
            .as_ref()
            .is_some_and(|pending| pending.parked_live_window_id == Some(window_id));
        if discarded_tile {
            self.video_tile_swap_pending = None;
            discarded = true;
        }
        if discarded_fast || discarded_tile {
            // fast/tile pending 中の追加ナビは App-global companion に残る。owner pending と
            // 同時に落とさないと別 context の poll が drain する (review-v2.3.0 追補3: A-2)。
            self.native_video_deferred_nav_delta = None;
        }
        discarded
    }

    /// close_fullscreen が現在 mount 中の context に属する pending だけを破棄する。
    #[cfg(windows)]
    pub(crate) fn clear_mounted_native_video_pending(&mut self) {
        // promoted active context は owner=None だが、main close 中は bundle が unmounted。
        // media 窓自身の close は active context を take + mount してからここへ来るため false。
        if self.active_detached_viewer_context_contains_video() {
            return;
        }
        if self
            .native_video_source_swap_pending
            .as_ref()
            .is_some_and(|pending| pending.parked_live_window_id.is_none())
            && let Some(pending) = self.native_video_source_swap_pending.take()
        {
            pending.native_output.set_navigation_preview(None);
        }
        if self
            .native_video_open_pending
            .as_ref()
            .is_some_and(|pending| pending.parked_live_window_id.is_none())
        {
            self.native_video_open_pending = None;
        }
        if self
            .native_video_fast_swap_pending
            .as_ref()
            .is_some_and(|pending| pending.parked_live_window_id.is_none())
        {
            self.native_video_fast_swap_pending = None;
        }
    }

    #[cfg(windows)]
    fn poll_native_video_source_swap_pending_after_owner_gate(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.native_video_source_swap_pending.as_ref() else {
            return;
        };
        let target_idx = pending.target_idx;
        let target_path = pending.target_path.clone();
        let requested_at = pending.requested_at;
        let deadline = pending.deadline;
        let input_seq = pending.input_seq;
        let reason = pending.reason;
        let parked_live_window_id = pending.parked_live_window_id;

        if self.fullscreen_idx != Some(target_idx)
            || !matches!(self.items.get(target_idx), Some(GridItem::Video(path)) if path == &target_path)
        {
            if let Some(pending) = self.native_video_source_swap_pending.take() {
                pending.native_output.set_navigation_preview(None);
            }
            let item_kind = self
                .items
                .get(target_idx)
                .map(Self::grid_item_kind_for_source_swap_log)
                .unwrap_or("missing");
            crate::logger::log(format!(
                "[native-video] deferred source swap dropped: reason=context_mismatch \
                 current_fs_idx={:?} target_idx={target_idx} target_item_kind={item_kind} \
                 parked_live_window_id={parked_live_window_id:?}",
                self.fullscreen_idx
            ));
            return;
        }

        if pending.native_output.is_closed() {
            if let Some(pending) = self.native_video_source_swap_pending.take() {
                pending.native_output.set_navigation_preview(None);
            }
            crate::logger::log(format!(
                "[native-video] deferred source swap aborted: presenter closed target_idx={target_idx}"
            ));
            self.show_feedback_toast("動画プレゼンターが閉じられました".to_string());
            if let Some(window_id) = parked_live_window_id {
                crate::logger::log(format!(
                    "[native-video] parked deferred source swap aborted; closing owner after poll: \
                     window_id={window_id} target_idx={target_idx} reason=presenter_closed"
                ));
                // mount 中 bundle を先に snapshot へ戻し、共通 teardown seam で owner 窓だけ閉じる。
                // close_fullscreen は mounted main を誤って閉じるため呼ばない。
                // (review-v2.3.0 追補7: R1 第2波 P2-1)
                self.request_parked_live_media_close_after_poll(
                    window_id,
                    "source_swap_presenter_closed",
                );
                ctx.request_repaint();
                return;
            }
            self.close_fullscreen();
            return;
        }

        if reason == "navigation" {
            let debounce = std::time::Duration::from_millis(NATIVE_VIDEO_NAV_SWAP_DEBOUNCE_MS);
            let elapsed = requested_at.elapsed();
            if elapsed < debounce {
                ctx.request_repaint_after(debounce - elapsed);
                return;
            }
        }

        let max_live_video_decode_threads = crate::video::decoder::MAX_LIVE_VIDEO_DECODE_THREADS;
        let live_decoders = crate::video::decoder::LIVE_VIDEO_DECODE_THREADS
            .load(std::sync::atomic::Ordering::Acquire);
        if live_decoders >= max_live_video_decode_threads {
            let now = std::time::Instant::now();
            if now >= deadline {
                if let Some(pending) = self.native_video_source_swap_pending.take() {
                    pending.native_output.set_navigation_preview(None);
                }
                crate::logger::log(format!(
                    "[native-video] deferred source swap timeout: reason={reason} target_idx={target_idx} waited_ms={:.1} live_video_decode_threads={live_decoders}",
                    requested_at.elapsed().as_secs_f64() * 1000.0
                ));
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "native_video",
                        "source_swap_deferred_timeout",
                        None,
                        input_seq,
                        &[
                            ("target_idx", serde_json::Value::from(target_idx as i64)),
                            (
                                "wait_ms",
                                serde_json::Value::from(
                                    requested_at.elapsed().as_secs_f64() * 1000.0,
                                ),
                            ),
                            (
                                "live_video_decode_threads",
                                serde_json::Value::from(live_decoders as i64),
                            ),
                            ("reason", serde_json::Value::from(reason)),
                        ],
                    );
                }
                self.show_feedback_toast(
                    "前の動画デコード終了待ちがタイムアウトしました".to_string(),
                );
                if let Some(window_id) = parked_live_window_id {
                    crate::logger::log(format!(
                        "[native-video] parked deferred source swap timed out; closing owner after poll: \
                         window_id={window_id} target_idx={target_idx} reason={reason}"
                    ));
                    // (review-v2.3.0 追補7: R1 第2波 P2-1)
                    self.request_parked_live_media_close_after_poll(
                        window_id,
                        "source_swap_decoder_timeout",
                    );
                    ctx.request_repaint();
                    return;
                }
                self.close_fullscreen();
            } else {
                ctx.request_repaint_after(
                    deadline
                        .saturating_duration_since(now)
                        .min(std::time::Duration::from_millis(50)),
                );
            }
            return;
        }

        let pending = match self.native_video_source_swap_pending.take() {
            Some(pending) => pending,
            None => return,
        };
        let NativeVideoSourceSwapPending {
            from_idx,
            target_idx,
            target_path,
            native_output,
            autoplay_override,
            ignore_resume,
            show_preparing_overlay,
            reason,
            requested_at,
            input_seq,
            history_trigger,
            cursor_state,
            parked_live_window_id,
            audio_mode_after_swap,
            ..
        } = pending;

        let source_epoch = self.next_native_video_source_epoch();
        let started_at = std::time::Instant::now();
        self.activity_gate.bump();
        let (mut new_player, start_normalize_scan_before_play) = self.build_video_player_for_open(
            target_idx,
            target_path.clone(),
            false,
            autoplay_override,
            ignore_resume,
            crate::video::VideoOutputConsumer::Presentation,
            None,
        );
        // 音声モードを維持する source swap は、decoder event を engine が取り込む前に
        // audio-only ファイルと同じ readiness 要件へ切り替える。presenter は引き続き
        // hidden consume-and-hold だが、再生開始は FirstFrameReady に依存しない。
        if audio_mode_after_swap {
            new_player.set_media_visual_mode(music_core::MediaVisualMode::Music);
        }
        new_player.attach_native_output(native_output);
        let payload = new_player.build_switch_source_payload(source_epoch, show_preparing_overlay);
        new_player.switch_native_source(payload);

        self.fs_cache.insert(
            target_idx,
            FsCacheEntry::Video {
                player: Box::new(new_player),
                load_seq: self.input_seq,
            },
        );
        self.init_normalize_state_for_opened_video(target_idx);
        let completed_via_open_fullscreen = self
            .complete_native_video_deferred_source_swap_viewer_state(
                ctx,
                target_idx,
                cursor_state,
                parked_live_window_id,
                history_trigger,
            );
        if start_normalize_scan_before_play {
            if !self.start_normalize_scan_for_deferred_play_intent(target_idx) {
                self.resume_deferred_normalize_playback_without_scan(target_idx);
            }
        } else {
            self.maybe_start_normalize_scan_for_play_intent(target_idx);
        }

        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&target_idx) {
            crate::logger::log(format!(
                "[video-debug] post-deferred-swap state: idx={target_idx} engine_state={} seek_serial={} clock_is_playing={} pos={:.3} video_rx_len={} audio_rx_len={} pending_frames={}",
                player.engine_state_name(),
                player.current_seek_serial(),
                player.is_playing(),
                player.position(),
                player.video_rx_len(),
                player.audio_rx_len(),
                player.pending_frames()
            ));
        }

        if show_preparing_overlay {
            self.set_native_video_tile_preparing_overlay(target_idx);
        } else if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&target_idx) {
            player.set_native_tile_overlay(None);
        }
        self.sync_native_video_metadata(target_idx);
        self.sync_native_video_timeline_markers(target_idx);
        self.sync_native_video_vst3_available(target_idx);
        self.sync_native_video_vst3_panel(target_idx);

        // Inc 7 (音声モード連続再生 EOF): この swap は音声モードの動画から次動画への送りだった
        // ので、動画セットアップ (上の sync 群) を終えた直後に「動画→音声モード」を再確立する。
        // 再利用した presenter は既に hidden なので、`enter_video_audio_mode` は VST/owner/HUD の
        // 後始末 + entry_target 捕捉 + video_audio_mode=Some(target) + music_bookmarks 再ロード
        // フラグを行い、set_native_window_visible(false) は既 hidden への冪等 no-op になる。
        // 通常 completion では `open_fullscreen(target_idx)` が video_audio_mode を一旦 None に
        // した後この 1 箇所で戻す。ParkedLive completion は `open_fullscreen` を呼ばず、
        // defer 開始時に video_audio_mode=Some(target) へ進めた bundle 内状態をそのまま保つ。
        if audio_mode_after_swap && completed_via_open_fullscreen {
            self.enter_video_audio_mode(ctx, target_idx);
        }

        if reason == "tile" {
            if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&target_idx) {
                player.set_playing(false);
            }
            self.video_tile_mode_active = true;
            self.video_tile_swap_pending = Some(VideoTileSwapPending {
                target_idx,
                target_path: target_path.clone(),
                source_epoch,
                started_at,
                deadline: started_at + std::time::Duration::from_secs(2),
                parked_live_window_id,
            });
            self.video_tile_reopen_pending = false;
            self.video_tile_reopen_deadline = None;
        } else {
            self.native_video_fast_swap_pending = Some(NativeVideoFastSwapPending {
                target_idx,
                target_path: target_path.clone(),
                source_epoch,
                started_at,
                deadline: started_at + std::time::Duration::from_secs(2),
                parked_live_window_id,
            });
        }

        crate::logger::log(format!(
            "[native-video] deferred source swap queued: reason={reason} from={from_idx} to={target_idx} epoch={source_epoch} waited_ms={:.1}",
            requested_at.elapsed().as_secs_f64() * 1000.0
        ));
        if crate::perf::is_enabled() {
            crate::perf::event(
                "native_video",
                "source_swap_deferred_start",
                None,
                input_seq,
                &[
                    ("from_idx", serde_json::Value::from(from_idx as i64)),
                    ("target_idx", serde_json::Value::from(target_idx as i64)),
                    ("source_epoch", serde_json::Value::from(source_epoch as i64)),
                    (
                        "wait_ms",
                        serde_json::Value::from(requested_at.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("reason", serde_json::Value::from(reason)),
                ],
            );
        }
        ctx.request_repaint();
    }

    #[cfg(windows)]
    fn grid_item_kind_for_source_swap_log(item: &GridItem) -> &'static str {
        match item {
            GridItem::Folder { .. } => "folder",
            GridItem::Image(_) => "image",
            GridItem::Video(_) => "video",
            GridItem::Audio(_) => "audio",
            GridItem::ZipFile(_) => "zip-file",
            GridItem::PdfFile(_) => "pdf-file",
            GridItem::ConvertibleArchive { .. } => "convertible-archive",
            GridItem::ZipImage { .. } => "zip-image",
            GridItem::ZipDir { .. } => "zip-dir",
            GridItem::PdfPage { .. } => "pdf-page",
            GridItem::Stack { .. } => "stack",
            GridItem::SearchContainer { .. } => "search-container",
        }
    }

    #[cfg(windows)]
    pub(crate) fn complete_native_video_deferred_source_swap_viewer_state(
        &mut self,
        ctx: &egui::Context,
        target_idx: usize,
        cursor_state: crate::ui_fullscreen::FullscreenCursorState,
        parked_live_window_id: Option<u64>,
        history_trigger: crate::app::HistoryTrigger,
    ) -> bool {
        if let Some(window_id) = parked_live_window_id {
            self.video_continuous_last_eof = None;
            self.refresh_fullscreen_video_marker_cache(target_idx);
            crate::logger::log(format!(
                "[native-video] parked deferred source swap committed without open_fullscreen: \
                 window_id={window_id} target_idx={target_idx}"
            ));
            ctx.request_repaint();
            return false;
        }

        // deferred swap を確定する open_fullscreen は「viewer 内ナビ」相当なので、
        // open_fullscreen 冒頭の一括ガードが現在の `viewer_presentation` から presentation
        // 維持 one-shot を焼き付ける。pending 中の F12 は player 不在で設定だけ反転しないよう
        // 共通入口で無視する (review-v2.3.0 追補3: 角度B-1、案a)。
        self.open_fullscreen(target_idx, history_trigger);
        self.restore_fullscreen_cursor_state(ctx, cursor_state);
        true
    }

    #[cfg(windows)]
    pub(super) fn ensure_native_video_front(&mut self) {
        if self.fullscreen_idx.is_none() {
            self.sync_native_video_main_cloak(false);
            if self.native_video_front_synced_hwnd != 0 {
                // CP7: フルスクリーン解除時に DspBridge 側の owner / hud HWND 登録もクリア。
                self.dsp_bridge.unregister_fullscreen_owner();
                self.dsp_bridge.set_hud_hwnd(0);
                self.vst_geometry_tracker.clear();
            }
            self.native_video_front_synced_hwnd = 0;
            self.native_video_front_last_raise = None;
            self.native_video_front_recover_after_external_foreground = false;
            return;
        }
        // Inc 7 hidden presenter (動画→音声モード): presenter は生存しているが hide されて
        // いる (egui 音楽ビュー表示中)。presenter HWND は非 0 のままなので、明示的に
        // 「非アクティブ」扱いにして owner / HUD 登録を外す (現行 detach 方式で hwnd=0
        // だったときと同じ挙動)。exit で video_audio_mode=None に戻ると次フレームの
        // 通常経路が presenter HWND を再登録する。cloak は update ループ側の
        // `native_video_fullscreen_active_for_main_backdrop`(!fs_music_view_active) が
        // 制御するのでここでは触らない (hwnd==0 分岐と同じ)。
        //
        // 7e: VST ホスト表示中 (`video_audio_vst` Some) は presenter を un-hide して VST owner に
        // するので、この early-return を通さず下の通常 owner 登録経路へ落とす。
        if self.video_audio_mode.is_some() && self.video_audio_vst.is_none() {
            if self.native_video_front_synced_hwnd != 0 {
                self.dsp_bridge.unregister_fullscreen_owner();
                self.dsp_bridge.set_hud_hwnd(0);
                self.vst_geometry_tracker.clear();
            }
            self.native_video_front_synced_hwnd = 0;
            self.native_video_front_last_raise = None;
            self.native_video_front_recover_after_external_foreground = false;
            return;
        }
        // Plan B: presentation 切替の進行中は presenter HWND が作り直される最中。新 HWND が
        // publish 済みでも `PlacementSwitched` 未処理のフレームでは実 presentation
        // (`native_video_in_window_active`) が旧いままなので、ここで owner / HUD を
        // 登録すると新 child HWND を fullscreen / VST owner と誤認しうる (Codex 再 P1)。
        // 切替完了 (`apply_video_presentation_switched`) 後の次フレームで再 sync される。
        //
        // **deadline 過ぎでの強制 clear (review #3 対応)**: presenter スレッドが
        // 応答しない / イベントが失われた等で pending が deadline 過ぎても残った場合、
        // ここで強制的に clear して owner/HUD 登録を再開する。さもないと
        // ensure_native_video_front が永続的に early-return して HUD/VST が
        // 死んだままになる。トグル時の保険 (line 1494) は次回トグルまで
        // 効かないので、こちらは毎フレーム駆動。
        if let Some(pending) = self.native_video_mode_switch {
            if std::time::Instant::now() < pending.deadline {
                return;
            }
            crate::logger::log(format!(
                "[native-video] placement switch request {} exceeded deadline \
                 without PlacementSwitched event; clearing pending and resuming \
                 front sync",
                pending.request_id
            ));
            self.native_video_mode_switch = None;
        }
        let (hwnd, hud_hwnd) = self
            .fullscreen_idx
            .and_then(|idx| {
                self.pending_native_video_output_hwnds_for_fs(idx)
                    .or_else(|| {
                        self.fs_cache.get(&idx).and_then(|entry| match entry {
                            FsCacheEntry::Video { player, .. } => {
                                let hwnd = player.native_presenter_hwnd();
                                if hwnd == 0 {
                                    None
                                } else {
                                    Some((hwnd, player.native_hud_hwnd()))
                                }
                            }
                            _ => None,
                        })
                    })
            })
            .unwrap_or((0, 0));
        if hwnd == 0 {
            if self.native_video_front_synced_hwnd != 0 {
                self.dsp_bridge.unregister_fullscreen_owner();
                self.dsp_bridge.set_hud_hwnd(0);
                self.vst_geometry_tracker.clear();
            }
            self.native_video_front_synced_hwnd = 0;
            self.native_video_front_last_raise = None;
            self.native_video_front_recover_after_external_foreground = false;
            return;
        }
        let is_new_hwnd = hwnd != self.native_video_front_synced_hwnd;
        let fullscreen_presentation =
            matches!(self.viewer_presentation, ViewerPresentation::Fullscreen);
        if !is_new_hwnd {
            // 既存 HWND が継続しているケースでも、HUD HWND は遅延生成されることがあるので
            // bridge 側の登録値が古い (0) のままにならないよう毎フレーム refresh する。
            self.sync_native_video_main_cloak(false);
            // foreground 復旧 / VST owner / HUD topmost 保守は fullscreen 専用。
            // in-window / detached viewer では presenter は通常ウィンドウの child なので、
            // メイン HWND が foreground であることが正常。ここで presenter を raise すると
            // F12 別ウィンドウがメイン操作を奪い返してしまう。
            if !fullscreen_presentation {
                self.dsp_bridge.unregister_fullscreen_owner();
                self.dsp_bridge.set_hud_hwnd(0);
                self.vst_geometry_tracker.clear();
                self.native_video_front_last_raise = None;
                self.native_video_front_recover_after_external_foreground = false;
                return;
            }
            self.dsp_bridge.set_hud_hwnd(hud_hwnd);
            // PrintScreen / Snipping Tool の範囲選択後や native startup の競合で
            // egui 側の黒 backdrop が presenter HWND より前に残ることがある。
            // UI thread から presenter HWND を直接 SetWindowPos せず、復旧が必要な
            // foreground 状態を観測したら presenter 所有スレッドへ依頼する。
            let now = std::time::Instant::now();
            let foreground_hwnd = crate::video::native_window::foreground_hwnd();
            let foreground_is_ours =
                crate::video::native_window::foreground_belongs_to_current_process_strict();
            let foreground_is_presenter =
                foreground_hwnd == hwnd || (hud_hwnd != 0 && foreground_hwnd == hud_hwnd);
            let internal_foreground_needs_recover = foreground_is_ours && !foreground_is_presenter;
            if !foreground_is_ours {
                self.native_video_front_recover_after_external_foreground = true;
            } else if internal_foreground_needs_recover {
                self.native_video_front_recover_after_external_foreground = true;
            }
            let presenter_raise_due = self
                .native_video_front_last_raise
                .map(|last| now.duration_since(last) >= std::time::Duration::from_millis(250))
                .unwrap_or(true);
            if presenter_raise_due
                && foreground_is_ours
                && self.native_video_front_recover_after_external_foreground
            {
                let recover_reason = if internal_foreground_needs_recover {
                    "internal-foreground"
                } else {
                    "external-return"
                };
                let mut requested = false;
                if let Some(pending) = self.native_video_source_swap_pending.as_ref() {
                    if pending.native_output.hwnd() == hwnd {
                        pending.native_output.request_presenter_raise();
                        requested = true;
                    }
                } else if let Some(idx) = self.fullscreen_idx {
                    if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&idx) {
                        if player.native_presenter_hwnd() == hwnd {
                            player.request_presenter_raise();
                            requested = true;
                        }
                    }
                }
                if requested {
                    crate::logger::log(format!(
                        "[native-video] request presenter recover reason={} hwnd=0x{:x} foreground=0x{:x} hud=0x{:x}",
                        recover_reason, hwnd, foreground_hwnd, hud_hwnd
                    ));
                    self.native_video_front_last_raise = Some(now);
                    self.native_video_front_recover_after_external_foreground = false;
                }
            }
            // 実機修正 (2026-05-12 C): VST window が top/bottom bar 帯に重なっていたら
            // ドラッグ/リサイズ終了後に自動で押し出す。`SWP_ASYNCWINDOWPOS` で非同期
            // 送信なので bridge GUI スレッドをブロックしない (= 旧 clamp の crash 回避)。
            self.tick_vst_window_overlap_adjustment(hwnd);
            return;
        }

        // The native HWND is created and raised on the presenter thread. Calling
        // SetWindowPos on that HWND from the egui UI thread can synchronously
        // cross into the presenter thread / DWM while HUD seek and overlay input
        // are active, which has produced UI-thread hangs. Owner z-order plus the
        // fullscreen black backdrop now cover the startup race, so the UI thread
        // only records the new HWND and leaves z-order mutation to its owner
        // thread.
        self.native_video_front_synced_hwnd = hwnd;
        self.native_video_front_last_raise = Some(std::time::Instant::now());
        self.native_video_front_recover_after_external_foreground = false;

        // CP7: presenter HWND が確定したので、DspBridge に owner / HUD HWND を登録する。
        // - `register_fullscreen_owner(presenter_hwnd)`: VST editor の owner を presenter HWND に
        //   強制 (= `current_gui_owner_hwnd` がフルスクリーン中は presenter を最優先で返す)。
        // - `set_hud_hwnd(hud_hwnd)`: HUD HWND を「raise allowlist の mIV 既知 HWND」として登録
        //   (= `foreground_allows_hud_raise` で許可される)。**owner 候補には絶対に出さない**
        //   (= `current_gui_owner_hwnd` 内で除外済み)。
        // in-window / detached viewer では VST editor の owner を presenter にしない
        // (WS_CHILD を owner にすると z-order/focus が壊れる。Codex P3)。
        if fullscreen_presentation {
            self.dsp_bridge.register_fullscreen_owner(hwnd);
            self.dsp_bridge.set_hud_hwnd(hud_hwnd);
        } else {
            self.dsp_bridge.unregister_fullscreen_owner();
            self.dsp_bridge.set_hud_hwnd(0);
            self.vst_geometry_tracker.clear();
        }
        self.sync_native_video_main_cloak(false);
        // 実機修正 (2026-05-12 P1 致命的問題): cross-process SetWindowPos(VST_HWND) は
        // bridge GUI スレッドをブロックして bridge 自殺 → VST 全消失。clamp 機能完全削除。
        crate::video::native_window::log_state(hwnd, "synced");
        crate::logger::log(format!(
            "[native-video] synced fullscreen presenter hwnd=0x{hwnd:x}"
        ));
    }

    /// 実機修正 (2026-05-12 C, Codex P1 #2/#3/#4 反映): VST GUI window が HUD top/bottom bar 帯と
    /// 重なる位置に **drag/resize で動かした後**、rect が 250ms 安定したら自動で外へ押し出す。
    ///
    /// ## 安全策まとめ
    ///   1. **rect 安定検出 (250ms)**: drag/resize 終了を `GetWindowRect` 比較で検出。
    ///   2. **1 イベント 1 発火**: 安定検出後 `SetWindowPos` を 1 回だけ呼んで pending を clear。
    ///   3. **`SWP_ASYNCWINDOWPOS`**: 非同期送信で bridge GUI スレッドをブロックしない。
    ///   4. **HWND 正規化** (Codex P1 #4): `GetAncestor(hwnd, GA_ROOT)` で top-level に揃え、
    ///      `WS_CHILD` style の child HWND は skip。stale / child 混入リスクを排除。
    ///   5. **マルチモニター安全** (Codex P1 #3): VST のタイトルバー中央点が presenter rect 内に
    ///      ない場合は nudge しない (= 上配置の別ディスプレイへ移動した VST を引き戻さない)。
    ///   6. **タイトルバー検出のみ** (Codex P1 #2): bottom 側は「VST 全体の bottom」ではなく
    ///      「VST のタイトルバー上端 (= rect.top + ~30px) が seek bar 帯と重なる」場合のみ発火。
    ///      大きい VST を画面下半分に置いただけで window 全体が動く症状を防ぐ。
    #[cfg(windows)]
    fn tick_vst_window_overlap_adjustment(&mut self, presenter_hwnd: u64) {
        use std::time::{Duration, Instant};
        use windows::Win32::Foundation::HWND as Win32Hwnd;
        use windows::Win32::Foundation::RECT as Win32Rect;
        use windows::Win32::UI::HiDpi::GetDpiForWindow;
        use windows::Win32::UI::WindowsAndMessaging::{
            GA_ROOT, GWL_STYLE, GetAncestor, GetWindowLongPtrW, GetWindowRect, IsWindow,
            IsWindowVisible, SWP_ASYNCWINDOWPOS, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
            SetWindowPos, WS_CHILD,
        };
        let editor_arc = self.dsp_bridge.editor_hwnds_snapshot();
        let raw_list: Vec<u64> = match editor_arc.read() {
            Ok(set) => set.iter().copied().collect(),
            Err(_) => return,
        };

        // HWND 正規化 (Codex 続編 P2 反映): 順序を「先に GA_ROOT で正規化 → 正規化後の root に
        // 対して IsWindow / IsWindowVisible / WS_CHILD を検査」に修正。
        // 旧版は raw HWND が WS_CHILD なら即 skip していたが、これだと「子 HWND の root を辿って
        // 正規化する」目的が満たせない (= child が混じった場合に正規化路に入る前に弾かれていた)。
        // GA_ROOT は raw が既に top-level なら同じ HWND を返すので、常に正規化を先に行うのが安全。
        let mut normalized: std::collections::HashSet<u64> = std::collections::HashSet::new();
        for raw in raw_list.iter().copied() {
            let h = Win32Hwnd(raw as *mut _);
            if !unsafe { IsWindow(Some(h)) }.as_bool() {
                continue;
            }
            // (1) 先に GA_ROOT で top-level に正規化。
            let root = unsafe { GetAncestor(h, GA_ROOT) };
            if root.0.is_null() {
                continue;
            }
            // (2) 正規化後の root に対して生存/可視/style を検査。
            if !unsafe { IsWindow(Some(root)) }.as_bool() {
                continue;
            }
            if !unsafe { IsWindowVisible(root) }.as_bool() {
                continue;
            }
            let root_style = unsafe { GetWindowLongPtrW(root, GWL_STYLE) } as u32;
            if (root_style & WS_CHILD.0) != 0 {
                // GA_ROOT が child を返すのは異常ケース (top-level でない) なので skip。
                continue;
            }
            normalized.insert(root.0 as u64);
        }

        if normalized.is_empty() {
            self.vst_geometry_tracker.clear();
            return;
        }
        // editor 一覧に居なくなった HWND は tracker からも削除 (= stale 防止)。
        self.vst_geometry_tracker
            .retain(|k, _| normalized.contains(k));

        let presenter_win = Win32Hwnd(presenter_hwnd as *mut _);
        let mut presenter_rect = Win32Rect::default();
        if unsafe { GetWindowRect(presenter_win, &mut presenter_rect) }.is_err() {
            return;
        }
        let dpi = unsafe { GetDpiForWindow(presenter_win) } as f32;
        let os_ppp = (dpi / 96.0).max(0.5);
        let overlay_ppp = crate::video::native_presenter::effective_overlay_pixels_per_point(
            os_ppp,
            self.settings.ui_scale_factor,
        );
        let top_band_px = (62.0_f32 * overlay_ppp).round() as i32; // 54pt + 8pt margin
        // 動画 HUD 2 段化リデザイン (Phase 3): HUD_BOTTOM_HEIGHT = 64pt + 8pt margin = 72pt。
        // 旧 1 段では 46+8=54pt だった。
        let bottom_band_px = ((crate::video::native_presenter::HUD_BOTTOM_HEIGHT + 8.0)
            * overlay_ppp)
            .round() as i32;
        // VST editor は bridge 所有の別 HWND なのでアプリ内 UI 倍率を掛けない。
        let titlebar_px = (30.0_f32 * os_ppp).round() as i32;
        let presenter_top = presenter_rect.top;
        let presenter_bottom = presenter_rect.bottom;
        let zone_top_limit = presenter_top + top_band_px;
        let zone_bottom_limit = presenter_bottom - bottom_band_px;

        let now = Instant::now();
        let debug = crate::video::native_presenter::hud_debug_enabled();
        let detached_debug = Self::detached_image_window_debug_enabled();

        for raw in normalized {
            let hwnd = Win32Hwnd(raw as *mut _);
            let mut rect = Win32Rect::default();
            if unsafe { GetWindowRect(hwnd, &mut rect) }.is_err() {
                continue;
            }
            // 実機修正 (Codex 続編 P2 反映): 初回観測時は **pending=false** で挿入する。
            // 旧版は or_insert_with で pending=true → 既に開いていた VST が overlapping
            // 位置にあるだけで 250ms 後に勝手に動く症状になった。「drag/resize 後のみ動かす」
            // 仕様にするには、初回観測 → 何もしない / その後 rect_changed が起きたら pending=true
            // で 250ms 安定後に発火、という形にする。
            let entry = self
                .vst_geometry_tracker
                .entry(raw)
                .or_insert_with(|| (rect, now, false));
            let rect_changed = entry.0.left != rect.left
                || entry.0.top != rect.top
                || entry.0.right != rect.right
                || entry.0.bottom != rect.bottom;
            if rect_changed {
                entry.0 = rect;
                entry.1 = now;
                entry.2 = true;
                continue;
            }
            if !entry.2 {
                continue;
            }
            if now.duration_since(entry.1) < Duration::from_millis(250) {
                continue;
            }
            entry.2 = false;

            // マルチモニター安全 (Codex P1 #3): VST のタイトルバー中央点が presenter rect の内側に
            // あるときだけ nudge する。上/横に別モニターで VST を動かした場合は nudge せず放置。
            let titlebar_center_x = (rect.left + rect.right) / 2;
            let titlebar_center_y = rect.top + titlebar_px / 2;
            let inside_presenter = titlebar_center_x >= presenter_rect.left
                && titlebar_center_x < presenter_rect.right
                && titlebar_center_y >= presenter_rect.top
                && titlebar_center_y < presenter_rect.bottom;
            if !inside_presenter {
                if debug {
                    crate::logger::log(format!(
                        "[HUD-DEBUG] vst overlap skip (off-monitor): hwnd=0x{raw:x} rect.top={} \
                         titlebar_center=({},{}) presenter=({},{} {}x{})",
                        rect.top,
                        titlebar_center_x,
                        titlebar_center_y,
                        presenter_rect.left,
                        presenter_rect.top,
                        presenter_rect.right - presenter_rect.left,
                        presenter_rect.bottom - presenter_rect.top,
                    ));
                }
                continue;
            }

            // タイトルバー重なり判定 (Codex P1 #2):
            //   - top 帯: タイトルバー上端 (= rect.top) が top 帯内 → 押し下げ
            //   - bottom 帯: タイトルバー帯 (rect.top..rect.top+titlebar_px) が bottom 帯と交差 → 押し上げ
            let titlebar_top = rect.top;
            let titlebar_bot = rect.top + titlebar_px;
            let overlaps_top_band = titlebar_top < zone_top_limit;
            let overlaps_bottom_band =
                titlebar_bot > zone_bottom_limit && titlebar_top < presenter_bottom;
            let target_top = if overlaps_top_band {
                Some(zone_top_limit)
            } else if overlaps_bottom_band {
                // タイトルバーを seek bar の上に出す: titlebar_bot == zone_bottom_limit になる位置
                Some(zone_bottom_limit - titlebar_px)
            } else {
                None
            };
            if let Some(t) = target_top {
                if t != rect.top {
                    if detached_debug {
                        self.log_detached_viewport_placement_event(
                            "vst_overlap_clamp",
                            "native_set_window_pos",
                            format!(
                                "hwnd=0x{raw:x} old=({},{} {}x{}) new_top={t} \
                                 overlaps_top={overlaps_top_band} overlaps_bottom={overlaps_bottom_band}",
                                rect.left,
                                rect.top,
                                rect.right - rect.left,
                                rect.bottom - rect.top
                            ),
                        );
                    }
                    let ok = unsafe {
                        SetWindowPos(
                            hwnd,
                            None,
                            rect.left,
                            t,
                            0,
                            0,
                            SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS,
                        )
                    }
                    .is_ok();
                    if debug {
                        crate::logger::log(format!(
                            "[HUD-DEBUG] vst overlap nudge: hwnd=0x{raw:x} old_top={} new_top={t} \
                             overlaps_top={overlaps_top_band} overlaps_bottom={overlaps_bottom_band} ok={ok}",
                            rect.top
                        ));
                    }
                }
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn native_video_presenter_hwnd_for_focus_guard(&self) -> bool {
        // Inc 7 hidden presenter: 音声モード中は presenter が hide されていて egui 音楽ビューが
        // main viewport / embedded で描かれる。presenter HWND は非 0 だが「アクティブな
        // native presenter」ではないので false を返す (= 音声ファイル / 現行 detach 方式と
        // 同じ挙動で、main focus guard が音楽ビューを誤って閉じない)。
        // 7e: VST ホスト表示中は presenter を un-hide して前面 native presenter として扱うので、
        // focus guard も通常動画と同じく presenter を保護する (early-return しない)。
        if self.video_audio_mode.is_some() && self.video_audio_vst.is_none() {
            return false;
        }
        self.fullscreen_idx.is_some_and(|idx| {
            self.pending_native_video_output_active_for_fs(idx)
                || self.fs_cache.get(&idx).is_some_and(|entry| match entry {
                    FsCacheEntry::Video { player, .. } => {
                        player.native_presenter_hwnd() != 0 || player.native_presenter_pending()
                    }
                    _ => false,
                })
        })
    }

    #[cfg(windows)]
    pub(crate) fn pending_native_video_output_active_for_fs(&self, fs_idx: usize) -> bool {
        self.native_video_source_swap_pending
            .as_ref()
            .is_some_and(|pending| {
                pending.target_idx == fs_idx && !pending.native_output.is_closed()
            })
    }

    #[cfg(windows)]
    pub(crate) fn pending_native_video_output_hwnds_for_fs(
        &self,
        fs_idx: usize,
    ) -> Option<(u64, u64)> {
        self.native_video_source_swap_pending
            .as_ref()
            .filter(|pending| pending.target_idx == fs_idx && !pending.native_output.is_closed())
            .and_then(|pending| {
                let hwnd = pending.native_output.hwnd();
                (hwnd != 0).then_some((hwnd, pending.native_output.hud_hwnd()))
            })
    }

    #[cfg(windows)]
    pub(crate) fn native_video_presenter_hwnd(&self) -> Option<u64> {
        self.fullscreen_idx.and_then(|idx| {
            self.pending_native_video_output_hwnds_for_fs(idx)
                .map(|(hwnd, _)| hwnd)
                .or_else(|| {
                    self.fs_cache.get(&idx).and_then(|entry| match entry {
                        FsCacheEntry::Video { player, .. } => {
                            let hwnd = player.native_presenter_hwnd();
                            (hwnd != 0).then_some(hwnd)
                        }
                        _ => None,
                    })
                })
        })
    }

    #[cfg(windows)]
    pub(super) fn native_video_fullscreen_active_for_main_backdrop(&self) -> bool {
        if self.viewer_session_is_detached_or_switching() {
            return false;
        }
        if !self.viewer_session_blocks_main_window() {
            return false;
        }
        let Some(fs_idx) = self.fullscreen_idx else {
            return false;
        };
        // 音声モードにトグルした動画 (Inc 7) は音楽ビュー (egui) を描くので、動画 backdrop 扱いに
        // しない。これを外さないと update ループが backdrop 分岐に入って main HWND を cloak したまま
        // early-return し、音だけ残って画面が消える (実機バグ、Codex 7d 検証)。
        matches!(self.items.get(fs_idx), Some(GridItem::Video(_)))
            && !self.fs_music_view_active(fs_idx)
    }

    #[cfg(windows)]
    pub(super) fn sync_native_video_iconic_thumbnail(&mut self) {
        let Some(hwnd_raw) = self.main_hwnd else {
            return;
        };
        let detached_or_switching = self.viewer_session_is_detached_or_switching();
        let source = self.fullscreen_idx.and_then(|fs_idx| {
            let is_video = matches!(self.items.get(fs_idx), Some(GridItem::Video(_)));
            let music_view_active = is_video && self.fs_music_view_active(fs_idx);
            // The explicit DWM bitmap exists only because an in-main native presenter
            // covers the main HWND. A detached viewer has its own top-level taskbar
            // preview, so overriding the main HWND would make both previews show the
            // detached video. Passing None below also clears a bitmap left by a
            // main -> detached transition.
            if !main_iconic_video_source_enabled(detached_or_switching, is_video, music_view_active)
            {
                return None;
            }
            // 音声モードにトグルした動画 (Inc 7) は音声ファイル扱いなので DWM タスクバー
            // サムネイル (動画フレーム) を出さない (Codex 7d 検証)。
            if let Some(pending) = self.native_video_source_swap_pending.as_ref() {
                if pending.target_idx == fs_idx && !pending.native_output.is_closed() {
                    return Some(crate::dwm_iconic_thumbnail::VideoIconicSource {
                        path: pending.target_path.clone(),
                        target_secs: 0.0,
                    });
                }
            }
            match self.fs_cache.get(&fs_idx) {
                Some(FsCacheEntry::Video { player, .. }) if player.error().is_none() => {
                    Some(crate::dwm_iconic_thumbnail::VideoIconicSource {
                        path: player.path().clone(),
                        target_secs: player.screenshot_target_secs(),
                    })
                }
                _ => None,
            }
        });
        crate::dwm_iconic_thumbnail::sync_video_source(hwnd_raw as u64, source);
    }

    #[cfg(windows)]
    pub(super) fn sync_native_video_main_chrome(&mut self, active: bool) {
        if active {
            self.native_video_main_chrome_restore_at = None;
            // The black fullscreen/backdrop HWND now covers the transition by itself.
            // Do not recolor the main non-client area here: the DWM caption glyph
            // contrast flip is visible just before the black viewport appears.
            if self.native_video_main_chrome_black {
                let dark = matches!(
                    crate::os_theme::resolve(self.settings.ui_theme),
                    crate::os_theme::ResolvedTheme::Dark
                );
                if let Some(hwnd_raw) = self.main_hwnd {
                    crate::dwm_transitions::restore_window_chrome_for_theme(
                        windows::Win32::Foundation::HWND(hwnd_raw as *mut _),
                        dark,
                    );
                }
                self.native_video_main_chrome_black = false;
            }
            return;
        }
        if active == self.native_video_main_chrome_black {
            return;
        }
        let Some(hwnd_raw) = self.main_hwnd else {
            return;
        };
        let hwnd = windows::Win32::Foundation::HWND(hwnd_raw as *mut _);
        if active {
            crate::dwm_transitions::set_window_chrome_black(hwnd);
        } else {
            let dark = matches!(
                crate::os_theme::resolve(self.settings.ui_theme),
                crate::os_theme::ResolvedTheme::Dark
            );
            crate::dwm_transitions::restore_window_chrome_for_theme(hwnd, dark);
        }
        self.native_video_main_chrome_black = active;
    }

    #[cfg(windows)]
    pub(super) fn sync_native_video_main_cloak(&mut self, cloaked: bool) {
        if cloaked == self.native_video_main_cloaked {
            return;
        }
        let Some(hwnd_raw) = self.main_hwnd else {
            crate::logger::log(format!(
                "[native-video] main cloak skipped cloaked={cloaked} hwnd=<none>"
            ));
            return;
        };
        let hwnd = windows::Win32::Foundation::HWND(hwnd_raw as *mut _);
        match crate::dwm_transitions::set_window_cloaked(hwnd, cloaked) {
            Ok(()) => {
                self.native_video_main_cloaked = cloaked;
                crate::logger::log(format!(
                    "[native-video] main cloak={cloaked} hwnd=0x{hwnd_raw:x}"
                ));
            }
            Err(err) => {
                crate::logger::log(format!(
                    "[native-video] main cloak failed cloaked={cloaked} \
                     hwnd=0x{hwnd_raw:x} err={err:?}"
                ));
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn schedule_native_video_main_chrome_restore(&mut self) {
        if self.native_video_main_chrome_black {
            self.native_video_main_chrome_restore_at =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(80));
        } else {
            self.native_video_main_chrome_restore_at = None;
        }
    }

    #[cfg(windows)]
    pub(super) fn process_native_video_main_chrome_restore(&mut self, ctx: &egui::Context) {
        self.process_pending_main_foreground_reclaim(ctx);

        let Some(deadline) = self.native_video_main_chrome_restore_at else {
            return;
        };
        let now = std::time::Instant::now();
        if self.fullscreen_idx.is_some() || self.fs_viewport_shown {
            self.native_video_main_chrome_restore_at =
                Some(now + std::time::Duration::from_millis(80));
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
            return;
        }
        if now >= deadline {
            self.native_video_main_chrome_restore_at = None;
            self.sync_native_video_main_chrome(false);
        } else {
            ctx.request_repaint_after(
                deadline
                    .saturating_duration_since(now)
                    .min(std::time::Duration::from_millis(16)),
            );
        }
    }

    #[cfg(windows)]
    fn process_pending_main_foreground_reclaim(&mut self, ctx: &egui::Context) {
        if !self.pending_main_foreground_reclaim {
            return;
        }

        // foreground 奪還 (条件付き)。close_fullscreen 時点で凍結した条件のみ尊重。
        // Alt+Tab で他アプリが mIV メインと native popup の間に z-order として
        // 割り込んでいた場合、popup destroy 後に他アプリが前面に残るのを防ぐ。
        //
        // chrome 復元は fullscreen viewport hide の DWM 反映を待つが、foreground
        // 奪還まで 80ms 待つと、その間だけ外部ウィンドウが見えることがある。
        let now = std::time::Instant::now();
        // T30 (Claude R3-10): `pending_main_foreground_reclaim_after_hwnd == 0` を
        // 「presenter は既に消えた」と扱うと、ESC で fullscreen を materialize 前に閉じた
        // ケース (= close_fullscreen 時点で presenter HWND がまだ作られていない) で即
        // claim_foreground が発火し、その直後に materialize する presenter の
        // SetForegroundWindow と競合する。
        //
        // 修正: hwnd==0 は「不明状態 (close_fullscreen 時点で未 materialize)」として
        // presenter_destroyed=false 扱いにし、`force_deadline` (close_fullscreen 後 200ms)
        // 経由でのみ reclaim ログを出して skip する。
        //
        // 注意: close_fullscreen 時に hwnd==0 で snapshot した場合、その後 presenter が
        // materialize → destroy しても本コードは hwnd==0 のままなので detect できない
        // (= force_deadline 経由の skip に必ず流れる)。snapshot 時に hwnd!=0 だったケース
        // のみ `is_window_alive` で精密に destroy 検出する。close_fullscreen 後に presenter
        // 側で SetForegroundWindow が走ってもユーザー操作で奪回できるので、これで十分。
        let presenter_destroyed = if self.pending_main_foreground_reclaim_after_hwnd != 0 {
            !crate::video::native_window::is_window_alive(
                self.pending_main_foreground_reclaim_after_hwnd,
            )
        } else {
            false
        };
        let force_deadline_passed = self
            .pending_main_foreground_reclaim_force_at
            .map(|t| now >= t)
            .unwrap_or(true);
        if presenter_destroyed {
            if let Some(hwnd_raw) = self.main_hwnd {
                let report = crate::video::native_window::claim_foreground(hwnd_raw as u64);
                crate::logger::log(format!(
                    "[native-video] reclaim main foreground hwnd=0x{:x} \
                     foreground=0x{:x} post=0x{:x} attach={} set_foreground={} \
                     set_active={} set_focus={}",
                    hwnd_raw,
                    report.foreground_hwnd,
                    report.post_foreground_hwnd,
                    report.attach_thread_input_ok,
                    report.set_foreground_ok,
                    report.set_active_ok,
                    report.set_focus_ok,
                ));
            }
            self.pending_main_foreground_reclaim = false;
            self.pending_main_foreground_reclaim_after_hwnd = 0;
            self.pending_main_foreground_reclaim_force_at = None;
        } else if force_deadline_passed {
            let status = if self.pending_main_foreground_reclaim_after_hwnd == 0 {
                "unknown (never materialized)"
            } else {
                "still alive"
            };
            crate::logger::log(format!(
                "[native-video] reclaim deadline exceeded, skip claim \
                 presenter_hwnd=0x{:x} status={status}",
                self.pending_main_foreground_reclaim_after_hwnd
            ));
            self.pending_main_foreground_reclaim = false;
            self.pending_main_foreground_reclaim_after_hwnd = 0;
            self.pending_main_foreground_reclaim_force_at = None;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    /// 動画 HUD の表示先切替 (Plan B: デコーダ保持)。`SwitchPlacement` コマンドで
    /// presenter ウィンドウ (HWND + DComp) だけを作り直す。`source` (デコーダ /
    /// 音声 / clock) は presenter スレッド側で保持されるため、Plan A の
    /// close+reopen で起きていた音声途切れ・別フレーム混入が起きない。
    ///
    /// `native_video_in_window_active` (= 実モード) はここでは**触らない**。presenter が
    /// 再構築を完了して `PlacementSwitched` を返したとき
    /// `apply_video_presentation_switched` で更新する。これにより再構築中も App は
    /// 「実際に画面に出ているウィンドウ」の presentation を指し続け、旧 child HWND を
    /// fullscreen / VST owner と誤認しない
    /// (Codex P1)。`request_id` で遅延イベントの誤適用を防ぐ (Codex P2)。
    #[cfg(windows)]
    pub(super) fn native_video_presentation_switch_source(&self) -> Option<usize> {
        let idx = self.fullscreen_idx?;
        if !matches!(self.items.get(idx), Some(GridItem::Video(_))) {
            return None;
        }
        // S タイルは動画表示の overlay state であり、presenter placement の切替可否とは
        // 独立している。ここへ video_tile_mode_active の gate を追加しないこと。
        Some(idx)
    }

    #[cfg(windows)]
    pub(crate) fn switch_native_video_viewer_presentation(
        &mut self,
        target_presentation: ViewerPresentation,
        activate_on_show: bool,
    ) {
        let Some(idx) = self.native_video_presentation_switch_source() else {
            return;
        };
        if !matches!(target_presentation, ViewerPresentation::DetachedWindow) {
            self.pending_detached_video_host_switch = None;
        }
        // DetachedWindow へ切り替えるときは、host を最初に出す **前に** window_id を確定する。
        // これで `fullscreen_viewport_id()` が切替開始から確定後まで一貫して同じ detached
        // ViewportId を返し、egui が OS 窓を作り直さない (= 再表示アニメ + presenter child 道連れ
        // 死 + resync 二枚目窓を防ぐ。Codex 実機レビュー P1)。`apply_video_presentation_switched`
        // も同じ `ensure_detached_viewer_window_id()` を呼ぶので id は一致する。
        if matches!(target_presentation, ViewerPresentation::DetachedWindow) {
            self.ensure_detached_viewer_window_id();
        }
        // 進行中のトグルがあれば無視する (連打防止 = Codex P2/P3)。deadline 超過は
        // presenter 無応答時の保険 — 過ぎていれば古い pending を捨てて続行する。
        if let Some(pending) = self.native_video_mode_switch {
            if std::time::Instant::now() < pending.deadline {
                return;
            }
            crate::logger::log(format!(
                "[native-video] placement switch request {} timed out; allowing new switch",
                pending.request_id
            ));
        }
        if !matches!(self.fs_cache.get(&idx), Some(FsCacheEntry::Video { .. })) {
            crate::logger::log(format!(
                "[native-video] placement switch ignored: active video player is not ready \
                 idx={idx} target={target_presentation:?}"
            ));
            return;
        }
        // フルスクリーン → ウィンドウ 切替時、VST3 GUI が表示中なら自動で隠す。
        // in-window モードは VST を対象外にするため、VST GUI ウィンドウ (owner は
        // フルスクリーン presenter HWND) を残したまま切り替えると、owner HWND の
        // 破棄で VST ウィンドウが宙に浮いて残骸表示になる。presenter がまだ
        // フルスクリーン (= VST owner HWND が有効) なうちに、VST ボタン 1 回ぶんと
        // 同じ hide を行う (`toggle_native_video_vst3_gui` は表示中なら hide 方向)。
        if !matches!(target_presentation, ViewerPresentation::Fullscreen) && self.show_vst3_manager
        {
            self.toggle_native_video_vst3_gui();
        }
        let Some((placement, rect, owner_hwnd)) =
            self.native_video_target_for_presentation(target_presentation)
        else {
            if matches!(target_presentation, ViewerPresentation::DetachedWindow) {
                self.defer_native_video_switch_until_detached_host(
                    target_presentation,
                    activate_on_show,
                );
                return;
            }
            crate::logger::log(format!(
                "[native-video] placement switch aborted: rect compute failed target={target_presentation:?}"
            ));
            return;
        };
        // 永続設定は presenter 確定 (`apply_video_presentation_switched`) まで触らない
        // (review #4 対応)。途中で crash / Alt+F4 で落ちても、未確定モードが
        // 次回起動時に持ち越されないようにする。実モード
        // (`native_video_in_window_active`) も成功イベントまで据え置く。
        self.native_video_mode_switch_seq = self.native_video_mode_switch_seq.wrapping_add(1);
        let request_id = self.native_video_mode_switch_seq;
        // presenter スレッドへライブ切替を依頼 (decoder/audio/clock は保持)。
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&idx) {
            player.switch_native_placement(
                request_id,
                placement,
                owner_hwnd,
                rect,
                activate_on_show,
            );
            self.native_video_mode_switch = Some(super::NativeVideoModeSwitchPending {
                request_id,
                target_presentation,
                deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
                announce_main_hint: false,
            });
        }
        crate::logger::log(format!(
            "[native-video] switch placement request={request_id} \
             -> target={target_presentation:?} activate={activate_on_show}"
        ));
    }

    /// detached の presenter child を現在の host / rect に合わせて再構築 (再親付け) する。
    /// 実際に `SwitchPlacement` を発行できたら `true`、条件不成立 (非 detached / host 未確定
    /// / player 不在 / mode switch 進行中) で何もしなかったら `false` を返す。呼び出し側
    /// (`try_resync_detached_video_host`) は false のとき再同期要求を保持して再試行する。
    #[cfg(windows)]
    pub(crate) fn sync_detached_video_child_presenter_rect(&mut self) -> bool {
        let Some(idx) = self.fullscreen_idx else {
            return false;
        };
        if !self.viewer_session_is_detached()
            || !matches!(self.viewer_presentation, ViewerPresentation::DetachedWindow)
            || !matches!(self.items.get(idx), Some(GridItem::Video(_)))
            || self.native_video_mode_switch.is_some()
        {
            return false;
        }
        if self.video_audio_mode_hides_native_presenter_for(idx) {
            crate::logger::log(format!(
                "[video-audio] skip sync_detached_video_child_presenter_rect while hidden presenter \
                 audio mode is active fs_idx={idx}"
            ));
            return true;
        }
        let Some((placement, rect, owner_hwnd)) =
            self.native_video_target_for_presentation(ViewerPresentation::DetachedWindow)
        else {
            return false;
        };
        if !matches!(
            placement,
            crate::video::NativeVideoPlacement::DetachedViewerChild
        ) {
            return false;
        }
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&idx) else {
            return false;
        };

        self.native_video_mode_switch_seq = self.native_video_mode_switch_seq.wrapping_add(1);
        let request_id = self.native_video_mode_switch_seq;
        let presenter_hwnd = player.native_presenter_hwnd();
        self.log_detached_viewport_placement_event(
            "sync_detached_video_child_presenter_rect",
            "native_switch_placement",
            format!(
                "request={request_id} owner=0x{owner_hwnd:x} presenter=0x{presenter_hwnd:x} \
                 rect=({},{} {}x{})",
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top
            ),
        );
        player.switch_native_placement(request_id, placement, owner_hwnd, rect, false);
        self.native_video_mode_switch = Some(super::NativeVideoModeSwitchPending {
            request_id,
            target_presentation: ViewerPresentation::DetachedWindow,
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(2),
            announce_main_hint: false,
        });
        crate::logger::log(format!(
            "[native-video] sync detached child rect request={request_id} \
             owner=0x{owner_hwnd:x} rect=({},{} {}x{})",
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top
        ));
        true
    }

    pub(super) fn toggle_video_window_mode(&mut self) {
        if self.viewer_session_is_detached() {
            crate::logger::log(
                "[native-video] detached F11 ignored by non-context toggle path".to_string(),
            );
            return;
        }
        let current_intent = self
            .native_video_mode_switch
            .map(|p| p.target_presentation)
            .unwrap_or(self.viewer_presentation);
        let target = if matches!(current_intent, ViewerPresentation::MainWindow) {
            ViewerPresentation::Fullscreen
        } else {
            ViewerPresentation::MainWindow
        };
        self.switch_native_video_viewer_presentation(target, true);
    }

    pub(crate) fn toggle_video_window_mode_for_input(&mut self, ctx: &egui::Context) {
        // 7e: 「動画→音声モード」の VST ホスト表示中 (video_audio_vst) にキーボード / リング /
        // ゲームパッドの窓・全画面切替が来たら、presenter を再構築せず VST を畳んで音楽ビュー
        // (波形) へ戻してから still-window toggle に振る。native HUD の ToggleWindowMode 分岐
        // (handle_native_video_output_event) と同じ整合経路にそろえる。これをしないと VST 中は
        // fs_music_view_active=false のため本メソッドが presenter 再構築経路
        // (toggle_video_window_mode) へ流れ、しかも VST を exit しないので、presenter placement /
        // viewer_presentation / video_audio_vst が相互不整合になり、入力ルーティングが詰まって
        // freeze したり、viewer_presentation が Fullscreen 以外に固着して VST ボタンが消えたりする
        // (2026-07-05 実機 fb)。stale な video_audio_vst は fullscreen_idx 一致で弾く。
        #[cfg(windows)]
        if let Some(fs_idx) = self.video_audio_vst.as_ref().map(|s| s.fs_idx) {
            if self.fullscreen_idx == Some(fs_idx) {
                crate::logger::log(format!(
                    "[video-audio-vst] window toggle -> exit VST + still-window toggle (fs_idx={fs_idx})"
                ));
                self.exit_video_audio_vst(ctx, fs_idx);
                self.toggle_egui_viewer_window_mode_for_input(ctx);
                ctx.request_repaint();
                return;
            }
        }
        #[cfg(windows)]
        if let Some(fs_idx) = self.video_audio_mode
            && self.fullscreen_idx == Some(fs_idx)
        {
            crate::logger::log(format!(
                "[video-audio] window toggle while in audio mode -> egui viewer toggle (fs_idx={fs_idx})"
            ));
            self.toggle_egui_viewer_window_mode_for_input(ctx);
            return;
        }
        // 音声ファイルの音楽ビュー: native presenter を持たないので still (egui viewer) の
        // ウィンドウ/全画面トグルへ振る。F11 キー (handle_fs_key_input) と音楽 chrome の
        // window ボタンは既にこの経路だが、ring / ゲームパッド / 右ドラッグの
        // ToggleWindowMode だけが下の toggle_video_window_mode →
        // switch_native_video_viewer_presentation の GridItem::Video ガードで silent no-op に
        // なっていた (review-v2.3.0 P2-5)。detached の音声は helper 内の detached 分岐が
        // borderless トグルに解決するので従来挙動と同じ。
        if let Some(fs_idx) = self.fullscreen_idx
            && self.fs_music_view_active(fs_idx)
        {
            self.toggle_egui_viewer_window_mode_for_input(ctx);
            return;
        }
        if self.viewer_session_is_detached() {
            if self.show_vst3_manager {
                self.toggle_native_video_vst3_gui();
            }
            // タイル overlay は presenter child 側の表示状態であり、host の装飾・サイズ変更と
            // 独立している。仮想フルスクリーン settle 後の child rect 同期でも維持する。
            self.toggle_detached_viewer_borderless_fullscreen(ctx);
            return;
        }
        self.toggle_video_window_mode();
    }

    /// 静止画フルスクリーンのウィンドウ / 全画面 表示を切り替える。
    ///
    /// 静止画は egui の描画先 (embedded CentralPanel ⇔ 専用フルスクリーン
    /// viewport) を切り替えるだけで、動画の Plan B のような presenter
    /// (HWND + DComp) 再構築は不要。設定と実モードフラグを同期フリップし、
    /// 次フレームの `render_fullscreen_viewport` が新モードで描画し直す。
    /// 永続設定 `video_in_window_mode` は動画と共有する単一の「in-window
    /// モード」設定で、動画 HUD のトグルと同じ値を切り替える。
    #[cfg(windows)]
    pub(crate) fn toggle_still_window_mode(&mut self) {
        let in_window = !self.settings.video_in_window_mode;
        let now = std::time::Instant::now();
        crate::dwm_transitions::disable_transitions_for_thread_windows();
        self.settings.video_in_window_mode = in_window;
        self.native_video_in_window_active = in_window;
        self.viewer_presentation = if in_window {
            ViewerPresentation::MainWindow
        } else {
            ViewerPresentation::Fullscreen
        };
        if in_window {
            self.still_fullscreen_viewport_enter_suppress_until = None;
            if self.fs_viewport_shown {
                self.request_main_font_atlas_resync(
                    crate::app::FONT_ATLAS_RESYNC_REASON_STILL_WINDOW_MODE,
                );
            }
        } else {
            if let Some(fs_idx) = self.fullscreen_idx {
                self.fs_holdover_tex = self
                    .capture_fs_display_unit(fs_idx)
                    .map(crate::app::FsHoldover::FolderNavigation);
            }
            self.still_fullscreen_viewport_enter_suppress_until =
                Some(now + std::time::Duration::from_millis(260));
        }
        self.settings.save();
        // embedded → viewport では新 viewport がフォーカスを取るまで数フレーム
        // main にフォーカスが残る。focus 起因の自動クローズ (update() の
        // フォーカスガード) を抑止するため grace を張り直す (open_fullscreen と同じ)。
        self.fs_opened_at = Some(now);
        self.fs_focus_grace_elapsed = false;
        crate::logger::log(format!(
            "[fs] still-image window mode toggled -> in_window={in_window}"
        ));
    }

    /// egui で描く fullscreen viewer (静止画 / 音楽ビュー / 動画→音声モードの波形) の
    /// F11/window ボタンを処理する。
    ///
    /// detached session 中は presentation を MainWindow/Fullscreen へ再解決せず、動画 detached と
    /// 同じく detached 窓自体の borderless を切り替える。非 detached では従来どおり
    /// embedded main window と fullscreen viewport を切り替える。
    #[cfg(windows)]
    pub(crate) fn toggle_egui_viewer_window_mode_for_input(&mut self, ctx: &egui::Context) {
        if self.viewer_session_is_detached() {
            self.toggle_detached_viewer_borderless_fullscreen(ctx);
        } else {
            self.toggle_still_window_mode();
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }

    /// Plan B: presenter から `PlacementSwitched` (切替成功) を受けたときに呼ぶ。
    /// App 側の実モード (`native_video_in_window_active`) を新モードへ更新し、
    /// presenter HWND が作り直されたことに伴う front 同期 / VST owner の再設定を行う。
    /// `native_video_mode_switch` (pending) の解除は呼び出し側が request_id 照合の
    /// うえで行う (stale イベントでも実モードだけは反映するため = Codex 再 P2)。
    ///
    /// **永続設定の保存タイミング (review #4 対応)**: `settings.video_in_window_mode`
    /// は MainWindow / Fullscreen への presenter 確定後だけ更新・保存する。
    /// DetachedWindow は F12 の一時的な別ウィンドウ表示先であり、F11 の
    /// 「ウィンドウ内表示か全画面表示か」という non-detached 側の好みを上書きしない。
    /// toggle 時点で save してしまうと、presenter rebuild 中に crash / Alt+F4 で落ちた場合に
    /// 「ユーザーが見ていない未確定モード」が次回起動時に持ち越される。
    #[cfg(windows)]
    pub(crate) fn apply_video_presentation_switched(&mut self, presentation: ViewerPresentation) {
        let in_window = matches!(presentation, ViewerPresentation::MainWindow);
        self.native_video_in_window_active = in_window;
        self.viewer_presentation = presentation;
        if matches!(presentation, ViewerPresentation::DetachedWindow) {
            let id = self.ensure_detached_viewer_window_id();
            self.begin_active_detached_session(id, super::DetachedSource::Video);
        } else if self.active_detached_session.is_some() {
            self.begin_active_detached_session_close("video_presentation_switched_non_detached");
            self.finish_active_detached_session_close("video_presentation_switched_non_detached");
        }
        self.last_viewer_sync_stamp = if matches!(presentation, ViewerPresentation::DetachedWindow)
        {
            self.fullscreen_idx
                .and_then(|idx| self.viewer_sync_stamp_for_idx(idx))
        } else {
            None
        };
        if !matches!(presentation, ViewerPresentation::DetachedWindow)
            && self.settings.video_in_window_mode != in_window
        {
            self.settings.video_in_window_mode = in_window;
            self.settings.save();
        }
        // presenter HWND が作り直されたので front 同期を強制リセットする。新 HWND は
        // publish 済みなので、次の ensure_native_video_front が is_new_hwnd 経路で
        // owner / HUD 登録をやり直す (Codex #4)。
        self.native_video_front_synced_hwnd = 0;
        self.native_video_front_last_raise = None;
        self.native_video_front_recover_after_external_foreground = false;
        if !matches!(presentation, ViewerPresentation::Fullscreen) {
            // in-window / detached viewer では VST を対象外にするため、全画面
            // owner / HUD 登録を解除する (Codex #4)。VST availability / panel は
            // 毎フレームの sync_* で false 化。
            self.dsp_bridge.unregister_fullscreen_owner();
            self.dsp_bridge.set_hud_hwnd(0);
            self.vst_geometry_tracker.clear();
        }
        crate::logger::log(format!(
            "[native-video] presentation switch applied -> {presentation:?}"
        ));
    }

    /// `PlacementSwitched` の presentation 反映部分 (committed 世代の更新は呼び出し側が
    /// path 別に済ませてから呼ぶ)。通常経路 (`handle_native_video_output_event`) と
    /// source swap drain の両方から使い、placement 反映ロジックが分岐しないようにする
    /// (Codex 再レビュー: source swap 中に PlacementSwitched が来ても presentation/session
    /// を確実に反映し、`native_video_mode_switch` を取り残さない)。
    ///
    /// **P1-1**: request_id 一致 (または pending target への収束) を先に判定し、一致時だけ
    /// presentation/session/settings を反映する。stale/mismatch な成功通知で新状態を巻き
    /// 戻さないため。
    #[cfg(windows)]
    pub(super) fn apply_native_video_placement_switch_state(
        &mut self,
        request_id: u64,
        placement: crate::video::NativeVideoPlacement,
        generation: u64,
    ) {
        let presentation = Self::native_video_placement_to_viewer_presentation(placement);
        match self.native_video_mode_switch {
            Some(p) if p.request_id == request_id => {
                self.apply_video_presentation_switched(presentation);
                self.native_video_mode_switch = None;
                crate::logger::log(format!(
                    "[native-video] PlacementSwitched request={request_id} matched pending; \
                     applied {presentation:?} generation={generation}"
                ));
                // F12/リングのメディア窓→メイン切替は、確定したここで初めて案内する
                // (Sol P2: 要求発行時に出すと presenter 無応答でも「切り替えました」が出る)。
                if p.announce_main_hint
                    && !matches!(presentation, ViewerPresentation::DetachedWindow)
                {
                    self.show_media_window_main_hint_toast();
                }
            }
            Some(p) if p.target_presentation == presentation => {
                // request_id はズレたが presenter が pending の目標に収束した。
                // 目標一致なので反映してよい (Codex 再 P2 の「収束」ケース)。
                self.apply_video_presentation_switched(presentation);
                self.native_video_mode_switch = None;
                crate::logger::log(format!(
                    "[native-video] PlacementSwitched request={request_id} stale but \
                     presenter converged to pending target {presentation:?}; applied \
                     generation={generation}"
                ));
                if p.announce_main_hint
                    && !matches!(presentation, ViewerPresentation::DetachedWindow)
                {
                    self.show_media_window_main_hint_toast();
                }
            }
            Some(_) => {
                // stale かつ pending target とも不一致: 反映しない (巻き戻し防止)。
                // committed 世代だけは呼び出し側で進めているので close の stale 判定は正しい。
                crate::logger::log(format!(
                    "[native-video] PlacementSwitched request={request_id} did not match \
                     pending; NOT applying {presentation:?} generation={generation}; \
                     pending still active"
                ));
            }
            None => {
                // pending 無し = presenter 主導の収束。現状に合わせて反映する。
                self.apply_video_presentation_switched(presentation);
                crate::logger::log(format!(
                    "[native-video] PlacementSwitched request={request_id} arrived with \
                     no pending; applied {presentation:?} generation={generation}"
                ));
            }
        }
    }

    /// close イベントの世代トークンが現在の committed 世代以上かを判定する pure ロジック。
    /// `generation < committed` = placement switch で作り直された旧 window 由来の
    /// 遅延 close → stale として棄却する。時間窓 (旧 500ms band-aid) を置き換える因果判定。
    #[cfg(windows)]
    pub(crate) fn native_video_close_generation_is_current(
        generation: u64,
        committed: u64,
    ) -> bool {
        generation >= committed
    }

    /// committed 世代 (呼び出し側が取得済み) と close の世代を照合し、受理すべきかを
    /// ログ付きで返す。`accept_native_video_close` (fs_cache 経路) と source swap drain
    /// (退避中 native_output 経路) の両方から使う共通ロジック。
    #[cfg(windows)]
    fn accept_native_video_close_with_committed(
        committed: u64,
        generation: u64,
        source: &str,
    ) -> bool {
        let accepted = Self::native_video_close_generation_is_current(generation, committed);
        if accepted {
            crate::logger::log(format!(
                "[native-video] accept close source={source} \
                 generation={generation} committed={committed}"
            ));
        } else {
            crate::logger::log(format!(
                "[native-video] reject stale close source={source} \
                 generation={generation} committed={committed}"
            ));
        }
        accepted
    }

    /// 指定 fs_idx の player の committed 世代 (無ければ 0)。
    #[cfg(windows)]
    pub(crate) fn native_video_committed_generation_for(&self, fs_idx: usize) -> u64 {
        self.fs_cache
            .get(&fs_idx)
            .and_then(|entry| match entry {
                FsCacheEntry::Video { player, .. } => player.native_committed_generation(),
                _ => None,
            })
            .unwrap_or(0)
    }

    /// 指定 fs_idx の player の committed 世代を単調非減少で進める。
    /// `PlacementSwitched` 受信時に呼ぶ。player / native_output 不在なら no-op。
    #[cfg(windows)]
    pub(crate) fn bump_native_video_committed_generation(&self, fs_idx: usize, generation: u64) {
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.bump_native_committed_generation(generation);
        }
    }

    /// close イベントを (fs_cache の player が持つ) committed 世代と照合し、受理すべきかを
    /// ログ付きで返す。stale なら `[native-video] reject stale close` を残して false。
    #[cfg(windows)]
    fn accept_native_video_close(&self, fs_idx: usize, generation: u64, source: &str) -> bool {
        let committed = self.native_video_committed_generation_for(fs_idx);
        Self::accept_native_video_close_with_committed(committed, generation, source)
    }

    /// Plan B: presenter から `PlacementSwitchFailed` を受けたときに呼ぶ。presenter は
    /// 旧 window/presenter を生かしたまま。永続設定は toggle 時点では触っていないので
    /// (review #4 対応の deferred save)、ここで戻すべきものは何も無く、pending を
    /// クリアするだけで OK。
    #[cfg(windows)]
    fn revert_failed_video_presentation_switch(&mut self, target_presentation: ViewerPresentation) {
        self.native_video_mode_switch = None;
        crate::logger::log(format!(
            "[native-video] placement switch failed (target={target_presentation:?}); \
             pending cleared (settings was never persisted, no revert needed)"
        ));
        // **主対象は「detached 再生中の再親付け rebuild 失敗」**。Stage 4 の二相 switch は
        // failure 時に旧 host を維持するため、再親付けを再要求して registry の target と
        // production pump の active host を収束させる (host が確定できなければ sync が
        // no-op で待機、session 終了で applicability が落ちて自然に破棄される)。
        //
        // 一方 **初回 main→detached の switch 失敗** は、mode_switch を上で None にした後
        // viewer_presentation が非 detached のまま残る (apply していない) ため
        // `detached_video_presentation_active_or_targeted()` が false になり、ここでは
        // 再試行しない (= 初回失敗の無限 retry を避ける。初回切替の再試行は別経路の責務)。
        if matches!(target_presentation, ViewerPresentation::DetachedWindow)
            && self.detached_video_presentation_active_or_targeted()
        {
            self.pending_detached_video_host_resync = true;
        }
    }

    #[cfg(windows)]
    pub(crate) fn native_video_output_event_allowed_while_parked_live(
        event: &crate::video::NativeVideoOutputEvent,
    ) -> bool {
        use crate::video::NativeVideoOutputEvent as Ev;
        use crate::video::native_window::NativeVideoWindowEvent as WinEv;

        match event {
            // Placement completion/failure is lifecycle bookkeeping from an in-flight presenter
            // operation, not user input. Dropping it can leave pending switch state stale.
            Ev::PlacementSwitched { .. } | Ev::PlacementSwitchFailed { .. } => true,
            // Passive/ParkedLive windows must not react to keys, wheel, or mouse buttons. Geometry
            // maintenance is harmless and keeps the presenter in sync if it reports DPI/rect data.
            Ev::Window(
                WinEv::GeometryChanged { .. }
                | WinEv::DpiChanged { .. }
                | WinEv::RequestRaiseHud
                | WinEv::MouseLeave,
            ) => true,
            Ev::Window(_) => false,
            _ => false,
        }
    }

    #[cfg(windows)]
    pub(crate) fn native_video_output_event_is_parked_live_left_button(
        event: &crate::video::NativeVideoOutputEvent,
        down: bool,
    ) -> bool {
        use crate::video::NativeVideoOutputEvent as Ev;
        use crate::video::native_window::{
            NativeVideoMouseButton, NativeVideoWindowEvent as WinEv,
        };

        matches!(
            event,
            Ev::Window(WinEv::MouseButton(button))
                if button.button == NativeVideoMouseButton::Left
                    && button.down == down
                    && !button.double_click
        )
    }

    #[cfg(windows)]
    pub(crate) fn native_video_output_event_is_parked_live_hud_click_activation(
        event: &crate::video::NativeVideoOutputEvent,
    ) -> bool {
        use crate::video::NativeVideoOutputEvent as Ev;

        match event {
            // Lifecycle/status/hover events keep parked state maintenance working and must not
            // activate the media window.
            Ev::Window(_)
            | Ev::PlacementSwitched { .. }
            | Ev::PlacementSwitchFailed { .. }
            | Ev::RequestSeekThumbnail { .. }
            | Ev::ClearSeekThumbnail
            | Ev::TileColumnsDelta { .. } => false,
            // Native presenter converts plain wheel into NavigateItem. Keep wheel-origin
            // navigation inert while parked, but treat the HUD prev/next buttons as clicks
            // that request activation.
            Ev::NavigateItem { via_wheel, .. } => !*via_wheel,
            // Every other event is produced by a native HUD command. While ParkedLive, button
            // functions stay inert; the click itself requests activation instead.
            _ => true,
        }
    }

    #[cfg(windows)]
    pub(crate) fn native_video_output_event_blocked_while_parked_live(
        event: &crate::video::NativeVideoOutputEvent,
    ) -> bool {
        !Self::native_video_output_event_allowed_while_parked_live(event)
            && !Self::native_video_output_event_is_parked_live_left_button(event, true)
            && !Self::native_video_output_event_is_parked_live_left_button(event, false)
    }

    #[cfg(windows)]
    fn queue_native_video_parked_live_activation_request(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        window_id: u64,
        reason: &'static str,
    ) {
        if !self
            .native_video_parked_live_activation_requests
            .contains(&window_id)
        {
            self.native_video_parked_live_activation_requests
                .push(window_id);
            ctx.request_repaint();
            crate::logger::log(format!(
                "[native-video] parked-live activation queued: idx={fs_idx} \
                 window_id={window_id} reason={reason}"
            ));
        }
    }

    #[cfg(windows)]
    pub(crate) fn native_video_event_blocked_by_parked_live_filter(
        &self,
        event: &crate::video::NativeVideoOutputEvent,
    ) -> bool {
        self.native_video_parked_live_input_window_id.is_some()
            && Self::native_video_output_event_blocked_while_parked_live(event)
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_output_event(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        source_epoch: u64,
        event: crate::video::NativeVideoOutputEvent,
    ) {
        // native video window は winit 管理外の独立 HWND / egui Context である。
        // raw key の text-input 抑止と IME 判定は `NativeOverlayInputRouting` が command / event
        // 発行前に所有する。ここから先で App viewport の `ime_input_active()` を再適用すると、
        // 別 viewport の composition が native の明示クリック・bookmark command・pointer event を
        // 無関係に破棄するため、target/epoch だけを検証する。
        if self.fullscreen_idx != Some(fs_idx) {
            crate::logger::log(format!(
                "[native-video] stale overlay event ignored: event_idx={fs_idx} current={:?}",
                self.fullscreen_idx
            ));
            return;
        }
        if self.remote_session_blocks_local_control()
            && Self::native_video_output_event_blocked_while_parked_live(&event)
        {
            crate::logger::log(
                "[native-video] user input ignored while remote session owns local controls",
            );
            return;
        }
        if let Some(window_id) = self.native_video_parked_live_input_window_id {
            if Self::native_video_output_event_is_parked_live_left_button(&event, true) {
                self.native_video_parked_live_left_down_window_id = Some(window_id);
                crate::logger::log(format!(
                    "[native-video] parked-live left-down captured: idx={fs_idx} window_id={window_id}"
                ));
                return;
            }
            if Self::native_video_output_event_is_parked_live_left_button(&event, false) {
                if self.native_video_parked_live_left_down_window_id == Some(window_id) {
                    self.queue_native_video_parked_live_activation_request(
                        ctx,
                        fs_idx,
                        window_id,
                        "left_click",
                    );
                }
                self.native_video_parked_live_left_down_window_id = None;
                return;
            }
            if Self::native_video_output_event_is_parked_live_hud_click_activation(&event) {
                self.native_video_parked_live_left_down_window_id = None;
                self.queue_native_video_parked_live_activation_request(
                    ctx,
                    fs_idx,
                    window_id,
                    "hud_command",
                );
                crate::logger::log(format!(
                    "[native-video] parked-live hud command converted to activation: \
                     idx={fs_idx} window_id={window_id} event={event:?}"
                ));
                return;
            }
            if self.native_video_event_blocked_by_parked_live_filter(&event) {
                crate::logger::log(format!(
                    "[native-video] parked-live passive event ignored: idx={fs_idx} window_id={window_id} event={event:?}"
                ));
                return;
            }
        }
        // 音声 VST シェル (Inc 6 ②-3): close / VST トグルはモード離脱に振り、動画専用イベントは
        // no-op にする。attach した native 出力は epoch=0 の単一 source なので epoch チェックの
        // 前に処理してよい (source swap を使わない)。
        if self
            .music_vst_shell
            .as_ref()
            .is_some_and(|s| s.fs_idx == fs_idx)
        {
            use crate::video::NativeVideoOutputEvent as Ev;
            use crate::video::native_window::NativeVideoWindowEvent as WinEv;
            // フルスクリーン / ウィンドウ 切替ボタン: 動画と同じく「ウィンドウモードに切替 +
            // VST モードを抜ける」。シェルを抜けて egui 音楽ビューへ戻り、ウィンドウ表示に切替える。
            if matches!(event, Ev::ToggleWindowMode) {
                self.exit_music_vst_shell();
                self.toggle_egui_viewer_window_mode_for_input(ctx);
                ctx.request_repaint();
                return;
            }
            // × / native close: 動画と同じくフルスクリーンを閉じて一覧へ戻る (VST ボタンは
            // 「VST モードを抜けて音楽ビューへ戻る」で役割分担。× は完全に閉じる)。
            if matches!(event, Ev::CloseFullscreen { .. })
                || matches!(&event, Ev::Window(w) if matches!(w, WinEv::CloseRequested { .. }))
            {
                self.handle_fullscreen_close_request_immediate();
                ctx.request_repaint();
                return;
            }
            // VST ボタン: VST モードを抜けて音楽ビューへ戻る (フルスクリーンは維持)。
            if matches!(event, Ev::ToggleVst3Gui) {
                self.exit_music_vst_shell();
                ctx.request_repaint();
                return;
            }
            if !Self::native_event_allowed_in_music_shell(&event) {
                return;
            }
        }
        // 7e: 「動画→音声モード」の VST ホスト表示中 (presenter を un-hide して VST owner に
        // している) は、VST ボタン / ♪ (音声モード) ボタン / ウィンドウ切替を「VST ホストを畳んで
        // 音楽ビュー (波形) へ戻る」に振る。× / close は下の通常 close 経路へフォールスルーして
        // フルスクリーンを閉じる。他の再生系イベント (seek/volume/nav 等) は通常動画として処理する。
        // これらは source 非依存の UI トグルなので epoch チェックの前に処理してよい (music_vst_shell
        // と同じ)。exit_video_audio_vst は presenter を re-hide するだけで drop しない。
        if self.video_audio_vst_active_for(fs_idx) {
            use crate::video::NativeVideoOutputEvent as Ev;
            match &event {
                Ev::ToggleVst3Gui | Ev::ToggleAudioMode => {
                    self.exit_video_audio_vst(ctx, fs_idx);
                    ctx.request_repaint();
                    return;
                }
                Ev::ToggleWindowMode => {
                    self.exit_video_audio_vst(ctx, fs_idx);
                    self.toggle_egui_viewer_window_mode_for_input(ctx);
                    ctx.request_repaint();
                    return;
                }
                _ => {}
            }
        }
        let current_epoch = self.fs_cache.get(&fs_idx).and_then(|entry| match entry {
            FsCacheEntry::Video { player, .. } => player.native_source_epoch(),
            _ => None,
        });
        if current_epoch != Some(source_epoch) {
            // **NavigateItem は epoch mismatch を許容する** (Codex 第 13 ラウンド指摘、
            // 2026-05-26 実機 fb の主犯)。
            //
            // ホイール A→B 切替直後、`NativeVideoOutput::switch_source` は player の
            // native_source_epoch を即座に進めるが、presenter thread が `SwitchSource`
            // コマンドを処理して新 source_epoch で events を emit するまで遅延がある。
            // この skew window 中に HUD の前/次項目ボタンを押すと、event は旧 epoch で
            // stamp されて発射されるが、player 側の current_epoch は既に新 epoch に
            // 進んでいるため epoch mismatch で silent drop されていた。
            //
            // `NavigateItem { delta, .. }` は source 非依存の汎用コマンドで、現在の
            // fullscreen_idx (= 既にチェック済み) + delta だけで意味が完結する。
            // epoch mismatch があっても安全に dispatch 可能なので bypass する。
            // 他の source-specific コマンド (Seek / SetVolume / SetPlaybackSpeed 等) は
            // 引き続き epoch reject (= 旧 source への操作が新 source に当たらないように)。
            if matches!(
                event,
                crate::video::NativeVideoOutputEvent::NavigateItem { .. }
                    | crate::video::NativeVideoOutputEvent::ToggleSidePanelMode
                    | crate::video::NativeVideoOutputEvent::ToggleClickInfoOpen
                    | crate::video::NativeVideoOutputEvent::OpenTouchInfoPanel
                    | crate::video::NativeVideoOutputEvent::DismissTouchSidePanels
                    | crate::video::NativeVideoOutputEvent::SetVideoAdjustments { .. }
            ) {
                // fall through: NavigateItem は dispatch 続行
            } else {
                crate::logger::log(format!(
                    "[native-video] stale overlay event ignored: event_idx={fs_idx} event_epoch={source_epoch} current_epoch={current_epoch:?}"
                ));
                return;
            }
        }
        match event {
            crate::video::NativeVideoOutputEvent::OverlayInputRouting(_) => {
                debug_assert!(false, "routing snapshots are consumed by NativeVideoOutput");
            }
            crate::video::NativeVideoOutputEvent::Window(event) => {
                self.handle_native_video_window_event(ctx, fs_idx, event);
            }
            crate::video::NativeVideoOutputEvent::Seek { target_secs } => {
                self.handle_native_video_seek_command(ctx, fs_idx, target_secs);
            }
            crate::video::NativeVideoOutputEvent::SeekRelative { delta_secs } => {
                self.native_video_seek_relative_with_hint(fs_idx, delta_secs);
            }
            crate::video::NativeVideoOutputEvent::TouchChromeLearned => {
                if !self.settings.touch_video_chrome_learned {
                    self.settings.touch_video_chrome_learned = true;
                    self.settings.save();
                    self.sync_native_video_metadata(fs_idx);
                }
                ctx.request_repaint();
            }
            crate::video::NativeVideoOutputEvent::TileSeek { target_secs } => {
                self.handle_native_video_tile_seek_command(ctx, fs_idx, target_secs);
            }
            crate::video::NativeVideoOutputEvent::NavigateItem { delta, .. } => {
                self.navigate_native_video_fullscreen(ctx, fs_idx, delta);
            }
            crate::video::NativeVideoOutputEvent::TileColumnsDelta { delta } => {
                self.adjust_native_video_tile_columns(ctx, fs_idx, delta);
            }
            crate::video::NativeVideoOutputEvent::RequestSeekThumbnail { target_secs } => {
                self.handle_native_video_request_seek_thumbnail(fs_idx, target_secs);
            }
            crate::video::NativeVideoOutputEvent::ClearSeekThumbnail => {
                // T35: hover が外れた。pump_native_hover_thumbnail の永久リトライを止める
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    player.clear_native_hover_thumbnail();
                }
            }
            crate::video::NativeVideoOutputEvent::ToggleTileMode => {
                let screen = self.video_tile_layout_size(fs_idx, ctx);
                self.toggle_video_tile_mode(fs_idx, screen);
                self.sync_native_video_tile_overlay(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::TogglePerfOverlay => {
                self.toggle_native_video_perf_overlay(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::ToggleSidePanelMode => {
                self.cycle_fs_side_panel_mode();
                let label = self.settings.fullscreen_side_panel_mode.label();
                self.show_native_video_overlay_toast(format!("パネル表示: {label}"), false);
                self.sync_native_video_metadata(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::ToggleClickInfoOpen => {
                self.toggle_fullscreen_click_info_open();
                self.sync_native_video_metadata(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::OpenTouchInfoPanel => {
                self.open_fullscreen_touch_info_panel();
                self.sync_native_video_metadata(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::DismissTouchSidePanels => {
                if self.fs_info_panel_open
                    == crate::ui_helpers::MetadataPanelOpenState::ByTouchHandle
                {
                    self.close_fullscreen_info_panel();
                    self.sync_native_video_metadata(fs_idx);
                }
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::ToggleVst3Gui => {
                if !self.vst3_playback_ui_context_is_main_fullscreen() {
                    self.show_feedback_toast(
                        "VST はメインウィンドウのフルスクリーンでのみ使用できます".to_string(),
                    );
                    self.mark_native_video_hud_activity(ctx);
                    return;
                }
                self.toggle_native_video_vst3_gui();
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::ToggleAudioMode => {
                // 動画 HUD の「音声モード」ボタン (Inc 7): 映像を切って音楽ビューへ。enter は
                // native presenter を detach するので、このイベントを処理した後は同バッチの残り
                // イベントが stale になる → poll_video 側で batch を打ち切る (video_audio_mode の
                // None→Some 遷移を検出)。fs_idx / fullscreen 前提の guard は enter 側で行う。
                self.enter_video_audio_mode(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::CloseFullscreen { generation } => {
                if !self.accept_native_video_close(fs_idx, generation, "output_close") {
                    return;
                }
                self.handle_fullscreen_close_request_immediate();
            }
            crate::video::NativeVideoOutputEvent::ToggleWindowMode => {
                self.toggle_video_window_mode_for_input(ctx);
            }
            crate::video::NativeVideoOutputEvent::PlacementSwitched {
                request_id,
                placement,
                generation,
            } => {
                // committed 世代は apply の可否に関わらず単調非減少で進める。generation は
                // presenter が焼いた「現 live window 世代」なので、これで close の stale
                // 判定基準が常に最新に追随する (旧 window の close だけが古い世代で残る)。
                self.bump_native_video_committed_generation(fs_idx, generation);
                self.apply_native_video_placement_switch_state(request_id, placement, generation);
            }
            crate::video::NativeVideoOutputEvent::PlacementSwitchFailed { request_id } => {
                match self.native_video_mode_switch {
                    Some(pending) if pending.request_id == request_id => {
                        self.revert_failed_video_presentation_switch(pending.target_presentation);
                    }
                    _ => {
                        crate::logger::log(format!(
                            "[native-video] stale PlacementSwitchFailed ignored: \
                             request={request_id}"
                        ));
                    }
                }
            }
            crate::video::NativeVideoOutputEvent::SetVst3PanelVisible { visible } => {
                self.set_native_video_vst3_panel_visible(visible);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::SetVst3VideoCompact { compact } => {
                self.set_native_video_vst3_compact(compact);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::SetVst3PanelPos { pos } => {
                self.set_native_video_vst3_panel_pos(pos);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::Vst3ShowSlotGui { idx, path } => {
                self.show_native_video_vst3_slot_gui(idx, path);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::Vst3HideSlotGui { idx, path } => {
                self.hide_native_video_vst3_slot_gui(idx, path);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::Vst3SetBypass { idx, path, bypass } => {
                self.set_native_video_vst3_slot_bypass(idx, path, bypass);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::Vst3LoadChainSlot { slot_idx } => {
                self.load_vst3_chain_slot(slot_idx);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::Vst3SaveChainSlot { slot_idx } => {
                self.save_vst3_chain_slot(slot_idx);
                self.sync_native_video_vst3_panel(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::VideoAdjustLoadSlot { slot_idx } => {
                self.load_video_adjust_slot(slot_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::VideoAdjustSaveSlot { slot_idx } => {
                self.save_video_adjust_slot(slot_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::SeekToStartAndPlay => {
                self.handle_native_video_seek_to_start_command(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::TogglePlay => {
                self.handle_native_video_toggle_play_command(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::ToggleMute => {
                self.handle_native_video_toggle_mute_command(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::ToggleLoop => {
                self.handle_native_video_toggle_loop_command(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::ToggleContinuous => {
                self.cycle_video_continuous_mode_common(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::SetVolume { volume, persist } => {
                self.handle_native_video_set_volume_command(ctx, fs_idx, volume, persist);
            }
            crate::video::NativeVideoOutputEvent::SetVideoAdjustments {
                mut adjustments,
                persist,
            } => {
                adjustments.sanitize();
                self.settings.video_adjustments = adjustments;
                self.sync_native_video_grade();
                if persist {
                    self.settings.save();
                }
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::SetPlaybackSpeed { speed } => {
                self.handle_video_playback_speed_command(ctx, fs_idx, speed);
            }
            crate::video::NativeVideoOutputEvent::CopyFrameToClipboard => {
                self.copy_video_frame_to_clipboard(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::FrameStep { direction } => {
                self.step_video_frame(ctx, fs_idx, direction);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::AddBookmarkAt { target_secs } => {
                self.handle_native_video_add_bookmark_command(ctx, fs_idx, target_secs);
            }
            crate::video::NativeVideoOutputEvent::SetPinAt { target_secs } => {
                self.handle_native_video_set_pin_command(ctx, fs_idx, target_secs);
            }
            // 動画 HUD 2 段化リデザイン (Phase 4): HUD の前/次マーカーボタンクリック
            // を J/K キーと同じ `jump_native_video_marker` に dispatch する。
            crate::video::NativeVideoOutputEvent::JumpMarker { next } => {
                self.jump_native_video_marker(fs_idx, next);
                self.mark_native_video_hud_activity(ctx);
            }
            // 動画 HUD 2 段化リデザイン (Phase 5): HUD カメラパレットの保存ボタンクリック
            // を Ctrl+S と同じ `save_video_frame_to_file` に dispatch する。
            crate::video::NativeVideoOutputEvent::SaveFrameToFile => {
                self.save_video_frame_to_file(ctx, fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::SetBookmarkTitle { id, title } => {
                self.handle_native_video_set_bookmark_title_command(ctx, fs_idx, id, title);
            }
            crate::video::NativeVideoOutputEvent::DeleteBookmark { id } => {
                self.handle_native_video_delete_bookmark_command(ctx, fs_idx, id);
            }
            crate::video::NativeVideoOutputEvent::DeletePin => {
                self.handle_native_video_delete_pin_command(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::BulkAddBookmarks { entries } => {
                self.handle_native_video_bulk_add_bookmarks_command(ctx, fs_idx, entries);
            }
            crate::video::NativeVideoOutputEvent::ExportBookmarksToClipboard { seconds_only } => {
                self.handle_native_video_export_bookmarks_command(ctx, fs_idx, seconds_only);
            }
            crate::video::NativeVideoOutputEvent::ClearAllBookmarksForCurrent => {
                self.handle_native_video_clear_all_bookmarks_command(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::OpenExternalUrl { url } => {
                self.handle_native_video_open_external_url_command(ctx, fs_idx, url);
            }
            crate::video::NativeVideoOutputEvent::SetRating { stars } => {
                if self.set_rating(fs_idx, stars) {
                    self.sync_native_video_metadata(fs_idx);
                }
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::ToggleTag { name } => {
                self.request_tag_toggle_for_selection(&name, crate::app::ActionSurface::Viewer);
                self.sync_native_video_metadata(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::AddTag { name } => {
                self.request_tag_add_for_selection(&name, crate::app::ActionSurface::Viewer);
                self.sync_native_video_metadata(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::RemoveTag { name } => {
                self.request_tag_remove_for_selection(&name, crate::app::ActionSurface::Viewer);
                self.sync_native_video_metadata(fs_idx);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::OpenTagViewForTag { name } => {
                self.open_tag_view_for_tag(&name);
                self.mark_native_video_hud_activity(ctx);
            }
            crate::video::NativeVideoOutputEvent::ToggleNormalize => {
                self.handle_toggle_normalize(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::DisableNormalize => {
                self.handle_disable_normalize(ctx, fs_idx);
            }
            crate::video::NativeVideoOutputEvent::CancelNormalizeScan => {
                self.handle_cancel_normalize_scan(ctx, fs_idx);
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_window_event(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        event: crate::video::native_window::NativeVideoWindowEvent,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        // 音声 VST シェル (Inc 6 ②-3): マウスホイール (動画の前/次ナビ) とマウスボタン
        // (右クリック close / ジェスチャ / タイル) は audio シェルに不適なので無視する。
        // CloseRequested は handle_native_video_output_event が既にシェル離脱へ振っており
        // ここには渡らない。MouseMove/Leave/キーは通す (HUD hover / Esc 離脱)。
        if self
            .music_vst_shell
            .as_ref()
            .is_some_and(|s| s.fs_idx == fs_idx)
            && matches!(
                event,
                crate::video::native_window::NativeVideoWindowEvent::MouseButton(_)
                    | crate::video::native_window::NativeVideoWindowEvent::MouseWheel(_)
            )
        {
            return;
        }
        match event {
            crate::video::native_window::NativeVideoWindowEvent::CloseRequested { generation } => {
                if !self.accept_native_video_close(fs_idx, generation, "window_close_requested") {
                    return;
                }
                self.handle_fullscreen_close_request_immediate();
            }
            crate::video::native_window::NativeVideoWindowEvent::KeyDown(key) => {
                self.handle_native_video_key_event(ctx, fs_idx, key);
            }
            crate::video::native_window::NativeVideoWindowEvent::KeyUp(_) => {}
            crate::video::native_window::NativeVideoWindowEvent::Text(_) => {}
            crate::video::native_window::NativeVideoWindowEvent::Ime(_) => {}
            crate::video::native_window::NativeVideoWindowEvent::MouseMove(mouse) => {
                if mouse.x < 340 {
                    self.sync_native_video_timeline_markers(fs_idx);
                }
                if self.mouse_ring_flick.is_some() {
                    let _ = self.update_native_mouse_ring_flick(
                        ctx,
                        crate::ring_shortcut::RingShortcutContext::VideoFullscreen,
                        egui::pos2(mouse.x as f32, mouse.y as f32),
                        true,
                        false,
                    );
                }
                if self.mouse_gesture.is_some() {
                    let _ = self.update_native_mouse_gesture(
                        ctx,
                        crate::ring_shortcut::RightDragContext::VideoFullscreen,
                        egui::pos2(mouse.x as f32, mouse.y as f32),
                        true,
                        false,
                    );
                }
                // navigation preview の HUD 全画面化で OS が届ける zero-delta (位置不変) move では
                // hud activity を入れない。`mark_native_video_hud_activity` は overlay の位置ゲートを
                // バイパスして `player.mark_cursor_activity()` で auto-hide 済みカーソルを復活させて
                // しまうため、**実際にカーソルが動いた move のときだけ** 呼ぶ (2026-06-06)。位置不変の
                // ときは repaint だけ行う。直近位置不明 (None) の扱いは overlay と同じ純関数
                // `cursor_move_is_activity` に委ねる: hidden 中の None は spurious とみなして抑制し、
                // クリック入場 (= move 未転送) 直後の zero-delta nav でも復活しないようにする。
                let pos = (mouse.x, mouse.y);
                let moved = crate::video::native_presenter::cursor_move_is_activity(
                    self.native_video_last_move_client,
                    pos,
                    self.cursor_hidden,
                );
                self.native_video_last_move_client = Some(pos);
                if moved {
                    self.mark_native_video_hud_activity(ctx);
                } else {
                    ctx.request_repaint();
                }
            }
            crate::video::native_window::NativeVideoWindowEvent::MouseButton(button) => {
                self.handle_native_video_mouse_button(ctx, fs_idx, button);
            }
            crate::video::native_window::NativeVideoWindowEvent::MouseWheel(wheel) => {
                self.mark_native_video_hud_activity(ctx);
                if wheel.ctrl && self.video_tile_mode_active {
                    let delta = if wheel.delta > 0 { -1 } else { 1 };
                    self.adjust_native_video_tile_columns(ctx, fs_idx, delta);
                } else if !wheel.ctrl {
                    let delta = if wheel.delta < 0 { 1 } else { -1 };
                    self.navigate_native_video_fullscreen(ctx, fs_idx, delta);
                }
            }
            crate::video::native_window::NativeVideoWindowEvent::MouseLeave => {
                self.native_video_pointer_down = None;
                self.native_video_context_menu_dismiss_click_started_at = None;
                self.native_video_secondary_press_start = None;
                self.native_video_middle_press_start = None;
                let gesture_was_active = self.mouse_gesture.is_some();
                self.cancel_mouse_ring_flick();
                self.set_native_video_ring_guide_overlay(None);
                if gesture_was_active {
                    self.set_native_video_ring_picker_overlay(None);
                }
                self.request_native_video_hud_repaint(ctx);
            }
            // 内部処理イベント (presenter thread が直接消費する)。UI には届かない想定。
            crate::video::native_window::NativeVideoWindowEvent::GeometryChanged { .. } => {}
            crate::video::native_window::NativeVideoWindowEvent::DpiChanged { .. }
            | crate::video::native_window::NativeVideoWindowEvent::RequestRaiseHud
            | crate::video::native_window::NativeVideoWindowEvent::RequestFocusClaim
            | crate::video::native_window::NativeVideoWindowEvent::Touch(_)
            | crate::video::native_window::NativeVideoWindowEvent::CursorOwnership(_)
            | crate::video::native_window::NativeVideoWindowEvent::Destroyed => {}
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_seek_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        target_secs: f64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        let did_seek = if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx)
        {
            player.seek(target_secs);
            self.mark_native_video_hud_activity(ctx);
            true
        } else {
            false
        };
        // CH/BM ループモード時、seek 後に loop_target_secs を再計算する
        // (= 新位置の chapter/bookmark 開始秒に揃える)。
        self.apply_loop_mode_to_player(fs_idx);
        if did_seek {
            self.maybe_start_normalize_scan_for_play_intent(fs_idx);
        }
    }

    #[cfg(windows)]
    pub(crate) fn handle_native_video_tile_seek_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        target_secs: f64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        let did_seek = if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx)
        {
            crate::logger::log(format!(
                "[native-video] tile seek command: idx={fs_idx} target={target_secs:.3} engine_state={} seek_serial={} playing={} pos={:.3} video_rx_len={} audio_rx_len={}",
                player.engine_state_name(),
                player.current_seek_serial(),
                player.is_playing(),
                player.position(),
                player.video_rx_len(),
                player.audio_rx_len()
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_presenter",
                    "tile_seek_command",
                    None,
                    0,
                    &[
                        ("idx", serde_json::Value::from(fs_idx as i64)),
                        ("target", serde_json::Value::from(target_secs)),
                        (
                            "engine_state",
                            serde_json::Value::from(player.engine_state_name()),
                        ),
                        (
                            "seek_serial",
                            serde_json::Value::from(player.current_seek_serial() as i64),
                        ),
                        ("playing", serde_json::Value::from(player.is_playing())),
                        ("position", serde_json::Value::from(player.position())),
                        (
                            "video_rx_len",
                            serde_json::Value::from(player.video_rx_len() as i64),
                        ),
                        (
                            "audio_rx_len",
                            serde_json::Value::from(player.audio_rx_len() as i64),
                        ),
                    ],
                );
            }
            player.seek(target_secs);
            player.set_native_tile_overlay(None);
            true
        } else {
            false
        };
        self.close_video_tile_mode();
        self.mark_native_video_hud_activity(ctx);
        self.apply_loop_mode_to_player(fs_idx);
        if did_seek {
            self.maybe_start_normalize_scan_for_play_intent(fs_idx);
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_seek_to_start_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        let did_seek = if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx)
        {
            // `seek(0.0)` は内部で `apply_command(Play)` を発行し autoplay intent を
            // 立てるので、追加の `toggle_play()` は不要 (Codex P2-1 2026-05-17)。
            // 旧コードは `if !is_playing() { toggle_play() }` で「念のため再生」を
            // 意図していたが、intent ベースの toggle_play (= 2026-05 修正) では
            // `intent_playing()=true` を見て Pause に反転していたバグ。
            player.seek(0.0);
            true
        } else {
            false
        };
        if did_seek {
            self.mark_native_video_hud_activity(ctx);
            // 0 への seek は loop_target も chapter/bookmark の最初の区間に揃える
            self.apply_loop_mode_to_player(fs_idx);
            self.maybe_start_normalize_scan_for_play_intent(fs_idx);
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_toggle_play_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        let will_request_play = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => !player.intent_playing(),
            _ => false,
        };
        if will_request_play && self.start_normalize_scan_for_deferred_play_intent(fs_idx) {
            self.mark_native_video_hud_activity(ctx);
            return;
        }
        let modal_scan_this_video = self.normalize_scan_is_modal_for_current_player(fs_idx);
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.toggle_play();
            if will_request_play && player.audio_preroll_suspended() && !modal_scan_this_video {
                crate::logger::log(format!(
                    "[native-video] resume playback after deferred normalize scan was unavailable idx={fs_idx}"
                ));
                player.set_audio_preroll_suspended(false);
            }
            self.mark_native_video_hud_activity(ctx);
            self.maybe_start_normalize_scan_for_play_intent(fs_idx);
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_open_external_url_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        url: String,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_playing(false);
        }
        crate::ui_helpers::open_url(&url);
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_toggle_mute_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        if self.toggle_video_session_mute_for_fs_idx(fs_idx) {
            self.mark_native_video_hud_activity(ctx);
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_toggle_loop_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        self.cycle_native_video_loop_common(ctx, fs_idx);
    }

    /// L キー / ループボタンクリックのサイクル処理 (cfg 不問の共通部)。
    /// CH/BM 段階は当該動画にデータが無いとき自動でスキップする。
    /// 設定保存 → effective mode を再計算して player に反映 → トースト → HUD activity 更新。
    pub(crate) fn cycle_native_video_loop_common(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.video_continuous_mode.is_enabled() {
            self.show_native_video_overlay_toast("連続再生中はループ無効".to_string(), false);
            self.mark_native_video_hud_activity(ctx);
            return;
        }
        // Phase 1: チャプター / ブックマークの有無を取得 (player から snapshot)
        let (path, has_ch) = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                let chapters_empty = player.info().map(|i| i.chapters.is_empty()).unwrap_or(true);
                (player.path().clone(), !chapters_empty)
            }
            _ => return,
        };
        // Phase 2: bookmark cache を ensure してから snapshot 経由で取得
        self.ensure_fullscreen_video_marker_cache(fs_idx);
        let (_pin, bookmarks) = self.fullscreen_video_marker_snapshot(fs_idx, &path);
        let has_bm = !bookmarks.is_empty();

        let next = crate::settings::cycle_loop_mode(self.settings.video_loop_mode, has_ch, has_bm);
        self.settings.video_loop_mode = next;
        // 旧 bool を in-memory でも同期 (= save() 内 clone でも導出するが念のため)。
        self.settings.video_loop = !matches!(next, crate::settings::VideoLoopMode::Off);
        self.settings.save();

        self.apply_loop_mode_to_player(fs_idx);

        // トースト通知 (右上、J/K や画像系ショートカットと同じ位置・サイズ)。
        // 中央 (= centered=true) はクリック面積が大きすぎてループボタンを覆ってしまう。
        let label = next.label();
        self.show_native_video_overlay_toast(format!("ループ: {label}"), false);

        #[cfg(windows)]
        self.mark_native_video_hud_activity(ctx);
        #[cfg(not(windows))]
        let _ = ctx;
    }

    /// 現在の `settings.video_loop_mode` と動画の chapter/bookmark 状況から
    /// `effective_mode` を計算し、player に loop_enabled / loop_target_secs / display_mode を
    /// 反映する。
    ///
    /// 呼ぶ場所: 動画 open 直後、VideoInfo 到着、L キー / ボタンクリック (cycle 経由)、
    /// 各種 seek 後 (J/K, ←/→, シークバー, タイル seek, マーカージャンプ),
    /// ブックマーク CRUD 直後, ナビゲーション後。
    pub(crate) fn apply_loop_mode_to_player(&mut self, fs_idx: usize) {
        if self.video_continuous_mode.is_enabled() {
            if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                player.set_loop_enabled(false);
                player.set_native_loop_enabled(false);
                player.set_native_loop_mode(self.settings.video_loop_mode);
                player.set_native_continuous_mode(self.video_continuous_mode);
            }
            return;
        }
        // Phase 1: player から必要データを snapshot して borrow を即解放
        let (path, chapter_starts, pos, serial) = {
            let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
                return;
            };
            let chapters = player.info().map(|i| i.chapters.as_slice()).unwrap_or(&[]);
            (
                player.path().clone(),
                crate::video::decoder::boundary_starts_from_chapters(chapters),
                player.position_secs(),
                player.current_seek_serial(),
            )
        };

        // Phase 2: cache 経由で bookmark を取得 (fallback DB 読みを許容、cycle 1 回限り)
        let (_pin, bookmarks) = self.fullscreen_video_marker_snapshot(fs_idx, &path);
        let bookmark_starts = crate::video_bookmarks::boundary_starts_from_bookmarks(&bookmarks);
        let has_ch = !chapter_starts.is_empty();
        let has_bm = !bookmark_starts.is_empty();
        let display_mode = self.settings.video_loop_mode;
        let eff = crate::settings::effective_loop_mode(display_mode, has_ch, has_bm);

        let target = match eff {
            crate::settings::VideoLoopMode::Off | crate::settings::VideoLoopMode::Full => 0.0,
            crate::settings::VideoLoopMode::Chapter => {
                crate::settings::start_at(&chapter_starts, pos).unwrap_or(0.0)
            }
            crate::settings::VideoLoopMode::Bookmark => {
                crate::settings::start_at(&bookmark_starts, pos).unwrap_or(0.0)
            }
        };
        let enabled = !matches!(eff, crate::settings::VideoLoopMode::Off);

        // Phase 3: 再度 player を借りて状態を push
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_loop_enabled(enabled);
            player.set_loop_target_secs(target);
            #[cfg(windows)]
            player.set_native_loop_mode(display_mode); // HUD 表示は user intent
            #[cfg(windows)]
            player.set_native_loop_enabled(enabled); // 互換のため残す
            player.set_native_continuous_mode(self.video_continuous_mode);
        }
        self.last_loop_pos.insert(fs_idx, (pos, serial));
    }

    /// 動画再生中、CH/BM ループモード時に「次境界跨ぎ」を検出してループ seek を発行する。
    /// `poll_video` の Phase 3 (= native_events 反映後) から fs_idx ごとに呼ぶ。
    ///
    /// 手動 seek (シークバー/J/K/←/→/タイル) は serial 変化で検出して baseline 更新のみに
    /// 切り替え、誤爆 seek を防ぐ (Codex P1 第2ラウンド)。
    pub(crate) fn tick_native_video_loop_boundary(&mut self, fs_idx: usize) {
        if self.video_continuous_mode.is_enabled() {
            return;
        }
        let display_mode = self.settings.video_loop_mode;
        // 早期 return: HUD 表示は CH/BM でも、effective が Off/Full なら何もしない
        // (= データ無し動画では境界 tick が発火しない)。
        if matches!(
            display_mode,
            crate::settings::VideoLoopMode::Off | crate::settings::VideoLoopMode::Full
        ) {
            return;
        }
        // Phase 1: player + cache から必要データを snapshot。再生中でなければ何もしない。
        // 一時停止 / scrub 中は baseline だけ最新位置に更新 (Codex P2 第7ラウンド —
        // 一時停止中に境界手前へ scrub すると即ループ開始点へ戻されるのを防ぐ)。
        let (cur, serial, chapters_owned, is_playing) = {
            let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
                return;
            };
            let chapters = player
                .info()
                .map(|i| i.chapters.clone())
                .unwrap_or_default();
            (
                player.position_secs(),
                player.current_seek_serial(),
                chapters,
                player.is_playing(),
            )
        };
        if !is_playing {
            self.last_loop_pos.insert(fs_idx, (cur, serial));
            return;
        }

        // bookmarks は cache 直読み (DB fallback しない)
        let bookmarks = match self
            .fullscreen_video_marker_cache
            .as_ref()
            .filter(|c| c.fs_idx == fs_idx)
        {
            Some(c) => c.bookmarks.clone(),
            None => return,
        };

        let chapter_starts = crate::video::decoder::boundary_starts_from_chapters(&chapters_owned);
        let bookmark_starts = crate::video_bookmarks::boundary_starts_from_bookmarks(&bookmarks);
        let has_ch = !chapter_starts.is_empty();
        let has_bm = !bookmark_starts.is_empty();
        let eff = crate::settings::effective_loop_mode(display_mode, has_ch, has_bm);
        if !matches!(
            eff,
            crate::settings::VideoLoopMode::Chapter | crate::settings::VideoLoopMode::Bookmark
        ) {
            return;
        }
        let starts = match eff {
            crate::settings::VideoLoopMode::Chapter => chapter_starts,
            crate::settings::VideoLoopMode::Bookmark => bookmark_starts,
            _ => unreachable!(),
        };
        if starts.is_empty() {
            return;
        }

        let (prev_pos, prev_serial) = self
            .last_loop_pos
            .get(&fs_idx)
            .copied()
            .unwrap_or((cur, serial));

        // 境界判定: prev_pos 側の区間で計算 (Codex P1 — cur 側は跨いだ瞬間に次区間に入る)
        let prev_start = crate::settings::start_at(&starts, prev_pos).unwrap_or(0.0);
        let next_boundary = crate::settings::first_boundary_after(&starts, prev_start);

        const LOOP_BOUNDARY_TOL: f64 = 0.020;
        match crate::settings::decide_boundary_action(
            prev_pos,
            prev_serial,
            cur,
            serial,
            prev_start,
            next_boundary,
            LOOP_BOUNDARY_TOL,
        ) {
            crate::settings::BoundaryDecision::BaselineUpdate => {
                // serial 変化 / 巻き戻り → loop_target_secs を cur 基準で再計算 + baseline 更新
                let new_target = crate::settings::start_at(&starts, cur).unwrap_or(0.0);
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    player.set_loop_target_secs(new_target);
                }
                self.last_loop_pos.insert(fs_idx, (cur, serial));
            }
            crate::settings::BoundaryDecision::Loop { seek_to } => {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    player.seek(seek_to);
                    // loop_target は同区間の開始 (= seek_to) で維持
                    player.set_loop_target_secs(seek_to);
                }
                // seek 後 baseline は seek_to に。次 tick で serial 変化を検出すれば baseline
                // が再更新される。
                self.last_loop_pos.insert(fs_idx, (seek_to, serial));
            }
            crate::settings::BoundaryDecision::Continue => {
                // 現区間の開始秒 (= prev_start) で loop_target を維持する。
                // これで動画 open 直後 (info() 未到着 → 初期 0.0) から info() 到着後の最初の
                // tick で正しい値に書き換わる。値が変わらない通常 tick では no-op。
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    player.set_loop_target_secs(prev_start);
                }
                self.last_loop_pos.insert(fs_idx, (cur, serial));
            }
        }
    }

    /// 音量ノーマライズ ボタン左クリック (3 状態モデル: Off → ON 化 / OnApplied → OFF 化 /
    /// OnUnmeasured → スキャン起動)。
    #[cfg(windows)]
    pub(crate) fn handle_toggle_normalize(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        // [Scanning] のモーダル段階だけクリック無効。仮 gain 適用後のバックグラウンド
        // scan 中は、クリック OFF で scan cancel + 全体 OFF にできる。
        if self
            .normalize_state
            .as_ref()
            .is_some_and(|state| !state.provisional_applied)
        {
            return;
        }
        use crate::video::normalize_types::NormalizeUiState;
        // ── snapshot phase: self の借用を短くする ──
        let current_state = self
            .normalize_ui_states
            .get(&fs_idx)
            .copied()
            .unwrap_or(NormalizeUiState::Off);
        let target_milli = self.settings.clamped_audio_normalize_target_lufs_milli();
        let current_path: Option<PathBuf> = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => Some(player.path().to_path_buf()),
            _ => None,
        };
        let Some(current_path) = current_path else {
            return;
        };

        match current_state {
            NormalizeUiState::OnApplied { .. } | NormalizeUiState::ProvisionalApplied { .. } => {
                // [OnApplied/ProvisionalApplied] → [Off]:
                // グローバル OFF + 全 player に gain=1.0 適用。仮 scan 中なら cancel も行う。
                self.disable_normalize_globally();
            }
            NormalizeUiState::OnUnmeasured => {
                // [OnUnmeasured] → [Scanning]: グローバル ON は維持、スキャン起動
                self.start_normalize_scan(fs_idx);
            }
            NormalizeUiState::Off => {
                // [Off] → [OnApplied] or [Scanning]: グローバル ON 化、現在動画 DB lookup
                self.settings.audio_normalize_enabled = true;
                self.settings.save();
                let lookup = self
                    .audio_normalize_db
                    .as_ref()
                    .and_then(|db| db.lookup(&current_path, target_milli));
                if let Some(result) = lookup {
                    self.apply_normalize_gain_db_to_player(fs_idx, result.gain_db);
                    self.normalize_ui_states.insert(
                        fs_idx,
                        NormalizeUiState::OnApplied {
                            gain_db: result.gain_db,
                        },
                    );
                } else {
                    self.start_normalize_scan(fs_idx);
                }
                // 他の動画にも反映 (ヒットしたものから順に適用)
                self.apply_normalize_to_all_videos_except(fs_idx, target_milli);
            }
            NormalizeUiState::Scanning => {
                // is_some() ガードで通常到達しない
            }
        }
        self.mark_native_video_hud_activity(ctx);
    }

    /// 音量ノーマライズ ボタン右クリック (どの状態からでもグローバル OFF 化、救済経路)。
    #[cfg(windows)]
    pub(crate) fn handle_disable_normalize(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        self.disable_normalize_globally();
        self.mark_native_video_hud_activity(ctx);
    }

    /// 進捗パネル × ボタン or ESC でキャンセル。
    /// take() で state を捨てて新規スキャン即開始可能にする。
    #[cfg(windows)]
    pub(crate) fn handle_cancel_normalize_scan(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let should_drop = self
            .normalize_state
            .as_ref()
            .map(|s| s.fs_idx == fs_idx)
            .unwrap_or(false);
        if !should_drop {
            return;
        }
        if let Some(state) = self.normalize_state.take() {
            state.cancel();
            self.normalize_auto_scan_suppressed.insert(state.fs_idx);
            // 元再生状態に復帰
            if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&state.fs_idx) {
                if state.was_playing {
                    player.set_playing(true);
                    player.set_audio_preroll_suspended(false);
                }
            }
            self.normalize_ui_states.insert(
                state.fs_idx,
                crate::video::normalize_types::NormalizeUiState::OnUnmeasured,
            );
            // worker は cancel atomic を見て早期 return、_join + rx も drop で解放される
        }
        self.mark_native_video_hud_activity(ctx);
    }

    /// 全 fs_cache の VideoPlayer に gain=1.0 を即時適用 + Settings 保存。
    /// DB エントリは残す (= 次回 ON 復帰で即適用できる)。
    #[cfg(windows)]
    pub(super) fn disable_normalize_globally(&mut self) {
        use crate::video::normalize_types::NormalizeUiState;
        if let Some(state) = self.normalize_state.take() {
            state.cancel();
            if state.was_playing {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&state.fs_idx) {
                    if player.path() == state.file_path.as_path() {
                        player.set_playing(true);
                        player.set_audio_preroll_suspended(false);
                    }
                }
            }
        }
        self.settings.audio_normalize_enabled = false;
        self.settings.save();
        self.normalize_auto_scan_suppressed.clear();
        let fs_idxs: Vec<usize> = self.fs_cache.keys().copied().collect();
        for idx in fs_idxs {
            if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&idx) {
                apply_normalize_gain_with_perf(player, idx, 1.0, 0.0, "toggle_off");
                self.normalize_ui_states.insert(idx, NormalizeUiState::Off);
            }
        }
    }

    /// 1 player に gain_db を線形変換して適用。
    /// **audio buffer は clear しない** (= `apply_normalize_gain_with_perf` の doc 参照)。
    /// 過去に `clear_audio_output_buffer()` を併用していたが、`raw_pending` 5 秒分を
    /// 捨てて A/V offset が永続的にズレるバグの原因だったので 2026-05-11 に削除。
    #[cfg(windows)]
    pub(super) fn apply_normalize_gain_db_to_player(&mut self, fs_idx: usize, gain_db: f32) {
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            let linear = 10.0_f64.powf(gain_db as f64 / 20.0);
            apply_normalize_gain_with_perf(player, fs_idx, linear, gain_db, "toggle_on");
        }
    }

    /// 他の fs_cache entry (= except_fs_idx 以外) について DB lookup → ヒットなら適用、
    /// ミスなら OnUnmeasured 設定。トグル ON 時の同期適用に使う。
    #[cfg(windows)]
    pub(super) fn apply_normalize_to_all_videos_except(
        &mut self,
        except_fs_idx: usize,
        target_milli: i32,
    ) {
        use crate::video::normalize_types::NormalizeUiState;
        let other_idxs: Vec<usize> = self
            .fs_cache
            .keys()
            .copied()
            .filter(|i| *i != except_fs_idx)
            .collect();
        for idx in other_idxs {
            let path = match self.fs_cache.get(&idx) {
                Some(FsCacheEntry::Video { player, .. }) => Some(player.path().to_path_buf()),
                _ => None,
            };
            let Some(path) = path else { continue };
            let lookup = self
                .audio_normalize_db
                .as_ref()
                .and_then(|db| db.lookup(&path, target_milli));
            match lookup {
                Some(result) => {
                    self.apply_normalize_gain_db_to_player(idx, result.gain_db);
                    self.normalize_ui_states.insert(
                        idx,
                        NormalizeUiState::OnApplied {
                            gain_db: result.gain_db,
                        },
                    );
                }
                None => {
                    self.normalize_ui_states
                        .insert(idx, NormalizeUiState::OnUnmeasured);
                }
            }
        }
    }

    /// ノーマライズ ON + 未測定 + 再生 intent が立っている動画だけ、自動スキャンを開始する。
    ///
    /// 再生中の seek / play toggle / open 直後など複数経路から呼ぶため、ここで冪等性を
    /// 一元的に守る。ユーザーキャンセルやスキャン失敗後は、この fullscreen セッション中の
    /// 自動再発火を抑止し、手動 Norm クリックだけで再試行できるようにする。
    #[cfg(windows)]
    pub(super) fn maybe_start_normalize_scan_for_play_intent(&mut self, fs_idx: usize) -> bool {
        let should_scan = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.intent_playing(),
            _ => false,
        };
        if !should_scan {
            return false;
        }
        self.start_normalize_scan_for_deferred_play_intent(fs_idx)
    }

    /// 未測定動画をこれから再生する場合に、再生開始前の一瞬の未補正音を避けるため
    /// player は paused intent + audio preroll suspended のまま scan を開始し、
    /// 完了後に再生 intent と preroll を復帰する。
    #[cfg(windows)]
    pub(super) fn start_normalize_scan_for_deferred_play_intent(&mut self, fs_idx: usize) -> bool {
        if self.mark_existing_normalize_scan_for_deferred_play(fs_idx) {
            return true;
        }
        if !self.normalize_auto_scan_target_ready(fs_idx) {
            return false;
        }
        self.start_normalize_scan_inner(fs_idx, Some(true));
        true
    }

    /// 再生前スキャンを開始できなかった場合の保険。
    ///
    /// 旧動画の scan がまだ残っているなどの理由で guard に弾かれても、player を
    /// audio_preroll_suspended のままにすると再生・seek が進まなくなる。未補正で
    /// 再生を始められる状態に戻し、次の機会の自動 scan に任せる。
    #[cfg(windows)]
    pub(super) fn resume_deferred_normalize_playback_without_scan(&mut self, fs_idx: usize) {
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            if player.audio_preroll_suspended() {
                crate::logger::log(format!(
                    "[native-video] deferred normalize scan not started; resume playback idx={fs_idx}"
                ));
            }
            player.set_playing(true);
            player.set_audio_preroll_suspended(false);
        }
    }

    #[cfg(windows)]
    fn normalize_auto_scan_target_ready(&self, fs_idx: usize) -> bool {
        use crate::video::normalize_types::NormalizeUiState;
        self.settings.audio_normalize_enabled
            && self.fullscreen_idx == Some(fs_idx)
            && !self.normalize_auto_scan_suppressed.contains(&fs_idx)
            && self.normalize_ui_states.get(&fs_idx).copied()
                == Some(NormalizeUiState::OnUnmeasured)
            && matches!(self.fs_cache.get(&fs_idx), Some(FsCacheEntry::Video { .. }))
    }

    #[cfg(windows)]
    fn normalize_scan_matches_current_player(&self, fs_idx: usize) -> bool {
        let Some(state) = self.normalize_state.as_ref() else {
            return false;
        };
        if state.fs_idx != fs_idx {
            return false;
        }
        match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.path() == state.file_path.as_path(),
            _ => false,
        }
    }

    #[cfg(windows)]
    /// 仮 gain 適用前の scan は、未補正音の先読みを避けるため再生・キー操作を
    /// モーダルに止める。`ProvisionalApplied` 後は同じ worker が走っていても
    /// 確定値待ちのバックグラウンド scan なので通常操作を許可する。
    pub(crate) fn normalize_scan_is_modal_for_current_player(&self, fs_idx: usize) -> bool {
        self.normalize_scan_matches_current_player(fs_idx)
            && self
                .normalize_state
                .as_ref()
                .is_some_and(|state| !state.provisional_applied)
    }

    #[cfg(windows)]
    fn mark_existing_normalize_scan_for_deferred_play(&mut self, fs_idx: usize) -> bool {
        if !self.normalize_scan_is_modal_for_current_player(fs_idx) {
            return false;
        }
        if let Some(state) = self.normalize_state.as_mut() {
            state.was_playing = true;
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_audio_preroll_suspended(true);
            player.set_playing(false);
        }
        true
    }

    /// スキャン worker thread を起動。再生中なら一時停止 → スキャン → poll で完了検知。
    #[cfg(windows)]
    pub(super) fn start_normalize_scan(&mut self, fs_idx: usize) {
        self.start_normalize_scan_inner(fs_idx, None);
    }

    #[cfg(windows)]
    fn start_normalize_scan_inner(&mut self, fs_idx: usize, was_playing_override: Option<bool>) {
        use crate::video::normalize_types::NormalizeUiState;
        let (path, was_playing) = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                let was_playing = was_playing_override.unwrap_or_else(|| player.intent_playing());
                (player.path().to_path_buf(), was_playing)
            }
            _ => return,
        };
        self.normalize_auto_scan_suppressed.remove(&fs_idx);
        // 既存 state を捨てる (cancel を立てておく) — 通常は is_some() で弾かれているが defensive
        if let Some(prev) = self.normalize_state.take() {
            prev.cancel();
            let prev_still_current = matches!(
                self.fs_cache.get(&prev.fs_idx),
                Some(FsCacheEntry::Video { player, .. }) if player.path() == prev.file_path.as_path()
            );
            if prev_still_current {
                self.normalize_ui_states
                    .insert(prev.fs_idx, NormalizeUiState::OnUnmeasured);
            } else {
                self.normalize_ui_states.remove(&prev.fs_idx);
            }
        }
        // 再生中なら一時停止し、測定前の raw→processed 先読みも止める。
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            if was_playing {
                player.set_audio_preroll_suspended(true);
                player.set_playing(false);
            }
        }
        let target_milli = self.settings.clamped_audio_normalize_target_lufs_milli();
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(crate::video::normalize_scanner::NormalizeScanProgress::default());
        let (tx, rx) = mpsc::channel();
        let cancel_clone = cancel.clone();
        let progress_clone = progress.clone();
        let path_clone = path.clone();
        let join = std::thread::Builder::new()
            .name("normalize-scan".to_string())
            .spawn(move || {
                let tx_provisional = tx.clone();
                let mut send_provisional =
                    move |result: crate::video::normalize_types::NormalizeResult| {
                        let _ = tx_provisional
                            .send(crate::app::normalize::NormalizeMessage::Provisional(result));
                    };
                let result = crate::video::normalize_scanner::scan_audio_loudness_with_provisional(
                    &path_clone,
                    target_milli,
                    cancel_clone,
                    progress_clone,
                    crate::video::normalize_scanner::PROVISIONAL_SCAN_AFTER_SECS,
                    &mut send_provisional,
                );
                let _ = tx.send(crate::app::normalize::NormalizeMessage::from(result));
            });
        let join = match join {
            Ok(j) => j,
            Err(e) => {
                crate::logger::log(format!("normalize-scan thread spawn failed: {e}"));
                // Codex P2: spawn 失敗時は元再生状態に戻し、UI 状態も OnUnmeasured に
                if was_playing {
                    if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                        player.set_playing(true);
                        player.set_audio_preroll_suspended(false);
                    }
                }
                self.normalize_ui_states
                    .insert(fs_idx, NormalizeUiState::OnUnmeasured);
                self.normalize_auto_scan_suppressed.insert(fs_idx);
                return;
            }
        };
        self.normalize_state = Some(crate::app::normalize::NormalizeScanState {
            fs_idx,
            cancel,
            progress,
            rx,
            was_playing,
            file_path: path,
            target_lufs_milli: target_milli,
            provisional_applied: false,
            provisional_result: None,
            _join: join,
        });
        self.normalize_ui_states
            .insert(fs_idx, NormalizeUiState::Scanning);
    }

    /// スキャン完了 / キャンセル / エラーを検知して後処理する。`App::update` から毎フレーム呼ぶ。
    #[cfg(windows)]
    pub(super) fn poll_normalize_scan(&mut self, _ctx: &egui::Context) {
        use crate::video::normalize_types::NormalizeUiState;
        // 1. メッセージ peek (try_recv)
        let msg = match self.normalize_state.as_ref() {
            Some(state) => match state.rx.try_recv() {
                Ok(msg) => Some(Ok(msg)),
                Err(mpsc::TryRecvError::Empty) => return,
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(())),
            },
            None => return,
        };
        if let Some(Ok(crate::app::normalize::NormalizeMessage::Provisional(result))) = msg {
            let Some((fs_idx, file_path, was_playing)) = self
                .normalize_state
                .as_ref()
                .map(|state| (state.fs_idx, state.file_path.clone(), state.was_playing))
            else {
                return;
            };
            let still_valid = match self.fs_cache.get(&fs_idx) {
                Some(FsCacheEntry::Video { player, .. }) => player.path() == file_path.as_path(),
                _ => false,
            };
            if still_valid {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    let linear = 10.0_f64.powf(result.gain_db as f64 / 20.0);
                    apply_normalize_gain_with_perf(
                        player,
                        fs_idx,
                        linear,
                        result.gain_db,
                        "scan_provisional",
                    );
                    if was_playing {
                        player.set_playing(true);
                        player.set_audio_preroll_suspended(false);
                    }
                }
                self.normalize_ui_states.insert(
                    fs_idx,
                    NormalizeUiState::ProvisionalApplied {
                        gain_db: result.gain_db,
                    },
                );
                if let Some(state) = self.normalize_state.as_mut() {
                    if state.fs_idx == fs_idx && state.file_path == file_path {
                        state.provisional_applied = true;
                        state.provisional_result = Some(result);
                    }
                }
            }
            return;
        }
        // 2. 完了確定: state を所有してから後処理
        let Some(state) = self.normalize_state.take() else {
            return;
        };
        let target_milli = state.target_lufs_milli;
        // 3. stale fs_idx 復活防止
        let still_valid = match self.fs_cache.get(&state.fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.path() == state.file_path.as_path(),
            _ => false,
        };
        match msg {
            Some(Ok(crate::app::normalize::NormalizeMessage::Done(result))) => {
                // 測定値はファイル単位なので、stale でも DB に保存しておく (= 次回開いたとき即適用)
                if let Some(db) = self.audio_normalize_db.as_ref() {
                    let _ = db.upsert(&state.file_path, &result);
                }
                self.normalize_auto_scan_suppressed.remove(&state.fs_idx);
                if still_valid {
                    if let Some(FsCacheEntry::Video { player, .. }) =
                        self.fs_cache.get(&state.fs_idx)
                    {
                        let linear = 10.0_f64.powf(result.gain_db as f64 / 20.0);
                        apply_normalize_gain_with_perf(
                            player,
                            state.fs_idx,
                            linear,
                            result.gain_db,
                            "scan_done",
                        );
                        if state.was_playing {
                            player.set_playing(true);
                            player.set_audio_preroll_suspended(false);
                        }
                    }
                    self.normalize_ui_states.insert(
                        state.fs_idx,
                        NormalizeUiState::OnApplied {
                            gain_db: result.gain_db,
                        },
                    );
                }
                let _ = target_milli; // suppress unused warning
            }
            Some(Ok(crate::app::normalize::NormalizeMessage::Cancelled))
            | Some(Ok(crate::app::normalize::NormalizeMessage::Error(_)))
            | Some(Err(())) => {
                if let Some(Ok(crate::app::normalize::NormalizeMessage::Error(ref m))) = msg {
                    crate::logger::log(format!("normalize-scan error: {m}"));
                }
                self.normalize_auto_scan_suppressed.insert(state.fs_idx);
                // DB に書かない、グローバル ON は維持、UI 状態を OnUnmeasured に戻す
                if still_valid {
                    if let Some(provisional) = state.provisional_result {
                        self.normalize_ui_states.insert(
                            state.fs_idx,
                            NormalizeUiState::ProvisionalApplied {
                                gain_db: provisional.gain_db,
                            },
                        );
                    } else {
                        if let Some(FsCacheEntry::Video { player, .. }) =
                            self.fs_cache.get(&state.fs_idx)
                        {
                            if state.was_playing {
                                player.set_playing(true);
                                player.set_audio_preroll_suspended(false);
                            }
                        }
                        self.normalize_ui_states
                            .insert(state.fs_idx, NormalizeUiState::OnUnmeasured);
                    }
                }
            }
            Some(Ok(crate::app::normalize::NormalizeMessage::Provisional(_))) => {
                // Provisional は上で state を残したまま処理済み。ここには通常到達しない。
            }
            None => {
                // unreachable - try_recv が Empty なら return 済み、Disconnected なら Some(Err(()))
            }
        }
    }

    /// fs_idx 単位の normalize state を cleanup (close_fullscreen / fs_cache evict 時に呼ぶ)。
    #[cfg(windows)]
    pub(super) fn cleanup_normalize_state_for_fs_idx(&mut self, fs_idx: usize) {
        self.normalize_ui_states.remove(&fs_idx);
        self.normalize_auto_scan_suppressed.remove(&fs_idx);
        // 同 fs_idx のスキャン中なら state を持ち去って捨てる (= 新規スキャン即開始可能に)
        let should_drop = self
            .normalize_state
            .as_ref()
            .map(|s| s.fs_idx == fs_idx)
            .unwrap_or(false);
        if should_drop {
            if let Some(state) = self.normalize_state.take() {
                state.cancel();
            }
        }
    }

    /// 動画 open 時の自動適用。Settings ON + DB ヒットなら gain を即適用、ミスなら
    /// OnUnmeasured 表示。OFF なら Off 状態で初期化。
    #[cfg(windows)]
    pub(super) fn init_normalize_state_for_opened_video(&mut self, fs_idx: usize) {
        use crate::video::normalize_types::NormalizeUiState;
        self.normalize_auto_scan_suppressed.remove(&fs_idx);
        let path = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.path().to_path_buf(),
            _ => return,
        };
        let target_milli = self.settings.clamped_audio_normalize_target_lufs_milli();
        let ui_state = if self.settings.audio_normalize_enabled {
            let lookup = self
                .audio_normalize_db
                .as_ref()
                .and_then(|db| db.lookup(&path, target_milli));
            if let Some(result) = lookup {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    let linear = 10.0_f64.powf(result.gain_db as f64 / 20.0);
                    // 再生開始前なので flush 不要
                    player.set_normalize_gain(linear);
                }
                NormalizeUiState::OnApplied {
                    gain_db: result.gain_db,
                }
            } else {
                NormalizeUiState::OnUnmeasured
            }
        } else {
            NormalizeUiState::Off
        };
        self.normalize_ui_states.insert(fs_idx, ui_state);
    }

    /// native overlay にノーマライズ UI 状態 + 進捗 snapshot を配信する。
    /// `App::update` から毎フレーム呼ぶ。
    #[cfg(windows)]
    pub(super) fn sync_native_video_normalize_state(&self, fs_idx: usize) {
        use crate::video::normalize_types::{
            NormalizeOverlayState, NormalizeProgressSnapshot, NormalizeUiState,
        };
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
            return;
        };
        let ui_state = self
            .normalize_ui_states
            .get(&fs_idx)
            .copied()
            .unwrap_or(NormalizeUiState::Off);
        let progress = if matches!(ui_state, NormalizeUiState::Scanning) {
            self.normalize_state
                .as_ref()
                .filter(|s| s.fs_idx == fs_idx)
                .map(|s| NormalizeProgressSnapshot {
                    pts_processed_ms: s
                        .progress
                        .pts_processed_ms
                        .load(std::sync::atomic::Ordering::Acquire),
                    duration_ms: s
                        .progress
                        .duration_ms
                        .load(std::sync::atomic::Ordering::Acquire),
                    indeterminate: s
                        .progress
                        .indeterminate
                        .load(std::sync::atomic::Ordering::Acquire),
                })
        } else {
            None
        };
        player.set_native_normalize_state(NormalizeOverlayState { ui_state, progress });
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_set_volume_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        volume: f64,
        persist: bool,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        let volume = if volume.is_finite() {
            crate::settings::clamp_video_volume(volume)
        } else {
            return;
        };
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_volume(volume);
            self.settings.video_volume = volume;
            if persist {
                self.settings.save();
            }
            self.mark_native_video_hud_activity(ctx);
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_video_playback_speed_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        speed: f64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        let speed = crate::video::clock::clamp_playback_speed(speed);
        self.video_playback_speed = speed;
        if (self.settings.video_playback_speed - speed).abs() > 1.0e-9 {
            self.settings.video_playback_speed = speed;
            self.settings.save();
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_playback_speed(speed);
            self.mark_native_video_hud_activity(ctx);
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_request_seek_thumbnail(
        &mut self,
        fs_idx: usize,
        target_secs: f64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        let path = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. })
                if player.error().is_none()
                    && player.info().is_some()
                    && target_secs.is_finite() =>
            {
                let target_secs = target_secs.max(0.0);
                player.request_native_hover_thumbnail(target_secs);
                Some(player.path().clone())
            }
            _ => None,
        };
        let Some(path) = path else {
            return;
        };
        self.ensure_fullscreen_video_marker_cache(fs_idx);
        let pinned = self
            .fullscreen_video_marker_snapshot(fs_idx, &path)
            .0
            .is_some();
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_hover_preview_pinned(pinned);
        }
        self.sync_native_video_timeline_markers(fs_idx);
    }

    #[cfg(windows)]
    pub(crate) fn sync_native_video_timeline_markers(&mut self, fs_idx: usize) {
        // 音声ファイルと動画→音声モードは egui の音楽ビューが表示を所有し、native
        // presenter の jump panel / marker thumbnail cache は使わない。ここを呼び出し元
        // だけで gate すると、毎フレーム同期や mouse-move、bookmark 更新などの sibling
        // entry point から再侵入できるため、ownership boundary で一括して止める。
        //
        // `fullscreen_video_marker_path` も同じ述語で None を返すが、guard が無かった旧実装は
        // ensure で cache を破棄した後、player.path から fallback snapshot を毎フレーム構築した。
        // その結果、worker cache に残った marker frame を WebP 再 encode → SQLite UPDATE し続け、
        // bookmark 数に比例して UI thread を塞いでいた。VST host 表示中の動画→音声モードは
        // `fs_music_view_active` が false (= native presenter が表示所有者) なので従来どおり同期する。
        if self.fs_music_view_active(fs_idx) {
            return;
        }
        self.ensure_fullscreen_video_marker_cache(fs_idx);
        let (path, chapters) = {
            let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
                return;
            };
            if player.error().is_some() {
                return;
            };
            (
                player.path().clone(),
                player
                    .info()
                    .map(|info| info.chapters.clone())
                    .unwrap_or_default(),
            )
        };
        let mut markers: Vec<crate::video::native_presenter::NativeOverlayTimelineMarker> =
            Vec::new();
        let mut entries: Vec<JumpEntryWork> = Vec::new();
        let snapshot = self.fullscreen_video_marker_cached_snapshot(fs_idx, &path);

        struct JumpEntryWork {
            entry: crate::video::native_presenter::NativeOverlayJumpEntry,
            cached: Option<VideoMarkerCachedThumbnail>,
            refresh_cached: bool,
            save_target: MarkerThumbSaveTarget,
        }

        if let Some(pts_secs) = snapshot.pin_pts {
            markers.push(
                crate::video::native_presenter::NativeOverlayTimelineMarker {
                    pts_secs,
                    kind: crate::video::native_presenter::NativeOverlayTimelineMarkerKind::Pin,
                },
            );
            entries.push(JumpEntryWork {
                entry: crate::video::native_presenter::NativeOverlayJumpEntry {
                    pts_secs,
                    kind: crate::video::native_presenter::NativeOverlayTimelineMarkerKind::Pin,
                    title: Some("代表フレーム".to_string()),
                    bookmark_id: None,
                    thumbnail: None,
                },
                cached: snapshot.pin_thumbnail.clone(),
                refresh_cached: !snapshot.pin_thumb_current,
                save_target: MarkerThumbSaveTarget::Pin,
            });
        }
        for bookmark in snapshot.bookmarks {
            markers.push(
                crate::video::native_presenter::NativeOverlayTimelineMarker {
                    pts_secs: bookmark.pts_secs,
                    kind: crate::video::native_presenter::NativeOverlayTimelineMarkerKind::Bookmark,
                },
            );
            entries.push(JumpEntryWork {
                entry: crate::video::native_presenter::NativeOverlayJumpEntry {
                    pts_secs: bookmark.pts_secs,
                    kind: crate::video::native_presenter::NativeOverlayTimelineMarkerKind::Bookmark,
                    title: bookmark.title.clone(),
                    bookmark_id: Some(bookmark.id),
                    thumbnail: None,
                },
                cached: snapshot.bookmark_thumbnails.get(&bookmark.id).cloned(),
                refresh_cached: false,
                save_target: MarkerThumbSaveTarget::Bookmark { id: bookmark.id },
            });
        }
        for chapter in chapters {
            markers.push(
                crate::video::native_presenter::NativeOverlayTimelineMarker {
                    pts_secs: chapter.start_secs,
                    kind: crate::video::native_presenter::NativeOverlayTimelineMarkerKind::Chapter,
                },
            );
            let chapter_key = crate::video_chapter_thumbs::chapter_start_key(chapter.start_secs);
            entries.push(JumpEntryWork {
                entry: crate::video::native_presenter::NativeOverlayJumpEntry {
                    pts_secs: chapter.start_secs,
                    kind: crate::video::native_presenter::NativeOverlayTimelineMarkerKind::Chapter,
                    title: chapter.title.clone(),
                    bookmark_id: None,
                    thumbnail: None,
                },
                cached: snapshot.chapter_thumbnails.get(&chapter_key).cloned(),
                refresh_cached: false,
                save_target: MarkerThumbSaveTarget::Chapter,
            });
        }
        markers.retain(|marker| marker.pts_secs.is_finite() && marker.pts_secs >= 0.0);
        markers.sort_by(|a, b| {
            a.pts_secs
                .partial_cmp(&b.pts_secs)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        entries.retain(|work| work.entry.pts_secs.is_finite() && work.entry.pts_secs >= 0.0);
        entries.sort_by(|a, b| {
            a.entry
                .pts_secs
                .partial_cmp(&b.entry.pts_secs)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let mut pending_saves = Vec::new();
        let output_entries = {
            let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
                return;
            };
            let requested_thumb = std::cell::Cell::new(false);
            let mut make_thumbnail =
                |pts_secs: f64,
                 cached: Option<&VideoMarkerCachedThumbnail>,
                 refresh_cached: bool,
                 save_target: MarkerThumbSaveTarget|
                 -> Option<crate::video::native_presenter::NativeOverlayThumbnail> {
                    if refresh_cached || cached.is_none() {
                        if let Some(thumb) = player.nearest_seek_thumbnail(pts_secs) {
                            let native = crate::video::native_presenter::NativeOverlayThumbnail {
                                target_secs: thumb.target_secs,
                                width: thumb.width,
                                height: thumb.height,
                                rgba: thumb.rgba,
                            };
                            if let (Some(thumb_webp), Some(cached_thumb)) = (
                                encode_native_overlay_thumbnail_webp(&native),
                                VideoMarkerCachedThumbnail::from_native_overlay_thumbnail(
                                    pts_secs, &native,
                                ),
                            ) {
                                pending_saves.push(match save_target {
                                    MarkerThumbSaveTarget::Pin => MarkerThumbSave::Pin {
                                        pts_secs,
                                        thumb_webp,
                                        cached: cached_thumb,
                                    },
                                    MarkerThumbSaveTarget::Bookmark { id } => {
                                        MarkerThumbSave::Bookmark {
                                            id,
                                            thumb_webp,
                                            cached: cached_thumb,
                                        }
                                    }
                                    MarkerThumbSaveTarget::Chapter => MarkerThumbSave::Chapter {
                                        pts_secs,
                                        thumb_webp,
                                        cached: cached_thumb,
                                    },
                                });
                            }
                            return Some(native);
                        }
                        if !requested_thumb.get() {
                            requested_thumb.set(player.request_marker_thumbnail_warmup(pts_secs));
                        }
                    }
                    cached.map(VideoMarkerCachedThumbnail::to_native_overlay_thumbnail)
                };
            entries
                .into_iter()
                .map(|mut work| {
                    work.entry.thumbnail = make_thumbnail(
                        work.entry.pts_secs,
                        work.cached.as_ref(),
                        work.refresh_cached,
                        work.save_target,
                    );
                    work.entry
                })
                .collect::<Vec<_>>()
        };
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_timeline_markers(markers);
            player.set_native_jump_entries(output_entries);
        }
        for save in pending_saves {
            self.persist_native_video_marker_thumbnail(fs_idx, &path, save);
        }
    }

    #[cfg(windows)]
    fn persist_native_video_marker_thumbnail(
        &mut self,
        fs_idx: usize,
        path: &std::path::Path,
        save: MarkerThumbSave,
    ) {
        match save {
            MarkerThumbSave::Pin {
                pts_secs,
                thumb_webp,
                cached,
            } => {
                let Some(db) = self.video_pin_db.as_ref() else {
                    return;
                };
                // DB に同じ pin 位置の現行サムネが既にあるなら書き直さない (dirty も
                // 立てない)。マーカーキャッシュは App-global のため、メイン側の
                // フォルダ再ロード (close_fullscreen) で破棄→再構築されるたびに
                // pending_saves が同一 WebP をここへ再発行する。無条件に set_pin +
                // dirty すると「dirty → メイン再ロード → キャッシュ再構築 → また
                // dirty」の自己ループになり、~50ms 周期の再ロード連打で全動画サムネが
                // Pending (黒) に固着する (実機 2026-07-10、§1.7 メディア別窓 + P ピン)。
                // in-memory キャッシュ側の表示フィールドだけ同期して終了する。
                // 許容制限 (Sol P2): 照合はメタ (pts) のみで blob 内容は見ない。同一パスで
                // 動画ファイル自体を差し替えた場合など、旧フレームが DB に残り得るが、
                // byte 比較は抽出フレームの揺らぎで再ループするリスクがあるため採らない
                // (ピンを付け直せば pts が変わり回復する)。
                let db_thumb_current = db.lookup_meta(path).is_some_and(|meta| {
                    (meta.pin_pts_secs - pts_secs).abs() < 1e-3 && meta.thumb_is_current()
                });
                if db_thumb_current {
                    if let Some(cache) = self
                        .fullscreen_video_marker_cache
                        .as_mut()
                        .filter(|cache| cache.fs_idx == fs_idx && cache.path.as_path() == path)
                    {
                        cache.pin_pts = Some(pts_secs);
                        cache.pin_thumbnail = Some(cached);
                        cache.pin_thumb_current = true;
                    }
                    return;
                }
                match db.set_pin(path, pts_secs, &thumb_webp) {
                    Ok(()) => {
                        self.video_thumb_overrides_dirty_paths
                            .insert(path.to_path_buf());
                        if let Some(cache) = self
                            .fullscreen_video_marker_cache
                            .as_mut()
                            .filter(|cache| cache.fs_idx == fs_idx && cache.path.as_path() == path)
                        {
                            cache.pin_pts = Some(pts_secs);
                            cache.pin_thumbnail = Some(cached);
                            cache.pin_thumb_current = true;
                        }
                        crate::logger::log(format!(
                            "video marker thumb cached: pin pts={pts_secs:.2}s webp={}B {}",
                            thumb_webp.len(),
                            path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                        ));
                    }
                    Err(e) => {
                        crate::logger::log(format!("video marker pin thumb cache failed: {e}"))
                    }
                }
            }
            MarkerThumbSave::Bookmark {
                id,
                thumb_webp,
                cached,
            } => {
                let Some(db) = self.video_bookmark_db.as_ref() else {
                    return;
                };
                match db.update_thumb(id, &thumb_webp) {
                    Ok(()) => {
                        if let Some(cache) = self
                            .fullscreen_video_marker_cache
                            .as_mut()
                            .filter(|cache| cache.fs_idx == fs_idx && cache.path.as_path() == path)
                        {
                            cache.bookmark_thumbnails.insert(id, cached);
                        }
                        crate::logger::log(format!(
                            "video marker thumb cached: bookmark id={id} webp={}B {}",
                            thumb_webp.len(),
                            path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                        ));
                    }
                    Err(e) => {
                        crate::logger::log(format!("video marker bookmark thumb cache failed: {e}"))
                    }
                }
            }
            MarkerThumbSave::Chapter {
                pts_secs,
                thumb_webp,
                cached,
            } => {
                let Some(db) = self.video_chapter_thumb_db.as_ref() else {
                    return;
                };
                match db.set(path, pts_secs, &thumb_webp) {
                    Ok(()) => {
                        if let Some(cache) = self
                            .fullscreen_video_marker_cache
                            .as_mut()
                            .filter(|cache| cache.fs_idx == fs_idx && cache.path.as_path() == path)
                        {
                            cache.chapter_thumbnails.insert(
                                crate::video_chapter_thumbs::chapter_start_key(pts_secs),
                                cached,
                            );
                        }
                        crate::logger::log(format!(
                            "video marker thumb cached: chapter pts={pts_secs:.2}s webp={}B {}",
                            thumb_webp.len(),
                            path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                        ));
                    }
                    Err(e) => {
                        crate::logger::log(format!("video marker chapter thumb cache failed: {e}"))
                    }
                }
            }
        }
    }

    /// 動画 native overlay 用のタグ候補カタログを `NativeOverlayTagDef` の Arc として返す。
    /// 初回 (またはタグ変更後) のみ構築し、以降は Arc clone を返すだけ。
    #[cfg(windows)]
    fn cached_native_overlay_tag_choices(
        &mut self,
    ) -> std::sync::Arc<[crate::video::native_presenter::NativeOverlayTagDef]> {
        if let Some(cached) = self.native_overlay_tag_choices_cache.as_ref() {
            return cached.clone();
        }
        let built: std::sync::Arc<[_]> = self
            .cached_tag_choice_catalog()
            .into_iter()
            .map(|tag| crate::video::native_presenter::NativeOverlayTagDef {
                name: tag.name,
                tag_key: tag.tag_key,
                count: tag.count,
                pinned: tag.pinned,
                last_applied_at: tag.last_applied_at,
            })
            .collect();
        self.native_overlay_tag_choices_cache = Some(built.clone());
        built
    }

    /// 動画 native overlay 用のピン留めタグ (常時表示ボタン) を Arc として返す。
    /// `last_applied_at` はカタログから引く。タグ変更後のみ再構築する。
    #[cfg(windows)]
    fn cached_native_overlay_shortcut_tags(
        &mut self,
    ) -> std::sync::Arc<[crate::video::native_presenter::NativeOverlayTagDef]> {
        if let Some(cached) = self.native_overlay_shortcut_tags_cache.as_ref() {
            return cached.clone();
        }
        let catalog = self.cached_native_overlay_tag_choices();
        let built: std::sync::Arc<[_]> = self
            .settings
            .tags
            .iter()
            .filter(|tag| tag.show_shortcut)
            .map(|tag| crate::video::native_presenter::NativeOverlayTagDef {
                name: tag.name.clone(),
                tag_key: tag.tag_key.clone(),
                count: 0,
                pinned: true,
                last_applied_at: catalog
                    .iter()
                    .find(|choice| choice.tag_key == tag.tag_key)
                    .map(|choice| choice.last_applied_at)
                    .unwrap_or(0),
            })
            .collect();
        self.native_overlay_shortcut_tags_cache = Some(built.clone());
        built
    }

    #[cfg(windows)]
    fn native_overlay_shortcut_labels(
        &self,
    ) -> crate::video::native_presenter::NativeOverlayShortcutLabels {
        let chord_list = |action| {
            let labels = self.keymap.chord_labels(action);
            (!labels.is_empty()).then(|| labels.join(" / "))
        };
        crate::video::native_presenter::NativeOverlayShortcutLabels {
            play_pause: chord_list(KeyAction::VideoPlayPause),
            seek_start: self.keymap.first_chord_label(KeyAction::VideoSeekStart),
            volume_up: self.keymap.first_chord_label(KeyAction::VideoVolumeUp),
            volume_down: self.keymap.first_chord_label(KeyAction::VideoVolumeDown),
            next_file: self.keymap.first_chord_label(KeyAction::VideoNextFile),
            prev_file: self.keymap.first_chord_label(KeyAction::VideoPrevFile),
            mute: self.keymap.first_chord_label(KeyAction::VideoMute),
            loop_mode: self.keymap.first_chord_label(KeyAction::VideoLoop),
            marker_prev: self.keymap.first_chord_label(KeyAction::VideoMarkerPrev),
            marker_next: self.keymap.first_chord_label(KeyAction::VideoMarkerNext),
            pin: self.keymap.first_chord_label(KeyAction::VideoPin),
            perf_overlay: self.keymap.first_chord_label(KeyAction::VideoPerfOverlay),
            window_mode: self.keymap.first_chord_label(KeyAction::FsToggleWindowMode),
            tile_mode: self.keymap.first_chord_label(KeyAction::VideoTileMode),
            bookmark: self.keymap.first_chord_label(KeyAction::VideoBookmark),
            capture: self.keymap.first_chord_label(KeyAction::VideoCapture),
            toggle_audio_mode: self
                .keymap
                .first_chord_label(KeyAction::VideoToggleAudioMode),
        }
    }

    #[cfg(windows)]
    fn native_video_help_includes_row(row: &CommandDisplayRow) -> bool {
        match row.spec.action {
            KeyAction::ToggleDetachedViewerMode
            | KeyAction::FsToggleWindowMode
            | KeyAction::FsBackToList
            | KeyAction::FsCtrlNavPrev
            | KeyAction::FsCtrlNavNext
            | KeyAction::FsSiblingPrev
            | KeyAction::FsSiblingNext => true,
            action if action.is_location_navigation_action() => !row.shortcut_labels.is_empty(),
            KeyAction::VideoCompareToggle
            | KeyAction::VideoCompareCycle
            | KeyAction::VideoCompareWipe
            | KeyAction::VideoCompareDiff => false,
            _ => matches!(row.spec.scope, CommandScope::Rating | CommandScope::FsVideo),
        }
    }

    #[cfg(windows)]
    fn build_native_overlay_shortcut_help(
        &self,
    ) -> crate::video::native_presenter::NativeOverlayShortcutHelp {
        use crate::video::native_presenter::{
            NativeOverlayShortcutHelp, NativeOverlayShortcutHelpRow,
            NativeOverlayShortcutHelpSection,
        };

        const FIXED_ROWS: &[(&str, &str)] = &[
            ("?", "このショートカット一覧を表示する"),
            (
                "Esc",
                "一覧へ戻る。タイルモード中は先にタイルモードを閉じる",
            ),
            (
                "← / →",
                "5秒戻る / 進む。タイルモード中はタイルカーソルを移動する",
            ),
            ("マウスホイール", "前または次の項目へ移動する"),
            ("Ctrl+ホイール", "タイルモード中はタイル列数を変更する"),
        ];

        let rows = self
            .keymap
            .command_display_rows_for_active_scopes(FS_VIDEO_ACTIVE_SCOPES, false)
            .into_iter()
            .filter(Self::native_video_help_includes_row)
            .collect::<Vec<_>>();

        let sections = FS_VIDEO_ACTIVE_SCOPES
            .iter()
            .filter_map(|scope| {
                let section_rows = rows
                    .iter()
                    .filter(|row| row.spec.scope == *scope && !row.shortcut_labels.is_empty())
                    .map(|row| NativeOverlayShortcutHelpRow {
                        keys: row.shortcut_labels.join(" / "),
                        description: row.spec.description().to_string(),
                    })
                    .collect::<Vec<_>>();
                (!section_rows.is_empty()).then(|| NativeOverlayShortcutHelpSection {
                    title: scope.description().to_string(),
                    rows: section_rows,
                })
            })
            .collect();

        let fixed_rows = FIXED_ROWS
            .iter()
            .map(|(keys, description)| NativeOverlayShortcutHelpRow {
                keys: if *keys == "?" {
                    self.keymap.context_shortcuts_help_label()
                } else {
                    (*keys).to_string()
                },
                description: (*description).to_string(),
            })
            .collect();

        let touch_rows = [
            ("中央をタップ", "HUD を表示 / 非表示"),
            ("左をタップ", "5 秒戻る"),
            ("右をタップ", "5 秒進む"),
        ]
        .into_iter()
        .map(|(keys, description)| NativeOverlayShortcutHelpRow {
            keys: keys.to_string(),
            description: description.to_string(),
        })
        .collect();

        NativeOverlayShortcutHelp {
            sections,
            touch_rows,
            fixed_rows,
        }
    }

    #[cfg(windows)]
    fn cached_native_overlay_shortcut_help(
        &mut self,
    ) -> std::sync::Arc<crate::video::native_presenter::NativeOverlayShortcutHelp> {
        if let Some(cached) = self.native_overlay_shortcut_help_cache.as_ref() {
            return cached.clone();
        }
        let built = std::sync::Arc::new(self.build_native_overlay_shortcut_help());
        self.native_overlay_shortcut_help_cache = Some(built.clone());
        built
    }

    #[cfg(windows)]
    pub(super) fn sync_native_video_metadata(&mut self, fs_idx: usize) {
        let Some(path) = self.fs_cache.get(&fs_idx).and_then(|entry| match entry {
            FsCacheEntry::Video { player, .. } => Some(player.path().clone()),
            _ => None,
        }) else {
            return;
        };
        let item_key = crate::tags_db::item_key_for_path(&path);
        self.hydrate_tags_cache_for_paths(std::slice::from_ref(&path));
        let current_tags = self.tags_cache.get(&item_key).cloned().unwrap_or_default();
        // tag_choices / shortcut_tags は Arc キャッシュからの clone (refcount bump) だけ。
        // 中身はタグ変更時のみ再構築される (cached_native_overlay_* / invalidate_tag_apply_suggestions)。
        // これでフルスクリーン動画中の毎フレーム sync が大きなカタログを作り直さなくなる。
        let tag_choices = self.cached_native_overlay_tag_choices();
        let shortcut_tags = self.cached_native_overlay_shortcut_tags();
        let shortcuts = self.native_overlay_shortcut_labels();
        let shortcut_help = self.cached_native_overlay_shortcut_help();
        let side_panel_mode = self.settings.fullscreen_side_panel_mode;
        let info_panel_open = self.fs_info_panel_open;
        let touch_video_chrome_learned = self.settings.touch_video_chrome_learned;
        // ★ レーティング (右パネル先頭。get_rating は &mut self なので player 借用より前に取る)。
        let rating = self.get_rating(fs_idx);

        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
            return;
        };
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("video")
            .to_string();
        let metadata = if let Some(info) = player.info() {
            // 動的状態を Acquire load して snapshot 化 (= overlay は毎フレーム rebuild
            // されるので、ここで読んだ値が右パネルに反映される)。
            use std::sync::atomic::Ordering;
            let last_present_path = crate::video::decoder::PresentPathSnapshot::from_atomic(
                info.dynamic.present_path.load(Ordering::Acquire),
            );
            let deinterlace_status = crate::video::decoder::DeinterlaceStatusSnapshot::from_atomic(
                info.dynamic.deinterlace_status.load(Ordering::Acquire),
            );
            let interlace_detected = info.dynamic.interlace_detected.load(Ordering::Acquire);
            crate::video::native_presenter::NativeOverlayMetadata {
                item_key: item_key.clone(),
                file_name,
                title: info.title.clone(),
                artist: info.artist.clone(),
                original_url: info.original_url.clone(),
                description: info.description.clone(),
                probe_info_available: true,
                rating,
                current_tags,
                shortcut_tags,
                tag_choices,
                width: info.width,
                height: info.height,
                duration_secs: info.duration_secs,
                video_codec: info.video_codec.clone(),
                video_decoder: info.video_decoder.clone(),
                audio_codec: info.audio_codec.clone(),
                audio_bit_rate_bps: info.audio_bit_rate_bps,
                avg_fps: info.avg_fps,
                bit_rate_bps: info.bit_rate_bps,
                chapter_count: info.chapters.len(),
                hw_decode_active: info.hw_decode_active,
                gpu_path_active: info.gpu_path_active,
                d3d11va_supported: info.d3d11va_supported,
                deinterlace_mode: info.effective_deinterlace_mode,
                last_present_path,
                deinterlace_status,
                interlace_detected,
                touch_video_chrome_learned,
                shortcuts,
                shortcut_help: shortcut_help.clone(),
            }
        } else {
            crate::video::native_presenter::NativeOverlayMetadata {
                item_key,
                file_name,
                title: None,
                artist: None,
                original_url: None,
                description: None,
                probe_info_available: false,
                rating,
                current_tags,
                shortcut_tags,
                tag_choices,
                width: 0,
                height: 0,
                duration_secs: 0.0,
                video_codec: String::new(),
                video_decoder: String::new(),
                audio_codec: None,
                audio_bit_rate_bps: 0,
                avg_fps: 0.0,
                bit_rate_bps: 0,
                chapter_count: 0,
                hw_decode_active: false,
                gpu_path_active: false,
                d3d11va_supported: false,
                deinterlace_mode: self.settings.video_deinterlace,
                last_present_path: crate::video::decoder::PresentPathSnapshot::Pending,
                deinterlace_status: crate::video::decoder::DeinterlaceStatusSnapshot::Pending,
                interlace_detected: false,
                touch_video_chrome_learned,
                shortcuts,
                shortcut_help,
            }
        };
        player.set_native_metadata(Some(metadata));
        player.set_native_side_panel_state(side_panel_mode, info_panel_open);
    }

    #[cfg(windows)]
    pub(super) fn sync_native_video_vst3_available(&self, fs_idx: usize) {
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
            return;
        };
        // VST GUI/owner/HUD は OFF モードのメイン fullscreen だけで有効。
        // 複数ウィンドウモード / F12 detached では音声チェーンだけを維持し、UI は出さない。
        let vst3_ok = self.native_video_vst3_controls_available();
        player.set_native_vst3_available(vst3_ok);
        player.set_native_video_compact(
            vst3_ok && self.settings.vst3_gui_visible && self.settings.vst3_video_compact,
        );
    }

    #[cfg(windows)]
    pub(super) fn sync_native_video_vst3_panel(&self, fs_idx: usize) {
        let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) else {
            return;
        };
        // Availability と同じ境界にそろえる。detached / 複数ウィンドウ mode では
        // panel イベントだけ残る半端な状態を作らない。
        let panel = if !self.native_video_vst3_controls_available() {
            None
        } else {
            self.build_native_video_vst3_panel()
        };
        player.set_native_vst3_panel(panel);
    }

    #[cfg(windows)]
    pub(super) fn build_native_video_vst3_panel(
        &self,
    ) -> Option<crate::video::native_presenter::NativeOverlayVst3Panel> {
        if !self.settings.vst3_enabled || !self.show_vst3_manager {
            return None;
        }
        use crate::video::dsp::{DspState, SlotState};
        use crate::video::native_presenter::{
            NativeOverlayVst3ChainSlot, NativeOverlayVst3Panel, NativeOverlayVst3Slot,
            NativeOverlayVst3SlotState,
        };

        let bridge_state = self.dsp_bridge.state();
        let state_text = match bridge_state {
            DspState::Disabled => "disabled".to_string(),
            DspState::Enabled => "enabled".to_string(),
            DspState::Error(err) => format!("error: {err}"),
        };
        let disabled_reason = if bridge_state == DspState::Disabled {
            self.dsp_bridge.session_disabled_reason()
        } else {
            None
        };
        let sample_rate = self.dsp_bridge.sample_rate();
        let bridge_slots = self.dsp_bridge.slots();
        let display_count = bridge_slots.len().max(self.settings.vst3_plugins.len());
        let plugin_label = |path: &str| -> String {
            std::path::Path::new(path)
                .file_stem()
                .or_else(|| std::path::Path::new(path).file_name())
                .map(|name| name.to_string_lossy().to_string())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| "(unknown)".to_string())
        };
        let mut slots = Vec::with_capacity(display_count);
        for idx in 0..display_count {
            if let Some(slot) = bridge_slots.get(idx) {
                let state = match slot.state {
                    SlotState::Loading => NativeOverlayVst3SlotState::Loading,
                    SlotState::Loaded => NativeOverlayVst3SlotState::Loaded,
                    SlotState::Error => NativeOverlayVst3SlotState::Error,
                };
                let latency_ms = if sample_rate > 0 && slot.latency_samples > 0 {
                    Some(slot.latency_samples as f64 / sample_rate as f64 * 1000.0)
                } else {
                    None
                };
                slots.push(NativeOverlayVst3Slot {
                    idx,
                    path: slot.plugin_path.clone(),
                    name: slot
                        .plugin_name
                        .clone()
                        .unwrap_or_else(|| plugin_label(&slot.plugin_path)),
                    state,
                    bypass: slot.bypass,
                    gui_visible: slot.gui_visible,
                    latency_ms,
                    auto_bypassed_for_latency: slot.auto_bypassed_for_latency,
                    placeholder: false,
                });
            } else if let Some(entry) = self.settings.vst3_plugins.get(idx) {
                slots.push(NativeOverlayVst3Slot {
                    idx,
                    path: entry.path.clone(),
                    name: plugin_label(&entry.path),
                    state: NativeOverlayVst3SlotState::Placeholder,
                    bypass: entry.bypass,
                    gui_visible: !entry.user_hidden,
                    latency_ms: None,
                    auto_bypassed_for_latency: false,
                    placeholder: true,
                });
            }
        }

        let chain_slots = self
            .settings
            .vst3_chain_slots
            .slots
            .iter()
            .enumerate()
            .map(|(idx, slot)| NativeOverlayVst3ChainSlot {
                idx,
                key_label: crate::adjustment::slot_key_label(idx),
                name: slot.as_ref().map(|slot| slot.name.clone()),
                plugin_count: slot.as_ref().map(|slot| slot.plugins.len()).unwrap_or(0),
            })
            .collect();

        Some(NativeOverlayVst3Panel {
            visible: true,
            video_compact: self.settings.vst3_video_compact,
            panel_pos: self.settings.vst3_panel_pos,
            state_text,
            disabled_reason,
            slots,
            chain_slots,
        })
    }

    #[cfg(windows)]
    pub(super) fn poll_native_video_fast_swap(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.native_video_fast_swap_pending.as_ref() else {
            return;
        };
        if !Self::parked_source_swap_poll_owner_matches(
            pending.parked_live_window_id,
            self.native_video_parked_live_input_window_id,
        ) {
            return;
        }
        let target_idx = pending.target_idx;
        let target_path = pending.target_path.clone();
        let source_epoch = pending.source_epoch;
        let started_at = pending.started_at;
        let deadline = pending.deadline;
        if self.fullscreen_idx != Some(target_idx) {
            self.native_video_fast_swap_pending = None;
            return;
        }

        let now = std::time::Instant::now();
        #[derive(Clone, Copy)]
        enum SwapStatus {
            Ready,
            Pending,
            Timeout,
            Error,
            Missing,
        }
        let status = match self.fs_cache.get(&target_idx) {
            Some(FsCacheEntry::Video { player, .. }) if player.path() == &target_path => {
                if player.error().is_some() {
                    SwapStatus::Error
                } else if !player.native_presenter_pending() {
                    SwapStatus::Ready
                } else if now >= deadline {
                    SwapStatus::Timeout
                } else {
                    SwapStatus::Pending
                }
            }
            _ => SwapStatus::Missing,
        };
        match status {
            SwapStatus::Ready => {
                self.native_video_fast_swap_pending = None;
                crate::logger::log(format!(
                    "[native-video] fast video swap ready: idx={target_idx} epoch={source_epoch} elapsed_ms={:.1}",
                    started_at.elapsed().as_secs_f64() * 1000.0
                ));
                ctx.request_repaint();
            }
            SwapStatus::Pending => {
                ctx.request_repaint_after(std::time::Duration::from_millis(33));
            }
            SwapStatus::Timeout => {
                self.native_video_fast_swap_pending = None;
                crate::logger::log(format!(
                    "[native-video] fast video swap timeout: idx={target_idx} epoch={source_epoch} elapsed_ms={:.1}",
                    started_at.elapsed().as_secs_f64() * 1000.0
                ));
                ctx.request_repaint();
            }
            SwapStatus::Error | SwapStatus::Missing => {
                self.native_video_fast_swap_pending = None;
                crate::logger::log(format!(
                    "[native-video] fast video swap ended before ready: idx={target_idx} epoch={source_epoch} status={}",
                    match status {
                        SwapStatus::Error => "error",
                        SwapStatus::Missing => "missing",
                        _ => "unknown",
                    }
                ));
                ctx.request_repaint();
            }
        }
    }

    /// native 動画タイルが実際に描画される borderless presenter HWND のクライアント
    /// 領域サイズを egui points で返す。
    ///
    /// タイル枚数 (= `build_video_tile_state_for` の `max_rows` / `pick_interval`) は、
    /// 描画先と同じ画面サイズを基準に計算しないと「生成枚数 < 敷き詰められる枚数」に
    /// なって画面上部だけ埋まる。presenter はモニター全面を覆う別 HWND なので、
    /// `ctx.content_rect()` (= メイン egui ウィンドウ。別モニター / 別サイズになり得る)
    /// ではなくこの実サイズを使う。HWND 未確定 / 取得失敗時は `None`。
    #[cfg(windows)]
    fn native_video_overlay_size_points(&self, fs_idx: usize) -> Option<egui::Vec2> {
        use windows::Win32::Foundation::{HWND as Win32Hwnd, RECT as Win32Rect};
        use windows::Win32::UI::HiDpi::GetDpiForWindow;
        use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

        let hwnd = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.native_presenter_hwnd(),
            _ => 0,
        };
        if hwnd == 0 {
            return None;
        }
        let win = Win32Hwnd(hwnd as *mut _);
        let mut rc = Win32Rect::default();
        if unsafe { GetClientRect(win, &mut rc) }.is_err() {
            return None;
        }
        let w_px = (rc.right - rc.left).max(0) as f32;
        let h_px = (rc.bottom - rc.top).max(0) as f32;
        if w_px < 1.0 || h_px < 1.0 {
            return None;
        }
        // presenter overlay と同じ `(GetDpiForWindow / 96.0) * ui_scale` で points に戻す。
        let os_ppp = (unsafe { GetDpiForWindow(win) } as f32 / 96.0).max(0.5);
        let ppp = crate::video::native_presenter::effective_overlay_pixels_per_point(
            os_ppp,
            self.settings.ui_scale_factor,
        );
        Some(egui::vec2(w_px / ppp, h_px / ppp))
    }

    /// タイル状態構築用の画面サイズ。native presenter の実クライアントサイズを優先し、
    /// 取得できない場合のみメインウィンドウの content rect にフォールバックする。
    /// タイル生成系の呼び出し元 (native overlay 経路 / egui fullscreen 経路の両方) は
    /// すべてこの関数を経由すること。
    #[cfg(windows)]
    pub(crate) fn video_tile_layout_size(&self, fs_idx: usize, ctx: &egui::Context) -> egui::Vec2 {
        self.native_video_overlay_size_points(fs_idx)
            .unwrap_or_else(|| ctx.content_rect().size())
    }

    #[cfg(windows)]
    pub(super) fn poll_video_tile_swap(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.video_tile_swap_pending.as_ref() else {
            return;
        };
        if !Self::parked_source_swap_poll_owner_matches(
            pending.parked_live_window_id,
            self.native_video_parked_live_input_window_id,
        ) {
            return;
        }
        let target_idx = pending.target_idx;
        let target_path = pending.target_path.clone();
        let source_epoch = pending.source_epoch;
        let started_at = pending.started_at;
        let deadline = pending.deadline;
        if self.fullscreen_idx != Some(target_idx) {
            self.video_tile_swap_pending = None;
            return;
        }
        let now = std::time::Instant::now();
        enum SwapStatus {
            Ready,
            Pending,
            Timeout,
            Error,
            Missing,
        }
        let status = match self.fs_cache.get(&target_idx) {
            Some(FsCacheEntry::Video { player, .. }) if player.path() == &target_path => {
                if player.error().is_some() {
                    SwapStatus::Error
                } else if player.info().is_some() {
                    SwapStatus::Ready
                } else if now >= deadline {
                    SwapStatus::Timeout
                } else {
                    SwapStatus::Pending
                }
            }
            _ => SwapStatus::Missing,
        };
        match status {
            SwapStatus::Ready => {
                let screen = self.video_tile_layout_size(target_idx, ctx);
                self.video_tile_mode_active = true;
                self.video_tile_state = self.build_video_tile_state_for(target_idx, screen);
                self.video_tile_swap_pending = None;
                self.video_tile_reopen_pending = false;
                self.video_tile_reopen_deadline = None;
                self.sync_native_video_tile_overlay(ctx, target_idx);
                crate::logger::log(format!(
                    "[native-video] fast tile swap ready: idx={target_idx} epoch={source_epoch} elapsed_ms={:.1}",
                    started_at.elapsed().as_secs_f64() * 1000.0
                ));
                ctx.request_repaint();
            }
            SwapStatus::Pending => {
                self.set_native_video_tile_preparing_overlay(target_idx);
                ctx.request_repaint_after(std::time::Duration::from_millis(33));
            }
            SwapStatus::Timeout => {
                self.video_tile_swap_pending = None;
                self.video_tile_state = None;
                self.video_tile_mode_active = true;
                self.video_tile_reopen_pending = true;
                self.video_tile_reopen_deadline = Some(now + std::time::Duration::from_secs(3));
                self.set_native_video_tile_preparing_overlay(target_idx);
                crate::logger::log(format!(
                    "[native-video] fast tile swap timeout: idx={target_idx} epoch={source_epoch} elapsed_ms={:.1}",
                    started_at.elapsed().as_secs_f64() * 1000.0
                ));
                ctx.request_repaint();
            }
            SwapStatus::Error | SwapStatus::Missing => {
                let status_label = match status {
                    SwapStatus::Error => "error",
                    SwapStatus::Missing => "missing",
                    _ => "unknown",
                };
                let error_text = self
                    .fs_cache
                    .get(&target_idx)
                    .and_then(|entry| match entry {
                        FsCacheEntry::Video { player, .. } if player.path() == &target_path => {
                            player.error().map(str::to_owned)
                        }
                        _ => None,
                    });
                self.video_tile_swap_pending = None;
                self.video_tile_state = None;
                // A failed/temporarily missing video open should not implicitly leave S tile mode.
                // During rapid wheel navigation the target can fail to produce metadata for a few
                // frames (or be an unsupported video), but the next wheel tick should still use the
                // tile fast-swap path instead of falling back to normal fullscreen navigation.
                self.video_tile_mode_active = true;
                self.video_tile_reopen_pending = true;
                self.video_tile_reopen_deadline = Some(now + std::time::Duration::from_secs(3));
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&target_idx) {
                    player.set_native_tile_overlay(Some(
                        Self::native_video_tile_preparing_overlay_for_path(
                            &target_path,
                            Some(player.prep_progress().snapshot()),
                        ),
                    ));
                }
                crate::logger::log(format!(
                    "[native-video] fast tile swap ended before ready: idx={target_idx} epoch={source_epoch} status={status_label} keep_mode=true error={}",
                    error_text.unwrap_or_default()
                ));
                ctx.request_repaint();
            }
        }
    }

    #[cfg(windows)]
    pub(crate) fn cancel_stale_video_tile_reopen(&mut self, fs_idx: Option<usize>, reason: &str) {
        if self.video_tile_mode_active {
            return;
        }
        let had_pending = self.video_tile_reopen_pending || self.video_tile_swap_pending.is_some();
        self.video_tile_reopen_pending = false;
        self.video_tile_reopen_deadline = None;
        self.video_tile_swap_pending = None;
        if let Some(idx) = fs_idx {
            if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&idx) {
                player.set_native_tile_overlay(None);
            }
        }
        if had_pending {
            crate::logger::log(format!(
                "[video-tile] cancel stale reopen: reason={reason} fs_idx={fs_idx:?}"
            ));
        }
    }

    #[cfg(windows)]
    pub(crate) fn sync_native_video_tile_overlay(&mut self, ctx: &egui::Context, fs_idx: usize) {
        let current_path = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) if player.error().is_none() => {
                Some(player.path().clone())
            }
            _ => None,
        };
        let Some(current_path) = current_path else {
            return;
        };

        if (self.video_tile_mode_active || self.video_tile_reopen_pending)
            && self.video_tile_state.is_none()
        {
            let now = std::time::Instant::now();
            let deadline = *self
                .video_tile_reopen_deadline
                .get_or_insert_with(|| now + std::time::Duration::from_secs(3));
            if now >= deadline {
                self.video_tile_reopen_pending = false;
                self.video_tile_reopen_deadline = None;
            } else {
                crate::logger::log(format!(
                    "[video-tile] reopen pending: fs_idx={fs_idx} active={} remaining_ms={:.1}",
                    self.video_tile_mode_active,
                    deadline.saturating_duration_since(now).as_secs_f64() * 1000.0
                ));
                let screen = self.video_tile_layout_size(fs_idx, ctx);
                self.video_tile_state = self.build_video_tile_state_for(fs_idx, screen);
                if self.video_tile_state.is_some() {
                    self.video_tile_mode_active = true;
                    self.video_tile_reopen_pending = false;
                    self.video_tile_reopen_deadline = None;
                } else {
                    ctx.request_repaint_after(
                        deadline
                            .saturating_duration_since(now)
                            .min(std::time::Duration::from_millis(80)),
                    );
                }
            }
        }

        let swap_pending_for_current =
            self.video_tile_swap_pending
                .as_ref()
                .is_some_and(|pending| {
                    pending.target_idx == fs_idx && pending.target_path == current_path
                });
        let mut clear_state = false;
        let tile_overlay = if let Some(state) = self.video_tile_state.as_ref() {
            if state.video_path != current_path {
                if swap_pending_for_current {
                    let open_status = self.fs_cache.get(&fs_idx).and_then(|entry| match entry {
                        FsCacheEntry::Video { player, .. } => {
                            Some(player.prep_progress().snapshot())
                        }
                        _ => None,
                    });
                    Some(Self::native_video_tile_preparing_overlay_for_path(
                        &current_path,
                        open_status,
                    ))
                } else {
                    crate::logger::log(format!(
                        "[video-tile] stale state path mismatch: fs_idx={fs_idx} old={} current={} -> keep mode and rebuild",
                        state.video_path.display(),
                        current_path.display()
                    ));
                    clear_state = true;
                    self.video_tile_reopen_pending = true;
                    self.video_tile_reopen_deadline =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
                    Some(Self::native_video_tile_preparing_overlay_for_path(
                        &current_path,
                        self.fs_cache.get(&fs_idx).and_then(|entry| match entry {
                            FsCacheEntry::Video { player, .. } => {
                                Some(player.prep_progress().snapshot())
                            }
                            _ => None,
                        }),
                    ))
                }
            } else {
                let snapshot = state.worker.snapshot();
                let (progress_done, progress_total) = state.worker.progress();
                let finished = state.worker.is_finished();
                if !finished {
                    ctx.request_repaint_after(std::time::Duration::from_millis(80));
                }
                let tiles = snapshot
                    .into_iter()
                    .map(|slot| {
                        slot.map(|thumb| {
                            crate::video::native_presenter::NativeOverlayTileThumbnail {
                                target_secs: thumb.pts_secs,
                                width: thumb.width,
                                height: thumb.height,
                                rgba: thumb.rgba,
                            }
                        })
                    })
                    .collect();
                let fallback_file_name = state
                    .video_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("")
                    .to_string();
                Some(crate::video::native_presenter::NativeOverlayTileOverlay {
                    interval_secs: state.interval_secs,
                    timestamps: state.timestamps.clone(),
                    tile_w: state.tile_w,
                    tile_h: state.tile_h,
                    columns: state.columns,
                    progress_done,
                    progress_total,
                    finished,
                    tiles,
                    selected_idx: state
                        .timestamps
                        .len()
                        .checked_sub(1)
                        .map(|max| state.selected_idx.min(max)),
                    fallback_file_name,
                    video_open_status: None,
                })
            }
        } else if swap_pending_for_current {
            let target_path = self
                .video_tile_swap_pending
                .as_ref()
                .map(|pending| pending.target_path.clone())
                .unwrap_or_else(|| current_path.clone());
            let open_status = self.fs_cache.get(&fs_idx).and_then(|entry| match entry {
                FsCacheEntry::Video { player, .. } if player.path() == &target_path => {
                    Some(player.prep_progress().snapshot())
                }
                _ => None,
            });
            Some(Self::native_video_tile_preparing_overlay_for_path(
                &target_path,
                open_status,
            ))
        } else if self.video_tile_reopen_pending {
            Some(Self::native_video_tile_preparing_overlay_for_path(
                &current_path,
                self.fs_cache.get(&fs_idx).and_then(|entry| match entry {
                    FsCacheEntry::Video { player, .. } => Some(player.prep_progress().snapshot()),
                    _ => None,
                }),
            ))
        } else {
            None
        };

        if clear_state {
            self.video_tile_state = None;
            self.video_tile_swap_pending = None;
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_tile_overlay(tile_overlay);
        }
    }

    #[cfg(windows)]
    pub(super) fn native_video_tile_preparing_overlay_for_path(
        path: &std::path::Path,
        open_status: Option<crate::video::avio_progress::PreparingStatus>,
    ) -> crate::video::native_presenter::NativeOverlayTileOverlay {
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        if let Some(open_status) = open_status {
            crate::video::native_presenter::NativeOverlayTileOverlay::preparing_with_open_status(
                file_name,
                open_status,
            )
        } else {
            crate::video::native_presenter::NativeOverlayTileOverlay::preparing_with_filename(
                file_name,
            )
        }
    }

    #[cfg(windows)]
    pub(super) fn set_native_video_tile_preparing_overlay(&self, fs_idx: usize) {
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            let path = player.path().clone();
            let open_status = player.prep_progress().snapshot();
            player.set_native_tile_overlay(Some(
                Self::native_video_tile_preparing_overlay_for_path(&path, Some(open_status)),
            ));
        }
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_add_bookmark_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        target_secs: f64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        self.add_native_video_bookmark(fs_idx, Some(target_secs));
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(crate) fn handle_native_video_set_pin_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        target_secs: f64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        self.set_native_video_pin(fs_idx, target_secs);
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(crate) fn handle_native_video_set_tile_pin_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) -> bool {
        if self.fullscreen_idx != Some(fs_idx) {
            return false;
        }
        let Some((target_secs, webp)) = self.selected_native_video_tile_pin_payload(fs_idx) else {
            return false;
        };
        self.set_native_video_pin_with_webp(fs_idx, target_secs, webp);
        self.mark_native_video_hud_activity(ctx);
        true
    }

    /// P キー、リング、フルスクリーンメニューに共通する「画面上で選んでいる動画
    /// フレームをサムネイルに設定」の所有境界。タイル表示中だけは選択中タイルを使い、
    /// 通常再生中は現在の再生位置を使う。
    #[cfg(windows)]
    pub(crate) fn pin_current_native_video_frame_for_input(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) -> bool {
        if self.video_tile_mode_active {
            self.handle_native_video_set_tile_pin_command(ctx, fs_idx)
        } else {
            let target_secs = self
                .fs_video_player(fs_idx)
                .map(|player| player.position())
                .unwrap_or(0.0);
            self.handle_native_video_set_pin_command(ctx, fs_idx, target_secs);
            true
        }
    }

    #[cfg(windows)]
    pub(crate) fn begin_native_video_context_menu_dismiss_click(&mut self) {
        self.native_video_context_menu_dismiss_click_started_at = Some(std::time::Instant::now());
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_delete_bookmark_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        id: i64,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        if let Some(db) = self.video_bookmark_db.as_ref() {
            if let Err(e) = db.remove(id) {
                crate::logger::log(format!("video bookmark remove failed: {e}"));
            } else {
                self.refresh_fullscreen_video_marker_cache(fs_idx);
                self.sync_native_video_timeline_markers(fs_idx);
                // BM ループ中なら境界リストが変わったので loop_target を再計算
                self.apply_loop_mode_to_player(fs_idx);
                self.notify_bookmarks_changed();
            }
        }
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_delete_pin_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        let path = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => Some(player.path().clone()),
            _ => None,
        };
        if let (Some(path), Some(db)) = (path, self.video_pin_db.as_ref()) {
            match db.remove(&path) {
                Ok(()) => {
                    // P ピン直後の後追い WebP 補完 (`tick_pending_pin_thumb_refresh`) が
                    // 残っていると、解除後に set_pin が走ってピンが復活する (Codex P1)。
                    // 解除した path の pending はここで破棄する。
                    if self
                        .pending_pin_thumb_refresh
                        .as_ref()
                        .is_some_and(|p| p.path == path)
                    {
                        self.pending_pin_thumb_refresh = None;
                    }
                    self.video_thumb_overrides_dirty_paths.insert(path.clone());
                    if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                        player.set_native_hover_preview_pinned(false);
                    }
                    self.refresh_fullscreen_video_marker_cache(fs_idx);
                    self.sync_native_video_timeline_markers(fs_idx);
                    crate::logger::log(format!(
                        "video pin removed: {}",
                        path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                    ));
                }
                Err(e) => crate::logger::log(format!("video pin remove failed: {e}")),
            }
        }
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_bulk_add_bookmarks_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        entries: Vec<(f64, String)>,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        let snapshot = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                if player.error().is_some() || player.info().is_none() {
                    None
                } else {
                    Some((player.path().clone(), player.duration()))
                }
            }
            _ => None,
        };
        let Some((path, duration)) = snapshot else {
            return;
        };
        let Some(db) = self.video_bookmark_db.as_mut() else {
            return;
        };
        // 動画長を超える時刻は登録しない (= 不正なリスト保護)。duration が 0 (未確定)
        // のときは range check を skip し、`finite_video_target_secs` の clamp に任せる。
        let mut prepared: Vec<(f64, Option<String>)> = Vec::with_capacity(entries.len());
        let mut skipped_out_of_range = 0usize;
        for (pts_raw, title) in entries {
            if duration > 0.0 && pts_raw > duration + 0.5 {
                skipped_out_of_range += 1;
                continue;
            }
            let pts = finite_video_target_secs(pts_raw, duration);
            let title_opt = if title.trim().is_empty() {
                None
            } else {
                Some(title)
            };
            prepared.push((pts, title_opt));
        }
        // 1 トランザクションで bulk INSERT。autocommit のオーバーヘッドを削減して
        // 大量貼り付け時に UI スレッドが長時間ブロックしないようにする (Codex P2 #2)。
        let entries_ref: Vec<(f64, Option<&str>)> = prepared
            .iter()
            .map(|(pts, t)| (*pts, t.as_deref()))
            .collect();
        let (added, skipped_duplicates, errors) =
            match db.bulk_add_if_no_duplicate(&path, &entries_ref, 1.0) {
                Ok(summary) => (summary.added, summary.skipped_duplicates, summary.errors),
                Err(e) => {
                    crate::logger::log(format!("video bookmark bulk add failed: {e}"));
                    (0, 0, prepared.len())
                }
            };
        if added > 0 {
            self.refresh_fullscreen_video_marker_cache(fs_idx);
            self.sync_native_video_timeline_markers(fs_idx);
            self.apply_loop_mode_to_player(fs_idx);
            self.notify_bookmarks_changed();
        }
        let mut msg_parts = vec![format!("一括登録: {added} 件追加")];
        if skipped_duplicates > 0 {
            msg_parts.push(format!("重複 skip {skipped_duplicates} 件"));
        }
        if skipped_out_of_range > 0 {
            msg_parts.push(format!("範囲外 {skipped_out_of_range} 件"));
        }
        if errors > 0 {
            msg_parts.push(format!("エラー {errors} 件"));
        }
        let msg = msg_parts.join(" / ");
        crate::logger::log(format!(
            "video bookmark bulk add: {msg} ({})",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
        ));
        self.show_native_video_overlay_toast(msg, true);
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_export_bookmarks_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        seconds_only: bool,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        let path = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => Some(player.path().clone()),
            _ => None,
        };
        let Some(path) = path else { return };
        let Some(db) = self.video_bookmark_db.as_ref() else {
            return;
        };
        let entries = db.list_marker_meta(&path);
        let count = entries.len();
        if count == 0 {
            self.show_native_video_overlay_toast(
                "コピーするブックマークがありません".to_string(),
                true,
            );
            self.mark_native_video_hud_activity(ctx);
            return;
        }
        let text = crate::video_bookmarks_parser::format_chapter_lines(&entries, seconds_only);
        ctx.copy_text(text);
        let unit_label = if seconds_only {
            "秒単位"
        } else {
            "ミリ秒精度"
        };
        let msg = format!("{count} 件をクリップボードへコピーしました ({unit_label})");
        crate::logger::log(format!(
            "video bookmark export: {msg} ({})",
            path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
        ));
        self.show_native_video_overlay_toast(msg, true);
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_clear_all_bookmarks_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        let path = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => Some(player.path().clone()),
            _ => None,
        };
        let Some(path) = path else { return };
        let Some(db) = self.video_bookmark_db.as_ref() else {
            return;
        };
        match db.clear_for(&path) {
            Ok(()) => {
                self.refresh_fullscreen_video_marker_cache(fs_idx);
                self.sync_native_video_timeline_markers(fs_idx);
                self.apply_loop_mode_to_player(fs_idx);
                self.notify_bookmarks_changed();
                crate::logger::log(format!(
                    "video bookmark cleared for {}",
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                ));
                self.show_native_video_overlay_toast(
                    "この動画のブックマークをすべて削除しました".to_string(),
                    true,
                );
            }
            Err(e) => {
                crate::logger::log(format!("video bookmark clear failed: {e}"));
                self.show_native_video_overlay_toast(format!("ブックマーク削除に失敗: {e}"), true);
            }
        }
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_set_bookmark_title_command(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        id: i64,
        title: String,
    ) {
        if self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        if let Some(db) = self.video_bookmark_db.as_ref() {
            if let Err(e) = db.update_title(id, Some(&title)) {
                crate::logger::log(format!("video bookmark title update failed: {e}"));
            } else {
                self.refresh_fullscreen_video_marker_cache(fs_idx);
                self.sync_native_video_timeline_markers(fs_idx);
                // タイトル変更だけでも boundary 起点に変化はないが、念のため apply。
                self.apply_loop_mode_to_player(fs_idx);
            }
        }
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn add_native_video_bookmark(&mut self, fs_idx: usize, target_secs: Option<f64>) {
        let snapshot = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                if player.error().is_some() || player.info().is_none() {
                    None
                } else {
                    let pts = target_secs.unwrap_or_else(|| player.position());
                    Some((
                        player.path().clone(),
                        finite_video_target_secs(pts, player.duration()),
                    ))
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
                self.sync_native_video_timeline_markers(fs_idx);
                // BM ループ中なら新 bookmark を含めて loop_target を再計算
                self.apply_loop_mode_to_player(fs_idx);
                self.notify_bookmarks_changed();
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn set_native_video_pin(&mut self, fs_idx: usize, target_secs: f64) {
        self.set_native_video_pin_with_webp(fs_idx, target_secs, None);
    }

    #[cfg(windows)]
    fn set_native_video_pin_with_webp(
        &mut self,
        fs_idx: usize,
        target_secs: f64,
        preencoded_webp: Option<Vec<u8>>,
    ) {
        let snapshot = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                if player.error().is_some() || player.info().is_none() {
                    None
                } else {
                    let pts = finite_video_target_secs(target_secs, player.duration());
                    let thumb = if preencoded_webp.is_some() {
                        None
                    } else {
                        player.request_seek_thumbnail(pts);
                        player.nearest_seek_thumbnail(pts)
                    };
                    Some((player.path().clone(), pts, thumb))
                }
            }
            _ => None,
        };
        let Some((path, pts, thumb)) = snapshot else {
            return;
        };
        let Some(db) = self.video_pin_db.as_ref() else {
            crate::logger::log("video pin: DB not open".to_string());
            return;
        };
        let webp = preencoded_webp.unwrap_or_else(|| {
            thumb
                .as_ref()
                .map(|t| {
                    let encoder = webp::Encoder::from_rgba(&t.rgba, t.width, t.height);
                    encoder.encode(75.0).to_vec()
                })
                .unwrap_or_default()
        });
        let webp_len = webp.len();
        let webp_was_empty = webp.is_empty();
        match db.set_pin(&path, pts, &webp) {
            Ok(()) => {
                self.video_thumb_overrides_dirty_paths.insert(path.clone());
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    player.set_native_hover_preview_pinned(true);
                }
                self.refresh_fullscreen_video_marker_cache(fs_idx);
                self.sync_native_video_timeline_markers(fs_idx);
                crate::logger::log(format!(
                    "video pin set: pts={pts:.2}s webp={}B {}",
                    webp_len,
                    path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                ));
                // thumb worker のキャッシュに pts のフレームがまだ無いタイミングで
                // ピン留めされると webp が空になり、set_pin SQL の「空なら既存温存」
                // 保護で古いサムネが残ったまま新サムネに更新されない。後続フレームで
                // worker 完了をポーリングして DB を書き直すための pending を立てる。
                if webp_was_empty {
                    self.pending_pin_thumb_refresh = Some(PendingPinThumbRefresh {
                        fs_idx,
                        path,
                        pts,
                        started_at: std::time::Instant::now(),
                    });
                } else {
                    // 取れていれば過去の pending は不要 (同じ path 別 pts でも上書き済み)
                    self.pending_pin_thumb_refresh = None;
                }
            }
            Err(e) => crate::logger::log(format!("video pin set failed: {e}")),
        }
    }

    #[cfg(windows)]
    fn selected_native_video_tile_pin_payload(
        &self,
        fs_idx: usize,
    ) -> Option<(f64, Option<Vec<u8>>)> {
        if self.fullscreen_idx != Some(fs_idx) || !self.video_tile_mode_active {
            return None;
        }
        let state = self.video_tile_state.as_ref()?;
        let selected_idx = state
            .selected_idx
            .min(state.timestamps.len().checked_sub(1)?);
        let pts = state.timestamps.get(selected_idx).copied()?;
        let webp = state
            .worker
            .get(selected_idx)
            .as_ref()
            .map(
                |thumb| crate::video::native_presenter::NativeOverlayTileThumbnail {
                    target_secs: thumb.pts_secs,
                    width: thumb.width,
                    height: thumb.height,
                    rgba: std::sync::Arc::clone(&thumb.rgba),
                },
            )
            .as_ref()
            .and_then(encode_native_overlay_tile_thumbnail_webp);
        Some((pts, webp))
    }

    /// `tick_native_video_loop_boundary` 直後に呼ばれ、`set_native_video_pin` で
    /// 空サムネのまま set_pin された pin に対して、thumb worker が抽出を完了したら
    /// WebP に encode し直して DB に書き戻す。完了 or タイムアウトで pending を解放。
    #[cfg(windows)]
    pub(crate) fn tick_pending_pin_thumb_refresh(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pending_pin_thumb_refresh.as_ref() else {
            return;
        };
        // 10s タイムアウト (worker 開始から数百 ms 〜 数秒で取れるのが想定。長尺で
        // seek が遅い動画でも 10s あれば足りる)。
        if pending.started_at.elapsed() > std::time::Duration::from_secs(10) {
            crate::logger::log(format!(
                "video pin thumb refresh timed out: pts={:.2}s {}",
                pending.pts,
                pending
                    .path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("?")
            ));
            self.pending_pin_thumb_refresh = None;
            return;
        }
        // 動画切替やフルスクリーン解除で pending と現状が一致しなくなったら諦める。
        let player_path = match self.fs_cache.get(&pending.fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => Some(player.path().clone()),
            _ => None,
        };
        if self.fullscreen_idx != Some(pending.fs_idx)
            || player_path.as_ref() != Some(&pending.path)
        {
            self.pending_pin_thumb_refresh = None;
            return;
        }
        // worker キャッシュをポーリング。取れなければ再要求 (worker が次の pause で
        // request を drop していた場合の保険)。
        let thumb =
            if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&pending.fs_idx) {
                let t = player.nearest_seek_thumbnail(pending.pts);
                if t.is_none() {
                    player.request_seek_thumbnail(pending.pts);
                    // worker が動くまで次フレームで repaint させる
                    ctx.request_repaint_after(std::time::Duration::from_millis(80));
                }
                t
            } else {
                None
            };
        let Some(thumb) = thumb else {
            return;
        };
        let webp = {
            let encoder = webp::Encoder::from_rgba(&thumb.rgba, thumb.width, thumb.height);
            encoder.encode(75.0).to_vec()
        };
        if webp.is_empty() {
            // encode 失敗 — ループに任せて次フレーム再試行
            return;
        }
        let pts = pending.pts;
        let path = pending.path.clone();
        let fs_idx = pending.fs_idx;
        if let Some(db) = self.video_pin_db.as_ref() {
            match db.set_pin(&path, pts, &webp) {
                Ok(()) => {
                    self.video_thumb_overrides_dirty_paths.insert(path.clone());
                    self.refresh_fullscreen_video_marker_cache(fs_idx);
                    self.sync_native_video_timeline_markers(fs_idx);
                    crate::logger::log(format!(
                        "video pin thumb refreshed: pts={pts:.2}s webp={}B {}",
                        webp.len(),
                        path.file_name().and_then(|n| n.to_str()).unwrap_or("?")
                    ));
                }
                Err(e) => crate::logger::log(format!("video pin thumb refresh failed: {e}")),
            }
        }
        self.pending_pin_thumb_refresh = None;
    }

    #[cfg(windows)]
    pub(crate) fn handle_native_video_key_event(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        key: crate::video::native_window::NativeVideoKeyEvent,
    ) {
        // 音声 VST シェル (Inc 6 ②-3): Escape はモード離脱、他キーは動画専用 (tile / frame step 等)
        // が多く audio に不適なので無視する。再生操作は HUD のマウス操作で行う。
        if self
            .music_vst_shell
            .as_ref()
            .is_some_and(|s| s.fs_idx == fs_idx)
        {
            if key.virtual_key == 0x1B && !key.repeat && !key.shift && !key.ctrl && !key.alt {
                self.exit_music_vst_shell();
                self.request_native_video_hud_repaint(ctx);
            }
            return;
        }
        // 7e: 「動画→音声モード」の VST ホスト表示中は Escape / Z (音声モードトグル) で VST ホストを
        // 畳んで波形ビュー (音声モード) へ戻る。他キーは通常動画として処理する (presenter 前面で
        // native focus のため)。native presenter (プラグイン GUI 非フォーカス) にキーが来たときの
        // ゲート (music_vst_shell の Escape 離脱と対を成す)。
        if self.video_audio_vst_active_for(fs_idx) {
            let esc = key.virtual_key == 0x1B && !key.repeat && !key.shift && !key.ctrl && !key.alt;
            let toggle = !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoToggleAudioMode, &key);
            if esc || toggle {
                self.exit_video_audio_vst(ctx, fs_idx);
                self.request_native_video_hud_repaint(ctx);
                return;
            }
        }
        // 仮 gain 適用前のノーマライズスキャンはモーダル動作のため、ESC (cancel)
        // 以外のキー入力 (Enter で再生再開、S で tile mode、B でブックマーク等) を
        // 全て遮断する。ProvisionalApplied 後のバックグラウンド scan 中は通常操作を許す。
        if self.normalize_scan_is_modal_for_current_player(fs_idx)
            && !(key.virtual_key == 0x1B && !key.repeat && !key.shift && !key.ctrl && !key.alt)
        {
            return;
        }
        if self.viewer_session_is_detached()
            && !key.repeat
            && !key.shift
            && !key.ctrl
            && !key.alt
            && matches!(key.virtual_key, 0x0D | 0x1B)
        {
            self.handle_fullscreen_close_request_immediate();
            return;
        }
        let mut hud_activity = true;
        if let Some(rating_key) = self.keymap.native_video_rating_action(&key) {
            if rating_key.container {
                match self.set_current_folder_rating(rating_key.stars) {
                    Ok(true) => self.show_container_rating_toast(rating_key.stars),
                    Ok(false) => hud_activity = false,
                    Err(error) => {
                        self.report_rating_write_error(&error);
                        hud_activity = false;
                    }
                }
            } else {
                self.apply_native_video_rating_key(fs_idx, rating_key.stars);
            }
            if hud_activity {
                self.request_native_video_hud_repaint(ctx);
            }
            return;
        }
        if !key.repeat && !self.video_audio_mode_hides_native_presenter_for(fs_idx) {
            let slot_actions = [
                KeyAction::VideoAdjustSlot1,
                KeyAction::VideoAdjustSlot2,
                KeyAction::VideoAdjustSlot3,
                KeyAction::VideoAdjustSlot4,
                KeyAction::VideoAdjustSlot5,
                KeyAction::VideoAdjustSlot6,
                KeyAction::VideoAdjustSlot7,
                KeyAction::VideoAdjustSlot8,
                KeyAction::VideoAdjustSlot9,
                KeyAction::VideoAdjustSlot10,
            ];
            if let Some(slot_idx) = slot_actions
                .iter()
                .position(|action| self.keymap.matches_vk_action(*action, &key))
            {
                self.load_video_adjust_slot(slot_idx);
                self.request_native_video_hud_repaint(ctx);
                return;
            }
        }
        let side_panel_key_owned_by_native = crate::ui_helpers::fs_side_panel_key_owner(
            matches!(self.items.get(fs_idx), Some(GridItem::Video(_))),
            self.fs_music_view_active(fs_idx),
        )
            == crate::ui_helpers::FsSidePanelKeyOwner::NativeVideo;
        if side_panel_key_owned_by_native
            && !key.repeat
            && self
                .keymap
                .matches_vk_action(KeyAction::FsToggleMetadata, &key)
        {
            self.cycle_fs_side_panel_mode();
            let label = self.settings.fullscreen_side_panel_mode.label();
            self.show_native_video_overlay_toast(format!("パネル表示: {label}"), false);
            self.sync_native_video_metadata(fs_idx);
            self.request_native_video_hud_repaint(ctx);
            return;
        }
        match key.virtual_key {
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoCloseFullscreen, &key) =>
            {
                self.handle_fullscreen_close_request_immediate();
                hud_activity = false;
            }
            // Shift+Enter: open in external player, matching the legacy egui
            // fullscreen video path.
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoExternalPlayer, &key) =>
            {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    crate::ui_helpers::open_external_player(player.path());
                }
            }
            // Enter in tile mode: start playback from the keyboard cursor.
            _ if !key.repeat
                && self.video_tile_mode_active
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoPlayPause, &key) =>
            {
                self.play_selected_video_tile(ctx, fs_idx);
            }
            // Enter: play / pause.
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoPlayPause, &key) =>
            {
                self.handle_native_video_toggle_play_command(ctx, fs_idx);
            }
            // Escape: close native fullscreen. If the native overlay has a text
            // editor focused this key is not forwarded here, so dialog editing
            // does not accidentally close the fullscreen window.
            // 仮 gain 適用前のモーダルスキャン中だけ ESC を cancel に優先ルーティング。
            // ProvisionalApplied 後は通常の ESC として tile/fullscreen close に使う。
            0x1B if !key.repeat && !key.shift && !key.ctrl && !key.alt => {
                if self.normalize_scan_is_modal_for_current_player(fs_idx) {
                    self.handle_cancel_normalize_scan(ctx, fs_idx);
                } else if self.close_video_tile_mode() {
                    self.sync_native_video_tile_overlay(ctx, fs_idx);
                } else {
                    self.handle_fullscreen_close_request_immediate();
                }
            }
            _ if !key.repeat && self.keymap.matches_vk_action(KeyAction::FsBackToList, &key) => {
                self.close_fullscreen();
                hud_activity = false;
            }
            // W: seek to start and play.
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoSeekStart, &key) =>
            {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    // `seek(0.0)` 自体が `apply_command(Play)` 経由で autoplay intent
                    // を立てるので、追加 `toggle_play()` は不要 (Codex P2-1 2026-05-17)。
                    player.seek(0.0);
                }
                self.maybe_start_normalize_scan_for_play_intent(fs_idx);
            }
            // F12: detached viewer mode toggle. Keep this as a keymap action
            // so a future remap works when the native video HWND has focus.
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::ToggleDetachedViewerMode, &key) =>
            {
                // placement 切替 (Plan B) で presenter を作り直すと、既に離された F12 の
                // KeyDown が数百 ms 遅れて再配送され、直前のトグルを打ち消す
                // (detached→main→detached、= main への一瞬フラッシュ + 再分離)。`repeat` は
                // stale でも false なので信用せず、OS に「今まだ F12 が押されているか」を
                // 問い合わせて、離された後の stale 再配送を弾く。実押下は KeyDown 到着時点で
                // まだ物理的に down なので通る (Codex 助言)。
                if !native_video_key_physically_down(&key) {
                    crate::logger::log(format!(
                        "[native-video] ignore stale native F12 toggle: os_down=false \
                         presentation={:?} (rebuild re-delivery of a released key)",
                        self.viewer_presentation
                    ));
                    return;
                }
                self.toggle_detached_viewer_mode();
                hud_activity = false;
            }
            // F11: ウィンドウ / 全画面 切り替え (HUD トグルボタンと同じ動作)。
            // toggle_video_window_mode は presenter rebuild を伴うので
            // toggle_still_window_mode (設定 flip だけ) では代用できない。
            // normalize scan 中は上の `normalize_state` ガードで既に弾かれている。
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::FsToggleWindowMode, &key) =>
            {
                self.toggle_video_window_mode_for_input(ctx);
                hud_activity = false;
            }
            // Tile mode: left/right move the keyboard cursor instead of seeking
            // behind the opaque tile grid. Ctrl moves by one visible row.
            0x25 | 0x27 if self.video_tile_mode_active && !key.alt => {
                self.handle_video_tile_cursor_key(ctx, fs_idx, key.ctrl, key.virtual_key == 0x25);
            }
            // Ctrl+Shift+Left / Right by default: frame step and pause.
            _ if self
                .keymap
                .matches_vk_action(KeyAction::VideoFrameStepBack, &key) =>
            {
                self.step_video_frame(ctx, fs_idx, -1);
            }
            _ if self
                .keymap
                .matches_vk_action(KeyAction::VideoFrameStepForward, &key) =>
            {
                self.step_video_frame(ctx, fs_idx, 1);
            }
            _ if self
                .keymap
                .matches_vk_action(KeyAction::VideoSeekBackSmall, &key) =>
            {
                self.native_video_seek_relative_with_hint(fs_idx, -1.0);
            }
            _ if self
                .keymap
                .matches_vk_action(KeyAction::VideoSeekForwardSmall, &key) =>
            {
                self.native_video_seek_relative_with_hint(fs_idx, 1.0);
            }
            _ if self
                .keymap
                .matches_vk_action(KeyAction::VideoSeekBackLarge, &key) =>
            {
                self.native_video_seek_relative_with_hint(fs_idx, -30.0);
            }
            _ if self
                .keymap
                .matches_vk_action(KeyAction::VideoSeekForwardLarge, &key) =>
            {
                self.native_video_seek_relative_with_hint(fs_idx, 30.0);
            }
            // Left / Right: same seek granularity as the egui fullscreen path.
            0x25 if !key.ctrl && !key.shift && !key.alt => {
                self.native_video_seek_relative_with_hint(fs_idx, -5.0);
            }
            0x27 if !key.ctrl && !key.shift && !key.alt => {
                self.native_video_seek_relative_with_hint(fs_idx, 5.0);
            }
            // Plain Up / Down: navigate files, matching the egui fullscreen path.
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::FsCtrlNavPrev, &key) =>
            {
                crate::logger::log(format!(
                    "[input-nav] source=native-video-key action=ctrl_nav_back fs_idx={fs_idx} keymap=FsCtrlNavPrev"
                ));
                self.handle_fullscreen_ctrl_nav_context(ctx, fs_idx, false, true);
            }
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::FsCtrlNavNext, &key) =>
            {
                crate::logger::log(format!(
                    "[input-nav] source=native-video-key action=ctrl_nav_forward fs_idx={fs_idx} keymap=FsCtrlNavNext"
                ));
                self.handle_fullscreen_ctrl_nav_context(ctx, fs_idx, true, true);
            }
            // Ctrl+PageUp / PageDown: move to the previous / next sibling folder.
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::FsSiblingPrev, &key) =>
            {
                self.handle_fullscreen_sibling_nav_context(ctx, fs_idx, false, true);
            }
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::FsSiblingNext, &key) =>
            {
                self.handle_fullscreen_sibling_nav_context(ctx, fs_idx, true, true);
            }
            // VK_BROWSER_BACK / VK_BROWSER_FORWARD: マウス進む/戻るボタンが Browser_Back/Forward
            // keystroke として届くケース (mouse driver や AutoHotkey が変換する経路)、または
            // 上で WM_APPCOMMAND を合成 KeyDown に変換した経路。マウス戻る/進む設定を通す。
            0xA6 => {
                self.mouse_ring_nav = self.apply_mouse_back_forward_button(
                    ctx,
                    false,
                    crate::app::ActionSurface::Viewer,
                    "native-video-key",
                );
            }
            0xA7 => {
                self.mouse_ring_nav = self.apply_mouse_back_forward_button(
                    ctx,
                    true,
                    crate::app::ActionSurface::Viewer,
                    "native-video-key",
                );
            }
            _ if self
                .keymap
                .matches_vk_action(KeyAction::VideoPrevFile, &key) =>
            {
                self.navigate_native_video_fullscreen(ctx, fs_idx, -1);
            }
            _ if self
                .keymap
                .matches_vk_action(KeyAction::VideoNextFile, &key) =>
            {
                self.navigate_native_video_fullscreen(ctx, fs_idx, 1);
            }
            // Home / End (keymap: FsJumpFirst/FsJumpLast): jump to the first / last visible navigable item.
            // Home: 先頭アイテムへ。既に先頭なら境界トーストを出す
            // (Phase 1: 画像と挙動を揃える、Codex 第 1 ラウンド P2 反映)。
            _ if !key.repeat && self.keymap.matches_vk_action(KeyAction::FsJumpFirst, &key) => {
                let display_order = self.current_grid_order().to_vec();
                let target =
                    crate::ui_helpers::boundary_navigable_idx(&self.items, &display_order, false);
                match target {
                    Some(idx) if idx != fs_idx => {
                        self.open_native_video_fullscreen_from_navigation(
                            ctx,
                            idx,
                            crate::app::HistoryTrigger::UserChosen,
                        );
                    }
                    _ => {
                        self.show_native_video_boundary_toast(ctx, false);
                    }
                }
            }
            // End: 末尾アイテムへ。既に末尾なら境界トーストを出す。
            _ if !key.repeat && self.keymap.matches_vk_action(KeyAction::FsJumpLast, &key) => {
                let display_order = self.current_grid_order().to_vec();
                let target =
                    crate::ui_helpers::boundary_navigable_idx(&self.items, &display_order, true);
                match target {
                    Some(idx) if idx != fs_idx => {
                        self.open_native_video_fullscreen_from_navigation(
                            ctx,
                            idx,
                            crate::app::HistoryTrigger::UserChosen,
                        );
                    }
                    _ => {
                        self.show_native_video_boundary_toast(ctx, true);
                    }
                }
            }
            // Shift+Up / Shift+Down: volume. Plain Up/Down remains for the
            // future full overlay phase as well, but the native HWND can already
            // perform the same item navigation without involving egui input.
            _ if self
                .keymap
                .matches_vk_action(KeyAction::VideoVolumeUp, &key) =>
            {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    let v =
                        crate::settings::step_video_volume_by_fader_key_step(player.volume(), 1);
                    player.set_volume(v);
                    self.settings.video_volume = v;
                    self.settings.save();
                }
            }
            _ if self
                .keymap
                .matches_vk_action(KeyAction::VideoVolumeDown, &key) =>
            {
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    let v =
                        crate::settings::step_video_volume_by_fader_key_step(player.volume(), -1);
                    player.set_volume(v);
                    self.settings.video_volume = v;
                    self.settings.save();
                }
            }
            // M: mute
            _ if !key.repeat && self.keymap.matches_vk_action(KeyAction::VideoMute, &key) => {
                if self.toggle_video_session_mute_for_fs_idx(fs_idx) {
                    self.request_native_video_hud_repaint(ctx);
                }
            }
            // L: loop (4 段階サイクル: Off → Full → Chapter → Bookmark)
            _ if !key.repeat && self.keymap.matches_vk_action(KeyAction::VideoLoop, &key) => {
                self.cycle_native_video_loop_common(ctx, fs_idx);
            }
            // J / K: previous / next chapter, bookmark, or pin marker.
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoMarkerPrev, &key) =>
            {
                self.jump_native_video_marker(fs_idx, false);
            }
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoMarkerNext, &key) =>
            {
                self.jump_native_video_marker(fs_idx, true);
            }
            // Space in tile mode: start playback from the keyboard cursor (= Enter と同じ).
            // 動画 HUD 2 段化リデザイン (Phase 1): Space を動画プレイヤー慣習に合わせて
            // 再生/停止トグルに変更。tile mode では選択タイル再生 (= Enter と同じ tile-aware 挙動)。
            // 旧 Space = チェックトグルは削除 (チェックしたい場合は Esc → 一覧 Space)。
            _ if !key.repeat
                && self.video_tile_mode_active
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoPlayPause, &key) =>
            {
                self.play_selected_video_tile(ctx, fs_idx);
            }
            // Space: play / pause (= Enter と等価)。
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoPlayPause, &key) =>
            {
                self.handle_native_video_toggle_play_command(ctx, fs_idx);
            }
            // P: pin the selected tile while tile mode is open; otherwise pin the current
            // frame (= HUD 📌 button). グリッドの P と統一した
            // 「P = Pin」の mnemonic。v0.9.x で perf overlay の P から再割り当て、
            // perf overlay は F に移動した。
            _ if !key.repeat && self.keymap.matches_vk_action(KeyAction::VideoPin, &key) => {
                if !self.pin_current_native_video_frame_for_input(ctx, fs_idx) {
                    // If tile metadata is not ready or contains no timestamps, tile-mode P is
                    // an intentional no-op; falling back to playback position would pin a
                    // different frame than the highlighted tile UI suggests.
                    hud_activity = false;
                }
            }
            // F: perf / framerate overlay toggle (旧 P)。Frames / FPS mnemonic。
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoPerfOverlay, &key) =>
            {
                self.video_perf_overlay_visible = !self.video_perf_overlay_visible;
                if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                    player.set_native_perf_overlay_visible(self.video_perf_overlay_visible);
                }
            }
            // Ctrl+S: save current video frame to the capture folder.
            _ if !key.repeat && self.keymap.matches_vk_action(KeyAction::VideoCapture, &key) => {
                self.save_video_frame_to_file(ctx, fs_idx);
            }
            // Ctrl+B: add current video frame to the active compiled book.
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoAddToActiveBook, &key) =>
            {
                self.add_current_video_frame_to_active_book(ctx, fs_idx);
            }
            // X / C: comparison view is static-image only. Consume as silent no-op.
            _ if !key.repeat
                && (self
                    .keymap
                    .matches_vk_action(KeyAction::VideoCompareToggle, &key)
                    || self
                        .keymap
                        .matches_vk_action(KeyAction::VideoCompareCycle, &key)
                    || self
                        .keymap
                        .matches_vk_action(KeyAction::VideoCompareWipe, &key)
                    || self
                        .keymap
                        .matches_vk_action(KeyAction::VideoCompareDiff, &key)) =>
            {
                hud_activity = false;
            }
            // S: tile mode toggle.
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoTileMode, &key) =>
            {
                let screen = self.video_tile_layout_size(fs_idx, ctx);
                self.toggle_video_tile_mode(fs_idx, screen);
                self.sync_native_video_tile_overlay(ctx, fs_idx);
            }
            // B: add video bookmark.
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoBookmark, &key) =>
            {
                self.add_native_video_bookmark(fs_idx, None);
            }
            // Z (default): 動画→音声モードへ切り替え (映像を消して音楽ビューで聴く、Inc 7)。
            // enter_video_audio_mode 内で音声トラック無し / detached / switch 中などは弾かれる。
            _ if !key.repeat
                && self
                    .keymap
                    .matches_vk_action(KeyAction::VideoToggleAudioMode, &key) =>
            {
                self.enter_video_audio_mode(ctx, fs_idx);
            }
            _ => {
                hud_activity = false;
            }
        }
        if hud_activity {
            self.request_native_video_hud_repaint(ctx);
        }
    }

    #[cfg(windows)]
    fn apply_native_video_rating_key(&mut self, fs_idx: usize, stars: u8) {
        let before = self.rating_cache.get(&fs_idx).copied().unwrap_or(0);
        if !self.set_rating(fs_idx, stars) {
            return;
        }
        if before != stars {
            let summary = if stars == 0 {
                "★解除".to_string()
            } else {
                format!("★{stars}")
            };
            self.capture_rating_undo(vec![(fs_idx, before, stars)], summary);
        }
        self.rebuild_visible_indices();
        if stars == 0 {
            self.show_feedback_toast("[★解除]".to_string());
        } else {
            self.show_feedback_toast(format!("[{}]", "★".repeat(stars as usize)));
        }
    }

    #[cfg(windows)]
    pub(super) fn jump_native_video_marker(&mut self, fs_idx: usize, next: bool) {
        const NAV_MARKER_EPSILON: f64 = 0.5;
        // 「すでに先頭」と判定する閾値。マーカースキップ用 NAV_MARKER_EPSILON (0.5s) とは別に
        // ここを小さく取ることで、現在位置 0.1s〜0.5s で J を押しても先頭へジャンプできる。
        const ALREADY_AT_START_TOL: f64 = 0.05;
        let markers = self.collect_video_nav_markers(fs_idx);
        let current = self
            .fs_video_player(fs_idx)
            .map(|p| p.position())
            .unwrap_or(0.0);
        let target = if next {
            markers
                .iter()
                .find(|marker| marker.pts > current + NAV_MARKER_EPSILON)
                .cloned()
        } else {
            markers
                .iter()
                .rev()
                .find(|marker| marker.pts < current - NAV_MARKER_EPSILON)
                .cloned()
        };
        match target {
            Some(marker) => {
                if let Some(player) = self.fs_video_player(fs_idx) {
                    player.seek(marker.pts);
                }
                // CH/BM ループ中ならマーカージャンプ後に loop_target を更新
                self.apply_loop_mode_to_player(fs_idx);
                self.maybe_start_normalize_scan_for_play_intent(fs_idx);
                let direction = if next { "次の" } else { "前の" };
                let kind_label = match marker.kind {
                    crate::ui_fullscreen::NavMarkerKind::Chapter => "チャプター",
                    crate::ui_fullscreen::NavMarkerKind::Bookmark => "ブックマーク",
                    crate::ui_fullscreen::NavMarkerKind::Pin => "ピン",
                };
                let toast = match (marker.kind, marker.title.as_deref()) {
                    (crate::ui_fullscreen::NavMarkerKind::Chapter, Some(title))
                    | (crate::ui_fullscreen::NavMarkerKind::Bookmark, Some(title))
                        if !title.is_empty() =>
                    {
                        format!(
                            "{} {}{}: {}",
                            crate::ui_helpers::format_hms(marker.pts),
                            direction,
                            kind_label,
                            title
                        )
                    }
                    _ => format!(
                        "{} {}{}",
                        crate::ui_helpers::format_hms(marker.pts),
                        direction,
                        kind_label
                    ),
                };
                self.show_feedback_toast(toast);
            }
            None if !next && current > ALREADY_AT_START_TOL => {
                // J キーで前のマーカーが見つからない (= 最初のマーカー手前または空) かつ
                // 既に先頭に居なければ動画先頭へ seek。
                if let Some(player) = self.fs_video_player(fs_idx) {
                    player.seek(0.0);
                }
                self.apply_loop_mode_to_player(fs_idx);
                self.maybe_start_normalize_scan_for_play_intent(fs_idx);
                // native presenter 経路では overlay 上にトーストを出す (= HUD と整合)
                self.show_native_video_overlay_toast(
                    format!("{} 動画先頭", crate::ui_helpers::format_hms(0.0)),
                    false,
                );
            }
            None => {
                // K キーでマーカーが無い (= 末尾以降) ケース、
                // または J キーで既に先頭にいるケースは何もしない。
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn toggle_native_video_vst3_gui(&mut self) {
        let opening = !self.show_vst3_manager;
        self.show_vst3_manager = opening;
        if let Some(hwnd) = self.native_video_presenter_hwnd() {
            self.dsp_bridge.set_existing_guis_owner_to_hwnd(hwnd);
            self.native_video_owner_synced_hwnd = hwnd;
        }
        // 実機修正 (2026-05-13 仕様修正): user_hidden は **clear しない**。
        //
        // ## 背景
        // 2026-05-12 に「VST ボタン無反応」の修正として settings + runtime 双方の
        // `user_hidden` を clear する実装を入れたが、ユーザー報告:
        // 「設定に関係なくすべての VST プラグインが表示される。前回表示していたもののみが
        //  表示される仕様」 — つまり個別 × で閉じた slot は VST ボタン全表示でも skip される
        // のが正しい仕様。
        //
        // ## 当時の bug の真因 (推定)
        // 原 bug 「VST ボタン押しても何も出ない」は user_hidden ではなく、cross-process
        // `SetWindowPos(clamp)` で bridge が GUI スレッドタイムアウト → 自殺 →
        // editor HWND が全部 stale、という別の問題が原因だった可能性が高い。
        // clamp 削除後は bridge が安定するので、この user_hidden clear は不要になった。
        //
        // ## 個別表示再開の経路
        // 個別に hide した slot を再表示したい場合は、VST3 設定パネルのチェーンスロット
        // 行から GUI ボタンを押すことで個別に再表示できる (`show_native_video_vst3_slot_gui`
        // が `user_hidden=false` を clear する)。
        self.dsp_bridge.set_all_guis_topmost(opening);
        std::sync::Arc::clone(&self.dsp_bridge).set_all_guis_visible_async(opening);
        // T23 (Claude R3-11): bridge が `Disabled` / `Error` のときは settings 永続化を
        // スキップする。bridge 死亡時に opening=true で save すると次回起動時に「VST3 GUI
        // 表示」設定が残り、bridge 再起動時に予期せず全 GUI が一斉に開く違和感がある。
        // 「ユーザーが今操作した結果が表に出ていない」ときは settings を変えない方針。
        if matches!(
            self.dsp_bridge.state(),
            crate::video::dsp::DspState::Enabled
        ) {
            self.settings.vst3_gui_visible = opening;
            self.settings.save();
        } else {
            crate::logger::log(format!(
                "toggle_native_video_vst3_gui: bridge state={:?}, skipping settings save",
                self.dsp_bridge.state()
            ));
        }
    }

    /// 音声 VST シェルに入る (music Inc 6 ②-3、B-prime)。走行中の headless audio player に
    /// native presenter を live-attach する。owner/HUD 登録とプラグイン GUI 表示は presenter
    /// HWND が publish されて fullscreen owner 登録が済んでから `tick_music_vst_shell` が行う
    /// (HWND 未確定で GUI を出すと editor が main HWND に生成され z-order が壊れる、Codex High)。
    /// 音声スレッド・音楽解析状態には触れないので無中断。呼び出し元は音楽 HUD の VST ボタン
    /// (`draw_music_bottom_hud`、Inc 6 ②-4)。
    #[cfg(windows)]
    pub(crate) fn enter_music_vst_shell(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.music_vst_shell.is_some() {
            return;
        }
        // 音声アイテムのみ。
        if !matches!(self.items.get(fs_idx), Some(GridItem::Audio(_))) {
            return;
        }
        // VST owner/HUD 機構は OFF モードのメイン fullscreen 前提。detached / 複数
        // ウィンドウ mode ではチェーン処理だけを共有し、GUI は開かない。
        if !self.vst3_playback_ui_context_is_main_fullscreen() {
            self.show_feedback_toast(
                "VST はメインウィンドウのフルスクリーンでのみ使用できます".to_string(),
            );
            return;
        }
        if !self.settings.vst3_enabled
            || !matches!(
                self.dsp_bridge.state(),
                crate::video::dsp::DspState::Enabled
            )
        {
            self.show_feedback_toast("VST3 が有効ではありません".to_string());
            return;
        }
        let Some((placement, rect, owner_hwnd)) =
            self.native_video_target_for_presentation(ViewerPresentation::Fullscreen)
        else {
            return;
        };
        let file_name = match self.items.get(fs_idx) {
            Some(GridItem::Audio(path)) => path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("audio")
                .to_string(),
            _ => "audio".to_string(),
        };
        let Some(config) = super::native_video_presenter_config(
            owner_hwnd,
            rect,
            placement,
            true, // activate_on_show (fullscreen)
            self.window_visible,
            file_name,
            self.video_perf_overlay_visible,
            false, // initial_tile_overlay: 音声は preparing タイルを出さない
            true,  // vst3_available
            self.checked.contains(&fs_idx), // checked: グリッドのチェック状態 (動画 HUD と同じ)
            self.settings.ui_scale_factor,
            self.settings.text_contrast,
            self.settings.ui_font.clone(),
            self.settings.fullscreen_cursor_hide_delay_secs,
            Some(self.dsp_bridge.editor_hwnds_snapshot()),
            self.main_hwnd.unwrap_or(0) as u64,
            self.creative_lut_library.video_snapshot(
                &self.settings.creative_luts,
                &self.settings.video_adjustments,
                &self.settings.video_preset_slots,
            ),
            true, // audio_only (frameless present、Inc 6 ②-1)
        ) else {
            return;
        };
        let attach = match self.fs_cache.get_mut(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                player.attach_native_output_from_config(config)
            }
            _ => Err("音声プレイヤーが見つかりません".to_string()),
        };
        match attach {
            Ok(()) => {
                self.music_vst_shell = Some(super::MusicVstShell {
                    fs_idx,
                    activated: false,
                });
                // presenter HWND publish → owner 登録 → tick で GUI 表示、と数フレームかかる。
                // 一時停止中の音声はアイドルで repaint が止まるので、activation に到達するよう
                // ここで明示的に起こす (tick も pending 中は継続 repaint する、Codex Medium)。
                ctx.request_repaint();
                crate::logger::log(format!("[music-vst] entered VST shell for fs_idx={fs_idx}"));
            }
            Err(err) => {
                self.show_feedback_toast(format!("VST 画面を開けません: {err}"));
                crate::logger::log(format!("[music-vst] attach failed: {err}"));
            }
        }
    }

    /// 音声 VST シェルの per-frame 駆動 (Inc 6 ②-3)。**`ensure_native_video_front` の後**に呼ぶ。
    /// presenter HWND が publish されて fullscreen owner 登録が済んだら
    /// (= `native_video_front_synced_hwnd == hwnd`)、プラグイン GUI を表示 + topmost にして
    /// `show_vst3_manager` を立てる。owner 登録前に GUI を出すと editor が main HWND に生成
    /// されるので必ず synced を待つ (Codex High)。`show_vst3_manager` もここで初めて立てる
    /// ことで、egui VST マネージャ窓が native 被覆前に音楽ビューへ 1 フレちらつくのを防ぐ。
    #[cfg(windows)]
    pub(crate) fn tick_music_vst_shell(&mut self, ctx: &egui::Context) {
        let Some(shell) = self.music_vst_shell else {
            return;
        };
        if shell.activated {
            return;
        }
        if !matches!(self.viewer_presentation, ViewerPresentation::Fullscreen) {
            return;
        }
        let hwnd = match self.fs_cache.get(&shell.fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.native_presenter_hwnd(),
            _ => 0,
        };
        // owner 登録済み (ensure_native_video_front が synced=hwnd をセット + register_fullscreen_owner)
        // を待つ。まだなら pending 継続なので、一時停止中でも次フレームを起こして polling を
        // 続ける (Codex Medium: 一時停止音声だと自然な repaint が来ない)。
        if hwnd == 0 || self.native_video_front_synced_hwnd != hwnd {
            ctx.request_repaint();
            return;
        }
        self.dsp_bridge.set_existing_guis_owner_to_hwnd(hwnd);
        self.native_video_owner_synced_hwnd = hwnd;
        self.show_vst3_manager = true;
        self.dsp_bridge.set_all_guis_topmost(true);
        std::sync::Arc::clone(&self.dsp_bridge).set_all_guis_visible_async(true);
        if matches!(
            self.dsp_bridge.state(),
            crate::video::dsp::DspState::Enabled
        ) && !self.settings.vst3_gui_visible
        {
            self.settings.vst3_gui_visible = true;
            self.settings.save();
        }
        if let Some(s) = self.music_vst_shell.as_mut() {
            s.activated = true;
        }
        crate::logger::log(format!(
            "[music-vst] activated VST GUIs for fs_idx={} hwnd=0x{:x}",
            shell.fs_idx, hwnd
        ));
    }

    /// 音声 VST シェルを抜けて egui 音楽ビューへ戻る (Inc 6 ②-3)。presenter HWND が生きて
    /// いるうちに VST GUI を main へ re-owner + hide (dying HWND に owner された孤児 window を
    /// 防ぐ) → owner/HUD 登録を明示クリア → native 出力だけ drop (presenter スレッド
    /// cancel+join)。音声スレッド・音楽解析状態 (解析/spectrum/bookmark/normalize) には
    /// 触れない (無中断・状態保持)。冪等 (シェル未在なら no-op)。
    #[cfg(windows)]
    pub(crate) fn exit_music_vst_shell(&mut self) {
        let Some(shell) = self.music_vst_shell.take() else {
            return;
        };
        // close_fullscreen の VST cleanup と同じ順序で GUI を main へ戻して隠す。
        self.dsp_bridge.set_existing_guis_owner_to_main();
        self.dsp_bridge.set_all_guis_visible(false);
        self.dsp_bridge.set_all_guis_topmost(false);
        self.show_vst3_manager = false;
        self.native_video_owner_synced_hwnd = 0;
        if self.settings.vst3_gui_visible {
            self.settings.vst3_gui_visible = false;
            self.settings.save();
        }
        // owner/HUD 登録を明示クリア。native 出力の drop は非同期 join なので、次フレームの
        // ensure_native_video_front (hwnd==0 経路) を待たずここで外す (§5.9)。
        self.dsp_bridge.set_hud_hwnd(0);
        self.dsp_bridge.unregister_fullscreen_owner();
        self.vst_geometry_tracker.clear();
        self.native_video_front_synced_hwnd = 0;
        self.native_video_front_last_raise = None;
        self.native_video_front_recover_after_external_foreground = false;
        // native 出力だけ drop (presenter スレッド cancel+join)。音声は不触。
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get_mut(&shell.fs_idx) {
            let _ = player.take_native_output();
        }
        // clear_music_view_state は呼ばない (解析/spectrum/bookmark/normalize を保持)。
        crate::logger::log(format!(
            "[music-vst] exited VST shell for fs_idx={}",
            shell.fs_idx
        ));
    }

    /// 音声 VST シェル中に native presenter イベントを通してよいか (Inc 6 ②-3)。再生・音量・
    /// VST パネル/スロット・normalize・perf・Window (キーは key handler 側で個別 gate) は通す。
    /// 動画フレーム専用 (tile / frame step / サムネ / フレームコピー / 保存 / compact) と、
    /// 動画ブックマーク・タグ/★・ナビ・placement は audio に不適なので no-op。close と
    /// ToggleVst3Gui は呼び出し側で「シェル離脱」に振るのでここには渡らない。
    #[cfg(windows)]
    fn native_event_allowed_in_music_shell(event: &crate::video::NativeVideoOutputEvent) -> bool {
        use crate::video::NativeVideoOutputEvent as Ev;
        matches!(
            event,
            Ev::Window(_)
                | Ev::Seek { .. }
                | Ev::SeekToStartAndPlay
                | Ev::TogglePlay
                | Ev::ToggleMute
                // ToggleLoop / ToggleContinuous は動画用 helper (chapter/freshness gate の意味が
                // audio とずれる) に流れるので no-op。ループ/連続は egui 音楽ビューで設定する
                // (Codex Medium)。
                | Ev::SetVolume { .. }
                | Ev::SetPlaybackSpeed { .. }
                | Ev::TogglePerfOverlay
                | Ev::ToggleSidePanelMode
                | Ev::ToggleClickInfoOpen
                | Ev::SetVst3PanelVisible { .. }
                | Ev::SetVst3PanelPos { .. }
                | Ev::Vst3ShowSlotGui { .. }
                | Ev::Vst3HideSlotGui { .. }
                | Ev::Vst3SetBypass { .. }
                | Ev::Vst3LoadChainSlot { .. }
                | Ev::Vst3SaveChainSlot { .. }
                | Ev::ToggleNormalize
                | Ev::DisableNormalize
                | Ev::CancelNormalizeScan
        )
    }

    /// 「動画→音声モード」に入る (Inc 7 hidden presenter、docs §5.7.0)。走行中の動画プレイヤーの
    /// native presenter を **drop せず hide** し (`set_native_window_visible(false)`)、presenter は
    /// consume-and-hold でデコードを続けたまま egui 音楽ビューを表示する。音声スレッド (pump / CPAL /
    /// decoder の audio 経路 / `DspBridge` / normalize / 解析状態) には一切触れないので **音声は
    /// 無中断**。presenter を生かすので exit も show するだけで済み、seek せず音切れが起きない。
    /// owner/HUD/VST GUI の後始末は presenter HWND が生きているうちに `exit_music_vst_shell` と同じ
    /// 順序で行い、孤児 VST window を防ぐ (Codex 7c 設計)。
    ///
    /// 呼び出し元 = 動画 HUD の「音声モード」ボタン (`NativeVideoOutputEvent::ToggleAudioMode` →
    /// `handle_native_video_output_event`、7d で配線)。
    #[cfg(windows)]
    pub(crate) fn reset_video_audio_side_panel_sessions(&mut self, fs_idx: usize) {
        self.music_left_panel_active = false;
        self.music_right_panel_active = false;
        self.music_left_panel_open = crate::ui_helpers::MetadataPanelOpenState::Closed;
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.reset_native_side_panel_session();
        }
    }

    pub(crate) fn enter_video_audio_mode(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.video_audio_mode.is_some() {
            return;
        }
        // 動画アイテムのみ・現在フルスクリーンで開いている idx・borderless presenter 前提。
        if !matches!(self.items.get(fs_idx), Some(GridItem::Video(_)))
            || self.fullscreen_idx != Some(fs_idx)
        {
            return;
        }
        // フルスクリーン / ウィンドウ内 (MainWindow) / 別ウィンドウ (F12 DetachedWindow) の
        // いずれでも使える。DetachedWindow は presenter child が detached viewport host に
        // ぶら下がるが、描画/backdrop/host-resync 経路は既に fs_music_view_active 述語を通して
        // 音楽ビューへ追随する (7e、docs §5.7.1)。ただし detached では下の entry_target 捕捉を
        // 必須にして、host 過渡フレームでの劣化 (exit fallback = 音切れ) を防ぐ。なお音声モード中の
        // F12 (presentation 切替) は hidden presenter の rebuild を伴うため gate off にしている
        // (ui_fullscreen.rs の handle_fs_key_input、Codex 7e 助言)。
        // placement switch / source-swap 進行中は presenter HWND が作り直される最中。ここで
        // detach すると `PlacementSwitched` / swap 完了イベントが届かず pending が stale 化して
        // owner/VST 同期が止まる (Codex 7d P2、docs §5.7.1「switch 中は entry block」)。
        //
        // 7e: 遅延 F12 detach migration (`pending_detached_video_host_switch`) も in-flight な
        // placement switch として扱い入場をブロックする。F12 で detach を試みて detached host が
        // 未 ready のまま音声モードに入ると、後で host ready 時に `poll_detached_video_host_switch_pending`
        // が `switch_native_video_viewer_presentation` を走らせ、hidden presenter を un-hide して
        // 「音声モードなのに動画が出る」ことがあるため (Codex 7e P2)。migration 完了後に再度
        // ♪/Z を押せば通常経路で音声モードへ入れる。
        if self.native_video_mode_switch.is_some()
            || self.native_video_source_swap_pending.is_some()
            || self.detached_video_host_switch_pending()
        {
            return;
        }
        // presenter が実際に上がっている (HWND publish 済み) ことを確認する。まだ preparing の
        // 動画で音声モードに入るのは無意味なので弾く。
        let has_presenter = matches!(
            self.fs_cache.get(&fs_idx),
            Some(FsCacheEntry::Video { player, .. }) if player.native_presenter_hwnd() != 0
        );
        if !has_presenter {
            return;
        }
        // 音声トラックが無い動画は音声モードにしても無音 + 空波形なので弾く (Codex 7d P3)。
        // info 未取得 (稀、presenter 起動済みなら通常取得済み) のときは許可する。
        let no_audio_track = matches!(
            self.fs_cache.get(&fs_idx),
            Some(FsCacheEntry::Video { player, .. }) if player.info().is_some_and(|i| !i.has_audio)
        );
        if no_audio_track {
            self.show_feedback_toast("この動画には音声トラックがありません".to_string());
            return;
        }
        // exit で placement 復帰の判定に使う enter 時点の物理ターゲット (placement / rect /
        // owner_hwnd) を先に捕捉する。DetachedWindow では host HWND / client rect が取れない
        // 過渡フレーム (別ウィンドウ生成中 / host 未捕捉) がある。target が None のまま進むと
        // exit が fallback (detach+attach+seek = 短い音切れ) に劣化するので、detached では
        // target 確定を必須にして、取れなければ teardown 前に no-op で抜ける (Codex 7e 助言)。
        // Fullscreen / MainWindow は main_hwnd 前提なので通常 Some。
        let entry_target = self.native_video_target_for_presentation(self.viewer_presentation);
        if matches!(self.viewer_presentation, ViewerPresentation::DetachedWindow)
            && entry_target.is_none()
        {
            self.show_feedback_toast(
                "別ウィンドウの準備中です。少し待ってからもう一度お試しください".to_string(),
            );
            return;
        }
        // ── VST/owner/HUD teardown (exit_music_vst_shell と同じ順序、presenter HWND 生存中に) ──
        self.dsp_bridge.set_existing_guis_owner_to_main();
        self.dsp_bridge.set_all_guis_visible(false);
        self.dsp_bridge.set_all_guis_topmost(false);
        self.show_vst3_manager = false;
        self.native_video_owner_synced_hwnd = 0;
        if self.settings.vst3_gui_visible {
            self.settings.vst3_gui_visible = false;
            self.settings.save();
        }
        self.dsp_bridge.set_hud_hwnd(0);
        self.dsp_bridge.unregister_fullscreen_owner();
        self.vst_geometry_tracker.clear();
        self.native_video_front_synced_hwnd = 0;
        self.native_video_front_last_raise = None;
        self.native_video_front_recover_after_external_foreground = false;
        // タイルモードは音声モードに無関係なので畳む。
        self.video_tile_mode_active = false;
        self.video_tile_state = None;
        self.video_tile_reopen_pending = false;
        self.video_tile_reopen_deadline = None;
        // ── hidden presenter 方式 (docs §5.7.0): presenter は drop せず「hide + デコード継続
        // (consume-and-hold)」にする。これで exit は show するだけで映像復帰でき、seek / audio
        // を触らないので音切れが起きない (現行 detach + re-attach + seek の弱点だった)。
        //
        // 上で捕捉した enter 時点の物理ターゲットを保存する。音声モード中に全画面⇔ウィンドウ⇔
        // 別ウィンドウを切り替えてから戻ったとき、これと現在ターゲットを比較して「そのまま show」か
        // 「SwitchPlacement で作り直し」かを選ぶ (Codex 案D)。
        self.video_audio_mode_entry_target = entry_target;
        self.video_audio_exit_pending = None;
        self.reset_video_audio_side_panel_sessions(fs_idx);
        self.video_audio_mode = Some(fs_idx);
        // 動画モードで追加/改名/削除したブックマークを音楽ビューへ確実に反映する (#6 修正)。
        // 動画側は video_bookmark_db + fullscreen_video_marker_cache を更新するが、音楽ビューの
        // music_bookmarks は別キャッシュ (music_bookmarks_loaded_for でゲート) なので、exit で
        // キャッシュを保持したまま動画側で書き換えると stale になる。enter でロード済みフラグを
        // 落として、次の draw_fs_music_view で DB から再ロードさせる。
        self.music_bookmarks_loaded_for = None;
        // presenter に hide コマンドを送る (drop しない)。presenter スレッドは以降
        // consume-and-hold に入り、映像を present せず最新フレームだけ hold する。音声 routing は
        // 無改変なので無中断。`video_audio_mode` を Some にした **後** に hide を送ることで、
        // 次フレームには egui 音楽ビューが描かれ、presenter ウィンドウが隠れても穴が空かない
        // (むしろ 1 フレーム映像が残るだけで視覚的に自然)。
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_media_visual_mode(music_core::MediaVisualMode::Music);
            player.set_native_window_visible(false);
        }
        ctx.request_repaint();
        crate::logger::log(format!(
            "[video-audio] entered audio mode (hidden presenter) for fs_idx={fs_idx}"
        ));
    }

    /// 「動画→音声モード」を抜けて動画表示へ戻る (Inc 7 hidden presenter、docs §5.7.0)。
    /// presenter は enter で drop せず hide しただけなので、基本は **show するだけ**で映像復帰でき、
    /// seek / audio を触らない = **音切れ無し**。exit は非同期 (presenter の再表示を confirm する
    /// まで `video_audio_mode` を Some に保って音楽ビューを描き続け、`poll_video_audio_exit_pending`
    /// が None に落とす。逆順だと 1 フレーム映像が出ない穴が空く = Codex Q4)。
    ///
    /// 分岐 (Codex 案D): enter 時点の物理ターゲット (placement / owner_hwnd) と現在ターゲットを
    /// 比較し、
    /// - **一致** → presenter を show するだけ (完全シームレス)。
    /// - **不一致** (音声モード中に全画面⇔ウィンドウを切り替えた) → `SwitchPlacement` で正しい
    ///   placement へ作り直して復帰 (source を保持するので音声は無中断、presenter 側 rebuild が
    ///   hold フレームで prime + show + hidden 解除)。
    /// - **ターゲット取得不可** (通常起きない) → 従来の detach + attach + seek に同期フォールバック。
    ///
    /// 比較は物理ターゲット `(placement, rect, owner)` の**フル tuple** で行う (Codex P3): 全画面⇔
    /// ウィンドウの往復だけでなく、同一 placement の rect 変化 (ウィンドウ resize / fullscreen の
    /// モニタ rect 変更) も検出して SwitchPlacement へ流す。SwitchPlacement の同一 placement resize
    /// 分岐も hidden 対応済み (rect 変更後に hold フレームで show + hidden 解除) なので、どちらの
    /// 変化でもシームレスに復帰できる。
    ///
    /// 呼び出し元 = 音楽ビュー上バーの「動画に戻る」ボタン (draw_music_top_bar、draw 中の直呼び)。
    #[cfg(windows)]
    pub(crate) fn exit_video_audio_mode(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.video_audio_mode != Some(fs_idx) {
            return;
        }
        // 既に exit 進行中なら二重起動しない (ボタンが複数フレーム描かれる間の連打対策)。
        if self.video_audio_exit_pending.is_some() {
            return;
        }
        // 音声モードの連続再生 EOF による source-swap 進行中は exit を受け付けない (Codex P2)。
        // この間は旧 player が既に外され新 player 未挿入で `fs_cache[fs_idx]` が無く、show /
        // SwitchPlacement の対象 presenter が pending 内に退避しているため、ここで exit すると
        // `poll_video_audio_exit_pending` が player-gone で video_audio_mode を落としても、swap の
        // `audio_mode_after_swap=true` で completion がまた音声モードに戻す = exit が失われる。
        // swap 完了 (= 数百 ms) 後に再度ボタンを押せば通常経路で正しく動画表示へ戻れる。enter 側も
        // 同じ理由で swap 中は entry をブロックしている (対称)。
        if self.native_video_source_swap_pending.is_some() {
            crate::logger::log(format!(
                "[video-audio] exit fs_idx={fs_idx} ignored: source-swap in flight (retry after swap)"
            ));
            return;
        }
        // ここからは動画表示へ復帰する操作。再 Buffering が起きた場合も通常の動画と同じく
        // FirstFrameReady を待つよう、presenter の show/re-place より先に要件を戻す。
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_media_visual_mode(music_core::MediaVisualMode::Video);
        }
        self.reset_video_audio_side_panel_sessions(fs_idx);
        // saw_hidden を「今 hidden か」でシードする (Codex P2 検証): 音声モードでは presenter は既に
        // hide 済み (でないと音楽ビューが見えずこのボタンも押せない) なので通常 true になる。これで
        // show / SwitchPlacement を送ったあと、次の poll より前に presenter が hidden→表示 を処理して
        // しまっても (= UI が hidden==true を観測し損ねる race)、seed 済み saw_hidden により
        // `!native_presenter_hidden()` で正しく完了できる。万一 hide 未処理 (= 今 false) なら seed も
        // false で、その稀ケースは timeout フォールバックに委ねる。
        let seed_saw_hidden = matches!(
            self.fs_cache.get(&fs_idx),
            Some(FsCacheEntry::Video { player, .. }) if player.native_presenter_hidden()
        );
        let cur_target = self.native_video_target_for_presentation(self.viewer_presentation);
        // 物理ターゲット (placement, rect, owner) が完全一致するか。RECT の PartialEq に依存せず
        // フィールド比較する。
        let placement_matches = match (cur_target, self.video_audio_mode_entry_target) {
            (Some((cur_pl, cur_rect, cur_owner)), Some((entry_pl, entry_rect, entry_owner))) => {
                cur_pl == entry_pl
                    && cur_owner == entry_owner
                    && cur_rect.left == entry_rect.left
                    && cur_rect.top == entry_rect.top
                    && cur_rect.right == entry_rect.right
                    && cur_rect.bottom == entry_rect.bottom
            }
            _ => false,
        };
        if placement_matches {
            // 高速 show (シームレス)。presenter が再表示 (`!native_presenter_hidden()`) を
            // confirm するまで video_audio_mode は Some のまま保つ。
            if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
                player.set_native_window_visible(true);
            }
            self.video_audio_exit_pending = Some(super::VideoAudioExitPending {
                fs_idx,
                deadline: std::time::Instant::now() + std::time::Duration::from_millis(400),
                saw_hidden: seed_saw_hidden,
            });
            crate::logger::log(format!(
                "[video-audio] exit fs_idx={fs_idx}: fast show (placement unchanged)"
            ));
            ctx.request_repaint();
        } else if cur_target.is_some() {
            // placement / rect 変更を伴う復帰: SwitchPlacement で presenter を作り直す / resize する
            // (source 保持 = 音声無中断)。presenter は hidden なので、rebuild / same-placement resize
            // が hold フレームで prime + show + presenter_hidden 解除する (video/mod.rs)。owner/HUD は
            // switch 完了後の ensure_native_video_front が再登録する。
            self.switch_native_video_viewer_presentation(self.viewer_presentation, true);
            self.video_audio_exit_pending = Some(super::VideoAudioExitPending {
                fs_idx,
                // rebuild は show より時間がかかるので長めの保険。
                deadline: std::time::Instant::now() + std::time::Duration::from_millis(1200),
                saw_hidden: seed_saw_hidden,
            });
            crate::logger::log(format!(
                "[video-audio] exit fs_idx={fs_idx}: placement changed, re-placing via SwitchPlacement"
            ));
            ctx.request_repaint();
        } else {
            // ターゲット取得不可 (通常起きない): 同期フォールバック。
            crate::logger::log(format!(
                "[video-audio] exit fs_idx={fs_idx}: target unavailable, fallback detach+attach+seek"
            ));
            self.exit_video_audio_mode_fallback(ctx, fs_idx);
        }
    }

    /// hidden presenter からの exit 完了待ちをポーリングする (`poll_video` から毎フレーム)。
    /// presenter が再表示された (`!native_presenter_hidden()`) ら `video_audio_mode` を None に
    /// 落として音楽ビュー描画を止め、動画表示へ戻す。deadline 超過 (presenter 無応答 /
    /// SwitchPlacement 失敗) は detach+attach+seek フォールバックへ回す。
    #[cfg(windows)]
    pub(crate) fn poll_video_audio_exit_pending(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.video_audio_exit_pending else {
            return;
        };
        // モードが既に別経路 (close_fullscreen / open_fullscreen) でクリアされていたら pending も捨てる。
        if self.video_audio_mode != Some(pending.fs_idx) {
            self.video_audio_exit_pending = None;
            return;
        }
        let (player_present, hidden) = match self.fs_cache.get(&pending.fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => (true, player.native_presenter_hidden()),
            _ => (false, false),
        };
        if !player_present {
            // player が消えた (evict 等): stuck を避けるため即完了扱いにしてモードを畳む。
            // mismatch 経路で張った SwitchPlacement pending が居ても PlacementSwitched は届かない
            // ので明示クリアする (Codex P3、fallback と同様)。
            self.video_audio_mode = None;
            self.video_audio_mode_entry_target = None;
            self.video_audio_exit_pending = None;
            self.native_video_mode_switch = None;
            ctx.request_repaint();
            return;
        }
        // hidden を一度でも観測してからでないと「再表示」を完了とみなさない (Codex P2)。
        // 通常 exit は enter で hide 済み = 最初の poll で saw_hidden が立ち、その後 show /
        // SwitchPlacement が処理されて hidden=false になった時点で完了する。
        if hidden && !pending.saw_hidden {
            if let Some(p) = self.video_audio_exit_pending.as_mut() {
                p.saw_hidden = true;
            }
        }
        let saw_hidden = pending.saw_hidden || hidden;
        if saw_hidden && !hidden {
            // 再表示済み: 音楽ビューを畳んで動画表示へ (owner/HUD は次フレームの
            // ensure_native_video_front が再登録)。
            self.video_audio_mode = None;
            self.video_audio_mode_entry_target = None;
            self.video_audio_exit_pending = None;
            ctx.request_repaint();
            crate::logger::log(format!(
                "[video-audio] exit fs_idx={} complete (presenter shown)",
                pending.fs_idx
            ));
        } else if std::time::Instant::now() >= pending.deadline {
            // presenter が無応答 / switch 失敗: フォールバックで確実に復帰させる。
            self.video_audio_exit_pending = None;
            crate::logger::log(format!(
                "[video-audio] exit fs_idx={} timed out; falling back to detach+attach+seek",
                pending.fs_idx
            ));
            self.exit_video_audio_mode_fallback(ctx, pending.fs_idx);
        } else {
            // まだ hidden: 再描画を回して次フレームで再判定。
            ctx.request_repaint();
        }
    }

    /// 音声モード exit のフォールバック (placement ターゲット不可 / presenter 無応答時)。旧
    /// detach 方式と同じく presenter を drop → 現在 presentation で attach → 現在位置へ seek して
    /// 映像復帰する。**この経路だけ seek が audio buffer を flush するので短い音切れが起き得る**が、
    /// 通常経路 (hidden show / SwitchPlacement) はシームレスで、これは異常時の保険。
    #[cfg(windows)]
    fn exit_video_audio_mode_fallback(&mut self, ctx: &egui::Context, fs_idx: usize) {
        self.video_audio_mode = None;
        self.video_audio_mode_entry_target = None;
        self.video_audio_exit_pending = None;
        // フォールバックは presenter を drop → 新規 attach するので、進行中の SwitchPlacement
        // (exit-mismatch 経路が張った pending) が居ても PlacementSwitched は届かない。stale pending
        // を明示クリアして owner/availability 同期が固まらないようにする (Codex P2)。
        self.native_video_mode_switch = None;
        let presentation = self.viewer_presentation;
        let target = self.native_video_target_for_presentation(presentation);
        let config = target.and_then(|(placement, rect, owner_hwnd)| {
            let file_name = match self.items.get(fs_idx) {
                Some(GridItem::Video(path)) => path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("video")
                    .to_string(),
                _ => "video".to_string(),
            };
            super::native_video_presenter_config(
                owner_hwnd,
                rect,
                placement,
                true,
                self.window_visible,
                file_name,
                self.video_perf_overlay_visible,
                false,
                self.settings.vst3_enabled,
                self.checked.contains(&fs_idx),
                self.settings.ui_scale_factor,
                self.settings.text_contrast,
                self.settings.ui_font.clone(),
                self.settings.fullscreen_cursor_hide_delay_secs,
                Some(self.dsp_bridge.editor_hwnds_snapshot()),
                self.main_hwnd.unwrap_or(0) as u64,
                self.creative_lut_library.video_snapshot(
                    &self.settings.creative_luts,
                    &self.settings.video_adjustments,
                    &self.settings.video_preset_slots,
                ),
                false,
            )
        });
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get_mut(&fs_idx) {
            player.set_media_visual_mode(music_core::MediaVisualMode::Video);
            let pos = player.position().max(0.0);
            let _ = player.take_native_output();
            player.seek(pos);
            if let Some(config) = config {
                match player.attach_native_output_from_config(config) {
                    Ok(()) => {
                        crate::logger::log(format!(
                            "[video-audio] fallback exit fs_idx={fs_idx}: re-attached presenter, seek={pos:.3}"
                        ));
                    }
                    Err(err) => {
                        crate::logger::log(format!(
                            "[video-audio] fallback exit re-attach failed fs_idx={fs_idx}: {err}"
                        ));
                        self.show_feedback_toast(format!("動画表示に戻れませんでした: {err}"));
                    }
                }
            } else {
                crate::logger::log(format!(
                    "[video-audio] fallback exit fs_idx={fs_idx}: presenter target unavailable"
                ));
            }
        }
        ctx.request_repaint();
    }

    /// VST ホストの GUI/owner/HUD 後始末だけを行う (presenter の re-hide と `video_audio_vst` の
    /// clear は**しない**)。`exit_video_audio_vst` (通常離脱、re-hide 付き) と `tick_video_audio_vst`
    /// の mismatch フォールバック (presenter は遷移先経路が扱うので re-hide しない) で共有する
    /// (Codex P2: どちらの経路でも GUI を確実に畳んで孤児 editor / topmost 残留を防ぐ)。
    #[cfg(windows)]
    fn teardown_video_audio_vst_gui(&mut self) {
        // GUI を main へ戻して隠す (close_fullscreen / exit_music_vst_shell と同じ順序)。
        self.dsp_bridge.set_existing_guis_owner_to_main();
        self.dsp_bridge.set_all_guis_visible(false);
        self.dsp_bridge.set_all_guis_topmost(false);
        self.show_vst3_manager = false;
        self.native_video_owner_synced_hwnd = 0;
        if self.settings.vst3_gui_visible {
            self.settings.vst3_gui_visible = false;
            self.settings.save();
        }
        self.dsp_bridge.set_hud_hwnd(0);
        self.dsp_bridge.unregister_fullscreen_owner();
        self.vst_geometry_tracker.clear();
        self.native_video_front_synced_hwnd = 0;
        self.native_video_front_last_raise = None;
        self.native_video_front_recover_after_external_foreground = false;
    }

    /// 「動画→音声モード」で VST プラグイン GUI を出す (7e、approach A = 映像プレゼンター流用)。
    /// 音声モードでは presenter が hide されていて VST エディタ窓 (presenter owner) を出せないので、
    /// presenter を **un-hide** して VST ホストにする (映像は GUI の背後に見える = ユーザー選択)。
    /// presenter は drop せず、`exit_video_audio_vst` の re-hide で音声モード (波形) へ戻れる。
    /// VST チェーン (プラグイン/パラメータ) は app グローバル `dsp_bridge` 共有なので音への効果は
    /// 元から引き継がれ、ここは GUI 表示だけを司る。GUI 表示自体は owner 登録を待って
    /// `tick_video_audio_vst` が行う (早いと editor が main HWND に生成される、§5.9)。
    /// 呼び出し元 = 音楽ビュー上バーの VST ボタン (`draw_music_top_bar`、draw 中の直呼び)。
    #[cfg(windows)]
    pub(crate) fn enter_video_audio_vst(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.video_audio_vst.is_some() {
            return;
        }
        // 音声モードにトグルした当該動画で・フルスクリーン表示中のみ。
        if self.video_audio_mode != Some(fs_idx) || self.fullscreen_idx != Some(fs_idx) {
            return;
        }
        // 「動画に戻る」exit の非同期完了待ち中は presenter が show/SwitchPlacement 進行中なので拒否。
        if self.video_audio_exit_pending.is_some() {
            return;
        }
        // VST owner/HUD 機構は OFF モードのメイン fullscreen 前提。detached / 複数
        // ウィンドウ mode ではチェーン処理だけを共有し、GUI は開かない。
        if !self.vst3_playback_ui_context_is_main_fullscreen() {
            self.show_feedback_toast(
                "VST はメインウィンドウのフルスクリーンでのみ使用できます".to_string(),
            );
            return;
        }
        if !self.settings.vst3_enabled
            || !matches!(
                self.dsp_bridge.state(),
                crate::video::dsp::DspState::Enabled
            )
        {
            self.show_feedback_toast("VST3 が有効ではありません".to_string());
            return;
        }
        // presenter が上がっている (HWND publish 済み) こと。音声モードなら通常 hidden で存在する。
        let has_presenter = matches!(
            self.fs_cache.get(&fs_idx),
            Some(FsCacheEntry::Video { player, .. }) if player.native_presenter_hwnd() != 0
        );
        if !has_presenter {
            return;
        }
        // presenter を un-hide して VST ホストにする (consume-and-hold 解除、映像は GUI の背後に
        // 見える)。音声スレッドは無改変なので音切れなし。
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_window_visible(true);
        }
        self.video_audio_vst = Some(super::VideoAudioVstState {
            fs_idx,
            phase: super::VideoAudioVstPhase::Opening,
        });
        // owner 登録 → GUI 表示は tick_video_audio_vst が担う。一時停止中はアイドルで repaint が
        // 来ないので明示的に起こす (tick も pending 中は継続 repaint)。
        ctx.request_repaint();
        crate::logger::log(format!(
            "[video-audio-vst] entered VST host (un-hid presenter) for fs_idx={fs_idx}"
        ));
    }

    /// VST ホストの per-frame 駆動 (7e)。**`ensure_native_video_front` の後**に呼ぶ。presenter HWND
    /// が fullscreen owner 登録済み (`native_video_front_synced_hwnd==hwnd`) になったらプラグイン
    /// GUI を表示 + topmost にして `show_vst3_manager` を立てる (owner 登録前だと editor が main
    /// HWND に生成される、§5.9)。bridge 死亡 / VST 無効化 / フルスクリーン離脱 / presenter 消失を
    /// 検出したら VST ホストを畳む (`tick_music_vst_shell` と同じ遅延ラッチ + guard 構造)。
    #[cfg(windows)]
    pub(crate) fn tick_video_audio_vst(&mut self, ctx: &egui::Context) {
        let Some(state) = self.video_audio_vst else {
            return;
        };
        let fs_idx = state.fs_idx;
        // 音声モードから外れた / フルスクリーンでなくなった (別経路が既に video_audio_mode を落とした
        // 稀ケース): presenter は遷移先経路が扱うので re-hide はしないが、GUI 後始末は必ず行って
        // 孤児 editor を残さない (Codex P2)。
        if self.video_audio_mode != Some(fs_idx) || self.fullscreen_idx != Some(fs_idx) {
            self.teardown_video_audio_vst_gui();
            self.video_audio_vst = None;
            return;
        }
        // presenter HWND は Active 早期 return より前に取得して、Active 中に presenter が消えた
        // (close/evict) ケースも畳めるようにする (Codex P2)。
        let hwnd = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.native_presenter_hwnd(),
            _ => 0,
        };
        // bridge 死亡 / VST 無効化 / 非フルスクリーン化 / presenter 消失: VST ホストを畳んで波形
        // ビューへ戻す (Codex P2)。
        if !self.settings.vst3_enabled
            || !matches!(
                self.dsp_bridge.state(),
                crate::video::dsp::DspState::Enabled
            )
            || !matches!(self.viewer_presentation, ViewerPresentation::Fullscreen)
            || hwnd == 0
        {
            self.exit_video_audio_vst(ctx, fs_idx);
            return;
        }
        if matches!(state.phase, super::VideoAudioVstPhase::Active) {
            return;
        }
        // owner 登録済みを待つ (ensure_native_video_front が synced=hwnd + register_fullscreen_owner)。
        // まだなら pending 継続 (一時停止中でも次フレームを起こして polling する、Codex Medium)。
        if self.native_video_front_synced_hwnd != hwnd {
            ctx.request_repaint();
            return;
        }
        self.dsp_bridge.set_existing_guis_owner_to_hwnd(hwnd);
        self.native_video_owner_synced_hwnd = hwnd;
        self.show_vst3_manager = true;
        self.dsp_bridge.set_all_guis_topmost(true);
        std::sync::Arc::clone(&self.dsp_bridge).set_all_guis_visible_async(true);
        if !self.settings.vst3_gui_visible {
            self.settings.vst3_gui_visible = true;
            self.settings.save();
        }
        if let Some(s) = self.video_audio_vst.as_mut() {
            s.phase = super::VideoAudioVstPhase::Active;
        }
        crate::logger::log(format!(
            "[video-audio-vst] activated VST GUIs for fs_idx={fs_idx} hwnd=0x{hwnd:x}"
        ));
    }

    /// VST ホストを畳んで「動画→音声モード」(波形ビュー) へ戻す (7e)。presenter は **drop せず
    /// re-hide** して consume-and-hold へ戻す (音声無中断)。GUI を main へ re-owner + hide し
    /// (dying HWND に owner された孤児窓を防ぐ)、owner/HUD 登録を明示クリアする。冪等
    /// (VST ホスト未在なら no-op)。呼び出し元 = native HUD の VST/♪/ウィンドウ切替/Esc/Z、
    /// bridge 死亡・非フルスクリーン化・presenter 消失時の tick からのフォールバック。
    #[cfg(windows)]
    pub(crate) fn exit_video_audio_vst(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.video_audio_vst.as_ref().map(|s| s.fs_idx) != Some(fs_idx) {
            return;
        }
        // GUI/owner/HUD を畳む (tick の mismatch フォールバックと共有)。
        self.teardown_video_audio_vst_gui();
        // presenter を re-hide して音声モード (consume-and-hold) へ戻す。音声は無改変 = 音切れなし。
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_window_visible(false);
        }
        self.video_audio_vst = None;
        ctx.request_repaint();
        crate::logger::log(format!(
            "[video-audio-vst] exited VST host (re-hid presenter) for fs_idx={fs_idx}"
        ));
    }

    #[cfg(windows)]
    pub(super) fn set_native_video_vst3_panel_visible(&mut self, visible: bool) {
        self.show_vst3_manager = visible;
    }

    #[cfg(windows)]
    pub(super) fn set_native_video_vst3_compact(&mut self, compact: bool) {
        if self.settings.vst3_video_compact == compact {
            return;
        }
        self.settings.vst3_video_compact = compact;
        self.settings.save();
    }

    #[cfg(windows)]
    pub(super) fn set_native_video_vst3_panel_pos(&mut self, pos: [f32; 2]) {
        if !pos[0].is_finite() || !pos[1].is_finite() {
            return;
        }
        let changed = self
            .settings
            .vst3_panel_pos
            .map(|prev| {
                let dx = prev[0] - pos[0];
                let dy = prev[1] - pos[1];
                dx * dx + dy * dy > 0.25
            })
            .unwrap_or(true);
        if !changed {
            return;
        }
        self.settings.vst3_panel_pos = Some(pos);
        self.settings.save();
    }

    #[cfg(windows)]
    pub(super) fn show_native_video_vst3_slot_gui(&mut self, idx: usize, path: String) {
        std::sync::Arc::clone(&self.dsp_bridge).show_slot_gui_async(idx);
        let mut changed = !self.settings.vst3_gui_visible;
        self.settings.vst3_gui_visible = true;
        if let Some(entry) = self.find_vst3_entry_mut(&path)
            && entry.user_hidden
        {
            entry.user_hidden = false;
            changed = true;
        }
        if changed {
            self.settings.save();
        }
    }

    #[cfg(windows)]
    pub(super) fn hide_native_video_vst3_slot_gui(&mut self, idx: usize, path: String) {
        self.dsp_bridge.user_hide_slot_gui(idx);
        if let Some(entry) = self.find_vst3_entry_mut(&path)
            && !entry.user_hidden
        {
            entry.user_hidden = true;
            self.settings.save();
        }
    }

    #[cfg(windows)]
    pub(super) fn set_native_video_vst3_slot_bypass(
        &mut self,
        idx: usize,
        path: String,
        bypass: bool,
    ) {
        self.dsp_bridge.set_bypass(idx, bypass);
        if let Some(entry) = self.find_vst3_entry_mut(&path)
            && entry.bypass != bypass
        {
            entry.bypass = bypass;
            self.settings.save();
        }
    }

    #[cfg(windows)]
    pub(crate) fn show_native_video_overlay_toast(&self, text: String, centered: bool) {
        self.show_native_video_overlay_toast_with_linger(text, centered, None);
    }

    /// `show_native_video_overlay_toast` の linger 指定版。`linger` が `Some` のとき
    /// その時間だけトーストを表示し続ける。←→ 押しっぱなしの境界トーストのように
    /// 「キーを離したら早めに消したい」用途で短い値を渡す。`None` なら従来どおり
    /// `centered` から既定値 (centered: 2.5s / それ以外: 1.8s) が使われる。
    #[cfg(windows)]
    pub(crate) fn show_native_video_overlay_toast_with_linger(
        &self,
        text: String,
        centered: bool,
        linger: Option<std::time::Duration>,
    ) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.show_native_overlay_toast(text, centered, linger);
        }
    }

    #[cfg(windows)]
    pub(crate) fn set_native_video_ring_picker_overlay(
        &self,
        overlay: Option<crate::video::native_presenter::NativeOverlayRingPicker>,
    ) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_ring_picker_overlay(overlay);
        }
    }

    #[cfg(windows)]
    pub(crate) fn set_native_video_ring_guide_overlay(
        &self,
        overlay: Option<crate::video::native_presenter::NativeOverlayRingGuide>,
    ) {
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_ring_guide_overlay(overlay);
        }
    }

    /// ←→ ホットキーの相対シーク。先頭 / 末尾に達してシークが発行されなかった
    /// 場合 (= `seek_relative` が `AtStart` / `AtEnd` を返した場合) は、
    /// overlay トーストで「動画先頭です」「動画末尾です」と通知する。
    /// 末尾でシークを発行すると decoder が target 付近のフレームを返せず
    /// 「シーク中...」表示が固着するため、ここでシーク自体を抑止している。
    #[cfg(windows)]
    fn native_video_seek_relative_with_hint(&mut self, fs_idx: usize, delta_secs: f64) {
        let outcome = match self.fs_cache.get(&fs_idx) {
            Some(FsCacheEntry::Video { player, .. }) => player.seek_relative(delta_secs),
            _ => return,
        };
        let hint = match outcome {
            crate::video::RelativeSeekOutcome::Seeked => {
                self.maybe_start_normalize_scan_for_play_intent(fs_idx);
                return;
            }
            crate::video::RelativeSeekOutcome::AtStart => "動画先頭です",
            crate::video::RelativeSeekOutcome::AtEnd => "動画末尾です",
        };
        // ←→ 押しっぱなしの間は repeat ごとに re-show されて表示が維持され、
        // キーを離すと linger 経過 (700ms) で早めに消える。通常トーストの
        // 既定値 (2.5s) のままだとキーを離した後も長く残り続けて煩わしい。
        self.show_native_video_overlay_toast_with_linger(
            hint.to_string(),
            true,
            Some(std::time::Duration::from_millis(700)),
        );
    }

    #[cfg(windows)]
    pub(super) fn native_boundary_hint_text(hint: crate::ui_fullscreen::FsBoundaryHint) -> String {
        match hint {
            crate::ui_fullscreen::FsBoundaryHint::Edge { at_end, .. } => {
                if at_end {
                    "最後の項目です  [Ctrl]+[↓] ツリー順で次へ".to_string()
                } else {
                    "最初の項目です  [Ctrl]+[↑] ツリー順で前へ".to_string()
                }
            }
            crate::ui_fullscreen::FsBoundaryHint::NoImageFolder { forward, .. } => {
                if forward {
                    "次の画像・動画フォルダが見つかりません".to_string()
                } else {
                    "前の画像・動画フォルダが見つかりません".to_string()
                }
            }
            crate::ui_fullscreen::FsBoundaryHint::NoSiblingFolder { forward, .. } => {
                if forward {
                    "次の兄弟フォルダはありません".to_string()
                } else {
                    "前の兄弟フォルダはありません".to_string()
                }
            }
            crate::ui_fullscreen::FsBoundaryHint::SearchEnd { forward, .. } => {
                if forward {
                    "これ以上先の検索結果はありません".to_string()
                } else {
                    "これ以上前の検索結果はありません".to_string()
                }
            }
            crate::ui_fullscreen::FsBoundaryHint::NavNoOp { reason, .. } => {
                Self::nav_noop_title(reason).to_string()
            }
        }
    }

    #[cfg(windows)]
    pub(super) fn navigate_native_video_fullscreen(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        base_delta: i32,
    ) {
        // tile/fast swap pending 中はナビを即時実行できないが、silent drop すると
        // 「ホイール / 前-次項目ボタンが反応しない」体感バグになる (実機 fb 2026-05-26)。
        // 最新 delta を deferred フィールドに格納し、swap 完了後の polling で drain する
        // (`maybe_apply_deferred_native_video_nav` 参照)。most-recent-wins。
        if self.video_tile_swap_pending.is_some() || self.native_video_fast_swap_pending.is_some() {
            self.native_video_deferred_nav_delta = Some(base_delta);
            return;
        }
        // `fs_nav_is_locked()` は video swap だけでなく Ctrl+↑↓ などのフルスクリーン
        // 横断ナビゲーション (= フォルダ移動) でも true になる。この場合 lock 解除後の
        // `fullscreen_idx` は別フォルダ / 非動画アイテムを指している可能性があり、
        // deferred delta を後追い適用すると予期せぬ 1 アイテム移動が発火しうる
        // (Codex 第 8 P2 指摘)。defer せず silent return に留める。
        if self.fs_nav_is_locked() {
            return;
        }
        if !self.video_tile_mode_active {
            self.cancel_stale_video_tile_reopen(Some(fs_idx), "wheel-navigation");
        }
        let display_order = self.current_grid_order().to_vec();
        let page_nav = self.spread_page_nav(base_delta);
        let nav_delta = match page_nav {
            crate::ui_fullscreen::FsPageNav::None => return,
            crate::ui_fullscreen::FsPageNav::Delta(delta) => delta,
            crate::ui_fullscreen::FsPageNav::Target(target) => {
                crate::ui_fullscreen::navigable_delta_between(
                    &self.items,
                    &display_order,
                    fs_idx,
                    target,
                )
                .unwrap_or(base_delta)
            }
            crate::ui_fullscreen::FsPageNav::Boundary { at_end } => {
                self.show_native_video_boundary_toast(ctx, at_end);
                return;
            }
        };
        self.start_manual_media_navigation(
            ctx,
            &display_order,
            fs_idx,
            nav_delta,
            "native_media_window_manual",
            crate::app::ManualMediaNavigationLanding::NativeVideo,
        );
    }

    /// `poll_native_video_fast_swap` / `poll_video_tile_swap_pending` が pending を
    /// クリアした直後に呼ぶ。`navigate_native_video_fullscreen` が pending 中に保持した
    /// 最新 nav delta を取り出して再 dispatch する。
    /// 全 pending が解消されている場合のみ drain (= 別 pending が残っているなら待つ)。
    #[cfg(windows)]
    pub(super) fn maybe_apply_deferred_native_video_nav(&mut self, ctx: &egui::Context) {
        if self.video_tile_swap_pending.is_some()
            || self.native_video_fast_swap_pending.is_some()
            || self.fs_nav_is_locked()
        {
            return;
        }
        let Some(delta) = self.native_video_deferred_nav_delta.take() else {
            return;
        };
        let Some(fs_idx) = self.fullscreen_idx else {
            return;
        };
        self.navigate_native_video_fullscreen(ctx, fs_idx, delta);
    }

    /// 動画フルスクリーンで境界 (先頭/末尾) に到達したことを示すトースト + state 更新。
    /// `navigate_native_video_fullscreen` の境界ブランチ、および Home/End キーの no-op 経路
    /// から共通で呼ぶ (動画 HUD 2 段化リデザイン Phase 1、Codex 第 1 ラウンド P2 反映)。
    #[cfg(windows)]
    pub(super) fn show_native_video_boundary_toast(&mut self, ctx: &egui::Context, at_end: bool) {
        let hint = crate::ui_fullscreen::FsBoundaryHint::Edge {
            at_end,
            at: std::time::Instant::now(),
        };
        self.show_native_video_overlay_toast(Self::native_boundary_hint_text(hint), true);
        self.fs_boundary_hint = Some(hint);
        self.request_native_video_hud_repaint(ctx);
    }

    #[cfg(windows)]
    pub(super) fn adjust_native_video_tile_columns(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        delta: i32,
    ) {
        if self.video_tile_swap_pending.is_some() || self.native_video_fast_swap_pending.is_some() {
            return;
        }
        if self.fullscreen_idx != Some(fs_idx) || delta == 0 {
            return;
        }
        let candidates = crate::settings::VIDEO_TILE_COLUMN_CANDIDATES;
        let current = self.settings.video_tile_columns;
        let current_idx = candidates
            .iter()
            .position(|&cols| cols == current)
            .unwrap_or_else(|| {
                candidates
                    .iter()
                    .position(|&cols| cols >= current)
                    .unwrap_or(candidates.len().saturating_sub(1))
            });
        let next_idx = (current_idx as i32 + delta)
            .clamp(0, candidates.len().saturating_sub(1) as i32) as usize;
        let next_cols = candidates[next_idx];
        if next_cols == current {
            return;
        }
        let was_open = self.video_tile_mode_active;
        crate::logger::log(format!(
            "[video-tile] adjust_columns: fs_idx={fs_idx} delta={delta} columns {current}->{next_cols} was_open={was_open}"
        ));
        self.settings.video_tile_columns = next_cols;
        self.settings.save();
        self.video_tile_state = None;
        self.video_tile_swap_pending = None;
        if was_open {
            let screen = self.video_tile_layout_size(fs_idx, ctx);
            self.video_tile_state = self.build_video_tile_state_for(fs_idx, screen);
            self.sync_native_video_tile_overlay(ctx, fs_idx);
        }
        self.mark_native_video_hud_activity(ctx);
    }

    #[cfg(windows)]
    pub(super) fn toggle_native_video_perf_overlay(&mut self, fs_idx: usize) {
        self.video_perf_overlay_visible = !self.video_perf_overlay_visible;
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx) {
            player.set_native_perf_overlay_visible(self.video_perf_overlay_visible);
        }
    }

    #[cfg(windows)]
    pub(super) fn next_native_video_source_epoch(&mut self) -> u64 {
        let epoch = self.native_video_source_epoch_next.max(1);
        self.native_video_source_epoch_next = epoch.wrapping_add(1).max(1);
        epoch
    }

    #[cfg(windows)]
    fn start_native_video_source_swap(
        &mut self,
        ctx: &egui::Context,
        target_idx: usize,
        autoplay_override: Option<bool>,
        ignore_resume: bool,
        show_preparing_overlay: bool,
        reason: &'static str,
        history_trigger: crate::app::HistoryTrigger,
    ) -> Option<NativeVideoSourceSwapStarted> {
        let Some(from_idx) = self.fullscreen_idx else {
            return None;
        };
        if from_idx == target_idx {
            return None;
        }
        let Some(GridItem::Video(target_path)) = self.items.get(target_idx).cloned() else {
            return None;
        };
        if !matches!(self.items.get(from_idx), Some(GridItem::Video(_))) {
            return None;
        }

        crate::logger::log(format!(
            "[native-video] fast source swap begin: reason={reason} from_idx={from_idx} -> target_idx={target_idx} target={}",
            target_path.display()
        ));
        self.save_all_video_resume_positions();
        let native_output = match self.fs_cache.get_mut(&from_idx) {
            Some(FsCacheEntry::Video { player, .. }) => {
                player.pause_audio_output();
                player.set_playing(false);
                player.clear_audio_output_buffer();
                player.take_native_output()
            }
            _ => None,
        };
        let Some(native_output) = native_output else {
            return None;
        };

        // 旧 player を **新 VideoPlayer / 新 video-decode thread を作る前に** drop する
        // (2026-05-13 fix)。VideoPlayer::drop は `cancel` フラグを Release で立てるだけで
        // join しない (= FFmpeg 内停止中の旧 thread は cancel を観測できずに残り続ける)
        // が、`cancel` をできるだけ早く立てておくと、(1) 旧 thread が安全点に居れば
        // 早めに自発 exit する、(2) live decoder count の throttle (= 新 swap 抑制)
        // が新 video decode thread の spawn 前にもう 1 つ古い枠を空けやすくなる、という
        // 2 つの効果がある。旧コードはここで drop せず末尾の `fs_cache.remove(&from_idx)`
        // まで遅延していたため、新 video decode thread と旧 thread の生存期間が
        // build_video_player_for_open + attach + switch_native_source 全工程分だけ
        // 不必要に重なっていた。
        if from_idx != target_idx {
            self.cleanup_normalize_state_for_fs_idx(from_idx);
            self.fs_cache.remove(&from_idx);
        }

        let source_epoch = self.next_native_video_source_epoch();
        let started_at = std::time::Instant::now();
        // build_video_player_for_open 内で decoder::spawn が走るので、
        // demux thread の avformat_open_input より前に bump する必要がある
        // (Codex P2 第 16 ラウンド指摘)。
        self.activity_gate.bump();
        let (mut new_player, start_normalize_scan_before_play) = self.build_video_player_for_open(
            target_idx,
            target_path.clone(),
            false,
            autoplay_override,
            ignore_resume,
            crate::video::VideoOutputConsumer::Presentation,
            None,
        );
        new_player.attach_native_output(native_output);
        let payload = new_player.build_switch_source_payload(source_epoch, show_preparing_overlay);
        new_player.switch_native_source(payload);

        self.fs_cache.insert(
            target_idx,
            FsCacheEntry::Video {
                player: Box::new(new_player),
                load_seq: self.input_seq,
            },
        );

        // T36 (Codex R-VNORM-001): 通常 open 経路は `app.rs::open_fullscreen → ...` が
        // `init_normalize_state_for_opened_video(target_idx)` を呼ぶが、fast-swap で
        // `fs_cache` に直接 insert した場合 `open_fullscreen` 内の cache-hit 分岐で
        // この初期化がスキップされ、ノーマライズ DB lookup + UI 状態セットが走らない。
        // 初期 gain は `build_video_player_for_open` で open 前に渡し、ここでは UI 状態と
        // 抑止状態を新しい動画に同期する。
        self.init_normalize_state_for_opened_video(target_idx);

        self.open_fullscreen(target_idx, history_trigger);
        if start_normalize_scan_before_play {
            if !self.start_normalize_scan_for_deferred_play_intent(target_idx) {
                self.resume_deferred_normalize_playback_without_scan(target_idx);
            }
        } else {
            self.maybe_start_normalize_scan_for_play_intent(target_idx);
        }
        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&target_idx) {
            crate::logger::log(format!(
                "[video-debug] post-swap state: idx={target_idx} engine_state={} seek_serial={} clock_is_playing={} pos={:.3} video_rx_len={} audio_rx_len={} pending_frames={}",
                player.engine_state_name(),
                player.current_seek_serial(),
                player.is_playing(),
                player.position(),
                player.video_rx_len(),
                player.audio_rx_len(),
                player.pending_frames()
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_presenter",
                    "post_swap_state",
                    None,
                    0,
                    &[
                        ("idx", serde_json::Value::from(target_idx as i64)),
                        (
                            "engine_state",
                            serde_json::Value::from(player.engine_state_name()),
                        ),
                        (
                            "seek_serial",
                            serde_json::Value::from(player.current_seek_serial() as i64),
                        ),
                        ("playing", serde_json::Value::from(player.is_playing())),
                        ("position", serde_json::Value::from(player.position())),
                        (
                            "video_rx_len",
                            serde_json::Value::from(player.video_rx_len() as i64),
                        ),
                        (
                            "audio_rx_len",
                            serde_json::Value::from(player.audio_rx_len() as i64),
                        ),
                        (
                            "pending_frames",
                            serde_json::Value::from(player.pending_frames() as i64),
                        ),
                    ],
                );
            }
        }
        if show_preparing_overlay {
            self.set_native_video_tile_preparing_overlay(target_idx);
        } else if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&target_idx) {
            player.set_native_tile_overlay(None);
        }
        self.sync_native_video_metadata(target_idx);
        self.sync_native_video_timeline_markers(target_idx);
        self.sync_native_video_vst3_available(target_idx);
        self.sync_native_video_vst3_panel(target_idx);

        // (旧 fs_cache.remove(&from_idx) はこの commit で take_native_output 直後に
        // 移動。詳細は上のコメント参照。)
        crate::logger::log(format!(
            "[native-video] fast source swap queued: reason={reason} from={from_idx} to={target_idx} epoch={source_epoch}"
        ));
        ctx.request_repaint();
        Some(NativeVideoSourceSwapStarted {
            from_idx,
            target_idx,
            target_path,
            source_epoch,
            started_at,
        })
    }

    #[cfg(windows)]
    pub(crate) fn try_start_video_tile_fast_swap(
        &mut self,
        ctx: &egui::Context,
        target_idx: usize,
    ) -> bool {
        // tile fast-swap も regular fast-swap と同じ video decode thread を spawn する
        // ため、`try_start_native_video_fast_swap` と同じ live count 上限で抑制する。
        // ただし throttle は「これは tile fast-swap の候補」と確定したあとに見る
        // (Codex P1 反映 — tile モード外や画像 target で誤発火しないように)。
        let Some(from_idx) = self.fullscreen_idx else {
            return false;
        };
        if from_idx == target_idx {
            return true;
        }
        if !self.video_tile_mode_active && self.video_tile_swap_pending.is_none() {
            return false;
        }
        if self.native_video_source_swap_pending.is_some() {
            return self.defer_native_video_source_swap_until_decoder_free(
                ctx,
                target_idx,
                Some(false),
                false,
                true,
                "tile",
                crate::app::HistoryTrigger::UserChosen,
            );
        }
        if self.video_tile_swap_pending.is_some() || self.native_video_fast_swap_pending.is_some() {
            return true;
        }
        // tile fast-swap は target/from が動画前提だが、念のため明示チェック。
        // `start_native_video_source_swap` 内でも同じチェックがあるが、ここで先に判定
        // しないと下の throttle が誤発火する。
        if !matches!(self.items.get(target_idx), Some(GridItem::Video(_)))
            || !matches!(self.items.get(from_idx), Some(GridItem::Video(_)))
        {
            return false;
        }

        // throttle 上限。詳細は `try_start_native_video_fast_swap` 側のコメント参照。
        let live_decoders = crate::video::decoder::LIVE_VIDEO_DECODE_THREADS
            .load(std::sync::atomic::Ordering::Acquire);
        let max_live_video_decode_threads = crate::video::decoder::MAX_LIVE_VIDEO_DECODE_THREADS;
        if live_decoders >= max_live_video_decode_threads {
            crate::logger::log(format!(
                "[native-video] fast tile swap throttled: live_video_decode_threads={live_decoders} max={max_live_video_decode_threads} target_idx={target_idx}"
            ));
            return self.defer_native_video_source_swap_until_decoder_free(
                ctx,
                target_idx,
                Some(false),
                false,
                true,
                "tile",
                crate::app::HistoryTrigger::UserChosen,
            );
        }

        let Some(started) = self.start_native_video_source_swap(
            ctx,
            target_idx,
            Some(false),
            false,
            true,
            "tile",
            crate::app::HistoryTrigger::UserChosen,
        ) else {
            return false;
        };

        if let Some(FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&target_idx) {
            player.set_playing(false);
        }
        self.video_tile_mode_active = true;
        self.video_tile_swap_pending = Some(VideoTileSwapPending {
            target_idx: started.target_idx,
            target_path: started.target_path,
            source_epoch: started.source_epoch,
            started_at: started.started_at,
            deadline: started.started_at + std::time::Duration::from_secs(2),
            parked_live_window_id: self.native_video_parked_live_input_window_id,
        });
        self.video_tile_reopen_pending = false;
        self.video_tile_reopen_deadline = None;
        crate::logger::log(format!(
            "[native-video] fast tile swap: from={} to={} epoch={}",
            started.from_idx, started.target_idx, started.source_epoch
        ));
        true
    }

    #[cfg(windows)]
    pub(crate) fn try_start_native_video_fast_swap(
        &mut self,
        ctx: &egui::Context,
        target_idx: usize,
        autoplay_override: Option<bool>,
        ignore_resume: bool,
        history_trigger: crate::app::HistoryTrigger,
    ) -> bool {
        // Gate checks first: 「これは本当に video→video fast-swap の候補か」が確定する
        // までは throttle 判定もしない。`live_count >= MAX` のときに throttle が早く
        // 走ると、動画→画像のような fast-swap 対象外の navigation まで握り潰してしまう
        // (Codex P1 反映)。
        if self.native_video_source_swap_pending.is_some() {
            return self.defer_native_video_source_swap_until_decoder_free(
                ctx,
                target_idx,
                autoplay_override,
                ignore_resume,
                false,
                "navigation",
                history_trigger,
            );
        }
        if self.native_video_fast_swap_pending.is_some() || self.video_tile_swap_pending.is_some() {
            return true;
        }
        if self.video_tile_mode_active {
            return false;
        }
        self.cancel_stale_video_tile_reopen(self.fullscreen_idx, "native-fast-swap");
        let Some(from_idx) = self.fullscreen_idx else {
            return false;
        };
        if from_idx == target_idx {
            return true;
        }
        // 動画→動画 fast-swap でないなら通常 open 経路に流す。`start_native_video_source_swap`
        // 内でも同じチェックがあるが、ここで先に判定しないと下の throttle が誤発火する。
        if !matches!(self.items.get(target_idx), Some(GridItem::Video(_)))
            || !matches!(self.items.get(from_idx), Some(GridItem::Video(_)))
        {
            return false;
        }

        // fast-swap 連射で HW decoder context (D3D11VA surface pool / video processor
        // slot) が短時間に重なり、新 video decode thread の `avcodec_send_packet` が
        // driver 内で永続待機する事象がある (2026-05-13 ログ調査)。`VideoPlayer::drop`
        // が cancel フラグを立てても、FFmpeg 内停止中の旧 thread は cancel を観測できず
        // 居座るので、`LIVE_VIDEO_DECODE_THREADS` が上限を超えていれば新 swap を抑制する。
        //
        // 上限は「現在再生中の 1 個」も含めた **総 live 数** でカウントしている。
        // 2026-05-15 の実機ログで、旧 decoder と新 decoder が重なっただけでも
        // D3D11VA / shared texture / keyed mutex 経路が秒単位に詰まり、最終的に
        // DXGI device removed へ進むことを確認した。mIV の fast-swap は decoder
        // create/drop と shared-output 回収が密に重なるため、安定性優先で
        // `MAX_LIVE_VIDEO_DECODE_THREADS=1` (= `>= 1` で抑制) とし、旧 decoder の
        // exit を待ってから次の HW decode を開始する。
        // UI thread をブロックする待ち合わせは禁止 (Codex 指摘 #1、2026-05-15)。
        // `>= MAX` のときは旧 player から NativeVideoOutput だけを退避し、
        // presenter HWND / DComp tree を表示したまま `NativeVideoSourceSwapPending` で
        // live=0 を待つ。normal open へ fallback すると旧 presenter が先に閉じ、
        // 新 presenter が出るまで背後のアプリや黒画面が 150-300ms 見えるため。
        // これにより「同時 HW decoder は 1 本」のまま、fullscreen を消さずに最新
        // target へ進められる。
        let live_decoders = crate::video::decoder::LIVE_VIDEO_DECODE_THREADS
            .load(std::sync::atomic::Ordering::Acquire);
        let max_live_video_decode_threads = crate::video::decoder::MAX_LIVE_VIDEO_DECODE_THREADS;
        if live_decoders >= max_live_video_decode_threads {
            crate::logger::log(format!(
                "[native-video] fast video swap throttled: live_video_decode_threads={live_decoders} max={max_live_video_decode_threads} target_idx={target_idx}"
            ));
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "native_video",
                    "fast_swap_throttled",
                    None,
                    0,
                    &[
                        (
                            "live_video_decode_threads",
                            serde_json::Value::from(live_decoders as i64),
                        ),
                        (
                            "max",
                            serde_json::Value::from(max_live_video_decode_threads as i64),
                        ),
                        ("target_idx", serde_json::Value::from(target_idx as i64)),
                    ],
                );
            }
            return self.defer_native_video_source_swap_until_decoder_free(
                ctx,
                target_idx,
                autoplay_override,
                ignore_resume,
                false,
                "navigation",
                history_trigger,
            );
        }

        // Even when the decoder slot is free, do not create/drop a decoder for every
        // intermediate wheel target. Rapid navigation can still create hundreds of
        // D3D11VA decoders and shared-output textures per minute with only one live
        // decoder at a time, which is enough to hit driver/resource pressure. Keep
        // the existing native presenter visible and coalesce wheel movement; the
        // pending target is updated by `defer_native_video_source_swap_until_decoder_free`.
        self.defer_native_video_source_swap_until_decoder_free(
            ctx,
            target_idx,
            autoplay_override,
            ignore_resume,
            false,
            "navigation",
            history_trigger,
        )
    }

    #[cfg(windows)]
    pub(super) fn open_native_video_fullscreen_from_navigation(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        history_trigger: crate::app::HistoryTrigger,
    ) {
        self.open_native_video_fullscreen_from_navigation_with_options(
            ctx,
            idx,
            None,
            false,
            history_trigger,
        );
    }

    #[cfg(windows)]
    pub(super) fn open_native_video_fullscreen_from_navigation_with_options(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        autoplay_override: Option<bool>,
        ignore_resume: bool,
        history_trigger: crate::app::HistoryTrigger,
    ) {
        self.sync_main_selection_from_viewer_idx(idx);

        // 7e: VST ホスト表示中に native ナビ (native HUD の前後ファイル / NavigateItem / wheel) で別
        // 動画へ移るとき、下の fast-swap が presenter を再利用 / `take_native_output` で破棄する前に
        // VST ホストを畳む。さもないと VST editor / topmost / owner が dying/再利用 presenter に残る
        // (Codex P1)。**ただし exit_video_audio_vst は presenter を re-hide してしまう** — この native
        // 経路は直後に**通常動画**の fast-swap が同 presenter を再利用して次動画を**映像表示**するので、
        // re-hide すると次動画が hidden presenter で開いて真っ黒になる (Codex P1 v2)。よって GUI/owner
        // だけ畳んで presenter は**可視のまま**にし、音声モードも抜ける (open_fullscreen 完了時にも
        // None 化されるが、swap 中フレームで音楽ビュー扱いにならないよう先に落とす)。
        if let Some(cur) = self.fullscreen_idx
            && self.video_audio_vst_active_for(cur)
        {
            self.teardown_video_audio_vst_gui();
            self.video_audio_vst = None;
            self.video_audio_mode = None;
            self.video_audio_mode_entry_target = None;
            self.video_audio_exit_pending = None;
            crate::logger::log(format!(
                "[video-audio-vst] left VST host for native video nav (presenter kept visible) fs_idx={cur}"
            ));
        }

        // fast-swap / tile-swap は presenter を再利用し、確定は deferred source-swap 完了時の
        // open_fullscreen で行う。presentation 維持 (「別ウィンドウで開く」設定を手動ナビで再適用
        // しない) は、その完了時 open_fullscreen 冒頭の一括ガードが現在の viewer_presentation から
        // 焼き付けるので、ここでは live one-shot を clear して、この関数がすぐ return した後に
        // auto-advance の明示 one-shot 等が別 open へ漏れるのを防ぐだけにする。
        if self.try_start_video_tile_fast_swap(ctx, idx) {
            self.fs_media_open_forced_presentation = None;
            return;
        }
        if self.try_start_native_video_fast_swap(
            ctx,
            idx,
            autoplay_override,
            ignore_resume,
            history_trigger,
        ) {
            self.fs_media_open_forced_presentation = None;
            return;
        }
        let started = std::time::Instant::now();
        let from_idx = self.fullscreen_idx;
        let restore_video_tile = self.video_tile_mode_active;
        let restore_target_is_video = matches!(self.items.get(idx), Some(GridItem::Video(_)));
        crate::logger::log(format!(
            "[native-video] wheel navigation open: from={from_idx:?} to={idx} tile_restore={restore_video_tile} target_video={restore_target_is_video}"
        ));
        if restore_video_tile {
            self.video_tile_state = None;
            self.video_tile_swap_pending = None;
            if let Some(current_idx) = self.fullscreen_idx {
                self.set_native_video_tile_preparing_overlay(current_idx);
            }
            if restore_target_is_video {
                self.video_tile_mode_active = true;
                self.video_tile_reopen_pending = true;
                self.video_tile_reopen_deadline =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(3));
            } else {
                self.video_tile_mode_active = false;
                self.video_tile_reopen_pending = false;
                self.video_tile_reopen_deadline = None;
            }
        } else {
            self.cancel_stale_video_tile_reopen(from_idx, "wheel-open");
        }

        self.fs_video_open_autoplay_override = autoplay_override;
        self.fs_video_open_ignore_resume_once = ignore_resume;
        let cursor_state = self.fullscreen_cursor_state();
        self.open_fullscreen(idx, history_trigger);
        self.restore_fullscreen_cursor_state(ctx, cursor_state);

        if restore_video_tile && restore_target_is_video {
            self.set_native_video_tile_preparing_overlay(idx);
        } else if restore_video_tile {
            self.video_tile_mode_active = false;
            self.video_tile_reopen_pending = false;
            self.video_tile_reopen_deadline = None;
        }
        crate::logger::log(format!(
            "[native-video] wheel navigation open queued: to={idx} elapsed_ms={:.1}",
            started.elapsed().as_secs_f64() * 1000.0
        ));
        ctx.request_repaint();
    }

    #[cfg(windows)]
    pub(super) fn handle_native_video_mouse_button(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        event: crate::video::native_window::NativeVideoMouseButtonEvent,
    ) {
        use crate::video::native_window::NativeVideoMouseButton;

        self.mark_native_video_hud_activity(ctx);
        if event.button == NativeVideoMouseButton::Right && !event.double_click {
            let pos = egui::pos2(event.x as f32, event.y as f32);
            if self.fs_context_menu_idx.is_some() {
                // The presenter receives native input behind the egui menu. Consume both
                // press and release: letting the press through starts a ring/gesture whose
                // release is then swallowed, leaving a stuck interaction. A new right press
                // dismisses the menu only, matching the existing left-click dismissal.
                self.native_video_secondary_press_start = None;
                if event.down {
                    self.fs_context_menu_idx = None;
                    self.cached_handlers = None;
                    ctx.request_repaint();
                }
                return;
            }
            if event.down {
                self.native_video_last_move_client = Some((event.x, event.y));
                match self
                    .settings
                    .ring_shortcuts
                    .right_drag_mode(crate::ring_shortcut::RightDragContext::VideoFullscreen)
                {
                    crate::ring_shortcut::RightDragMode::RingShortcut => self
                        .start_mouse_ring_flick(
                            ctx,
                            crate::ring_shortcut::RingShortcutContext::VideoFullscreen,
                            pos,
                            None,
                        ),
                    crate::ring_shortcut::RightDragMode::MouseGesture => self.start_mouse_gesture(
                        ctx,
                        crate::ring_shortcut::RightDragContext::VideoFullscreen,
                        pos,
                        None,
                    ),
                    crate::ring_shortcut::RightDragMode::Disabled
                    | crate::ring_shortcut::RightDragMode::Unknown(_) => {
                        self.native_video_secondary_press_start =
                            Some((std::time::Instant::now(), pos));
                        ctx.request_repaint_after(std::time::Duration::from_millis(400));
                    }
                }
                return;
            }
            if self.mouse_ring_flick.is_some() {
                let outcome = self.update_native_mouse_ring_flick(
                    ctx,
                    crate::ring_shortcut::RingShortcutContext::VideoFullscreen,
                    pos,
                    false,
                    true,
                );
                if matches!(
                    outcome,
                    crate::ring_shortcut::MouseFlickOutcome::Fired
                        | crate::ring_shortcut::MouseFlickOutcome::Cancelled
                ) {
                    return;
                }
                if matches!(outcome, crate::ring_shortcut::MouseFlickOutcome::ShortTap) {
                    self.handle_native_video_short_right_click(ctx, fs_idx, pos);
                    return;
                }
            }
            if self.mouse_gesture.is_some() {
                let outcome = self.update_native_mouse_gesture(
                    ctx,
                    crate::ring_shortcut::RightDragContext::VideoFullscreen,
                    pos,
                    false,
                    true,
                );
                if matches!(
                    outcome,
                    crate::ring_shortcut::MouseFlickOutcome::Fired
                        | crate::ring_shortcut::MouseFlickOutcome::Cancelled
                ) {
                    return;
                }
                if matches!(outcome, crate::ring_shortcut::MouseFlickOutcome::ShortTap) {
                    self.handle_native_video_short_right_click(ctx, fs_idx, pos);
                    return;
                }
            }
            if let Some((start_time, start_pos)) = self.native_video_secondary_press_start.take() {
                if pos.distance(start_pos) < 20.0 {
                    if start_time.elapsed() >= std::time::Duration::from_millis(400) {
                        self.fs_context_menu_idx = Some(fs_idx);
                        self.fs_context_menu_pos = pos;
                        ctx.request_repaint();
                    } else {
                        self.handle_native_video_short_right_click(ctx, fs_idx, pos);
                    }
                }
                return;
            }
            self.handle_native_video_short_right_click(ctx, fs_idx, pos);
            return;
        }
        if !event.double_click && event.down {
            match event.button {
                NativeVideoMouseButton::Extra1 => {
                    self.native_video_pointer_down = None;
                    self.mouse_ring_nav = self.apply_mouse_back_forward_button(
                        ctx,
                        false,
                        crate::app::ActionSurface::Viewer,
                        "native-video-mouse",
                    );
                    return;
                }
                NativeVideoMouseButton::Extra2 => {
                    self.native_video_pointer_down = None;
                    self.mouse_ring_nav = self.apply_mouse_back_forward_button(
                        ctx,
                        true,
                        crate::app::ActionSurface::Viewer,
                        "native-video-mouse",
                    );
                    return;
                }
                _ => {}
            }
        }
        if event.button == NativeVideoMouseButton::Middle {
            self.native_video_pointer_down = None;
            if event.double_click {
                self.native_video_middle_press_start = None;
                return;
            }
            if event.down {
                self.native_video_middle_press_start = Some(super::NativeVideoMiddlePressStart {
                    fs_idx,
                    x: event.x,
                    y: event.y,
                    at: std::time::Instant::now(),
                });
                return;
            }
            let Some(start) = self.native_video_middle_press_start.take() else {
                return;
            };
            if start.fs_idx != fs_idx {
                return;
            }
            let dx = event.x - start.x;
            let dy = event.y - start.y;
            let moved_sq = dx.saturating_mul(dx) + dy.saturating_mul(dy);
            let threshold = crate::ui_fullscreen::MIDDLE_DRAG_THRESHOLD_PX.ceil() as i32;
            let click_like = moved_sq <= threshold.saturating_mul(threshold)
                && start.at.elapsed() <= std::time::Duration::from_millis(500);
            if click_like {
                self.mouse_ring_nav = self.apply_mouse_button(
                    ctx,
                    crate::ring_shortcut::MouseButtonSlot::Middle,
                    crate::app::ActionSurface::Viewer,
                    "native-video-middle-mouse",
                );
                ctx.request_repaint();
            }
            return;
        }
        if event.button != NativeVideoMouseButton::Left {
            return;
        }

        if self.fs_context_menu_idx.is_some() {
            // egui fallback menu の外側を動画上でクリックした場合。その click は menu を
            // 閉じるためだけの入力であり、背面 presenter の再生トグルへ渡さない。
            self.native_video_pointer_down = None;
            self.native_video_context_menu_dismiss_click_started_at = None;
            if event.down {
                self.fs_context_menu_idx = None;
                self.cached_handlers = None;
                ctx.request_repaint();
            }
            return;
        }

        if let Some(started_at) = self.native_video_context_menu_dismiss_click_started_at {
            // Win32 TrackPopupMenuEx を閉じたクリックは presenter の queue に遅れて届く。
            // 500ms は click sequence の相関上限で、時間経過による挙動変更ではない。
            if started_at.elapsed() <= std::time::Duration::from_millis(500) {
                self.native_video_pointer_down = None;
                if !event.down {
                    self.native_video_context_menu_dismiss_click_started_at = None;
                }
                return;
            }
            self.native_video_context_menu_dismiss_click_started_at = None;
        }

        if event.double_click {
            self.native_video_pointer_down = None;
            return;
        }

        if event.down {
            self.native_video_pointer_down = Some(NativeVideoPointerDown {
                fs_idx,
                x: event.x,
                y: event.y,
                at: std::time::Instant::now(),
            });
            return;
        }

        let Some(start) = self.native_video_pointer_down.take() else {
            return;
        };
        if start.fs_idx != fs_idx {
            return;
        }
        let dx = event.x - start.x;
        let dy = event.y - start.y;
        let moved_sq = dx.saturating_mul(dx) + dy.saturating_mul(dy);
        let click_like =
            moved_sq <= 36 && start.at.elapsed() <= std::time::Duration::from_millis(500);
        if !click_like || self.settings.vst3_gui_visible {
            return;
        }
        self.handle_native_video_toggle_play_command(ctx, fs_idx);
    }

    #[cfg(windows)]
    fn handle_native_video_short_right_click(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        pos: egui::Pos2,
    ) {
        if self.apply_viewer_short_right_click_action(
            crate::ring_shortcut::RightDragContext::VideoFullscreen,
            Some(fs_idx),
            pos,
        ) {
            self.handle_fullscreen_close_request_immediate();
        }
        ctx.request_repaint();
    }

    #[cfg(windows)]
    pub(crate) fn maybe_open_native_video_secondary_long_press_menu(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) {
        if self
            .settings
            .ring_shortcuts
            .mouse_ring_enabled(crate::ring_shortcut::RingShortcutContext::VideoFullscreen)
            || self.fs_context_menu_idx.is_some()
        {
            return;
        }
        let Some((start_time, pos)) = self.native_video_secondary_press_start else {
            return;
        };
        let current_pos = self
            .native_video_last_move_client
            .map(|(x, y)| egui::pos2(x as f32, y as f32))
            .unwrap_or(pos);
        if current_pos.distance(pos) >= 20.0 {
            return;
        }
        let remaining = std::time::Duration::from_millis(400).saturating_sub(start_time.elapsed());
        if remaining.is_zero() {
            self.native_video_secondary_press_start = None;
            self.fs_context_menu_idx = Some(fs_idx);
            self.fs_context_menu_pos = pos;
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(remaining);
        }
    }

    #[cfg(windows)]
    pub(crate) fn mark_native_video_hud_activity(&mut self, ctx: &egui::Context) {
        let now = std::time::Instant::now();
        // ネイティブビデオウィンドウの pointer 入力は eframe フルスクリーンビューポートの
        // input には現れないため、カーソル auto-hide のアクティビティタイマもここで更新する。
        // キー操作はカーソルを再表示しないので `request_native_video_hud_repaint` を使う。
        self.cursor_last_activity = Some(now);
        self.cursor_hidden = false;
        // eframe 経由の pointer 活動は native presenter HWND の `push_native_event` を
        // 経由しないことがあるため、current の player に明示的に伝搬する。
        if let Some(idx) = self.fullscreen_idx
            && let Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) =
                self.fs_cache.get(&idx)
        {
            player.mark_cursor_activity();
        }
        ctx.request_repaint();
    }

    #[cfg(windows)]
    pub(crate) fn request_native_video_hud_repaint(&mut self, ctx: &egui::Context) {
        if let Some(idx) = self.fullscreen_idx
            && let Some(crate::fs_animation::FsCacheEntry::Video { player, .. }) =
                self.fs_cache.get(&idx)
        {
            player.request_native_overlay_render();
        }
        ctx.request_repaint();
    }
}

#[cfg(all(test, windows))]
mod iconic_thumbnail_tests {
    use super::main_iconic_video_source_enabled;

    #[test]
    fn main_iconic_video_source_is_only_for_in_main_video() {
        assert!(main_iconic_video_source_enabled(false, true, false));
        assert!(!main_iconic_video_source_enabled(true, true, false));
        assert!(!main_iconic_video_source_enabled(false, false, false));
        assert!(!main_iconic_video_source_enabled(false, true, true));
    }
}
