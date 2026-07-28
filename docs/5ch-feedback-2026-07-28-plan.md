# 5ch フィードバック対応 (2026-07-28) — 実装ブリーフ

> ステータス: **設計確定 / 未実装** (2026-07-28)。実装 = Codex Sol、レビュー = ClaudeCode。
> 出典: 5ch ソフトウェア板 mImageViewer スレのレス 123〜125。
> 行番号は `32494ef6` 時点のもの。先行スライスが入ると前後するので、着手時に再確認する。

## 対象

| ID | 種別 | 内容 |
|---|---|---|
| S1 | 不具合 | 詳細表示の右寄せ数値 (ページ数 / サイズ / 解像度) の右端が 1〜2 物理px 欠ける |
| S2 | 要望 | 設定メニューで「ツールバー」サブメニューが「環境設定…」を隠してクリックしにくい |
| S3 | 要望 | 「環境設定」「操作カスタマイズ」をキー / ジェスチャのコマンドとして登録できるようにする |
| S4 | 要望 | 一覧のカーソル移動をループさせる設定 (先頭で↑→末尾、末尾で↓→先頭) |
| S5 | 派生 | 詳細列の右クリックメニューが縦長すぎるので 2 列化 + ScrollArea 安全網 |
| S6 | 要望 | 下部情報バーの表示項目を、サムネイル表示時と詳細表示時で分けられるようにする |

実装順序: **S1 → S2 → S4 → S3 → S5 → S6**。S6 は S5 のメニュー構造を前提にする。
各スライスは独立コミットにする。

---

## S1. 詳細表示の右寄せテキストが右端で欠ける

### 症状

Windows スケーリング 100% + mIV スケーリング 100% + UI フォント MS PGothic で、詳細表示の
右寄せ列 (ページ数 / サイズ / 解像度) の数字の右端が 1〜2 ドット欠ける。欠ける行と欠けない行がある。

### 原因

[src/ui_main.rs](../src/ui_main.rs) `draw_details_text` (≈2430):

```rust
let clip = rect.shrink2(egui::vec2(6.0, 1.0));
let x = if 右寄せ { clip.right() } else { clip.left() };
ui.painter().with_clip_rect(clip).text(egui::pos2(x, clip.center().y), align, ...);
```

右寄せテキストの**アンカー x と clip 右端が完全に同一で、余白がゼロ**。ここに 2 つの独立した
丸めが乗る:

- galley 位置の物理ピクセル丸め: `epaint-0.33.3/src/tessellator.rs:2014`
  (`round_text_to_pixels` は既定 true) → 描画開始 x が最大 0.5 物理px **右へ**ずれる
- scissor 矩形の丸め: `egui-wgpu-0.33.3/src/renderer.rs:1113-1122` (`clip_max_x.round()`)
  → clip 右端が最大 0.5 物理px **左へ**ずれる

結果、最後のグリフの右端 0.5〜1 物理px が切られる。galley 幅の小数部が文字列ごとに変わるため
欠ける行と欠けない行が混在し、プロポーショナルフォントで顕在化しやすい。ppp=1.0 のとき
1 物理px = 1 論理px なので最も目立つ。**OS 非依存** (Windows 10 固有ではない)。

対象列が `Align2::RIGHT_CENTER` の PageCount / Size / ImageDimensions / VideoDuration /
VideoDimensions (≈12545-12600) で、報告された列と一致する。

### 修正方針

`draw_details_text` の右寄せ時だけ、テキスト用 clip の右端に **1 物理px (= `1.0 / ppp`) の余白**を
足す。アンカーは `clip.right()` のまま (見た目の右揃え位置を変えない)。列の内側パディングが
6px あるので、隣接列や区切り線を侵さない。

- 余白の算出は既存の [`details_layout_right_guard`](../src/ui_main.rs) (≈1444) と同じ考え方。
  同関数をそのまま使うか、同等の小さなヘルパーを新設するかは実装者判断。
