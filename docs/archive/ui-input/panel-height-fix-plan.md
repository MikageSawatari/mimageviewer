# パネル縦サイズが半分しか伸びない問題 — 修正設計ブリーフ

**対象**: 消しゴム / 隠蔽 / 画像補正の 3 パネル
**症状**: ウィンドウ縦幅の半分くらいの高さしか取れず、コンテンツが下に詰まる
**進め方**: Codex GUI で実装 → Claude Code で手作業レビュー → コミット

このブリーフは前 2 件 (`docs/archive/editing/erase-cache-refactor-plan.md` /
`e5150bad 消しゴムプレビューと元画像表示を修正`) と同じ進め方で使う。
内容を読んでから着手し、迷ったら本ファイルにあるユーザー要求 / 既存試行 /
仮説を全て確認すること。

---

## 1. ユーザー要求 (再掲)

R5 実機 FB:

> ウィンドウのスクロールは出ますが、×ボタンとスクロールバーが重なっています。
> また、ウィンドウの半分くらいのサイズになってしまい使いづらいです。
> ウィンドウのしたギリギリくらいまでのサイズにして、それでも収まらない場合のみ
> スクロールを必要とする形にしたいです。

その後の R5 二次 FB:

> 隠蔽加工や消しゴムツールの縦のパネル幅はまだ小さいままです。ウィンドウ縦幅の
> 半分もありません。

**達成したい挙動**:

- 3 パネルとも、コンテンツが少なくても **ウィンドウ下端から ~20 px 上**
  くらいまで Frame::popup の枠 (= 暗背景 + ボーダー) が広がる
- コンテンツが多くてその高さに収まらない場合だけ ScrollArea の縦スクロールが
  発動する
- ヘッダ (タイトル + プレビュー目アイコン + 閉じる × ボタン) は ScrollArea
  の **外** に固定したまま (= スクロールバーが × に重ならない)
- 画像補正パネルは **全体スクロール** (= ヘッダ以外すべて ScrollArea 内)

---

## 2. 過去の試行 (うまくいかなかった理由)

### 2.1 R5 #1: ScrollArea を入れて max_height = body_rect.height()

- コミット: `289530b5 消しゴム + 隠蔽パネルを ScrollArea で囲み、ウィンドウ縦幅に追従 (R4 #2)`
- 配置: `ScrollArea::vertical().max_height(max_height).auto_shrink([false, true])`
- 問題: `auto_shrink([_, true])` = 縦方向にコンテンツへ収縮するので、コンテンツが
  短いとパネルも短くなる

### 2.2 R5 #2: auto_shrink を両軸 false に

- コミット: `76ecd8a7 R5 応急処置: preview ミスマッチクラッシュ回避 + パネル下端まで拡張`
- 配置: `ScrollArea::vertical().max_height(max_height).auto_shrink([false, false])`
- ヘッダを ScrollArea の外に出した (× とスクロールバーの干渉解消)
- `max_height` 計算: `ctx.content_rect().max.y - ui.cursor().top() - 20`
- それでもウィンドウ半分くらいしか伸びない
  (= ユーザー二次 FB の症状)

### 2.3 画像補正パネルの全体 ScrollArea 化

- コミット: 同 R5 で `ui_adjustment_panel.rs` を refactor
- `body_rect = panel_rect - HEADER_H` を `child.new_child(UiBuilder::max_rect)` で
  確保し、その中で `ScrollArea::vertical().max_height(body_rect.height())
  .auto_shrink([false, false])`
- ヘッダ以外のすべて (spread セレクタ / scope text / action buttons / スライダー /
  保存スロット / お気に入り) が ScrollArea 内に入った
- それでも `body_rect.height()` 分しか伸びていない印象

---

## 3. 現状コード参照

すべて 2026-05-27 時点 (`e5150bad`)。

### 3.1 消しゴム

