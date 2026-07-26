# v2.3.0 出荷前レビュー: UI 応答性・性能 (ClaudeCode Fable)

レビュー日: 2026-07-09。対象範囲: `7eff5a9e..01910684` (v2.2.0 → HEAD)。
観点: brief.md「UI 応答性・性能」(同期 I/O / load_texture / 毎フレーム重計算 /
cpal 共有 Mutex / mpsc 無制限成長 / detached 窓数倍化 / perf 計装)。
調査方法: 差分の追加行を機械 grep → 現行 HEAD を Read → UI スレッド (`App::update`
同期到達) まで呼び出し経路を遡って確認。**コード修正なし・指摘レポートのみ。**

---

## 指摘一覧 (深刻度順)

### [P2] メイン文脈のページ送りごとに ParkedLive 音声窓の解析・PCM が全破棄→フル再デコードされる (churn)

- 場所:
  - src/app.rs:29430-29437 (`open_fullscreen` — 「音声以外を開いたら `clear_music_view_state()`」)
  - src/app.rs:35684 (`close_fullscreen` — 同上)
  - src/app.rs:26389-26407 (`update_parked_live_audio_music_view_state` — parked poll が毎フレーム `ensure_music_analysis` を再駆動)
  - src/app.rs:34138-34212 (`ensure_music_analysis` — path 不一致で cancel + 全 clear + worker respawn)
  - src/app.rs:34330-34339 (`clear_music_view_state` — parked 文脈保護のガードなし)
- シナリオ: F12 detached 窓で音声 (または動画→音声モード) を再生 → メイン窓へ戻って
  live-park (`park_current_viewer_context_as_live_media`、ParkedLive 窓で BGM 継続) →
  メイン文脈でグリッドから**画像**をフルスクリーンで開いてページ送り。
  `open_fullscreen` は開いた item が `GridItem::Audio` でない限り毎回
  `clear_music_view_state()` を呼ぶが、このとき音楽解析シングルトン
  (`music_analysis_path` / `music_analysis` / `music_pcm` / `music_spectrum` /
  `music_timeline_cache`) は **ParkedLive 窓の所有物**である。破棄の次フレームに
  `poll_parked_live_detached_windows` → `update_parked_live_audio_music_view_state` が
  `ensure_music_analysis(parked path)` を呼び直し、解析ワーカーがゼロから respawn される。
  - ユーザー視点の症状: ①メインで画像を 1 ページ送るたびに、parked 窓の live スペクトラム
    (draw_parked_live_music_window は spectrum を実描画する、src/ui_fullscreen.rs:4566-4579)
    が消えて「解析中」状態に戻り、デコードが再生位置へ追い付くまで (長尺ファイルで数十秒)
    blank のまま。②毎ページ送りごとにフルデコード (FFmpeg 全尺 decode + 48kHz stereo f32
    バッファの `try_reserve` = 1 時間ファイルで約 1.4GB) が再起動し、ページ送りがデコード
    完了より速いと CPU 1 コア + ディスク read + GB 級アロケーションが**恒久的に churn**する
    (音声再生自体は player 側なので途切れない = 気付きにくい)。
  - timeline 解析自体は LRU (`music_analysis_lru`, bin 予算 1.5M) がヒットするので
    `want_analysis=false` になるが、**spectrum 用 PCM デコードは LRU ヒットでも毎回走る**
    (src/app.rs:34137 のコメントどおり) ため、churn の主コストは消えない。
- 根拠: 上記コード経路。`clear_music_view_state` は「フルスクリーン内で音声→画像/動画へ
  移動したときの teardown」(Codex Inc 4 P2) として入ったもので、ParkedLive (メイン文脈と
  独立に音声が生きている) の存在を考慮していない。音楽状態が bundle 外グローバルである
  ことが根本原因。
- BA マッピング: **BA-7 (状態分散 / bundle 外グローバル) 系**。findings-12 D3
  (「book open が bundle 外グローバル auto_aspect / queue を汚染」) と同型で、
  music シングルトン (`music_analysis_*` / `music_pcm` / `music_spectrum`) が
  `ViewerContextBundle` に入っていない (bundle に入っているのは `music_bookmarks` /
  `music_bookmarks_loaded_for` のみ、src/app.rs:2273-2274)。修正パッチではなく
  リワーク側で「music 状態の bundle 収容 or parked 文脈保護ゲート」として扱うべき。
