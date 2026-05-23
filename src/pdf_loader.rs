//! PDF ファイルの列挙・レンダリングモジュール。
//!
//! ## アーキテクチャ (マルチプロセス並列化)
//!
//! PDFium はスレッドセーフではない。pdfium-render の `thread_safe` feature は
//! 内部 Mutex で全操作を直列化するだけで性能向上なし。
//!
//! そこで、mImageViewer の exe 自体を `--pdf-worker` モードで起動する
//! ワーカープロセスプール (`PdfWorkerPool`) を実装。各ワーカーが独立に
//! PDFium を初期化し、真の並列レンダリングを実現する。
//!
//! ```text
//! [Main Process]
//!   ├── PdfWorkerPool (N 個のワーカープロセス)
//!   │     ├── Worker 0: mimageviewer.exe --pdf-worker
//!   │     ├── Worker 1: mimageviewer.exe --pdf-worker
//!   │     └── Worker 2: mimageviewer.exe --pdf-worker
//!   │
//!   └── PdfWorker (in-process, 優先チャネル用)
//!       Enumerate / CheckPassword / async Render は従来通り
//! ```
//!
//! 通信: stdin/stdout バイナリプロトコル (長さプレフィックス付き)。
//!
//! PDFium DLL は exe 内に埋め込まれており、初回アクセス時に
//! `%APPDATA%/mimageviewer/pdfium.dll` に展開される。

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock, mpsc};

/// PDF レンダ要求の優先度 (3 段階)。
///
/// `Critical` はフルスクリーンで今まさに表示中のページや、ユーザーが Enter で開いた
/// PDF の `enumerate_pages_async` など、ユーザーが即座の応答を待っているもの。
/// `HighNormal` はサムネイルグリッドで現在画面に見えている可視セル (`req.priority=true`)。
/// `Normal` は先読み・サムネイル・アイドル品質アップグレードなど画面外。
///
/// dispatcher の pop 順: `Critical → HighNormal → Normal`。
///
/// **常時 1 ワーカー予約**: プールが 2 ワーカー以上ある場合、HighNormal + Normal の
/// 同時実行数を `worker_count - 1` (最低 1) に制限し、残り 1 ワーカーを Critical 用に
/// 温存する。グリッドで先読みが 3 ワーカー全部を埋めて、`Enter` で開いた PDF の Critical
/// な enumerate が「in-flight な Normal IPC の終了を待つ」状態 (実測 2-3 秒) を防ぐ。
/// 代償はバルクサムネ生成のスループットが 3→2 になる -33%。手動キャッシュ作成等で
/// しか観測されない非対話処理なので受容範囲。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobPriority {
    Critical,
    /// 可視セルのサムネ render。`req.priority=true` の grid item から来る。
    /// Critical 予約下では `worker_count - 1` 枠を Normal と共有する。
    HighNormal,
    Normal,
}

/// `pool.execute` で cancel が立った時の待ちポリシー。
///
/// 通常 (`AbortOnCancel`) は cancel 検出と同時に `Err(Interrupted)` で early bail し、
/// dispatch 済み IPC があれば結果は dispatcher が silently 捨てる。これが既存挙動。
///
/// `HarvestOnCancel` は cancel が立っても reply を待ち続け、in-flight IPC があれば
/// 結果を受け取って caller に渡す。caller (= `load_one_cached`) は cache 保存に進める
/// ので「PDFium が既に処理した高価な render 結果」が捨てられず、ユーザが同フォルダに
/// 戻ったとき cache hit になる。
///
/// **`HarvestOnCancel` は thumbnail PDF render の cache-savable 経路のみ**で使う。
/// enumerate / Critical / background catch-up / bulk cache creator は `AbortOnCancel` の
/// まま。詳細は [docs/pdf-pool-harvest-on-cancel-plan.md](../docs/pdf-pool-harvest-on-cancel-plan.md)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelWaitPolicy {
    /// cancel.load() が true になった瞬間に `Err(Interrupted)` で抜ける (既存挙動)。
    AbortOnCancel,
    /// cancel が立っても reply を待ち続ける。in-flight IPC の結果を harvest できる。
    HarvestOnCancel,
}

// -----------------------------------------------------------------------
// Context epoch (render ジョブの世代管理)
// -----------------------------------------------------------------------

/// Render ジョブの「コンテキスト世代」。`App::bump_full_context_for_load` /
/// `App::bump_render_epoch_only` で +1 される。
/// 0 は予約値 (= epoch チェック対象外、background 経路用 sentinel)。
///
/// 用途: フォルダ移動 / Ctrl+G 検索結果差替えで、それ以前の UI nav から enqueue された
/// render ジョブを pool 内で stale 化して prune する。これにより、ユーザが新しい
/// コンテキストに入った瞬間、見えていない古い PDF のページレンダリングが queue から
/// 一掃される。
static CURRENT_CONTEXT_EPOCH: AtomicU64 = AtomicU64::new(1);

/// 現在の context epoch を読む。UI スレッドが `LoadRequest.context_epoch` に
/// 焼き付けるために使う (TOCTOU 防止のため worker thread からは呼ばない)。
pub fn current_render_context_epoch() -> u64 {
    CURRENT_CONTEXT_EPOCH.load(Ordering::Relaxed)
}

/// Context epoch を +1 して、pool 内の stale Normal/HighNormal ジョブを即座に prune する。
/// Critical ジョブは touch しない (UI nav の直結なので常に fresh)。
/// 戻り値は bump 後の新 epoch。
pub fn bump_render_context_epoch() -> u64 {
    let new = CURRENT_CONTEXT_EPOCH.fetch_add(1, Ordering::Relaxed) + 1;
    if let Some(pool) = POOL.get() {
        pool.prune_stale_jobs(new);
    }
    new
}

/// Critical 用ワーカー予約フラグ。
///
/// 既定で `true` (常時 ON)。グリッド表示中・フルスクリーン中どちらでも Critical は
/// `pool_dispatch` の wait_ms がほぼ 0 になる。`false` にする呼び出しは現状無いが、
/// 将来「対話 UI を一切使わないバルク処理を一時的に走らせる」用途のため API は残す。
static CRITICAL_RESERVATION_ACTIVE: AtomicBool = AtomicBool::new(true);

/// 予約モードを切り替える (将来のバルク処理拡張用)。現状の通常運用では呼び出さない。
pub fn set_critical_reservation(active: bool) {
    CRITICAL_RESERVATION_ACTIVE.store(active, Ordering::Relaxed);
}

fn critical_reservation_active() -> bool {
    CRITICAL_RESERVATION_ACTIVE.load(Ordering::Relaxed)
}

use pdfium_render::prelude::*;

// -----------------------------------------------------------------------
// 定数
// -----------------------------------------------------------------------

/// ワーカープロセス起動時の引数。main.rs と pdf_loader.rs の両方で参照。
pub const PDF_WORKER_ARG: &str = "--pdf-worker";

/// Windows: ワーカープロセスがコンソールウィンドウを表示しないようにするフラグ。
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

// -----------------------------------------------------------------------
// PDFium DLL 埋め込み & 展開
// -----------------------------------------------------------------------

static PDFIUM_DLL_BYTES: &[u8] = include_bytes!("../vendor/pdfium/bin/pdfium.dll");

static DLL_PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();

fn ensure_dll_extracted() -> Result<&'static PathBuf, String> {
    DLL_PATH
        .get_or_init(|| {
            let dir = crate::data_dir::get();
            std::fs::create_dir_all(&dir).map_err(|e| format!("data_dir create failed: {e}"))?;
            let dll_path = dir.join("pdfium.dll");
            crate::data_dir::extract_embedded_file(&dll_path, PDFIUM_DLL_BYTES, "pdfium.dll")
                .map_err(|e| format!("pdfium.dll extract failed: {e}"))?;
            Ok(dll_path)
        })
        .as_ref()
        .map_err(|e| e.clone())
}

// -----------------------------------------------------------------------
// 共通 PDFium 操作 (IPC ワーカー / in-process ワーカー両方で使用)
// -----------------------------------------------------------------------

// ── ページコンテンツ解析 ──

/// ページ内のオブジェクトを走査し、ラスター/ベクターを判定する。
fn analyze_page_content(page: &PdfPage) -> PdfPageContentType {
    let mut has_vector = false;
    let mut image_sizes: Vec<(u32, u32)> = Vec::new();

    analyze_objects(page.objects().iter(), &mut has_vector, &mut image_sizes);

    if has_vector || image_sizes.is_empty() {
        return PdfPageContentType::Vector;
    }

    // ラスターのみ: 単一画像ならそのサイズ、複数タイルなら合算推定
    if image_sizes.len() == 1 {
        let (w, h) = image_sizes[0];
        PdfPageContentType::Raster { w, h }
    } else {
        estimate_tiled_size(&image_sizes)
    }
}

/// オブジェクトイテレータを走査し、ベクター要素の有無と画像サイズを収集する。
/// XObjectForm は再帰的に走査する。
fn analyze_objects<'a>(
    iter: impl Iterator<Item = PdfPageObject<'a>>,
    has_vector: &mut bool,
    image_sizes: &mut Vec<(u32, u32)>,
) {
    for obj in iter {
        if *has_vector {
            return; // 早期打ち切り
        }
        match obj {
            PdfPageObject::Image(ref img) => {
                let w = img.width().unwrap_or(0).max(0) as u32;
                let h = img.height().unwrap_or(0).max(0) as u32;
                if w > 0 && h > 0 {
                    image_sizes.push((w, h));
                }
            }
            PdfPageObject::Text(ref txt) => {
                if is_visible_text(txt) {
                    *has_vector = true;
                }
            }
            PdfPageObject::Path(_) | PdfPageObject::Shading(_) => {
                *has_vector = true;
            }
            PdfPageObject::XObjectForm(ref form) => {
                analyze_objects(form.iter(), has_vector, image_sizes);
            }
            PdfPageObject::Unsupported(_) => {}
        }
    }
}

/// テキストオブジェクトが可視かどうかを判定する。
/// OCR テキストレイヤー (Invisible モードまたは完全透明) は不可視と見なす。
fn is_visible_text(txt: &PdfPageTextObject) -> bool {
    // render_mode が Invisible 系なら不可視
    let mode = txt.render_mode();
    if matches!(
        mode,
        PdfPageTextRenderMode::Invisible | PdfPageTextRenderMode::InvisibleClipping
    ) {
        return false;
    }
    // フィルカラーとストロークカラーの両方が完全透明なら不可視
    let fill_alpha = txt.fill_color().ok().map(|c| c.alpha()).unwrap_or(255);
    let stroke_alpha = txt.stroke_color().ok().map(|c| c.alpha()).unwrap_or(255);
    if fill_alpha == 0 && stroke_alpha == 0 {
        return false;
    }
    true
}

