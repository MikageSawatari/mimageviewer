# 描画処理の失敗で終了するとき、理由を知らせて記録する (§1.428)

2026-10-24 起票・設計 (ClaudeCode)。第 2 版 (2026-10-25): 独立設計レビュー (gpt-6.1-sol / xhigh、ACCEPT WITH CHANGES、
P2 4 件・P3 4 件) を取り込んだ。実装は Codex、差分の独立レビューは通常どおり。

2026-10-26 追補: GPU 初期化と起動中の限定ログ収集は [§1.448 の計画](gpu-init-fallback-plan.md) で拡張する。
以下の起票・実装記録は §1.428 単独の範囲を示す。有限の API 選択と診断追加の現行方針は §6 と後継計画を参照。

## 1. 背景と目的

- X で「4.8.0 から起動しなくなった」という投稿があった (症状・環境は不明。投稿者には連絡しない = 利用者決定)。
  v4.8.1 は Windows Sandbox (VC++ なし) の GPU あり / GPU なし (WARP) の両方で、初回と 2 回目の起動に成功している
  (ClaudeCode が 2026-10-25 に観測、`C:\miv-sandbox\results\v4.8.1-20261025-*`)。投稿者の原因は分かっていない。
- コードで確かな弱点: **`eframe::run_native` が `Err` を返すと、mIV は何も表示せずに終了し、エラーの文もどこにも残らない**。
  - `src/lib.rs::run()` は `run_result` をそのまま返し、`src/main.rs::main` が `eframe::Result` を返して終わる
    (`windows_subsystem = "windows"` なので stderr も見えない)。
  - 製品の logger (`src/logger.rs::ProductDiagnostics`) は `tantivy::*` と `egui_wgpu::surface_window_limit` の Warn 以上しか保存しない。
  - startup-logs には段階 (`RunNative` の成否) は残るが、何に失敗したかの文は残らない。
- `run_native` の `Err` は初期化専用ではない。イベント処理中の失敗 (例: fallible な surface 設定) も同じく返る
  (`vendor/eframe/src/native/run.rs:553`、`wgpu_integration.rs:863`)。なので文面は「起動時」に限定しない。
- 目的: 利用者が理由を見られること、問い合わせで原因を絞れること。**描画の初期化そのものを変える (backend の再試行・fallback) のは範囲外** (§6)。

## 2. 対象

- 対象は **`run()` の `run_native` が `Err` を返した経路だけ**。
- 対象外 (理由): `RuntimePin` の早期 return (`src/lib.rs:1132`、data_dir・logger の初期化前で別の契約が要る)、
  panic で終わる経路 (panic.log に残る。§6)、data_dir の既存のエラー表示と `process::exit`、CLI / worker の `exit_startup`、
  描画ドライバー内のハング (heartbeat watchdog が panic.log に書く)。

## 3. 終了の順序 (固定)

`run_native` 復帰 → **heartbeat watchdog を既存 API で休止** → 失敗の記録 (`logger::log` と startup の `record_detail`) →
既存の runtime cleanup (`src/lib.rs:2014` 付近、remote service 等の後片付け) → `miv_startup::terminal(Error)` → `finish_logs()`
(既存の合計 2 秒上限の flush を再利用) → **test-script の自動実行でないときだけ**メッセージボックス → 元の `Err` を返す。

- 表示はログの flush の後に置く。保存失敗への再試行・同期 fallback は足さない。「強制終了されても必ず残る」保証にはしない
  (まれな失敗は既定で割り切る)。
- watchdog を止めないと、GPU 初期化失敗では main HWND が未捕捉のまま、ボックスを開いている間に偽の `UI THREAD HANG` を
  繰り返し記録する (`src/lib.rs:682`)。
- single-instance mutex はボックスを閉じるまで保持してよい。winit の event loop は `run_and_return` で解除済みなので、
  ボックスを別スレッドへ移す必要はない。
- test-script の判定は既存の `scripted_run` (`src/lib.rs:1183`、解析済み script path の有無) を使う。feature が有効なだけの
  通常起動では抑止しない。feature 無効のビルドでは常に表示する。

## 4. 分類・記録・文面

- 純粋な関数 1 つにまとめる (例: `run_native_failure_report(&eframe::Error, logs_dir_display: &Path) -> Report { kind, log_detail, dialog_body }`)。
- 分類は `eframe::Error` (現構成で 7 variant) と `egui_wgpu::WgpuError` (7 variant: `WindowSizeLimit` / `RequestAdapterError` /
  `CustomNativeAdapterSelectionError` / `NoSurfaceFormatsAvailable` / `RequestDeviceError` / `CreateSurfaceError` / `HandleError`) の
  **cfg に合わせた網羅 match**。`_` で丸めない。
