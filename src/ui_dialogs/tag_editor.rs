//! タグ編集ダイアログ (docs/tag-feature.md §4.1)。
//!
//! ユーザがタグ一覧を編集するための UI。お気に入り編集 (`favorites_editor.rs`) と
//! 同じ構造で、表示名編集 / 並べ替え / 削除 / 末尾への新規追加 をサポート。
//! よく使うタグをメニュー / ツールバーへピン留めするための管理 UI。

use std::collections::{HashMap, HashSet};
use std::sync::mpsc;

use eframe::egui;
use uuid::Uuid;

use crate::app::App;
use crate::settings::TagDef;

#[derive(Clone, Debug)]
struct RetagOp {
    old_key: String,
    new_key: String,
    new_name: String,
}

impl App {
    /// ダイアログを開く (呼び出し側で `show_tag_editor = true` にする前に呼ぶ)。
    /// 現在の Settings のタグ一覧を draft にコピーする。
    pub(crate) fn open_tag_editor(&mut self) {
        self.tag_editor_draft = self.settings.tags.clone();
        self.show_tag_editor = true;
    }

    pub(crate) fn show_tag_editor_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_tag_editor {
            return;
        }
        let mut open = true;
        let mut apply = false;
        let mut cancel = false;
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);

        let mut swap: Option<(usize, usize)> = None;
        let mut remove: Option<usize> = None;
        let mut add_empty_row = false;

        egui::Window::new("ピン留めタグの管理")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(480.0);

                ui.label(
                    egui::RichText::new(
                        "よく使うタグをピン留めすると、メニューとツールバーに表示されます。名前変更は既存タグにも反映され、既存タグ名にすると統合されます。",
                    )
                    .size(11.0)
                    .weak(),
                );
                ui.add_space(6.0);

                let n = self.tag_editor_draft.len();
                egui::ScrollArea::vertical()
                    .id_salt("tag_edit_scroll")
                    .max_height(360.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Grid::new("tag_edit_grid")
                            .striped(true)
                            .num_columns(4)
                            .spacing([8.0, 4.0])
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new("タグ名").strong());
                                ui.label(egui::RichText::new("プレビュー").strong());
                                ui.label(egui::RichText::new("表示").strong());
                                ui.label(egui::RichText::new("操作").strong());
                                ui.end_row();

                                for i in 0..n {
                                    let resp = ui.add_sized(
                                        [200.0, 20.0],
                                        egui::TextEdit::singleline(
                                            &mut self.tag_editor_draft[i].name,
                                        ),
                                    );
                                    // Enter で次行 or 追加行作成 (未実装、v1.1 検討)
                                    let _ = resp;

                                    // プレビュー (#付き、空なら "—")
                                    let name = self.tag_editor_draft[i].name.trim();
                                    let preview = if name.is_empty() {
                                        "—".to_string()
                                    } else {
                                        format!("#{name}")
                                    };
                                    ui.label(
                                        egui::RichText::new(preview)
                                            .monospace()
                                            .color(egui::Color32::from_rgb(100, 170, 100)),
                                    );

                                    ui.checkbox(
                                        &mut self.tag_editor_draft[i].show_shortcut,
                                        "ピン留め",
                                    );

                                    // 操作
                                    ui.horizontal(|ui| {
                                        let up_en = i > 0;
                                        let dn_en = i + 1 < n;
                                        if ui.add_enabled(up_en, egui::Button::new("↑")).clicked()
                                        {
                                            swap = Some((i - 1, i));
                                        }
                                        if ui.add_enabled(dn_en, egui::Button::new("↓")).clicked()
                                        {
                                            swap = Some((i, i + 1));
                                        }
                                        if ui.button("削除").clicked() {
                                            remove = Some(i);
                                        }
                                    });
                                    ui.end_row();
                                }
                            });
                    });

                ui.add_space(6.0);
                if ui.button("＋ タグを追加").clicked() {
                    add_empty_row = true;
                }
                if self.tag_editor_draft.iter().any(|tag| {
                    let name = crate::tags_db::normalize_tag_display_name(&tag.name);
                    !name.is_empty() && crate::tags_db::tag_display_name_has_whitespace(&name)
                }) {
                    ui.label(
                        egui::RichText::new("タグ名に空白は使えません。")
                            .size(11.0)
                            .color(egui::Color32::from_rgb(200, 80, 60)),
                    );
                }

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() {
                        apply = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });

                if enter_pressed {
                    apply = true;
                }
                if escape_pressed {
                    cancel = true;
                }
            });

        if let Some((a, b)) = swap {
            self.tag_editor_draft.swap(a, b);
        }
        if let Some(i) = remove {
            self.tag_editor_draft.remove(i);
        }
        if add_empty_row {
            self.tag_editor_draft.push(TagDef::new(String::new()));
        }

        if apply {
            if self.tag_maintenance_rx.is_some() {
                self.show_feedback_toast("タグの改名/統合を反映中です".to_string());
                return;
            }
            let previous = previous_tag_defs_by_id(&self.settings.tags);
            let draft = std::mem::take(&mut self.tag_editor_draft);
            let (cleaned, retag_ops) = build_tag_editor_apply_plan(draft, &previous);
            self.settings.tags = cleaned;
            self.settings.save();
            // タグ定義が変わったら、タグ候補 / 動画 native overlay 用キャッシュを必ず破棄する。
            // retag_ops が空 (ピン表示の切替や並び替えだけの変更) でも、fullscreen / video の
            // タグピッカーへ即反映させるため、tag maintenance の有無に関わらず invalidate する
            // (Codex P2 2026-06-19: 以前は start_tag_maintenance 経由でしか落ちず、ピン/並び替え
            //  だけの保存が再起動まで反映されなかった)。
            self.invalidate_tag_apply_suggestions();
            if !retag_ops.is_empty() {
                self.start_tag_maintenance(retag_ops);
            }
            self.show_tag_editor = false;
        } else if cancel || !open {
            self.show_tag_editor = false;
            self.tag_editor_draft.clear();
        }
    }

    fn start_tag_maintenance(&mut self, ops: Vec<RetagOp>) {
        let ops = order_retag_ops(ops);
        let data_dir = crate::data_dir::get();
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("tag-maintenance".to_string())
            .spawn(move || {
                let result = run_tag_maintenance(data_dir, ops);
                let _ = tx.send(result);
            })
            .ok();
        self.tag_maintenance_rx = Some(rx);
        self.show_feedback_toast("タグの改名/統合を反映中".to_string());
    }

    pub(crate) fn poll_tag_maintenance_results(&mut self) {
        let result = match self.tag_maintenance_rx.as_ref() {
            Some(rx) => match rx.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Empty) => None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("タグの改名/統合処理が終了しました".to_string()))
                }
            },
            None => None,
        };
        let Some(result) = result else {
            return;
        };
        self.tag_maintenance_rx = None;
        match result {
            Ok(message) => {
                self.tags_cache.clear();
                self.prewarm_grid_tags();
                if self.settings.facet_filter.uses_tag_state() {
                    self.rebuild_visible_indices();
                }
                if self.tag_view.active {
                    self.execute_tag_view();
                }
                self.invalidate_tag_apply_suggestions();
                self.show_feedback_toast(message);
            }
            Err(e) => self.show_feedback_toast(format!("タグの改名/統合に失敗: {e}")),
        }
    }
}

