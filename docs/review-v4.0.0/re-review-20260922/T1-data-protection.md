# T1 データ保護領域 最終再レビュー (v4.0.0 出荷直前)

読み取り専用のコード照合のみ。アプリは起動しておらず cargo も実行していない。
以下の「〜になる」はすべてコードを読んだ限りの推定であり、実行時に観測した事実ではない。
対象は作業ツリー (未コミット差分を含む) と `6fdd912a7`。

## 結論

- **P1: 0 件**
- **確度の高い P2: 0 件**
- A (collection.db バックアップ) / B (壊れた移行記録) / C (サイドカー確認の再利用) の
  3 領域とも、設計条件どおりに実装されている。データ損失・誤削除・操作不能・永久固着・
  起動不能に当たる欠陥は見つからなかった。
- 出荷を止める指摘なし。

## 元指摘の判定

| ID | 判定 | 根拠 (作業ツリー) |
| --- | --- | --- |
| RE-1 バックアップ成否がログに残らない | **解消** | `src/collection_store/db.rs:269-282`(schema 移行前)、`:346-354`(初回変更前) が `crate::logger::log` で成否と `elapsed_ms` を出す。`eprintln!` は残っていない |
| RE-2 毎起動無条件 rotate | **解消** | `db.rs:250-285`。`ExistingV2` は `catalog_revision==0 && COUNT(*)==0` なら `NotRequired`、そうでなければ `Required`(= そのセッション最初の `Write` の直前に一度だけ)。`New` は `NotRequired` で、最初の書込み成功後に `Required` へ arm (`db.rs:324-326`)。編集しない起動では世代が動かない |
| RE-3 起動 3 パスに計装が無い | **解消** | `db.rs:1057-1097` の `collection/db_open` が `startup / open_ms / validate_ms / backup_ms / schema_ms / outcome`、`db.rs:1099-1117` の `collection/db_backup` が `trigger / outcome / ms`、`runtime.rs` の `collection/startup_catalog` が全行読みの `ms / outcome` を出す |
| RB-1 壊れた journal による恒久拒否 | **解消** (下の残件は P3 相当) | `Parse` のみ復旧モーダルを出す (`app.rs:32325-32345`)。退避は worker 上 (`app.rs:32492-32542`)、同一 handle 検証 + `ReplaceIfExists=false` + 一意名 (`rename_key_migration.rs:442, 597`)。失敗 / Changed / Cancelled では元記録と保護を維持 (`app.rs:32615-32650`) |
| RB-8 表示文言に serde の英語 | **解消** | `ui_dialogs/rename_migration_recovery.rs:19-49` と `app.rs` の toast 11 本すべて日本語。利用者可視の経路に `JournalLoadError` の `Display` は現れない |
| S2-3 再利用時の owner 分裂 | **解消** | `sidecar_import/probe_reuse.rs:102-107` の `matches()` が `sidecar.folder() == folder`(綴り完全一致) と `folder_key == normalize_path(folder)` と `data_dir` と `families` をすべて要求する。別綴りは `KeyMismatch` で strict probe へ戻る。`App.sidecars` の key は `sidecar.folder()` (`app/sidecar_restore.rs:1149,1160`) なので要求綴りと一致し、同一ファイルの owner が 2 つできる経路は消えている |

## A. collection.db バックアップ — 確認結果

1. **ログ / perf**: 上表 RE-1 / RE-3 のとおり両方に出る。schema backup 失敗も
   `db.rs:275-279` で 1 行残る。
2. **no-op / 重複拒否 / 容量拒否 / Conflict**: `execute_prepared` (`db.rs:308-331`) が
   `Write` のときだけ `ensure_session_backup()` を呼ぶ。`NoOp` / `Reject` は backup も
   revision 前進もしない。`add_batch` は全件が重複 / 容量拒否なら `added.is_empty()` で
   `NoOp` (`db.rs:651-657`)、`migrate_source_batch` は `replacements.is_empty()` で `NoOp`
   (`db.rs:974-979`)、revision 不一致は `MutationGuard::collection` が `Reject` (`db.rs:89-91`)。
