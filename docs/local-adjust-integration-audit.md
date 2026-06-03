# 補正レイヤー結合 — 機能別監査チェックリスト

ラボツール (`tools/local_adjust_lab/src/main.rs`, 25,489 行) を mIV 本体に結合する作業の
**機能単位での突き合わせ監査**。Codex への修正指示書として使う。

## 2026-06-03 Codex 対応状況

今回の修正で以下を mIV 側に反映済み。

- ✅ レイヤーカラー切替ボタンを左パネルへ復元。
- ✅ 「選択レイヤーまでプレビュー」を表示専用の非同期 prefix preview として復元。
- ✅ マスクプレビュー色をプリセット化し、ラボ寄りの透明度へ調整。
- ✅ マスクプレビューを粗い矩形グリッドからプレビュー画像描画へ変更し、ギザつきを軽減。
- ✅ 手動マスク作成時の寸法を、補正レイヤー処理入力画像の寸法に合わせるよう修正。
- ✅ `1px拡張` / `1px縮小` を復元。
- ✅ RasterVector の右パネル操作を `ビットマップ消去` / `オブジェクト消去` の2つに戻す。
- ✅ ダイアログ表示中のキャンバス入力・オーバーレイ描画を抑止。
- ✅ 線形/円形グラデーションのハンドル hit-test とドラッグを復元。
- ✅ グラデーション未初期化時の右パネルUIをラボ同様「画像上でドラッグ」に戻す。
- ✅ 補正レイヤーモードのショートカット `D/F/B/A/G/L/P/S/I/V/H/R/O` を復元。
- ✅ 処理中表示は現在世代の render pending のみを対象にするよう修正。

残りは主に実機確認項目。

- 輝度マスク / カラーマスクは、寸法基準とプレビューソース取得の修正で改善している想定。実画像で反映確認する。
- 被写体 / 領域マスクは自動生成経路が残っていることをコード確認済み。モデル配置環境で実機確認する。
- 効果別パラメータUIは既存レビュー通り全効果カバー済み。代表効果で実機スモーク確認する。

> 注: 各項目内の「mIV 現状」は監査作成時点の記録として残している。項目冒頭に
> 「2026-06-03 Codex 対応」がある場合は、その対応内容が最新状態。

## 進め方

1. 機能カテゴリ A〜L の各項目について、**ラボ実装の位置** と **mIV 現状の位置 / 状態** を
   コード行レベルで突き合わせる。
2. 判定: ✅ 一致 / ⚠️ 部分一致 (動くが見た目/挙動が違う) / ❌ 欠落 / 🐛 壊れている
3. ⚠️ / ❌ / 🐛 の項目は **修正指示** に「何を / どこに / どう書くか」を具体化する。
4. ラボのコードをそのまま移植するのが基本方針。mIV 独自の「改良」と思しき差分は
   原則差し戻して、ラボの挙動・見た目に合わせる (ラボはユーザーが時間をかけて UX を
   詰めたリファレンス実装)。

凡例:
- `lab:NNNN` = `tools/local_adjust_lab/src/main.rs` の行番号
- `panel:NNNN` = `src/ui_adjustment_panel.rs` の行番号
- `app:NNNN` = `src/app.rs` の行番号
- `effect_ui:NNNN` = `src/local_adjust_effect_ui.rs` の行番号

---

## A. レイヤーパネル (左) — `draw_layer_list` 系

### A-1. レイヤーカラー切替ボタン行 (MaskColorPreset) ✅ **修正済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: mIV の左パネルへ `LocalAdjustMaskColorPreset::ALL`
  の色サンプルボタン行を追加。クリックで `local_adjust_mask_color_preset` を更新し、
  マスク表示色に反映する。

- **ラボ**: `lab:4422-4453` (`draw_display_controls`) — `MaskColorPreset::ALL` を逆順で
  並べた色サンプルボタン群 (PinkCyan / CyanOrange / 他)。クリックすると `mask_color_preset`
  が切り替わり、`reveal_mask_preview()` + `mask_dirty = true` が立つ。サイズは `24x18`、
  各プリセットの代表色で塗る。
- **mIV 現状**: `panel:558-577` の `draw_local_adjust_left_panel` には「元画像 [Q]」と
  「マスク [W]」ボタンしか無い。`MaskColorPreset` の選択 UI 行が完全に**欠落**。
  - `app:380-393` に `LocalAdjustMaskColorPreset` enum 定義はあるが、UI で書き換える
    手段がないためデフォルト値で固定。
- **修正指示**:
  1. `MaskColorPreset` 行をラボの `draw_display_controls` (4423-4453) と同じレイアウトで
     `panel:558` 直前 (元画像 / マスクボタンの上) に追加。
  2. クリック時に `App::local_adjust_mask_color_preset` を更新し、必要なら
     `local_adjust_layer_bypass_cache.clear()` + repaint を要求 (ラボの
     `reveal_mask_preview() + mask_dirty = true` 相当)。
  3. **下流: B-2 で実際にプリセット色をマスクプレビュー描画に使う**。

