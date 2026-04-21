//! Ctrl+G グローバルメタ検索の UI + 状態管理 (docs/search-expansion-design.md §10.3)。
//!
//! - 状態: `GlobalSearchState` (App が所有)
//! - キーバインド: Ctrl+G で toggle
//! - トップパネル: クエリ入力 + 進捗バッジ
//! - streaming 受信: `poll_events()` が SearchStreamEvent を try_recv で消費、
//!   ContainerHit に集約、items を再構築
//!
//! ## v1 スコープ
//!
//! - トップレベル集約ビュー (ヒットを含むフォルダ/ZIP を SearchContainer セルで並べる)
//! - Drill-down view (1 階層降りた先の絞り込み表示) は本モジュールには含まず、
//!   後続コミットで追加予定 (docs §10.3 の [3] 絞り込みビュー)

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::app::App;
use crate::global_search::{DoneReason, GlobalHit, SearchStreamEvent};
use crate::grid_item::{GridItem, SearchContainerKind};
use crate::indexer_manager::SearchHandle;

/// クエリ入力後、検索実行までの debounce 間隔 (既存 Ctrl+F と揃える)。
const DEBOUNCE_MS: u64 = 300;
/// 1 フレームで消費するイベント数の上限 (UI ブロックを防ぐ)。
const MAX_EVENTS_PER_FRAME: usize = 8;
/// ContainerHit の再ソート間隔 (チラつき防止、docs §10.4.3)。
const RESORT_INTERVAL_MS: u64 = 1000;

/// Ctrl+G のビュー状態 (v1 は 1 階層 drill-down のみ、docs §10.3 [2]-[3])。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalSearchView {
    /// トップレベル集約表示。SearchContainer セルがヒット件数降順で並ぶ
    Aggregated,
    /// drill-down 中。`container` の配下のヒットファイル一覧を表示する
    DrilledInto { container: PathBuf, is_zip: bool },
}

/// Ctrl+G 検索の状態 (App が所有する)。
pub struct GlobalSearchState {
    /// true のとき、トップバー表示 + 検索結果ビュー有効
    pub active: bool,
    /// クエリ入力テキスト
    pub query: String,
    /// 最後に実行したクエリ (変更検知用)
    pub last_executed: String,
    /// 次フレームでフォーカス要求
    pub focus_request: bool,
    /// TextEdit がフォーカスを持っているか (他キーバインドが取らないようにするため)
    pub has_focus: bool,
    /// 入力が変わった時刻 (debounce 用)
    pub last_change_at: Option<Instant>,
    /// 最後に結果をソートし直した時刻
    pub last_sort_at: Option<Instant>,
    /// 起動中の検索
    pub pending: Option<SearchHandle>,
    /// 親コンテナ path → 集約済みヒット (docs §10.4.2 ContainerHit)
    pub containers: HashMap<PathBuf, ContainerHit>,
    /// 生の全ヒット (drill-down 時に container でフィルタするため保持)
    pub all_hits: Vec<GlobalHit>,
    /// 現在のビューモード (Aggregated or DrilledInto)
    pub view: GlobalSearchView,
    /// streaming 経過統計
    pub total_valid: usize,
    pub total_scanned: usize,
    /// 完了フラグ
    pub done: bool,
    /// HARD_MAX で打ち切られたか
    pub truncated: bool,
    /// RejectReason のユーザー向けメッセージ (空クエリ / 短すぎる / NOT-only)
    pub reject_message: Option<String>,
    /// Ctrl+G 以前の current_folder (戻り先として保存)
    pub saved_folder: Option<PathBuf>,
}

impl Default for GlobalSearchState {
    fn default() -> Self {
        Self {
            active: false,
            query: String::new(),
            last_executed: String::new(),
            focus_request: false,
            has_focus: false,
            last_change_at: None,
            last_sort_at: None,
            pending: None,
            containers: HashMap::new(),
            all_hits: Vec::new(),
            view: GlobalSearchView::Aggregated,
            total_valid: 0,
            total_scanned: 0,
            done: false,
            truncated: false,
            reject_message: None,
            saved_folder: None,
        }
    }
}

