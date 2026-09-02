# Codex Sol レビュー指摘の対応状況 (2026-09-02)

対象: 外部ツール連携 + 焼き込み段階の統一 (前半)。
ブランチ `context-menu-owner-hwnd`、**2026-09-02 に master へマージ済み**。

> マージ時点で、外部ツールの引数プレースホルダは **`{files}` 1 つだけ**になっている
> (利用者判断)。場所の値 (`{container}` / `{page}` 等) とファイル名の部品
> (`{stem}` / `{ext}` 等) は実装したうえで撤去した。理由は
> [external-tool-launch-plan.md](../external-tool-launch-plan.md) §4.4。
> このため #4 / #11 / #12 の一部は**対象そのものが無くなっている**。

指摘の原文は [review-findings-bake-stage-raw.md](review-findings-bake-stage-raw.md)。
依頼書は [review-bake-stage-and-external-tool.md](review-bake-stage-and-external-tool.md)。

## 対応済み

| # | 内容 | 対応 |
| --- | --- | --- |
| 1 | embedded fullscreen で起動確認 modal / ピッカーが描かれない | `0292abed` 描き手を `show_external_tool_modals` に一本化 |
| 6 | `page_requires_full_composite` が段を見ない | `0292abed` 判定に段と表示専用 params / AI モデルを渡す |
| 7 | `DisplayAdjust` の LUT がどの経路にも運ばれていない | `0292abed` 表示側と同じ resolver を通す |
| 2 | native video 経路で context menu と ACK に到達しない | `9096effe` **両方の早期 return を確認**。本体を `update_frame` へ分け、`eframe::App::update` を「飛ばせない tail」にした |
| 3 | frame 番号が要求元 viewport の所有権を持たない | `9096effe` modal の所有者を「利用者が操作した窓」にした。所有 viewport が消えた frame は main が肩代わり |
| 9 | supersede がツール横断 | `de95cfa5` **実装ではなく正本の記述が誤り**。準備中は入力が止まるので準備中の要求は常に高々 1 つ。プランを訂正 |
| 10 | PDF パスワードがパス単位でない | `de95cfa5` `pdf_open_password` に 1 か所化。開く経路と同じ順序 |
| 11 | `{page}` が items の位置由来 | `de95cfa5` 読書履歴と同じ数え方に。ページでない項目が混ざる一覧では付けない |
| 12 | `VideoFrame` が音声も通す | `b25cd718` `has_video` を判定に追加。SAR / source 差し替えは出荷済みの Ctrl+S と共有の制限としてプランへ記録 |
| 4 (前半) | `TempOriginal` + `Merged` が「元バイト列」を約束しながら PNG を出す | `b25cd718` `effective_spread` に規則を 1 つ置いた |

## 未対応

| # | 内容 | 状況 |
| --- | --- | --- |
| 4 (後半) | `Merged` に焼き込み段が効かない | **段取り 6 で解消**。合成をワーカー経路へ移すときに一緒に直す。それまでは設定画面に明記 |
| 8 | Merged の合成と全画素 hash が UI スレッド | **段取り 6 で解消** (同上)。hash は pixel ベースのままにする — 構造的な鍵は #13 の課題を丸ごと抱えるため |
| 5 | バッチ Ctrl+E が既に `DisplayAdjust` へ配線済み | **私の報告が誤りだった。挙動変更は利用者の意図どおり**なのでこのまま (master へは混ぜない) |
| 13 | 指紋に AI の feature mode / 実モデル / ロード状態が無い | **段取り 4 と同時**。AI を配線した瞬間に効く |
| 14 | stack 経路だけ `comic_source_dims` が `None` | **段取り 4 と同時**。AI を配線すると注釈位置が通常ページと分岐する |

## 進め方

1. 段取り 4 (AI のモデル決定を表示側から切り出す) — #13 / #14 をここで同時に
2. 段取り 5 / 6 — #4 後半 と #8 をここで同時に
3. 段取り 7 (マニュアル / 製品ページ)

## 段取りの現況

[bake-stage-unification-plan.md](../bake-stage-unification-plan.md) の §5 を参照。
1〜3 済み、4 (AI のモデル決定を表示側から切り出す) が残り全部の前提。
