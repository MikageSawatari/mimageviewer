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

## 2. 範囲を「回転していないページ」に限定した理由

**分割の crop は、既存の自動表示トリムと同じ仕組みに乗る。** `content_bbox`
(元画像の正規化部分矩形) が `DisplayedImageTransform` へ渡り、
`normalized_sub_rect` が描画矩形へ写す。分割は `[0,0,0.5,1]` / `[0.5,0,1,1]` を
渡すだけで済む。ナビゲータ・ルーペ・連結読みも同じ経路を通るので、そこも自動で付いてくる。

ただし **`fs_image_fit_bbox` は回転していると `content_bbox` を捨てる**:

```rust
content_bbox.filter(|_| rotation.is_none() && free_rotation_rad.abs() <= TRANSFORM_EPSILON)
```

トリムにとっては妥当な割り切りだが、分割で捨てられると**同じページが 2 回そのまま出る**
(左半分のはずが全体、右半分のはずも全体)。これは見て分かる不具合なので許容できない。

選択肢は 2 つあった:

1. `content_bbox: Option<egui::Rect>` を型付きの `SourceCrop { Trim, Slice }` へ変える。
   回転で落とすかどうかを型で分ける。**正しいが 215 箇所** (`displayed_image_transform` /
   `ui_fullscreen` 109 / `margin_fit` / `remote_ipc` / `pdf_loader` / `ui_view_trim` / tests)。
   §1.119 の範囲を超える。
2. **分割対象を「回転していないページ」に限る。** crop が実際に効く条件と、分割する条件を
   一致させる。回転して横長になったページは分割されず、そのまま 1 枚で出る。

**2 を採る。** 1 は独立した改修として後で立てる (トリムが回転時に効かない問題も同時に直る)。

- 保存回転は per-page なので、判定はステップ列を作るときに per-idx で見る。
- 任意角度回転 (`App::fs_free_rotation`) は**保存しない App 全体の一時値**なので、
  0 でない間は分割を丸ごと止める (per-idx ではなく 1 回の判定)。ステップ列は毎回作り直す
  ので、回転をやめれば分割へ戻る。
- **`is_landscape` は回転を反映して判定する**ので、見開きのペアリングと分割とで
  「横長」の範囲が食い違う (回転して横長のページは、ペアにはなるが分割はされない)。
  意図的な差である。

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
- 回転したページの分割 (§2)
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
