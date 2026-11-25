# 操作のスクリプト 設計計画 (§1.363)

ステータス: **設計 r9 (2026-11-25、設計担当 = ClaudeCode Opus 5.5)。利用者の判断 (§12) を反映済み。
独立レビュー (Sol xhigh) は r1〜r4 の REVISE を経て r5 で ACCEPT (§13)。実装の着手にあたり、今の master
(`16590597e`) のコードで前提を確かめ直し、入口と結果の受け渡し・右クリック・別ウィンドウとの関係を直した (r6、§14)。
r6〜r8 の独立レビュー (いずれも REVISE) を受けて、描画の中の入口の実行位置・結果の扱い・競合と利用資格を直した
(r7〜r9、§14)。r9 で ACCEPT WITH CHANGES (文言 1 件、反映済み)。実装中。** 実装ブランチは `script-363-impl`。
収録版は v5.7.0 の次の版 (利用者 2026-11-24、統括経由)。
要望の経緯は [backlog-on-hold.md](backlog-on-hold.md) の §1.363。

本書の事実はすべてコードと文書の参照による。製品バイナリは起動しておらず、実行時の挙動の観測は無い。
NeeView 側の使われ方は、利用者が別セッションで調べた要約の伝聞で、設計担当は確認していない。

利用者の判断 (2026-11-23、統括経由) は §12 に記録した。要点: レーティング・ピン留めタグは含め、
削除・移動・名前の変更は含めない (Q1)。キャプチャ保存は含め、書き出しは含めない (Q2)。
イベントは最初の版から入れる (Q3)。割り当ては操作の入力すべて + 右クリック (Q4)。
AI (MCP) に書かせる、§1.843 の後の段 (Q5)。第三者のスクリプトは説明と初回の確認だけ (Q6)。Rhai を続ける (Q7)。

---

## 0. 目的と範囲

キーなどの操作 1 つで、mIV の既存の操作を**続けて実行する**・**状況に応じて切り替える**。
NeeView のスクリプト機能 (`.nvjs`) の使われ方として挙がった例 (伝聞) を目安にする:

| 例 | 必要なもの | 本書 |
| --- | --- | --- |
| 最終ページでもう一度押すと次の本へ | 状態 (最終ページか) + 条件 + 本の移動 | 最初の版 |
| 全画面なら解除、通常なら終了確認 | 状態 (全画面か) + 条件 + 終了 | 全画面の解除だけ最初の版。終了は `KeyAction` ではなくメニューの `FileQuit` (`keymap.rs:3048`) なので後の段 |
| 画像と動画で操作を切り替える | 状態 (種類) + 条件 | 最初の版 |
| パス名で開き方向を自動設定 | イベント (全画面に入ったとき・ページ) + 状態 (パス) | 最初の版 (Q3) |
| PNG なら市松背景 | イベント (ページ) + 状態 (拡張子・今の背景) + 巡回操作 | 最初の版 (巡回で書く。値の指定は §9 の後の段) |
| 決まったフォルダへワンタッチ保存 | キャプチャ保存 (保存先は環境設定のもの) | 最初の版 (Q2、押したときだけ) |
| ★を付けて次へ | レーティング + ページ送り | 最初の版 (Q1) |
| 閲覧中の本を移動して次の本へ | ファイル移動 | 対象外 (Q1) |
| 2 ページ飛ばす (次のページを 2 回) | 移動を 2 つ | 対象外 (移動は 1 回の起動に 1 つ、§5.3) |

**作らないもの**: 任意のファイル操作・外部プログラムの起動・通信・設定値の直接書き換え・
mIV の外の状態の読み取り。スクリプトにできるのは「状態を読んで、許可リストの操作を選ぶ」ことだけ。
ファイルを作るのは、既存のキャプチャ保存 (保存先・名前は mIV の設定どおり、スクリプトは指定できない) だけ。

---

## 1. 既存の仕組み (コードから)

### 1.1 Rhai サンドボックス (ファイル名スタック)

[src/filename_stack_script.rs](../src/filename_stack_script.rs)、設計 [filename-stack-scripting-plan.md](filename-stack-scripting-plan.md)。

- `rhai = "1"` は既に依存にある (`Cargo.toml:516`、Cargo.lock では 1.27.0)。純 Rust で crt-static と両立する。
  `sync` feature は有効になっていない (スタック用はコンパイルと評価を同じ worker で行うので不要だった)。
  この設定では `AST` が `Rc` を持ち、スレッドをまたいで送れない (Rhai 1.27.0 `ast.rs:29`・`func/native.rs:38`、
  レビュー r2 P2-5)。
- `build_engine()` (`filename_stack_script.rs:171`) が `eval` と `import` を無効化し (`import` 無効でファイル読みの
  module resolver を封じる)、操作数・呼び出し深さ・式の深さ・文字列・配列・マップの上限を設定する。
  I/O の関数は登録しない。
- **ただし `Engine::new()` は標準のパッケージを全部読み込む。その中には、`std::thread::sleep` を直接呼ぶ `sleep`
  (`LanguageCorePackage`、`packages/lang_core.rs:137`・`158`、レビュー r2 P1-1) がある。また `print` / `debug`
  (`BasicStringPackage` が登録、`packages/string_basic.rs:30`・`112`・`124`) は、エンジンの既定の callback で
  標準出力へ書く (Rhai 1.27.0 `engine.rs:322`、レビュー r1 P2-10・r3 P3-5)**。`sleep` は操作数の上限では止まらない。
  本機能のエンジンは標準のパッケージを丸ごと使わない (§6)。スタック用の同じ点は別件 §1.863 として
  master で直された (`c938e318f`: `sleep` と空文字列の `pad` を拒否し、`print` / `debug` をログへ送る)。
  スタック用のエンジンの作り方は本機能では変えない。
- 正規表現の補助 (`regex_*`) は、共有のキャッシュの mutex を持ったままコンパイルする (`filename_stack_script.rs:138`)。
  本機能では使わない (§7)。
- 契約は「入力 → 出力の純関数」。スクリプトは結果を返すだけで、適用の判断は Rust 側が持つ。
- 置き場所は `<data_dir>/stack_rules.rhai`。環境設定から「開く」「既定に戻す」。

本機能はこの形 (純関数・Rust 側が適用を持つ・失敗は通知して何もしない) を踏襲する。

### 1.2 キー操作の入口

- `KeyAction` は 595 個 (`src/keymap.rs:1448`)。文脈 (`KeyContext`、13 種)、`ini_name()` (設定に保存する
  安定した名前)、`trigger()`、`default_chords()` を持つ。
- egui の経路では、各ハンドラが keymap に押下を問い合わせる。問い合わせの関数は `consume_action` だけでなく、
  `consume_action_press_count` / `consume_page_turn_action` / `consume_action_press` /
  `consume_action_no_repeat` / 非消費の `pressed_action` などがある (`keymap.rs:8617`・`8653`・`8776`・`8813`・`8977`、
  レビュー r1 P2-4)。`KeyAction` → 実行関数の対応表は無い。
- 動画・音声の全画面 (native presenter) は別の入口。native の窓が受けた VK を `src/app/native_video.rs` が受け、
  前半で `resolved_action` (ナビゲーションまで、`native_video.rs:12639`) を解決し、後半は操作ごとに
  `Keymap::matches_vk_action` で実キーを照合し直す (例: `VideoCapture` は `native_video.rs:13358`)。
  `matches_vk_action` は実キーと実効割り当ての比較 (`keymap.rs:9329`)。native へ転送する操作は列挙式
  (`native_video_shortcut_action`、`keymap.rs:7760`) で、Global 全体ではない。
- 画面をまたぐ操作の前例 `ToggleAlwaysOnTop` は、一覧 (`input_main_grid_shortcuts`、`update_phases.rs:928`)・
  画像の全画面 (`handle_fs_key_input_with_operation`、`ui_fullscreen.rs:36063`)・native 動画 (前半の解決
  `native_video.rs:12701`、実行 `13026`) で、それぞれ明示的に処理している。
  `GlobalLocalSearch` の消費は main 側のショートカット処理 (`update_phases.rs:967`) だけ。
- **egui で描く動画・音楽の表示のキーは、画像の全画面のキー処理の中で処理される**。`handle_video_input`
  (`ui_fullscreen.rs:60437`) の呼び出し元は `handle_fs_key_input_with_operation` の中 (`36459`) だけで、
  音楽の表示のキーも同じ関数の中 (`36463` 以降)。結果も同じ `FsKeyAction`。独立した入口ではない (r6)。
- 画像の全画面のキー処理 `handle_fs_key_input_with_operation` は 3 か所から呼ばれる: `render_fs_body` の
  ROOT から転送されたキー (`ui_fullscreen.rs:28888`)、通常のキー (`29377`)、`render_fs_body` の外の ROOT 側の
  予備経路 `handle_fullscreen_root_key_input` (`34844`、呼び出しは `update_phases.rs:2403`)。予備経路は
  返った `FsKeyAction` を、`render_fs_body` の後処理と同じ移動の dispatcher
  `handle_fs_navigation_with_list_order_operation` へ渡し、`Rebased` では番号を取り直すだけで描画の準備は
  次の描画に任せる (`ui_fullscreen.rs:34926`〜`34975`)。ページ一覧の中のキーは別の処理 (`29134`) で、
  許可された操作の表 `FS_PAGE_OVERVIEW_ACTION_PRIORITY` (`keymap.rs:7717`) だけを受ける。
  Global 文脈に足せば各画面から自動的に問い合わせられる、という仕組みは無い (レビュー r1 P2-5)。
- キー以外の入口: リング・マウスジェスチャー・マウスの戻る/進む/中ボタン・パッド X リングは
  `RingActionId` (`src/ring_shortcut.rs:537`) を `App::apply_ring_action`
  (`src/app/gamepad_input.rs:8458`) で実行する。一部は対応する `KeyAction` の実行関数
  (`execute_copy_key_action`、`apply_saved_group_key_action`、`apply_pinned_tag_key_action` など) を呼ぶ。
  対応表は [ring-keyaction-parity.md](ring-keyaction-parity.md)。呼び出し元は、リングとジェスチャー
  (`execute_right_drag_command`、`gamepad_input.rs:3889`・`3917`。ジェスチャーは押下のフレームで実行せず、
  命令として持ち越して所有 context で実行する、`app.rs:70287`)、パッド X リング (`7948`)、マウスボタン
  (`dispatch_mouse_button`、`8127`)。native 動画の窓の XButton (`WM_XBUTTON`、`native_video.rs:16123`) も
  `dispatch_mouse_button` へ入る (ダブルクリックは 2 回分実行される、同ファイルのテスト `271`〜`296`)。
- **`apply_ring_action` が呼び出し元へ返すのは `Option<AddressBarNav>` だけ** (`gamepad_input.rs:8464`)。
  画像の全画面のページの移動・閉じる操作は、リングの側でその場で実行し終える (例:
  `handle_fullscreen_boundary_jump`、`8832`、`apply_ring_close_fullscreen`)。`FsKeyAction` を受ける呼び出し元は
  無い。egui の全画面のマウスの戻る/進むは、キー処理の中から `apply_mouse_back_forward_button` を呼び、
  `FsKeyAction` の `mouse_nav` として返す (`ui_fullscreen.rs:38455`)。
- **native 動画のキーの処理は、操作をその場で実行し終える** (例: 次のファイル `navigate_native_video_fullscreen`、
  `native_video.rs:13208`、閉じる `handle_fullscreen_close_request_immediate`、`12944`)。返す
  `NativeVideoKeyOutcome` (`native_video.rs:1108`) はログの分類で、呼び出し元の後処理は無い。
- 右クリックメニューは 2 段: Win32 のネイティブメニュー (`try_show_native_grid_context_menu`、
  `src/ui_dialogs/context_menu.rs:1257` → `dispatch_native_context_menu_result`、`1359`) を先に試し、出せない
  ときだけ egui のメニュー。項目は `MenuCommand` (`context_menu_model.rs:863`) / `MenuNode` で表す。どちらの経路も
  選んだ操作を実行してから idx を消す (一覧: egui `1223` → `1227`、native 後 `1183`。全画面: egui `2486` → `2490`、
  native `1375` → `2439`)。一覧には本のしおりの別メニュー (`show_bookmark_grid_context_menu`、`996`) もある。
- 割り当ての仕組みを持たない入力 (本機能の入口にしない): 全画面の左クリックのモード (§1.471、
  `FullscreenPrimaryClickMode`、`settings.rs:3480`)、3D 写真の焦点のクリック (§1.874、`plan_depth_focus_click`、
  `ui_fullscreen.rs:43302`)、右クリックの短押し (`ViewerShortRightClickAction`、`ring_shortcut.rs:182`)、
  ホイール、タッチ (`TouchCommand`)、ツールバー、パッドのボタン、全画面の「…」パネル、メニューバー。
  パッドの一部は native 動画へ VK を合成する (`dispatch_native_video_key`、`gamepad_input.rs:10226`) ので、
  そのキーへスクリプト操作を割り当てるとパッドからも起動する (既存の仕様どおり。トリガーの ←→ は
  repeat 付きで合成される、`5275`)。
- **別ウィンドウ (detached) の viewer context を取り付けている間も、同じ関数が走る**。
  `update_active_viewer_context` (`app.rs:70197`) の中で、パッド (`70227`)、持ち越したジェスチャー
  (`70287`)、全画面の描画 `render_fullscreen_viewport` (`70347`、中で `render_fs_body`・全画面のキー処理・
  `show_fs_context_menu`) が、別ウィンドウの context を取り付けた状態で実行される。今の context が
  メインかの既存の述語は `projected_viewer_context_id() == viewer_context_main()`
  (`viewer_context_registry.rs:5469`・`5460`。§1.733 の「この操作はメインウィンドウでのみ使用できます」の前例、
  `saved_group_actions.rs:415`)。一覧のショートカット処理 (`app.rs:103457`) はメインの pass だけで走る。
- **F12 の別ウィンドウ表示は、メインの context のままでも起きる** (r6 レビュー P2)。表示先の確定は context ID を
  交換せず、今の context に窓を結び付ける (`app.rs:94938`・`65554`)。結び付いた viewer はメインの一覧を投影する
  (`viewer_context_registry.rs:3644`)。今の context の viewer の表示先が別ウィンドウか、そこへ切り替え中かは
  `current_viewer_session_is_detached_or_switching` (`app.rs:65174`、`viewer_presentation` と
  `video_presentation_transition` の目標) で分かる。公開の `viewer_session_is_detached_or_switching` (`65193`) は、
  ほかに止まっている別ウィンドウの context があるだけでも真になる (`active_detached_context_is_at_rest`) ので、
  メインの一覧の利用資格には使えない。
- **全画面の描画の所有者 (`render_fullscreen_viewport_with_root_input`) は、1 フレームの移動の要求をローカル変数
  (`close_fs`・`page_nav`・`ctrl_nav`・`list_order_nav`・`sibling_nav`・`mouse_nav`・`jump_to`、
  `ui_fullscreen.rs:28536`〜`28545`) に集め、描画の後に 1 回だけ dispatcher
  (`handle_fs_navigation_with_render_perf`、`31518`) へ渡す**。描画の中の入力は、キー処理 (`29377`、結果は
  `FsKeyAction`)、ホイール・クリック・右ボタン (ジェスチャー・リング)・中ボタン (`handle_fs_wheel_and_click_frame`、
  `29454`、結果は `FsWheelFrameResult`)、右クリックメニュー (`show_fs_context_menu`、`30939`、結果は閉じるかどうか) の
  順に処理され、それぞれの結果がこの変数に合流する。ホイールの一部は「提案」として描画の後まで持ち越し、
  dispatcher の直前に適用する前例がある (`wheel_frame_proposal`、`31482`)。
  キー処理の `Rebased` は、描画の途中で素材と見開きの構成を準備し直す (`29394`)。メニューの後には、ポップアップが
  一時的に書き換えた表示設定を描画前の値へ戻してから適用し直す処理がある (`30553`・`30964` 以降)。
  描画の中で実行した操作の結果を描画の中だけで消費すると、この準備し直し・終了・設定の適用し直しと食い違う
  (r6 レビュー P1)。描画を抜ける (`frame_exited` / `frame_path_changed`) と dispatcher は呼ばれない (`31298`)。
