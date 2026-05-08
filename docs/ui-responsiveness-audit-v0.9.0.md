# UI 応答性監査 (v0.9.0)

Date: 2026-05-09
Reviewer target: Codex (read-only)
Author: Claude

## 目的

[CLAUDE.md「UI スレッドでの同期 I/O は即 worker 化する」](../CLAUDE.md) と
[docs/ui-responsiveness.md §4](ui-responsiveness.md) のチェックリストを基準に、
v0.9.0 で追加された動画 / VST3 / TensorRT / オフラインアップスケール の各機能で
UI スレッド同期 I/O の混入が無いかを系統的に監査する。

監査は **Phase 10+ のリファクタ着手前** に実施し、⚠️ が見つかれば Tier 0.5 として
リファクタより先に修正する。

---

## サマリ

| 区分 | 件数 |
|---|---:|
| 🔴 **要修正** (UI スレッド毎フレーム同期 I/O が確認された) | 1 |
| 🟡 既知の制限 (環境依存・ドキュメント済) | 1 |
| ✅ 確認済 (worker thread 化・init 1 回のみ・atomic 等で安全) | 多数 |

⚠️ **🔴 を 1 件確認**。動画フルスクリーン中、`App::update` から毎フレーム
`video_pin_db.lookup_pts` + `video_bookmark_db.list` の SQLite SELECT が走る。

**対応状況 (Codex)**: 妥当な指摘として確認し、`FullscreenVideoMarkerCache` を導入して
対応済み。フルスクリーン動画の open / ピン・ブックマーク変更時だけ SQLite から軽量
メタデータを再読込し、`App::update`、native overlay 同期、旧 egui HUD のシークバー /
J/K ジャンプ、左ジャンプパネル描画はキャッシュを参照する。

---

## 🔴 要修正

### #1 動画フルスクリーン中の毎フレーム SQLite クエリ

**場所**: [src/app.rs](../src/app.rs)
- `App::update` → 13203 行 `self.sync_native_video_timeline_markers(fs_idx)`
  (条件: `fullscreen_idx.is_some() && cfg(windows)`)
- 13534 行 `MouseMove` ハンドラ内 `if mouse.x < 340` でも追加発火 (= 左パネル領域に
  マウスを動かすと **マウス移動のたびに** 再クエリ)

**呼ばれる重い処理** (`sync_native_video_timeline_markers` 内):
```rust
// 毎フレーム実行される SQLite SELECT
self.video_pin_db.as_ref().and_then(|db| db.lookup_pts(&path))   // line 13794
self.video_bookmark_db.as_ref().map(|db| db.list(&path))          // line 13811
```

- `lookup_pts`: 1 行 SELECT (path key 検索)
- `list`: prepare_cached + 全件 fetch (path + ORDER BY pts_secs)

**実害の可能性**:
- 通常時は SQLite のホットキャッシュで sub-millisecond で返るので体感影響は小
- ただし `Vec<VideoBookmark>` を毎フレーム alloc するため、ブックマーク 100 個級で
  ~100KB/frame の allocation、60fps で 6 MB/sec の GC pressure
- 動画再生中は他の高頻度処理 (decoder dispatch、native present) と競合
- 2026-04 の Ctrl+↑↓ 引っかかり調査 (= ui-responsiveness.md §5) と同じ
  「per-frame な見えない同期 I/O」の典型パターン

**該当チェックリスト** ([ui-responsiveness.md §4](ui-responsiveness.md)):
> - [ ] **SQLite クエリ**: キャッシュヒット時は OK だが、cold open や大量 SELECT は
>       worker thread に出す。

→ 該当: 「**毎フレーム** SELECT する設計はチェックリスト違反」。

**修正方針** (推奨):

`App` にキャッシュフィールドを追加:
```rust
struct App {
    // ...
    /// 現在 fullscreen で再生中の動画のブックマーク・ピン キャッシュ。
    /// fullscreen_idx が変わる / B キー追加 / 削除 / リネーム で invalidate。
    /// 毎フレーム SQLite を叩かないようにするための per-fullscreen キャッシュ。
    fullscreen_video_marker_cache: Option<FullscreenVideoMarkerCache>,
}

struct FullscreenVideoMarkerCache {
    fs_idx: usize,
    path: PathBuf,
    pin_pts: Option<f64>,
    bookmarks: Vec<VideoBookmark>,
}
```

