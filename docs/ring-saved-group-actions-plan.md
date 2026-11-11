# §1.733 コレクション・本を開く操作をリング・ジェスチャー等へ割り当てる — 設計

- 状態: 実装・関連自動検証済み (2026-11-11)。全件・実機検証は未実施。ブランチ `feat-733-ring-actions`。
- 要望 (メール、2026-11-07): 「コレクション 1 を開く」などをマウスジェスチャーへ割り当てたい。
- 利用者決定: 追加する (2026-11-07)、次の版に入れる (2026-11-10)。

## 1. 現状 (コードで確認)

キー操作 (`KeyAction`、すべて Grid 文脈・既定キーなし) には次がある (`src/keymap.rs` の `SAVED_GROUP_ACTIONS`)。

- 本: `GridOpenBook1..20`、`GridBookPrev` / `GridBookNext`、`GridOpenActiveBook`、`GridManageBooks`
- コレクション: `GridOpenCollection1..20`、`GridCollectionPrev` / `GridCollectionNext`、`GridOpenCollectionTarget`、`GridManageCollections`
- `GridManageFavorites`

リング・マウスジェスチャー・マウスボタン (戻る/進む/ホイールクリック)・パッド X リングの割り当て先は
`RingActionId` (`src/ring_shortcut.rs`) で、上のどれも入っていない。お気に入りは
`OpenFavorite1..20` と `CycleFavorite` があり、場所移動の仲間 (`is_location_navigation_action`) として

- リング・ジェスチャー・パッド X リング: 一覧・画像の全画面・動画の全画面で候補に出る
- マウスボタン: 一覧だけ (`is_valid_for_mouse_button_context`)
- 一覧の更新中 (stale aggregate refresh) でも実行できる (`grid_ring_action_allowed_during_refresh`)

となっている。本・コレクションが出せない理由は、`RingActionId` に対応する ID が無いことだけで、
実行側 (`App::apply_saved_group_key_action`、`src/app/saved_group_actions.rs`) は入力経路に依存しない。

## 2. 追加する操作

| `RingActionId` | 保存名 | 表示名 | 対応する `KeyAction` |
| --- | --- | --- | --- |
| `OpenCollection1..20` | `open_collection_1`..`open_collection_20` | コレクション 1 … 20 | `GridOpenCollection1..20` |
| `CollectionPrev` / `CollectionNext` | `collection_prev` / `collection_next` | 前のコレクション / 次のコレクション | `GridCollectionPrev` / `GridCollectionNext` |
| `OpenBook1..20` | `open_book_1`..`open_book_20` | 本棚の本 1 … 20 | `GridOpenBook1..20` |
| `BookPrev` / `BookNext` | `book_prev` / `book_next` | 本棚順で前の本 / 本棚順で次の本 | `GridBookPrev` / `GridBookNext` |

- 詳細説明 (操作カスタマイズのホバー) は「コレクションの管理順で N 番目を開く」「本棚の管理順で N 番目の本を開く」
  「前/次のコレクション (本) を開く。端で循環します」程度。番号の意味はキーと同じ (管理一覧の全件、§keymap-spec)。
- 前/次の本は backlog の方針 (コレクション 1〜20・本 1〜20・前/次のコレクション) に無いが、同じ実行入口に乗るだけで
  費用がほぼ無く、片方だけ無いと不自然なので一緒に足す。
- 足さないもの: `GridOpenActiveBook` (追加先の本を開く)・`GridOpenCollectionTarget` (追加先のコレクションを開く)・
  管理画面 3 つ。要望の範囲外で、管理画面は全画面から開くと見えない (環境設定を全画面で出さないのと同じ理由)。
  必要になれば同じ形で後から足せる。
- 新しいキー操作は作らない (既存の `KeyAction` を使う) ので、`ini_name()`・`default_chords()`・`ALL_ACTIONS`・
  `docs/keymap.ini.default` は変わらない。

## 3. 使える場面 (お気に入りに合わせる)

