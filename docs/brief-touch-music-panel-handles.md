# ブリーフ: 音声モード / 音楽ビューの左右パネルをタッチで開けるようにする

対象: v2.13.0 タッチ対応の穴 (利用者報告 2026-08-09)。実装 = Codex Sol /
レビュー・検収 = ClaudeCode / 実機確認 = 利用者。

正本: [docs/touch-support-plan.md](touch-support-plan.md) §5.7 (「未対応: 音声モード /
音楽ビューの左右パネルがタッチで開けない」の節)。

前提 (コミット済み): `c333e03d` まで。master の作業ツリーで作業する。

---

## 1. 症状と原因 (特定済み)

音声モード (動画の映像を切って音声で聴く状態) と音楽ビューで、**左右パネルを開く手段が
タッチに無い**。

`draw_music_panel_callouts` ([src/ui_fullscreen.rs:6712](../src/ui_fullscreen.rs)) は
`pointer = hover_pos()` を取り、`callout_hit(panel_callout_edge_rect(...), pointer)` が真の
フレームだけ callout Area を描く。**タッチには hover が無いので callout が存在しない。**
`ClickToShow` だけでなく `Hover` モードも、辺へのホバーで出す設計なので同じく到達不能。

これは静止画 Step 3b / 動画 Step 3h と**同型**であり、その 2 面では
「hover に依存しない 48pt 幅の左右ハンドル」で解決済み。音楽面だけ残っている。

---

## 2. 実装方針 (利用者承認済み。この形で実装すること)

### 2.1 静止画と同じハンドルを音楽面にも出す

見えないタップゾーンは**作らない**。静止画の
`still_touch_panel_handle_rect` / `visible_still_touch_panel_handle_rects` /
`interact_still_touch_panel_handle` / `paint_panel_callout_chrome`
([src/ui_fullscreen.rs:1161 付近](../src/ui_fullscreen.rs) と
[6566 の `draw_fs_touch_panel_handles`](../src/ui_fullscreen.rs)) と**同じ形・同じ描画・
同じ「開いている側は消す」規則**を使う。

- 静止画の geometry helper は上下の安全域を `TOP_BAR_HEIGHT` / `FS_SEEK_BAR_HEIGHT` で
  **直書き**している。音楽面の帯は `MUSIC_TOP_BAR_HEIGHT` (54) と
  `crate::ui_music_panels::MUSIC_HUD_HEIGHT` なので、**安全域を引数に取る共通 helper へ
  切り出して両者から使う**。静止画側の算出結果は 1pt も変えないこと (スナップショットで確認)。
- 動画 native 側の `native_touch_panel_handle_rect`
  ([overlay_draw.rs](../src/video/native_presenter/overlay_draw.rs)) は HWND 事情が違うので
  今回は**触らない**。

### 2.2 表示条件は「このビューポートでタッチを使ったか」

静止画・動画はクロームの latch が gate だが、**音楽面は上下クロームが常時表示で latch が
無い** (= 常に latch されているのと同じ状態)。したがって:

- **このビューポートで一度でもタッチを観測したら、以後ハンドルを出す**
- **自動で消さない。時間で隠さない** (消える affordance は出続けるものより分かりにくい。
  §5.10 の「見えない時間状態を作らない」にも従う)
- ファイルを移っても消さない (曲送りのたびに隠れると使えない)。
  **`fs_idx` で key を作らないこと**
- タッチを一度も観測していなければ**出さない** = マウスだけの利用者の見た目は不変 (fail-closed)
- `MIV_DISABLE_TOUCH_GESTURES=1` では出さない

**「タッチを観測した」の取り方**: 音楽ビューは今のところ `TouchCorrelation` を駆動して
いない (グラフのタップシークは egui の pointer emulation のまま動いており、利用者も現状で
問題ないと言っている)。**所有権機構をここへ持ち込まないこと。** 必要なのは 1 bit なので、
raw event に `egui::Event::Touch` があるかを**読み取るだけ**の sticky フラグにする。
既存の入力経路・抑止・コマンド解決を一切変えないこと。

保存先は `still_touch_chrome_latch_id(ctx)` と同じくビューポート単位で解決する
`ctx.data_temp` に揃える。

### 2.3 押したときの挙動

`draw_music_panel_callouts` のクリック分岐 (`toggle_left` / `toggle_right`) と**同じ
状態更新へ合流**させる。ハンドル専用の別経路を作らない。

⚠ **左パネルは Hover モードで開けない問題がある**。
`music_left_panel_visible_from_inputs` ([ui_fullscreen.rs:1646](../src/ui_fullscreen.rs)) は
`Hover` アームで `hover_active` しか見ないので、`music_left_click_open` を立てても
Hover モードでは表示されない。一方 `music_right_panel_visible_from_inputs` は
`Hover => hover_active || explicit` で明示オープンを尊重している。

