//! メタ情報と非破壊補正の Undo/Redo スタック。
//!
//! # スコープ
//!
//! このスタックが扱うのは次の軽量な値変更:
//!
//! - レーティング: F1-F5 / F6 / Shift+F1-F6 でファイル本体・コンテナ (フォルダ /
//!   ZIP / PDF) に付与/解除されるスター値。
//! - タグ: トグル (1 タグの追加/削除) と全 mIV タグクリア。
//! - 画像補正: ページ個別 / お気に入り標準 / 全体標準。
//! - 補正レイヤー: ページ単位のレイヤー配列。
//!
//! ファイル操作 (削除・移動・改名・カット&ペースト) は**含めない**。Cut/Paste は
//! mIV 自身が機能として持っていないし、削除はゴミ箱経由なので OS 側の責任で十分。
//! 実装範囲を絞ることで、スタックに積むエントリを「失敗しても巻き戻せる軽い操作」だけに
//! 限定できる。
//!
//! # 設計
//!
//! - 単一の VecDeque で `undo` / `redo` 2 本のスタックを保持。
//! - 1 操作 = 1 [`UndoEntry`]。10 ファイル一括の F3 でも、Ctrl+Z 1 回で全部戻る。
//! - 各エントリに `before` と `after` の両方を持たせ、Redo は `after` を再適用するだけ。
//! - 容量上限 ([`UndoStack::CAPACITY`]) 超過時は古い方から FIFO で捨てる。
//!
//! # フォルダ移動時の扱い
//!
//! `App::load_folder` で [`UndoStack::clear`] を呼んでスタックを破棄する。フォルダ間で
//! 操作を巻き戻す UX は混乱を生むため、シンプルに「現在フォルダの操作だけ Undo できる」
//! 仕様にしている。
//!
//! # タグ Undo の正しさ
//!
//! `tag_write_worker` は単一スレッド + FIFO キューで動くので、操作直後に「逆方向の状態
//! 復元ジョブ」をそのまま積めば順序は保たれる (詳細は `tag_write_worker.rs` 冒頭)。
//! 復元には [`crate::tag_write_worker::TagJobKind::SetTags`] を使い、worker 側は与えられた一覧で
//! `dc:subject` を完全に置き換える。Toggle の逆ではなく明示的な置換にしているのは、
//! Undo 待機中に外部ツールが XMP を書き換えた場合でも mIV が記録した「操作直前の状態」
//! にきっちり戻せるようにするため。

use std::collections::VecDeque;
use std::path::PathBuf;

use crate::adjustment::AdjustParams;

/// 画像補正の Undo がどの層に積まれたかを示す。
/// 書き戻しは `App::apply_adjustment_change_to_app` がスコープごとに分岐する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdjustUndoScope {
    /// ページ個別設定を**表示上の index だけ**で指す。ページキーを取れない項目
    /// (画像でない等) 用の退避先で、通常の補正はここへ積まない。
    ///
    /// この前提は「フルスクリーン中の画像移動・終了で undo_stack ごとクリアされるので
    /// idx は陳腐化しない」だったが、成り立つのは 1 つの窓の中だけだった。別の窓は自分の
    /// index を持つので、そこで Ctrl+Z を押すと同じ番号の別の画像を書き換えていた
    /// (v3.5.0 レビュー T01)。`App::page_adjust_undo_scope` を使って `PageKey` で積む。
    Page(usize),
    /// ページ個別設定を**ページの識別子**で指す。論理キーと永続化座標を capture 時に
    /// 固定し、並べ替え・別ウィンドウ・一覧に mount されていない remote ページでも
    /// 同じ対象へ Undo/Redo する。
    PageKey(crate::app::PageAdjustmentTarget),
    /// お気に入り標準 (`adjustment_favorite_params[uuid]`)。
    Favorite(uuid::Uuid),
    /// アプリ全体標準 (`settings.global_preset`)。
    Global,
}

/// 画像補正 1 件の変更記録。
///
/// `before` / `after` は `Option<AdjustParams>`:
/// - `None`: そのスコープに**エントリが無い**状態 (個別/お気に入り標準は省略可能)
/// - `Some(p)`: そのスコープに `p` が記録されている状態
///
/// `Global` スコープは常に `Some` (settings.global_preset は Optional ではない)。
#[derive(Debug, Clone, PartialEq)]
pub struct AdjustmentChange {
    pub scope: AdjustUndoScope,
    pub before: Option<AdjustParams>,
    pub after: Option<AdjustParams>,
}

