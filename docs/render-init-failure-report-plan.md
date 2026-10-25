# 起動時の描画初期化の失敗を、理由つきで知らせて記録する (§1.428)

2026-10-24 起票・設計 (ClaudeCode)。実装は Codex、独立レビューは通常どおり。

## 1. 背景と目的

- X で「4.8.0 から起動しなくなった」という投稿があった (症状・環境は不明)。v4.8.1 は、Windows Sandbox (VC++ なし) の
  GPU あり / GPU なし (WARP) の両方で、初回と 2 回目の起動に成功している (ClaudeCode が 2026-10-25 に観測、
  `C:\miv-sandbox\results\v4.8.1-20261025-*`)。投稿者の原因は分かっていない。
- コードの調査 (Codex read-only、2026-10-25) で確かになった弱点: **`eframe::run_native` が `Err` を返すと、
  mIV は何も表示せずに終了し、エラーの中身もどこにも残らない**。
  - `src/lib.rs` の `run()` は `run_result` をそのまま返し、`src/main.rs` の `main` が `eframe::Result` を返して終わる
    (`windows_subsystem = "windows"` なので stderr も見えない)。
  - 製品の logger (`src/logger.rs` の `ProductDiagnostics`) は `tantivy::*` と `egui_wgpu::surface_window_limit` の Warn 以上
    しか保存しないので、eframe / egui-wgpu / wgpu が出す失敗のログも残らない。
  - 起動の段階 (startup-logs の `RunNative` の成否など) は残るが、何に失敗したかの文は残らない。
- v4.8.0 で wgpu 27→30・eframe 0.33→0.37 へ入れ替えたので、この経路に入る環境が増えた可能性がある
  (例: wgpu 30 の DX12 shader compiler が `Auto` になり、読めるが使えない DXC があると DX12 の初期化が失敗する)。
- 目的: 利用者が「起動しない」ではなく理由を見られること、問い合わせを受けたときに原因を絞れること。
  **描画の初期化そのものを変える (backend の再試行・fallback) のは今回の範囲外** (§6)。

## 2. 変える挙動 (利用者から見て)

- 起動時に描画の準備に失敗して `run_native` が `Err` を返したとき、**終了する前にメッセージボックスを 1 回出す**。
  - 題名: `mImageViewer` (ポータブル版は既存の `data_dir.rs` に合わせて `mImageViewer (ポータブル版)`)。
  - 本文 (案。文言はレビューと実装で整えてよい。固定文言に絵文字・環境依存の記号を使わない):
    ```
    画面の表示を始められなかったため、mImageViewer を終了します。

    グラフィックスドライバーを最新版に更新すると直ることがあります。
    直らない場合は、次のフォルダーのログを添えてお知らせください。
    <logs フォルダーの絶対パス>

    詳細: <エラーの種類と文 (1〜数行に丸める)>
    ```
  - 描画以外の理由で `Err` を返す既存の経路 (例: `RuntimePin` の `AppCreation`) は、本文の 1 行目を
    「起動の準備に失敗したため、mImageViewer を終了します。」にする (ドライバー更新の案内は出さない)。
- 正常終了 (`Ok`) では何も出さない。
- **`test-script` の自動実行 (`scripted_run`) ではメッセージボックスを出さない** (自動化が止まるため)。記録だけ行う。

## 3. 記録する内容

- `mimageviewer.log` に 1 行以上: `startup: run_native failed kind=<種類> error=<Display と source の連鎖>`。
  種類は eframe 0.37 の `eframe::Error` の variant から決める (`Wgpu` / `Winit` / `WinitEventLoop` / `Glutin` / `NoGlutinConfigs` /
  `OpenGL` / `AppCreation` / その他)。`Wgpu` は `egui_wgpu::WgpuError` の variant (RequestAdapter / CustomNativeAdapterSelection /
  NoSurfaceFormatsAvailable / RequestDevice / CreateSurface / HandleError / WindowSizeLimit) まで書く。