/// 集約済みコンテナ (v1 トップレベルビュー用)。
#[derive(Clone, Debug)]
pub struct ContainerHit {
    pub path: PathBuf,
    pub kind: SearchContainerKind,
    pub hit_count: usize,
}

impl GlobalSearchState {
    /// 新規検索を開始する (既存 pending があれば cancel してから)。
    pub fn reset_for_new_query(&mut self) {
        // SearchHandle は Drop で cancel するので、take() だけで OK
        self.pending = None;
        self.containers.clear();
        self.all_hits.clear();
        self.view = GlobalSearchView::Aggregated; // 新クエリで drill state もリセット
        self.total_valid = 0;
        self.total_scanned = 0;
        self.done = false;
        self.truncated = false;
        self.reject_message = None;
    }

    /// 集約ロジック (docs §10.4.2): 1 ヒットをコンテナに追加 + 生データも保持
    fn accumulate_hit(&mut self, hit: &GlobalHit) {
        let (container_path, kind) = parent_container(&hit.path);
        let entry = self
            .containers
            .entry(container_path.clone())
            .or_insert_with(|| ContainerHit {
                path: container_path,
                kind,
                hit_count: 0,
            });
        entry.hit_count += 1;
        // drill-down 用に生のヒットも保持 (path で後でフィルタする)
        self.all_hits.push(hit.clone());
    }
}

/// ヒット path から親コンテナを決定する (docs §10.4.2)。
/// - ZIP エントリ (`<zippath>!<entry>` 形式) → ZIP ファイルパス
/// - 通常ファイル → 親フォルダパス
fn parent_container(hit_path: &str) -> (PathBuf, SearchContainerKind) {
    if let Some(idx) = hit_path.find('!') {
        let (zip_part, _entry) = hit_path.split_at(idx);
        return (PathBuf::from(zip_part), SearchContainerKind::Zip);
    }
    let p = PathBuf::from(hit_path);
    if let Some(parent) = p.parent() {
        (parent.to_path_buf(), SearchContainerKind::Folder)
    } else {
        (p, SearchContainerKind::Folder)
    }
}

/// ContainerHit を hit_count 降順 / 名前昇順 でソートした Vec を返す。
pub fn sorted_containers(
    containers: &HashMap<PathBuf, ContainerHit>,
) -> Vec<ContainerHit> {
    let mut v: Vec<ContainerHit> = containers.values().cloned().collect();
    v.sort_by(|a, b| {
        b.hit_count
            .cmp(&a.hit_count)
            .then_with(|| a.path.cmp(&b.path))
    });
    v
}

// -----------------------------------------------------------------------
// App 側との連携 (impl App 拡張)
// -----------------------------------------------------------------------

impl App {
    /// Ctrl+G を押したときのエントリ (open or close toggle)。
    pub(crate) fn toggle_global_search(&mut self) {
        if self.global_search.active {
            self.close_global_search();
        } else {
            self.open_global_search();
        }
    }

    pub(crate) fn open_global_search(&mut self) {
        if self.global_search.active {
            return;
        }
        self.global_search.active = true;
        self.global_search.focus_request = true;
        self.global_search.saved_folder = self.current_folder.clone();
    }

