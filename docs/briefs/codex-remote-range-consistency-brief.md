# リモート閲覧: スライダー操作の統一 / 効かない音量の検出

作業ツリー: `C:\home\mimageviewer-web` (branch `web-remote`)。
実装 = Codex、レビュー・テスト・統合 = ClaudeCode、実機確認 = 利用者。

## 0. 前提 — 規則は既にある。作り直さないこと

**「タップ = その位置へ絶対移動 / ドラッグ = 相対移動」が現行の規則**であり、既に 2 か所で
実装されている。

| 部品 | 実装 |
| --- | --- |
| 動画・音声のシークバー | [video-stream.mjs](../crates/remote-web/web/video-stream.mjs) `finishSeekPointerDrag` |
| 静止画のページバー | [app.js](../crates/remote-web/web/app.js) の seek pointer 終了処理 |

共通部品も [command-core.mjs](../crates/remote-web/web/command-core.mjs) にある。

- `seekRangePointerGestureDecision` — tap / drag / cancel の判定
- `seekRangeAbsoluteValue` — 押下位置から絶対値
- `relativeRangeDragValue` — 開始値からの相対移動

**新しい判定や計算を書かないこと。** 上の 3 つを使う。

なお計画書 §12.9 の「pointer は押下位置の絶対値を採らず」は**この規則より古い記述**である
(§4 で直す)。実装が正しい。

## 1. 音量をこの規則へ揃える

直前の増分で音量をドラッグ相対にしたが、**タップの絶対移動が入っていない**。シークバーと
同じく `seekRangePointerGestureDecision` で tap / drag を判定し、tap なら
`seekRangeAbsoluteValue` を使う。

## 2. 画像補正・表示トリムのスライダーをこの規則へ揃える

利用者報告: 「画像補正のバーはクリックでは動かない」。

素の `<input type=range>` で、pointer 処理を持っていない。§12.20 の「mouse / pen の native
range 操作を抑える既存処理」により、クリックしても動かない状態になっている。

### 2.1 §12.20 を壊さないこと (先に読むこと)

補正パネルは**縦スクロールする文脈**なので、次が意図的に維持されている。**変えないこと。**

- range は `touch-action: pan-y`。画像・動画の seek range (`touch-action: none`) とは違う
- **touch の pointer event で `preventDefault()` して viewport パンを所有しようとしない**
- ブラウザが縦パンを選んだ場合 (`pointercancel` / `touchcancel`) は確定終了として扱わず、
  pointer 開始時の値へ戻す。preview 発行済みなら開始値の preview を再投入し、永続書込みはしない

### 2.2 やること

tap / drag の確定は**pointerup 側**で行う。これは §12.20 と両立する。

- ブラウザが縦パンを取ったら `pointercancel` が来る → **既存の「開始値へ戻す」処理がそのまま働く**
- 縦パンにならなかった場合だけ tap / drag として確定する

つまり、`touch-action` も `preventDefault` の方針も変えずに、終了時の分岐だけを足せばよい。
**変えなくてよいものを変えないこと。**

## 3. 効かない音量スライダーが iPad で消えない

### 3.1 観測

利用者が切り分け済み。**iPad で消えない**。PC Chrome では音量が効く。

### 3.2 原因の見立て

検出 (`mediaElementVolumeControlSupported`) を、`<video>` を作った直後・**メディアソースを
繋ぐ前**に実行している。iOS / iPadOS の `volume` 制約はメディアエンジンが結び付いてから
効くため、ソース未接続の要素では代入が通り「対応」と誤判定していると考えられる。

### 3.3 やること

- 検出を**実際に制約が効く状態**へ移す (`loadedmetadata` 以降など)。どの時点が正しいかは
  調べて決め、根拠をコメントに残す
- **判定結果をテレメトリへ残す。** 今回のように「効くはずの検出が効かない」ときに、
  ログで確認できるようにする。推測での往復を繰り返さないため
- 検出前は音量スライダーを出さない。判定後に対応端末でだけ出す
  (出してから消すと操作中に消える)
- UA 判定は使わない
- 効く端末で聞こえる音量変化を起こさないこと

## 4. ドキュメント

計画書 §12.9 の「range は keyboard / ARIA の owner として残す一方、pointer は押下位置の絶対値を
採らず、押下時の group index からの相対移動を使う」は実装より古い。

**現行の規則 (タップ = 絶対 / ドラッグ = 相対、判定は pointerup) へ書き換える。**
古い記述を残さず置き換えること。どの部品がこの規則に従うかも列挙する
(動画・音声シーク、静止画ページバー、音量、補正・トリム)。

## 5. やってはいけないこと

- tap / drag の判定や絶対値・相対値の計算を新しく書くこと (§0 の 3 つを使う)
- 補正スライダーの `touch-action: pan-y` を変えること
- 補正スライダーの touch で `preventDefault()` すること (§12.20)
- `pointercancel` を確定終了として扱うこと
- UA 文字列で音量の可否を判定すること
- 音量スライダーを出してから消すこと

## 6. テスト

- 音量: tap で絶対、drag で相対
- 補正・トリム: tap で絶対、drag で相対
- 補正・トリム: `pointercancel` で開始値へ戻る (§12.20 の回帰)
- シークバーとページバーが従来どおり動くこと (回帰)
- 音量の実効性判定が、対応・非対応それぞれで正しい表示になること

## 7. 確認と報告

- `cargo test -p mimageviewer --lib` 全件、`crates/remote-ipc` / `crates/remote-web`、
  web テスト一式
- `cargo fmt --all -- --check`、`python scripts/check_ui_glyphs.py`、`git diff --check`
- `cargo check` の警告が増えていないこと
- **§3.3 で検出をどの時点へ移したか、その根拠を報告に含める**
- ビルドとコミットは行わない。`htdocs/` は触らない
