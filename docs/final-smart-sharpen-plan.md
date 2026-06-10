# 最終段スマートシャープ 実装プラン

> **ステータス: 実装済み (2026-06-10、v1.3.0 予定)**。実装の正本は
> [preset-and-adjustment.md §2.6](preset-and-adjustment.md) を参照。
> 本書は設計経緯メモとして残す。

## 目的

画像補正パネルに、閲覧時の最終表示だけへ適用する「シャープ化」スライダーを追加する。
左パネルは既に項目が多いため、初期実装では詳細パラメータを前面に出さず、
利用者には 1 本のスライダーとして見せる。

この機能は AI モデルによる復元ではなく、既存の補正レイヤーにある
`SmartSharpen` 系の計算式シャープを final pipeline に追加するもの。
5ch 等で説明するときは「AI シャープ化」ではなく「シャープ化調整」または
「スマートシャープ」と呼ぶのが安全。

## 方針

- UI はまず `シャープ化: 0..100` の 1 本にする。
- 内部処理はスマートシャープのみ。標準アンシャープマスクとの選択 UI は出さない。
- サムネイルには反映しない。フルスクリーン最終表示、コピー、書き出し側の final pixels にだけ反映する。
- 処理順は `色調補正 -> final AI -> シャープ化 -> post_filter -> GPU upload` とする。
- GPU 実装は初期スコープ外。CPU + rayon 並列化で実用速度を狙う。
- 8192px 超の元画像は既存表示パイプラインと同じく、表示用に 8192px 以内へ縮小された後の画像に対して処理する。

## 背景メモ

一般的なシャープ化の代表はアンシャープマスクで、元画像からぼかし画像を引いた差分を
ディテールとして足し戻す。実装は単純だが、ノイズ、JPEG 圧縮荒れ、紙目、トーンのザラつき、
白黒のフチ (ハロー) も強調しやすい。

スマートシャープは、基本はアンシャープ系だが、エッジ強度を見て輪郭中心に効かせ、
平坦部や弱いザラつきに効きにくくする。mIV の既存実装ではさらに `halo_suppression` で
明暗境界のフチ浮きを抑える。

利用者向けには、細かい半径やしきい値よりも「少しクッキリ」「強めにクッキリ」を
調整できるほうが分かりやすい。詳細調整は将来要望が出てから、折りたたみ UI または
上級者向け設定として検討する。

## 既存実装

- `crates/local-adjust-core/src/lib.rs`
  - `SharpenParams`: `amount`, `radius_px`, `threshold`
  - `SmartSharpenParams`: `amount`, `radius_px`, `edge_threshold`, `halo_suppression`
  - `apply_smart_sharpen(...)`: 既存のスマートシャープ本体
  - `box_blur_rgba(...)`: スマートシャープ前段のぼかし
- `src/post_filter.rs`
  - `PostFilter::Sharpen`: 固定値のアンシャープマスク。現在は post_filter の一種なので、
    疑似カラーや CRT などとは排他的。
- `src/app.rs`
  - `ensure_final_composite_texture(...)`: final pipeline の中心。
  - `hash_adjust_final_params(...)`: final composite cache key に含める必要がある。
- `src/adjustment.rs`
  - `AdjustParams`: ページ個別 / お気に入り / グローバル設定に保存するフィールドを追加する場所。
- `docs/preset-and-adjustment.md`
  - final pipeline とサムネイル非反映の仕様を更新する。

## UI 案

画像補正パネルのポストフィルタ UI の上あたりに追加する。

```text
シャープ化  [----------|------] 60
```

表示は 0..100 の整数でよい。

- `0`: OFF
- `30`: 弱め
- `60`: 標準
- `100`: 強め

初期実装では `半径`, `輪郭しきい値`, `ハロー抑制` は表示しない。
必要になった場合だけ、後で「詳細設定」を折りたたみで追加する。

## 内部パラメータ案

1 本スライダーから `SmartSharpenParams` を生成する。
最初は固定プリセット間を線形補間するのが扱いやすい。