- リングの一部は描画の中で実行し、移動の要求を `mouse_ring_nav` に入れ、表示先が変わったら
  `FsSecondaryInputOutcome::ViewerReplaced` でその入力の段を終える (`ui_fullscreen.rs:43537`〜`43546`)。
  別ウィンドウの viewer への右ドラッグの命令は持ち越して所有 context で実行する (`app.rs:3492`・`70287`)。
- `test-script` feature 限定の注入口 `test_script::consume_pending_action` (`test_script.rs:5010`) は、所有者・
  viewport・pass の照合と配送待ちを持つ。本機能はこれを使わない (§5.2)。

### 1.3 状態の変化

- 全画面の表示ページ `fullscreen_idx` への代入は製品コードで約 375 か所 (grep の件数)。
  「ページが変わった」を通知する単一の入口は無い。
- 見開きの切り替えは `normalize_spread_position` で表示単位の先頭へ移り、カーソルも動かす
  (`ui_fullscreen.rs:34539`・`47474`)。移り方は同期とは限らず、本の構成の確認へ委ねて戻る経路もある
  (`ui_fullscreen.rs:34563`、`book_composition_permission.rs:1994`、レビュー r2 P2-3)。見開きの操作は
  表示ページの識別を変え得て、遅れて移動を始めることもある。
- 画像の全画面のキー処理は、実行するだけでなく、呼び出し元へ結果 (`FsKeyAction`、`ui_fullscreen.rs:20239`) を
  返す。結果には移動の要求と `FsKeyPageTransition::{Unchanged, Rebased, Closed}` があり、呼び出し元はそれを
  受けて描画の素材・見開きの構成を準備し直し、移動の要求を後段の dispatcher へ渡す
  (`ui_fullscreen.rs:29170`・`31302`)。見開きの変更は、項目の番号が同じでも `Rebased` を返す
  (`ui_fullscreen.rs:37522`)。一覧の場所を開く操作も `Option<AddressBarNav>` を返す
  (`saved_group_actions.rs:488`)。実行関数を共用するときは、この「呼び出し元への結果」も共用する必要がある
  (レビュー r2 P1-2)。
- PDF のページは同じ `pdf_path` と別の `page_num` で表され (`grid_item.rs:76`)、既存のページの識別
  (`edit_source.rs:369` の `page_key_for_pdf` など) は両方を含む。
- 一覧のレーティング操作は、チェックした複数の項目へ当てた後でチェックを消す (`app.rs:80918`)。続くタグの操作の
  対象は、カーソルの項目が同じでも複数件から 1 件へ変わる (`tag_ops.rs:102`、レビュー r2 P2-6)。
- レーティングの変更は、レーティングで絞った場所では一覧の採用し直しで表示ページの移動・全画面の終了を起こし得る
  (`rating_membership_ops.rs:452`・`477`)。
- ZIP は列挙の worker が終わる前に `current_folder` を変える (`app.rs:40379`)。同じ ZIP の中の本・階層の切り替えは
  別の prefix 状態が持つ (`zip_tree.rs:514`・`559`)。パスの比較だけでは「開き終えた」を表せない (レビュー r1 P2-8)。
- 一覧の世代 `items_generation` は `set_items_generation` (`app.rs:45986`) で進む。

### 1.4 §1.843 (MCP) との関係

- §1.843 の利用者決定 (6) (2026-11-21) で、外部ツールとスタック用スクリプトも AI に変えさせる対象に入れた。
  原則として AI に変えさせないのは「AI 連携の許可そのもの」だけ、という方向で設計中
  (ブランチ `mcp-guide-843` の `docs/mcp-guide-settings-plan.md`)。
- 操作スクリプトも AI に書かせる (利用者決定 Q5、§6.2)。

---

## 2. 全体の形

```
入力 (キー / リング / ジェスチャー / マウスボタン / パッド / 右クリックメニュー)
  └ 「スクリプト操作 N」を受けた画面の処理
        │  (自動繰り返しの押下はここで拒否。メインウィンドウの context でなければ通知して拒否)
        ▼
  全画面の描画の中の入口 (キー・マウス・ジェスチャー・メニュー) は、ここで実行せず
  起動の要求を描画の所有 pass のローカル変数へ返す。所有 pass の末尾の 1 か所で次を呼ぶ (§5.2)
        ▼
  App::run_script_slot(ctx, 画面, N, 入口) -> ScriptRunResult (画面ごとの型)  ← 起動の受付は 1 つ
        │  最初に利用資格 (§3.2) を見る。編集モード等は通知なしで、別ウィンドウは通知して拒否
        │
        │  結果を、呼んだ場所の既存の後処理へ渡す (§5.2):
        │    全画面の描画の所有 pass → 移動の要求の変数に合流 (既に閉じた Closed は描画を抜ける)、
        │                              既存の dispatcher が 1 回だけ処理
        │    描画の前の入口 (ROOT 側の予備経路) → その経路の既存の手順
        │    一覧 → Option<AddressBarNav>、native 動画 → その場で実行し終える
        │  状態の写し (§4.3) を作る → Rhai: スロットの関数 (s) → 操作の並び (+ 短い通知)
        │  並びを全部検査 (§5.3)。1 つでも外れたら何も実行しない
        ▼
  同じフレームのうちに、並びを順に実行する
  (各操作は、キーで押したときと同じ実行関数を呼び、同じ形の結果を受け取る。§5.2)
        │  呼び出し元の後処理が要る結果 (描画の準備し直し・移動の要求・閉じた・遷移を委ねた) が出たら、
        │  そこで終わり、その結果を呼び出し元へ返す。対象が変わったときも残りを捨てる
        ▼
  呼び出し元は、キーのときと同じ後処理をする (「実行中」の状態は残らない)

毎フレーム 1 回 (キー処理の前): 識別を前回と比べる → 変わっていれば on_event(s) を同じ手順で実行
```

スクリプトは**操作を直接呼ばない**。状態を読んで「実行する操作の並び」を返すだけで、実行は Rust 側が行う。
1 回の起動は、そのフレームのうちに始まって終わる。

### 2.1 簡素化の検討 (CLAUDE.md「設計の簡素化」)

| 検討した手 | 採用 | 理由 |
| --- | --- | --- |
| スクリプトの中から操作を即時に呼び、結果を読んで続ける (NeeView 型) | **しない** | 操作の多くは非同期 (本を開く・全画面を閉じる・読み込み) で、呼んだ直後の状態は途中。待ち合わせ・中断・再開の状態が増える |
| スクリプトは「状態の写し → 操作の並び」の純関数にする | **する** | スクリプトが途中で止まる状態が無い。判断は実行前に 1 回で終わる |
| 並びを 1 フレームに 1 つずつ、キーの入口へ「押された」として注入する (r1 の案) | **やめた** | 注入が全部の問い合わせ関数・viewport・pass・native の 2 段の照合に届く必要があり、届いたかの判定も要る。複数フレームにまたがる「実行中」の状態ができ、利用者の入力 (パッド・ホイールを含む)・Remote・設定の変更との組み合わせを全部扱う必要が出る (レビュー r1 P1-1・P1-3・P2-4・P2-7) |
| 並びを同じフレームのうちに全部実行し、各操作はキーと共用の実行関数を呼ぶ | **する** | 「実行中」の状態が無くなり、途中に割り込む入力が無い。実行関数はキーの処理から切り出して共用するので、キーとスクリプトで別々の実装にならない (§5.2) |
| 呼び出し元の後処理が要る結果が出たら、並びの途中でもそこで終える | **する** | 後処理 (描画の準備し直し・移動の dispatcher) をスクリプトの中で真似しない。キーのときと同じ後処理へ結果を返すだけで、後処理を待って続ける状態を作らない (§5.2) |
| 移動 (ページ・本・全画面の開閉) を並びの中ほどや複数置ける | **しない** | 移動は「並びの最後に 1 つだけ」。同じフレームでの 2 回目の移動が、遅れて完了するページ送り (`page_turn/barrier.rs:400`) と組み合わさらない。A が遅れて移動を始める場合も上の「後処理が要る結果」で終わる |
| 各操作の後で識別を比べ、変わっていたら残りを捨てる | **する** | 写しを作った時点の対象と違うものへ、残りの操作 (レーティング等) が当たらない。比べるだけで状態は持たない |
| スクリプトを worker で走らせる | **しない** | 結果が返るまでに状態が変わり、写しが古くなる。保留・世代・キャンセルの状態が要る。短い計算なので UI スレッドで、小さな上限の中で走らせ、`perf::event` で測る (§7)。読み込みとコンパイルだけは worker (§4.1、`rhai/sync` が要る) |
| Rhai の標準のパッケージを丸ごと使い、危ない関数だけ外す | **しない** | 外し漏れ (`sleep`・`print`・文字列の `pad`) が版の更新でも起きる。文字列・配列・マップの関数は mIV 側で 1 つずつ書いて登録し、使える関数の一覧をテストで固定する (§6) |
| イベントから動くスクリプトに、ページ・本の移動を許す | **しない** | イベントの連鎖 (ページ変更 → 次のページ → ページ変更 …) が構造上起きない (§3.3) |
| イベントの種類ごとに受け口を分け、同じフレームの複数のイベントを順に走らせる | **しない** | 受け口は `on_event` 1 つ、1 フレーム 1 回。変わった種類は配列で渡す |
| キャプチャ保存の回数を時間や件数で数えて制限する | **しない** | 「1 回の起動で 1 つまで」「イベントからは不可」「自動繰り返しの押下では起動しない」で、数える状態を持たない (§5.3) |
| 別ウィンドウ (F12 / detached) でも動かす | **しない** | 対象をメインウィンドウとその全画面に絞り、別ウィンドウの viewer context を作らない・触らない (§3.2) |
| ファイルの変更を監視して自動で読み直す | **しない** | 環境設定の「再読み込み」・有効にしたとき・起動時だけ読む |
| 値を指定する新しい操作 (背景を市松に、など) を最初から作る | **しない** | 最初の版は既存の `KeyAction` だけ。値の指定は後の段 (§9) |

まれな失敗 (スクリプトのファイルが読めない等) は既定の割り切り: ログ + 通知。スクリプト操作はその回は何もしない。
回復の仕組みは作らない。

---

## 3. 呼び出しの入口とイベント

### 3.1 割り当て (利用者決定 Q4: 操作の入力すべて + 右クリック)

- `KeyAction::ScriptSlot1..ScriptSlot20` を追加する。文脈は **Global** の 1 組。同じキーがどの画面でも同じ
  スクリプトを呼び、画面ごとの違いはスクリプトが `s.context` で分ける。既定のキーは無し (`ChordList::EMPTY`)。
  `ini_name()` / `context()` / `trigger()` = Press / `default_chords()` / `ALL_ACTIONS` /
  `docs/keymap.ini.default` / `docs/keymap-spec.md` を揃える (CLAUDE.md のキー操作の規則)。
- Global に足しても自動では問い合わせられない (§1.2) ので、**問い合わせる場所を明示的に足す** (r6 で 3 か所に):
  1. 一覧: `input_main_grid_shortcuts` (`update_phases.rs` の `ToggleAlwaysOnTop` と同じ場所)
  2. 画像の全画面と egui で描く動画・音楽の表示: `handle_fs_key_input_with_operation` の
     `ToggleAlwaysOnTop` と同じ場所。egui の動画・音楽の表示のキーはこの関数の中で処理される (§1.2) ので、
     ここ 1 か所で覆う。この関数を呼ぶ 3 つの経路 (転送・通常・ROOT 側の予備) は、どれもここを通る
  3. 動画・音声の全画面 (native): `native_video_shortcut_action` の転送に `ScriptSlot*` を足し
     (native の窓が転送する和音は `install_global_native_video_shortcuts`、`keymap.rs:9381` で同じ関数から作られる)、
     `native_video.rs` のキー処理の前半の解決 (`ToggleAlwaysOnTop` と同じ場所) で `!key.repeat` のときだけ受ける
  受けたら `App::run_script_slot` を呼び、返った結果をその場所の既存の後処理へ渡す (§5.2)。
- **自動繰り返しの押下では起動しない**。egui の経路は `consume_action_no_repeat`、native は `!key.repeat`。
  repeat の情報が入口で分かるうちに拒否し、`run_script_slot` へは渡さない。
- 一覧と画像の全画面の Browser キー (VK `0xA6` / `0xA7`) は、フックで受ける
  `WM_KEYDOWN` / `WM_SYSKEYDOWN` の lParam bit 30 から repeat を判別する。方向ごとの押下件数と
  「繰り返しでない押下があったか」を同じ atomic に積み、一緒に取り出して割り当ての解決まで運ぶ。
  一覧の更新中の制限付きキー処理も同じ値を使う。割り当てが `ScriptSlot*` に解決された場合だけ、
  繰り返しの押下だけの入力を拒否する。通常の戻る/進むの割り当ては、繰り返しによる連続移動を維持する。
  native 動画の同じ Browser キーも `key.repeat` を同じ割り当ての解決へ渡す (スクリプトの実行の配線は段 3)。
  `WM_APPCOMMAND` は repeat のフラグを持たず、受けたメッセージごとに 1 回の押下として扱う。
  XButton の経路 (`WM_XBUTTON`、§1.2) も押下ごとに 1 回で repeat を持たない。
  ダブルクリックが 2 回分になるのは既存の仕様で、スクリプトでも 2 回起動する (既知の制限に足す)。
  押し続けの繰り返し (`tick_native_video_mouse_seek_holds`) は動画のシークの操作だけが対象で、`ScriptSlot*` には
  働かない (`native_video.rs:16138`)。
- `RingActionId::ScriptSlot1..20` を追加し、`saved_group_key_action` と同じ形で `KeyAction` へ対応付ける。
  リング・ジェスチャー・マウスの戻る/進む/中ボタン・パッド X リングは `apply_ring_action` から
  `run_script_slot` を呼ぶ。`ring-keyaction-parity.md` に行を足す。
- 右クリックメニュー: 一覧・画像の全画面・動画の全画面のメニューに「スクリプト」サブメニュー
  (名前の付いたスロットだけ並べる)。スロットが無ければ出さない。本のしおりの別メニューには出さない。
  メニューは Win32 のネイティブと egui の 2 経路がある (§1.2) ので、**両方に** `MenuCommand` の 1 つの variant
  (スロット番号を持つ) で足す。
  **全画面のメニューで選んだら、その場では実行せず、スロット番号をメニューの戻り値として描画の所有者へ返す**
  (r7)。メニューは通常どおり閉じ (idx を消し)、所有 pass の末尾で `run_script_slot` を呼ぶ (§5.2)。
  これで、メニューが開いたままだとキャプチャ・コピーの受付が拒否する点 (`ui_fullscreen.rs:37104`・`37122`、
  レビュー r3 P2-3) と、メニューの後の表示設定の戻し・適用し直し (§1.2) がスクリプトの変更を上書きする点の
  両方が起きない。一覧のメニューは、選んだ後に idx を消してから同じフレームで `run_script_slot` を呼び、結果の
  `Option<AddressBarNav>` を既存のメニューの移動の扱いへ渡す (一覧には描画の所有者の後処理の問題が無い)。
  どちらも次のフレームへ持ち越す状態は作らない。