- ppp は `ui.ctx().pixels_per_point()` から取る。
- **左寄せ側は今回変更しない**。理屈上は同じ丸めで左端が欠け得るが実害報告がないため、
  現状維持とし、「右寄せのみ余白を入れている理由」をコード内コメントに残す。

### テスト

clip 矩形と ppp から「テキスト用 clip 矩形」を返す純関数に切り出し、ui_main.rs の
`#[cfg(test)]` にユニットテストを追加する。

- ppp = 1.0 / 1.25 / 1.5 で余白が `1.0 / ppp` になること
- 左寄せでは余白が入らないこと
- clip 幅が極小 (`<= 1.0`) のとき従来どおり描画をスキップすること

### ドキュメント

コード内コメントのみ。ユーザー向け文書の変更なし。

---

## S2. 設定メニューの「ツールバー」サブメニューが「環境設定…」を隠す

### 症状

「設定」メニューを開いて「ツールバー」にカーソルを乗せると、幅 600px 超の
「表示するセクション」パネルが親項目の真下に開き、サムネイルキャッシュ管理から
「環境設定…」までの後続項目を覆う。

### 原因

[src/ui_main.rs](../src/ui_main.rs) の `draw_sticky_settings_menu_header` で、タイトルの右端に
× ボタンを置くために使っていた `Layout::right_to_left` が、ポップアップの残り利用可能幅
(設定メニューの左端から画面右端まで) をすべて確保していた。そのため「表示するセクション」
パネルの幅が内容ではなく画面上のメニュー位置から決まり、実機では約 605px まで膨張した。

この幅では egui の `RectAlign::RIGHT_START` (親項目の右) にも `LEFT_START` (左) にも収まらず、
`egui-0.33.3/src/containers/popup.rs:482` の `MENU_ALIGNS` 先頭である `BOTTOM_START`
(親項目の真下) へフォールバックする。これが後続項目を覆う直接原因だった。

### 修正方針

- `draw_sticky_settings_menu_header` のヘッダ行から、残り幅を消費する `right_to_left` レイアウトを
  外す。共通の `set_min_width(220.0)` は維持し、× はタイトル直後に内容幅で描く。共通の狭い
  `set_max_width` は追加しないため、「場所」(520px) と「タグ」(360px) は各メニュー固有の幅を維持する。
- 「ツールバー」サブメニューはコマンドのループを回し切った後、`ui.separator()` を挟んで
  **設定メニューの最後**に無条件で描く。仮に左右へ収まらず真下へ開いても、その下に覆う項目が
  無い防御になる。
- 「ツールバー」サブメニューが設定メニューの最後の砦である (全セクションを隠しても再表示できる)
  という既存の設計意図とコメントを維持する。末尾に置く理由もコードコメントで固定する。
- 「設定 → 環境設定…」を非表示にできない仕様 (`docs/key-command-catalog-plan.md`) は変更しない。

当初の「他のサブメニュー群の直後へ移す」方針は誤りで、覆われる後続項目の範囲を広げて悪化させた。

### テスト

800px の利用可能幅でも共通ヘッダの自動サイズが 220〜280px に留まることをレイアウトテストで確認し、
既存テストと UI snapshot が壊れないことも確認する。実機では「設定 → ツールバー」の
「表示するセクション」が概ね 220〜280 論理px に収まり、通常のウィンドウ幅では親項目の右側へ
開くこと、および「環境設定…」までの項目を覆わないことを確認する。

### ドキュメント

本節に、実機で判明した幅膨張の原因と最終修正を記録する。

---

## S3. 「環境設定」「操作カスタマイズ」をコマンド化

### 要望

環境設定をよく使うのでジェスチャに登録したい / 操作カスタマイズも同様。

### 現状

[src/keymap.rs](../src/keymap.rs) の `MenuCommandSpec` で
`SettingsOperationCustomize` (≈2465) と `SettingsPreferences` (≈2471) は `action: None`。
`KeyAction` が無いためコマンド一覧にも出ず、`RingActionId` にも対応が無い。

