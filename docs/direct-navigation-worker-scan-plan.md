# 直接移動のフォルダ走査を worker へ移す (§1.805)

状態: 設計 r4 (2026-11-18、設計レビュー r1〜r3 の REJECT と統括の「組み合わせを減らす」指示を反映、§3.1)。担当: 設計・検収 = ClaudeCode Opus 5.5、
実装 = Codex GPT-6.1 Sol / high、独立レビュー = GPT-6.1 Sol / xhigh。

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

| 入口 | 経路 | owner / restore_intent / 全画面予約 |
| --- | --- | --- |
| ツールバーのお気に入り・ピン留め本・本棚 | `toolbar_fav_nav` → `dispatch_favorite_navigation` → `Some(path)` | Navigation / ExplicitList / なし |
| メニューバーのお気に入り | `fav_nav` (`LocationJumpTarget::Physical`) → 同上 | 同上 |
| キー・リング・ゲームパッドのお気に入り | `AddressBarNav::Favorite` (`input_nav`) → 同上。全画面中は先に閉じる (`gamepad_input.rs` 5813・8011 付近) | 同上 |
| フォルダを開くダイアログ | `open_folder_nav` → 同上 | 同上 |
| アドレス欄 Enter (検索外) | `AddressBarNav::AddressInput` → `dispatch_plain_location_jump` → 直接 `open_direct_navigation_target_at_stage` | 同上 |
| アドレス欄の履歴メニュー (最近開いたフォルダ) | `AddressBarNav::Direct(path, ExplicitList)` (`ui_main.rs` の address_input) | 同上 |
| 一覧のフォルダを開く (ダブルクリック・Enter・ゲームパッド) | `accept_grid_container_open` → `grid_physical_navigation` → `Direct` / `CollectionSource` (`ui_main.rs` 18174、`collection_grid.rs` 1441〜1485、`gamepad_input.rs` 9983 付近) | Navigation または CollectionGridPhysical / 各 intent / **`pending_auto_fs_open` を先に立てることがある** |
| 親へ (BS・`..`・スマート/サブフォルダ展開の親) | `AddressBarNav::Direct(parent, …)` (`grid_parent.rs`・`close_return.rs` の非即時経路・`smart_folder.rs`・`subfolder_expansion.rs`・`app.rs` 24605 付近) | Navigation / 各 intent |
| コレクションの物理元 | `AddressBarNav::CollectionSource` | CollectionGridPhysical / 各 intent |
| コンテキストメニューの書庫ジャンプ | `app.rs` 60616 付近 (書庫なので対象外) | Navigation / ExplicitList |

検索中 (`global_search` / `favsearch` / `tag_view`) のお気に入り・アドレス欄は既に
`begin_location_jump_to_physical` → `FolderOpenScanPurpose::JumpToPhysicalFolder` で worker 走査している。
履歴の戻る/進む (`dispatch_main_folder_history_input`) は `PhysicalHistoryTransition` の preflight worker で
走査済み。分類が必要な名前 (`.epub`・変換書庫の拡張子) は分類 worker が `folder_scan` を渡す。
常に新しい窓で開くモードの一覧フォルダは `GridFolderCandidate` で worker 走査済み。
いずれも本件の対象外 (変更しない)。

### 2.3 この consumer を通らない同期走査 (範囲外として残す)

- `dispatch_main_input_nav_immediate` (全画面を閉じる即時経路) の `Direct` / `CollectionSource` は
  `load_folder_or_convert_archive_with_auto_fullscreen_owned` を直接呼ぶ。
- `pub fn load_folder` / `load_folder_with_scan(path, None)` の各呼び出し元 (本棚マネージャ等)。
- main 以外の viewer context (detached・独立窓の `CollectionRoot` 等) での直接移動。detached viewer は
  リワーク中で凍結されているため触らない。
これらは本件の後に別項目として扱う (§7)。

## 3. 案の比較 (CLAUDE.md「設計の簡素化」)

