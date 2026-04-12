//! PDF ファイルの列挙・レンダリングモジュール。
//!
//! 専用ワーカースレッドが Pdfium インスタンスを所有し、全ての PDFium 操作を
//! シリアルに処理する。呼び出し側はチャネル経由でリクエストを送り、結果を受け取る。
//!
//! - バックグラウンドスレッド (サムネイル/フルスクリーン) は `.recv()` でブロック待ち (OK)
//! - UI スレッドは `.try_recv()` でポーリングし、ブロックしない
//!
//! PDFium DLL は exe 内に埋め込まれており、初回アクセス時に
//! `%APPDATA%/mimageviewer/pdfium.dll` に展開される。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, OnceLock};

use pdfium_render::prelude::*;

// -----------------------------------------------------------------------
// PDFium DLL 埋め込み & 展開
// -----------------------------------------------------------------------

static PDFIUM_DLL_BYTES: &[u8] = include_bytes!("../vendor/pdfium/bin/pdfium.dll");

static DLL_PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();

fn ensure_dll_extracted() -> Result<&'static PathBuf, String> {
    DLL_PATH
        .get_or_init(|| {
            let dir = crate::data_dir::get();
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("data_dir create failed: {e}"))?;
            let dll_path = dir.join("pdfium.dll");
            let needs_extract = match std::fs::metadata(&dll_path) {
                Ok(meta) => meta.len() != PDFIUM_DLL_BYTES.len() as u64,
                Err(_) => true,
            };
            if needs_extract {
                std::fs::write(&dll_path, PDFIUM_DLL_BYTES)
                    .map_err(|e| format!("pdfium.dll extract failed: {e}"))?;
                crate::logger::log(format!(
                    "pdfium.dll extracted to {} ({} bytes)",
                    dll_path.display(),
                    PDFIUM_DLL_BYTES.len()
                ));
            }
            Ok(dll_path)
        })
        .as_ref()
        .map_err(|e| e.clone())
}

// -----------------------------------------------------------------------
// ワーカースレッド
// -----------------------------------------------------------------------

/// ワーカーへのリクエスト。
enum WorkerRequest {
    Enumerate {
        path: PathBuf,
        password: Option<String>,
        reply: mpsc::Sender<std::io::Result<Vec<PdfPageEntry>>>,
    },
    CheckPassword {
        path: PathBuf,
        reply: mpsc::Sender<PdfAccessStatus>,
    },
    Render {
        path: PathBuf,
        page_num: u32,
        target_px: u32,
        password: Option<String>,
        cancel: Option<Arc<AtomicBool>>,
        reply: mpsc::Sender<std::io::Result<image::DynamicImage>>,
    },
}

struct PdfWorker {
    /// 通常チャネル (サムネイルワーカーからの Render 用)
    tx: mpsc::Sender<WorkerRequest>,
    /// 優先チャネル (Enumerate / CheckPassword / ズーム再レンダリング用)
    /// ワーカーは各リクエスト処理後にこちらを先にチェックする
    priority_tx: mpsc::Sender<WorkerRequest>,
}

static WORKER: OnceLock<PdfWorker> = OnceLock::new();

fn get_worker() -> &'static PdfWorker {
    WORKER.get_or_init(|| PdfWorker::start())
}

