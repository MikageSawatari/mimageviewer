# ブリーフ: タッチ対応 Phase 1 / Step 3 — 静止画フルスクリーンの配線

対象: v2.13.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode。
正本: [docs/touch-support-plan.md](touch-support-plan.md)。
**着手前に §5.3、§5.5、§5.6、§5.10、§5.15 を読むこと。**
表示・ズームに触るので [docs/display-pipeline.md](display-pipeline.md) も読む。

前提 (すべて完了・コミット済み):

- Step 0 診断プローブ `bb9574b2` — **出荷ゲート §6-1 通過**
- Step 1 認識器 `src/touch_input.rs` `66cb2910`
- Step 2 相関と所有権 `src/touch_correlation.rs` `6be3afd3`

---

## 0. これは何か

**Phase 1 で初めて実挙動が変わるステップ。** Step 2 までは分類するだけだった。
ここで静止画フルスクリーンに実際のタッチ操作を配線する。

最重要の成果は **「フルスクリーンに入るとタッチだけでは出られない」詰みの解消** (plan §2.4)。
タブレットには Esc キーが無く、現状これは逃げ道の無い罠になっている。

---

## 1. やること

| 入力 | 動作 |
| --- | --- |
| 左右タップ | ページ送り (既存の RTL 計算を通す) |
| **中央矩形タップ** | **クロームの表示トグル** — 上バー (× を含む) と下シークバー |
| 2 本指ピンチ | ズーム |
| 2 本指移動 | パン |

あわせて、相関済みクリックの既存 click 経路を抑止する (代替コマンドと**同時に**入れる)。

### 1.1 やらないこと (次ステップ)

- **端からの内向きスワイプ → 左右パネル** (plan §5.5 補助経路) — Step 3b
- **クローム内の大きなパネル呼び出しハンドル** (plan §5.5) — Step 3b
- 初回オーバーレイヘルプ (plan §5.5) — Step 3b
- 一覧グリッドのスクロール / 再タップ open — Phase 2
- native 動画 / 音楽 — Phase 3

→ Step 3 完了時点でクロームに出るのは **上バーと下シークバーだけ**。
左右パネルへの到達性は現状のまま (Step 3b で改善する)。**それでも × に到達できるので詰みは解消する。**

---

## 2. `TapZoneGeometry.excluded` を実際に埋めること (必須)

Step 2 では**両方の呼び出し元で空**にしてある。ここで実データを渡す。
埋めないと**クロームやパネルの上でも中央タップが成立する**。

既存の click ナビゲーションが同じ除外判定を持っているので、**それを再利用すること**
([ui_fullscreen.rs:16350-16372](../src/ui_fullscreen.rs) 付近):

- `fullscreen_side_panel_contains_pointer(...)` — 左右パネル領域
- `seek_panel_rect` (`seek_panel_interactive` のとき) — 下シークバー
- 上バーが表示されているフレームはその矩形も

**新しい矩形計算を作らないこと。** 既存の判定と食い違うと、マウスとタッチで当たり判定が
ずれて保守できなくなる。既存が矩形を返さない形なら、**既存側を矩形を返す形に整理してから**
両方で使う (症状パッチ的な二重定義を作らない)。

`continuous_active` (連続読み) でページタップを抑制する既存条件も、タッチ側で同じく尊重すること。

---

## 3. RTL — 二重管理を作らないこと

`TouchCommand::PageSide { left }` は**物理的な画面の左右**しか持たない (Step 1 の設計)。
読み方向の解決は既存の
`fullscreen_click_nav_base_delta(pos_x, center_x, rtl)` ([ui_fullscreen.rs:1303](../src/ui_fullscreen.rs)) が正本。

**指示**: 次の形に整理して、マウスとタッチが同じ 1 つの規則を通るようにする。

```rust
fn fullscreen_click_nav_delta_for_side(left: bool, rtl: bool) -> i32 { ... }

fn fullscreen_click_nav_base_delta(pos_x: f32, center_x: f32, rtl: bool) -> i32 {
    fullscreen_click_nav_delta_for_side(pos_x < center_x, rtl)
}
```

- **既存の `fullscreen_click_nav_base_delta` のテスト
  ([ui_fullscreen.rs:30689-30696](../src/ui_fullscreen.rs)) はそのまま通ること**
- タッチ側は `fullscreen_click_nav_delta_for_side(left, rtl)` を呼び、
  結果を既存の `spread_page_nav(base_delta)` ([ui_fullscreen.rs:12048](../src/ui_fullscreen.rs)) へ流す
- **ページ送りロジックを複製しないこと**

---

## 4. 中央タップのクローム (plan §5.5)

### 4.1 状態の置き場所と寿命

- **`App` にフィールドを足さない。** `ctx.data_temp` に viewport (+ surface) をキーとして持つ
  (plan §5.1-5、detached リワーク凍結ルール)
- 中央タップで**表示 / 非表示をトグル**
- **ページ移動とフルスクリーン終了で解除**
- **時間で消さない** (指を離すとホバーが消えるタッチでは、時間切れは事故になる)
- **`fullscreen_top_bar_locked` 設定を書き換えないこと** (永続設定を UI 操作で勝手に変えない)

### 4.2 上バー