### A-2. 「選択レイヤーまでプレビュー」チェックボックス ✅ **修正済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: `+補正レイヤー` 直下へチェックボックスと
  `表示中: 1〜N / total` ラベルを追加。表示専用の prefix preview を非同期生成し、
  通常の補正レイヤー結果とは別キャッシュで管理する。

- **ラボ**: `lab:5434-5453` (`draw_layer_list` 内) — `+補正レイヤー` ボタン直下に
  `ui.checkbox(&mut self.preview_to_selected_layer, "選択レイヤーまでプレビュー")` と
  「表示中: 1〜N / total」ラベルが付く。変更で `mark_dirty()` 発火。
- **mIV 現状**: `panel:646-654` (`draw_local_adjust_layer_list`) では `+補正レイヤー` 直後に
  layer リストへ進んでいて、チェックボックスが**抜けている**。
  - フィールド `local_adjust_preview_to_selected_layer: bool` は `app:3256` に**存在**する
    (初期値 false @ `app:4549`, フォルダ切替でリセット @ `app:8624`)。
  - layer bypass cache / pending インフラ (`app:3319-3322, 18346-18411`) も実装済み。
  - しかし UI からこのフラグを ON/OFF する手段が無いため、現状は常に false のまま。
- **修正指示**:
  1. `panel:654` (`+補正レイヤー` button の `if ui.add_sized(...).clicked() { ... }` の直後)
     にラボの 5434-5453 と同形式のチェックボックス + 件数ラベルを追加。
  2. `select_layer` / フラグ更新を経由してマスクキャッシュ無効化 + repaint を要求する経路を
     確認 (ラボでは `mark_dirty()`, mIV では `local_adjust_layer_bypass_cache` クリア相当)。
  3. Ctrl+Shift によるキーボード bypass プレビューはラボの `layer_bypass_preview_active`
     (`lab:4963`) 経路。mIV にも同等のキーボード経路があるか別途 G カテゴリで確認する。

### A-3. レイヤーリスト本体・行レイアウト ✅ ほぼ一致

- ラボ `lab:5454-5607` と mIV `panel:664-806` を突き合わせた結果、配色 / 行構造 /
  「前」「後」ボタン / サムネ / マスク種類ラベル / 効果ラベル / OFF ラベル / クリック領域は
  ほぼ一致。
- 残課題: 行クリックの cursor (`PointingHand`) と spacer の挙動も揃っている。問題なし。

### A-4. ↑ / ↓ / 複製 / 削除 行 ✅ 一致

- ラボ `lab:5609-5633` / mIV `panel:808-844` でレイアウト / 配色 / disable 条件が揃って
  いる。問題なし。

---

## B. 表示制御 — 元画像 / マスク / プレビュー切替

### B-1. 元画像 [Q] / マスク [W] トグル ✅ 一致

- ラボ `lab:4454-4466` / mIV `panel:559-577`。レイアウト・ショートカット・トグル動作が
  一致。問題なし。
- ⚠️ ただし A-1 で MaskColorPreset 行を追加する場合、この 2 行は**その下**に置く
  (= ラボの並び順と同じく「色プリセット → 元画像 / マスク」)。

### B-2. マスクプレビュー描画の配色 / 不透明度 ✅ **修正済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: ハードコード色を廃止し、A-1 の
  `local_adjust_mask_color_preset.colors()` を使用。ベース不透明度もラボ寄りへ下げ、
  さらに粗い矩形グリッドではなく縮小プレビュー画像として描画する。

- **ラボ**: `lab:5137-5170` (`draw_mask_tile_preview`) — `self.mask_color_preset.colors()` から
  edit/base 色を引き、`base(80)` 〜 `base(145)` のレンジで alpha 制御。プリセット切替に
  追従して色相も alpha も変わる。
- **mIV 現状**: `panel:4292-4361` (`draw_local_adjust_mask_preview_overlay`) は色を
  **`egui::Color32::from_rgba_unmultiplied(255, 80, 125, a)` にハードコード**、alpha は
  `(55.0 + alpha * 175.0)` で 55〜230 のレンジ。ラボより大幅に不透明側に振っていて、
  ユーザー指摘「mIV 側は不透明度が高く見づらい」と整合する。
