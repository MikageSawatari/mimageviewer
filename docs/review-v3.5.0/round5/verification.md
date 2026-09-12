# 第5回 検証記録

対象: `7127fd14f7046024b34bea76b324922f78e36b91`。
アプリコード、通常 profile、配布物を変更せず、レビュー資料と独立した probe のみ作成。

## 実行結果

| 検証 | 結果 |
|---|---|
| 追加テスト + 評価の失敗 / Undo / 安定キー | 15 passed / 0 failed。`focused-tests.log` |
| 全体ゲート `scripts/test-full.ps1` | PASS。8,177 passed / 0 failed / 36 ignored (51 harness の合計)。`test-full.log` |
| `cargo fmt --check` | PASS。`fmt.log` は空 |
| リリース差分の `git diff --check` | PASS |
| 関連付けの完了通知だけで進む検証 | 0 / 1 / 8 / 9 / 17 / 25 種類、および実行中の世代変更、7 ケース PASS |
| コンテナ評価 + 画像ピッカー終端 | 2 行 × 3 配送先の6ケース。評価保存先とUndoは正しい。画像設定の誤配送 / 消失を確認 → S01 |

狭いテストは、今回追加された2テストを含むことを `--list` で確かめた既存の lib test
executable `target/debug/deps/mimageviewer-fe3ca2e444d02a8b.exe` で実行した。
続く全体ゲートは Cargo 経由で現在のソースから実行して完了した。本体 lib は
7,253 passed / 0 failed / 30 ignored、319.25 秒。既存 executable だけに依存していない。
本レビューが開始したテスト / fmt / probe プロセスはすべて終了済み。
最終 HEAD は開始時と同じ `7127fd14f7046024b34bea76b324922f78e36b91`。

## Q01 の確認

- `RingPickerContainerTarget` は開いた時点の rating key / source / metadata を保持する。
  preview / finalize / Undo payload が同じ記録を使う。保存先を現在のフォルダから再解決する
  前回のコンテナ評価の不具合は解消。
- `write_container_rating_for_target` は既存の `write_user_rating_shared` を通る。
  DB 成功後だけ共有世代を公開する順序、DB 失敗時のキャッシュ非公開を維持する。
- 現在表示中のコンテナが一致する場合だけ、その表示キャッシュを直接更新する。
  他 context は既存の `sync_current_context_rating_session_writes` を mount / unmount 時に
  消費する構造を維持。`meta_undo` は App-global であり、別 context から積むこと自体は不具合と
  していない。新 Undo payload の保存先は正しい。
- Folder / ZIP / ZIP 内ディレクトリ / PDF / 変換アーカイブの target resolver は従来と同じ。
  検索等の合成ビューで target がない場合は書き込まない。
- キーボードの通常 / 全画面 / native video、情報パネル、フォルダバーの評価入口は
  `set_current_folder_rating` に接続し、従来どおり現在の target を一度解決して共通 writer を呼ぶ。
  キー割り当てやマウス / タッチイベントの意味を変える差分はない。
- Remote の `persist_remote_rating` は従来どおり RemoteAddress から target を解決し、
  共有 writer を呼ぶ。今回の picker snapshot を Remote の入力へ流していない。
- ただし終了時は評価以外の dirty row も処理する。所有者の保持は picker 全体には及んでおらず、
  PostFilter / UpscaleModel で S01 を確認。SpreadMode / ReadingFlow / ReadingDirection /
  FitMode / 動画設定も現在の状態から確定するので、修正時には全行の所有境界を揃える必要がある。

## Q02 の確認

- worker は結果送信後に再描画を要求する。受信した poll は同じ呼び出し内で次の batch を起動する。
  実行中は `try_recv` で即時 return し、UI が待機しない。列挙処理は worker のまま。
- `flow_probe.py` のアプリ側は完了による repaint callback の通知を受け取った時だけ次 frame を
  実行する。任意の sleep / 手動の連続 poll で継続性を補っていない。
- 8 件境界の前後、4 batch、実行中に別の拡張子集合へ世代を変える条件で、最終集合をすべて更新。
  処理後は worker / queue とも空。新世代では古い queue の残りを使わず再構築する。
- 拡張子別 cache は App-global であり、前フォルダの完了結果を格納すること自体は正しい。
  実際の Shell、COM、関連付け登録内容は probe では代用。追加された app test は本来の列挙経路を通る。
- プロセス起動、引数構築、ACK、取消、外部プロセス制御の差分はない。
  延期済み R10 (cache miss の同期列挙) は今回の完了通知修正とは別に扱う。

## 変更ファイル別の確認

| ファイル | 確認内容 |
|---|---|
| `src/app.rs` | 固定 target writer、既存入力からの呼出、成功後公開、prewarm の受信→継続と起床 |
| `src/app/gamepad_input.rs` | target snapshot と全 preview 入口、finalize、Undo、終了要求、全 dirty row の適用先 |
| `src/ring_shortcut.rs` | 一時 picker の型、source / metadata の寿命、設定永続化への影響なし |
| `src/undo_ops.rs` | 明示 target の payload、通常コンテナ評価の委譲、Undo / Redo の DB 保存先 |
| `src/app/tests.rs` | 追加2テストと既存3テストの接続。Grid の評価だけでは画像設定の終端を検出できない |

## 未実施・範囲

- 実 HWND のフォーカス遷移、ゲームパッド抜去、native video overlay、実 Shell の遅延・COM 障害、
  実機 Remote 操作は実行していない。抽出 probe と実デバイスの通し確認を区別する。
- 端数ピクセル、書き出し AI、外部起動、Remote protocol のコードは前回確認から変更なし。
  前回の geometry 8,640 境界 / AI 5 条件の記録を保持し、変更のない計算を今回再実行したとはしない。
- アプリの動作変更を行っていないため、dev / release / portable のアプリ起動や配布ビルドは行わない。
  他作業のビルドを停止せず、同時編集によるビルド不整合を起こすソース変更も行っていない。
- 既存の延期判断 R07 / R08 / R09 / R10 / R11 を維持。追加の延期は本レビューでは判断しない。

S01 を修正した後の実機確認は、異なる個別エフェクトを持つ B / C を開き、B のピッカーを
残して C が操作対象になった状態で切断 / OFF する。B のみ確定され、C の表示・保存値が変わらず、
Undo が B の真の変更前値へ戻すことを確認する。メイン一覧 A へ移る場合は B のモデル選択が
失われないことも確認する。