fn previous_tag_defs_by_id(tags: &[TagDef]) -> HashMap<Uuid, (String, String)> {
    tags.iter()
        .map(|tag| (tag.id, (tag.tag_key.clone(), tag.name.clone())))
        .collect()
}

fn build_tag_editor_apply_plan(
    draft: Vec<TagDef>,
    previous: &HashMap<Uuid, (String, String)>,
) -> (Vec<TagDef>, Vec<RetagOp>) {
    let mut cleaned: Vec<TagDef> = Vec::new();
    let mut by_key: HashMap<String, usize> = HashMap::new();
    let mut rows: Vec<(Option<(String, String)>, String)> = Vec::new();

    for t in draft {
        let Some(name) = normalize_editor_tag_name(&t.name) else {
            continue;
        };
        let tag_key = crate::tags_db::normalize_tag_key(&name);
        if tag_key.is_empty() {
            continue;
        }
        if let Some(&idx) = by_key.get(&tag_key) {
            cleaned[idx].show_shortcut |= t.show_shortcut;
        } else {
            by_key.insert(tag_key.clone(), cleaned.len());
            cleaned.push(TagDef {
                id: t.id,
                tag_key: tag_key.clone(),
                name,
                show_shortcut: t.show_shortcut,
            });
        }
        rows.push((previous.get(&t.id).cloned(), tag_key));
    }

    let display_by_key: HashMap<String, String> = cleaned
        .iter()
        .map(|tag| (tag.tag_key.clone(), tag.name.clone()))
        .collect();
    let mut retag_ops = Vec::new();
    let mut seen_old = HashSet::new();
    for (previous, new_key) in rows {
        let Some((old_key, old_name)) = previous else {
            continue;
        };
        if old_key.is_empty() || !seen_old.insert(old_key.clone()) {
            continue;
        }
        let Some(new_name) = display_by_key.get(&new_key).cloned() else {
            continue;
        };
        let old_display = crate::tags_db::normalize_tag_display_name(&old_name);
        if old_key != new_key || old_display != new_name {
            retag_ops.push(RetagOp {
                old_key,
                new_key,
                new_name,
            });
        }
    }

    (cleaned, retag_ops)
}