更新タイミング:
- `open_fullscreen` で動画を開いたとき (キャッシュ構築)
- `add_native_video_bookmark` / `remove_native_video_bookmark` /
  `update_video_bookmark_title` (line 14248, 14269, 14299, 14543) で invalidate or 増減
- `set_pin` / `remove_pin` (line 14333, 14357) で invalidate
- `close_fullscreen` でキャッシュクリア

`sync_native_video_timeline_markers` は cache を参照するだけ → SQLite 接触ゼロ。

**修正コスト**: 中。10〜15 箇所の修正点があるが、すべて呼出し点が判明している。
キャッシュ構造体の導入と無効化フックの配線で 1〜2 時間程度。

**Tier 位置付け**: **Tier 0.5** (Tier 1 #2 の前に修正)。

---

## 🟡 既知の制限

### #2 CPU 経路 (非 DX12) での `ctx.load_texture` 毎フレーム

**場所**: [src/video/mod.rs:3142](../src/video/mod.rs)
```rust
self.texture =
    Some(ctx.load_texture(label, color, TextureOptions::LINEAR));
```
(以降は `tex.set(color, TextureOptions::LINEAR)` で在テクスチャを上書きする)

**条件**: wgpu backend が DX12 以外 (Vulkan / WARP / RDP 等) のときに動画フレームを
CPU 経路 (swscale → RGBA → load_texture) で UI スレッドに上げる。

**コスト**:
- 4K RGBA で 26-58ms/枚 (CLAUDE.md 記載値)
- 30fps 動画で 33ms 周期 → CPU 経路では UI が常時 50%+ ブロックされる感覚

**現状判断**: [docs/video-architecture.md](video-architecture.md) で「非 DX12 環境
(リモートデスクトップ等) での fallback、4K は重い」と documented。GPU 経路では
`egui_wgpu::CallbackTrait` 経由で `load_texture` を経由しないため発生しない。

**修正は将来課題**: CPU 経路でも `egui::TextureHandle::set` で 1 回確保した
テクスチャを書き換える方式に統一すれば再 alloc コストは消える。コード上は既にそう
なっていて、初回のみ `load_texture` で枠を確保している。

**Tier 位置付け**: **修正不要** (= 現状のコードは既に最適化済、初回 1 回のみ
load_texture)。本書には記録のみ。

---

## ✅ 確認済 (worker thread 化されている)

### 動画再生

| 機能 | 場所 | 確認 |
|---|---|---|
| `VideoPlayer::open` の重い処理 | [src/video/mod.rs](../src/video/mod.rs) | `decoder::spawn` (3-thread) / `audio::start` / `NativeVideoOutput::spawn` (`native-video-presenter` thread) で全て worker 化、UI thread はチャネル / Arc を渡すだけ |
| `NativeVideoPresenter::new` (D3D11 device + COM init + DComp tree + font load) | [src/video/native_presenter.rs](../src/video/native_presenter.rs) | `run_native_video_output` 内で実行 = `native-video-presenter` 専用 thread。UI thread には影響なし |
| 動画フォント読み込み (`std::fs::read("C:\\Windows\\Fonts\\...")`) | [native_presenter.rs:546-571](../src/video/native_presenter.rs) | `configure_overlay_fonts` は `NativeVideoPresenter::new` 内で 1 回のみ実行、worker thread |
| 動画 thumbnail worker | [src/video/thumbnail.rs](../src/video/thumbnail.rs) | `ThumbnailWorker::spawn` で専用 thread |
| Tile thumbnail 抽出 | [src/video/tile_thumbnails.rs](../src/video/tile_thumbnails.rs) | `start_extraction` で専用 thread。UI からはチャネルで結果を受け取るのみ |
| Tile thumbnail SQLite キャッシュ | [src/video/tile_thumb_cache.rs](../src/video/tile_thumb_cache.rs) | DB open は startup で 1 回 (`startup.db_open_video_tile_cache` perf event)。読み書きは抽出 worker からのみ |

### VST3

| 機能 | 場所 | 確認 |
|---|---|---|
| `dsp::add_plugin` (~数百ms 〜 数秒) | [src/video/dsp/mod.rs:582](../src/video/dsp/mod.rs) | 全呼出し点が worker thread: `vst3-startup-load` ([app.rs:185](../src/app.rs)) / `vst3-chain-rebuild` ([vst3_actions.rs:262](../src/ui_dialogs/vst3_actions.rs)) |
| `set_all_guis_visible_async(target_visible=true)` | [dsp/mod.rs:1363](../src/video/dsp/mod.rs) | show=true は GUI attach が秒オーダーになり得るため明示的に async (background worker)。show=false は SetWindowPos のみで sync OK |
| `set_all_guis_topmost` | [dsp/mod.rs:1547](../src/video/dsp/mod.rs) | atomic store + per-slot SetWindowPos (μs オーダー)。sync OK |
| Plugin scanner (`scan` / `scan_with_audio_probe_progress`) | [src/video/dsp/scanner.rs](../src/video/dsp/scanner.rs) | 環境設定→VST3 ページから別 thread で起動、進捗は mpsc で UI に送信 |
| `process_block` (Tier 1 #1 簡素化後) | [dsp/mod.rs:process_block](../src/video/dsp/mod.rs) | audio-pump thread からのみ呼ばれる (cpal RT ではない)、UI thread からは呼ばれない |

### TensorRT

| 機能 | 場所 | 確認 |
|---|---|---|
| `TrtWorkerPool::start` (子プロセス spawn + モデルロード) | [src/ai/trt_worker_pool.rs](../src/ai/trt_worker_pool.rs) | `App::spawn_trt_worker_pool_guarded` ([app.rs:11302, 11317, 11323](../src/app.rs)) で background thread から起動、`trt_restart_in_flight: AtomicBool` で多重 spawn ガード |
| 自動再起動 (silent recovery 3 回) | [app.rs poll_trt_worker_notice](../src/app.rs) | `poll_trt_worker_notice` は UI thread で毎フレーム呼ばれるが、判定後の再 spawn は `spawn_trt_worker_pool_guarded` で background thread に投げる |
| TRT pack DL / インストール | [src/ai/tensorrt_installer.rs](../src/ai/tensorrt_installer.rs) | UI から起動するインストーラは独立 thread + 進捗 mpsc。HTTP DL / extraction は UI thread に到達しない |
| `--tensorrt-build` engine builder | [src/ai/tensorrt_builder.rs](../src/ai/tensorrt_builder.rs) | 子プロセス分岐、メイン UI とは別 process |

### オフライン動画アップスケール

| 機能 | 場所 | 確認 |
|---|---|---|
| `run_job` (FFmpeg open / segment encode / concat / mux) | [src/video/upscale/job.rs](../src/video/upscale/job.rs) | 専用 worker thread (= `video-upscale-job-N`)、UI からは TaskQueue 越しにキューイングするのみ |
| `probe_video_info` (FFmpeg avformat::input) | [upscale/job.rs:396](../src/video/upscale/job.rs) | `run_job` 内 = worker thread |
| Manifest 永続化 (`save_json_atomic` / `load_json`) | [upscale/manifest.rs](../src/video/upscale/manifest.rs) | worker thread |
| Sidecar 検証 (`File::open` + `read_to_end`) | [upscale/sidecar.rs:126](../src/video/upscale/sidecar.rs) | worker thread |
| Disk space 計算 | [upscale/disk.rs](../src/video/upscale/disk.rs) | worker thread |
| Resumable queue 永続化 (`fs::read_to_string`) | [upscale/queue.rs](../src/video/upscale/queue.rs) | startup + worker。UI thread は `TaskQueue` の atomic 操作のみ |

### 動画関連 DB

| 機能 | 場所 | 確認 |
|---|---|---|
| `VideoPinDb::open` / `VideoBookmarkDb::open` / `TileThumbCache::open` | [app.rs:2610-2620](../src/app.rs) | startup で 1 回のみ。`startup.db_open_video_*` perf event で計測されており通常 1ms 以下 |
| `video_pin_db.lookup_pts` (folder load 時の grid サムネ pin 適用) | [app.rs:5751](../src/app.rs) | `spawn_folder_nav` の worker thread から呼ばれる (folder load は worker 化済) |
| `video_pin_db.set_pin` / `remove` (UI 操作) | [app.rs:14322-14358](../src/app.rs) | UI 操作 (B キー / 📌 ボタン) は infrequent (人が押した瞬間 1 回)。SQLite UPDATE は数 ms |
| `video_bookmark_db.remove` / `update_title` (UI 操作) | [app.rs:14245, 14266](../src/app.rs) | 同上 (infrequent UI イベント) |

### サイドカー / 同名画像 / フォルダ走査

| 機能 | 場所 | 確認 |
|---|---|---|
| `skip_image_if_video_exists` の同名画像フィルタ | [app.rs:8991](../src/app.rs) | `filter_video_image_duplicates` は `load_folder_with_scan` の中で worker thread 内で実行。UI thread はキャッシュ越しに参照 |
| FFmpeg DLL `init` (`exists()` チェック) | [video/ffmpeg_loader.rs:48](../src/video/ffmpeg_loader.rs) | `OnceLock` で 1 回のみ、初回は VideoPlayer::open の中だが、これは UI から `start_fs_load` 経由で呼ばれる ⚠ **要確認** |

---

## ⚠ 追加確認が必要な項目

### `ffmpeg_loader::init` (UI 同期の可能性)

[src/video/mod.rs:2000](../src/video/mod.rs):
```rust
pub fn open(...) -> Self {
    if let Err(e) = ffmpeg_loader::init() {
        // dummy player を返す
    }
    // ...
}
```

`VideoPlayer::open` は `start_fs_load` から呼ばれ、その大元は `App::open_fullscreen`
(UI thread)。動画を **初めて** 開いたときに以下が UI thread で実行される:

1. FFmpeg DLL を APPDATA に展開 (= `include_bytes!` のバイト列を `std::fs::write`)
2. `SetDllDirectoryW` 呼び出し
3. `LoadLibrary` で 5 個の DLL をロード

**コスト**: 初回 100ms〜数百 ms (DLL 展開 + ロード)。**ランチャー側ではなく core
側で初回フルスクリーン open 時に走る**。

ただし v0.9.0 のランチャー方式では起動時に core spawn されているので、core 自身は
DLL を **既に隣同居している前提**で `LoadLibrary` できる (= 展開コストはゼロ、
ロードのみ ~10ms)。実害は小さいが、新規ユーザーが初めて動画を開くと UI が一瞬止まる
可能性。

**判断**: 🟡 計測ベースで判断。`startup.video_init_load_dlls` perf event を追加して
実機計測 → 50ms 超なら start_fs_load 前段で worker 化を検討。リリース blocker ではない。

---

## Codex に確認してほしい点

1. **#1 (毎フレーム SQLite クエリ) の修正方針**:
   - キャッシュ + 無効化方式は妥当か?
   - 無効化フックの漏れはないか? (10〜15 箇所の bookmark / pin 操作点)
   - 別動画への切替時に確実にキャッシュクリアされるか?
   - 並行する mouse-move event との race は無いか? (UI thread 単独なので race は無いはずだが念のため)
2. **見落としチェック**: 上記の `App::update` 毎フレーム呼出し系統 (`sync_native_video_*`) で
   他に SQLite / file I/O / ctx.load_texture 連発などがあるか
3. **Tile mode の native presenter 経路**: タイル一覧表示中の `sync_native_video_tile_overlay`
   や `sync_jump_entry_textures` で per-frame `load_texture` が複数発生していないか
4. **VST3 GUI 操作の clossing 路 (`set_existing_guis_owner_to_main` 等)** が UI thread で
   重い処理になっていないか (= app.rs:11017-11019 のシーケンス)
5. **Native presenter init の起動コスト**: VideoPlayer::open は構造的に UI thread を
   ブロックしないが、`NativeVideoOutput::spawn` の thread 起動 + `info_rx` 待ち合わせなどが
   後段で `App::update` を待たせるパスが無いか

## 反映方針

1. `#1` は Codex で妥当性を確認し、Tier 0.5 として実装済み
2. 実装後の差分レビューと automated check で P1 ゼロを確認する
3. P1 ゼロを確認したら Tier 1 #2 (native_presenter.rs drawing fn 抽出) に進む
4. Tier 1 #2 完了時にもこの監査を再実行 (= リファクタで新たな同期 I/O が混入していないか)

## ロールバック・継続性

本書は監査の **スナップショット** であり、リリース後も `docs/ui-responsiveness.md`
本体に統合する作業は別途行う。本書は v0.9.0 リリース時点の実装に対する点検記録として
保存する。
