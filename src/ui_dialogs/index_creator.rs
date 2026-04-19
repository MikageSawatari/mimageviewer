//! 「お気に入り > インデックス作成」ダイアログ。
//!
//! `cache_creator.rs` と同じ構造で、チェックしたお気に入り配下を再帰的に走査して
//! フォルダ・ZIP・PDF の名称を `search_index.db` に登録する。
//! チェック状態は `settings.search_index_checks` に永続化される。

use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

use eframe::egui;

use crate::app::App;
use crate::folder_tree::{is_apple_double, walk_dirs_recursive_with_progress};
use crate::search_index_db::{IndexEntry, IndexKind};

impl App {
    pub(crate) fn show_index_creator_dialog(&mut self, ctx: &egui::Context) {
        if !self.ic.show {
            return;
        }

        // 完了初回に結果メッセージをセット
        if self.ic.finished.load(Ordering::Relaxed) && self.ic.result.is_none() {
            let done = self.ic.done.load(Ordering::Relaxed);
            let total = self.ic.total.load(Ordering::Relaxed);
            let cancelled = self.ic.cancel.load(Ordering::Relaxed);
            let entries = self.ic.entries.load(Ordering::Relaxed);
            self.ic.result = Some(if cancelled {
                format!(
                    "キャンセルされました（{} / {} フォルダ走査、{} 件登録）",
                    done, total, entries,
                )
            } else {
                format!(
                    "{} フォルダを走査し、{} 件を登録しました。",
                    done, entries,
                )
            });
        }

        let mut open = true;
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        egui::Window::new("インデックス作成")
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(520.0);

                if !self.ic.running && !self.ic.finished.load(Ordering::Relaxed) {
                    // ── 選択前画面 ──
                    ui.label(
                        egui::RichText::new(
                            "インデックスを作成すると、お気に入り＞検索でフォルダ・ZIPファイル・PDFファイルを検索できます。",
                        )
                        .weak(),
                    );
                    ui.add_space(6.0);
                    ui.separator();
                    ui.add_space(6.0);
                    ui.label("インデックスを作成するお気に入りを選んでください：");
                    ui.add_space(6.0);

                    if self.settings.favorites.is_empty() {
                        ui.label(egui::RichText::new("（お気に入りが未登録です）").weak());
                    } else {
                        // checked の長さが favorites と一致していることを保証
                        if self.ic.checked.len() != self.settings.favorites.len() {
                            self.ic.checked = vec![false; self.settings.favorites.len()];
                            // settings.search_index_checks から復元
                            for (i, fav) in self.settings.favorites.iter().enumerate() {
                                if self.settings.search_index_checks.iter().any(|p| p == &fav.path)
                                {
                                    self.ic.checked[i] = true;
                                }
                            }
                        }

                        for (i, fav) in self.settings.favorites.iter().enumerate() {
                            let label = format!("{}  ({})", fav.name, fav.path.display());
                            ui.checkbox(&mut self.ic.checked[i], label);
                        }
                    }

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    let any_checked = self.ic.checked.iter().any(|&b| b);
                    if ui
                        .add_enabled(any_checked, egui::Button::new("  インデックス作成  "))
                        .clicked()
                    {
                        // チェック状態を settings に保存
                        self.settings.search_index_checks = self
                            .settings
                            .favorites
                            .iter()
                            .zip(self.ic.checked.iter())
                            .filter_map(|(f, &c)| if c { Some(f.path.clone()) } else { None })
                            .collect();
                        self.settings.save();
                        self.start_index_creation();
                    }
                } else {
                    // ── 実行中 / 完了画面 ──
                    let counting = self.ic.counting.load(Ordering::Relaxed);
                    let total = self.ic.total.load(Ordering::Relaxed);
                    let done = self.ic.done.load(Ordering::Relaxed);
                    let entries = self.ic.entries.load(Ordering::Relaxed);

                    if counting {
                        ui.label("フォルダを列挙中…");
                    } else {
                        ui.label(format!("フォルダ: {} / {}", done, total));
                    }

                    let current = self.ic.current.lock().unwrap().clone();
                    if !current.is_empty() {
                        ui.label(
                            egui::RichText::new(format!("現在: {}", current))
                                .weak()
                                .small(),
                        );
                    }

                    ui.add_space(4.0);
                    ui.label(format!("登録エントリ: {} 件", entries));

                    ui.add_space(8.0);
                    ui.separator();
                    ui.add_space(4.0);

                    if self.ic.finished.load(Ordering::Relaxed) {
                        if let Some(ref msg) = self.ic.result {
                            ui.label(msg.as_str());
                            ui.add_space(4.0);
                        }
                        if ui.button("  閉じる  ").clicked() {
                            self.ic.show = false;
                            self.ic.running = false;
                        }
                    } else {
                        if ui.button("  キャンセル  ").clicked() {
                            self.ic.cancel.store(true, Ordering::Relaxed);
                        }
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    }
                }
            });

        if !open || escape_pressed {
            if self.ic.running && !self.ic.finished.load(Ordering::Relaxed) {
                self.ic.cancel.store(true, Ordering::Relaxed);
            }
            self.ic.show = false;
            self.ic.running = false;
        }
    }
}

