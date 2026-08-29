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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

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

/// 版が変わるたび / 隔離が起きるたびに増えるバックアップの保持世代数。
///
/// `bak1..bak10` はローテーションで最古が 1 個消えるが、`preupgrade-v*` と隔離した
/// `.corrupted-*` には **消す仕組みが無かった**。実機で `settings.db*` が 124 ファイル
/// 2,980 MB になっているのを観測した (2026-08-29、backlog §1.0c)。内訳は
/// `preupgrade-v*` 36 個 (1,053 MB) と隔離 22 個 (775 MB)。mIV は現在 46 版あるので、
/// 初期から使い続けている利用者は版の数だけ持つ。**量ではなく上限が無いことが問題**
/// なので、作る側で上限を持たせる。
pub const RETAINED_UNROTATED_BACKUPS: usize = 3;

/// 同じ世代に属するバックアップファイルの集まり。
///
/// 隔離は `settings.db{,-wal,-shm}.corrupted-<suffix>` の 3 ファイルで 1 世代。
/// **1 ファイルだけ消すと WAL の無い main が残る** ので、必ずセット単位で扱う。
#[derive(Debug, Clone)]
pub struct BackupGroup {
    pub key: String,
    /// セット内でいちばん新しい更新時刻。読めなければ `None`。
    pub modified: Option<SystemTime>,
    pub files: Vec<PathBuf>,
}

/// `data_dir` 直下から、`group_of` が同じキーを返すファイルを世代ごとに集める。
pub fn collect_backup_groups(
    data_dir: &Path,
    log: &dyn Fn(&str),
    group_of: &dyn Fn(&str) -> Option<String>,
) -> Vec<BackupGroup> {
    let entries = match std::fs::read_dir(data_dir) {
        Ok(entries) => entries,
        Err(e) => {
            log(&format!(
                "backup prune: cannot read {}: {e}",
                data_dir.display()
            ));
            return Vec::new();
        }
    };
    let mut by_key: BTreeMap<String, BackupGroup> = BTreeMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(key) = group_of(&name) else {
            continue;
        };
        let modified = entry.metadata().and_then(|meta| meta.modified()).ok();
        let group = by_key.entry(key.clone()).or_insert_with(|| BackupGroup {
            key,
            modified: None,
            files: Vec::new(),
        });
        group.files.push(entry.path());
        group.modified = match (group.modified, modified) {
            (Some(current), Some(next)) => Some(current.max(next)),
            (current, next) => current.or(next),
        };
    }
    by_key.into_values().collect()
}

/// 消す世代のキーを決める。**副作用は持たない。**
///
/// 新しい順に `keep` 個を残し、それ以外を返す。`modified` が読めなかったものは
/// **最新として扱って残す** — 日付が分からないものを消すより、上限を一時的に超えるほうが
/// 安全側。同時刻はキーの降順で決める (決定的にするためだけの規則)。
pub fn backup_groups_to_drop(groups: &[BackupGroup], keep: usize) -> Vec<String> {
    let mut ordered = groups.iter().collect::<Vec<_>>();
    ordered.sort_by(|a, b| match (a.modified, b.modified) {
        (Some(x), Some(y)) => y.cmp(&x).then_with(|| b.key.cmp(&a.key)),
        (None, Some(_)) => std::cmp::Ordering::Less,
        (Some(_), None) => std::cmp::Ordering::Greater,
        (None, None) => b.key.cmp(&a.key),
    });
    ordered
        .into_iter()
        .skip(keep)
        .map(|group| group.key.clone())
        .collect()
}

/// 指定したキーの世代を、**セットごと**削除する。戻り値は消えたファイル数。
pub fn remove_backup_groups(
    groups: &[BackupGroup],
    drop_keys: &[String],
    log: &dyn Fn(&str),
) -> usize {
    let mut removed = 0usize;
    for group in groups.iter().filter(|group| drop_keys.contains(&group.key)) {
        for path in &group.files {
            match std::fs::remove_file(path) {
                Ok(()) => {
                    log(&format!("backup prune: removed {}", path.display()));
                    removed += 1;
                }
                Err(e) => log(&format!(
                    "backup prune: cannot remove {}: {e}",
                    path.display()
                )),
            }
        }
    }
    removed
}

/// 新しい順に `keep` 世代だけ残して、残りをセットごと削除する。
///
/// **新しいバックアップが手に入ってから呼ぶこと。** 先に消すと、失敗したときに
/// 安全網が 1 世代ぶん減った状態で残る。
pub fn prune_backup_groups(
    data_dir: &Path,
    keep: usize,
    log: &dyn Fn(&str),
    group_of: &dyn Fn(&str) -> Option<String>,
) -> usize {
    let groups = collect_backup_groups(data_dir, log, group_of);
    if groups.len() <= keep {
        return 0;
    }
    let drop_keys = backup_groups_to_drop(&groups, keep);
    remove_backup_groups(&groups, &drop_keys, log)
}

/// `<db_file_name>.preupgrade-v<label>` の世代キー。1 ファイルで 1 世代。
pub fn preupgrade_group_of(db_file_name: &str, name: &str) -> Option<String> {
    let prefix = format!("{db_file_name}.preupgrade-v");
    name.strip_prefix(&prefix)
        .filter(|label| !label.is_empty())
        .map(|label| label.to_string())
}