- **原因の連鎖は variant ごとに内包エラーを明示的に取り出して辿る**。`eframe::Error` の `Error` 実装は `source()` を公開しない
  (`vendor/eframe/src/lib.rs:545`) ので、外側からの汎用走査では辿れない。ログには最深部まで全文、ダイアログだけ短く丸める。
- ログ行: `run_native failed: kind=<種類> detail=<連鎖>` (`mimageviewer.log`)。startup-logs にも kind を detail として残す
  (`terminal()` の前。`terminal()` 後の `record_detail` は採用されない、`crates/startup-diagnostics/src/lib.rs:214`)。
- **依存ライブラリ (eframe / egui_wgpu / wgpu*) のログ保存は広げない**。Error でも毎フレーム出る経路があり (wgpu-core `present.rs:272`)、
  ログの書き換え頻度と古い診断の押し出しが増えるため。既存の `tantivy::*` と `egui_wgpu::surface_window_limit` の Warn 保存は維持する。
  限界: wgpu 30 の DXC 初期化失敗の理由は wgpu-core の Debug ログにしか出ない (`instance.rs:138`) ので、今回の記録では取れない。
- 文面 (案。文言は実装で整えてよい。固定文言に絵文字・環境依存の記号を使わない):
  - `Wgpu` / `Winit` (描画の準備・surface・adapter・device):
    ```
    画面の表示処理に失敗したため、mImageViewer を終了します。

    グラフィックスドライバーを最新版に更新すると直ることがあります。
    直らない場合は、次のフォルダーのログを添えてお知らせください。
    <logs フォルダーの絶対パス>

    詳細: <種類と短い文>
    ```
  - `AppCreation` / `WinitEventLoop` / その他: 1 行目を「mImageViewer の実行中に問題が起きたため、終了します。」にし、
    ドライバー更新の案内は出さない (GPU が原因と断定しない)。
- 題名: `mImageViewer` (ポータブル版は既存の `data_dir.rs::show_fatal_messagebox` と同じ `mImageViewer (ポータブル版)`)。
- ログの場所は `data_dir::get().join("logs")` を、ファイルの存在に依存しない方法で**絶対パス化**して文面生成へ渡す
  (`--data-dir` の相対指定がそのまま保存されるため、`src/data_dir.rs:28`)。通常版の startup-logs は別の
  `%LOCALAPPDATA%\mimageviewer\startup-logs` にあるので、文面とマニュアルで混同しない。

## 5. 表示

- `MessageBoxW` (owner なし、`MB_OK | MB_ICONERROR | MB_SETFOREGROUND`)。`run_native` 復帰後に owner HWND を探し直さない。
  前面化のための HWND 探索・再試行ループは作らない。
- 表示関数は注入できる形にする (テストでは記録用の fake)。`data_dir.rs::show_fatal_messagebox` と helper を共有できるなら共有する。

## 6. 採らない案と理由

