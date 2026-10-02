//! 起動時差分走査 (docs/archive/search-metadata/search-expansion-design.md §7.4)。
//!
//! お気に入りルートを再帰的に walk し、現在の FS 状態と `fts_meta.db` の
//! 登録状態を 3-way diff して「ingest すべき path」「削除すべき path」を返す。
//!
//! ## 重要な制約 (CLAUDE.md §UI スレッド同期 I/O)
//!
//! - `entry.file_type()` を使う (`Path::is_dir()` / `is_file()` は `GetFileAttributes` syscall を
//!   per-entry で呼ぶため数百ファイルで 500-1000ms ブロックになる)
//! - UI スレッドから呼ばない。専用スレッドで実行する
//! - `GlobalIoSemaphore` で read_dir の同時実行を制御する (§7.5)
//! - キャンセルトークンで中断可能 (大量のお気に入り走査中に終了されても OK)
//!
//! ## 本モジュールのスコープ (§16 step 6)
//!
//! - FS walker + 3-way diff の計算のみ
//! - メタ抽出・Tantivy commit は **このモジュールの責務外**
//!   (Ingest Worker = §16 step 9 に切り出す)
//! - ZIP はアイテム索引 (Ctrl+G) の対象外なので候補に含めない
//!   (docs/search-container-item-redesign.md §3.2)。ZIP のコンテナ検索は Ctrl+S 専属。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use uuid::Uuid;

use crate::folder_tree;
use crate::fts_meta::FtsMetaDb;
use crate::indexer_progress::ProgressReporter;
use crate::io_semaphore::{GlobalIoSemaphore, IoPriority};
use crate::search_index_db::normalize_path;

/// 1 候補ファイル (通常画像 / PDF / 動画)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CandidateFile {
    /// 絶対パス (表示・I/O 用)
    pub abs_path: PathBuf,
    /// 正規化済み DB キー (`normalize_path` 済み)
    pub key: String,
    pub kind: CandidateKind,
    /// **表示用** mtime (画像本体)。Tantivy doc に入り Ctrl+G 一覧の日付ソートに使う。
    pub mtime: i64,
    /// **表示用** file_size (画像本体)。
    pub file_size: i64,
    /// **差分用** mtime。画像はサイドカーを織り込んだ `max(画像, サイドカー)`。
    /// fts_meta に保存され walker の 3-way diff で比較される。サイドカーの追加・編集を
    /// 「変化あり」として検出するため (docs/sidecar-metadata-ingest.md §14-3/§14-4)。
    pub diff_mtime: i64,
    /// **差分用** size。画像はサイドカーの size を加味した `画像 size + サイドカー size`
    /// (サイドカー無しは画像 size)。サイドカーの追加・削除を size 変化として検出するため。
    pub diff_size: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CandidateKind {
    /// ネイティブ対応画像 (Susie プラグイン拡張含む)
    Image,
    /// PDF (v1 は document info のみ ingest、本文は対象外)
    Pdf,
    /// 動画 (ファイル名 + mXD XMP + sidecar tags + container metadata)
    Video,
    /// 音声 (ファイル名のみ。ID3 等の埋め込みタグは対象外)
    Audio,
}

/// 3-way diff 結果。
#[derive(Debug, Default)]
pub struct ScanResult {
    /// FS にあり DB に無い、または mtime/size が変化したもの → ingest キュー対象
    pub to_ingest: Vec<CandidateFile>,
    /// DB にあり FS にないもの → tombstone (削除) 対象
    pub to_delete: Vec<String>,
    /// 差分なしの件数 (進捗表示用)
    pub unchanged: usize,
    /// 走査中に encountered した候補ファイル総数 (stats 用)
    pub total_scanned: usize,
    /// 診断統計 (Codex 6 回目 nice-to-have #2): インデックス管理ダイアログの
    /// トラブルシューティング表示で使える
    pub diag: ScanDiag,
    /// 観測の完全性。Incomplete でも観測済み候補の取り込みは継続する。
    /// 完全な Full の印を立てる際は、別途すべての書き込み成功も確認すること。
    pub completeness: ObservationCompleteness,
}

/// FS の不在を確定できるか。未走査の既定値を Complete にしない。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ObservationCompleteness {
    Complete,
    #[default]
    Incomplete,
}

/// walker の診断統計。read_dir / file_type / metadata 失敗の件数を持つ。
#[derive(Debug, Default, Clone, Copy)]
pub struct ScanDiag {
    /// std::fs::read_dir が失敗した回数 (典型例: アクセス拒否フォルダ)
    pub read_dir_errors: usize,
    /// read_dir の iterator が個別エントリを返せなかった回数
    pub entry_errors: usize,
    /// DirEntry::file_type() が失敗した回数 (稀)
    pub file_type_errors: usize,
    /// reparse point 等の分類中に属性を取得できなかった回数
    pub classification_errors: usize,
    /// DirEntry::metadata() が失敗した回数 (削除競合等)
    pub metadata_errors: usize,
    /// 最大深度 (MAX_DEPTH) に到達して打ち切ったディレクトリ数
    pub depth_limit_hits: usize,
}

impl ScanDiag {
    fn completeness(self) -> ObservationCompleteness {
        if self.read_dir_errors == 0
            && self.entry_errors == 0
            && self.file_type_errors == 0
            && self.classification_errors == 0
            && self.metadata_errors == 0
            && self.depth_limit_hits == 0
        {
            ObservationCompleteness::Complete
        } else {
            ObservationCompleteness::Incomplete
        }
    }
}

/// 実 FS と同じ走査経路で、観測失敗を決定的に検証するための I/O 境界。
trait WalkerIo {
    type Entries: Iterator<Item = std::io::Result<std::fs::DirEntry>>;

