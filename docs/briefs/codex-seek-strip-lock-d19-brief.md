# D19: シークストリップの固定 (鍵アイコン)

正本: `docs/video-seek-strip-plan.md` の **D19 (未実装)** 節。ブランチ `video-strip`
(worktree `C:\home\mimageviewer-video-strip`)。実装前に正本の D19 節と、D12 / D13 / D18 を読むこと。

## 目的

ストリップが見やすいので常時出しておきたい、という利用者要望。固定すると**ストリップの領域を
常に確保し、映像はその内側に表示する**。既存の下部バー固定 (`video_seek_bar_locked` と
`bottom_reserved`) と同じ仕組みに乗せる。新しいレイアウト構造は作らない。

## 決まっていること (利用者判断 2026-08-25、変更しない)

- **到達できる状態は 3 つだけ**。
  1. 固定なし
  2. 下部 HUD だけ固定 (ストリップは必要なときだけ出す) — 既存の挙動
  3. 下部 HUD + ストリップ固定 (常時閲覧)

  「HUD 非固定 + ストリップ固定」は**作らない**。
- **既存の `video_seek_bar_locked` (bool) は変えない**。これはリリース済みの設定なので、
  3 値の列挙へ作り変えると利用者の設定 DB にある旧値の移行が要る。移行コードを書かないこと。
- 代わりに `video_seek_strip_locked` (bool、**未リリース**) を足し、**両方向から**不変条件を守る。
  - ストリップ固定を ON → バー固定も ON にする
  - バー固定を OFF → ストリップ固定も OFF にする
- 固定は**ストリップが出ているときだけ意味を持つ**。ストリップを「なし」にしたら領域を解放する。
  ただし `video_seek_strip_locked` の値自体は保持する (再度出したときに固定が戻る)。
- 鍵アイコンは**ストリップの右上**。D18 の現在値表示 (「画像間隔 15 秒」/「表示範囲 3 分」) も
  同じ右上にあるので、**両方の配置を同時に決める**こと。

## 実装の要点

### 1. 状態の所有 — `(false, true)` を型で表現できなくする

CLAUDE.md「相互排他的な状態を複数の bool / Option で表現している場合、新しい分岐を足さず、
単一の typed request / state owner へ集約する」に従う。**2 つの bool に if を 2 本足す実装にしない。**

`src/settings.rs` に、到達できる 3 状態だけを持つ型を置く:

```rust
/// 動画下部の固定状態。到達できるのはこの 3 つだけで、
/// 「バー非固定 + ストリップ固定」は表現できない。
pub enum VideoBottomLock { None, BarOnly, BarAndStrip }
```

- `from_settings(bar: bool, strip: bool) -> Self` — 2 つの bool (= 永続形) から復元する。
  `(false, true)` は手編集や旧 DB からしか来ないので `BarOnly` ではなく **`None`** へ正規化する
  (ストリップ固定はバー固定を含意するので、含意元が無い指定は無効とみなす)。
- `to_settings(self) -> (bool, bool)` — 永続形へ戻す。
- `with_bar(self, locked: bool) -> Self` / `with_strip(self, locked: bool) -> Self` — 上の
  2 方向の含意をこの 2 メソッドだけが持つ。呼び出し側に if を書かない。

`Settings` 側の bool 2 本は**シリアライズ形**として残し、変更経路は必ずこの型を通す。
`video_seek_strip_locked` は `#[serde(default)]` + 既定 `false`。**移行コードは書かない**
(未リリース設定。その旨をコミットメッセージに 1 行残す)。

**テスト** (`src/settings.rs` の `#[cfg(test)]`):
- `with_strip(true)` は必ずバー固定も立てる。
- `with_bar(false)` は必ずストリップ固定も落とす。
- `to_settings()` の戻り値が `(false, true)` になる状態が存在しない (3 variant を総当たり)。
- `from_settings(false, true)` が `None` に正規化される。
- 既定は `None` (どちらも false)。

### 2. トグルの入口を 1 本にする

現在 `src/app/native_video.rs` の `toggle_native_video_bar_lock_setting` が
`NativeVideoBar::{Top, Seek}` の bool を直接反転している。ここを上の型経由に書き換える。

- `NativeVideoBar::Top` は**この不変条件と無関係**。従来どおり独立に反転する。
- `NativeVideoBar::Seek` の反転は `VideoBottomLock::with_bar` を通す。
- ストリップ固定の反転用に新しい command を足す (`NativeOverlayCommand` / `NativeVideoOutputEvent`
  の `ToggleBarLock` に第 3 の bar を足すか、専用 command にするかは実装の判断に任せる。
  ただし `NativeVideoBar` に `Strip` を足す場合、`Top`/`Seek` と違って**バーではない**ので、
  既存の `native_bar_lock_buttons_are_inside_the_same_snapshot_regions_as_their_bars`
  のような bar 前提のテスト・snapshot region 判定に巻き込まれないか確認すること)。
- トグル後の toast は既存の `ToggleBarLock` 経路 (`"上部情報バー"` / `"下部シークバー"`) に
  合わせて `"シークストリップ"` を出す。

### 3. 領域の確保

`src/video/native_presenter/render_core.rs` の `compute_video_visual_target_rect` /
`VideoVisualLayout` に、ストリップ固定を反映する。