- 操作カスタマイズの表示名は「スクリプト操作 N」+ スクリプトが付けた名前 (§4.2 の `label`)。名前が無ければ番号だけ。
- ツールバーには入れない (利用者決定 Q4)。

既知の制限 (repeat とは別の問題): native 動画の窓のマウスの戻る/進む (Browser キー) は、別の受け取りによる
重複が未解決とコードに書かれている (`src/video/native_window.rs:2534`〜`2537`)。この入口にスクリプト操作を
割り当てると、1 回の押下で 2 回起動することがあり得る。XButton のダブルクリックも 2 回起動する (上)。
キーで同じ入口にキャプチャ保存を割り当てた場合も同じで、スクリプト固有ではない。本機能では直さず、重複の
正規化は入力側の別件として扱う (統括へ報告済み)。
設計書・マニュアルでは「押した回数を超えない」と書かず、「1 回の起動で 1 枚まで、押しっぱなしやイベントでは
保存しない」と書く。

### 3.2 動く画面

メインウィンドウの「一覧 (Grid)」「画像の全画面 (FsImage)」「動画・音声の全画面 (FsVideo、音楽の表示を含む)」だけ。

**利用資格は 1 つの関数で決める** (r7、r6 レビュー P2-2・P2-3)。`run_script_slot` の冒頭 (Rhai を評価する前) と、
イベントの検出の冒頭 (§3.3) の両方がこれを呼ぶ。入口は、自分がどの面から来たか (一覧 / 全画面の viewer) を渡す。
結果は「使える (面)」「通知なしで拒否」「通知して拒否」の 3 つ:

1. 今取り付けている context がメインでない (`projected_viewer_context_id() != viewer_context_main()`) →
   通知して拒否「この操作はメインウィンドウでのみ使用できます」(§1.733 と同じ述語・文言)。
2. 全画面の viewer の面で、今の context の viewer の表示先が別ウィンドウか、そこへ切り替え中
   (`current_viewer_session_is_detached_or_switching`、§1.2) → 通知して拒否 (同じ文言)。F12 の別ウィンドウ表示は
   メインの context のままでも起きるため、1 の述語だけでは除けない。プロセス全体の判定
   (`viewer_session_is_detached_or_switching` の、ほかに止まっている別ウィンドウがあると真になる部分) は使わない。
   **一覧の面はこの条件を見ない**: メインの viewer が別ウィンドウに表示されている間も、メインウィンドウの一覧から
   の実行は許す。そのときの `s.context` は `"grid"` (面で決め、`fullscreen_idx` の有無では決めない)。
3. 編集モード (消しゴム・隠蔽・切り取り・SNS 分割・テキスト注釈・補正レイヤー、`is_overlay_edit_mode_active` 等)、
   ページ一覧、モーダルなダイアログ、文字入力欄のフォーカス → **通知なしで拒否** (使えないキーを押したときと同じ)。
   キーの経路はもともと編集モード・ページ一覧の分岐へ先に渡るが、ジェスチャー・中ボタンなどキー以外の入口は
   全画面の操作として届き得る (`ring_shortcut.rs:101`・`2008`、`viewer_chrome.rs:8`) ので、入口ごとの分岐に頼らず
   ここで止める。
4. mIV Remote が表示を取得している間、スクリプトが読み込み中・失敗・`operation_script_enabled` が OFF → 下の
   それぞれの扱い。

各操作の実行の直前の確かめ (§5.2、「受け付けられなければ残りを捨てて通知」) は、利用資格を通った後に
状態が変わった場合のためのもので、利用資格の代わりにはしない。
- mIV Remote が表示を取得している間 (Remote の入力はスクリプトを起動しない。取得中はイベントも検出しない。
  通知なしで拒否)
- スクリプトがまだ読み込み中・読み込みに失敗・`operation_script_enabled` が OFF (§4.1)。OFF は通知なし、
  読み込み中・失敗は押したときに通知する

#### 凍結の決まり (detached リワーク) との照合 (r6)

入口の関数とイベントの検出点 (`render_fs_body`・全画面のキー処理・`apply_ring_action`・右クリックメニュー) は、
別ウィンドウの context を取り付けている間も走る (§1.2)。したがって本機能は detached の経路に**触れる**。
CLAUDE.md「Detached viewer リワーク中のルール」と `detached-rework-plan.md` §2 (憲法) に照らした結果:

- 触れ方は、上の利用資格の関数 1 つ (今の context がメインか、今の viewer の表示先が別ウィンドウか・そこへ
  切り替え中か、を既存の述語で読む) と、それを呼ぶ 2 か所 (`run_script_slot` の冒頭と、イベントの検出の冒頭、
  §3.3) だけ。別ウィンドウの述語・viewport・窓の寿命・placement・HWND の捕捉・context の取り付けと bundle の
  入れ替え・表示先の遷移は変えない。
- 憲法 3 (App に detached 用の bool / Option を足さない): 本機能が App に足す状態 (読み込み結果の 1 つの値、
  イベントの基準、検出を済ませたフレームの番号、直近の実行時刻) は detached 用ではなく、メインの context の
  ためのもの。viewer context の bundle (`swap_field!`) には入れず、メインの context を取り付けているときだけ
  読み書きする (別ウィンドウの context の値と混ざらない)。
- 憲法 5 (時間窓で競合を吸収しない): 「1 秒に 10 回」の上限は競合の吸収ではなく、イベントの実行回数の保険
  (§3.3)。判定は同じフレームの事実 (今の context・ROOT のフレームの番号) で行う。
- 憲法 7 (ついでに直さない): native の Browser キーの重複 (上の既知の制限) は直さない。
- これは症状パッチ (guard・delay・retry・追加の repaint・一括 reset・silent fallback) ではなく、
  「メインウィンドウだけの機能」という所有の範囲を、既存の context・表示先・遷移の事実で表す構造的な決定である。
  独立設計レビュー (r6) でこの主張への合意を取り、`detached-rework-plan.md` §11 に記録する。実装の
  独立レビューでも同じ合意を取る。

### 3.3 イベント (最初の版から、利用者決定 Q3)

#### 種類

| 名前 | いつ | 識別 (この値が変わったら発火) |
| --- | --- | --- |
| `"open"` | 一覧の読み込みが完了し、採用された一覧の場所が前回と違うとき (起動時の最初のフォルダを含む) | 論理的な場所 (下の型付きの値)。読み込み途中の `current_folder` の変化は使わない |
| `"view"` | 一覧 ↔ 画像の全画面 ↔ 動画・音声の全画面 の切り替わり | `s.context` |
| `"page"` | 全画面の表示単位が変わったとき (全画面に入った直後を含む) | (論理的な場所, 表示単位を決める入力の型付きの組 (§5.2 の「(2) の作り方」), 分割ページの左右)。項目の識別は既存のページの識別 (`edit_source.rs:369` と同じ考え方: 通常のファイルはパス、ZIP は (ZIP, エントリ)、PDF は (PDF, ページ番号)) |
| `"select"` | 一覧の選択中の項目 (カーソル) が変わったとき | (論理的な場所, 選択中の項目の型付きの識別) |

- **論理的な場所は型付きの値**にする (レビュー r3 P2-4)。場所の種類ごとに、その場所を決める引数を持つ:
  通常のフォルダ (パス)、ZIP・PDF 等の中 (コンテナのパス, 中の prefix)、コレクション (`collection_id`。
  採用時は `current_folder = None` になる、`collection_grid.rs:3941`、`top_level_grid_view.rs:544`)、
  レーティングの場所 (★の数、`top_level_grid_view.rs:1483`)、本棚・読書履歴・ドライブ一覧などの固定の場所
  (種類だけ)、検索の結果 (検索の種類と条件の文字列)。どの種類があるかは、一覧の上位の表示の型
  (`top_level_grid_view.rs`) を実装担当が棚卸しして全部に対応させる。値は既存のメモリ上の状態から作る。
  一覧の世代・revision・表示位置は含めない (読み込み直しで発火させないため)。
- 同じ場所の読み込み直し (自動更新・再読み込み) では `"open"` は発火しない (場所が同じ)。別の場所へ行って
  戻ったときは発火する。
- 識別を既存の状態から取る具体的な方法 (読み込み完了の印、ZIP prefix、表示単位、分割の左右) は実装担当が
  コードで確かめ、取れない項目があれば設計担当へ返す。どれも読み込み・描画のために既にメモリ上にある値を
  読むだけにする。

入れないもの (理由):

- 起動時・終了時: 起動時は最初のフォルダの `"open"` で足りる。終了時は操作を流す意味が無い。
- 動画・音声の再生終了: イベントからは移動できない (下の決まり) ので、使い道が無い。
- キーの押下全般・マウス移動・タイマー・時刻: 発火が多すぎるか、利用者の操作と無関係に実行が続く形になる。
- 別ウィンドウ・mIV Remote の表示の変化 (§3.2)。

#### 受け口と検出

- スクリプトの `fn on_event(s)` 1 つで受ける (無ければイベントは使わない)。`s.events` に、そのフレームで
  変わった種類の配列が入る (例: 全画面で次の本へ移ると `["open", "page"]`)。
- **検出は、§3.2 の利用資格が「使える」pass だけで行う** (r6・r7)。使えなければ、比べも基準の更新もしない。
  別ウィンドウの context を取り付けている間、`fullscreen_idx` などは別ウィンドウの値 (§1.2) なので、読むと
  メインの基準が壊れるため。メインの viewer が別ウィンドウに表示されている間は、viewer の面の検出は止まり、
  メインウィンドウの一覧の面の検出 (`"select"`、`s.context = "grid"`) は続く。編集モード・ページ一覧の間も
  検出しない (抜けた後の最初の検出で、その間の変化がまとめて出る)。
- **検出と実行は、そのフレームで今の文脈を持つ画面の処理 (所有 pass) の、キー処理の前で行う** (レビュー r2 P2-4)。
  メインの update の位置では ROOT が今の viewport で、全画面の viewport は後の `show_viewport_immediate` の中で
  有効になる (`ui_fullscreen.rs:31067`)。egui の `viewport_id()` は今の viewport の stack から決まる
  (`vendor/egui/src/context.rs:752`) ので、`ctx` を取っておいて後で使うことはできない。そこで:
  - 一覧: ROOT の pass で、一覧のキー処理の前
  - 画像の表示と egui で描く動画・音楽の表示: 実際の描画処理 `render_fs_body` の、ROOT から転送されたキーの
    処理 (`ui_fullscreen.rs:28888`)・ページ一覧の処理 (`29134`)・通常のキーの処理 (`29377`) の**どれよりも前**の
    1 か所 (レビュー r3 P2-1)。所有者は、ウィンドウ内の静止画表示 (ROOT に直接描く) では ROOT、それ以外では
    全画面の子 viewport。転送されたキーで起きた変化は、ほかの入口と同じく次のフレームで検出される。
    ROOT 側の予備経路 (`handle_fullscreen_root_key_input`) は `render_fs_body` より前の ROOT の pass で
    キーを処理するので、その変化は同じフレームの検出で出ることがある。キーからの変化はいつ検出されても
    よい (下の「基準の更新」) ので、順序の保証は要らない
  - 動画・音声の全画面 (native): native の窓は egui の pass を持たないので、ROOT の pass で行う。操作できるかは
    既存の `NativeOverlayInputRouting` の snapshot で判断する (文字入力の所有は別の Context で判定されている、
    `native_video.rs:5865`)。動画の A の実行関数は VK を要らない形にする (§5.2)
  - egui で描く動画・音楽の表示: その表示の pass で、キー処理の前
- **検出は 1 フレームに 1 回まで**。「検出を済ませたフレームの番号」(u64 1 つ) を、イベントがあったかどうかに
  よらず記録し、同じフレームで別の所有 pass が来たら検出しない。フレームの番号は viewport ごとに別なので
  (`vendor/egui/src/context.rs:2261`)、ROOT の番号 `cumulative_frame_nr_for(ViewportId::ROOT)` に揃える
  (レビュー r3 P2-1)。同じフレームで文脈が変わった (一覧から全画面を開いた等) ときは、先に検出した側で
  そのフレームは終わり、変化は次のフレームの検出で `"view"` として出る。
- 前回の比較のときの識別 (基準) と今の識別を比べ、違う種類を集める。

#### 基準の更新 (r1 P2-7 対応)

- **キーなどから起動したスクリプト**が起こした変化は、通常の操作と同じに扱う。基準は動かさないので、
  次のフレームの比較で (同期で終わった移動も、遅れて完了した移動も) 発火する。
  移動の後に方向を決める、という使い方のため。
- **イベントから起動したスクリプト**は、実行の直後 (同じフレームのうち) に識別を取り直して基準にする。
  イベントの実行が同期に起こした変化 (見開きの正規化など) では発火しない。
- イベントの実行の操作が後のフレームで遅れて識別を変えた場合は、通常どおり発火する
  (どの起動が原因かを追う状態を持たない)。下の保険の上限で回数を抑える。

#### 連鎖を起こさない決まり

- **イベントからの実行が返せるのは、種別 A のうち「イベント可」の印のある操作だけ** (§5.1)。
  移動 (B・C)、レーティング・タグ (D)、キャプチャ保存・クリップボード (E) は返せない。返したらその回は
  何も実行せず通知する。
  - D と E をイベントから外す理由: ページを送るだけで、押していないのにレーティングが変わり、ファイルが増える
    形になる。利用者の決定 Q1・Q2 は「押したときに呼べる」ことの決定と読んだ (§12、統括へ報告済み)。
- 連鎖の深さは最大 1 段 (キー → 移動 → イベント → その場の操作だけ)。
- 保険の上限: イベントからの実行は 1 秒に 10 回まで。超えたフレームのイベントは捨てる (通知は起動中に 1 回、
  後で走らせ直さない)。持つのは直近 10 回の時刻だけ。

---

## 4. スクリプトの形と公開する状態

### 4.1 ファイルと読み込み

- `<data_dir>/operation_scripts.rhai` の 1 ファイル。内蔵の雛形 (`assets/operation_scripts.default.rhai`) を
  `include_str!` し、「スクリプトを開く」で無ければ書き出してから開く。既存の利用者ファイルは上書きしない。
- **ファイルの読み込み・コンパイル・`slots()` の評価は worker で行う** (起動時・有効にしたとき・「再読み込み」)。
  AST を UI スレッドへ送るため、`rhai` の `sync` feature を有効にする (§1.1、レビュー r2 P2-5)。UI スレッドで
  コンパイルし直す回避はしない。`sync` は既存のスタック用スクリプトの型にも及ぶので、スタック用のテストと
  計測テスト (`perf_10k_default_script`) を流し直して、遅くならないことを確かめる。
  結果 (コンパイル済みの AST・スロットの表・`on_event` の有無・エラー) を UI スレッドへ返す。状態は
  「読み込み中 / 使える / 失敗」の 1 つの値で、App が持つ。読み込み中に再読み込みが来たら、古い結果は捨てる
  (世代 1 つ)。UI スレッドで走るのは、スロットの関数と `on_event` の呼び出しだけ。
- 環境設定 (操作カスタマイズのタブ、または環境設定の 1 ページ。表示場所は実装時に既存の UI に合わせる) に:
  有効/無効、スクリプトを開く、再読み込み、既定に戻す、スロットの一覧 (番号・名前・関数名・読み込み結果)。