3. **失敗時の扱い**: schema 移行前 backup の失敗は `Err` を返して移行しない
   (`db.rs:274-281`、`initialize_schema` / `migrate_v1_to_v2` の前)。通常の初回変更前
   backup の失敗はログを残して書込みを続行する (`db.rs:351-354`)。設計条件どおり。
4. **UI を止めないこと**: backup は actor スレッド上 (`execute_prepared` → `ensure_session_backup`)。
   client API は `create_collection` / `add_batch` 等すべて `Receiver` を返す形
   (`runtime.rs:509-608`) で、製品経路に blocking `recv()` は無い (該当 grep のヒットは
   すべてテスト)。段4B の期限は active-time 10 分なので、数百 ms〜数秒の `VACUUM INTO` で
   失敗表示にはならない。二重送信しても `add_batch` は `source_exists` で重複扱い、
   revision 不一致は `Conflict` になるため、データ面の害は無い。
5. **Prepare と transaction の間の競合**: `MutationGuard::revalidate` を必ず transaction 内で
   呼んでいる (`db.rs:450, 507, 535, 588, 673, …`)。catalog revision と対象 collection revision の
   両方を再確認し、`migrate_source_batch` はさらに `load_all_entries` の同一性まで見る
   (`db.rs:991-995`)。`VACUUM INTO` は transaction の外 (`execute_prepared` の `apply` 前)。
6. **新規 / 空 / 将来版 / 破損 DB**: 将来版は `IncompatibleSchema` を backup 前に返す
   (`db.rs:216, 242-244`)。破損は `validate_*` が backup 前に失敗する (`db.rs:220-227`)。
   新規・空 DB は `NotRequired` なので空の世代を押し出さない。
7. **変更入口の網羅**: `db.rs` の変更系 9 メソッド (create / rename / delete / set_order /
   add_batch / remove_entries / reorder_manual / relink / migrate_source_batch) はすべて
   `execute_prepared` を通る。collection.db へ書く接続は `db.rs:238` の 1 本だけで、
   Remote も同じ actor 経由 (`remote_ipc/persistent_collections.rs` は client の `Receiver` のみ)。
   **auto-aspect cache は `auto_aspect_cache.db`、動画ピンは別 DB** なので collection backup の
   引き金にならない (`auto_aspect_cache.rs:58`)。
8. 世代 rotate は snapshot 先行 (`db_backup.rs:41-57`) なので、`VACUUM INTO` 失敗時に
   既存 bak1..bak10 は動かない。
9. 未コミット差分の `sort_order_as_str` → `collection_sort_order_wire_name` 移動
   (`collection_store/model.rs:189-212`) は綴りが 8 種とも同一で、永続値の互換は壊れていない。

**備考 (指摘ではない)**: Remote 側の `wait_actor_reply` は 9 秒予算 (`persistent_collections.rs:41`)。
端末からの追加がそのセッション最初の変更になると backup が同じ actor で直列に走る。
10,000 件の実測 (actor 書込 93.557ms) から見て予算内に収まる見込みだが、これは計測記録に
基づく推定であって、backup を含む端末経路の実測ではない。

## B. 壊れた名前変更移行記録 — 確認結果

1. **出口への到達**: 退避ダイアログは `Parse` のときだけ出る
   (`app.rs:32328-32332`)。`admit_rename_migration_source_change` が操作のたびに
   `prompt = Shown` へ戻す (`app.rs:32682-32690`) ので、閉じても次の操作で必ず再表示される。
   admission は削除の単一入口 `start_delete_files` (`app.rs:33428`)、本の
   rename / delete / reorder flush / transfer (`app.rs:35626-35721`)、名前変更ダイアログ
   (`ui_dialogs/rename_item.rs:141`) を覆う。これらはいずれもメイン一覧の操作で、
   フルスクリーンからの削除入口は存在しない (`ui_fullscreen*` に delete 系の呼出なし)。
   ポーリングは `poll_rename_migration_pending` → `ensure_rename_migration_journal_loaded`
   が `App::update` から毎フレーム無条件で走る (`app.rs:74078`, `33012`, `32314-32315`)。
   トレイ格納は `root_tray_hide_modal_owner` がモーダル表示中を拒否する
   (`tray_integration.rs:255-263, 644-646`)。
