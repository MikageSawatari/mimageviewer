//! TensorRT 高速化パックのオンライン取得実装。
//!
//! ## 役割
//!
//! `setup-tensorrt-pack.ps1` が NVIDIA / NuGet から直接 DL する開発用フローを
//! 受け持っていた領域を、配布版では「mikage が事前に作って GitHub Releases に置いた
//! pack を 1 ストロークで DL する」フローに置き換える。
//!
//! 1. `manifest.json` を fetch & parse (~3 KB)
//! 2. `notices/` (NOTICE-NVIDIA / LICENSE-onnxruntime) と `common/` (DLL 群) を
//!    `%APPDATA%/mimageviewer/tensorrt/` に DL & SHA-256 検証 & 配置
//! 3. ユーザー GPU の SM に合致する `engines/` zip 1 個を DL & 検証 & 展開
//! 4. INSTALL_OK sentinel を atomic 書き込みして完了 (= [`tensorrt_pack::is_pack_installed`]
//!    が true を返すようになる)
//!
//! ## スレッドモデル
//!
//! - メインスレッド (UI) が [`InstallHandle`] を保持し、毎フレーム [`InstallHandle::poll`]
//!   を呼んで [`InstallProgress`] を取り出す
//! - worker thread 1 個が逐次実行 (manifest → notices → DLLs → engine zip → sentinel)。
//!   全 DLL が ~14 個なので並列 DL の効果は薄い、シンプルさ優先
//! - キャンセルは `Arc<AtomicBool>` で worker 側に通知。チャンク間で確認するので
//!   最大数 MB 分だけ余分に DL してから停止する
//!
//! ## 再開 (resume) 方針
//!
//! - `tensorrt/` 直下に SHA-256 一致の同名ファイルが既に存在 → スキップ
//! - 部分 DL ファイル (`<name>.partial`) が存在 → HTTP `Range:` で続きを取得
//! - SHA-256 不一致 → ファイル削除して最初から DL し直す
//!
//! ## エラー処理
//!
//! - 通信失敗・hash mismatch・SM 未対応・ディスク不足は全て worker thread 側で
//!   `Result::Err` として捕捉し、`InstallProgress::Error { message }` で UI に
//!   通知。worker thread は exit する。INSTALL_OK は書かないので半分 DL 状態でも
//!   `is_pack_installed()` が true にならない
//! - キャンセル後も部分 DL ファイルは残す (= 次回 `start_install` で再開可能)

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Instant;

use serde::Deserialize;
use sha2::{Digest, Sha256};

use super::tensorrt_pack;

/// pack v1 のアセットが置かれる GitHub Releases ベース URL。
///
/// タグ名は `trt-pack-v<N>` (= `tensorrt_pack::EXPECTED_TRT_PACK_VERSION`)。
/// pack バージョンを bump する際はこの URL も連動して更新する。
///
/// 構造:
///   `<base>/manifest.json` ← まず fetch
///   `<base>/<asset.name>`  ← manifest 内の各 AssetEntry を name 付きで DL
///
/// **テスト時の override**: 環境変数 `MIV_TRT_PACK_BASE_URL` を設定するとそちらが
/// 優先される。ローカル HTTP サーバー (例: `python -m http.server 8000` を
/// `dist/trt-pack-v1/` で起動 → URL=`http://127.0.0.1:8000`) で E2E 動作確認に使う。
const DEFAULT_PACK_BASE_URL: &str =
    "https://github.com/MikageSawatari/mimageviewer/releases/download/trt-pack-v1";

const PACK_BASE_URL_ENV: &str = "MIV_TRT_PACK_BASE_URL";

/// 実際に DL に使う base URL を返す。env var が立っていればそちら、なければ既定値。
/// 末尾の `/` は呼び出し側が `format!("{}/{}", base, name)` で連結するので付けない。
fn pack_base_url() -> String {
    match std::env::var(PACK_BASE_URL_ENV) {
        Ok(v) if !v.trim().is_empty() => v.trim_end_matches('/').to_string(),
        _ => DEFAULT_PACK_BASE_URL.to_string(),
    }
}