### 実装方針

キーボードとリング/ジェスチャの両方に登録する (`docs/ring-keyaction-parity.md` の方針に従い、
片側だけに足さない)。

**文脈は Grid のみ**。フルスクリーン中はメインウィンドウのダイアログが見えないため
(`viewer_session_blocks_main_window`)、`ImageFullscreen` / `VideoFullscreen` には追加しない。

**既定キーは割り当てない** (空の `ChordList`)。既存キーとの衝突を避けるため、ユーザーが
操作カスタマイズで任意に割り当てる。`default_chords` が空リストを返せるかを確認し、
返せない場合は既存の「既定なし」アクションの書き方に合わせる。

#### KeyAction 側 ([src/keymap.rs](../src/keymap.rs))

追加する 2 アクション (名前は実装者判断。例: `GridOpenPreferences` / `GridOpenOperationCustomize`):

- enum 定義 (≈1304 付近)
- `ALL_ACTIONS` (≈1722 付近)
- `ini_name()` (≈3203 付近)
- 表示名 (≈3743 付近) — 「環境設定を開く」「操作カスタマイズを開く」
- `context()` の Grid グループ (≈4164 付近)
- ヘルプ分類のグループ (≈4541 付近)
- `default_chords()` (≈4953 付近) — 既定なし
- `MenuCommandSpec` の `action` を `Some(...)` に (≈2465 / ≈2471)
- [docs/keymap.ini.default](keymap.ini.default) に追記

#### RingActionId 側 ([src/ring_shortcut.rs](../src/ring_shortcut.rs))

- enum 定義 (≈526)
- `as_str()` (≈909) / `from_str()` (≈1041) — 文字列 id は `open_preferences` /
  `open_operation_customize` のような snake_case
- `label_for_context()` (≈1136)
- `is_valid_for_context()` / `available_for_context()` (≈1408) — Grid にのみ追加

#### 実行

[src/app/gamepad_input.rs](../src/app/gamepad_input.rs) `apply_ring_action` (≈4732) に分岐を追加し、
`self.show_preferences = true` / `self.show_operation_customize = true` を立てる
(`RingActionId::GridToggleDetails` と同様に `context == RingShortcutContext::Grid` でガード)。

キーボード側は [src/app.rs](../src/app.rs) の `handle_keyboard` (≈29874) で
`self.keymap.consume_action(ctx, KeyAction::...)` を拾って同じフラグを立てる。既定キーが
無いので通常は発火しないが、割り当てたときに動くこと。

#### 相互参照

[src/ui_dialogs/preferences/pages.rs](../src/ui_dialogs/preferences/pages.rs) の
`ring_bindings_for_key_action` (≈1271-) に KeyAction → RingActionId のマッピングを追加。
必要なら説明文テーブル (≈1180-1225) にも 1 行ずつ追加する。

### 注意

`available_for_context` は**リング / マウスジェスチャ / マウスボタン / ゲームパッド X+方向で
共有**される。ジェスチャだけに出すことはできない (全部に出る)。これは既存の全アクションと
同じ扱いなので許容する。

### テスト

- `MenuCommandId` ⇄ catalog の drift テスト、`KeyAction::ALL_ACTIONS` の網羅テストなど
  既存の enum 整合テストが通ること
- `RingActionId::as_str()` / `from_str()` のラウンドトリップテストに新 variant を追加
- 新 KeyAction が Grid 文脈で列挙されること

### ドキュメント

- [docs/keymap.ini.default](keymap.ini.default)
- [docs/keymap-spec.md](keymap-spec.md)
- [docs/ring-keyaction-parity.md](ring-keyaction-parity.md) — 対応表に 2 行追加
- `htdocs/mimageviewer/manual/settings.html` / `shortcuts.html` — コマンド一覧に触れている箇所

---

## S4. 一覧のカーソル移動をループさせる設定

### 要望

一番上で↑を押したら一番下へ、一番下で↓を押したら一番上へ。ON/OFF 設定付き。

### 設定