- `bottom_reserved` は現在 `(HUD_BOTTOM_HEIGHT + gap_points) * ppp`。ストリップを固定して
  **かつストリップが実際に出ているとき**は `+ SEEK_STRIP_HEIGHT * ppp` する。
  `native_seek_strip_rect` はストリップを下部 HUD の**すぐ上**に隙間なく置くので、
  gap を 2 回足さないこと。
- **「実際に出ている」の正本を 1 つに決める。** ストリップ snapshot は
  `player.set_native_seek_strip(overlay)` (`src/app/native_video.rs` の
  `sync_native_video_seek_strip` 末尾、`overlay: Option<NativeOverlaySeekStrip>`) で
  presenter に渡る。`Some` = 出ている。`video_seek_strip_state != None` を presenter 側で
  再判定しない (長さ不明素材・material unavailable・tile モード・音声モードで閉じる経路が
  あり、設定値だけでは出ているかどうかが決まらない。Increment 12 / `axis_unavailable` を参照)。
- レイアウトが変わったら `update_video_visual_transform` が走ること
  (`set_overlay_bar_lock_state` が `layout_changed` で呼んでいるのと同じ扱い)。
  ストリップの出現・消失でも映像矩形が変わるので、**snapshot の Some/None が変わった時点でも**
  transform を更新する必要がある。ここは既存 lock 経路と別の入口になるので取りこぼさないこと。
- 既存テスト `compute_video_visual_target_rect` 群 (`render_core.rs` の 12844 付近) と同じ形で、
  固定 3 状態 × ストリップ有無の矩形をテストする。

### 4. 鍵アイコンとレンジ表示の配置

`draw_native_seek_strip` (`render_core.rs`、レンジ表示は 420-445 行付近) に鍵ボタンを足す。

- 既存の `crate::ui_fullscreen::draw_icons::draw_seek_lock_icon` と
  `draw_overlay_button_bg` を使う (`overlay_draw.rs` の `draw_native_bar_lock_button` が手本)。
  **絵文字・記号グリフを使わない** (CLAUDE.md「UI 文字列の Unicode グリフ選定ルール」。
  ストリップのフィルムアイコンも同じ理由でベクター描画になっている)。
- 配置: 鍵をストリップ矩形の**右上角**に置き、D18 のレンジ表示を鍵の**左**へ寄せる。
  レンジ表示は現在 `local_rect.right_top() + vec2(-7.0, 5.0)` の `RIGHT_TOP` 揃えなので、
  鍵ボタンの幅 + 余白だけ左へずらす。鍵とテキストが重ならないことを実機で確認する。
- tooltip は `hover_tip_dark` で「ストリップを固定表示」/「ストリップ固定を解除」。
- 鍵の上でのクリックが**ストリップ本体のドラッグ・シーク**を起こさないこと。
  `draw_native_seek_strip` はストリップ矩形全体に `Sense::click_and_drag` の
  `ui.interact` を張っているので、鍵の `interact` を**先に**登録して pointer を奪うか、
  ストリップ側の drag 開始判定で鍵の矩形を除外する。どちらでもよいが、**ホイールによる
  レンジ変更 (D18) が鍵の上でも従来どおり効くこと**を壊さない。

### 5. 環境設定

`src/ui_dialogs/preferences/pages.rs` の 6076 行付近に既存のバー固定チェックボックスがある。
その隣にストリップ固定を足し、**同じ不変条件を通す** (チェックボックス 2 つが直接 bool を
触るのではなく、§1 の型を経由する)。バー固定を外したときにストリップ固定のチェックが
同じフレームで外れること。

### 6. マニュアル・ドキュメント

- `docs/video-seek-strip-plan.md` の D19 節を「実装済み」に更新し、実装状況の行から D19 を外す。
  決めた配置 (鍵とレンジ表示の位置関係) を書き残す。
- `htdocs/mimageviewer/manual/` の動画まわりのページに、ストリップ固定の説明を足す。
  **バージョン番号を書かない**、**内部用語を書かない** (CLAUDE.md「マニュアル・製品ページの
  記述方針」)。
- `docs/spec.md` の設定項目一覧に `video_seek_strip_locked` を足す。

## やってはいけないこと

- `video_seek_bar_locked` を列挙へ作り変える / 移行コードを書く。
- 2 つの bool を別々の場所で書き換え、if で辻褄を合わせる。
- 到達不能な `(bar=false, strip=true)` を「起きないはず」としてテストなしで放置する。
- 設定値 `video_seek_strip_state` だけを見て presenter 側で「出ている」を再判定する。
- ストリップが「なし」のときも領域を確保したままにする。
- 症状パッチ (固定が効かないときの追加 repaint / 一括 reset / silent fallback)。
  構造で解けないなら実装せずに報告すること。

## 完了条件

- `cargo fmt` 済み。
- `cargo check -p mimageviewer --bin mimageviewer-core` が通る。
- `cargo test -p mimageviewer --lib` が通る (新規テストを含む)。
- `python scripts/check_ui_glyphs.py` が 0 件。
- 上の §1 / §3 のテストが入っている。
- 実機確認は**こちら (ClaudeCode) が利用者へ依頼する**ので、Codex 側では起動しないこと。

## 報告してほしいこと

- `VideoBottomLock` をどこへ置き、どの経路がそこを通るようになったか。
- 「ストリップが出ている」の正本にどれを選んだか、`Some/None` 変化で transform 更新を
  どこに足したか。
- 鍵とレンジ表示の最終的な座標。
- 判断に迷った点と、正本のどの記述を根拠にしたか。
