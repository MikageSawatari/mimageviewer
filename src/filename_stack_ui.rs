//! ファイル名 prefix スタック (v2.0.0、`docs/filename-stack-plan.md`) の App 側グルー。
//!
//! 純グループ化ロジックは [`crate::filename_stack`]。ここはそれを `App` の状態
//! (`stack_view` / `stack_mode_requested` / `stack_showing_flat`) とビュー (`self.items`) に橋渡しする。
//!
//! ビューは 2 段 (メンバーグリッドは設けない。1 枚スタックの割合が高く中間グリッドが煩雑なため):
//! - **集約グリッド** (`stack_showing_flat=false`): 1 グループ = 1 セル。複数枚画像はスタックセル +
//!   バッジ、単独は通常 Image/Video セル。コンテナ (フォルダ/ZIP/PDF) は先頭に素通し表示。
//! - **フラット読書フルスクリーン** (`stack_showing_flat=true`): セルを開くと `self.items` を全画像
//!   展開 (materialize_flat) に差し替えてフルスクリーンへ。`↓↑` は境界を越えて順送り、`Shift+↓↑` で
//!   次/前のスタック先頭へジャンプ、`Ctrl+↓↑` はフォルダ移動 (据え置き)。閉じると
//!   `stack_reconcile_after_fullscreen_close` が集約グリッドへ戻す。
//!
//! 集約グリッドの構築は `load_folder_with_scan` の hook 経由 (start_loading_items が動画サムネ
//! スレッドを起動するため必ずフォルダ読込を通す)。集約⇔フラットの切替は in-memory
//! (`swap_stack_view_items` = install_new_items + 軽量ビュー切替後始末)。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};

use crate::filename_stack::{StackMember, StackView};
use crate::grid_item::GridItem;
use crate::settings::SortOrder;

/// ユーザー定義スクリプトによるグループ分けを **ワーカーで** 実行する際の保留状態
/// (`docs/filename-stack-scripting-plan.md`)。スクリプトは任意に重くなり得る (10 万件で
/// ~1 秒) ので UI スレッドで走らせず、通常フォルダを先に表示しつつ裏で計算し、完了後に
/// `poll_stack_script` が集約ビューへ差し替える。
pub(crate) struct StackScriptPending {
    /// ワーカー結果を捨てるためのキャンセルトークン (新ロード時に立てる)。
    pub cancel: Arc<AtomicBool>,
    /// ワーカーからの結果。`Ok((画像分のキー列, 採用ルール名))` または `Err(要旨)`。
    rx: Receiver<Result<(Vec<String>, Option<String>), String>>,
    /// このグループ分けが束縛されている実フォルダ (別フォルダへ移ったら破棄)。
    folder: PathBuf,
    /// 全メディア (画像 + 動画、表示順)。完了時に keys と合わせてグループ化する。
    media: Vec<StackMember>,
    /// 画像以外のコンテナ先頭 (集約ビューの passthrough)。
    passthrough: Vec<GridItem>,
    passthrough_metas: Vec<Option<(i64, i64)>>,
    separator: char,
    sort: SortOrder,
    /// `media` 内で画像 (= スクリプトへ渡した対象) の index。戻りキーを media 順へ戻すのに使う。
    image_indices: Vec<usize>,
    /// 変換前 (全 items) のサムネキャッシュキー。集約適用時 `start_loading_items` に渡して
    /// delete_missing が非代表メンバーのキャッシュを巻き添えで消さないようにする (同期パスと同じ)。
    existing_keys: std::collections::HashSet<String>,
    /// フォルダ signature (同期パスと同じく `start_loading_items` へ渡す)。
    folder_signature: Option<u64>,
    /// サブ展開ビュー向け: 親フォルダ単位でスクリプト分類し、別フォルダの同名 prefix を混ぜない。
    group_per_parent: bool,
}