可視判定は既に純関数
`still_top_bar_visible_from_inputs(StillTopBarVisibilityInputs)` ([ui_fullscreen.rs:1018](../src/ui_fullscreen.rs))
なので、**構造体に 1 フィールド足すだけ**で済むはず。既存のテストパターンに乗せること。

### 4.3 下シークバー

同じ扱いにする。可視判定が純関数になっていなければ、**上バーと同じ形に整理してから**足す。

---

## 5. ピンチズームとパン (plan §5.6)

### 5.1 既存の適用層を再利用する

**新しいズーム計算を書かないこと。** 既にある:

- `zoom_preserve_pivot(mouse, rect_center, base_pan, base_zoom, new_zoom)` ([ui_fullscreen.rs:4333](../src/ui_fullscreen.rs))
- `set_fs_pan_from_input(proposed_zoom, proposed_pan)` ([ui_fullscreen.rs:4075](../src/ui_fullscreen.rs))
- 既存の zoom min-max clamp と pan clamp

plan §5.6 の言う `apply_zoom_factor_about_pivot(factor, pivot)` は、
**上記を束ねる薄いラッパー**として作る。`TouchCommand::Zoom { factor, pivot }` の
`factor` を現在のズームに掛け、既存の clamp を通し、`zoom_preserve_pivot` で pan を求める。

`TouchCommand::Pan { delta }` は既存の pan 経路へ。
**認識器が返した順序 (Zoom → Pan) のまま適用すること。**

### 5.2 ⚠ Ctrl+ホイール分岐へ合流させないこと

plan §5.6 の明示的な警告。`zoom_delta()` は**マルチタッチが無ければ Ctrl+ホイールの合成値も返す**
ため、既存のホイール処理と**二重適用**になる。

- **`zoom_delta()` を入力源の判定に使わないこと**
- タッチのズームは `TouchCommand::Zoom` からのみ来る (Step 2 の相関を通ったもの)
- 既存の Ctrl+ホイール経路には一切触れない

### 5.3 その他

- **ピンチ中は PDF 再レンダリングを毎サンプル発行しない。ジェスチャー終了時に 1 回**
- **回転成分は無視する** (mIV の回転は非破壊 DB 管理で、ピンチ回転とは意味が衝突する)
- モーダル / TextEdit / 左右パネル / 上下バー / シークバー / 編集モードの各ゲートより**後**で処理する

---

## 6. クリック抑止 (plan §5.10)

Step 2 が計算済みの判定を、ここで初めて**適用**する。

- `TouchFrame::should_suppress_primary(pos, pressed)` / `should_suppress_response(response)` を使う
- **相関済みの primary にだけ適用する。** Step 2 のブリーフ §3.4 の罠
  (無条件に問い合わせるとタッチ直後のマウスクリックを食う) を再発させないこと
- **同じ release で touch のコマンドと既存の `clicked()` が二重実行されないこと**
- **全接点が離れるまで primary 抑止を維持する**
- **ドラッグは抑止しない。** 拡大画像の単指ドラッグは既存の pointer パンへ委譲する
  (`TouchOwner::ViewerPointerPassthrough`)

既存の類似機構 `fs_suppress_primary_until_release` ([ui_fullscreen.rs:16377](../src/ui_fullscreen.rs) 付近)
の作りが参考になる。**ただし別state を増やす前に、既存機構に相乗りできないかを先に検討すること。**

---

## 7. マウス無影響の保証 (plan §5.15)

**このステップで最も壊しやすいのはマウス。** 以下を回帰テストで固定すること:

- マウスの左クリックによるページ送りが従来どおり (左右・RTL 両方)
- マウスの右ドラッグ (リングショートカット) が従来どおり
- Ctrl+ホイールのズームが従来どおり (**二重適用していないこと**)
- 中ボタンドラッグのズームが従来どおり
- X ボタン (進む/戻る) が従来どおり
- タッチ完了直後のマウスクリックが抑止されないこと
- `MIV_DISABLE_TOUCH_GESTURES=1` で**すべて現行挙動に戻ること**

---

## 8. 完了条件

- `cargo fmt` (引数なし) を通すこと
- `cargo test -p mimageviewer --lib` が**全件**通ること (現在 4869 件。退行を出さない)
- `cargo check -p mimageviewer --bin mimageviewer-core` が通ること
- UI スナップショット (`cargo test --test ui_snapshot`) に差分が出るなら、
  **意図を確認のうえ更新し、PNG を目視したことを報告する**
- **Step 1 / Step 2 の dead_code 警告が解消しているはず**。残るものがあれば
  どれが Step 3b / Phase 2 / Phase 3 用かを報告すること
- 非 Windows を壊さないこと

## 9. 制約

- **アプリを起動しないこと。** 検証ビルドは ClaudeCode が用意し、利用者が実機確認する
- ブランチ操作・コミットは不要。master の作業ツリーで作業する
- **`App` にフィールドを足さない** (§4.1)
- **detached-rework 凍結ルールは有効。** detached 述語 / viewport 経路の判定を変えないこと。
  触れる必要が出たら**触らずに報告すること**
- **範囲を広げないこと。** §1.1 の一覧は次ステップ

完了したら、変更内容・**除外矩形をどう既存判定から取ったか**・**抑止を既存機構に相乗りさせたか
新設したか (と理由)**・テスト結果・**マウス経路の回帰確認内容**を報告すること。
plan と食い違う判断をした箇所があれば理由も明記すること。
