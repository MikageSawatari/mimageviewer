# 動画タイルモード中の動画切替を高速化する

**Status**: ClaudeCode レビュー反映済み (2026-05-06)。この計画を基準に Codex が実装する。

## 1. 目的

mIV のフルスクリーン動画再生中に `S` キーで動画タイルモードに入り、ホイールで前後の動画へ移動すると、現状はタイル UI とネイティブ動画表示が一度破棄され、次の動画を開いたあとにタイル UI が再生成される。そのため 100-300ms 程度の画面切替フラッシュが見える。

目的は、動画ばかりのフォルダで目的の動画を探す操作を高速化すること。動画タイルモード中の動画から動画へのホイール移動では、ネイティブ表示 HWND とタイル UI のモードを維持し、内部の動画ソースだけを差し替える。

## 2. 操作仕様

- 対象は「動画タイルモード中の動画から動画へのホイール移動」。
- 移動先も動画なら、タイルモードを維持する。
- 移動先動画は保存済み resume 位置を尊重する。
- 切替直後は一時停止状態にする。`S` でタイルを解除したら、前回の resume 位置の動画表示へ戻れる。
- 最初から、または特定位置から見たい場合は、ユーザーがタイルサムネイルをクリックして seek する。
- 動画から画像、画像から動画、境界到達などは既存挙動を維持する。

## 3. 現状フロー

動画 A のタイルモード中にホイールで動画 B へ移動すると、主に次の経路を通る。

```text
Native wheel event
  -> App::navigate_native_video_fullscreen
  -> App::open_native_video_fullscreen_from_navigation
  -> video_tile_state = None
  -> video_tile_textures.clear()
  -> video_tile_reopen_pending = true
  -> App::open_fullscreen(idx)
  -> App::start_fs_load(idx)
  -> old FsCacheEntry::Video remove/drop
  -> VideoPlayer::shutdown()
  -> AudioOutput::Drop
  -> new VideoPlayer::open()
  -> new NativeVideoOutput::spawn()
  -> player.info() 到着後に toggle_video_tile_mode()
```

主な遅延要因は次の 3 つ。

- `NativeVideoOutput` が動画ごとに破棄/再生成され、HWND、D3D11 presenter、overlay が作り直される。
- `VideoPlayer::shutdown()` の途中で `AudioOutput::Drop` が pump thread を同期 join し、UI スレッドを止め得る。
- `video_tile_reopen_pending` が `player.info()` を待って 80ms 間隔で再試行するため、タイル UI が一度消える。

## 4. 設計方針

中心方針は、`NativeVideoOutput` を動画ソースより長生きする表示器として扱い、動画タイルモード中の動画から動画への移動では破棄しないこと。

旧 `VideoPlayer` から `NativeVideoOutput` を取り外し、新 `VideoPlayer` に attach する。presenter thread には `NativeVideoOutputCommand::SwitchSource` を送り、HWND、D3D11 presenter、overlay を維持したまま、`video_rx`、`AvClock`、engine event channel、duration などの source binding だけを差し替える。

タイル UI は `video_tile_state` を即座に閉じず、`video_tile_swap_pending` として pending 状態を持つ。新動画の `player.info()` が到着した時点で新しい `VideoTileState` を構築し、タイルサムネイルを progressive に埋める。pending 中は native overlay に preparing 表示を出す。

## 5. 実装計画

### Step 1: 準備リファクタ

`src/video/mod.rs` の `run_native_video_output` にある source 固有のローカル変数を `PresenterSourceState` にまとめる。

対象例:

- `video_rx`
- `clock`
- `engine_event_tx`
- `displayed_frame_seq`
- `duration_secs_bits`
- `queue`
- `last_seen_serial`
- `first_frame_event_last_epoch`
- `pending_first_frame_event`
- `present_stats`
- `last_present_wall`
- `last_present_source_pts`
- source pacing counters

この step は挙動を変えない純リファクタとして先に分ける。SwitchSource 時に `PresenterSourceState::new(payload)` で reset 漏れを防ぐための準備。

同じ準備 step で、`src/ui_video_tile.rs` の `toggle_video_tile_mode()` から `VideoTileState` 構築処理を `build_video_tile_state_for(fs_idx, screen_size)` へ切り出す。現状は `toggle_video_tile_mode()` 内に inline されているため、swap pending 完了時に「トグル」ではなく「指定動画の state 構築」だけを呼べる形にする。