- 確度: **中** (静的解析のみ・実機未確認。live-park → メイン画像ナビの並行操作が前提。
  経路の到達性はコード上すべて確認済み: parked poll は無条件、clear にガードなし、
  ensure の path 比較は clear 後必ず不一致)。

### [P3] 音楽サブシステム全体に perf::event 計装がゼロ (§4 チェックリスト違反)

- 場所: src/ui_music_timeline.rs / src/ui_music_spectrum.rs / src/ui_music_panels.rs /
  src/audio_decode.rs (全ファイルで `perf::event` 0 件)、src/app.rs の音楽経路
  (`ensure_music_analysis` / `poll_music_analysis` / `run_music_analysis`) にも無し。
- シナリオ: 音楽ビューで hitches が出たとき `--perf-log` +
  `scripts/analyze_perf.py hitches` で原因区間を特定できない。特に以下は計装対象として
  妥当な新規同期区間:
  - timeline row texture の `ctx.load_texture` (src/ui_music_timeline.rs:247。最大
    4096×(116×ppp)px RGBA ≈ 2-6MB/枚。1 枚/frame 予算はあるが upload_ms の実測手段がない)
  - `poll_music_analysis` の TimelineComplete 受領 (数十 MB の bins move + LRU 挿入)
  - 解析ワーカーの milestone (probe / partial N / final / PCM 完了) — nav との相関を
    取るのに必要
- 根拠: docs/ui-responsiveness.md §4「perf 計装: 該当区間に perf::event を追加」。
  同じ v2.3.0 範囲でも PDF / scroll 系は計装が追加されているのに対し、音楽系は皆無。
- 確度: **高** (grep で機械的に確認)。

### [P3] タイムライン row texture キャッシュに offscreen eviction がなく、長尺ファイルで GPU メモリが数百 MB〜GB 級まで単調増加

- 場所: src/ui_music_timeline.rs:79-98 (`TimelineTextureCache.rows: Vec<Option<TimelineRowTexture>>`)、
  247-259 (`ctx.load_texture`)、172-186 (`ensure` — key 変化時のみ全 clear)
- シナリオ: 1 時間超の音声 (DJ mix / コンサート録音) を音楽ビューで開き、Row=10s に
  切り替えて全体をスクロール閲覧 (または 1 曲を最後まで再生 = 追従スクロールが全行を可視化)。
  row 数 = 3600/10 = 360。可視化された row の texture (4096×174px @ppp1.5 ≈ 2.85MB) が
  **一度作られると file 変更 / row_secs 変更まで解放されない** → 最悪 ~1GB VRAM。
  既定 Row=30s でも 120 row ≈ 340MB。VRAM 逼迫環境では他のサムネ/フルスクリーン
  texture を圧迫し、システム全体の描画が重くなる。
- 根拠: `rows` への挿入 (`poll_finished_rows`) はあるが、可視範囲外の row texture を
  drop する経路が `clear()` / `ensure()` (key 変化) 以外に存在しない。raster 要求側は
  可視行 + focus 行に限定されている (`prioritized_timeline_request_rows`) ので、
  増加は「一度でも可視化した行」に限られる = 通常曲 (数分、~10 rows) では問題にならない。
- 確度: **高** (機構)、実害の体感は **中** (長尺 + 全域閲覧が条件。決定論優先ポリシー
  (feedback_deterministic_over_adaptive) 的には許容側だが、無上限は明記に値する)。

### [P3] 解析ワーカーの `analyze_stereo_timeline` 実行中は cancel が効かず、連続ナビで GB 級 PCM が一時的に多重常駐する

- 場所: src/app.rs:5320-5335 (partial 解析、cancel チェックは前後のみ) /
  5358-5360 (final 解析、cancel 不可) / src/ui_music_spectrum.rs:191-194 (`with_prefix`)