| 案 | 判断 |
| --- | --- |
| A. ツリーと同じ `FolderPaneOpenPending` で走査し、完了を通常 nav 裁定へ戻して `pre_scan` 付きで consumer に再入する | **採用**。走査・置き換え・取消・stale 判定・完了の裁定・`pre_scan` 付きの読込は、ツリーのクリックが通る既存経路。直接移動側は「拒否判定と所有権の受理を今の同期経路のまま済ませ、走査の代わりに要求を出す」だけにする (§4.2)。採用時の読込は今の `pre_scan = Some` 分岐 (ツリー完了・DFS と同じ) で、install の挙動差を作らない |
| B. ZIP の直接移動・履歴の戻る/進むと同じ `PhysicalHistoryTransition` (`Preflighting` → `Folder(ScannedDir)`) へ入れる | **不採用**。採用が `commit_physical_history_transition` / `prepare_physical_listing` の別の install 経路になり、今の直接移動のフォルダ読込 (検索 origin、smart 対象、book append、rating 行き先など) と挙動を照合し直す範囲が広い。[file-type-visibility-plan.md](file-type-visibility-plan.md) §9.2 の表は親の読込をこの経路へ寄せる方針を書いているが、現状は未実施で、本件では広げない |
| C. `load_folder_with_scan_claimed_with_search_origin` の同期走査を全呼び出し元で worker 化する | **不採用 (今回)**。`load_folder` 等の多数の同期呼び出し元が読込直後の状態に依存している可能性があり、範囲が大きい。§7 の後続候補 |
| D. 走査中は全体をモーダルにする | **不採用**。移動の入力 (お気に入り・アドレス欄・ツリー・履歴等) まで止める理由はない。後勝ちで扱える (§4.4)。採るなら利用者への相談が要る |

### 3.1 割り込みの組み合わせを減らす (設計レビュー r1〜r3 と統括指示、2026-11-18)

r1〜r3 のレビューでは、毎回「走査中に旧一覧で別の操作をする」組み合わせの指摘が出た
(一覧のフォルダを開く、同じフレームの入力、画像・動画を開く、ZIP/PDF の構成確認待ちを経た open、
保存済みの本を開く、DFS)。入口ごとに取消を足す形では、レビューのたびに次の入口が見つかる
(CLAUDE.md「設計の簡素化」の EPUB 変換と履歴の経緯と同じ形)。組み合わせ自体を減らす形を比べた。

