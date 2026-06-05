//! 編集用追加パックのオンライン取得実装 (HTTP + 展開 + 検証)。
//!
//! [`crate::editing_addon`] が定義するパス・マニフェスト型を使い、GitHub Releases から
//! 単一 zip の pack をダウンロードして `%APPDATA%/mimageviewer/addons/editing/packs/<version>/`
//! へ導入する。TensorRT pack の `ai::tensorrt_installer` と同じスレッドモデル
//! (worker thread 1 個 + mpsc 進捗 + `Arc<AtomicBool>` cancel) を踏襲する。
//!
//! ## フロー (docs/editing-add-on-download-spec.md §7)
//!
//! 1. `editing-pack-index.json` を fetch & parse。
//! 2. [`editing_addon::pick_pack`] で app 互換のうち最新 pack を選定。
//! 3. `<base>/<zip_name>` を `downloads/<zip_name>.partial` へ DL (Range resume + retry)。
//! 4. zip 全体の SHA-256 を index 値と照合。
//! 5. `downloads/staging/` に展開 (zip-slip 防御)。
//! 6. `staging/pack-manifest.json` を読み、各ファイルの SHA-256 を照合。
//! 7. `packs/<version>/` へ atomic rename (既存があれば置換)。
//! 8. INSTALL_OK sentinel を書き、`active.json` を <version> に更新。
//! 9. `.partial` zip を削除。
//!
//! ## セキュリティ
//!
//! - release ビルドでは base URL の env override を無視 (固定 https のみ受理)。
//!   平文 http からの実行ファイル / モデル DL は任意コード実行ベクタになるため。
//!   debug ビルドではローカル HTTP サーバーでの E2E 用に override を許可。
//! - zip 内パス / manifest 内パスは [`editing_addon::validate_pack_relpath`] /
//!   [`editing_addon::validate_version_dirname`] で path traversal を拒否する。
//! - 信頼チェーン: https index → zip sha256 → zip 内容 (pack-manifest 含む) →
//!   各ファイル sha256 (= spec §7 step 6 の defense-in-depth)。

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use sha2::{Digest, Sha256};

use crate::editing_addon::{
    self, IndexEntry, PackIndex, PackManifest, validate_pack_relpath, validate_safe_filename,
    validate_version_dirname,
};

/// pack アセットが置かれる GitHub Releases ベース URL。
///
/// 構造:
///   `<base>/editing-pack-index.json` ← まず fetch
///   `<base>/<IndexEntry.zip_name>`   ← 選定 pack の zip を DL
///
/// pack を更新するときはこのタグ (`editing-pack-v1`) を bump し、新しい index/zip を
/// その release にアップロードする。
const DEFAULT_PACK_BASE_URL: &str =
    "https://github.com/MikageSawatari/mimageviewer/releases/download/editing-pack-v1";

/// base URL を debug ビルドで上書きする env var。release では無視される。
const PACK_BASE_URL_ENV: &str = "MIV_EDITING_PACK_BASE_URL";

/// 実際に DL に使う base URL を返す。
///
/// SECURITY (TRT pack と同方針): release ビルドでは `MIV_EDITING_PACK_BASE_URL` の
/// override を **無視** する。任意 `http://` (平文) からの実行ファイル / モデル DL は
/// 任意コード実行ベクタになるため。debug ビルドのみローカル HTTP サーバー用に許可。
fn pack_base_url() -> String {
    #[cfg(debug_assertions)]
    {
        match std::env::var(PACK_BASE_URL_ENV) {
            Ok(v) if !v.trim().is_empty() => return v.trim_end_matches('/').to_string(),
            _ => {}
        }
    }
    #[cfg(not(debug_assertions))]
    {
        if let Ok(v) = std::env::var(PACK_BASE_URL_ENV) {
            if !v.trim().is_empty() {
                crate::logger::log(format!(
                    "[editing pack] ignoring {PACK_BASE_URL_ENV}={v:?} in release build \
                     (security: only the embedded https URL is accepted)"
                ));
            }
        }
    }
    DEFAULT_PACK_BASE_URL.to_string()
}

const HTTP_CONNECT_TIMEOUT_SECS: u64 = 30;
const HTTP_READ_TIMEOUT_SECS: u64 = 60;
/// 一過性 HTTP エラー (5xx / transport) のリトライ間隔 (ミリ秒)。配列長 = 最大リトライ回数。
const HTTP_RETRY_BACKOFFS_MS: &[u64] = &[1000, 3000, 7000, 15000];
/// HTTP DL / sha256 のチャンクサイズ (256 KiB)。進捗更新 + cancel チェックポイント。
const DL_CHUNK_SIZE: usize = 256 * 1024;

