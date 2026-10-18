//! 一覧の前回位置。本の全行読込は既存writer、媒体はlive設定表と取得済み長さを参照する。
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use super::{ActionSurface, App};
use crate::book_resume_db::{BookResumeWriter, MeterReadResult, ReadingMeterValue};
use crate::keymap::KeyAction;

#[derive(Clone)]
enum Delta {
    Record(String, Option<ReadingMeterValue>),
    RemoveScopes(Vec<String>),
    Clear,
}

impl Delta {
    fn apply(&self, rows: &mut HashMap<String, Option<ReadingMeterValue>>) {
        match self {
            Self::Record(key, value) => {
                rows.insert(key.clone(), *value);
            }
            Self::RemoveScopes(scopes) => rows.retain(|key, _| {
                !scopes.iter().any(|scope| {
                    key == scope
                        || key
                            .strip_prefix(scope)
                            .is_some_and(|tail| tail.starts_with('/') || tail.starts_with("::"))
                })
            }),
            Self::Clear => rows.clear(),
        }
    }
}

struct PendingRead {
    rx: mpsc::Receiver<MeterReadResult>,
    deltas: Vec<Delta>,
}

struct PendingClear {
    rx: mpsc::Receiver<Result<usize, String>>,
    origin: Option<(
        super::viewer_context_registry::ViewerContextId,
        ActionSurface,
    )>,
}

#[derive(Default)]
pub(crate) struct BookResumeMeters {
    // NULL行も保持するので、環境設定の登録件数に同期DB countは不要。
    rows: Option<HashMap<String, Option<ReadingMeterValue>>>,
    pending: Option<PendingRead>,
    clear: Option<PendingClear>,
    // path自身をmemo keyにする。items/idxの差替えやviewer切替で別セルを指さない。
    keys: HashMap<PathBuf, ResumeMeterPathKeys>,
}

struct ResumeMeterPathKeys {
    book: String,
    media: String,
}

/// Unknown or inconsistent media positions never become guessed endpoints.
pub(crate) fn media_resume_fraction(position: f64, duration: f64) -> Option<f32> {
    if !position.is_finite()
        || !duration.is_finite()
        || position <= 0.0
        || duration <= 0.0
        || position > duration
    {
        return None;
    }
    let fraction = (position / duration) as f32;
    (fraction > 0.0).then_some(fraction)
}

impl BookResumeMeters {
    pub(crate) fn read_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(crate) fn reload(&mut self, writer: &BookResumeWriter) {
        self.pending = Some(PendingRead {
            rx: writer.read_all(),
            deltas: Vec::new(),
        });
    }

    fn update(&mut self, delta: Delta) {
        if let Some(rows) = &mut self.rows {
            delta.apply(rows);
        }
        if let Some(pending) = &mut self.pending {
            pending.deltas.push(delta);
        }
    }

    pub(crate) fn record(&mut self, path: &Path, value: Option<ReadingMeterValue>) {
        self.update(Delta::Record(crate::path_key::normalize(path), value));
    }

    pub(crate) fn remove_scopes(&mut self, paths: &[PathBuf]) {
        self.update(Delta::RemoveScopes(
            paths
                .iter()
                .map(|p| crate::path_key::normalize(p))
                .collect(),
        ));
        self.keys.clear();
    }

    pub(crate) fn get(&mut self, path: &Path) -> Option<ReadingMeterValue> {
        self.rows.as_ref()?;
        self.ensure_keys(path);
        self.rows
            .as_ref()?
            .get(&self.keys.get(path)?.book)
            .copied()
            .flatten()
    }

    fn ensure_keys(&mut self, path: &Path) {
        if !self.keys.contains_key(path) {
            self.keys.insert(
                path.to_path_buf(),
                ResumeMeterPathKeys {
                    book: crate::path_key::normalize(path),
                    media: crate::adjustment_db::normalize_path(path),
                },
            );
        }
    }

    pub(crate) fn media_key(&mut self, path: &Path) -> &str {
        self.ensure_keys(path);
        &self.keys.get(path).expect("memo inserted").media
    }

    pub(crate) fn count(&self) -> usize {
        self.rows.as_ref().map_or(0, HashMap::len)
    }

    pub(crate) fn poll(&mut self) -> Option<Result<(), String>> {
        let result = match self.pending.as_ref()?.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => return None,
            Err(mpsc::TryRecvError::Disconnected) => Err("book-resume worker stopped".into()),
        };
        let pending = self.pending.take().expect("pending read");
        Some(result.map(|mut snapshot| {
            for delta in pending.deltas {
                delta.apply(&mut snapshot.values);
            }
            self.rows = Some(snapshot.values);
        }))
    }
}