- **修正指示**:
  1. A-1 を先に入れる (色プリセット選択 UI を有効化)。
  2. `panel:4292-4361` のハードコード色を `App::local_adjust_mask_color_preset.colors()`
     経由に置換。`base/edit` 関数を `MaskColorPreset.colors()` で取得する仕組みを mIV
     側にも作る (ラボの `lab:912-` 周辺の `impl MaskColorPreset` を参照)。
  3. alpha レンジを `base(80)` 〜 `base(145)` 相当 (ラボの上限 145 / 下限 80) に下げる。
     `55.0 + alpha * 175.0` ではなく、たとえば `35.0 + alpha * 110.0` 程度。
  4. 視認テスト: ラボ単体と並べて開き、同じレイヤー / マスクで配色が同じになるか確認する。

### B-3. Segmentation 領域の境界アニメーション ✅ 一致

- mIV `panel:4313-4336` で `local_adjust_region_boundary_color(label, time_sec)` を
  使っていて、ラボの相当ロジックを移植済み。

---

## C. マスク種類別エディタ — 右パネル下半分

### C-1. RasterVector マスクのボタン (致命的差分) ✅ **修正済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: 「ビットマップ塗り」を削除し、
  「図形消去」をラボ同様の「オブジェクト消去」へ戻した。

- **ラボ**: `lab:8847-8860` (`draw_mask_controls` の `LocalMask::RasterVector` arm)
  - **「ビットマップ消去」**: `mask.alpha.fill(0.0)` (ラスター部分をクリア)
  - **「オブジェクト消去」**: `mask.shapes.clear()` (図形リストをクリア)
  - ボタンは 2 つだけ。
- **mIV 現状**: `panel:4643-4663` (`draw_local_mask_editor` の `LocalMask::RasterVector` arm)
  - 「ビットマップ消去」: `mask.alpha.fill(0.0)` — ✅ 名前一致
  - **「ビットマップ塗り」: `mask.alpha.fill(1.0)` — ❌ ラボには無い、勝手に追加されている**
  - **「図形消去」: `mask.shapes.clear()` — ❌ ラボでは「オブジェクト消去」**
  - ボタン 3 つ並んでいる。
- **ユーザー指摘との対応**: 「ビットマップ塗り、図形消去などおかしな物が増えていて、
  必要なオブジェクト消去がなくなっています」と完全に一致。
- **修正指示**:
  1. `panel:4649-4652` の「ビットマップ塗り」ボタンを**削除**。
  2. `panel:4653` の「図形消去」を**「オブジェクト消去」**にリネーム (ロジックはそのまま)。
  3. 結果としてラボと同じ 2 ボタン構成になる。

### C-2. Raster マスクの「クリア / 塗りつぶし」ボタン ✅ 一致

- ラボ `lab:8827-8839` と mIV `panel:4630-4642` は同じ名前 / 動作。問題なし。
- ⚠️ ただしユーザー指摘「塗りつぶしを実行しても、塗りつぶされません」が出ている。
  ボタン文字列としては正しいが**実機で fill が反映されない**バグの可能性。
  → C-3 / E カテゴリ参照。

### C-3. Raster マスク塗りつぶし不発バグ ✅ **寸法経路修正済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: マスク作成・描画で参照する画像寸法を、
  元画像メタデータではなく補正レイヤー処理に使う実際の `ColorImage` 寸法へ合わせた。
  `RasterMask::alpha.fill(1.0)` の単体動作は既存テストと全体テストで確認済み。

- ユーザー指摘: 「手動マスクで、塗りつぶしを実行しても、塗りつぶされません。おそらく
  手動マスクが壊れていそうです。」
- 仮説 (優先順):
  1. **マスクバッファの寸法が image_dims とミスマッチ**: `panel:4636` で `mask.alpha.fill(1.0)`
     は実行されるが、`mask.width * mask.height != mask.alpha.len()` だとプレビュー側が描画
     できない / 0 とみなす。
  2. **changed フラグ反映漏れ**: `changed = true` は立つが、`draw_local_mask_editor` の
     呼び出し元 (`panel:1397`) でその戻り値を使って `local_adjust_layer_bypass_cache.clear()`
     と repaint を発火していない。
  3. **マスクは fill されているが mask preview overlay が走らない**: `show_mask` が
     OFF なのにユーザーは確認している → 表示状態の問題。
- **修正指示**:
  1. **再現**: 補正レイヤー追加 → 手動マスク作成 → 「塗りつぶし」クリック。
     `mask.alpha` の `iter().sum()` を log で出力して fill 後に 1.0 が反映されているか
     確認 (panel.rs に一時的に `dbg!` を入れる)。
  2. fill 反映後、`mark_dirty / cache clear / repaint` が走るかも確認。
     `panel:1397` 周辺の `changed |= draw_local_mask_editor(...)` の使い方を確認し、
     `changed` 時に `self.local_adjust_layer_bypass_cache.clear()` + repaint を実行する
     経路があるか追跡。なければ追加。
  3. **下流: D-1 と E の解像度問題が直れば、ここも自然に直る可能性がある** (= 解像度
     ミスマッチが本当の原因)。