impl App {
    /// バックグラウンドワーカーを起動してインデックスを構築する。
    pub(crate) fn start_index_creation(&mut self) {
        let targets: Vec<(String, PathBuf)> = self
            .settings
            .favorites
            .iter()
            .zip(self.ic.checked.iter())
            .filter_map(|(f, &c)| if c { Some((f.name.clone(), f.path.clone())) } else { None })
            .collect();
        if targets.is_empty() {
            return;
        }
        let Some(db) = self.search_index_db.clone() else {
            self.ic.result = Some("検索インデックス DB が開けませんでした。".to_string());
            self.ic.finished.store(true, Ordering::Relaxed);
            self.ic.running = true;
            return;
        };

        // 状態リセット
        self.ic.running = true;
        self.ic.counting.store(true, Ordering::Relaxed);
        self.ic.total.store(0, Ordering::Relaxed);
        self.ic.done.store(0, Ordering::Relaxed);
        self.ic.entries.store(0, Ordering::Relaxed);
        self.ic.finished.store(false, Ordering::Relaxed);
        self.ic.result = None;
        *self.ic.current.lock().unwrap() = String::new();
        let cancel = Arc::new(AtomicBool::new(false));
        self.ic.cancel = Arc::clone(&cancel);

        let counting = Arc::clone(&self.ic.counting);
        let total = Arc::clone(&self.ic.total);
        let done = Arc::clone(&self.ic.done);
        let entries_counter = Arc::clone(&self.ic.entries);
        let finished = Arc::clone(&self.ic.finished);
        let current = Arc::clone(&self.ic.current);

        std::thread::spawn(move || {
            // 最初に各お気に入り配下の既存エントリをクリア
            for (_, fav_path) in &targets {
                if cancel.load(Ordering::Relaxed) {
                    finished.store(true, Ordering::Relaxed);
                    return;
                }
                let _ = db.clear_for_favorite(fav_path);
            }

            // Pass 1: サブフォルダ列挙 (進捗として訪問中フォルダ名を current に反映)
            let mut all_folders: Vec<(PathBuf, PathBuf)> = Vec::new(); // (folder, favorite_root)
            let mut last_update = std::time::Instant::now();
            let update_interval = std::time::Duration::from_millis(200);
            for (_, fav_path) in &targets {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let mut found: Vec<PathBuf> = Vec::new();
                let current_ref = Arc::clone(&current);
                walk_dirs_recursive_with_progress(
                    fav_path,
                    &mut found,
                    &cancel,
                    &mut |visited| {
                        // スロットリング: 最後の更新から update_interval 経過している場合のみ書き込む
                        let now = std::time::Instant::now();
                        if now.duration_since(last_update) >= update_interval {
                            last_update = now;
                            if let Ok(mut s) = current_ref.lock() {
                                // UI 側は "現在: {s}" と表示するので path だけ入れる
                                *s = visited.to_string_lossy().to_string();
                            }
                        }
                    },
                );
                for f in found {
                    all_folders.push((f, fav_path.clone()));
                }
            }
            total.store(all_folders.len(), Ordering::Relaxed);
            counting.store(false, Ordering::Relaxed);

            if cancel.load(Ordering::Relaxed) {
                finished.store(true, Ordering::Relaxed);
                return;
            }

            // Pass 2: 各フォルダ直下の Folder/ZipFile/PdfFile を収集して upsert
            for (folder, fav_root) in &all_folders {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                let folder_display = targets
                    .iter()
                    .find(|(_, base)| folder.starts_with(base))
                    .map(|(name, base)| match folder.strip_prefix(base) {
                        Ok(rel) if rel.as_os_str().is_empty() => name.clone(),
                        Ok(rel) => format!("{} > {}", name, rel.to_string_lossy()),
                        Err(_) => folder.to_string_lossy().to_string(),
                    })
                    .unwrap_or_else(|| folder.to_string_lossy().to_string());
                *current.lock().unwrap() = folder_display;

                let mut children: Vec<IndexEntry> = Vec::new();
                if let Ok(entries) = std::fs::read_dir(folder) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if is_apple_double(&p) {
                            continue;
                        }
                        let meta = entry.metadata().ok();
                        let mtime = meta
                            .as_ref()
                            .map_or(0, |m| crate::ui_helpers::mtime_secs(m));
                        let kind = if p.is_dir() {
                            Some(IndexKind::Folder)
                        } else {
                            match p.extension().and_then(|e| e.to_str()).map(str::to_ascii_lowercase) {
                                Some(ref e) if e == "zip" => Some(IndexKind::ZipFile),
                                Some(ref e) if e == "pdf" => Some(IndexKind::PdfFile),
                                _ => None,
                            }
                        };
                        if let Some(kind) = kind {
                            let name = p
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("")
                                .to_string();
                            children.push(IndexEntry {
                                path: p,
                                display_name: name,
                                kind,
                                mtime,
                            });
                        }
                    }
                }

                if !children.is_empty() {
                    let n = children.len();
                    if let Err(e) = db.upsert_children(fav_root, folder, &children) {
                        crate::logger::log(format!("index upsert failed: {e}"));
                    } else {
                        entries_counter.fetch_add(n, Ordering::Relaxed);
                    }
                }
                done.fetch_add(1, Ordering::Relaxed);
            }

            *current.lock().unwrap() = String::new();
            finished.store(true, Ordering::Relaxed);
        });
    }
}

