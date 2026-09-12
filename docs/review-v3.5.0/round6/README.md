# v3.5.0 再レビュー (第6回)

開始: 2026-09-03。基点 `7127fd14f` → `6cd99d915`。
2 commits / 8 files / +428 -15。実動作の変更は `af6fe006d` の picker 所有者の保持と確定時 mount。
他は重複コメントとマニュアルページ数の修正。

**再レビュー完了。S01の初回確定先は改善、残件2件 (P1: T01 / P2: T02)**。
本体 lib は 7,261 passed / 0 failed / 30 ignored。
UI snapshotの並列実行が停滞し、全体15分経過後に当該processを停止して逐次再実行した。
同じ43件は成功。残りのworkspaceとvendored検証も完了し、合計8,185 passed / 0 failed / 36 ignored。
**通常の並列ゲート完走PASSとは区別する**。停滞原因は未確定。
最終HEAD: `6cd99d91596189f82a4601ba85d89209d715ced7` (開始時と同じ)。
アプリコードは編集せず、既存の未コミット `docs/duplicate-detection-plan.md` とレビュー履歴を保持。

| 確認対象 | 状況 |
|---|---|
| S01: 所有 context 上での設定確定 | 初回確定は解消。Undo / Redoはindexのまま別窓へ適用される → T01 |
| 全 dirty row と寿命 | 画像・動画・一覧のapply、所有窓消失、ページ変更を確認。native動画の表示解除がownerを通らない → T02 |
| nested mount と一覧の再構築 | 実装を照合、実フォルダのreloadテストPASS |
| 自動検証 | 新しいownerテスト8件PASS、Undo6ケース / native表示2ケースの抽出probe、fmt PASS。UI逐次再実行43件も成功し、全targetの確認完了 |
| 延期承認済み R07 / R08 / R09 / R10 / R11 | 判断を維持。新たな延期を推定しない |

指摘は [findings.md](findings.md)、検証の範囲と結果は [verification.md](verification.md)。
ownerを要求に保持する方向は構造的に妥当だが、Undoの適用先と表示終了まで完結していない。
本レビューが開始したテストprocessはすべて終了済み。ソース修正・アプリ起動・配布build・commit / pushは行っていない。