新しい ID は、お気に入り 1〜20 と同じ判定を通す (場所移動の仲間に加える、または同じ規則の別述語を作り、
`is_valid_for_context` / `is_valid_for_mouse_button_context` / `available_for_context` で同じ扱いにする)。

- リング・ジェスチャー・パッド X リング: 一覧・画像の全画面・動画の全画面
- マウスボタン: 一覧だけ
- 編集モードのジェスチャー (`RightDragContext::EditMode`): お気に入りと同じ (除外しない)
- ページ一覧 (page overview) 中: お気に入りと同じく実行しない (既存の `page_overview_key_action` の対象外)
- 一覧の更新中: お気に入りと同じ扱い (`grid_ring_action_allowed_during_refresh` に加える)。キー側も
  `SAVED_GROUP_ACTIONS` を `grid_item_input_allowed` の前で処理しているので揃う。ただし更新中のホイールクリックは
  既存の更新中 handler (`handle_stack_refresh_keyboard`) が中ボタン短クリックを扱わないため、お気に入りを含む
  どの操作も今は発火しない。これは既存の制限で、この変更では直さない (別項目として報告する)。

## 4. 実行

1 つの入口に寄せる。`RingActionId` から対応する `KeyAction` を引く関数 (例: `saved_group_key_action()`) を置き、
`apply_ring_action` で `App::apply_saved_group_key_action` に渡す。本・コレクションの解決、待機ダイアログ、
中止、一覧更新中の扱いはキーと完全に同じ経路になる。

### 4.1 全画面から使ったとき

順序は 1 通り: 「受け付けの判定 → 全画面を最後まで閉じる → 閉じた後の状態で一覧と同じ入口」。
お気に入り (`apply_ring_favorite_slot`) の「判定 → `close_fullscreen` → 移動」と同じ並び。

1. 受け付けの判定: `apply_saved_group_key_action` の冒頭の拒否 (スナップショット中・復元処理中・別の場所を開く処理中・
   本棚の更新中・`smart_folder_source_lease()` が取れない) を helper に切り出し、キーとリングの両方から使う。
   拒否するときは全画面を閉じず、メッセージはキーと同じ。ここで取った lease は要求へ持ち越さない (判定にだけ使う)。
2. 全画面を最後まで閉じる: `close_fullscreen()` は動画の表示遷移中だと `TerminalClose` を予約して戻り、
   `book_append_undo_in_progress()` 中は何もしない。そのため終了処理を消化する既存の
   `close_fullscreen_to_completion(ctx)` (`src/app.rs`) を使う (非 Windows では `close_fullscreen()`)。
   閉じた後も `projected_viewer_session_open()` が真なら (閉じられなかった)、要求を作らずに
   「全画面を閉じられなかったため開けませんでした」の通知で終える。待ち・再試行は足さない。
3. `apply_saved_group_key_action` を呼ぶ。lease・現在の本・現在のコレクションはここで取り直す
   (閉じた後の一覧の状態が正本)。ここで拒否されたら全画面は閉じた後の一覧に通知が出る。

採用理由 (状態の組み合わせを減らす): 全画面を開いたまま本の走査・コレクション一覧の読み込みを待つと、
「全画面中に待機ダイアログ (メインウィンドウに出る) がある」「採用時に全画面が開いている」という組み合わせができ、
採用経路 (`adopt_saved_group_ready` → `load_folder_with_scan_owned`) が全画面中の読み込みを扱う必要が出る。
先に最後まで閉じれば、待機・採用は一覧から押したときと同じ状態だけを扱えばよい。
本の番号が未登録などで「開けない」と後で分かる場合も全画面は閉じた後になるが、本・コレクションは一覧の読み込みを
待たないと番号を解決できない場合があるので、読み込み済みかどうかで閉じる時期を変える分岐は作らない (2 通りの順序を持たない)。

