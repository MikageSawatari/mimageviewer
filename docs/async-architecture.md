# 非同期・並列アーキテクチャ

「どの処理がどのスレッド/プロセスで走るか」「どうやってキャンセルするか」「キャッシュ競合をどう避けるか」
の一覧。並列処理を追加・変更するときの設計テンプレートとして使う。

---

## 1. ワーカー一覧

| ワーカー | 実装 | 個数 | 用途 |
| --- | --- | --- | --- |
| サムネイル (通常) | `std::thread` + mpsc | `parallelism - 重I/O` | Image / ZipImage / PdfPage の軽いデコード + PdfFile のフォルダ代表画 (PDFium pool への IPC 待ちなのでメインプロセス内 CPU は消費しない。PDFium pool 3 並列を活かすためここに置く) |
| サムネイル (重 I/O) | `std::thread` + mpsc | 1〜2 (総数 ≤4 なら 1) | Folder / ZipFile の全体走査 (本物の同期 I/O。`fs::read_dir` 再帰探索 / ZIP セントラルディレクトリ読み込みなどメインプロセス内ブロッキング) |
| フルスクリーンロード | `std::thread` (使い捨て) | 1 枚ごとに spawn | フルサイズ画像デコード + アニメ展開 |
| PDF ワーカー | **別プロセス** (`--pdf-worker`) + 各プロセス専用のディスパッチャースレッド | 3 (`POOL_SIZE`) | PDFium は非スレッドセーフ → マルチプロセスで並列化。要求は JobQueue に enqueue |
| PDF ページ列挙 | `std::thread` | 1 (PDF 開く都度) | PDF ワーカーに列挙要求を送る |
| Susie ワーカー | **別プロセス** (`mimageviewer-susie32.exe`、32bit ビルド) + ディスパッチャースレッド | 3 (設定で 1 に落とせる) | 32bit の Susie 画像プラグイン (`.spi`) をロードし IsSupported/GetPicture を呼び出す。プラグインクラッシュの隔離も兼ねる |
| AI 推論 | `std::thread` + mpsc | 1 (全モデル共通) | ort (DirectML) の upscale/denoise/inpaint |
| 音声出力 warm-up | `std::thread` (`cpal-warmup`) | 起動時 1 本 | WASAPI の初回 audio session 確立をバックグラウンドで済ませる。小さな無音 cpal stream を短時間だけ開いて閉じ、初回動画 open の UI スレッド停止を避ける |
| 動画サムネイル | `std::thread` | 1 | Windows Shell API を逐次呼び出し |
| 動画 demux | `std::thread` (`video-demux`、= `run_decoder` の本体) | 動画 1 つにつき 1 本 | FFmpeg `avformat_open_input` で開いた `Input` を保持し、`packets()` で取り出した packet を stream index で振り分けて `video_pkt_tx` / `audio_pkt_tx` (各 bounded=256) に流す。seek 要求 (`AvClock::take_seek_request`) は demux thread が pull し、`input.seek` 後に `Flush` marker を両 decode thread に送る。packet 送信待ち中に新しい seek が来た場合は古い packet を捨てて demux loop に戻り、`Flush` を優先する。EOF 時は両 channel に `Eof` を送って自身は idle wait |
| 動画 video decode | `std::thread` (`video-decode`、= `run_video_decode`) | 動画 1 つにつき 1 本 | `video_pkt_rx` から `VideoWorkerMsg::{Packet, Flush, Eof}` を受け、HW (D3D11VA + GPU blit) → SW (`av_hwframe_transfer_data` + swscale) で frame を生成、PACE_LEAD=0.30 の pacing 後に `video_tx` (bounded=24) へ `try_send`。`Flush` で `flush()` + `current_seek_serial` / `drop_before_secs` / `post_seek_frame_sent` を更新 |
| 動画 audio decode | `std::thread` (`video-audio-decode`、= `run_audio_decode`) | 動画 1 つにつき 1 本 (音声無し動画では起動しない) | `audio_pkt_rx` から `AudioWorkerMsg::{Packet, Flush, Eof}` を受け、avcodec decode + swresample で f32 stereo 48kHz に揃え、post-seek packet/sample trim 後に `audio_tx` (bounded=32) へ送出。`audio_tx` 会計は source 秒ではなく enqueue 時点の playback speed で wall 秒へ換算する。`Eof` では `avcodec_send_packet(NULL)` + receive_frame ループで残サンプルを drain (= 末尾の数十 ms の音声を出し切る) |
| 動画音声 pump | `std::thread` (`audio-pump`) | 動画 1 つにつき 1 本 | `audio_tx` から受けたフレームを ring buffer に押し込む。RT 出力は cpal の専用スレッドが担当。1.0x 以外では Signalsmith Stretch で pitch を維持したまま output/wall 秒へ time-stretch し、その後 VST3 enable + プラグインロード済みなら ring buffer に push する直前に `DspBridge::process_block` 経由で bridge プロセスへ往復する (= ~1-2ms IPC roundtrip) |
| 動画 native presenter | `std::thread` (`native-video-presenter`, Windows) | フルスクリーン動画 1 つにつき 1 本。ただし動画タイルモード中の動画→動画移動では `SwitchSource` で再利用 | 専用 HWND + D3D11 presenter + egui overlay を保持し、`video_rx` から受けた `VideoFrame` を表示する。`NativeVideoOutputCommand::SwitchSource` で source binding (`video_rx` / `AvClock` / engine event tx / duration / displayed_frame_seq) を差し替え、HWND と overlay を破棄せず次動画へ切り替える |
| VST3 host bridge | **別プロセス** (`mimageviewer-vst3-host.exe`、C++) | アプリ起動中 0 or 1 本 | VST3 SDK は C++ 前提なので bridge プロセス分離。bridge 内部は 3 thread (audio loop + GUI message pump + stdin pump)。詳細は [docs/vst3-integration.md](vst3-integration.md) と `crates/vst3-host/` ソースコメント |
| VST3 plugin GUI ホスト | `std::thread` (`vst3-plugin-gui`) | プラグイン GUI 表示中のみ 1 本 | Win32 `CreateWindowExW` で独立 HWND を作成 → bridge にその HWND を渡してプラグイン側で `IPlugView::attached()`。HWND の WndProc + メッセージループはこのスレッドで回す (eframe (winit) と衝突しない) |
| 動画音声 RT 出力 | `cpal::Stream` 内部スレッド | 動画 1 つにつき 1 本 | WASAPI Shared モード。コールバックで ring buffer から f32 stereo を pop し、**実消費サンプル数 (= `real_consumed`) 分のみ** `next_pts_secs` を進めて `AvClock::set_audio_pts` でマスタークロックを更新。silence 出力中 (= `real_consumed=0`) は pts 進行 skip。`!clock.is_playing()` (= 一時停止 / EOF) と `pump_seek_serial < clock_serial` (= pre-seek サンプル全消去) は早期 return。`AvClock::set_audio_pts` 側に defensive wall-rate cap (= `wall_dt + 5ms` で pts 進行を頭打ち) を保持し、buffer 非空 pre-fill burst の異常前進への保険にしている (Phase 9 後の cleanup refactor、詳細は [docs/video-engine-redesign.md](video-engine-redesign.md) の「Phase 9 後の Post-cleanup refactor」節) |
| フォルダナビゲーション | `std::thread` | 1 (常時 ≤ 1 本) | 深さ優先で次フォルダを検索。連打は `pending_folder_nav_steps` に累積され、完了ごとに連鎖実行する (並行 DFS による FS 競合を避ける) |
| キャッシュ一括生成 | `rayon` | (ユーザー設定) | ダイアログから起動するバッチ処理 |
| メタ索引 supervisor (Ctrl+F/G 用) | `std::thread` (常駐) | お気に入りごとに 1 本 (`auto_index_metadata=true`) | 初期スキャン + notify-rs 監視 + ingest を統括。共有 `Arc<Mutex<IndexWriter>>` 経由で Tantivy writer を直列化 (Tantivy は Index あたり writer 1 本制約) |
| メタ ingest worker | `std::thread` (supervisor 内部) | 速度プロファイルで 1 / 2 / 4 | メタ抽出 + Tantivy buffer + バッチ commit (100 件 or 5 秒) + commit 成功後に fts_meta upsert_meta_ok / delete_paths (Tantivy First) |
| メタ walker | `std::thread` (supervisor 内部、1 回) | 1 | 起動時 3-way diff (FS vs fts_meta.db) |
| メタ FsWatcher | `std::thread` (notify-rs 内部) | お気に入りごとに 1 本 | `ReadDirectoryChangesW` + 500ms debounce → `DebouncedChange` 送信 |
| 名前索引 supervisor (Ctrl+S 用) | `std::thread` (常駐) | お気に入りごとに 1 本 (`auto_index_structure=true`) | `search_index.db` は SQLite 単独なので複数 supervisor が真並列で動く |
| Ctrl+G クエリワーカー | `std::thread` (使い捨て) | 1 入力ごとに spawn | Tantivy ページング (Searcher snapshot 固定) + token matching (post-filter で Tantivy STORED 原文を引く) + streaming 送信 |
| タグ書き込みワーカー | `std::thread` (常駐) | 1 | UI の Toggle / Clear を serial に処理: XMP 書込 → 共有 writer で Tantivy upsert (タグ含む全 STORED 原文を更新) → 32 件 or 500ms でバッチ commit |
| タスクトレイ (v0.9) | `std::thread` (常駐) | 1 (設定 ON 時のみ) | `mimv-tray` スレッド。`tray-icon` クレートで隠し HWND を作成 → `PeekMessageW` ポンプ (50ms 周期) + `TrayIconEvent` / `MenuEvent` の try_recv → `TrayEvent::Open / TogglePause / Quit` を UI に送信。`ActivityGate::set_paused` + `GlobalIoSemaphore::set_throttled` はメインスレッドで適用 |