/// 複数タイル画像の合算サイズを推定する。
/// 同じ幅のタイルが縦に並んでいると仮定し合算する。
/// 推定できなければ最大画像のサイズを返す。
fn estimate_tiled_size(sizes: &[(u32, u32)]) -> PdfPageContentType {
    // 全タイルの幅が一致しているか確認 (横タイリング)
    let all_same_w = sizes.windows(2).all(|p| p[0].0 == p[1].0);
    if all_same_w {
        let w = sizes[0].0;
        let h: u32 = sizes.iter().map(|(_, h)| h).sum();
        return PdfPageContentType::Raster { w, h };
    }
    // 全タイルの高さが一致しているか確認 (縦タイリング)
    let all_same_h = sizes.windows(2).all(|p| p[0].1 == p[1].1);
    if all_same_h {
        let w: u32 = sizes.iter().map(|(w, _)| w).sum();
        let h = sizes[0].1;
        return PdfPageContentType::Raster { w, h };
    }
    // 推定不可: 最大面積の画像サイズを返す
    let (w, h) = sizes
        .iter()
        .max_by_key(|(w, h)| (*w as u64) * (*h as u64))
        .copied()
        .unwrap_or((0, 0));
    PdfPageContentType::Raster { w, h }
}

/// PDF のページ一覧を列挙する (コアロジック)。
fn core_enumerate(
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

// (旧 `core_render` は `core_render_with_count` (v1.0.0 で page_count も返す)
//  に置き換えられた。後者は ipc_render と in-process worker の両方から使われる。)
//
// **lopdf fast-path 検討 (撤回、v1.0.0 開発中)**:
// `Document::load` は eager に全オブジェクトをパースする実装で、PDFium の lazy
// 経路 (warm 0.4ms p50) より 200-12000x 遅いことが `examples/bench_pdf_count.rs`
// で判明した (lopdf p50=77ms、p99=5344ms)。production cold 開封 (824-1338ms) を
// 改善する目的だったが、lopdf を挟むと逆に CPU で 76ms-7s 上乗せされるので採用見送り。
// 改善するなら自前 xref パーサーが必要 (= docs/pdf-page-count-cache-plan.md の
// Option B、工数 3-5 日)。`examples/bench_pdf_count.rs` (dev-dependencies)
// に比較ベンチマークを残してあるので、将来再検討する際の比較ベースに使える。

// -----------------------------------------------------------------------
// バイナリプロトコル (stdin/stdout IPC)
// -----------------------------------------------------------------------
//
// リクエスト (main → worker):
//   [4B msg_len LE][1B msg_type][payload]
//     Enumerate (1): [2B path_len][path_utf8][2B pw_len][pw_utf8]
//     Render    (2): [2B path_len][path_utf8][4B page_num][4B target_px][2B pw_len][pw_utf8]
//     Shutdown  (3): (no payload)
//
// レスポンス (worker → main):
//   [4B msg_len LE][1B status][payload]
//     Success (0):
//       Enumerate: [4B page_count][per page: 8B mtime LE + 8B file_size LE]
//       Render:    [4B width][4B height][rgba_bytes...]
//     Error (1): [error_message_utf8]

const MSG_ENUMERATE: u8 = 1;
const MSG_RENDER: u8 = 2;
const MSG_SHUTDOWN: u8 = 3;
/// PDF document info (Title / Author / Subject / Keywords) を返す。
/// 全文検索インデクサが PDF メタ情報を ingest するために使う (§16 step 17)。
const MSG_GET_INFO: u8 = 4;
const STATUS_OK: u8 = 0;
const STATUS_ERR: u8 = 1;

fn write_msg(w: &mut impl std::io::Write, data: &[u8]) -> std::io::Result<()> {
    let len = data.len() as u32;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(data)?;
    w.flush()
}

fn read_msg(r: &mut impl std::io::Read) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 512 * 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("message too large: {len} bytes"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

/// パス + パスワードをバッファに書き込む (Enumerate / Render 共通)。
fn encode_path_and_password(buf: &mut Vec<u8>, path: &Path, password: Option<&str>) {
    let path_lossy = path.to_string_lossy();
    let path_bytes = path_lossy.as_bytes();
    let pw_bytes = password.unwrap_or("").as_bytes();
    buf.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(path_bytes);
    buf.extend_from_slice(&(pw_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(pw_bytes);
}

fn encode_enumerate_request(path: &Path, password: Option<&str>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    buf.push(MSG_ENUMERATE);
    encode_path_and_password(&mut buf, path, password);
    buf
}

fn encode_get_info_request(path: &Path, password: Option<&str>) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    buf.push(MSG_GET_INFO);
    encode_path_and_password(&mut buf, path, password);
    buf
}

fn encode_render_request(
    path: &Path,
    page_num: u32,
    target_px: u32,
    password: Option<&str>,
) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    buf.push(MSG_RENDER);
    let path_lossy = path.to_string_lossy();
    let path_bytes = path_lossy.as_bytes();
    let pw_bytes = password.unwrap_or("").as_bytes();
    buf.extend_from_slice(&(path_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(path_bytes);
    buf.extend_from_slice(&page_num.to_le_bytes());
    buf.extend_from_slice(&target_px.to_le_bytes());
    buf.extend_from_slice(&(pw_bytes.len() as u16).to_le_bytes());
    buf.extend_from_slice(pw_bytes);
    buf
}

fn encode_shutdown_request() -> Vec<u8> {
    vec![MSG_SHUTDOWN]
}

/// パス + パスワードをペイロードからデコードし、残りスライスも返す。
fn decode_path_and_password(payload: &[u8]) -> std::io::Result<(PathBuf, Option<String>, &[u8])> {
    if payload.len() < 2 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "payload too short for path",
        ));
    }
    let path_len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
    if payload.len() < 2 + path_len + 2 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "payload truncated",
        ));
    }
    let path_str = std::str::from_utf8(&payload[2..2 + path_len])
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    let rest = &payload[2 + path_len..];
    let pw_len = u16::from_le_bytes([rest[0], rest[1]]) as usize;
    let password = if pw_len > 0 {
        Some(
            std::str::from_utf8(&rest[2..2 + pw_len])
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
                .to_string(),
        )
    } else {
        None
    };
    let remaining = &rest[2 + pw_len..];
    Ok((PathBuf::from(path_str), password, remaining))
}

fn decode_request(data: &[u8]) -> std::io::Result<DecodedRequest> {
    if data.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "empty request",
        ));
    }
    let msg_type = data[0];
    let payload = &data[1..];
    match msg_type {
        MSG_ENUMERATE => {
            let (path, password, _) = decode_path_and_password(payload)?;
            Ok(DecodedRequest::Enumerate { path, password })
        }
        MSG_RENDER => {
            // Render: [path][page_num(4B)][target_px(4B)][password]
            // path_len(2B) + path + page_num + target_px の後にパスワード
            if payload.len() < 2 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "render request too short",
                ));
            }
            let path_len = u16::from_le_bytes([payload[0], payload[1]]) as usize;
            if payload.len() < 2 + path_len + 8 + 2 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "render request truncated",
                ));
            }
            let path_str = std::str::from_utf8(&payload[2..2 + path_len])
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let after_path = &payload[2 + path_len..];
            let page_num =
                u32::from_le_bytes([after_path[0], after_path[1], after_path[2], after_path[3]]);
            let target_px =
                u32::from_le_bytes([after_path[4], after_path[5], after_path[6], after_path[7]]);
            let pw_payload = &after_path[8..];
            let pw_len = u16::from_le_bytes([pw_payload[0], pw_payload[1]]) as usize;
            let password = if pw_len > 0 && pw_payload.len() >= 2 + pw_len {
                Some(
                    std::str::from_utf8(&pw_payload[2..2 + pw_len])
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?
                        .to_string(),
                )
            } else {
                None
            };
            Ok(DecodedRequest::Render {
                path: PathBuf::from(path_str),
                page_num,
                target_px,
                password,
            })
        }
        MSG_SHUTDOWN => Ok(DecodedRequest::Shutdown),
        MSG_GET_INFO => {
            let (path, password, _) = decode_path_and_password(payload)?;
            Ok(DecodedRequest::GetInfo { path, password })
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("unknown message type: {msg_type}"),
        )),
    }
}

enum DecodedRequest {
    Enumerate {
        path: PathBuf,
        password: Option<String>,
    },
    Render {
        path: PathBuf,
        page_num: u32,
        target_px: u32,
        password: Option<String>,
    },
    GetInfo {
        path: PathBuf,
        password: Option<String>,
    },
    Shutdown,
}

// -----------------------------------------------------------------------
// ワーカープロセス側 (--pdf-worker モード)
// -----------------------------------------------------------------------

/// `--pdf-worker` 引数で起動された場合に呼ばれる。
/// stdin からリクエストを読み、PDFium で処理し、stdout にレスポンスを書く。
/// stdin が閉じたら (メインプロセス終了) 自動終了する。
pub fn run_worker_process() {
    let dll_path = match ensure_dll_extracted() {
        Ok(p) => p.clone(),
        Err(e) => {
            eprintln!("pdf-worker: DLL extract failed: {e}");
            return;
        }
    };
    let dll_dir = match dll_path.parent() {
        Some(d) => d,
        None => {
            eprintln!("pdf-worker: cannot determine DLL directory");
            return;
        }
    };

    let bindings = match Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(
        dll_dir.to_str().unwrap_or(""),
    )) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("pdf-worker: PDFium binding failed: {e}");
            return;
        }
    };
    let pdfium = Pdfium::new(bindings);

    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();

    loop {
        let msg = match read_msg(&mut stdin) {
            Ok(m) => m,
            Err(_) => break,
        };

        let req = match decode_request(&msg) {
            Ok(r) => r,
            Err(e) => {
                let _ = send_error(&mut stdout, &format!("decode error: {e}"));
                continue;
            }
        };

        match req {
            DecodedRequest::Enumerate { path, password } => {
                match ipc_enumerate(&pdfium, &path, password.as_deref()) {
                    Ok(resp) => {
                        let _ = write_msg(&mut stdout, &resp);
                    }
                    Err(e) => {
                        let _ = send_error(&mut stdout, &e.to_string());
                    }
                }
            }
            DecodedRequest::Render {
                path,
                page_num,
                target_px,
                password,
            } => match ipc_render(&pdfium, &path, page_num, target_px, password.as_deref()) {
                Ok(resp) => {
                    let _ = write_msg(&mut stdout, &resp);
                }
                Err(e) => {
                    let _ = send_error(&mut stdout, &e.to_string());
                }
            },
            DecodedRequest::GetInfo { path, password } => {
                match ipc_get_info(&pdfium, &path, password.as_deref()) {
                    Ok(resp) => {
                        let _ = write_msg(&mut stdout, &resp);
                    }
                    Err(e) => {
                        let _ = send_error(&mut stdout, &e.to_string());
                    }
                }
            }
            DecodedRequest::Shutdown => break,
        }
    }
}

