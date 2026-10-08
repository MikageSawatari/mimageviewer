# 第7回の検証記録

## 対象

2026-09-03。HEAD `6cd99d91596189f82a4601ba85d89209d715ced7` + T01 / T02 の未コミット差分。
レビュー中に同じ内容が `8868b4378` としてコミットされた (9 files / +353 -34)。
`reviewed.diff` に対象 7 source files + 関連 2 docs を保存した。
`docs/duplicate-detection-plan.md` は無関係な作業として変更・レビュー対象から除外。
参照文書: docs/README、CLAUDE の領域別ガイド、architecture-overview、preset-and-adjustment、
detached-rework-plan §2 / §11、virtual-folders、video-architecture の native presenter、
keymap-spec / key-customization-impl-plan、ui-responsiveness、development-build-and-test。

## ソース確認

| 領域 | 確認した経路と結果 |
|---|---|
| 記録元 | `capture_adjust_full_inner` の既存 / 新規ページ差分、パネルの即時変更 / drag release、detached pause の drag 確定が共通の PageKey へ接続。プロダクションで Page を直接積む別経路は検索上なし (キーを得られない場合の退避を除く) |
| Undo / Redo | 共通 stack の配送先、既存 Global → Favorite → Page の復元順、逆向き Redo、PageKey 復元、解除時のキャッシュ処理、reset 後の履歴破棄を追跡。大量ページの反復検索は U01 |
| 保存形式 | 通常画像は normalize_path、ZIP はコンテナ + entry、PDF はコンテナ + page キーを共用。location / sidecar / compiled_book を要求に保持し、復元側も同じ set / clear writer に到達。製本の標準無補正 / sidecar 無効は維持 |
| Remote | mounted / unmounted ページの既存 PageKey writer、capture の二重記録防止、標準変更と個別解除の記録を確認。新たな IPC / Remote 設定変更はない |
| native HUD | `commit_ring_picker` → owner mount → HUD clear → anchor 照合 → 設定適用。物理入力の解除は共通状態に維持。所有窓消失時は他窓へ付け替えず終了 |
| 入力 / マージ | 直接記録箇所の置換と setter 戻り値の既存 caller を確認。キー / マウス / タッチ / パッドの割り当てや外部プロセス操作・描画 geometry の変更はない |
| 延期 | R07–R11 を維持。R11 の今回の追記 (別窓 Undo 後の元 viewer の表示失効) を新規指摘と混同しない |

## 実行済み

- `cargo test -p mimageviewer --lib ring_picker_owner_tests`: **12 passed / 0 failed**。テスト本体 11.69 秒。[focused-tests.log](focused-tests.log)
- `cargo fmt --all --check`: **PASS**。[fmt.log](fmt.log)
- `python docs/review-v3.5.0/round7/overlay_probe.py`: **4 条件 PASS**。実装の dispatch / commit / owner apply / clear / setter を抽出し、保持される HUD 状態を確認。同窓 / 前面一覧 / 前面別動画で元 HUD を解除し、無関係な前面動画の HUD を維持。所有者消失では配送しない。[overlay-probe.log](overlay-probe.log)
- `python docs/review-v3.5.0/round7/lookup_probe.py`: **完走、U01 の計測を保存**。実装の検索・キー生成を抽出した最適化済み CPU probe。初回の fixture 型不一致 (PDF page_num を usize としていた) は fixture を u32 に訂正し再実行。アプリのコンパイル不具合ではない。[lookup-probe.log](lookup-probe.log)
- 開始時・中間時・終了時の対象 7 source files の SHA-256 は一致。

## 全体ゲート完了

`$env:RUST_TEST_THREADS='8'; .\scripts\test-full.ps1`。
前回の UI snapshot の高並列実行停滞を踏まえ、テストの対象は変えず同時実行数を 8 に制限。
環境変数はこの子 shell のみ。**exit 0 / [test-full] PASS**。結果は [test-full.log](test-full.log)。

| 対象 | 結果 |
|---|---|
| 本体 lib | 7,265 passed / 0 failed / 30 ignored、415.34 秒 |
| UI snapshot | 43 passed / 0 failed、6.33 秒。前回の停滞は今回の同時実行数 8 では発生せず |
| vendor/egui-wgpu | 9 passed / 0 failed |
| vendor/eframe | 15 passed / 0 failed |
| workspace / doc / vendor 全体合計 | **8,189 passed / 0 failed / 36 ignored、51 harness** |

合計は全体ゲートの各 footer の合算。先行した owner 12 件や抽出 probe は二重計上しない。
最終 HEAD: `8868b43780a557121e285adf0ff907bf6ca0f367`。
本レビューが開始したテスト・probe のプロセスはすべて終了済み。他作業のプロセスは操作していない。

## 検証の限界

native player / HWND を起動した検証、パッドの実切断、実機 Remote、配布ビルドは実行していない。
非 Windows shadow チェックは今回再実行していない (Windows の全体ゲートと cfg 分岐のソース確認)。
HUD probe は player の保持状態 / mount 境界を置換し、HWND 描画そのものの検証とは区別する。
通常ユーザープロファイルは操作していない。レビューのみのため、アプリコード変更・検証用バイナリ作成・commit / push は行わない。
