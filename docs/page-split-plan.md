# 横長ページの左右分割 (§1.119) 実装プラン

仕様の正本は [next-release-backlog.md](next-release-backlog.md) §1.119 (2026-08-24 の
「採用する簡略仕様」)。本書は**実装の当たりを付けた結果**を残す。読み直さないと
分からなかったこと、範囲を決めた根拠、次に触る場所を書く。

## 1. 現状 (2026-08-25)

| | 状態 |
| --- | --- |
| [page_split.rs](../src/page_split.rs) | 分割の順序 (純ロジック)。単体テスト 12 本 |
| `SpreadMode::SplitLtr` / `SplitRtl` | 追加済み。**`all()` に入れていない**ので UI には出ない |
| `App::fullscreen_page_slice` + bundle + `swap_field!` | per-viewer 状態として追加済み |
| ページ送り / 描画 / 着地 | **未実装。ここから** |

## 2. 回転したページの分割 — 解決済み (2026-08-25)

> 当初は「回転していないページだけ分割する」で出す案だった。**利用者判断で最初の
> リリースに含めることにした** — 回転したページだけ分割されないのは、使う側からは
> 不具合にしか見えず、報告が来る。以下は当初の見立てと、実際に必要だった修正。

### 当初の見立て (外れていた)

**分割の crop は、既存の自動表示トリムと同じ仕組みに乗る。** `content_bbox`
(正規化部分矩形) が `DisplayedImageTransform` へ渡り、`normalized_sub_rect` が描画矩形へ
写す。分割は `[0,0,0.5,1]` / `[0.5,0,1,1]` を渡すだけで済み、ナビゲータ・ルーペ・連結読みも
同じ経路なので付いてくる。ここまでは合っていた。

ただし `fs_image_fit_bbox` / `effective_bbox` が**回転していると矩形を捨てる**。捨てられると
同じページが 2 回そのまま出る。これを直すには `content_bbox: Option<egui::Rect>` を型付きの
`SourceCrop { Trim, Slice }` へ変えるしかない (215 箇所) —— と見立てた。**これが誤り。**

### 実際の原因

`content_bbox` は **`margin_fit::detect_content_bbox` が復号画素をそのまま走査して作る**ので
**元画像空間**である。ところが `resolve` は同じ矩形を 2 つの用途に使っていた:

- fit 倍率と `content_center`: `bbox.width() * display_size.x` —— `display_size` は**回転後**
- UV: `uv_rect = bbox` —— **元画像空間**

回転すると両立しない。**捨てていたのはその辻褄合わせ**で、矩形自体は回転しても使える。
UV 側は元々正しかった。

### 修正

用途ごとに座標系を分けた ([displayed_image_transform.rs](../src/displayed_image_transform.rs))。

- `rotate_bbox_to_display(bbox, rotation)` を追加。写像は screen ↔ source と同じ
  `forward_uv` を使う (別の式を書かない)。
- fit と paint rect は表示空間の矩形、UV は元画像空間の矩形。
- `effective_bbox` は**自由回転中だけ**降ろす (傾いた矩形の外接が広がる分の拡大量を解けない)。
  自由回転は保存しない一時値なので、やめれば次の解決で戻る。

**型の作り替えは不要だった。** `content_bbox` の型も 215 箇所も動かしていない。

**副産物: 表示トリムが回転したページでも効くようになった。** これまでは無効で、利用者から
見れば「回転すると自動トリムが外れる」状態だった。リリースノートに書く価値がある。

旧テスト `fit_rotation_and_trim_share_paint_and_hit_geometry` は「回転時 UV は全体」を
固定していた。**制限を仕様として固定していたテスト**なので、期待値を新しい不変条件
(部分矩形は回転しても同じものが UV になる) へ更新した。

正本は [display-pipeline.md](display-pipeline.md) の「部分矩形 (content bbox) の座標系」。

## 2.2 分割する条件

- 静止画であること (`is_spread_pairable_item`)
- `is_landscape` が真 (**保存回転を反映した**縦横比)。見開きのペアリングと同じ判定を使い、
  分割のために縦横比を読み直さない
