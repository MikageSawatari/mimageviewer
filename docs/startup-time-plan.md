# 起動時間の調査と短縮プラン

対象: v3.1.0 (調査は v3.0.0 のインストール版 + 開発機の実プロファイルで実測、2026-08-18)

## 0. 結論 (先に要点)

- **ウィンドウが出るまで ≒ 1.0 秒、実際のグリッドが出るまで ≒ 1.4 秒** (開発機・warm)。
- 内訳は「**eframe/wgpu の GPU 初期化 450–685ms**」「**IndexerManager 待ちの起動
  オーバーレイ 370ms**」「settings.db 読み込み ~95ms」「フォント 40ms」「DB 25 本
  オープン 30ms」で説明が付く。**画像を 1 枚も読んでいない時間**が支配的。
- eframe は **最初のフレームを描くまでウィンドウを表示しない**
  (`vendor/eframe/src/native/epi_integration.rs:309` の `post_rendering` →
  `set_visible(true)`)。つまり現状は「約 1 秒間なにも出ない」。MangaMeeya が
  瞬時に見える最大の理由はここ。
- 遅延・並行化・順序変更だけで **ウィンドウ出現 ~50–100ms / UI 描画 ~400ms** まで
  短縮できる見込み。GPU 初期化そのものは消せないが、**直列区間から外せる**。

## 1. 計測方法 (再現手順)

1. **本体の perf log**: 利用者プロファイルの
   `%APPDATA%\mimageviewer\logs\perf_events*.jsonl` から `"cat":"startup"` を抽出。
   直近 5 セッション分 (すべて warm 起動) を使用。
   - 注意: `--perf-log` を付けずに設定 (`perf_log_enabled`) で有効化した場合、
     `perf::init_with_path` が settings ロード**後**に走るため
     `data_dir_init` / `models_extract` / `susie_worker_extract` / `settings_load` の
     4 イベントが記録されない。この 4 つは通常ログ (`mimageviewer.log`) の
     タイムスタンプで補完した。→ §4-I で改善提案。
2. **プロセスロード時間**: `mimageviewer.exe --version` /
   `mimageviewer-core.exe --version` を `Measure-Command` で 5 回。`--version` は
   `run()` 冒頭で GUI 初期化前に return するので、実質「プロセス生成 + ローダ」だけを
   測れる。
3. **wgpu 初期化の内訳**: 同じ wgpu 27 を使う独立の probe を作り、
   `Instance::new` / `enumerate_adapters` / `request_device` を backends 別に計測。
   同一プロセス内の 2 回目はドライバ DLL が warm になるため、**必ずプロセスを分ける**。

## 2. 実測: 現状の内訳 (開発機 RTX 4090 / warm)

| 区間 | 実測 | 備考 |
| --- | --- | --- |
| プロセス生成 〜 `run()` 入口 | **~72ms** | core exe 304MB + FFmpeg DLL 6 本の静的 import。perf log には出ない区間 |
| `run()` 〜 `before_run_native` | **123–150ms** | うち settings.db 読み込みが ~95ms (§3.2) |
| `before_run_native` 〜 `creator_enter` | **450–685ms** | eframe = winit + **wgpu 初期化**。全体の 6 割 (§3.1) |
| `setup_fonts` | 34–42ms | fallback フォントの実測メトリクス計算 |
| `app_default` (`App::new`) | 33–43ms | うち SQLite 25 本の open が ~27ms |
| `creator_exit` 〜 `first_frame` | 42–44ms | フォントアトラス生成 + 初回パイプライン |
| **first_frame (≒ ウィンドウ表示)** | **711 / 726 / 750 / 788 / 990 ms** | 5 セッションの実測値 |
| first_frame 〜 起動オーバーレイ解除 | **~370ms** | `IndexerManager::new` 待ち (§3.3) |
| **グリッド表示開始 (`load_folder`)** | **~1.37s** | |

launcher (`mimageviewer.exe`) 自体は warm なら ~9ms (`--version` 実測)。
初回のみ 892ms (Defender の初回スキャン等) だった。

## 3. ボトルネックの内訳

### 3.1 wgpu 初期化 (450–685ms) — 最大

probe の実測 (別プロセス・各 3 回以上):

| backends | `Instance::new` | `enumerate_adapters` | `request_device` | 合計 |
| --- | --- | --- | --- | --- |
| DX12 + Vulkan (現状) | 158–204ms | 195–269ms | 62–86ms | **419–558ms** |
| DX12 のみ | 8–12ms | 215–245ms | 61–79ms | **286–331ms** |
| Vulkan のみ | 159ms | 1.8ms | 193ms | 355ms |

