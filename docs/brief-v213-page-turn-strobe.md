# ブリーフ: ページ送り中に、同じページのサムネイルと完成画像が交互に出る

## 前提 (必ず守ること)

- **アプリを起動しない**。ビルドとテストまでで止める。
- **git 操作をしない** (commit / add / stash / branch / reset いずれも)。master の作業ツリーに
  未コミットのまま残す。統合はこちらで行う。
- 作業ツリーは他セッションと共有している。触った範囲を報告すること。
- 着手前に [docs/display-pipeline.md](display-pipeline.md) と
  [docs/preset-and-adjustment.md](preset-and-adjustment.md) を読むこと。

## 観測 (v2.13.0 出荷前の実機確認、2026-08-11)

利用者報告: 「PDF のページ送りがおかしい。ページ移動ごとに、同じページが繰り返し再表示されて
いるような動きに見える」「一瞬カラー化されていないページのフレームが混ざってそうにも見える」。
画面録画あり。カラー化を有効にした PDF (`20181221_...画集.pdf`) をページ送りキーの
押しっぱなしで読んでいる状況。

perf log (`--perf-log`) の `fs.page_turn_ready` を並べると、**同一 idx のまま
`pass_through/thumbnail` と `materialized/final_composite` が毎フレーム交互に出ている**:

```
idx=0 t=105.0 pass_through/thumbnail
idx=0 t=105.1 materialized/final_composite
idx=0 t=105.1 pass_through/thumbnail
idx=0 t=105.1 materialized/final_composite
...  (0.5 秒で 14 往復)
```

`t=104..122` の集計は `pass_through/thumbnail` 51 件 / `materialized/final_composite` 30 件。
ページは動いていない (`idx` は同じ) のに、表示だけが 2 つの絵の間で往復している。

これが利用者の見た 2 つの症状の**両方**を説明する:

1. **「同じページが繰り返し再表示される」** — 同じページの低解像度サムネイルと完成画像が
   交互に出るので、ページが戻って再描画されたように見える。
2. **「カラー化されていないフレームが混ざる」** — pass-through が出すのは
   **カタログサムネイル = カラー化・補正を通す前の絵**。完成画像はカラー化後。
   つまり毎フレーム モノクロ ⇄ カラー が入れ替わる。

## 壊れている前提

判定は純関数 [ui_fullscreen.rs:6509](../src/ui_fullscreen.rs:6509) の
`page_turn_materialization_for_inputs` で、入力は 2 つだけ:

```rust
if page_turn_input_pending && catalog_thumbnails_ready {
    FsPageTurnMaterialization::ThumbnailPassThrough
} else {
    FsPageTurnMaterialization::Materialize
}
```

`page_turn_input_pending` は「このフレームに未消費のページ送りキー入力が残っているか」
(§1.58 の設計どおり、時間閾値ではなく Win32 frame edge queue を読む)。キーリピートは
30 回/秒程度で届くので、60fps では**おおむね 1 フレームおきに true になる**。

この関数は「**現在の idx の完成画像がもう出来ているか**」を知らない。そのため、完成画像を
出せる状態でも、入力が残っているフレームではサムネイルへ落ちる。§1.58 が意図していたのは
「**まだ出来ていないページを待たずに通り過ぎる**」ことなので、**出来ているものをわざわざ
下げる**のは意図の外側にある。

静止画では 2 つの絵の差が解像度だけなので目立たず、実機確認を通ってしまった
(利用者も「一瞬サムネイル画質になるのは許容」と判断済み)。カラー化を有効にすると
差がモノクロ/カラーになるため、はっきり見える。

**カラー化の白黒 → カラーの切り替わりをユーザーに見せないことは確定要件**である
(利用者判断、既存)。今の挙動はこれに真正面から反する。

## 直し方

`page_turn_materialization_for_inputs` に**第 3 の入力**を足し、
「現在 idx の最終合成がすでに利用可能」なら `page_turn_input_pending` にかかわらず
`Materialize` を返す。

```rust
fn page_turn_materialization_for_inputs(
    page_turn_input_pending: bool,
    catalog_thumbnails_ready: bool,
    final_composite_available: bool,   // 追加
) -> FsPageTurnMaterialization
```

- `final_composite_available` は「**このフレームで、新しい仕事を始めずにその idx の完成画像を
  出せるか**」。producer (`resolve_fs_processed_texture` 相当) を呼んで**新規に生成させては
  いけない**。キャッシュ在住かどうかの読み取りだけで判定する。
- 見開き・連結読みでは、**表示単位に含まれる全ページが揃っているとき**だけ available とする。
  片側だけ完成している状態で `Materialize` へ倒すと、揃うまで別の欠けた絵が出る。
  判定の単位は既存の display unit に合わせること。
- 判定は `fs_page_turn_materialization_for_frame` の既存のフレームキャッシュ
  (`frame_nr` / `items_generation` / `idx`) にそのまま乗せる。同一フレーム内で判定がぶれない
  という現在の不変条件を壊さないこと。

### 同型の経路を洗い出すこと

この判定の consumer は 1 か所ではない。**片方だけ直して終わりにしない**
(このプロジェクトで同型の直し残しを何度も出している)。少なくとも次を確認する:

- [app.rs:54426](../src/app.rs:54426) と [app.rs:59865](../src/app.rs:59865) の 2 か所
- [ui_fullscreen.rs:10743](../src/ui_fullscreen.rs:10743)
- 単ページ / 見開き / 縦連結 / 横連結
- main embedded と detached の両方 (detached はリワーク中につき、症状パッチではなく
  構造的修正であることを説明できる形にすること。プラン §2 を読むこと)

## 完了条件 / 回帰テスト

純関数のテストで次を固定する:

1. 入力 pending + サムネイルあり + **完成画像あり** → `Materialize`
   (**今回の退行を直接押さえるケース**)
2. 入力 pending + サムネイルあり + 完成画像なし → `ThumbnailPassThrough` (§1.58 の意図は維持)
3. 入力なし → 常に `Materialize`
4. サムネイルなし → 常に `Materialize`

加えて **状態遷移テスト**を 1 本足す: 同一 idx のまま「入力 pending」が
true / false / true / false と続くフレーム列で、完成画像が用意できた後の判定が
**`Materialize` から二度と `ThumbnailPassThrough` へ戻らない**こと。これが今回の症状
(交互に出る) を直接表す不変条件。

- `cargo fmt` 済み、`cargo fmt --check` が通る。
- `cargo check -p mimageviewer --bin mimageviewer-core` が warning なしで通る。
- `cargo test -p mimageviewer --lib page_turn` が通る。

## 報告してほしいこと

- `final_composite_available` をどこから読んだか (関数名)。新規生成を起こさないと言える根拠。
- 見開き / 連結読みの display unit をどう扱ったか。
- 触った consumer の一覧と、detached 経路に触れたならその判断理由。
- 追加したテストの一覧。