- シナリオ: 長尺音声 A を音楽ビューで開く → 解析途中で ↑↓ 連打して別音声 B, C… へ移動。
  `ensure_music_analysis` は旧 worker に cancel を立てるが、旧 worker が partial/final の
  `analyze_stereo_timeline` (1 時間 prefix で秒単位) を実行中だと完走まで抜けられず、
  その間 旧 PCM バッファ (~1.4GB/1h) を Arc で保持し続ける。新 worker も自分の PCM を
  reserve するので、ナビが速いと**複数 worker × GB バッファが一時的に併存**し、
  メモリスパイク (最悪ページング) になる。UI スレッドはブロックされない。
- 根拠: decode ループの cancel はパケット境界で速効 (src/audio_decode.rs:314-317) だが、
  `analyze_stereo_timeline` は cancel トークンを受け取らない設計 (コメントにも
  「cancel 不可なので前後で確認」と明記)。
- 確度: **中** (機構は確実。実スパイク量は解析所要時間に依存し未実測)。

### [P3] ParkedLive 音声再生中はアプリ全体が ~60fps 常時再描画 + spectrum FFT 常時稼働

- 場所: src/app.rs:45544-45553 (`poll_video` — playing 中は repaint 間隔を 16ms に clamp)、
  src/app.rs:26446-26484 (`poll_parked_live_detached_windows` — parked bundle でも毎フレーム
  `poll_video`)、src/ui_music_spectrum.rs:404-406 (`update` — playing 中 16ms repaint)、
  src/ui_fullscreen.rs:4588 (parked 窓自身も 50ms repaint)
- シナリオ: 音声を ParkedLive 窓に park して BGM として流しながら、メイン窓ではグリッドを
  ただ眺めている (操作なし)。それでも app 全体 (メイングリッド + 全 immediate viewport) が
  ~60fps で再描画され続け、spectrum worker は ±1 秒窓 (~96k サンプル) の FFT を毎 16ms
  回し続ける。ノート PC でのバッテリー消費 / 他アプリ (ゲーム等) の GPU 帯域圧迫。
- 根拠: parked 窓は live スペクトラムを実描画する設計 (draw_parked_live_music_window が
  `music_spectrum.draw` を呼ぶ) なので 60fps 駆動自体は意図的。ただし parked 窓の表示物は
  小さな spectrum + mm:ss のみで、メイングリッド側の 60fps 再描画は純粋な巻き添え。
  通常フルスクリーン動画再生中と同じコスト構造であり、新規の退行ではなく「park しても
  再生中はアイドル省電力に落ちない」という仕様上の注意点。
- 確度: **中** (repaint 経路はコードで確認。実測 fps / 消費は未計測)。

### [P3] `draw_beat_grid` が可視 row ごとに全 beat / bar を線形走査 (O(beats × 可視行) / frame)

- 場所: src/ui_music_timeline.rs:1876-1937 (`draw_beat_grid`、`for beat in
  &analysis.beat_grid.beats` を row ごとに全走査)、呼び出しは 602 (可視 row ループ内)
- シナリオ: 1 時間 / 174BPM 級で beats ≈ 10k 本 + bars ≈ 2.6k 本 × 可視 6 row ≈
  毎フレーム ~76k 回の範囲判定。ほかの bin 参照 (`timeline_bin_at_time` 等) が
  `partition_point` で対数化されているのに対しここだけ線形。
- 根拠: 上記コード。ただし判定は f64 比較 2 回程度で、76k 回でも ~0.1ms 級。
  60fps を脅かす量ではない。beats も `partition_point` で範囲を切り出せば消える。
- 確度: **高** (機構)。実害は**ほぼ無し** (品質メモとして記録)。

### [P3] `render_detached_image_windows` が毎フレーム `detached_image_windows` 全体を clone

- 場所: src/ui_fullscreen.rs:5054 (`let windows = self.detached_image_windows.clone();`)、
  および deferred 窓ごとの `DeferredDetachedImageWindowView::from_snapshot`
  (frozen_continuous_pages Vec + String 群の clone、src/app.rs:250-268)
