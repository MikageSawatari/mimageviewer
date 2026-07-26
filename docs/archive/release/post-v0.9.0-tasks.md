# v0.10 以降タスク一覧

v0.9.0 リリース時点 (2026-05-16) で残った全件をここに集約する。元の `docs/archive/release/v0.9.0-release-tasks.md`
は完了記録 (frozen) として保持し、本ドキュメントが今後の作業対象の単一の真実源。

優先度カテゴリは v0.9.0 レビュー時の Claude / Codex 評価をそのまま引き継いでいる:

- **A. v0.9.0 から v0.10 に持ち越した P2** — v0.9.0 で triage / sign-off は済んでいるが、
  実コード fix が landed していないもの。リリースブロッカ候補だったが、修正規模が大きいか
  doc-only の暫定対応にとどめたため v0.10 に持ち越し。優先度は本ドキュメント内で最上位
  (5 件、うち T14 は 2026-05-18 に解決済み、T41 は v0.9.0 で doc コメントのみ追加、
  残り 3 件は完全未着手)。
- **B. P3 (T58〜T121)** — 「望ましい」修正。実害シナリオは稀 or 単発トラブルで済むが、品質の
  底上げに価値がある項目。
- **C. リファクタ負債 (R-1〜R-10)** — 巨大モジュール分割やテスト整理。コードの保守性向上が
  主目的で、エンドユーザー影響なし。
- **D. v0.9.0 で部分対応した項目の follow-on** — 主経路は v0.9.0 で実装済、残経路 / 真の
  修正 を v0.10 に持ち越した「タスク内 follow-up」。本体の triage は v0.9.0 で済んでいる。

---

## A. v0.9.0 から v0.10 に持ち越した P2 (リリースブロッカ候補)

### A-1. T14 [P2→v0.10][Compat][C+X] EOF 状態が EngineActor に伝わらない ✅ 解決済み (2026-05-18)
- 対応場所: `src/video/mod.rs`, `src/video/engine/actor.rs`, `docs/video-architecture.md`
- 解決内容: native / non-native 両経路の EOF drain 完了後に
  `EngineActor::handle_decoder_event(DecoderEvent::EofReached { epoch, duration_secs })` を同期的に呼ぶ。
  これにより `engine_state_atomic` は EOF 後に `state_code::EOF` を publish し、App 側から
  `VideoPlayer::engine_state_code()` で EOF 到達を検出できる。
- テスト: `eof_reached_freezes_av_clock_and_publishes_eof_state` など、EngineActor の EOF 遷移と
  stale epoch discard を検証する unit test を追加済み。
- 補足: v0.10 の動画連続再生はこの解決済み経路を前提にする。将来 audio drain / loop seek の
  さらなる一本化余地はあるが、EOF 状態 publish 自体は未解決ブロッカではない。

### A-2. T24 [P2→v0.10][Safety][C] wndproc 内 `&'static mut WindowState` aliasing 🛑
- 場所: `src/video/native_presenter/hud_window.rs:637`, `native_window.rs:841`
- 問題: arm ごとに `&mut *ptr` を再生成。`SetCapture` / `claim_foreground` がメッセージを同期的に
  再入させると外側 `&mut` が生きたまま二つ目の `&mut` が作られ Rust aliasing 違反 (UB-prone)
- v0.9.0 で延期した理由: 全 15+ call site を `*mut` + scoped reborrow に refactor するか、
  WindowState 全フィールドを interior mutability (Atomic/RefCell/Mutex) に変換するかの 2 択。
  実発火経路は soak test で未観測のため優先度低と判断
- 修正方針: WindowState 全体を `Cell<...>` ベースに変換が最も clean (UI スレッド単独実行を
  type-system で表現)。AtomicXxx は外との共有がある field のみ

### A-3. T25 [P2→v0.10][Stability][C] D3D11 device-lost を握り潰す 🛑
- 場所: `src/video/native_presenter/mod.rs:1531`
- 問題: `DXGI_ERROR_DEVICE_REMOVED` / `DEVICE_RESET` を汎用 `Err(String)` に潰し、呼び出し側は
  log + 16ms sleep + ループ → device-lost で presenter が永久に黒画面でスピン
- v0.9.0 で延期した理由: presenter 側で device 再生成 (新 ID3D11Device + swap chain +
  backbuffer + GpuVideoDevice の再 attach) or graceful fullscreen 終了 + toast の経路を組む
  必要があり、~300 行の追加実装。低頻度シナリオ (driver crash / GPU リセット)