```text
0:
  disabled

30:
  amount 0.40
  radius_px 0.80
  edge_threshold 0.06
  halo_suppression 0.45

60:
  amount 0.80
  radius_px 1.20
  edge_threshold 0.08
  halo_suppression 0.60

100:
  amount 1.25
  radius_px 1.60
  edge_threshold 0.11
  halo_suppression 0.78
```

スライダー値は保存互換を考えて `u8` または `f32` で `AdjustParams` に持つ。
UI 表示が 0..100 なので、内部も `u8` のほうが差分が小さい。

候補フィールド:

```rust
#[serde(default)]
pub smart_sharpen: u8, // 0 = off, 1..=100 = strength
```

`AdjustParams::default()` は `0`。
`is_identity()` / `is_removable()` / cache hash では `smart_sharpen == 0` を無補正扱いにする。

## 実装手順

1. `AdjustParams` に `smart_sharpen: u8` を追加する。
2. `hash_adjust_final_params(...)` に `smart_sharpen` を含める。
3. `AdjustParams::is_identity()` / `is_removable()` の判定へ反映する。
4. final pipeline に `apply_final_smart_sharpen(...)` を追加する。
5. 処理順を `adjust -> final AI -> smart sharpen -> post_filter` にする。
6. 画像補正パネルに `シャープ化` スライダーを追加する。
7. スライダー変更時は final pipeline cache を無効化し、サムネイル補正 cache は触らない。
8. `docs/preset-and-adjustment.md` と `docs/spec.md` を更新する。
9. 必要なら Web マニュアル / release note に「シャープ化調整」を追記する。

## CPU 高速化

既存の `apply_smart_sharpen(...)` は本体ループが逐次処理なので、final pipeline 用に
使う前に CPU 並列化する。

優先度:

1. 出力合成ループを `rayon` の `par_chunks_exact_mut(4)` または行単位で並列化する。
2. `box_blur_rgba(...)` の水平 / 垂直パスを行単位で並列化する。
3. 半径を初期値 0.8..1.6 程度に抑え、巨大半径を使わない。
4. 必要なら `radius_px` は内部で最大 2.0 か 3.0 に clamp する。

GPU 化は初期スコープ外。
wgpu compute / shader 化は理屈上可能だが、CPU final pixels、書き出し、比較表示、
GPU 非対応環境フォールバックとの整合が増えるため、別プロジェクトとして扱う。

## キャッシュとサムネイル

サムネイルは現状通り色調補正のみを反映する。
`thumb_adjust_tex` にはスマートシャープを反映しない。

final composite cache にはスマートシャープ後の pixels を入れる。
そのため `FinalCompositeKey` の `params_hash` へ `smart_sharpen` を含める必要がある。

AI 未完了中は、色調補正済み画像にスマートシャープを掛けた暫定表示になる。
AI 完了後は final composite を作り直し、AI 後の画像にスマートシャープを掛ける。

## テスト観点

- `smart_sharpen == 0` は完全に従来表示と同じ。
- `smart_sharpen` 変更で final composite cache が切り替わる。
- `smart_sharpen` 変更で `thumb_adjust_tex` は不要に再生成されない。
- post_filter と併用できる。疑似カラー / CRT / Nearest と排他にならない。
- AI アップスケール / AI ノイズ除去後にも適用される。
- 連結読み / 見開きでも左右ページそれぞれに適用される。
- 透明ピクセルやアルファ境界で hidden RGB が漏れない。
- 4K, 8K 相当で UI スレッドを長く止めない。

## リリース文言案

```text
画像補正パネルにシャープ化調整を追加しました。
最終表示段で輪郭を自然に強調するスマートシャープ方式で、疑似カラーやポストフィルタとも併用できます。
```

AI モデルを使う機能ではないため、「AI シャープ化」とは書かない。

## 後回しにするもの

- 標準アンシャープ / スマートシャープの方式選択 UI
- 半径 / 輪郭しきい値 / ハロー抑制の個別 UI
- GPU compute / shader 実装
- サムネイルへの反映
- 8192px 超の元画像に対する原寸処理