/// 通常フォルダの items から、集約に必要な材料 (passthrough / passthrough_metas / media) を
/// **非破壊で** 取り出す (`build_stack_aggregated` の前半と同じだが items を消費しない。
/// ワーカー経路では items をそのまま通常表示に使うため)。
pub(crate) fn extract_stack_parts(
    items: &[GridItem],
    image_metas: &[Option<(i64, i64)>],
) -> (Vec<GridItem>, Vec<Option<(i64, i64)>>, Vec<StackMember>) {
    let mut passthrough: Vec<GridItem> = Vec::new();
    let mut passthrough_metas: Vec<Option<(i64, i64)>> = Vec::new();
    let mut media: Vec<StackMember> = Vec::new();
    for (it, meta) in items.iter().zip(image_metas) {
        let (mtime, size) = meta.unwrap_or((0, 0));
        match it {
            GridItem::Image(path) => media.push(StackMember {
                path: path.clone(),
                mtime,
                size,
                is_video: false,
            }),
            GridItem::Video(path) => media.push(StackMember {
                path: path.clone(),
                mtime,
                size,
                is_video: true,
            }),
            // 想定外種別は素通し (build_stack_aggregated と同じ防御)。
            other => {
                passthrough.push(other.clone());
                passthrough_metas.push(*meta);
            }
        }
    }
    (passthrough, passthrough_metas, media)
}

/// 集約 / メンバービューの items から動画セルの `(idx, path, size)` を集める
/// (`start_loading_items` の video サムネスレッド用、元の媒体ループと同形式)。
pub(crate) fn stack_video_items(
    items: &[GridItem],
    metas: &[Option<(i64, i64)>],
) -> Vec<(usize, PathBuf, u64)> {
    items
        .iter()
        .enumerate()
        .filter_map(|(idx, it)| {
            if let GridItem::Video(p) = it {
                let size = metas
                    .get(idx)
                    .and_then(|m| *m)
                    .map(|(_, s)| s.max(0) as u64)
                    .unwrap_or(0);
                Some((idx, p.clone(), size))
            } else {
                None
            }
        })
        .collect()
}

fn stack_script_keys_for_images(
    images: &[StackMember],
    source: &str,
    cancel: Arc<AtomicBool>,
    group_per_parent: bool,
) -> Result<(Vec<String>, Option<String>), String> {
    if !group_per_parent {
        return crate::filename_stack_script::group_keys_cancellable(images, source, cancel)
            .map(|r| (r.keys, r.rule));
    }

    let script =
        crate::filename_stack_script::CompiledStackScript::compile(source, Arc::clone(&cancel))?;
    let mut order: Vec<String> = Vec::new();
    let mut buckets: HashMap<String, Vec<usize>> = HashMap::new();
    for (idx, image) in images.iter().enumerate() {
        let parent_key = image
            .path
            .parent()
            .map(crate::adjustment_db::normalize_path)
            .unwrap_or_default();
        if !buckets.contains_key(&parent_key) {
            order.push(parent_key.clone());
        }
        buckets.entry(parent_key).or_default().push(idx);
    }

    let mut keys = vec![String::new(); images.len()];
    let mut rule: Option<String> = None;
    let mut mixed_rule = false;
    for parent_key in order {
        if cancel.load(Ordering::Relaxed) {
            return Err("キャンセルされました".to_string());
        }
        let Some(indices) = buckets.get(&parent_key) else {
            continue;
        };
        let scoped_images: Vec<StackMember> =
            indices.iter().map(|&idx| images[idx].clone()).collect();
        let result = script.group_keys(&scoped_images)?;
        match result.rule {
            Some(next) => match rule.as_ref() {
                None => rule = Some(next),
                Some(current) if current == &next => {}
                Some(_) => mixed_rule = true,
            },
            None => {}
        }
        for (&image_idx, key) in indices.iter().zip(result.keys.into_iter()) {
            if let Some(slot) = keys.get_mut(image_idx) {
                *slot = crate::filename_stack::parent_scoped_key_for_path(
                    &images[image_idx].path,
                    &key,
                );
            }
        }
    }

    let rule = if mixed_rule {
        Some("親フォルダ別".to_string())
    } else {
        rule
    };
    Ok((keys, rule))
}

