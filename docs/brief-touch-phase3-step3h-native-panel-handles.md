# ブリーフ: Phase 3 Step 3h — 動画 native の左右パネルをタッチで開く

対象: v2.13.0 タッチ対応 Phase 3。実装 = Codex Sol / レビュー・検収 = ClaudeCode /
実機確認 = 利用者。

正本: [docs/touch-support-plan.md](touch-support-plan.md) §5.5「Step 3b 実装記録」/ §5.7-1。

前提 (コミット済み・実機確認済み、`9ab40c8b` まで): Phase 3 Step 3g まで。
**これが Phase 3 の最後の実装項目**。

---

## 1. 症状

実機報告 (2026-08-09):

> 動画は左のパネルを出す方法がまだなく、ブックマークは確認できませんでした。

動画 native overlay の左パネル (ジャンプ / ブックマーク) と右パネル (メタデータ) は
**ホバーの callout でしか開けない** — `side_panel_callout_visibility`
([render_core.rs:5858](../src/video/native_presenter/render_core.rs)) が
`visibility_hover_pos()` を要求する。

**タッチにはホバーが無い**ので、指では到達できない。静止画で Step 3b が解いたのと
同じ問題が、動画側に残っていた。この導線が無いために、
**native パネルの指スクロールが動くかどうかも検証できていない**。

---

## 2. 静止画 (Step 3b) と同じ仕様にする

plan §5.5「Step 3b 実装記録」と、その後の実機修正で確定した仕様をそのまま持ち込む:

| 項目 | 仕様 |
| --- | --- |
| 表示条件 | **タッチのクロームラッチが ON の間だけ** (中央タップで出る)。常時表示にしない |
| 形 | 画面左右端のハンドル。静止画と同じ幅 (`STILL_TOUCH_PANEL_HANDLE_WIDTH_PT` = 48pt) |
| 開いた側 | **ハンドルを消す**。パネル操作を塞がないため (静止画で実機指摘を受けて確定) |
| 閉じ方 | **パネル外のタップで左右まとめて閉じる**。そのタップはページ送り等に再利用しない |
| マウス | 既存のホバー callout を**そのまま残す**。見た目も挙動も変えない |

動画の左パネルは**タブを持つ** (`NativeVideoLeftPanelTab::Jump` 等)。
**開いたときのタブは既存の既定のまま**にし、この Step でタブ選択の導線を足さない。

---

## 3. 実装の方向 — OS の hit-test に乗せる

**新しい hit resolver を作らないこと。** Step 1 で HUD HWND の `PT_TOUCH` を所有し、
**HUD 由来の stream は幾何に関係なく widget passthrough** になっている。

したがって:

- **ハンドルの矩形を `compute_hud_regions()`
  ([render_core.rs:5243](../src/video/native_presenter/render_core.rs)) の
  interactive region に含める**
- そうすると OS の hit-test がハンドル上のタッチを **HUD HWND へ配送**し、
  Step 1 の規約でそのまま widget passthrough になる。
  ハンドルの egui ボタンが**普通の click として成立する**
- 既存の callout / パネルが同じ仕組みで region に入っているので、**同じ扱いに揃えるだけ**

この形なら、presenter 側の `TapZoneGeometry.excluded` を触る必要も、
タップを最寄り id へ解決する機構も要らない。

⚠ もしこの seam が成立しないと分かったら (例: ハンドルを HUD ではなく presenter 面に
描く必要がある等)、**症状パッチを当てずに報告すること**。

## 4. ⚠ クロームラッチとの関係

- ハンドルの表示は `native_touch.chrome_latched()` に従う。
  **新しいラッチや bool を足さないこと**
- 中央タップでクロームを消したらハンドルも消える (静止画と同じ)
- ラッチが OFF の間はハンドルの region も出さないこと。
  出しっぱなしにすると、映像上のタップが HUD HWND に吸われてシークが効かなくなる

## 5. マウス無影響 (§5.15)

- **ホバーの callout、パネルの開閉、閉じるボタン、ドラッグがすべて不変**であること
- ハンドルはタッチのクロームラッチ中にしか出ないので、マウス操作中は現れない
- `MIV_DISABLE_TOUCH_GESTURES=1` で現行挙動へ戻ること

## 6. ⚠ 先に読むこと

- plan §5.5「Step 3b 実装記録」— 静止画側の確定仕様
- plan §5.9「Phase 3 Step 1 実装記録」— HUD 所有と widget passthrough の規約
- plan §5.10 — 新しい cross-frame state を足すなら必読 (今回は足さない想定)

## 7. テスト

- クロームラッチ ON でハンドルの矩形が region に含まれ、OFF で含まれないこと
- **開いている側のハンドルが消える**こと (左だけ開けば右だけ残る)
- パネル外のタップで左右まとめて閉じ、そのタップがシーク等に再利用されないこと
- ハンドル矩形が既存の HUD ボタン / シークバーの region と**重ならない**こと
  (重なると既存操作を奪う)
- マウスのホバー callout が従来どおりであること

## 8. 完了条件

- `cargo fmt` (引数なし)
- `cargo test -p mimageviewer --lib` が**全件**通ること (現在 4981 件)
- `cargo test -p mimageviewer --test ui_snapshot` が通ること
- `cargo check -p mimageviewer --bin mimageviewer-core`
- `python scripts/check_ui_glyphs.py` が 0 件
- 非 Windows を壊さないこと
- **[docs/touch-support-plan.md](touch-support-plan.md) と
  [docs/video-architecture.md](video-architecture.md) に実装記録**を書く。
  特に「HUD region に含めることで OS hit-test に乗せた」判断を残す

## 9. 制約

- **アプリを起動しないこと。** 検証ビルドは ClaudeCode が用意する
- **ブランチ操作・コミットをしないこと。** master の作業ツリーで作業する
- detached-rework 凍結ルールは有効
- **範囲を広げないこと**。パネルの中身、タブ選択の導線、ボタンのターゲットサイズ、
  静止画側には手を出さない

---

完了したら次を報告すること:

1. ハンドルを HUD region に載せた方法と、ラッチとの連動
2. 開いた側を消す / 外タップで閉じるを静止画とどう揃えたか
3. 既存 region との重なりが無いことの根拠
4. マウスのホバー callout が不変であることの根拠
5. テスト結果
6. **実機で確認してほしいこと** (native パネルの**指スクロール**の確認手順を含めること。
   この導線が無くて未検証のままなので)