    fn read_dir(&self, path: &Path) -> std::io::Result<Self::Entries>;
    fn file_type(&self, entry: &std::fs::DirEntry) -> std::io::Result<std::fs::FileType>;
    fn metadata(&self, entry: &std::fs::DirEntry) -> std::io::Result<std::fs::Metadata>;
    fn classify(
        &self,
        entry: &std::fs::DirEntry,
        file_type: &std::fs::FileType,
    ) -> std::io::Result<crate::fs_entry::DirEntryKind> {
        crate::fs_entry::try_classify_dir_entry(entry, file_type)
    }
}

struct FsWalkerIo;

impl WalkerIo for FsWalkerIo {
    type Entries = std::fs::ReadDir;

    fn read_dir(&self, path: &Path) -> std::io::Result<Self::Entries> {
        std::fs::read_dir(path)
    }

    fn file_type(&self, entry: &std::fs::DirEntry) -> std::io::Result<std::fs::FileType> {
        entry.file_type()
    }

    fn metadata(&self, entry: &std::fs::DirEntry) -> std::io::Result<std::fs::Metadata> {
        entry.metadata()
    }
}

/// 走査開始パラメータ。
pub struct ScanParams {
    pub favorite_id: Uuid,
    pub root: PathBuf,
    pub excluded_roots: Vec<PathBuf>,
    pub cancel: Arc<AtomicBool>,
    /// "今どこを walk してる" を UI に見せるためのレポーター。
    /// None なら通知しない (テスト等で便利)。
    pub progress: Option<ProgressReporter>,
}

/// 進捗通知 (UI への stream)。Walker は I/O-bound なので頻繁に通知しすぎないこと。
pub enum WalkerEvent {
    /// 定期的な進捗
    Progress {
        scanned: usize,
        current_dir: Option<PathBuf>,
    },
    /// 完了
    Done(ScanResult),
    /// エラー (フォルダが消えた等)
    Error(String),
}

/// お気に入りルートを走査して 3-way diff を計算する。
///
/// `io_sem` は read_dir 呼び出しの順番制御用。複数お気に入りを並列走査する場合は
/// 同じセマフォを共有することでグローバル I/O 同時実行数を制御できる。
/// `activity_gate` を渡すと、walker が各ディレクトリの `read_dir` 前にユーザー操作を待つ
/// (2026-04 F: **Codex P2 対応**、walker phase も操作中停止の対象にする)。
///
/// 呼び出し側は典型的に別スレッドで実行し、結果を mpsc で受け取る。
pub fn scan(
    params: ScanParams,
    db: &FtsMetaDb,
    io_sem: &GlobalIoSemaphore,
    priority: IoPriority,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
) -> Result<ScanResult, String> {
    scan_with_io(params, db, io_sem, priority, activity_gate, &FsWalkerIo)
}