| 形 | 判断 |
| --- | --- |
| (1) 移動の要求の持ち主を 1 か所にまとめ、どの入口からでも後勝ちにする。DFS と同じ経路・同じ世代番号に乗せる | **一部採用**。移動の要求どうしは、既存の一か所の退役 `retire_replaced_main_list_requests` と通常 nav 裁定 (§4.4) で後勝ちになる。DFS (`folder_nav_pending`) は独自の累積・モード・fs nav lock・bundle 所有を持つため、pane pending と保存場所を統合する改修は本件では行わない。代わりに、DFS の受付と直接移動の受付が互いの pending を退役させる 2 点だけを足す |
| (1') 全ての main 要求の `MainListNavigation` に受付順の番号を入れ、`main_list_navigation_is_current` で「新しい受付があれば失効」とする | **不採用**。捕捉は 48 か所・照合は 42 か所あり、続きの処理での取り直しが自分の要求を失効させる危険がある。一覧の画像を開く操作や構成確認待ちは navigation を捕捉しないので、入口の列挙も残る。全非同期要求に効く広い改修で、本件の範囲を超える |
| (2) 既存のフォルダ移動 (pre_scanned の経路) に直接移動を合流させる | **採用** (案 A そのもの)。完了は既存の `pre_scan = Some` の読込へ戻る |
| (3) 走査待ちの間は、旧一覧の項目への入力を受け付けない | **採用**。既存の単一の判定 `grid_item_input_allowed()` (キー・ポインタ・タッチ・リング・ゲームパッド・メニューの全一覧入力が通る) は、履歴の戻る/進む・レーティング移動の待ち中に既に項目入力を止めている (`location_navigation_pending`)。直接移動の走査待ちをこの判定に加えるだけで、旧一覧での割り込みの組み合わせがまとめて消える。従来は走査中に UI 全体が止まっていたので、項目入力だけを止めるのは通常の使い方の退行ではない。移動の入力は止めない |
| (3') ツリーの走査待ちにも (3) を適用する | **今回は不採用**。ツリーはリリース済みの挙動で、走査中も旧一覧を操作できる。変えるなら相談する。後続候補 (§7) |

採った形のまとめ: **旧一覧の項目入力は止め、移動の入力は後勝ちにする。** 個別に残る受付点は、DFS の開始、
直接移動の受付での DFS の退役、保存済みの本の受付の 3 つだけ (§4.4)。

待ち時間中の見え方: 走査が終わるまで今の一覧を表示し続け、完了したフレームで新しい一覧に替わる。
従来は UI が走査時間だけ止まってから替わっていた。走査中は旧一覧の項目入力を受け付けない (スクロール等の
項目以外の操作と、移動の入力は受け付ける)。走査が 500 ms を超えたら、履歴の戻る/進むと同じ
「移動先を読み込み中…」の表示を出す。短い走査では出さない (毎回一瞬出るとちらつきになるため)。

## 4. 設計

### 4.1 新しい用途

```rust
pub(crate) enum FolderOpenScanPurpose {
    …
    /// main context の直接移動 (§2.2) の実フォルダ。拒否判定と所有権の受理は dispatch 時に済ませ、
    /// 完了したら pre_scan 付きで open_direct_navigation_target_at_stage へ戻る。
    DirectNavigation {
        restore_intent: StartupListIntent,
        collection_owner: Option<top_level_grid_view::CollectionGridPhysicalLoadOwner>,
        /// 受付時の `pending_auto_fs_open`。要求と寿命を同じにし、取消・失敗・stale では捨てる。
        auto_fullscreen: bool,
    },
}
```

- `owner` は `Navigation` / `CollectionGridPhysical` の 2 種だけなので、`ResolvedMainFolderOpen` と同じ
  `collection_owner: Option<…>` で持つ。それ以外の owner は worker 経路に入れない。
- `detached_loading_lease()` / `diagnostic_trace()` / `take_diagnostic_trace()` / 失敗ログの purpose 名
  (`"direct-navigation"`) の各 match に腕を足す。detached lease・診断 trace は持たない。

### 4.2 受付: 拒否判定 → 所有権の受理 → 競合の退役 → spawn

worker へ切り替える場所は、同期経路が走査に到達する直前、すなわち
`load_folder_or_convert_archive_with_auto_fullscreen_classified_with_navigation_at_stage` の末尾で
`load_folder_with_scan_claimed(…)` を呼ぶ箇所とする。ここまでに今の同期経路の拒否判定
(file-type visibility、snapshot scope、smart synthetic、pdf refusal、smart physical target、変換書庫の分岐) と
`claim_open_request_owner_with_startup_intent` (modal 拒否、owner の現在性、書庫変換・未解決 startup open・
競合 bookmark の取消) がすべて今の順序で済んでいる。拒否された要求は既存 owner を壊さない (現行の回帰契約)。

この関数は他に 5 か所から呼ばれるため、型付きの引数で経路を選ぶ:

```rust
enum DirectoryScanRoute { UiThread, Worker }
```

`open_direct_navigation_target_classified` の `None` 腕だけが `Worker` を渡し、他の呼び出し元は `UiThread`
(挙動不変)。`Worker` のときに次をすべて満たせば、`load_folder_with_scan_claimed` の代わりに走査要求を出す:

1. `pre_scan.is_none()`。
2. `self.main_folder_history_available()` (main context の同定。detached・独立窓は `UiThread` のまま) かつ
   `navigation.is_some()`。navigation は consumer が capture 済みの値を使い、取り直さない。
3. owner が `Navigation` または `CollectionGridPhysical`。
4. `load_folder_with_scan_claimed_with_search_origin` の中で走査より前に返る分岐に当たらない:
   `crate::folder_tree::is_virtual_folder(path)` (ZIP 系・PDF 等の paged document を含む) なら、
   `classified_kind == Some(OpenPathKind::Directory)` または `visible_grid_item_is_folder(path)` のときだけ対象。
   判定は拡張子と在メモリの一覧だけで行い、UI スレッドで stat しない。合成パスなど実フォルダでない行き先は、
   従来どおり走査の失敗 (ログ + 通知) になる (従来は UI スレッドで失敗していた)。

判定は 1 つの helper に置き、同期経路の対応する条件の隣にコメントで相互参照する。

満たすときの受付処理 (この順):

1. `pending_auto_fs_open` を取り出して purpose の `auto_fullscreen` に移し、フィールドは `false` に戻す。
2. 競合する main 要求を退役させる: `retire_replaced_main_list_requests_with_startup_intent(Some(Pane),
   restore_intent.replaces_deferred_startup_open() && stage == MainListRequestStage::Admission)`。
3. 進行中の DFS (`folder_nav_pending`) を取り消し、累積ステップを消す。これは同期経路の install が
   `folder_nav_pending` を取り消していた処理 (`app.rs` 43930 付近) を受付時へ前倒しするもの。旧 DFS が
   全画面の移動 sequence (fs nav lock・holdover) を持っていれば、その sequence も既存の終端 API
   (`finish_fs_navigation_sequence(Superseded)` 等) で終える。`poll_folder_nav` が結果を取り出した後は
   `folder_nav_pending == None` でも sequence が残り得るので、pending の有無ではなく sequence の所有で判断する。
4. 旧 pane pending を取り消し (既存の `cancel_folder_pane_open`)、worker を spawn して
   `FolderPaneOpenPending { purpose: DirectNavigation{…}, navigation, epub_restore: None, … }` を置く。
   spawn・pending 生成は `start_folder_open_scan_with_restore` と共有し、複製しない
   (navigation を呼び出し元から受け取る入口を足し、既存 purpose の capture は変えない)。
5. `FolderOpenOutcome` は走査中を表す値 (既存の `Classifying` と同じ扱いでよいか、新しい値が要るかは実装時に
   呼び出し元の match を見て決める) を返す。consumer は `finish_pane_open_restore` を `pane_epub_restore = None`
   で呼ぶだけなので影響しない。`clear_pending_folder_nav_steps()` は従来どおり行う。
6. test-script の要求数は、受理した要求を受付時に 1 回数える現行の契約を保つ (完了時の再入で二重に数えない)。

### 4.3 完了 (既存の裁定へ戻す)

- `poll_folder_pane_open` はそのまま (sidecar 復元中の保留、navigation の stale 取消、`Disconnected` の扱い)。
- `resolve_main_folder_open_ready` に `DirectNavigation` の腕を足し、`pending_auto_fs_open = auto_fullscreen` を
  設定してから `ResolvedMainFolderOpen { restore_intent, path, scan, collection_owner, epub_restore: None, navigation }`
  を返す (`GridFolderCandidate` が `pending_auto_fs_open = image_book` を設定する前例と同じ)。走査失敗は既存の
  共通処理 (ログ + 「フォルダを読み取れませんでした」) に乗り、予約は捨てる。
- **全画面予約はその要求のすべての終端で消える**: 取消・走査失敗・stale に加え、完了後の再入で採用が拒否された
  場合 (文書モーダル中の claim 拒否、snapshot 範囲外、owner の失効等) も、`pending_auto_fs_open` を `false` に戻す。
  consumer の `pre_scan = Some` 分岐で、走査完了からの再入 (`GridFolderCandidate` の完了を含む) の結果が採用でなければ
  予約を消す。予約を立てた要求以外が消費する経路を残さない。`GridFolderCandidate` の既存の漏れも同じ規則で閉じる。
- `update_phases.rs` の `folder_pane_open_ready` 分岐は変更不要: `navigate_stage = Continuation`、
  `navigate_pre_scan = Some(scan)`、`collection_owner` から `navigate_owner` を作り、
  `open_direct_navigation_target_at_stage(…, Some(scan), …)` へ入る。`#[cfg(not(windows))]` 側の match も
  コンパイルが通る形で腕を足す (非 Windows のビルド検査はしない)。
- 採用時の読込は今の `pre_scan = Some` 分岐で、snapshot scope・pdf refusal・smart 対象・claim (owner の現在性の
  再検証を含む)・install はツリー完了と同じ。

### 4.4 走査待ちの間の入力: 旧一覧の項目入力は止め、移動の入力は後勝ち

**規則 1 (旧一覧の項目入力)**: main に `DirectNavigation` の pending があるあいだ、`grid_item_input_allowed()` は
偽を返す。判定は既存の `location_navigation_pending()` と並ぶ一つの述語 (例: `direct_navigation_scan_pending()`) に置き、
`grid_item_input_allowed()` から呼ぶ。これで一覧のフォルダを開く (`grid_nav`)・画像/動画を開く・構成確認待ちを経た
open・コンテキストメニュー・スタック・ゲームパッドの項目操作など、項目入力の全入口がまとめて止まる
(`grid_nav` も出なくなるので、完了と同じフレームの `grid_nav` という組み合わせも生じない)。
`location_navigation_pending()` 自体には加えない (待機表示と `book_structural_op` の判定まで変わるため)。
待機表示は §4.6。

**規則 2 (移動の入力は後勝ち)**: 移動の入力は止めず、新しい要求が走査待ちを退役させる。

| 新しい入力 | 仕組み |
| --- | --- |
| 別の直接移動・ツリーのクリック・検索中の場所ジャンプ | 新しい要求の受付が旧 pending を cancel (既存) |
| お気に入り・キー/ゲームパッド/リングの nav・DFS 結果・アドレス欄・コンテキストの nav | 通常 nav 裁定の `higher_priority_than_folder_pane` で cancel (既存) |
| 履歴の戻る/進む、コレクション、レーティング、Quick Folder 切替、Remote 取得等の main 要求 | `retire_replaced_main_list_requests` が `cancel_folder_pane_open` (既存) |
| **DFS (Ctrl+↑↓ 等) の開始** | **追加 (受付点 1)**: `start_folder_nav` が新しい DFS を受理したとき (既存 DFS への累積は除く)、main の `DirectNavigation` / `PaneNavigation` の pending を `cancel_folder_pane_open` で cancel する。`cancel_pending_folder_nav()` は新しい DFS 用の lock・holdover まで消すので流用しない。detached 用の purpose・`RequiredFullscreenTarget`・`CurrentViewOrderRefresh` 等には触れない |
| **直接移動の受付 (DFS 実行中)** | **追加 (受付点 2)**: §4.2 の 3 |
| **保存済みの本・コレクションを開く** | **変更 (受付点 3)**: 現在は `saved_group_other_main_nav_pending()` が pane pending を理由に「別の場所を開く処理が進行中です」で拒否する。受付 (`saved_group_open_source_if_allowed`) で、本棚更新中・行き先の未登録など**既知の拒否判定をすべて通した後**に、置換できる走査 (`DirectNavigation` / `PaneNavigation`) だけを例外として取り消して進める。拒否された操作は走査待ちを壊さない。共通 helper `saved_group_other_main_nav_pending()` は変えない (既存 saved-group が後発の pane scan に道を譲る契約 `ready_book_yields_candidate_to_later_pending_main_navigation` を保つ) |

**規則 3 (既存の失効)**:

| 状況 | 仕組み |
| --- | --- |
| 走査中に表示中の一覧が別の採用で変わる | `main_list_navigation_is_current` が偽になり poll で cancel (既存) |
| sidecar 復元中に完了 | poll が保留 (既存) |
| worker の異常終了 | 既存の `Disconnected` 処理。予約も捨てる |

まれな失敗 (走査中の I/O エラー) は既存どおりログ + 通知で割り切る。再試行は足さない。

### 4.5 計測

- `start_folder_open_scan_with_restore` の worker で走査時間を測り、`nav/folder_open_scan`
  (`ms`, `purpose`, `folders`, `media`, `failed`, `cancelled`) を spawn 時に捕捉した `input_seq` で出す。全 purpose 共通
  (ツリーの走査も見えるようになる)。`dfs_scan` と同じく `crate::perf::is_enabled()` のときだけ。
- 採用時の `nav/lf_scan` は `pre_scanned=true` になり、UI 側の `ms` は走査を含まない。
- `scripts/analyze_perf.py` の nav 内訳に `folder_open_scan` を足す。

### 4.6 待機表示

- 直接移動の走査待ちが 500 ms を超えたら、既存の `render_location_navigation_wait_overlay`
  (「移動先を読み込み中…」) を同じ見た目で出す。500 ms 未満では出さない。開始時刻は pending に持たせ、
  猶予の判定と再描画の予約は既存の待機表示の描画に寄せる (別の表示を作らない)。
- 猶予の定数は本件専用に置く (sidecar 復元の猶予とは意味が違うので共有しない)。

## 5. 不変条件とテスト

1. 対象の直接移動 (お気に入り・アドレス Enter・最近開いたフォルダ・一覧のフォルダ・親へ・コレクション物理元) は、
   dispatch したフレームで `scan_directory_with_settings` を呼ばない。テスト用の計数 (cfg(test) の hook か counter) で
   UI スレッドの同期走査が 0 回であることを確かめる。
2. worker 完了後の poll + 裁定で、従来の同期経路と同じ一覧・選択・履歴 (Direct の push 1 回)・address が採用される。
3. 拒否: snapshot 範囲外・file-type visibility の拒否は受付時に起き、既存の owner (startup open、履歴遷移、rating、
   collection 等) を壊さない。
4. 規則 1: 直接移動の走査待ちのあいだ `grid_item_input_allowed()` が偽で、一覧のフォルダを開く・画像を開く
   (ZIP/PDF の構成確認待ちを経る open を含む) 操作が何も起こさない。走査が終われば項目入力が戻る。
   ツリーの走査待ち (`PaneNavigation`) では従来どおり項目入力を受け付ける。
5. 規則 2 の置き換え: 直接移動 A → B、直接移動 ↔ ツリー、直接移動 → 履歴の戻る、直接移動 → キー nav、
   DFS → 直接移動 (旧 DFS の結果を採用しない)、直接移動 → DFS (完了した直接移動が新しい DFS を消さない)、
   saved-group open (既知の拒否を通った後に取り消す)、saved-group の受付拒否 (走査待ちは保たれる)、
   saved-group → 後発の pane scan (既存契約のまま saved-group が譲る)、Quick Folder A→B→A、Remote 取得。
   最後の要求だけが採用される。DFS の退役で fs nav lock・holdover が残らないこと
   (結果取り出し後・同じフレームの境界を含む) も確かめる。
6. 待機表示: 500 ms 未満の走査では「移動先を読み込み中…」を描かず、500 ms を超えたら描く。
7. stale: 走査中に別の採用で一覧が変わったら、完了しても採用しない。
8. 全画面予約: 自動全画面付きの一覧フォルダ A → A を取消 → お気に入り B で、B は全画面にならない。A がそのまま
   完了したときは従来どおり全画面になる。完了後の採用が拒否された場合 (文書モーダル中など) も予約が残らない。
9. 対象外の型 (ZIP/PDF、`.epub`/変換書庫、main 以外の context) は従来の経路のまま。
10. `CollectionGridPhysical` owner と非 ExplicitList の restore_intent が完了時に保たれる。
11. 走査失敗で一覧を変えず、通知とログを出す。
12. test-script の要求数が受付時 1 回のまま (取消された受理済み要求も数える)。
13. 既存テストのうち、`pre_scan = None` の直接移動の直後に同期採用を前提にしていたものは、worker 完了 + 裁定を
    回す helper を通す形に直す (挙動の期待値は変えない)。期待値を変える必要が出たら止めて報告する。

## 6. 文書の更新

- [ui-responsiveness.md](ui-responsiveness.md): 本件の節 (上部の近年の節と同じ形) と §5 の表への 1 行。
- [async-architecture.md](async-architecture.md): 「物理フォルダ open 事前走査」の行と §718 付近の表に
  `DirectNavigation` と §4.4 の取消規則を追記。
- [detached-rework-plan.md](detached-rework-plan.md) §11: bundle 所有の `FolderOpenScanPurpose` に main 専用の
  variant を足したこと、`start_folder_nav` の取消が detached 用 purpose に触れないこと、detached context は
  経路を変えないことを記録する (設計レビュー r1 で「構造的変更」に AGREE)。

## 7. 範囲外 (後続候補)

- `dispatch_main_input_nav_immediate` の `Direct` / `CollectionSource` (全画面を閉じる即時経路) の同期走査。
- `load_folder` / `load_folder_with_scan(path, None)` の同期呼び出し元。
- main 以外の context の直接移動 (detached はリワーク側で扱う)。
- ツリーの走査待ち・DFS 実行中の旧一覧の項目入力にも規則 1 を適用するか (リリース済みの挙動の変更なので相談)。
  現在はツリー・DFS の走査待ち中に旧一覧を操作でき、r1〜r3 で挙がった割り込み (同じフレームの入力、画像を開く等)
  がそのまま起こり得る。