/// インストール進捗イベント。worker → UI へ mpsc で送る。
#[derive(Debug, Clone)]
pub enum InstallProgress {
    /// index.json を fetch 中。
    FetchingIndex,
    /// index fetch 完了 & pack 選定済み。
    IndexFetched {
        version: String,
        zip_bytes: u64,
        font_count: u32,
        subject_model: String,
    },
    /// zip を DL 中。
    Downloading { bytes_done: u64, bytes_total: u64 },
    /// DL 完了、zip 全体の SHA-256 を検証中。
    VerifyingZip,
    /// zip を展開中。
    Extracting { entry_index: usize, total: usize },
    /// 展開後の各ファイルを SHA-256 検証中。
    VerifyingFiles { file_index: usize, total: usize },
    /// packs/<version>/ へ配置中 (rename + sentinel + active.json)。
    Installing,
    /// 完了。active.json は version を指す。
    Done { version: String },
    /// ユーザーキャンセル。`.partial` は resume 用に残る。
    Cancelled,
    /// 致命的エラー。worker は exit、再試行はユーザーがダイアログ再オープン。
    Error { message: String },
}

impl InstallProgress {
    /// この進捗で worker が終了したか (= 以降 progress は来ない)。
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            InstallProgress::Done { .. }
                | InstallProgress::Cancelled
                | InstallProgress::Error { .. }
        )
    }
}

/// インストールハンドル。UI 側はこれを保持して [`Self::poll`] で進捗を取り出す。
pub struct InstallHandle {
    rx: mpsc::Receiver<InstallProgress>,
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    last_progress: Option<InstallProgress>,
}

impl InstallHandle {
    /// チャネルから 1 件 progress を取り出す。残っていなければ None。
    pub fn poll(&mut self) -> Option<InstallProgress> {
        match self.rx.try_recv() {
            Ok(p) => {
                self.last_progress = Some(p.clone());
                Some(p)
            }
            Err(_) => None,
        }
    }

    /// 最後に observe した進捗 (poll 済みでも残る)。UI の状態表示用。
    pub fn last_progress(&self) -> Option<&InstallProgress> {
        self.last_progress.as_ref()
    }

    /// キャンセル要求。worker は次のチャンク境界で停止する。
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// worker thread が終了したか。
    pub fn is_finished(&mut self) -> bool {
        if let Some(handle) = &self.join {
            if handle.is_finished() {
                if let Some(h) = self.join.take() {
                    let _ = h.join();
                }
                return true;
            }
        } else {
            return true;
        }
        false
    }
}

impl Drop for InstallHandle {
    /// UI 側がハンドルを drop したら worker も止める (孤児防止)。
    /// non-blocking: cancel フラグを立てて JoinHandle を detach するだけ。
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(h) = self.join.take() {
            std::mem::drop(h);
        }
    }
}

/// インストールを開始する。worker thread を spawn する。
pub fn start_install() -> InstallHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_thread = cancel.clone();
    let tx_for_spawn_error = tx.clone();
    let spawn_result = thread::Builder::new()
        .name("editing-pack-installer".to_string())
        .spawn(move || {
            let cancel_inner = cancel_for_thread.clone();
            let tx_inner = tx.clone();
            match run_install(cancel_inner, &tx_inner) {
                Ok(()) => {}
                Err(e) => {
                    // cancel フラグが立っていれば Cancelled、そうでなければ Error。
                    if cancel_for_thread.load(Ordering::SeqCst) {
                        // staging の中途半端な展開物を best-effort で掃除 (.partial は残す)。
                        let _ = fs::remove_dir_all(staging_dir());
                        let _ = tx.send(InstallProgress::Cancelled);
                    } else {
                        let _ = tx.send(InstallProgress::Error { message: e });
                    }
                }
            }
        });
    let join = match spawn_result {
        Ok(join) => Some(join),
        Err(e) => {
            crate::logger::log(format!(
                "[editing pack] failed to spawn installer thread: {e}"
            ));
            let _ = tx_for_spawn_error.send(InstallProgress::Error {
                message: format!("編集用追加パックの導入 worker を開始できません: {e}"),
            });
            None
        }
    };
    InstallHandle {
        rx,
        cancel,
        join,
        last_progress: None,
    }
}

/// 一時展開ディレクトリ (version 非依存、毎回掃除して使う)。
fn staging_dir() -> PathBuf {
    editing_addon::downloads_dir().join("staging")
}