**rayon は通常サムネイル生成には使っていない** (逐次ワーカーの方がキャンセル制御しやすいため)。

---

## 2. スレッド間通信

### 2.1 共有アトミック

| 名前 | 型 | 書き手 | 読み手 | 用途 |
| --- | --- | --- | --- | --- |
| `cancel_token` | `Arc<AtomicBool>` | UI (フォルダ切替) | 全ワーカー | 停止シグナル |
| `scroll_hint` | `Arc<AtomicUsize>` | UI (スクロール) | サムネワーカー | 優先度計算の基準 |
| `keep_start_shared` / `keep_end_shared` | `Arc<AtomicUsize>` | UI | サムネワーカー | 範囲外の要求を破棄する境界 |
| `visible_end_shared` | `Arc<AtomicUsize>` | UI | サムネワーカー | 可視範囲の終端 (exclusive)。先読み forward 側の距離計算に使用 |
| `display_px_shared` | `Arc<AtomicU32>` | UI (設定変更) | サムネワーカー | 生成時の目標ピクセル数 |
| `cache_gen_done` | `Arc<AtomicUsize>` | キャッシュ生成 rayon | UI | 進捗カウンタ |
| `SupervisorHandle.cancel` | `Arc<AtomicBool>` | UI (お気に入り OFF, App drop) | メタ / 名前索引 supervisor | supervisor 全体の停止シグナル |
| `GlobalSearchHandle.cancel` | `Arc<AtomicBool>` | UI (クエリ変更, バー閉じ, folder 遷移, Handle drop) | Ctrl+G クエリワーカー | Tantivy ページングループの中断 |
| `tag_write_worker.cancel` | `Arc<AtomicBool>` | App drop | タグ書き込みワーカー | 書込ループ + commit の中断 |
| `NativeVideoOutput.source_epoch` | `Arc<AtomicU64>` | UI / native presenter | UI / native presenter | native presenter 再利用時の stale event 防止。`SwitchSource` ごとに epoch を進め、presenter から UI へ送る `NativeVideoOutputEvent` に付与する。UI は現在の player epoch と一致しない event を破棄する |
| `ActivityGate.paused` (v0.9) | `AtomicBool` | UI (トレイメニュー「一時停止」 / ウィンドウ hide) | `wait_until_idle` を呼ぶ全ワーカー (walker / ingest / name_bulk_indexer) | true の間 wait ループが解除 or cancel まで抜けない。cancel は貫通 (終了時の固まり防止) |
| `GlobalIoSemaphore.throttled` (v0.9) | `Mutex` ガード | UI (ウィンドウ hide/show) | 全インデクサ worker | true の間、実効 permit=1 (in_use ≥ 1 なら新規 acquire 不可)。解除で `notify_all` |

**ルール**: アトミックは単発の値伝搬にのみ使う。リスト/辞書の共有は `Arc<Mutex<...>>` か mpsc。

### 2.2 チャネル

