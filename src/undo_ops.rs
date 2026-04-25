//! Undo/Redo の積み込み・適用ロジック (App 側ファサード)。
//!
//! [`crate::undo_stack`] は純粋なデータ構造で、実際にレーティング DB やタグ worker と
//! やり取りするのはここ。スタック自体に App を持ち込まないことで undo_stack 単体で
//! テストしやすくしている。
//!
//! # キー設計
//!
//! - **Rating**: rating_db のキー (lowercased path) で記録。Undo 時に
//!   `apply_rating_change_to_app` が DB を直接書き換え、grid 内に該当 idx があれば
//!   `rating_cache` も同期する。
//! - **Tag**: パスと `dc:subject` の完全リストで記録。Undo 時は
//!   `TagJobKind::SetTags(target)` を worker に積むだけ。worker が単一スレッド + FIFO
//!   なので、ユーザーの操作順序を超えた race は発生しない (詳細は
//!   `tag_write_worker.rs` 冒頭参照)。

use std::path::PathBuf;

use crate::adjustment::AdjustParams;
use crate::app::App;
use crate::tag_ops::find_favorite_id;
use crate::tag_write_worker::{TagJobKind, TagWriteJob};
use crate::undo_stack::{
    AdjustUndoScope, AdjustmentChange, RatingChange, TagChange, UndoEntry,
};

impl App {
    // ── 積み込み (操作直後に呼ぶ) ────────────────────────────────────

    /// 1 回のレーティング操作 (単発 / バルク / コンテナ) を Undo スタックに積む。
    /// `before == after` の変更はフィルタ済みで OK。空エントリは undo_stack 側で破棄される。
    pub(crate) fn push_rating_undo_entry(
        &mut self,
        changes: Vec<RatingChange>,
        summary: String,
    ) {
        // 実質変化のないものは積まない (Ctrl+Z 1 回が "見た目変化なし" になるのを防ぐ)
        let filtered: Vec<RatingChange> =
            changes.into_iter().filter(|c| c.before != c.after).collect();
        if filtered.is_empty() {
            return;
        }
        self.meta_undo.push(UndoEntry::Rating {
            changes: filtered,
            summary,
        });
    }

    /// 1 回のタグ操作 (Toggle / ClearMiv / バルク) を Undo スタックに積む。
    /// `before == after` (= XMP 書き込みは発生しない見込み) はフィルタ済み。
    pub(crate) fn push_tag_undo_entry(&mut self, changes: Vec<TagChange>, summary: String) {
        let filtered: Vec<TagChange> =
            changes.into_iter().filter(|c| c.before != c.after).collect();
        if filtered.is_empty() {
            return;
        }
        self.meta_undo.push(UndoEntry::Tag {
            changes: filtered,
            summary,
        });
    }

    /// 1 回の画像補正操作 (スライダー drag-release / U・N・P / Q / Ctrl+1-9 / パネルの
    /// アクションボタン) を Undo スタックに積む。
    pub(crate) fn push_adjustment_undo_entry(
        &mut self,
        changes: Vec<AdjustmentChange>,
        summary: String,
    ) {
        let filtered: Vec<AdjustmentChange> = changes
            .into_iter()
            .filter(|c| c.before != c.after)
            .collect();
        if filtered.is_empty() {
            return;
        }
        self.meta_undo.push(UndoEntry::Adjustment {
            changes: filtered,
            summary,
        });
    }

    /// Undo/Redo スタック両方を破棄。フォルダ移動・フルスクリーン遷移・フルスクリーン
    /// 中の画像移動など「コンテキストが切り替わる境界」で呼ぶ。
    pub(crate) fn clear_meta_undo(&mut self) {
        if self.meta_undo.can_undo() || self.meta_undo.can_redo() {
            crate::logger::log(format!(
                "[UNDO] clear_meta_undo: drop {} undo, {} redo entries",
                self.meta_undo.undo_len(),
                self.meta_undo.redo_len(),
            ));
            self.meta_undo.clear();
        }
        // ドラッグ中に境界を跨いだ場合 (フォルダ移動・フルスクリーン遷移・消しゴム遷移)、
        // 進行中のセッションを安全に終了させる。
        //
        // ドラッグ中は DB / sidecar 書き込みをスキップしているため、在の `adjustment_page_params`
        // (in-memory) はドラッグ最後の値を持つが永続化されていない可能性がある。ここで
        // session.fs_idx の現在値を `set_page_params` で 1 回だけ書き出して、ユーザーが
        // 「ドラッグ中に画面が切り替わってしまっても操作は失われない」挙動を保証する。
        // Undo エントリは積まない (= boundary を跨ぐ操作は redo 不可) — Undo スタック自体を
        // クリアする処理の最中なので一貫性が取れる。
        if let Some(session) = self.adjustment_drag_session.take() {
            if let Some(p) = self.adjustment_page_params.get(&session.fs_idx).cloned() {
                if Some(&p) != session.before.as_ref() {
                    self.set_page_params(session.fs_idx, p);
                }
            }
        }
    }

