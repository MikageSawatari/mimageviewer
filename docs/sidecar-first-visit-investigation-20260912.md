# サイドカー復元ダイアログの初回訪問調査（2026-09-12）

## 対象と結論

v3.9.0 で `G:\home\comfyui\202609-21\_ランウェイのモデル` を初めて開いたときだけ、
「サイドカーから設定を復元中」ダイアログが一時表示された事象を読み取り専用で調査した。

この訪問で約 0.9 秒を占めた主因は、サイドカーの存在確認そのものではなく、初訪問時に
300 画像を対象に行った旧 XMP タグ seed の完了待ちである。ダイアログの見出しは全phaseで
固定されているため、保存処理の確定待ちでも「サイドカーから設定を復元中」と表示される。

## ログ証拠

通常プロファイルの `%APPDATA%\mimageviewer\logs\mimageviewer.log` に次の時系列がある。

- 初回: `load_folder` 3059.779s、restore開始 3059.782s、入力解放 3060.704s
  （restore ownerの存続時間は約922ms）。
- 3060.682s に
  `[TAG] legacy seed complete: candidates=300 read=300 imported_items=0 inserted_tags=0 marked_empty=300 skipped_decided=0 read_errors=0 db_errors=0`
  が記録されている。seed完了から入力解放までは約22msだった。
- 同じフォルダの再訪は restore開始 3061.895s、入力解放 3061.920s（約25ms）。
- 再々訪は restore開始 3066.890s、入力解放 3066.906s（約16ms）。
- 初回の `content_identity: detection ready` は入力解放と同じ 3060.704s に出ており、
  約0.9秒の待機を代表サムネイル選定やcontent identity検出の時間とは分類できない。

旧タグseedは未決定itemのXMPを読み、タグが無いitemも `tag_item_state` で決定済みにする。
再訪時は決定済みkeyをbulk queryで先に除外するため、同じ300ファイルを再読込しない。

## コード上の境界

- `src/app/sidecar_restore.rs` の `begin_sidecar_restore` は、通常フォルダで編集またはタグの
  sidecar backupが有効なら、`mimageviewer.dat` の存在を調べる前にApp-global restore stateを
  `Quiescing` で開始する。
- `Quiescing` は `quiesce_metadata_transfer_context_writers` を通じ、
  `tag_legacy_seed_pending` が完了するまで次の確認workerへ進まない。これは旧タグseedと
  tags DB marker/importの競合を避ける保存保護である。
- 待機が100ms以内に終わればダイアログは描画しない。100msを超えると
  `src/ui_dialogs/sidecar_restore.rs` の固定見出し「サイドカーから設定を復元中」と、
  phase別detail（このケースでは待機中なら「保存中の設定を確定中」）を描画する。
- seed完了後のworkerはsidecar writerのstrict idleを確認し、`mimageviewer.dat` のmetadata/readと
  source再検証、要求familyの中央DB marker読取を行う。sidecarが無ければmarkerの有無を調べ、
  古いmarkerだけがあれば削除する。sidecarがありmarkerと一致しなければimportへ進む。
- フォルダを訪問済みというだけでrestore coordinatorを省略するcacheは無い。再訪が短い主な
  理由は、今回のログでは旧タグseedの300件が決定済みになったことにある。sidecarが存在する
  場合は、中央DB markerが同期済みになることも再訪を短くする。

## 確認できなかった点

該当sessionは成功時のprobe outcomeを通常ログへ記録せず、該当時刻のperf logも無かったため、
実際にsidecar importまたはmissing-marker clearを行ったかは確定できない。調査環境では現在
`G:` の対象フォルダを参照できず、`mimageviewer.dat` の有無も直接確認していない。

今回の証拠から確定できるのは、初回の約0.9秒が300件の旧タグseed完了に支配され、seed後の
sidecar flush/probeから入力解放までが約22msだったこと、固定見出しが実処理phaseを誤認させた
ことである。100msの安全gateやwriter待機を変更せず、見出しをphaseに合わせるUI改善は
別項目として扱う。