fn send_error(w: &mut impl std::io::Write, msg: &str) -> std::io::Result<()> {
    let mut buf = Vec::with_capacity(1 + msg.len());
    buf.push(STATUS_ERR);
    buf.extend_from_slice(msg.as_bytes());
    write_msg(w, &buf)
}

/// core_enumerate の結果を IPC バイナリにシリアライズする。
fn ipc_enumerate(pdfium: &Pdfium, path: &Path, password: Option<&str>) -> std::io::Result<Vec<u8>> {
    let entries = core_enumerate(pdfium, path, password)?;
    let count = entries.len() as u32;
    let mut buf = Vec::with_capacity(1 + 4 + entries.len() * 16);
    buf.push(STATUS_OK);
    buf.extend_from_slice(&count.to_le_bytes());
    for e in &entries {
        buf.extend_from_slice(&e.mtime.to_le_bytes());
        buf.extend_from_slice(&e.file_size.to_le_bytes());
    }
    Ok(buf)
}

/// PDF document metadata (Title / Author / Subject / Keywords) を取得する。
///
/// 全文検索インデクサが PDF のタイトル等を ingest するために使う (§16 step 17)。
/// pdfium-render の PdfDocument::metadata() で取れる 4 タグだけを抽出する。
/// v1 では他のタグ (Creator / Producer / *Date) は取らない (検索価値が低い)。
fn core_get_info(
    pdfium: &Pdfium,
    path: &Path,
    password: Option<&str>,
) -> std::io::Result<PdfDocumentInfo> {
    use pdfium_render::prelude::PdfDocumentMetadataTagType;

    let doc = pdfium
        .load_pdf_from_file(path, password)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?;
    let metadata = doc.metadata();
    let extract = |tag: PdfDocumentMetadataTagType| -> Option<String> {
        metadata.get(tag).and_then(|t| {
            let v = t.value().to_string();
            if v.is_empty() { None } else { Some(v) }
        })
    };
    Ok(PdfDocumentInfo {
        title: extract(PdfDocumentMetadataTagType::Title),
        author: extract(PdfDocumentMetadataTagType::Author),
        subject: extract(PdfDocumentMetadataTagType::Subject),
        keywords: extract(PdfDocumentMetadataTagType::Keywords),
    })
}

/// core_get_info の結果を IPC バイナリにシリアライズする。
/// フォーマット: [status][4B title_len][title_bytes][4B author_len][author_bytes]...×4
fn ipc_get_info(pdfium: &Pdfium, path: &Path, password: Option<&str>) -> std::io::Result<Vec<u8>> {
    let info = core_get_info(pdfium, path, password)?;
    let mut buf = Vec::with_capacity(256);
    buf.push(STATUS_OK);
    for field in [&info.title, &info.author, &info.subject, &info.keywords] {
        let s = field.as_deref().unwrap_or("");
        let bytes = s.as_bytes();
        buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        buf.extend_from_slice(bytes);
    }
    Ok(buf)
}

/// core_render の結果を IPC バイナリ (RGBA ピクセル) にシリアライズする。
///
/// レスポンス: [status 1B][w 4B][h 4B][type_tag 1B][raster_w 4B][raster_h 4B][page_count 4B][rgba_pixels]
/// ピクセル開始オフセット = 22B。`page_count` は呼び出し側 (thumb_loader) で PDF メタ
/// キャッシュへ書き込むために返している (v1.0.0、`pdf_meta` テーブル)。
fn ipc_render(
    pdfium: &Pdfium,
    path: &Path,
    page_num: u32,
    target_px: u32,
    password: Option<&str>,
) -> std::io::Result<Vec<u8>> {
    let (img, content_type, page_count) =
        core_render_with_count(pdfium, path, page_num, target_px, password)?;
    let rgba = img.to_rgba8();
    let w = rgba.width();
    let h = rgba.height();
    let pixels = rgba.as_raw();
    let mut buf = Vec::with_capacity(1 + 4 + 4 + 9 + 4 + pixels.len());
    buf.push(STATUS_OK);
    buf.extend_from_slice(&w.to_le_bytes());
    buf.extend_from_slice(&h.to_le_bytes());
    match content_type {
        PdfPageContentType::Vector => {
            buf.push(0);
            buf.extend_from_slice(&0u32.to_le_bytes());
            buf.extend_from_slice(&0u32.to_le_bytes());
        }
        PdfPageContentType::Raster { w: rw, h: rh } => {
            buf.push(1);
            buf.extend_from_slice(&rw.to_le_bytes());
            buf.extend_from_slice(&rh.to_le_bytes());
        }
    }
    buf.extend_from_slice(&page_count.to_le_bytes());
    buf.extend_from_slice(pixels);
    Ok(buf)
}

