//! App が共有する文字起こし表示用キャッシュ。viewer bundle には置かない。
//! UI はメモリと channel だけを扱い、遅延起動する単一 worker が SELECT と署名読取りを行う。
//! 終了時は channel を閉じるだけで、worker の join や待機はしない。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use crate::transcript_db::{
    FileSignature, TranscriptDb, TranscriptRow, TranscriptSegment, TranscriptStatus,
};

const MAX_ENTRIES: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TranscriptDisplayRow {
    pub signature: FileSignature,
    pub audio_stream: Option<usize>,
    pub model_id: String,
    pub config_version: i64,
    pub status: TranscriptStatus,
    pub created_at: i64,
    pub segments: Arc<Vec<TranscriptSegment>>,
}

impl From<TranscriptRow> for TranscriptDisplayRow {
    fn from(row: TranscriptRow) -> Self {
        Self {
            signature: row.signature,
            audio_stream: row.audio_stream,
            model_id: row.model_id,
            config_version: row.config_version,
            status: row.status,
            created_at: row.created_at,
            // DB の Vec を所有権ごと渡す。長い本文を複製して保持しない。
            segments: Arc::new(row.segments),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TranscriptDisplayValue {
    pub row: Option<TranscriptDisplayRow>,
    pub current_signature: Option<FileSignature>,
    pub read_failed: bool,
}

impl TranscriptDisplayValue {
    /// 消えたファイルは一覧の lifecycle が処理する。署名が揃うときだけ比較する。
    pub fn is_stale(&self) -> bool {
        self.row
            .as_ref()
            .zip(self.current_signature)
            .is_some_and(|(row, signature)| row.signature != signature)
    }

    fn failed() -> Self {
        Self {
            read_failed: true,
            ..Self::default()
        }
    }
}

enum Entry {
    Pending {
        generation: u64,
        previous: Option<TranscriptDisplayValue>,
    },
    Loaded {
        generation: u64,
        value: TranscriptDisplayValue,
    },
}

impl Entry {
    fn generation(&self) -> u64 {
        match self {
            Self::Pending { generation, .. } | Self::Loaded { generation, .. } => *generation,
        }
    }
}

struct ReadRequest {
    key: String,
    path: PathBuf,
    epoch: u64,
    generation: u64,
    ctx: egui::Context,
}

#[derive(Debug)]
struct ReadResult {
    key: String,
    epoch: u64,
    generation: u64,
    value: TranscriptDisplayValue,
}

struct Worker {
    requests: Sender<ReadRequest>,
    results: Receiver<ReadResult>,
}

impl Worker {
    fn start(db_path: Option<PathBuf>) -> std::io::Result<Self> {
        let (requests, request_rx) = mpsc::channel::<ReadRequest>();
        let (result_tx, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("transcript-display".into())
            .spawn(move || {
                let db_path = db_path.unwrap_or_else(TranscriptDb::db_path);
                while let Ok(request) = request_rx.recv() {
                    let value = read_value(&db_path, &request.key, &request.path);
                    let result = ReadResult {
                        key: request.key,
                        epoch: request.epoch,
                        generation: request.generation,
                        value,
                    };
                    if result_tx.send(result).is_err() {
                        break;
                    }
                    // 共有 cache の採用は root だけで行う。
                    request.ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            })?;
        Ok(Self { requests, results })
    }
}

fn read_value(db_path: &Path, key: &str, path: &Path) -> TranscriptDisplayValue {
    let started = Instant::now();
    // 初回作成や取り込み後の DB を次の要求で見るため、接続を要求間で持ち越さない。
    // get が完了し DB 接続を閉じてから署名を読む。
    let read = TranscriptDb::open_read_only(db_path).and_then(|db| match db {
        Some(db) => db.get(key),
        None => Ok(None),
    });
    let (row, mut read_failed) = match read {
        Ok(row) => (row.map(TranscriptDisplayRow::from), false),
        Err(error) => {
            crate::logger::log(format!("transcript display: reading {key} failed: {error}"));
            (None, true)
        }
    };
    let current_signature = match std::fs::metadata(path) {
        Ok(metadata) => FileSignature::from_metadata(&metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            crate::logger::log(format!(
                "transcript display: reading signature for {key} failed: {error}"
            ));
            read_failed = true;
            None
        }
    };
    if crate::perf::is_enabled() {
        crate::perf::event(
            "ai",
            "transcript_display_read",
            Some(key),
            0,
            &[
                (
                    "ms",
                    serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                ),
                ("has_row", serde_json::json!(row.is_some())),
                (
                    "has_signature",
                    serde_json::json!(current_signature.is_some()),
                ),
                ("failed", serde_json::json!(read_failed)),
            ],
        );
    }
    TranscriptDisplayValue {
        row,
        current_signature,
        read_failed,
    }
}

#[derive(Default)]
pub(crate) struct TranscriptDisplayCache {
    entries: HashMap<String, Entry>,
    epoch: u64,
    // invalidate 後の同じキーにも別の番号を使い、古い結果を受け付けない。
    next_generation: u64,
    worker: Option<Worker>,
}

impl TranscriptDisplayCache {
    pub fn get(&self, key: &str) -> Option<&TranscriptDisplayValue> {
        match self.entries.get(key) {
            Some(Entry::Loaded { value, .. }) => Some(value),
            Some(Entry::Pending { previous, .. }) => previous.as_ref(),
            _ => None,
        }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    pub fn needs_request(&self, key: &str, refresh: bool) -> bool {
        refresh || !self.contains(key)
    }

    /// 表示開始・再入場時だけ要求する。読取りだけの再確認中は既存の本文を保持する。
    /// clear / invalidate は entry ごと捨てるため、その後の要求に以前の本文は持ち越さない。
    /// 起動・送信失敗は失敗値を終端として保持し、毎フレームの再要求を防ぐ。
    pub fn request(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        path: &Path,
        refresh: bool,
    ) -> Result<bool, String> {
        self.request_with_start(ctx, key, path, refresh, || Worker::start(None))
    }

    fn request_with_start(
        &mut self,
        ctx: &egui::Context,
        key: &str,
        path: &Path,
        refresh: bool,
        start: impl FnOnce() -> std::io::Result<Worker>,
    ) -> Result<bool, String> {
        if self.contains(key) {
            if !refresh {
                return Ok(false);
            }
        } else if self.entries.len() >= MAX_ENTRIES {
            self.clear();
        }
        self.next_generation = self.next_generation.wrapping_add(1);
        let generation = self.next_generation;
        let previous = self.entries.remove(key).and_then(|entry| match entry {
            Entry::Loaded { value, .. } => Some(value),
            Entry::Pending { previous, .. } => previous,
        });
        self.entries.insert(
            key.to_owned(),
            Entry::Pending {
                generation,
                previous,
            },
        );
        let request = ReadRequest {
            key: key.to_owned(),
            path: path.to_owned(),
            epoch: self.epoch,
            generation,
            ctx: ctx.clone(),
        };
        if self.worker.is_none() {
            match start() {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => {
                    crate::logger::log(format!(
                        "transcript display: starting reader failed: {error}"
                    ));
                    self.adopt(ReadResult {
                        key: request.key,
                        epoch: request.epoch,
                        generation,
                        value: TranscriptDisplayValue::failed(),
                    });
                    return Err(error.to_string());
                }
            }
        }
        if let Err(error) = self
            .worker
            .as_ref()
            .expect("started reader")
            .requests
            .send(request)
        {
            crate::logger::log("transcript display: reader disconnected");
            let request = error.0;
            self.adopt(ReadResult {
                key: request.key,
                epoch: request.epoch,
                generation,
                value: TranscriptDisplayValue::failed(),
            });
            return Err("transcript display reader disconnected".into());
        }
        Ok(true)
    }

    pub fn invalidate(&mut self, key: &str) {
        self.entries.remove(key);
    }

    pub fn clear(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.entries.clear();
    }

    /// root frame で非blocking採用する。別キーの結果もそのキーにだけ収める。
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        loop {
            let result = self
                .worker
                .as_ref()
                .and_then(|worker| worker.results.try_recv().ok());
            let Some(result) = result else { break };
            changed |= self.adopt(result);
        }
        changed
    }

    fn adopt(&mut self, result: ReadResult) -> bool {
        if result.epoch != self.epoch
            || self.entries.get(&result.key).map(Entry::generation) != Some(result.generation)
        {
            return false;
        }
        self.entries.insert(
            result.key,
            Entry::Loaded {
                generation: result.generation,
                value: result.value,
            },
        );
        true
    }

    #[cfg(test)]
    pub(crate) fn seed(&mut self, key: &str, value: TranscriptDisplayValue) {
        if !self.contains(key) && self.entries.len() >= MAX_ENTRIES {
            self.clear();
        }
        self.next_generation = self.next_generation.wrapping_add(1);
        self.entries.insert(
            key.to_owned(),
            Entry::Loaded {
                generation: self.next_generation,
                value,
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn epoch_for_test(&self) -> u64 {
        self.epoch
    }
    #[cfg(test)]
    pub(crate) fn request_generation_for_test(&self) -> u64 {
        self.next_generation
    }
}

#[cfg(test)]
mod tests;