- **左も右と同じ形に揃える**。動画 Step 3h が同じ理由で「左の session bool を
  `MetadataPanelOpenState` へ置き換え」たのと同じ扱いにし、`ByPointer` / `ByTouchHandle` を
  区別する。
- **マウスの見た目・挙動は変わらない**: 今日の Hover モードでは左の callout 自体が
  描かれないので `music_left_click_open` は常に false であり、Hover アームに explicit 項を
  足しても現行のマウス挙動に差は出ない。この根拠をテストで固定すること。
- ハンドルは静止画と同じく**開く方向のみ** (`open_still_side_panel_by_touch` と同じ考え方)。
  開いている側のハンドルは描画・hit test の両方から消える。

### 2.4 モードに依らず出す

静止画の `draw_fs_touch_panel_handles` は `draw_fs_panel_callouts` と違い
**side panel mode で gate していない**。音楽面も同じにする (Hover / ClickToShow の
どちらでもタッチからは到達できないため)。

### 2.5 passive / parked-live

`draw_music_panel_callouts` に渡している `enabled` と**同じ gate**を使う。
passive 窓ではハンドルを出さない。

---

## 3. 確認してから決めること (勝手に握り潰さない)

音楽ビューの中央帯には可視化 (スペクトラム / DJ 波形 / 鍵盤) があり、**波形のタップでも
シークできる**。48pt × 96〜220pt のハンドルがその**タップシーク面と重なる**なら、
帯の左右端でシークが押せなくなる。

- 重なるかどうかを実レイアウトで確認すること
- 重なる場合は**黙って上に載せない**。ハンドルの縦位置をシーク面から外す等の案を添えて
  報告し、判断を仰ぐこと (重ならないなら、そう報告するだけでよい)

---

## 4. やらないこと

- 音楽面に初回ヘルプを出すこと (利用者判断で不要と確定済み。ハンドルは見えるので自明)
- 音楽面で `TouchCorrelation` / `TouchOwner` / タップゾーンを駆動すること
- 既存の hover callout の表示条件・形・色を変えること
- 動画 native のハンドル (Step 3h) を変えること
- マウスの click / drag / hover / wheel の挙動を変えること (§5.15)

## 5. テスト

1. **geometry の純関数テスト**: 共通 helper が静止画の安全域で従来と同一の rect を返すこと
   (退行ガード)。音楽面の安全域では上下 HUD に重ならないこと。極端に低いウィンドウで
   `Rect::NOTHING` になること。
2. **表示条件**: タッチ未観測ならハンドル無し / 観測後は出る / ファイルを移っても出たまま /
   `MIV_DISABLE_TOUCH_GESTURES=1` で出ない。
3. **左パネルの Hover モード**: 明示オープンが Hover モードでも表示されること、かつ
   **マウスのみの経路では従来どおり explicit が立たない**こと。
4. **開いている側のハンドルが消える**こと (左右それぞれ)。
5. 既存の `music_panel_visibility_uses_hover_or_explicit_click_state_by_mode`
   ([ui_fullscreen.rs:30612](../src/ui_fullscreen.rs)) 等が通り続けること。
6. スナップショットは、静止画側に差分が出ないこと。音楽面のハンドルを足すなら
   `draw_still_panel_reach_snapshot_fixture` と同じ形の fixture を用意して比較する。

## 6. 完了条件

- `cargo fmt` (引数なし)
- `cargo test -p mimageviewer --lib` が全件 (現在 4993 件) /
  `cargo test -p mimageviewer --test ui_snapshot`
- `cargo check -p mimageviewer --bin mimageviewer-core`
- `python scripts/check_ui_glyphs.py` が 0 件
- 非 Windows を壊さないこと
- **[docs/touch-support-plan.md](touch-support-plan.md) §5.7 の該当節を実装記録へ書き換える**
  (「未対応」の見出しを実装記録にする)

## 7. 制約

- **アプリを起動しないこと。** 検証ビルドと実機依頼は ClaudeCode が行う
- **ブランチ操作・コミットをしないこと。** master の作業ツリーで未コミットのまま残す
- 症状を消すガード / 時間ガード / 追加 repaint を足さないこと

---

完了したら次を報告すること:

1. 共通 helper の切り出し方と、静止画側が 1pt も変わらない根拠
2. 「タッチを観測した」フラグの置き場所と、既存入力経路に触れていない根拠
3. §3 の重なり確認の結果
4. 左パネルを Hover モードでも開けるようにした方法と、マウス挙動が変わらない根拠
5. テスト結果
6. **実機で確認してほしいこと**