/// core_render に page_count 取得を追加した拡張版 (v1.0.0)。
/// ipc_render と in-process worker から共用する。
fn core_render_with_count(
    pdfium: &Pdfium,
    path: &Path,
    page_num: u32,
    target_px: u32,
    password: Option<&str>,
) -> std::io::Result<(image::DynamicImage, PdfPageContentType, u32)> {
    let doc = pdfium
        .load_pdf_from_file(path, password)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?;
    let page_count = doc.pages().len() as u32;
    let page = doc
        .pages()
        .get(page_num as u16)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?;
    let content_type = analyze_page_content(&page);
    let page_w = page.width().value;
    let page_h = page.height().value;
    let (tw, th) = fit_to_target(page_w, page_h, target_px as f32);
    let render_config = PdfRenderConfig::new()
        .set_target_width(tw as i32)
        .set_maximum_height(th as i32);
    let bitmap = page
        .render_with_config(&render_config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?;
    Ok((bitmap.as_image(), content_type, page_count))
}

// -----------------------------------------------------------------------
// ワーカープロセスプール (メインプロセス側)
// -----------------------------------------------------------------------
//
// 設計: 優先度キュー + ディスパッチャースレッド
//
// 従来は `Mutex<ProcessWorkerIo>` を共有して各リクエストが try_lock ポーリング
// する方式だったが、10ms ポーリングの隙間に新着スレッドが横取りする飢餓バグが
// あり、特定スレッドが秒単位で詰まることがあった。
//
// 新設計:
// - 共有 JobQueue (Mutex + Condvar) に Critical / Normal の 2 レベル優先度
// - 各 worker プロセスに専用ディスパッチャースレッドを置き、stdin/stdout を
//   スレッド内に閉じ込める (共有 Mutex 不要)
// - リクエスト側は Job を enqueue → `mpsc::Receiver` で応答待ち (途中で
//   cancel が立てば早期 bail)
// - worker スレッドは Condvar で起床し、Critical を先に取り、続いて Normal
//   (予約中は `max_normal` 制限) を pop する。pop 時に cancel チェック、
//   セットされていれば IPC せず Err を送る。

struct ProcessWorkerIo {
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

fn spawn_worker_process(exe_path: &Path) -> std::io::Result<(Child, ProcessWorkerIo)> {
    let mut cmd = Command::new(exe_path);
    cmd.arg(PDF_WORKER_ARG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "no stdout"))?;
    Ok((
        child,
        ProcessWorkerIo {
            stdin,
            stdout: std::io::BufReader::new(stdout),
        },
    ))
}

fn send_recv_io(io: &mut ProcessWorkerIo, request: &[u8]) -> std::io::Result<Vec<u8>> {
    write_msg(&mut io.stdin, request)?;
    read_msg(&mut io.stdout)
}

/// ディスパッチャースレッドに渡される 1 件のジョブ。
struct Job {
    request: Vec<u8>,
    cancel: Option<Arc<AtomicBool>>,
    reply: mpsc::Sender<std::io::Result<Vec<u8>>>,
    priority: JobPriority,
    enqueued_at: std::time::Instant,
    /// perf 相関キー (存在すれば dispatch/cancel イベントに載せる)
    perf_key: Option<String>,
    /// enqueue 時点の context epoch。`CURRENT_CONTEXT_EPOCH` より小さければ stale。
    /// 0 = epoch チェック対象外 (background 経路 / Critical 用 sentinel)。
    context_epoch: u64,
}

struct JobQueue {
    critical: std::collections::VecDeque<Job>,
    /// 可視セルのサムネ render。Normal より先に pop される (両方とも `normal_in_flight` 枠を共有)。
    high_normal: std::collections::VecDeque<Job>,
    normal: std::collections::VecDeque<Job>,
    /// 現在処理中の HighNormal + Normal ジョブ数 (`max_normal` 以下に制限)。
    /// Critical はこのカウントに含めない (= 予約枠を消費しない)。
    normal_in_flight: usize,
    /// 現在 IPC 実行中のワーカー数 (perf 用)
    workers_busy: usize,
    /// Drop 時に true になり、ディスパッチャースレッドが cleanly 終了する
    shutdown: bool,
}

struct PdfWorkerPool {
    queue: Arc<(Mutex<JobQueue>, Condvar)>,
    /// 起動したワーカープロセス (subprocess) の数
    worker_count: usize,
    /// ディスパッチャースレッド (Pool drop 時に join する)
    dispatcher_threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

const POOL_SIZE: usize = 3;

static POOL: OnceLock<PdfWorkerPool> = OnceLock::new();

fn get_pool() -> &'static PdfWorkerPool {
    POOL.get_or_init(|| PdfWorkerPool::start())
}

impl PdfWorkerPool {
    fn start() -> Self {
        let exe_path =
            std::env::current_exe().unwrap_or_else(|_| PathBuf::from("mimageviewer.exe"));
        let _ = ensure_dll_extracted();

        let queue = Arc::new((
            Mutex::new(JobQueue {
                critical: std::collections::VecDeque::new(),
                high_normal: std::collections::VecDeque::new(),
                normal: std::collections::VecDeque::new(),
                normal_in_flight: 0,
                workers_busy: 0,
                shutdown: false,
            }),
            Condvar::new(),
        ));

        // 子プロセスを先に全部 spawn してから worker_count を確定させ、その値を
        // dispatcher スレッドに渡す (run_dispatcher が `max_normal` を計算するときに
        // 「実際に生きているワーカー数」を使うため。POOL_SIZE 固定だと、起動失敗で
        // 1-2 worker しか居ない degraded 環境で `max_normal` が間違って計算され、
        // Critical 予約が機能しなくなる)。
        let mut pending_workers: Vec<(usize, Child, ProcessWorkerIo)> =
            Vec::with_capacity(POOL_SIZE);
        for i in 0..POOL_SIZE {
            match spawn_worker_process(&exe_path) {
                Ok((child, io)) => {
                    let pid = child.id();
                    crate::logger::log(format!("pdf-pool: worker {i} started (pid={pid})"));
                    pending_workers.push((i, child, io));
                }
                Err(e) => {
                    crate::logger::log(format!("pdf-pool: worker {i} spawn failed: {e}"));
                }
            }
        }
        let worker_count = pending_workers.len();

        if worker_count == 0 {
            crate::logger::log("pdf-pool: WARNING: no workers spawned, falling back to in-process");
        } else {
            crate::logger::log(format!("pdf-pool: {worker_count} workers ready"));
        }

        let mut dispatcher_threads = Vec::with_capacity(worker_count);
        for (i, child, io) in pending_workers {
            let q = Arc::clone(&queue);
            let actual_workers = worker_count;
            let handle = std::thread::Builder::new()
                .name(format!("pdf-pool-{i}"))
                .spawn(move || run_dispatcher(i, actual_workers, q, child, io))
                .expect("failed to spawn pdf-pool dispatcher thread");
            dispatcher_threads.push(handle);
        }

        PdfWorkerPool {
            queue,
            worker_count,
            dispatcher_threads: Mutex::new(dispatcher_threads),
        }
    }

    /// 現在 IPC 実行中のワーカー数 (perf イベント用の snapshot)。
    fn workers_busy(&self) -> usize {
        self.queue.0.lock().map(|q| q.workers_busy).unwrap_or(0)
    }

    /// `current_epoch` より小さい `context_epoch` を持つ HighNormal / Normal ジョブを
    /// 一括 prune する。各ジョブの reply に `Interrupted` を送って requester を解放する。
    /// Critical はプルーンしない (= UI nav の直結は守る)。
    /// `context_epoch == 0` のジョブは epoch チェック対象外なのでプルーンしない。
    ///
    /// `bump_render_context_epoch()` から呼ばれる。
    fn prune_stale_jobs(&self, current_epoch: u64) {
        let drained: Vec<Job> = {
            let (mtx, _cv) = &*self.queue;
            let mut q = mtx.lock().unwrap();
            let mut dropped: Vec<Job> = Vec::new();
            let mut filter_queue = |queue: &mut std::collections::VecDeque<Job>| {
                let mut kept: std::collections::VecDeque<Job> =
                    std::collections::VecDeque::with_capacity(queue.len());
                while let Some(j) = queue.pop_front() {
                    if j.context_epoch != 0 && j.context_epoch < current_epoch {
                        dropped.push(j);
                    } else {
                        kept.push_back(j);
                    }
                }
                *queue = kept;
            };
            filter_queue(&mut q.high_normal);
            filter_queue(&mut q.normal);
            dropped
        };

        // Mutex 外で reply を送る (notify_one で deadlock しないように)
        let count = drained.len();
        for j in drained {
            if crate::perf::is_enabled() {
                let waited_ms = j.enqueued_at.elapsed().as_secs_f64() * 1000.0;
                crate::perf::event(
                    "pdf",
                    "pool_prune_stale_epoch",
                    j.perf_key.as_deref(),
                    0,
                    &[
                        ("waited_ms", serde_json::Value::from(waited_ms)),
                        ("job_epoch", serde_json::Value::from(j.context_epoch)),
                        ("current_epoch", serde_json::Value::from(current_epoch)),
                        (
                            "priority",
                            serde_json::Value::from(format!("{:?}", j.priority)),
                        ),
                    ],
                );
            }
            let _ = j.reply.send(Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "context epoch advanced",
            )));
        }
        if count > 0 {
            crate::logger::log(format!(
                "pdf-pool: pruned {count} stale jobs (current_epoch={current_epoch})"
            ));
        }
    }

    fn execute(
        &self,
        request: &[u8],
        cancel: Option<&Arc<AtomicBool>>,
        priority: JobPriority,
        perf_key: Option<String>,
        context_epoch: u64,
        cancel_policy: CancelWaitPolicy,
    ) -> std::io::Result<Vec<u8>> {
        if self.worker_count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "no pdf worker processes available",
            ));
        }

        let (reply_tx, reply_rx) = mpsc::channel();
        // perf_key を後段 (cancel 検出時の perf イベント) でも使うので clone
        let job = Job {
            request: request.to_vec(),
            cancel: cancel.cloned(),
            reply: reply_tx,
            priority,
            enqueued_at: std::time::Instant::now(),
            perf_key: perf_key.clone(),
            context_epoch,
        };

        // Job をキューに積んで worker を 1 つ起こす
        {
            let (mtx, cv) = &*self.queue;
            let mut q = mtx.lock().unwrap();
            match priority {
                JobPriority::Critical => q.critical.push_back(job),
                JobPriority::HighNormal => q.high_normal.push_back(job),
                JobPriority::Normal => q.normal.push_back(job),
            }
            cv.notify_one();
        }

        // 応答を待つ。cancel フラグが途中で立った場合の挙動は `cancel_policy` で決まる:
        // - AbortOnCancel: 早期 bail し Err(Interrupted) を返す (dispatcher は IPC 結果が
        //   来たら reply.send で silently 捨てる)
        // - HarvestOnCancel: cancel が立っても reply を待ち続け、in-flight IPC があれば
        //   結果を harvest する。caller (= load_one_cached) が cache 保存に進めて、
        //   PDFium の処理結果を投資回収する
        let t_wait = std::time::Instant::now();
        let mut harvest_logged = false;
        loop {
            match reply_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(c) = cancel
                        && c.load(Ordering::Relaxed)
                    {
                        match cancel_policy {
                            CancelWaitPolicy::AbortOnCancel => {
                                if crate::perf::is_enabled() {
                                    let waited_ms = t_wait.elapsed().as_secs_f64() * 1000.0;
                                    crate::perf::event(
                                        "pdf",
                                        "pool_cancel_requester",
                                        perf_key.as_deref(),
                                        0,
                                        &[("waited_ms", serde_json::Value::from(waited_ms))],
                                    );
                                }
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::Interrupted,
                                    "cancelled while waiting for reply",
                                ));
                            }
                            CancelWaitPolicy::HarvestOnCancel => {
                                // 初回のみ perf イベント (harvest 待ちが発動した旨)。
                                // 以降は同じループで何度 cancel.load() が true でも追加発火しない。
                                if !harvest_logged {
                                    harvest_logged = true;
                                    if crate::perf::is_enabled() {
                                        let waited_ms = t_wait.elapsed().as_secs_f64() * 1000.0;
                                        crate::perf::event(
                                            "pdf",
                                            "pool_cancel_harvest_wait",
                                            perf_key.as_deref(),
                                            0,
                                            &[("waited_ms", serde_json::Value::from(waited_ms))],
                                        );
                                    }
                                }
                                // 待ち継続 → 次の iteration で reply を待つ
                            }
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "pdf-pool dispatcher disconnected",
                    ));
                }
            }
        }
    }

    fn parse_enumerate_response(data: &[u8]) -> std::io::Result<Vec<PdfPageEntry>> {
        if data.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "empty response",
            ));
        }
        if data[0] == STATUS_ERR {
            let msg = std::str::from_utf8(&data[1..]).unwrap_or("unknown error");
            return Err(std::io::Error::new(std::io::ErrorKind::Other, msg));
        }
        if data[0] != STATUS_OK || data.len() < 5 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid enumerate response",
            ));
        }
        let count = u32::from_le_bytes([data[1], data[2], data[3], data[4]]) as usize;
        let mut entries = Vec::with_capacity(count);
        let mut offset = 5;
        for i in 0..count {
            if offset + 16 > data.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "enumerate response truncated",
                ));
            }
            let mtime = i64::from_le_bytes(data[offset..offset + 8].try_into().unwrap());
            let file_size = u64::from_le_bytes(data[offset + 8..offset + 16].try_into().unwrap());
            entries.push(PdfPageEntry {
                page_num: i as u32,
                mtime,
                file_size,
            });
            offset += 16;
        }
        Ok(entries)
    }

    /// `ipc_get_info` レスポンスを PdfDocumentInfo にデコード。
    /// フォーマット: [status][4B title_len][title_bytes][4B author_len][author_bytes]
    ///             [4B subject_len][subject_bytes][4B keywords_len][keywords_bytes]
    fn parse_get_info_response(data: &[u8]) -> std::io::Result<PdfDocumentInfo> {
        if data.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "empty response",
            ));
        }
        if data[0] == STATUS_ERR {
            let msg = std::str::from_utf8(&data[1..]).unwrap_or("unknown error");
            return Err(std::io::Error::new(std::io::ErrorKind::Other, msg));
        }
        if data[0] != STATUS_OK {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid get_info response",
            ));
        }
        let mut offset = 1;
        let read_field = |data: &[u8], offset: &mut usize| -> std::io::Result<Option<String>> {
            if *offset + 4 > data.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "get_info truncated",
                ));
            }
            let len = u32::from_le_bytes(data[*offset..*offset + 4].try_into().unwrap()) as usize;
            *offset += 4;
            if *offset + len > data.len() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "get_info field truncated",
                ));
            }
            let s = if len == 0 {
                None
            } else {
                let text = std::str::from_utf8(&data[*offset..*offset + len])
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
                Some(text.to_string())
            };
            *offset += len;
            Ok(s)
        };
        let title = read_field(data, &mut offset)?;
        let author = read_field(data, &mut offset)?;
        let subject = read_field(data, &mut offset)?;
        let keywords = read_field(data, &mut offset)?;
        Ok(PdfDocumentInfo {
            title,
            author,
            subject,
            keywords,
        })
    }

    fn parse_render_response(data: &[u8]) -> std::io::Result<RenderResult> {
        if data.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "empty response",
            ));
        }
        if data[0] == STATUS_ERR {
            let msg = std::str::from_utf8(&data[1..]).unwrap_or("unknown error");
            return Err(std::io::Error::new(std::io::ErrorKind::Other, msg));
        }
        // [status 1B][w 4B][h 4B][type_tag 1B][raster_w 4B][raster_h 4B][page_count 4B][pixels...]
        // 全 22B のヘッダ。`page_count` は v1.0.0 で追加 (PDF メタキャッシュ用)。
        if data[0] != STATUS_OK || data.len() < 22 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid render response",
            ));
        }
        let w = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
        let h = u32::from_le_bytes([data[5], data[6], data[7], data[8]]);
        let type_tag = data[9];
        let raster_w = u32::from_le_bytes(data[10..14].try_into().unwrap());
        let raster_h = u32::from_le_bytes(data[14..18].try_into().unwrap());
        let page_count = u32::from_le_bytes(data[18..22].try_into().unwrap());
        let content_type = if type_tag == 1 {
            PdfPageContentType::Raster {
                w: raster_w,
                h: raster_h,
            }
        } else {
            PdfPageContentType::Vector
        };
        let pixels = &data[22..];
        let expected = (w as usize) * (h as usize) * 4;
        if pixels.len() != expected {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "pixel data mismatch: expected {expected}, got {}",
                    pixels.len()
                ),
            ));
        }
        let img_buf = image::RgbaImage::from_raw(w, h, pixels.to_vec()).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "failed to create RgbaImage",
            )
        })?;
        Ok(RenderResult {
            image: image::DynamicImage::ImageRgba8(img_buf),
            content_type,
            page_count,
        })
    }
}