- 設定 `operation_script_enabled: bool` 既定 OFF、`operation_script_notice_confirmed: bool` 既定 false (§6.1)。
  新しい設定で、未リリースなので移行は不要。

### 4.2 雛形の例

```rhai
// どの番号にどの関数を割り当てるか。label は操作カスタマイズと右クリックメニューに出る。
fn slots() {
    [
        #{ slot: 1, label: "最後のページなら次の本へ", run: "next_or_next_book" },
        #{ slot: 2, label: "★3 を付けて次へ", run: "rate3_next" },
    ]
}

fn next_or_next_book(s) {
    if s.context != "fs_image" { return []; }
    if s.page.is_last { ["FsSiblingNext"] } else { ["FsPageNext"] }
}

fn rate3_next(s) {
    if s.context != "fs_image" { return []; }
    ["RatingItem3", "FsPageNext"]
}

// イベント (無ければ使わない)。s.events にそのフレームで変わった種類が入る。
// ここから返せるのは、その場で効く表示の操作のうち「イベント可」のものだけ (§5.1)。
fn on_event(s) {
    if s.context != "fs_image" { return []; }
    if ("view" in s.events || "page" in s.events) && s.book.path.contains("\\manga\\") {
        if s.view.direction != "rtl" { return ["FsReadingDirectionToggle"]; }
    }
    []
}
```

- 戻り値は操作名の配列。`#{ run: [...], notice: "…" }` の形なら、実行後に短い通知 (トースト) を出す。
  `()` と空配列は「何もしない」。
- 操作名は `KeyAction::ini_name()` の文字列 (設定にも保存している安定した名前) をそのまま使う
  (例の綴りは仮。実際の綴りは `ini_name()` を正とする)。
- 例の `on_event` は、全画面に入ったときと表示ページが変わったときに読む向きを確かめる。一覧では
  全画面の操作が使えないので何もしない (§5.3 の文脈の検査)。読む向きの操作が「イベント可」かは
  §5.1 の確認で決まる。可にならなければ、例を差し替える。

### 4.3 公開する状態

スクリプトに渡す `s` は読み取り専用のマップ。**メモリ上に既にある値だけ**を入れる
(ファイル・DB・索引を読まない、デコードを待たない)。まだ分からない値は `()`。
既存の取得関数がキャッシュに無いとき DB を読むもの (例: `get_rating`、`app.rs:79235`) は使わず、
キャッシュだけを見る取得関数を使う (無ければ足す)。

| キー | 型 | 内容 |
| --- | --- | --- |
| `api` | int | 状態と操作の版 (最初は 1) |
| `context` | string | `"grid"` / `"fs_image"` / `"fs_video"` |
| `events` | array | イベントで動いたとき、そのフレームで変わった種類 (§3.3)。キーなどから呼んだときは空 |
| `window.fullscreen` / `window.mode` | bool / string | 全画面か、全画面モードかウィンドウ表示か |
| `item.name` / `item.ext` / `item.path` | string | 選択中 (一覧) または表示中 (全画面) の項目。ZIP・PDF の中はコンテナ内の名前 |
| `item.kind` | string | `"image"` / `"video"` / `"audio"` / `"pdf_page"` / `"folder"` / `"archive"` など |
| `item.width` / `item.height` | int / () | 表示中の画像の大きさ (分かっているときだけ) |
| `item.rating` | int / () | キャッシュにあるときだけ |
| `item.pinned_tags` | array / () | 付いているピン留めタグの番号 (1〜20)。キャッシュにあるときだけ |
| `book.path` / `book.kind` | string | 開いている場所 (フォルダ・ZIP・PDF・EPUB 等) |
| `page.index` / `page.count` | int | 全画面の現在位置と総数 (一覧では選択位置と件数) |
| `page.is_first` / `page.is_last` | bool | 先頭・最後か (見開きでは表示単位で判断。既存の判定を使う) |
| `view.spread` / `view.direction` / `view.background` | string | 単ページ / 見開きなど、左から右 / 右から左、全画面の背景 |
| `video.playing` / `video.position_sec` / `video.duration_sec` | bool / float | 動画・音声のとき |
| `grid.selected_count` | int | 一覧で選んでいる数 |

- 文字列が上限 (§7) より長い値は `()` で渡す (長いパスはまれ。割り切り)。
- 入れないもの: 他の項目の一覧 (フォルダ内の全件)、タグ一覧、お気に入り・本棚・コレクションの中身、
  履歴、設定値、時刻、メタデータ (EXIF・AI プロンプト)。必要が出たら版を上げて足す (§9)。

---

## 5. 操作の実行

### 5.1 許可リスト (最初の版の案)

Rust 側の表に「操作・種別・イベント可か・実行関数」を持つ。表に無い操作は呼べない。新しい `KeyAction` を
足しても、表に足さない限りスクリプトから呼べない。

| 種別 | 意味 | 並びの中の位置 | イベントから |
| --- | --- | --- | --- |
| A: その場 | 表示・再生の切り替え | どこでも | 「イベント可」の印があるものだけ |
| B: 本の中の移動 | 同じ場所の中のページ・項目の移動 | 最後に 1 つだけ (B と C を合わせて 1 つ) | 不可 |
| C: 場面が変わる | 場所を離れる・全画面の開閉 | 最後に 1 つだけ (B と C を合わせて 1 つ) | 不可 |
| D: 利用者のデータを変える | レーティング・ピン留めタグ (Q1) | どこでも | 不可 |
| E: mIV の外に結果が残る | キャプチャ保存 (Q2)・クリップボードへのコピー | 並び全体で E は 1 つまで | 不可 |

最初の版に入れる操作の案 (名前は `KeyAction` の variant。実際の `ini_name()` は表を作るときに引く):

| 種別 | 文脈 | 操作 |
| --- | --- | --- |
| A | 画像の全画面 | 見開きの形 `FsSpreadSingle` `FsViewSpread` `FsSpreadLtr` `FsSpreadLtrCover` `FsSpreadRtl` `FsSpreadRtlCover`、読む向き `FsReadingDirectionToggle`、背景 `FsBgCycle`、合わせ方 `FsFitModeCycle`、`FsZoomIn` `FsZoomOut`、`FsToggleMetadata`、`FsNavigatorToggle`、`FsPixelGrid`、`FsPageTurnAnimationToggle` |
| A | 動画・音声の全画面 | `VideoPlayPause` `VideoMute` `VideoVolumeUp` `VideoVolumeDown` `VideoLoop`、シーク `VideoSeekStart` `VideoSeek{Back,Forward}{Small,Medium,Large}`、`VideoSeekStripToggle` |
| A | 一覧 | `GridColumnCount1..20`、`GridToggleDetailsView` |
| B | 画像の全画面 | `FsPagePrev` `FsPageNext` `FsJumpFirst` `FsJumpLast` `FsFixedJumpPrev` `FsFixedJumpNext` |
| B | 一覧 | `GridMoveFirst` `GridMoveLast` |
| C | 画像の全画面 | `FsSiblingPrev` `FsSiblingNext` `FsCtrlNavPrev` `FsCtrlNavNext` `FsClose` `FsBackToList` |
| C | 動画・音声の全画面 | `VideoPrevFile` `VideoNextFile` `VideoCloseFullscreen` |
| C | 一覧 | `GridOpenSelected` `GridParentFolder` `GridHistoryBack` `GridHistoryForward` `GridSiblingFolderPrev` `GridSiblingFolderNext` `GridBookPrev` `GridBookNext` `GridOpenFavorite1..20` `GridOpenBook1..20` `GridOpenCollection1..20` |
| D | 画像・動画の全画面 / 一覧 | `RatingItem1..5` `RatingItemClear` `RatingItemStepUp` `RatingItemStepDown`、`RatingContainer*`、`RatingBookPreferred*` |
| D | 一覧だけ | `GridTogglePinnedTag1..20` (既存コードで一覧専用、`gamepad_input.rs:8227`) |
| E | 画像・動画の全画面 / 一覧 | `FsCapture` `VideoCapture`、`FsCopyPath` `FsCopyFileName` `FsCopyImage` `GridCopyPath` `GridCopyFileName` `GridCopyImage` |

- **「イベント可」の印**は、A のうち「識別 (§3.3) を変えない」と実装担当がコードとテストで確かめた操作にだけ付ける。
  見開きの形の切り替えは `normalize_spread_position` で表示単位の先頭を動かす (§1.3) ので、印は付かない見込み。
  読む向き・背景・ズーム・表示の切り替え・再生の操作は付く見込み (未確認)。
- **実行関数を共用にできない操作は、最初の版から外す** (§5.2)。外したら設計担当へ報告し、表を直す。
  ズーム・列数・コピー・表示の切り替えは共用が現実的。移動・閉じる・見開き・レーティング (絞った場所での
  採用し直し) は、本体に加えて呼び出し元への結果の共用が要る族 (レビュー r2 の見立て)。
- 入れない操作:
  - 外部ツール (`ExternalTool*`)、外部プレーヤー、エクスプローラー連携 — 外部プログラムの起動
  - 削除・名前の変更・整理 (移動)・切り取り — 利用者決定 Q1
  - 書き出し・EPUB→PDF 変換 — ファイルを作る。キャプチャ保存以外は含めない (利用者決定 Q2)
  - 回転 (`FsRotateCw` など)・補正スロット・消しゴム・隠蔽の適用 — 項目ごとに保存される編集で、Q1 の範囲の外
  - タグの付与ダイアログ・タグビュー・お気に入り等の管理画面・環境設定・操作カスタマイズ — ダイアログ
  - 履歴・最近のフォルダ・検索履歴・保存位置の全件クリア — 利用者のデータを消す
  - 元に戻す (`UndoLastAction`) — 何を戻すかがスクリプトから見えない
  - 別ウィンドウの切り替え (`ToggleDetachedViewerMode`) — §3.2
  - 編集モード・ページ一覧の中の操作、押している間だけの操作 (`ModifierHold` / `KeyHold`)
  - AI 高画質化・フィルタの巡回 — 最初の版は数を絞る。要望を見て足す

### 5.2 実行方法 (キーと共用の実行関数を、同じフレームで呼ぶ)

- 表の各操作について、**キーの処理の本体を実行関数として切り出し、キーの処理とスクリプトの両方から呼ぶ**。
  キーの処理は「押下を問い合わせる → 実行関数を呼ぶ」だけになる。既に切り出されている操作
  (`execute_copy_key_action`、`apply_pinned_tag_key_action`、`apply_saved_group_key_action` など) はそれを使う。
  これはスクリプト用の二重実装ではなく、既存の処理の共用 (レビュー r1 P1-1 の指摘どおり)。
- native 動画は、VK の照合 (`resolved_action` / `matches_vk_action`) と、解決済みの `KeyAction` の実行を分ける。
  照合は VK の処理に残し、実行は `KeyAction` を受ける関数にする。egui で描く動画・音楽の表示も同じ実行関数を呼ぶ。
  切り出しは表の操作の分だけ行い、表に無い操作は触らない。
- **実行関数は、キーの処理が呼び出し元へ返しているのと同じ形の結果を返す** (レビュー r2 P1-2)。
  文脈ごとに 1 つの型にまとめ、`run_script_slot` は `ScriptRunResult` (文脈ごとの variant) を返す。案:
  - 画像の全画面 (egui の動画・音楽の表示を含む): `FsKeyAction` (移動の要求と `FsKeyPageTransition`) に、
    「拒否した」「既存の遷移へ委ねた」(本の構成の確認など、レビュー r2 P2-3) を区別できる形を足したもの
  - 一覧: `Option<AddressBarNav>` と「拒否した」
  - native 動画: 後処理の要る結果は無い。native のキーの処理は操作をその場で実行し終える (§1.2) ので、
    実行関数も同じく実行し終え、「実行した / 拒否した」だけを返す。`NativeVideoKeyOutcome` はログの分類として
    キーの処理の側で付ける (r6 で訂正)
  型の細部は実装担当がキーの処理の既存の結果から決める。新しい永続の pending は作らない。