- startup-logs (`miv_startup`) の終端記録に、同じ種類を detail として残す (既存の `record_detail` 等の API に合わせる)。
- 製品 logger の保存対象を広げ、**`eframe` / `egui_wgpu` / `wgpu_core` / `wgpu_hal` / `wgpu` の Error レベル**だけを
  `mimageviewer.log` に残す (Warn は残さない。描画中に毎フレーム出る警告でログが膨らむのを避けるため)。
  Error レベルでも毎フレーム出る可能性があるなら、レビューで上限の要否を判断する。
- ログは `finish_logs()` の前に書き、メッセージボックスはログを書き終えてから出す (ボックスを閉じずに強制終了されても記録は残る)。

## 4. 所有と配置

- 判断と文面の組み立ては 1 か所の純粋な関数にまとめる (例: `startup_failure_report(&eframe::Error) -> Report { kind, log_line, dialog_body }`)。
  単体テストで種類ごとの分類と本文を固定する。
- 表示は `src/lib.rs` の `run()` の `run_result` が `Err` のときの経路で 1 回だけ。表示は注入できる形にし
  (テストでは記録用の fake)、本番では `MessageBoxW` (owner なし、`MB_OK | MB_ICONERROR`)。
  既存の `data_dir.rs::show_fatal_messagebox` と同型の helper を共有できるなら共有する。
- logs フォルダーのパスは `data_dir::get()` から作る (ポータブル版は `<exe_dir>\data\logs`)。data_dir が未確定の段階の失敗は
  この経路に来ない (それより前に既存の data_dir のエラー表示がある) ことを確かめる。
- launcher は変更しない (core が自分で表示するので、単体 exe 版・インストーラ版・ポータブル版のすべてで同じ挙動になる)。
- creator closure の中の失敗 (mIV 自身の初期化) が `AppCreation` で返る経路も同じ報告に乗る。panic (panic.log)、
  描画ドライバー内のハング (UI heartbeat watchdog が panic.log に書く) は今回の対象外。

## 5. テスト

- 分類と本文: 各 `eframe::Error` / `WgpuError` の代表値 (作れるもの) で kind・ログ行・本文の 1 行目を確かめる。
  作れない variant は、分類の match が網羅していることをコンパイルで担保する (`_` で黙って丸めない)。
- 経路: `run_result` が `Err` のときに、記録 → 表示の順で 1 回だけ呼ばれること、`Ok` では呼ばれないこと、
  `scripted_run` では表示が呼ばれないこと。表示は fake で受ける。
- logger の保存対象: 追加した target の Error は保存され、Warn と無関係な target は保存されないこと。
- 実機: 描画の初期化を失敗させる手段 (例: 開発用の環境変数 `WGPU_BACKEND` で使えない backend だけを指定する、
  あるいは test 用の注入) があれば、隔離 portable で 1 回だけ、ボックスとログを ClaudeCode が確かめる。
  手段が無ければ、単体と経路のテストで済ませ、その旨を記録する。

## 6. 採らない案と理由

- **DX12 が失敗したら Vulkan、FXC 固定などで自動的にやり直す**: 失敗する環境を一度も観測していないので、どの再試行が
  効くか確かめられない。確かめられない分岐は足さない (feedback: テストできない分岐を残さない)。報告で原因が分かってから判断する。
- launcher 側で core の終了コードを見て表示する: ポータブル版に launcher が無く、core の中の情報 (エラーの種類) も持たない。
- ログを常時 Warn まで広げる: 通常の描画中の警告で `mimageviewer.log` が膨らむ。

## 7. 文書

- `htdocs/mimageviewer/manual/troubleshooting.html` に「起動時に『画面の表示を始められなかった』と出る」場合の案内
  (ドライバー更新・ログの場所・連絡方法) を足す。
- `docs/startup-diagnostics-plan.md` と `docs/ui-responsiveness.md` は必要に応じて参照を足す。