判明したこと:

- **Vulkan を候補に入れているだけで ~150ms 損している**。`Instance::new` で
  `vkCreateInstance` (ローダ + レイヤ + ドライバ DLL) が走るため。mIV の
  `native_adapter_selector` は常に DX12 を最優先し、Vulkan は RDP 等の
  フォールバックでしか選ばれない。**平常時に Vulkan instance を作る意味がない**。
- `enumerate_adapters` の 220ms は DX12 アダプタ 3 個
  (NVIDIA / Intel iGPU / Microsoft Basic Render Driver) それぞれに対する
  D3D12 デバイス生成と機能照会のコスト。`request_adapter` に変えても内部で同じ列挙を
  するので **同じ 230ms** (probe で確認済み)。wgpu の公開 API では避けられない。
- `RenderState::create` は **selector を呼ぶ前に必ず `instance.enumerate_adapters()`** を
  実行する (`vendor/egui-wgpu/src/lib.rs:186`)。ここは vendored なので手を入れられる。

### 3.2 settings.db の読み込み (~95ms) — このプロファイル固有だが構造的な問題

開発機の `settings.db` は **38MB**。中身を調べた結果:

| テーブル | 実サイズ |
| --- | --- |
| `settings_kv` (本来の設定 330 件) | **31KB** |
| `vst3_plugins.state` (7 プラグイン) | **33.6MB** |

つまり **設定 DB の 99.9% は VST3 プラグインの状態 blob**。起動時に
`build_settings_from_db` がこれを全部読み、`serde_json::Value` に変換し、さらに
`Settings` へデシリアライズする (3 コピー)。その `Settings` を
`RemoteIpcServer::start(saved.clone())` と `App::new_from_settings(saved.clone(), …)` で
**さらに 2 回 clone** している。

VST3 の state を実際に使うのは起動 1 秒後に走る `kick_off_vst3_startup_load` だけで、
UI が出る前の直列区間で読む理由がない。

> 注: VST3 を使っていない利用者の settings.db は数百 KB で、この区間は実測 6ms 程度
> (`--perf-log` を CLI で付けた開発プロファイルの `settings_load` = 5.5–6.5ms)。
> したがって「利用者一般で 95ms」ではないが、**設定 DB のサイズが起動時間に直結する
> 構造**そのものは直しておく価値がある。

### 3.3 起動オーバーレイ 370ms — 体感上いちばん惜しい

`App::update` は `startup_done` が立つまで **早期 return して起動オーバーレイだけを描き、
入力も捨てる** (`src/app.rs:64541` 付近)。`startup_done` は `IndexerManager::new` の
完了で立つ。開発機ではその中の reconciliation が 313ms、init 全体で 371ms。

`IndexerManager` は全文検索インデックスの管理であって、**フォルダを開いてサムネイルを
出すのに必要ではない**。ここを待つ設計のため、ウィンドウが出てからさらに 0.4 秒
「起動中…」が出続けている。

### 3.4 その他

- `load_icon` 〜 `before_run_native` の ~39ms: `RemoteIpcServer::start` (パイプ +
  ワーカースレッド 20 本) と `saved.clone()`。remote は初回フレームまでに動いている
  必要がない。
- `setup_fonts` 40ms: fallback フォントを `ttf-parser` で実測してベースライン補正を
  計算している。`ui_fonts::prepare_fonts()` という **ワーカースレッドから先に構築して
  キャッシュする API が既にある** のに、起動経路では使っていない。
- `App::new` の 27ms: SQLite を 25 本、直列に open。個々は 0.3–6.5ms。
- `GpuVideoDevice::new()` (D3D11) が creator 内で ~25ms。動画を開くまで不要。
- core の静的 import (FFmpeg DLL 6 本 = 117MB) は warm なら `LoadLibrary` 実測で合計
  ~7ms。72ms の残りは 304MB イメージのリロケーション / ページインとみられる。

## 4. 改善案

効果欄は開発機 warm 実測からの見積り。