- **入口ごとの結果の渡し先** (r6・r7)。どの入口も、その入口が今使っている後処理へ渡すだけで、スクリプト用の
  後処理は作らない:
  - **全画面 (egui) の描画の中の入口はすべて、その場で実行せず、スロット番号 (と入口の種類) を描画の所有者へ
    返す** (r7、r6 レビュー P1)。対象: 通常・転送のキー処理 (`FsKeyAction` に 1 つの欄を足す。マウスの戻る/進む・
    中ボタンがキー処理の中から `ScriptSlot*` に解決された場合も同じ欄)、ホイール・クリック・右ボタン・中ボタン
    (`FsWheelFrameResult` に同じ欄。ジェスチャー・リングが `apply_ring_action` で `ScriptSlot*` に解決されたら、
    `apply_ring_action` はスロット番号を返す形の小さな戻り値を持つ。実装の形は実装担当が決める)、全画面の
    右クリックメニュー (`show_fs_context_menu` の戻り値に同じ欄)。
    **実行する位置は、描画の所有 pass の末尾** (r8、r7 レビュー P2-2)。`render_fs_body` の、全画面の右クリック
    メニューとその後の表示設定の戻し・適用し直しの処理 (`ui_fullscreen.rs:30939`〜`31060` 付近) の後で、
    `show_viewport_immediate` のコールバック (またはウィンドウ内の静止画表示の ROOT の描画) を抜ける前の 1 か所。
    ここではまだ所有 viewport の `ctx` が有効なので、文字入力・IME を含む利用資格 (§3.2) と実行を、その pass の
    事実で判断できる。コールバックを抜けた後 (dispatcher の直前) では `viewport_id()` が ROOT を指すので使わない
    (`vendor/egui/src/context.rs:752`、§3.3 と同じ制約)。
    - **受けた要求の形**: 所有 pass のローカル変数 1 つに、スロット番号・入口の種類と、受けた時点の表示の識別を
      入れる。表示の識別は、(1) `page_turn_destination()` と `items_generation` (既存のジェスチャーの置き換えの判定、
      `ui_fullscreen.rs:43535`〜`43546` と同じ事実。context・窓・viewport と一覧の世代) と、(2) §3.3 の `"page"` の
      識別 (論理的な場所・表示単位を決める入力の型付きの組・分割ページの左右) の両方 (r9、r8 レビュー P1)。
      (1) だけでは同じ一覧の中のページ移動 (例: 同じフレームに中ボタンへ割り当てた `ImageHome` がその場で移動する、
      `ui_fullscreen.rs:41994`、`gamepad_input.rs:8830`) を見分けられないため。(2) は §3.3 の検出と同じ関数で作る。
      **(2) の作り方 (2b の実装で確かめ直し、2026-11-25)**: 表示単位の先頭を、組み立て済みの単位から読むことはしない。
      `fullscreen_idx` は単位の先頭とは限らず (例: 縦長 3 枚・左→右・`fullscreen_idx` = 1・ずらしの起点 1 で単位は [1,2]、
      読み方向を変えてずらしが外れると同じ 1 のまま [0,1])、組み立て済みの単位は未準備・失効中のことがあり、組み立て直すと
      同期の DB 読みに届き得るため。代わりに、**単位を決める入力のうちメモリ上の App の値** (今の項目の型付きの識別、
      見開きのモードと実効の読み方向、ずらしの起点の項目、1 ページ送りの起点の項目、分割ページの左右など。どれを含むかは
      実装担当が単位の組み立ての関数の入力を棚卸しして決める) を型付きの組にして識別とする。いつでも作れて、DB を読まない。
      入力が同じなら単位も同じなので、同じフレームの中の移動・ずらし・読み方向の変更は見分けられる。画像の大きさの判明
      (横長の判定) など、後から届くキャッシュの値による単位の組み直しは識別に含めない。これは同じフレームのキーの処理とも
      同じ扱いで、§3.3 の `"page"` でも「ページが変わった」とは数えない (既知の制限)。採らなかった案: 正確な先頭が
      取れないフレームでは通知して拒否する。ページ送りの直後や全画面に入った直後の押下という通常の操作で拒否が
      起きるため。
      実装で採用した入力は、現在項目・reader 順の項目列・ずらしと1ページ送りの起点を、それぞれ型付き項目識別へ写したもの、
      `spread_mode`・`reading_flow`・mode が持つ方向を優先した実効入力方向・page slice・自由回転による分割可否の境界、
      実効のページ単独設定と単独ページの端配置設定・紙めくり演出設定、連結読みを抑止する分析／比較／パノラマの App モード、
      grid 表示方式と、端ページの完全な本判定の入力 (検索欄・スタック要求・スタック表示・物理フォルダか) である。
      自動構成の入力には、完全な本の判定に使う行領域の件数 (通常は items 件数、DirectMedia は snapshot の元一覧件数、Smart root は採用済み rule 該当件数) と
      items 件数の上限も保持する。reader 順に出ない行の削除でも、完全な本かどうかと単位が変わり得るためである。
      最終ページの見開き補足は、構成 service の照会へ入らないよう、本別 preference と全体既定・採用済み表示元・ブラウザ保存先の設定・
      抽出面を生の App/settings 入力として保持する。この組は、実効値が同じでも入力の変更を検出することがある。
      採用済みの本の構成は既存の immutable projection の Arc を共有し、同じ allocation なら定数時間、異なれば scope・方向・表紙・
      complete・型付きページ順・各ページの override の意味内容を比較する。自動構成の行件数とは型付き source variant で分け、
      構成ありの本には自動構成の件数を含めない。行番号の付け直し・token/revision・準備済み回転値は比較しない。
      reader 順の写しは O(N) のメモリ走査と型付き項目の clone を要する。段2bでは要求を受けたときと末尾の照合時だけ作り、
      要求のないフレームには作らない。段4で同じ関数を毎フレーム使う費用は未計測で、段4へ引き継ぐ。
      寸法・保存回転・rating 等の後着 cache と、それらに依存する完全な本の証明・組み立て済み単位・準備や失効の token は含めない。
      1 フレームに 1 つ。2 つ目以降は捨てて通知する。
    - **捨てる場合** (どれも通知する。並び全体を実行しない): その入力の段が `FsSecondaryInputOutcome::ViewerReplaced`
      で終わった (今は移動の候補と operation を消すだけで描画は続く、`ui_fullscreen.rs:42920`。r7 レビュー P2-3)、
      末尾の時点で表示の識別 ((1)・(2) のどちらか) が受けたときと違う、ページ一覧へ切り替わった、描画を抜けた
      (`frame_exited` / `frame_path_changed`。末尾に届かない)、**そのフレームでホイールの提案が保留された** (下)。
    - **保留されたホイールの提案との関係** (r9、r8 レビュー P2)。複数のホイールの入力が混ざって移動を生むフレーム
      では、右ボタンの操作 (ジェスチャー・リング) を含む提案が保留され、コールバックを抜けた後 (`31482`) に
      適用される (`ui_fullscreen.rs:42264`〜`42300`)。所有 pass の末尾では、その提案が後で置き換え・ページ一覧への
      切り替え・移動を起こすかが分からない。そこで:
      - スクリプトの要求があるフレームでホイールの提案が保留されたら、要求を捨てて通知する
      - 保留された提案の適用の中で、ジェスチャー・リングが `ScriptSlot*` に解決されたら、実行せずに通知する
        (所有 pass の外なので実行できない)
      採らなかった案: 提案の適用を所有 pass の中 (`claim_fs_wheel_frame_after_ui`、`31249` の後) へ移し、結果を集めて
      からスクリプトを実行する (r8 レビューの例)。リリース済みのホイールの処理の順序を変えることになり、ホイールの
      挙動の退行を確かめる範囲が大きい。この理由でスクリプトの要求を捨てるのは「複数のホイールの入力が混ざって移動を
      生み、同じフレームにスクリプトの起動がある」組み合わせだけで、捨てても利用者のデータは変わらず通知が出る (押し直せば動く)。
      CLAUDE.md の「状態の組み合わせを減らす」に従い、組み合わせを扱わず捨てる。
    - **既存の移動との競合** (r7 レビュー P2-3): そのフレームで既に移動・閉じるが受け付けられているかを、
      所有 pass のローカルの移動の要求 (`close_fs`・`close_to_page_list`・`page_nav`・`ctrl_nav`・`list_order_nav`・
      `sibling_nav`・`mouse_nav`・`jump_to`)、ホイールの提案、帰還の継続 (`continuation_committed`、
      `pending_return_to_parent`)、リングが main の移動へ渡した `self.mouse_ring_nav` (`update_phases.rs:1218` で
      main が受け取る) を見て判断する。受け付けられているなら、**B・C を含む並びは、実行関数を呼ぶ前の検査で
      拒否する** (§5.3 の 7)。A・D・E だけの並びはそのまま実行する。
    - **結果の扱い**: 返った `FsKeyAction` の移動の要求を、所有 pass のローカルの移動の要求の変数に合流させ、
      描画の後の既存の dispatcher (`31518`) が 1 回だけ処理する。
      共通の合流は `FsKeyAction::merge_script_action`。ROOT の予備経路と描画前のリングは
      `run_fullscreen_script_slot_for_action` から呼び、段 2b の所有 pass の末尾でも同じ合流を使う。
      既存の移動・閉じる・`Rebased` を含む遷移は `navigation_already_accepted` で実行前に伝える。
      拒否・空の結果は既存の要求を変えず、既存の移動があればその要求と `navigation_operation` を保つ。
      新しくスクリプトだけが作った移動には元のキーの operation を付けない。
      - `Closed` は「既に閉じた」結果で、閉じる要求ではない (r7 レビュー P1)。レーティングの変更で表示対象が
        無くなった場合などは、専用の終了の処理を済ませてから `Closed` を返す (`ui_fullscreen.rs:37390`、
        `rating_membership_ops.rs:477`)。キー処理と ROOT 側の予備経路と同じく、そのフレームの移動の候補を捨て、
        dispatcher を呼ばずに描画を抜ける (`frame_exited`。`ui_fullscreen.rs:29391`・`34929` と同じ)。閉じる要求
        (`close`) を足して dispatcher に通常の終了をさせない (Undo の履歴を消す `clear_meta_undo` が走るため)。
        既存の帰還の継続は変えない。
      - `Rebased` は、描画の後の dispatcher が使う番号 (`frame_snapshot`) を取り直すだけで、素材と見開きの準備は
        次の描画に任せる (このフレームの描画は終わっているので、準備し直す相手が無い)。
    ローカル変数なので、次のフレームへ持ち越す状態は無い。
  - 全画面の描画の前の入口: ROOT 側の予備経路のキー処理 (`handle_fullscreen_root_key_input`) は、そこで
    `run_script_slot` を呼び、返った `FsKeyAction` をその経路の既存の手順 (`Closed` で終わり、`Rebased` で番号を
    取り直し、`handle_fs_navigation_with_list_order_operation` へ渡す、`ui_fullscreen.rs:34926`〜`34975`) で処理する。
    パッド X リングなど描画の前に `apply_ring_action` を呼ぶ入口も、同じ手順を関数に切り出して共用する。
    どの呼び出し場所が描画の前か中かは、実装担当が呼び出し元をたどって一覧にし、設計書に足す
  - 一覧の入口 (キー・リング・マウス・メニュー): `Option<AddressBarNav>` を、その入口の今の戻り値・移動の扱い
    へ返す (`apply_ring_action` の戻り値の型はそのまま)
  - native 動画の入口 (キー・XButton・リング): 実行し終えているので渡すものは無い
- `run_script_slot` は、検査 (§5.3) を通った並びを、呼ばれたその場で順に実行する。
  - 各操作の前に、文脈が写しのときと同じか、その画面でその操作を今受け付けられるか (キーの処理が実行関数を
    呼ぶ前に見ている条件と同じもの) を確かめる。受け付けられなければ残りを捨てて
    「スクリプト操作 N: 『…』を実行できませんでした」と通知する。
  - 操作の結果が「何も後処理が要らない」以外 (描画の準備し直し `Rebased`、閉じた `Closed`、移動の要求、
    `AddressBarNav`、遷移を委ねた) なら、**そこで並びを終え、その結果を呼び出し元へ返す**。残りがあれば捨てて
    通知する。呼び出し元はキーのときと同じ後処理をする (最後の B/C は、既存の移動の dispatcher へ渡るだけ)。
  - 一覧の共用の開く処理は、キー処理の早期 return (`ControlFlow`) と受付 (`ActionAdmission`) を分けて返す。
    既存の非同期の所有者へ要求を渡せた場合は受付、履歴の guard や参照先の不存在などの即時の拒否は拒否。
    Enter は従来の制御だけを使い、スクリプトは受付を確かめ、拒否なら完了の `notice` を出さない。
  - 後処理の要らない操作の後には、対象 (文脈・論理的な場所・表示単位または選択の型付きの識別・一覧の
    チェックした項目の集合) を写しのときの値と比べ、違えば残りを捨てて通知する。比べる値は
    `run_script_slot` の中のローカルな値で、状態としては持たない。
  - 待たない。次のフレームへ持ち越さない。
- **キーを続けて押したときと同じ扱い**: 遅れて完了する処理 (ページ送りの完了、レーティングの書き込みと絞り込み
  の採用し直し、本の読み込み) は、キーで同じ操作を素早く続けて押したときと同じ既存の経路で処理される。
  本機能はそれらを待ったり、結果を見て続けたりしない。
- 押したときに保存されるもの (見開きの設定など) はスクリプトでも保存される。

#### リング実行の呼び出し元と段 2a / 2b の境界 (2026-11-25)

製品コードの共通実行境界 `apply_ring_action` / `apply_ring_action_in_phase` へは、
`execute_right_drag_command_in_phase` のリング・ジェスチャーの 2 分岐から `apply_right_drag_action_in_phase` を介する経路と、
`dispatch_mouse_button`、履歴消去キーの共用入口 `apply_history_clear_key_action` から到達する。
テストからの直接呼び出しは下表の入力入口に数えない。各入口を所有者までたどった結果:

| 入力と呼び出しの列 | 時点 / 所有者 | 段 2a のスクリプト操作 | 段 2b | 後続の段 |
| --- | --- | --- | --- | --- |
| パッド X リング: `finish_gamepad_west_release` → `trigger_gamepad_ring_action` → `execute_right_drag_command_before_fullscreen_render` | 描画前。main update または取り付け済み context の更新 | 一覧は `Grid`、画像は `Viewer` で即時実行。画像の結果は `finish_fullscreen_key_action` に渡す | 変更なし | 動画・音声の実行は段 3 |
| 持ち越した別ウィンドウの右ドラッグ: `update_active_viewer_context` → `execute_pending_right_drag_command_in_mounted_context` → `execute_right_drag_command_before_fullscreen_render` | 描画前。既存の取り付け済み context | 共通の利用資格へ渡し、別ウィンドウの viewer は拒否。取り付け・寿命は変更しない | 描画内の実行へは流さない | — |
| 一覧の右ドラッグ: `ui_main` の `update_grid_mouse_ring_flick_at` → 位置付きリング / ジェスチャー更新 → `dispatch_right_drag_recognition` → `execute_right_drag_command` | 一覧の描画中。ROOT の一覧面 | `Grid` で即時実行し、戻った `AddressBarNav` を既存の `mouse_ring_nav` へ渡す | 変更なし | 一覧には全画面の後処理問題はない |
| 一覧の戻る / 進む / 中ボタン: `handle_keyboard` (一覧更新中は `handle_stack_refresh_keyboard`) → `apply_mouse_button` / `apply_mouse_back_forward_button_with_repeat` → `dispatch_mouse_button_with_repeat` | 一覧のキー処理中。ROOT の一覧面 | `Grid` で即時実行し、戻り値の `navigation` をそのまま返す。Browser キーの非repeatの事実を割り当ての解決まで渡す | 変更なし | 同上 |
| 画像・egui メディアの戻る / 進む: `handle_fs_key_input_with_operation` → `dispatch_mouse_button_with_repeat` | `render_fs_body` の準備後。または ROOT の予備キー処理と共用 | `MouseButtonDispatchResult.script_slot` を `FsKeyAction.script_slot` へ返す (Browser キーのrepeatだけなら要求を作らない)。ROOT の予備経路は描画前に実行して後処理する | 通常・転送キーと同じく `FsScriptFrame` に受け、所有 pass の末尾だけで実行する | メディアの操作本体は段 3 |
| 画像・egui メディアのリング / ジェスチャー: `apply_fs_secondary_input` → `execute_right_drag_command_for_fullscreen_render` | `render_fs_body` 内、または保留されたホイール提案の適用内 | その場では実行しない | スロットと受信時の識別を `FsWheelFrameResult.script_slot` へ返す。保留提案の適用内では通知して拒否する | メディアの操作本体は段 3 |
| 画像・egui メディアの中ボタン: ホイール / クリック処理 → `dispatch_viewer_middle_short_click` → `dispatch_mouse_button` | `render_fs_body` 内 | その場では実行しない | `FsWheelFrameResult.script_slot` から同じ所有者へ返す。ページ一覧の呼び出し元は実行しない | メディアの操作本体は段 3 |
| native 動画の Browser キー / XButton / 中ボタン: `native_video` → `apply_mouse_back_forward_button_with_repeat` / `dispatch_mouse_button` | native 入力処理。ROOT の更新 | 実行は未接続。Browser キーは `key.repeat` を渡し、ScriptSlot の繰り返しを拒否 | 返却欄のみ。native 側の実行は接続しない | 実行の配線は段 3 |
| native 動画のリング / ジェスチャー: 位置付き native 更新 → `dispatch_right_drag_recognition` → `execute_right_drag_command` | native 入力処理。ROOT、または既存の別ウィンドウ命令へ委譲 | 実行は未接続 | 変更なし | 段 3 |
| `apply_history_clear_key_action` → `apply_ring_action` | 一覧のキー処理 | 履歴消去操作だけの対応表であり、`ScriptSlot*` は到達しない | 変更なし | スクリプト入口にはしない |

`RingScriptPhase` は呼び出し時の区別だけで、App の状態ではない。既存の `apply_ring_action` /
`execute_right_drag_command` は描画内からも呼ばれるため、viewer のスクリプトを即時実行しない。
描画前と確認できる入口だけが `*_before_fullscreen_render` を呼ぶ。段 2b はこの区別を引き継ぎ、
描画中の要求を描画前の後処理へ流さない。

採らなかった代案:

- キーの入口へ注入して 1 フレームに 1 つずつ流す (r1 の案)。§2.1 の理由で取りやめた。
- リング・メニューからの起動を、次のフレームのキー処理まで持ち越して `FsKeyAction` で返す (r6 で検討)。
  持ち越しの状態ができ、その間の入力・context の切り替え・全画面の終了との組み合わせが増える。描画の所有者が
  既に 1 フレームの移動の要求を集めて最後に 1 回処理しているので、同じフレームのうちにそこへ返せば足りる (r7)。
- 描画の中の入口で、その場で実行して結果を ROOT 側の予備経路の手順で処理する (r6 の案)。描画の途中の
  準備し直し・メニューの後の表示設定の戻しと食い違う (r6 レビュー P1) ので取りやめた。
