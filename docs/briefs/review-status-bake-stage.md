# Codex Sol レビュー指摘の対応状況 (2026-09-02)

対象: 外部ツール連携 + 焼き込み段階の統一 (前半)。
ブランチ `context-menu-owner-hwnd`、**master へは未マージ**。

指摘の原文は [review-findings-bake-stage-raw.md](review-findings-bake-stage-raw.md)。
依頼書は [review-bake-stage-and-external-tool.md](review-bake-stage-and-external-tool.md)。

## 対応済み (`0292abed`)

| # | 内容 | 判定 |
| --- | --- | --- |
| 1 | embedded fullscreen で起動確認 modal / ピッカーが描かれない | **本物。私の見落とし。** 3 つのうち進捗だけ複製していた。描き手を `show_external_tool_modals` に一本化し、他所から個別に呼んでいないかをソース走査テストで固定 |
| 6 | `page_requires_full_composite` が段を見ず、表示専用効果だけのページで合成が飛ぶ | **本物。最重要。** 判定に段を渡し、表示専用 params と AI モデルを直接見る |
| 7 | `DisplayAdjust` の LUT がどの経路にも運ばれていない | **本物。** 製本 / バッチ / 外部ツールで、表示側と同じ resolver を通して解決 |

## 未対応

| # | 内容 | 確認状況 |
| --- | --- | --- |
| 2 | native video 経路で context menu と ACK に到達しない | **fs body 側は確認済み** — [ui_fullscreen.rs:14353](../../src/ui_fullscreen.rs) の `return` が backdrop 分岐にある。**root 側 (app.rs の early return) は未確認**。両方飛ぶなら、動画からキーで外部ツールを起動すると進捗も出ず永久待ちになる |
| 3 | frame 番号が要求元 viewport の所有権を持たない (背面 F12 窓が main 由来の modal を先取りし得る) | 未確認 |
| 4 | `SpreadPolicy::Merged` が `PayloadPolicy` と `BakeStage` を迂回 (表示最終画素を先に合成するため「編集前」でも表示補正込みの PNG が出る) | 未確認。**設計判断が要る** — Merged を段の下へ入れるか、UI で組み合わせを禁じるか |
| 5 | バッチ Ctrl+E が既に `DisplayAdjust` へ配線済み | **私の報告が誤りだった (配線済み)。挙動変更は利用者の意図どおり**なので、コードはこのままでよい。ただし AI 未配線のため現状は「表示補正は焼くが AI は焼かない」中間状態。利用者判断でこのまま進める (master へは混ぜない) |
| 8 | Merged の合成と全画素 hash が UI スレッド | 未確認 |
| 9 | supersede がツール横断 (別ツールの準備中要求まで cancel) | 未確認。設計書 §4.7 は同一ツールのみ置換と書いている |
| 10 | PDF materialization がパス単位のパスワードを見ず `pdf_current_password` 固定 | 未確認 |
| 11 | `{page}` が論理ページ列でなく items の位置由来 | 未確認 |
| 12 | `VideoFrame(path, millis)` が提示済みフレームを一意に表さない (音声との潰し / source swap / SAR 未反映) | 未確認 |
| 13 | 指紋に AI の feature mode / 実モデル / ロード状態が無い (段取り 4 で顕在化) | 未確認 |
| 14 | stack 経路だけ `comic_source_dims` が `None` で、AI 配線後に注釈位置が分岐 (段取り 4 で顕在化) | 未確認 |

## 進め方

1. #2 の root 側を確認 → 対応
2. #4 は設計判断を利用者に確認してから
3. #3 / #8 / #9〜#12 を順に
4. #13 / #14 は段取り 4 (AI のモデル決定の切り出し) と同時に

## 段取りの現況

[bake-stage-unification-plan.md](../bake-stage-unification-plan.md) の §5 を参照。
1〜3 済み、4 (AI のモデル決定を表示側から切り出す) が残り全部の前提。