    // ── 適用 (Ctrl+Z / Ctrl+Y で呼ぶ) ────────────────────────────────

    /// Ctrl+Z ハンドラ。スタック先頭を取り出し、`before` 状態を再適用してから
    /// Redo スタックへ移す。スタックが空なら no-op。
    pub(crate) fn apply_meta_undo(&mut self) {
        let Some(entry) = self.meta_undo.pop_undo() else {
            return;
        };
        let summary = entry.summary().to_string();
        crate::logger::log(format!("[UNDO] applying undo: {summary}"));
        match &entry {
            UndoEntry::Rating { changes, .. } => {
                for c in changes {
                    self.apply_rating_change_to_app(c, /* use_before */ true);
                }
                self.rebuild_visible_indices();
            }
            UndoEntry::Tag { changes, .. } => {
                self.submit_tag_restore_jobs(changes, /* use_before */ true);
            }
            UndoEntry::Adjustment { changes, .. } => {
                // カスケード復元順序: Global → Favorite → Page
                // (Page を先に書くと set_page_params の "matches default → prune" が
                //  古い Favorite 標準で発火して entry が消える事故が起きる。Codex P1)
                let mut order: Vec<usize> = (0..changes.len()).collect();
                order.sort_by_key(|&i| adjust_scope_priority(&changes[i].scope));
                for i in order {
                    self.apply_adjustment_change_to_app(&changes[i], true);
                }
            }
        }
        self.show_feedback_toast(format!("元に戻す: {summary}"));
        self.meta_undo.push_redo(entry);
    }

    /// Ctrl+Y / Ctrl+Shift+Z ハンドラ。Redo スタック先頭を取り出し、`after` 状態を
    /// 再適用してから Undo スタックへ戻す。
    pub(crate) fn apply_meta_redo(&mut self) {
        let Some(entry) = self.meta_undo.pop_redo() else {
            return;
        };
        let summary = entry.summary().to_string();
        crate::logger::log(format!("[UNDO] applying redo: {summary}"));
        match &entry {
            UndoEntry::Rating { changes, .. } => {
                for c in changes {
                    self.apply_rating_change_to_app(c, /* use_before */ false);
                }
                self.rebuild_visible_indices();
            }
            UndoEntry::Tag { changes, .. } => {
                self.submit_tag_restore_jobs(changes, /* use_before */ false);
            }
            UndoEntry::Adjustment { changes, .. } => {
                // Redo は元操作の自然順 (Page → Favorite → Global、つまり下層から上層)
                let mut order: Vec<usize> = (0..changes.len()).collect();
                order.sort_by_key(|&i| {
                    std::cmp::Reverse(adjust_scope_priority(&changes[i].scope))
                });
                for i in order {
                    self.apply_adjustment_change_to_app(&changes[i], false);
                }
            }
        }
        self.show_feedback_toast(format!("やり直し: {summary}"));
        self.meta_undo.push_undo_from_redo(entry);
    }

    /// グリッドとフルスクリーンの両ハンドラ共通: Ctrl+Z / Ctrl+Y / Ctrl+Shift+Z を消費して
    /// Undo/Redo を実行する。ダイアログ抑止や IME 抑止は呼び出し元 (`handle_keyboard` /
    /// フルスクリーン入力) が既に弾いてからここに来る前提。
    ///
    /// **consume 順序の注意**: egui の `consume_key` は `matches_logically` でマッチ
    /// するため、`Modifiers::CTRL` 指定でも Shift が併用された Ctrl+Shift+Z を吸って
    /// しまう。先に Ctrl+Shift+Z (Redo) → 次に Ctrl+Y (Redo) → 最後に Ctrl+Z (Undo)
    /// の順で consume することで、Ctrl+Shift+Z が Undo 側に流れない。
    pub(crate) fn handle_meta_undo_keys(&mut self, ctx: &egui::Context) {
        let (undo, redo) = ctx.input_mut(|i| {
            let redo = i.consume_key(
                egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
                egui::Key::Z,
            ) || i.consume_key(egui::Modifiers::CTRL, egui::Key::Y);
            let undo = i.consume_key(egui::Modifiers::CTRL, egui::Key::Z);
            (undo, redo)
        });
        if undo {
            self.apply_meta_undo();
        }
        if redo {
            self.apply_meta_redo();
        }
    }