| 名前 | 方向 | 内容 |
| --- | --- | --- |
| `tx / rx` (App) | ワーカー → UI | `ThumbMsg`: (idx, ColorImage, from_cache, source_dims, canceled, finalized)。from-source 経路 (cache miss) では **2 シグナル**: ① 第 1 シグナル = display ColorImage (canceled=false, finalized=false) → UI は Loaded 化、`requested` は保持 ② 第 2 シグナル = cache save 完了通知 (None + finalized=true) → UI は `requested` を抜くだけで **`thumbnails[i]` は変更しない**。cache hit は 1 ショット (canceled=false で即 remove)。`canceled=true` は STALE 専用 (worker bail-out) で、UI は Evicted に戻して再試行可能にする (Failed にしない)。`finalized=true` と `canceled=true` は排他 |
| `fs_pending[idx].1` | フルスクリーンスレッド → UI | `FsLoadResult`: **DimsOnly (非終端) / Static / Animated / Failed**。`DimsOnly` はヘッダ解析直後に先行送信される原寸ヒントで、UI は `fs_early_dims` に積み fs_pending は維持する (本デコードが続く)。詳細は [display-pipeline.md §2.2](display-pipeline.md) 参照 |
| `ai_upscale_pending[idx].1` | AI スレッド → UI | `UpscaleResult` |
| `pdf_enumerate_pending` | PDF 列挙スレッド → UI | `(pages, password_needed)` |
| PDF ワーカー stdin/stdout | UI プロセス ↔ PDF ワーカープロセス | 長さプレフィクス付きバイナリプロトコル (Enumerate / Render / Shutdown) |
| Ctrl+G `SearchStreamEvent` | Ctrl+G ワーカー → UI | `Batch { hits, scanned_candidates, valid_hits }` / `Done { truncated, reason }` / `Error`。毎フレーム `try_recv` を MAX_EVENTS_PER_FRAME=8 までループ消費 |
| `DebouncedChange` (notify-rs) | FsWatcher → supervisor | 500ms ウィンドウで集約した変更イベント (`favorite_id`, `path`, `ChangeKind`) |
| `SupervisorCommand` | UI (`IndexerManager`) → supervisor | 一時停止 / 再開 / フル再スキャン要求 |
| `IndexerManager.writer` | 全書き込み経路で共有 | `Arc<FtsWriterDispatcher>` — Tantivy は Index あたり writer 1 本制約。専用ディスパッチャースレッドが優先度キュー (Interactive > Background) でジョブを直列処理する。 ingest worker (Background) と tag_write_worker (Interactive) は `WriterJob::Upsert` / `Delete` / `Commit` / `Batch` を `submit` するだけで、writer に直接触らない (§5.5)。 |

### 2.3 ワーカーキュー

| キュー | 型 | 内容 |
| --- | --- | --- |
| `reload_queue` | `Arc<Mutex<Vec<LoadRequest>>>` | 通常サムネイル要求 (Image/ZipImage/PdfPage に加え、PdfFile のフォルダ代表画も IPC 待ちのためここに振る) |
| `heavy_io_queue` | `Arc<Mutex<Vec<LoadRequest>>>` | Folder/ZipFile 要求 (本物の同期 I/O のみ) |
| `pdf_pool.queue` | `Arc<(Mutex<JobQueue>, Condvar)>` | PDF ワーカーへのレンダ/列挙要求。`critical` / `normal` VecDeque + `normal_in_flight` + `workers_busy` を同一 Mutex で保護 |
| `texture_backlog` | ローカル Vec (App) | GPU アップロード未完の ColorImage。MAX_TEXTURES_PER_FRAME=8 超過分 |

ワーカーが要求を取り出すときは **優先度 (priority フラグ) → 距離 → forward/backward** でソート。
距離計算は可視範囲の端からの歩数: backward は `scroll_hint - idx`, forward は `idx - visible_end + 1`
で、同距離では forward (次ページ方向) が先。これは `fs_cache` 先読み / AI アップスケール先読み /
サムネイルグリッドワーカーの全てで統一されており、`+1, -1, +2, -2, ...` の順 (forward 先) となる
(共通ヘルパ: `interleaved_prefetch_targets`)。

### 2.4 GlobalIoSemaphore (I/O 横断調停)

`src/io_semaphore.rs`。PDF ワーカー (3 プロセス) / サムネイル背景ジョブ /
インデクサ (walker + ingest) が同時に HDD をシークすると UI スクロールがつまる。
これを防ぐため、**全ワーカー横断で同時 I/O 数を優先度付きで制限する**。

| 優先度 | 用途 |
| --- | --- |
| `High` | UI が今見ているフォルダ / ページ (UI 経路、PDF critical) |
| `Normal` | PDF 背景レンダ、通常サムネイル |
| `Low` | インデクサ (メタ / 名前、速度プロファイルで 1 / 2 / 4 permit) |

実装は `Mutex + Condvar` で自前 (`try_lock + sleep` 禁止、§5.5 参照)。permit の
drop で自動 release + `notify_all` で起床。spurious wakeup 耐性のため条件は
`while` ループで再確認。

**飢餓ポリシー (明示)**: High が連続投入される間 Low は無制限に待つ。これは
UI 応答性最優先という方針の意図的な選択。アイドル 数秒で High キューが空き、
Low が進む。不足する場面は「AC 電源時のみインデックス」等の別機構で制御する。

---

## 3. キャンセル規約

### 3.1 フォルダ切替時

`load_folder()` が呼ばれたら:

1. 旧 `cancel_token` に `true` をセット
2. 新しい `cancel_token` を作って `Arc` を差し替え
3. 旧 mpsc 受信は drop (新しい tx/rx に置き換え)
4. 新しいワーカーを新トークン付きで spawn
5. 各種キャッシュ (`fs_cache`, `adjustment_cache`, `ai_upscale_cache`, `rotation_cache` …) をクリア

**旧プールを毎回捨てる**のが肝。同じプールを使い回さないので競合を気にしなくてよい。

### 3.1.5 フォルダナビゲーション (Ctrl+↑↓) のキャンセル + アキュームレート

Ctrl+↑/↓ はフォルダツリーを DFS で辿って次の「画像/動画/ZIP/PDF があるフォルダ」を
見つけるが、キーリピート (30Hz) で連打すると、過去は毎プレスで新スレッドを spawn +
旧スレッドに cancel を投げる設計だった。ただし `navigate_folder_with_skip` 自体は
cancel を見ていなかったので、cancel 済みスレッドも DFS を最後まで走り切り、
並行 DFS が FS を奪い合って単発 DFS が 200ms → 1s 級に遅延する事故を起こしていた
(2026-04 セッションで実測、PDF だらけの scan フォルダで顕著)。

現在の挙動 (2026-04 修正後):

- `navigate_folder_with_skip` と `folder_should_stop` は `Option<&AtomicBool>` を受け取り、
  各 DFS ステップとディレクトリエントリ走査のたびに cancel をチェックする。旧スレッドは
  cancel 検出時点で `None` を返して即終了 → FS 競合が消える。
- `start_folder_nav` は in-flight 中の追加プレスを `pending_folder_nav_steps: i32` に
  累積する (forward=+1 / backward=-1)。**新スレッドは spawn しない**。
  累積は `±MAX_PENDING_NAV = 5` で飽和する (それ以上のプレスは捨てる) ので、
  キーを離した後に「離したのに動き続ける」違和感が出ない (drain は最長 ~500ms)。
- 現 nav が完了 → `apply_folder_nav_result` がモード別後処理 → `chain_folder_nav_if_pending`
  で累積が残っていれば 1 消費して次のステップ (新しい current からの DFS) を連鎖起動する。
