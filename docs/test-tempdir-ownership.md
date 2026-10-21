# テスト用一時領域の所有と終了 (2026-10-21)

対象基点: `materializer-test-tempdir` / `5cd996fde`。製品の保存先・DB schema・
`settings_db::with_db` の `SaveSuppressed` 防御は変更しない。

## 既定 data_dir の判断

`cfg(test)` の既定値は `tempfile::Builder` の乱数名で一度だけ作成する。
`OnceLock<Mutex<Option<TempDir>>>` がプロセス共通の所有者で、同プロセスの別スレッド・
override 解除後も同じパスを返す。`libc` は終了フック用の dev-dependency のみ。

より単純なローカル変数・thread-local の `TempDir` では、最初の呼出元より長生きする
worker を保護できない。static の `TempDir` だけでは通常終了でも Drop されない。
そこで CRT `atexit` で所有者から `TempDir` を一度取り出して破棄する。
worker 停止・SQLite 接続の全収集・再試行の状態は作らず、終了時にロックを取れない場合や
削除失敗、異常終了の残存は許容する。残存しても後続プロセスは乱数名で隔離される。
`%TEMP%` の全体走査や過去の残存領域の一括削除は行わない。

回帰テストは同じ unit-test executable の子プロセスを2回起動し、最初の呼出スレッドが
終了した後の存在、プロセス内の同一性、override 優先と解除後の同一性、プロセス間の
一意性、通常終了後のファイルごとの回収を確認する。製品バイナリは起動しない。

## 個別テストの変更

| 対象 | 所有と削除順 |
| --- | --- |
| `src/adjustment_db.rs` 3件、`src/mask_db.rs` 1件 | 接続より先に `TempDir` を束縛し、接続 Drop 後に領域を削除。開いた接続のままの `remove_file` を撤去 |
| `src/folder_rating_counter.rs` 1件 | `TempDir` 内の DB。テスト時だけ handle が worker の JoinHandle を所有し、cancel → join → 領域削除。panic 時にも同順序。本番の handle Drop は引き続き待機しない。50ms sleep と個別ファイル削除を撤去し、`TempDir::close` 成功も検査 |
| `src/folder_thumb_pins.rs` 9件 | resolve / cascade 用の親領域を `TempDir` で所有 |
| `src/video_chapter_thumbs.rs` 1件 | ダミー動画を `TempDir` 内へ移動 |
| `crates/epub-pdf-worker/src/render.rs` 7件 | PDF fixture と出力を同じ `TempDir` で所有 |
| `src/comic_db.rs` 9件、`src/conceal_db.rs` 9件 | PID＋時刻で衝突回避済みだったが、開いた接続のままの削除を発見。helper を `(TempDir, Db)` にし、束縛順で接続を先に Drop |
| `src/ai/runtime.rs` 1経路 | DLL 不在検査の PID 固定パスを、所有する一意な領域内の不存在パスに変更 |
| EPUB worker `src/paths.rs` 3件、`src/main.rs` 1件、`tests/progress_cli.rs` 4件 | PID＋時刻から `TempDir` 所有へ統一。不在を前提にする子ディレクトリは、領域の下の未作成パスを使って従来の前提を維持。cleanup 動作そのものの検査は保持 |

## `rg 'process::id\(\)' src crates` の洗い直し

上表に PID 固定名のテスト用ディスク領域を残していない。残す箇所は次のとおり。