    pub(crate) fn close_global_search(&mut self) {
        if !self.global_search.active {
            return;
        }
        // pending があれば SearchHandle の Drop impl で cancel される
        self.global_search.pending = None;
        self.global_search.active = false;
        self.global_search.has_focus = false;
        // Codex round-9 Should-fix #2: drill state / all_hits / done フラグも明示クリア。
        // 旧実装は containers だけクリアしていたため、DrilledInto のまま閉じて再度 Ctrl+G を
        // 開くと、検索前なのに戻るボタンや古い drill-down UI が残る可能性があった。
        self.global_search.containers.clear();
        self.global_search.all_hits.clear();
        self.global_search.view = GlobalSearchView::Aggregated;
        self.global_search.query.clear();
        self.global_search.last_executed.clear();
        self.global_search.reject_message = None;
        self.global_search.done = false;
        self.global_search.truncated = false;
        self.global_search.total_valid = 0;
        self.global_search.total_scanned = 0;
        // 元のフォルダに戻る
        if let Some(folder) = self.global_search.saved_folder.take() {
            self.load_folder(folder);
        }
    }

    /// debounce 経過チェック + 新クエリがあれば検索 spawn (App::update から毎フレーム呼ぶ)。
    pub(crate) fn poll_global_search_debounce(&mut self) {
        if !self.global_search.active {
            return;
        }
        // クエリが変わっていないならスキップ
        if self.global_search.query == self.global_search.last_executed {
            return;
        }
        // debounce: 最後の変更から DEBOUNCE_MS 経過するまで待つ
        let Some(t) = self.global_search.last_change_at else {
            return;
        };
        if t.elapsed() < Duration::from_millis(DEBOUNCE_MS) {
            return;
        }
        self.spawn_global_search();
    }

    /// 現在のクエリで検索を spawn する。
    pub(crate) fn spawn_global_search(&mut self) {
        self.global_search.reset_for_new_query();
        self.global_search.last_executed = self.global_search.query.clone();

        let Some(mgr) = self.indexer_manager.as_ref() else {
            self.global_search.reject_message =
                Some("全文検索インデクサが利用できません".to_string());
            self.global_search.done = true;
            self.rebuild_items_from_global_search();
            return;
        };

        // auto_index_metadata=true のお気に入り UUID を集める
        let favs: Vec<uuid::Uuid> = self
            .settings
            .favorites
            .iter()
            .filter(|f| f.auto_index_metadata)
            .map(|f| f.id)
            .collect();

        let handle = mgr.spawn_search(self.global_search.query.clone(), favs);
        self.global_search.pending = Some(handle);
        // items を空にして "検索中" 表示に切り替え
        self.rebuild_items_from_global_search();
    }

    /// SearchStreamEvent を try_recv で処理する (毎フレーム呼ぶ)。
    pub(crate) fn poll_global_search_events(&mut self, ctx: &egui::Context) {
        if self.global_search.done {
            return;
        }
        // rx を clone してから global_search の他フィールドを可変借用可能にする
        // (crossbeam-channel の Receiver は Clone 可能)
        let rx = match self.global_search.pending.as_ref() {
            Some(h) => h.rx.clone(),
            None => return,
        };
        let mut events_processed = 0;
        let mut changed = false;
        while events_processed < MAX_EVENTS_PER_FRAME {
            match rx.try_recv() {
                Ok(SearchStreamEvent::Batch {
                    hits,
                    scanned_candidates,
                    valid_hits,
                }) => {
                    for h in &hits {
                        self.global_search.accumulate_hit(h);
                    }
                    self.global_search.total_scanned = scanned_candidates;
                    self.global_search.total_valid = valid_hits;
                    if !hits.is_empty() {
                        changed = true;
                    }
                    events_processed += 1;
                }
                Ok(SearchStreamEvent::Done { truncated, reason }) => {
                    self.global_search.done = true;
                    self.global_search.truncated = truncated;
                    match reason {
                        DoneReason::RejectedQuery(r) => {
                            self.global_search.reject_message =
                                Some(r.as_user_message().to_string());
                        }
                        _ => {}
                    }
                    changed = true;
                    break;
                }
                Ok(SearchStreamEvent::Error(msg)) => {
                    self.global_search.done = true;
                    self.global_search.reject_message = Some(format!("エラー: {msg}"));
                    changed = true;
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.global_search.done = true;
                    changed = true;
                    break;
                }
            }
        }
        if changed {
            // docs §10.4.3: 順序再評価は 1 秒毎で十分 (頻繁な入れ替えでチラつかない)
            let should_resort = match self.global_search.last_sort_at {
                None => true,
                Some(t) => t.elapsed() >= Duration::from_millis(RESORT_INTERVAL_MS),
            };
            if should_resort || self.global_search.done {
                self.rebuild_items_from_global_search();
                self.global_search.last_sort_at = Some(Instant::now());
            }
        }
        if !self.global_search.done {
            // 次イベントを拾うため再描画を要求
            ctx.request_repaint();
        }
    }