/// worker thread 本体。
fn run_install(cancel: Arc<AtomicBool>, tx: &mpsc::Sender<InstallProgress>) -> Result<(), String> {
    let _ = tx.send(InstallProgress::FetchingIndex);
    let index = fetch_index(&cancel)?;
    if index.schema > editing_addon::EXPECTED_INDEX_SCHEMA {
        return Err(format!(
            "index schema {} は本バージョンの mIV では非対応です (対応上限 {})。\
             mIV を更新してください。",
            index.schema,
            editing_addon::EXPECTED_INDEX_SCHEMA
        ));
    }
    let entry: IndexEntry = editing_addon::pick_pack(&index)
        .ok_or_else(|| {
            "この mIV バージョンに対応する編集用追加パックが配布一覧に見つかりません。\
             mIV を更新するか、しばらくしてから再試行してください。"
                .to_string()
        })?
        .clone();

    // version は packs/<version>/ のディレクトリ名になるので検証する。
    validate_version_dirname(&entry.version)?;
    // SECURITY: zip_name は downloads.join() と URL 連結に使うので、index 由来の値を
    // そのまま信用せず単純ファイル名であることを検証する (path traversal 防止)。Codex P1。
    validate_safe_filename(&entry.zip_name)
        .map_err(|e| format!("index の zip_name が不正です: {e}"))?;

    let _ = tx.send(InstallProgress::IndexFetched {
        version: entry.version.clone(),
        zip_bytes: entry.zip_bytes,
        font_count: entry.font_count,
        subject_model: entry.subject_model.clone(),
    });

    // ── 準備 ──
    let downloads = editing_addon::downloads_dir();
    fs::create_dir_all(&downloads).map_err(|e| format!("create downloads dir: {e}"))?;
    let partial = downloads.join(format!("{}.partial", entry.zip_name));

    // ── (1) zip を DL (resume + retry) ──
    download_zip(&entry, &partial, &cancel, tx)?;

    // ── (2) zip 全体の SHA-256 検証 ──
    let _ = tx.send(InstallProgress::VerifyingZip);
    let actual = sha256_of_file(&partial, &cancel)?;
    if !actual.eq_ignore_ascii_case(&entry.zip_sha256) {
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "ダウンロードした zip の SHA-256 が一致しません。\nexpected: {}\nactual: {}\n\
             ネットワークエラーまたは配布更新中の可能性があります。再試行してください。",
            entry.zip_sha256, actual
        ));
    }

    // ── (3) staging へ展開 ──
    let staging = staging_dir();
    let _ = fs::remove_dir_all(&staging); // 前回残骸を掃除
    fs::create_dir_all(&staging).map_err(|e| format!("create staging dir: {e}"))?;
    extract_zip(&partial, &staging, &cancel, tx)?;

    // ── (4) pack-manifest を読み、各ファイルを検証 ──
    let manifest = read_staging_manifest(&staging)?;
    verify_manifest(&manifest, &entry, &staging)?;
    verify_files(&manifest, &staging, &cancel, tx)?;
    // manifest に載っていないファイル (= per-file sha256 / license の管理外) は配置前に
    // 取り除く。これで installed_fonts() が未検証フォントを拾うことを防ぐ。Codex P2。
    prune_unmanifested(&manifest, &staging)?;

    // ── (5) 配置: staging → packs/<version>/ (atomic rename) ──
    // 検証完了後・配置直前の遅い cancel も拾う (ここを過ぎると install は確定する)。Codex P3。
    check_cancel(&cancel)?;
    let _ = tx.send(InstallProgress::Installing);
    install_staging(&staging, &entry.version)?;

    // ── (6) sentinel + active.json ──
    write_install_sentinel(&entry.version)?;
    write_active_pointer(&entry.version)?;

    // ── (7) .partial zip を削除 (検証済み、resume 不要) ──
    let _ = fs::remove_file(&partial);

    let _ = tx.send(InstallProgress::Done {
        version: entry.version,
    });
    Ok(())
}

/// `<base>/editing-pack-index.json` を fetch & parse。
fn fetch_index(cancel: &Arc<AtomicBool>) -> Result<PackIndex, String> {
    check_cancel(cancel)?;
    let url = format!("{}/editing-pack-index.json", pack_base_url());
    let agent = build_agent();
    let resp = agent
        .get(&url)
        .call()
        .map_err(|e| format!("fetch index: {e}"))?;
    let json = resp
        .into_string()
        .map_err(|e| format!("read index body: {e}"))?;
    serde_json::from_str(&json).map_err(|e| format!("parse index: {e}"))
}

fn build_agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS))
        .timeout_read(std::time::Duration::from_secs(HTTP_READ_TIMEOUT_SECS))
        .build()
}