/// ディスパッチャースレッドのメインループ。
///
/// キューを覗き込み、Critical > Normal の順に pop して IPC を実行する。
/// Normal は `critical_reservation_active()` が true のとき
/// `worker_count - 1` (最低 1) 件までしか同時に走らない (1 ワーカー分を Critical 用に予約)。
///
/// `worker_count` は `PdfWorkerPool::start()` が実際に spawn に成功した数 (POOL_SIZE
/// と異なる degraded 環境を想定)。`worker_count == 1` の最劣化ケースでは予約による
/// `max_n = 0` (= Normal 全凍結) を防ぐため `max(1)` でクランプする。
///
/// `shutdown` フラグが立つと、サブプロセスに shutdown メッセージを送って
/// 子プロセスの終了を待ち、スレッド自体も終了する。
fn run_dispatcher(
    worker_id: usize,
    worker_count: usize,
    queue: Arc<(Mutex<JobQueue>, Condvar)>,
    mut child: Child,
    mut io: ProcessWorkerIo,
) {
    let pid = child.id();

    loop {
        // ── キューから 1 件取る ──
        let job = {
            let (mtx, cv) = &*queue;
            let mut q = mtx.lock().unwrap();
            loop {
                if q.shutdown {
                    break None;
                }
                // Critical を最優先
                if let Some(j) = q.critical.pop_front() {
                    q.workers_busy = q.workers_busy.saturating_add(1);
                    break Some(j);
                }
                // HighNormal / Normal: 予約中なら max_normal 制限 (両者で枠を共有)。
                // 最低でも 1 は確保しないと、1-worker pool で Normal ジョブが永久に
                // 動かなくなる (deadlock)。
                let reservation = critical_reservation_active();
                let max_n = if reservation {
                    worker_count.saturating_sub(1).max(1)
                } else {
                    worker_count.max(1)
                };
                if q.normal_in_flight < max_n {
                    // HighNormal (= 可視セル) を Normal より先に取る
                    if let Some(j) = q.high_normal.pop_front() {
                        q.normal_in_flight += 1;
                        q.workers_busy = q.workers_busy.saturating_add(1);
                        break Some(j);
                    }
                    if let Some(j) = q.normal.pop_front() {
                        q.normal_in_flight += 1;
                        q.workers_busy = q.workers_busy.saturating_add(1);
                        break Some(j);
                    }
                }
                // 取れなかった → Condvar で寝る
                q = cv.wait(q).unwrap();
            }
        };

        let Some(job) = job else {
            // shutdown
            break;
        };

        // HighNormal と Normal の両方が `normal_in_flight` 枠を消費する
        let counts_against_normal_slots =
            matches!(job.priority, JobPriority::HighNormal | JobPriority::Normal);

        // ── cancel + epoch チェック (pop 後): どちらかが立っていれば IPC せず Err を送る ──
        let cancelled = job
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed));
        let current_epoch = CURRENT_CONTEXT_EPOCH.load(Ordering::Relaxed);
        let stale_epoch = job.context_epoch != 0 && job.context_epoch < current_epoch;

        if cancelled || stale_epoch {
            if crate::perf::is_enabled() {
                let waited_ms = job.enqueued_at.elapsed().as_secs_f64() * 1000.0;
                let kind = if cancelled {
                    "pool_cancel_queued"
                } else {
                    "pool_stale_epoch_skip"
                };
                crate::perf::event(
                    "pdf",
                    kind,
                    job.perf_key.as_deref(),
                    0,
                    &[
                        ("waited_ms", serde_json::Value::from(waited_ms)),
                        ("pid", serde_json::Value::from(pid)),
                        ("job_epoch", serde_json::Value::from(job.context_epoch)),
                        ("current_epoch", serde_json::Value::from(current_epoch)),
                        (
                            "priority",
                            serde_json::Value::from(format!("{:?}", job.priority)),
                        ),
                    ],
                );
            }
            let msg = if cancelled {
                "cancelled in queue"
            } else {
                "context epoch advanced"
            };
            let _ = job.reply.send(Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                msg,
            )));
        } else {
            // ── IPC 実行 ──
            if crate::perf::is_enabled() {
                let wait_ms = job.enqueued_at.elapsed().as_secs_f64() * 1000.0;
                crate::perf::event(
                    "pdf",
                    "pool_dispatch",
                    job.perf_key.as_deref(),
                    0,
                    &[
                        ("wait_ms", serde_json::Value::from(wait_ms)),
                        ("pid", serde_json::Value::from(pid)),
                        (
                            "priority",
                            serde_json::Value::from(format!("{:?}", job.priority)),
                        ),
                    ],
                );
            }
            let result = send_recv_io(&mut io, &job.request);
            // reply 側 (requester) が既に recv_timeout で bail していると送信は失敗するが、
            // 無視してよい (結果は棄却されるだけで副作用なし)
            let _ = job.reply.send(result);
        }

        // ── 完了: カウンタ更新 + 他ワーカーを起こす ──
        {
            let (mtx, cv) = &*queue;
            let mut q = mtx.lock().unwrap();
            q.workers_busy = q.workers_busy.saturating_sub(1);
            if counts_against_normal_slots {
                q.normal_in_flight = q.normal_in_flight.saturating_sub(1);
            }
            // 他ワーカーが Normal スロット待ちで寝ている可能性があるので notify_all。
            // (Critical が来た/Normal スロットが空いた、の両方ともこれで波及する)
            cv.notify_all();
        }

        let _ = worker_id; // 名前付きスレッド用の参考、未使用
    }

    // ── Shutdown パス ──
    crate::logger::log(format!(
        "pdf-pool: worker {worker_id} shutting down (pid={pid})"
    ));
    let _ = write_msg(&mut io.stdin, &encode_shutdown_request());
    let _ = child.wait();
}

impl Drop for PdfWorkerPool {
    fn drop(&mut self) {
        // ディスパッチャースレッドに shutdown を通知
        {
            let (mtx, cv) = &*self.queue;
            if let Ok(mut q) = mtx.lock() {
                q.shutdown = true;
                cv.notify_all();
            }
        }
        // 全スレッドを join (各スレッドが自分の子プロセスを終了させる)
        if let Ok(mut threads) = self.dispatcher_threads.lock() {
            for h in threads.drain(..) {
                let _ = h.join();
            }
        }
    }
}

// -----------------------------------------------------------------------
// In-process ワーカースレッド (UI スレッドの非同期 API 用)
// -----------------------------------------------------------------------

enum WorkerRequest {
    Enumerate {
        path: PathBuf,
        password: Option<String>,
        reply: mpsc::Sender<std::io::Result<Vec<PdfPageEntry>>>,
        /// UI ナビゲーション経路 (`enumerate_pages_async`) のみ `Some(epoch)` を添え、
        /// ワーカーが pickup 時に最新 epoch と比較して stale なら skip する。
        /// バックグラウンド (キャッシュ作成等の `enumerate_pages` 同期経路) は `None` で、
        /// 常に実行する (Codex P2 対策: UI nav の epoch とバックグラウンドが干渉して
        /// アクティブな UI の PDF が Interrupted で落ちるのを防ぐ)。
        epoch: Option<u64>,
        /// **review #14 対応**: `enumerate_pages_async` の in-process fallback で、
        /// `PdfEnumerateHandle::cancel` を worker 経路にも伝搬する。pool 経由なら
        /// `pool.execute(.., Some(&cancel), ..)` で同じ仕組みが効くが、pool 不在環境
        /// での fallback では従来 None だったため、ハンドル drop で cancel が立っても
        /// 実 enumerate を止められなかった。pool ありの経路では None でよい。
        cancel: Option<Arc<AtomicBool>>,
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
        reply: mpsc::Sender<std::io::Result<RenderResult>>,
    },
    /// PDF document info (§16 step 17, ingest_worker 経由で呼ばれる)
    GetInfo {
        path: PathBuf,
        password: Option<String>,
        reply: mpsc::Sender<std::io::Result<PdfDocumentInfo>>,
    },
}

struct PdfWorker {
    tx: mpsc::Sender<WorkerRequest>,
    priority_tx: mpsc::Sender<WorkerRequest>,
}

static WORKER: OnceLock<PdfWorker> = OnceLock::new();