impl App {
    /// Input ownership (modal, text/IME, viewer focus) is decided by the existing callers.
    pub(crate) fn consume_saved_position_clear_shortcuts(
        &mut self,
        ctx: &egui::Context,
        surface: ActionSurface,
    ) -> bool {
        // Share the current-player normalize predicate with native input. Text/IME
        // ownership stays with each caller (native receives a root egui Context).
        if self.fullscreen_idx.is_some_and(|idx| {
            self.music_normalize_modal_active(idx)
                || (self.fs_music_view_active(idx) && self.music_bookmark_modal_open())
        }) {
            return false;
        }
        for action in [
            KeyAction::ClearVideoMediaMemory,
            KeyAction::ClearBookResumePositions,
        ] {
            if self.keymap.consume_action_no_repeat(ctx, action) {
                self.execute_saved_position_clear(action, surface);
                return true;
            }
        }
        false
    }

    pub(crate) fn execute_saved_position_clear(
        &mut self,
        action: KeyAction,
        surface: ActionSurface,
    ) {
        match action {
            KeyAction::ClearVideoMediaMemory => {
                let (positions, watched, tracks) = self.clear_live_video_media_memory();
                let message = if positions == 0 && watched == 0 && tracks == 0 {
                    "保存済みの再生位置・視聴済み記録・音声トラックの選択はありません".into()
                } else {
                    format!(
                        "再生位置 {positions} 件、視聴済み記録 {watched} 件、音声トラックの選択 {tracks} 件をクリアしました"
                    )
                };
                // Like recording playback progress, save at the existing settings boundaries.
                self.show_feedback_toast_on(message, surface);
            }
            KeyAction::ClearBookResumePositions => self.clear_book_resume_async_on(Some(surface)),
            _ => unreachable!("not a saved-position clear action"),
        }
    }

    pub(crate) fn thumbnail_resume_meter(&mut self, idx: usize) -> Option<f32> {
        if !self.settings.thumb_show_resume_meter {
            return None;
        }
        if let Some(
            crate::grid_item::GridItem::Video(path) | crate::grid_item::GridItem::Audio(path),
        ) = self.items.get(idx)
        {
            let key = self.book_resume_meters.media_key(path);
            if self.settings.video_watched_to_end.contains(key) {
                return Some(1.0);
            }
            let position = *self.settings.video_resume_positions.get(key)?;
            // Same source-stamp validation as details_lazy_meta_for_idx, using the memoized key.
            let meta = self
                .details_lazy_meta
                .get(key)
                .filter(|meta| meta.matches_source(self.image_metas.get(idx).copied().flatten()))?;
            return media_resume_fraction(position, meta.media.read()?.duration_secs?);
        }
        self.thumbnail_book_resume_meter(idx)
            .map(ReadingMeterValue::fraction)
    }

    pub(crate) fn persist_book_resume(
        &mut self,
        path: PathBuf,
        idx: usize,
        value: Option<ReadingMeterValue>,
    ) {
        self.persist_book_resume_with_identity(path, idx, value, None);
    }

    pub(crate) fn persist_book_resume_with_identity(
        &mut self,
        path: PathBuf,
        idx: usize,
        value: Option<ReadingMeterValue>,
        identity: Option<crate::book_resume_db::PageIdentity>,
    ) -> bool {
        let value = value.and_then(|v| ReadingMeterValue::new(v.ordinal, v.total));
        let record = (path, idx, value, identity);
        if self.last_book_resume.as_ref() == Some(&record) {
            return true;
        }
        let recorded = self
            .book_resume_writer
            .as_ref()
            .is_some_and(|writer| writer.record_identity(&record.0, idx, value, record.3.clone()));
        if recorded {
            self.book_resume_meters.record(&record.0, value);
        }
        self.last_book_resume = Some(record);
        recorded
    }

    pub(crate) fn reload_book_resume_meters(&mut self) {
        if let Some(writer) = &self.book_resume_writer {
            self.book_resume_meters.reload(writer);
        }
    }

    pub(crate) fn clear_book_resume_async(&mut self) {
        self.clear_book_resume_async_on(None);
    }

    fn clear_book_resume_async_on(&mut self, surface: Option<ActionSurface>) {
        // Keep the first completion ticket, even if the worker already replied but
        // the UI has not polled it yet. Do not admit another mutation or enqueue it.
        if self.book_resume_meters.clear.is_some() {
            let message = "本の読書位置をクリア中です".to_string();
            if let Some(state) = &mut self.pref_state {
                state.book_resume_clear_result = Some(message.clone());
            }
            if let Some(surface) = surface {
                self.show_feedback_toast_on(message, surface);
            } else {
                self.show_feedback_toast(message);
            }
            return;
        }
        if self.book_resume_writer.is_some() && !self.admit_book_append_followup_mutation() {
            return;
        }
        let origin = surface.map(|surface| (self.projected_viewer_context_id(), surface));
        if let Some(writer) = &self.book_resume_writer {
            self.book_resume_meters.clear = Some(PendingClear {
                rx: writer.clear(),
                origin,
            });
            self.book_resume_meters.update(Delta::Clear);
            self.last_book_resume = None;
        } else if let Some(surface) = surface {
            crate::logger::log("book-resume clear: writer unavailable");
            self.show_feedback_toast_on(
                "読書位置をクリアできませんでした。再起動してください".into(),
                surface,
            );
        }
    }