### Step 2: SwitchSource コマンドを追加する

`NativeVideoOutputCommand` に `SwitchSource` を追加する。

```rust
struct SwitchSourcePayload {
    video_rx: crossbeam_channel::Receiver<VideoFrame>,
    clock: Arc<AvClock>,
    engine_event_tx: crossbeam_channel::Sender<EngineEvent>,
    displayed_frame_seq: Arc<AtomicU64>,
    duration_secs_bits: Arc<AtomicU64>,
    source_epoch: u64,
    show_preparing_overlay: bool,
}
```

SwitchSource 受信時に行うこと:

- `PresenterSourceState` を新 payload で作り直す。
- 未表示 frame queue を drain/reset する。
- `first_presented_out.store(false, Release)` を行う。
- overlay playback status を first-frame pending に戻す。
- `show_preparing_overlay` が true なら `NativeOverlayTileOverlay::preparing()` を表示する。
- stale event 対策として source epoch を更新する。

`first_presented_out` を false に戻すことは必須。これをしないと `native_presenter_pending()` が新動画の first frame 前でも pending ではないと判断してしまう。

### Step 3: stale event 対策を入れる

Native presenter を再利用すると、旧動画で発生した input event が新動画の `fs_idx` に紐付いて処理されるリスクがある。

対策は `source_epoch` を正式採用する。

- `NativeVideoOutputEvent` に `source_epoch` を付与する。
- `NativeVideoOutput` は現在の epoch を保持し、SwitchSource 時に更新する。
- presenter thread が App へ送る event には、その時点の source epoch を付ける。
- App 側は現在の player/native output epoch と一致しない event を破棄する。
- SwitchSource 前後で event queue を drain する処理も補助的に入れてよいが、正しさは epoch filter で担保する。

レビューでは、古い `WheelNavigate`、`Seek`、`TileSeek`、`TogglePlay` が新動画へ誤適用されないことを重点確認する。

### Step 4: VideoPlayer に native output 移譲 API を追加する

`VideoPlayer` に次の API を追加する。

```rust
#[cfg(windows)]
pub(crate) fn take_native_output(&mut self) -> Option<NativeVideoOutput>;

#[cfg(windows)]
pub(crate) fn attach_native_output(&mut self, output: NativeVideoOutput);

#[cfg(windows)]
pub(crate) fn build_switch_source_payload(&self, source_epoch: u64, show_preparing: bool)
    -> SwitchSourcePayload;
```

新 `VideoPlayer` は `native_output_config=None` で構築し、通常の `NativeVideoOutput::spawn()` は走らせない。構築後、旧 player から取り外した native output を attach し、SwitchSource を送る。

### Step 5: App 側 fast path を追加する

`open_native_video_fullscreen_from_navigation` の先頭で、次の条件を満たす場合だけ fast path に入る。

- 現在の `fullscreen_idx` が動画。
- 移動先 `idx` も動画。
- `video_tile_state.is_some()` または `video_tile_swap_pending.is_some()`。
- 現在の `VideoPlayer` が `NativeVideoOutput` を持っている。

fast path の流れ:

1. 旧 player から `NativeVideoOutput` を `take_native_output()` する。
2. 新 `VideoPlayer` を `native_output_config=None` で構築する。
3. resume は通常動画 open と同じく `settings.video_resume_positions` を渡す。
4. autoplay は fast path では `false` に固定し、タイル scrub 中の切替で再生を開始しない。
5. 新 player の SwitchSource payload を作り、native output に送る。
6. native output を新 player に attach し、`fs_cache[target_idx]` に挿入する。
7. `fullscreen_idx`、`selected`、metadata load など `open_fullscreen` 相当の副作用を整える。
8. `video_tile_swap_pending` に target idx/path/epoch を保存し、tile overlay は preparing にする。
9. 最後に旧 video entry を `fs_cache` から remove する。

旧 video entry の remove は最後に行う。旧 player を先に drop すると、presenter thread がまだ旧 `video_rx` を見ている間に sender が close し、SwitchSource 到着まで想定外の disconnected 状態を経由するため。

SwitchSource 後に App 側から再送、または `poll_video` / sync 系で確実に更新すべき overlay 項目:

| 項目 | 目的 |
|---|---|
| `SetMetadata` | 新動画のタイトル、コーデック、チャプターなど |
| `SetTimelineMarkers` | 新動画のブックマーク、タイムラインマーカー |
| `SetJumpEntries` | 新動画の pin / chapter jump |
| `SetLoopEnabled` | 現在設定されている loop 状態 |
| `SetVst3Available` | VST3 有効状態 |
| `SetVst3Panel` | VST3 panel 表示状態 |
| `SetPlaybackStatus` | `first_frame_presented=false, error=None` から開始 |
| `SetTileOverlay(preparing())` | 新 tile state 構築までの準備中表示 |

native output が取れない、target が動画でない、構築に失敗した場合は既存の reopen 経路へ fallback する。

### Step 5.5: pending 中の追加ホイール入力

`video_tile_swap_pending` が存在する間、追加のホイール navigation は無視する。queue も delta accumulation も行わない。

意図:

- Ctrl+上下のナビロックと同じく、準備できていない間の過剰入力で中間動画を大量に開かない。
- ユーザーがホイール操作を止めた時点で、遅れて溜まった移動が発火しない。
- pending が解除されたあと、ユーザーがまだ物理的にホイール操作を続けていれば、その時点の新しい input event だけを処理する。

### Step 6: tile pending rebuild を実装する

`video_tile_swap_pending` を追加する。

```rust
struct VideoTileSwapPending {
    target_idx: usize,
    target_path: PathBuf,
    source_epoch: u64,
    started_at: Instant,
    deadline: Instant,
}
```

`deadline` の目安は 2 秒とする。SwitchSource 経由なら通常は数百 ms で `info()` が来るため、2 秒を超えたら decoder 初期化失敗や異常に近い状態として fallback する。

`poll_video` 後、または `sync_native_video_tile_overlay` 内で、pending target の `player.info()` を確認する。

- info 未到着なら preparing overlay を維持し、短い repaint を要求する。
- info 到着後に `build_video_tile_state_for(target_idx, screen_size)` で新 `VideoTileState` を構築する。
- `video_tile_textures.clear()` を行う。
- pending を解除する。
- 新 worker の snapshot/progress を native overlay に流す。

既存の path mismatch 判定は 2 箇所あるため、両方で pending 例外を入れる。

- `src/app.rs` の `sync_native_video_tile_overlay()` 内の `state.video_path != current_path` 判定。
- `src/ui_video_tile.rs` の `draw_video_tile_overlay()` 内の current path 判定。

pending 中だけは path mismatch でタイル UI を閉じない。pending ではない mismatch は従来どおり close する。

deadline 到達時の動作:

- target player が error を持っている場合は pending を解除し、tile state を閉じ、既存の動画エラー表示へ任せる。
- deadline までに `info()` が来ない場合は pending を解除し、tile state を閉じ、既存 reopen 経路へ fallback する。
- fallback 後も `video_tile_reopen_pending` の既存 3 秒 deadline を超える場合は、従来どおりタイルなしの動画表示に戻る。

### Step 7: egui fullscreen 経路は段階的に扱う

初回実装の主対象は Windows native presenter 経路。

egui fullscreen 経路では `NativeVideoOutput` が存在しないため、同じ SwitchSource は使えない。初回では既存 reopen 経路を維持するか、必要なら tile state を pending で維持する見た目改善だけを行う。

重要なのは、native presenter 無効時に壊れず fallback すること。

### Step 8: prewarm は初回実装から外す

タイルサムネイル prewarm は今回の初回実装には含めない。

理由:

- SwitchSource と native output 移譲だけで変更範囲が大きい。
- prewarm は追加の FFmpeg input、seek、I/O を発生させる。
- まず presenter 再利用で画面フラッシュと UI hitch がどれだけ減るかを測るべき。

必要なら、安定後に別パッチで「隣接動画 1 本だけ prewarm」を検討する。

## 6. AudioOutput の扱い

`AudioOutput::Drop` の pump thread join は UI hitch の要因として残る可能性がある。

ただし初回実装では、まず NativeVideoOutput 再利用と tile pending rebuild に集中する。perf-log で 50-100ms 級の hitch が残る場合、別 step または別 PR で `AudioOutput` shutdown の非同期化を検討する。

非同期化する場合も、cpal stream の pause/drop は同期的に終わらせ、pump thread join だけを background に逃がせるかを慎重に確認する。

## 7. レビュー観点

ClaudeCode には次を重点レビューしてもらう。