- `apply_ring_action` の戻り値の型を `FsKeyAction` も返せる形に変える (r6 で検討)。呼び出し元 4 か所の
  型と後処理を全部変えることになり、リングの既存の操作の扱いまで変わる。
- `apply_ring_action` のように、スクリプト専用の実行関数を操作ごとに書く。キーの処理との二重の綴りになり、
  ずれたとき気付けない ([ring-keyaction-parity.md](ring-keyaction-parity.md) の経緯と同じ問題)。

### 5.3 実行前の検査

スクリプトが返した並びを、実行の前に全部検査する。1 つでも外れたら**何も実行しない**で通知する:

1. 操作名が許可リストにある
2. 操作の文脈 (`KeyAction::context()`、Rating など共通の文脈を含む) が今の画面で有効
3. B と C は合わせて 1 つまでで、並びの最後
4. E は並び全体で 1 つまで
5. イベントからの実行では、「イベント可」の印のある A だけ
6. 並びの長さが上限 (§7) 以内
7. 全画面 (egui) の描画の所有 pass の末尾で実行するとき (§5.2): そのフレームで既に移動・閉じる・帰還の継続・
   main への移動 (`mouse_ring_nav`) が受け付けられていれば、B・C を含まない

キャプチャ保存でファイルが増え続けないこと (利用者決定 Q2) は、検査 4・5 と、自動繰り返しの押下では
起動しないこと (§3.1) で決める。回数・時刻を数える状態は持たない。保証するのは「1 回の起動で 1 枚まで、
イベントや押しっぱなしでは保存しない」まで (§3.1 の既知の制限)。

---

## 6. 安全の決まり

- スクリプトに渡すのは状態の写し (§4.3) だけで、返せるのは操作名の並びと通知の文字列だけ。
- **エンジンは `Engine::new_raw()` から作り、文字列・配列・マップのパッケージは読み込まない。
  使う関数は mIV 側で 1 つずつ書いて登録する** (レビュー r2 P1-1・r4 P1-1)。
  - 経緯: 標準のパッケージには、操作数の上限で止まらない関数がある。`sleep` (`LanguageCorePackage`) のほか、
    Rhai 1.27.0 の文字列の `pad` は、埋める文字列が空だと長さが増えず終わらない
    (`packages/string_more.rs:1377`、`"".pad(1, "")` で UI スレッドが止まる)。パッケージを丸ごと入れて
    危ない関数を探して外す形は、見落としと版の更新に弱い。
  - 読み込むパッケージ: `ArithmeticPackage` / `LogicPackage` (四則・比較・論理) だけ。どれも許可一覧のテスト (下)
    で関数を固定し、上限で止まらない関数が無いことを監査する。
  - `for` の反復は、`BasicIteratorPackage` を読み込まず、配列と整数の範囲 (`a..b`・`a..=b`) の反復だけを手で
    登録する (実装時の変更、2026-11-25)。`BasicIteratorPackage` は文字列の反復と `chars()` / `bits()` も登録するが、
    `chars()` が返すストリーム (`CharsStream`、`packages/iter_basic.rs:195`・`362`・`478`) は中に文字の列を持ち、
    Rhai の文字列・配列・マップの大きさの上限の集計に入らない。8,192 文字のストリームを配列の上限まで持って
    複製を重ねられるため (実装担当の段 1 の報告)、公開しない。範囲の反復は 1 回ごとに操作数に数えられ、配列の
    反復は配列の上限の中に収まる。
  - 関数ポインタを作る構文 (`Fn` / `curry` / `call` / 匿名関数、名前付きの関数を値として使うこと) は、パッケージ
    無しでも使え、複製を重ねて大きさの上限の外で値を増やせる (実装担当の段 1 の報告)。これらは無効にする。
    通常の関数の定義と直接の呼び出しは使える。
  - 最適化は無効にする (`OptimizationLevel::None`、実装時の変更、2026-11-25)。定数の入れ子の配列・マップが最適化で
    定数 (`DynamicConstant`) に畳まれると、大きさの上限の合計の検査を通らずに作れるため (実装担当の段 1 の報告)。
    スタック用のエンジンは変えない。配列のリテラルは、パーサが閉じ括弧の前に上限を確かめるので上限より 1 少ない個数 (今の上限では 199 個) まで、
    実行時に作る配列は上限 (今は 200 個) まで (入れ子は外側の要素も合算)。
  - **添字・プロパティへの代入 (`a[i] = x`、`m[k] = x`、`m.k = x`、それらへの `+=` など) は使えない。コンパイル時に
    AST を調べて拒否する** (段 1 の独立レビュー P1、2026-11-25)。Rhai 1.27.0 は添字への代入の後に、代入した要素
    だけを検査し、親のコンテナ全体を検査し直さない (`eval/chaining.rs:355`)。マップへの添字による新しいキーの
    追加にも件数の検査が無い (`eval/indexing.rs:119`)。大きな子を多数持つコンテナを作って複製を重ねると、
    操作数の上限の中でも UI スレッドを長く止められる。スクリプトは状態を読んで操作名の並びを返すだけで、
    入れ子を書き換える必要は無いので、構文の側で外す (評価中の大きさを毎回数え直す仕組みは作らない)。
    変数そのものへの代入と、変数への `+=` (例: `ops += "FsPageNext"`) は使える。
    **検査は字句 (トークン) の列で行う** (2026-11-25): Rhai の `AST::walk` は値付きの `break` の式を走査しない
    (`ast/stmt.rs` の `walk` は `BreakLoop` を `_ => ()` で飛ばす) など、AST の走査には届かない所があり得る。
    `engine.lex` のトークンの列を先頭から見て、代入の記号 (`=` と複合代入) の直前が `]`・`.`/`?.` に続く名前・
    `this` なら拒否する。走査の届き方に左右されない。AST の検査は補助として残す。
  - **値付きの `break` (`break 式`) は使えない** (2026-11-25)。スクリプトに必要なく、AST の走査が値の式に届かない。
  - AST の走査に頼る検査 (定数の大きさ・マップのキーの長さ) は、Rhai 1.27.0 の `Stmt` / `Expr` の全 variant について
    子が走査されるかを確かめ、走査されない子は構文の禁止・自前の走査で埋めた (空の範囲の `switch` の条件・腕)。
    例外: `switch` の case に書いた定数はパーサの中で hash になり AST に値が残らないので、この検査の対象外。
    実行中の値にはならないので、上限の外で値を増やす経路にはならない (実装担当の段 1 の報告、2026-11-25)。
  - **速い演算 (fast operators) を無効にする** (`set_fast_operators(false)`、段 1 の独立レビュー P2)。有効だと
    配列同士の `+=` が追加の前の配列だけを検査して追加後の検査を飛ばす (`func/builtin.rs:943`、
    `eval/stmt.rs:147`・`173`)。
  - **`try` / `catch` は使えない** (`disable_symbol`、2026-11-25)。`catch` が受け取るエラーの文字列・マップは
    大きさの検査なしで作られ (`eval/stmt.rs:753`〜`800`)、長い識別子のエラーから上限を超える文字列を作れる
    (実装担当の報告、ソースの確認)。スクリプトは失敗したらその回は何もしない (§6) ので、エラーを捕まえる必要は無い。
  - AST の中の定数の配列・マップ (関数の引数の定数を含む) も、コンパイルの時点で合計の大きさを検査する
    (実装担当の報告: 定数の関数引数に別の迂回があった)。
  - mIV 側で登録する関数 (最初の版): 文字列の `contains(s, sub)` / `starts_with` / `ends_with` / `to_lower` /
    `len`、配列とマップの `contains` (`in` 演算子が呼ぶ、`array_basic.rs:797`・`map_basic.rs:46` と同じ名前と形)、
    配列の `len`。どれも 1 回の呼び出しの処理が入力の大きさ (§7 の上限) に比例し、ループは入力の長さで終わる
    ことを実装で確かめる。足すときも同じ条件で 1 つずつ足す。
  - 配列・マップのリテラル、関数の定義と呼び出しはパーサの機能で、パッケージ無しで使える
    (`parser.rs:1493`・`1497`・`3569`)。
  - `print` / `debug` は登録しない (`BasicStringPackage` を読み込まないので無い)。念のため出力の callback も空にする。
    `eval` / `import` は無効。正規表現の補助は登録しない (§7)。
- **使える関数の一覧をテストで固定する**。エンジンが公開する関数の署名の一覧 (`gen_fn_signatures` 相当) を
  リポジトリの許可一覧と突き合わせ、Rhai の更新などで関数が増えたらテストが落ちるようにする。許可一覧に
  入れるときは、時間・スレッド・ファイル・プロセス・通信・出力に触れないことと、1 回の呼び出しの処理量が
  入力の大きさ (§7 の上限) に比例し、必ず終わることを確かめる。`sleep` / `pad` / `print` / `debug` が
  一覧に無いことをテストで明示的に確かめる。
- ファイル・フォルダの読み書き、外部プログラムの起動、通信、クリップボードの読み取り、設定値の読み書きは
  公開しない。既存の操作を経由した「外のプログラムを動かす」「ファイルを消す・動かす・書き出す」も
  許可リストで外す (§5.1)。ファイルを作るのはキャプチャ保存だけで、保存先と名前は mIV の設定どおり。
- パスなどの文字列は読めるが、スクリプト自身はそれを外へ出す手段を持たない。外に結果が残るのは種別 E
  (キャプチャ保存・クリップボードへのコピー) だけで、押したときの 1 回の起動に 1 つまで。通知は画面に出るだけ。
- 利用者のデータで変えられるのはレーティングとピン留めタグだけ (種別 D)。キーで押したときと同じ実行関数を
  通るので、レーティングの XMP 書き込み (既定 OFF) が ON ならそれも同じく書かれる。
- 上限は §7。失敗 (コンパイル・実行・検査・上限) はログ + 通知で、その回は何もしない。

### 6.1 第三者のスクリプト (利用者決定 Q6: 説明と初回の確認だけ)

- mIV に「スクリプトを取り込む」機能は作らない。利用者がファイルを置き換えるか、中身を貼り付ける。
- マニュアルと設定画面に、できること・できないことを短く書く。案:
  「スクリプトは mIV の画面の状態を読み、許可された mIV の操作 (ページ送り・表示の切り替え・
  レーティングやピン留めタグの付け外し・キャプチャ保存など) を自動で実行します。本を開いたときや
  ページが変わったときに、表示の切り替えを自動で行うこともできます。ファイルの削除・移動、
  ほかのプログラムの起動、インターネットへの接続はできません。
  人から受け取ったスクリプトは、何をするか確かめてから使ってください。」
- 有効にした最初の 1 回だけ、上の説明を確認ダイアログで出す (`operation_script_notice_confirmed`)。
- スロット一覧に「このスクリプトが使う操作」は出さない (利用者決定 Q6)。

### 6.2 AI (MCP) に書かせる (利用者決定 Q5)

- AI 連携で許可した利用者だけ、MCP からスクリプトのファイルを読み書きできるようにする。§1.843 の後の段で入れる
  (本機能の実装後)。許可の段と tool の形は §1.843 の設計 (`docs/mcp-guide-settings-plan.md`) に合わせて、そのとき決める。
- AI が書いたスクリプトも、できることは §5.1・§6 の範囲に限られる。外からの指示が混ざっても被害はその範囲
  (レーティング・ピン留めタグの変更と、押したときのキャプチャ保存・クリップボードまで)。
- 書き込みの後は「再読み込み」と同じ処理で読み直す (ファイルの監視はしない)。
  `operation_script_enabled` と `operation_script_notice_confirmed` は AI に変えさせない案
  (§1.843 の「AI 連携の許可そのもの」と同じ扱いにするかは §1.843 側で決める)。

---

## 7. 上限 (案、実装時に測って決める)

| 項目 | 値 (案) | スタック用 |
| --- | --- | --- |
| 操作数 (`max_operations`) | 2,000 (計測で決定、下) | 100,000,000 |
| 呼び出し深さ | 32 | 64 |
| 式の深さ | 32 / 32 | 64 / 64 |
| 文字列 | 8 KiB | 256 MiB |
| 配列 / マップ | 200 (計測で決定、下) | 8,000,000 |
| 返せる操作の数 | 16 | — |
| スクリプトのファイルの大きさ | 256 KiB (読み込みの失敗にする) | — |
| マップのリテラルのキーの長さ | 256 バイト (コンパイル時に拒否) | — |

- 操作数の上限だけでは時間の上限にならない (レビュー r1 P2-9)。Rhai の組み込みの関数 (`contains`、配列の
  `sort` など) は 1 回の呼び出しが 1 操作で、中の処理は数えられない。そこで、(1) 時間を止める関数 (`sleep`) を
  持つパッケージを読み込まない (§6)、(2) 正規表現の補助は登録しない (コンパイルと共有の mutex を UI スレッドへ
  持ち込まない)、(3) 文字列・配列の上限を小さくし、写しの文字列もその上限で切る (§4.3)、(4) 使える関数を
  許可一覧で固定する (§6)、で 1 回の実行の最悪を「操作数 × 入力の大きさ」程度に抑える。
- 実装時に、許可一覧の関数のうち処理量の大きいもの (文字列の `contains`、配列・マップの `contains`) を
  上限いっぱいの入力で上限回呼ぶ最悪の入力を、release で測る `#[ignore]` の計測テストを置き、値を決める
  (標準の `sort` などは登録しないので対象外)。目安は 1 回の実行が 10ms 以内。`perf::event` (`script/run`、所要・操作数・結果) を入れる。
- **計測 (2026-11-25、release、担当が run-heavy で実行、29c089df0 相当)**: 最初の案 (操作数 20,000・配列/マップ 1,000) では、
  配列の `contains` を上限いっぱいで呼ぶ最悪の入力が 441ms、入れ子の値を探す場合 35〜50ms、マップ 19ms、文字列 1.1ms
  だった。最悪の量は「操作数 × 配列の大きさ」に比例するので、操作数を 2,000、配列・マップを 200 に下げた
  (見込み 441ms × 1/10 × 1/5 ≈ 9ms)。ふつうのスクリプトは数十〜百操作で、状態の写しの配列も 20 個以下なので足りる。
  下げた後 (fcc1b946a、release、担当が run-heavy で実行): 配列の `contains` の最悪 8.79ms、入れ子の値 0.98〜1.14ms、
  マップ 0.20ms、文字列 0.17ms で、どれも目安の 10ms に収まった。
- スタック用 (`perf_10k_default_script`) の `rhai/sync` の前後 (段 1 の独立レビュー P3 を受けて計測。release、
  同じ target、`-j 1`、担当が run-heavy で続けて 1 回ずつ実行。前 = master 16590597e、後 = 6da3649db):
  | 件数 | 規則 | 前 | 後 | 差 |
  | --- | --- | --- | --- | --- |
  | 1 万 | 命名パターン | 107.2ms | 108.7ms | +1.4% |
  | 1 万 | 末尾連番 | 53.3ms | 55.6ms | +4.3% |
  | 1 万 | 更新時刻 | 99.2ms | 106.2ms | +7.1% |
  | 10 万 | 命名パターン | 1111.2ms | 1138.0ms | +2.4% |
  | 10 万 | 末尾連番 | 562.8ms | 577.7ms | +2.6% |
  | 10 万 | 更新時刻 | 1065.9ms | 1123.0ms | +5.4% |
  増えたのは 1.4〜7.1%。各 1 回の計測で、同じ版を以前に流したときの値 (1 万件 110.1 / 55.7 / 107.0ms) との揺れも
  同じ程度なので、`sync` (値の共有に `Arc` と lock を使う) による遅れは数 % 以内と見る。スタックのスクリプトの評価は
  worker で行う ([filename-stack-plan.md](filename-stack-plan.md) の v4.1.0 A2) ので、この差は受け入れる。