/// `<db_file_name>{,-wal,-shm}.corrupted-<suffix>` の世代キー。3 ファイルで 1 世代。
pub fn quarantine_group_of(db_file_name: &str, name: &str) -> Option<String> {
    let (head, suffix) = name.split_once(".corrupted-")?;
    if suffix.is_empty() {
        return None;
    }
    let matches_family = head == db_file_name
        || head == format!("{db_file_name}-wal")
        || head == format!("{db_file_name}-shm");
    matches_family.then(|| suffix.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn group(key: &str, secs: Option<u64>, files: &[&str]) -> BackupGroup {
        BackupGroup {
            key: key.to_string(),
            modified: secs.map(|s| SystemTime::UNIX_EPOCH + Duration::from_secs(s)),
            files: files.iter().map(PathBuf::from).collect(),
        }
    }

    #[test]
    fn the_newest_generations_are_the_ones_kept() {
        let groups = vec![
            group("old", Some(100), &[]),
            group("newest", Some(400), &[]),
            group("middle", Some(300), &[]),
            group("oldest", Some(10), &[]),
        ];
        assert_eq!(
            backup_groups_to_drop(&groups, 2),
            vec!["old".to_string(), "oldest".to_string()]
        );
    }

    #[test]
    fn keeping_more_than_exist_drops_nothing() {
        let groups = vec![group("a", Some(1), &[]), group("b", Some(2), &[])];
        assert!(backup_groups_to_drop(&groups, 5).is_empty());
    }

    /// 日付が読めなかったものは消さない。**消すのは取り返しがつかない**ので、
    /// 上限を一時的に超えるほうを選ぶ。
    #[test]
    fn a_generation_with_no_date_is_kept() {
        let groups = vec![
            group("dated_new", Some(500), &[]),
            group("undated", None, &[]),
            group("dated_old", Some(100), &[]),
        ];
        assert_eq!(
            backup_groups_to_drop(&groups, 2),
            vec!["dated_old".to_string()]
        );
    }

    #[test]
    fn preupgrade_keys_are_one_file_each_and_ignore_everything_else() {
        assert_eq!(
            preupgrade_group_of("settings.db", "settings.db.preupgrade-v3.2.0"),
            Some("3.2.0".to_string())
        );
        assert_eq!(preupgrade_group_of("settings.db", "settings.db"), None);
        assert_eq!(preupgrade_group_of("settings.db", "settings.db.bak1"), None);
        assert_eq!(
            preupgrade_group_of("settings.db", "settings.db.preupgrade-v"),
            None
        );
        assert_eq!(
            preupgrade_group_of("settings.db", "tags.db.preupgrade-v3.2.0"),
            None
        );
    }

    /// 隔離は main / -wal / -shm の 3 ファイルで 1 世代。**同じ suffix に畳めないと
    /// 片方だけ消えて**、WAL の無い main が残る (quarantine の rollback が防いでいる形)。
    #[test]
    fn a_quarantine_set_is_one_generation() {
        for name in [
            "settings.db.corrupted-1756400000-0",
            "settings.db-wal.corrupted-1756400000-0",
            "settings.db-shm.corrupted-1756400000-0",
        ] {
            assert_eq!(
                quarantine_group_of("settings.db", name),
                Some("1756400000-0".to_string()),
                "{name}"
            );
        }
        assert_eq!(
            quarantine_group_of("settings.db", "settings.db.corrupted-"),
            None
        );
        assert_eq!(quarantine_group_of("settings.db", "settings.db"), None);
        assert_eq!(
            quarantine_group_of("settings.db", "tags.db.corrupted-1756400000-0"),
            None
        );
    }

    /// 集める側: 隔離の 3 ファイルが 1 世代に畳まれ、本体や bak は拾わない。
    #[test]
    fn collecting_folds_a_quarantine_set_and_ignores_the_live_database() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in [
            "settings.db",
            "settings.db-wal",
            "settings.db.bak1",
            "settings.db.corrupted-10-0",
            "settings.db-wal.corrupted-10-0",
            "settings.db-shm.corrupted-10-0",
            "settings.db.corrupted-20-0",
        ] {
            std::fs::write(root.join(name), b"x").unwrap();
        }
        let groups = collect_backup_groups(root, &|_| {}, &|name| {
            quarantine_group_of("settings.db", name)
        });
        let mut keys = groups.iter().map(|g| g.key.as_str()).collect::<Vec<_>>();
        keys.sort_unstable();
        assert_eq!(keys, vec!["10-0", "20-0"]);
        let first = groups.iter().find(|g| g.key == "10-0").unwrap();
        assert_eq!(first.files.len(), 3, "3 ファイルで 1 世代のはず");
    }

    /// 消す側: 指定した世代のファイルだけが消え、他は残る。
    #[test]
    fn removing_takes_the_whole_set_and_leaves_everything_else_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for name in [
            "settings.db",
            "settings.db.bak1",
            "settings.db.corrupted-10-0",
            "settings.db-wal.corrupted-10-0",
            "settings.db-shm.corrupted-10-0",
            "settings.db.corrupted-20-0",
        ] {
            std::fs::write(root.join(name), b"x").unwrap();
        }
        let groups = collect_backup_groups(root, &|_| {}, &|name| {
            quarantine_group_of("settings.db", name)
        });
        let removed = remove_backup_groups(&groups, &["10-0".to_string()], &|_| {});
        assert_eq!(removed, 3);
        for name in [
            "settings.db",
            "settings.db.bak1",
            "settings.db.corrupted-20-0",
        ] {
            assert!(root.join(name).exists(), "消してはいけない: {name}");
        }
        for name in [
            "settings.db.corrupted-10-0",
            "settings.db-wal.corrupted-10-0",
            "settings.db-shm.corrupted-10-0",
        ] {
            assert!(!root.join(name).exists(), "消えていない: {name}");
        }
    }
}