### C-4. LinearGradient / RadialGradient エディタ ✅ **修正済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: ラボに無い「作成」ボタンと `有効` チェックを外し、
  未初期化時は「画像上でドラッグして範囲を生成します。」のみ表示するよう戻した。

- **ラボ**: `lab:8861-8911` — 未初期化時は「画像上でドラッグして範囲を生成します」と
  指示のみ表示し、UI でハンドルは作らせない。初期化後にクリア + 4〜6 スライダー。
- **mIV 現状**: `panel:4665-4703` — 未初期化時に「上下グラデーションを作成」「中央円形
  グラデーションを作成」ボタンを置き、押すと初期値で作成する。ラボには無い動線。
- **判定**: 機能としては動くが UX がラボと違う (ラボはキャンバスドラッグで作成を強制)。
- **修正指示**:
  1. ラボに合わせて「上下グラデーションを作成」「中央円形グラデーションを作成」ボタンは
     **削除**してよい。代わりに `mask.initialized == false` のとき「画像上でドラッグして
     範囲を生成します。」ラベルだけ出す (ラボ `lab:8862, 8884`)。
  2. ただし F-1 (キャンバスでのドラッグ作成) が壊れている報告 (= 線形/円形マスクでハンドル
     ドラッグできない) があるので、先に F-1 を直してからこの UI 簡素化をする。
  3. mIV 側の `mask.initialized` チェックボックス (`panel:4675, 4694`) はラボには無いが、
     クリアボタン (= 未初期化に戻す) の代わりとして残してもよい。判断は実機優先。

### C-5. LumaRange / ColorRange エディタ — UI 自体は移植済み ✅
### C-6. Subject / Segmentation エディタ ✅ ほぼ一致

- 値の操作 UI は揃っている。ただし「動かない」報告 (C-7) は別途。

### C-7. 「輝度マスク、カラーマスクは全く動いていないように見えます」⚠️ **寸法・ソース経路修正済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: プレビュー評価に渡す寸法とソース画像の取得経路を
  補正レイヤー処理入力に揃えた。輝度/カラー範囲の core 評価は既存実装を使用するため、
  残りは実画像での反映確認対象。

- ユーザー指摘: スライダーを動かしてもプレビューに反映されない / マスクが計算されない
  挙動の可能性。
- 仮説 (優先順):
  1. **`changed = true` は立つが repaint / cache 無効化が走らない** (C-3 と同じ系統)。
  2. **`local_adjust_luma_range_preview_alpha / color_range_preview_alpha` が常に 0 を
     返す**: `panel:4410-4414` で `source` が `None` の場合これらは 0.0 を返す可能性が
     高い。`source` (元画像 ColorImage) の渡し方を確認する。
  3. **`mask.initialized` が立っていない**: ColorRange は spoit クリックで `initialized = true`
     になる設計だが、mIV の初期化フローに穴がある可能性。
- **修正指示**:
  1. **再現**: 輝度マスク追加 → 範囲スライダー操作。プレビューが変化するか確認。
     ColorRange は「白を対象色にする」ボタンで初期化されるはず (`panel:4713`) — 押した
     あと R/G/B/許容/ぼかしスライダーが効くか確認。
  2. プレビュー描画 (`panel:4292`) に渡している `source` が `Some` になっているか
     `panel:6707` (`draw_local_adjust_canvas_overlay` 内の呼び出し) で確認。
  3. `evaluate_layer_mask` / `apply_local_adjust_layers` (core 側) が LumaRange /
     ColorRange を実際に評価しているかも確認。
  4. 反映経路が切れていれば C-3 と同じく `changed → cache clear → repaint` を追加。

---

## D. 手動マスクツールバー (左パネル下)

### D-1. ツール並びとセクション ✅ 一致

- ラボ `lab:4854-4936` (`draw_manual_mask_tool_panel`) と mIV `panel:1062-1144` を
  突き合わせた結果、`描画 [D] / 消去 [F]` / `ビットマップ:` ヘッダ / 5 ツール
  (Brush / EdgeBrush / GapFillBrush / Lasso / Polygon) / `オブジェクト:` ヘッダ /
  6 ツール (Select / Line / VertLine / HorizLine / Rect / Ellipse) は全て一致。

### D-2. 「1px拡張」「1px縮小」ボタン ✅ **修正済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: `LocalAdjustBitmapMaskOp::{Expand,Shrink}` と
  3x3 の 1px 膨張/収縮処理を追加し、左パネルの手動マスクツールに 2 ボタンを復元。
  base / override add / override subtract の現在編集対象へ適用する。