    // ── 内部: 1 件適用 ──────────────────────────────────────────────

    /// レーティング 1 件の Undo/Redo 適用。`use_before=true` なら `before` を、false なら
    /// `after` を新しい値として書き戻す。
    ///
    /// 現在のグリッドに該当 path のアイテムがあれば、そのまま `App::set_rating(idx, ...)`
    /// を呼ぶ — これで rating_db / rating_cache / user_set_rating_keys /
    /// folder_rating_counts / current_folder_rating_cache / XMP worker submit など
    /// すべての副作用が一括で正しく走る (Undo 用に再実装するとフォルダ★件数バッジの
    /// 更新が抜けやすい)。フォルダ移動で undo_stack はクリアされるので、
    /// 「現在のグリッドに必ず該当 idx がある」前提は通常成立する。
    ///
    /// グリッドに該当 idx が無い (= search results 等) のレアケースは、最低限
    /// rating_db だけ書き戻す (visible_indices 再計算で表示は追従する)。
    fn apply_rating_change_to_app(&mut self, c: &RatingChange, use_before: bool) {
        let target = if use_before { c.before } else { c.after };

        // 該当 path のアイテムを idx で集める。`rating_path_key` は Image/ZipImage/
        // PdfPage と Folder/ZipFile/PdfFile を統一的に扱うため、ここから 1 経路で済む。
        let matching: Vec<usize> = (0..self.items.len())
            .filter(|&i| self.rating_path_key(i).as_deref() == Some(c.path_key.as_str()))
            .collect();

        if matching.is_empty() {
            // 現在のグリッドに無い (= 通常はコンテナレーティングの Undo: current_folder
            // 自身は self.items に含まれないため、ここに来る)。永続化に加えて、現在表示中
            // フォルダと一致する場合はアドレスバー側のキャッシュも同期する (Codex P2)。
            if let Some(db) = self.rating_db.as_ref() {
                let _ = db.set(&c.path_key, target);
            }
            self.user_set_rating_keys.insert(c.path_key.clone());
            if let Some(folder) = &self.current_folder {
                if crate::adjustment_db::normalize_path(folder) == c.path_key {
                    self.current_folder_rating_cache = Some(target);
                }
            }
            if self.settings.write_rating_to_xmp
                && crate::xmp_writer::is_writable_format(&c.source_path)
            {
                self.ensure_rating_write_handle();
                if let Some(h) = self.rating_write_handle.as_ref() {
                    h.submit(crate::rating_write_worker::RatingWriteJob {
                        path: c.source_path.clone(),
                        rating: if target == 0 { None } else { Some(target) },
                    });
                }
            }
            return;
        }

        for idx in matching {
            self.set_rating(idx, target);
        }
    }

    /// 画像補正 Undo/Redo の 1 件適用。スコープに応じて対応する書き込みパス
    /// (`set_page_params` / `clear_page_params` / `set_favorite_default` /
    /// `clear_favorite_default` / `copy_params_to_global`) に流す — これらは
    /// すべて DB / settings / キャッシュクリアの副作用を含むので、Undo 用に
    /// 別経路を組まずに既存の書き込み API を再利用する。
    fn apply_adjustment_change_to_app(&mut self, c: &AdjustmentChange, use_before: bool) {
        let target = if use_before { &c.before } else { &c.after };
        match &c.scope {
            AdjustUndoScope::Page(idx) => match target {
                Some(p) => self.set_page_params(*idx, p.clone()),
                None => {
                    // 個別エントリを消す経路。`clear_page_params` は AI キャッシュも
                    // 適切に落としてくれる。エントリが既に無ければ no-op。
                    if self.adjustment_page_params.contains_key(idx) {
                        self.clear_page_params(*idx);
                    }
                }
            },
            AdjustUndoScope::Favorite(id) => match target {
                Some(p) => self.set_favorite_default(*id, p.clone()),
                None => self.clear_favorite_default(*id),
            },
            AdjustUndoScope::Global => {
                // Global は常に値を持つので、Some 前提。万一 None ならデフォルトに戻す。
                let p = target.clone().unwrap_or_default();
                self.copy_params_to_global(p);
            }
        }
    }

