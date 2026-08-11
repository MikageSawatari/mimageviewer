# ブリーフ: 比較シェーダの uniform を 1 フレームで奪い合っている (§1.60 の真因)

対象: v2.13.0。実装 = Codex Sol / レビュー・検収 = ClaudeCode / 実機確認 = 利用者。

正本: [docs/next-release-backlog.md](next-release-backlog.md) §1.60 / §1.55-2。

前提: master の作業ツリー。`[compare-geometry]` の一時計測が入っている状態から始める。

---

## 1. 原因は確定した (実機ログ + ソースで裏取り済み)

### 1.1 実機ログが示したこと

同じフレームの 2 行 (`pair_key=4` が同一 = 同じ GPU ペアを共有):

```
caller=main      ... draw_rect=(-451.3,-927.0)-(724.6,836.9) visible=(0,0)-(724.6,724.7)
                     uv_window=(0.3838,0.5255)-(1.0000,0.9364) callback_rect=(0,0)-(724.6,724.7)
caller=navigator ... draw_rect=(912.7,452.7)-(1086.0,712.7)
                     uv_window=(0.0000,0.0000)-(1.0000,1.0000) callback_rect=(912.7,452.7)-(1086.0,712.7)
```

**`draw_rect` も `visible` も `uv_window` も正しい**。`draw_rect` の縦横比は 1175.9 : 1763.9 =
2 : 3 で `target=4096x6144` と一致し、uv 窓の幅 0.6162 は `visible.width / draw_rect.width`
= 724.571 / 1175.906 と一致する。**CPU 側の幾何計算に誤りはない。**

### 1.2 ソースが示したこと

- `CompareGpuResources` は `pair: Option<(key, CompareGpuPair)>` を **1 つだけ**持ち、
  `CompareGpuPair` の `uniform` バッファと `bind_group` も **1 つだけ**
  ([src/compare_wgpu.rs](../src/compare_wgpu.rs))
- `prepare` は `key` が一致する限り**毎回同じ uniform バッファへ上書き**する
- egui-wgpu は **全 callback の `prepare` を回してから、全 callback の `paint` を回す**
  ([vendor/egui-wgpu/src/renderer.rs:1057](../vendor/egui-wgpu/src/renderer.rs) の
  "prepare callbacks" ループと、後段の paint ループ)

### 1.3 したがって

§1.55-2 でナビゲータの比較描画を足したことで、**1 フレームに比較 callback が 2 つ**になった。
2 つは同じ `pair.key` を持つので同じ uniform を共有し、**後に prepare されたナビゲータの
`uv_window=(0,0)-(1,1)` が、本文用に書いた uv 窓を上書きする**。paint はその後に走るので、
**本文も (0,0)-(1,1) で描く** = 合成画像の全体を、クリップ済みの callback 矩形へ引き伸ばす。

これが「ズームしているのに全体が写る」「縦横比が崩れる」「パンで帯に潰れる」の正体である。
**`096a4bad` の uv 窓の実装は正しく、上書きされて効いていなかっただけ。**

**これは §1.55-2 が入れた退行**である (それ以前は比較 callback がフレームに 1 つだった)。

---

## 2. 直し方

**uniform と bind group を呼び出し箇所ごとに分ける。テクスチャと mip は共有のまま。**

- `CompareShaderCallback` に呼び出し箇所を表す typed な slot を足す (本文 / ナビゲータ)。
  bool や `Option` ではなく enum にすること
- `CompareGpuPair` は **slot ごとの uniform バッファと bind group** を持つ。
  重い `pinned` / `current` テクスチャと mip chain は **`pair.key` で従来どおり共有**する
  (ここを slot ごとに複製しない。数十 MB × 2 の無駄になる)
- `prepare` は自分の slot の uniform だけ書き、`paint` は自分の slot の bind group を使う
- 将来 3 つ目の呼び出し箇所が増えたときに**同じ壊れ方をしない**構造にすること。
  slot を追加し忘れたら**コンパイルが通らない**形が望ましい

### やらないこと

- 描画順に依存して「ナビゲータを先に描く」等で回避すること
- ナビゲータの比較描画をやめること
- テクスチャを slot ごとに複製すること
- `096a4bad` の uv 窓の計算を変えること (ログで正しいと確認済み)

## 3. テスト

GPU を要する部分は unit test で直接触れないので、**何を固定できて何ができなかったかを
報告すること**。最低限:

- slot ごとに別の uniform バイト列が作られること (`uniform_bytes` は純関数)
- 本文とナビゲータが**異なる slot** を渡していること (呼び出し側の wiring を型か純関数で固定)
- 既存の `uniform_bytes` テストを slot 込みへ更新

## 4. 一時計測の扱い

`[compare-geometry]` のログは **残すこと**。利用者の実機で直ったことを確認してから、
次のラウンドで撤去する。

## 5. 完了条件

- `cargo fmt` (引数なし)
- `cargo test -p mimageviewer --lib` が全件 / `cargo test -p mimageviewer --test ui_snapshot`
- `cargo check -p mimageviewer --bin mimageviewer-core`
- `python scripts/check_ui_glyphs.py` が 0 件
- 非 Windows を壊さないこと
- **バックログ §1.60 と §1.55-2 に、この真因と「1 フレームに複数 callback を出すときは
  per-callback の GPU 状態が要る」という一般則を記録**する

## 6. 制約

- **アプリを起動しないこと。** 検証ビルドと実機確認は ClaudeCode と利用者が行う
- **ブランチ操作・コミットをしないこと。** master の作業ツリーで未コミットのまま残す

---

完了したら次を報告すること:

1. slot をどう表現したか、3 つ目が増えたときに壊れない根拠
2. テクスチャを共有したままである根拠
3. テストで固定できたこと / できなかったこと
4. **実機で確認してほしいこと**