- 自由回転 (`App::fs_free_rotation`) が 0 —— **保存しない App 全体の一時値**なので、
  per-idx ではなく 1 回だけ見る。0 でない間は分割を丸ごと止める。ステップ列は毎回
  作り直すので、回転をやめれば分割へ戻る

## 3. ページ送りの配線 (次にやること)

`spread_page_nav` → `spread_page_nav_for_indices` が中心。返す `FsPageNav` を
`handle_fs_navigation` が捌く。**production の呼び出しは 2 か所だけ**
([gamepad_input.rs:6196](../src/app/gamepad_input.rs:6196) /
[native_video.rs:9496](../src/app/native_video.rs:9496))、どちらも
`handle_fs_navigation` へ流す。

```rust
// spread_page_nav_for_indices の先頭
if let Some(steps) = self.build_presentation_steps_for_nav(nav) {
    return self.split_page_nav(&steps, fs_idx, dir);
}
if !self.spread_mode.is_spread() { return FsPageNav::Delta(base_delta); }
```

`FsPageNav` へ足す変種は **1 つ**にする:

```rust
/// 分割表示での移動先。同じページの反対側へ移る場合も、別ページへ移る場合も
/// これで表す (`PresentationStep` が元ページと左右の両方を持つ)。
Split(crate::page_split::PresentationStep),
```

- **`Target(usize)` を `Target { idx, slice }` に変えない。** 既存のテストと分岐が
  `Target(4)` の形に依存しており、見開きの意味 (表示ユニットの先頭) は分割と別である。
- 前へ進んで別ページに入るときは相手の**最初の半分**、後ろへ戻って別ページに入るときは
  相手の**最後の半分**へ着地する。`presentation_steps` の並びがそのまま答えになるので、
  呼び出し側で向きを再計算しない。
- 現在位置は `(fullscreen_idx, fullscreen_page_slice)` でステップ列を引く。見つからない
  (寸法が届いてステップ列が変わった直後など) 場合は `landing_step` で最初の半分へ倒す。

`handle_fs_navigation` 側:

- `Split(step)` で `step.source_idx == fullscreen_idx` なら `fullscreen_page_slice` を
  差し替えて repaint するだけ。**ページ遷移の機構を通さない** (テクスチャは同じ)。
- 別ページなら既存の `Target` と同じ遷移 (`begin_fs_page_navigation_sequence` →
  `land_still_page_navigation_target`) を通し、**着地後に** `fullscreen_page_slice` を
  `step.slice` にする。遷移前に置くと、遷移が失敗したときに左右だけ動く。
- スライドショーの分岐 ([ui_fullscreen.rs:23409](../src/ui_fullscreen.rs:23409)) は
  分割を使わない (`is_spread()` が偽なので `Delta(1)` へ行く)。`Split` は届かないが、
  match の腕は `None` で塞ぐ。

## 4. 描画

`fullscreen_page_slice` を `content_bbox` に変換して既存経路へ渡す。トリムとの併用は
しない —— **分割中はトリムを無効**にする (仕様。両方が `content_bbox` を要求するため、
どちらか一方しか渡せない)。1 か所の resolver に集約し、ページ送り・描画・ナビゲータ・
ルーペで左右の解釈を重複させない。

## 5. MVP に入れないもの

- **縦連結**での 2 領域展開 (ステップ列は同じものを使える)
- 横連結、通常の見開きとの併用 (排他モードなので併用は存在しない)
- シークバーの「12ページ・左側」表示
- リモート (元ページ表示を維持)
- 分割位置の手動調整
- 既定キー割り当て (1〜5 が埋まっているため。プルダウンから選ぶ)

## 6. 完成の条件

`all()` に 2 モードを足すのは**ページ送りと描画が揃ってから**。選べるのに何も起きない
状態を master に置かない。マニュアル ([fullscreen.html](../htdocs/mimageviewer/manual/fullscreen.html))
と製品ページへの追記も同時に行う。

回帰条件は backlog §1.119 の「回帰条件」に列挙済み (左→右 / 右→左の往復、横長と縦長の
混在、回転後の分割判定、先頭 / 末尾、キー長押し、縦連結、シーク、編集画面への出入り、
ブックマーク再表示)。