    /// 現在の view モードに応じて App::items を再構築する。
    /// 既存 items は全て捨てて置き換える。
    pub(crate) fn rebuild_items_from_global_search(&mut self) {
        self.items.clear();
        self.thumbnails.clear();
        self.selected = None;

        match self.global_search.view.clone() {
            GlobalSearchView::Aggregated => {
                let containers = sorted_containers(&self.global_search.containers);
                for c in containers {
                    self.push_grid_item_pending(GridItem::SearchContainer {
                        path: c.path,
                        kind: c.kind,
                        hit_count: c.hit_count,
                    });
                }
            }
            GlobalSearchView::DrilledInto { container, is_zip } => {
                // container 配下のヒットを Image or ZipImage として展開。
                // self.global_search.all_hits は unmutable borrow、self.push_grid_item_pending
                // は mutable なので、先に filter した path 群を Vec<String> として確定させる。
                let container_key = crate::search_index_db::normalize_path(&container);
                let hit_paths: Vec<String> = self
                    .global_search
                    .all_hits
                    .iter()
                    .filter_map(|h| {
                        if is_zip {
                            h.path
                                .split_once('!')
                                .filter(|(zip, _)| *zip == container_key)
                                .map(|_| h.path.clone())
                        } else if PathBuf::from(&h.path)
                            .parent()
                            .map(|p| {
                                crate::search_index_db::normalize_path(p) == container_key
                            })
                            .unwrap_or(false)
                        {
                            Some(h.path.clone())
                        } else {
                            None
                        }
                    })
                    .collect();

                for hit_path in hit_paths {
                    let grid_item = if is_zip {
                        // "<zippath>!<entry>" から entry を抽出
                        if let Some((_, entry)) = hit_path.split_once('!') {
                            GridItem::ZipImage {
                                zip_path: container.clone(),
                                entry_name: entry.to_string(),
                            }
                        } else {
                            continue;
                        }
                    } else {
                        GridItem::Image(PathBuf::from(&hit_path))
                    };
                    self.push_grid_item_pending(grid_item);
                }
            }
        }
        // items を差し替えたら visible_indices も再計算する。怠ると、前に表示していた
        // フォルダのインデックス値 (新 items 外) が残って、グリッド描画で items[idx] が
        // panic する (panic.log で ui_main.rs:1013 の out-of-bounds として観測)。
        self.rebuild_visible_indices();
        // scroll を先頭に戻す
        self.scroll_offset_y = 0.0;
    }

    /// drill-down view に切り替える (SearchContainer クリック時)。
    pub(crate) fn drill_into_container(&mut self, container: PathBuf, is_zip: bool) {
        self.global_search.view = GlobalSearchView::DrilledInto { container, is_zip };
        self.rebuild_items_from_global_search();
    }

    /// Aggregated view に戻る (drill-down 状態から)。
    pub(crate) fn drill_back_to_aggregated(&mut self) {
        self.global_search.view = GlobalSearchView::Aggregated;
        self.rebuild_items_from_global_search();
    }

