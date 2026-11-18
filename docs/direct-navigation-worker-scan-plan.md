# 直接移動のフォルダ走査を worker へ移す (§1.805)

状態: 設計 r9 (2026-11-18、r8 のレビューの指摘を反映。r8 は r7 の新しい設計レビューの指摘を反映。r7 は 利用者決定で範囲をお気に入り・アドレス欄 Enter・最近開いたフォルダのメニューに戻した版、§3.2。r6 までは r4・r5 の指摘も反映。設計レビュー r1〜r3 の REJECT と統括の「組み合わせを減らす」指示を反映、§3.1)。担当: 設計・検収 = ClaudeCode Opus 5.5、
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

`open_direct_navigation_target_at_stage` → `pre_scan` が `Some` なら確定済みのDirectoryとして、
`None` なら `path_needs_open_classification` に従って、`open_direct_navigation_target_classified` へ進む。
`pre_scan` が `Some` なら
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

### 2.2.1 対象とする入口 (利用者決定 2026-11-18)

worker 走査にするのは次の入口だけ:

- お気に入り: ツールバーのお気に入り・ピン留め本・本棚、メニューバーのお気に入り、キー・リング・ゲームパッドのお気に入り
- アドレス欄 Enter (検索外)
- アドレス欄の履歴メニュー (最近開いたフォルダ)

同じ consumer を通る次の入口は**従来どおり UI スレッドの同期走査のまま**とし、挙動を変えない: フォルダを開くダイアログ、
一覧のフォルダを開く (Enter・ダブルクリック・ゲームパッド)、親へ (BS・親ボタン・ZIP からの BS・スマート/サブフォルダ展開の親)、
コレクションの物理元、コンテキストメニュー経由の open。理由は §3.2。

印の付け方 (r8、設計レビュー r7 の割り振りによる): `Worker` の印は次の生成点で付ける。
メニューのお気に入り (`fav_nav` の `Physical`、`update_phases.rs` 1272 付近)、ツールバーのお気に入り・ピン留め本
(`toolbar_fav_nav`、同 1282 付近)、キーのお気に入り番号・前後とリング/ジェスチャ/ゲームパッドのお気に入り番号
(`gamepad_input.rs` 7990・8221 付近の共通 producer)、ゲームパッドのお気に入りピッカー確定 (同 5816 付近)、
アドレス欄 Enter の `Physical` (`ui_main.rs` 16707 付近。`dispatch_plain_location_jump` まで印を運ぶ)、
最近開いたフォルダの選択 (`ui_main.rs` 16664 付近、専用の値で通常の `Direct` と区別)。
同じ関数 (`dispatch_favorite_navigation`・`dispatch_plain_location_jump`) や同じ値 (`Favorite`・`Direct`・
`LocationJump::Physical`) を通る他の入口は印を持たず `UiThread`: フォルダを開くダイアログ、一覧のフォルダ open、
親移動 (通常・close-return・smart・サブ展開)、コレクション、コンテキストメニュー、`grid_nav`、場所メニュー・整理先・
既知フォルダ・ドライブ選択/切替・ゲームパッドの場所ピッカー等の `LocationJump::Physical`、コンテキストの書庫ジャンプ。

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

### 3.2 範囲を広げなかった理由 (利用者決定 2026-11-18)

r1〜r6 の設計は、同じ consumer を通る一覧のフォルダを開く操作・親へ・コレクションの物理元まで worker 化していた
(実装 ce0b39b49・b90e0b111)。本体 lib 全件では、それらの入口の挙動を前提にした 39 件が失敗した
(history_parent_1473・ratingnav649・same_position_double_click・section1328)。結果 (行き先・選択・履歴) は同じだが、
同期の切り替えが worker 待ちに変わり、待ちの間は一覧の項目入力が止まる。

一覧側を非同期にすると、Enter の直後に押したキー (先行入力。例: 開いた直後の矢印・Enter) が、待っている間に
規則 1 で捨てられるおそれがある。従来は UI が止まっていた間の入力が、新しい一覧に対して処理されていた。
この扱いを決めないと退行になり得るため、利用者の判断で当初の範囲 (§2.2.1) に戻した。一覧側は §7 の後続候補とする。