- 連打中に別経路のナビ (click / favsearch / address / BS) が入ると累積はクリアされ、
  in-flight もキャンセルされる (`load_folder` → `start_loading_items` の既存処理)。

これにより 30 回連打は 30 ステップ分の DFS を逐次的に進める (並行ではなく直列)。
各 DFS 間で cancel チェックが入るので、途中で方向が反転しても即座に対応できる。

#### 3.1.5.1 モード別後処理 (grid / fullscreen / favsearch)

Ctrl+↑↓ は 3 つの起点から発火し、DFS 完了時に異なる後処理が必要になる。同じ非同期
パイプラインで扱えるように `FolderNavMode` をキーにして後処理を分岐している:

| モード | 発火元 | DFS 完了時の処理 |
| --- | --- | --- |
| `Grid` | `navigate()` の Ctrl+↑↓ (通常グリッド) | `load_folder(p)` のみ |
| `Fullscreen` | `handle_fs_navigation` の Ctrl+↑↓ (フルスクリーン中) | `close_fullscreen` → `load_folder(p)` → `open_fullscreen(先頭/末尾 image-like idx)` |
| `Favsearch { root }` | `favsearch_ctrl_nav` (お気に入り検索中) | `is_under(p, root)` が真なら `nav_stack.push + load_folder + update_favsearch_address`、偽なら `favsearch_navigate_sibling(±1)` |

実装上の要点:

- `FolderNavPending { cancel, rx, forward, mode }` が進行中 DFS の状態を持ち、
  `poll_folder_nav` が `FolderNavResult { path, forward, mode }` を返す。
- `apply_folder_nav_result` がモードに応じて分岐。Fullscreen ブランチで
  `close_fullscreen` を呼ぶが、そこは既に `folder_nav_pending = None` なので
  再帰的な自己キャンセルは起きない。
- 連鎖ステップでも同じモードを引き継ぐ。`pending_folder_nav_mode` を App 側に保持し、
  `chain_folder_nav_if_pending` がそれを参照して次の `spawn_folder_nav` に渡す。
- Favsearch モードでは起点フォルダが `nav_stack.last()` なので、連鎖時には
  `current_folder` ではなくスタックトップを使う。

モード境界のキャンセル:

- ユーザーが ESC / 右クリックでフルスクリーンを抜ける → `close_fullscreen` が
  走行中の Fullscreen モード nav を検出してキャンセル + pending クリア。
- Favsearch を閉じる (`close_favsearch`) や favsearch_back → `load_folder` 経由で
  `start_loading_items` が folder_nav_pending を一律キャンセル。
- モード違いで `start_folder_nav` が呼ばれた場合 (理論的エッジケース) は、
  旧 DFS をキャンセルしてから新モードで仕切り直す。

### 3.1.6 動画タイルモード中の動画→動画切替

Windows native presenter 有効時、動画タイルモード中にホイールで隣の動画へ移動する場合は
通常の fullscreen reopen 経路ではなく fast path を使う。

処理順序:

1. 旧 `VideoPlayer` から `NativeVideoOutput` を取り外す。
2. 新 `VideoPlayer` を `native_output_config=None` で構築する。
3. 新 player の `SwitchSourcePayload` を既存 `NativeVideoOutput` に送る。
4. 新 player に native output を attach し、`fs_cache[target_idx]` に入れる。
5. `fullscreen_idx` と overlay / metadata 同期を新 idx へ更新する。
6. 最後に旧 video entry を remove する。

旧 entry の remove を最後にする理由は、旧 decoder を先に drop すると presenter thread が
まだ旧 `video_rx` を見ている間に sender が close し、SwitchSource 到着まで disconnected
状態を経由するため。source binding を先に差し替えてから旧 player を shutdown する。

`video_tile_swap_pending` は新動画の `player.info()` 到着を待つ UI 側 pending state。
pending 中は追加ホイール入力を捨て、queue も delta 累積もしない。これは Ctrl+↑↓ の
ロックと同じく、ユーザーが操作を止めたあとに溜まった移動が遅れて発火しないようにするため。
`info()` が来たら新しい `VideoTileState` を構築し、来なければ既存 reopen 経路へ fallback する。

### 3.2 フルスクリーン / AI のキャンセル

1 枚ごとに `Arc<AtomicBool>` を `fs_pending[idx]` / `ai_upscale_pending[idx]` に持たせる。
要求を取り下げるときは個別にこのフラグを立てる。
ワーカーは大きな処理の合間 (タイル推論の各タイル、フレームデコード直後、など) でフラグを確認する。

### 3.3 フルスクリーン読み込みの優先度制御

`start_fs_load` はプールを持たない使い捨て `std::thread::spawn` なので、素朴に先読みを
並列起動すると現在表示中の画像のデコードが先読みスレッドに CPU を奪われて遅延する。
これを防ぐため `update_prefetch_window` は以下のルールで動く:

1. 現在画像が `fs_cache` に入っていない (デコード中) 間は、**他の全ての pending スレッドを
   キャンセル**する (KEEP 範囲内でも)。現在画像が CPU を独占する。
2. 同時に、先読みの新規 spawn も **延期**する。
3. `poll_prefetch` が現在画像の完了を検出したら、再度 `update_prefetch_window` を呼び、
   そこで初めて先読みが起動する。

AI アップスケール (`maybe_start_ai_upscale`) も同様: 同時実行は 1 枚のみで、現在画像が
来たら古い先読みをキャンセル。

### 3.4 サムネイルワーカーの STALE 取消と重複エンキュー抑制

サムネイルは「keep_range 内かどうか」が毎フレーム変化するため、単純なキャンセルでは
**同じ idx が in-flight なのに scroll 戻りで再エンキューされ、PDF 再レンダが二重に走る**
事故を起こす。2026-04 のセッションで以下のルールを確立した:

- **`update_keep_range_and_requests` は `self.requested` を範囲外一括 remove しない**。
  ワーカー処理中の idx まで抜いてしまい再エンキューを誘発するため。step 1 は Loaded→Evicted
  の遷移だけ行う。
