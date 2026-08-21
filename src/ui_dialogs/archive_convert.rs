//! RAR / 7z / LZH → ZIP 変換の確認・進捗ダイアログ。
//!
//! フロー:
//!   1. グリッドで RAR / 7z / LZH をクリック → `App::request_archive_convert` が
//!      `ArchiveConvertState::Scanning` に遷移し、バックグラウンドで画像エントリを数える。
//!   2. スキャン完了 → `Confirm` フェーズに遷移し、画像数・サイズ見積もりを表示。
//!   3. [ 変換して開く ] → `Converting` フェーズに遷移、変換ワーカーを spawn。
//!   4. 完了 → キャッシュ DB に記録し、`pending_post_convert_nav` にキャッシュ ZIP パスを
//!      セット → 次フレームで通常の ZIP として開く。
//!
//! キャンセルは `Arc<AtomicBool>` を立ててワーカーにシグナルする。ワーカーは
//! 各エントリ境界で検査する。

#![allow(unused_imports)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;

use eframe::egui;

use crate::app::App;
use crate::archive_cache::ArchiveCacheDb;
use crate::archive_converter::{
    ArchiveFormat, ArchiveImageSummary, ConvertError, ConvertOptions, ConvertProgress,
    convert_to_zip_with_password, scan_summary_with_password_cancelable,
};

// ──────────────────────────────────────────────────────────────────────
// ステート型
// ──────────────────────────────────────────────────────────────────────

/// スキャン完了 / 変換完了通知用メッセージ。
pub(crate) enum ArchiveConvertMsg {
    ScanDone(Result<(ArchiveImageSummary, bool, PathBuf), ConvertError>),
    /// 変換完了。Ok なら (summary, cached_zip_path, cached_zip_size)
    ConvertDone(Result<(ArchiveImageSummary, PathBuf, i64), ConvertError>),
    SiblingConvertDone(Result<(ArchiveImageSummary, PathBuf, i64), ConvertError>),
}

/// What may happen after this archive state reaches a readable backing archive.
///
/// Bookmark and detached-grid variants carry the same request owner used at dispatch. Keeping
/// this as one typed policy prevents a conversion result from falling back to unowned navigation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ArchiveConvertCompletionPolicy {
    Navigation,
    Bookmark(crate::bookmark_browser::BookmarkOpenRequestOwner),
    DetachedGridArchive(crate::app::DetachedGridArchiveOpenRequestOwner),
    SiblingZip,
}

impl ArchiveConvertCompletionPolicy {
    fn is_sibling_zip(&self) -> bool {
        matches!(self, Self::SiblingZip)
    }

    pub(crate) fn bookmark_owner(
        &self,
    ) -> Option<&crate::bookmark_browser::BookmarkOpenRequestOwner> {
        match self {
            Self::Bookmark(owner) => Some(owner),
            Self::Navigation | Self::DetachedGridArchive(_) | Self::SiblingZip => None,
        }
    }

    pub(crate) fn detached_grid_archive_owner(
        &self,
    ) -> Option<&crate::app::DetachedGridArchiveOpenRequestOwner> {
        match self {
            Self::DetachedGridArchive(owner) => Some(owner),
            Self::Navigation | Self::Bookmark(_) | Self::SiblingZip => None,
        }
    }

    fn open_owner(&self) -> Option<crate::app::OpenRequestOwner> {
        match self {
            Self::Navigation => Some(crate::app::OpenRequestOwner::Navigation),
            Self::Bookmark(owner) => Some(crate::app::OpenRequestOwner::Bookmark(owner.clone())),
            Self::DetachedGridArchive(_) | Self::SiblingZip => None,
        }
    }
}

/// 進捗の共有ハンドル。変換ワーカーが書き、UI スレッドが読む。
pub(crate) struct ArchiveConvertProgressShared {
    pub files_done: AtomicU64,
    pub files_total: AtomicU64,
    pub bytes_written: AtomicU64,
}

impl ArchiveConvertProgressShared {
    pub fn new() -> Self {
        Self {
            files_done: AtomicU64::new(0),
            files_total: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
        }
    }
}

/// 変換ダイアログのフェーズ。
pub(crate) enum ArchiveConvertPhase {
    /// 事前スキャン中 (画像数カウント)
    Scanning,
    /// 変換層が再試行可能と判定したパスワード入力待ち
    PasswordRequired {
        message: Option<String>,
        resume: ArchivePasswordResume,
    },
    /// スキャン完了、ユーザーの確認待ち
    Confirm { summary: ArchiveImageSummary },
    /// 変換実行中
    Converting {
        progress: Arc<ArchiveConvertProgressShared>,
    },
    /// エラー (ユーザーが閉じるまで表示)
    Error { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchivePasswordResume {
    Scan,
    Convert,
}

#[derive(Clone, Debug)]
pub(crate) struct ArchiveConvertDeferredFullscreen {
    pub reopen: crate::app::DeferredFsReopen,
    pub restore_video_tile: bool,
}

pub(crate) struct ArchiveConvertState {
    pub src_path: PathBuf,
    /// Correlation id owned by the pointer/open request that created this lifecycle.
    pub input_seq: u64,
    pub format: ArchiveFormat,
    pub password: Option<String>,
    pub password_input: String,
    pub phase: ArchiveConvertPhase,
    /// One cancellation owner for the entire scan / password retry / conversion lifecycle.
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<ArchiveConvertMsg>,
    /// 変換完了後にメイン UI がナビゲーションに使うキャッシュ ZIP パス。
    /// `update()` が毎フレーム見に行き、Some なら `load_folder` を呼んでクリアする。
    pub pending_nav: Option<PathBuf>,
    /// Direct-readable RAR path waiting to enter the ZipImage-compatible viewer.
    pub pending_direct_nav: Option<PathBuf>,
    /// Probe flat non-solid RAR before falling back to conversion/cache.
    pub allow_direct_read: bool,
    /// Existing conversion cache, used only when the direct-read probe rejects the RAR.
    pub fallback_cached_zip: Option<PathBuf>,
    pub completion: ArchiveConvertCompletionPolicy,
    pub pending_sibling_output: Option<PathBuf>,
    /// 履歴の戻る/進むから未変換アーカイブに入ろうとしてダイアログが出た場合、
    /// キャンセル時に戻る/進むスタックをクリック前へ戻すためのスナップショット。
    pub nav_history_rollback: Option<crate::app::FolderNavHistorySnapshot>,
    /// この変換完了後に 1 ページ目を自動フルスクリーン表示するか。明示的なオープン
    /// (グリッド Enter / ダブルクリック / ゲームパッド × 設定 ON) のときだけ true。
    /// キャンセル時は state ごと drop されるので stale フラグが残らない。
    pub auto_fullscreen: bool,
    /// Ctrl+↑↓ 等のフルスクリーン横断ナビ中に、確認なしの自動変換を挟んだ場合の
    /// 復帰予約。確認ダイアログ / パスワード入力 / エラーに入ったら破棄する。
    pub deferred_fullscreen: Option<ArchiveConvertDeferredFullscreen>,
    /// true の場合、Scanning のウィンドウを出さず、Confirm を自動通過する。
    /// 変換中の進捗、パスワード入力、エラーは表示する。
    pub suppress_confirm: bool,
    /// Confirm 画面の「次回から表示しない」。変換開始時に設定へ反映する。
    pub suppress_confirm_next_time: bool,
}

impl Drop for ArchiveConvertState {
    fn drop(&mut self) {
        // The state owns every worker in this archive-open lifecycle. Dropping the receiver makes
        // late results inert; setting the same token first also stops scan/convert work itself.
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn spawn_archive_scan(
    src: PathBuf,
    format: ArchiveFormat,
    password: Option<String>,
    allow_direct_read: bool,
    input_seq: u64,
    cancel: Arc<AtomicBool>,
) -> mpsc::Receiver<ArchiveConvertMsg> {
    spawn_archive_scan_task(cancel, move |cancel| {
        if allow_direct_read && format == ArchiveFormat::Rar && password.is_none() {
            match crate::rar_loader::inspect_for_direct_read_cancelable_traced(
                &src,
                cancel,
                crate::rar_loader::RarInspectionOrigin::ExplicitOpen,
                input_seq,
            ) {
                Ok(inspection) => Ok((
                    inspection.summary,
                    inspection.decision == crate::rar_loader::RarDirectReadDecision::Direct,
                    inspection.resolved_path,
                )),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                    Err(ConvertError::Cancelled)
                }
                Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                    scan_summary_with_password_cancelable(&src, format, None, cancel)
                        .map(|summary| (summary, false, src.clone()))
                }
                Err(error) => Err(ConvertError::Archive(error.to_string())),
            }
        } else {
            scan_summary_with_password_cancelable(&src, format, password.as_deref(), cancel)
                .map(|summary| (summary, false, src.clone()))
        }
    })
}

fn spawn_archive_scan_task<F>(cancel: Arc<AtomicBool>, task: F) -> mpsc::Receiver<ArchiveConvertMsg>
where
    F: FnOnce(&AtomicBool) -> Result<(ArchiveImageSummary, bool, PathBuf), ConvertError>
        + Send
        + 'static,
{
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = task(&cancel);
        let _ = tx.send(ArchiveConvertMsg::ScanDone(result));
    });
    rx
}