fn normalize_editor_tag_name(raw: &str) -> Option<String> {
    let mut name = raw.trim().to_string();
    while name.starts_with('#') {
        name.remove(0);
    }
    let name = crate::tags_db::normalize_tag_display_name(&name);
    if name.is_empty()
        || name.chars().count() > 64
        || crate::tags_db::tag_display_name_has_whitespace(&name)
    {
        None
    } else {
        Some(name)
    }
}

fn order_retag_ops(ops: Vec<RetagOp>) -> Vec<RetagOp> {
    let (mut key_ops, display_ops): (Vec<_>, Vec<_>) =
        ops.into_iter().partition(|op| op.old_key != op.new_key);
    let mut ordered = Vec::with_capacity(key_ops.len() + display_ops.len());
    // 改名サイクルを切るための一時キー経由 hop の「後段」(tmp → 最終キー)。
    // サイクル構成キーが全て退避し終わった後 (= ordered の末尾) で実行する。
    let mut deferred: Vec<RetagOp> = Vec::new();
    let mut tmp_counter = 0usize;
    while !key_ops.is_empty() {
        let old_keys: HashSet<_> = key_ops.iter().map(|op| op.old_key.as_str()).collect();
        if let Some(pos) = key_ops
            .iter()
            .position(|op| !old_keys.contains(op.new_key.as_str()))
        {
            ordered.push(key_ops.remove(pos));
        } else {
            // 残りが全て「new_key も別 op の old_key」= 改名サイクル (例: cat↔dog の
            // 入れ替え)。そのまま順次実行すると retag がマージ (衝突行の hard delete)
            // になり、2 つのタグが不可逆に合体する。先頭の op を一時キー経由の
            // 2 hop (old→tmp, 後段で tmp→new) に分割してサイクルを切る。
            // tmp は U+0001 を含む合成キーで、normalize_tag_key (trim/NFKC/lowercase)
            // を素通りし、かつユーザー入力と衝突しない。
            let mut op = key_ops.remove(0);
            tmp_counter += 1;
            let tmp = format!("\u{1}retag-tmp-{tmp_counter}");
            deferred.push(RetagOp {
                old_key: tmp.clone(),
                new_key: op.new_key.clone(),
                new_name: op.new_name.clone(),
            });
            op.new_key = tmp.clone();
            op.new_name = tmp;
            ordered.push(op);
        }
    }
    ordered.extend(deferred);
    ordered.extend(display_ops);
    ordered
}

