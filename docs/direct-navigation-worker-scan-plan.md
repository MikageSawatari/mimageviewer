# 直接移動のフォルダ走査を worker へ移す (§1.805)

状態: 設計 (2026-11-18)。担当: 設計・検収 = ClaudeCode Opus 5.5、実装 = Codex GPT-6.1 Sol / high、
独立レビュー = GPT-6.1 Sol / xhigh。

## 1. 問題

ツールバーのお気に入り・メニューのお気に入り・アドレス欄の Enter・アドレス欄の履歴 (最近開いたフォルダ)
から実フォルダへ移ると、`App::open_direct_navigation_target_classified` (`src/app.rs`) が
`pre_scan = None` のまま `load_folder_or_convert_archive_with_auto_fullscreen_classified_with_navigation_at_stage`
へ進み、最後に `load_folder_with_scan_claimed_with_search_origin` の
`None => scan_directory_with_settings(&path, &self.settings)` が **UI スレッドで** `read_dir` と per-entry
metadata を読む。利用者の perf ログでは `nav/lf_scan` (`pre_scanned=false`) が最大約 590 ms あり、
[ui-responsiveness.md](ui-responsiveness.md) §1・§4 の「UI スレッドで同期 I/O をしない」に反する。
フォルダツリー (`start_user_folder_pane_open` → `start_folder_open_scan_with_restore`) と DFS (`spawn_folder_nav`)
は既に worker で走査し、`pre_scan` 付きで同じ読込へ戻している。

観測の出どころ: 590 ms は利用者の perf ログ (2026-11-18 の報告調査、起点セッション)。本件の担当は実行時の挙動を
観測していない。

## 2. 対象の producer と consumer

### 2.1 consumer (1 か所)

`open_direct_navigation_target_at_stage` → (`path_needs_open_classification` でなければ)
`open_direct_navigation_target_classified`。`pre_scan` が `Some` なら
`load_folder_with_scan_owned_outcome_and_effects_classified_with_navigation_at_stage`、`None` なら上記の同期経路。

### 2.2 `pre_scan = None` でこの consumer に入る producer

| 入口 | 経路 | owner / restore_intent |
| --- | --- | --- |
| ツールバーのお気に入り・ピン留め本・本棚 | `toolbar_fav_nav` → `dispatch_favorite_navigation` → `Some(path)` | Navigation / ExplicitList |
| メニューバーのお気に入り | `fav_nav` (`LocationJumpTarget::Physical`) → 同上 | 同上 |
| キー・リング・ゲームパッドのお気に入り | `AddressBarNav::Favorite` (`input_nav`) → 同上 | 同上 |
| フォルダを開くダイアログ | `open_folder_nav` → 同上 | 同上 |
| アドレス欄 Enter (検索外) | `AddressBarNav::AddressInput` → `dispatch_plain_location_jump` → 直接 `open_direct_navigation_target_at_stage` | 同上 |
| アドレス欄の履歴メニュー (最近開いたフォルダ) | `AddressBarNav::Direct(path, ExplicitList)` (`ui_main.rs` の address_input) | 同上 |
| 親へ (BS・`..`・閉じて親へ・スマート/サブフォルダ展開の親) | `AddressBarNav::Direct(parent, …)` (`grid_parent.rs`・`close_return.rs`・`smart_folder.rs`・`subfolder_expansion.rs`・`app.rs` 24605 付近) | Navigation / 各 intent |
| コレクションの物理元 | `AddressBarNav::CollectionSource` | CollectionGridPhysical / 各 intent |
| コンテキストメニューの書庫ジャンプ | `app.rs` 60616 付近 (書庫なので対象外) | Navigation / ExplicitList |

