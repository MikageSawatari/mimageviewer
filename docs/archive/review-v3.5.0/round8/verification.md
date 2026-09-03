# 第8回の検証記録

## 対象と方法

2026-09-03。開始 HEAD `8868b43780a557121e285adf0ff907bf6ca0f367`。
途中で同じ内容が `200c7781b` としてコミットされた (6 files / +385 -30)。
対象は U01 修正の 5 source files と `docs/detached-rework-plan.md`。差分は [reviewed.diff](reviewed.diff)。
無関係な `docs/duplicate-detection-plan.md` と過去のレビュー記録を保持する。

前回参照済みの architecture / preset / detached §2 / virtual folder の所有境界を継続し、
今回の §11 差分、ui-responsiveness §4、web-remote-plan の読み書き境界を追加確認した。

## ソースと probe

- 全 `PageAdjustmentTarget` / `PageIndexHint` の生成・利用箇所を検索。UI の `At`、Remote / Undo 記録の `Unresolved`、Undo / Redo 直前の一括解決を確認。
- 両 stack 適用入口と共通 writer、Global / Favorite のカスケード復元、Remote capture、content identity の入口から worker まで追跡。
- `python docs/review-v3.5.0/round8/lookup_probe.py`: **PASS**。通常画像 / ZIP / PDF の At → Absent → 並べ替え後 At、stale At の拒否、重複キーの全 index 反映、Global / Favorite / 旧 Page の通過を確認。同 probe で最適化済み CPU 計測を実施。[lookup-probe.log](lookup-probe.log)
- `cargo fmt --all --check`: **PASS**。[fmt.log](fmt.log)
- 開始時・中間時・終了時の対象 5 source files の SHA-256 は一致。

## 自動テスト

- 初回の `cargo test -p mimageviewer --lib adjustment_undo_target_tests` は **リンク段階で中断**。Windows が `target/debug/deps/mimageviewer-fe3ca2e444d02a8b.exe` の書き換えを拒否した (`permission denied`)。実行テストの assertion failure ではない。[target-tests.log](target-tests.log)
- 配布ゲートと同じ `--features pack-build-tools` の別 test executable で再実行し、**new target 4 passed / 0 failed** (0.94 秒)、**既存 owner 12 passed / 0 failed** (1.91 秒)。[target-tests-pack.log](target-tests-pack.log)、[owner-tests.log](owner-tests.log)。既存の他作業プロセスは停止していない。
- `$env:RUST_TEST_THREADS='8'; .\scripts\test-full.ps1` は **exit 101**。`mimageviewer-launcher` の build script が、埋め込み入力 `target/release/mimageviewer-core.exe` と `mimageviewer-remote.exe` の未生成を検出して停止。[test-full.log](test-full.log)。これは U01 の assertion failure ではない。
- 残る対象を `cargo test --workspace --exclude mimageviewer-launcher --features pack-build-tools --no-fail-fast` と既存の vendored 2 gate に分けて実行し、**いずれも exit 0**。同時実行数は 8。**標準全体ゲート PASS と扱わない**。不足する release バイナリを偽のファイルで代用したり、build script の前提チェックを変更したりはしていない。

| 対象 | 結果 |
|---|---|
| 本体 lib | **7,269 passed / 0 failed / 30 ignored**、360.79 秒 |
| UI snapshot | **43 passed / 0 failed**、5.09 秒 |
| その他 workspace (launcher を除く) | 全て PASS |
| vendor/egui-wgpu | **9 passed / 0 failed** |
| vendor/eframe | **15 passed / 0 failed** |
| 上記合計 | **8,186 passed / 0 failed / 36 ignored、50 harness** |

ログ: [workspace-tests.log](workspace-tests.log)、[vendor-egui-wgpu.log](vendor-egui-wgpu.log)、
[vendor-eframe.log](vendor-eframe.log)。合計は footer の合算で、先行 4 + 12 件 / probe は二重計上しない。
launcher の 7 件は今回は未実施。release core / remote の生成後に標準全体ゲートを再実行する必要がある。

最終 HEAD: `200c7781b93519278147e306b53aeb383c7074ae`。
今回開始したテスト・probe のプロセスは全て終了。他作業のビルド / テストは停止していない。

## 検証の限界

probe は関数本体を抽出し、無関係な型 / フィールドを最小化した CPU / 状態遷移検証。
DB 書き込み・実 HWND / gamepad / 実機 Remote・配布物の署名 / packaging の検証とは区別する。
非 Windows shadow は今回再実行していない。通常プロファイルは操作せず、アプリも起動しない。
