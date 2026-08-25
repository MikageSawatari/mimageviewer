# 横長ページの左右分割 (§1.119) 実装プラン

仕様の正本は [next-release-backlog.md](next-release-backlog.md) §1.119 (2026-08-24 の
「採用する簡略仕様」)。本書は**実装の当たりを付けた結果**を残す。読み直さないと
分からなかったこと、範囲を決めた根拠、次に触る場所を書く。

## 1. 現状 (2026-08-25)

| | 状態 |
| --- | --- |
| [page_split.rs](../src/page_split.rs) | 分割の順序と座標写像 (純ロジック)。単体テスト 15 本 |
| `SpreadMode::SplitLtr` / `SplitRtl` | 追加済み。`all()` にも入れたのでメニューから選べる |
| `App::fullscreen_page_slice` + bundle + `swap_field!` | per-viewer 状態 |
| 回転したページの分割 | **対応済み** (§2) |
| ページ送り (`FsPageNav::Split`) | 実装済み。App レベルのテスト 3 本 |
| 描画 (`fs_page_content_bbox`) | 実装済み。分割中はトリムより分割が勝つ |
| 着地 (`reconcile_fullscreen_page_slice`) | 実装済み。表示側で毎フレーム 1 か所そろえる |
| 縦連結 | 実装済み。段ごとに片側を持つ |
| 実機確認 | 通常表示は確認済み (2026-08-25)。縦連結は**未** |

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

**表示トリムは、まだ回転したページで効かない** (2026-08-25 訂正)。変換側で扱えるように
なっただけで、**トリムを作る側に同じ規則の複製が残っている**:

```rust
let content_bbox = if rotation.is_none() { self.view_trim_single_content_bbox(idx) } else { None };
```

`capture_fs_display_unit_*` (単ページ / 見開き)、Z ズーム、連結読みの 4 か所。コメントには
「回転ページは draw_fs_image 側で bbox を使わない**ので**」と、消費側の振る舞いを理由として
書いてある —— 対になっていた片方だけが変わった状態である。

**外すのは §1.119 とは別件**にする。見開きの `view_trim_spread_content_bboxes` は左右の
余白そろえを**表示空間で**定義しており、180 度回転で左右が入れ替わるページに素直には
適用できない。分割は自前の resolver (`fs_page_content_bbox`) から矩形を出すので、
このガードの影響を受けない。

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

## 4. 描画 — 解決は `draw_fs_image` が所有する

分割中は分割の矩形が `content_bbox` を占める。トリムとの併用はしない (両方が同じ 1 つの
枠を要求するため、どちらか一方しか渡せない)。

**解決は呼び出し側に任せない。** `draw_fs_image` が先頭で `fs_page_content_bbox` を通し、
呼び出し側は素の表示トリムを渡すだけにする。

> **実機で踏んだ (2026-08-25)**: 最初は呼び出し側で解決する形にした。`content_bbox` を
> 作る場所は散っていて (holdover 保存 / Z ズーム / 通常表示 / 連結読み / PDF 解像度 /
> detached 焼き込み)、そのうち**通常表示の 1 か所を通し忘れた**。結果、ページ送りは
> 半分ずつ進むのに絵は横長のまま。「認識はしているが割れない」という、原因が
> 見えにくい形になった。
>
> 数えて塞ぐだけでは同じ穴がまた開く。**実際に描くのは `draw_fs_image` ただ 1 つ**なので、
> そこが解決を所有すれば呼び出し側は通し忘れようがない。

例外は Z ズーム。倍率と寄せ先を `draw_fs_image` より前に決めるので、その計算にも
分割後の矩形が要る。ここだけ `fs_page_content_bbox` を先に呼ぶ。

## 4.1 縦連結

**同じステップ列から段を組む。** 分割したページは 2 段になり、それぞれが自分の側の矩形を
持つ。並べる順序はページ送りの順序と同じものなので、連結用に別の列は作らない。

- 現在位置は**左右まで見て**選ぶ (`ContinuousReadingUnitSpec::is_step`)。同じ元 index の段が
  2 つ並ぶので、`contains_idx` だけでは必ず先頭の段に吸われる。テクスチャ・先読み・
  キャッシュは元ページ単位のままでよいので、`contains_idx` はどちらの段にも真を返す。
- 連結読みの描画は `draw_fs_spread_page` を通り、`draw_fs_image` の解決は通らない。
  段ごとの矩形がそのまま使われるので、**現在の左右で上書きされない**。
- **スクロールで段が変わったら左右も採り直す** (`reanchor_continuous_reading_viewer` の
  `new_slice`)。分割中は元 index が変わらないまま段だけが変わるので、ここを書かないと
  次のフレームで現在位置が元の段へ解決され、スクロールが引き戻されて先へ進めなくなる。
  引数で渡すので、呼び出し側は左右を決めずに済ませられない。
- `ContinuousReadingPageSize` にも `content_bbox` と寸法の座標系のずれがあった
  (`width` / `height` は回転後、`content_bbox` は元画像空間)。`rotation` を持たせ、
  `bbox()` が表示空間へ写すようにした。§2 と同じ形の修正である。

## 5. 入れないもの

- **「12ページ・左側」のような片側表示** (2026-08-25、利用者判断で**不採用**)。ページ番号は
  元ページ単位のまま「12 / 180」で据え置く。番号が半分ごとに変われば総ページ数と食い違い、
  片側を添えると表示が増える割に分かることが少ない。**保留ではなく決定なので蒸し返さない。**
- 横連結、通常の見開きとの併用 (排他モードなので併用は存在しない)
- シークバーの「12ページ・左側」表示
- 分割位置の手動調整
- 既定キー割り当て (1〜5 が埋まっているため。プルダウンから選ぶ)

## 5.1 リモートは対象に含めた (2026-08-26)

当初は「元ページ表示を維持」= 対象外としていたが、**スマートフォンの縦画面こそ分割が
一番効く**ため利用者判断で実施した。サーバ側 `83f1d5a7`、端末側 `03ced000`。設計と
実装の結果は [web-remote-plan.md](web-remote-plan.md) §15。

本体と同じ `page_split::presentation_steps` から group を作るので、並べ替えの規則を
wire 用に書き直していない。端末側は元ページ 1 枚を受け取って CSS で切り抜くため、
サーバ側の切り出しも追加の転送も無い。

## 6. 残り

- **実機確認**。回帰条件は backlog §1.119 に列挙済み (左→右 / 右→左の往復、横長と縦長の
  混在、回転後の分割判定、先頭 / 末尾、キー長押し、縦連結、シーク、編集画面への出入り、
  ブックマーク再表示)。**リモートはスマートフォン実機で**、縦持ち・向きの変更・
  ページ送り・モード切替後の位置維持・補正プレビュー後の左右保持を見る。
- 製品ページ ([index.html](../htdocs/mimageviewer/index.html)) への追記。
- リリース時の更新履歴。

### 着地を 1 か所で解いている理由

ページを開く入口は多い (一覧・しおり・検索・シークバー・履歴・<kbd>Ctrl</kbd>+↑↓・
detached)。そのすべてに「左右を戻す」を足すと必ずどれか漏れ、**前のページの右半分を
見ていた記憶が次のページに残る**。`reconcile_fullscreen_page_slice` が
`render_fullscreen_viewport` の先頭で毎フレーム 1 回そろえるので、入口が増えても漏れない。
分割していないページと分割 OFF では `Full` へ戻すため、記憶が居残ることもない。