- 複数ウィンドウ: 判定は既存の入口 (`smart_folder_source_lease()`) のまま。
  - 独立した context の別ウィンドウ (投影中の context がメインではない) では lease が取れず、閉じずに拒否する
    (キーと同じ「この操作はメインウィンドウでのみ使用できます」)。
  - 連動型の F12 別ウィンドウはメイン context を投影しているので lease が取れる。お気に入りと同じく、
    別ウィンドウの表示を閉じてメインの一覧で開く。メインの一覧がそのまま見えている状態なので、
    待機ダイアログもメインウィンドウに出る。お気に入りと挙動を揃えるのが目的で、detached 述語・viewport 経路・
    共通 lease の意味は変えない。
- パッドの同じバッチ: 本・コレクションの待機ダイアログが開いた後は、同じバッチの残りの意味操作 (B の親移動など) を
  実行しない (物理ボタンの release の状態更新は続ける)。バッチ冒頭で一度だけ受付を判定しているため、
  1 件ごとに受付を判定し直す形にする。待機ダイアログ専用の分岐ではなく、バッチ内でモーダルが開いたら止まる一般の規則にする。
- 動画の native presenter 経路: native 動画の全画面中は `src/app.rs` の早期 return 経路が `gamepad_nav` のうち
  `RatingSource`・`GridVirtual`・`Direct` しか処理しない。全画面を閉じた後のフレームでこの経路を通らないことを確かめ、
  通る場合は `CollectionOpen` も通常の navigation dispatcher と同じ処理へ渡す (落とさない)。

前提 (レビューで確認済み): メインウィンドウの全画面中は投影 context がメインで、一覧の source が変わらなければ
`smart_folder_source_lease()` は全画面表示に依存しない。通常の終了経路は新しく `folder_nav_pending` /
`smart_folder_transition` を始めない。

### 4.2 一覧から使ったとき

`apply_saved_group_key_action` をそのまま呼ぶ。返り値 (コレクションの一覧が読み込み済みなら
`AddressBarNav::CollectionOpen`) は、お気に入りの `AddressBarNav::Direct` と同じように `apply_ring_action` の返り値として返す。
呼び出し側 (リング・ジェスチャー・マウスボタン・パッド) がその nav を適用することを確認する。

## 5. 表示

- 操作カスタマイズの候補: `available_for_context` に加わるので、リング・ジェスチャー・パッド・マウスボタン (一覧) の
  候補に出る。並びはお気に入りの直後 (コレクション 1〜20、前/次、本 1〜20、前/次)。
- 一覧の行: `ring_bindings_for_key_action` (`src/ui_dialogs/preferences/pages.rs`) で `GridOpenCollection1` などの
  キー行に、対応するリング ID の割り当てを 3 文脈ぶん表示する (お気に入りと同じ)。これでリング専用の重複行は出ない
  (`ring_action_has_key_action`)。
- リング・ガイドの表示名は固定文言 (お気に入りと同じく、登録名は出さない)。
- UI 文言を足すので `python scripts/check_ui_glyphs.py` を回す。

## 6. 保存データ

- リング・ジェスチャー・マウスボタン・パッドの割り当ては `settings.db` の既存の設定 (リリース済み) に、
  文字列の ID として入る。新しい値を足すだけで、形式は変わらないので移行は不要。
- 移行は不要だが、旧版での割り当ての保持は別の話。未知の ID は読み込みで `Unknown` になり、sanitize で「なし」に
  置き換わる。版の保護 (`settings_db.rs`) は保存版が実行版より新しいときだけ働くので、公開時に版番号を上げれば
  旧版は保存を止める。同じ版番号の開発ビルド間 (このブランチと master のビルドを行き来する) では割り当てが消え得る
  (`docs/keymap-spec.md` の `GridColumnCount11..20` の段落と同じ)。
- 操作カスタマイズの書き出し/読み込み (`src/operation_customize_share.rs`) は同じ文字列 ID を使う。旧版で読み込むと
  版違いを警告して続け、未知の ID は sanitize で「なし」になる。共有形式の版は変えない。

## 7. まれな失敗

まれな失敗は既定の割り切り (ログ + 通知) による。新しい回復処理は足さない。