範囲内の入口にも同じ性質はある: お気に入りを押した・アドレス欄で Enter した直後のキー入力は、走査待ちの間
一覧に届かない (履歴の戻る/進むの待ちと同じ扱い)。これらの入口では、押した直後に一覧へキー入力を続ける使い方は
一覧の Enter ほど一般的でないと判断し、規則 1 を適用する。この扱いは統括経由で利用者に確認し、利用者が了承した (2026-11-18、「これでよい」)。了承の対象には、アドレス欄で
パスを入力して Enter した直後に ↓ や Enter などのキーを一覧へ続ける操作も含む (アドレス確定でフォーカスが一覧へ移る
ため、走査待ちの間のキーは一覧で処理されず捨てられる。従来の同期走査では UI が止まっている間 OS が入力を保持し、
新しい一覧で処理されていた)。

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
| (3) 走査待ちの間は、履歴の戻る/進むの待ちと同じ入力の契約にする | **採用**。既存の判定 `grid_item_input_allowed()` (一覧のキー・ポインタ・タッチ・リング・ゲームパッド・コンテキストメニューの入力が通る) は、履歴の戻る/進む・レーティング移動の待ち (`location_navigation_pending()`) で既に偽になる。直接移動の走査待ちを `location_navigation_pending()` に加え、**直接移動の待ちを「場所の移動待ち」という既存の一つの状態として扱う**。旧一覧での割り込みの組み合わせがまとめて消え、入力の扱いが履歴の待ちと同じになる。従来は走査中に UI 全体が止まっていたので、通常の使い方の退行ではない (設計レビュー r4 で同意) |
| (3') ツリーの走査待ちにも (3) を適用する | **今回は不採用**。ツリーはリリース済みの挙動で、走査中も旧一覧を操作できる。変えるなら相談する。後続候補 (§7) |

採った形のまとめ: **直接移動の待ちは、履歴の戻る/進むの待ちと同じ「場所の移動待ち」とする。** 一覧の判定の
内側の入力は止まり、判定の外にある移動の入力 (ツールバー・メニューのお気に入り、アドレス欄、ツリーのクリック、
戻る/進む等) は後勝ちになる。個別に残る受付点は、DFS の開始、直接移動の受付での DFS の退役、保存済みの本の
受付の 3 つだけ (§4.4)。

待ち時間中の見え方: 走査が終わるまで今の一覧を表示し続け、完了したフレームで新しい一覧に替わる。
従来は UI が走査時間だけ止まってから替わっていた。走査中の入力は履歴の戻る/進むの待ちと同じ扱いで、
一覧の判定を通る入力 (項目操作、ゲームパッドの左スティックでのツリー選択、右ドラッグのリング・マウスジェスチャー、
タッチ・ゲームパッドの一覧スクロール等) は止まる。走査が 500 ms を超えたら、履歴の戻る/進むと同じ
「移動先を読み込み中…」の表示を出す。短い走査では出さない (毎回一瞬出るとちらつきになるため)。

## 4. 設計

### 4.1 新しい用途

```rust
pub(crate) enum FolderOpenScanPurpose {
    …
    /// main context の直接移動 (§2.2) の実フォルダ。拒否判定と所有権の受理は dispatch 時に済ませ、
    /// 完了したら pre_scan 付きで open_direct_navigation_target_at_stage へ戻る。
    DirectNavigation,
}
```

- 対象の入口 (§2.2.1) の owner は `Navigation` だけなので、owner は持たない (`ResolvedMainFolderOpen` の
  `collection_owner` は `None`)。`Navigation` 以外の owner は worker 経路に入れない。
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

`Worker` は §2.2.1 の入口の producer だけが型付きの印として consumer に渡す (`open_direct_navigation_target_at_stage`
の引数、`update_phases.rs` の裁定で勝った入口ごとに決める値、最近開いたフォルダのメニューは専用の
`AddressBarNav` の値など)。consumer は値の形 (例: `AddressBarNav::Favorite` は一覧の container open からも出る) から
入口を推測しない。印の無い直接移動・他の呼び出し元はすべて `UiThread` (挙動不変)。`Worker` のときに次をすべて満たせば、`load_folder_with_scan_claimed` の代わりに走査要求を出す:

1. `pre_scan.is_none()`。
2. `self.main_folder_history_available()` (main context の同定。detached・独立窓は `UiThread` のまま) かつ
   `navigation.is_some()`。navigation は consumer が capture 済みの値を使い、取り直さない。
3. owner が `Navigation`。
4. `load_folder_with_scan_claimed_with_search_origin` の中で走査より前に返る分岐に当たらない:
   `crate::folder_tree::is_virtual_folder(path)` (ZIP 系・PDF 等の paged document を含む) なら、
   `classified_kind == Some(OpenPathKind::Directory)` または `visible_grid_item_is_folder(path)` のときだけ対象。
   判定は拡張子と在メモリの一覧だけで行い、UI スレッドで stat しない。合成パスなど実フォルダでない行き先は、
   従来どおり走査の失敗 (ログ + 通知) になる (従来は UI スレッドで失敗していた)。

判定は 1 つの helper に置き、同期経路の対応する条件の隣にコメントで相互参照する。

満たすときの受付処理 (この順):

1. (r8 で削除: 範囲内の入口は全画面予約を作らないので、`pending_auto_fs_open` の移送はしない。グローバルの値には触れない)
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

- r7: 実装レビュー r1 の P2 対応で入れた「走査済みのパスを再分類しない」変更 (Directory の推定とそれに伴う文書モーダルの
  拒否を含む) は、ツリー・GridFolderCandidate の完了にも効いていたので撤回する (範囲外の経路を変えない)。DirectNavigation の対象は §4.2 の条件で分類の要らない名前に
  限られるので、完了の再入で再分類は起きない。
- `poll_folder_pane_open` はそのまま (sidecar 復元中の保留、navigation の stale 取消、`Disconnected` の扱い)。
- `resolve_main_folder_open_ready` に `DirectNavigation` の腕を足し、
  `ResolvedMainFolderOpen { restore_intent: ExplicitList, path, scan, collection_owner: None, epub_restore: None, navigation }`
  を返す。走査失敗は既存の共通処理 (ログ + 「フォルダを読み取れませんでした」) に乗る。
- r8: 全画面予約の移送と、走査完了後の採用拒否での予約消去は削除する (範囲内の入口は予約を作らない。消去を残すと
  ツリー・GridFolderCandidate の完了にも及ぶ)。実装済みの `pre_scan.is_some()` による消去と分類入口の消去も外す。
- `update_phases.rs` の `folder_pane_open_ready` 分岐は変更不要: `navigate_stage = Continuation`、
  `navigate_pre_scan = Some(scan)`、`collection_owner` から `navigate_owner` を作り、
  `open_direct_navigation_target_at_stage(…, Some(scan), …)` へ入る。`#[cfg(not(windows))]` 側の match も
  コンパイルが通る形で腕を足す (非 Windows のビルド検査はしない)。
- 採用時の読込は今の `pre_scan = Some` 分岐で、snapshot scope・pdf refusal・smart 対象・claim (owner の現在性の
  再検証を含む)・install はツリー完了と同じ。

### 4.4 走査待ちの間の入力: 旧一覧の項目入力は止め、移動の入力は後勝ち

**規則 1 (場所の移動待ち)**: main に `DirectNavigation` の pending があるあいだ、`location_navigation_pending()`
を真にする (履歴遷移・レーティング移動の待ちと同じ状態として扱う)。これにより `grid_item_input_allowed()` が偽になり、
一覧のフォルダを開く (`grid_nav`)・画像/動画を開く・構成確認待ちを経た open・コンテキストメニュー・スタック・
ゲームパッドの項目操作などがまとめて止まる (`grid_nav` も出なくなるので、完了と同じフレームの `grid_nav` という
組み合わせも生じない。pending は一覧描画の後に生成・poll されるので、完了フレームの描画中も止まったまま、r4 で確認)。
`book_structural_op` は既に非 refresh の pane pending で拒否しているので変わらない。待機表示は §4.6。

**規則 1 の付随修正 (止める理由を型で持つ)**: 現在、`grid_item_input_allowed()` が偽のとき `handle_keyboard` は
`handle_stack_refresh_keyboard()` へ進み、Esc で無条件に `toggle_stack_mode()` を呼ぶ (`app.rs` 58965・59188 付近)。
スタック更新以外の理由 (構造操作・画像フォルダの ZIP 化・場所の移動待ち) でも Esc がスタック表示を切り替え、
旧フォルダを再読込して移動待ちを取り消し得る (履歴の待ちでも起きる既存の不具合)。`grid_item_input_allowed()` の
中身を「止める理由」の型付き値を返す関数にし (例: `grid_item_input_block() -> Option<GridItemInputBlock>`、
`grid_item_input_allowed()` はその `is_none()`)、スタック専用の操作 (Esc の切り替え) は理由がスタック更新のときだけ
受け付ける。理由による制限はスタック専用の操作 (Esc 等) に限り、同じ関数が今許している非項目の操作 (戻る/進むボタン、
ヘルプ・設定・操作カスタマイズ、最大化の切替等) はどの理由でも従来どおり通す。
r7: 範囲外の挙動を変えないため、Esc・スタック切替キーを止めるのは**理由が直接移動の走査待ちのときだけ**とし
(r8: 停止理由に直接移動の走査待ち専用の値を設け、履歴・レーティングの待ちの値と区別する)、
履歴・レーティングの待ち・構造操作・画像フォルダの ZIP 化中の既存の扱い (Esc で切り替わる) は変えない。
その既存の不具合は §7 に記録する。

**許す操作 (一覧の判定の外)**: メニューバーの「AI説明文を作成」「文字起こしを作成」は、受付時に選択中のファイルの
パスを捕捉して独立した仕事を始める (`ui_dialogs/caption_job.rs`・`transcript_job.rs`)。一覧の状態に依存せず、
移動待ちと所有権が交わらないので、履歴の待ちと同じく許す。

**規則 2 (判定の外の移動の入力は後勝ち)**: 新しい要求が走査待ちを退役させる。

| 新しい入力 | 仕組み |
| --- | --- |
| 別の直接移動・ツリーのクリック・検索中の場所ジャンプ | 新しい要求の受付が旧 pending を cancel (既存) |
| お気に入り・キーの nav・DFS 結果・アドレス欄・コンテキストの nav のうち一覧の判定の外で出るもの | **r8 変更**: 通常 nav 裁定の `higher_priority_than_folder_pane` による事前の cancel は、`DirectNavigation` には適用しない (`PaneNavigation` は従来どおり事前に cancel)。そのフレームでは `DirectNavigation` の完了を poll せず、後続の候補が**受理されたときだけ**退役させる。後続が拒否された場合 (表示対象外の ZIP の指定、snapshot 中など) は走査待ちが残り、後のフレームで採用される。**r9: 受理の判定は既存の受付境界 2 点に集約し、要求の種類を列挙しない**: (1) `claim_open_request_owner_with_startup_intent` が成功したとき (main context。open 系の要求はすべてここを通る。未キャッシュの PDF の `ColdCandidate`・書庫・同期のフォルダ読込を含む。書庫変換・未解決 startup open・競合 bookmark を取り消している既存の受付と同じ位置)、(2) `retire_replaced_main_list_requests` (open 以外の main 要求: 履歴・レーティング・コレクション・合成 surface 等。`retained == Pane` のときも `DirectNavigation` は退役させ、直接移動・ツリーの新しい走査の開始は従来どおり旧 pending を置き換える)。直接移動自身の完了の再入で claim が走るときは、pending は poll で取り出し済みなので自分を消さない。claim 成功の後で拒否される要求 (変換書庫の無視設定など) でも走査待ちは退役する: claim を受理とみなす既存の契約 (書庫変換等の取消) と同じ扱い |
| 履歴の戻る/進む、コレクション、レーティング、Quick Folder 切替、Remote 取得等の main 要求 | `retire_replaced_main_list_requests` が `cancel_folder_pane_open` (既存) |
| **DFS (Ctrl+↑↓ 等) の開始** | **追加 (受付点 1)**: `start_folder_nav` が新しい DFS を受理したとき (既存 DFS への累積は除く)、main の `DirectNavigation` の pending を `cancel_folder_pane_open` で cancel する (r7: ツリーの `PaneNavigation` は従来どおり触れない)。`cancel_pending_folder_nav()` は新しい DFS 用の lock・holdover まで消すので流用しない。detached 用の purpose・`RequiredFullscreenTarget`・`CurrentViewOrderRefresh` 等には触れない |
| **直接移動の受付 (DFS 実行中)** | **追加 (受付点 2)**: §4.2 の 3 |
| **保存済みの本・コレクションを開く** | **変更 (受付点 3)**: 現在は `saved_group_other_main_nav_pending()` が pane pending を理由に「別の場所を開く処理が進行中です」で拒否する。受付 (`saved_group_open_source_if_allowed`) で、本棚更新中・行き先の未登録など**既知の拒否判定をすべて通した後**に、置換できる走査 (`DirectNavigation`) だけを例外として取り消して進める (r7: ツリーの走査待ちでの拒否は従来どおり)。拒否された操作は走査待ちを壊さない。共通 helper `saved_group_other_main_nav_pending()` は変えない (既存 saved-group が後発の pane scan に道を譲る契約 `ready_book_yields_candidate_to_later_pending_main_navigation` を保つ) |

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

- 既存の `render_location_navigation_wait_overlay` (「移動先を読み込み中…」) は `location_navigation_pending()` で
  描くので、直接移動の待ちだけが理由のときは、走査待ちが 500 ms を超えるまで描かない。履歴・レーティングの待ちの
  表示は変えない。開始時刻は pending に持たせ、猶予の判定と再描画の予約は既存の待機表示の描画に寄せる
  (別の表示を作らない)。
- 猶予の定数は本件専用に置く (sidecar 復元の猶予とは意味が違うので共有しない)。

## 5. 不変条件とテスト

1. 対象の入口 (§2.2.1 の各お気に入り・アドレス欄 Enter・最近開いたフォルダのメニュー) は、
   dispatch したフレームで `scan_directory_with_settings` を呼ばない。範囲外の入口 (フォルダを開くダイアログ・一覧の
   フォルダを開く・親へ・コレクションの物理元・コンテキストメニュー経由の container open) は従来どおり同期で採用され、
   その既存テスト (history_parent_1473・ratingnav649・same_position_double_click・section1328 等) が**元の待ち方・期待値のまま**通る。テスト用の計数 (cfg(test) の hook か counter) で
   UI スレッドの同期走査が 0 回であることを確かめる。
2. worker 完了後の poll + 裁定で、従来の同期経路と同じ一覧・選択・履歴 (Direct の push 1 回)・address が採用される。
3. 拒否: snapshot 範囲外・file-type visibility の拒否は受付時に起き、既存の owner (startup open、履歴遷移、rating、
   collection 等) を壊さない。
4. 規則 1: 直接移動の走査待ちのあいだ `grid_item_input_allowed()` が偽で、一覧のフォルダを開く・画像を開く
   (ZIP/PDF の構成確認待ちを経る open を含む) 操作が何も起こさない。走査が終われば項目入力が戻る。
   ツリーの走査待ち (`PaneNavigation`) では従来どおり項目入力を受け付ける。直接移動の走査待ちの間に
   Esc を押してもスタック表示が切り替わらず、走査待ちが保たれる。スタック更新中・履歴の待ち等の Esc は従来どおり。
   直接移動の走査待ち中でも、ヘルプ・設定・操作カスタマイズ・最大化の切替・戻る/進むは従来どおり使える。
   メニューの「AI説明文を作成」「文字起こしを作成」は走査待ち中も選択中のファイルで受け付ける。
5. 規則 2 の置き換え: 直接移動 A → B、直接移動 ↔ ツリー、直接移動 → 履歴の戻る、直接移動 → キー nav、
   DFS → 直接移動 (旧 DFS の結果を採用しない)、直接移動 → DFS (完了した直接移動が新しい DFS を消さない)、
   saved-group open (既知の拒否を通った後に取り消す)、saved-group の受付拒否 (走査待ちは保たれる)、
   saved-group → 後発の pane scan (既存契約のまま saved-group が譲る)、Quick Folder A→B→A、Remote 取得。
   ツリーの走査待ち中の DFS 開始・saved-group の扱いは従来どおり (変えていないことを確かめる)。
   最後の要求だけが採用される。DFS の退役で fs nav lock・holdover が残らないこと
   (結果取り出し後・同じフレームの境界を含む) も確かめる。
6. 待機表示: 500 ms 未満の走査では「移動先を読み込み中…」を描かず、500 ms を超えたら描く。
7. stale: 走査中に別の採用で一覧が変わったら、完了しても採用しない。
8. (r8 で削除: 全画面予約の移送はしない)
8a. 拒否された後続: お気に入り A の走査中に、拒否される入力 (表示対象外の ZIP をアドレス欄で指定等) を実際の裁定で
   与えても A の走査待ちは残り、後で A が採用される。ツリーの走査待ちは従来どおり事前に取り消される。
8b. 受理された後続: 実際の裁定で「お気に入り A の走査中 → アドレス欄から未キャッシュの B.pdf を受理 → B の列挙待ちの間に
   A の worker が完了」としても A は採用されず、B が採用される (claim 成功で A が退役)。履歴の戻る (retire 経由) も同様。
9. 対象外の型 (ZIP/PDF、`.epub`/変換書庫、main 以外の context) は従来の経路のまま。
10. (r7 で削除: CollectionGridPhysical は範囲外)
11. 走査失敗で一覧を変えず、通知とログを出す。
12. test-script の要求数が受付時 1 回のまま (取消された受理済み要求も数える)。
13. 既存テストのうち、対象の入口の直後に同期採用を前提にしていたものだけ、worker 完了 + 裁定を回す helper を通す
    形に直す (挙動の期待値は変えない)。範囲外の入口のテストは書き換えない。

独立実装レビューP3対応（2026-11-18）: 完了helperは `post_render_main_navigation` を通す。
ready済みの直接移動と同じフレームのお気に入り・アドレス欄・ツリー入力の競合、および
実DFS workerの結果を実際の `poll_folder_nav` で取り出した後の置換を回帰テストに含める。

## 6. 文書の更新

- [ui-responsiveness.md](ui-responsiveness.md): 本件の節 (上部の近年の節と同じ形) と §5 の表への 1 行。
- [async-architecture.md](async-architecture.md): 「物理フォルダ open 事前走査」の行と §718 付近の表に
  `DirectNavigation` と §4.4 の取消規則を追記。
- [detached-rework-plan.md](detached-rework-plan.md) §11 (r8: 範囲を r7 に合わせて書き直す): bundle 所有の `FolderOpenScanPurpose` に main 専用の
  variant を足したこと、`start_folder_nav` の取消が detached 用 purpose に触れないこと、detached context は
  経路を変えないことを記録する (設計レビュー r1 で「構造的変更」に AGREE)。

## 7. 範囲外 (後続候補)

- `dispatch_main_input_nav_immediate` の `Direct` / `CollectionSource` (全画面を閉じる即時経路) の同期走査。
- `load_folder` / `load_folder_with_scan(path, None)` の同期呼び出し元。
- main 以外の context の直接移動 (detached はリワーク側で扱う)。
- 一覧のフォルダを開く (Enter・ダブルクリック・ゲームパッド)・親へ (BS・親ボタン・ZIP からの BS)・コレクションの物理元・
  フォルダを開くダイアログの同期走査。worker 化するなら、Enter 直後の先行入力が走査待ちの間に捨てられる扱いを
  先に決める (退行になり得る。§3.2)。リリース済みの挙動の変更なので利用者に相談する。
- 履歴・レーティングの待ち、構造操作・画像フォルダの ZIP 化中に Esc がスタック表示を切り替える既存の不具合
  (`handle_stack_refresh_keyboard` が停止理由を見ない)。本件では直接移動の待ちだけを型付きの理由で除いた。
- `GridFolderCandidate` の完了で、採用が拒否されたときに全画面予約が残り得る既存の漏れ (実装レビュー r1 で判明)。
- ツリーの走査待ち・DFS 実行中の旧一覧の項目入力にも規則 1 を適用するか (リリース済みの挙動の変更なので相談)。
  現在はツリー・DFS の走査待ち中に旧一覧を操作でき、r1〜r3 で挙がった割り込み (同じフレームの入力、画像を開く等)
  がそのまま起こり得る。