/// 補正レイヤー 1 ページ分の変更記録。
///
/// 空配列は「補正レイヤーなし」を表す。DB/sidecar への反映は
/// `App::set_local_adjust_layers_for_idx` が担当する。
#[derive(Debug, Clone, PartialEq)]
pub struct LocalAdjustmentChange {
    pub idx: usize,
    pub before: local_adjust_core::LocalAdjustmentLayers,
    pub after: local_adjust_core::LocalAdjustmentLayers,
}

/// レーティング 1 件の変更記録。`path_key` は `adjustment_db::normalize_path` 正規化済み。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RatingChange {
    /// rating_db のキー (lowercased absolute path)
    pub path_key: String,
    /// XMP 書き込み判定や grid 走査用に元のパスも保持。
    /// コンテナ (フォルダ / ZIP / PDF) でも入れる。
    pub source_path: PathBuf,
    /// rating.db の復元用メタデータ。ZipDir など、path/source だけでは
    /// 種別を復元できない仮想コンテナの Undo/Redo で使う。
    pub meta: Option<crate::rating_db::RatingMeta>,
    /// 通常の評価書き込みと同じ適用条件で確定した物理 XMP 対象。
    /// 製本ページ・ZIP/PDF 内ページ・コンテナは `None`。
    pub xmp_target: Option<PathBuf>,
    pub before: u8,
    pub after: u8,
}

/// タグ 1 件の変更記録。`before`/`after` は mIV タグの表示リスト。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagChange {
    pub path: PathBuf,
    pub tag_sidecar: Option<crate::tag_write_worker::TagSidecarTarget>,
    /// 操作直前の mIV タグ。
    pub before: Vec<String>,
    /// 操作後に**期待される** mIV タグ。Redo で使う。
    pub after: Vec<String>,
}

/// Undo スタックに積む単位。バルク操作も 1 エントリにまとめる。
#[derive(Debug, Clone)]
pub enum UndoEntry {
    Rating {
        changes: Vec<RatingChange>,
        /// トースト表示用ラベル (例: "★3 を 5 件に付与")
        summary: String,
    },
    Tag {
        changes: Vec<TagChange>,
        summary: String,
    },
    Adjustment {
        changes: Vec<AdjustmentChange>,
        summary: String,
    },
    LocalAdjustment {
        changes: Vec<LocalAdjustmentChange>,
        summary: String,
    },
}

impl UndoEntry {
    pub fn summary(&self) -> &str {
        match self {
            UndoEntry::Rating { summary, .. }
            | UndoEntry::Tag { summary, .. }
            | UndoEntry::Adjustment { summary, .. }
            | UndoEntry::LocalAdjustment { summary, .. } => summary,
        }
    }

    /// changes が 1 件以上ある = Undo する価値がある。
    pub fn is_meaningful(&self) -> bool {
        match self {
            UndoEntry::Rating { changes, .. } => !changes.is_empty(),
            UndoEntry::Tag { changes, .. } => !changes.is_empty(),
            UndoEntry::Adjustment { changes, .. } => !changes.is_empty(),
            UndoEntry::LocalAdjustment { changes, .. } => !changes.is_empty(),
        }
    }
}

#[derive(Debug, Default)]
pub struct UndoStack {
    undo: VecDeque<UndoEntry>,
    redo: VecDeque<UndoEntry>,
}

impl UndoStack {
    /// Undo/Redo 各スタックの最大長。erase_undo_stack は 20 だが、メタ操作は
    /// メモリが軽い (PathBuf + Vec<String> 程度) ので 50 まで持たせる。
    pub const CAPACITY: usize = 50;

    pub fn new() -> Self {
        Self::default()
    }

    /// 新規エントリを Undo スタックに積む。Redo スタックはクリアされる。
    /// 空のエントリ ([`UndoEntry::is_meaningful`] が false) は無視。
    pub fn push(&mut self, entry: UndoEntry) {
        if !entry.is_meaningful() {
            return;
        }
        Self::push_capped(&mut self.undo, entry);
        self.redo.clear();
    }