- **起動失敗時の自動再試行（2026-10-26 方針更新）**: §1.428 では採らなかったが、後継の
  [§1.448](gpu-init-fallback-plan.md#52-claudemd-との関係) で有限の API 選択を採用する。
  既定の `auto` で初回の DX12 `request_device` が失敗したときだけ、同じ window / surface の Vulkan 候補を一度試す。
  両方失敗・候補なしなら終了する。明示指定、他の初期化段階、実行中の device lost、panic / native exception は再試行しない。
  DX12 compiler は FXC に固定し、DXC → FXC → Vulkan の多段再試行は作らない。各試行の結果と採用 API をログ・起動記録に残し、
  成功 API は保存しない。連絡できない報告者のスクリーンショットから確かなのは root の `request_device` 失敗までで、
  backend も原因も未確定である。原因修正の証明ではなく、有限の API 選択による起動互換性の向上として扱う。
- **launcher 側で core の終了コードを見て表示する**: ポータブル版に launcher が無く、エラーの種類も持たない。
- **依存ライブラリのログを常時保存する**: 引き続き採らない。§1.448 では、Instance 初期化前から最初の `RootPresented` まで
  （Err 復帰時は cleanup 前まで）に限り、`wgpu_hal` / `wgpu_core` / `gpu_allocator` の Error を保存する。
  crate 直下と子 module を含め、global 上限 Warn と既存の保存対象を維持する。別スレッドの行はログへ残すだけで root 試行へ帰属させない。
  各試行の確定結果・採用後の root 状態を一つの診断 owner で保持し、`End(false)` だけで致命的失敗とは決めない。
- **起動中の panic も同じ報告に乗せる**: panic 経路 (例: surface 非対応時の `vendor/egui-wgpu/src/winit.rs:165` の expect、
  wgpu の既定 uncaptured-error handler) は `Err` にならない。別項目 §1.429 で扱う。

## 7. テスト

- 分類と文面: 作れる variant の代表値で kind・ログ詳細・本文 1 行目・案内の有無を確かめる。`AppCreation` に二段以上の source を
  持つ独自エラーを入れ、最深部の文がログ詳細に残る回帰テスト。作れない variant は網羅 match でコンパイル時に担保する。
- 絶対パス: 通常・portable・相対 `--data-dir` のそれぞれで、表示用パスが絶対になること。
- 経路: **テスト専用の結果注入** (cfg(test) か test-script 限定の、`run_native` の結果を `Err` に差し替える入口) で、
  watchdog 休止 → 記録 → cleanup → `terminal(Error)` が 1 回 → `finish_logs` 完了の後に fake 表示へ 1 回入ること、
  `Ok` では表示されないこと、`scripted_run` では記録・flush を維持して表示だけ抑止されること、元の `Err` (非ゼロ終了) が返ることを検査する。
  `WGPU_BACKEND` 環境変数は mIV が backend を `DX12 | VULKAN` に上書きする (`src/lib.rs:1643`) ので、失敗の起こし方に使えない。
- ビルド: 通常 / portable / test-script の有無でコンパイルが通ること。
- 実機: 実際のボックスの見た目は、注入の入口を使って隔離 portable で確かめる。エージェントの画面操作は
  [対話検証規則](interactive-release-verification.md) の個別了承の後に行う。

## 8. 文書

- `htdocs/mimageviewer/manual/troubleshooting.html` に、この表示が出たときの案内 (ドライバー更新・ログの場所・連絡方法) を足す。
  通常版の startup-logs (`%LOCALAPPDATA%`) とログ (`%APPDATA%\mimageviewer\logs`) の場所を混同しない。
- 必要に応じて `docs/startup-diagnostics-plan.md` に参照を足す。detached 凍結規則には触れない (`run()` の終端・logger・native helper のみ)。

## 9. 実装・検証記録（2026-10-25）

対象は `render-init-1428`、開始HEAD `ebc4e1c67` の未コミット差分。製品exeの起動・commitはしない。
§1.429のpanic、adapter selector、backend選択、依存ライブラリのログ保存対象は変更していない。

### 実装と第2版からの具体化

- `src/run_native_failure.rs` に分類・文面の純粋関数と、製品・fake共通の終了境界を置いた。
  現在のCargo構成はglow/wgpuの両方を有効にするため、eframeの7種とWgpuErrorの7種を網羅し、
  native/Windows専用variantには依存元と同じtarget cfgを付ける。
- `run()` の既存cleanupを `FnOnce` としてそのまま渡す。新しいworkerや非同期状態は作らず、
  event loop復帰後の直列処理に統一することで組み合わせを増やさない。既存script成功時の
  exit code判定・terminal・flush・process exitはcleanupの末尾に残す。
- 文面へ渡すログの場所は `std::path::absolute` で解決し、存在確認・canonicalizeを行わない。
  解決自体のまれな失敗は、元Errをpanicで置き換えず、解決失敗をログと通知へ加えて終了する。
  保存/解決失敗へのretry・同期fallbackは足さない（既定の割り切り）。
- `data_dir::show_fatal_messagebox` を通常版でも使える共通helperにした。ownerなし・
  `MB_OK | MB_ICONERROR | MB_SETFOREGROUND`、build flavorごとの題名を共有する。
- 結果注入は `cfg(test)` のfake harnessで共通終了境界へErrを渡す方式を採った。
  通常版/test-script版とも、実行引数・環境変数などの故障注入入口は追加していない。
  そのため実ダイアログの外観を注入で確認するには、別途隔離テスト用の準備が必要。
- 公開案内の起動記録パスは実装で照合した。ポータブル既定は `data/logs/startup`、
  明示 `--data-dir` では指定先の `logs/startup`。通常版既定の `%LOCALAPPDATA%/mimageviewer/startup-logs`
  と区別して記載した。

### 回帰検出と独立レビュー

- `failed_run_records_flushes_then_shows_and_returns_original_error` を、共通終了境界の本体が
  旧動作相当（cleanup→terminal→flushだけ）の段階で実行し、**exit 101 / 1 failed** を確認。
  実際のイベントは `[cleanup, terminal:Error, flushed]` で、要求する休止・記録・表示が無い。
  コマンド: `cargo test -p mimageviewer --lib run_native_failure::tests::failed_run_records_flushes_then_shows_and_returns_original_error`。
  記録: `target/render-init-red.log`。
- 実装後の最初の9件は8成功/1失敗（exit 101）。HandleErrorの依存Displayが `not available` のところを
  `unavailable` と期待したテスト側の誤りで、実際の全文記録・分類・案内は正しかった。
  期待値を依存sourceどおり修正し、短縮テストの改行/NULを短縮より前へ移して検査を強化した。
  記録: `target/render-init-tests.log`。
- 独立レビュー: GPT-6.1 Sol / xhigh、最終コードと文書を確認して **ACCEPT / P1-P3追加指摘なし**。
  テストは実装担当が所有し、レビュー担当は重複実行していない。
- 最終scope確認で、watchdog休止はErrのreport構築先頭だけに限定した。§2の対象外である
  Ok終了時のwatchdog動作は従来どおりとする最小調整で、独立レビュアーも同意。
  関連23件の成功後にこの調整を行ったため、影響する新規9件だけを再実行して成功した。

### 最終自動検証

初回の依存ビルドは `ffmpeg-sys-the-third` のlibclangでアクセス違反
（runner exit 1、build script STATUS_ACCESS_VIOLATION）になり、テストには未到達。
`CARGO_BUILD_JOBS=2` に抑えた再実行で通過した。focusedテストも並列数2。
テスト用FFmpeg DLLはこの作業ツリーの `vendor/ffmpeg/bin` から `target/debug/deps` へ実体コピーした。

通常core checkの初回は `turbojpeg-sys` のCMake/MSBuildがexit 1で止まり、Cargoはexit 101。
本体の型検査前で、詳しいコンパイルエラーは無い（`target/render-init-check-default-initial-failed.log`）。
同じ生成済みbuild directoryで
`cmake --build target/debug/build/turbojpeg-sys-4adae978226e4e06/out/build --target install --config Debug --parallel 1`
を実行し、exit 0で完了した。停止理由は未特定。以後のcheck/buildは `CARGO_BUILD_JOBS=1` とする。
最後のscope調整で成功テストのassertの行折りが変わり、fmt checkは一度exit 1。
`cargo fmt` でその1箇所を揃え、最終 `cargo fmt --check` はexit 0。テストの意味は変更していない。

| コマンド | 結果 | 記録 |
| --- | --- | --- |
| `.\scripts\test-focused.ps1 -Path @('src/run_native_failure.rs', 'src/data_dir.rs', 'src/logger.rs')` | exit 0、新規9件 + data_dir 9件 + logger 5件、計23件成功 | `target/render-init-focused.log`、`target/test-focused/runs/20261025T014218206Z-532481ef/` ほか同logのrun一覧 |
| `.\scripts\test-focused.ps1 -Path src/run_native_failure.rs` | exit 0、scope調整後の新規9件成功。data_dir/loggerの成功結果は変更影響がないため再利用 | `target/render-init-focused-final.log`、`target/test-focused/runs/20261025T014522417Z-89bacc0b/` |
| `cargo fmt --check` | exit 0 | command runner出力 |
| `python scripts/check_ui_glyphs.py` | exit 0、危険glyph 0件 | command runner出力 |
| `python scripts/check_site_cards.py` | exit 0、49枚、上限超過0件 | command runner出力 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0（依存の直列build後の再実行） | `target/render-init-check-default.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core --features portable` | exit 0 | `target/render-init-check-portable.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core --features test-script` | exit 0 | `target/render-init-check-test-script.log` |
| `.\scripts\build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | exit 0、通常featureのcore・remote・EPUB workerを生成、FFmpeg/VC runtime DLLとEffeTune bundleを配置。PE検査runtime=4 / pe=3成功 | `target/render-init-build-dev.log`（phase transcript）、command runner出力、`target/vcrt-pe-reports/dev-runtime.json` |

3構成のcheckコマンド/exit codeの機械可読記録は `target/render-init-check-results.json`。
確認用buildはcold cache・並列数1で、core 38m53s / remote 3m30s / EPUB worker 2m11s。
成果物は `target/dev-runtime/mimageviewer-core.exe` と同じ場所のcompanion 2本。
`portable` featureは有効にしていない。`-PreserveRuntime` により稼働中アプリの停止は行わず、
エージェントは生成した製品exeを起動していない。

実機外観・前面化・実GPU障害での通知は未実施。製品exeは起動していない。
lib全件は利用者指定どおりClaudeCodeが後で実施する。