fn prepare_archive_password_retry(
    state: &mut ArchiveConvertState,
) -> Option<(ArchivePasswordResume, String)> {
    let password = state.password_input.trim().to_string();
    if password.is_empty() {
        return None;
    }
    let resume = match &state.phase {
        ArchiveConvertPhase::PasswordRequired { resume, .. } => *resume,
        _ => ArchivePasswordResume::Scan,
    };
    state.password = Some(password.clone());
    state.password_input.clear();
    Some((resume, password))
}

fn archive_convert_window_suppressed(
    phase: &ArchiveConvertPhase,
    suppress_confirm: bool,
    allow_direct_read: bool,
) -> bool {
    (suppress_confirm || allow_direct_read) && matches!(phase, ArchiveConvertPhase::Scanning)
}

fn sibling_zip_path(src: &std::path::Path) -> PathBuf {
    src.with_extension("zip")
}

// ──────────────────────────────────────────────────────────────────────
// App 側 API
// ──────────────────────────────────────────────────────────────────────

impl App {
    /// 有効なキャッシュがあれば ZIP パスを返す。無効 / 未変換なら None。
    pub(crate) fn try_archive_cache_lookup(&self, src: &std::path::Path) -> Option<PathBuf> {
        let db = self.archive_cache_db.as_ref()?;
        let meta = std::fs::metadata(src).ok()?;
        let mtime = crate::ui_helpers::mtime_secs(&meta);
        let size = meta.len() as i64;
        db.lookup(src, mtime, size)
    }

    /// 変換済みアーカイブを開く。`src` は元 (RAR/7z/LZH)、`cached_zip` は
    /// 変換済み ZIP のパス。キャッシュ ZIP を load_folder し、その後
    /// `archive_source_override` / `address` を元パスに書き戻す。
    ///
    /// Enter / ダブルクリックのキャッシュヒット経路で使う。変換直後の
    /// pending_nav 経路は `show_archive_convert_dialog` 内で直接処理する
    /// (そちらは `archive_convert` のライフサイクルと絡むため)。
    /// `auto_fullscreen` は **明示的なオープン** (グリッド Enter / ダブルクリック /
    /// ゲームパッド / 起動引数・SendTo × 設定 ON) のときだけ true。履歴の戻る/進む・アドレスバー経由の
    /// `load_folder_or_convert_archive` からは false で呼び、ZIP/PDF と挙動を揃える
    /// (ZIP/PDF も明示オープン時のみ自動フルスクリーン)。`load_folder(cache_zip)` →
    /// `load_zip_as_folder` が同フレームで `pending_auto_fs_open` を消化するので stale 化しない。
    pub(crate) fn open_archive_via_cache(
        &mut self,
        src: PathBuf,
        cached_zip: PathBuf,
        auto_fullscreen: bool,
    ) -> bool {
        self.open_archive_via_cache_owned(
            src,
            cached_zip,
            auto_fullscreen,
            crate::app::OpenRequestOwner::Navigation,
        )
    }

    pub(crate) fn open_archive_via_cache_owned(
        &mut self,
        src: PathBuf,
        cached_zip: PathBuf,
        auto_fullscreen: bool,
        owner: crate::app::OpenRequestOwner,
    ) -> bool {
        if let crate::app::OpenRequestOwner::DetachedGridArchive(detached_owner) = &owner {
            crate::logger::log(format!(
                "[detached-grid-archive] reject main cache navigation id={} source={} cache={}",
                detached_owner.request_id,
                src.display(),
                cached_zip.display()
            ));
            return false;
        }
        if let crate::app::OpenRequestOwner::Bookmark(bookmark_owner) = &owner
            && !self.bookmark_open_owner_is_current(bookmark_owner)
        {
            return false;
        }
        if auto_fullscreen {
            self.pending_auto_fs_open = true;
        }
        // load_folder(cache_zip) は load_folder_with_scan のクリアで履歴 / ブックマーク一覧の
        // 戻り先予約を落とす (cache_zip != 元アーカイブのため)。元アーカイブから開いた
        // viewer なら、override と同じく予約も元パスへ復元する。
        let restore_reading_history = self
            .reading_history_return_from
            .as_ref()
            .is_some_and(|from| crate::folder_tree::path_eq(from, &src));
        let restore_bookmark_view = self
            .bookmark_view_state
            .as_ref()
            .filter(|state| {
                state
                    .target()
                    .is_some_and(|target| target.matches_loaded_container(&src))
            })
            .cloned();
        self.authorize_smart_folder_session_alias(&src, &cached_zip);
        self.load_folder_with_scan_owned(cached_zip.clone(), None, owner);
        // load が ★固定 (snapshot lock) の範囲外ガード等でブロックされると current_folder は
        // 変わらない (load_zip_as_folder が current_folder = cache_zip を同期セットする前に
        // return するため)。その場合は override / address / recent を更新しない
        // (current_folder は元の場所のまま override だけ範囲外アーカイブを指す不整合を防ぐ、
        // Codex P1)。戻り値でブロックを呼び出し側にも伝える。
        if !self
            .current_folder
            .as_ref()
            .is_some_and(|cur| crate::folder_tree::path_eq(cur, &cached_zip))
        {
            return false;
        }
        self.address = src.to_string_lossy().to_string();
        // 検索 (Ctrl+G / Ctrl+S) 中は recent_folders を一切変更しない
        // (remember_recent_folder 自体もガード済みだが、retain も検索中は走らせない)。
        if !(self.global_search.active || self.favsearch.active) {
            self.forget_recent_folder(&cached_zip);
            self.remember_recent_folder(&src);
        }
        self.update_active_quick_folder_target(&src);
        if restore_reading_history {
            self.reading_history_return_from = Some(src.clone());
        }
        if let Some(state) = restore_bookmark_view {
            self.bookmark_view_state = Some(state);
        }
        self.archive_source_override = Some(src);
        true
    }

    /// 変換ダイアログを開始する (スキャン fase から)。
    /// 既に別のダイアログが動作中なら無視 (二重起動防止)。
    pub(crate) fn request_archive_convert(
        &mut self,
        src: PathBuf,
        format: ArchiveFormat,
        auto_fullscreen: bool,
    ) -> bool {
        self.request_archive_convert_owned(
            src,
            format,
            auto_fullscreen,
            crate::app::OpenRequestOwner::Navigation,
        )
    }

