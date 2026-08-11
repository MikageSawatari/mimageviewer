# ブリーフ: 通過表示が「色を待つ」規則を迂回している

## 前提 (必ず守ること)

- **アプリを起動しない**。ビルドとテストまでで止める。
- **git 操作をしない**。master の作業ツリーに未コミットのまま残す。統合はこちらで行う。
- 着手前に [docs/display-pipeline.md](display-pipeline.md) の
  **「ページ送り中の表示規則」** 節を読むこと (この修正と同時にこちらで追記する)。

## すでに存在する規則

`prepare_fullscreen_state` に、**カラー化が要る絵では白黒サムネイルを出さない**規則が
以前から入っている ([ui_fullscreen.rs:13477](../src/ui_fullscreen.rs:13477) 付近):

```rust
let waiting_for_colorize = matches!(paint_source, FsPageTurnPaintSource::Composite)
    && !original_preview_active
    && tex.is_none()
    && self.colorize_display_requires_final_effect(fs_idx);
let thumb_tex = if waiting_for_colorize {
    // 生サムネイルは白黒なので使わない。
    None
} else {
    /* サムネイルを使う */
};
```

`colorize_display_requires_final_effect`
([ui_fullscreen.rs:3850](../src/ui_fullscreen.rs:3850)) が判定の正本で、対象は**色の最終段だけ**:

- Creative LUT が乗る
- カラー化が `AllImages`
- カラー化が `MonochromeOnly` で、色補正が identity でない / 判定材料が揃っていない / 残差が
  モノクロ相当

消しゴム・隠蔽・注釈は**対象外** (意図的。色味だけが影響が大きいという判断)。
関数内のコメントに、この規則が**利用者報告 2026-07-29** (「LUT 未適用の絵が 1 フレーム見えてから
色が変わる」) に対応して入ったことが記録されている。

## 壊れている前提

上の `waiting_for_colorize` は **`paint_source == Composite` の枝でしか評価されない**。
§1.58 で入れた通過表示は `paint_source = Thumbnail` を返し、**この枝に入る前に生サムネイルを
描いてしまう**。つまり通過表示が、既存の「色を待つ」規則を丸ごと迂回している。

実測 (2026-08-11、カラー化した PDF をキー押しっぱなしで往復):

```
送り: idx=2..7 カラー / idx=8 白黒 / idx=9..12 カラー / idx=13 白黒 / ...
戻り: idx=30..22 カラー / idx=21..1 すべて白黒
```

完成画像がキャッシュに在るページだけカラー、無いページは白黒サムネイル。隣り合うページで
色と白黒が交互になる。

`ThumbnailPassThrough` は `1ea4d824` (v2.13.0、未リリース) で入ったので、**出荷前の退行**。

## やること

`colorize_display_requires_final_effect` を**通過表示の判定そのものへ入れる**。
描画側の後段で打ち消すのではなく、**`paint_source` が `Thumbnail` にならないようにする**。

- 判定に第 4 の入力 `thumbnail_is_faithful` (仮称、命名は任せる) を足す。
  値は `!colorize_display_requires_final_effect(idx)`。
- **見開き / 表示単位では、単位内のどれか 1 ページでも色待ちが要るなら単位全体で通過表示を
  使わない**。`fs_page_turn_display_unit_readiness` が既に単位で回しているので、そこへ足す。
- `defer_ui_uploads` は**変えないこと**。UI スレッドのアップロード保留は §1.58 の中核で、
  描画元とは別の軸 (`00d23a33` で分離済み)。色待ちのページでも保留自体は続けてよい。
- 後段の `waiting_for_colorize` は**残す**。通過表示に入らなくなっても、完成画像がまだ無い
  フレームでは従来どおり白黒サムネイルを避け、「カラー化中」の表示に倒れる必要がある。

### 効果の確認 (退行させないこと)

- **色に関わる処理が乗っていないページ**では、従来どおり通過表示が働くこと。§1.58 の
  「押しっぱなしで引っかからない」はここで保たれる。
- カラー化した本では通過表示に入らないので、ページ送りは §1.58 以前の速度に戻る。
  **これは意図した引き換え**であり、直すべき退行ではない。

## 完了条件 / 回帰テスト

純関数レベル:

| 入力 pending | サムネイル | 完成画像 | 色待ち要 | 描くもの | upload 保留 |
| --- | --- | --- | --- | --- | --- |
| true | あり | なし | **なし** | サムネイル | する |
| true | あり | なし | **あり** | **完成画像 (= 通過しない)** | する |
| true | あり | あり | あり / なし | 完成画像 | する |
| false | — | — | — | 完成画像 | しない |

状態遷移:

- 色待ちが要るページを含むバーストで、**どのフレームでも `paint_source` が `Thumbnail` に
  ならない**こと。
- 色待ちが不要なページだけのバーストでは、従来どおり通過表示が出ること
  (§1.58 の効果が消えていないことの担保)。
- 見開きで**片方のページだけ**色待ちが要る場合、単位全体が通過しないこと。

- `cargo fmt --check` / `cargo check -p mimageviewer --bin mimageviewer-core` が warning なしで通る。
- `cargo test -p mimageviewer --lib page_turn` と `... --lib colorize` が通る。

## 報告してほしいこと

- 足した入力の名前と、見開き単位での扱い。
- `waiting_for_colorize` の後段をそのまま残せたか。残せなかったならその理由。
- 追加したテストの一覧。
- **色補正だけ (カラー化 Disabled + 色調補正あり) のページ**が現状どう扱われるか。
  `colorize_display_requires_final_effect` は `MonochromeOnly` の枝でしか
  `is_color_identity` を見ていないように読めるので、事実だけ報告すること。
  **この範囲を広げる変更は今回入れない** (利用者判断待ち)。