[`src/ui_erase.rs`](../src/ui_erase.rs)

```rust
// L1832 draw_erase_panel
let panel_pos = egui::pos2(
    full_rect.min.x + PANEL_MARGIN_X,            // 16
    full_rect.min.y + PANEL_MARGIN_Y,            // 60
);
let sink_rect =
    egui::Rect::from_min_size(panel_pos, egui::vec2(PANEL_W + 4.0, ERASE_PANEL_SINK_H));
//                                                  200+4         1000

egui::Area::new(...).fixed_pos(panel_pos).order(Foreground).show(ctx, |ui| {
    ui.interact(sink_rect, ..., Sense::click_and_drag());      // クリック吸収 sink
    egui::Frame::popup(ui.style())
        .fill(...)
        .stroke(...)
        .corner_radius(6.0)
        .show(ui, |ui| {
            ui.set_min_width(PANEL_W);     // 200
            ui.set_max_width(PANEL_W);     // 200
            *ui.visuals_mut() = egui::Visuals::dark();

            ui.horizontal(|ui| { /* ヘッダ: タイトル + プレビュー + × */ });

            // ── ここから ScrollArea (問題箇所) ────────────────────
            let screen_max_y = ctx.content_rect().max.y;
            let avail_top = ui.cursor().top();
            let max_height = (screen_max_y - avail_top - 20.0).max(120.0);
            egui::ScrollArea::vertical()
                .max_height(max_height)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    /* ツール選択 / ブラシ太さ / 直線太さ / プリセット /
                       スロット F1〜F4 / 削除 / 適用 / ヘルプ */
                });
        });
});
```

### 3.2 隠蔽

[`src/ui_conceal.rs`](../src/ui_conceal.rs) L1614 周辺。構造は消しゴムと同じ
(`Area::fixed_pos` + `Frame::popup` + `ScrollArea::vertical()`)。

```rust
// L1714-1720
let screen_max_y = ctx.content_rect().max.y;
let avail_top = ui.cursor().top();
let max_height = (screen_max_y - avail_top - 20.0).max(120.0);
egui::ScrollArea::vertical()
    .max_height(max_height)
    .auto_shrink([false, false])
    .show(ui, |ui| { /* ツール選択 / モード / プリセット / スロット / ヘルプ */ });
```

### 3.3 画像補正

[`src/ui_adjustment_panel.rs`](../src/ui_adjustment_panel.rs) L688 周辺
(`draw_adjustment_panel`)。配置構造が消しゴム/隠蔽と異なり、`panel_rect`
(= 絶対座標) で受け取って `child.new_child(UiBuilder::max_rect)` を使う。

```rust
// L797-802 body_rect 算出
let body_rect = egui::Rect::from_min_max(
    egui::pos2(panel_rect.min.x, panel_rect.min.y + HEADER_H),    // 36 px ヘッダ
    panel_rect.max,
);
let mut body_child = child.new_child(egui::UiBuilder::new().max_rect(body_rect));
let body_height = body_rect.height();

// L877-882 ScrollArea
let (changed, is_dragging) = egui::ScrollArea::vertical()
    .max_height(body_height)
    .min_scrolled_height(0.0)
    .auto_shrink([false, false])
    .show(&mut body_child, |ui| { /* 全コンテンツ */ });
```

呼び出し側 ([`src/ui_fullscreen.rs`](../src/ui_fullscreen.rs) L948):

```rust
fn adjustment_panel_rect(full_rect: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(full_rect.min.x, full_rect.min.y + TOP_BAR_HEIGHT),    // 44
        egui::pos2(
            full_rect.min.x + crate::ui_adjustment_panel::LEFT_PANEL_WIDTH,  // 260
            full_rect.max.y,
        ),
    )
}
```

---

## 4. 仮説 (Codex に検証してもらう)

### 4.1 [本命] `auto_shrink([false, false])` は親 UI の available_rect に頼る