- シナリオ: passive detached 窓 N 枚保持中、毎フレーム N 個の snapshot clone
  (TextureHandle clone は安価だが、title/location String + frozen page Vec + descriptor の
  heap alloc が毎フレーム発生)。N が小さい (数枚) 前提では µs 級で実害なし。
- 根拠: 上記コード。borrow 分離のための clone で、機能上は正しい。
- BA マッピング: 構造は R2 (状態集約) の管轄。凍結中のため記録のみ。
- 確度: **高** (機構)、実害は**低**。

---

## 確認済み・問題なし (観点別)

### 1. UI スレッドからの同期ファイル I/O

- **src/audio_decode.rs**: `analyze_audio_file*` / `decode_audio_file_progressive` /
  `probe_audio_file` の呼び出し元は `run_music_analysis` (src/app.rs:5244) のみで、
  `ensure_music_analysis` が `miv-music-analysis` スレッドに spawn する (app.rs:34192-34198)。
  UI スレッド到達なし。
- **`run_music_analysis` 内の `std::fs::metadata`** (app.rs:5271) は worker 側。UI 側は
  `image_metas` スナップショットを使い stat しない (設計コメント通りに実装されている)。
- **詳細ビューの音声 probe** (`probe_audio_details`, src/app/metadata_ops.rs:809) は
  cancel トークン + 10 秒 deadline 付きで詳細ワーカー側。UI 到達なし。
- **`ensure_music_bookmarks_loaded`** (src/ui_music_panels.rs:156) は UI スレッドで
  SQLite SELECT だが path 変化時のみの one-shot (warm DB、既存パターン準拠)。
- **fs_animation.rs の `File::open` 化** (decode_gif/apng/webp_frames): 呼び出しは
  `start_fs_load` worker 内 (app.rs:34970-35026)。UI 到達なし。むしろ全読み→streaming 化
  の改善。
- app.rs の diff に現れる `is_dir()`/`metadata` 追加行の大半は移動コード
  (`try_apply_pdf_meta_cache` / `setup_virtual_folder_seed_and_writeback` / D&D partition 等
  は v2.2.0 に既存であることを `git show 7eff5a9e:src/app.rs` で確認)。
- **動画 resume 5 秒毎保存** (poll_video, app.rs:45833-45847) は in-memory HashMap 更新のみ
  (ディスク書き込みは別経路の settings.save)。

### 2. `ctx.load_texture` の頻度・サイズ

- **timeline row raster**: worker rasterize + `TIMELINE_ROW_TEXTURE_UPLOAD_BUDGET_PER_FRAME=1`
  (src/ui_music_timeline.rs:55, 211-270) で 1 枚/frame にペーシング済み。generation /
  row_version 不一致の stale 結果は upload せず破棄。設計は §3.1 準拠 (計装欠如のみ上記 P3)。
- **spectrum / 鍵盤 / HUD**: painter プリミティブのみで texture なし。
- 範囲内で新規追加された他の `load_texture` は one-shot (parked live 1×1 backdrop、
  edit result upload) またはテストコード。`poll_comic_bake` の複数枚/frame upload は
  v2.2.0 既存 (範囲外)。

### 3. 毎フレームの重い再計算 (長時間ファイルスケール)

- `draw_music_timeline` は可視 row のみ描画、bin 参照は `partition_point`
  (timeline_bins_window_range / timeline_bin_at_time)。hover 計算も O(log n)。
- 鍵盤の `compute_keyboard_visuals` / `update_keyboard_sustain` は O(note_count=132) ×
  窓 13 の小ソート/frame — µs 級。
- progressive partial は幾何級数 (2s→倍々、全長 50% で停止 + 150ms throttle) で
  再解析総コスト ≤ 全長 2x が unit test で固定されている (audio_decode.rs:635-657)。
- partial 差し替え時の全 row 再ラスタも partial 回数が有界なので有界。
- 例外は `draw_beat_grid` の線形走査 (上記 P3、実害ほぼ無し)。
- **MusicPcm 常駐** (1h ≈ 1.4GB、reserve 上限 4h=5.5GB clamp、`try_reserve` で
  OOM abort 回避、close/非音声 open で解放) は docs 明記の設計内 (決定論優先ポリシー
  整合) と判断。churn (P2) と多重常駐 (P3) のみ指摘。

