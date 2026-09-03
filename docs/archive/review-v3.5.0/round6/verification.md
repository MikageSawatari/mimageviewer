# 第6回 検証記録

対象: `6cd99d91596189f82a4601ba85d89209d715ced7`。
実動作は `af6fe006d`、続くcommitはCLAUDE.mdのマニュアルページ数の修正のみ。

## 自動検証

| 検証 | 結果 |
|---|---|
| `cargo test -p mimageviewer --lib ring_picker_owner_tests` | 8 passed / 0 failed。`focused-tests.log` |
| `scripts/test-full.ps1` | 本体7,261件とUI snapshot以外のworkspace targetは成功。UI snapshotが進まなくなり、全体実行15分経過後に当該processのみ停止。通常の全体ゲートPASSとはしない |
| UI snapshot逐次再実行 | 同じ43件がPASS (22.29秒)。`--test-threads=1`、`ui-serial.log` |
| workspace外の描画 / scheduler検証 | `egui-wgpu` 9 passed、`eframe` 15 passed。スクリプト後段と同じコマンドで別途完了 |
| `cargo fmt --check` | PASS。`fmt.log` は空 |
| `git diff --check 7127fd14f..HEAD` | PASS |
| `undo_probe.py` | PostFilter / UpscaleModel × B→B / B→A / B→C の6ケース。初回確定は正常、A/CでUndoすると誤書き込み → T01 |
| `overlay_probe.py` | native動画B→Bは表示解除、B→メインAではAppのpickerが消えてもnative表示が残る → T02 |

probe は元のsourceから関数本体を読み出して別のRust executableへ組み込み、境界を代用する。
アプリsourceへのテスト追記・差し替えは行わず、実機描画・通常profileを使わない。
出力Rustとexecutableは `target/review-v350-round6/`、ログと再現スクリプトは本ディレクトリ。
前回のfixture定義を再利用しているが、実行する関数本体は **今回のHEAD** から抽出する。

### 全体ゲートの停止と再確認

初回は `tests/ui_snapshot.rs` の43件中20件が完了した後、残りの完了が進まなくなった。
processはCPUを使用し続けていたが、これだけでは変更コードの不具合・GPU・テスト実行基盤の
どれが原因かは確定できない。全体開始から15分経過した16:33:18に、本レビューが開始した
UI test processのPID / 名前 / 開始時刻を照合してそのprocessだけを停止した。
`ui-retry-note.log` に操作を記録している。

そのため初回 `test-full.log` はUI target失敗として終了する。この終了コードは手動停止の結果であり、
snapshotの不一致やアプリのテストassertion失敗と分類しない。`--no-fail-fast` により残りのworkspace /
doc testは完了。スクリプトがそこから先へ進まないため、後段のvendored 2 targetは同じ引数で
別途実行した。UIの並列停滞は、逐次再実行が成功しても解消を証明したことにはならない。

逐次実行では同じ43件が22.29秒で成功した。残りのworkspace / doc testとvendoredの2 targetも
成功しており、完了harnessの合計は **8,185 passed / 0 failed / 36 ignored**。
途中で止めたUIの20件は再実行43件に含まれるので二重計上せず、狭い8件やprobeもこの合計には
加えていない。通常の並列 `test-full.ps1` を完走したPASSとは区別する。

本レビューで開始したgate / 再実行 / fmt / probeのprocessはすべて終了済み。
最終HEADは開始時と同じ `6cd99d91596189f82a4601ba85d89209d715ced7`。

## 確認できた改善

- `RingPickerState.owner` は生成時の `edit_request_owner_context()` から取得する。
  後で前面の窓から所有者を引き直していない。
- 画像 / 動画 / 一覧の設定applyは既存 `with_owner_viewer_context` → `with_viewer_context`
  で所有者をmountして実行する。前の投影を復元し、閉じたcontextへ別の保存先を当てない。
  同じcontextの場合も既存のmount計画に任せている。
- 同じ窓が別ページへ進んだ場合、既存のfolder / item anchorで一致を確認してから画像 / 動画の
  indexを使う。今回のテストで「同じindexに別ファイルが入る」場合の誤保存を防いでいる。
- S01で報告した初回確定先とPostFilter確定時の変更前値は、正しいBのcontext上で扱われる。
  カラー化のoriginal復元も同じscopeへ戻る。AIモデルは確定時だけ適用する仕様を維持。