/// HTTP timeout (秒)。GitHub Releases CDN の応答時間に余裕を持たせる。
const HTTP_CONNECT_TIMEOUT_SECS: u64 = 30;
/// 単一 read の timeout。CDN がストリーム途中で stall しても worker が無限待機しないように。
const HTTP_READ_TIMEOUT_SECS: u64 = 60;

/// HTTP DL 時のチャンクサイズ (256 KiB)。
/// 進捗更新の単位 + cancel チェックポイントを兼ねる。
const DL_CHUNK_SIZE: usize = 256 * 1024;

/// manifest.json のスキーマ。`build_trt_pack.rs::Manifest` と必ず同期させる。
/// `manifest_format = 3` (NOTICE/LICENSE 同梱版) を前提。
#[derive(Debug, Deserialize)]
pub struct Manifest {
    /// マニフェスト構造のバージョン。3 未満は本コードがパースできない (engines/notices なし)。
    pub manifest_format: u32,
    /// pack バージョン。`tensorrt_pack::EXPECTED_TRT_PACK_VERSION` と一致するか検証。
    pub pack_version: u32,
    /// CUDA / cuDNN / TRT / ORT のバージョン情報 (UI 表示 + 互換性ログ用)。
    #[allow(dead_code)]
    pub versions: BTreeMap<String, String>,
    /// 全ユーザー共通の DLL 群 (CUDA / TRT runtime / ORT)。
    pub common: Vec<AssetEntry>,
    /// 法務同梱必須テキスト (NOTICE-NVIDIA.txt / LICENSE-onnxruntime.txt)。
    pub notices: Vec<AssetEntry>,
    /// GPU 世代別の事前ビルド engine zip。
    pub engines: Vec<EnginePack>,
    /// 進捗 UI の分母として使う合計バイト数。
    /// (notices と engine zip は別途 [`AssetEntry::bytes`] を sum すれば total に出来る)
    #[allow(dead_code)]
    pub common_total_bytes: u64,
    /// pack 作成時刻 (UTC ISO 8601)。デバッグ用。
    #[allow(dead_code)]
    pub created_at: String,
}

/// アセット 1 個 (DLL / NOTICE / engine zip 共通)。
#[derive(Debug, Clone, Deserialize)]
pub struct AssetEntry {
    /// ファイル名。GitHub Releases のアセット名と一致させる。
    pub name: String,
    /// SHA-256 (hex 64 文字)。
    pub sha256: String,
    /// ファイルサイズ (バイト)。
    pub bytes: u64,
}

/// engine pack 1 個 (GPU 世代単位)。
#[derive(Debug, Clone, Deserialize)]
pub struct EnginePack {
    /// pack 識別子 (= zip ファイル名のキー)。例: "ampere_plus"
    #[allow(dead_code)]
    pub id: String,
    /// 対応最小 SM × 10。例: 80 = sm80+ (Ampere 以降)
    pub compute_capability_min: u32,
    /// UI 用の人間可読ラベル。
    #[allow(dead_code)]
    pub human_label: String,
    /// この pack を構成するファイル群 (通常 1 個の zip)。
    pub files: Vec<AssetEntry>,
}

/// インストール進捗イベント。worker → UI へ mpsc で送る。
#[derive(Debug, Clone)]
pub enum InstallProgress {
    /// manifest.json を fetch 中。
    FetchingManifest,
    /// manifest fetch 完了。total_bytes は notices + common + 選択 engine zip の合計。
    ManifestFetched {
        pack_version: u32,
        total_files: usize,
        total_bytes: u64,
        engine_pack_label: String,
    },
    /// あるファイルの DL が始まった。
    StartingFile {
        name: String,
        file_index: usize,
        total_files: usize,
        bytes_total: u64,
    },
    /// あるファイルの DL 進捗。bytes_done は今回 DL 開始時点 + 累積 (resume の場合 base 分は含む)。
    FileProgress {
        name: String,
        bytes_done: u64,
        bytes_total: u64,
    },
    /// あるファイルが SHA-256 検証中。
    VerifyingFile { name: String },
    /// engine zip を展開中。
    ExtractingEngine { entry_index: usize, total: usize },
    /// pack 全体の完了。INSTALL_OK 書き込み済み。
    Done,
    /// 致命的エラー (worker は exit、再試行はユーザーがダイアログを再オープン)。
    Error { message: String },
}