`Settings` に `grid_cursor_wrap: bool` を追加 (既定 `false` = 現状動作)。
環境設定の一覧系ページ、「一覧のクリック選択」
([src/ui_dialogs/preferences/pages.rs](../src/ui_dialogs/preferences/pages.rs) ≈596) の近くに
チェックボックスを置く。

- ラベル: 「カーソル移動をループする」
- 補足: 「先頭で↑、末尾で↓を押したとき、反対の端へ移動します。」

### 対象と挙動

**矢印キー相当の 1 アイテム / 1 行移動のみループさせる**。Home / End / PageUp / PageDown は
従来どおり端で止める (クランプ)。

純関数として切り出し、キーボードとゲームパッドの両方から使う (現状ロジックが 2 箇所に
並行実装されているため、必ず共通化する):

```
last = len - 1
right : vis_pos < last ? vis_pos + 1 : 0
left  : vis_pos > 0    ? vis_pos - 1 : last
down  : vis_pos + cols <= last ? vis_pos + cols : vis_pos % cols            // 同じ列の先頭行
up    : vis_pos >= cols ? vis_pos - cols : 最終行の同じ列 (無ければ 1 行上)
```

詳細表示は `nav_cols = 1` なので上式が単純な線形ループに縮退する。

### 編集箇所

- キーボード: [src/app.rs](../src/app.rs) ≈30179-30197 の `new_vis_pos` 算出
- ゲームパッド / リング: [src/app/gamepad_input.rs](../src/app/gamepad_input.rs)
  `gamepad_grid_nav_target_pos` (≈6180)。詳細モードの Left / Right は**ページ移動**なので
  ループ対象外 (クランプのまま)
- フォルダツリーペイン ([src/folder_pane.rs](../src/folder_pane.rs) `move_cursor` ≈404) は
  **今回の対象外**

### テスト

純関数に対して境界テストを追加する。

- 1 列 (詳細表示) で先頭↑ → 末尾、末尾↓ → 先頭
- 複数列で最終行↓ → 同じ列の先頭行、先頭行↑ → 同じ列の最終行
- 最終行が埋まっていない (len が cols の倍数でない) ケースで、↑が範囲外にならないこと
- 設定 OFF のとき従来どおりクランプすること
- アイテム 1 件 / 0 件で破綻しないこと

### ドキュメント

- [docs/spec.md](spec.md) の設定項目
- `htdocs/mimageviewer/manual/grid.html` または `settings.html`

---

## S5. 詳細列の右クリックメニューを 2 列化 + ScrollArea

### 背景

[src/ui_main.rs](../src/ui_main.rs) `draw_details_column_context_menu` (≈12188) は
チェックボックス 15 + ラジオ 8 + ラベル 4 + セパレータ 5 で **約 640 論理px**。詳細ヘッダは
画面上部にあるので合計 790 論理px 必要になり、1366×768 や高 DPI + UI 拡大の環境で画面外へ
はみ出す。**egui のメニューは自動スクロールしない** (`find_best_align` が全滅すると
first_choice のまま描画してクリップされる) ため、下端の項目が操作不能になる。
このメニューは詳細ヘッダと下部情報バーのヘッダ ([ui_main.rs](../src/ui_main.rs) ≈11676) の
両方から開く。

### 修正方針

**2 列レイアウトにする。**

```
左列: 表示する列 (見出し)          右列: 書式 (すべての表示で共通) (見出し)
  名前 (固定・disabled)              サイズ表示  ◉最適 ○バイト ○KB ○MB
  名前の幅を自動調整                  日時       ☑秒まで表示
  プレビュー / ★ / タグ / 種類 /      行表示     ◉線のみ ○交互背景色 ○線+交互 ○なし
  ページ数 / サイズ / 更新日時 / 状態
  作成日時 / 画像解像度 / 長さ /
  動画解像度 / コーデック
```