/// zip を DL する。`.partial` から Range resume、一過性エラーは backoff リトライ。
fn download_zip(
    entry: &IndexEntry,
    partial: &Path,
    cancel: &Arc<AtomicBool>,
    tx: &mpsc::Sender<InstallProgress>,
) -> Result<(), String> {
    // 既に完全サイズの .partial があれば (前回 verify 前にクラッシュ等) そのまま検証へ。
    if let Ok(meta) = fs::metadata(partial) {
        if meta.len() == entry.zip_bytes {
            let _ = tx.send(InstallProgress::Downloading {
                bytes_done: entry.zip_bytes,
                bytes_total: entry.zip_bytes,
            });
            return Ok(());
        }
        if meta.len() > entry.zip_bytes {
            // 期待より大きい異常。捨てて 0 から。
            let _ = fs::remove_file(partial);
        }
    }

    let url = format!("{}/{}", pack_base_url(), entry.zip_name);
    let agent = build_agent();

    let mut last_err: Option<String> = None;
    for (attempt, &backoff_ms) in std::iter::once(&0u64)
        .chain(HTTP_RETRY_BACKOFFS_MS.iter())
        .enumerate()
    {
        if backoff_ms > 0 {
            crate::logger::log(format!(
                "[editing pack] {} 再試行 (attempt={}, prev error: {})",
                entry.zip_name,
                attempt,
                last_err.as_deref().unwrap_or("?")
            ));
            let start = Instant::now();
            while start.elapsed() < std::time::Duration::from_millis(backoff_ms) {
                check_cancel(cancel)?;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        let resume_now = fs::metadata(partial)
            .ok()
            .map(|m| m.len())
            .unwrap_or(0)
            .min(entry.zip_bytes);
        match try_download_one(&agent, &url, partial, entry, resume_now, cancel, tx) {
            Ok(()) => {
                last_err = None;
                break;
            }
            Err(DownloadAttemptError::Permanent(msg)) => {
                return Err(format!("HTTP {}: {}", entry.zip_name, msg));
            }
            Err(DownloadAttemptError::Retryable(msg)) => {
                last_err = Some(msg);
                continue;
            }
        }
    }
    if let Some(msg) = last_err {
        return Err(format!(
            "HTTP {} (max retries exhausted): {}",
            entry.zip_name, msg
        ));
    }
    Ok(())
}

/// HTTP DL 1 試行のエラー型。
enum DownloadAttemptError {
    Retryable(String),
    Permanent(String),
}

/// zip の HTTP DL を 1 試行 (retry なし)。`resume_from` から Range で append。
fn try_download_one(
    agent: &ureq::Agent,
    url: &str,
    partial: &Path,
    entry: &IndexEntry,
    resume_from: u64,
    cancel: &Arc<AtomicBool>,
    tx: &mpsc::Sender<InstallProgress>,
) -> Result<(), DownloadAttemptError> {
    let mut req = agent.get(url);
    if resume_from > 0 {
        req = req.set("Range", &format!("bytes={}-", resume_from));
    }
    let resp = match req.call() {
        Ok(r) => r,
        Err(ureq::Error::Status(code, _)) => {
            if (500..600).contains(&code) {
                return Err(DownloadAttemptError::Retryable(format!("status {code}")));
            }
            return Err(DownloadAttemptError::Permanent(format!("status {code}")));
        }
        Err(ureq::Error::Transport(t)) => {
            return Err(DownloadAttemptError::Retryable(format!(
                "transport error: {t}"
            )));
        }
    };

    let server_full_response = resp.status() == 200;
    let mut file = if resume_from > 0 && !server_full_response {
        let mut f = match fs::OpenOptions::new().write(true).open(partial) {
            Ok(f) => f,
            Err(e) => {
                return Err(DownloadAttemptError::Permanent(format!(
                    "open partial: {e}"
                )));
            }
        };
        if let Err(e) = f.seek(SeekFrom::End(0)) {
            return Err(DownloadAttemptError::Permanent(format!(
                "seek partial: {e}"
            )));
        }
        f
    } else {
        match fs::File::create(partial) {
            Ok(f) => f,
            Err(e) => {
                return Err(DownloadAttemptError::Permanent(format!(
                    "create partial: {e}"
                )));
            }
        }
    };

    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; DL_CHUNK_SIZE];
    let mut bytes_done: u64 = if server_full_response { 0 } else { resume_from };
    let mut last_progress = Instant::now();
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(DownloadAttemptError::Permanent(
                "ユーザーによってキャンセルされました".to_string(),
            ));
        }
        let n = match reader.read(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                return Err(DownloadAttemptError::Retryable(format!(
                    "read chunk: {e} (after {bytes_done} bytes)"
                )));
            }
        };
        if n == 0 {
            break;
        }
        // advertised サイズを超えて書かない (= 想定外に大きい応答で .partial を肥大化
        // させない)。残量だけ書いて打ち切る。Codex P2 指摘。
        let take = if entry.zip_bytes > 0 {
            (n as u64).min(entry.zip_bytes.saturating_sub(bytes_done)) as usize
        } else {
            n
        };
        if take > 0 {
            if let Err(e) = file.write_all(&buf[..take]) {
                return Err(DownloadAttemptError::Permanent(format!("write chunk: {e}")));
            }
            bytes_done += take as u64;
        }
        if entry.zip_bytes > 0 && bytes_done >= entry.zip_bytes {
            break; // 必要量に到達
        }
        if last_progress.elapsed().as_millis() >= 50 {
            let _ = tx.send(InstallProgress::Downloading {
                bytes_done,
                bytes_total: entry.zip_bytes,
            });
            last_progress = Instant::now();
        }
    }
    if let Err(e) = file.flush() {
        return Err(DownloadAttemptError::Retryable(format!("flush: {e}")));
    }
    // EOF が advertised サイズより手前 = ストリーム途中切断。.partial は残したまま
    // Retryable にして、次の attempt で Range resume させる (hash mismatch で消すより
    // resume 進捗を温存する)。Codex P2 指摘。
    if entry.zip_bytes > 0 && bytes_done < entry.zip_bytes {
        return Err(DownloadAttemptError::Retryable(format!(
            "truncated download: {bytes_done} / {} bytes",
            entry.zip_bytes
        )));
    }
    let _ = tx.send(InstallProgress::Downloading {
        bytes_done: entry.zip_bytes.max(bytes_done),
        bytes_total: entry.zip_bytes,
    });
    Ok(())
}

