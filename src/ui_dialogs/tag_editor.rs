//! タグ編集ダイアログ (docs/tag-feature.md §4.1)。
//!
//! ユーザがタグ一覧を編集するための UI。お気に入り編集 (`favorites_editor.rs`) と
//! 同じ構造で、表示名編集 / 並べ替え / 削除 / 末尾への新規追加 をサポート。
//! XMP 書き換えの注意書きを常時表示する (ダイアログ以外でユーザがタグを
//! トグル操作しても警告が出ないため、登録段階で 1 回は目に入るようにしている)。

use eframe::egui;

use crate::app::App;
use crate::settings::TagDef;

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

        egui::Window::new("タグを編集")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(480.0);

                ui.label(
                    egui::RichText::new(
                        "タグを付与すると画像ファイルの XMP メタデータを書き換えます\n\
                         (アトミック rename で書き込みますが、自動バックアップは\n\
                         作成しません)。",
                    )
                    .color(egui::Color32::from_rgb(200, 170, 60))
                    .size(11.0),
                );
                ui.add_space(4.0);
                ui.separator();
                ui.add_space(4.0);

                ui.label(
                    egui::RichText::new(
                        "登録したタグはメニューとツールバーに表示され、\n\
                         画像を選択してクリックすると `#タグ名` が付与されます。",
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
                            .num_columns(3)
                            .spacing([8.0, 4.0])
                            .show(ui, |ui| {
                                ui.label(egui::RichText::new("タグ名").strong());
                                ui.label(egui::RichText::new("プレビュー").strong());
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

                                    // 操作
                                    ui.horizontal(|ui| {
                                        let up_en = i > 0;
                                        let dn_en = i + 1 < n;
                                        if ui
                                            .add_enabled(up_en, egui::Button::new("↑"))
                                            .clicked()
                                        {
                                            swap = Some((i - 1, i));
                                        }
                                        if ui
                                            .add_enabled(dn_en, egui::Button::new("↓"))
                                            .clicked()
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
            // バリデーション:
            // - 空行は破棄
            // - 先頭 `#` は剥がす (ユーザが `#` を付けて入力した救済)
            // - 空白/制御文字は `_` に正規化 — 索引側 `build_tags_column` と同じ規則なので
            //   タグピッカーが挿入する `#タグ名` と Tantivy 内の表記が必ず一致する
            // - 重複は先着優先 (正規化後の lowercase でキー判定)
            let mut seen = std::collections::HashSet::new();
            let mut cleaned: Vec<TagDef> = Vec::new();
            for t in self.tag_editor_draft.drain(..) {
                let mut name = t.name.trim().to_string();
                while name.starts_with('#') {
                    name.remove(0);
                }
                let name = crate::ingest_text::canonicalize_tag_name(&name);
                if name.is_empty() {
                    continue;
                }
                let key = name.to_lowercase();
                if !seen.insert(key) {
                    continue;
                }
                cleaned.push(TagDef { id: t.id, name });
            }
            self.settings.tags = cleaned;
            self.settings.save();
            self.show_tag_editor = false;
            self.tag_editor_draft.clear();
        } else if cancel || !open {
            self.show_tag_editor = false;
            self.tag_editor_draft.clear();
        }
    }
}