// -----------------------------------------------------------------------
// ダイアログ状態
// -----------------------------------------------------------------------

pub(crate) struct IndexCreatorState {
    pub show: bool,
    pub checked: Vec<bool>,
    pub running: bool,
    pub counting: Arc<AtomicBool>,
    pub total: Arc<AtomicUsize>,
    pub done: Arc<AtomicUsize>,
    /// DB に登録したエントリ数 (累積)
    pub entries: Arc<AtomicUsize>,
    pub cancel: Arc<AtomicBool>,
    pub current: Arc<Mutex<String>>,
    pub finished: Arc<AtomicBool>,
    pub result: Option<String>,
}

impl Default for IndexCreatorState {
    fn default() -> Self {
        Self {
            show: false,
            checked: Vec::new(),
            running: false,
            counting: Arc::new(AtomicBool::new(false)),
            total: Arc::new(AtomicUsize::new(0)),
            done: Arc::new(AtomicUsize::new(0)),
            entries: Arc::new(AtomicUsize::new(0)),
            cancel: Arc::new(AtomicBool::new(false)),
            current: Arc::new(Mutex::new(String::new())),
            finished: Arc::new(AtomicBool::new(false)),
            result: None,
        }
    }
}

impl IndexCreatorState {
    /// ダイアログを開き直すときの状態リセット (メニュークリック時に呼ばれる)。
    /// `checked` は呼び出し側が favorites と揃える。
    pub(crate) fn reset_for_open(&mut self) {
        self.running = false;
        self.result = None;
        self.total.store(0, Ordering::Relaxed);
        self.done.store(0, Ordering::Relaxed);
        self.entries.store(0, Ordering::Relaxed);
        self.finished.store(false, Ordering::Relaxed);
        *self.current.lock().unwrap() = String::new();
    }
}
