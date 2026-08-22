# 動画の上部バー / 下部シークバーを固定表示できるようにする (改訂 2)

正本は [next-release-backlog.md](../next-release-backlog.md) **§1.101**。
関連: [video-architecture.md](../video-architecture.md) (native presenter の節)。

## 0. この改訂について — 1 周目の前提が間違っていた

1 周目は「固定したバーを映像の**上に重ねる**」実装で完成し、コミット済み (`1acbfbfd`)。
しかし利用者確認で、**静止画のフルスクリーンに同じ機能が既にあり、挙動が違う**ことが判明した。

私 (ClaudeCode) が 1 周目のブリーフで前例として示した `FsSidePanelMode` (左右パネル) は
**誤った前例**だった。正しい前例は静止画の上部情報バー / 下部ページシークバーの固定表示で、
そちらは **領域を確保して画像をその手前までフィットさせる** (重ねない)。

**このブリーフは 1 周目の成果を作り直す。**ただし §5 に挙げた部分は**壊さずに残す**。

## 1. 静止画側の既存実装 (これに合わせる)

| | 場所 |
| --- | --- |
| 設定値 | `fullscreen_top_bar_locked` / `fullscreen_seek_bar_locked` (**bool**) |
| 設定 UI | [pages.rs:7469](../../src/ui_dialogs/preferences/pages.rs:7469) / [pages.rs:7506](../../src/ui_dialogs/preferences/pages.rs:7506) の **`ui.checkbox`** |
| 領域確保 | [ui_fullscreen.rs:7542](../../src/ui_fullscreen.rs:7542) `fullscreen_rect_excluding_fixed_bars` |
| 余白 | `fullscreen_fixed_bar_gap_px` (上下共通の 1 設定) |
| 鍵アイコン | [draw_icons.rs:486](../../src/ui_fullscreen/draw_icons.rs:486) `draw_seek_lock_icon` (ベクター描画) |
| 画面上の切替 | [ui_fullscreen.rs:12708](../../src/ui_fullscreen.rs:12708) 付近。クリックで設定反転 → `save()` → repaint |

設定の説明文 (静止画):

> ON のときはフルスクリーン下端にシークバー領域を確保し、画像をその上の領域にフィットします。
> 下部シークバー端の鍵アイコンからも切り替えできます。

## 2. 決まっていること (利用者判断、2026-08-22)

1. **動画専用の設定にする。**静止画の `fullscreen_*_locked` は**共有しない**。
   静止画の「ページシークバー」(ページ送り) と動画の「シークバー」(時間) は別の操作であり、
   released 設定の意味を後から変えて既存利用者の見え方を無言で変えない。
2. **領域を確保して映像をフィットさせる。**映像の上に重ねない。

## 3. やること

### 3.1 設定を bool 2 つに作り直す

`VideoBarVisibilityMode { Hover, Pinned }` と `video_top_bar_visibility` /
`video_seek_bar_visibility` を**廃止**し、静止画と同じ bool 2 つにする。

- **この 2 つは未リリース** (`v3.1.3` の後、今日 `1acbfbfd` で入っただけ)。
  **移行コードを書かない。**廃止してよい。CLAUDE.md「永続データ・スキーマ変更時の判断」参照。
- 命名は静止画の `*_locked` と既存の `video_*` 接頭辞の**両方に沿わせる**。
  揃えられない事情があれば報告に書く。
- 設定 UI は **ComboBox ではなく `ui.checkbox`**。文言は静止画の 2 項目と同じ調子にする
  (「ON のときは…領域を確保し、映像をその…の領域にフィットします。
  …端の鍵アイコンからも切り替えできます。」)。
- 余白は **`fullscreen_fixed_bar_gap_px` を再利用**する。動画専用の余白設定を増やさない。
  再利用が成り立たない理由が見つかったら、**実装せずに報告する**。

### 3.2 映像の領域を確保する

固定中のバーは映像に**かぶらない**。映像は残りの領域へ letterbox fit する。

[render_core.rs:8680](../../src/video/native_presenter/render_core.rs:8680)
`compute_video_visual_transform` が `(target_x, target_y, target_w, target_h)` を決めており、
VST の `compact` が既に「サブ矩形へフィットさせる」前例になっている。**同じ seam を使う。**

- フルスクリーンの presenter HWND は monitor 全域
  ([app.rs:43695](../../src/app.rs:43695) が `info.rcMonitor` を返す)。
  **HWND を縮めるのではなく、presenter 内部のフィット矩形を縮める。**
