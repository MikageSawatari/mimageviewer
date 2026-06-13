//! SQLite DB の世代バックアップ (`<db>.bak1` .. `<db>.bak10`) 共有実装。
//!
//! settings.db (spec §6.1) と tags.db (D19) が同じアルゴリズムを使う。**順序が
//! crash 安全性の核心** (T42 / Codex P2 / 2026-05-16):
//!
//! 1. まず `VACUUM INTO` を temp 名 (`<db>.bak.tmp-snapshot`) に書き出す。
//!    失敗してもまだ bak1..bak10 は元の状態で保たれる。
//! 2. temp snapshot が手に入ってから bak10 削除 + bak1..bak9 → bak2..bak10 rotate。
//! 3. temp snapshot を bak1 に rename (失敗時は temp が残るので手動復旧可能)。
//!
//! これで「rotate 完了 + snapshot 失敗」でチェーンに穴が空く事故を構造的に防ぐ。
//! **このフローを変更するときは両 DB に同時に効くことを意識すること** (旧実装は
//! 2 ファイルにコピーされており、T42 級の修正が片方に入らないリスクがあった)。

use std::path::Path;

const MAX_GENERATIONS: usize = 10;

/// 世代バックアップを 1 回実行する。
///
/// - `db_file_name`: `tags.db` のような bak 接頭辞になるファイル名。
/// - `log`: 進捗/失敗のログ出力先 (settings.db は diag ログ併用のため注入式)。
/// - `snapshot_to`: `VACUUM INTO` 相当を行うクロージャ (各 DB の接続型に依存しない)。
///
/// 個別世代の rename 失敗は abort せずログのみ (世代を 1 つスキップしても次回
/// rotate で復帰する)。snapshot 失敗・bak1 への最終 rename 失敗は `Err` を返す。
pub fn rotate_generation_backups(
    data_dir: &Path,
    db_file_name: &str,
    log: &dyn Fn(&str),
    snapshot_to: &dyn Fn(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let bak = |n: usize| data_dir.join(format!("{db_file_name}.bak{n}"));
    let snapshot_tmp = data_dir.join(format!("{db_file_name}.bak.tmp-snapshot"));
    // 前回 crash で残った snapshot_tmp は消す
    let _ = std::fs::remove_file(&snapshot_tmp);

    // 1. VACUUM INTO temp_snapshot。ここで失敗したらまだ既存 bak1..bak10 は無事。
    snapshot_to(&snapshot_tmp)?;

    // 2. bak10 を削除 → bak9 → bak10, ..., bak1 → bak2 (新→古の rename)。
    let _ = std::fs::remove_file(bak(MAX_GENERATIONS));
    for n in (1..MAX_GENERATIONS).rev() {
        let src = bak(n);
        let dst = bak(n + 1);
        if src.exists()
            && let Err(e) = std::fs::rename(&src, &dst)
        {
            log(&format!(
                "rotate_backups rename {} -> {} failed: {e}",
                src.display(),
                dst.display()
            ));
        }
    }

    // 3. temp snapshot を bak1 へ。前段 rename で bak1 は空のはずだが、残っている
    //    場合は事前 remove (rename の安全性確保)。失敗時は temp が残り手動復旧可能。
    let bak1 = bak(1);
    if bak1.exists()
        && let Err(e) = std::fs::remove_file(&bak1)
    {
        log(&format!("rotate_backups: residual bak1 remove failed: {e}"));
        return Err(format!("bak1 residual cannot be removed: {e}"));
    }
    if let Err(e) = std::fs::rename(&snapshot_tmp, &bak1) {
        log(&format!(
            "rotate_backups: rename {} -> {} failed: {e}",
            snapshot_tmp.display(),
            bak1.display()
        ));
        return Err(format!("snapshot tmp rename to bak1 failed: {e}"));
    }
    log(&format!("rotate_backups: snapshot -> {}", bak1.display()));
    Ok(())
}