## 8. テスト

- `RingActionId`: 新 ID の `as_str` / `from_str` の往復、`KeyAction` との対応 (1〜20・前/次の全件)、
  文脈ごとの候補 (一覧・画像・動画でリングに出る、マウスボタンは一覧だけ、編集モードのジェスチャーで除外しない)。
- `apply_ring_action`: 一覧からコレクション 1 が `CollectionOpen` を返す、本 1 が待機要求 (`saved_group_open`) を作る。
- 全画面から: 受け付けたときは全画面が閉じてから要求ができる。スナップショット中・別の処理中は全画面を閉じずに拒否する。
- 一覧の更新中でも実行できる。ページ一覧中は実行しない。
- 操作カスタマイズ: `GridOpenCollection1` の行にリングの割り当てが出て、リング専用の重複行が出ない。
  ホバー説明は `ring_action_detail_label` (`pages.rs`)。
- 既存の全列挙テスト `ring_actions_are_classified_for_key_action_parity` (`src/keymap.rs`) の分類を更新する。
- 設定と共有設定の sanitize を通しても、新 ID の割り当てが全文脈で残る。
- 全画面: 動画の表示遷移中でも `close_fullscreen_to_completion` で閉じきれたら要求を作る。閉じるのに失敗した
  (`Err` が返る、または viewer が残る) ときは要求を作らない。`book_append_undo_in_progress` 中は別のケースとして、
  閉じずに要求を作らない。
- 複数ウィンドウ: 連動型の F12 別ウィンドウからは、要求ができてメインに待機ダイアログが出て、通常の後片付けの後に
  その表示のセッションと window の結び付きが終わる。独立 context の別ウィンドウからは、要求を作らず、その viewer と
  メインの一覧をそのまま残す。ほかの viewer が開いている場合も、その状態を変えない。
- パッド: X の release で本の待機を始めた同じバッチの B press が親移動を実行しない (handler-level)。
- native 動画の全画面から X リングでコレクションを開いたとき、`CollectionOpen` が落ちない。

## 9. 文書

- `docs/keymap-spec.md`: 本・コレクションの段落 (リング・ジェスチャー・マウスボタン・パッドにも割り当てられること、
  使える場面)、マウスボタンの行 (全画面では場所移動系と同じく出さない)。
- マニュアル: `shortcuts.html` (操作カスタマイズで割り当てられる操作の説明)、`tut-books.html`・`tut-collections.html`・
  `books.html`・`collections.html` の番号の説明、必要なら `gamepad.html`・`fullscreen.html` のリングの説明。
  版固有の説明・実装用語は書かない。

## 10. レビュー記録

- 設計レビュー 1 回目 (Sol / xhigh、2026-11-11): ACCEPT WITH CHANGES。P1×2・P2×3・P3×1。
  - P1 連動型の別ウィンドウでも lease が取れる → §4.1 の記述を直した。拒否の範囲は既存入口のまま (独立した context だけ拒否、
    連動型はお気に入りと同じく閉じてメインで開く) とし、レビュー案の「連動型も拒否」は採らなかった。理由: キー・お気に入りと
    同じ判定に揃えるのが要望の方針で、連動型ではメインの一覧が見えているため待機ダイアログも見える。detached 述語にも触れない。
  - P1 `close_fullscreen` が閉じきらない場合がある → `close_fullscreen_to_completion` + 閉じた後の確認に変えた。
    native 動画の早期 return 経路で `CollectionOpen` を落とさない要件を足した。
  - P2 パッドの同じバッチの後続操作 → 1 件ごとに受付を判定し直す要件を足した。
  - P2 更新中のホイールクリック → 既存の制限として範囲外にした (お気に入りも同じ)。
  - P2 全列挙テスト・ホバー説明の場所 → §8 に足した。
  - P3 旧版での保持 → §6 を直した。
  - 候補名: 本は「本棚の本 N」「本棚順で前/次の本」にした (全画面の一覧順の前/次と区別する)。