fn run_tag_maintenance(data_dir: std::path::PathBuf, ops: Vec<RetagOp>) -> Result<String, String> {
    let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db"))
        .map_err(|e| format!("タグDBを開けません: {e}"))?;
    let mut affected = 0usize;
    let mut conflicts = 0usize;
    for op in ops {
        let report = db
            .retag_key(&op.old_key, &op.new_name)
            .map_err(|e| format!("{} -> {}: {e}", op.old_key, op.new_key))?;
        affected += report.affected_items;
        conflicts += report.removed_conflicts;
    }
    if conflicts > 0 {
        Ok(format!(
            "タグの改名/統合を反映: {affected} 件 ({conflicts} 件を統合)"
        ))
    } else {
        Ok(format!("タグの改名を反映: {affected} 件"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag_with(id: Uuid, key: &str, name: &str, show_shortcut: bool) -> TagDef {
        TagDef {
            id,
            tag_key: key.to_string(),
            name: name.to_string(),
            show_shortcut,
        }
    }

    #[test]
    fn apply_plan_keeps_key_and_updates_display_name() {
        let id = Uuid::new_v4();
        let previous = previous_tag_defs_by_id(&[tag_with(id, "fate", "FATE", true)]);
        let (cleaned, ops) =
            build_tag_editor_apply_plan(vec![tag_with(id, "fate", "Fate", true)], &previous);

        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned[0].tag_key, "fate");
        assert_eq!(cleaned[0].name, "Fate");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].old_key, "fate");
        assert_eq!(ops[0].new_key, "fate");
        assert_eq!(ops[0].new_name, "Fate");
    }

    #[test]
    fn apply_plan_merges_duplicate_target_keys() {
        let cat_id = Uuid::new_v4();
        let dog_id = Uuid::new_v4();
        let previous = previous_tag_defs_by_id(&[
            tag_with(cat_id, "cat", "cat", false),
            tag_with(dog_id, "dog", "dog", true),
        ]);
        let (cleaned, ops) = build_tag_editor_apply_plan(
            vec![
                tag_with(cat_id, "cat", "dog", false),
                tag_with(dog_id, "dog", "dog", true),
            ],
            &previous,
        );

        assert_eq!(cleaned.len(), 1);
        assert_eq!(cleaned[0].tag_key, "dog");
        assert!(cleaned[0].show_shortcut);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].old_key, "cat");
        assert_eq!(ops[0].new_key, "dog");
    }

    #[test]
    fn apply_plan_drops_whitespace_tag_names() {
        let id = Uuid::new_v4();
        let previous = previous_tag_defs_by_id(&[]);
        let (cleaned, ops) =
            build_tag_editor_apply_plan(vec![tag_with(id, "", "Blue Archive", true)], &previous);

        assert!(cleaned.is_empty());
        assert!(ops.is_empty());
    }

    fn retag(old_key: &str, new_key: &str, new_name: &str) -> RetagOp {
        RetagOp {
            old_key: old_key.to_string(),
            new_key: new_key.to_string(),
            new_name: new_name.to_string(),
        }
    }

    /// 改名サイクル (cat↔dog の入れ替え) は一時キー経由に分割される。
    /// 分割しないと retag が「マージ」として実行され 2 タグが不可逆に合体する。
    #[test]
    fn order_retag_ops_breaks_rename_cycle_via_temp_key() {
        let ordered = order_retag_ops(vec![retag("cat", "dog", "dog"), retag("dog", "cat", "cat")]);

        assert_eq!(ordered.len(), 3);
        // hop1: cat → tmp (tmp はユーザー入力と衝突しない U+0001 キー)
        assert_eq!(ordered[0].old_key, "cat");
        assert!(ordered[0].new_key.starts_with('\u{1}'));
        // dog → cat は cat が空いた後に実行できる
        assert_eq!(ordered[1].old_key, "dog");
        assert_eq!(ordered[1].new_key, "cat");
        // hop2: tmp → dog (サイクル解消後)
        assert_eq!(ordered[2].old_key, ordered[0].new_key);
        assert_eq!(ordered[2].new_key, "dog");
        assert_eq!(ordered[2].new_name, "dog");
    }

    /// 実 DB での swap 適用結果: アイテム集合が入れ替わるだけで合体しない回帰テスト。
    #[test]
    fn retag_swap_preserves_distinct_item_sets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut db = crate::tags_db::TagsDb::open_at(&dir.path().join("tags.db")).expect("open");
        db.set_item_tags("item_a", ["cat"], "edit").unwrap();
        db.set_item_tags("item_b", ["dog"], "edit").unwrap();

        let ordered = order_retag_ops(vec![retag("cat", "dog", "dog"), retag("dog", "cat", "cat")]);
        for op in ordered {
            db.retag_key(&op.old_key, &op.new_name).expect("retag");
        }

        assert_eq!(db.display_tags_for_item("item_a"), vec!["#dog"]);
        assert_eq!(db.display_tags_for_item("item_b"), vec!["#cat"]);
    }

    /// 3 タグのローテーション (a→b→c→a) も temp 1 個で解消される。
    #[test]
    fn order_retag_ops_handles_three_way_rotation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut db = crate::tags_db::TagsDb::open_at(&dir.path().join("tags.db")).expect("open");
        db.set_item_tags("item_a", ["a"], "edit").unwrap();
        db.set_item_tags("item_b", ["b"], "edit").unwrap();
        db.set_item_tags("item_c", ["c"], "edit").unwrap();

        let ordered = order_retag_ops(vec![
            retag("a", "b", "b"),
            retag("b", "c", "c"),
            retag("c", "a", "a"),
        ]);
        for op in ordered {
            db.retag_key(&op.old_key, &op.new_name).expect("retag");
        }

        assert_eq!(db.display_tags_for_item("item_a"), vec!["#b"]);
        assert_eq!(db.display_tags_for_item("item_b"), vec!["#c"]);
        assert_eq!(db.display_tags_for_item("item_c"), vec!["#a"]);
    }
}