impl PdfWorker {
    fn start() -> Self {
        let (tx, rx) = mpsc::channel::<WorkerRequest>();
        let (priority_tx, priority_rx) = mpsc::channel::<WorkerRequest>();

        std::thread::Builder::new()
            .name("pdf-worker".to_string())
            .spawn(move || {
                crate::logger::log("pdf-worker: starting (dual-channel)");

                let pdfium = match Self::init_pdfium() {
                    Ok(p) => p,
                    Err(e) => {
                        crate::logger::log(format!("pdf-worker: init failed: {e}"));
                        // エラー状態で両チャネルのリクエストにエラーを返す
                        loop {
                            match priority_rx.try_recv() {
                                Ok(req) => { Self::reply_init_error(&req, &e); continue; }
                                Err(mpsc::TryRecvError::Disconnected) => return,
                                Err(mpsc::TryRecvError::Empty) => {}
                            }
                            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                                Ok(req) => Self::reply_init_error(&req, &e),
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
                            }
                        }
                    }
                };

                crate::logger::log("pdf-worker: ready");

                loop {
                    // 優先チャネルを先にすべて処理する
                    loop {
                        match priority_rx.try_recv() {
                            Ok(req) => Self::handle_request(&pdfium, req),
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                crate::logger::log("pdf-worker: stopped");
                                return;
                            }
                        }
                    }
                    // 通常チャネルから 1 件処理 (タイムアウト付きで優先チャネルを再チェック)
                    match rx.recv_timeout(std::time::Duration::from_millis(10)) {
                        Ok(req) => Self::handle_request(&pdfium, req),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            crate::logger::log("pdf-worker: stopped");
                            return;
                        }
                    }
                }
            })
            .expect("failed to spawn pdf-worker thread");

        PdfWorker { tx, priority_tx }
    }

    /// 1 件のリクエストを処理する。
    fn handle_request(pdfium: &Pdfium, req: WorkerRequest) {
        match req {
            WorkerRequest::Enumerate { path, password, reply } => {
                let result = Self::do_enumerate(pdfium, &path, password.as_deref());
                let _ = reply.send(result);
            }
            WorkerRequest::CheckPassword { path, reply } => {
                let status = Self::do_check_password(pdfium, &path);
                let _ = reply.send(status);
            }
            WorkerRequest::Render { path, page_num, target_px, password, cancel, reply } => {
                // キャンセル済みならスキップ (reply を drop → 受信側は RecvError)
                if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                    crate::logger::log(format!(
                        "pdf-worker: render cancelled (pre-start) page={}",
                        page_num + 1
                    ));
                    return;
                }
                let result = Self::do_render(
                    pdfium, &path, page_num, target_px, password.as_deref(),
                );
                // レンダリング完了後もキャンセルチェック (結果が不要なら送信しない)
                if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                    crate::logger::log(format!(
                        "pdf-worker: render cancelled (post-render) page={}",
                        page_num + 1
                    ));
                    return;
                }
                let _ = reply.send(result);
            }
        }
    }

    fn init_pdfium() -> Result<Pdfium, String> {
        let dll_path = ensure_dll_extracted()?;
        let dll_dir = dll_path
            .parent()
            .ok_or_else(|| "cannot determine DLL directory".to_string())?;

        let bindings = Pdfium::bind_to_library(
            Pdfium::pdfium_platform_library_name_at_path(
                dll_dir.to_str().ok_or("non-UTF8 path")?,
            ),
        )
        .map_err(|e| format!("PDFium binding failed: {e}"))?;
        Ok(Pdfium::new(bindings))
    }

    fn reply_init_error(req: &WorkerRequest, e: &str) {
        match req {
            WorkerRequest::Enumerate { reply, .. } => {
                let _ = reply.send(Err(std::io::Error::new(
                    std::io::ErrorKind::Other, e.to_string(),
                )));
            }
            WorkerRequest::CheckPassword { reply, .. } => {
                let _ = reply.send(PdfAccessStatus::Error(e.to_string()));
            }
            WorkerRequest::Render { reply, .. } => {
                let _ = reply.send(Err(std::io::Error::new(
                    std::io::ErrorKind::Other, e.to_string(),
                )));
            }
        }
    }

    fn do_enumerate(
        pdfium: &Pdfium,
        path: &Path,
        password: Option<&str>,
    ) -> std::io::Result<Vec<PdfPageEntry>> {
        let meta = std::fs::metadata(path)?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs() as i64);
        let file_size = meta.len();

        let doc = pdfium
            .load_pdf_from_file(path, password)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?;

        let count = doc.pages().len() as u32;
        Ok((0..count)
            .map(|i| PdfPageEntry {
                page_num: i,
                mtime,
                file_size,
            })
            .collect())
    }

    fn do_check_password(pdfium: &Pdfium, path: &Path) -> PdfAccessStatus {
        match pdfium.load_pdf_from_file(path, None) {
            Ok(_) => PdfAccessStatus::Ok,
            Err(PdfiumError::PdfiumLibraryInternalError(
                PdfiumInternalError::PasswordError,
            )) => PdfAccessStatus::PasswordRequired,
            Err(e) => PdfAccessStatus::Error(format!("{e}")),
        }
    }

    fn do_render(
        pdfium: &Pdfium,
        path: &Path,
        page_num: u32,
        target_px: u32,
        password: Option<&str>,
    ) -> std::io::Result<image::DynamicImage> {
        let doc = pdfium
            .load_pdf_from_file(path, password)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?;

        let page = doc
            .pages()
            .get(page_num as u16)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?;

        let page_w = page.width().value;
        let page_h = page.height().value;
        let (tw, th) = fit_to_target(page_w, page_h, target_px as f32);

        let render_config = PdfRenderConfig::new()
            .set_target_width(tw as i32)
            .set_maximum_height(th as i32);

        let bitmap = page
            .render_with_config(&render_config)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?;

        Ok(bitmap.as_image())
    }
}

// -----------------------------------------------------------------------
// 公開データ型
// -----------------------------------------------------------------------