- **`requested` の cleanup 経路は 4 本**:
  1. エンキュー済・pop 前の取消 → step 2 の `q.retain` が dropped idx を `requested.remove`
  2. ワーカー pop 後の STALE → ワーカーが `ThumbMsg` に `canceled=true` を載せて送信 →
     `poll_thumbnails` が `requested.remove` + `Evicted` (Failed にしない)
  3. cache hit 正常完了 → 第 1 シグナルで `poll_thumbnails` が `requested.remove`
  4. cache miss 正常完了 → **第 1 シグナル (display ColorImage) では remove しない**、
     第 2 シグナル (cache save 完了、`finalized=true`) で remove。`finalized=true` の場合は
     `thumbnails[i]` を **変更しない** (texture アップロード待ちの Pending を Evicted に
     書き換えると次フレームに再エンキュー → 重複デコード地獄になる事故を防ぐ)。
     旧実装は STALE と同じ `canceled=true` で送信していたため、texture_backlog に
     アップロード待ちが詰まっている時に Pending → Evicted の上書きが起きていた。

     **さらに finalized-vs-backlog レース**: 第 2 シグナルが先着したが第 1 シグナルの
     ColorImage は `texture_backlog` に積まれてアップロード待ち、というケースで
     即 `requested.remove` すると `Pending && !requested.contains` → 次フレーム再エンキュー
     の無限ループになる。対策として `pending_finalize: HashSet<usize>` を追加し、
     finalized 受信時に thumbnails[i] が **Pending のとき** は idx を pending_finalize
     へ積む。アップロード完了 (新規 or backlog から) で Loaded 化した瞬間に
     `pending_finalize.remove(&i)` が true を返せばその場で `requested.remove` する。

     **さらに finalized-vs-evict レース (v0.7.3)**: 第 1 シグナル到着後にユーザーが
     スクロールして keep_range から外れると、`update_keep_range_and_requests` が
     Loaded → Evicted に落とす (この時点では `requested` は意図的に残す = cache-save 完了
     待ち)。その直後に第 2 シグナルが届くと、旧実装は state=Evicted でも pending_finalize
     に積んでしまい、pending_finalize は Loaded 遷移時にしか掃除されないため、
     `requested[i]` が永久に居座る。スクロールで戻ってきても再エンキューループの
     `if requested.contains_key { continue; }` に弾かれてサムネが Evicted のまま固着する。
     対策として finalize ハンドラを 3 分岐に分け、**Evicted / Failed のときは
     `requested.remove + pending_finalize.remove` で即時掃除**する (ワーカーは第 2 シグナル
     送信済みなので「処理中の idx を抜くな」の規約には違反しない)。ログ `[poll] finalize
     on Evicted idx=N → cleanup requested` で発動を可視化する。
     `canceled` / 失敗 / `load_folder` リセット時にも pending_finalize をクリア
- **STALE チェックはワーカーパイプラインの 3 箇所**:
  - `spawn_worker` が pop 直後 (app.rs): キャッシュ lookup すら不要な明白な範囲外
  - `process_load_request` の heavy I/O resolve 後 (thumb_loader.rs): ZIP/folder の
    I/O (秒単位) 完了後に範囲外になっていないか
  - `process_load_request` の PDF レンダ直前 (thumb_loader.rs): cache miss で PDFium
    に投げる前。これがないと scroll 往復で同じページの 1 秒レンダが重複する
- 3 箇所とも `canceled=true` を送信して `requested` cleanup する。`continue` だけでは
  `requested` に残って「再エンキューされない idx=Pending」状態で固まる。

**なぜ 2 シグナル方式か**: `load_one_cached` は decode → tx.send (display) → WebP encode →
DB save → cache_map.insert の順で処理する。もし第 1 シグナル到着時に `requested` を抜くと、
cache save 進行中 (数百 ms) は `requested` 空かつ cache_map にも未登録の窓が開き、
その間に scroll 往復が起きると別 worker が同じ idx を cache miss 扱いで取得し重い decode
(ZIP 取り出し・PDFium レンダ等) を二重に走らせる。第 2 シグナルで cache save 完了後に
初めて `requested` を抜くことで、cache save 中の再エンキューは `requested.contains_key=true`
で弾かれる。

### 3.4.1 検索系のキャンセル規約

| ワーカー | 発火元 | シグナル |
| --- | --- | --- |
| Ctrl+G クエリワーカー (`global_search::run`) | クエリ変更 / フィルタ変更 / バー閉じ / folder 遷移 / `GlobalSearchHandle` drop | `Arc<AtomicBool>` を Tantivy ページングループ頭と post-filter ループ頭で check |
| IndexerSupervisor (メタ / 名前) | `IndexerManager::sync_with_favorites` で OFF 化、App drop | `SupervisorHandle::stop()` → cancel + FsWatcher drop + thread join (最大 ~250ms) |
| walker / ingest (supervisor 内部) | supervisor cancel | 各ループ checkpoint で `Ordering::Relaxed` read。大ファイル走査中も数百 ms 以内に抜ける |
| tag_write_worker | App drop | `None` 送信 + cancel フラグ。commit 後のループ先頭で check |

**Tantivy writer 共有ルール**: `IndexerManager.writer: Arc<Mutex<IndexWriter>>` を
ingest worker と tag_write_worker が共有する。独自に `fts.writer()` を呼ぶと
`LockBusy` で **全 upsert が無効化される**。新しい書き込み経路を足すなら必ず
共有 writer を使う。

### 3.5 新ワーカー追加時のテンプレ

```rust
let cancel = Arc::clone(&self.cancel_token);  // フォルダ単位のキャンセル
let my_cancel = Arc::new(AtomicBool::new(false));  // 個別キャンセル (必要なら)
let tx = self.tx.clone();
std::thread::spawn(move || {
    // 大きな処理の合間で両方チェック
    if cancel.load(Relaxed) || my_cancel.load(Relaxed) {
        return;
    }
    // ... 処理 ...
    let _ = tx.send(result);
});
```

送信失敗 (受信側 drop) は無視する。フォルダ切替で既に捨てられているだけ。

---

## 4. GPU テクスチャ予算

### 4.1 keep_set ベースの退去 (display list vs filesystem list)

mImageViewer は 2 つのリストを使い分ける:

| 変数 | 役割 |
| --- | --- |
| `App::items: Vec<GridItem>` | **filesystem list** (ソース)。raw idx はフォルダセッション中 stable。 |
| `App::visible_indices: Vec<usize>` | **display list**。`items` への参照を ★フィルタ / 検索フィルタ通過後だけに絞ったもの。 |

**prefetch / eviction / retain 系のループは必ず display list (の部分列) を使う**。
`items` の raw idx 連続範囲 (`keep_start..keep_end`) を直接回してはいけない。
`visible_indices` が疎になっているとき (例: 1300 フォルダ中★5 のみ 3 件可視) に、
連続範囲を回すと非可視の 1000 件近くを prefetch キューに流し込んでしまう。

具体像:

- **`App::keep_range: (usize, usize)`** — `keep_set` の bounding box。worker 側で
  atomic に読める `keep_start_shared` / `keep_end_shared` の値を供給する。
  疎な keep_set に対しては「広め」の判定になるので、worker が稀に非可視 idx を
  掴んでしまうが、main thread 側で enqueue しなければほぼ発生しない。
- **`App::keep_set: HashSet<usize>`** — 実際に prefetch / 保持したい idx 集合。
  `visible_indices[vis_keep_start..vis_keep_end]` から毎フレーム構築する。
  enqueue / eviction / retain / idle upgrade / tag prewarm / 補正テクスチャの
  バックグラウンド処理はすべてこの `keep_set.contains(&i)` で判定する。