検索中 (`global_search` / `favsearch` / `tag_view`) のお気に入り・アドレス欄は既に
`begin_location_jump_to_physical` → `FolderOpenScanPurpose::JumpToPhysicalFolder` で worker 走査している。
履歴の戻る/進む (`dispatch_main_folder_history_input`) は `PhysicalHistoryTransition` の preflight worker で
走査済み。分類が必要な名前 (`.epub`・変換書庫の拡張子) は分類 worker が `folder_scan` を渡す。
いずれも本件の対象外 (変更しない)。

### 2.3 この consumer を通らない同期走査 (範囲外として残す)

- `dispatch_main_input_nav_immediate` (全画面を閉じる即時経路) の `Direct` / `CollectionSource` は
  `load_folder_or_convert_archive_with_auto_fullscreen_owned` を直接呼ぶ。
- `pub fn load_folder` / `load_folder_with_scan(path, None)` の各呼び出し元 (本棚マネージャ等)。
- detached context (`navigation_scope.is_detached_physical()`) での直接移動。detached viewer は
  リワーク中で凍結されているため触らない。
これらは本件の後に別項目として扱う (§7)。

## 3. 案の比較 (CLAUDE.md「設計の簡素化」)

| 案 | 判断 |
| --- | --- |
| A. ツリーと同じ `FolderPaneOpenPending` で走査し、完了を通常 nav 裁定へ戻して `pre_scan` 付きで consumer に再入する | **採用**。走査開始・置き換え・取消・stale 判定・完了の裁定・`pre_scan` 付きの読込は、ツリーのクリックが毎回通る既存経路そのもの。新しく足すのは「直接移動が要求を持ち込む入口」と、その要求が持つ owner / restore_intent / navigation だけ。採用時の読込は今の `pre_scan = Some` 分岐 (ツリー完了・DFS と同じ) なので、install の挙動差を作らない |
| B. ZIP の直接移動・履歴の戻る/進むと同じ `PhysicalHistoryTransition` (`Preflighting` → `Folder(ScannedDir)`) へ入れる | **不採用**。受付時の claim を残せる利点はあるが、採用が `commit_physical_history_transition` / `prepare_physical_listing` の別の install 経路になり、今の直接移動のフォルダ読込 (検索 origin、smart 対象、book append、rating 行き先など) と挙動を照合し直す範囲が広い。分岐を入れる `load_folder_or_convert_…_at_stage` は他に 4 か所から呼ばれ、型付きの経路指定を足す改修も要る。[file-type-visibility-plan.md](file-type-visibility-plan.md) §9.2 の表は親の読込をこの経路へ寄せる方針を書いているが、現状は未実施で、本件では広げない |
| C. `load_folder_with_scan_claimed_with_search_origin` の同期走査を全呼び出し元で worker 化する | **不採用 (今回)**。`load_folder` 等の多数の同期呼び出し元が読込直後の状態に依存している可能性があり、範囲が大きい。§7 の後続候補 |
| D. 走査中はモーダルにする | **不採用**。ツリーと同様、走査中の別移動は既存の置き換え・取消で扱えており、操作を止める理由がない |

待ち時間中の見え方: 走査が終わるまで今の一覧を表示し続け、完了したフレームで新しい一覧に替わる。
従来は UI が走査時間だけ止まってから替わっていた。ツリーのクリック・検索中のお気に入り・履歴の戻る/進むと
同じ見え方であり、通常の使い方を遅くする変更ではない。

## 4. 設計

### 4.1 新しい用途

```rust
pub(crate) enum FolderOpenScanPurpose {
    …
    /// 直接移動 (お気に入り・アドレス欄・最近開いたフォルダ・親へ・コレクションの物理元)
    /// の実フォルダ。受付は dispatch 時に済ませ、完了したら pre_scan 付きで
    /// open_direct_navigation_target_at_stage へ戻る。
    DirectNavigation {
        restore_intent: StartupListIntent,
        collection_owner: Option<top_level_grid_view::CollectionGridPhysicalLoadOwner>,
    },
}
```