    /// 単一スコープの単一変更を Undo スタックに積む共通ヘルパー。`before == after`
    /// は捨てられるので、呼び出し側で抑止判定を書かなくて済む。
    pub(crate) fn capture_adjustment_undo(
        &mut self,
        scope: AdjustUndoScope,
        before: Option<AdjustParams>,
        after: Option<AdjustParams>,
        summary: String,
    ) {
        self.push_adjustment_undo_entry(
            vec![AdjustmentChange {
                scope,
                before,
                after,
            }],
            summary,
        );
    }

    /// 補正書き込み操作の **直前 → 直後** で 3 層 (Page / Favorite / Global) すべての
    /// スナップショットを取り、差分を 1 つの `UndoEntry::Adjustment` にまとめて積む。
    ///
    /// 単に `(scope, before, after)` を 1 件積むだけだと不足するケース:
    /// - `set_favorite_default` は内部で「お気に入り標準と一致する個別ページを冗長判定で
    ///   削除」する。Favorite スコープの 1 件だけ Undo に積むと、削除された個別ページが
    ///   復元されない (Codex P1)。
    /// - `apply_params_to_all_pages` / `clear_all_page_params` は多数のページを 1 操作で
    ///   書き換える。スコープ 1 件では表現できない (Codex P2)。
    ///
    /// このヘルパーは write_op 前後の `adjustment_page_params` / `adjustment_favorite_params`
    /// / `settings.global_preset` をそれぞれ比較し、変化したエントリすべてを
    /// `AdjustmentChange` として記録する。
    pub(crate) fn capture_adjust_full<F>(&mut self, summary: String, write_op: F)
    where
        F: FnOnce(&mut App),
    {
        let pages_before = self.adjustment_page_params.clone();
        let favs_before = self.adjustment_favorite_params.clone();
        let global_before = self.settings.global_preset.clone();

        write_op(self);

        let mut changes: Vec<AdjustmentChange> = Vec::new();

        // Page 差分: 旧側のキー全部走査 (削除と更新)
        for (idx, before_p) in &pages_before {
            let after_p = self.adjustment_page_params.get(idx);
            if Some(before_p) != after_p {
                changes.push(AdjustmentChange {
                    scope: AdjustUndoScope::Page(*idx),
                    before: Some(before_p.clone()),
                    after: after_p.cloned(),
                });
            }
        }
        // 新規追加された Page (旧側に無かったキー)
        for (idx, after_p) in &self.adjustment_page_params {
            if !pages_before.contains_key(idx) {
                changes.push(AdjustmentChange {
                    scope: AdjustUndoScope::Page(*idx),
                    before: None,
                    after: Some(after_p.clone()),
                });
            }
        }

        // Favorite 差分
        for (id, before_p) in &favs_before {
            let after_p = self.adjustment_favorite_params.get(id);
            if Some(before_p) != after_p {
                changes.push(AdjustmentChange {
                    scope: AdjustUndoScope::Favorite(*id),
                    before: Some(before_p.clone()),
                    after: after_p.cloned(),
                });
            }
        }
        for (id, after_p) in &self.adjustment_favorite_params {
            if !favs_before.contains_key(id) {
                changes.push(AdjustmentChange {
                    scope: AdjustUndoScope::Favorite(*id),
                    before: None,
                    after: Some(after_p.clone()),
                });
            }
        }

        // Global 差分
        if global_before != self.settings.global_preset {
            changes.push(AdjustmentChange {
                scope: AdjustUndoScope::Global,
                before: Some(global_before),
                after: Some(self.settings.global_preset.clone()),
            });
        }

        self.push_adjustment_undo_entry(changes, summary);
    }

    /// タグ Undo/Redo の worker 投入。`use_before=true` なら各 path の `before` を
    /// SetTags で復元、false なら `after` を再適用。
    ///
    /// `capture_tag_undo` と同じく `tags_cache` を即時反映する (Codex P1)。
    /// 次の操作 (続けて Ctrl+Z や別トグル) が古いキャッシュを見ない。
    fn submit_tag_restore_jobs(&mut self, changes: &[TagChange], use_before: bool) {
        self.ensure_tag_write_handle();
        // 完了トーストのラベルは tag_ops の format_completion_toast で `restored` 件数を
        // 専用文言に切り替えるので、ここではラベル不要。
        self.tag_toast_label = None;
        let Some(h) = self.tag_write_handle.as_ref() else {
            self.show_feedback_toast(
                "タグ書き込みが初期化されていないため Undo できません".to_string(),
            );
            return;
        };
        for c in changes {
            let target = if use_before { &c.before } else { &c.after };
            let fav_id = find_favorite_id(&self.settings.favorites, &c.path);
            h.submit(TagWriteJob {
                path: c.path.clone(),
                kind: TagJobKind::SetTags(target.clone()),
                favorite_id: fav_id,
            });
        }
        // tags_cache 楽観的更新は h の借用を解放してから実施 (`&mut self`/`&self` 衝突回避)。
        for c in changes {
            let target = if use_before { &c.before } else { &c.after };
            let key = crate::adjustment_db::normalize_path(&c.path);
            self.tags_cache.insert(key, target.clone());
        }
    }
}

