# 動画の上部 HUD / 下部シークバーを個別に固定表示できるようにする

正本は [next-release-backlog.md](../next-release-backlog.md) **§1.101**。着手前に同項を読むこと。
関連: [video-architecture.md](../video-architecture.md) (特に native presenter の節)、
[docs/fullscreen-side-panel-mode-plan.md](../fullscreen-side-panel-mode-plan.md) (静止画側の前例)。

## 1. 要望

動画のシークバーを常時表示したい (専用スレ >>271)。
**上部 HUD と下部シークバーを、それぞれ独立して**固定 / 自動表示へ切り替えられるようにする。
既定は現在の自動表示 (hover) を維持する。

## 2. 実装先

**native presenter の HUD overlay** (`src/video/native_presenter/{render_core.rs,overlay_draw.rs}`)。
**旧 egui 動画 UI へだけ設定を足さない** (§1.101)。

`ui_fullscreen.rs` ではなく `src/video/native_presenter/` を見ること
(CLAUDE.md の領域表)。

## 3. ⚠️ いちばん壊しやすいところ — 描画と hit-test は同じ述語を通す

上下の表示判定は既に純関数へ切り出されている:

- 下: `hud_visible()` → `native_hud_bottom_visible_from_hover(...)`
  ([render_core.rs:5874](../../src/video/native_presenter/render_core.rs:5874))
- 上: `top_bar_visible()` → `native_hud_top_visible_from_hover(...)`
  ([render_core.rs:5884](../../src/video/native_presenter/render_core.rs:5884))

**固定状態はこの純関数の入力として入れる。** 呼び出し側で `|| pinned` を後付けしない。

そして [render_core.rs:5430](../../src/video/native_presenter/render_core.rs:5430) に
明示された不変条件がある:

> **bottom_hud_visible** は描画側 (`render_once`) と完全一致させる。

HWND の **hit-test region と z-order** は `bottom_hud_visible` から作られている
([render_core.rs:5477](../../src/video/native_presenter/render_core.rs:5477) 付近)。
片方だけに固定状態を通すと、**固定したバーが描かれるのにクリックを受けない** / **何も無い所が
クリックを吸う**、のどちらかになる。同じ場所で過去に 1 度直している (CP5 P2 #1 のコメント)。

## 4. 設定

- **上部 HUD と下部シークバーで独立した設定**にする。片方だけの設定にしない。
- **既定は現在の自動表示** (挙動を変えない)。
- 静止画の左右パネルに前例がある: `FsSidePanelMode { Hover, ClickToShow }`
  ([settings.rs:641](../../src/settings.rs:641))。**同じ考え方で命名・UI を揃えるか、
  揃えない理由を報告に書く。**
- 文言に実装語を出さない (CLAUDE.md「マニュアル・製品ページの記述方針」)。
  `HUD` / `overlay` / `hit-test` といった語を UI に出さない。

## 5. 確認する干渉 (§1.101 が列挙しているもの)

固定中に次が壊れないこと:

1. **映像との重なり** — 固定した帯が映像を隠す。ズーム / パン / フィット時の見え方。
2. **VST editor** — `vst3_panel_visible()` との重なりと z-order。
3. **タッチ操作** — `native_touch.chrome_latched()` が hover 判定に入っている。
   固定中に latch がどう振る舞うべきかを決めて報告する。
4. **HUD HWND の hit-test region / z-order** — §3 のとおり。
5. **固定解除後に、既存の自動表示へ戻ること。**

## 6. 音声モードの扱いは決めて報告する

§1.101 は「音声モードでも同じ下部固定状態を使うかは、**既存の動画 / 音声 HUD 共有契約に
沿って実装時に確定する**」としている。共有契約を読んだうえで決め、**理由を報告に書く**。
勝手に別設定を増やさない。

## 7. 制約

- **時間窓・sleep・retry で吸収しない。**
- 既定の hover 動作を変えない。
- 旧 egui 動画 UI に設定を足さない。
- detached / viewport 述語に触る必要が出たら、**触る前に止めて報告する**
  (CLAUDE.md「Detached viewer リワーク中のルール」)。

## 8. テスト

- 上下それぞれの固定 / 自動が独立に効くこと (4 通り)。
- **固定中、描画と hit-test region が一致すること** (§3。純関数レベルで固定できる)。
- 既定値が現在の hover 動作と同一であること (**回帰**)。
- 固定解除後に hover 判定へ戻ること。
- 音声モードについて §6 で決めた挙動。
- UI スナップショット (設定ページ)。

## 9. 完了条件

- `cargo fmt` 済み / `cargo test -p mimageviewer --lib` が緑
- `cargo test --test ui_snapshot` が緑
- `cargo check -p mimageviewer --bin mimageviewer-core` が通る
- `python scripts/check_ui_glyphs.py` が 0 件
- `docs/video-architecture.md` / `docs/spec.md` / マニュアルの更新
- **報告に、音声モードの判断・タッチ latch の判断・静止画側と命名を揃えたか**を書く

> **実機確認が要る項目**: 映像との重なり、VST editor の z-order、タッチ、hit-test region。
> 利用者不在のため、**ビルドまで用意して確認手順を残す**。