    pub(crate) fn request_archive_convert_owned(
        &mut self,
        src: PathBuf,
        format: ArchiveFormat,
        auto_fullscreen: bool,
        owner: crate::app::OpenRequestOwner,
    ) -> bool {
        if self.settings.archive_file_handling_ignores_convertible() {
            return false;
        }
        if self.archive_convert.is_some() || self.batch_convert.is_some() {
            return false;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = spawn_archive_scan(
            src.clone(),
            format,
            None,
            false,
            self.input_seq,
            Arc::clone(&cancel),
        );
        let suppress_confirm = self.settings.archive_convert_suppresses_confirm();
        self.archive_convert = Some(ArchiveConvertState {
            src_path: src,
            input_seq: self.input_seq,
            format,
            password: None,
            password_input: String::new(),
            phase: ArchiveConvertPhase::Scanning,
            cancel,
            rx,
            pending_nav: None,
            pending_direct_nav: None,
            allow_direct_read: false,
            fallback_cached_zip: None,
            completion: match owner {
                crate::app::OpenRequestOwner::Navigation => {
                    ArchiveConvertCompletionPolicy::Navigation
                }
                crate::app::OpenRequestOwner::Bookmark(owner) => {
                    ArchiveConvertCompletionPolicy::Bookmark(owner)
                }
                crate::app::OpenRequestOwner::DetachedGridArchive(owner) => {
                    ArchiveConvertCompletionPolicy::DetachedGridArchive(owner)
                }
            },
            pending_sibling_output: None,
            nav_history_rollback: None,
            auto_fullscreen,
            deferred_fullscreen: None,
            suppress_confirm,
            suppress_confirm_next_time: false,
        });
        true
    }

    /// Probe a RAR on the scan worker and open it directly when eligible.
    /// Rejected RARs continue through the unchanged conversion-cache flow.
    pub(crate) fn request_rar_open_owned(
        &mut self,
        src: PathBuf,
        auto_fullscreen: bool,
        fallback_cached_zip: Option<PathBuf>,
        owner: crate::app::OpenRequestOwner,
    ) -> bool {
        if self.settings.archive_file_handling_ignores_convertible()
            || self.archive_convert.is_some()
            || self.batch_convert.is_some()
        {
            return false;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = spawn_archive_scan(
            src.clone(),
            ArchiveFormat::Rar,
            None,
            true,
            self.input_seq,
            Arc::clone(&cancel),
        );
        self.archive_convert = Some(ArchiveConvertState {
            src_path: src,
            input_seq: self.input_seq,
            format: ArchiveFormat::Rar,
            password: None,
            password_input: String::new(),
            phase: ArchiveConvertPhase::Scanning,
            cancel,
            rx,
            pending_nav: None,
            pending_direct_nav: None,
            allow_direct_read: true,
            fallback_cached_zip,
            completion: match owner {
                crate::app::OpenRequestOwner::Navigation => {
                    ArchiveConvertCompletionPolicy::Navigation
                }
                crate::app::OpenRequestOwner::Bookmark(owner) => {
                    ArchiveConvertCompletionPolicy::Bookmark(owner)
                }
                crate::app::OpenRequestOwner::DetachedGridArchive(owner) => {
                    ArchiveConvertCompletionPolicy::DetachedGridArchive(owner)
                }
            },
            pending_sibling_output: None,
            nav_history_rollback: None,
            auto_fullscreen,
            deferred_fullscreen: None,
            suppress_confirm: self.settings.archive_convert_suppresses_confirm(),
            suppress_confirm_next_time: false,
        });
        true
    }

    pub(crate) fn request_explicit_zip_convert(
        &mut self,
        src: PathBuf,
        format: ArchiveFormat,
    ) -> bool {
        if self.archive_convert.is_some() || format == ArchiveFormat::Zip {
            return false;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = spawn_archive_scan(
            src.clone(),
            format,
            None,
            false,
            self.input_seq,
            Arc::clone(&cancel),
        );
        self.archive_convert = Some(ArchiveConvertState {
            src_path: src,
            input_seq: self.input_seq,
            format,
            password: None,
            password_input: String::new(),
            phase: ArchiveConvertPhase::Scanning,
            cancel,
            rx,
            pending_nav: None,
            pending_direct_nav: None,
            allow_direct_read: false,
            fallback_cached_zip: None,
            completion: ArchiveConvertCompletionPolicy::SiblingZip,
            pending_sibling_output: None,
            nav_history_rollback: None,
            auto_fullscreen: false,
            deferred_fullscreen: None,
            suppress_confirm: false,
            suppress_confirm_next_time: false,
        });
        true
    }

    pub(crate) fn archive_convert_deferred_fullscreen_active(&self) -> bool {
        self.archive_convert
            .as_ref()
            .and_then(|state| state.deferred_fullscreen.as_ref())
            .is_some()
    }

    /// 背面入力を止める実ダイアログが現在表示されるか。
    ///
    /// 確認なし変換の走査中は state 自体は存在するが Window は描画しないため、
    /// `archive_convert.is_some()` だけで判定すると変換中の一覧操作まで塞いでしまう。
    pub(crate) fn archive_convert_dialog_visible(&self) -> bool {
        self.archive_convert.as_ref().is_some_and(|state| {
            !archive_convert_window_suppressed(
                &state.phase,
                state.suppress_confirm,
                state.allow_direct_read,
            )
        })
    }

    pub(crate) fn attach_archive_convert_deferred_fullscreen(
        &mut self,
        restore_video_tile: bool,
        resume_slideshow: bool,
        history_trigger: crate::app::HistoryTrigger,
    ) -> bool {
        let resume_to_last_page = self.settings.book_nav_resume.resumes();
        let Some(state) = self.archive_convert.as_mut() else {
            return false;
        };
        if !state.suppress_confirm && !state.allow_direct_read {
            return false;
        }
        state.deferred_fullscreen = Some(ArchiveConvertDeferredFullscreen {
            restore_video_tile,
            reopen: crate::app::DeferredFsReopen {
                history_trigger,
                resume_slideshow,
                target: crate::app::DeferredFsTarget::None,
                resume_to_last_page,
                from_explicit_open: false,
                preserve_after_password_prompt: false,
            },
        });
        true
    }

    pub(crate) fn clear_archive_convert_deferred_fullscreen(&mut self) {
        let had_deferred = self
            .archive_convert
            .as_mut()
            .and_then(|state| state.deferred_fullscreen.take())
            .is_some();
        if had_deferred {
            self.release_fs_nav_lock();
        }
    }

    pub(crate) fn archive_convert_owns_bookmark_request(
        &self,
        request_id: crate::bookmark_browser::BookmarkOpenRequestId,
    ) -> bool {
        self.archive_convert
            .as_ref()
            .and_then(|state| state.completion.bookmark_owner())
            .is_some_and(|owner| owner.request_id == request_id)
    }

    pub(crate) fn cancel_archive_convert_for_bookmark_request(
        &mut self,
        request_id: crate::bookmark_browser::BookmarkOpenRequestId,
    ) -> bool {
        if !self.archive_convert_owns_bookmark_request(request_id) {
            return false;
        }
        let mut state = self
            .archive_convert
            .take()
            .expect("matching archive conversion must still exist");
        state.cancel.store(true, Ordering::Relaxed);
        let had_deferred = state.deferred_fullscreen.take().is_some();
        crate::logger::log(format!(
            "[bookmark-open] cancel archive transition id={} source={}",
            request_id.0,
            state.src_path.display()
        ));
        drop(state);
        if had_deferred {
            self.release_fs_nav_lock();
        }
        true
    }

    /// A new navigation owns the visible context and supersedes any archive dialog/worker.
    /// Same-folder refreshes are not navigation and intentionally keep the dialog alive.
    pub(crate) fn cancel_archive_convert_for_navigation_to(
        &mut self,
        path: &std::path::Path,
        reason: &'static str,
    ) -> bool {
        if self.archive_convert.is_none()
            || self
                .current_folder
                .as_ref()
                .is_some_and(|current| crate::folder_tree::path_eq(current, path))
        {
            return false;
        }
        self.cancel_archive_convert_for_navigation(reason)
    }

    pub(crate) fn cancel_archive_convert_for_navigation(&mut self, reason: &'static str) -> bool {
        let Some(mut state) = self.archive_convert.take() else {
            return false;
        };
        let detached_owner = state.completion.detached_grid_archive_owner().cloned();
        state.cancel.store(true, Ordering::Relaxed);
        let had_deferred = state.deferred_fullscreen.take().is_some();
        crate::logger::log(format!(
            "archive transition cancelled reason={reason} source={}",
            state.src_path.display()
        ));
        drop(state);
        if let Some(owner) = detached_owner.as_ref() {
            self.invalidate_detached_grid_archive_open_owner(owner);
        }
        if had_deferred {
            self.release_fs_nav_lock();
        }
        true
    }

    pub(crate) fn cancel_detached_grid_archive_open_for_replacement(
        &mut self,
        reason: &'static str,
    ) -> bool {
        if !self
            .archive_convert
            .as_ref()
            .is_some_and(|state| state.completion.detached_grid_archive_owner().is_some())
        {
            return false;
        }
        self.cancel_archive_convert_for_navigation(reason)
    }

    fn discard_stale_archive_bookmark_request(&mut self) -> bool {
        let owner = self
            .archive_convert
            .as_ref()
            .and_then(|state| state.completion.bookmark_owner())
            .cloned();
        let Some(owner) = owner else {
            return false;
        };
        if self.bookmark_open_owner_is_current(&owner) {
            return false;
        }
        self.cancel_bookmark_open_request(owner.request_id, "archive_owner_stale")
    }

    /// 毎フレーム呼ばれるダイアログ描画・メッセージ処理のエントリポイント。
    pub(crate) fn show_archive_convert_dialog(&mut self, ctx: &egui::Context) {
        if self.discard_stale_archive_bookmark_request() {
            return;
        }
        // 先にメッセージ処理 (ステート遷移)
        self.poll_archive_convert_messages();

        if self.discard_stale_archive_bookmark_request() {
            return;
        }

        if self
            .archive_convert
            .as_ref()
            .is_some_and(|state| state.pending_direct_nav.is_some())
        {
            let mut state = self
                .archive_convert
                .take()
                .expect("direct archive navigation state must exist");
            let src = state
                .pending_direct_nav
                .take()
                .expect("direct archive path must exist");
            let input_seq = state.input_seq;
            let auto_fs = state.auto_fullscreen;
            let deferred = state.deferred_fullscreen.take();
            let completion = state.completion.clone();
            drop(state);
            if crate::perf::is_enabled() {
                let archive_key = crate::path_key::normalize_keep_drive(&src);
                crate::perf::event(
                    "archive",
                    "pending_direct_nav_consume",
                    Some(&archive_key),
                    input_seq,
                    &[],
                );
            }
            if let ArchiveConvertCompletionPolicy::DetachedGridArchive(owner) = &completion {
                #[cfg(windows)]
                let opened =
                    self.open_converted_grid_archive_in_detached_context(ctx, src.clone(), owner);
                #[cfg(not(windows))]
                let opened = false;
                if deferred.is_some() {
                    self.release_fs_nav_lock();
                }
                if !opened {
                    self.fail_detached_grid_archive_open(
                        owner,
                        "archive_detached_direct_open_failed",
                    );
                }
                return;
            }
            if let ArchiveConvertCompletionPolicy::Bookmark(owner) = &completion {
                if !self.bookmark_open_owner_is_current(owner) {
                    self.cancel_bookmark_open_request(
                        owner.request_id,
                        "archive_direct_completion_stale",
                    );
                    return;
                }
                #[cfg(windows)]
                if let Some(opened) =
                    self.open_converted_bookmark_in_detached_context(ctx, src.clone(), owner)
                {
                    if !opened {
                        self.cancel_bookmark_open_request(
                            owner.request_id,
                            "archive_detached_direct_open_failed",
                        );
                        self.show_feedback_toast(
                            "ブックマーク先の本を別ウィンドウで開けませんでした".to_string(),
                        );
                    }
                    return;
                }
            }
            if auto_fs {
                self.pending_auto_fs_open = true;
            }
            self.load_zip_as_folder_with_input_seq(src.clone(), input_seq);
            let loaded = self
                .current_folder
                .as_ref()
                .is_some_and(|current| crate::folder_tree::path_eq(current, &src));
            if let ArchiveConvertCompletionPolicy::Bookmark(owner) = &completion {
                if loaded {
                    self.begin_bookmark_page_wait(owner);
                } else {
                    self.cancel_bookmark_open_request(
                        owner.request_id,
                        "archive_direct_navigation_blocked",
                    );
                    if deferred.is_some() {
                        self.release_fs_nav_lock();
                    }
                    return;
                }
            }
            self.advance_drilled_current_path(&src);
            if self.favsearch.active {
                self.update_favsearch_address();
            }
            if self.tag_view.active {
                self.update_tag_view_address();
            }
            if let Some(deferred) = deferred {
                let reason = self.reopen_fullscreen_after_folder_nav_load(
                    ctx,
                    deferred.restore_video_tile,
                    deferred.reopen.resume_slideshow,
                    deferred.reopen.history_trigger,
                );
                if reason == "enumerate_defer" {
                    ctx.request_repaint();
                }
            }
            return;
        }

        if let Some(output) = self
            .archive_convert
            .as_mut()
            .and_then(|state| state.pending_sibling_output.take())
        {
            self.archive_convert = None;
            let output_name = output
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("ZIP")
                .to_string();
            self.show_feedback_toast(format!("ZIP ファイルを作成しました: {output_name}"));
            let parent = output.parent().map(std::path::Path::to_path_buf);
            if let Some(parent) = parent
                && self
                    .current_folder
                    .as_ref()
                    .is_some_and(|current| crate::folder_tree::path_eq(current, &parent))
            {
                self.select_after_load = Some(output_name);
                self.load_folder(parent);
            }
            return;
        }

        // 変換完了後のナビゲーション処理 (別フィールドに移動して state を Drop)
        if let Some(nav) = self
            .archive_convert
            .as_mut()
            .and_then(|s| s.pending_nav.take())
        {
            // ConvertDone 受信時に `exists()` は通過しているが、pending_nav 消費までの
            // 短い間隔で並行 maintenance (clear_all/delete_entry) が先に削除する順序レースが
            // 残るため、navigate 直前にもう一度確認する。消えていたらエラー表示に戻す。
            if !nav.exists() {
                self.clear_archive_convert_deferred_fullscreen();
                if let Some(s) = self.archive_convert.as_mut() {
                    s.phase = ArchiveConvertPhase::Error {
                        message: "変換直後にキャッシュが削除されました。再度お試しください。"
                            .to_string(),
                    };
                }
            } else {
                // 元 (RAR/7z/LZH) のパスを退避してから load_folder (キャッシュ ZIP) を実行、
                // その後 override に元パスを書き戻すことで、UI 表示は元ファイルの場所のままに保つ。
                let src = self.archive_convert.as_ref().map(|s| s.src_path.clone());
                // load_folder(cache_zip) で閲覧履歴の戻り先予約が落ちるので、元アーカイブと
                // 一致していたかを退避し、override 復元と同じく後で書き戻す。
                let restore_reading_history = match (&src, &self.reading_history_return_from) {
                    (Some(s), Some(from)) => crate::folder_tree::path_eq(from, s),
                    _ => false,
                };
                let restore_bookmark_view = src.as_ref().and_then(|source| {
                    self.bookmark_view_state
                        .as_ref()
                        .filter(|state| {
                            state
                                .target()
                                .is_some_and(|target| target.matches_loaded_container(source))
                        })
                        .cloned()
                });
                // 明示オープンからの変換 (state.auto_fullscreen=true) のときだけ、変換成功
                // 直後に 1 ページ目を自動フルスクリーン表示する (履歴/アドレスバー経由の
                // 変換は false なので発火しない)。
                let auto_fs = self
                    .archive_convert
                    .as_ref()
                    .map(|s| s.auto_fullscreen)
                    .unwrap_or(false);
                let completion = self
                    .archive_convert
                    .as_ref()
                    .map(|s| s.completion.clone())
                    .unwrap_or(ArchiveConvertCompletionPolicy::Navigation);
                if let ArchiveConvertCompletionPolicy::Bookmark(owner) = &completion
                    && !self.bookmark_open_owner_is_current(owner)
                {
                    self.cancel_bookmark_open_request(
                        owner.request_id,
                        "archive_cache_completion_stale",
                    );
                    return;
                }
                let deferred_fullscreen = self
                    .archive_convert
                    .as_mut()
                    .and_then(|s| s.deferred_fullscreen.take());
                // ブロック時に履歴スタックを巻き戻せるよう、state を drop する前に退避する。
                let nav_history_rollback = self
                    .archive_convert
                    .as_ref()
                    .and_then(|s| s.nav_history_rollback.clone());
                self.archive_convert = None;
                if let ArchiveConvertCompletionPolicy::DetachedGridArchive(owner) = &completion {
                    #[cfg(windows)]
                    let opened = self.open_converted_grid_archive_in_detached_context(
                        ctx,
                        nav.clone(),
                        owner,
                    );
                    #[cfg(not(windows))]
                    let opened = false;
                    if deferred_fullscreen.is_some() {
                        self.release_fs_nav_lock();
                    }
                    if !opened {
                        self.fail_detached_grid_archive_open(
                            owner,
                            "archive_detached_converted_open_failed",
                        );
                    }
                    return;
                }
                #[cfg(windows)]
                if let ArchiveConvertCompletionPolicy::Bookmark(owner) = &completion {
                    if let Some(opened) =
                        self.open_converted_bookmark_in_detached_context(ctx, nav.clone(), owner)
                    {
                        if deferred_fullscreen.is_some() {
                            self.release_fs_nav_lock();
                        }
                        if !opened {
                            self.cancel_bookmark_open_request(
                                owner.request_id,
                                "archive_detached_converted_open_failed",
                            );
                            self.show_feedback_toast(
                                "ブックマーク先の本を別ウィンドウで開けませんでした".to_string(),
                            );
                        }
                        return;
                    }
                }
                if auto_fs {
                    self.pending_auto_fs_open = true;
                }
                let open_owner = completion
                    .open_owner()
                    .expect("pending cache navigation must have an open policy");
                if let Some(source) = src.as_deref() {
                    self.authorize_smart_folder_session_alias(source, &nav);
                }
                self.load_folder_with_scan_owned(nav.clone(), None, open_owner);
                // load が ★固定 (snapshot lock) の範囲外ガード等でブロックされると
                // current_folder は変わらない。その場合は override / address / recent を
                // 更新せず、変換ダイアログを開いたときに変えた履歴スタックも巻き戻す
                // (override と current_folder の不整合・nav スタック残りを防ぐ、Codex P1/P2)。
                let loaded = self
                    .current_folder
                    .as_ref()
                    .is_some_and(|cur| crate::folder_tree::path_eq(cur, &nav));
                if !loaded {
                    if let ArchiveConvertCompletionPolicy::Bookmark(owner) = &completion {
                        self.cancel_bookmark_open_request(
                            owner.request_id,
                            "archive_cache_navigation_blocked",
                        );
                    }
                    if let Some(snapshot) = nav_history_rollback {
                        self.restore_folder_nav_history(snapshot);
                    }
                    if deferred_fullscreen.is_some() {
                        self.release_fs_nav_lock();
                    }
                    return;
                }
                if let Some(src) = src {
                    self.address = src.to_string_lossy().to_string();
                    // 検索 (Ctrl+G / Ctrl+S) 中は recent_folders を一切変更しない。
                    if !(self.global_search.active || self.favsearch.active) {
                        self.forget_recent_folder(&nav);
                        self.remember_recent_folder(&src);
                    }
                    self.update_active_quick_folder_target(&src);
                    if restore_reading_history {
                        self.reading_history_return_from = Some(src.clone());
                    }
                    if let Some(state) = restore_bookmark_view {
                        self.bookmark_view_state = Some(state);
                    }
                    self.archive_source_override = Some(src);
                }
                if let ArchiveConvertCompletionPolicy::Bookmark(owner) = &completion {
                    self.begin_bookmark_page_wait(owner);
                }
                if self.favsearch.active {
                    self.update_favsearch_address();
                }
                if let Some(deferred) = deferred_fullscreen {
                    let reason = self.reopen_fullscreen_after_folder_nav_load(
                        ctx,
                        deferred.restore_video_tile,
                        deferred.reopen.resume_slideshow,
                        deferred.reopen.history_trigger,
                    );
                    if reason == "enumerate_defer" {
                        ctx.request_repaint();
                    }
                }
                return;
            }
        }

        if self.archive_convert.as_ref().is_some_and(|s| {
            archive_convert_window_suppressed(&s.phase, s.suppress_confirm, s.allow_direct_read)
        }) {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }

        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let Some(state) = self.archive_convert.as_mut() else {
            return;
        };
        let src_name = state
            .src_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let fmt_label = state.format.label();
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        let mut should_close = false;
        let mut start_convert = false;
        let mut cancel_convert = false;
        let mut apply_password = false;

        // ZIP (入れ子アーカイブ展開、v1.3.0) は「ZIP を ZIP に変換」だと意味が通らない
        // ので展開系の文言にする。
        let is_zip_expand = state.format == ArchiveFormat::Zip;
        let is_sibling_zip = state.completion.is_sibling_zip();
        let title = match &state.phase {
            ArchiveConvertPhase::Scanning => format!("{fmt_label} を読み込み中..."),
            ArchiveConvertPhase::PasswordRequired { .. } => {
                format!("{fmt_label} パスワード入力")
            }
            ArchiveConvertPhase::Confirm { .. } if is_zip_expand => {
                "ZIP 内のアーカイブを展開".to_string()
            }
            ArchiveConvertPhase::Confirm { .. } => {
                format!("{fmt_label} を ZIP に変換")
            }
            ArchiveConvertPhase::Converting { .. } if is_zip_expand => {
                "ZIP 内のアーカイブを展開中".to_string()
            }
            ArchiveConvertPhase::Converting { .. } => {
                format!("{fmt_label} を ZIP に変換中")
            }
            ArchiveConvertPhase::Error { .. } => "変換エラー".to_string(),
        };

        let mut open = true;
        egui::Window::new(title)
            .id(egui::Id::new("archive_convert_dialog"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(420.0);

                match &state.phase {
                    ArchiveConvertPhase::Scanning => {
                        ui.label(format!("入力: {src_name}"));
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("画像エントリを列挙しています…");
                        });
                        ui.add_space(6.0);
                        if ui.button("キャンセル").clicked() {
                            should_close = true;
                        }
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    }
                    ArchiveConvertPhase::PasswordRequired { message, .. } => {
                        ui.label(format!(
                            "この{fmt_label}ファイルを開くにはパスワードが必要です:"
                        ));
                        ui.label(
                            egui::RichText::new(src_name.as_str())
                                .size(12.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                        ui.add_space(4.0);
                        let resp = crate::ime_focus::add_singleline_sensitive(
                            ui,
                            &mut state.password_input,
                            None,
                            |edit| {
                                edit.password(true)
                                    .desired_width(f32::INFINITY)
                                    .hint_text("パスワード")
                            },
                        );
                        if !resp.has_focus() && !ui.memory(|m| m.focused().is_some()) {
                            resp.request_focus();
                        }
                        if enter_pressed && (resp.has_focus() || resp.lost_focus()) {
                            apply_password = true;
                        }
                        if let Some(message) = message {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(message.as_str())
                                    .color(crate::ui_helpers::ERROR_TEXT_COLOR)
                                    .size(crate::ui_helpers::ERROR_TEXT_SIZE),
                            );
                        }
                        ui.add_space(4.0);
                        let password_note = if is_sibling_zip {
                            "パスワードは保存しません。変換後の ZIP ファイルはパスワードなしで保存されます。"
                        } else {
                            "パスワードは保存しません。変換後の ZIP キャッシュはパスワードなしで保存され、キャッシュが残っている間は次回以降そのまま開けます。"
                        };
                        ui.label(egui::RichText::new(password_note).small().weak());
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let can_apply = !state.password_input.trim().is_empty();
                            if ui
                                .add_enabled(can_apply, egui::Button::new("  OK  "))
                                .clicked()
                            {
                                apply_password = true;
                            }
                            if ui.button("キャンセル").clicked() {
                                should_close = true;
                            }
                        });
                    }
                    ArchiveConvertPhase::Confirm { summary } => {
                        if is_sibling_zip {
                            ui.label(format!(
                                "{fmt_label} を同じフォルダの同名 ZIP ファイルに変換します。"
                            ));
                            ui.label("元ファイルはそのまま残ります。");
                        } else if is_zip_expand {
                            ui.label(
                                "この ZIP には RAR / 7z / LZH などのアーカイブが\
                                 入れ子になっています。",
                            );
                            ui.label(
                                "中身の画像も表示できるように、入れ子を展開した\
                                 閲覧用キャッシュを作成します。",
                            );
                        } else {
                            ui.label(format!(
                                "{fmt_label} を ZIP に変換して閲覧できるようにします。"
                            ));
                        }
                        if !is_sibling_zip {
                            ui.label(
                                "元ファイルはそのまま残り、変換したファイルが\
                                 キャッシュとして作成されます。",
                            );
                            ui.label("キャッシュ管理メニューから削除することができます。");
                        }
                        if state.password.is_some() {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(
                                    "この変換キャッシュはパスワードなしの ZIP として保存されます。",
                                )
                                .color(ui.visuals().warn_fg_color),
                            );
                        }
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(format!("ファイル: {src_name}"))
                                .size(12.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                        let mut info = format!(
                            "画像ファイル数: {} / 変換後 ZIP の目安: 約 {}",
                            summary.image_count,
                            crate::ui_helpers::format_bytes(summary.total_uncompressed_bytes)
                        );
                        if summary.nested_archive_count > 0 {
                            info.push_str(&format!(
                                " / 入れ子アーカイブ: {} 個 (変換時に展開され、画像数が増えます)",
                                summary.nested_archive_count
                            ));
                        }
                        ui.label(
                            egui::RichText::new(info)
                                .size(12.0)
                                .color(ui.visuals().weak_text_color()),
                        );
                        ui.add_space(10.0);
                        if !is_sibling_zip {
                            ui.checkbox(
                                &mut state.suppress_confirm_next_time,
                                "次回から表示しない",
                            );
                            ui.add_space(6.0);
                        }
                        // 直下画像が 0 でも入れ子アーカイブがあれば変換する価値がある
                        // (中身の画像は変換時に展開されて初めて数えられる)。
                        let convertible =
                            summary.image_count > 0 || summary.nested_archive_count > 0;
                        ui.horizontal(|ui| {
                            let action_label = if is_sibling_zip {
                                "ZIP ファイルに変換"
                            } else {
                                "変換して開く"
                            };
                            if ui
                                .add_enabled(convertible, egui::Button::new(action_label))
                                .clicked()
                            {
                                start_convert = true;
                            }
                            if ui.button("キャンセル").clicked() {
                                should_close = true;
                            }
                        });
                        if !convertible {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(
                                    "このアーカイブには画像ファイルが含まれていません。",
                                )
                                .color(ui.visuals().error_fg_color),
                            );
                        }
                    }
                    ArchiveConvertPhase::Converting { progress, .. } => {
                        let done = progress.files_done.load(Ordering::Relaxed);
                        let total = progress.files_total.load(Ordering::Relaxed).max(1);
                        let bytes = progress.bytes_written.load(Ordering::Relaxed);
                        let frac = (done as f32 / total as f32).clamp(0.0, 1.0);
                        ui.label(format!("入力: {src_name}"));
                        ui.add_space(6.0);
                        ui.add(egui::ProgressBar::new(frac).show_percentage());
                        ui.add_space(4.0);
                        ui.label(format!(
                            "{} / {} ファイル ({})",
                            done,
                            total,
                            crate::ui_helpers::format_bytes(bytes)
                        ));
                        ui.add_space(6.0);
                        if ui.button("キャンセル").clicked() {
                            cancel_convert = true;
                        }
                        ctx.request_repaint_after(std::time::Duration::from_millis(80));
                    }
                    ArchiveConvertPhase::Error { message } => {
                        ui.label(format!("入力: {src_name}"));
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(message.as_str())
                                .color(ui.visuals().error_fg_color),
                        );
                        ui.add_space(6.0);
                        if ui.button("閉じる").clicked() {
                            should_close = true;
                        }
                    }
                }
            });

        if !open || escape_pressed {
            should_close = true;
        }

        if cancel_convert {
            if let Some(state) = self.archive_convert.as_ref() {
                state.cancel.store(true, Ordering::Relaxed);
            }
        }
        if start_convert {
            let suppress_next_time = self
                .archive_convert
                .as_ref()
                .is_some_and(|state| state.suppress_confirm_next_time);
            if suppress_next_time && !self.settings.archive_convert_suppresses_confirm() {
                self.settings
                    .set_archive_file_handling(crate::settings::ArchiveFileHandling::Convert);
                self.settings.save();
            }
            self.start_archive_convert();
        }
        if apply_password {
            self.apply_archive_password();
        }
        if should_close {
            // Scan / password retry / conversion all share this lifecycle token.
            if let Some(state) = self.archive_convert.as_ref() {
                state.cancel.store(true, Ordering::Relaxed);
            }
            let nav_history_rollback = self
                .archive_convert
                .as_ref()
                .and_then(|state| state.nav_history_rollback.clone());
            let bookmark_owner = self
                .archive_convert
                .as_ref()
                .and_then(|state| state.completion.bookmark_owner())
                .cloned();
            let detached_owner = self
                .archive_convert
                .as_ref()
                .and_then(|state| state.completion.detached_grid_archive_owner())
                .cloned();
            let had_deferred_fullscreen = self
                .archive_convert
                .as_mut()
                .and_then(|state| state.deferred_fullscreen.take())
                .is_some();
            self.archive_convert = None;
            if let Some(owner) = bookmark_owner {
                self.cancel_bookmark_open_request(owner.request_id, "archive_dialog_closed");
            }
            if let Some(owner) = detached_owner.as_ref() {
                self.cancel_detached_grid_archive_open_owner(owner, "archive_dialog_closed");
            }
            if let Some(snapshot) = nav_history_rollback {
                self.restore_folder_nav_history(snapshot);
            }
            if had_deferred_fullscreen {
                self.release_fs_nav_lock();
            }
        }
    }

    /// バックグラウンドメッセージを取り込んでフェーズ遷移させる。
    fn poll_archive_convert_messages(&mut self) {
        let Some(state) = self.archive_convert.as_mut() else {
            return;
        };
        let mut start_convert_after_poll = false;
        let mut clear_deferred_fullscreen = false;
        while let Ok(msg) = state.rx.try_recv() {
            match msg {
                ArchiveConvertMsg::ScanDone(Ok((summary, direct_read, resolved_src))) => {
                    if !crate::folder_tree::path_eq(&state.src_path, &resolved_src) {
                        state.src_path = resolved_src;
                        // A cache lookup made for a requested subsequent volume does not identify
                        // the first volume that header inspection resolved.
                        state.fallback_cached_zip = None;
                    }
                    if direct_read {
                        state.pending_direct_nav = Some(state.src_path.clone());
                        if crate::perf::is_enabled() {
                            let archive_key =
                                crate::path_key::normalize_keep_drive(&state.src_path);
                            crate::perf::event(
                                "archive",
                                "pending_direct_nav_publish",
                                Some(&archive_key),
                                state.input_seq,
                                &[],
                            );
                        }
                        continue;
                    }
                    state.allow_direct_read = false;
                    if let Some(cached_zip) = state.fallback_cached_zip.take() {
                        state.pending_nav = Some(cached_zip);
                        continue;
                    }
                    // 直下画像 0 でも入れ子アーカイブがあれば変換対象 (展開で画像が出る)。
                    if summary.image_count == 0 && summary.nested_archive_count == 0 {
                        state.phase = ArchiveConvertPhase::Error {
                            message: "このアーカイブには画像ファイルが含まれていません。"
                                .to_string(),
                        };
                        clear_deferred_fullscreen = true;
                    } else if state.suppress_confirm {
                        state.phase = ArchiveConvertPhase::Confirm { summary };
                        start_convert_after_poll = true;
                    } else {
                        state.phase = ArchiveConvertPhase::Confirm { summary };
                        clear_deferred_fullscreen = true;
                    }
                }
                ArchiveConvertMsg::ScanDone(Err(ConvertError::PasswordRequired)) => {
                    state.password = None;
                    state.password_input.clear();
                    state.phase = ArchiveConvertPhase::PasswordRequired {
                        message: None,
                        resume: ArchivePasswordResume::Scan,
                    };
                    clear_deferred_fullscreen = true;
                }
                ArchiveConvertMsg::ScanDone(Err(ConvertError::BadPassword)) => {
                    state.password = None;
                    state.password_input.clear();
                    state.phase = ArchiveConvertPhase::PasswordRequired {
                        message: Some("パスワードが正しくありません".to_string()),
                        resume: ArchivePasswordResume::Scan,
                    };
                    clear_deferred_fullscreen = true;
                }
                ArchiveConvertMsg::ScanDone(Err(ConvertError::Cancelled))
                | ArchiveConvertMsg::ConvertDone(Err(ConvertError::Cancelled))
                | ArchiveConvertMsg::SiblingConvertDone(Err(ConvertError::Cancelled)) => {
                    // User cancellation closes every phase of the shared archive lifecycle.
                    let nav_history_rollback = state.nav_history_rollback.clone();
                    let had_deferred = state.deferred_fullscreen.is_some();
                    let bookmark_owner = state.completion.bookmark_owner().cloned();
                    let detached_owner = state.completion.detached_grid_archive_owner().cloned();
                    self.archive_convert = None;
                    if let Some(owner) = bookmark_owner {
                        self.cancel_bookmark_open_request(
                            owner.request_id,
                            "archive_conversion_cancelled",
                        );
                    }
                    if let Some(owner) = detached_owner.as_ref() {
                        self.cancel_detached_grid_archive_open_owner(
                            owner,
                            "archive_conversion_cancelled",
                        );
                    }
                    if let Some(snapshot) = nav_history_rollback {
                        self.restore_folder_nav_history(snapshot);
                    }
                    if had_deferred {
                        self.release_fs_nav_lock();
                    }
                    return;
                }
                ArchiveConvertMsg::ScanDone(Err(e)) => {
                    state.phase = ArchiveConvertPhase::Error {
                        message: format!("スキャン失敗: {e}"),
                    };
                    clear_deferred_fullscreen = true;
                }
                ArchiveConvertMsg::ConvertDone(Ok((_summary, cached_zip, _cached_size))) => {
                    // DB への record は worker 側で convert_lock を握ったまま済ませている
                    // (docs/async-architecture.md: maintenance と convert の直列化)。
                    // ただし convert worker が `ConvertDone` を送信してから guard を drop する
                    // までの間に、待機していた clear_all / delete_entry が動き出して、
                    // 今 record したばかりのエントリごと消す余地がある (convert_lock は
                    // 「変換と保守が同時に走らない」を保証するが、「変換完了 → 保守開始 →
                    // 保守完了 → UI 受信」の順序は許容される)。
                    // ここで navigation 直前に存在確認し、削除済みなら再変換を促す。
                    if cached_zip.exists() {
                        state.pending_nav = Some(cached_zip);
                    } else {
                        state.phase = ArchiveConvertPhase::Error {
                            message: "変換直後にキャッシュが削除されました。再度お試しください。"
                                .to_string(),
                        };
                        clear_deferred_fullscreen = true;
                    }
                }
                ArchiveConvertMsg::SiblingConvertDone(Ok((_summary, output, _size))) => {
                    state.pending_sibling_output = Some(output);
                }
                ArchiveConvertMsg::ConvertDone(Err(ConvertError::PasswordRequired))
                | ArchiveConvertMsg::SiblingConvertDone(Err(ConvertError::PasswordRequired)) => {
                    state.password = None;
                    state.password_input.clear();
                    state.phase = ArchiveConvertPhase::PasswordRequired {
                        message: None,
                        resume: ArchivePasswordResume::Convert,
                    };
                    clear_deferred_fullscreen = true;
                }
                ArchiveConvertMsg::ConvertDone(Err(ConvertError::BadPassword))
                | ArchiveConvertMsg::SiblingConvertDone(Err(ConvertError::BadPassword)) => {
                    state.password = None;
                    state.password_input.clear();
                    state.phase = ArchiveConvertPhase::PasswordRequired {
                        message: Some("パスワードが正しくありません".to_string()),
                        resume: ArchivePasswordResume::Convert,
                    };
                    clear_deferred_fullscreen = true;
                }
                ArchiveConvertMsg::ConvertDone(Err(e))
                | ArchiveConvertMsg::SiblingConvertDone(Err(e)) => {
                    state.phase = ArchiveConvertPhase::Error {
                        message: format!("変換失敗: {e}"),
                    };
                    clear_deferred_fullscreen = true;
                }
            }
        }
        if clear_deferred_fullscreen {
            self.clear_archive_convert_deferred_fullscreen();
        }
        if start_convert_after_poll && self.archive_convert.is_some() {
            self.start_archive_convert();
        }
    }

    fn apply_archive_password(&mut self) {
        let Some((resume, password, src, format)) =
            self.archive_convert.as_mut().and_then(|state| {
                let (resume, password) = prepare_archive_password_retry(state)?;
                Some((resume, password, state.src_path.clone(), state.format))
            })
        else {
            return;
        };

        match resume {
            ArchivePasswordResume::Scan => {
                if let Some(state) = self.archive_convert.as_mut() {
                    state.rx = spawn_archive_scan(
                        src,
                        format,
                        Some(password),
                        false,
                        state.input_seq,
                        Arc::clone(&state.cancel),
                    );
                    state.allow_direct_read = false;
                    state.phase = ArchiveConvertPhase::Scanning;
                }
            }
            ArchivePasswordResume::Convert => {
                self.start_archive_convert();
            }
        }
    }

    /// Confirm 段階で「変換して開く」が押されたときの遷移。
    fn start_archive_convert(&mut self) {
        if self
            .archive_convert
            .as_ref()
            .is_some_and(|state| state.completion.is_sibling_zip())
        {
            self.start_sibling_zip_convert();
            return;
        }
        let Some(state) = self.archive_convert.as_mut() else {
            return;
        };
        // キャッシュ DB が初期化できていないと書き込み先を確定できない
        let Some(db) = self.archive_cache_db.clone() else {
            state.phase = ArchiveConvertPhase::Error {
                message: "キャッシュ DB の初期化に失敗しています。".to_string(),
            };
            if state.deferred_fullscreen.take().is_some() {
                self.release_fs_nav_lock();
            }
            return;
        };
        let dst = match db.reserve_cache_zip_path(&state.src_path) {
            Ok(p) => p,
            Err(e) => {
                state.phase = ArchiveConvertPhase::Error {
                    message: format!("出力先の作成に失敗: {e}"),
                };
                if state.deferred_fullscreen.take().is_some() {
                    self.release_fs_nav_lock();
                }
                return;
            }
        };
        let progress = Arc::new(ArchiveConvertProgressShared::new());
        let (tx, rx) = mpsc::channel();
        let src = state.src_path.clone();
        let format = state.format;
        let password = state.password.clone();
        let cancel_worker = Arc::clone(&state.cancel);
        let progress_worker = progress.clone();
        let db_worker = Arc::clone(&db);
        let archive_cache_max_bytes = self.settings.archive_cache_max_bytes;
        thread::spawn(move || {
            // 変換と保守 (clear_all / delete_entry) を直列化する。guard は worker thread
            // スコープを抜けるまで保持され、その間は maintenance がブロックされる。
            // MutexGuard は !Send なのでここで取り、同 thread 内の record() まで持ち越す。
            let _convert_guard = db_worker.begin_convert();

            let cb = |p: ConvertProgress| {
                progress_worker
                    .files_done
                    .store(p.files_done as u64, Ordering::Relaxed);
                progress_worker
                    .files_total
                    .store(p.files_total as u64, Ordering::Relaxed);
                progress_worker
                    .bytes_written
                    .store(p.bytes_written, Ordering::Relaxed);
            };
            let result = convert_to_zip_with_password(
                &src,
                &dst,
                format,
                password.as_deref(),
                &cancel_worker,
                Some(&cb),
                ConvertOptions::default(), // 閲覧キャッシュは従来どおり寛容・上書き可
            );
            let msg = match result {
                Ok(summary) => {
                    let cached_size = std::fs::metadata(&dst).map(|m| m.len() as i64).unwrap_or(0);
                    // ここで record。convert_guard 保持中なので maintenance と排他。
                    if let Ok(meta) = std::fs::metadata(&src) {
                        let src_mtime = crate::ui_helpers::mtime_secs(&meta);
                        let src_size = meta.len() as i64;
                        let record_result = db_worker.record(
                            &src,
                            src_mtime,
                            src_size,
                            format,
                            &dst,
                            cached_size,
                            summary.image_count,
                            password.is_some(),
                        );
                        match record_result {
                            Ok(()) => {
                                if archive_cache_max_bytes > 0 {
                                    match db_worker
                                        .prune_to_size_limit_locked(archive_cache_max_bytes, &src)
                                    {
                                        Ok(removed) if removed > 0 => {
                                            crate::logger::log(format!(
                                                "archive_cache: pruned {removed} entries to stay under {} bytes",
                                                archive_cache_max_bytes
                                            ));
                                        }
                                        Ok(_) => {}
                                        Err(e) => crate::logger::log(format!(
                                            "archive_cache: prune_to_size_limit failed: {e}"
                                        )),
                                    }
                                }
                            }
                            Err(e) => crate::logger::log(format!(
                                "archive_cache: record failed after convert: {e}"
                            )),
                        }
                    }
                    ArchiveConvertMsg::ConvertDone(Ok((summary, dst, cached_size)))
                }
                Err(e) => ArchiveConvertMsg::ConvertDone(Err(e)),
            };
            // guard を先に drop する: 待機中の maintenance を走らせてから `ConvertDone` を
            // 送るため、UI 側の `exists()` チェックは「maintenance 完了後」の状態を見ることに
            // なる。guard 保持のまま send してしまうと、UI が先に `exists()` を評価して
            // pending_nav を立て、その後で maintenance が走って同 ZIP を削除する race が残る。
            drop(_convert_guard);
            let _ = tx.send(msg);
        });
        state.phase = ArchiveConvertPhase::Converting { progress };
        state.rx = rx;
    }

    fn start_sibling_zip_convert(&mut self) {
        let Some(state) = self.archive_convert.as_mut() else {
            return;
        };
        let dst = sibling_zip_path(&state.src_path);
        let progress = Arc::new(ArchiveConvertProgressShared::new());
        let (tx, rx) = mpsc::channel();
        let src = state.src_path.clone();
        let format = state.format;
        let password = state.password.clone();
        let cancel_worker = Arc::clone(&state.cancel);
        let progress_worker = Arc::clone(&progress);
        thread::spawn(move || {
            let cb = |p: ConvertProgress| {
                progress_worker
                    .files_done
                    .store(p.files_done as u64, Ordering::Relaxed);
                progress_worker
                    .files_total
                    .store(p.files_total as u64, Ordering::Relaxed);
                progress_worker
                    .bytes_written
                    .store(p.bytes_written, Ordering::Relaxed);
            };
            let result = convert_to_zip_with_password(
                &src,
                &dst,
                format,
                password.as_deref(),
                &cancel_worker,
                Some(&cb),
                ConvertOptions {
                    no_clobber: true,
                    verify: true,
                },
            )
            .map(|summary| {
                let size = std::fs::metadata(&dst).map_or(0, |meta| meta.len() as i64);
                (summary, dst, size)
            });
            let _ = tx.send(ArchiveConvertMsg::SiblingConvertDone(result));
        });
        state.phase = ArchiveConvertPhase::Converting { progress };
        state.rx = rx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_scan_test_zip(path: &std::path::Path) {
        use std::io::Write as _;

        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("page-001.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"image").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn cancelled_scan_worker_stops_and_followup_scan_completes() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("scan.zip");
        write_scan_test_zip(&source);

        let cancelled = Arc::new(AtomicBool::new(false));
        let started = Arc::new(AtomicBool::new(false));
        let stopped = Arc::new(AtomicBool::new(false));
        let started_worker = Arc::clone(&started);
        let stopped_worker = Arc::clone(&stopped);
        let cancelled_rx = spawn_archive_scan_task(Arc::clone(&cancelled), move |cancel| {
            started_worker.store(true, Ordering::Relaxed);
            while !cancel.load(Ordering::Relaxed) {
                std::thread::yield_now();
            }
            stopped_worker.store(true, Ordering::Relaxed);
            Err(ConvertError::Cancelled)
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !started.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(started.load(Ordering::Relaxed));
        cancelled.store(true, Ordering::Relaxed);
        assert!(matches!(
            cancelled_rx.recv_timeout(std::time::Duration::from_secs(2)),
            Ok(ArchiveConvertMsg::ScanDone(Err(ConvertError::Cancelled)))
        ));
        assert!(stopped.load(Ordering::Relaxed));

        let followup = Arc::new(AtomicBool::new(false));
        let followup_rx = spawn_archive_scan(source, ArchiveFormat::Zip, None, false, 0, followup);
        match followup_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("follow-up scan result")
        {
            ArchiveConvertMsg::ScanDone(Ok((summary, false, _))) => {
                assert_eq!(summary.image_count, 1);
            }
            _ => panic!("follow-up scan should complete normally"),
        }
    }

    fn state_for_password_resume(resume: ArchivePasswordResume) -> ArchiveConvertState {
        let (_tx, rx) = mpsc::channel();
        ArchiveConvertState {
            src_path: PathBuf::from(r"C:\tmp\locked.rar"),
            input_seq: 0,
            format: ArchiveFormat::Rar,
            password: None,
            password_input: "  mivtest2026  ".to_string(),
            phase: ArchiveConvertPhase::PasswordRequired {
                message: None,
                resume,
            },
            cancel: Arc::new(AtomicBool::new(false)),
            rx,
            pending_nav: None,
            pending_direct_nav: None,
            allow_direct_read: false,
            fallback_cached_zip: None,
            completion: ArchiveConvertCompletionPolicy::Navigation,
            pending_sibling_output: None,
            nav_history_rollback: None,
            auto_fullscreen: false,
            deferred_fullscreen: None,
            suppress_confirm: false,
            suppress_confirm_next_time: false,
        }
    }

    #[test]
    fn prepare_password_retry_keeps_scan_resume() {
        let mut state = state_for_password_resume(ArchivePasswordResume::Scan);
        let (resume, password) = prepare_archive_password_retry(&mut state).unwrap();

        assert_eq!(resume, ArchivePasswordResume::Scan);
        assert_eq!(password, "mivtest2026");
        assert_eq!(state.password.as_deref(), Some("mivtest2026"));
        assert!(state.password_input.is_empty());
    }

    #[test]
    fn prepare_password_retry_keeps_convert_resume() {
        let mut state = state_for_password_resume(ArchivePasswordResume::Convert);
        let (resume, password) = prepare_archive_password_retry(&mut state).unwrap();

        assert_eq!(resume, ArchivePasswordResume::Convert);
        assert_eq!(password, "mivtest2026");
        assert_eq!(state.password.as_deref(), Some("mivtest2026"));
        assert!(state.password_input.is_empty());
    }

    #[test]
    fn suppress_confirm_hides_only_scanning_phase() {
        assert!(archive_convert_window_suppressed(
            &ArchiveConvertPhase::Scanning,
            true,
            false
        ));
        assert!(!archive_convert_window_suppressed(
            &ArchiveConvertPhase::Converting {
                progress: Arc::new(ArchiveConvertProgressShared::new()),
            },
            true,
            false
        ));
        assert!(!archive_convert_window_suppressed(
            &ArchiveConvertPhase::PasswordRequired {
                message: None,
                resume: ArchivePasswordResume::Scan,
            },
            true,
            false
        ));
        assert!(!archive_convert_window_suppressed(
            &ArchiveConvertPhase::Error {
                message: "failed".to_string(),
            },
            true,
            false
        ));
        assert!(!archive_convert_window_suppressed(
            &ArchiveConvertPhase::Scanning,
            false,
            false
        ));
        assert!(archive_convert_window_suppressed(
            &ArchiveConvertPhase::Scanning,
            false,
            true
        ));
    }

    #[test]
    fn explicit_conversion_uses_same_folder_and_basename() {
        assert_eq!(
            sibling_zip_path(std::path::Path::new(r"C:\books\Comic.CBR")),
            PathBuf::from(r"C:\books\Comic.zip")
        );
        assert_eq!(
            sibling_zip_path(std::path::Path::new(r"C:\books\set.7z")),
            PathBuf::from(r"C:\books\set.zip")
        );
    }
}