- `owner` は consumer に入る `Navigation` / `CollectionGridPhysical` の 2 種だけなので、
  `ResolvedMainFolderOpen` と同じ `collection_owner: Option<…>` で持つ。それ以外の owner は worker 経路に入れない
  (§4.2 の条件)。
- `detached_loading_lease()` / `diagnostic_trace()` / `take_diagnostic_trace()` / 失敗ログの purpose 名
  (`"direct-navigation"`) の各 match に腕を足す。detached lease・診断 trace は持たない。

### 4.2 受付 (consumer 側)

`open_direct_navigation_target_classified` の `None` 腕の前で、次をすべて満たすときだけ worker 経路にする。
判定は 1 つの helper (`direct_navigation_scans_on_worker(&self, path, owner, classified_kind) -> bool`) に置き、
条件は同期経路が `scan_directory_with_settings` に到達する場合を拡張子と型だけで (I/O なしに) 表す:

1. main context (`!self.navigation_scope.is_detached_physical()`)。
2. `classified_kind` が `None` または `Some(OpenPathKind::Directory)`。
3. owner が `Navigation` または `CollectionGridPhysical`。
4. `is_synthetic_view_path(path)` でない (スマートフォルダ root 等は従来の分岐が扱う)。
5. `crate::folder_tree::is_virtual_folder(path)` でない。ただし `classified_kind == Some(Directory)` または
   `visible_grid_item_is_folder(path)` のときは実フォルダとして worker 経路に入れる (同期経路の除外条件と同じ)。

満たすとき:

- `pane_epub_restore` は `None` のはず (`pre_scan = None` の producer は渡さない)。`debug_assert!` し、
  `Some` なら従来の同期経路に回す。
- consumer が既に capture した `navigation` (無ければ従来どおり `capture_main_list_navigation(Direct(Navigation),
  copied_open_source_proof(&owner))`) をそのまま pending に入れる。走査開始側で取り直さない。
- 走査開始は既存の `start_folder_open_scan_with_restore` を共有する。ツリーの `PaneNavigation` と同じ受付
  (`document_open_modal_admission_blocked` の拒否、`retire_replaced_main_list_requests(Some(Pane))` 相当、
  main bookmark open の取消、旧 pending の取消) を行う。startup open を置き換えるかどうかは、同期経路と同じく
  `restore_intent.replaces_deferred_startup_open() && stage == MainListRequestStage::Admission` で決める
  (`retire_replaced_main_list_requests_with_startup_intent`)。navigation の capture は「呼び出し元が渡した値を使う」
  入口を足し、`PaneNavigation` などの既存 purpose の capture は変えない。
- worker の spawn・pending 生成は既存のコードを共有し、複製しない。
- 戻る前に `clear_pending_folder_nav_steps()` を行う (従来の consumer 末尾と同じ)。

### 4.3 完了 (既存の裁定へ戻す)

- `poll_folder_pane_open` はそのまま (sidecar 復元中の保留、navigation の stale 取消、`Disconnected` の扱い)。
- `resolve_main_folder_open_ready` に `DirectNavigation` の腕を足し、`ResolvedMainFolderOpen { restore_intent,
  path, scan, collection_owner, epub_restore: None, navigation }` を返す。走査失敗は既存の共通処理
  (ログ + 「フォルダを読み取れませんでした」) に乗る。
- `update_phases.rs` の `folder_pane_open_ready` 分岐は変更不要: `navigate_stage = Continuation`、
  `navigate_pre_scan = Some(scan)`、`collection_owner` から `navigate_owner` を作り、
  `open_direct_navigation_target_at_stage(…, Some(scan), …)` へ入る。`#[cfg(not(windows))]` 側の match も
  コンパイルが通る形で腕を足す (非 Windows のビルド検査はしない)。
- 採用時の読込は今の `pre_scan = Some` 分岐で、snapshot scope・pdf refusal・smart 対象・claim・install は
  ツリー完了と同じ。

### 4.4 置き換え・取消 (既存の規則のまま)