- v0.9.0 一時策: ESC キー入力経路は cancel atomic を立てるだけで device に依存しないため、
  黒画面に陥ってもユーザーが ESC で抜けられる
- 修正方針: D3D11 device-removed 検出 → fullscreen 終了 + 「グラフィックドライバが応答停止
  しました」toast。device 再生成は cost が高すぎるので v0.10 では非対応にする

### A-4. T41 [P2→v0.10][Compat][C] VFR ソースが無言で CFR 変換 (full fix) 🛑
- 場所: `src/video/upscale/job.rs` finalize-mux ループ
- v0.9.0 対応: 「VFR 動画は CFR に flatten される」挙動を巨大コメントで明示済
- 修正方針 (v0.10): `output_total_pts_ticks` を実 packet timing から導出 + segment 間の
  シームレス連結 (= 各 segment が記録した `source_last_pts` と次 segment の `source_start_pts`
  の差分を活用)

### A-5. T50 [P2→v0.10][Compat][C] engine cache が GPU/driver/TRT バージョン変更で無効化されない 🛑
- 場所: `src/ai/runtime.rs:543`
- 問題: driver 更新後に dead-end「spawn failed」バナー
- v0.9.0 で延期した理由: 真の修正には (a) NVIDIA driver version の runtime 取得 (`nvml.dll` /
  WMI 経由) + INSTALL_OK 記録 + mismatch 検出、(b) mismatch 時の engine cache 自動破棄 +
  再 build フロー、の 2 つが必要。新規依存または Windows API 経由で 100+ 行
- v0.9.0 部分対応: T46 の `PackStatus::Stale` 経路で pack 更新時の検出はカバー済 (manual 再
  install で復旧可能)
- 修正方針 (v0.10): nvml.dll 検出 → driver version 取得 → INSTALL_OK に追記 → 起動時に
  mismatch なら engine cache `tensorrt-engines/*` を一括削除 + 自動再 build (UI 経由)

---

## D. v0.9.0 で部分対応した項目の follow-on

### D-1. T07 follow-on: VST3 bridge discard/reset handshake
- v0.9.0 対応: wedge auto-disable + hysteresis で症状を抑える
- v0.10 で対応: pipe-pairing drift の構造的解消には bridge との discard/reset handshake が必要
  (`src/video/audio.rs` コメントに記載)

### D-2. T17 follow-on: ClockAnchor の seqlock / atomic-bit-pack 化
- v0.9.0 対応: doc コメントで実態 (= 1 callback あたり ~5 Mutex acquire) を記述。実害観測なし
- v0.10 で対応: `pts_secs` + `wall_at_anchor.elapsed_secs()` を `AtomicU64` (= micro-pts as u64
  か固定小数) に圧縮 + 8-bit version で seqlock 化。RT callback の Mutex を撤廃

### D-3. T18 follow-on: clear_buffer の sum を atomic running total 化
- v0.9.0 結論: 現状コードで実害なし (< 1μs)、対応不要
- v0.10 で対応 (要望あれば): `processed_total_secs` / `raw_pending_total_secs` を atomic
  running total として push/pop で増減する設計

### D-4. T19 follow-on: cpal channel-agnostic downmix
- v0.9.0 対応: device の channels をエラーメッセージに含めてユーザーに「設定変更」を促す
- v0.10 で対応: `fill_output` の stereo packed f32 前提を解除して任意 channel 数に対応

### D-5. T23 follow-on: VST3 settings save の 3 経路にも bridge guard
- v0.9.0 対応: `toggle_native_video_vst3_gui` 主経路のみ修正
- v0.10 で対応: `ui_fullscreen.rs:1656`, `app/native_video.rs:3734`,
  `ui_dialogs/vst3_manager.rs:485` の他 3 経路にも同じ `dsp_bridge.state() == Enabled` ガード
  パターン適用。共通ヘルパー化が望ましい

### D-6. T27 follow-on: close_fullscreen の bridge cleanup gap
- v0.9.0 対応: `run_native_video_output` 正常 exit パスで `hud_hwnd_out.store(0)` 追加
- v0.10 で対応: `close_fullscreen` (app.rs:12225) も bridge `unregister_fullscreen_owner` /
  `set_hud_hwnd(0)` を呼ばない。bridge cleanup gap として別レイヤーで対応

### D-7. T47 follow-on: TRT pack manifest の公開鍵署名検証
- v0.9.0 対応: `MIV_TRT_PACK_BASE_URL` env override を debug ビルドに閉じた
- v0.10 で対応: manifest の pinned 公開鍵署名検証で発行者認証を本格対応