/// インストールハンドル。UI 側はこれを保持して [`Self::poll`] で進捗を取り出す。
pub struct InstallHandle {
    rx: mpsc::Receiver<InstallProgress>,
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
    /// 最後に observe した進捗。`poll` が None を返した後でも UI が最終状態を表示できるよう保持。
    last_progress: Option<InstallProgress>,
}

impl InstallHandle {
    /// チャネルから 1 件 progress を取り出す。残っていなければ None。
    /// `Done` または `Error` を受け取ったら以降は新規 progress は来ない。
    pub fn poll(&mut self) -> Option<InstallProgress> {
        match self.rx.try_recv() {
            Ok(p) => {
                self.last_progress = Some(p.clone());
                Some(p)
            }
            Err(_) => None,
        }
    }

    /// 最後に observe した進捗 (poll で受け取り済みでも残る)。UI が状態表示に使う。
    pub fn last_progress(&self) -> Option<&InstallProgress> {
        self.last_progress.as_ref()
    }

    /// キャンセル要求。worker thread は次のチャンク境界で停止する。
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// worker thread が終了したか。`true` なら `poll` でこれ以上 progress は来ない。
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
    /// UI 側がハンドルを drop したらワーカも止める (孤児スレッド防止)。
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(h) = self.join.take() {
            let _ = h.join();
        }
    }
}

/// インストールを開始する。worker thread が spawn される。
///
/// `target_sm_x10`: ユーザー GPU の compute capability × 10 (例: 89 = RTX 4090)。
/// 該当する engine pack (= `compute_capability_min <= target_sm_x10` のうち最大値) が
/// 選ばれる。`None` の場合は最も低い `compute_capability_min` を持つ pack を使う。
pub fn start_install(target_sm_x10: Option<u32>) -> InstallHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_for_thread = cancel.clone();
    let tx_for_err = tx.clone();
    let join = thread::Builder::new()
        .name("trt-installer".to_string())
        .spawn(move || {
            if let Err(e) = run_install(target_sm_x10, cancel_for_thread, tx) {
                let _ = tx_for_err.send(InstallProgress::Error { message: e });
            }
        })
        .expect("failed to spawn TRT installer thread");

    InstallHandle {
        rx,
        cancel,
        join: Some(join),
        last_progress: None,
    }
}

/// worker thread の本体。エラーは `Err(String)` で返し、呼び出し側が
/// `InstallProgress::Error` に翻訳する。
fn run_install(
    target_sm_x10: Option<u32>,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<InstallProgress>,
) -> Result<(), String> {
    let _ = tx.send(InstallProgress::FetchingManifest);
    let manifest = fetch_manifest(&cancel)?;
    if manifest.manifest_format < 3 {
        return Err(format!(
            "manifest_format {} はこのバージョンの mIV では非対応です (3 以上を要求)。\
             pack 配布が古い可能性があります。",
            manifest.manifest_format
        ));
    }
    if manifest.pack_version != tensorrt_pack::EXPECTED_TRT_PACK_VERSION {
        return Err(format!(
            "pack_version {} は本 mIV ({}) と非互換です。\
             mIV のバージョンを更新するか、対応する pack を選んでください。",
            manifest.pack_version,
            tensorrt_pack::EXPECTED_TRT_PACK_VERSION
        ));
    }

    // 対応する engine pack を選ぶ。
    let engine_pack = pick_engine_pack(&manifest.engines, target_sm_x10)?;
    let engine_label = engine_pack.id.clone();

    // 全 DL 対象ファイルを順序付きで列挙する: notices → common → engine pack。
    // notices を最初に置くのは、何らかの理由で中断したときに「ライセンス文書だけは
    // 読める」状態を作りやすくするため (実用的には影響ない、好みの問題)。
    let mut all_files: Vec<&AssetEntry> = Vec::new();
    all_files.extend(manifest.notices.iter());
    all_files.extend(manifest.common.iter());
    all_files.extend(engine_pack.files.iter());
    let total_files = all_files.len();
    let total_bytes: u64 = all_files.iter().map(|a| a.bytes).sum();

    let _ = tx.send(InstallProgress::ManifestFetched {
        pack_version: manifest.pack_version,
        total_files,
        total_bytes,
        engine_pack_label: engine_pack.human_label.clone(),
    });

    let pack_dir = tensorrt_pack::pack_dir();
    fs::create_dir_all(&pack_dir).map_err(|e| format!("create pack_dir: {e}"))?;

    // INSTALL_OK が残っていたら一旦消す (= 不完全 install のままユーザーから完了に
    // 見えてしまうのを防ぐ)。最後に書き直す。
    let sentinel = tensorrt_pack::install_sentinel_path();
    if sentinel.exists() {
        let _ = fs::remove_file(&sentinel);
    }

    // ── DL ループ ──
    for (idx, asset) in all_files.iter().enumerate() {
        check_cancel(&cancel)?;
        let dst = pack_dir.join(&asset.name);
        let _ = tx.send(InstallProgress::StartingFile {
            name: asset.name.clone(),
            file_index: idx,
            total_files,
            bytes_total: asset.bytes,
        });

        download_and_verify(asset, &dst, &cancel, &tx)?;
    }

    // ── engine zip 展開 ──
    // engine_pack.files[0] は zip ファイル。中身を tensorrt-engines/<model>/<file> に展開。
    let engine_zip_path = pack_dir.join(&engine_pack.files[0].name);
    extract_engine_zip(&engine_zip_path, &cancel, &tx)?;

    // ── INSTALL_OK 書き込み (atomic) ──
    write_install_sentinel(&manifest, &engine_label)?;

    let _ = tx.send(InstallProgress::Done);
    Ok(())
}