| # | 施策 | 見込み | 難易度 | リスク |
| --- | --- | --- | --- | --- |
| **A** | **wgpu の instance/adapter/device を自前で作り `WgpuSetup::Existing` で渡す。作成は `run()` 冒頭で spawn した専用スレッドで行い、`run_native` 直前に join** | 直列区間から **300–450ms** 除去 | 中 | 中 (下記) |
| **B** | **A と同時に backends を DX12 のみにし、失敗時だけ Vulkan で再試行** | **-150ms** | 小 | 小 |
| **C** | **起動オーバーレイのゲートを外す** (IndexerManager 完了を待たずに通常 update へ入り、検索系だけ「準備中」表示にする) | 体感 **-370ms** | 中 | 中 (検索 / インデックス経路が未初期化に耐えるか) |
| **D** | **VST3 state を遅延読み込み**にする (boot では `plugin_path` / `bypass` / GUI 位置だけ読み、`state` は VST3 ロード時に個別取得) | -80ms (VST3 利用時) | 中 | **高**: 保存経路 (`hash_vst3_plugins` / `write_vst3_plugins`) が「未読み込み = 空」を消去と誤認すると **プラグイン状態が消える**。sentinel を型で表すこと |
| **E** | **`prepare_fonts` を settings ロード直後にワーカーで先行実行**し、creator では cache clone のみにする | -35ms | 小 | 小 |
| **F** | **`GpuVideoDevice::new()` と `RemoteIpcServer::start` を初回フレーム後 / ワーカーへ移す** | -50ms | 小 | 小〜中 (remote は起動直後に端末が繋ぎに来るケースで待ちが増える) |
| **G** | **`App::new` の SQLite open を並列化 or 遅延化** | -20ms | 中 | 中 (open 順に依存した migration がないか要確認) |
| **H** | **`Settings` の clone を減らす** (`Arc<Settings>` で渡す) | -10〜30ms | 中 | 小 |
| **I** | **perf の startup 計装を早める** (`prog_start` 直後に `perf_log_enabled` を先読みして init する)。現状は設定 ON だと最初の 4 イベントが落ちる | 計測精度 | 小 | なし |
| **J** | **インストーラ版で launcher を廃止**し、core + remote + FFmpeg DLL を Program Files へ直接配置 (portable と同じ loose 構成、data_dir は APPDATA のまま) | -10〜30ms + APPDATA の 420MB 複製が不要になる | 中 | 中 (インストーラ / 更新 / 署名の手順変更) |
| **K** | **ウィンドウを先に出す** (vendored eframe の `with_visible(false)` をやめ、wgpu 初期化前にテーマ色で塗ったウィンドウを表示する) | **体感で最大**: ウィンドウ出現 ~1.0s → ~0.1s | 大 | 高 (白フラッシュ対策として upstream が入れた挙動。DPI 確定前のサイズ、フォーカス、タスクバー、detached viewport との相互作用を要検証) |

### A の注意点

- `WgpuSetup::Existing` を渡すと `RenderState::create` は
  `instance.enumerate_adapters(Backends::all())` を実行する
  (`vendor/egui-wgpu/src/lib.rs:186-192`) ため、**そのままだと GL / Vulkan まで列挙して
  逆に遅くなる**。vendored なので `Existing` のときは列挙をスキップするよう直す。
  `available_adapters` が情報表示以外に使われていないかを先に確認すること。
- `device_descriptor` は eframe の既定と揃える (features / limits がずれると egui の
  レンダラが動かない)。
- surface は同じ instance から作られるので互換性は保たれる。
- 失敗時 (アダプタなし / device 作成失敗) は **必ず現行の `CreateNew` 経路へフォール
  バック**する。ここを握り潰すと「起動しない環境」を作りかねない。

## 5. 推奨する進め方

1. **Stage 0 (計測基盤)**: I。あわせて「プロセス生成 → `run()` 入口」を
   `GetProcessTimes` の creation time から算出して `startup.process_load` として出す。
   これがないと A/B の効果を利用者環境で確認できない。
2. **Stage 1 (低リスクで効く)**: B + E + F。想定 -200ms 前後。
3. **Stage 2 (構造)**: A。想定さらに -200ms。ここまでで first_frame ~400ms が目標。
4. **Stage 3 (体感)**: C。グリッドが出るまでが ~1.4s → ~0.5s。
5. **Stage 4 (要判断)**: D / G / H / J。
6. **Stage 5 (別議論)**: K。効果は最大だが upstream の挙動を覆すので、
   Stage 1–3 の結果を見てから判断する。

## 6. 未確定 / 追加計測が必要な点

- `enumerate_adapters` の 220ms が NVIDIA / Intel / WARP のどれにどれだけ乗っているか。
  iGPU と WARP を除外できれば追加で 50–80ms 見込めるが、それには
  `wgpu::Instance::from_hal` + `create_adapter_from_hal` で自前列挙する必要があるので、
  効果を測ってから着手する。
- cold 起動 (再起動直後 / 初回インストール直後) の内訳。今回の数値はすべて warm。
  cold では launcher の初回起動 892ms を実測しており、AV の影響が大きい。
- 利用者一般の settings.db サイズ分布 (§3.2 の 95ms がどこまで一般的か)。