- 組み方は `ui.horizontal(|ui| { ui.vertical(左); ui.separator(); ui.vertical(右) })`。
  `ui.columns` は egui 0.33 の新しいメニュー実装が `spacing.menu_width` を適用しないため
  幅の明示が必要になる。`ui.separator()` は horizontal 内では縦線として描かれる。
- 左列・右列それぞれの先頭に見出しラベルを置く (`RichText::strong()`)。S6 で左列の見出しを
  状況によって出し分けるので、**見出し文字列は引数で受け取れる形にしておく**。
- 高さは約 380 論理px になる見込み。
- **安全網として全体を `ScrollArea::vertical().max_height(...)` で包む**。max_height は
  画面高から余白を引いた値。CLAUDE.md の方針に従い、popup 内 ScrollArea はホイールが
  背面のサムネイル一覧へ素通りしないよう消費処理をセットで入れる
  (既存の `suppress_menu_button_wheel_passthrough` 相当)。

### 注意

- チェックボックス / ラジオのクリックでメニューが閉じない現状の挙動を変えない。
- メニューの上下キー移動は縦 1 列前提の作りなので 2 列だと不自然になるが、コマンドメニュー
  ではなく設定パネルなので許容する。

### テスト

描画のユニットテストは無い。既存の UI スナップショットテストに影響が出ないことを確認し、
実機で高さと操作性を確認する。

### ドキュメント

`htdocs/mimageviewer/manual/grid.html` に列設定メニューの説明があれば表現を合わせる。

### 実機フィードバック後の補正 (2026-07-28)

最初の 2 列化では、実機で popup がウィンドウ右端まで広がり、左列だけが縦 1 列で見え、
右列が画面外へ押し出された。原因は egui 0.33.3 のメニュー popup が
`Layout::top_down_justified(Align::Min)` を使う一方、`ScrollArea` 直下の `ui.horizontal` と
その中の `ui.vertical` に列幅を指定していなかったことにある。`ScrollArea` の content UI は
popup の利用可能幅をそのまま受け、左列内の `ui.separator()` がその `max_rect` 全幅を実使用幅に
確定した。その後ろへ右列を置くため、popup 自身もさらに広がるレイアウトになっていた。

対処は、`ui.horizontal_top` の中で左列を 240px、右列を 200px の
`allocate_ui_with_layout(..., Layout::top_down(Align::Min), ...)` に分け、各 child UI にも同じ
固定幅を設定すること。これにより各列は justified でない独立した `max_rect` を持ち、列内の
区切り線も各列幅で止まる。`ScrollArea` は内容幅へ shrink するため、popup 全体も 2 列 + 中央の
区切り線と spacing の幅に収まる。左右で高さが違っても上端は揃う。

番人として
`details_column_context_menu_layout_tests::menu_width_is_bounded_and_columns_are_side_by_side`
を追加した。egui_kittest で 1200px の `top_down_justified` 親 UI を再現し、メニュー本体が
500px 以下、左右が 240px / 200px、上端揃え、x 範囲が非重複、右列がメニュー矩形内であることを
実測する。列幅を外して利用可能幅へ追従すると、今回と同じく左列が 1200px 側へ伸び、このテストが
失敗する。S6 で同じメニューを分岐するときも、メニュー直下の利用可能幅を列 UI へ渡さないこと。

### 実機フィードバック後の高さ補正 (2026-07-28)

2 列表示後、ウィンドウに十分な高さがある環境でも縦スクロールバーが出て全項目を一度に確認
できない退行が見つかった。原因は egui 0.33.3 の `ScrollArea::begin` が外枠サイズを
`ui.available_rect_before_wrap().size().at_most(max_size)` で決めることにある。
自動サイズの menu popup 内では親 `Ui` の `available_height()` が中身より小さい値になり得るため、
画面高から求めた `ScrollArea::max_height` が十分でも、親の利用可能高が先に上限となっていた。