fn scan_with_io(
    params: ScanParams,
    db: &FtsMetaDb,
    io_sem: &GlobalIoSemaphore,
    priority: IoPriority,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    io: &impl WalkerIo,
) -> Result<ScanResult, String> {
    let ScanParams {
        favorite_id,
        root,
        excluded_roots,
        cancel,
        progress,
    } = params;

    // 1. FS を walk して候補を集める
    let mut fs_map = std::collections::HashMap::<String, CandidateFile>::new();
    let mut diag = ScanDiag::default();
    let mut visited = std::collections::HashSet::new();
    walk_dir_recursive(
        &root,
        io_sem,
        priority,
        activity_gate,
        &cancel,
        progress.as_ref(),
        &mut fs_map,
        &mut diag,
        0,
        &mut visited,
        &excluded_roots,
        io,
    )?;
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }

    // 2. DB 側の登録一覧を取得
    let db_entries = db
        .list_favorite_files(favorite_id)
        .map_err(|e| format!("fts_meta list failed: {e}"))?;
    let db_map: std::collections::HashMap<String, (i64, i64)> = db_entries
        .into_iter()
        .map(|(p, m, s)| (p, (m, s)))
        .collect();

    // 3. 3-way diff
    let mut result = ScanResult::default();
    result.total_scanned = fs_map.len();
    result.diag = diag;
    result.completeness = diag.completeness();

    for (key, cand) in &fs_map {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        match db_map.get(key) {
            None => {
                // FS only → 新規 ingest
                result.to_ingest.push(cand.clone());
            }
            Some(&(db_mtime, db_size)) => {
                // 差分判定は **差分用** 署名 (画像 + サイドカーを織り込んだ値) で行う。
                // fts_meta には ingest_worker が diff_mtime/diff_size を保存している。
                if db_mtime == cand.diff_mtime && db_size == cand.diff_size {
                    result.unchanged += 1;
                } else {
                    // 変化あり (本体 or サイドカーの追加/編集/削除) → 再 ingest
                    result.to_ingest.push(cand.clone());
                }
            }
        }
    }
    // 不完全な観測は「FS に無い」証明にならない。取り込みは残し、削除だけ生成しない。
    if result.completeness == ObservationCompleteness::Complete {
        for key in db_map.keys() {
            if cancel.load(Ordering::Relaxed) {
                return Err("cancelled".into());
            }
            if !fs_map.contains_key(key) {
                result.to_delete.push(key.clone());
            }
        }
    }

    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }

    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn walk_dir_recursive(
    dir: &Path,
    io_sem: &GlobalIoSemaphore,
    priority: IoPriority,
    activity_gate: Option<&crate::activity_gate::ActivityGate>,
    cancel: &AtomicBool,
    progress: Option<&ProgressReporter>,
    out: &mut std::collections::HashMap<String, CandidateFile>,
    diag: &mut ScanDiag,
    depth: u32,
    visited: &mut std::collections::HashSet<String>,
    excluded_roots: &[PathBuf],
    io: &impl WalkerIo,
) -> Result<(), String> {
    // 安全策: シンボリックループ対策 (深さ制限)。通常フォルダは 20 階層あれば十分
    const MAX_DEPTH: u32 = 40;
    if depth > MAX_DEPTH {
        diag.depth_limit_hits += 1;
        return Ok(());
    }
    if crate::books::path_is_under_any(dir, excluded_roots) {
        return Ok(());
    }
    if !crate::fs_entry::mark_directory_visited(dir, visited) {
        return Ok(());
    }

    // read_dir 1 回は通常 <10ms だが HDD/NAS で数百ディレクトリ連続すると操作中の
    // サムネ I/O と競合する。ディレクトリ単位で ActivityGate を待てば操作中は walk
    // が停止する。cancel check も兼ねる。
    if crate::activity_gate::wait_and_check_cancel(activity_gate, cancel) {
        return Ok(());
    }

    // このディレクトリに入る時点で UI に通知 (1 ディレクトリ 1 回なので mutex 競合は軽微)
    if let Some(p) = progress {
        p.set(format!("スキャン: {} ({} 件)", dir.display(), out.len()));
    }

    let Some(_permit) = io_sem.acquire_cancellable(priority, cancel) else {
        return Ok(());
    };
    let rd = match io.read_dir(dir) {
        Ok(r) => r,
        Err(_) => {
            diag.read_dir_errors += 1;
            return Ok(());
        }
    };
    // read_dir 中は permit を握ったまま全エントリを舐める
    let (entries, has_possible_sidecar) = collect_directory_sidecar_entries(rd, diag);
    drop(_permit); // read_dir 完了後は permit を返し、子 walk 時に再取得

    let mut subdirs: Vec<PathBuf> = Vec::new();
    for entry in entries {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        if crate::fs_entry::is_internal_app_entry_name(&entry.file_name()) {
            continue;
        }
        // ★ file_type() は entry がキャッシュしているので syscall なし
        let file_type = match io.file_type(&entry) {
            Ok(ft) => ft,
            Err(_) => {
                diag.file_type_errors += 1;
                continue;
            }
        };
        let path = entry.path();

        if folder_tree::is_apple_double(&path) {
            continue;
        }
        if crate::books::path_is_under_any(&path, excluded_roots) {
            continue;
        }

        let entry_kind = match io.classify(&entry, &file_type) {
            Ok(kind) => kind,
            Err(_) => {
                diag.classification_errors += 1;
                continue;
            }
        };
        if entry_kind.is_directory() {
            subdirs.push(path);
            continue;
        }
        if !entry_kind.is_file() {
            continue; // device 等はスキップ
        }

        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        // ZIP はアイテム索引 (Ctrl+G) の対象外。候補に含めないことで、既存の ZIP doc は
        // 3-way diff で「FS になし + DB あり」と判定され to_delete に落ちて Tantivy から
        // 消える (docs/search-container-item-redesign.md §3.2)。
        // `is_recognized_image_ext` は Susie プラグインが申告した拡張子も画像扱いにする
        // ため、アーカイブ系 Susie プラグインが "zip" を申告しても確実に除外できるよう、
        // 拡張子分類より前に明示的に弾く (Codex P2)。
        if ext == "zip" {
            continue;
        }
        let kind = if folder_tree::is_paged_document_path(&path) {
            CandidateKind::Pdf
        } else if folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&ext.as_str()) {
            CandidateKind::Video
        } else if folder_tree::is_audio_ext(&ext) {
            CandidateKind::Audio
        } else if folder_tree::is_recognized_image_ext(&ext) {
            CandidateKind::Image
        } else {
            continue;
        };

        let metadata = match io.metadata(&entry) {
            Ok(m) => m,
            Err(_) => {
                diag.metadata_errors += 1;
                continue;
            }
        };
        let modified = metadata.modified();
        if modified.is_err() {
            diag.metadata_errors += 1;
        }
        let mtime = modified
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let file_size = metadata.len() as i64;

        // 差分用署名: 画像はサイドカー (同名 .json/.txt) の mtime/size を織り込む。
        // これでサイドカーの追加・編集・削除が 3-way diff で検出される (§14-3/§14-4)。
        // 候補の無いフォルダは 8.3 の別名になり得る <stem>.txt だけ stat する。
        let (diff_mtime, diff_size) = if kind == CandidateKind::Image {
            match directory_sidecar_signature(&path, has_possible_sidecar) {
                Some(sig) => (mtime.max(sig.mtime), file_size + sig.fingerprint),
                None => (mtime, file_size),
            }
        } else {
            (mtime, file_size)
        };

        let key = normalize_path(&path);
        out.insert(
            key.clone(),
            CandidateFile {
                abs_path: path,
                key,
                kind,
                mtime,
                file_size,
                diff_mtime,
                diff_size,
            },
        );
    }

    // 子ディレクトリを再帰 (permit は再帰先で取り直す)
    for sub in subdirs {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        walk_dir_recursive(
            &sub,
            io_sem,
            priority,
            activity_gate,
            cancel,
            progress,
            out,
            diag,
            depth + 1,
            visited,
            excluded_roots,
            io,
        )?;
    }
    Ok(())
}

/// Windows の短い名前・Unicode 比較を取りこぼさない、保守的な拡張子判定。
fn is_possible_sidecar_entry(path: &Path) -> bool {
    let Some(extension) = path.extension() else {
        return false;
    };
    let extension = extension.to_string_lossy();
    !extension.is_ascii()
        || extension.eq_ignore_ascii_case("json")
        || extension
            .get(..3)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("txt"))
}