新しく「可視範囲の画像に対する背景処理」を追加する場合:

1. 反復対象は `self.keep_set.iter()` (必要なら `sorted` に clone してから)。
2. `keep_start..keep_end` の range ループは絶対に書かない。
3. `rebuild_visible_indices()` は `keep_set` を直接触らない。次フレームの
   `update_keep_range_and_requests` が再計算する (フィルタ変更で疎になっても
   1 フレーム遅れで自然に収束する)。

### 4.2 通常ロードの流れ

- 可視範囲 + prev/next ページ分のみ GPU に保持 (`keep_set` の範囲)
- 範囲外に出た瞬間に `TextureHandle` を drop (eviction)
- `egui_ctx.load_texture` でアップロードするコマ数を MAX_TEXTURES_PER_FRAME=8 に制限
- 超過分は `texture_backlog` に積んで次フレーム以降に処理

### 4.3 VRAM キャップ

- `gpu_info.rs` で取得した VRAM 量から動的にテクスチャ上限バイト数を決定
- 超過しそうなら visible slice (display list 上の区間) を両端から狭める (古い側から evict)

新しいテクスチャキャッシュ (例: 将来の補正 LUT プレビュー) を追加する時は、
この退去ロジックにも登録すること。

---

## 5. よくある事故パターン

### 5.1 キャンセル忘れ

新機能を作った時、`cancel_token` を参照し忘れると、フォルダ切替後もゾンビとして動き続ける。
→ 最悪 mpsc が満杯になるか、UI に古い結果が届く。必ずテンプレに従う。

### 5.2 キャッシュの部分更新

「補正は変わったけど AI は変わってない」のような時、`adjustment_cache` だけクリアして
`ai_upscale_cache` を残す。両方同時に消すと AI の再実行 (数秒) が発生してユーザーを待たせる。
詳細は [preset-and-adjustment.md](preset-and-adjustment.md) の無効化ルール表。

### 5.2.1 items 差し替え / 削除時の世代 bump 忘れ

`items` / `thumbnails` / `image_metas` を書き換える全経路で `items_generation` の
bump + idx ベース状態の破棄を忘れずに行う。忘れると、進行中ワーカーが旧 idx 向けに
生成した `ThumbMsg` が新 items の同じ idx に着地して**サムネが化ける**。

現在の経路と使うヘルパー:

- **フォルダ切替**: `start_loading_items` → `install_new_items`
- **Ctrl+G 結果差し替え**: `replace_search_view_items` → `install_new_items` +
  `invalidate_idx_state_and_queues` + path-keyed cache clear
- **削除**: `start_delete_files` (ゴミ箱移動を別 thread で実行) → 完了時に
  `poll_delete_pending` が path から現在の idx を引き直して `remove_items_batch` を
  呼ぶ。`remove_items_batch` は降順 idx 配列を受け取り、items/thumbnails/image_metas の
  物理 shift + `items_generation` bump + `adjustment_page_params` / `mask_pages` /
  `search_filter` の O(K log K) idx shift + `invalidate_idx_state_and_queues` を行う。
  キャンセルは各ファイルの `SHFileOperationW` 呼び出し前に判定 (1 件あたり 10-20ms)。

新しい差し替え経路を増やすときは、必ず以下を揃える:

1. `items_generation` を必ず bump (install_new_items 経由か直接 +1)
2. `invalidate_idx_state_and_queues()` を呼ぶ — requested / pending_finalize /
   texture_backlog / checked / keep_range / keep_set / keep_*_shared / idx-keyed
   HashMap 群 / in-flight pending (fs_pending / ai_upscale_pending) / reload_queue /
   heavy_io_queue を一括で片付ける
3. path-keyed キャッシュ (metadata_cache / exif_cache / xmp_cache / tags_cache) も
   items が総入れ替わりする経路ではリセット (部分削除ならリセット不要)
4. `items.remove` / `items.push` を直接書かない — 必ずヘルパー経由に通す。
   レビューでは `rg 'self\.items\.(remove|push|clear)'` で直接触っていないか確認する

この設計が崩れると 2026-04 に発生した「削除後に別 item のサムネが表示される」
「Ctrl+G 直後に重い ZIP/PDF decode が worker を占有して新結果のサムネが来ない」
といった再発しやすいバグが戻ってくる。

### 5.3 UI スレッドで重処理

`App::update` 内で CPU 重めの処理をすると fps が落ちる。
- 補正の LUT 計算: 軽いので同期 OK (`maybe_apply_adjustment`)
- AI 推論: 絶対に別スレッド
- 画像デコード: 絶対に別スレッド
- **GPU 上限超過画像のリサイズ**: 2026-04 に 7168×9216 の PNG をフルスクリーンで開くと
  UI が 10 秒近く固まる事故があった。`clamp_for_gpu(&ColorImage)` を UI スレッドで
  呼ぶと ColorImage → DynamicImage への premultiply 往復 (ピクセル毎ループ) と
  `resize_exact(Triangle)` が同期で走って 1 発 5 秒級になる。`start_fs_load` の
  worker 側で `clamp_dynamic_for_gpu(DynamicImage)` を先に掛ける方針に変更し、
  `fs_cache` / `ai_upscale_cache` / `adjustment_cache` の `Static.pixels` は
  **常に 8192px 以内** という不変条件に格上げした。UI スレッドの `clamp_for_gpu`
  は異常経路の安全網として残してあるが、通常パスでは `Cow::Borrowed` で返り
  リサイズは走らない。発動したらログに `clamp_for_gpu (UI-thread fallback)` が出る。
  詳細は [display-pipeline.md §2.2](display-pipeline.md) 参照。

**Ctrl+↑↓ / Ctrl+F / Ctrl+S / open_fullscreen の UI ブロック事件 (2026-04)**:
ファイル読込・SQLite・GPU アップロード・read_dir など、一見軽そうな処理が per-operation
で 100ms超ブロックする事例が複数判明した。対策と設計方針は
[ui-responsiveness.md](ui-responsiveness.md) にまとめてある。新機能追加前に
§4 チェックリストを必ず見ること。Windows 特有の罠として `Path::is_dir()` が
per-entry で `GetFileAttributes` syscall を呼ぶ件も記載。

### 5.4 PDF ワーカー / Susie ワーカーの想定外終了

ワーカープロセスがクラッシュしたら、親は検出して再起動する仕組みになっている。
新しい PDF / Susie 操作を追加する時はタイムアウト処理を忘れずに (stdout 読み取りで詰まらない)。

**Susie プラグインの並列実行に関する注意**: Susie 画像プラグインは 1990〜2000 年代の
レガシー規格で、並列実行 (特にプロセス跨ぎ) を想定していないプラグインが稀にある。
別プロセス隔離によりスレッド不安全性は解消されるが、以下は残る:

- 一時ファイル衝突 (固定名で temp を書くプラグイン)
- INI / レジストリの race 書き込み
- プラグインが間接ロードする外部 DLL にプロセス跨ぎのロックがある場合

対策として `Settings::susie_allow_parallel = false` でプールサイズを 1 に固定する
オプションを用意している。環境設定 → Susie プラグイン → 「プラグインを並列実行する」
チェックで切り替え可能。問題プラグインの切り分けはユーザー側に委ねる方針。

**Susie プール初期化の race (v0.7.0 修正済み)**: `susie_loader::supports_extension()`
は初期は `try_get_pool()` (プール未初期化時は None→false を返す) で判定していたが、
起動直後に Susie 対応拡張子を含む ZIP / フォルダを開くと、バックグラウンド init
スレッドの完了前に列挙が走って PI / MAG / Q0 などが無視されていた。
`get_pool()` (未初期化ならブロック) に切り替え、一度だけ数百 ms ブロックして
プールを取得する方式に変更。ネイティブ拡張子は `is_recognized_image_ext` 内の
`SUPPORTED_EXTENSIONS.contains` でショートサーキットされるためここに到達しない。
Susie を無効化していれば `get_pool()` は即座に empty プールを返すので無害。

**Susie プールキューの 2 レベル優先度**: `Job::priority` フィールドで可視セルかどうかを
区別し、`SusieWorkerPool::execute(req, hint, priority, cancel)` の `priority=true` 引数で
キュー先頭 (`push_front`)、`false` でキュー末尾 (`push_back`) に挿入する。

スクロール中の動作:
- 既に Susie キューに居座っていた古い (画面外) ジョブは末尾側
- 新しく投入された可視セルは先頭側 → ワーカーが次に pop する
- 結果として **画面外ジョブを待たずに可視セルが処理される**

priority 内の順序は LIFO (後着の push_front が先着を追い越す) になるが、可視範囲内で
あればどのセルから埋まっても体感上問題ないため許容。完全な FIFO 内優先度が必要に
なったら priority 用のサブキューを足すと良い。

`thumb_loader::process_load_request` から `req.priority` を `load_one_cached` に
渡し、その中の `decode_file` / `decode_bytes` 呼び出しに伝播。フルスクリーン読み込み
は常に priority=true (現在表示中の画像)。

**Susie 1 ジョブごとの計測ログ (環境変数で ON/OFF)**: 環境変数
`MIV_SUSIE_PERF_LOG=1` を設定して起動すると、各 Susie デコード呼び出しごとに
`mimageviewer.log` へ次の形式で計測ログを出す。サムネイル一括ロード時に
何が遅いか (キュー待ち / IPC 自体 / プラグイン処理) を切り分けるため。

```
susie: w0 OK  P ext=mag    queue=  0.4ms ipc=   12.3ms req=64B resp=512080B
```

- `w0` … ワーカー番号 (0..2)
- `queue` … `execute()` で enqueue された時刻 から ディスパッチャが pop した時刻
- `ipc` … `write_msg`+`read_msg` の合計 (32bit ifmag.spi 等のプラグイン処理時間も含む)
- `req`/`resp` … バイナリフレーム長
- `P`/`-` … priority フラグ (P=可視セル、-=背景ロード)

常時 ON だと数千件のサムネイルロードでログが膨大になるため、調査時のみ手動で
ON にする運用。GUI 設定には出していない。

### 5.5 try_lock + sleep ポーリングループ (禁止パターン)

「`Mutex` を `try_lock` して、失敗したら sleep して再試行」というループは、**複数スレッドが
同じ Mutex を奪い合う場面では飢餓 (starvation) を起こす**。10ms の sleep 中に fresh arrival が
割り込んで Mutex を横取りできるため、先に待ち始めたスレッドが秒単位で待たされる。

2026-04 に PDF ワーカープールで実際にこの現象が発生し、Critical 要求が 10 秒ブロックされた
(1 ワーカーに 62 件の連続ディスパッチが集中、他の 2 ワーカーは完全にアイドル)。

**代わりに使うべき設計**: **Mutex + Condvar で保護した優先度キュー + 専用ディスパッチャー
スレッド**。

```rust
// リソース要求側 (UI スレッド等)
fn execute(&self, job: Job) -> Result<R> {
    let (tx, rx) = mpsc::channel();
    {
        let (mtx, cv) = &*self.queue;
        let mut q = mtx.lock().unwrap();
        q.push(job);           // critical / normal などにソート
        cv.notify_one();       // ディスパッチャーを 1 つ起こす
    }
    // タイムアウト付き受信で cancel チェックを挟む
    rx.recv_timeout(Duration::from_millis(50))
}

// ディスパッチャースレッド (ワーカーごとに 1 本)
fn dispatcher(queue: Arc<(Mutex<JobQueue>, Condvar)>, resource: Resource) {
    loop {
        let job = {
            let (mtx, cv) = &*queue;
            let mut q = mtx.lock().unwrap();
            loop {
                if q.shutdown { return; }
                if let Some(j) = q.pop_with_priority() { break j; }
                q = cv.wait(q).unwrap();    // Condvar で起床
            }
        };
        // Mutex 外でリソースを使って処理
        let result = resource.process(job);
        let _ = job.reply.send(result);
    }
}
```

**この設計の利点**:
- 同一優先度内で **FIFO 公平性** (Condvar が queue に並んだ順で起こす)
- 10ms ポーリングの無駄なスピン消費がなくレイテンシも低い
- ワーカー選択が「先に空いた方の勝ち」ではなく「空いた瞬間に push されたジョブを pop」になる
- `shutdown` フラグと `notify_all()` だけで停止シグナルが全スレッドに伝わる
- cancel は pop 時と requester 側 (`recv_timeout` ループ) の両方でチェック可能

実装は `src/pdf_loader.rs` の `PdfWorkerPool` / `JobQueue` / `run_dispatcher` を参照。

**いつ try_lock を使って良いか**: 非ブロッキングな best-effort 取得 (「取れたら使う、取れなければ
今回は諦める」) のみ。`try_lock` の後に sleep して再試行する構造は避ける。

---

### 5.6 タスクトレイ常駐 + インデクサ throttle / pause (v0.9)

「ビューワ特性上、使い終わったらアプリを閉じる」 → notify-rs が止まり、次回起動時に
初回スキャンが必要になる問題への対策。ウィンドウの `[×]` でプロセス終了する代わりに
タスクトレイへ格納し、notify-rs を継続走行させる。

- **エントリポイント**: `src/tray.rs` (常駐スレッド) + `src/tray_integration.rs` (App メソッド)
- **単一インスタンス保証**: `src/single_instance.rs` の `Global\mImageViewerInstance_v1`。
  `installer/mimageviewer.iss` の `AppMutex` と一致させることで、インストーラが自動で
  「閉じてください」ダイアログを出してくれる (常駐中に DLL 上書きが失敗するのを防ぐ)。