2. **退避**: worker 上 (`JournalQuarantineTask::spawn`)。衝突しない一意名
   `<file>.invalid-<128bit>-<attempt>` (`rename_key_migration.rs:442`)、`ReplaceIfExists=false`
   (`:597`)。成功後は空の prior として扱い、局所保持ジョブを 1 本の完全スナップショットへ
   統合して save ACK を待つ (`app.rs:32598-32613`)。元の操作は自動再実行せず、
   「もう一度実行してください」を明示する。
3. **退避しない場合**: `RecoveryFailed` のまま停止するだけで、閲覧とアプリ終了は可能
   (admission は物理変更の入口のみ)。
4. **一過性 Read 失敗での誤退避**: 起きない。`prompt` は `parse_bytes().is_some()` でのみ
   `Shown` になり (`app.rs:32328-32332`)、`Read` は明示再読込だけを行う (`app.rs:32691-32695`)。
   `journal_load` は `std::fs::read` でバイト列を読むため、不正 UTF-8 や切断 JSON は
   `Read` ではなく `Parse` に分類される (`rename_key_migration.rs:282-288`)。確認後に内容が
   変わった場合は `Changed` として採用せず、元の保護を維持する (`app.rs:32615-32628`)。
5. **文言**: 上表 RB-8 のとおり全文日本語。
6. **終了時**: `finish_rename_migration_quarantine_for_exit` が cancel + join し、
   すでに linearize 済みの rename は成功として統合してから最終 flush する
   (`app.rs:32653-32670`, `32246-32257`)。新しい blocking 警告画面は開かない。

**残件 (P3 相当、出荷を止めない)**: 決定的な `Read` 失敗 (journal パスがディレクトリである、
ACL で読めない等) は現状も無限再試行のままで、退避の出口が無い。発生条件が極めて限定的で、
RB-1 が問題にした「壊れた JSON」は `Parse` 側に分類されるため、今回の修正で実害の大半は
塞がっている。

## C. サイドカー確認の再利用 — 確認結果

- 別綴り到達は `KeyMismatch` で strict probe へ戻る (上表 S2-3)。
- **復元省略条件は緩んでいない**。`revalidate_disk_import_source_for_reuse_from_with`
  (`sidecar.rs:358-393`) は ① pending writer ② failed writer ③ `folder_key` 一致
  ④ len/mtime (短絡用の安価な判定のみ) ⑤ **全 bytes ダイジェスト** (`token.revalidate`) を
  この順で必ず通す。len/mtime 一致だけで復元を省略する経路は無い。
- `probe_reuse.rs:142-166` が revalidate → 両 family の marker 照合 → **再度 revalidate**
  (after-marker の linearization point) を行い、各段で cancel を見る。marker 不一致・
  読み取り失敗・dirty・`source_validation_error` あり・予算超過は proof にしない
  (`probe_reuse.rs:283-292`)。

## 使用した主な根拠ファイル

- `C:\home\mimageviewer\src\collection_store\db.rs`
- `C:\home\mimageviewer\src\collection_store\runtime.rs`
- `C:\home\mimageviewer\src\collection_store\model.rs`
- `C:\home\mimageviewer\src\db_backup.rs`
- `C:\home\mimageviewer\src\app.rs` (32098-32760, 74078, 74769)
- `C:\home\mimageviewer\src\ui_dialogs\rename_migration_recovery.rs`
- `C:\home\mimageviewer\src\rename_key_migration.rs`
- `C:\home\mimageviewer\src\tray_integration.rs`
- `C:\home\mimageviewer\src\sidecar_import\probe_reuse.rs`
- `C:\home\mimageviewer\src\sidecar.rs`
- `C:\home\mimageviewer\src\app\sidecar_restore.rs`