/// Enumerate エポック。`enumerate_pages_async` が呼ばれるたびに +1 され、
/// 要求に現在値を添付する。ワーカーが要求をピックアップした時点で `LATEST_ENUMERATE_EPOCH`
/// より古いなら、ユーザーは既に別の PDF へ移動済みなので skip して次を処理する。
///
/// **バグ修正 (2026-04)**: Ctrl+↑↓ で PDF を連打すると enumerate 要求がキューに積まれ、
/// ワーカー (pdfium はスレッドセーフ不可で 1 スレッド) が古い要求を律儀に処理し続けて
/// 最新の PDF が開くまで 10 秒超の黒画面になる事故が発生していた。
/// epoch 比較で stale 要求を即捨てれば、ユーザー視点では実質「最新の 1 件だけ」処理される。
static LATEST_ENUMERATE_EPOCH: AtomicU64 = AtomicU64::new(0);

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
                        loop {
                            match priority_rx.try_recv() {
                                Ok(req) => {
                                    Self::reply_init_error(&req, &e);
                                    continue;
                                }
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

    fn handle_request(pdfium: &Pdfium, req: WorkerRequest) {
        match req {
            WorkerRequest::Enumerate {
                path,
                password,
                reply,
                epoch,
                cancel,
            } => {
                // Stale な enumerate 要求は即捨てる。ユーザーが Ctrl+↑↓ 連打で複数 PDF を
                // 通過した場合、古い要求を律儀に処理する必要はない。
                //
                // Codex P2 対策: epoch は UI ナビゲーション経路
                // (`enumerate_pages_async`) のみ `Some(_)` で渡される。バックグラウンド
                // のキャッシュ作成等で呼ばれる同期 `enumerate_pages()` は `None` を渡すので
                // skip 判定の対象外。`is_stale = epoch.is_some_and(|e| e < latest)` とすること
                // で、nav 側が後から来て epoch を進めても background 要求は影響を受けない。
                if let Some(e) = epoch {
                    let latest = LATEST_ENUMERATE_EPOCH.load(Ordering::SeqCst);
                    if e < latest {
                        crate::logger::log(format!(
                            "pdf-worker: skipping stale enumerate (epoch {e} < latest {latest}) for {}",
                            path.display()
                        ));
                        let _ = reply.send(Err(std::io::Error::new(
                            std::io::ErrorKind::Interrupted,
                            "enumerate request superseded by newer navigation",
                        )));
                        return;
                    }
                }
                // **review #14 対応**: cancel が立っていれば実 PDFium を呼ばずに
                // Interrupted を返す。fallback 経路で UI が `PdfEnumerateHandle` を
                // drop した直後はここで捨てる (PDFium 呼び出しは長い場合に分秒級)。
                if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                    let _ = reply.send(Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "enumerate cancelled before start",
                    )));
                    return;
                }
                let result = core_enumerate(pdfium, &path, password.as_deref());
                if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                    // 結果が出てから cancel に追いつかれたケース: rx は既に drop されて
                    // いる可能性が高いので、send 失敗は無視。
                    return;
                }
                let _ = reply.send(result);
            }
            WorkerRequest::CheckPassword { path, reply } => {
                let _ = reply.send(Self::do_check_password(pdfium, &path));
            }
            WorkerRequest::Render {
                path,
                page_num,
                target_px,
                password,
                cancel,
                reply,
            } => {
                if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                    return;
                }
                let result =
                    core_render_with_count(pdfium, &path, page_num, target_px, password.as_deref())
                        .map(|(image, content_type, page_count)| RenderResult {
                            image,
                            content_type,
                            page_count,
                        });
                if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                    return;
                }
                let _ = reply.send(result);
            }
            WorkerRequest::GetInfo {
                path,
                password,
                reply,
            } => {
                let _ = reply.send(core_get_info(pdfium, &path, password.as_deref()));
            }
        }
    }

    fn init_pdfium() -> Result<Pdfium, String> {
        let dll_path = ensure_dll_extracted()?;
        let dll_dir = dll_path
            .parent()
            .ok_or_else(|| "cannot determine DLL directory".to_string())?;

        let bindings = Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(
            dll_dir.to_str().ok_or("non-UTF8 path")?,
        ))
        .map_err(|e| format!("PDFium binding failed: {e}"))?;
        Ok(Pdfium::new(bindings))
    }

    fn reply_init_error(req: &WorkerRequest, e: &str) {
        match req {
            WorkerRequest::Enumerate { reply, .. } => {
                let _ = reply.send(Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                )));
            }
            WorkerRequest::CheckPassword { reply, .. } => {
                let _ = reply.send(PdfAccessStatus::Error(e.to_string()));
            }
            WorkerRequest::Render { reply, .. } => {
                let _ = reply.send(Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                )));
            }
            WorkerRequest::GetInfo { reply, .. } => {
                let _ = reply.send(Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    e.to_string(),
                )));
            }
        }
    }

    fn do_check_password(pdfium: &Pdfium, path: &Path) -> PdfAccessStatus {
        match pdfium.load_pdf_from_file(path, None) {
            Ok(_) => PdfAccessStatus::Ok,
            Err(PdfiumError::PdfiumLibraryInternalError(PdfiumInternalError::PasswordError)) => {
                PdfAccessStatus::PasswordRequired
            }
            Err(e) => PdfAccessStatus::Error(format!("{e}")),
        }
    }
}

// -----------------------------------------------------------------------
// 公開データ型
// -----------------------------------------------------------------------

/// PDF ページのコンテンツ種別。
/// ラスター画像のみで構成されるページ (スキャン PDF) と、
/// ベクター要素 (テキスト・パス等) を含むページを区別する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PdfPageContentType {
    /// ベクター要素 (可視テキスト・パス・シェーディング等) を含む。
    Vector,
    /// ラスター画像のみ (OCR 透明テキストは無視)。原寸ピクセルサイズを保持。
    Raster { w: u32, h: u32 },
}

impl PdfPageContentType {
    /// レンダリング基準解像度 (長辺ピクセル数) を返す。
    /// ラスターページは画像の原寸、ベクターページは固定 4096px。
    pub fn base_render_px(&self) -> f32 {
        match self {
            Self::Raster { w, h } => (*w).max(*h) as f32,
            Self::Vector => 4096.0,
        }
    }
}

/// PDF ページ render の結果一式 (v1.0.0)。
///
/// 従来は `(image, content_type)` のタプルだったが、PDF メタキャッシュ
/// (`catalog::CatalogDb::set_pdf_meta`) に `page_count` を投入するため、worker
/// から page_count も返すようにした。caller は `result.image` / `result.content_type`
/// で従来コードを置き換えるだけで OK。PDF context (= path.ends_with(".pdf")) の
/// caller は `result.page_count` を catalog DB へ書き込むことで C-thumb 経路でも
/// cache が温まる。
pub struct RenderResult {
    pub image: image::DynamicImage,
    pub content_type: PdfPageContentType,
    pub page_count: u32,
}

pub struct PdfPageEntry {
    pub page_num: u32,
    pub mtime: i64,
    pub file_size: u64,
}

/// PDF document metadata (§16 step 17)。全文検索インデクサが ingest する。
/// pdfium-render の `PdfDocument::metadata()` から 4 タグを抜き取ったもの。
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct PdfDocumentInfo {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub keywords: Option<String>,
}

impl PdfDocumentInfo {
    /// 検索用テキストを 1 本の文字列にして返す (空白区切り、空フィールドは省略)。
    /// `search_norm::normalize_for_match` は呼ばない (呼び出し側で行う)。
    pub fn as_search_text(&self) -> String {
        let mut parts: Vec<&str> = Vec::new();
        if let Some(s) = self.title.as_deref() {
            if !s.is_empty() {
                parts.push(s);
            }
        }
        if let Some(s) = self.author.as_deref() {
            if !s.is_empty() {
                parts.push(s);
            }
        }
        if let Some(s) = self.subject.as_deref() {
            if !s.is_empty() {
                parts.push(s);
            }
        }
        if let Some(s) = self.keywords.as_deref() {
            if !s.is_empty() {
                parts.push(s);
            }
        }
        parts.join(" ")
    }
}

pub enum PdfAccessStatus {
    Ok,
    PasswordRequired,
    Error(String),
}

// -----------------------------------------------------------------------
// 公開 API — 同期版 (バックグラウンドスレッド用)
// -----------------------------------------------------------------------

/// PDF document info (Title / Author / Subject / Keywords) を取得する。
/// 全文検索インデクサ (`ingest_worker`) が PDF メタを ingest するときに呼ぶ (§16 step 17)。
///
/// worker プロセスプールがあれば IPC 経由、なければ in-process ワーカーにフォールバック。
pub fn get_document_info(
    pdf_path: &Path,
    password: Option<&str>,
) -> std::io::Result<PdfDocumentInfo> {
    let pool = get_pool();
    if pool.worker_count > 0 {
        let req = encode_get_info_request(pdf_path, password);
        let perf_key = crate::grid_item::pdf_file_perf_key(pdf_path);
        // get_document_info は indexer 経由の background なので epoch=0 + AbortOnCancel
        let resp = pool.execute(
            &req,
            None,
            JobPriority::Normal,
            Some(perf_key),
            0,
            CancelWaitPolicy::AbortOnCancel,
        )?;
        return PdfWorkerPool::parse_get_info_response(&resp);
    }
    // in-process フォールバック
    let (tx, rx) = mpsc::channel();
    let _ = get_worker().priority_tx.send(WorkerRequest::GetInfo {
        path: pdf_path.to_path_buf(),
        password: password.map(String::from),
        reply: tx,
    });
    rx.recv()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?
}

pub fn enumerate_pages(
    pdf_path: &Path,
    password: Option<&str>,
) -> std::io::Result<Vec<PdfPageEntry>> {
    enumerate_pages_with_cancel(pdf_path, password, None)
}

/// `enumerate_pages` の cancel 対応版。`process_meta_only` のように、上位の
/// epoch / cancel 機構 (例: `thumb_loader::bump_catchup_epoch`) から呼ばれる経路で
/// 使う (Codex P3-2 対応)。pool 経路では `pool.execute` の cancel に、in-process
/// fallback では `WorkerRequest::Enumerate.cancel` に伝搬する。
///
/// `cancel=None` だと旧 `enumerate_pages` と同等動作。バックグラウンドのキャッシュ
/// 作成等で使う。
pub fn enumerate_pages_with_cancel(
    pdf_path: &Path,
    password: Option<&str>,
    cancel: Option<Arc<AtomicBool>>,
) -> std::io::Result<Vec<PdfPageEntry>> {
    let pool = get_pool();
    if pool.worker_count > 0 {
        let req = encode_enumerate_request(pdf_path, password);
        // enumerate は列挙のみで軽量 (PDFium page 列挙) だが Normal 扱いでよい
        let perf_key = crate::grid_item::pdf_file_perf_key(pdf_path);
        // enumerate_pages_with_cancel は background catch-up 経路なので epoch=0
        // + AbortOnCancel (enumerate は cheap、cache 保存ロジック無し)。
        // (UI nav の enumerate は `enumerate_pages_async` 側で別途 LATEST_ENUMERATE_EPOCH
        // で stale 判定する)
        let resp = pool.execute(
            &req,
            cancel.as_ref(),
            JobPriority::Normal,
            Some(perf_key),
            0,
            CancelWaitPolicy::AbortOnCancel,
        )?;
        return PdfWorkerPool::parse_enumerate_response(&resp);
    }
    // フォールバック: in-process ワーカー。
    // 同期経路 (キャッシュ作成等) なので epoch は None を渡す (Codex P2: UI nav の
    // epoch 進行でキャッシュ作成中の enumerate が Interrupted で落ちるのを防ぐ)。
    let (tx, rx) = mpsc::channel();
    let _ = get_worker().priority_tx.send(WorkerRequest::Enumerate {
        path: pdf_path.to_path_buf(),
        password: password.map(String::from),
        reply: tx,
        epoch: None,
        cancel,
    });
    rx.recv()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?
}