    /// Ctrl+G トップパネルの描画 (既存 render_favsearch_bar と同パターン)。
    pub(crate) fn render_global_search_bar(&mut self, ctx: &egui::Context) {
        if !self.global_search.active {
            return;
        }
        let raw_enter = ctx.input(|i| i.key_pressed(egui::Key::Enter));
        let escape_pressed = self.dialog_escape_pressed(ctx);

        let mut close_requested = false;
        let mut query_changed = false;

        let mut drill_back = false;
        egui::TopBottomPanel::top("global_search_bar").show(ctx, |ui| {
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                // drill-down 中は「← 戻る」ボタン + 現在のコンテナ表示
                if let GlobalSearchView::DrilledInto { container, .. } =
                    self.global_search.view.clone()
                {
                    if ui
                        .button("←")
                        .on_hover_text("検索結果一覧に戻る (BS でも可)")
                        .clicked()
                    {
                        drill_back = true;
                    }
                    ui.label(
                        egui::RichText::new(format!("📁 {}", container.display()))
                            .size(11.0)
                            .color(egui::Color32::from_gray(150)),
                    );
                    ui.separator();
                }
                ui.label("🌐 全検索:");
                let response = ui.add_sized(
                    [360.0, 20.0],
                    egui::TextEdit::singleline(&mut self.global_search.query)
                        .hint_text(r#"お気に入り全体のメタデータ検索 (AND / -除外 / "…")"#),
                );
                if self.global_search.focus_request {
                    self.global_search.focus_request = false;
                    response.request_focus();
                }
                self.global_search.has_focus = response.has_focus();
                if response.changed() {
                    query_changed = true;
                }
                if response.lost_focus() && raw_enter {
                    query_changed = true;
                }
                if ui.small_button("×").on_hover_text("検索を閉じる").clicked() {
                    close_requested = true;
                }

                // 進捗/結果バッジ
                ui.separator();
                if let Some(msg) = &self.global_search.reject_message {
                    ui.label(
                        egui::RichText::new(msg)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(200, 120, 40)),
                    );
                } else if self.global_search.pending.is_some() && !self.global_search.done {
                    let progress = format!(
                        "検索中... {} 件 (候補 {})",
                        self.global_search.total_valid, self.global_search.total_scanned
                    );
                    ui.label(
                        egui::RichText::new(progress)
                            .size(11.0)
                            .color(egui::Color32::from_rgb(180, 180, 80)),
                    );
                } else if self.global_search.done {
                    let text = if self.global_search.truncated {
                        format!(
                            "{} 件で打ち切り (絞り込みキーワードを追加してください)",
                            self.global_search.total_valid
                        )
                    } else {
                        format!("{} 件", self.global_search.total_valid)
                    };
                    let color = if self.global_search.truncated {
                        egui::Color32::from_rgb(200, 140, 40)
                    } else {
                        egui::Color32::from_gray(140)
                    };
                    ui.label(egui::RichText::new(text).size(11.0).color(color));
                }
            });
            ui.add_space(2.0);
        });

        if !self.any_dialog_open() && escape_pressed {
            close_requested = true;
        }
        if close_requested {
            self.close_global_search();
            return;
        }
        if drill_back {
            self.drill_back_to_aggregated();
        }
        if query_changed {
            self.global_search.last_change_at = Some(Instant::now());
            // クエリが変わったら drill state もリセット (sorted_containers 側で処理)
            self.global_search.view = GlobalSearchView::Aggregated;
        }
    }
}