| 状況 | 結果 |
| --- | --- |
| 走査中に別の直接移動・ツリーのクリック・検索中の場所ジャンプ | 新しい要求の `start_folder_open_scan_*` が旧 pending を cancel (最後の操作が勝つ) |
| 走査中に高優先の nav (お気に入り・キー・DFS 結果・アドレス欄・コンテキスト) | `higher_priority_than_folder_pane` で cancel |
| 走査中に別経路の main 要求 (履歴の戻る/進む、コレクション、レーティング等) が受理 | `retire_replaced_main_list_requests` が `cancel_folder_pane_open` |
| 走査中に表示中の一覧が別の採用で変わる | `main_list_navigation_is_current` が偽になり poll で cancel |
| sidecar 復元中に完了 | poll が保留し、復元終了後に裁定 |
| worker の異常終了 | 既存の `Disconnected` 処理 (pending を破棄) |

まれな失敗 (走査中の I/O エラー) は既存どおりログ + 通知で割り切る。再試行は足さない。

### 4.5 計測

- `start_folder_open_scan_with_restore` の worker で走査時間を測り、`nav/folder_open_scan`
  (`ms`, `purpose`, `folders`, `media`, `failed`, `cancelled`) を出す。全 purpose 共通 (ツリーの走査も見えるようになる)。
  `dfs_scan` と同じく `crate::perf::is_enabled()` のときだけ。
- 採用時の `nav/lf_scan` は `pre_scanned=true` になり、UI 側の `ms` は走査を含まない。
- `scripts/analyze_perf.py` の nav 内訳に `folder_open_scan` を足す。

## 5. 不変条件とテスト

1. 対象の直接移動 (お気に入り・アドレス Enter・最近開いたフォルダ・親へ・コレクション物理元) は、dispatch した
   フレームで `scan_directory_with_settings` を呼ばない。テスト用の計数 (cfg(test) の hook か counter) で
   UI スレッドの同期走査が 0 回であることを確かめる。
2. worker 完了後の poll + 裁定で、従来の同期経路と同じ一覧・選択・履歴 (Direct の push 1 回)・address が採用される。
3. 走査中の置き換え: 直接移動 A → 直接移動 B、直接移動 → ツリー、ツリー → 直接移動、直接移動 → 履歴の戻る、
   直接移動 → キー nav で、最後の要求だけが採用され、古い結果は採用されない。
4. stale: 走査中に別の採用で一覧が変わったら、完了しても採用しない。
5. 対象外の型 (ZIP/PDF、合成パス、`.epub`/変換書庫、detached context) は従来の経路のまま。
6. `CollectionGridPhysical` owner と非 ExplicitList の restore_intent が完了時に保たれる。
7. 走査失敗で一覧を変えず、通知とログを出す。
8. 既存テストのうち、`pre_scan = None` の直接移動の直後に同期採用を前提にしていたものは、worker 完了 + 裁定を
   回す helper を通す形に直す (挙動の期待値は変えない)。

## 6. 文書の更新

- [ui-responsiveness.md](ui-responsiveness.md): 本件の節 (上部の近年の節と同じ形) と §5 の表への 1 行。
- [async-architecture.md](async-architecture.md): 「物理フォルダ open 事前走査」の行と §718 付近の表に
  `DirectNavigation` を追記。
- [detached-rework-plan.md](detached-rework-plan.md): `FolderOpenScanPurpose` (bundle 所有の pending) に
  main 専用の variant を足したことを記録する (detached context は経路を変えない)。

## 7. 範囲外 (後続候補)

- `dispatch_main_input_nav_immediate` の `Direct` / `CollectionSource` (全画面を閉じる即時経路) の同期走査。
- `load_folder` / `load_folder_with_scan(path, None)` の同期呼び出し元。
- detached context の直接移動 (リワーク側で扱う)。
- 実フォルダなのに ZIP 系の拡張子を持ち、一覧にフォルダとして見えていない場合 (同期経路のまま)。