/// `<base_url>/manifest.json` を fetch & parse する。
fn fetch_manifest(cancel: &Arc<AtomicBool>) -> Result<Manifest, String> {
    check_cancel(cancel)?;
    let url = format!("{}/manifest.json", pack_base_url());
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS))
        .timeout_read(std::time::Duration::from_secs(HTTP_READ_TIMEOUT_SECS))
        .build();
    let resp = agent
        .get(&url)
        .call()
        .map_err(|e| format!("fetch manifest: {e}"))?;
    let json = resp
        .into_string()
        .map_err(|e| format!("read manifest body: {e}"))?;
    serde_json::from_str(&json).map_err(|e| format!("parse manifest: {e}"))
}

/// `engines` の中から、ユーザー SM に合致する最大の `compute_capability_min` の pack を選ぶ。
/// `target_sm_x10 == None` のときは最小 SM の pack を返す (フォールバック)。
fn pick_engine_pack<'a>(
    engines: &'a [EnginePack],
    target_sm_x10: Option<u32>,
) -> Result<&'a EnginePack, String> {
    if engines.is_empty() {
        return Err("manifest.engines が空です。pack が壊れている可能性があります。".to_string());
    }

    if let Some(sm) = target_sm_x10 {
        // 「sm 以下の最大 pack」を選ぶ。例: sm=89, packs=[80, 90] → 80。
        let candidate = engines
            .iter()
            .filter(|p| p.compute_capability_min <= sm)
            .max_by_key(|p| p.compute_capability_min);
        if let Some(p) = candidate {
            return Ok(p);
        }
        // ターゲット SM がどの pack にも対応しない (例: sm75 で AMPERE_PLUS のみ提供時)
        return Err(format!(
            "GPU の compute capability ({}.{}) に対応する事前ビルド engine pack がありません。\
             現状 sm{} 以上の GPU が必要です (= RTX 30 シリーズ以降)。\
             RTX 20 シリーズ以前は DirectML を引き続きご利用ください。",
            sm / 10,
            sm % 10,
            engines
                .iter()
                .map(|p| p.compute_capability_min)
                .min()
                .unwrap()
        ));
    }

    // SM 不明 → 最も低い要求を選ぶ (将来 sm75 pack を追加したら自動で fallback)
    Ok(engines
        .iter()
        .min_by_key(|p| p.compute_capability_min)
        .unwrap())
}