- 設計レビュー 2 回目 (同じセッション): ACCEPT WITH CHANGES。連動型を受け付ける判断は了承 (待機ダイアログが隠れる・
  状態が壊れる欠陥は無い。後片付けは既存の通常 cleanup が行う)。P2 §8 の動画遷移中の期待値の矛盾、P3 別ウィンドウの
  検証項目 → §8 を直した。

## 11. 実装記録 (2026-11-11)

### 実装と前提確認

変更ファイル一覧と主要な実装箇所 (15ファイル):

| ファイル | 設計項目・位置 |
| --- | --- |
| `src/ring_shortcut.rs` | §2/3/5/6: 番号表756、KeyAction対応1015、場所移動判定1270、候補順1286 |
| `src/app/saved_group_actions.rs` | §4: 共通受付311、キー入口370、一覧コレクション回帰715 |
| `src/app/gamepad_input.rs` | §3/4.1: 更新中1811、バッチ再判定4147、現在モーダル判定4434、リング実行8176 |
| `src/ui_dialogs/preferences/pages.rs` | §5: ホバー2777、キー行への対応2828、行表示回帰12499 |
| `src/keymap.rs` | §8: 既存parity全列挙の分類11803 |
| `src/operation_customize_share.rs` | §6/8: 共有importの全44件回帰647 |
| `src/app/tests.rs` | §8: 全画面・複数窓・native動画回帰114404以降 |
| `docs/keymap-spec.md` | §9: 本・コレクションとマウスボタンの仕様 |
| `docs/key-customization-impl-plan.md` | §9: 操作対応・受付の保守記録 |
| `docs/ring-saved-group-actions-plan.md` | 実装・検証記録 (本節) |
| `htdocs/mimageviewer/manual/shortcuts.html` | §9: 割り当て方法と使える場面 |
| `htdocs/mimageviewer/manual/tut-books.html` | §9: 管理順・前後移動・割り当ての案内 |
| `htdocs/mimageviewer/manual/tut-collections.html` | §9: 管理順・前後移動・割り当ての案内 |
| `htdocs/mimageviewer/manual/books.html` | §9: 番号の意味と使える場面 |
| `htdocs/mimageviewer/manual/collections.html` | §9: 番号の意味と使える場面 |

- `src/ring_shortcut.rs`: 44 ID の保存名・固定表示名・既存 KeyAction との双方向対応を追加。
  番号は本・コレクションそれぞれの配列定数で保持し、候補と場所移動判定をお気に入りと共有する。
- `src/app/saved_group_actions.rs`: 既存受付を `saved_group_open_source_if_allowed` へ抽出。
  管理画面の受付順は維持し、番号・前/次はキーとリングから同じ受付・要求 owner を使用する。
- `src/app/gamepad_input.rs`: 全画面を閉じる前の受付、既存 terminal close、閉じた後の再受付の順に接続。
  終了の Err または viewer 残存時はログと通知だけで終え、要求・再試行・回復処理を作らない。
  一覧更新中も許可し、ページ一覧の既存有限集合は広げない。
- 同ファイルのパッドバッチは各イベントと analog 操作の前に受付を再判定する。
  モーダルが開いた後の意味操作を止め、物理 press/release/axis/session end の配送は維持する。
  Grid 側の keyboard owner は pass 内で cache されるため、受付では現在の `common_modal_dialog_open()` も直接読む。
  回帰は owner を先に cache してから実 dispatcher に入力し、cache の再計算に頼らず後続操作を止めることを検査する。
- `src/ui_dialogs/preferences/pages.rs`: キー行への3文脈の割り当て表示と管理順・循環のホバー説明を追加。
  `ring_action_has_key_action` の既存規則でリング専用の重複行を除く。
- `smart_folder_source_lease` は投影 context と一覧 source/generation/quick slot から作り、全画面の有無を見ない。
  連動 F12 は main context のため取得でき、独立 context は既存判定で取得できない。
  通常の terminal close は新しい `folder_nav_pending` / `smart_folder_transition` を開始しない。
  確認箇所: `src/app/smart_folder.rs:2148`、`src/app/snapshot_ops.rs:2264`、`src/app.rs:82086`・`82278`。