- **ラボ**: `lab:4898-4912` — `ビットマップ:` セクションのツール行と「オブジェクト:」
  セクションの間に挟まる形で 2 ボタン (`apply_bitmap_mask_op(BitmapMaskOp::Expand/Shrink)`)。
  - `lab:865-`: `enum BitmapMaskOp { Expand, Shrink }`
  - `lab:4136-`: `fn apply_bitmap_mask_op(&mut self, op: BitmapMaskOp)` — RasterVector
    だけでなく Raster にも適用 (mask kind で分岐)
  - 内部: `dilate_alpha / erode_alpha` で 1px の形態学的処理。
- **mIV 現状**: `panel:1112` の「オブジェクト:」ヘッダの直前に何もない。
  - mIV 側に `BitmapMaskOp` / `apply_bitmap_mask_op` / `dilate_alpha` / `erode_alpha`
    に相当するコードは**存在しない** (grep 確認済み)。
- **修正指示**:
  1. ラボの `lab:865-` の `BitmapMaskOp` 定義と `lab:4136-` の `apply_bitmap_mask_op`
     関数本体、および `dilate_alpha / erode_alpha` (lab 内のヘルパー、別途位置確認) を
     mIV に移植。core クレートに置くと再利用しやすい (`crates/local-adjust-core/src/lib.rs`
     の Raster / RasterVector まわり)。
  2. `panel:1111` (ビットマップツール行の最後の閉じ括弧の直後、`オブジェクト:` ラベルの直前)
     にラボの `lab:4898-4912` と同じレイアウトで 2 ボタンを追加。
  3. クリック時に App 側の対応 method を呼び、現在編集中のマスク (`mask_edit_target` で
     base / overrideAdd / overrideSubtract を切り分けたうえで) に対して 1px expand/shrink
     を適用 → cache クリア + repaint。
  4. 動作確認: ラボで動かして 1px 単位で広がる / 縮むことを目視し、mIV で同等の挙動になるか
     確認。

### D-3. 「描画 [D] / 消去 [F]」「筆[B] / 境界筆[A] / 隙間補完[G] / 囲み[L] / 多角形[P]」 ✅ 一致
### D-4. 「選択[S] / 直線[I] / 縦線[V] / 横線[H] / 矩形[R] / 楕円[O]」 ✅ 一致

---

## E. 手動マスクの実体 — 解像度 / 描画品質

### E-1. 「mIV 側で手動マスクを作ると解像度が低くギザギザの洗いマスクになります」✅ **寸法・表示品質修正済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: 手動マスクの寸法基準を補正レイヤー処理入力画像に合わせ、
  マスク表示も縮小プレビュー画像方式へ変更。実体の低解像度化と表示上の粗さの両方を
  低減した。

- **症状**: 同じブラシで手動マスクを作っても、ラボより mIV のほうが粗い (= 1 pixel あたりの
  マスク値が荒い、ぼかしが効いていない、段差が見える)。
- **仮説**:
  1. **マスクバッファが画像解像度ではなく表示解像度で作られている**: mIV の RasterMask /
     RasterVectorMask の `width / height` 初期化を確認。ラボでは `image.source.width *
     image.source.height` で作る (例: lab の Raster::empty 呼び出し位置)。
  2. **描画時の補間 / ぼかしが効いていない**: mIV のブラシペイント関数が
     `local_adjust_screen_to_pixel` でピクセルを 1 つだけ立てているとギザギザになる
     (ラボはアンチエイリアス付きの円ブラシ)。
  3. **プレビュー描画のグリッド分解能の問題ではない**: B-2 で確認したとおり mIV と
     ラボのプレビュー grid 解像度の式は同じ (`width / 12.0, [32, 128]`)。よって
     プレビュー側の問題ではなく**マスク実体側**の解像度問題。
- **修正指示**:
  1. **再現**: 同じ画像で同じツール / 半径でブラシを引き、ラボと mIV の `RasterMask::alpha`
     の `(width, height)` を log で出力して比較する。
  2. mIV の `paint_local_adjust_mask_tool_segment` (`panel:6206`) の挙動 (特に円ブラシ
     生成部) をラボの `lab:???` (要特定: `MaskTool::Brush` の handle 部分) と並べて確認。
     fall-off / アンチエイリアス計算が消えていれば復元する。
  3. **マスク作成時の寸法**: mIV で RasterMask / RasterVectorMask を新規作成する箇所
     (例: `+ 補正レイヤー → 手動マスク` ダイアログ確定時) で
     `RasterVectorMask::empty(image_width, image_height)` のように **画像解像度** で
     作っていることを確認。表示解像度 (例: 1280 x 720) で作っていれば 4K 元画像で
     ギザギザになる。
  4. ラボ単体と mIV を並べて、同じ画像 / 同じツール設定で描いた結果を screenshot して
     比較。

---

## F. キャンバスのハンドル / ドラッグ

### F-1. 線形マスク / 円形マスクのハンドルドラッグ ✅ **修正済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: 線形 start/end、円形 center/inner/outer の
  専用 hit-test と drag kind を追加。14px 半径で掴めるようにし、描画位置と同じ
  座標変換で判定する。