/// zip を `staging` へ展開する。zip-slip 対策 + per-file atomic。
fn extract_zip(
    zip_path: &Path,
    staging: &Path,
    cancel: &Arc<AtomicBool>,
    tx: &mpsc::Sender<InstallProgress>,
) -> Result<(), String> {
    let file = fs::File::open(zip_path).map_err(|e| format!("open zip: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("read zip: {e}"))?;
    let total = archive.len();
    for i in 0..total {
        check_cancel(cancel)?;
        let _ = tx.send(InstallProgress::Extracting {
            entry_index: i,
            total,
        });
        let mut zentry = archive
            .by_index(i)
            .map_err(|e| format!("zip entry {i}: {e}"))?;
        // zip-slip: enclosed_name で `..` 含むパスを reject。
        let rel_path = match zentry.enclosed_name() {
            Some(p) => p.to_owned(),
            None => return Err(format!("zip entry [{}] が不正なパスです", zentry.name())),
        };
        if rel_path.as_os_str().is_empty() || zentry.is_dir() {
            continue;
        }
        // 文字列化して二重チェック (`..` / 絶対パス / drive)。
        let rel_str = rel_path.to_string_lossy();
        validate_pack_relpath(&rel_str)
            .map_err(|e| format!("zip entry [{}]: {e}", zentry.name()))?;
        let dst = staging.join(&rel_path);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir staging subdir: {e}"))?;
        }
        let mut out = fs::File::create(&dst).map_err(|e| format!("create staging file: {e}"))?;
        // 大きな ONNX モデル (~490MB) の展開中も cancel に応答できるよう、チャンク単位で
        // copy しながら cancel を確認する (std::io::copy は entry EOF まで戻らない)。Codex P3。
        copy_with_cancel(&mut zentry, &mut out, cancel)
            .map_err(|e| format!("extract {rel_str}: {e}"))?;
        out.sync_all()
            .map_err(|e| format!("sync staging file: {e}"))?;
    }
    Ok(())
}

/// reader → writer をチャンク単位でコピーしつつ、各チャンクで cancel を確認する。
fn copy_with_cancel<R: Read, W: Write>(
    reader: &mut R,
    writer: &mut W,
    cancel: &Arc<AtomicBool>,
) -> Result<(), String> {
    let mut buf = vec![0u8; DL_CHUNK_SIZE];
    loop {
        check_cancel(cancel)?;
        let n = reader.read(&mut buf).map_err(|e| format!("read: {e}"))?;
        if n == 0 {
            break;
        }
        writer
            .write_all(&buf[..n])
            .map_err(|e| format!("write: {e}"))?;
    }
    Ok(())
}

/// `staging/pack-manifest.json` を読む。
fn read_staging_manifest(staging: &Path) -> Result<PackManifest, String> {
    let path = staging.join("pack-manifest.json");
    let raw = fs::read_to_string(&path).map_err(|e| format!("read pack-manifest.json: {e}"))?;
    serde_json::from_str(&raw).map_err(|e| format!("parse pack-manifest.json: {e}"))
}

/// manifest のスキーマ / version 整合を検証 (各ファイル sha256 は [`verify_files`])。
fn verify_manifest(
    manifest: &PackManifest,
    entry: &IndexEntry,
    _staging: &Path,
) -> Result<(), String> {
    if manifest.schema > editing_addon::EXPECTED_PACK_SCHEMA {
        return Err(format!(
            "pack schema {} は本バージョンの mIV では非対応です (対応上限 {})。",
            manifest.schema,
            editing_addon::EXPECTED_PACK_SCHEMA
        ));
    }
    if manifest.version != entry.version {
        return Err(format!(
            "pack-manifest の version ({}) が index ({}) と一致しません。配布が壊れています。",
            manifest.version, entry.version
        ));
    }
    if manifest.files.is_empty() {
        return Err("pack-manifest の files が空です。".to_string());
    }
    Ok(())
}

/// manifest 内の各ファイルの SHA-256 を staging 上の実体と照合する。
fn verify_files(
    manifest: &PackManifest,
    staging: &Path,
    cancel: &Arc<AtomicBool>,
    tx: &mpsc::Sender<InstallProgress>,
) -> Result<(), String> {
    let total = manifest.files.len();
    for (idx, f) in manifest.files.iter().enumerate() {
        check_cancel(cancel)?;
        let _ = tx.send(InstallProgress::VerifyingFiles {
            file_index: idx,
            total,
        });
        validate_pack_relpath(&f.path)?;
        let path = staging.join(f.path.replace('\\', "/"));
        if !path.exists() {
            return Err(format!("pack 内ファイルが見つかりません: {}", f.path));
        }
        let actual = sha256_of_file(&path, cancel)?;
        if !actual.eq_ignore_ascii_case(&f.sha256) {
            return Err(format!(
                "{} の SHA-256 が一致しません。\nexpected: {}\nactual: {}",
                f.path, f.sha256, actual
            ));
        }
    }
    Ok(())
}

/// manifest に載っていない実ファイルを staging から取り除く。
///
/// zip 全体の sha256 は検証済みなので「攻撃者が紛れ込ませた」ものではないが、配布者の
/// 手違いで余分なファイルが入っていると per-file sha256 / license 管理外のまま配置され、
/// `installed_fonts()` がそれを拾ってしまう。許可するのは manifest.files の各 path +
/// pack-manifest.json のみ。それ以外は削除して log する。Codex P2。
fn prune_unmanifested(manifest: &PackManifest, staging: &Path) -> Result<(), String> {
    use std::collections::HashSet;
    // 許可セット (Windows は大小無視なので lowercase + 前方スラッシュ正規化)。
    let mut allowed: HashSet<String> = HashSet::new();
    allowed.insert("pack-manifest.json".to_string());
    for f in &manifest.files {
        allowed.insert(f.path.replace('\\', "/").to_ascii_lowercase());
    }
    let mut files = Vec::new();
    collect_files_rel(staging, staging, &mut files)?;
    for rel in files {
        let key = rel.replace('\\', "/").to_ascii_lowercase();
        if !allowed.contains(&key) {
            let full = staging.join(&rel);
            crate::logger::log(format!(
                "[editing pack] manifest 外のファイルを除外: {}",
                rel
            ));
            let _ = fs::remove_file(&full);
        }
    }
    Ok(())
}

/// `base` 配下の全ファイルを `base` からの相対パス文字列で再帰収集する。
fn collect_files_rel(base: &Path, dir: &Path, out: &mut Vec<String>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("read_dir {}: {e}", dir.display()))?;
    for entry in entries.flatten() {
        let ft = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        let path = entry.path();
        if ft.is_dir() {
            collect_files_rel(base, &path, out)?;
        } else if ft.is_file() {
            if let Ok(rel) = path.strip_prefix(base) {
                out.push(rel.to_string_lossy().to_string());
            }
        }
    }
    Ok(())
}