- `close_fullscreen_to_completion` は通常表示でも content teardown を実行し、動画遷移中は terminal effects を消化する。
  連動 F12 の session/window 結合は既存の `finalize_closed_active_detached_viewport` が終了させる。
  detached の述語・viewport 配送・共通 lease の意味は変更していない。
- native 動画の早期 return 判定はパッド配送後の `native_video_fullscreen_active_for_main_backdrop` による。
  この述語は `fullscreen_idx` を必須とし、終了成功時は None になるため、そのフレームで早期 return を通らない。
  `CollectionOpen` は通常の `post_render_main_navigation` へ届く。X release から実 dispatcher までを回帰で検査する。
  したがって native 早期 return 内の分岐追加は不要。リング・ジェスチャ・マウスの返却 nav も既存 caller が回収する。
  確認箇所: `src/app.rs:82119`・`100535`・`100762`、`src/app/native_video.rs:4826`、
  `src/app/update_phases.rs:1403`・`1567`。既存経路を変更せず、headless 回帰でも確認した。
- 保存形式・共有形式の版、既定キー、既定割り当ては変更なし。旧開発ビルド間の保持に関する §6 の注意も維持する。
- マニュアル5ページとキー仕様・実装プランを更新。製品ページのカードの主張は変わらない。
  UI への新しい同期 I/O は追加していない。

### 検証

追加した回帰テスト (17件):

| 対象 | テスト名 |
| --- | --- |
| ID・全件対応 | `saved_group_actions_round_trip_and_match_existing_key_actions` |
| 候補・文脈・順序 | `saved_group_candidates_follow_favorites_and_share_location_context_rules` |
| 設定保存・sanitize | `saved_group_assignments_survive_settings_serde_and_sanitize_without_new_defaults` |
| 共有設定 import | `saved_group_ring_actions_survive_shared_import_sanitize_in_all_contexts` |
| 操作行・ホバー | `saved_group_ring_assignments_share_key_rows_and_describe_management_order` |
| 一覧からコレクション | `saved_group_ring_collection_from_grid_returns_the_shared_navigation` |
| 一覧更新中・本の要求 | `saved_group_ring_book_request_is_allowed_during_grid_refresh` |
| ページ一覧 | `saved_group_ring_actions_do_not_execute_in_page_overview` |
| 同一バッチのモーダル | `gamepad_batch_stops_semantics_after_any_modal_but_delivers_releases` |
| 通常全画面 | `ring_saved_group_closes_fullscreen_before_creating_main_grid_request` |
| 動画遷移中 | `ring_saved_group_drains_video_terminal_effects_before_request` |
| 全画面を閉じる前の拒否 | `ring_saved_group_snapshot_and_other_navigation_refuse_before_close` |
| UndoAppend・close Err | `ring_saved_group_pending_append_undo_keeps_viewer_and_rejects_requests` |
| close Ok・viewer 残存 | `ring_saved_group_rejects_remaining_deferred_viewer_even_when_close_returns_ok` |
| 連動 F12・後片付け | `ring_saved_group_linked_f12_uses_main_lease_and_normal_terminal_cleanup` |
| 独立 context・兄弟保持 | `ring_saved_group_independent_context_refusal_preserves_main_and_sibling` |
| native X リング・通常 dispatcher | `ring_saved_group_native_video_x_release_delivers_collection_to_main_dispatcher` |

既存の `ring_actions_are_classified_for_key_action_parity` も新44 IDを含む分類へ更新した。

最終ソースの関連 nextest:

```powershell
cargo nextest run -p mimageviewer --lib -E 'test(/ring|saved_group|gamepad|keymap|operation|preferences|collection_navigation/)' --no-fail-fast
```

- **1,253件実行・1,253件成功**、3件 slow、15,987件 skipped (フィルタ・既定除外)。実行時間152.907秒、exit 0。
  ログ: `target/section733-related.log`。追加17件もすべて含む。