pub fn render_page(
    pdf_path: &Path,
    page_num: u32,
    target_px: u32,
    password: Option<&str>,
    cancel: Option<Arc<AtomicBool>>,
    priority: JobPriority,
    // 0 = epoch チェック対象外 (background / Critical 用)。
    // それ以外は `current_render_context_epoch()` を **UI スレッドの enqueue 時点で**
    // 焼き付けた値を渡す (TOCTOU 防止のため worker thread からは呼ばない)。
    context_epoch: u64,
    // cancel 時に in-flight IPC を harvest するかどうか。
    // thumbnail PDF render の cache-savable 経路のみ `HarvestOnCancel`、それ以外は
    // `AbortOnCancel`。詳細は `CancelWaitPolicy` の doc コメント参照。
    cancel_policy: CancelWaitPolicy,
) -> std::io::Result<RenderResult> {
    if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Interrupted,
            "cancelled",
        ));
    }

    let perf_enabled = crate::perf::is_enabled();
    let perf_key = crate::grid_item::pdf_page_perf_key(pdf_path, page_num);
    let t0 = std::time::Instant::now();

    let pool = get_pool();
    if pool.worker_count > 0 {
        if perf_enabled {
            let busy_count = pool.workers_busy();
            crate::perf::event(
                "pdf",
                "pool_send",
                Some(&perf_key),
                0,
                &[
                    ("page", serde_json::Value::from(page_num)),
                    ("target_px", serde_json::Value::from(target_px)),
                    ("busy", serde_json::Value::from(busy_count)),
                    ("total", serde_json::Value::from(pool.worker_count)),
                    ("priority", serde_json::Value::from(format!("{priority:?}"))),
                ],
            );
        }
        let req = encode_render_request(pdf_path, page_num, target_px, password);
        let resp = pool.execute(
            &req,
            cancel.as_ref(),
            priority,
            Some(perf_key.clone()),
            context_epoch,
            cancel_policy,
        )?;
        let result = PdfWorkerPool::parse_render_response(&resp);
        if perf_enabled {
            let ms = t0.elapsed().as_secs_f64() * 1000.0;
            crate::perf::event(
                "pdf",
                "pool_recv",
                Some(&perf_key),
                0,
                &[
                    ("page", serde_json::Value::from(page_num)),
                    ("rtt_ms", serde_json::Value::from(ms)),
                    ("ok", serde_json::Value::from(result.is_ok())),
                ],
            );
        }
        return result;
    }

    // フォールバック: in-process ワーカー
    if perf_enabled {
        crate::perf::event(
            "pdf",
            "inproc_send",
            Some(&perf_key),
            0,
            &[("page", serde_json::Value::from(page_num))],
        );
    }
    let (tx, rx) = mpsc::channel();
    let _ = get_worker().tx.send(WorkerRequest::Render {
        path: pdf_path.to_path_buf(),
        page_num,
        target_px,
        password: password.map(String::from),
        cancel,
        reply: tx,
    });
    let result = rx
        .recv()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, format!("{e}")))?;
    if perf_enabled {
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        crate::perf::event(
            "pdf",
            "inproc_recv",
            Some(&perf_key),
            0,
            &[
                ("page", serde_json::Value::from(page_num)),
                ("rtt_ms", serde_json::Value::from(ms)),
                ("ok", serde_json::Value::from(result.is_ok())),
            ],
        );
    }
    result
}

pub fn check_password_needed(pdf_path: &Path) -> PdfAccessStatus {
    let (tx, rx) = mpsc::channel();
    let _ = get_worker().priority_tx.send(WorkerRequest::CheckPassword {
        path: pdf_path.to_path_buf(),
        reply: tx,
    });
    rx.recv()
        .unwrap_or(PdfAccessStatus::Error("worker channel closed".to_string()))
}

// -----------------------------------------------------------------------
// 公開 API — 非同期版 (UI スレッド用)
// -----------------------------------------------------------------------

