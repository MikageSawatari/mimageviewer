//! 一覧の読書位置。全行読込は既存writer、通常paintはpath memoとmap参照だけ。
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use super::App;
use crate::book_resume_db::{BookResumeWriter, MeterReadResult, ReadingMeterValue};

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

#[derive(Default)]
pub(crate) struct BookResumeMeters {
    // NULL行も保持するので、環境設定の登録件数に同期DB countは不要。
    rows: Option<HashMap<String, Option<ReadingMeterValue>>>,
    pending: Option<PendingRead>,
    clear: Option<mpsc::Receiver<Result<usize, String>>>,
    // path自身をmemo keyにする。items/idxの差替えやviewer切替で別セルを指さない。
    keys: HashMap<PathBuf, String>,
}

impl BookResumeMeters {
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
        let rows = self.rows.as_ref()?;
        if !self.keys.contains_key(path) {
            self.keys
                .insert(path.to_path_buf(), crate::path_key::normalize(path));
        }
        let key = self.keys.get(path)?;
        rows.get(key).copied().flatten()
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
    pub(crate) fn persist_book_resume(
        &mut self,
        path: PathBuf,
        idx: usize,
        value: Option<ReadingMeterValue>,
    ) {
        let value = value.and_then(|v| ReadingMeterValue::new(v.ordinal, v.total));
        let record = (path, idx, value);
        if self.last_book_resume.as_ref() == Some(&record) {
            return;
        }
        if let Some(writer) = &self.book_resume_writer
            && writer.record(&record.0, idx, value)
        {
            self.book_resume_meters.record(&record.0, value);
        }
        self.last_book_resume = Some(record);
    }

    pub(crate) fn reload_book_resume_meters(&mut self) {
        if let Some(writer) = &self.book_resume_writer {
            self.book_resume_meters.reload(writer);
        }
    }

    pub(crate) fn clear_book_resume_async(&mut self) {
        if let Some(writer) = &self.book_resume_writer {
            self.book_resume_meters.clear = Some(writer.clear());
            self.book_resume_meters.update(Delta::Clear);
            self.last_book_resume = None;
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
        if !self.settings.thumb_show_book_resume_meter
            || !physical_folder
            || self.book_bookmark_view_is_synthetic()
            || self.grid_is_zip_entries()
            || self.grid_is_pdf_pages()
        {
            return None;
        }
        let path = match self.items.get(idx)? {
            crate::grid_item::GridItem::Folder(path)
            | crate::grid_item::GridItem::ZipFile(path)
            | crate::grid_item::GridItem::PdfFile(path) => path,
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
            self.book_resume_meters
                .clear
                .as_ref()
                .and_then(|rx| match rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Empty) => None,
                    Err(mpsc::TryRecvError::Disconnected) => Some(Err("worker stopped".into())),
                });
        if let Some(result) = clear_result {
            self.book_resume_meters.clear = None;
            if result.is_err() {
                self.reload_book_resume_meters();
            }
            if let Some(ps) = &mut self.pref_state {
                ps.book_resume_entry_count = self.book_resume_meters.count();
                ps.book_resume_clear_result = Some(match result {
                    Ok(count) => format!("{count} 件の読書位置をクリアしました"),
                    Err(error) => format!("クリアに失敗しました: {error}"),
                });
            }
            ctx.request_repaint();
        }
        if self.book_resume_meters.pending.is_some() || self.book_resume_meters.clear.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}