/// 単一アセットを DL → SHA-256 検証 → 配置 (atomic rename)。
/// 既に同名で正しいハッシュのファイルがあればスキップ。
fn download_and_verify(
    asset: &AssetEntry,
    dst: &Path,
    cancel: &Arc<AtomicBool>,
    tx: &mpsc::Sender<InstallProgress>,
) -> Result<(), String> {
    // (1) 既存ファイルが正しい hash ならスキップ。
    if dst.exists() {
        let _ = tx.send(InstallProgress::VerifyingFile {
            name: asset.name.clone(),
        });
        if let Ok(actual) = sha256_of_file(dst, cancel) {
            if actual.eq_ignore_ascii_case(&asset.sha256) {
                let _ = tx.send(InstallProgress::FileProgress {
                    name: asset.name.clone(),
                    bytes_done: asset.bytes,
                    bytes_total: asset.bytes,
                });
                return Ok(());
            }
            // hash 不一致 → 削除して DL し直す。
            let _ = fs::remove_file(dst);
        }
        check_cancel(cancel)?;
    }

    // (2) <name>.partial で resume 可能ならそうする。
    let partial = dst.with_extension(format!(
        "{}.partial",
        dst.extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
    ));
    let resume_from: u64 = fs::metadata(&partial)
        .ok()
        .map(|m| m.len())
        .unwrap_or(0)
        // 期待サイズ以上に膨らんでいたら不正、捨てる。
        .min(asset.bytes);
    if fs::metadata(&partial).is_ok() && resume_from < fs::metadata(&partial).unwrap().len() {
        // 期待サイズより partial が大きい異常状態。捨てて 0 から。
        let _ = fs::remove_file(&partial);
    }
    let resume_from = if partial.exists() {
        resume_from
    } else {
        0
    };

    // (3) HTTP DL with optional Range header.
    let url = format!("{}/{}", pack_base_url(), asset.name);
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(HTTP_CONNECT_TIMEOUT_SECS))
        .timeout_read(std::time::Duration::from_secs(HTTP_READ_TIMEOUT_SECS))
        .build();
    let mut req = agent.get(&url);
    if resume_from > 0 {
        req = req.set("Range", &format!("bytes={}-", resume_from));
    }
    let resp = req
        .call()
        .map_err(|e| format!("HTTP {}: {e}", asset.name))?;

    // 206 (resume) でも 200 (full) でも reader を straight に書く。
    // ただし 200 が返ってきたのに resume_from > 0 だと既存 partial の内容が残るので
    // truncate してやり直す。
    let server_full_response = resp.status() == 200;
    let mut file = if resume_from > 0 && !server_full_response {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .open(&partial)
            .map_err(|e| format!("open partial: {e}"))?;
        f.seek(SeekFrom::End(0))
            .map_err(|e| format!("seek partial: {e}"))?;
        f
    } else {
        // 0 から書き直し。
        fs::File::create(&partial).map_err(|e| format!("create partial: {e}"))?
    };
    let mut reader = resp.into_reader();
    let mut buf = vec![0u8; DL_CHUNK_SIZE];
    let mut bytes_done: u64 = if server_full_response { 0 } else { resume_from };
    let mut last_progress = Instant::now();
    loop {
        check_cancel(cancel)?;
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("read chunk {}: {e}", asset.name))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n])
            .map_err(|e| format!("write chunk {}: {e}", asset.name))?;
        bytes_done += n as u64;
        // 進捗は ~50ms 間隔で間引く (UI の repaint に十分な頻度、過剰送信を防ぐ)。
        if last_progress.elapsed().as_millis() >= 50 {
            let _ = tx.send(InstallProgress::FileProgress {
                name: asset.name.clone(),
                bytes_done,
                bytes_total: asset.bytes,
            });
            last_progress = Instant::now();
        }
    }
    file.flush()
        .map_err(|e| format!("flush partial: {e}"))?;
    drop(file);

    // (4) hash 検証。
    let _ = tx.send(InstallProgress::VerifyingFile {
        name: asset.name.clone(),
    });
    let actual = sha256_of_file(&partial, cancel)?;
    if !actual.eq_ignore_ascii_case(&asset.sha256) {
        // 不正 partial を消して上位にエラー (ユーザー再試行で fresh DL)。
        let _ = fs::remove_file(&partial);
        return Err(format!(
            "{} の SHA-256 が一致しません。\nexpected: {}\nactual: {}\n\
             ネットワークエラーまたは pack の更新中の可能性があります。再試行してください。",
            asset.name, asset.sha256, actual
        ));
    }

    // (5) atomic rename (.partial → 最終名)。
    if dst.exists() {
        let _ = fs::remove_file(dst);
    }
    fs::rename(&partial, dst).map_err(|e| format!("rename {}: {e}", asset.name))?;

    let _ = tx.send(InstallProgress::FileProgress {
        name: asset.name.clone(),
        bytes_done: asset.bytes,
        bytes_total: asset.bytes,
    });
    Ok(())
}

