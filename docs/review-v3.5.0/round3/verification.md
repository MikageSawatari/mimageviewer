# 第3回 検証と確認範囲

## 実行結果

対象: `6a3fe26357d623bd5a368ab38bafa4d2748886e6`。実装ファイルへの未コミット差分なし。
終了時HEAD `25010330e4852fe0a6173cb84af239e1256687f5` の追加はバックログ文書95行のみ。
差分を確認し、実コード同一のためテストを再実行していない。

| 検証 | 結果 | 証拠 |
|---|---|---|
| 狭い連結境界のlibテスト | 2 passed | `test-geometry-unit.log` |
| `scripts/test-full.ps1` | PASS。51結果群の合計 8,170 passed / 0 failed / 36 ignored | `test-full.log` |
| main lib (全体ゲート内) | 7,246 passed / 30 ignored | 同上 |
| 除外workspaceのegui-wgpu / eframe | 9 passed / 15 passed | 同上末尾 |
| `cargo fmt --check` | PASS (出力なし) | `fmt.log` |
| UI文字チェック | 危険文字0 | `glyphs.log` |
| 現算術の連結境界probe | 8,640境界中1,440でgap誤差>0.01px。単枚controlは0 | `geometry-probe.log` |
| 本番終了/確定/列挙選択の制御フローprobe | N01/N05を再現 | `routing_probe.log` |
| 本番compositorのAI後注釈probe | 元寸法None/Someとも32×32画像の(16,16)が赤、(8,8)は黒 | `annotation_probe.log` |
| 本番AI要求/対象サイズ判定probe | N03の判定差を確認 | `ai_request_probe.log` |

指摘を再現するprobeは、現状の不具合の観測値をassert/出力するもの。終了コード0を製品の正常性と取り違えないこと。
本番メソッド/算術を抽出したprobeと、公開された本番compositorをリンクしたprobeを区別した。
前者のDB・Shell・UI境界は代用であり、ネイティブAPIの安全性や実HWNDの挙動を実測したものではない。
生成したRust/実行ファイル/画像は `target/review-v350-round3` 配下。compositorには同配下のdata overrideを設定した。

## 機能ごとの照合

- **連結・見開き・端数**: 共有量子化のepsilon変更、unit可視原点、span、連結offset、Originalの高さ非統一、トリムbboxの回転を追跡。100/125/150/200%DPI、正負原点、奇偶混在、0/1/20gap、複数倍率・回転を算術検査。N02に残件。画面キャプチャの比較ではない。
- **外部変更の走査**: 同じ走査条件への再要求予約、条件変更時の取消、一覧世代、削除競合、結果棄却、適用後再実行を確認。R02の同一owner再通知とR03の古い結果上書きは改善。N04の完了起床欠落を確認。非投影ownerの結果は今も棄却する設計で、全contextへの変更配送を完成したとは判定しない。
- **一括書き出し**: 選択したpage keyの保持、index再解決、対象消滅、通常/ZIP/PDF/スタック分岐を確認。対象すり替わりは改善。編集snapshotの重い準備と要求所有者全体のworker化は承認済みR08の延期として扱う。
- **AIと出力cache**: `BookAiPolicy` のfeature mode、upscale/denoise上限、背景色がfingerprintとrunnerで共有されることを確認。非AI段/機能OFF/モデル未指定はpolicyを使わない。以前のcache誤再利用は改善。新しいruntime必須判定にはN03。
- **スタック注釈/標準補正**: AI直前の寸法を合成所有者が保持し、保存済みauthoring寸法を優先する修正を実compositorで確認。ページ個別→標準を持つ最寄り祖先→globalの共有resolverも確認。
- **パッド/操作カスタマイズ**: 通常のmount後配送と終了経路の差からN01。単一contextのUndoテストは通るが複数contextを証明しない。今回の差分にキーボード/マウス/タッチのbinding変更はなく、共通keymap・リング・入力の既存テストは全体ゲートで通過。マウスで呼べるリングを含め、同じ状態の終了契機は所有者に従う必要がある。
- **外部ツール/他プロセス**: 今回の関連付け変更は候補cacheの更新。起動対象/成功失敗別の一時ファイル所有/プロセス終了処理には差分なし。候補cacheの上限にN05。cache missのUI同期列挙はR10として延期承認済み。
- **UI応答性**: 走査のworker化は維持。重い一括snapshot、見開き合成、関連付けmissを「解消」とはせず延期として記録。N04にはイベント完了の起床が必要で、常時描画による回避は勧めない。
- **mIV Remote**: 今回はRemote wire形式/IPC/serviceの差分なし。以前修正した両出力workerのmandatory `LocalAiActivityLease` を維持し、既存のRemote初期化/排他/解除テストは全体ゲートで通過。`WaitingForRemote` が失敗と異なる状態であることも確認。実端末での接続・モデル推論・操作切替は未検証で、このレビューだけで実機退行なしとは断定しない。
- **文書/延期判断**: UI注記・README・manualのexport/settings/changelogを照合。単枚/Mergedが段を使わない制約を記載済み。5つの延期項目には残存症状と将来の修正境界が記載されている。
- **マージ整合性**: 全17ファイルの差分を分類、実コードと関連テストを照合。重複画像検出の2調査/設計文書と索引更新は実コードを持たない別作業。終了直前の書庫ロード改善計画も文書のみと確認し、実装レビューから除外。全体ゲートで通常/統合/開発補助bin/vendorを確認。非Windows shadowチェックとLinux CIは今回再実行していない。

## 未実施・作業境界

通常profileのアプリ、portable smoke UI、外部アプリは起動していない。実ゲームパッド/タッチ/複数モニター/実Remote端末による操作検証は未実施。
アプリの修正をしていないため検証用バイナリの新規ビルド、署名/配布物の作り直しはこのレビューの対象外。
commit/pushなし。元から変更中の `docs/duplicate-detection-plan.md` と以前のレビュー資料を保持した。