// -----------------------------------------------------------------------
// tests
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_container_parses_zip_entries() {
        let (p, k) = parent_container("c:/photos/album.zip!subdir/img.jpg");
        assert_eq!(p, PathBuf::from("c:/photos/album.zip"));
        assert_eq!(k, SearchContainerKind::Zip);
    }

    #[test]
    fn parent_container_parses_normal_files() {
        let (p, k) = parent_container("c:/photos/sunset/IMG.jpg");
        assert_eq!(p, PathBuf::from("c:/photos/sunset"));
        assert_eq!(k, SearchContainerKind::Folder);
    }

    #[test]
    fn accumulate_aggregates_by_container() {
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/b/1.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/b/2.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/c/1.jpg".into(),
            score: 1.0,
        });
        assert_eq!(state.containers.len(), 2);
        let b = state
            .containers
            .get(&PathBuf::from("c:/a/b"))
            .expect("c:/a/b bucket");
        assert_eq!(b.hit_count, 2);
    }

    #[test]
    fn sorted_containers_orders_by_hit_count_desc() {
        let mut map = HashMap::new();
        map.insert(
            PathBuf::from("c:/low"),
            ContainerHit {
                path: "c:/low".into(),
                kind: SearchContainerKind::Folder,
                hit_count: 2,
            },
        );
        map.insert(
            PathBuf::from("c:/high"),
            ContainerHit {
                path: "c:/high".into(),
                kind: SearchContainerKind::Folder,
                hit_count: 10,
            },
        );
        map.insert(
            PathBuf::from("c:/mid"),
            ContainerHit {
                path: "c:/mid".into(),
                kind: SearchContainerKind::Folder,
                hit_count: 5,
            },
        );
        let v = sorted_containers(&map);
        assert_eq!(v[0].path, PathBuf::from("c:/high"));
        assert_eq!(v[1].path, PathBuf::from("c:/mid"));
        assert_eq!(v[2].path, PathBuf::from("c:/low"));
    }

    #[test]
    fn drill_state_transitions_roundtrip() {
        let mut state = GlobalSearchState::default();
        assert_eq!(state.view, GlobalSearchView::Aggregated);
        state.view = GlobalSearchView::DrilledInto {
            container: PathBuf::from("c:/photos"),
            is_zip: false,
        };
        assert!(matches!(state.view, GlobalSearchView::DrilledInto { .. }));
        // クエリリセットで drill state もリセットされる契約
        state.reset_for_new_query();
        assert_eq!(state.view, GlobalSearchView::Aggregated);
    }

    #[test]
    fn reset_clears_all_transient_state() {
        // Codex round-9 Should-fix #2 回帰: reset_for_new_query で all_hits / view /
        // done / truncated / total_* が全部クリアされる (close 時に相当の処理を走らせる想定)
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/1.jpg".into(),
            score: 1.0,
        });
        state.view = GlobalSearchView::DrilledInto {
            container: PathBuf::from("c:/a"),
            is_zip: false,
        };
        state.done = true;
        state.truncated = true;
        state.total_valid = 42;
        state.total_scanned = 100;
        state.reject_message = Some("test".into());

        state.reset_for_new_query();

        assert!(state.all_hits.is_empty());
        assert!(state.containers.is_empty());
        assert_eq!(state.view, GlobalSearchView::Aggregated);
        assert!(!state.done);
        assert!(!state.truncated);
        assert_eq!(state.total_valid, 0);
        assert_eq!(state.total_scanned, 0);
        assert!(state.reject_message.is_none());
    }

    #[test]
    fn all_hits_preserved_for_drill_down() {
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/1.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/a/2.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/b/x.jpg".into(),
            score: 1.0,
        });
        // containers は 2 つに集約されているが、all_hits は 3 つ保持
        assert_eq!(state.containers.len(), 2);
        assert_eq!(state.all_hits.len(), 3);
    }

    #[test]
    fn accumulate_mixed_folders_and_zips() {
        let mut state = GlobalSearchState::default();
        state.accumulate_hit(&GlobalHit {
            path: "c:/album.zip!0001.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/album.zip!0002.jpg".into(),
            score: 1.0,
        });
        state.accumulate_hit(&GlobalHit {
            path: "c:/photos/x.jpg".into(),
            score: 1.0,
        });
        let zip = state.containers.get(&PathBuf::from("c:/album.zip")).unwrap();
        assert_eq!(zip.hit_count, 2);
        assert_eq!(zip.kind, SearchContainerKind::Zip);
        let folder = state.containers.get(&PathBuf::from("c:/photos")).unwrap();
        assert_eq!(folder.hit_count, 1);
        assert_eq!(folder.kind, SearchContainerKind::Folder);
    }
}