---

## B. P3 (望ましい)

### B-動画 (領域 1〜5)

- [ ] **T58** 動画ブックマーク編集モーダルが source switch を超えて旧ブックマークを書く (Codex R-VBM-001)
- [ ] **T59** Fast source switch が previous hover state を carry (Codex R-VTT-006)
- [ ] **T60** Tile thumbnail cache 無効化が mtime 秒単位のみ (Codex R-VTT-003)
- [ ] **T61** Tile thumbnail worker のコメントが Drop join と書くが detach (Codex R-VTT-004)
- [ ] **T62** VST3 モジュールヘッダ/snapshot コメント/integration doc が pre-chain-bridge 設計のまま (Codex R-VST-004)
- [ ] **T63** Post-seek 全フレーム scale 失敗で seek override 永久固着 (Claude R1-4)
- [ ] **T64** Stuck HW decoder bound demux thread が seek 復旧不能 (R1-5)
- [ ] **T65** Presenter thread panic で「動画を準備中…」永久固着 (R1-6)
- [ ] **T66** Decoder panic 経路の try_send が full 時に黙って drop (R1-7)
- [ ] **T67** EngineActor::handle_decoder_event の Failed arm が紛らわしい (R1-8)
- [ ] **T68** Poisoned mutex の連鎖 panic が RT callback に波及 (R2-6)
- [ ] **T69** fill_output で out.len() の偶数前提を debug_assert (R2-7)
- [ ] **T70** pump スレッドの fx_out/stretched.samples を毎ブロック clone (R2-8)
- [ ] **T71** 死にコード: cap_samples (audio.rs:686), current_processed_secs (R2-9)
- [ ] **T72** Stale コメント: fill_output が BufferReady/BufferStarved を発火しない (R2-10)
- [ ] **T73** shared_texture_cache が raw handle 値キー + generation guard なし (R3-12)
- [ ] **T74** acquire_source_keyed_mutex が cast 失敗時に無同期で copy 続行 (R3-13)
- [ ] **T75** Tile overlay 描画ループが画面外行も O(n) 走査 (R3-14)
- [ ] **T76** overlay_draw.rs:761 の edit.take().expect() (R3-15)
- [ ] **T77** Jump-row 削除ボタンが "X" ASCII (R3-16)
- [ ] **T78** signed_low_word/signed_high_word が hud/native window で別実装 (R3-17)
- [ ] **T79** tick_native_video_loop_boundary の unreachable!() が hot path (R3-18)
- [ ] **T80** acquire_shared_output の Condvar が dummy Mutex とペア (R4-2)
- [ ] **T81** reset_unpresented_shared_output が実質 no-op (R4-3)
- [ ] **T82** acquire_shared_output の戻り値が 8-tuple (R4-4)
- [ ] **T83** thumbnail.rs キャッシュにエントリ上限なし (R5-1)
- [ ] **T84** tile_thumbnails/thumbnail の Drop がスレッド detach (R5-2)
- [ ] **T85** fit_within と decode ループ構造が 3 ファイルで重複 (R5-3)

### B-アップスケール (領域 6)

- [ ] **T86** is_path_inside (paths.rs:55) が安全網だが非テスト呼出ゼロ (R6-8)
- [ ] **T87** run_segments_parallel が全マルチセグメント動画のデフォルト経路、シリアル本体が死にコード (R6-9)

### B-VST3 (領域 7)

- [ ] **T88** ShmHeader レイアウト一致が C++ のみ static_assert (R7-4)
- [ ] **T89** plugin_loader.cpp の controller/processor 戻り値無視 (R7-6)
- [ ] **T90** audio_loop が process_block 失敗で audio thread 永久終了 + bridge 生存 (R7-7)
- [ ] **T91** gui.rs run_message_loop が try_recv + sleep(16ms) (R7-8)
- [ ] **T92** IPC 入力検証の甘さ: raw u32 len 32MB 信頼、bad_alloc 未捕捉 (R7-9)
- [ ] **T93** extract_string_field が JSON unescape しない (R7-10、要検証)
- [ ] **T94** Watchdog が log only で wedge を kill しない (R7-11)
- [ ] **T95** process_audio_blocking の partial pull で shm ring 永続 desync (R7-12)
- [ ] **T96** extract.rs の OnceLock が初回失敗を永続キャッシュ (R7-13)

### B-音声ノーマライズ (領域 8)

