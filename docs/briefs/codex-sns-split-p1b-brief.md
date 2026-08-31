# Codex ブリーフ — SNS 分割書き出し P1b (P1 レビュー指摘の修正)

P1 の実装をレビューしました。**19 テストは全て緑、`frames()` の同寸保証も要件どおり**です。
`sns_split.rs` を新規に切ったこと自体、既存の切り取りへ影響を出さない形になっていて良いです。

そのうえで**修正してほしい点が 2 つ**あります。どちらも `src/sns_split.rs` の中で完結します。
このブリーフの範囲もそこだけで、他のファイルは触らないでください。

---

## F1. `frames()` が寸法ゼロの枠を返す (再現済み)

**これは実際に再現しました。**次の layout で `frames()` を呼ぶと、4 枚とも同じ点に潰れた
ゼロ寸法の矩形が返ります。

```rust
SnsSplitLayout {
    target: SnsTarget::X,
    count: 4,
    group: CropRect { min_x: 10.0, min_y: 10.0, max_x: 13.0, max_y: 11.0 },
}
```

```
frame 0: w=0 h=0 rect=CropRect { min_x: 10.0, min_y: 10.0, max_x: 10.0, max_y: 10.0 }
frame 1: w=0 h=0 rect=CropRect { min_x: 10.0, min_y: 10.0, max_x: 10.0, max_y: 10.0 }
frame 2: w=0 h=0 rect=CropRect { min_x: 10.0, min_y: 10.0, max_x: 10.0, max_y: 10.0 }
frame 3: w=0 h=0 rect=CropRect { min_x: 10.0, min_y: 10.0, max_x: 10.0, max_y: 10.0 }
```

原因は `frame_metrics` の `let width = (group_width / divisor).floor();` に下限が無いこと。
`group_width` が `divisor` (X/4 なら 4.051) より小さいと 0 になり、`height` も `step` も 0 になります。

さらに `clamped()` はこの退化した layout を**そのまま通します**。外接矩形がゼロ寸法なので
「画像に収まっている」と判定されてしまうためです。

```
clamped group=CropRect { min_x: 9.98, min_y: 10.0, max_x: 13.02, max_y: 11.0 }
        extent=CropRect { min_x: 10.0, min_y: 10.0, max_x: 10.0, max_y: 10.0 }
```

### なぜ P1 で直すか

`SnsSplitLayout` のフィールドは `pub` で、`clamped()` も公開 API です。P2 の UI は
グループ矩形をドラッグで縮められるので、**この状態は必ず通ります。**ここで下限を持たないと、
P2 が「枠が潰れていないか」を自分で見張る責務を負うことになり、同じ不変条件の持ち主が
2 箇所に増えます。幾何の不変条件は幾何のモジュールが持ってください。

### 直し方

- `frames()` が返す枠は、**幅・高さがともに 1px 以上**であることを常に保証する
- `step >= width` も保証する (枠が重ならない)
- 「画像が小さすぎて N 枚が物理的に入らない」ケース (画像幅が数 px など) が
  **初めて実在するようになる**ので、答えを決めて明文化してください。次の形を推奨します。
  - `clamped()` は**常に非退化な layout を返す** (枠が 1px 未満にならない)
  - その代わり、画像に収まりきらない場合は**はみ出したまま返してよい**
  - 収まっているかどうかは `pub fn fits(&self, image_size: [usize; 2]) -> bool` で別途返す
    (P2 の UI がこれを見てツールを無効化する)
  - この振る舞いを doc comment に書く

---

## F2. `clamped()` の反復探索と、収束しなかったときの黙った失敗

現在の `clamped()` は 16 回のループで、

- 収まらなければ `retry_scale` で等比縮小
- `retry_scale >= 1.0` なのに収まらない場合は `progress_scale` で **1px 分または半分**に縮める
- 16 回回っても収まらなければ `last` を **そのまま返す** (`rect_inside` を満たさないかもしれない)

という構造になっています。

これは CLAUDE.md「バグ修正の一般原則」が名指しで禁じている形です。

> 症状を消す guard、delay、retry、追加 reset、silent fallback を根本原因の代わりに追加しない。

`progress_scale` の分岐は「なぜ収まらないのか分からないので少し縮めてもう一度試す」であり、
16 回目の `return last` は**テストできない silent fallback** です。
実際、F1 の下限を入れると「どう縮めても入らない」状態が実在するようになるので、この経路は
到達可能になります。

### 求める構造

**整数バジェットから決定的に構成してください。**「入るかどうか」を反復で確かめるのではなく、
構成の時点で入るようにします。おおよそ次の形を想定しています (細部は任せます)。

```
avail_w = floor(image_width)   avail_h = floor(image_height)
w_px    = floor(min(group.width(), avail_w) / divisor)     // 上限から出発
loop {                                                      // w_px について単調減少・有界
    h_px    = round(w_px / frame_aspect)
    step    = round(w_px * (1 + seam_ratio))
    total_w = (N-1) * step + w_px
    if (total_w <= avail_w && h_px <= avail_h) || w_px <= 1 { break }
    w_px -= 1
}
x0 = clamp(round(group.min_x), 0, avail_w - total_w)
y0 = clamp(round(group.min_y), 0, avail_h - h_px)
```

- ループは `w_px` について**単調減少で有界**なので、必ず止まることが自明です
- 各反復が「1px 小さくして測り直す」という意味のある操作で、当てずっぽうの倍率がありません
- 「収まらない」は `w_px <= 1` で **1 箇所に集約**され、`fits()` で外へ返せます
- `frames_extent()` と `group` の食い違いを気にする必要が無くなります

**グループ矩形を `frames_extent()` にスナップしてしまう**設計 (= `group == extent` を
構成で保証する) にしても構いません。その場合グループの比率は整数丸めの分だけ厳密値から
ずれますが、グループは利用者が掴むハンドルなので実害はありません。既存テストの
`assert_layout_aspect` は許容幅を持たせて調整してください。**どちらを選んだかは報告してください。**

---

## 守ること

- 変更するのは `src/sns_split.rs` のみ。他のファイルは触らない
- **既存 19 テストは緑のまま**にする。仕様変更で意味が変わるテストがあれば、
  消さずに新しい期待値へ書き換え、その理由を報告する
- 追加するテスト:
  - F1 の再現 layout (上記) で全枠が 1px 以上であること
  - グループを極小にした様々な (target, count) で退化しないこと
  - 画像が小さすぎるケース (例 `[3, 3]`) で `fits()` が false を返し、かつ枠が退化しないこと
  - `clamped()` が収まる場合には必ず `fits()` が true になること
  - 反復が止まることを示すテスト (極端な入力で panic / 無限ループしない)
- `cargo fmt` をかけてから終わる
- `cargo test -p mimageviewer --lib sns_split` と
  `cargo check -p mimageviewer --bin mimageviewer-core` が緑であること
- コミットしない
- 正本 (docs/sns-split-export-plan.md) を書き換えない。仕様の変更が要ると思ったら報告する

## 報告してほしいこと

- F1 をどう保証したか (下限をどこに置いたか)
- F2 で採った構造と、`fits()` の意味づけ
- グループを extent にスナップしたかどうか
- 既存テストで期待値を変えたものがあれば、その一覧と理由
- テスト結果