pub fn render_page_async(
    pdf_path: &Path,
    page_num: u32,
    target_px: u32,
    password: Option<&str>,
) -> (
    Arc<AtomicBool>,
    mpsc::Receiver<std::io::Result<RenderResult>>,
) {
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

/// UI ナビゲーション経路の PDF ページ列挙ハンドル。
///
/// 呼び出し側 (App) は `pdf_enumerate_pending` にこれを保管し、次のナビに入る前に
/// `cancel()` を呼ぶか、単に置き換えれば `Drop` で自動キャンセルされる。
/// pool dispatcher は pop 時に cancel を確認して IPC 前にジョブを捨てるので、
/// 古い enumerate が PDFium 実行時間を消費してキューを詰まらせるのを防げる。
pub struct PdfEnumerateHandle {
    pub cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<std::io::Result<Vec<PdfPageEntry>>>,
}

impl PdfEnumerateHandle {
    /// 明示キャンセル。pool dispatcher / in-process worker が pop 時に確認して早期破棄する。
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for PdfEnumerateHandle {
    fn drop(&mut self) {
        // 置き換えで drop される場合、古いジョブが pool で in-flight にならないよう
        // 必ずキャンセルする (SearchHandle と同じパターン)。
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// UI ナビゲーション経路の PDF ページ列挙。
///
/// Ctrl+↑↓ で PDF を高速連打したときの grid 更新頻度を上げるため、以下の 3 点を組み合わせる:
///
/// 1. **multi-process pool + `JobPriority::Critical`** — pool が利用可能なら 3 並列で
///    列挙、Normal priority のキャッシュ作成等を押しのけて先に処理する。
/// 2. **cancel token をジョブに添える** — 旧ハンドルを drop すると `PdfEnumerateHandle::Drop`
///    が cancel を立て、pool dispatcher が pop 時に IPC 前で捨てる。古いジョブが PDFium
///    時間を消費してキューを詰まらせない。
/// 3. **in-process fallback では `LATEST_ENUMERATE_EPOCH` で stale skip** — pool 不在
///    環境 (setup-pdfium.sh 未実行等) でも連打による黒画面長期化を抑止する。
pub fn enumerate_pages_async(pdf_path: &Path, password: Option<&str>) -> PdfEnumerateHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    if crate::perf::is_enabled() {
        let perf_key = crate::grid_item::pdf_file_perf_key(pdf_path);
        crate::perf::event("pdf", "enumerate_send", Some(&perf_key), 0, &[]);
    }

    let pool = get_pool();
    if pool.worker_count > 0 {
        // 別スレッドで pool.execute を呼ぶ (pool.execute は内部で reply_rx を待つため
        // ブロッキング; UI は即 rx を受け取って pending に入れたい)。
        // thread 寿命は dispatcher が job を pop して cancel 検出で早期 reply するまで、
        // 通常 0〜数 ms。連打で 30/sec spawn しても CPU 負荷は許容範囲。
        let (tx, rx) = mpsc::channel();
        let req = encode_enumerate_request(pdf_path, password);
        let perf_key = crate::grid_item::pdf_file_perf_key(pdf_path);
        let cancel_w = Arc::clone(&cancel);
        match std::thread::Builder::new()
            .name("pdf-enumerate-nav".into())
            .spawn(move || {
                // Critical は epoch チェック対象外 (= 0 で send) + AbortOnCancel
                // (UI nav の即時応答 UX を優先、harvest は不要)
                let resp = pool.execute(
                    &req,
                    Some(&cancel_w),
                    JobPriority::Critical,
                    Some(perf_key),
                    0,
                    CancelWaitPolicy::AbortOnCancel,
                );
                let result = resp.and_then(|bytes| PdfWorkerPool::parse_enumerate_response(&bytes));
                let _ = tx.send(result);
            }) {
            Ok(_) => return PdfEnumerateHandle { cancel, rx },
            Err(e) => {
                // リソース不足等で Builder::spawn が失敗すると、tx が閉じ込められた
                // closure もろとも drop されて rx が即 Disconnected になる → App 側で
                // 「空 PDF」にフォールバックしてしまう。代わりに in-process worker 経路へ
                // 落として、ユーザには正常な enumerate か explicit なエラーを返す。
                crate::logger::log(format!(
                    "pdf-enumerate-nav: spawn failed ({e}), falling back to in-process worker"
                ));
                drop(rx); // 旧 rx は使わない
                // fall through to in-process path below
            }
        }
    }

    // Pool 不在 / spawn 失敗フォールバック: in-process worker + epoch skip の旧経路。
    // **review #14 対応**: in-process worker にも cancel を伝搬する。これがないと
    // `PdfEnumerateHandle` の drop で `cancel` が立っても worker は完走してしまい、
    // ナビゲーション応答性 (v1.0.0 の目標) が pool 不在環境で失われていた。
    let epoch = LATEST_ENUMERATE_EPOCH.fetch_add(1, Ordering::SeqCst) + 1;
    let (tx, rx) = mpsc::channel();
    let _ = get_worker().priority_tx.send(WorkerRequest::Enumerate {
        path: pdf_path.to_path_buf(),
        password: password.map(String::from),
        reply: tx,
        epoch: Some(epoch),
        cancel: Some(Arc::clone(&cancel)),
    });
    PdfEnumerateHandle { cancel, rx }
}

pub fn check_password_async(pdf_path: &Path) -> mpsc::Receiver<PdfAccessStatus> {
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
fn fit_to_target(w: f32, h: f32, target: f32) -> (f32, f32) {
    let long = w.max(h);
    if long <= 0.0 {
        return (w, h);
    }
    let scale = target / long;
    (w * scale, h * scale)
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------
//
// PDFium を実際に使うテストは CI で flakey (DLL 展開が走る等) なので、
// ここでは純粋ロジック — IPC のシリアライズ/デシリアライズと PdfDocumentInfo の整形 — だけ検証。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pdf_document_info_as_search_text_joins_fields() {
        let info = PdfDocumentInfo {
            title: Some("夕焼けの記録".to_string()),
            author: Some("Tarō Yamada".to_string()),
            subject: None,
            keywords: Some("landscape, sunset".to_string()),
        };
        let t = info.as_search_text();
        assert!(t.contains("夕焼けの記録"));
        assert!(t.contains("Tarō Yamada"));
        assert!(t.contains("landscape, sunset"));
        assert!(!t.contains("  "), "連続空白なし (空フィールドは落ちる)");
    }

    #[test]
    fn pdf_document_info_empty_gives_empty_string() {
        let info = PdfDocumentInfo::default();
        assert_eq!(info.as_search_text(), "");
    }

    #[test]
    fn parse_get_info_response_roundtrip() {
        // IPC エンコード (ipc_get_info) とデコード (parse_get_info_response) の往復
        let mut buf = Vec::new();
        buf.push(STATUS_OK);
        for field in ["Title-abc", "", "件名", "key1 key2"] {
            let bytes = field.as_bytes();
            buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            buf.extend_from_slice(bytes);
        }
        let info = PdfWorkerPool::parse_get_info_response(&buf).unwrap();
        assert_eq!(info.title.as_deref(), Some("Title-abc"));
        assert_eq!(info.author, None, "空文字列は None にデコードされる");
        assert_eq!(info.subject.as_deref(), Some("件名"));
        assert_eq!(info.keywords.as_deref(), Some("key1 key2"));
    }

    #[test]
    fn parse_get_info_response_handles_error_status() {
        let mut buf = vec![STATUS_ERR];
        buf.extend_from_slice(b"file not found");
        let result = PdfWorkerPool::parse_get_info_response(&buf);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("file not found"));
    }

    #[test]
    fn parse_get_info_response_rejects_truncated_field() {
        // title_len = 100 だが実際のバイト数は 4 バイトだけ → エラー
        let mut buf = vec![STATUS_OK];
        buf.extend_from_slice(&100u32.to_le_bytes());
        buf.extend_from_slice(b"abcd");
        let result = PdfWorkerPool::parse_get_info_response(&buf);
        assert!(result.is_err());
    }

    // ── Context epoch tests (PdfWorkerPool 内部ロジックのみ、PDFium IPC は使わない) ──

    /// JobQueue を直接構築して prune_stale_jobs と pop ロジックを検証する。
    /// 実プールを起動せず、Mutex<JobQueue> を直接操作する。
    fn make_test_job(
        priority: JobPriority,
        context_epoch: u64,
    ) -> (Job, mpsc::Receiver<std::io::Result<Vec<u8>>>) {
        let (tx, rx) = mpsc::channel();
        let job = Job {
            request: vec![],
            cancel: None,
            reply: tx,
            priority,
            enqueued_at: std::time::Instant::now(),
            perf_key: None,
            context_epoch,
        };
        (job, rx)
    }

    fn empty_queue() -> Arc<(Mutex<JobQueue>, Condvar)> {
        Arc::new((
            Mutex::new(JobQueue {
                critical: std::collections::VecDeque::new(),
                high_normal: std::collections::VecDeque::new(),
                normal: std::collections::VecDeque::new(),
                normal_in_flight: 0,
                workers_busy: 0,
                shutdown: false,
            }),
            Condvar::new(),
        ))
    }

    /// prune_stale_jobs の代替実装 (pool 起動なし、JobQueue 直操作)。
    fn prune_stale_in_queue(
        queue: &Arc<(Mutex<JobQueue>, Condvar)>,
        current_epoch: u64,
    ) -> Vec<Job> {
        let (mtx, _cv) = &**queue;
        let mut q = mtx.lock().unwrap();
        let mut dropped: Vec<Job> = Vec::new();
        let mut filter_queue = |queue: &mut std::collections::VecDeque<Job>| {
            let mut kept: std::collections::VecDeque<Job> =
                std::collections::VecDeque::with_capacity(queue.len());
            while let Some(j) = queue.pop_front() {
                if j.context_epoch != 0 && j.context_epoch < current_epoch {
                    dropped.push(j);
                } else {
                    kept.push_back(j);
                }
            }
            *queue = kept;
        };
        filter_queue(&mut q.high_normal);
        filter_queue(&mut q.normal);
        dropped
    }

    #[test]
    fn context_epoch_bump_increments_monotonically() {
        let initial = current_render_context_epoch();
        let a = bump_render_context_epoch();
        let b = bump_render_context_epoch();
        assert!(a > initial);
        assert!(b > a);
    }

    #[test]
    fn prune_drops_stale_normal_and_high_normal_only() {
        let queue = empty_queue();
        let (j_crit, _rx_c) = make_test_job(JobPriority::Critical, 5);
        let (j_high_stale, rx_h_stale) = make_test_job(JobPriority::HighNormal, 5);
        let (j_high_fresh, _rx_h_fresh) = make_test_job(JobPriority::HighNormal, 10);
        let (j_norm_stale, rx_n_stale) = make_test_job(JobPriority::Normal, 5);
        let (j_norm_fresh, _rx_n_fresh) = make_test_job(JobPriority::Normal, 10);
        let (j_norm_sentinel, _rx_n_s) = make_test_job(JobPriority::Normal, 0);
        {
            let (mtx, _) = &*queue;
            let mut q = mtx.lock().unwrap();
            q.critical.push_back(j_crit);
            q.high_normal.push_back(j_high_stale);
            q.high_normal.push_back(j_high_fresh);
            q.normal.push_back(j_norm_stale);
            q.normal.push_back(j_norm_fresh);
            q.normal.push_back(j_norm_sentinel);
        }
        let dropped = prune_stale_in_queue(&queue, 10);
        assert_eq!(dropped.len(), 2, "stale HighNormal + Normal が drop される");
        // Critical は触らない
        {
            let (mtx, _) = &*queue;
            let q = mtx.lock().unwrap();
            assert_eq!(q.critical.len(), 1, "Critical はプルーン対象外");
            assert_eq!(q.high_normal.len(), 1, "fresh HighNormal は残る");
            assert_eq!(q.normal.len(), 2, "fresh Normal + epoch=0 sentinel は残る");
        }
        // dropped 側に Interrupted reply を送る (= 実装と同じ挙動)
        for j in dropped {
            let _ = j.reply.send(Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "test prune",
            )));
        }
        // requester は受け取れる
        assert!(matches!(
            rx_h_stale.recv().unwrap().unwrap_err().kind(),
            std::io::ErrorKind::Interrupted
        ));
        assert!(matches!(
            rx_n_stale.recv().unwrap().unwrap_err().kind(),
            std::io::ErrorKind::Interrupted
        ));
    }

    #[test]
    fn prune_keeps_epoch_zero_sentinel() {
        // epoch=0 は background 用 sentinel。current_epoch がどれだけ進んでもプルーンされない。
        let queue = empty_queue();
        let (j_zero, _rx) = make_test_job(JobPriority::Normal, 0);
        {
            let (mtx, _) = &*queue;
            let mut q = mtx.lock().unwrap();
            q.normal.push_back(j_zero);
        }
        let dropped = prune_stale_in_queue(&queue, u64::MAX);
        assert_eq!(dropped.len(), 0);
        let (mtx, _) = &*queue;
        let q = mtx.lock().unwrap();
        assert_eq!(q.normal.len(), 1, "epoch=0 sentinel は残る");
    }

    #[test]
    fn race_old_epoch_enqueue_after_bump_caught_by_pop() {
        // race: 古い epoch を取得した後 bump、enqueue が prune の後に来るケース。
        // pop 時の epoch チェックが拾うことを確認する。
        // (実 dispatcher を回さず、ロジックの等価チェック)
        let queue = empty_queue();
        // bump 前に古い epoch を capture
        let old_epoch = 5u64;
        // bump 後 (current=10) に old epoch のジョブが enqueue されたシナリオ
        let current = 10u64;
        // prune 後に enqueue
        let _ = prune_stale_in_queue(&queue, current);
        let (j_late, _rx) = make_test_job(JobPriority::Normal, old_epoch);
        {
            let (mtx, _) = &*queue;
            let mut q = mtx.lock().unwrap();
            q.normal.push_back(j_late);
        }
        // run_dispatcher の pop ロジックを mimic: pop 後の stale_epoch チェック
        let popped = {
            let (mtx, _) = &*queue;
            let mut q = mtx.lock().unwrap();
            q.normal.pop_front().unwrap()
        };
        let stale = popped.context_epoch != 0 && popped.context_epoch < current;
        assert!(stale, "pop 時の epoch チェックが race を拾う");
    }

    // ── CancelWaitPolicy tests (recv ループの cancel 反応ロジック) ──
    //
    // execute() を直接呼ぶには PdfWorkerPool 起動が必要 (子プロセス spawn) で test には
    // 重すぎる。代わりに execute の recv ループと等価なロジックを mock で再現し、
    // AbortOnCancel と HarvestOnCancel の挙動差を検証する。

    fn mock_recv_loop(
        reply_rx: mpsc::Receiver<std::io::Result<Vec<u8>>>,
        cancel: Arc<AtomicBool>,
        policy: CancelWaitPolicy,
    ) -> std::io::Result<Vec<u8>> {
        loop {
            match reply_rx.recv_timeout(std::time::Duration::from_millis(10)) {
                Ok(result) => return result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if cancel.load(Ordering::Relaxed) {
                        match policy {
                            CancelWaitPolicy::AbortOnCancel => {
                                return Err(std::io::Error::new(
                                    std::io::ErrorKind::Interrupted,
                                    "cancelled",
                                ));
                            }
                            CancelWaitPolicy::HarvestOnCancel => {
                                // 待ち継続
                            }
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "disconnected",
                    ));
                }
            }
        }
    }

    #[test]
    fn abort_on_cancel_returns_interrupted_immediately() {
        let (_tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = Arc::clone(&cancel);
        // 別スレッドで 30 ms 後に cancel を立てる (reply は来ない = receiver hold)
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            cancel_clone.store(true, Ordering::Relaxed);
        });
        let t0 = std::time::Instant::now();
        let result = mock_recv_loop(rx, cancel, CancelWaitPolicy::AbortOnCancel);
        let elapsed_ms = t0.elapsed().as_millis();
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Interrupted);
        // ~30-50 ms (cancel 検出 + 次の timeout) で返るはず
        assert!(
            elapsed_ms < 200,
            "AbortOnCancel should return quickly, took {elapsed_ms}ms"
        );
    }

    #[test]
    fn harvest_on_cancel_waits_for_reply() {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = Arc::clone(&cancel);
        let tx_clone = tx.clone();
        // 30 ms 後に cancel、80 ms 後に reply を投入
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(30));
            cancel_clone.store(true, Ordering::Relaxed);
            std::thread::sleep(std::time::Duration::from_millis(50));
            let _ = tx_clone.send(Ok(vec![1, 2, 3]));
        });
        drop(tx); // 元の tx は drop して、clone した tx だけが live
        let t0 = std::time::Instant::now();
        let result = mock_recv_loop(rx, cancel, CancelWaitPolicy::HarvestOnCancel);
        let elapsed_ms = t0.elapsed().as_millis();
        // HarvestOnCancel: cancel が立っても待ち続け、reply (= Ok([1,2,3])) を受け取る
        assert!(
            result.is_ok(),
            "harvest should receive reply, got {result:?}"
        );
        assert_eq!(result.unwrap(), vec![1, 2, 3]);
        // ~80 ms 後 + 1 timeout = ~80-100 ms
        assert!(
            elapsed_ms >= 70,
            "harvest should have waited at least 70ms, took {elapsed_ms}ms"
        );
    }

    #[test]
    fn harvest_on_cancel_still_returns_on_disconnect() {
        let (tx, rx) = mpsc::channel::<std::io::Result<Vec<u8>>>();
        let cancel = Arc::new(AtomicBool::new(true)); // 最初から cancel
        drop(tx); // sender 即 drop → receiver は disconnected を受ける
        let result = mock_recv_loop(rx, cancel, CancelWaitPolicy::HarvestOnCancel);
        assert!(result.is_err());
        // disconnected の Other エラーが返るはず
        assert_eq!(result.unwrap_err().kind(), std::io::ErrorKind::Other);
    }
}