- 一覧の並べ替えテストは実際の一時フォルダと `load_folder` を使い、nested mount後にも
  一覧の2項目が戻り、別contextの1項目が残ることを確認する。
  設定はApp共通、一覧の組み直しは所有者、という既存の分担を維持している。

## 終端とUndoの確認

- `commit_ring_picker` のうちownerを使うのは設定applyの包み。
  native overlay clear、評価finalize、評価Undo構築はその前に実行される。
  「確定全体が所有context上で行われる」とは異なるため、各処理の保存先を別々に確認した。
- 評価は既存の固定pathと成功後公開ledgerを使い、今回の差分で保存先を元へ戻してはいない。
  App-globalな評価Undoにpathがあること自体は適切。
- 画像補正のUndoはその場で生成した `Page(idx)` をApp-globalなスタックへ渡す。
  mount後の保存先は正しくても、取り消す時点の保存先は固定されない → T01。
- 動画Volume / PlaybackSpeed / ContinuousModeの設定関数はmount中の `fs_cache` / idxを参照する。
  しかしnative pickerの表示解除はそのmountの前なので、別のplayerまたはplayerなしを参照する → T02。
- ParkedLiveの動画には別の毎フレーム同期があることを確認。T02はそれに含まれないactive動画窓を
  メインと併用する条件に限定した。native presenterの表示保持、setterと呼び出し側を照合。

## 機能別・ファイル別の範囲

| 領域 / 変更ファイル | 確認 |
|---|---|
| `src/app/gamepad_input.rs` / `src/ring_shortcut.rs` | ownerの生成・寿命・全行apply・窓消失・ページ変更・通常確定/OFF/切断、Undo消費とnative表示終了まで追跡 |
| `src/app/tests.rs` | 追加8テストを読んで実行。Bへ戻る前後でのUndoの違い、native表示を観測しない範囲を確認 |
| `docs/detached-rework-plan.md` | §2と新しい§11記録を照合。要求ownerを保持する方向は妥当だが、Undo消費・表示終了の残件は未解消 |
| 一覧 / 画像の表示設定 | ソート再読込、見開き、読み方向、fitの既存関数を確認。新規の端数計算や描画変換の変更なし |
| 操作カスタマイズ / キーボード / マウス / タッチ | キー・ボタン割当の変更なし。Undoの固定キー入口は現投影のAppへ渡すためT01に関係。表示applyの既存入口を維持 |
| 外部ツール / UI応答性 | 起動・引数・ACK・cancel・workerの変更なし。新しい待機・I/O・decodeを追加していない。既存mountの利用を確認 |
| mIV Remote | IPC/remote処理の差分なし。共有Undoの復元先はT01で確認し、既存RemoteのPageKey経路との違いを照合 |
| `src/books.rs` / `src/ui_dialogs/export_batch.rs` / `src/ui_fullscreen.rs` | 実行コードの変更なし。重複docコメントの整理のみ |
| `CLAUDE.md` | マニュアルページ数28→29の記載訂正のみ |

## 検証の限界と引き継ぎ

- HWNDフォーカス・物理ゲームパッド・実D3D11 overlay・実機Remoteを通した確認はしていない。
  source/抽出probeと実ハードウェアの合否を区別する。
- 非Windows shadowのPASSは修正担当者の報告。今回こちらでは実行していない。
  新owner型とsingle-context分岐を静的確認。UI文字列の変更はなくglyph検査も再実行していない。
- 端数geometry / 書き出しAI / 関連付け起床は前回までの改善を維持する差分であり、変更のない
  個別行列を今回も再実行したとはしない。
- R07 / R08 / R09 / R10 / R11は既存の延期判断を維持。共有スタックそのものを問題視していないが、
  別ページへ書くT01はR11のcache失効通知とは別のため、追加延期を推定していない。
- アプリsource・通常profile・配布物・他のビルドを操作せず、レビュー資料だけを追加。
  アプリの動作変更をしていないので検証用アプリのbuild/launch、commit/pushは行わない。

修正後は、Bの画像設定を確定した直後に **前面のA/Cで** Undo / Redoし、Bだけが戻ることを確認する。
動画はactive別窓Bでpickerを開き、メインAで切断 / OFFしても、Bの表示が消えてパッドなしで
閲覧を続けられることを確認する。BをParkedLiveへ変えたケースだけで代用しない。