| 残す箇所 | 理由 |
| --- | --- |
| `src/keymap.rs` の `miv-keymap-test-*` 2件 | PID＋ナノ秒時刻で一意化し、正常時は明示削除済み。panic 時の回収だけが未対応で、PID 固定名の衝突や開いた DB 接続の問題はないため今回は維持 |
| `src/logger.rs::unique_test_dir` (2件が使用) | 同じく PID＋ナノ秒時刻、正常時に明示削除。panic 時だけの残存を今回の必須範囲外として維持 |
| `src/ai/trt_worker_shm.rs` 3件 | ディスク領域ではなく pagefile backing の named mapping。RAII で Unmap / Close、プロセス終了でも OS が解放。同名再作成検査の意味を維持 |
| `src/video/dsp/bridge.rs::existing_named_event_is_rejected` | named event の重複拒否を検査。永続ファイルではなく HANDLE を閉じる対象 |
| `crates/remote-web/src/ipc_client.rs` のテスト用 named pipe | サーバースレッドの RAII HANDLE で所有するカーネル資源。ディスク上には残らない |
| `crates/startup-diagnostics/src/output.rs` のテスト内のログパス | 既に `TempDir` の下。PID は製品のログ名と照合するため |
| `src/materializer.rs` の test constructor | `5cd996fde` で共有 `MaterializerInner` が乱数名 `TempDir` を所有済み。PID はその下の製品と共通のレイアウト要素 |
| `src/runtime_cleanup.rs`、`crates/startup-diagnostics/src/lib.rs`、EPUB worker `src/job_probe.rs` のテスト | 現プロセスの照合・イベント値・command line 検査。テスト領域名ではない |
| `src/log_writer.rs`、`src/log_writer/tests.rs` のテスト | 既存の領域 owner 配下のログに PID メタデータを出すだけ |
| `src/bin/bench_search.rs` の `bench_search_{pid}` | `dev-tools` の手動ベンチマークで、lib / nextest のテストではない。`--keep` による成果物保持を持つため対象外 |
| その他の一致 | 製品の領域・IPC 名・診断・process ownership。`app.rs`、`clipboard_capture/fetch.rs`、`diagnostics.rs`、`epub_convert.rs`、`similar_search_array.rs`、`rename_key_migration.rs`、`ui_dialogs/context_menu.rs`、AI / video runtime、`runtime_locks.rs`、`runtime_cleanup.rs`、`logger` / `log_writer` / `perf` / `gpu_info`、`test_script` の実機診断、EPUB worker / startup-diagnostics / vst3-host-tester の実装経路。テスト専用の固定ディスク領域ではないため変更しない |

## 検証記録

環境変数は各 Cargo / MSBuild 呼出しで `MSBUILDDISABLENODEREUSE=1`。
対象は本基点からの未コミット差分、通常 feature / test profile。

- `cargo nextest run -p epub-pdf-worker --no-tests fail --retries 0 --no-fail-fast`:
  exit 0、62件成功、0 skipped。run ID `88f04063-b1f6-498d-b8d2-8c693c14b408`。
- 本体 `cargo check -p mimageviewer --lib --tests` 初回:
  `turbojpeg-sys` の CMake / MSBuild 並列 build が exit 1。Rust の変更箇所の診断前。
  同じ生成済み CMake graph を `--parallel 1 -- /nr:false /v:diag` で build し、
  exit 0・0エラーを確認。製品や build 設定の変更はしていない。
- `cargo check -p mimageviewer --lib --tests -j 1`: exit 0、5分03秒。
  ログ `target/tmp/test-tempdir-check.log`。変更対象外の既存 warning あり。
- `cargo fmt` / `cargo fmt --check`: exit 0。
- 本体 focused nextest: exit 0、344件成功、13,592件は filter 除外。
  run ID `3372eba7-8944-4624-b091-375284f2dd8c`、実行13.562秒
  (test executable build は4分17秒)。変更した9モジュールと既存の
  `settings_db` / `materializer` を検証。追加した data_dir 子プロセス回帰と
  rating worker 終了後の `TempDir::close` も成功。
  ログ `target/tmp/test-tempdir-focused.log`、JUnit
  `target/tmp/test-tempdir-focused.junit.xml`。
- EPUB の JUnit は `target/tmp/test-tempdir-epub.junit.xml`、検証対象の変更された
  Cargo / Rust 入力15ファイルの SHA-256 は `target/tmp/test-tempdir-sources.sha256`。
- 本体全件 `test-lib-full.ps1` は依頼どおり ClaudeCode が担当。
- テスト支援とテスト fixture のみの変更のため、実行アプリ用の検証 build は不要。
  mIV 本体や通常 profile の起動・既存一時領域の削除・git commit は行わない。
  子プロセスの起動は unit-test executable と要求された EPUB CLI 自動テストの範囲だけ。

本体 focused の再現コマンド (通常 feature / test profile):

```powershell
$env:MSBUILDDISABLENODEREUSE = '1'
cargo nextest run -p mimageviewer --lib -E 'test(~data_dir::) | test(~adjustment_db::) | test(~mask_db::) | test(~folder_rating_counter::) | test(~folder_thumb_pins::) | test(~video_chapter_thumbs::) | test(~comic_db::) | test(~conceal_db::) | test(~ai::runtime::) | test(~settings_db::) | test(~materializer::)' --ignore-default-filter --run-ignored default --retries 0 --no-fail-fast --no-tests fail
```