- `compact` (VST) と固定バーが**同時**に有効なときの合成を決めて報告する。
- 固定を解除したら元のフィットへ戻ること。

### 3.3 画面上で切り替えられるようにする

各バーの端に**鍵アイコン**を置き、クリックで固定 / 解除できるようにする。
静止画と同じく、設定を反転して保存し、再描画する。

- **鍵の見た目は静止画と同じにする。**`draw_seek_lock_icon` は
  `src/ui_fullscreen/draw_icons.rs` にあり `pub(super)`、native presenter は現在
  `draw_icons` を使っていない。**共有するか描き直すかを選び、理由を報告に書く。**
  ただし**同じ形を 2 箇所で別々に描く実装にはしない** (後で必ずずれる)。
- 絵文字・記号グリフを使わない (ベクター描画のまま)。CLAUDE.md「UI 文字列の Unicode グリフ選定ルール」。
- hover tip の文言も静止画に合わせる (「シークバー固定を解除」/「シークバーを固定表示」)。

## 4. ⚠️ 描画と hit-test は同じ述語を通す (1 周目と同じ、最重要)

[render_core.rs:5499](../../src/video/native_presenter/render_core.rs:5499) の不変条件は維持する。
**固定状態を region 側で再計算しない。**鍵アイコン自身も押せる必要があるので、
**アイコンの rect が region に入っていること**を確認する
(静止画と違い、ここは HWND の hit-test region が絡む)。

## 5. 1 周目の成果で、壊さずに残すもの

- 可視性の純関数へ**入力として**固定状態を渡す形 (呼び出し側で `|| pinned` しない)。
- `render_once` が算出した `top_bar_drawn_visible` / `bottom_hud_visible` を snapshot し、
  `compute_hud_regions` が**同じ値**を読む構造。
- **tile grid / navigation preview 中は上下とも抑止**する guard とそのテスト。
- **上部の固定を下部へ漏らさない** (上端 hover と左右パネルだけが従来どおり下部を連動表示)。
- 固定は external drag 中も維持し、`native_touch.chrome_latched()` は変えない。
- 音声専用の設定を増やさない (通常の音楽ビューは対象外、VST 用 audio-only shell だけ共有)。

これらのテストは**緑のまま**であること。意味が変わるものがあれば、変える前に報告する。

## 6. 制約

- **時間窓・sleep・retry で吸収しない。**
- 既定は OFF (= 現在の自動表示) のまま。既定の挙動を変えない。
- 旧 egui 動画 UI に設定を足さない。
- **静止画側の `!is_video` 述語を変えない** ([ui_fullscreen.rs:12379](../../src/ui_fullscreen.rs:12379) /
  [ui_fullscreen.rs:12403](../../src/ui_fullscreen.rs:12403))。今回は設定を共有しない。
- detached / viewport 述語に触る必要が出たら、**触る前に止めて報告する**。

## 7. テスト

- 上下それぞれの固定 / 解除が独立に効くこと (4 通り)。
- **固定中、映像のフィット矩形がバーの領域を除いていること** (純関数で固定できる)。
  余白 0 px と最大値の両端も見る。
- 固定中、描画と hit-test region が一致すること。**鍵アイコンが region に入ること。**
- 既定 (両方 OFF) が現在の hover 動作と同一であること (**回帰**)。
- 鍵アイコンのクリックで設定が反転し、保存されること。
- `compact` (VST) と固定バーの同時適用について §3.2 で決めた挙動。
- UI スナップショット (設定ページ)。既存の
  `preferences_video_bar_visibility_dark` は作り直しになる。

## 8. 完了条件

- `cargo fmt` 済み / `cargo test -p mimageviewer --lib` が緑
- `cargo test --test ui_snapshot` が緑
- `cargo check -p mimageviewer --bin mimageviewer-core` が通る
- `python scripts/check_ui_glyphs.py` が 0 件
- **docs の「映像の上に重なります」を全て直す**:
  [spec.md](../spec.md)、[video-architecture.md](../video-architecture.md)、
  [manual/settings.html](../../htdocs/mimageviewer/manual/settings.html)、
  [manual/video.html](../../htdocs/mimageviewer/manual/video.html)、
  および backlog §1.101 の実装済みメモ。
- **報告に、余白設定の再利用 / 鍵アイコンの共有可否 / compact との合成**を書く

> **実機確認が要る項目**: 映像のフィット、鍵アイコンのクリック、VST editor との重なり、タッチ。
> **ビルドまで用意して確認手順を残す** (エージェントは起動しない)。