/// PDF の 1 ページの情報。
pub struct PdfPageEntry {
    pub page_num: u32,
    pub mtime: i64,
    pub file_size: u64,
}

/// パスワード要否の判定結果。
pub enum PdfAccessStatus {
    Ok,
    PasswordRequired,
    Error(String),
}

// -----------------------------------------------------------------------
// 公開 API — 同期版 (バックグラウンドスレッド用)
// -----------------------------------------------------------------------

/// PDF のページ一覧を取得する (ブロッキング)。優先チャネル経由。
pub fn enumerate_pages(
    pdf_path: &Path,
    password: Option<&str>,
) -> std::io::Result<Vec<PdfPageEntry>> {
    let (tx, rx) = mpsc::channel();
    let _ = get_worker().priority_tx.send(WorkerRequest::Enumerate {
        path: pdf_path.to_path_buf(),
        password: password.map(String::from),
        reply: tx,
    });
    rx.recv()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?
}

/// 指定ページをレンダリングする (ブロッキング)。
/// サムネイルワーカー等のバックグラウンドスレッドから呼ぶ。通常チャネル経由。
/// `cancel` を渡すと、ワーカーが処理開始前にキャンセルチェックする。
pub fn render_page(
    pdf_path: &Path,
    page_num: u32,
    target_px: u32,
    password: Option<&str>,
    cancel: Option<Arc<AtomicBool>>,
) -> std::io::Result<image::DynamicImage> {
    let (tx, rx) = mpsc::channel();
    let _ = get_worker().tx.send(WorkerRequest::Render {
        path: pdf_path.to_path_buf(),
        page_num,
        target_px,
        password: password.map(String::from),
        cancel,
        reply: tx,
    });
    rx.recv()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?
}

/// パスワードが必要かどうかを判定する (ブロッキング)。優先チャネル経由。
pub fn check_password_needed(pdf_path: &Path) -> PdfAccessStatus {
    let (tx, rx) = mpsc::channel();
    let _ = get_worker().priority_tx.send(WorkerRequest::CheckPassword {
        path: pdf_path.to_path_buf(),
        reply: tx,
    });
    rx.recv().unwrap_or(PdfAccessStatus::Error("worker channel closed".to_string()))
}

// -----------------------------------------------------------------------
// 公開 API — 非同期版 (UI スレッド用)
// -----------------------------------------------------------------------

/// ページレンダリングを非同期でリクエストする (優先チャネル経由)。
/// 戻り値: `(cancel_token, receiver)`.
/// - `cancel_token` を `true` にセットするとワーカーが処理開始前にスキップする
/// - `receiver` を `.try_recv()` でポーリングする
pub fn render_page_async(
    pdf_path: &Path,
    page_num: u32,
    target_px: u32,
    password: Option<&str>,
) -> (Arc<AtomicBool>, mpsc::Receiver<std::io::Result<image::DynamicImage>>) {
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let _ = get_worker().priority_tx.send(WorkerRequest::Render {
        path: pdf_path.to_path_buf(),
        page_num,
        target_px,
        password: password.map(String::from),
        cancel: Some(Arc::clone(&cancel)),
        reply: tx,
    });
    (cancel, rx)
}

/// ページ列挙を非同期でリクエストする (優先チャネル経由)。
pub fn enumerate_pages_async(
    pdf_path: &Path,
    password: Option<&str>,
) -> mpsc::Receiver<std::io::Result<Vec<PdfPageEntry>>> {
    let (tx, rx) = mpsc::channel();
    let _ = get_worker().priority_tx.send(WorkerRequest::Enumerate {
        path: pdf_path.to_path_buf(),
        password: password.map(String::from),
        reply: tx,
    });
    rx
}

/// パスワードチェックを非同期でリクエストする (優先チャネル経由)。
pub fn check_password_async(
    pdf_path: &Path,
) -> mpsc::Receiver<PdfAccessStatus> {
    let (tx, rx) = mpsc::channel();
    let _ = get_worker().priority_tx.send(WorkerRequest::CheckPassword {
        path: pdf_path.to_path_buf(),
        reply: tx,
    });
    rx
}

// -----------------------------------------------------------------------
// 内部ユーティリティ
// -----------------------------------------------------------------------

/// PDF ページのポイント寸法を target ピクセルにフィットさせる。
/// PDF はベクター形式なので、ラスター画像と違って常にターゲット解像度に
/// スケーリングする（縮小だけでなくスケールアップも行う）。
fn fit_to_target(w: f32, h: f32, target: f32) -> (f32, f32) {
    let long = w.max(h);
    if long <= 0.0 {
        return (w, h);
    }
    let scale = target / long;
    (w * scale, h * scale)
}