- 先行する焦点検証は51件中49成功・2失敗。追加テストのストア起動待ちとリングモード有効化が不足していたため、
  fixtureだけを修正した。修正2件の独立再実行は2件成功 (`target/section733-fixtures-final.log`)。
  製品の期待値は変更せず、最終の上記1,253件でも成功した。
- 初回テストビルドは TurboJPEG の並列 MSBuild が警告0・エラー0で失敗した。
  同じ CMake build を `--parallel 1` で実行すると成功した。根因は未確定で、製品・build script は変更していない。
  初回の本体testコンパイル後、テスト一覧取得が `0xc0000135` で0件のまま停止したため、
  指定された FFmpeg 6 DLL を `vendor/ffmpeg/bin` から `target/debug/deps` へ実体コピーして実行を再開した。
- 独立完成レビュー: Sol / xhigh、ソースレビュー ACCEPT・確定指摘なし。
  初回の自己点検で発見した keyboard owner cache の gap は上記の現在 modal 判定と実 dispatcher 回帰で修正し、再レビュー済み。
- `cargo fmt` (引数なし)、`cargo fmt --check`、`python scripts/check_ui_glyphs.py`、`git diff --check` 成功。
  UI危険グリフ0件。
- 最終 `cargo check -p mimageviewer --bin mimageviewer-core` 成功 (52.41秒、exit 0)。
  既存の警告は残る。ログ: `target/section733-check.log`。
- 焦点検証コマンド: `cargo nextest run -p mimageviewer --lib -E 'test(/saved_group|gamepad_batch_stops/)' --no-fail-fast`。
  fixture修正2件のコマンド: `cargo nextest run -p mimageviewer --lib -E 'test(/saved_group_ring_collection_from_grid|saved_group_ring_assignments_share_key/)' --no-fail-fast`。
- 依存物の環境対応: `cmake --build target/debug/build/turbojpeg-sys-f4a0b3122eba9190/out/build --target install --config Debug --parallel 1` 成功。
  確認用ビルドに先立つ `cargo build --profile dev-runtime -p turbojpeg-sys -j 1` も成功 (2分11秒)。
  ログ: `target/section733-runtime-native.log`。本体は通常の並列数でビルドする。
- `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` 成功、exit 0。
  TurboJPEGを先に準備したため、既存のMSBuildプロセスを止めずに待機だけ省略した。
  normal feature set、`dev-runtime` profileでcore・remote・EPUB worker・MCP adapter・music-video editorの5本を生成。
  FFmpeg 6 DLLとVCランタイムの配置、PE依存検査 (`runtime=4 pe=5`) も成功。
  ログ: `target/section733-build-dev.log`、検査報告: `target/vcrt-pe-reports/dev-runtime.json`。
  検証用coreは `target/dev-runtime/mimageviewer-core.exe`。起動はしていない。

全件 `test-lib-full.ps1` は設計担当が PC 全体のロックを取って実施するため、この worktree の実装担当は実行しない。
実アプリの起動・デスクトップ操作・コミットも実施しない。

### 引き継ぐ確認・既存制限

- 実装判断の未解決事項や、設計の主要な前提との矛盾はない。
- 一覧更新中のホイールクリックは、既存 handler が短クリックを扱わないため発火しない (§3)。
  お気に入りと同じ既存制限として残し、この項目では変更していない。
- 全件 gate は設計担当が実施する。実際の Windows 窓の見え方・focus・native presenter の実機確認は未実施。
  確認用 core の起動は利用者が行い、画像・native動画・連動F12で本／コレクション操作を実行した後、
  全画面が閉じてメイン一覧が開くこと、独立した別窓では閉じずに拒否することを確認する。

利用者の起動コマンド (このworktreeのルートから):

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

引数なしでは通常の `%APPDATA%\mimageviewer` を使い、実際の設定・データを更新し得る。
single-instance mutexも共有するので、先にインストール版・常駐tray版のmImageViewerを終了する。