/// AdjustUndoScope のカスケード優先度。
/// Undo (use_before) は **昇順** に適用 (Global=0 が最初、Page=2 が最後)。
/// Redo (use_after) は **降順** に適用 (Page=2 が最初、Global=0 が最後)。
/// これで `set_page_params` の冗長判定 (matches_default → prune) が
/// 既に正しい上層状態の下で動く (Codex P1)。
fn adjust_scope_priority(scope: &AdjustUndoScope) -> u8 {
    match scope {
        AdjustUndoScope::Global => 0,
        AdjustUndoScope::Favorite(_) => 1,
        AdjustUndoScope::Page(_) => 2,
    }
}

// ── 操作キャプチャ ──────────────────────────────────────────────────
//
// レーティング/タグ操作の **直前** に呼ぶ snapshot ヘルパー。各操作のエントリポイント
// (`apply_rating_to_selection` 等) から呼ばれる。

impl App {
    /// レーティング操作をスタックに積むためのスナップショットを作る。
    /// `records` は (idx, before, after) の列。`before == after` は呼び出し側で
    /// 弾いていなくても [`Self::push_rating_undo_entry`] が落としてくれる。
    pub(crate) fn capture_rating_undo(
        &mut self,
        records: Vec<(usize, u8, u8)>,
        summary: String,
    ) {
        let mut changes = Vec::with_capacity(records.len());
        for (idx, before, after) in records {
            let Some(path_key) = self.rating_path_key(idx) else {
                continue;
            };
            let Some(source_path) = self.rating_source_path(idx) else {
                continue;
            };
            changes.push(RatingChange {
                path_key,
                source_path,
                before,
                after,
            });
        }
        self.push_rating_undo_entry(changes, summary);
    }

    /// コンテナレーティング (Shift+F*) 用。current_folder 1 件の変更だけを積む。
    pub(crate) fn capture_container_rating_undo(&mut self, before: u8, after: u8) {
        let Some(folder) = self.current_folder.clone() else {
            return;
        };
        let path_key = crate::adjustment_db::normalize_path(&folder);
        let summary = if after == 0 {
            "コンテナの★解除".to_string()
        } else {
            format!("コンテナを★{after}")
        };
        self.push_rating_undo_entry(
            vec![RatingChange {
                path_key,
                source_path: folder,
                before,
                after,
            }],
            summary,
        );
    }

    /// タグ操作の **直前** に呼ぶ。`paths` の各ファイルについて現在の dc:subject を
    /// `tags_cache` から取得し、`after_for` で操作後の期待状態を計算してエントリを作る。
    /// tags_cache に値が無い path はスキップ (= XMP 直読みは避ける、Undo より UI 応答性優先)。
    ///
    /// **重要 (Codex P1 対応)**: 計算した `after` を `tags_cache` に**即時反映**する。
    /// tag_write_worker は非同期なので、worker 完了前に次の操作 (例: 別タグのトグル) が
    /// 走ると、後続の `capture_tag_undo` が古い `tags_cache` 値で `before` を捏造してしまい、
    /// その Undo を実行すると先行操作分のタグまで剥がれる事故になる。`after` を先に
    /// 書き戻しておけば、worker poll 完了時の上書きと等価な値になり race を消せる。
    pub(crate) fn capture_tag_undo(
        &mut self,
        paths: &[PathBuf],
        summary: String,
        mut after_for: impl FnMut(&[String]) -> Vec<String>,
    ) {
        let mut changes = Vec::with_capacity(paths.len());
        for p in paths {
            let key = crate::adjustment_db::normalize_path(p);
            let Some(before) = self.tags_cache.get(&key).cloned() else {
                continue; // キャッシュ未読み込みは Undo 対象外 (希少ケース)
            };
            let after = after_for(&before);
            // 楽観的キャッシュ更新: 連続トグルでも次の capture が正しい before を見られるように。
            self.tags_cache.insert(key, after.clone());
            changes.push(TagChange {
                path: p.clone(),
                before,
                after,
            });
        }
        self.push_tag_undo_entry(changes, summary);
    }
}