- **症状**: ユーザー指摘「線形マスクや円形マスクでハンドルをドラッグできません」。
  グラデーションマスク追加 → キャンバスに線 / 円が描画される → 端点をドラッグしようとしても
  反応しない。
- **mIV 既存実装**: `panel:6593-6612` でクリックでドラッグ開始の枝が組まれており、
  `panel:5616-5625, 5607-` で `apply_local_adjust_gradient_drag` が `LocalMask::LinearGradient`
  / `LocalMask::RadialGradient` を更新する。コードは存在する。
- **仮説**:
  1. **ハンドル位置のヒットテスト範囲が狭すぎる**: クリック判定が 2-3px しかなく、
     ユーザーが正確にクリックできない。
  2. **ハンドル描画位置とヒットテスト位置がズレている**: `local_adjust_norm_to_screen` の
     座標変換と `local_adjust_screen_to_norm` の逆変換が一致していない。
  3. **`apply_local_adjust_gradient_drag` の中で `LinearGradient/RadialGradient` のケースを
     更新していない**: コード上は組まれていそうだが、実際に `mask.start/end` が書き換わって
     いるか log 確認が必要。
  4. **クリック前に別のハンドル (effect_position / tilt_shift / shape) を先に判定していて
     gradient まで到達していない**: `panel:6321-` の if 分岐の順序を確認。
- **修正指示**:
  1. **再現**: 線形マスクのレイヤーを追加 → C-4 の「上下グラデーションを作成」ボタン
     (将来削除予定) で初期化 → キャンバスの端点ハンドル (上端 / 下端) をクリック・ドラッグ。
     ログで `local_adjust_canvas_drag = Some` になるか確認。
  2. ヒットテスト範囲を 14px 半径程度に確認 (mIV `panel:6427` の TiltShift / EffectCenter
     は `<= 14.0` を使っている)。Gradient のヒットテストはどこに居るか追跡する。
  3. ハンドルが描かれている位置 (`panel:4404-4407` の overlay 経路 + `lab:23417/23448` 相当の
     `draw_linear/radial_gradient_handles`) と、ヒットテスト座標が同じ式で計算されているか
     対比する。
  4. 必要なら gradient ハンドル専用の hit test 関数を `lab:7485-` (`draw_gradient_handles`)
     から移植する (現状 mIV では gradient handle 描画 + hit test の分離が不十分の可能性)。

### F-2. TiltShift ハンドル (focus / outer / inner X/Y) ✅ 一致

- 直近のコミット (`c6f9d001`, `3f205e4c`) で復元済み。`panel:3366-3505, 3877-3920,
  3921-4065` に該当。実機で挙動確認は要だが、コード上は揃っている。

### F-3. EffectCenter (各効果の中心マーカー) ✅ 一致

- `panel:3312-3347` で復元済み。

### F-4. Shape (Rect/Ellipse/Polygon) のハンドルドラッグ ✅ 一致

- `panel:2937-3110, 3111-3311` に handle drawing と drag apply が揃っている。

---

## G. キャンバスのカーソル管理 / ダイアログ干渉

### G-1. 補正レイヤー追加ダイアログ上でもカーソルが + のまま ✅ **修正済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: 補正レイヤー追加 / マスク変更 / 効果選択ダイアログ中は
  キャンバス入力とオーバーレイ描画を早期 return し、drag / brush / shape 状態もクリアする。

- **症状**: ユーザー指摘「補正レイヤーの追加で、マウスカーソルが正しくありません。
  ダイアログの上なのに＋の形状などになったままです。」
- **ラボ**: `lab:5001` で `let dialog_open = self.add_layer_dialog_open ||
  self.effect_picker_dialog_open;` を取り、`lab:5034-5052` の **cursor 設定経路を全て
  `!dialog_open` でガード**している。`draw_brush_cursor` (`lab:7405-7444`) も
  pointer_screen が dialog 上にあるかチェックして cursor を None に倒す経路あり。
- **mIV 現状**:
  - `panel:6102-` の `handle_local_adjust_canvas_input` 冒頭に **dialog_open 早期 return が
    無い**。`add_layer_dialog_open / effect_picker_dialog_open / change_mask_dialog_open` の
    どれかが true でもキャンバス入力ハンドラが回ってしまう。
  - `panel:6219, 6245-6249, 6283, 6293-6305, 6393, 6416, 6439, 6548, 6591-6617, 6640,
    6648, 6669` の `ctx.set_cursor_icon(...)` 群はすべて dialog_open ガードが無い。
  - 結果として「ダイアログを開いた瞬間 / 開いている間にマウスを動かすと、最後の cursor
    (= Crosshair) が永続化される」状態になる。