    pub(crate) fn book_resume_entry_count(&self) -> usize {
        self.book_resume_meters.count()
    }

    pub(crate) fn thumbnail_book_resume_meter(&mut self, idx: usize) -> Option<ReadingMeterValue> {
        use super::top_level_grid_view::{
            CollectionGridPosition, SmartFolderPosition, TopLevelGridSurface,
        };
        let physical_folder = match self.top_level_grid_view.surface() {
            TopLevelGridSurface::Folder => true,
            TopLevelGridSurface::SmartFolder(state) => {
                matches!(state.position, SmartFolderPosition::Scoped { .. })
            }
            TopLevelGridSurface::Collection(_) => self
                .top_level_grid_view
                .collection_session()
                .is_some_and(|session| {
                    matches!(
                        session.position,
                        CollectionGridPosition::PhysicalSource { .. }
                    )
                }),
            _ => false,
        };
        if !self.settings.thumb_show_resume_meter
            || !physical_folder
            || self.book_bookmark_view_is_synthetic()
            || self.grid_is_zip_entries()
            || self.grid_is_pdf_pages()
        {
            return None;
        }
        let converted_key;
        let path = match self.items.get(idx)? {
            crate::grid_item::GridItem::Folder(path)
            | crate::grid_item::GridItem::ZipFile(path)
            | crate::grid_item::GridItem::PdfFile(path) => path,
            crate::grid_item::GridItem::ConvertibleArchive { path, .. } => {
                let source = self
                    .converted_archive_cache_paths
                    .get(&crate::path_key::normalize_keep_drive(path))?;
                // Header-confirmed later volumes retain thumbnail resolution, but cannot open.
                // Use only the worker's existing source proof; no paint-time I/O/name inference.
                if matches!(
                    source.rar_volume_proof(),
                    Some(crate::rar_loader::RarVolumeProof::Subsequent { .. })
                ) {
                    return None;
                }
                match source.read_source() {
                    super::ConvertedArchiveSourceState::Direct(path)
                    | super::ConvertedArchiveSourceState::CachedZip { path, .. } => path,
                    super::ConvertedArchiveSourceState::Unavailable {
                        logical_source: Some(logical_source),
                    } => {
                        // Missing cache: compute the next conversion key without I/O.
                        // Existing CachedZip paths remain the open/save/restore key,
                        // including valid absolute paths retained after a data-dir copy.
                        converted_key = crate::archive_cache::cache_zip_path_for_data_dir(
                            &crate::data_dir::get(),
                            logical_source,
                        );
                        &converted_key
                    }
                    super::ConvertedArchiveSourceState::Rar { .. } => {
                        unreachable!("read_source unwraps RAR evidence")
                    }
                    super::ConvertedArchiveSourceState::Pending
                    | super::ConvertedArchiveSourceState::Unavailable {
                        logical_source: None,
                    } => return None,
                }
            }
            _ => return None,
        };
        self.book_resume_meters.get(path)
    }

    pub(crate) fn poll_book_resume_meters(&mut self, ctx: &egui::Context) {
        if let Some(result) = self.book_resume_meters.poll() {
            if let Err(error) = result {
                crate::logger::log(format!("book-resume meter: {error}"));
                self.show_feedback_toast(
                    "読書位置のメーターを取得できませんでした。再起動してください".into(),
                );
            }
            if let Some(ps) = &mut self.pref_state {
                ps.book_resume_entry_count = self.book_resume_meters.count();
            }
            ctx.request_repaint();
        }
        let clear_result =
            self.book_resume_meters.clear.as_ref().and_then(|pending| {
                match pending.rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => Some(Err("worker stopped".into())),
                }
            });
        if let Some(result) = clear_result {
            let pending = self.book_resume_meters.clear.take().expect("pending clear");
            if let Err(error) = &result {
                crate::logger::log(format!("book-resume clear: {error}"));
                self.reload_book_resume_meters();
            }
            let message = match result {
                Ok(0) => "保存済みの本の読書位置はありません".into(),
                Ok(count) => format!("{count} 件の読書位置をクリアしました"),
                Err(error) => format!("読書位置のクリアに失敗しました: {error}"),
            };
            if let Some(ps) = &mut self.pref_state {
                ps.book_resume_entry_count = self.book_resume_meters.count();
                ps.book_resume_clear_result = Some(message.clone());
            }
            if let Some((context, surface)) = pending.origin {
                // Completion is polled from main, after the initiating viewer was unmounted.
                // Use its existing context owner to reach the correct native overlay.
                if self
                    .with_viewer_context(context, |app| {
                        app.show_feedback_toast_on(message, surface);
                    })
                    .is_err()
                {
                    crate::logger::log(format!(
                        "book-resume clear feedback: origin context {} is gone",
                        context.serial()
                    ));
                }
            } else if self.pref_state.is_none() {
                self.show_feedback_toast(message);
            }
            ctx.request_repaint();
        }
        if self.book_resume_meters.pending.is_some() || self.book_resume_meters.clear.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}