### 4. cpal 音声コールバックと UI の共有 Mutex

- cpal callback が触る `Arc<Mutex<AudioBuffer>>` の相手は audio-pump スレッドのみ。
  UI スレッドは `AvClock` の atomic (`position`/`is_playing`/`volume`) と
  `last_displayed_pts_bits` (atomic) を読むだけで buffer Mutex を取らない。
  範囲内の変更 (preroll release edge snap) も pump スレッド内で完結 (src/video/audio.rs)。
- **MusicPcm の RwLock**: UI スレッドは `is_complete` (atomic) と Arc clone のみで
  lock を取らない。長い解析 read (`with_prefix`) と spectrum の窓コピー read は並行可能、
  writer (`append`) は解析と同一スレッドで逐次 → writer 待ちによる read 飢餓なし
  (Mutex→RwLock 化の設計コメント通り。実機 FB 2026-07-07 対応済み)。

### 5. mpsc 無制限成長 / poll の 1 フレーム全件処理

- `MusicAnalysisMsg` チャネル: メッセージ数は probe 1 + Pcm 1 + partial ≤ ~15 (幾何級数) +
  final 1 で有界。`poll_music_analysis` は全 drain するが件数・サイズとも有界。
  pending 中の repaint は 50ms throttle (busy spin なし)。
- spectrum request チャネル: `pending` フラグで in-flight ≤ 1、worker 側も最新のみ
  coalesce (run_music_spectrum_worker の try_recv ループ)。
- timeline raster: 要求は row ごとの pending dedup 付き、結果 drain は budget=1
  (stale は無コスト破棄)。要求対象が可視行 + focus 行に限定されるためチャネル滞留は
  最大でも可視行数オーダー。
- native presenter イベント / detached activation watcher / deferred window イベントは
  毎フレーム全 drain + 件数有界。watcher は専用スレッド 8ms ポーリングで UI 外。

### 6. detached 複数窓 + メインのフレーム毎仕事の重複

- **passive still 窓 = `show_viewport_deferred`** (src/ui_fullscreen.rs:5106): 表示専用
  snapshot をイベント駆動で描く。メイン frame ごとの仕事は builder + view clone の
  再登録のみ (上記 P3 微小)。窓数に比例して重い描画が走る構造ではない。
- **ParkedLive 窓 = `show_viewport_immediate`** (5234) + `poll_parked_live_detached_windows`
  の bundle swap + `poll_video`: メイン frame に同期加算されるが、
  `close_parked_live_media_windows_for_new_media` (app.rs:29203) が新メディア open 時に
  全 parked media 窓を閉じるため **media の ParkedLive は実質 1 枚**に抑えられている。
  また `should_poll_main_video_context` (app.rs:26517) が main 側 poll_video と排他して
  二重 tick を防いでいる。`swap_viewer_context_bundle` は mem::swap の集合で O(1) 級。
- 音楽解析シングルトンの文脈間衝突 (ping-pong) は「音声 open が parked を先に閉じる」
  ことで毎フレーム級の相互 cancel は起きない (ただし画像ナビ経由の churn = 上記 P2)。

### 7. 新規同期区間の perf 計装

- 音楽サブシステム: **ゼロ** (上記 P3)。
- detached リワーク側: perf::event ではなく `log_detached_image_window_debug`
  (env gate + 600 frame 間引き) 中心。恒常コストは小さいが、hitches 分析には載らない。
  凍結中のため指摘はリワーク側課題として記録のみ。

---

## 総括

- 音楽統合は「decode / 解析 / FFT / rasterize を全て worker 化し、UI は atomic 読み +
  Arc 渡し + 1 枚/frame upload に限定する」という §1-§3 の設計原則に忠実で、
  UI スレッドの同期 I/O・ブロッキングは新規経路には見つからなかった。
- 残る問題は (a) ParkedLive × メイン画像ナビでの解析 churn (P2、bundle 外グローバルの
  detached 憲法問題)、(b) 長尺ファイルでのメモリ上限なし (timeline texture / PCM 多重常駐、
  P3×2)、(c) perf 計装の欠如 (P3) に集約される。
