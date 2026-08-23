# セッション指示書: レーン C 先行 — 360 度ビューの投影モード追加 (静止画側)

体制: 単独で完結する小粒。レーン B と**並行してよい** (触るファイルが重ならない)。
レーン構成の正本は [next-cycle-work-lanes.md](../next-cycle-work-lanes.md)。

## 0. 先に読む

1. [next-release-backlog.md](../next-release-backlog.md) **§1.59** — 提案の経緯と、
   **2026-08-13 に先送り判断が出ている**こと
2. [panorama-360-view-plan.md](../panorama-360-view-plan.md) — 現行仕様の正本
3. [next-release-backlog.md](../next-release-backlog.md) **§1.112** — 360 度動画。
   **ここで確定させた数式をあとで HLSL へ移す**

## 1. 最初にユーザー判断を取る

§1.59 は**先送り中**であり、却下ではない。着手前に「今やるか」を確認すること。
やる根拠は、360 度動画 (§1.112) の前提だった §1.47 が **v3.2.0 で出荷済み**になり、
**投影を差し込む場所ができた**こと。静止画側で投影モードを一般化しておくと、
動画へ移す数式が確定する。

## 2. 目的

`panorama_wgpu.rs` の透視投影固定を、**投影モードを選べる形**に一般化する。

現状は `tan_half = tan(fov_y * 0.5)` でカメラ方向を作るため、**原理的に 180 度へ近づくと発散**し、
「引いた画角」そのものを表現できない。

候補 (半径 `r` と入射角 `θ` の対応。**シェーダ上はどれも 1 行の差**):

| 方式 | 対応 | 見え方 |
| --- | --- | --- |
| 透視 (現行) | `r = f·tan θ` | 既定。180 度へ近づくと発散 |
| 立体射影 | `r = 2f·tan(θ/2)` | 周辺の伸びが最も穏やか。いわゆるリトルプラネット |
| 等距離 | `r = f·θ` | レンズの物理仕様としての標準表記。中央がやや膨らむ |
| 等立体角 | `r = 2f·sin(θ/2)` | 参考 |

**利用者の要望「引いたときに 360 度カメラっぽい絵」は立体射影の可能性が高い** (§1.59 の調査)。
どれか一方に決めず、**方式を選ぶ形にするのが素直** (実装コストがほぼ同じため)。

## 3. 作業の本体はシェーダではない

シェーダ側は数行。実際の作業は:

- 投影モードの uniform 追加
- 設定への永続化 (`settings.rs`)
- 切り替え UI / キー割り当て (`KeyAction` + keymap helper 経由。CLAUDE.md の keymap 方針)
- **画角スライダの意味と上限が方式ごとに変わる**ことの再定義 (魚眼なら 180 度超も扱える)
- 既定は**透視のまま**にするか、切り替えをどこに置くか (パノラマ設定か HUD か) を決める
- ドキュメント: [panorama-360-view-plan.md](../panorama-360-view-plan.md) と
  `htdocs/mimageviewer/manual/`

## 4. 動画への移植を見据える

§1.112 では同じ投影を **presenter 側の実行時 HLSL** (`grade_pipeline.rs` の `D3DCompile` と
`resample_pipeline.rs`) へ移す。**WGSL 側の数式を、そのまま移せる形** (分岐の位置、uniform の
持ち方、seam 処理) にしておくこと。

⚠ 静止画側は seam をまたぐ U 勾配を wrap した `textureSampleGrad` を使っている。
**投影を変えても seam の扱いを壊さないこと** (v2.7.0 で入った回帰テスト
`panorama_shader_uses_wrapped_explicit_gradients` を通す)。

## 5. スコープ外

- **動画側 (presenter) には触らない。** レーン B と衝突する。
- パノラマの検出・lifecycle・settle 方針は変えない。

## 6. 触るファイル

`src/panorama.rs` / `src/panorama_wgpu.rs` / `src/settings.rs` / `src/keymap.rs` +
`docs/panorama-360-view-plan.md` / `docs/keymap-spec.md` / `docs/keymap.ini.default` /
`htdocs/mimageviewer/manual/`。**A とも B とも重ならない。**