impl crate::app::App {
    /// 進行中のスタックスクリプトワーカーをキャンセルして破棄する。
    pub(crate) fn cancel_stack_script_pending(&mut self) {
        if let Some(p) = self.stack_script_pending.take() {
            p.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// ユーザー定義スクリプトによるグループ分けをワーカーで開始する。通常フォルダは既に
    /// 表示済みで、完了後 `poll_stack_script` が集約ビューへ差し替える。
    ///
    /// `script_enabled` = 「分類ルールをスクリプトで行う」設定。true ならユーザーの
    /// `stack_rules.rhai` (無ければ内蔵既定)、false なら内蔵 `DEFAULT_SCRIPT` を使う。
    /// **ソースの読み込み (ファイル I/O) はワーカースレッド内で行う** (= UI スレッドに
    /// `read_to_string` を乗せない)。
    pub(crate) fn spawn_stack_script_worker(
        &mut self,
        folder: PathBuf,
        passthrough: Vec<GridItem>,
        passthrough_metas: Vec<Option<(i64, i64)>>,
        media: Vec<StackMember>,
        separator: char,
        sort: SortOrder,
        existing_keys: std::collections::HashSet<String>,
        folder_signature: Option<u64>,
        script_enabled: bool,
        group_per_parent: bool,
    ) {
        // 動画はルール判定に渡さない (常に単独)。画像だけをスクリプトへ。
        let image_indices: Vec<usize> = media
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.is_video)
            .map(|(i, _)| i)
            .collect();
        let images: Vec<StackMember> = image_indices.iter().map(|&i| media[i].clone()).collect();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_w = Arc::clone(&cancel);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("stack-script".into())
            .spawn(move || {
                // スクリプト本文の読み込みもワーカー側で行う (UI スレッドの同期 I/O 回避)。
                let source = if script_enabled {
                    crate::filename_stack_script::active_script_source()
                } else {
                    crate::filename_stack_script::DEFAULT_SCRIPT.to_string()
                };
                let result = stack_script_keys_for_images(
                    &images,
                    &source,
                    Arc::clone(&cancel_w),
                    group_per_parent,
                );
                if cancel_w.load(Ordering::Relaxed) {
                    return;
                }
                let _ = tx.send(result);
            })
            .ok();
        self.stack_script_pending = Some(StackScriptPending {
            cancel,
            rx,
            folder,
            media,
            passthrough,
            passthrough_metas,
            separator,
            sort,
            image_indices,
            existing_keys,
            folder_signature,
            group_per_parent,
        });
    }

    /// スタックスクリプトワーカーの結果を取り込む (毎フレーム)。完了したら集約ビューへ
    /// 差し替える。フォルダ移動 / スタック OFF で無効化された pending は破棄する。
    pub(crate) fn poll_stack_script(&mut self, ctx: &egui::Context) {
        // 妥当性: フォルダが変わった / スタック OFF / 既に集約済み なら破棄。
        let still_valid = self.stack_script_pending.as_ref().is_some_and(|p| {
            self.stack_mode_requested
                && self
                    .current_folder
                    .as_ref()
                    .is_some_and(|c| crate::folder_tree::path_eq(c, &p.folder))
                && self.stack_view.is_none()
                && !self.stack_showing_flat
        });
        if self.stack_script_pending.is_some() && !still_valid {
            self.cancel_stack_script_pending();
            return;
        }
        if self.stack_script_pending.is_none() {
            return;
        }
        // 計算中は再描画を促す (通常フォルダのサムネ完了後でも poll を回すため)。
        ctx.request_repaint();
        // メイン窓内フルスクリーン表示中は items 差し替えを避ける (閉じてから適用。
        // 結果は channel で待つ)。detached セッション (別ウィンドウ) はここでは
        // 弾かない: 結果が届いた時点で退避 (下) してから適用する。旧ゲートの
        // 「fullscreen_idx.is_some() で一律保留」は、別ウィンドウを開いている間
        // 集約が永久に適用されない実害があった (2026-07-10 監査)。
        if self.fullscreen_idx.is_some() && !self.viewer_session_is_detached() {
            return;
        }
        let received = match self.stack_script_pending.as_ref().unwrap().rx.try_recv() {
            Ok(r) => r,
            Err(TryRecvError::Empty) => return,
            // ワーカー起動失敗等で tx が落ちた → 組み込み既定でフォールバック集約する
            // (通常フォルダ表示のまま放置せず、スタックを成立させる)。適用前の
            // detached 退避も Ok 経路と共有する (Codex P3: fallback だけ素通りさせない)。
            Err(TryRecvError::Disconnected) => {
                Err("スタックスクリプトのワーカーを起動できませんでした".to_string())
            }
        };
        // 結果が届いた今、main items を差し替える前に detached セッションを退避する
        // (メディア窓は in-place live-park で再生継続 + main 文脈は clone 維持、
        //  画像 still はフォルダ移動時と同じ preserve → close で parked 化)。
        // 計算中に窓を畳まないよう、退避は受信後にだけ行う。
        #[cfg(windows)]
        if self.fullscreen_idx.is_some() {
            self.park_detached_session_for_stack_aggregation(ctx);
        }
        if self.fullscreen_idx.is_some() {
            // 退避に失敗した稀ケース (park snapshot 失敗等)。dangling fs_idx の上に
            // items を差し替えないよう、ハードに閉じてから適用する。
            self.close_fullscreen();
        }
        let pending = self.stack_script_pending.take().unwrap();
        self.apply_stack_script_result(pending, received);
    }

    /// ワーカー結果から集約ビューを組み立てて差し替える。
    fn apply_stack_script_result(
        &mut self,
        pending: StackScriptPending,
        result: Result<(Vec<String>, Option<String>), String>,
    ) {
        let StackScriptPending {
            media,
            passthrough,
            passthrough_metas,
            separator,
            sort,
            image_indices,
            folder,
            existing_keys,
            folder_signature,
            group_per_parent,
            ..
        } = pending;
        let (groups, rule, err) = match result {
            Ok((keys, rule)) => {
                // 画像分のキーを media 順へ戻す (動画位置は空キー = group_by_keys が一意化)。
                let mut full = vec![String::new(); media.len()];
                for (k, &idx) in keys.into_iter().zip(image_indices.iter()) {
                    if let Some(slot) = full.get_mut(idx) {
                        *slot = k;
                    }
                }
                (
                    crate::filename_stack::group_by_keys(media, &full, sort),
                    rule,
                    None,
                )
            }
            Err(e) => {
                crate::logger::log(format!(
                    "stack script failed (async), fallback to builtin: {e}"
                ));
                (
                    if group_per_parent {
                        crate::filename_stack::group_media_by_parent(media, separator, sort)
                    } else {
                        crate::filename_stack::group_media(media, separator, sort)
                    },
                    None,
                    Some(e),
                )
            }
        };
        let sv = StackView::from_groups_with_display_order(
            folder.clone(),
            passthrough,
            passthrough_metas,
            separator,
            sort,
            groups,
            self.settings.grid_display_order.clone(),
        );
        let collapsible = sv.has_collapsible_stack();
        let (agg_items, agg_metas) = sv.materialize_aggregated();
        let agg_videos = stack_video_items(&agg_items, &agg_metas);
        // 集約ビューを通常ロードと同じ経路で適用する (= 同期パスの末尾と同形)。これにより
        // 動画サムネスレッドの起動 / キャッシュ保護 (existing_keys) / 世代更新が正しく行われる
        // (swap_stack_view_items は in-memory swap で動画スレッドを再起動しないため不可。Codex P2)。
        // start_loading_items が stack_* をリセットするので、その後に意図を復元する。
        let is_subfolder_expansion_stack =
            crate::folder_tree::path_eq(&folder, &crate::app::subfolder_expansion_synthetic_path());
        self.start_loading_items(
            folder,
            agg_items,
            agg_metas,
            existing_keys,
            agg_videos,
            folder_signature,
        );
        if is_subfolder_expansion_stack {
            self.restore_subfolder_expansion_view_state_after_items_install();
        }
        self.stack_mode_requested = true;
        self.stack_view = Some(sv);
        self.stack_active_rule = rule.clone();
        self.stack_script_error = err.clone();
        // トグル ON 時のカーソル画像を含むスタックセルへカーソルを移す (被写体を保つ)。
        if let Some(target) = self.stack_toggle_select_path.take()
            && let Some(idx) = self
                .stack_view
                .as_ref()
                .and_then(|sv| sv.aggregated_index_for_member_path(&target))
        {
            self.selected = Some(idx);
            self.scroll_to_selected = true;
        }
        if err.is_some() {
            self.show_feedback_toast(
                "スタックのスクリプトでエラー。既定ルールで表示します (詳細はヘルプ参照)".into(),
            );
        } else if !collapsible {
            self.show_feedback_toast(
                "まとめられるスタックがありませんでした (分類ルールはヘルプ参照)".into(),
            );
        } else if let Some(rule) = rule {
            self.show_feedback_toast(format!("スタック: 「{rule}」でまとめました"));
        }
    }

    /// スタックモードのトグルが使える状況か。
    /// 通常フォルダ、またはサブ展開スナップショット表示で有効。ZIP ツリー / PDF ページ一覧 /
    /// 検索 / タグ / 読書履歴 / ドライブ一覧などの特殊・仮想ビューでは無効。
    pub(crate) fn stack_mode_available(&self) -> bool {
        let regular_folder = self.current_folder_last_mtime.is_some();
        let subfolder_expansion = self.items_are_subfolder_expansion_view
            && self.subfolder_expansion_snapshot.is_some()
            && self.subfolder_expansion_pending.is_none()
            && self.subfolder_expansion_install_pending.is_none();
        self.current_folder.is_some()
            && (regular_folder || subfolder_expansion)
            && self.zip_nav.is_none()
            && !self.items_are_global_search_view
            && !self.items_are_tag_view
            && !self.items_are_reading_history_view
            && !self.items_are_drive_list
            && !self.global_search.active
            && !self.favsearch.active
            && !self.tag_view.active
            // Ctrl+F (現在地フィルタ) 中はトグル不可: トグルは load_folder 経由なので
            // search_filter / search_query が消えてしまうため。
            && !self.show_search_bar
            && self.search_filter.is_none()
            && self.search_pending.is_none()
    }

    /// スタックモードが ON か。トグルボタンの選択状態表示に使う。
    pub(crate) fn stack_mode_on(&self) -> bool {
        self.stack_mode_requested
    }

    /// 集約グリッドを表示中か (= スタックモード ON かつフラット読書フルスクリーン中でない)。
    /// グリッドのセルクリック → フラットフルスクリーンへ入れる状態かの判定に使う。
    pub(crate) fn stack_mode_aggregated(&self) -> bool {
        self.stack_view.is_some() && !self.stack_showing_flat
    }

    /// 現在カーソル位置 (selected) のセルの代表画像/動画パス。スタックトグルで被写体を保つため。
    /// 通常セル (Image/Video) はそのパス、スタックセルは代表画像、コンテナは `None`。
    fn current_selected_representative_path(&self) -> Option<std::path::PathBuf> {
        let idx = self.selected?;
        match self.items.get(idx)? {
            GridItem::Image(p) | GridItem::Video(p) => Some(p.clone()),
            GridItem::Stack { representative, .. } => Some(representative.clone()),
            _ => None,
        }
    }

    /// スタックモードを切り替える。同一フォルダを再読込して集約 / 通常を作り直す
    /// (folder_changes=false なので `stack_mode_requested` は維持される)。
    pub(crate) fn toggle_stack_mode(&mut self) {
        if !self.stack_mode_available() {
            self.show_feedback_toast("スタック表示は通常フォルダまたはサブ展開で使えます".into());
            return;
        }
        if self.items_are_subfolder_expansion_view {
            self.toggle_subfolder_stack_mode();
            return;
        }
        let Some(folder) = self.current_folder.clone() else {
            return;
        };
        // トグル前のカーソル画像 (代表パス) を捕まえ、トグル後も同じ被写体に留まるようにする。
        let target = self.current_selected_representative_path();
        // 通常フォルダでの選択は名前ベースの select_after_load で復元する (ON 時の計算中の
        // 通常表示 / OFF 時の最終表示の両方で効く)。ON 時の最終的な集約ビューでは、その画像を
        // 含むスタックセルへ apply_stack_script_result が選択し直す。
        if let Some(name) = target
            .as_ref()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
        {
            self.select_after_load = Some(name.to_string());
        }
        self.stack_mode_requested = !self.stack_mode_requested;
        self.stack_toggle_select_path = if self.stack_mode_requested {
            target
        } else {
            None
        };
        self.load_folder(folder);
        // スクリプトをワーカーで計算中 (async) のときは、ここではトーストしない。完了時に
        // poll_stack_script が採用ルール / 失敗 / 非該当のトーストを出す。
        if self.stack_mode_requested && self.stack_script_pending.is_none() {
            // スクリプトが失敗して既定ルールへフォールバックした場合は最優先で知らせる。
            if self.stack_script_error.is_some() {
                self.show_feedback_toast(
                    "スタックのスクリプトでエラー。既定ルールで表示します (詳細はヘルプ参照)"
                        .into(),
                );
            } else if self
                .stack_view
                .as_ref()
                .is_some_and(|sv| !sv.has_collapsible_stack())
            {
                self.show_feedback_toast(
                    "まとめられるスタックがありませんでした (分類ルールはヘルプ参照)".into(),
                );
            } else if let Some(rule) = self.stack_active_rule.clone() {
                // 採用された分類ルール名をトーストで知らせる (どのルールが当たったか可視化)。
                self.show_feedback_toast(format!("スタック: 「{rule}」でまとめました"));
            }
        }
    }

    /// サブ展開ビュー上でスタック表示を切り替える。
    ///
    /// ON: 現在のサブ展開一覧を親フォルダ単位で分類し、同じ合成ビューに集約セルを表示する。
    /// OFF: 保持済みスナップショットを再インストールして、サブ展開のフラット一覧へ戻す。
    fn toggle_subfolder_stack_mode(&mut self) {
        let target = self.current_selected_representative_path();
        self.cancel_stack_script_pending();
        self.stack_active_rule = None;
        self.stack_script_error = None;

        if self.stack_mode_requested {
            self.stack_mode_requested = false;
            self.stack_toggle_select_path = None;
            self.stack_view = None;
            self.stack_showing_flat = false;
            if self.reinstall_subfolder_expansion_snapshot() {
                if let Some(target) = target
                    && let Some(idx) = self.items.iter().position(|item| match item {
                        GridItem::Image(path) | GridItem::Video(path) => {
                            crate::folder_tree::path_eq(path, &target)
                        }
                        _ => false,
                    })
                {
                    self.selected = Some(idx);
                    self.scroll_to_selected = true;
                }
            } else if let Some(root) = self.subfolder_expansion_root.clone() {
                let roots = if self.subfolder_expansion_roots.is_empty() {
                    vec![root.clone()]
                } else {
                    self.subfolder_expansion_roots.clone()
                };
                self.start_subfolder_expansion_scan_roots(root, roots);
            }
            return;
        }

        if self.subfolder_expansion_snapshot.is_none() {
            self.show_feedback_toast("サブ展開のスナップショットがありません".into());
            return;
        }

        let (passthrough, passthrough_metas, media) =
            extract_stack_parts(&self.items, &self.image_metas);
        self.stack_mode_requested = true;
        self.stack_toggle_select_path = target;
        self.stack_showing_flat = false;
        self.stack_view = None;
        let sort_path = self
            .subfolder_expansion_root
            .as_deref()
            .or(self.current_folder.as_deref());
        let sort = sort_path
            .map(|path| self.book_sort_order_for_path(path))
            .unwrap_or(self.settings.sort_order);
        self.spawn_stack_script_worker(
            crate::app::subfolder_expansion_synthetic_path(),
            passthrough,
            passthrough_metas,
            media,
            self.settings.stack_separator,
            sort,
            std::collections::HashSet::new(),
            None,
            self.settings.stack_script_enabled,
            true,
        );
    }

    /// 集約グリッドでメディアセル (スタック / 単独画像 / 動画) を開いたとき、フラット読書
    /// フルスクリーンへ入る。`agg_idx` は集約 `self.items` の index。コンテナ (passthrough) は
    /// `false` を返して通常ナビ (フォルダ/ZIP/PDF を開く) に委ねる。戻り値 true = ここで処理した。
    ///
    /// `from_double_click` = ダブルクリック経由か (動画の場合に 2 打目の play/pause トグルを
    /// 抑制するため)。通常の grid→fullscreen 経路と同じ開幕ガードをここで張る。
    pub(crate) fn stack_try_open_from_grid(
        &mut self,
        ctx: &egui::Context,
        agg_idx: usize,
        from_double_click: bool,
    ) -> bool {
        if !self.stack_mode_aggregated() {
            return false;
        }
        let flat_idx = self
            .stack_view
            .as_ref()
            .and_then(|sv| sv.flat_index_for_aggregated(agg_idx));
        let Some(flat_idx) = flat_idx else {
            // passthrough コンテナ → フルスクリーンでなく通常ナビへ。
            return false;
        };
        #[cfg(not(windows))]
        let _ = ctx;
        // items をフラット配列へ差し替える前に、集約セルの path で same-media 前面化を
        // 判定する。別 media/still のときだけ現 active context を先に park する。
        // (review-v2.3.0 追補4: stack flat grid open)
        #[cfg(windows)]
        if !self.prepare_detached_context_for_grid_open(ctx, agg_idx) {
            return true;
        }
        // 開幕ガード (通常の grid open 経路と同じ):
        // - Enter で開いた同フレームに fullscreen 側が同じ Enter を拾って即 close するのを防ぐ
        //   (Enter が押下されていなければ fullscreen 側初フレームで自動リセットされるので、
        //    click/gamepad 経由で立てても無害)。
        self.fs_suppress_enter_close_until_release = true;
        // - ダブルクリックで動画を開いたとき、2 打目クリックが fullscreen の動画 play/pause を
        //   トグルしないよう抑制する (静止画は open_fullscreen の focus-regain グレースで足りる)。
        if from_double_click && matches!(self.items.get(agg_idx), Some(GridItem::Video(_))) {
            self.fs_suppress_primary_until_release = true;
        }
        self.stack_enter_flat_fullscreen(flat_idx);
        true
    }

    /// フラット読書ビュー (全画像を展開した並び) へ `self.items` を差し替え、`flat_idx` を
    /// フルスクリーンで開く。
    ///
    /// in-memory な items 差し替えなので、`zip_nav_show_current_level` と同じ軽量ビュー切替の
    /// 後始末 (idx 状態 + キュー破棄 / visible_indices 再構築 / ページ編集状態の再 hydrate /
    /// rating・tag prewarm) を必ず行う。これを怠ると旧 (集約) ビューの stale な
    /// `visible_indices` が範囲外参照 panic を起こす (Codex P1)。
    fn stack_enter_flat_fullscreen(&mut self, flat_idx: usize) {
        let (items, metas) = match self.stack_view.as_ref() {
            Some(sv) => sv.materialize_flat(),
            None => return,
        };
        let Some(folder) = self.current_folder.clone() else {
            return;
        };
        self.swap_stack_view_items(items, metas, &folder, Some(flat_idx));
        self.stack_showing_flat = true;
        self.fs_open_intent_from_grid = true;
        self.open_fullscreen(flat_idx);
    }

    /// フルスクリーン中の `Shift+↓↑`: 次/前のスタックの先頭画像へジャンプする。
    /// フラット読書ビューでないときは `false` (= 呼び出し側が通常のページ送りに委ねる)。
    /// 端では stack ジャンプ可能位置が無いので `true` (消費) のまま no-op にする。
    pub(crate) fn stack_jump(&mut self, ctx: &egui::Context, forward: bool) -> bool {
        if !self.stack_showing_flat {
            return false;
        }
        let Some(cur) = self.fullscreen_idx else {
            return false;
        };
        let target = self
            .stack_view
            .as_ref()
            .and_then(|sv| sv.stack_jump_target(cur, forward));
        if let Some(t) = target {
            self.open_fullscreen_from_fs_navigation(ctx, t);
        }
        true
    }

    /// フラットフルスクリーンが閉じたら集約グリッドへ戻す (毎フレーム reconcile、
    /// `render_grid` の直前で呼ぶ)。スタックモードが解除済み (フォルダナビ等) なら何もしない。
    pub(crate) fn stack_reconcile_after_fullscreen_close(&mut self) {
        if !self.stack_showing_flat || self.fullscreen_idx.is_some() {
            return;
        }
        // フルスクリーンが閉じた → フラグを落とす。
        self.stack_showing_flat = false;
        // 集約を再構築するための材料を取り出す (借用は install 前に閉じる)。
        let Some((items, metas, folder, select_agg)) = ({
            let Some(sv) = self.stack_view.as_ref() else {
                // フォルダナビ等で stack_view が破棄済み → 通常フォルダが表示されている。何もしない。
                return;
            };
            // close_fullscreen が selected に復元した「最後に見ていた flat index」を集約セルへ写す。
            let select_agg = self
                .selected
                .and_then(|flat| sv.group_of_flat_index(flat))
                .map(|g| sv.aggregated_index_of_group(g));
            let (items, metas) = sv.materialize_aggregated();
            Some((items, metas, sv.folder.clone(), select_agg))
        }) else {
            return;
        };
        self.swap_stack_view_items(items, metas, &folder, select_agg);
    }

    /// 集約/フラット間の in-memory ビュー切替の共通後始末。`select` を選択し scroll する。
    fn swap_stack_view_items(
        &mut self,
        items: Vec<GridItem>,
        metas: Vec<Option<(i64, i64)>>,
        folder: &std::path::Path,
        select: Option<usize>,
    ) {
        // 旧ビューの in-flight 検索 / 詳細メタ pending を停止 (idx が付け替わる)。
        if let Some(pending) = self.search_pending.take() {
            pending.cancel();
        }
        if let Some(pending) = self.metadata_pending.take() {
            pending.cancel();
        }
        self.install_new_items(items, metas);
        if crate::folder_tree::path_eq(folder, &crate::app::subfolder_expansion_synthetic_path()) {
            self.restore_subfolder_expansion_view_state_after_items_install();
        }
        self.invalidate_idx_state_and_queues();
        self.current_folder_rating_cache = None;
        // セルは実 Image (フラット) / 単独 Image (集約)。実フォルダ prefix でページ編集状態
        // (補正 / crop / view-trim / 消しゴム / ローカル調整 / 隠蔽) を再 hydrate する
        // (page_path_key が実パスキーを返すので folder prefix で正しく載る)。
        self.rehydrate_page_edit_state_for_current_items(folder);
        self.local_adjust_generation.clear();
        self.local_adjust_cache.clear();
        self.metadata_cache.clear();
        self.exif_cache.clear();
        self.xmp_cache.clear();
        self.tags_cache.clear();
        self.search_filter = None;
        self.search_query.clear();
        self.selected = select;
        self.scroll_offset_y = 0.0;
        self.scroll_to_selected = select.is_some();
        self.scroll_hint
            .store(0, std::sync::atomic::Ordering::Relaxed);
        self.prewarm_rating_cache();
        self.mark_color_filter_scope_dirty();
        // ★ visible_indices 再構築。stale index による範囲外参照 panic を防ぐ (Codex P1)。
        self.rebuild_visible_indices();
        self.prewarm_grid_tags();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(path: &str) -> StackMember {
        StackMember {
            path: PathBuf::from(path),
            mtime: 0,
            size: 0,
            is_video: false,
        }
    }

    #[test]
    fn script_grouping_scopes_keys_by_parent_for_subfolder_stack() {
        let images = vec![
            image(r"C:\root\a\scan_001.jpg"),
            image(r"C:\root\a\scan_002.jpg"),
            image(r"C:\root\a\other_001.jpg"),
            image(r"C:\root\a\other_002.jpg"),
            image(r"C:\root\b\scan_001.jpg"),
            image(r"C:\root\b\scan_002.jpg"),
            image(r"C:\root\b\other_001.jpg"),
            image(r"C:\root\b\other_002.jpg"),
        ];
        let (keys, _rule) = stack_script_keys_for_images(
            &images,
            crate::filename_stack_script::DEFAULT_SCRIPT,
            Arc::new(AtomicBool::new(false)),
            true,
        )
        .expect("default script groups per parent");
        assert_eq!(keys.len(), images.len());
        assert_eq!(keys[0], keys[1]);
        assert_eq!(keys[2], keys[3]);
        assert_eq!(keys[4], keys[5]);
        assert_eq!(keys[6], keys[7]);
        assert_ne!(keys[0], keys[2]);
        assert_ne!(keys[0], keys[4]);
        assert_ne!(keys[2], keys[6]);
    }
}