最初の対処は 2 パス計測とした。menu 固有の `egui::Id` へ実高さを temp data として memo し、
初回だけ 420px を使う。次フレーム以降は memo を `allocate_ui_with_layout` で親領域として明示確保し、
その中へ同じ高さの `ScrollArea` を置く。高さ方向は `auto_shrink` を無効にして親領域を使い切らせる
一方、横方向は従来どおり内容幅へ縮めて 2 列の幅回帰を避ける。

ただし、中央に `ui.separator()` を置いたまま左右 pane の最大高を memo した版では、十分な高さでも
ほぼ全高の scrollbar が残った。OS 125% + アプリ UI 130%、popup y=0.3pt の再現条件で実測すると、
左右 pane 最大高と viewport は 347.78125pt だが、horizontal layout の available height まで伸びた
separator と ScrollArea content は 348.31250pt だった。差は 0.53125pt で、separator の伸長分と
完全に一致した。末尾 `item_spacing.y` や ScrollArea 内側 margin の追加寄与はなかった。

最終形では、中央に `egui::Separator::default()` と同じ 6pt 幅だけを高さ 0 の gap として予約する。
左右 pane を描いて矩形を実測した後、gap 中央へ
`ui.visuals().widgets.noninteractive.bg_stroke` を使った `Painter::vline` を、両 pane の上端から最大下端
まで描く。線は content 高の決定に参加しないため、memo は `ScrollAreaOutput::content_size.y` そのもの
を使える。親領域の高さは
`min(memo + 2 device px / effective pixels_per_point, 画面予算)` とし、端の物理 pixel 丸めにも余裕を
持たせる。画面予算は従来どおり
`content_rect().height() - DETAILS_COLUMN_MENU_SCREEN_MARGIN` であり、これを超える場合だけ scrollbar を
残す。

高さの番人として次を追加した。

- `details_column_context_menu_layout_tests::tall_screen_shows_the_full_menu_without_scrolling`:
  900px 画面、小さく評価された自動サイズ popup 親、OS 125% + UI 130%、小数 popup 座標を再現する。
  `content_size.y <= viewport_rect.height() + 0.5` を直接検証する。左右 pane 最大高を memo し中央に
  伸長 separator を置く修正前実装では content 348.31250pt / viewport 347.78125pt となり失敗する。
- `details_column_context_menu_layout_tests::short_screen_bounds_the_menu_and_keeps_it_scrollable`:
  200px 画面では viewport が画面予算 152px 以内、content が viewport より高いことに加え、末尾を
  `scroll_to_me` した後の offset が増えることを確認する。

S6 で同じメニューの内容を分岐して高さが変わっても、layout に参加しない中央線、ScrollArea の実
content 高を memo する所有境界、2 device px の丸め余裕、画面予算を唯一の最大高とする契約を
維持すること。

---

## S6. 下部情報バーの表示項目をサムネイル表示 / 詳細表示で分ける

### 完了状況 (2026-07-28)

- **S6a 実装済み**: 3 値モードとセット C の永続データ、sanitize、セット A → C のコピー、
  環境設定 OK 時の live/snapshot マージ後コピー、モード別の読み取り経路、詳細表示時だけの
  バー非表示と遅延メタデータ抑止、環境設定 UI、回帰テスト、仕様 / 設定マニュアル。
- **S6b 実装済み**: 下部情報バーのヘッダ境界ドラッグ、ダブルクリック best-fit、書き込み先セットを
  切り替える共通リサイズ処理、best-fit job の対象セット対応とサムネイル表示中の advance、
  右クリックメニューの状況別見出しと A/C 編集対象切替、C からの
  「一覧と同じ設定 / 専用の設定」トグル、回帰テスト、仕様 / 設定マニュアル。
- **S6 完了**: S6a / S6b を合わせ、読み取り・編集・モード遷移・永続化の全経路を実装済み。

### 要望

詳細表示では下部情報バーがカーソル行と全く同じ項目になり意味がない。詳細表示ではファイル名
だけにしたい。サムネイル表示時は現状のままでよい。

### 現状