- **修正指示**:
  1. `panel:6102` (`handle_local_adjust_canvas_input` 冒頭) で
     ```rust
     if self.local_adjust_add_layer_dialog_open
         || self.local_adjust_effect_picker_dialog_open
         || self.local_adjust_change_mask_dialog_open
     {
         self.local_adjust_canvas_drag = None;
         self.local_adjust_mask_brush_stroke = None;
         self.local_adjust_shape_drag = None;
         return;
     }
     ```
     を最初に置く (ラボの `lab:5002-5007` 相当: ダイアログ中は pan_drag_start も
     クリアする)。
  2. `draw_local_adjust_canvas_overlay` (`panel:6673`) でも同じ dialog_open ガードを
     最上位に置き、overlay 描画 (effect position handles / shape outline / gradient
     handles 等) を全てスキップする。これがないと「ダイアログ上だが overlay 経由で
     handle がヒット → cursor 変化」が発生する。
  3. 効果ピッカーダイアログを閉じた瞬間にも一度 `ctx.set_cursor_icon(egui::CursorIcon::
     Default)` を 1 フレームだけ強制設定して残骸を消す。

### G-2. 通常時の cursor 切替 ✅ 一致

- shape drag / gradient drag / brush stroke 中の cursor (Grabbing / Crosshair) はラボと
  一致。G-1 のみが課題。

---

## H. 効果別パラメータ UI

### H-1. 全効果 (106/106) の UI 関数 ✅ 全件カバー

- `src/local_adjust_effect_ui.rs` (10,623 行) にラボの `draw_effect_params` 関連が移植済み。
  以前のレビューでカバレッジ確認済み。
- ⚠️ ただし C-7 の Luma/Color マスクが動かない問題と同根で、効果側でも特定の効果が
  反映されない可能性がある。実機で各カテゴリ (tone_detail / focus_motion / distort) から
  代表的な効果を 1 つずつ動かして確認する。

### H-2. RGB スポイト / 選択色スポイト ✅ 一致

- `panel:6322-6371` で sample → mutate → toast 通知の経路が揃っている。

---

## I. ステータス表示 / 進捗インジケータ

### I-1. 「左上の処理中ステータスが、ずっと処理中のままで最新になりません」✅ **修正済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: 補正レイヤーの「処理中」表示は `fs_idx` だけでなく
  現在世代の `current_local_adjust_key(fs_idx)` と一致する pending だけを見るよう変更。
  古い pending が残っても最新表示を塞がない。

- **症状**: ユーザー指摘。フルスクリーン左上に「処理中」表示が出るが、処理完了後も消えない。
- **mIV の関連箇所**: `grep '処理中'` で出た結果は CacheMaintPending / FTS / PDF など補正
  レイヤー外の場所が多い。補正レイヤー専用の status field は要追跡。
  - `app:3195` に「AI ステータス表示の完了時刻 (全処理完了後に記録、一定時間後に非表示)」
    のコメントあり。AI status と混同している可能性。
- **仮説**:
  1. **AI 処理 (アップスケール / Denoise) のステータスが補正レイヤーの計算と独立に
     カウントされていて、片方が終わってもクリアされない**。
  2. **`local_adjust_layer_bypass_pending` (`app:3322`) が cancel 後に take されないまま
     残る**: 18362 の `cancel_local_adjust_layer_bypass_pending` が呼ばれない経路がある。
  3. **`segmentation_pending` / `subject_pending` の終了通知ハンドラで status を
     クリアしていない**。
- **修正指示**:
  1. フルスクリーンで status を描画している箇所を特定する (`src/ui_fullscreen.rs` の
     左上テキスト描画 + `App::status_text` 相当)。
  2. 補正レイヤー関連のすべての pending / busy フラグ (layer_bypass_pending /
     segmentation_pending / subject_pending / mask_dirty 等) を列挙し、完了通知の
     受信ハンドラで全て `None` にリセットする経路を確認する。
  3. 完了後に `take()` 漏れ箇所を修正し、`ctx.request_repaint()` で UI を更新する。
  4. **動作確認**: 補正レイヤーを 1 つ追加 → 効果を適用 → ステータスが「処理中」→
     完了したら消えるか確認。

---

## J. キーボードショートカット

### J-1. Q / W / D / F / B / A / G / L / P / S / I / V / H / R / O ✅ **修正済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: 補正レイヤーモード中に D/F と
  B/A/G/L/P/S/I/V/H/R/O のマスクツールショートカットを復元。

- ラボはキー単独で各ツール / プレビューモード切替に割り当て (`lab:4456, 4461, 4863,
  4878, ...` のラベル参照)。
- mIV のショートカット処理経路は未確認。実装されているか、フルスクリーンモードで衝突が
  ないか (Q が「終了」など) を確認する。
- **修正指示**: `src/ui_fullscreen.rs` の補正レイヤーモード分岐内で、ラボと同じキーが
  同じ動作に割り当てられているか目視確認。欠落 / 衝突があれば修正。

