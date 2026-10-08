# v3.5.0 再レビュー (第8回)

開始: 2026-09-03。基点 `8868b43780a557121e285adf0ff907bf6ca0f367` + U01 修正作業ツリー。
レビュー中に同じ内容が `200c7781b` としてコミットされた (6 files / +385 -30)。対象ソースの SHA-256 は一致。
アプリソースは編集しない。レビュー用資料と probe のみ作成する。

**再レビュー完了。U01 の解消を確認し、新規の修正必須指摘なし。**
ランチャー以外の workspace + vendor は **8,186 passed / 0 failed / 36 ignored**。
本体 7,269 件 / UI snapshot 43 件を含む。標準全体ゲートは、launcher の入力 release exe
2 点が未生成のため未完了。launcher 7 件を未実施として区別する。

| 確認対象 | 状況 |
|---|---|
| U01 の一括解決と検索コスト | 一意キーの一覧で解消を確認。1 万件 / 5,000 ページの検索は約 14.7 秒 → 7.3 ms |
| 解決結果の寿命、Undo / Redo、窓 / 一覧の変更 | 毎回の再解決を確認。At → Absent → 並べ替え後 At、stale hint 拒否、重複キーの全 index 反映も probe PASS |
| Remote・content identity・共通 writer | 生成 / 使用 / 再解決、正本 / runtime、worker 配送を確認。新規指摘なし |
| 回帰テスト・全体ゲート | 新規 4 + owner 12 件と fmt は PASS。本体・周辺・vendor も PASS。launcher は release 入力の未生成で検証できず、標準全体ゲート PASS とはしない |
| 既存の延期事項 | R07–R11 および第7回で明示された表示更新の延期を維持 |

無関係な `docs/duplicate-detection-plan.md` の変更、既存のレビュー履歴を保持する。

詳細: [findings.md](findings.md)、検証の範囲と結果: [verification.md](verification.md)。
最終 HEAD: `200c7781b93519278147e306b53aeb383c7074ae`。
開始時・中間時・終了時の対象ソース SHA-256 は一致。今回開始した検証プロセスは全て終了済み。