/// engine zip を開いて `tensorrt-engines/<model>/<file>` に展開する。
/// zip 内パスは `<model_name>/<file>` のフラット 2 階層を想定 (build_trt_pack.rs と整合)。
fn extract_engine_zip(
    zip_path: &Path,
    cancel: &Arc<AtomicBool>,
    tx: &mpsc::Sender<InstallProgress>,
) -> Result<(), String> {
    let engine_root = tensorrt_pack::engine_cache_dir();
    fs::create_dir_all(&engine_root).map_err(|e| format!("create engine_root: {e}"))?;

    let file = fs::File::open(zip_path).map_err(|e| format!("open engine zip: {e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("read zip: {e}"))?;
    let total = archive.len();
    for i in 0..total {
        check_cancel(cancel)?;
        let _ = tx.send(InstallProgress::ExtractingEngine {
            entry_index: i,
            total,
        });
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("zip entry {i}: {e}"))?;
        // zip-slip 対策: enclosed_name で `..` 含むパスを reject。
        let rel_path = match entry.enclosed_name() {
            Some(p) => p.to_owned(),
            None => {
                return Err(format!(
                    "engine zip の entry [{}] が不正なパスです",
                    entry.name()
                ));
            }
        };
        if rel_path.as_os_str().is_empty() || entry.is_dir() {
            continue;
        }
        let dst = engine_root.join(&rel_path);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir engine subdir: {e}"))?;
        }
        let mut out = fs::File::create(&dst).map_err(|e| format!("create engine file: {e}"))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|e| format!("extract {}: {e}", rel_path.display()))?;
    }
    Ok(())
}

/// INSTALL_OK sentinel を atomic 書き込み (=tmp に書いて rename)。
/// 中身は manifest の versions マップ + engine_pack id を JSON で保存。
fn write_install_sentinel(manifest: &Manifest, engine_pack_id: &str) -> Result<(), String> {
    let pack_dir = tensorrt_pack::pack_dir();
    let sentinel = tensorrt_pack::install_sentinel_path();
    let tmp = pack_dir.join("INSTALL_OK.tmp");
    let body = serde_json::json!({
        "version": manifest.pack_version,
        "manifest_format": manifest.manifest_format,
        "ort_gpu_version": manifest.versions.get("ort_gpu").cloned().unwrap_or_default(),
        "cuda_cudart_version": manifest.versions.get("cuda_cudart").cloned().unwrap_or_default(),
        "cuda_cublas_version": manifest.versions.get("cuda_cublas").cloned().unwrap_or_default(),
        "cudnn_version": manifest.versions.get("cudnn").cloned().unwrap_or_default(),
        "trt_version": manifest.versions.get("trt").cloned().unwrap_or_default(),
        "engine_pack_id": engine_pack_id,
        "installed_at": utc_now_iso8601(),
    });
    let body_str = serde_json::to_string_pretty(&body)
        .map_err(|e| format!("serialize INSTALL_OK: {e}"))?;
    fs::write(&tmp, body_str).map_err(|e| format!("write INSTALL_OK.tmp: {e}"))?;
    if sentinel.exists() {
        let _ = fs::remove_file(&sentinel);
    }
    fs::rename(&tmp, &sentinel).map_err(|e| format!("rename INSTALL_OK: {e}"))?;
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

/// SHA-256 ダイジェストを hex 小文字に。`build_trt_pack.rs` 側と同じロジック。
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

/// cancel フラグが立っていたらエラーで早期 return する小さなヘルパ。
fn check_cancel(cancel: &Arc<AtomicBool>) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        Err("ユーザーによってキャンセルされました".to_string())
    } else {
        Ok(())
    }
}