fn collect_directory_sidecar_entries(
    entries: impl IntoIterator<Item = std::io::Result<std::fs::DirEntry>>,
    diag: &mut ScanDiag,
) -> (Vec<std::fs::DirEntry>, bool) {
    let mut observed = Vec::new();
    let mut has_possible_sidecar = false;
    for entry in entries {
        match entry {
            Ok(entry) => {
                // 種類や除外を見る前に確認する。フォルダ・リンクも stat 版が判定する。
                has_possible_sidecar |= is_possible_sidecar_entry(&entry.path());
                observed.push(entry);
            }
            // 不完全な一覧では不在を確認できないため、既存の stat 経路を維持する。
            Err(_) => {
                has_possible_sidecar = true;
                diag.entry_errors += 1;
            }
        }
    }
    (observed, has_possible_sidecar)
}

/// 8.3 の形だけを確認する。明示設定された別名も扱うため文字種は制限しない。
fn could_be_short_name(name: &str) -> bool {
    if name.chars().filter(|ch| *ch == '.').count() > 1 {
        return false;
    }
    let (stem, extension) = name.split_once('.').unwrap_or((name, ""));
    stem.encode_utf16().count() <= 8 && extension.encode_utf16().count() <= 3
}

/// `.json` は 4 文字、<full>.txt は複数ドットなので、該当候補は <stem>.txt だけ。
fn short_txt_sidecar_candidate(image_path: &Path) -> Option<PathBuf> {
    image_path.file_name()?.to_str()?;
    let stem = image_path.file_stem()?.to_str()?;
    let name = format!("{stem}.txt");
    could_be_short_name(&name).then(|| image_path.with_file_name(name))
}