1. `PresenterSourceState` に per-source 変数のリセット漏れがないか。
2. SwitchSource 後に `first_presented_out` が false へ戻り、pending 判定が正しいか。
3. source epoch などで stale event が新動画へ誤適用されないか。
4. 新 `VideoPlayer` が `native_output_config=None` で作られ、既存 HWND が再利用されているか。
5. SwitchSource が旧 player drop より前に送られているか。
6. `DspBridge` が通常 open と同じく新 player に渡され、VST3 chain が維持されるか。
7. resume 位置が通常 open と同じく渡され、タイル scrub で 0 秒に強制されていないか。
8. fast path の autoplay が `false` に固定され、タイル scrub で再生が始まらないか。
9. pending 中の追加ホイールが queue されず、過剰入力が蓄積されないか。
10. `video_tile_swap_pending` 中に path mismatch 判定でタイル UI が閉じないか。`sync_native_video_tile_overlay()` と `draw_video_tile_overlay()` の両方を見る。
11. 動画から画像、画像から動画、境界到達、Esc close の既存挙動が壊れていないか。
12. native presenter 無効時に既存 reopen 経路へ fallback できるか。

## 8. 検証手順

### ビルド

```bash
cargo build
cargo build --release
```

リリース作業に近い確認では、既存手順に従って `scripts/build-release.sh` や dependency 確認も行う。

### 手動テスト

| # | シナリオ | 期待動作 |
|---|---|---|
| 1 | MP4 だけのフォルダで動画をフルスクリーン表示し、`S` でタイルモード、ホイール前後 | 画面切替フラッシュなし。タイルモードが維持される |
| 2 | タイルモード中に動画 A から動画 B へ移動 | HWND が再生成されない。preparing overlay から B のタイルへ progressive に更新 |
| 3 | resume 位置がある動画へホイール移動 | 保存 resume 位置を使う。0 秒に強制されない |
| 4 | 切替後に `S` でタイル解除 | resume 位置の動画表示へ戻る |
| 5 | タイルをクリック | クリックした位置へ seek し、既存どおりタイルモードを閉じる |
| 6 | 動画から画像へ移動 | タイル UI は閉じ、通常フルスクリーンになる |
| 7 | 画像から動画へ移動 | タイルモードは自動で開かない |
| 8 | ホイール 10 連打 | 最後の移動先に収束し、古いイベントが誤適用されない |
| 9 | フォルダ末尾/先頭でさらにホイール | boundary hint が表示され、タイルモードは維持 |
| 10 | VST3 GUI 表示中に動画切替 | GUI owner と overlay が破綻しない |
| 11 | native presenter 無効環境 | 既存 reopen 経路で壊れず動作 |

### perf-log

```bash
target/release/mimageviewer.exe --perf-log
python scripts/analyze_perf.py "%APPDATA%\mimageviewer\logs\perf_events.jsonl" hitches
```

期待:

- 動画から動画へのホイール移動で 100ms 級の UI hitch が消える。
- 16ms 以下が理想。残る場合は `AudioOutput::Drop` の同期 join を次の改善対象にする。

## 9. ドキュメント更新

実装時に次を更新する。

- `docs/display-pipeline.md`: 動画タイルモード中の native presenter reuse と overlay 状態遷移。
- `docs/async-architecture.md`: `NativeVideoOutput::SwitchSource` の所有権遷移、source epoch、pending rebuild。
- `htdocs/mimageviewer/manual/`: ユーザー向けに「動画タイルモード中にホイールで前後の動画へ滑らかに切り替えられる」旨を追記。内部実装語は出さない。

## 10. Out of scope

- タイルサムネイル prewarm。
- 音声のシームレス連続再生。
- 動画から画像、画像から動画での native presenter 再利用。
- `AudioOutput::Drop` 非同期化。ただし perf-log 次第で後続対応。

## 11. 関連ファイル

| ファイル | 役割 |
|---|---|
| `src/app.rs` | fullscreen navigation、video tile state、native overlay sync |
| `src/ui_video_tile.rs` | `VideoTileState` 構築と egui tile overlay |
| `src/ui_fullscreen.rs` | fullscreen viewport と legacy reopen 経路 |
| `src/video/mod.rs` | `VideoPlayer`、`NativeVideoOutput`、presenter thread |
| `src/video/native_presenter.rs` | native overlay UI |
| `src/video/audio.rs` | `AudioOutput` と同期 shutdown |
| `src/fs_animation.rs` | `FsCacheEntry::Video` drop |