- **ウィンドウ hide/show**: `App::maybe_intercept_close` が `viewport().close_requested()`
  を検出して `ViewportCommand::CancelClose` + `Visible(false)` に差し替える。トレイ
  メニュー「開く」やトレイアイコン左クリックで `Visible(true)` + `Focus`。
- **インデクサ throttle**: `hide_to_tray` から `IndexerManager::set_io_throttled(true)`。
  `GlobalIoSemaphore` の実効 permit を 1 に絞るため、ユーザーが選んだ速度プロファイル
  (Low/Medium/High) に関係なく常駐中は 1 permit 相当になる。`show_from_tray` で解除。
- **インデクサ pause (オプトイン)**: 設定 `pause_indexer_while_minimized = true` のときだけ、
  `hide_to_tray` から `ActivityGate::set_paused(true)`。既存の `wait_until_idle` は
  paused 中ループブロックし、`show_from_tray` で解除されると通常動作に戻る。
  **cancel は paused を貫通** させる (アプリ終了時に supervisor スレッドが固まらないため)。
- **GPU リソース解放**: `App::release_gpu_resources` が `thumbnails[*] = Evicted` や
  `fs_cache.clear()` 等で `TextureHandle` を drop。ウィンドウ復帰後は通常ロード経路で再取得。
- **UI heartbeat watchdog**: `App::update` は SW_HIDE 中に止まるため、`hide_to_tray` で
  watchdog を suspended にし、復帰時に resume する。これにより正常なトレイ常駐を
  `panic.log` の `UI THREAD HANG suspected` として記録し続けない。

設計上の注意:
- notify-rs の crossbeam-channel (unbounded) は paused 中も受信し続けるので、溜まった
  イベントは復帰時にスパイク的に処理される (OS 側の `ReadDirectoryChangesW` リングバッファ
  overflow リスクは notify-rs が即ドレインするので増えない)。
- throttle 有効化で既存 permit holder は revoke しない (drop まで維持)。hide 直後に 1 permit
  分の処理が残るが、通常は数百 ms で収まる。
- トレイ常駐中の `quit_requested` は `[×]` 乗っ取りロジックを貫通させるため、先に立ててから
  `ViewportCommand::Close` を送る。

---

## 6. 参考 (実測値)

`docs/bench-scroll-report.md` に詳細あり。要点:

- キャッシュヒット時のサムネ読み込み: 2〜3 ms/枚
- PDF レンダリング: 3 ワーカー並列で Cold 1441ms → 10ms (2 枚目以降)
- JPEG デコード: turbojpeg で 1.5〜2.4 倍高速化 (5MB 超は image crate にフォールバック)
- キャンセル遅延: 最大 1 枚デコード分 (数百 ms)

---

## 7. パフォーマンス計装 (perf.rs)

「キー入力 → 画面表示」レイテンシを後から解析するための構造化イベントログ。
既存 `logger.rs` (人間可読) はそのまま残り、`perf.rs` が JSON Lines を別ファイルに書く。

### 7.1 有効化

- **CLI 引数**: `mimageviewer.exe --perf-log` を付けたときのみ ON
- **無効時のコスト**: `perf::is_enabled()` の Atomic 1 回読みのみで `perf::event` は即 return
- **出力先**: `%APPDATA%\mimageviewer\logs\perf_events.jsonl` (起動毎に truncate)

### 7.2 `input_seq` の伝搬規約

`App` が `input_seq: u64` を持ち、**ユーザー入力イベント発生時のみ** `bump_input_seq()` で +1 する。
フレーム境界では増えない。0 は「相関なし」として予約。

| 発火箇所 | 種別 | 備考 |
| --- | --- | --- |
| `ui_fullscreen.rs::render_fullscreen_viewport` | `fs_key` / `fs_wheel` / `fs_close_*` | nav_delta / wheel_nav / close が確定した直後 |
| `app.rs::handle_keyboard` | `grid_key` | カーソルキーで selected が変わった時 |
| `app.rs::process_scroll` | `grid_wheel` / `grid_cols` | スクロールオフセットまたは列数が変わった時 |
| `app.rs::open_fullscreen` | `fs_open` | フルスクリーン遷移 |

**ワーカーへの伝搬**: UI スレッドは enqueue 時点の `input_seq` をタスク構造体にコピーする。

- `thumb_loader::LoadRequest.input_seq` — サムネイルワーカー用
- フルスクリーン非同期ロード: `start_fs_load` が `perf_seq` をクロージャにムーブする
- AI アップスケール / 色調補正ジョブ: 同様にクロージャへ
- PDF ワーカー IPC は seq=0 (プロセス間相関は現状非対応)

### 7.3 イベント構造

```json
{"t":12.345,"tid":5,"cat":"fs","kind":"paint","key":"C:\\a.jpg","seq":42,"idx":3}
```

主なカテゴリ:

- `input`  — ユーザー入力 (seq が振られる唯一のカテゴリ)
- `frame`  — 毎フレーム begin。`n` はフレーム番号
- `fs`     — フルスクリーン画像: `load_begin` / `decode_begin` / `decode_end` / `ready` / `paint`
- `thumb`  — サムネイル: `enqueue` / `pick` / `skip` / `decode_begin` / `decode_end` / `ready`
- `pdf`    — PDF ワーカー IPC: `pool_send` / `pool_recv` / `inproc_*` / `enumerate_send`
- `ai`     — AI: `upscale_begin` / `upscale_tile` / `upscale_end` / `denoise_*` / `job_start` / `job_ready`

### 7.4 解析

`scripts/analyze_perf.py` で集計。主要サブコマンド:

```bash
python scripts/analyze_perf.py <path>/perf_events.jsonl summary   # 件数/カテゴリ breakdown
python scripts/analyze_perf.py <path>/perf_events.jsonl latency   # seq → ready/paint ms
python scripts/analyze_perf.py <path>/perf_events.jsonl priority  # 優先度違反検出
python scripts/analyze_perf.py <path>/perf_events.jsonl thumbs    # decode 時間分布
python scripts/analyze_perf.py <path>/perf_events.jsonl dump 42   # 特定 seq の全イベント
python scripts/analyze_perf.py <path>/perf_events.jsonl timeline  # ガントチャート (matplotlib)
```

### 7.5 新ワーカー追加時のテンプレ

1. ワーカーに渡すタスク構造体に `input_seq: u64` フィールドを追加
2. UI スレッドの enqueue 箇所で `req.input_seq = self.input_seq` を設定
3. UI 側で `perf::event("<cat>", "enqueue", key, self.input_seq, &[...])` を emit
4. ワーカー側で `perf::event("<cat>", "begin"/"end", key, req.input_seq, &[...])` を emit
5. Ready 遷移 (texture upload 完了) で `perf::event("<cat>", "ready", ...)` を emit
6. `docs/async-architecture.md` のこの表にエントリを追加
