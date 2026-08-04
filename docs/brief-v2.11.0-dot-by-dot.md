# ブリーフ: 100%原寸のドットバイドット化 + 描画矩形のピクセルスナップ (v2.11.0 段階1・2)

対象: v2.11.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode。

**正本は [docs/dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md)。
着手前に必ず全文を読むこと。** 本ブリーフはそのうち **段階1・2 のみ** を対象とする
(段階3・4 の GPU Lanczos は別ブリーフで後続)。

---

## 1. やること

正本 §4.1 と §4.2 を実装する。

### 段階1: 物理ピクセル 1:1

`FullscreenFitMode::Original` と `FullscreenFitScaleLimits`(拡大しない / 縮小しない) の
基準を、論理ポイントから**物理ピクセル**へ変更する。

- `Original` の倍率を `1.0` から `1.0 / pixels_per_point` にする
- `apply()` のクランプ基準も `1.0 / pixels_per_point` にする
- `DisplayedImageTransformInput` と `FullscreenFitScaleLimits` に実効 ppp を持たせて配る
- **detached / frozen viewport は ppp が異なる。必ずその viewport の値を使う** (main の値を
  流用しない)

対象は正本 §5 の表にある 4 サイト (単ページ / 見開き / 見開き frozen / 連結読み)。
消費者側 (編集オーバーレイ等 40 箇所以上) は `DisplayedImageTransform` のメソッド経由なので
変更不要のはず。**もし変更が必要になったら、それは設計の見落としなので報告すること。**

### 段階2: ピクセルスナップ

**位置だけを丸め、サイズは丸めない。** サイズまで丸めると倍率が変わりアスペクトが狂う。

**実効の物理スケールが整数に十分近いときだけ適用する。** 非整数倍率ではスナップしても
意味がなく、ズーム中のカクつきの原因になるだけ。この条件により、ズーム / リサイズ中は
自然に無効化される。

- 単ページ系: `DisplayedImageTransform::resolve` / `from_resolved_rect` の末尾 1 箇所
  (全経路がここを通る)
- 見開き: `layout_spread_page_rects` を物理 px 単位で組み直す。
  **ページ間隔も `round(gap * ppp) / ppp` に量子化する** (利用者報告の条件2 の直接原因)
- 連結読み: ページ位置の累積を都度丸める (累積誤差を持ち越さない)
- クリップ矩形・hit rect・detached frozen の正規化矩形との整合を確認する

---

## 2. やらないこと (スコープ外)

- GPU Lanczos リサンプル (段階3・4)。別ブリーフで扱う
- 拡大側 (100% 超) の補間方式の変更
- PDF のラスタライズ密度 (`PdfDisplayFitMode::Original`) の意味変更。正本 §4.5 参照。
  ただし PDF ページの**描画矩形のスナップは対象に含む**
- `version_highlights.rs` / マニュアル / README 更新 (段階5 でまとめて行う)

---

## 3. 制約

- **detached viewer**: [detached-rework-plan.md](detached-rework-plan.md) の凍結ルールが有効。
  本作業は表示ジオメトリの構造的修正であり、detached frozen snapshot の
  `layout_spread_page_rects` 呼び出しに波及する。着手前にプラン §2 (憲法) を読み、
  **触れた範囲と判断理由を同プランへ記録すること**。症状パッチではなく構造的修正である
  という主張について、ClaudeCode が検収時に合意を確認する。
- 連結読みには `Original => zoom` という別の意味論の分岐がある (正本 §4.6)。物理 1:1 の定義と
  矛盾しないよう個別に整理すること。
- ピクセルグリッド機能は既に ppp 対応済み。整合を壊さないこと。
- `cargo fmt` (引数なし・ワークスペース全体) を必ず通す。pre-commit フックが番人。

---

## 4. テスト

正本 §6.1 の自動テストを実装する。最低限:

- `ppp = 1.25 / 1.5` で `Original` の `total_scale` が `1/ppp` になること
- 見開きのページ間隔が**奇数**でも、各ページの描画矩形の原点が物理 px 整数へ着地すること
- 単ページで画像幅と表示領域幅の**偶奇が食い違って**も、原点が物理 px 整数へ着地すること
- 連結読みで N ページ積み上げても累積誤差で原点が非整数にならないこと
- 既存 9 テスト (`displayed_image_transform.rs`) のうち倍率 1.0 前提のものを ppp 対応へ更新

実行:

```powershell
cargo test -p mimageviewer --lib displayed_image_transform
cargo test -p mimageviewer --lib
```

---

## 5. 実機検証用の素材 (ClaudeCode が用意済み)

`C:\tmp\miv-dpi-blur-test\` (バックアップ: `C:\home\mimageviewer_testdata_dpi\`)。
すべて完全 2 値 (純黒 / 純白) なので、**画面に灰色が見えたら表示側の補間で作られたもの**。

| ファイル | 用途 |
| --- | --- |
| `probe_800x1120` / `801x1120` / `800x1121` / `801x1121` .png | 幅・高さの偶奇違い 4 枚 |
| `spread_L.png` / `spread_R.png` | 見開きページ間隔の検証ペア |
| `manga_p01`〜`p04.png` | 白黒漫画ページ風 (網点・効果線・9〜18px の台詞) |

判定基準 (probe の帯 (1)(2) の 1px 縞):

- 黒白の線としてはっきり見える → 物理 1:1・整数位置 (正常)
- 一様な灰色に潰れている → 0.5px ずれている
- 数 px 周期でうねっている → 倍率が非整数

**最重要の検証条件**: 原寸表示でポストフィルタを「標準(補間あり)」⇔「ニアレスト(補間なし)」
に切り替えても**見た目が変わらない**こと。これが物理 1:1 + 整数位置が達成できた証拠になる。

実機確認は利用者が行う。検証用ビルドの作成と依頼は ClaudeCode 側で行うので、Codex は
アプリを起動しないこと。

---

## 6. 作業手順

1. ブランチ `feat/dot-by-dot-display` を切る (トレイ修正の `fix/tray-residency-cpu-spin` とは別)
2. 正本 §4.1 → §4.2 の順に実装する
3. テスト追加、`cargo fmt`、`cargo test -p mimageviewer --lib`
4. `docs/detached-rework-plan.md` へ触れた範囲と判断理由を記録する
5. `docs/display-pipeline.md` の表示変換の記述を実装に合わせて更新する
6. 完了したら ClaudeCode へ「変更内容・触れた範囲・テスト結果・設計上の判断」を報告する

## 7. 参照

- [dot-by-dot-and-downscale-plan.md](dot-by-dot-and-downscale-plan.md) — **正本**
- [display-pipeline.md](display-pipeline.md) — 表示テクスチャの優先順位と合成順序
- [detached-rework-plan.md](detached-rework-plan.md) — detached 凍結ルール
- `src/displayed_image_transform.rs` — 表示ジオメトリの唯一のオーナー