egui の `ScrollArea` は `max_height` をセットしてあっても、**親 UI の
available_rect** が小さければそれに収まる方を選ぶ可能性がある。

- 消しゴム/隠蔽: 親 ui は `Frame::popup` 内、その親は `Area::fixed_pos`。
  `Area` は `fixed_size` を指定していないので **自身は available_rect を
  持たない (= 内側へ伝播するのは ~INFINITY か、egui の default 振る舞い次第)**。
  この場合 ScrollArea は max_height ではなく content_height にフォールバック
  しているのではないか。
- 画像補正: `child.new_child(UiBuilder::max_rect(body_rect))` で max_rect は
  確保しているので、available_rect は body_rect.height() のはず。ここは
  動くはずだがユーザー報告では半分。要再現確認。

**検証手順 (Codex 側)**:

1. 各パネルの ScrollArea 起動直前に以下を `crate::logger::log` でダンプ:
   - `ctx.content_rect()` の min / max
   - `ui.available_rect_before_wrap()` の min / max / height
   - `ui.cursor()` の top
   - `max_height` (= 計算後の値)
   - `ui.max_rect()` の height
2. ScrollArea の `show()` 直後に戻り値 `ScrollAreaOutput` の `content_size`
   と `state` をログ
3. Frame::popup の閉じカッコの直後に `ui.min_rect()` をログ
   (= Frame::popup が実際に確保した rect)
4. ウィンドウ縦が **明らかに 1080+ px** あるときの値と、ユーザー報告
   「半分しかない」の値を比較する

ログ結果から原因を絞り込んで修正案を選択する (= 後述 §5)。

### 4.2 `ui.cursor().top()` が想定外の値を返している

ヘッダの `ui.horizontal(|ui| {...})` の中で `with_layout(right_to_left, ...)`
を使っているので、egui の cursor 進行が想定どおりに動いていない可能性。
- 想定: `cursor.top()` ≒ panel 上端 + 26 px (ヘッダ行高)
- 実際: それ以下 / 以上 になっている?

→ §4.1 のログで判明する。

### 4.3 Frame::popup が ScrollArea のサイズに引きずられている

`Frame::popup` は children の min_rect を見て padding + border 込みのサイズに
auto-grow する。ScrollArea が短く allocate すると Frame::popup も短くなる
(= 二段の縮み)。

これは §4.1 の派生で、ScrollArea が max_height を信用しない時の追随現象。
**修正は ScrollArea 側で固定する**ので Frame::popup は自動で追随する。

---

## 5. 修正案 (検証結果に応じて選ぶ)

### 5.1 [推奨候補 A] ScrollArea を `ui.allocate_ui_with_layout` で囲む

ScrollArea の前に親 UI の高さを明示的に確保する。これで ScrollArea の
available_rect は確実に max_height ぶん取れる。

```rust
let max_height = (screen_max_y - avail_top - 20.0).max(120.0);
ui.allocate_ui_with_layout(
    egui::vec2(PANEL_W, max_height),
    egui::Layout::top_down(egui::Align::LEFT),
    |ui| {
        egui::ScrollArea::vertical()
            .max_height(max_height)
            .auto_shrink([false, false])
            .show(ui, |ui| { /* 中身 */ });
    },
);
```

利点:
- ScrollArea の親 ui に明確な max_rect が伝播する
- Frame::popup が ScrollArea の allocated size に追随して下端まで広がる
- 既存コードへの変更が最小

### 5.2 [推奨候補 B] `ui.set_min_height(max_height)` を ScrollArea の前に呼ぶ

`set_min_height` は egui Ui の最低保証高さ。これだけで親 ui が縦に広がる
ので、ScrollArea の available_rect も広がる。

```rust
let max_height = (screen_max_y - avail_top - 20.0).max(120.0);
ui.set_min_height(max_height);
egui::ScrollArea::vertical()
    .max_height(max_height)
    .auto_shrink([false, false])
    .show(ui, |ui| { /* 中身 */ });
```

