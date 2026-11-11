# §1.733 コレクション・本を開く操作をリング・ジェスチャー等へ割り当てる — 設計

- 状態: 設計 (2026-11-11)。ブランチ `feat-733-ring-actions`。
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
| `OpenBook1..20` | `open_book_1`..`open_book_20` | 本 1 … 20 | `GridOpenBook1..20` |
| `BookPrev` / `BookNext` | `book_prev` / `book_next` | 前の本 / 次の本 | `GridBookPrev` / `GridBookNext` |

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
- 一覧の更新中: 実行できる (`grid_ring_action_allowed_during_refresh` に加える)。キー側も
  `SAVED_GROUP_ACTIONS` を `grid_item_input_allowed` の前で処理しているので揃う。

## 4. 実行

1 つの入口に寄せる。`RingActionId` から対応する `KeyAction` を引く関数 (例: `saved_group_key_action()`) を置き、
`apply_ring_action` で `App::apply_saved_group_key_action` に渡す。本・コレクションの解決、待機ダイアログ、
中止、一覧更新中の扱いはキーと完全に同じ経路になる。

### 4.1 全画面から使ったとき

順序は「受け付けの判定 → 全画面を閉じる → 一覧と同じ入口」。お気に入り (`apply_ring_favorite_slot`) の
「判定 → `close_fullscreen` → 移動」と同じ順。

1. 受け付けの判定: `apply_saved_group_key_action` の冒頭の拒否 (スナップショット中・復元処理中・別の場所を開く処理中・
   `smart_folder_source_lease()` が取れない = メインウィンドウ以外) を helper に切り出し、キーとリングの両方から使う。
   全画面のまま拒否し、メッセージはキーと同じ。
2. `projected_viewer_session_open()` なら `close_fullscreen()`。
3. `apply_saved_group_key_action` を呼ぶ (ここでも同じ判定を通る。閉じたことで状況が変わった場合はここで拒否される)。

採用理由 (状態の組み合わせを減らす): 全画面を開いたまま本の走査・コレクション一覧の読み込みを待つと、
「全画面中に待機ダイアログ (メインウィンドウに出る) がある」「採用時に全画面が開いている」という組み合わせができ、
採用経路 (`adopt_saved_group_ready` → `load_folder_with_scan_owned`) が全画面中の読み込みを扱う必要が出る。
先に閉じれば、待機・採用は一覧から押したときと同じ状態だけを扱えばよい。
本の番号が未登録などで「開けない」と後で分かる場合も全画面は閉じた後になるが、お気に入りでも
未登録の判定は閉じる前にしかしておらず、本・コレクションは一覧の読み込みを待たないと番号を解決できない場合があるので、
読み込み済みかどうかで閉じる時期を変える分岐は作らない (2 通りの順序を持たない)。

- 複数ウィンドウ: detached viewer の中で使うと、投影中の context がメインではないので 1. で拒否され、
  detached viewer は閉じない (キーと同じ「この操作はメインウィンドウでのみ使用できます」)。detached 述語・viewport 経路は触らない。
- 動画の全画面: `close_fullscreen` は遷移中なら `TerminalClose` を出して後で閉じる。お気に入りと同じ扱いで、新しい待ちは足さない。

前提 (実装者がコードで確認する): メインウィンドウの全画面中は `projected_viewer_context_id() == viewer_context_main()`
で、`smart_folder_source_lease()` が一覧と同じ値を返すこと。違う場合は 1. の判定をどう組むかを設計担当へ返す。

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
- 旧版で読むと未知の ID は「なし」になり得る (`docs/keymap-spec.md` の `GridColumnCount11..20` の段落と同じ)。
  公開時に版番号を上げれば旧版は `Incompatible` で保存を止める。
- 操作カスタマイズの書き出し/読み込み (`src/operation_customize_share.rs`) は同じ文字列 ID を使う。

## 7. まれな失敗

まれな失敗は既定の割り切り (ログ + 通知) による。新しい回復処理は足さない。

## 8. テスト

- `RingActionId`: 新 ID の `as_str` / `from_str` の往復、`KeyAction` との対応 (1〜20・前/次の全件)、
  文脈ごとの候補 (一覧・画像・動画でリングに出る、マウスボタンは一覧だけ、編集モードのジェスチャーで除外しない)。
- `apply_ring_action`: 一覧からコレクション 1 が `CollectionOpen` を返す、本 1 が待機要求 (`saved_group_open`) を作る。
- 全画面から: 受け付けたときは全画面が閉じてから要求ができる。スナップショット中・別の処理中は全画面を閉じずに拒否する。
- 一覧の更新中でも実行できる。ページ一覧中は実行しない。
- 操作カスタマイズ: `GridOpenCollection1` の行にリングの割り当てが出て、リング専用の重複行が出ない。

## 9. 文書

- `docs/keymap-spec.md`: 本・コレクションの段落 (リング・ジェスチャー・マウスボタン・パッドにも割り当てられること、
  使える場面)、マウスボタンの行 (全画面では場所移動系と同じく出さない)。
- マニュアル: `shortcuts.html` (操作カスタマイズで割り当てられる操作の説明)、`tut-books.html`・`tut-collections.html`・
  `books.html`・`collections.html` の番号の説明、必要なら `gamepad.html`・`fullscreen.html` のリングの説明。
  版固有の説明・実装用語は書かない。