    /// Undo: 最新エントリを取り出す (実行は呼び出し側)。
    /// 呼び出し側は `before` 状態を適用してから [`push_redo`] で Redo に積む。
    pub fn pop_undo(&mut self) -> Option<UndoEntry> {
        self.undo.pop_back()
    }

    /// Redo: 最新エントリを取り出す。
    /// 呼び出し側は `after` 状態を再適用してから [`push_undo_from_redo`] で戻す。
    pub fn pop_redo(&mut self) -> Option<UndoEntry> {
        self.redo.pop_back()
    }

    /// Undo 実行直後にエントリを Redo スタックへ移す。
    pub fn push_redo(&mut self, entry: UndoEntry) {
        Self::push_capped(&mut self.redo, entry);
    }

    /// Redo 実行直後にエントリを Undo スタックへ戻す (Redo を消費しない普通の push と
    /// 違って Redo クリアを伴わない)。
    pub fn push_undo_from_redo(&mut self, entry: UndoEntry) {
        Self::push_capped(&mut self.undo, entry);
    }

    /// 容量上限を超えたら古い順 (front) に捨てる FIFO 切り詰め付き push。
    fn push_capped(deque: &mut VecDeque<UndoEntry>, entry: UndoEntry) {
        deque.push_back(entry);
        while deque.len() > Self::CAPACITY {
            deque.pop_front();
        }
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn peek_undo(&self) -> Option<&UndoEntry> {
        self.undo.back()
    }

    pub fn peek_redo(&self) -> Option<&UndoEntry> {
        self.redo.back()
    }

    /// 両スタックを破棄。フォルダ移動時に呼ぶ。
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// 貼り付け / リセットで外部から置き換えたページについて、古い編集 Undo/Redo だけを
    /// 両スタックから除く。Rating / Tag、別ページ、Favorite / Global の補正履歴は残す。
    ///
    /// 1 entry が複数ページを含む場合は対象 change だけを落とし、残りを同じ entry として
    /// 保持する。これにより一括補正の別ページまで Undo 不可にしない。
    pub fn discard_page_edit_changes(
        &mut self,
        page_indices: &[usize],
        page_key: &str,
        adjustment: bool,
        local_adjustment: bool,
    ) {
        fn retain_entry(
            mut entry: UndoEntry,
            page_indices: &[usize],
            page_key: &str,
            adjustment: bool,
            local_adjustment: bool,
        ) -> Option<UndoEntry> {
            match &mut entry {
                UndoEntry::Adjustment { changes, .. } if adjustment => {
                    changes.retain(|change| match &change.scope {
                        AdjustUndoScope::Page(idx) => !page_indices.contains(idx),
                        AdjustUndoScope::PageKey(target) => target.page_key != page_key,
                        AdjustUndoScope::Favorite(_) | AdjustUndoScope::Global => true,
                    });
                }
                UndoEntry::LocalAdjustment { changes, .. } if local_adjustment => {
                    changes.retain(|change| !page_indices.contains(&change.idx));
                }
                _ => {}
            }
            entry.is_meaningful().then_some(entry)
        }

        fn prune(
            stack: &mut VecDeque<UndoEntry>,
            page_indices: &[usize],
            page_key: &str,
            adjustment: bool,
            local_adjustment: bool,
        ) {
            *stack = std::mem::take(stack)
                .into_iter()
                .filter_map(|entry| {
                    retain_entry(entry, page_indices, page_key, adjustment, local_adjustment)
                })
                .collect();
        }

        prune(
            &mut self.undo,
            page_indices,
            page_key,
            adjustment,
            local_adjustment,
        );
        prune(
            &mut self.redo,
            page_indices,
            page_key,
            adjustment,
            local_adjustment,
        );
    }

    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rating_entry(path: &str, before: u8, after: u8) -> UndoEntry {
        UndoEntry::Rating {
            changes: vec![RatingChange {
                path_key: path.to_lowercase(),
                source_path: PathBuf::from(path),
                meta: None,
                xmp_target: None,
                before,
                after,
            }],
            summary: format!("★{after}"),
        }
    }

    fn adjust_entry(
        scope: AdjustUndoScope,
        before: Option<AdjustParams>,
        after: Option<AdjustParams>,
    ) -> UndoEntry {
        UndoEntry::Adjustment {
            changes: vec![AdjustmentChange {
                scope,
                before,
                after,
            }],
            summary: "adj".into(),
        }
    }

    fn tag_entry(path: &str, before: Vec<&str>, after: Vec<&str>) -> UndoEntry {
        UndoEntry::Tag {
            changes: vec![TagChange {
                path: PathBuf::from(path),
                tag_sidecar: None,
                before: before.into_iter().map(String::from).collect(),
                after: after.into_iter().map(String::from).collect(),
            }],
            summary: "tag".into(),
        }
    }

    #[test]
    fn push_pop_cycle() {
        let mut s = UndoStack::new();
        assert!(!s.can_undo());
        s.push(rating_entry("c:/a.jpg", 0, 3));
        s.push(rating_entry("c:/b.jpg", 2, 5));
        assert_eq!(s.undo_len(), 2);
        assert!(s.can_undo());
        assert!(!s.can_redo());

        let popped = s.pop_undo().unwrap();
        match popped {
            UndoEntry::Rating { changes, .. } => {
                assert_eq!(changes[0].path_key, "c:/b.jpg");
                assert_eq!(changes[0].before, 2);
                assert_eq!(changes[0].after, 5);
            }
            _ => panic!("expected rating"),
        }
    }

    #[test]
    fn push_clears_redo() {
        let mut s = UndoStack::new();
        s.push(rating_entry("c:/a.jpg", 0, 3));
        let entry = s.pop_undo().unwrap();
        s.push_redo(entry);
        assert_eq!(s.redo_len(), 1);

        // 新しい操作は redo を捨てる
        s.push(rating_entry("c:/b.jpg", 0, 4));
        assert_eq!(s.redo_len(), 0);
        assert_eq!(s.undo_len(), 1);
    }

    #[test]
    fn redo_round_trip_preserves_after_state() {
        let mut s = UndoStack::new();
        s.push(rating_entry("c:/a.jpg", 1, 4));

        // Undo: pop → push to redo
        let e = s.pop_undo().unwrap();
        s.push_redo(e);
        assert!(s.can_redo());

        // Redo: pop redo → push back to undo
        let e = s.pop_redo().unwrap();
        match &e {
            UndoEntry::Rating { changes, .. } => {
                assert_eq!(changes[0].before, 1);
                assert_eq!(changes[0].after, 4);
            }
            _ => panic!(),
        }
        s.push_undo_from_redo(e);
        assert!(s.can_undo());
        assert!(!s.can_redo());
    }

    #[test]
    fn capacity_overflow_drops_oldest() {
        let mut s = UndoStack::new();
        for i in 0..(UndoStack::CAPACITY + 5) {
            s.push(rating_entry(&format!("c:/{i}.jpg"), 0, 1));
        }
        assert_eq!(s.undo_len(), UndoStack::CAPACITY);
        // 最古 5 件は捨てられている
        let oldest = match s.peek_undo() {
            Some(UndoEntry::Rating { changes, .. }) => changes[0].path_key.clone(),
            _ => panic!(),
        };
        // 最新のは "54.jpg" (CAPACITY=50, 0..55 で push)
        assert_eq!(oldest, format!("c:/{}.jpg", UndoStack::CAPACITY + 4));
    }

    #[test]
    fn empty_changes_are_ignored() {
        let mut s = UndoStack::new();
        s.push(UndoEntry::Rating {
            changes: vec![],
            summary: "noop".into(),
        });
        s.push(UndoEntry::Tag {
            changes: vec![],
            summary: "noop".into(),
        });
        s.push(UndoEntry::Adjustment {
            changes: vec![],
            summary: "noop".into(),
        });
        assert!(!s.can_undo());
    }

    #[test]
    fn adjustment_entry_round_trip() {
        let mut s = UndoStack::new();
        let before = AdjustParams::default();
        let mut after = AdjustParams::default();
        after.brightness = 25.0;
        s.push(adjust_entry(
            AdjustUndoScope::Page(7),
            None,
            Some(after.clone()),
        ));
        s.push(adjust_entry(
            AdjustUndoScope::Global,
            Some(before.clone()),
            Some(after.clone()),
        ));
        assert_eq!(s.undo_len(), 2);

        // pop → push_redo
        let e = s.pop_undo().unwrap();
        match &e {
            UndoEntry::Adjustment { changes, .. } => {
                assert!(matches!(changes[0].scope, AdjustUndoScope::Global));
                assert_eq!(changes[0].before.as_ref().unwrap().brightness, 0.0);
                assert_eq!(changes[0].after.as_ref().unwrap().brightness, 25.0);
            }
            _ => panic!("expected adjustment"),
        }
        s.push_redo(e);
        assert_eq!(s.redo_len(), 1);
    }

    #[test]
    fn clear_drops_both_stacks() {
        let mut s = UndoStack::new();
        s.push(rating_entry("c:/a.jpg", 0, 3));
        let e = s.pop_undo().unwrap();
        s.push_redo(e);
        assert!(s.can_redo());

        s.push(tag_entry("c:/b.jpg", vec!["#a"], vec!["#a", "#b"]));
        s.clear();
        assert!(!s.can_undo());
        assert!(!s.can_redo());
    }

    #[test]
    fn edit_bundle_bulk_discard_keeps_metadata_and_other_page_changes() {
        let mut s = UndoStack::new();
        s.push(rating_entry("c:/rating.jpg", 0, 3));
        s.push(UndoEntry::Adjustment {
            changes: vec![
                AdjustmentChange {
                    scope: AdjustUndoScope::Page(7),
                    before: None,
                    after: Some(AdjustParams::default()),
                },
                AdjustmentChange {
                    scope: AdjustUndoScope::Page(8),
                    before: None,
                    after: Some(AdjustParams::default()),
                },
                AdjustmentChange {
                    scope: AdjustUndoScope::PageKey(crate::app::PageAdjustmentTarget {
                        page_key: "remote-target".to_string(),
                        location_path: None,
                        sidecar_coords: None,
                        compiled_book: false,
                        idx_hint: crate::app::PageIndexHint::Unresolved,
                    }),
                    before: None,
                    after: Some(AdjustParams::default()),
                },
                AdjustmentChange {
                    scope: AdjustUndoScope::Global,
                    before: Some(AdjustParams::default()),
                    after: Some(AdjustParams {
                        brightness: 0.5,
                        ..AdjustParams::default()
                    }),
                },
            ],
            summary: "mixed adjustment".to_string(),
        });
        s.push(UndoEntry::LocalAdjustment {
            changes: vec![
                LocalAdjustmentChange {
                    idx: 7,
                    before: local_adjust_core::LocalAdjustmentLayers::default(),
                    after: local_adjust_core::LocalAdjustmentLayers::new(vec![
                        local_adjust_core::LocalAdjustmentLayer::new(
                            "target",
                            local_adjust_core::LocalMask::Full,
                            local_adjust_core::LocalEffect::None,
                        ),
                    ]),
                },
                LocalAdjustmentChange {
                    idx: 8,
                    before: local_adjust_core::LocalAdjustmentLayers::default(),
                    after: local_adjust_core::LocalAdjustmentLayers::new(vec![
                        local_adjust_core::LocalAdjustmentLayer::new(
                            "other",
                            local_adjust_core::LocalMask::Full,
                            local_adjust_core::LocalEffect::None,
                        ),
                    ]),
                },
            ],
            summary: "mixed local adjustment".to_string(),
        });
        let local_entry = s.pop_undo().unwrap();
        s.push_redo(local_entry);

        s.discard_page_edit_changes(&[7], "remote-target", true, true);

        assert_eq!(s.undo_len(), 2, "rating と adjustment entry は残る");
        assert_eq!(s.redo_len(), 1, "別ページの local-adjust change は残る");
        match &s.undo[1] {
            UndoEntry::Adjustment { changes, .. } => {
                assert_eq!(changes.len(), 2);
                assert!(matches!(changes[0].scope, AdjustUndoScope::Page(8)));
                assert!(matches!(changes[1].scope, AdjustUndoScope::Global));
            }
            _ => panic!("expected adjustment"),
        }
        match &s.redo[0] {
            UndoEntry::LocalAdjustment { changes, .. } => {
                assert_eq!(changes.len(), 1);
                assert_eq!(changes[0].idx, 8);
            }
            _ => panic!("expected local adjustment"),
        }
    }
}