/// `staging` を `packs/<version>/` へ配置する。
///
/// 既存の同 version pack を上書きする場合、「remove → rename」の隙間でクラッシュすると
/// 動作中 pack を失う。これを避けるため、既存があれば一旦 `<dir>.old-<ts>` へ rename して
/// 退避 → staging を本番名へ rename → 退避を削除、の順で行う。配置確定後に sentinel /
/// active.json を書くので、本関数の途中失敗では active pack は壊れない。Codex P2。
fn install_staging(staging: &Path, version: &str) -> Result<(), String> {
    let final_dir = editing_addon::pack_dir(version);
    if let Some(parent) = final_dir.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create packs root: {e}"))?;
    }
    let backup: Option<PathBuf> = if final_dir.exists() {
        let b = final_dir.with_file_name(format!("{version}.old-{}", utc_now_iso8601_compact()));
        fs::rename(&final_dir, &b).map_err(|e| format!("backup old pack dir: {e}"))?;
        Some(b)
    } else {
        None
    };
    match fs::rename(staging, &final_dir) {
        Ok(()) => {
            // 退避した旧 pack を掃除 (best-effort、失敗してもインストールは成功)。
            if let Some(b) = backup {
                let _ = fs::remove_dir_all(&b);
            }
            Ok(())
        }
        Err(e) => {
            // rename 失敗。退避した旧 pack を戻して元状態へ復旧する。
            if let Some(b) = backup {
                let _ = fs::rename(&b, &final_dir);
            }
            Err(format!("install (rename staging): {e}"))
        }
    }
}