[src/ui_main.rs](../src/ui_main.rs) `render_selection_info_bar` (≈13324) は
`DetailsColumnSet::TextOnly` で詳細表示と**同じ列設定** (`details_column_order` /
`details_column_widths` / `details_show_*` / `details_name_width*`) を使い回している。
`TextOnly` の違いはプレビュー列を除外するだけ (≈1055)。

### 決定した設計モデル

設定が効く面は 3 つある:

| 面 | 説明 |
|---|---|
| **A** | サムネイル表示時の下部情報バー |
| **B** | 詳細表示のヘッダ + 行 (一覧そのもの) |
| **C** | 詳細表示時の下部情報バー |

**A = B は常に成立させる** (= 既存の `details_*` 設定セット。以下「セット A」)。
分岐するのは **C だけ**。

- 共通モード: C もセット A を使う (= 現状動作)
- 別々モード: C は専用セット (以下「セット C」) を使う
- 非表示モード: C を描画しない

「別々」へ切り替えた瞬間に**セット A をセット C へ複製する**。別々 → 共通 → 別々と往復すると
再度 A から複製されて上書きされる。これは仕様として許容する (ユーザー確認済み)。

**プレビュー列はバー (A・C) では常に自動除外**する (現状の `TextOnly` の挙動を維持)。

### 設定

`Settings` に追加:

- モード enum (3 値 + `#[serde(other)] Unknown`、`normalized()` で既定へ正規化)。
  既存の `SelectionInfoDisplayMode` (≈516) を手本にする。既定は「一覧と同じ設定」
- セット C 用フィールド: 列表示 ON/OFF 13 個 + `..._column_order` +
  `..._column_widths` + `..._name_width` + `..._name_width_auto`

**`overwrite_non_preferences_from` ([src/settings.rs](../src/settings.rs) ≈6237) への追記が必須。**
セット C のフィールドは環境設定ダイアログではなく右クリックメニューとヘッダドラッグで編集する
ため、既存の `details_*` と同じくここに列挙しないと、環境設定を開いている間の編集が OK 押下で
消える。

### コピー実行タイミングの罠

モード切替を環境設定ダイアログのコンボで行う場合、**スナップショット上でコピーしてはいけない**。
OK 押下時に `overwrite_non_preferences_from` が live の値でセット C を上書きしてコピーが消える。

→ コピーは [src/ui_dialogs/preferences.rs](../src/ui_dialogs/preferences.rs) ≈1506-1514 の
`overwrite_non_preferences_from` の**直後**、`self.settings = state.settings` の直前で、
「旧モード (live) vs 新モード (snapshot)」を比較して実行する。同じ場所に
`reading_history_limit` の clamp など後処理の先例がある。

コピー本体は `Settings` の 1 メソッドに集約し、環境設定経由とメニュー経由の両方から呼ぶ。

### UI

#### 環境設定

「選択情報の表示」([src/ui_dialogs/preferences/pages.rs](../src/ui_dialogs/preferences/pages.rs)
≈613-626) にコンボを追加:

> 詳細表示時の下部情報バー: ◯一覧と同じ設定 ◯専用の設定 ◯表示しない

#### 右クリックメニュー (S5 の 2 列レイアウトを使う)

メニューの構造はどの面から開いても同一。**左列の見出しだけ状況で変える**:

| モード | 開いた場所 | 左列の見出し |
|---|---|---|
| 共通 | A / B / C どこでも | 一覧と下部情報バー共通 |
| 別々 | B / A | 一覧・サムネイル表示の下部情報バー |
| 別々 | C | **詳細表示の下部情報バー専用** |

右列の見出しは常に「書式 (すべての表示で共通)」。右列の項目 (サイズ表示 / 日時 / 行表示) は
別々モードでも**常にセット A と共有**する (同じデータの書式を分ける意味が薄く、行表示は
1 行のバーでは無意味なため)。

C のメニューには「一覧と同じ設定にする ⇔ 専用の設定にする」のトグルも置く (その場で切り替え
られないと混乱するため)。「表示しない」だけはバーごと消えて入口が無くなるので環境設定のみ。

