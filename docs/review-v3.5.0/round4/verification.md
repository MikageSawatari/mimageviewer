# 第4回 検証記録

対象: `b46bf8e1b6a15861b7ed8b7f594eb08434fc776a`。
7ファイルのproduction/テスト差分を確認。作業中のアプリコード編集なし。

## 検証方法

- `geometry_probe.py`: 第3回と同じケース行列を、新しい本番 `continuous_unit_layout` とpage rect / paint band / offsetで計算。抽出する型/関数名と高さの取得先だけを新APIへ合わせ、本番算術は変更しない。DPI100/125/150/200%、倍率1/.73/1.125、4回転、gap0/1/20、正負を含む5原点。
- `flow_probe.py`: 本番のbatch配送/確定/評価finalize/Undo構築、および関連付けprewarmメソッドを抽出。第3回と同じDB・Undo・Shell・UI境界の代用品で、所有者と配送先の差、アイドル状態の後続起床を観測。実HWND/パッド/OS関連付けの検査ではない。
- `ai_probe.rs`: ビルド済み本番ライブラリの公開 `book_ai_snapshot` を実行。runtime=Noneで上限外/上限一致/上限内/機能OFF/モデル未指定を検査。通常profileに触れないdata overrideを指定。AIモデル/GPU推論は実行しない。
- 再走査: watcher、フォーカス復帰、トレイ復帰、再要求へctxが接続され、送信後にrepaintすることをソース照合。取消済み要求はpendingから外されるので、起動直後のcancel早期returnに新しい採用待ちは残らない。失敗(None)の送信も起床する。

## 結果

| 検証 | 結果 | 記録 |
|---|---|---|
| N03の狭い本番libテスト | 1 passed | `test-ai.log` |
| 幾何probe | 8,640境界すべてgap誤差0.01px以内 (第3回は1,440境界で超過) | `geometry-probe.log` |
| 本番AI runner probe | 5条件すべて期待どおり。非適用時の寸法/全画素も一致 | `ai_probe.log` |
| 終了配送・prewarm probe | Q01/Q02を再現。同一ownerと継続pollのcontrolは正常 | `flow_probe.log` |
| 全体ゲート | PASS。51結果群、8,175 passed / 0 failed / 36 ignored | `test-full.log` |
| fmt / UI文字 | PASS / 危険文字0 | `fmt.log` / `glyphs.log` |

全体ゲートはworkspace、開発補助bin、統合テストに加えて、workspace除外のegui-wgpu (9件) / eframe (15件) も含む。
本体libは7,251 passed / 30 ignored。新しい5件のテストもすべて通過した。
開始から終了までHEADは同じ。全体ゲート・fmt・狭いテストの実行はすべて終了した。

指摘再現probeの終了コード0は、記載した不具合の観測値をassertできた意味。製品正常性を示す合格と混同しないこと。

## 周辺経路と限界

- N02は配置後unionと描画が同じ計算関数を使う修正で、端数epsilonや特定の左右条件で症状を抑える形ではない。前回の見開き残件は改善した。ネイティブ表示のスクリーンショット比較は未実施。
- N03は通常/stackの製本・一括出力、および外部ツールmaterializerの両producerがOptional runtimeを渡す。AI policy fingerprint、注釈の元寸法、出力workerのleaseは維持している。
- 外部プロセスの起動/終了対象、IPC形式、Remote service本体に差分なし。既存の停止/解除/worker lifetimeを全体ゲートの範囲で確認した。実Remote端末の接続は未検証。
- 新しいキーボード/マウス/タッチのbinding変更はない。gamepadの終了はQ01の所有問題を残す。共通表示geometryを通る入力方法は同じ算術検証の対象になる。
- フォルダの世代・削除中の結果棄却、AI cache policy、祖先preset resolverの前回修正は保持。非Windows shadowと配布ビルドは今回のレビューでは実行しない。
- 前回までに延期承認された大きい修正は、解消とせず延期のまま扱う。未コミットの重複検出計画、他のレビュー結果、他のプロセスは変更しない。