A より簡潔。ただし `set_min_height` は egui の内部実装で「最終的な ui.min_rect
を伸ばす」効果なので、ScrollArea が**先に確定する**サイズ計算ロジックには
影響しない可能性あり。要検証。

### 5.3 [補助] Area に `constrain_to(...)` を付ける

`egui::Area::new(...).fixed_pos(...).constrain_to(window_rect)` で Area の
最大占有領域を明示。

```rust
egui::Area::new(...)
    .fixed_pos(panel_pos)
    .order(Foreground)
    .constrain_to(egui::Rect::from_min_max(
        panel_pos,
        egui::pos2(panel_pos.x + PANEL_W + 4.0, ctx.content_rect().max.y),
    ))
    .show(ctx, |ui| { ... });
```

これは Area 自体のクリッピングで、内側 UI の available_rect には直接影響
しないが、念のため。

### 5.4 画像補正パネル固有

画像補正は `child.new_child(UiBuilder::max_rect(body_rect))` で body_rect を
ぶ厚く確保しているはずなので、それでも半分しか伸びないなら ScrollArea が
parent の max_rect を見ていない (= egui のバグ or 仕様外)。

候補:
- `body_child.set_min_height(body_height)` を ScrollArea の前に追加
- もしくは `body_child.set_height(body_height)` で明示確保

---

## 6. 実装ステップ案

### Step 1: 計測 (= 仮説検証、必須)

§4.1 のログを 3 パネルとも追加し、cargo build --release で実機起動 →
パネルを開いてログを集める。**ここで値の食い違いが判明する**。
ログを残したまま次ステップへ。

### Step 2: 推奨候補 A or B を 1 パネルだけ試す

消しゴムパネルにまず `allocate_ui_with_layout` (5.1) を入れて実機確認。
- ウィンドウ下端まで広がるか
- コンテンツ多時にスクロールバーが出るか
- スクロールバーが × に重ならないか
- 他パネル (隠蔽 / 補正) と並べたとき配置が崩れないか

### Step 3: 全パネルに展開

消しゴムが OK なら同じパターンを `ui_conceal.rs` / `ui_adjustment_panel.rs`
にも適用。画像補正パネルは body_rect が既に確保されているので、ScrollArea の
親 ui (`body_child`) に `set_min_height(body_height)` を入れるだけで足りる
可能性が高い (= §5.2 / §5.4)。

### Step 4: ログ削除

Step 1 で入れたデバッグログを削除。Codex は「§4.1 のログを残したまま」
コミットせず、原因特定後の cleanup でログを消してから commit する。
ログを残したい場合は perf 計装 (`perf::event(...)`) に格上げするか、
`#[cfg(debug_assertions)]` で囲む。

### Step 5: 動作確認

- 1080p / 1440p / 4K / 縦長 (例: 縦 1200) で挙動を見る
- パネル開閉トグル (Ctrl+E / Ctrl+M / Ctrl+A) を連打しても崩れない
- 見開きモード切替で配置がずれない
- 透過チェッカ背景 (Ctrl+B) と同時表示しても問題ない
- AI / 補正アイコンと干渉しない

### Step 6: ドキュメント更新

修正方針が固まったら以下を更新:
- `docs/ui-responsiveness.md` — パネル配置のレイアウト rule
  (`Area::fixed_pos + ScrollArea` の組み合わせは `allocate_ui_with_layout`
  か `set_min_height` で親サイズを明示することと書く)
- `CLAUDE.md` の「### ダイアログ (egui::Window)」隣に「### パネル
  (Area + Frame::popup)」節を新設して同じ rule を載せる

---

## 7. 既知の落とし穴 / レビュー観点 (Claude が見るところ)