現在バーのヘッダ右クリックは一覧用のメニューを開いている ([ui_main.rs](../src/ui_main.rs) ≈11676)
ので、どのセットを編集するかを渡す形に変える。

### 列幅・レイアウトの読み取り経路

幅を読む本番コードは ui_main.rs に閉じている:

| 関数 | 位置 | 現状の引数 |
|---|---|---|
| `details_column_width` | ≈1282 | `&Settings, col` |
| `details_name_fixed_width` | ≈1323 | `&Settings` |
| `details_fixed_columns_width` | ≈1754 | `&Settings` |
| `details_layout` | ≈1762 | `&Settings` |
| `details_content_width_for_column_set` | ≈1802 | `&Settings, column_set` |
| `details_column_rects_for_columns` | ≈1815 | `&Settings, column_set` |

`DetailsColumnSet` を「どの設定セットを見るか」の判別子へ拡張する (`All` / `TextOnly` を
セット A 用 / セット C 用に整理し直す)。既に `column_set` を受け取っている 3 関数は引数追加
不要。分岐が要るのは末端の 2 アクセサ、引数追加が要るのは `details_layout` と
`details_fixed_columns_width`。呼び出し側の追加は ≈13348 / ≈13359 程度。

### C のヘッダを編集可能にする

現在 `draw_details_header_static` (≈11642) は `Sense::click()` のみでリサイズできない。
別々モードで幅を独立させるので、**リサイズドラッグとダブルクリック best-fit を付ける**。

- 詳細ヘッダ側のリサイズ処理 (≈12122-12170、約 50 行) を共通ヘルパーに切り出し、書き込み先の
  設定セットを引数で受ける。列の並べ替えドラッグ (`DetailsHeaderDrag`) とソートクリックは
  バーには不要 (1 行なので意味がない) ので切り出さない
- best-fit の測定母集団は**一覧と同じ全アイテム** (バーは 1 行だが、選択のたびに幅が変わると
  落ち着かないため)。サムネイル表示 / 詳細表示のどちらでも同じ挙動にする
- ジョブはグローバルに 1 本 (`details_best_fit_job_id` ≈11681)。`DetailsBestFitJobKey` (≈950) に
  対象セットのフィールドを追加し、`apply_details_best_fit_width` (≈11792) が書き込み先を
  切り替える。ジョブ自体は 1 本のままでよい (母集団・フォント・ppp が同じ)
- ⚠️ **`advance_details_best_fit_job` は現在 `draw_details_header` (≈12007) からしか毎フレーム
  呼ばれていない。サムネイル表示では詳細ヘッダが描かれないため、バーで開始したジョブが
  永久に進まなくなる。`draw_details_header_static` からも advance を呼ぶこと。**

### 遅延メタデータ

`selection_info_bottom_bar_shows_column` (≈1265) は、バーのために必要なメタデータを遅延読み込み
するかの判定に使われている ([src/app.rs](../src/app.rs) ≈39857-39995 の約 15 箇所)。
`grid_view_mode` は `Settings` のフィールド (≈2773) なので**シグネチャを変えずに**モードと
セットを見て判定できる。非表示モードのときバー用の遅延読み込みが止まることを確認する。

### テスト

- 設定のラウンドトリップ (セット C のフィールドが保存・復元されること)
- コピー処理: 共通 → 別々でセット A がセット C へ複製されること / 別々 → 共通 → 別々で
  再度複製されて上書きされること
- `overwrite_non_preferences_from` でセット C が live から引き継がれること
- モード別に `details_visible_columns` / `details_column_rects_for_columns` が正しいセットを
  参照すること (既存のレイアウトテスト群 ≈14735-15430 をセット別に拡張)
- 非表示モードで `render_selection_info_bar` が早期 return し、遅延読み込み判定も false になること

### ドキュメント

- [docs/spec.md](spec.md) — 設定項目
- `htdocs/mimageviewer/manual/grid.html` / `settings.html` — 下部情報バーの説明