/// ファイル名に使える詰めた timestamp (YYYYMMDDThhmmssZ)。`.old-<ts>` 退避用。
fn utc_now_iso8601_compact() -> String {
    utc_now_iso8601().replace([':', '-'], "")
}

/// INSTALL_OK sentinel を atomic 書き込み (tmp → rename)。
fn write_install_sentinel(version: &str) -> Result<(), String> {
    let sentinel = editing_addon::install_sentinel_path(version);
    let dir = editing_addon::pack_dir(version);
    let tmp = dir.join("INSTALL_OK.tmp");
    let body = serde_json::json!({
        "version": version,
        "installed_at": utc_now_iso8601(),
    });
    let body_str =
        serde_json::to_string_pretty(&body).map_err(|e| format!("serialize INSTALL_OK: {e}"))?;
    fs::write(&tmp, body_str).map_err(|e| format!("write INSTALL_OK.tmp: {e}"))?;
    if sentinel.exists() {
        let _ = fs::remove_file(&sentinel);
    }
    fs::rename(&tmp, &sentinel).map_err(|e| format!("rename INSTALL_OK: {e}"))?;
    Ok(())
}

/// `active.json` を atomic 書き込みして version を指す。
fn write_active_pointer(version: &str) -> Result<(), String> {
    let root = editing_addon::addon_root();
    fs::create_dir_all(&root).map_err(|e| format!("create addon root: {e}"))?;
    let pointer = editing_addon::active_pointer_path();
    let tmp = root.join("active.json.tmp");
    let body = serde_json::json!({
        "schema": 1,
        "active_version": version,
    });
    let body_str =
        serde_json::to_string_pretty(&body).map_err(|e| format!("serialize active.json: {e}"))?;
    fs::write(&tmp, body_str).map_err(|e| format!("write active.json.tmp: {e}"))?;
    if pointer.exists() {
        let _ = fs::remove_file(&pointer);
    }
    fs::rename(&tmp, &pointer).map_err(|e| format!("rename active.json: {e}"))?;
    Ok(())
}