- [ ] **T97** Duration 不明短尺クリップが momentary fallback を受けない (R8-1)
- [ ] **T98** デコード不能音声ストリームが SilentInput を返す (R8-2)
- [ ] **T99** emitted_frames が incremented されるだけ (R8-3)
- [ ] **T100** graph.get("in")/get("out") 毎回呼出 (R8-4)

### B-ブックマーク/ピン (領域 9)

- [ ] **T101** folder_thumb_pins の source_id が `|` 区切り (R9-1)
- [ ] **T102** Poisoned mutex を rusqlite::Error::InvalidQuery にマップ (R9-2)
- [ ] **T103** resolve_pin_target の std::fs::metadata 経路 (R9-3、要検証)

### B-設定 (領域 10)

- [ ] **T104** quarantine_db_files 衝突カウンタが process-local (R10-6)
- [ ] **T105** is_family_filename と legacy_json_family_exists が near-identical (R10-7)
- [ ] **T106** Video resume 5 秒タイマーが disk persist しない (Codex R-VRES-001)
- [ ] **T107** Preferences OK が stale runtime video settings を replay (Codex R-SET-002)

### B-TensorRT (領域 11)

- [ ] **T108** extract_engine_zip cancel/error 時 partial 残存 (R11-7) — v0.9.0 T49 で類似経路は対応済、本件は別レイヤー要検証
- [ ] **T109** start_install の .expect() が UI スレッドを panic (R11-8)
- [ ] **T110** Detached installer スレッドが最大 60s 生存 + 再入で race (R11-9)
- [ ] **T111** upscale.rs:249 の out_w * out_h が u32 overflow (R11-10)
- [ ] **T112** SharedMem のサイズ境界チェックが tautological (R11-11)
- [ ] **T113** SharedMem::create の GetLastError 読みが分離 (R11-12)

### B-診断/ロギング (領域 12)

- [ ] **T114** export_diagnostics_zip が UI スレッド同期 (R12-1)
- [ ] **T115** logger::log 1 行ごと flush (R12-2、意図的だが doc 化)

### B-外部リンク (領域 13)

- [ ] **T116** trim_url_trailing_punctuation が末尾 `)` を常に除去 (R13-1)
- [ ] **T117** find_http_urls が xhttp:// のような語中も検出 (R13-2)

### B-検索 (領域 14)

- [ ] **T118** walk_dirs_recursive_with_progress の DirEntry::Err 握り潰し (R14-3)
- [ ] **T119** Watcher イベントストーム O(N²) (R14-4)
- [ ] **T120** commit a9982d04 由来の死にコード (log_disk_snapshot / any_settings_file_exists) (R14-5)

### B-ランチャー (領域 15)

- [ ] **T121** マウス前後ボタンのフレーム跨ぎ二重ナビ (R15-2、要検証)

---

## C. リファクタ負債

巨大モジュールの分割。リリース後フォローアップで十分だが、新機能追加で同モジュールを触る
ときに同時にやると累積負債が抑えられる。

- [ ] **R-1** `decoder.rs` (6321 行) → サブモジュール分割 + `PacingDecision` helper で ~200 行重複解消
- [ ] **R-2** `native_presenter/mod.rs` (6494 行) → util/test_overlay/background 抽出
- [ ] **R-3** `overlay_draw.rs` (3175 行) → overlay_icons/overlay_format
- [ ] **R-4** `video/mod.rs` (4796 行) → native_output (約半分)
- [ ] **R-5** `app/native_video.rs` (3602 行) → normalize/vst3_panel/fast_swap
- [ ] **R-6** `dsp/mod.rs` (2350 行) → gui_orchestration
- [ ] **R-7** `settings_db.rs` (3165 行) → tables/tests 分離
- [ ] **R-8** `upscale/job.rs` (2551 行) → options/probe/plan + 死にコード除去 (`internal_final_part_path`, `is_work_dir_name`, `parallel_segments`)
- [ ] **R-9** `name_index_supervisor.rs` (1039 行) — 分解は clean だが将来 apply_change サブモジュール化
- [ ] **R-10** `app.rs` 巨大化 (+7971 行)

---

## 件数サマリ (2026-05-16 時点)

| カテゴリ | 件数 |
|----------|------|
| A. v0.9.0 持ち越し P2 | 5 件 (未解決 4 件) |
| D. v0.9.0 部分対応の follow-on | 7 件 |
| B. P3 (T58〜T121) | 64 件 |
| C. リファクタ負債 (R-1〜R-10) | 10 件 |
| **合計** | **86 件 (未解決 85 件)** |

着手順は A → D → B (重要度高い順) → C を推奨。