### 7.1 ヘッダの × は ScrollArea の外に残すこと
R5 の修正で「スクロールバーが × に重なる」問題を解決済み。ヘッダを
ScrollArea 内に戻すと退行する。

### 7.2 sink_rect は ScrollArea の上に残す
`sink_rect` (= クリック吸収) は ScrollArea の rect より大きくないと
パネル下半分のクリックがすり抜ける。修正後にパネル下端まで広がったら
`ERASE_PANEL_SINK_H = 1000` でカバーできるかを確認。足りなければ動的に
`max_height` に揃える。

### 7.3 IME 対応
TextEdit を含むダイアログでは `dialog_enter_pressed` / `dialog_escape_pressed`
を使うこと (本ファイル CLAUDE.md「IME 対応」節)。パネル内に
TextEdit は今のところ無いが、将来追加するなら忘れずに。

### 7.4 配色は dark 固定
`*ui.visuals_mut() = egui::Visuals::dark();` を Frame::popup の中に **必ず**
入れる (テーマ非依存)。ScrollArea を囲う `allocate_ui_with_layout` の中でも
親 visuals を引き継ぐので明示的に dark を再セットしなくてよいが、別 closure
で囲い直すなら確認すること。

### 7.5 borrow 衝突
`Frame::popup.show(ui, |ui| { ... ScrollArea.show(ui, |ui| { ... self メソッド呼び出し ... }) })`
で self を closure 内で `&mut` 使うと borrow 違反になりやすい。ローカル
変数で値をキャプチャ → closure 外でディスパッチする既存パターンを維持する
こと。

### 7.6 snapshot test
3 パネルとも `tests/snapshots/` に PNG がある (`erase_panel.png` /
`conceal_panel.png` / `adjustment_panel.png` 等)。レイアウト変更で snapshot
が変わるはず。`UPDATE_SNAPSHOTS=1 cargo test --test ui_snapshot` で更新後、
PNG を目視確認してから commit する (詳細: `docs/ui-snapshot-policy.md`)。

### 7.7 cargo fmt + pre-commit hook
リポジトリは `cargo fmt --check` clean を維持。コミット前に `cargo fmt`
を**全体**にかける (CLAUDE.md「Formatting」節)。

### 7.8 確認コマンド
```bash
cargo fmt --check
cargo check
cargo test --lib
cargo test --test ui_snapshot   # スナップショット変更したら UPDATE_SNAPSHOTS=1
cargo build --release           # CRT 静的リンク + ランチャー含めて通る
```
---

## 8. 完了条件

- ウィンドウ縦 1080 px 以上で、3 パネルとも下端から ~20 px までフレームが
  伸びている
- コンテンツが少ないとき (= ツール選択だけ) でもパネルが縦に広い (= スカスカ
  な暗背景がパネル下半分に伸びている)
- コンテンツが多いとき (= 全部展開) はスクロールバーが出て、バーが × ボタンに
  重ならない
- 1440p / 4K / 縦長ウィンドウでも同じ挙動
- UI snapshot test が更新済み (PNG 目視 OK)
- ドキュメント更新済み (CLAUDE.md or docs/ui-responsiveness.md)
- `cargo fmt --check` / `cargo check` / `cargo test --lib` / `cargo test --test ui_snapshot` / `cargo build --release` すべて通る

実機 FB ループに戻ったら、ユーザーが「下端まで広がった」と確認するまでが
完了。

---

## 9. 工数感

- Step 1 (ログ追加): 30 分
- Step 2 (消しゴム 1 パネルだけ fix): 1-2 時間
- Step 3 (3 パネルに展開): 1 時間
- Step 4 (ログ削除 + cleanup): 30 分
- Step 5 (実機確認): 30 分
- Step 6 (docs 更新): 30 分

合計 **4-5 時間** ぐらいを見込む。仮説検証で詰まったら長くなる可能性あり
(= egui 0.33 のスクロール内部実装を読み込む羽目になる)。