- マップのキーは Rhai の大きさの集計 (`calc_map_sizes`) に入らない (実装担当の段 1 の報告)。添字による追加は
  構文で外した (§6) のでキーはリテラルからしか来ないが、長いキーのマップを複製し続けると集計の外で量が増える。
  そこでファイルの大きさとリテラルのキーの長さを上の表で抑える (2026-11-25)。
- 上限に当たったら「時間がかかりすぎたため中止」と通知し、その回は何もしない。

---

## 8. 言語: Rhai を続ける

- 既に依存にあり、サンドボックスの作り方 (§1.1) とテストの前例がある。純 Rust。
- 操作数の上限・`eval` / `import` の無効化・出力 callback の差し替えが標準で書ける。
- 文法は JavaScript / Rust に近く、NeeView の利用者や AI が書きやすい。
- 代案: JavaScript (boa / QuickJS) は依存が重く、サンドボックスと上限を自前で固める必要がある。
  Lua は C のビルドが要る (スタック用の選定記録 §11 と同じ理由)。
- 結論: Rhai を続ける (利用者決定 Q7)。

---

## 9. 段階案

| 段 | 中身 | 時期 |
| --- | --- | --- |
| 最初の版 | スロット 20 個 + 状態の写し + 条件 + 許可リストの操作の並び (§5.1)。キー・リング・ジェスチャー・マウスボタン・パッド・右クリックから呼ぶ。イベント `on_event` (`open` / `view` / `page` / `select`、§3.3) | 少し先の版 |
| MCP | AI にスクリプトを読み書きさせる (§6.2) | §1.843 の後の段 |
| 後の段 | 値を指定する操作 (背景を市松に・ズーム倍率・読む向きの指定など)、許可リストの追加 (フィルタ・AI 高画質化など)、状態の追加 (メタデータ・タグ全般) | 要望を見て |

- 背景の要望のような「巡回する操作で特定の値にする」は、最初の版では「状態を見て、違えば巡回を必要な回数並べる」
  で書ける (`view.background` で今の値が分かる。A は並びの中に複数置ける)。
- 当初の段階案 (マクロ → 条件付き → 状態を参照) は、本書の形では言語の費用がほぼ同じなので最初の版にまとめた。
  GUI で操作を並べるだけのマクロは、GUI の編集画面という別の費用がかかるので、要望が出てから検討する。

---

## 10. テストの方針 (実装時)

- 純ロジック: 並びの検査 (§5.3 の 6 項目それぞれの拒否)、状態の写しの組み立て (文脈ごと、キャッシュに無い値が
  `()` になり DB を読まないこと)、`slots()` の解釈 (番号の重複・範囲外・関数が無い)。
- サンドボックス: `eval` / `import` の拒否、`sleep` / `pad` / `print` / `debug` が無い、`print` / `debug` が何も出さない、操作数の上限で止まる、
  公開する関数の署名の一覧が許可一覧と一致する。**内蔵の雛形 (§4.2) を実際に評価して成功する** (使う関数が
  登録されていること。レビュー r3 P2-2)。最悪の入力の計測 (§7、`#[ignore]`)。`sync` を有効にした後の
  スタック用のテストと計測。
- 実行: 並びを同じフレームで順に実行する、受け付けない操作で残りを捨てて通知、後処理の要る結果 (見開きの
  `Rebased`・遷移を委ねた・閉じた・移動の要求) でそこで終わり結果が呼び出し元へ返る、対象 (チェックの集合を含む)
  が変わったら残りを捨てる、B/C が最後でない並びを拒否、E を 2 つ含む並びを拒否。
- 既存のキーの結果との合流: ROOT の矢印によるページ移動と同時に空・A だけ・B を含む・拒否されるスロットが
  来ても、元の移動と operation を保持する。`Rebased` も競合に含め、`Closed` は二重に終了しない。
  本の構成の受付の確認へ実際に委譲したときも、残りを実行せず `delegated: true` を返す。
- 一覧の開く操作: 参照先の不存在・履歴の guard・親や履歴の移動の guard による拒否では完了の `notice` を出さず、
  非同期の開始の受付とキーの早期 return を区別する。
- 入口の repeat: 一覧 (更新中の制限付き処理を含む)・画像の全画面・native のマウスの戻る/進むで、
  `ScriptSlot*` に解決される repeat の押下を拒否する。通常の戻る/進むの繰り返しによる移動は維持する。
- 共用の実行関数: 表の各操作で、キーの処理とスクリプトが同じ実行関数を呼ぶことの handler-level テスト
  (egui の経路・native 動画の経路・egui の動画/音楽の表示)。
- 入口: 3 つの問い合わせ場所 (一覧・全画面のキー処理・native)・リング・右クリック (native と egui の両方の
  経路) から `run_script_slot` に入る。全画面のキー処理の 3 つの経路 (転送・通常・ROOT 側の予備) と egui の
  動画・音楽の表示で起動する。自動繰り返しの押下で起動しない。全画面の描画の中の入口 (キー・マウスの戻る/進む・
  中ボタン・ジェスチャー・メニュー) がその場で実行せず、所有者が描画の後に 1 回実行し、移動の要求が dispatcher で
  1 回だけ処理される (`Rebased`・`Closed`・移動の要求)。同じフレームにキーの移動があるとスクリプトの B/C が捨てられる。
  メニューから見開きの形を変える A を実行しても、メニューの後の表示設定の戻しで上書きされない。描画を抜けた
  フレームで実行されない。描画の前の入口 (ROOT 側の予備経路・パッド X リング) が予備経路と共用の手順で処理される。
  ページ一覧・編集モードでは、キー・ジェスチャー・中ボタンのどれからも、Rhai を評価せず通知なしで起動しない。
  所有 pass の末尾での実行 (r8): 子 viewport の pass で、その viewport の文字入力のフォーカスを利用資格で見る。
  スクリプトの結果の `Closed` (例: レーティングで対象が無くなった) で dispatcher の通常の終了が走らず、Undo の
  履歴が残る。`ViewerReplaced` で終わったフレーム・表示の識別が変わったフレームでは実行されない。同じフレームに
  ジェスチャーの履歴移動 (`mouse_ring_nav`) や帰還の継続があると、B/C を含む並びは実行の前に拒否され、A だけの
  並びは実行される。同じフレームに中ボタンの `ImageHome` 等でページが変わると (表示の識別 (2) の変化)、レーティング
  を含む並びも実行されない (r9)。ホイールの提案が保留されたフレームではスクリプトの要求が捨てられ、保留された
  提案の中で `ScriptSlot*` に解決されたジェスチャーは実行されない (r9)。
- イベント: 識別の各要素で発火する/しない (ZIP の読み込み途中で `open` が出ない、完了で出る、同じ場所の
  読み込み直しで出ない、分割ページの左右で `page` が出る、同じ PDF の別のページで `page` が出る)、
  検出が転送されたキーの処理より前にある (転送されたキーの変化は次のフレームで出る)、ウィンドウ内の静止画表示でも
  検出する、同じフレームで文脈が変わっても検出は 1 回だけ、別のコレクション・別の★の場所へ移ると `open` が出る、
  右クリックからのキャプチャがメニューを閉じてから実行され拒否されない、1 フレーム 1 回で `events` に変わった種類が全部入る、
  キー起動の移動で次のフレームに発火する、イベント起動の同期の変化で発火しない、イベントから B〜E と
  「イベント可」でない A を拒否、1 秒あたりの上限で捨てる。
- 「イベント可」の印を付けた A の各操作が識別を変えないこと (印の根拠のテスト)。
- 割り当て: `KeyAction` / `RingActionId` の対応の恒久テスト (既存のパリティテストへ追加)。
- 別ウィンドウ・編集モード・ページ一覧・Remote の取得中・読み込み中・OFF で動かないこと。
- 別ウィンドウ (r6・r7): 別ウィンドウの context を取り付けた状態で、キー・リング・メニューから起動しても実行されず
  通知が出る。メインの context のまま F12 で別ウィンドウに表示した viewer・そこへ切り替え中の viewer からも
  起動せず、その間 viewer の面の検出が走らない。その間もメインウィンドウの一覧からは起動でき、`s.context` が
  `"grid"`。ほかに止まっている別ウィンドウがあるだけでは、メインの一覧・全画面からの実行は拒否されない。
  イベントの検出が別ウィンドウの context で走らず、メインの基準が変わらない。既存の detached のテストを変えない。

## 11. 文書の更新 (実装時)

- [keymap-spec.md](keymap-spec.md)、[key-customization-impl-plan.md](key-customization-impl-plan.md)、
  `docs/keymap.ini.default`、[ring-keyaction-parity.md](ring-keyaction-parity.md)
- [architecture-overview.md](architecture-overview.md) (新しいモジュール)、[docs/README.md](README.md) (本書の索引)、
  [ui-responsiveness.md](ui-responsiveness.md) (UI スレッドの新しい区間)
- マニュアル (新しいページ、スタック用と同じく AI へ頼むときの雛形を含める)、製品ページ
- 通信の追加は無い。「安心して使えます」と `privacy.html` に、スクリプトができないことを書くかを確認

---

## 12. 利用者の判断 (2026-11-23、統括経由)

草案の 7 問に対する利用者の答え。「」は利用者が選んだ選択肢の文言 (統括の引用)。

| 問 | 答え | 本書への反映 |
| --- | --- | --- |
| Q1 利用者のデータを変える操作 | 「★・ピン留めタグまで (推奨)」。レーティングとピン留めタグの付け外しは含める。削除・移動・名前の変更は含めない | 種別 D (§5.1)、`item.rating` / `item.pinned_tags` (§4.3)、§6 |
| Q2 ファイルを作る操作 | 「キャプチャ保存だけ含める」(推奨と違う)。書き出しは含めない。ファイルが増え続ける点は、状態を増やさない形で扱う | 種別 E (§5.1)、検査 4・5 と自動繰り返しで起動しないこと (§5.3・§3.1)。数える状態は持たない (§2.1) |
| Q3 最初の版の範囲 | 「イベントも最初から」(推奨と違う)。「イベントからはその場の操作だけ」の決まりは保ったまま範囲を広げる | イベント 4 種 + 受け口 `on_event` 1 つ (§3.3)、段階案 (§9) |
| Q4 割り当て先 | 「操作の入力すべて+右クリック (推奨)」 | §3.1 |
| Q5 AI (MCP) に書かせるか | 「書かせる (推奨)」。AI 連携で許可した利用者だけ。§1.843 の後の段 | §6.2 |
| Q6 第三者のスクリプト | 「説明と初回の確認だけ (推奨)」 | §6.1 |
| Q7 言語 | Rhai を続ける (異論なし) | §8 |

設計担当の解釈 (統括へ報告し、利用者の確認が要るなら統括が取る):

- Q1 のレーティング・タグと Q2 のキャプチャ保存は、イベントからは呼べないことにした (§3.3)。
  ページを送るだけで、押していないのにデータが変わり・ファイルが増える形を避けるため。
  クリップボードへのコピーも同じ扱い (種別 E)。
- 草案の 2 種 (`open` / `page`) に、一覧の選択変更 `select` と全画面の切り替わり `view` を足し、受け口を
  `on_event` 1 つにまとめたのは、Q3 の「範囲を広げる」を受けて設計担当が決めた。起動時・終了時・再生終了などを
  入れない理由は §3.3。

利用者の確定 (2026-11-23、統括経由): 上の 1 点目は「押したときだけ (推奨)」で確定した。★・ピン留めタグの付け外し、
キャプチャ保存、クリップボードへのコピーは、キー・右クリックなどで起動したときだけ実行でき、イベントからは
表示・再生の操作だけを返せる。

---

## 13. 独立レビューの記録

### r1 (2026-11-23、GPT-6.1 Sol / xhigh、read-only、対象 1b270cf4b): REVISE

指摘 11 件。設計担当が根拠のコードを読み、すべて妥当と判断した (false positive なし)。r2 での対応:

| # | 重さ | 指摘 | r2 の対応 |
| --- | --- | --- | --- |
| 1 | P1 | native 動画への注入は、前半の解決と後半の VK 照合の両方を通せない | 注入をやめ、照合と実行を分けて共用の実行関数を呼ぶ形へ (§5.2) |
| 2 | P1 | 「押した回数を超えない」は native の Browser キーの repeat 消失・重複で崩れる。`FsCapture` は repeat を拒否しない | 受付を 1 つ (`run_script_slot`) にし、repeat は入口で拒否。保証を「1 回の起動で 1 枚まで」に改め、Browser キーの重複は既知の制限として別件へ (§3.1・§5.3) |
| 3 | P1 | 実行途中の中止条件がパッド・ホイール・Remote・設定変更を覆っていない | 同じフレームで全部実行し、「実行中」の状態を無くした (§2.1・§5.2) |
| 4 | P2 | egui 側の問い合わせ関数が多く、注入 1 か所では届かない・pass 境界が要る | 注入をやめたので不要 (§5.2) |
| 5 | P2 | Global に足しても各画面から問い合わせられない。native 転送は列挙式 | 問い合わせる 4 か所と native 転送への追加を明記 (§3.1) |
| 6 | P2 | 見開きの切り替え・レーティングで識別が変わる。A〜E から遷移の制約を導けない | 操作の表を案として確定 (§5.1)、「イベント可」の印を確かめた A だけに付ける、各操作の後に識別を比べて残りを捨てる (§5.2) |
| 7 | P2 | キー起動の移動のイベントと基準の更新が矛盾 | キー起動は基準を動かさない、イベント起動は直後に基準を取り直す、と一意に定義 (§3.3) |
| 8 | P2 | パスの比較は「開き終えた」にならない (ZIP の途中・prefix・分割ページ) | 読み込み完了で採用された論理的な場所、表示単位と分割の左右で識別を定義 (§3.3) |
| 9 | P2 | 操作数の上限は時間の上限にならない (正規表現の補助・共有 mutex)。`get_rating` は DB を読む | 正規表現の補助を登録しない、文字列の上限を小さく、最悪の計測テスト、読み込みとコンパイルは worker、写しはキャッシュだけ (§4.1・§4.3・§7) |
| 10 | P2 | `print` / `debug` が標準出力へ書く | 出力 callback を空にする (§6)。スタック用の同じ点は別件として統括へ報告 |
| 11 | P3 | 雛形の `on_event` が一覧でも全画面の操作を返す。detached の書き方が広すぎる | 雛形を直した (§4.2)。detached の手順を正しく書いた (§3.2) |

### r2 (2026-11-23、同じセッションの続き、対象 b51de68b9): REVISE

r1 の 11 件のうち、3・4・5・10・11 は解消、1・7 は注入に固有の問題は解消、2・6・8・9 は一部対応、と評価された。
新しい指摘 8 件。設計担当が根拠のコードを読み、すべて妥当と判断した (false positive なし)。r3 での対応:

| # | 重さ | 指摘 | r3 の対応 |
| --- | --- | --- | --- |
| 1 | P1 | Rhai 標準の `sleep` が `std::thread::sleep` を呼び、操作数の上限で止まらない。`sort` も 1 呼び出しで重い | `Engine::new_raw()` + 必要なパッケージだけ、言語の中核のパッケージは読み込まない、公開する関数の一覧をテストで固定、計測の対象を広げた (§6・§7) |
| 2 | P1 | 画像の全画面のキー処理は `FsKeyAction` (移動の要求・`Rebased`/`Closed`) を返し、呼び出し元が後処理する。「共用関数を呼ぶだけ」では足りない | 実行関数がキーの処理と同じ形の結果を返し、後処理の要る結果でそこで終えて呼び出し元へ返す (§2・§5.2) |
| 3 | P2 | 見開きの変更は本の構成の確認へ委ねて遅れて移動することがある。§1.3 の「同期」は誤り | §1.3 を訂正。「遷移を委ねた」を結果に含め、そこで終える (§5.2) |
| 4 | P2 | イベントを実行する所有 pass と ROOT からの転送の順序が未確定 | 画面ごとに所有 pass を決め、全画面は子 viewport の pass だけ、native は ROOT + `NativeOverlayInputRouting`、1 フレーム 1 回はフレーム番号 1 つで判断 (§3.3) |
| 5 | P2 | `rhai` に `sync` が無く、worker でコンパイルした AST を UI へ送れない | `sync` を有効にし、スタック用のテストと計測を流し直す (§4.1) |
| 6 | P2 | PDF のページは同じパスで別のページ番号。一覧のレーティングはチェックを消し、続く操作の対象が変わる | 項目の識別を型付き (PDF はページ番号を含む) に、続行の判定にチェックの集合を含めた (§3.3・§5.2) |
| 7 | P2 | native のマウスの戻る/進むは repeat を拒否せず、`apply_ring_action` では repeat が分からない | その分岐で `ScriptSlot*` に解決される repeat を拒否。重複の既知の制限と分けて書いた (§3.1) |
| 8 | P3 | ピン留めタグは一覧専用。「通常なら終了確認」の終了は `KeyAction` ではない | 表を直した (§5.1)。終了は後の段 (§0) |

### r3 (2026-11-23、同じセッションの続き、対象 b4504de30): REVISE (P1 なし)

r2 の 8 件のうち 4 を除いて解消、r1 の 2・6・9 は解消、8 は一部対応と評価された。§0 の 2 つの例は
後処理で終える規則でも書ける、との確認があった。新しい指摘 5 件。設計担当が根拠を読み、すべて妥当と判断した。r4 での対応:

| # | 重さ | 指摘 | r4 の対応 |
| --- | --- | --- | --- |
| 1 | P2 | 子 viewport の pass だけではウィンドウ内の静止画表示 (ROOT に描く) を覆わない。転送キーとの前後も食い違う。フレーム番号は viewport ごと | 検出点を `render_fs_body` の両方のキー処理より前に。所有者は表示の形で決め、ROOT のフレーム番号で「検出済み」を記録 (§3.3) |
| 2 | P2 | 文字列の `contains` は `MoreStringPackage` にあり、選んだパッケージでは雛形が動かない | `MoreStringPackage` を監査して加え、雛形を評価するテストを足した (§6・§10) |
| 3 | P2 | 右クリックからの起動は、自身のメニューが開いたままで受付の条件に引っ掛かる | 選んだスロットを受け取り、メニューを閉じてから同じフレームで実行 (§3.1) |
| 4 | P2 | `open` の識別がコレクション・★の場所などパスの無い場所を区別しない | 論理的な場所を型付きの値に (§3.3) |
| 5 | P3 | `print` / `debug` は `LanguageCorePackage` ではなく `BasicStringPackage` | 記述を訂正 (§1.1・§6) |

### r4 (2026-11-23、同じセッションの続き、対象 70f6b3a43): REVISE

r3 の 5 件はすべて解消と評価された。新しい指摘 1 件 (P1)。設計担当が Rhai 1.27.0 の該当箇所を読み、妥当と判断した。r5 での対応:

| # | 重さ | 指摘 | r5 の対応 |
| --- | --- | --- | --- |
| 1 | P1 | `MoreStringPackage` の文字列 `pad` は埋める文字列が空だと終わらない (`string_more.rs:1377`)。UI スレッドが止まる | 文字列・配列・マップのパッケージを読み込まず、使う関数を mIV 側で 1 つずつ書いて登録する形にした。`sleep` / `pad` / `print` / `debug` が無いことをテストで確かめる (§6・§7・§10) |

スタック用スクリプト (`filename_stack_script.rs`) は `Engine::new()` を使うので、`sleep` と文字列の `pad` の同じ問題を
持つ (worker で走るので UI は止まらないが、キャンセルの確認 `on_progress` が効かず worker が終わらない)。
本機能とは別件として統括へ報告した。

### r5 (2026-11-23、同じセッションの続き、対象 627c279dd): ACCEPT

r4 の P1 は解消。追加の指摘なし。最小の関数の組で雛形 (§4.2) が書けること (`in` は引数を入れ替えて `contains` を呼ぶ、
`parser.rs:2395`)、残す 3 つのパッケージに操作数の上限で止まらない関数が無いこと (`iter_basic.rs:110`・`269`・`313`、
反復は `eval/stmt.rs:660` で操作数に数えられる) が確認された。雛形の評価・関数一覧の固定・最悪の入力の計測は、
設計どおり実装時に確かめる事項。

---

## 14. 実装着手前の確かめ直し (r6、2026-11-24)

r5 の ACCEPT の後、master が約 250 コミット進んだ (§1.471 閲覧中の左クリックのモード、§1.874 3D 写真の拡大中の
クリックなど)。実装の着手にあたり、設計担当が調査担当 (読み取り専用) に入口の棚卸しを頼み、主な点を設計担当が
コードで確かめた。製品は起動していない。

| # | 見つかったこと | r6 の対応 |
| --- | --- | --- |
| 1 | egui で描く動画・音楽の表示のキーは、画像の全画面のキー処理の中で処理される。独立した入口ではない | 問い合わせ場所を 4 から 3 へ (§3.1) |
| 2 | `apply_ring_action` は `Option<AddressBarNav>` しか返さず、リング側の全画面の移動はその場で実行する。リング・メニューの入口には「キー処理と同じ形の結果を返す」先が無い | 入口ごとの渡し先を決めた。全画面は ROOT 側の予備経路の後処理の手順を切り出して共用 (§5.2) |
| 3 | native 動画のキーの処理は操作をその場で実行し終える。`NativeVideoKeyOutcome` はログの分類 | 結果の型の記述を訂正 (§5.2) |
| 4 | 右クリックメニューは Win32 のネイティブと egui の 2 経路 | 両方に足し、両方で「閉じてから実行」(§3.1) |
| 5 | native 動画の窓の XButton の経路 (ダブルクリックで 2 回) | 入口と既知の制限に足した (§1.2・§3.1) |
| 6 | ページ一覧・編集モードでは、問い合わせ前に別の処理へ渡る | 通知をやめ、問い合わせない形に簡素化 (§3.2) |
| 7 | 入口の関数とイベントの検出点は、別ウィンドウの context を取り付けている間も走る | メインかの既存の述語で実行・検出を止める。凍結の決まりとの照合を記録 (§3.2・§3.3) |
| 8 | §1.471・§1.874・右クリックの短押しは割り当ての仕組みを持たない | 入口にしない (§1.2) |
| 9 | スタック用スクリプトの `sleep` / `pad` は §1.863 で直った | §1.1 を更新 |

行番号は `16590597e` のもの。

### r6 の独立レビュー (2026-11-24、GPT-6.1 Sol / xhigh、read-only、新しいセッション、対象 385701249): REVISE

§14 の #1・#3・#4・#5・#8・#9 は今のコードと整合、問い合わせ場所の 3 か所化で egui の動画・音楽と全画面のキーの
3 経路を覆える、ページ一覧のキー・リングの除外も成り立つ、と評価された。detached については「メイン専用の
所有範囲を既存の context・表示先・遷移の事実で定義する方針自体は、症状パッチではなく構造的な決定」と評価され、
r6 の具体案 (ID の比較だけ) には合意が無かった。指摘 3 件。設計担当が根拠のコードを読み、すべて妥当と判断した。

| # | 重さ | 指摘 | r7 の対応 |
| --- | --- | --- | --- |
| 1 | P1 | 描画の準備の後の入口 (キー処理の中のマウスの戻る/進む、描画の中のジェスチャー・リング、メニュー) で結果を内側だけで処理すると、外側へ `Unchanged` が返り、準備し直し・終了・メニューの後の表示設定の適用し直しと食い違う | 描画の中の入口はスロット番号だけを描画の所有者へ返し、所有者が描画の後・dispatcher の直前の 1 か所で実行して、1 フレームの移動の要求に合流させる (§1.2・§3.1・§5.2) |
| 2 | P2 | context ID の比較だけでは、メインの context のままの F12 の別ウィンドウ表示を除けない | 利用資格の関数を 1 つにし、viewer の面では今の viewer の表示先・切り替えの事実も見る。一覧の面は見ない (§3.2・§3.3) |
| 3 | P2 | 編集モードの「問い合わせない」は、ジェスチャー・中ボタンなどキー以外の入口を覆わない | 利用資格の関数で、Rhai の評価の前に通知なしで拒否する。イベントの検出も同じ関数を使う (§3.2・§3.3) |

### r7 の独立レビュー (2026-11-24、同じセッションの続き、対象 c75710add): REVISE

r6 の P2 2 件は解消と評価された (利用資格: メインの context の確認と viewer の面だけの
`current_viewer_session_is_detached_or_switching()` で F12 の表示・切り替え中を除け、止まっている別 context で誤って
拒否せず、別ウィンドウ表示中のメインの一覧を `"grid"` で許す。編集モード等を Rhai の評価前に通知なしで拒否)。
`Rebased` の準備を次の描画へ任せる方針、ページ一覧への切り替えと `frame_exited` / `frame_path_changed` で捨てる点も
妥当と評価された。**detached の合意を得た** (下)。指摘 3 件。設計担当が根拠を読み、すべて妥当と判断した。

| # | 重さ | 指摘 | r8 の対応 |
| --- | --- | --- | --- |
| 1 | P1 | `Closed` は「既に閉じた」結果。閉じる要求に変えると終了の処理が二重になり、レーティング由来の終了で残した Undo の履歴が消える | `Closed` は移動の候補を捨てて dispatcher を呼ばずに描画を抜ける (キー処理・予備経路と同じ) (§5.2) |
| 2 | P2 | dispatcher の直前では所有していた子 viewport の `ctx` が有効でなく、文字入力・IME の利用資格を判断できない | 実行の位置を、所有 pass の末尾 (コールバックを抜ける前) に移した (§5.2) |
| 3 | P2 | 競合の判定がローカルの移動の変数だけだと `mouse_ring_nav`・帰還の継続を見落とす。`ViewerReplaced` は `frame_exited` へ伝わらない | 受けたときの表示の識別を持ち、`ViewerReplaced`・識別の変化で捨てる。競合の判定に `mouse_ring_nav`・帰還の継続・ホイールの提案を含め、B/C を含む並びを実行の前に拒否する (§5.2・§5.3 の 7) |

detached についてのレビュー担当の言葉 (r7):「r7 が、既存の context・現在の表示先・遷移先・入力面によってメイン専用の
所有範囲を定義することは、症状パッチではなく構造的な決定であることに合意します。detached の寿命・placement・
取り付けを変更せず、メイン専用状態を bundle 外で保持する方針も、憲法と context 所有規則に整合します。」
設計担当 ClaudeCode も同じ主張に同意する。

### r8 の独立レビュー (2026-11-25、同じセッションの続き、対象 286507b0a): REVISE

r7 の P1 (`Closed`) は解消、所有 pass の中へ実行を戻した点、ウィンドウ内の静止画・egui の動画/音楽で所有 viewport の
`ctx` を使える点、`Rebased` の準備を次の描画に任せる点は成り立つと評価された。detached の合意は変わらない
(「r8 も既存の context・表示先・遷移先・入力面でメイン専用の所有範囲を定義しており、症状パッチではなく構造的な決定」)。
指摘 2 件。設計担当が根拠を読み、どちらも妥当と判断した。

| # | 重さ | 指摘 | r9 の対応 |
| --- | --- | --- | --- |
| 1 | P1 | 要求に持たせる識別 (`page_turn_destination` と `items_generation`) はページを含まず、同じフレームの中ボタンの `ImageHome` などで別のページへレーティングが当たり得る | §3.3 の `"page"` の識別 (型付きのページの識別・分割の左右) も持たせ、違えば並び全体を捨てる (§5.2) |
| 2 | P2 | 保留されたホイールの提案の中のジェスチャーは、所有 pass の末尾より後に処理される | 提案が保留されたフレームではスクリプトの要求を捨てて通知し、保留された提案の中で `ScriptSlot*` に解決されたものは実行しない。提案の適用の順序を変える案は、リリース済みのホイールの挙動を変えるので採らなかった (§5.2) |

### r9 の独立レビュー (2026-11-25、同じセッションの続き、対象 f4d9da7e9): ACCEPT WITH CHANGES

r8 の 2 件は解消、追加の P1 / P2 なし。保留の有無は所有 pass の末尾で `wheel_frame_proposal` から判定できる
(`ui_fullscreen.rs:29475`・`31249`)。既存のホイールの処理の順序を保ち、この組み合わせを通知付きで拒否する判断は
妥当と評価された。P3 1 件 (保留の発生条件とスクリプトを拒否する条件の文言が混ざっている) は、§5.2 の文言を直して
反映した。detached の合意は変わらない (「r9 も既存の context・表示先・遷移先・入力面でメイン専用の所有範囲を定義する
構造的な決定であり、寿命・placement・取り付けや bundle の所有境界を変えていません」)。

## 15. 実装の独立レビューの記録

### 段 1 (2026-11-25)

- GPT-6.1 Sol / xhigh (36516bafb): REVISE。P1 添字代入の大きさの確認漏れ、P2 配列の `+=` (§6)。直した。
- 2 回目・3 回目は相手側の安全判定で途中で止まり、結論なし (言い換えての再依頼はしない、統括の指示)。3 回目の途中の指摘 (値付きの `break`) は設計担当が Rhai のソースで確かめて直した (§6)。
- GPT-6 Astra / medium (利用者の判断、6da3649db): ACCEPT WITH CHANGES。P3 `sync` の前後の性能の比較が無い → 計測して §7 に記録 (7130b440c)。

### 段 2a (2026-11-25、GPT-6 Astra / medium、read-only、新しい会話)

| 回 | 対象 | 判定 | 指摘 | 対応 |
| --- | --- | --- | --- | --- |
| 1 | cd48a44fc | REVISE | P2 一覧の Browser 戻る/進むの自動繰り返しでスクリプトが起動する / P2 ROOT 側の予備経路でスクリプトの結果が同じフレームのキーの結果を置き換える / P3 `GridOpenSelected` が開けなかった場合も成功を返す。委ねた経路のテストの不足 | f106e2e3f: 件数と非 repeat の事実を 1 つの atomic で運ぶ (§3.1)、`merge_script_action` で 1 か所で合流 (§5.2)、受付の結果を開く処理から返す |
| 2 | f106e2e3f (同じ会話の続き) | ACCEPT WITH CHANGES | P3 detached-rework-plan.md §11 に、受付の結果を返すために別ウィンドウへ開く経路の関数に触れた範囲を追記する | §11 に追記 |
| 3 | f35aaa598 (master 0381f253e の取り込み、同じ会話の続き) | ACCEPT | なし。衝突の解消 (ScriptSlot のキーとリングの対応を master で移った `ring_shortcut.rs` の共有関数へ、設定の分類の件数) と、master が元の場所だけ変えて切り出した実行関数へ届いていない変更が無いかを確認。コンパイルで見つかった 1 件 (§1.746 の `input_advances_right_to_left`) はマージで直した | — |

detached の合意は 3 回とも得た (1・2 回目の記録と引用は detached-rework-plan.md §11)。3 回目の言葉:「既存の所有境界・受付結果を使う構造的な変更であり、症状パッチではない」。マージで detached への触れ方は変わっていない。