fn directory_sidecar_signature(
    image_path: &Path,
    has_possible_sidecar: bool,
) -> Option<crate::external_metadata::SidecarSig> {
    if has_possible_sidecar {
        crate::external_metadata::sidecar_signature(image_path)
    } else {
        short_txt_sidecar_candidate(image_path)
            .and_then(|path| crate::external_metadata::sidecar_signature_for_candidate(&path))
    }
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fts_index::IndexKind;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn possible_sidecar_extensions_are_conservative() {
        for name in [
            "a.JSON",
            "a.TxT",
            "a.txtold",
            "a.TXToLD",
            "a.jsön",
            "a.ＴＸＴ",
            "x.jpg.json",
        ] {
            assert!(is_possible_sidecar_entry(Path::new(name)), "{name}");
        }
        for name in ["a.jpg", "a.jsonold", "a.tx", "日本語.jpg", "a", "a."] {
            assert!(!is_possible_sidecar_entry(Path::new(name)), "{name}");
        }
    }

    #[test]
    fn short_name_shape_counts_utf16_and_dots_without_character_restrictions() {
        for name in [
            "IMG_0001.txt",
            "日本語漢字abc.txt",
            "😀😀😀😀.txt",
            "a b!@#$%.txt",
        ] {
            assert!(could_be_short_name(name), "{name}");
        }
        for name in ["IMG_00010.txt", "a.json", "a.jpg.txt", "😀😀😀😀a.txt"] {
            assert!(!could_be_short_name(name), "{name}");
        }
    }

    #[test]
    fn no_possible_sidecar_only_probes_short_stem_txt() {
        assert_eq!(
            short_txt_sidecar_candidate(Path::new("dir/IMG_0001.jpg")),
            Some(PathBuf::from("dir/IMG_0001.txt")),
        );
        // 純関数で stat する候補を分離。明示設定された短い別名も同じ形の判定を通る。
        for image in [
            "dir/long_image_name.png",
            "dir/a.b.jpg",
            "dir/😀😀😀😀a.jpg",
        ] {
            assert!(short_txt_sidecar_candidate(Path::new(image)).is_none());
        }
    }

    #[test]
    fn short_candidate_signature_matches_existing_signature() {
        let tmp = TempDir::new().unwrap();
        let image = tmp.path().join("IMG_0001.jpg");
        fs::write(&image, b"image").unwrap();
        fs::write(image.with_extension("txt"), b"sidecar").unwrap();
        let expected = crate::external_metadata::sidecar_signature(&image);
        assert!(expected.is_some());
        // 一覧に出ない明示 short name の経路も、候補名から同じ署名を作る。
        assert_eq!(directory_sidecar_signature(&image, false), expected);
        assert_eq!(directory_sidecar_signature(&image, true), expected);
    }

    #[test]
    fn possible_sidecar_directory_keeps_existing_priority_and_signature() {
        let tmp = TempDir::new().unwrap();
        let image = tmp.path().join("x.jpg");
        fs::write(&image, b"image").unwrap();
        fs::create_dir(tmp.path().join("x.jpg.json")).unwrap();
        fs::write(tmp.path().join("x.txt"), b"text").unwrap();
        assert!(is_possible_sidecar_entry(&tmp.path().join("x.jpg.json")));
        assert_eq!(
            directory_sidecar_signature(&image, true),
            crate::external_metadata::sidecar_signature(&image),
        );
        let db = FtsMetaDb::open_at(&tmp.path().join("fts_meta.db")).unwrap();
        let result = scan_sync(Uuid::new_v4(), tmp.path(), &db);
        let sig = crate::external_metadata::sidecar_signature(&image).unwrap();
        assert_eq!(result.to_ingest[0].diff_size, 5 + sig.fingerprint);
    }

    #[test]
    fn incomplete_directory_listing_uses_existing_sidecar_detection() {
        let tmp = TempDir::new().unwrap();
        let image = tmp.path().join("long_image_name.jpg");
        fs::write(&image, b"image").unwrap();
        let entry = fs::read_dir(tmp.path()).unwrap().next().unwrap().unwrap();
        // 一覧にサイドカーが含まれなかった途中エラーを再現する。
        fs::write(tmp.path().join("long_image_name.jpg.json"), b"{}").unwrap();
        let mut diag = ScanDiag::default();
        let (entries, possible) = collect_directory_sidecar_entries(
            [Ok(entry), Err(std::io::ErrorKind::PermissionDenied.into())],
            &mut diag,
        );
        assert_eq!(entries.len(), 1);
        assert!(possible);
        assert_eq!(diag.entry_errors, 1);
        assert_eq!(diag.completeness(), ObservationCompleteness::Incomplete);
        assert!(short_txt_sidecar_candidate(&image).is_none());
        assert!(directory_sidecar_signature(&image, possible).is_some());
        assert_eq!(
            directory_sidecar_signature(&image, possible),
            crate::external_metadata::sidecar_signature(&image)
        );
    }

    fn make_file(dir: &Path, name: &str, content: &[u8]) {
        fs::write(dir.join(name), content).unwrap();
    }

    fn tmp_db() -> (TempDir, FtsMetaDb) {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("fts_meta.db");
        let db = FtsMetaDb::open_at(&db_path).unwrap();
        (dir, db)
    }

    fn scan_sync(fav_id: Uuid, root: &Path, db: &FtsMetaDb) -> ScanResult {
        let sem = GlobalIoSemaphore::new(2);
        let cancel = Arc::new(AtomicBool::new(false));
        scan(
            ScanParams {
                favorite_id: fav_id,
                root: root.to_path_buf(),
                excluded_roots: Vec::new(),
                cancel,
                progress: None,
            },
            db,
            &sem,
            IoPriority::Normal,
            None,
        )
        .unwrap()
    }

    #[test]
    fn real_walker_long_image_name_makes_zero_sidecar_probes() {
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("photos");
        fs::create_dir(&root).unwrap();
        let image = root.join("long_image_name.jpg");
        fs::write(&image, b"image").unwrap();

        let (result, probes) = crate::external_metadata::count_sidecar_probes(|| {
            scan_sync(Uuid::new_v4(), &root, &db)
        });
        assert_eq!(result.to_ingest.len(), 1);
        assert_eq!(probes, 0);

        // 旧 four-probe 直呼びも同じ stat 境界で計数され、回帰した walker は上で落ちる。
        let (sig, old_probes) = crate::external_metadata::count_sidecar_probes(|| {
            crate::external_metadata::sidecar_signature(&image)
        });
        assert!(sig.is_none());
        assert_eq!(old_probes, 4);
    }

    #[test]
    fn real_walker_short_image_name_makes_exactly_one_sidecar_probe() {
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("photos");
        fs::create_dir(&root).unwrap();
        make_file(&root, "IMG_0001.jpg", b"image");

        let (result, probes) = crate::external_metadata::count_sidecar_probes(|| {
            scan_sync(Uuid::new_v4(), &root, &db)
        });
        assert_eq!(result.to_ingest.len(), 1);
        assert_eq!(probes, 1);
    }

    #[derive(Clone, Copy)]
    enum ObservationFault {
        ReadDir,
        Entry,
        FileType,
        Classification,
        Metadata,
    }

    struct FaultIo {
        target: PathBuf,
        fault: ObservationFault,
    }

    impl WalkerIo for FaultIo {
        type Entries = std::vec::IntoIter<std::io::Result<std::fs::DirEntry>>;

        fn read_dir(&self, path: &Path) -> std::io::Result<Self::Entries> {
            if path == self.target && matches!(self.fault, ObservationFault::ReadDir) {
                return Err(std::io::ErrorKind::PermissionDenied.into());
            }
            let mut entries: Vec<_> = fs::read_dir(path)?.collect();
            if path == self.target && matches!(self.fault, ObservationFault::Entry) {
                entries.push(Err(std::io::ErrorKind::PermissionDenied.into()));
            }
            Ok(entries.into_iter())
        }

        fn file_type(&self, entry: &std::fs::DirEntry) -> std::io::Result<std::fs::FileType> {
            if entry.path() == self.target && matches!(self.fault, ObservationFault::FileType) {
                return Err(std::io::ErrorKind::PermissionDenied.into());
            }
            entry.file_type()
        }

        fn classify(
            &self,
            entry: &std::fs::DirEntry,
            file_type: &std::fs::FileType,
        ) -> std::io::Result<crate::fs_entry::DirEntryKind> {
            if entry.path() == self.target && matches!(self.fault, ObservationFault::Classification)
            {
                return Err(std::io::ErrorKind::PermissionDenied.into());
            }
            crate::fs_entry::try_classify_dir_entry(entry, file_type)
        }

        fn metadata(&self, entry: &std::fs::DirEntry) -> std::io::Result<std::fs::Metadata> {
            if entry.path() == self.target && matches!(self.fault, ObservationFault::Metadata) {
                return Err(std::io::ErrorKind::PermissionDenied.into());
            }
            entry.metadata()
        }
    }

    fn assert_incomplete_scan_keeps_observed_changes(fault: ObservationFault) -> ScanDiag {
        let (tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = tmp.path().join("photos");
        let child = root.join("child");
        fs::create_dir_all(&child).unwrap();
        make_file(&root, "new_image.jpg", b"new");
        make_file(&root, "updated_image.jpg", b"updated");
        make_file(&root, "unchanged_image.jpg", b"unchanged");
        make_file(&child, "unobserved_image.jpg", b"unobserved");

        let first = scan_sync(fav, &root, &db);
        assert_eq!(first.completeness, ObservationCompleteness::Complete);
        for candidate in first.to_ingest {
            if candidate.abs_path != root.join("new_image.jpg") {
                let changed = candidate.abs_path == root.join("updated_image.jpg");
                db.upsert_meta_ok(
                    &candidate.key,
                    fav,
                    &root,
                    IndexKind::Image,
                    if changed { 0 } else { candidate.diff_mtime },
                    if changed { 0 } else { candidate.diff_size },
                )
                .unwrap();
            }
        }
        let absent = normalize_path(&root.join("gone_image.jpg"));
        db.upsert_meta_ok(&absent, fav, &root, IndexKind::Image, 1, 1)
            .unwrap();

        let target = match fault {
            ObservationFault::ReadDir | ObservationFault::Entry => child,
            _ => child.join("unobserved_image.jpg"),
        };
        let result = scan_with_io(
            ScanParams {
                favorite_id: fav,
                root: root.clone(),
                excluded_roots: Vec::new(),
                cancel: Arc::new(AtomicBool::new(false)),
                progress: None,
            },
            &db,
            &GlobalIoSemaphore::new(2),
            IoPriority::Normal,
            None,
            &FaultIo { target, fault },
        )
        .unwrap();
        assert_eq!(result.completeness, ObservationCompleteness::Incomplete);
        assert!(
            result.to_delete.is_empty(),
            "不完全観測では別の枝の不在行も削除しない"
        );
        let ingests: std::collections::HashSet<_> = result
            .to_ingest
            .iter()
            .map(|candidate| candidate.abs_path.clone())
            .collect();
        assert_eq!(
            ingests,
            [root.join("new_image.jpg"), root.join("updated_image.jpg")]
                .into_iter()
                .collect()
        );
        assert!(result.unchanged >= 1);

        // エラーを解消した次の完全走査では、同じ不在行が通常どおり削除候補になる。
        let complete = scan_sync(fav, &root, &db);
        assert_eq!(complete.completeness, ObservationCompleteness::Complete);
        assert_eq!(complete.to_delete, vec![absent]);
        result.diag
    }

    #[test]
    fn read_dir_failure_suppresses_deletes_but_keeps_ingests() {
        assert_eq!(
            assert_incomplete_scan_keeps_observed_changes(ObservationFault::ReadDir)
                .read_dir_errors,
            1
        );
    }

    #[test]
    fn entry_failure_suppresses_deletes_but_keeps_ingests() {
        assert_eq!(
            assert_incomplete_scan_keeps_observed_changes(ObservationFault::Entry).entry_errors,
            1
        );
    }

    #[test]
    fn file_type_failure_suppresses_deletes_but_keeps_ingests() {
        assert_eq!(
            assert_incomplete_scan_keeps_observed_changes(ObservationFault::FileType)
                .file_type_errors,
            1
        );
    }

    #[test]
    fn classification_failure_suppresses_deletes_but_keeps_ingests() {
        assert_eq!(
            assert_incomplete_scan_keeps_observed_changes(ObservationFault::Classification)
                .classification_errors,
            1
        );
    }

    #[test]
    fn metadata_failure_suppresses_deletes_but_keeps_ingests() {
        assert_eq!(
            assert_incomplete_scan_keeps_observed_changes(ObservationFault::Metadata)
                .metadata_errors,
            1
        );
    }

    #[test]
    fn depth_limit_suppresses_deletes_but_keeps_ingests() {
        let (tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = tmp.path().join("deep");
        let mut deepest = root.clone();
        for _ in 0..41 {
            deepest.push("d");
        }
        fs::create_dir_all(&deepest).unwrap();
        make_file(&root, "new_image.jpg", b"new");
        make_file(&deepest, "hidden_image.jpg", b"hidden");
        for path in [
            deepest.join("hidden_image.jpg"),
            root.join("gone_image.jpg"),
        ] {
            db.upsert_meta_ok(&normalize_path(&path), fav, &root, IndexKind::Image, 1, 1)
                .unwrap();
        }
        let result = scan_sync(fav, &root, &db);
        assert_eq!(result.diag.depth_limit_hits, 1);
        assert_eq!(result.completeness, ObservationCompleteness::Incomplete);
        assert!(result.to_delete.is_empty());
        assert_eq!(result.to_ingest.len(), 1);
        assert_eq!(result.to_ingest[0].abs_path, root.join("new_image.jpg"));
    }

    #[test]
    fn missing_root_does_not_turn_unobserved_rows_into_deletes() {
        let (tmp, db) = tmp_db();
        let fav = Uuid::new_v4();
        let root = tmp.path().join("missing");
        let key = normalize_path(&root.join("image.jpg"));
        db.upsert_meta_ok(&key, fav, &root, IndexKind::Image, 1, 1)
            .unwrap();
        let result = scan_sync(fav, &root, &db);
        assert_eq!(result.completeness, ObservationCompleteness::Incomplete);
        assert_eq!(result.diag.read_dir_errors, 1);
        assert!(result.to_delete.is_empty());
        assert!(result.to_ingest.is_empty());
        assert!(db.get(&key).unwrap().is_some());
    }

    #[test]
    fn empty_fs_empty_db_returns_zero() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("photos");
        fs::create_dir_all(&root).unwrap();
        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 0);
        assert_eq!(r.completeness, ObservationCompleteness::Complete);
        assert!(r.to_ingest.is_empty());
        assert!(r.to_delete.is_empty());
    }

    #[test]
    fn new_files_go_to_ingest() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("p");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "a.jpg", b"xx");
        make_file(&root, "b.png", b"yy");
        make_file(&root, "ignore.txt", b"zz");
        // ZIP はアイテム索引の対象外なので候補にならない (§3.2)
        make_file(&root, "archive.zip", b"PK");
        make_file(&root, "doc.pdf", b"%PDF");
        make_file(&root, "book.epub", b"epub");
        make_file(&root, "clip.mp4", b"fake mp4");
        make_file(&root, "song.MP3", b"fake mp3");

        let r = scan_sync(fav, &root, &db);
        assert_eq!(
            r.total_scanned, 6,
            "jpg+png+pdf+epub+mp4+mp3 の 6 つ (zip/txt は除外)"
        );
        assert_eq!(r.to_ingest.len(), 6);
        assert_eq!(r.unchanged, 0);
        assert!(r.to_delete.is_empty());

        let kinds: Vec<_> = r.to_ingest.iter().map(|c| c.kind).collect();
        assert!(kinds.contains(&CandidateKind::Image));
        assert!(kinds.contains(&CandidateKind::Pdf));
        assert!(kinds.contains(&CandidateKind::Video));
        assert!(kinds.contains(&CandidateKind::Audio));
    }

    #[test]
    fn excluded_roots_are_not_scanned_and_stale_rows_are_deleted() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("p");
        let books_root = root.join("books");
        let book = books_root.join("名前なし");
        fs::create_dir_all(&book).unwrap();
        make_file(&root, "keep.jpg", b"ok");
        make_file(&book, "0001_page.png", b"compiled");

        let stale_key = normalize_path(&book.join("0001_page.png"));
        db.upsert_meta_ok(&stale_key, fav, &root, IndexKind::Image, 1, 1)
            .unwrap();

        let sem = GlobalIoSemaphore::new(2);
        let cancel = Arc::new(AtomicBool::new(false));
        let r = scan(
            ScanParams {
                favorite_id: fav,
                root: root.clone(),
                excluded_roots: vec![books_root],
                cancel,
                progress: None,
            },
            &db,
            &sem,
            IoPriority::Normal,
            None,
        )
        .unwrap();

        assert_eq!(r.total_scanned, 1, "除外 root 配下のページは候補にしない");
        assert_eq!(r.to_ingest.len(), 1);
        assert_eq!(r.to_ingest[0].abs_path, root.join("keep.jpg"));
        assert_eq!(
            r.to_delete,
            vec![stale_key],
            "除外 root 配下に残った旧行は削除候補に落とす"
        );
    }

    #[test]
    fn unchanged_files_not_re_ingested() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("u");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "a.jpg", b"hello");
        let abs = root.join("a.jpg");
        let metadata = abs.metadata().unwrap();
        let mtime = metadata
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let size = metadata.len() as i64;
        let key = normalize_path(&abs);

        // DB に同じ mtime/size で登録済み
        db.upsert_meta_ok(&key, fav, &root, IndexKind::Image, mtime, size)
            .unwrap();

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 1);
        assert_eq!(r.unchanged, 1);
        assert!(r.to_ingest.is_empty());
        assert!(r.to_delete.is_empty());
    }

    #[test]
    fn modified_files_go_to_re_ingest() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("m");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "a.jpg", b"x");
        let abs = root.join("a.jpg");
        let key = normalize_path(&abs);
        // DB に "古い" mtime で登録
        db.upsert_meta_ok(&key, fav, &root, IndexKind::Image, 1, 1)
            .unwrap();

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.unchanged, 0);
        assert_eq!(r.to_ingest.len(), 1, "mtime/size が変わったので再 ingest");
    }

    #[test]
    fn deleted_files_go_to_delete() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("d");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "survivor.jpg", b"s");

        let dead_key = normalize_path(&root.join("gone.jpg"));
        db.upsert_meta_ok(&dead_key, fav, &root, IndexKind::Image, 1, 1)
            .unwrap();
        let _ = dead_key.clone();
        let surv_key = normalize_path(&root.join("survivor.jpg"));
        let surv_meta = root.join("survivor.jpg").metadata().unwrap();
        let surv_mtime = surv_meta
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        db.upsert_meta_ok(
            &surv_key,
            fav,
            &root,
            IndexKind::Image,
            surv_mtime,
            surv_meta.len() as i64,
        )
        .unwrap();
        let _ = surv_key.clone();

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 1);
        assert_eq!(r.unchanged, 1);
        assert_eq!(r.to_delete, vec![dead_key]);
    }

    #[test]
    fn existing_zip_with_stale_db_row_goes_to_delete() {
        // 移行シナリオ: 旧版が索引した ZIP の fts_meta 行が残った状態で、ZIP ファイル
        // 自体は FS に存在し続ける。walker は ZIP を候補にしないので 3-way diff で
        // to_delete に落ち、ingest worker が Tantivy から掃除する (§3.2、Codex P3)。
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("z");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "album.zip", b"PK");
        let zip_key = normalize_path(&root.join("album.zip"));
        // 旧版が入れた ZIP 行を seed する
        db.upsert_meta_ok(&zip_key, fav, &root, IndexKind::Zip, 1, 1)
            .unwrap();

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 0, "ZIP は候補にならない");
        assert!(r.to_ingest.is_empty());
        assert_eq!(
            r.to_delete,
            vec![zip_key],
            "stale な ZIP 行が to_delete に落ちる"
        );
    }

    #[test]
    fn recursive_subdir_is_scanned() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("r");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(root.join("sub")).unwrap();
        make_file(&root, "top.jpg", b"1");
        make_file(&root.join("sub"), "nested.jpg", b"2");

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 2);
        assert_eq!(r.to_ingest.len(), 2);
    }

    #[cfg(windows)]
    #[test]
    fn directory_symlink_subtree_is_scanned_once_without_looping() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("root");
        let outside = tmp.path().join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        make_file(&outside, "linked.jpg", b"1");
        if std::os::windows::fs::symlink_dir(&outside, root.join("link")).is_err() {
            return;
        }
        if std::os::windows::fs::symlink_dir(&root, root.join("loop")).is_err() {
            return;
        }

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 1);
        assert_eq!(r.to_ingest.len(), 1);
        assert!(
            r.to_ingest[0].abs_path.ends_with("link\\linked.jpg")
                || r.to_ingest[0].abs_path.ends_with("link/linked.jpg"),
            "linked image should be indexed through the symlink path: {:?}",
            r.to_ingest[0].abs_path
        );
    }

    #[test]
    fn apple_double_files_are_ignored() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("a");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "photo.jpg", b"ok");
        make_file(&root, "._photo.jpg", b"metadata");

        let r = scan_sync(fav, &root, &db);
        assert_eq!(r.total_scanned, 1);
        assert_eq!(r.to_ingest.len(), 1);
        assert!(r.to_ingest[0].abs_path.ends_with("photo.jpg"));
    }

    #[test]
    fn scope_respects_favorite_id() {
        // 別 favorite 配下の登録は本 scan の diff 対象にならない
        let fav_a = Uuid::new_v4();
        let fav_b = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root_a = tmp.path().join("A");
        fs::create_dir_all(&root_a).unwrap();
        let key_b = normalize_path(&tmp.path().join("B/other.jpg"));
        // fav_b 所属の行を追加
        db.upsert_meta_ok(&key_b, fav_b, &tmp.path().join("B"), IndexKind::Image, 1, 1)
            .unwrap();

        // fav_a の scan 結果に fav_b は出てこない
        let r = scan_sync(fav_a, &root_a, &db);
        assert!(
            r.to_delete.is_empty(),
            "別 favorite の deleted は検出しない"
        );
    }

    #[test]
    fn diag_counters_increment_on_bad_dir() {
        // Codex 6 回目 nice-to-have #2: 診断 stats を出して原因調査を助ける
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        // 存在しないパスを root にする (read_dir がエラーを返すはず)
        let root = tmp.path().join("does_not_exist");
        let sem = GlobalIoSemaphore::new(2);
        let cancel = Arc::new(AtomicBool::new(false));
        let r = scan(
            ScanParams {
                favorite_id: fav,
                root,
                excluded_roots: Vec::new(),
                cancel,
                progress: None,
            },
            &db,
            &sem,
            IoPriority::Normal,
            None,
        )
        .unwrap();
        assert_eq!(r.total_scanned, 0);
        assert_eq!(
            r.diag.read_dir_errors, 1,
            "存在しないルートは read_dir エラーとしてカウントされるはず"
        );
    }

    #[test]
    fn cancel_stops_walk() {
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("c");
        fs::create_dir_all(&root).unwrap();
        for i in 0..50 {
            make_file(&root, &format!("f{}.jpg", i), b"x");
        }
        let cancel = Arc::new(AtomicBool::new(true)); // 最初から cancel
        let sem = GlobalIoSemaphore::new(2);
        let r = scan(
            ScanParams {
                favorite_id: fav,
                root: root.clone(),
                excluded_roots: Vec::new(),
                cancel,
                progress: None,
            },
            &db,
            &sem,
            IoPriority::Normal,
            None,
        );
        assert_eq!(r.unwrap_err(), "cancelled");
    }

    #[test]
    fn sidecar_removal_re_ingests_image() {
        // サイドカーを後から削除したら、画像本体が変わっていなくても再 ingest 候補になる
        // (stale な sidecar_text をクリアするため。docs §14-3)。
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("scrm");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "a.jpg", b"img");
        make_file(&root, "a.jpg.json", b"{\"k\":\"value\"}");

        // 1 回目: サイドカーありの差分署名で DB を seed する
        let r1 = scan_sync(fav, &root, &db);
        assert_eq!(r1.to_ingest.len(), 1);
        let cand = r1.to_ingest[0].clone();
        db.upsert_meta_ok(
            &cand.key,
            fav,
            &root,
            IndexKind::Image,
            cand.diff_mtime,
            cand.diff_size,
        )
        .unwrap();

        // 2 回目: 変化なし
        let r2 = scan_sync(fav, &root, &db);
        assert_eq!(r2.unchanged, 1, "サイドカー不変なら再 ingest しない");
        assert!(r2.to_ingest.is_empty());

        // サイドカー削除 → 3 回目で差分検出される
        fs::remove_file(root.join("a.jpg.json")).unwrap();
        let r3 = scan_sync(fav, &root, &db);
        assert_eq!(r3.to_ingest.len(), 1, "サイドカー削除で再 ingest される");
        assert_eq!(r3.unchanged, 0);
    }

    #[test]
    fn sidecar_priority_switch_same_size_re_ingests() {
        // Codex P3: `a.jpg.json` (優先1) 消失 → 同 size の `a.json` (優先3) に切替わったとき、
        // mtime/size が偶然一致しても差分署名の fingerprint がファイル名を含むので検出される。
        let fav = Uuid::new_v4();
        let (tmp, db) = tmp_db();
        let root = tmp.path().join("scsw");
        fs::create_dir_all(&root).unwrap();
        make_file(&root, "a.jpg", b"img");
        make_file(&root, "a.jpg.json", b"{\"x\":1}"); // 7 bytes

        // mtime も同一に固定する。これがないと、置換後 sidecar の mtime 差で
        // fingerprint 未導入の旧実装でもテストが通ってしまい、P3 修正を直接守れない。
        let sc_mtime = fs::metadata(root.join("a.jpg.json"))
            .unwrap()
            .modified()
            .unwrap();

        let r1 = scan_sync(fav, &root, &db);
        assert_eq!(r1.to_ingest.len(), 1);
        let cand = r1.to_ingest[0].clone();
        db.upsert_meta_ok(
            &cand.key,
            fav,
            &root,
            IndexKind::Image,
            cand.diff_mtime,
            cand.diff_size,
        )
        .unwrap();

        // full 形式を削除し、同じ 7 バイト・別名・**同一 mtime** の stem 形式に差し替える
        fs::remove_file(root.join("a.jpg.json")).unwrap();
        make_file(&root, "a.json", b"{\"y\":2}"); // 7 bytes
        std::fs::OpenOptions::new()
            .write(true)
            .open(root.join("a.json"))
            .unwrap()
            .set_modified(sc_mtime)
            .unwrap();

        let r2 = scan_sync(fav, &root, &db);
        assert_eq!(
            r2.to_ingest.len(),
            1,
            "サイドカーの優先順位切替 (同 size・同 mtime) でも fingerprint で再 ingest される"
        );
    }
}