### J-2. Ctrl / Ctrl+Shift / Alt / Shift モディファイア ✅ **コード確認済み / 実機確認待ち**

- **2026-06-03 Codex 対応**: Ctrl は元画像表示、Ctrl+Shift は選択レイヤー基準の
  bypass 表示、Alt はマスク表示反転の経路を確認。Shift ルーペは既存の
  フルスクリーン側挙動として維持。

- ラボ: Ctrl = 元画像表示、Ctrl+Shift = 選択レイヤー除外、Alt = マスク反転、
  Shift = ルーペ (`lab:4599 ヘルプ文字列参照`)。
- mIV: A-2 でレイヤーバイパスの infra は確認したが、Ctrl+Shift キーで発火する経路の
  実装と動作を実機確認する。

---

## K. ステージ切替 (Adjust / Crop / Save 等)

### K-1. ステージ切替 UI ⚠️ **要再確認**

- ラボの workflow_panel 切替 (`lab:4469-4488`) は mIV の export 経路統合で
  `panel:8209 draw_adjustment_panel` 周辺と統合済み (`b10d407f`)。実機で各ステージへの
  遷移と戻りに退行がないか確認する。

---

## L. 永続化 / Sidecar

### L-1. Sidecar 互換 ✅ 一致

- 直近の `a47f4e65` で `import_to_dbs(local_adjust_db, export_crop_db)` シグネチャ追加
  + 全テスト pass。

### L-2. クリップボード コピー / ペースト ✅ 一致

- `effect_clipboard` 経路は移植済み。

### L-3. Undo / Redo ✅ 一致

- `e865b938` で移植済み、テストカバー済み。

---

## まとめ — 優先度順 Codex タスクリスト

| 優先度 | カテゴリ | 内容 | 対応難易度 |
|---|---|---|---|
| **P0** | C-1 | RasterVector マスクボタンを「ビットマップ消去 + オブジェクト消去」の 2 個に直す | ✅ 修正済み |
| **P0** | G-1 | キャンバス入力ハンドラと overlay 描画に dialog_open ガードを追加 | ✅ 修正済み |
| **P0** | D-2 | 1px拡張 / 1px縮小 ボタン + `BitmapMaskOp` / `apply_bitmap_mask_op` / 1px morph を移植 | ✅ 修正済み |
| **P1** | A-1 + B-2 | MaskColorPreset 行を UI に追加し、プレビュー描画でその色を使う | ✅ 修正済み |
| **P1** | A-2 | 「選択レイヤーまでプレビュー」チェックボックスを UI に追加 | ✅ 修正済み |
| **P1** | F-1 | 線形/円形マスクハンドルのヒットテスト調査 + 修正 | ✅ 修正済み |
| **P1** | E-1 | 手動マスクの解像度問題: マスクバッファ寸法 + 表示品質の調査 + 修正 | ✅ 修正済み |
| **P1** | C-3, C-7 | Raster fill / LumaRange / ColorRange の反映不発の原因切り分け + 修正 | ✅ 寸法・ソース経路修正済み / 実機確認待ち |
| **P2** | I-1 | 処理中ステータスのリセット漏れ調査 + 修正 | ✅ 修正済み |
| **P2** | C-4 | LinearGradient/RadialGradient エディタの「作成」ボタンを削除しキャンバスドラッグ作成に統一 | ✅ 修正済み |
| **P3** | J-1, J-2, K-1 | ショートカット / ステージ切替の実機回帰確認 | ✅ コード対応済み / 実機確認待ち |

## 進め方の推奨

1. **P0 から順に Codex に投げる**。1 機能 1 コミットで進める。
2. 各コミット後に `cargo test` を回し、回帰がないことを確認。
3. 修正後はユーザーが**実機で**該当機能を 1 つずつチェック。OK なら次へ。
4. P1 の調査系 (F-1, E-1, C-3, C-7, I-1) は事前に `log::info!` を仕込んだ debug ビルドで
   原因を特定してから fix する。当てずっぽうの修正で時間を浪費しない。
5. ラボ単体 (`cargo run --bin local_adjust_lab`) を立ち上げて、同じ操作を並べて比較
   できる状態を作る。視覚差分は screenshot で記録する。

## このドキュメントの更新ルール

- 各項目を Codex が対応したら、対応コミット ID をその項目に追記し、判定を **✅ 修正済み**
  に更新する。
- 実機確認で OK が出たら **✓ 確認済み (yyyy-mm-dd)** に進める。
- 新たな差分が見つかったら同じ書式 (`X-N. 名前 🐛 / ⚠️ / ❌`) で追加する。
- すべて ✓ になったら本ドキュメントを `docs/local-adjust-integration-complete.md` に
  リネーム (= リリース完了の歴史的記録として残す)。