/// SHA-256 を計算する。256 KiB チャンクごとに cancel チェック。
fn sha256_of_file(path: &Path, cancel: &Arc<AtomicBool>) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("open hash: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; DL_CHUNK_SIZE];
    loop {
        check_cancel(cancel)?;
        let n = file.read(&mut buf).map_err(|e| format!("read hash: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex_encode(&hasher.finalize()))
}

/// SHA-256 ダイジェストを hex 小文字に。
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

/// cancel フラグが立っていたらエラー early return。
fn check_cancel(cancel: &Arc<AtomicBool>) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        Err("ユーザーによってキャンセルされました".to_string())
    } else {
        Ok(())
    }
}

/// 現在時刻を UTC ISO 8601 形式で (chrono 非依存、手書き)。
fn utc_now_iso8601() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let (year, month, day, hour, min, sec) = unix_to_ymdhms(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn unix_to_ymdhms(mut secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let sec = (secs % 60) as u32;
    secs /= 60;
    let min = (secs % 60) as u32;
    secs /= 60;
    let hour = (secs % 24) as u32;
    secs /= 24;
    let mut days = secs;
    let mut year: u32 = 1970;
    loop {
        let dy: u64 = if is_leap(year) { 366 } else { 365 };
        if days < dy {
            break;
        }
        days -= dy;
        year += 1;
    }
    let leap = is_leap(year);
    let mdays: [u32; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month: u32 = 1;
    let mut day = days as u32 + 1;
    for &dm in &mdays {
        if day <= dm {
            break;
        }
        day -= dm;
        month += 1;
    }
    (year, month, day, hour, min, sec)
}

fn is_leap(y: u32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editing_addon::{FileKind, PackFile};

    fn mk_file(path: &str, kind: FileKind) -> PackFile {
        PackFile {
            path: path.to_string(),
            kind,
            license: String::new(),
            sha256: String::new(),
            bytes: 0,
            model_id: None,
        }
    }

    #[test]
    fn prune_removes_unmanifested_files() {
        let base = std::env::temp_dir().join("miv_editing_prune_test_a1");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("fonts")).unwrap();
        fs::create_dir_all(base.join("models")).unwrap();
        fs::write(base.join("pack-manifest.json"), "{}").unwrap();
        fs::write(base.join("fonts/A.ttf"), "a").unwrap();
        fs::write(base.join("fonts/Evil.ttf"), "evil").unwrap(); // manifest 外
        fs::write(base.join("models/m.onnx"), "m").unwrap();
        fs::write(base.join("README.txt"), "stray").unwrap(); // manifest 外

        let manifest = PackManifest {
            schema: 1,
            pack_id: "x".to_string(),
            version: "1".to_string(),
            app_min_version: String::new(),
            files: vec![
                mk_file("fonts/A.ttf", FileKind::Font),
                mk_file("models/m.onnx", FileKind::SubjectMatteModel),
            ],
        };
        prune_unmanifested(&manifest, &base).unwrap();
        assert!(base.join("fonts/A.ttf").exists());
        assert!(base.join("models/m.onnx").exists());
        assert!(base.join("pack-manifest.json").exists());
        assert!(
            !base.join("fonts/Evil.ttf").exists(),
            "manifest 外フォントは除外される"
        );
        assert!(
            !base.join("README.txt").exists(),
            "manifest 外ファイルは除外される"
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn collect_files_rel_recurses() {
        let base = std::env::temp_dir().join("miv_editing_collect_test_a1");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("a/b")).unwrap();
        fs::write(base.join("top.txt"), "1").unwrap();
        fs::write(base.join("a/mid.txt"), "2").unwrap();
        fs::write(base.join("a/b/deep.txt"), "3").unwrap();
        let mut out = Vec::new();
        collect_files_rel(&base, &base, &mut out).unwrap();
        let norm: Vec<String> = out.iter().map(|s| s.replace('\\', "/")).collect();
        assert_eq!(norm.len(), 3);
        assert!(norm.contains(&"top.txt".to_string()));
        assert!(norm.contains(&"a/mid.txt".to_string()));
        assert!(norm.contains(&"a/b/deep.txt".to_string()));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn hex_encode_basic() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0xab]), "00ffab");
        assert_eq!(hex_encode(&[]), "");
    }

    #[test]
    fn pack_base_url_behavior() {
        // env を触るテストはこの 1 個に集約する (cargo test は並列実行なので、
        // 同じ env var を複数テストが set/remove すると race してフレーキーになる)。
        // SAFETY: 本テストだけが PACK_BASE_URL_ENV を触る前提。read/write のみで副作用なし。

        // (1) env 未設定なら埋め込み https URL。
        unsafe {
            std::env::remove_var(PACK_BASE_URL_ENV);
        }
        assert_eq!(pack_base_url(), DEFAULT_PACK_BASE_URL);
        assert!(pack_base_url().starts_with("https://github.com/"));

        // (2) override の扱いはビルド種別で変わる。
        unsafe {
            std::env::set_var(PACK_BASE_URL_ENV, "http://127.0.0.1:8123/");
        }
        #[cfg(debug_assertions)]
        {
            // debug: override が効く (末尾 / は剥がす)。ローカル HTTP サーバー E2E 用。
            assert_eq!(pack_base_url(), "http://127.0.0.1:8123");
        }
        #[cfg(not(debug_assertions))]
        {
            // release: override を無視して固定 https URL (security)。
            assert_eq!(pack_base_url(), DEFAULT_PACK_BASE_URL);
        }
        unsafe {
            std::env::remove_var(PACK_BASE_URL_ENV);
        }
    }

    #[test]
    fn install_progress_terminal_flags() {
        assert!(
            InstallProgress::Done {
                version: "x".to_string()
            }
            .is_terminal()
        );
        assert!(InstallProgress::Cancelled.is_terminal());
        assert!(
            InstallProgress::Error {
                message: "e".to_string()
            }
            .is_terminal()
        );
        assert!(!InstallProgress::FetchingIndex.is_terminal());
        assert!(
            !InstallProgress::Downloading {
                bytes_done: 1,
                bytes_total: 2
            }
            .is_terminal()
        );
    }

    #[test]
    fn utc_iso8601_epoch() {
        // 1970-01-01T00:00:00Z を手書きロジックで再現できるか。
        let (y, mo, d, h, mi, s) = unix_to_ymdhms(0);
        assert_eq!((y, mo, d, h, mi, s), (1970, 1, 1, 0, 0, 0));
        // 2021-01-01T00:00:00Z = 1609459200
        let (y, mo, d, h, mi, s) = unix_to_ymdhms(1_609_459_200);
        assert_eq!((y, mo, d, h, mi, s), (2021, 1, 1, 0, 0, 0));
    }
}