/// 現在時刻を UTC ISO 8601 形式で。chrono 等の依存を避けて手書き。
/// `bin/build_trt_pack.rs` と同じロジック。
fn utc_now_iso8601() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let (year, month, day, hour, min, sec) = unix_to_ymdhms(secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, min, sec
    )
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
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
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

    fn fake_engine(id: &str, sm_min: u32) -> EnginePack {
        EnginePack {
            id: id.to_string(),
            compute_capability_min: sm_min,
            human_label: format!("test pack {id}"),
            files: vec![],
        }
    }

    #[test]
    fn pick_engine_pack_picks_highest_under_target() {
        let packs = vec![
            fake_engine("turing", 75),
            fake_engine("ampere_plus", 80),
            fake_engine("hopper", 90),
        ];
        // RTX 4090 (sm89) は ampere_plus を使う
        let p = pick_engine_pack(&packs, Some(89)).unwrap();
        assert_eq!(p.id, "ampere_plus");
        // Hopper (sm90) は hopper
        let p = pick_engine_pack(&packs, Some(90)).unwrap();
        assert_eq!(p.id, "hopper");
        // Turing (sm75) は turing
        let p = pick_engine_pack(&packs, Some(75)).unwrap();
        assert_eq!(p.id, "turing");
    }

    #[test]
    fn pick_engine_pack_rejects_unsupported_sm() {
        let packs = vec![fake_engine("ampere_plus", 80)];
        // Pascal (sm61) は Ampere pack ではカバーされない
        assert!(pick_engine_pack(&packs, Some(61)).is_err());
    }

    #[test]
    fn pick_engine_pack_fallback_when_unknown_sm() {
        let packs = vec![
            fake_engine("ampere_plus", 80),
            fake_engine("hopper", 90),
        ];
        // SM 不明 → 最小要求 (= 最も広く動く) pack
        let p = pick_engine_pack(&packs, None).unwrap();
        assert_eq!(p.id, "ampere_plus");
    }

    #[test]
    fn pick_engine_pack_empty_errors() {
        assert!(pick_engine_pack(&[], Some(89)).is_err());
    }

    #[test]
    fn pack_base_url_uses_env_when_set() {
        // 重要: テスト同士の env var 競合を避けるため SAFETY: テストプロセスは
        // シングルスレッド前提では無いが、本テスト 1 個だけが PACK_BASE_URL_ENV を
        // 触る (= cargo test の他テストから参照されない) ので問題ない。
        // SAFETY: read/write のみで、外部に副作用なし。
        unsafe {
            std::env::set_var(PACK_BASE_URL_ENV, "http://127.0.0.1:9999/");
        }
        // 末尾の `/` は剥がされる
        assert_eq!(pack_base_url(), "http://127.0.0.1:9999");
        unsafe {
            std::env::remove_var(PACK_BASE_URL_ENV);
        }
        // 既定は GitHub Releases URL
        assert_eq!(pack_base_url(), DEFAULT_PACK_BASE_URL);
    }

    #[test]
    fn manifest_parser_accepts_real_dist_manifest() {
        // dist/trt-pack-v1/manifest.json (= 直近 build_trt_pack の出力) が
        // 本コードの `Manifest` スキーマでパースできることを保証する。
        // ファイルが無ければ skip (CI 等 build 前は無いため)。
        let path = std::path::Path::new("dist/trt-pack-v1/manifest.json");
        if !path.exists() {
            eprintln!("[skip] {} not found, run build_trt_pack first", path.display());
            return;
        }
        let body = std::fs::read_to_string(path).expect("read manifest");
        let m: Manifest = serde_json::from_str(&body).expect("parse manifest");
        assert!(m.manifest_format >= 3, "manifest_format must be >=3");
        assert_eq!(m.pack_version, tensorrt_pack::EXPECTED_TRT_PACK_VERSION);
        assert!(!m.common.is_empty(), "common DLL list must not be empty");
        assert!(!m.notices.is_empty(), "notices list must not be empty");
        assert!(!m.engines.is_empty(), "engines list must not be empty");
        // SHA-256 が hex 64 文字であることを確認
        for asset in m.common.iter().chain(m.notices.iter()) {
            assert_eq!(asset.sha256.len(), 64, "{} hash len", asset.name);
            assert!(
                asset.sha256.chars().all(|c| c.is_ascii_hexdigit()),
                "{} hash not hex",
                asset.name
            );
        }
    }

    #[test]
    fn hex_encode_basic() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0xab]), "00ffab");
        assert_eq!(hex_encode(&[]), "");
    }
}
