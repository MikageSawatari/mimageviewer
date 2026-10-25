# 描画処理の失敗で終了するとき、理由を知らせて記録する (§1.428)

2026-10-24 起票・設計 (ClaudeCode)。第 2 版 (2026-10-25): 独立設計レビュー (gpt-6.1-sol / xhigh、ACCEPT WITH CHANGES、
P2 4 件・P3 4 件) を取り込んだ。実装は Codex、差分の独立レビューは通常どおり。

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

- **DX12 が失敗したら Vulkan / FXC 固定などで自動的にやり直す**: 失敗する環境を観測しておらず、どの再試行が効くか確かめられない。
  確かめられない分岐は足さない。報告で原因が分かってから判断する。
- **launcher 側で core の終了コードを見て表示する**: ポータブル版に launcher が無く、エラーの種類も持たない。
- **依存ライブラリのログを常時保存する**: §4 のとおり。
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
