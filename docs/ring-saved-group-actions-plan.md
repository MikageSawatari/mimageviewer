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
