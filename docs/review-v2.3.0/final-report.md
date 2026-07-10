# v2.3.0 出荷前 品質レビュー 統合レポート (確定版)

実施: 2026-07-09 / 対象: `7eff5a9e` (v2.2.0) .. `01910684` (HEAD、362 コミット / +77k 行)
体制: Codex CLI ×3 セッション (性能 / レース / モード) + Claude (Fable) エージェント ×3 (同 3 観点)
+ 委任サブ調査 ×3 (音楽解析 / detached 共有基盤 / 動画エンジン・音声) + ドキュメント調査 ×1。
全 P1/P2 候補は検収側 (このセッション) が HEAD のコードを直接読んで裏取り済み。
Codex の当初 P1 は反証質問により**撤回** (P3 へ再分類) — 経緯は codex-race-followup.md。

ベースライン: `cargo test` フル実行 **green** (exit 0)。

素材: 同ディレクトリの codex-{perf,race,mode}.md / codex-race-followup.md /
claude-{perf,race,mode}.md / brief.md。

---

## 総評

- **P1 (クラッシュ・データ破壊・UI 長時間ブロック) は 0 件**。ユーザーが最重視していた
  「UI スレッドの同期 I/O・長時間ブロック」は、音楽/音声/detached の新規経路について
  Codex・Claude 双方が独立に「なし」で一致した (デコード・解析・probe はすべて worker 側)。
- リワーク本丸 (HWND 同定 / presenter 世代照合 / bundle 移送 / enter-exit 対称ガード) は
  憲法どおり堅牢。「問題なし」確認リストが各レポートに残してある。
- 残る問題は 2 つの境界に集中している:
  1. **bundle 化 (keepalive/R2b) が App-global なワーカー基盤 (requested / cancel_token /
     music_* / continuous EOF 状態) を巻き込めていない境界** → P2-8/9/3、P3 多数 (BA-7 系)
  2. **音声モード (Inc7) の transient 状態 (exit_pending / EOF 継続 / swap) の相互ガード漏れ**
     → P2-1、P3 数件

---

## P2 確定指摘 (11 件、対処優先順)

凡例: [裏取り] = 検収でコード照合済み / (検出元)

### 毎回踏む・実害大

**P2-3. ParkedLive 音楽再生中、メイン文脈の非メディア open/close が音楽解析状態を無条件破棄**
(音楽解析サブ調査 + Claude perf が独立検出、[裏取り済])
- app.rs:29435-29437 (`open_fullscreen` 非 Audio) / app.rs:35684 (`close_fullscreen`) の
  無条件 `clear_music_view_state()` が、parked 音声窓が毎フレーム消費する global の
  `music_pcm` / spectrum / 解析ワーカー (app.rs:26389-26406) を破棄する。
- 症状: **音楽を別窓で流しながらメイン窓で画像を見る (この機能の中心ユースケース) と、
  ページを開くたびに全尺再デコードが respawn** (1h 級ファイルで ~1.4GB 確保 + フルデコード/回、
  スペクトラムは playhead 追いつきまで空白 = 数十秒)。
- 対処案: clear 条件に「parked live music 窓が存在する場合はスキップ」を追加 (メディア open は
  既存の `close_parked_live_media_windows_for_new_media` 経路で整合済み)。

**P2-8. bundle swap 末尾の `requested.clear()` 毎フレーム化 → サムネ重複エンキュー/重複デコード**
(detached 基盤サブ調査、[裏取り済]、BA-7)
- app.rs:10863 の無条件 clear × active/parked の毎フレーム swap × enqueue 側の
  `requested` 依存 dedup (app.rs:21600-21607、queue 自体に dedup なし)。
- 症状: **detached 窓を開いたまま main の大フォルダをスクロールすると、Pending サムネが
  フレーム毎に再エンキューされ CPU 高騰・サムネ激遅**。PDF は cancel 不可 pool に流れ最悪。
- 凍結ルール上、症状パッチではなくリワークのステージ項目 (bundle とワーカー基盤の境界再設計、
  最低限「swap 時 clear の条件化 or requested の bundle 化」) として扱う。

**P2-9. global `cancel_token` が bundle 非含有 → detached book open が main の動画サムネ抽出を恒久停止**
(detached 基盤サブ調査、[裏取り済: app.rs:20951-20956 のコメント自身が「動画は再リクエスト不能」
と明記、抽出スレッドは cancel で残件破棄 = app.rs:20629-20631]、BA-7)
- 症状: 動画入りフォルダ表示中に「ZIP/PDF を別ウィンドウで開く」→ main の残り動画サムネが
  フォルダ再読込まで Pending 固着。
- P2-8 と同根 (cancel_token / tx-rx の文脈分離漏れ)。リワーク側へ。

### 特定操作で確実に発生

**P2-5. 音声ファイルの音楽ビューで ring/ゲームパッド/右ドラッグの「ウィンドウ/全画面切替」が無言 no-op**
(Codex mode + Claude mode が独立検出、[裏取り済])
- `toggle_video_window_mode_for_input` (native_video.rs:2305) が video_audio_vst /
  video_audio_mode / detached しか見ず、純音声は `toggle_video_window_mode` →
  `switch_native_video_viewer_presentation` の `GridItem::Video` ガード (native_video.rs:2128) で
  silent return。F11 キー (ui_fullscreen.rs:10401) と音楽 chrome のボタンは動くので入力手段間で不一致。
- 対処案: 同関数に `fs_music_view_active` 分岐 (→ `toggle_egui_viewer_window_mode_for_input`) を追加。
  9a952d26 で VideoLoop/Bookmark/Marker/TileMode に入れた分岐の取り残し。

**P2-10. 音声ファイルの音楽ビューで Z が ZipPla ズームモードを不可視ラッチ → 次の画像で突然ズーム**
(Claude mode、[裏取り済])
- `fs_zoom_mode_context_ok` (ui_fullscreen.rs:2142) が `GridItem::Video` しか除外せず、
  `update_fs_zoom_mode_keys` (9386) が音楽ビューの Z consume (9740) より先に走る。
  `fs_zoom_active` は bundle 持ちでナビ後も残存 (reset は close_fullscreen のみ)。
  動画→音声モードは item が Video なので偶然守られ、**純音声だけすり抜ける非対称**。
- 対処案: 除外条件に `fs_music_view_active(fs_idx)` (または `GridItem::Audio`) を追加。

### レース (発生窓は狭いが実在、ロジック上必然)

**P2-1. 音声モード exit 進行中の連続再生 EOF が exit を追い越し、exit 操作消失 + 旧動画フレーム露出**
(Codex race + Claude race が独立検出、[裏取り済])
- `handle_video_audio_mode_continuous_eof` の多重起動ガード (app.rs:45103-45108) に
  `video_audio_exit_pending` が無い。exit は presenter 再表示確認まで `video_audio_mode=Some` を
  維持する設計 (native_video.rs:7064) なので、EOF 分類 (app.rs:45630) が音声モード継続に入り、
  `poll_video_audio_exit_pending` (native_video.rs:7093) が mode 不一致で pending を黙って破棄。
  enter 側 (`enter_video_audio_vst` は exit_pending を明示拒否) と非対称。
- 症状: 曲の終わり際に「動画へ戻る」を押すと、無視されて次の曲が音声モードのまま + 旧動画の
  hold フレームが一瞬〜数百 ms 露出しうる。
- 対処案: ガードに `video_audio_exit_pending.is_some()` を 1 条件追加。

**P2-4. 音楽解析 Arc の生ポインタ identity 比較 (ABA) → 確定解析への再ラスタが不発**
(音楽解析サブ調査、[裏取り済: ui_music_timeline.rs:163 のポインタ比較 + app.rs:34247-34278 の
複数メッセージ連続 drain で、旧 Arc free → 新 Arc 同アドレス再利用が成立し得る])
- 症状 (確率的): 曲後半のタイムライン行が空白のまま固着 (リサイズ / 曲切替まで)。
- 対処案: ポインタでなく単調増加の版数カウンタを identity に使う。

**P2-2. detached/複数窓中のグリッド削除完了が共有 `fs_cache.clear()` で再生中 player を破棄 + `fullscreen_idx`/`video_audio_mode` の idx シフト漏れ**
(Claude race、[裏取り済: remove_items_batch のシフト対象一覧 (app.rs:18900-19065) に両系統が無い。
app.rs:18735-18737 の「idx-keyed 状態を足したら shift 群にも追加」規約から v2.3.0 新設フィールドが漏れ])
- 症状: 別窓で再生中にメイン窓グリッドで無関係ファイルを削除 → 再生停止、復帰後に視聴位置が
  隣のアイテムへズレる。ズレた idx へのレーティング等が別ファイルに当たる可能性。
- 補足: 「fullscreen_idx 非シフト + fs_cache.clear」自体は既存挙動の可能性が高いが、
  detached 化で「別窓視聴中にグリッド操作」が一級フローになり実際に踏めるようになった。
  リワークと調整の上、shift 対象追加 + 再生中ガード (削除確認 or 再オープン) を検討。

**P2-6. 音声実長 0.1 秒未満のファイルが「準備中」のまま永久に再生開始しない**
(エンジンサブ調査、[裏取り済: BufferReady の唯一の emit 条件が `processed_secs >= 0.10`
(audio.rs:818/1440) で、EOF 時の閾値緩和経路なし])
- 対象は極短 SFX 等で稀だがロジック上必然。音声トラック 0.1s 未満の動画も同様。
- 対処案: audio デコード完了 (EOF) 時は processed 残量に関わらず BufferReady を emit。

**P2-7. engine イベント lane (bounded 64) 満杯時に SeekCompleted が silently drop → 「シーク中」+ 無音固着**
(エンジンサブ調査、[裏取り済: audio.rs:1441-1462 の T15 コメント自身が過去実害を記録。
修正は Playing 中の flood 停止のみで、Buffering 中の ~2ms 間隔 emit は残存])
- 発生条件: Buffering 中の UI 停滞 ≥128ms + 直後の seek。再 seek で回復する。
- 対処案: SeekCompleted に native FirstFrameReady と同じ pending 再送 (mod.rs:2224-2231 と対称) を
  持たせる、または Buffering 中の BufferReady emit を状態変化 edge 化。

### プロセス (コードバグではない)

**P2-11. F12 detached × 音声モードは実装済みだが検証網の外**
(Codex mode + Claude mode、[裏取り済])
- brief / 過去メモの「Inc7e 未対応」は誤り — stage-audio (b2063ef3) で意図的に解禁済み
  (native_video.rs:6868-6874 に設計コメント、detached では entry_target 必須 + F12 は音声モード中
  choke = app.rs:41753)。
- ただし `docs/detached-rework-ship-checklist.md` に「active detached 動画 → ♪/Z → exit / F11 /
  host 再生成 / EOF 継続」の smoke 行がなく、hidden presenter × detached host という最も壊れ
  やすいクラスの組合せが未検証。呼び出しサイトのコメント (native_video.rs:6473「detached は
  弾かれる」) も stale で、次の修正セッションを誤誘導する。
- 対処: ship checklist に smoke 行追加 + コメント修正 + 実機 smoke 1 回。

---

## P3 (検収済み or 妥当と判断、テーマ別)

### detached / bundle 境界 (BA-7 系、リワークへ引き継ぎ)
- source-swap pending の update 分岐が owner 一致をローカル検証せず外部 invariant (メディア窓 1 本
  規則) に依存 (native_video.rs:771/803/1048。Codex P1 → 反証で撤回 → P3 再分類)
- ring minimize / close が in-flight detached switch 2 条件を見ない (gamepad_input.rs:4697-4722、BA-7)
- `current_folder_last_mtime` / `signature` が bundle 非含有 → detached book open 後、main の
  フォーカス復帰時外部変更検知が無効化 (app.rs:8165 ほか)
- `video_continuous_mode` / `video_continuous_last_eof` が global — EOF dedup キー (fs_idx, serial) が
  文脈横断で衝突し得る (現状は 1 本規則でほぼ顕在化せず)
- `handle_video_continuous_eof` (映像) に parked-live ガードなし — 現状は deferred swap の owner
  焼き込みで偶然防御。open_fullscreen フォールバック分岐が増えると重大事故化する構造リスク
- 独立 detached 窓での回転・レーティングが main 側 per-bundle cache に伝播しない (BA-6 類型)
- detached book context のサムネ結果は世代不一致で全破棄 (現状実害小、将来の地雷)
- native VK の detached Enter/Esc 固定分岐がタイル / ノーマライズモーダルより先にマッチし
  窓ごと閉じる + keymap 迂回 (native_video.rs:6051-6060、**v2.2.0 以前由来**、BA-7)
- トレイ格納が ParkedLive / passive 窓を考慮しない (確度低) /
  `viewer_session_blocks_main_window` が切替中を含まない (確度低・窓極小)

### 音楽ビュー / 音声モード
- ring VideoCapture / AddToBook / ExternalPlayer が音楽ビュー無ガード (純音声は toast/no-op で
  受け止まるが、動画→音声モードでは hidden 動画のフレーム操作が通る。キーボード側の抑止と不一致)
- EOF 種別分類がフレーム冒頭 snapshot 依存で、同フレーム内の ♪ 押下が 1 回無視される窓 (1 フレーム)
- 音楽解析 worker の同一 path 外部置換 TOCTOU (適用側照合が path のみ)
- timeline raster worker が死ぬと key 変化まで respawn しない (spectrum 側と非対称)
- デコード途中失敗後、spectrum が stale 末尾窓を表示し続ける / stale LRU hit 中の失敗で表示中
  波形が Err 置換される
- `?` ショートカット一覧が音楽ビューで動画専用行をそのまま表示
- native_video.rs:6473 の stale コメント (P2-11 に含めて修正)

### 性能 (機構確認済み、実測はなし)
- 音楽ビュー主要同期区間に perf::event 計装ゼロ (CLAUDE.md §4 違反。ヒッチ発生時に切り分け不能)
- timeline row テクスチャに offscreen eviction なし — 長尺で VRAM 単調増加
- `analyze_stereo_timeline` が cancel 不可 — 曲連打で GB 級 PCM が多重常駐
- ParkedLive 再生中は全体 60fps 常時再描画
- `draw_beat_grid` が可視行ごとに全曲 beat/bar を線形走査 (bins は partition_point 済みで非対称)
- 音楽ブックマーク初回ロードが UI スレッド同期 SQLite (path 変化時のみ、cold DB でヒッチ要因)

### エンジン (既存由来を含む)
- FirstFrameReady / InfoReceived の try_send 失敗でフラグだけ先行 (非 native 経路、再送なし)
- pause と in-flight cpal callback の anchor 上書き (~10ms 表示ズレ、resume で回復)
- set_playback_speed と callback の anchor 相互上書き (数 ms ジッタ)
- 音声出力デバイス初期化失敗の一部経路で demux ごと終了 → 映像も凍結の疑い (既存由来の可能性大)
- 再生中デバイス喪失時の AudioInactive 未配線 (wall master 切替が発火しない)

---

## ドキュメントギャップ (ドキュメント調査エージェント、詳細は当該結果参照)

| 区分 | 内容 |
| --- | --- |
| 必須 | `docs/architecture-overview.md` に音楽系 (audio_decode / ui_music_* / crates/music-core / GridItem::Audio) と detached 系 (DetachedWindowRuntime / ViewerContextBundle / F12) が**丸ごと未収載** (入口ドキュメントなのに v2.3.0 の 2 大機能とも不在) |
| 推奨 | `detached-rework-plan.md` §9 進捗表が findings-15/16/17/19 未記録で現在地が読めない |
| 推奨 | `music-integration-plan.md` Inc8 の済/残が未記録 (実際は manual/spec/video-architecture の大半が反映済み) |
| 推奨 | `docs/README.md` 索引に detached-rework ドキュメント群 ~30 本が未収載 (特に stage-audio) |
| 軽微 | `manual/shortcuts.html` の動画表に Z (音声モード) の行がない |

良い側の確認: video-architecture / async-architecture / keymap-spec / spec.md / manual (music.html) は
差分によく追従しており、「コードにあるがどこにも説明がない大概念」は無かった。

---

## 過去認識の訂正 (メモリ/ブリーフの誤り)

1. 「Inc7e 残 = DetachedWindow(F12) 音声モード未対応」→ **stage-audio (b2063ef3) で実装済み**。
   残っているのは実機 smoke と stale コメント修正 (P2-11)。
2. 「step9 (inert 残置削除) 未実施」→ **実施済み** (42c96236)。
3. 「Inc8 (docs) 未実施」→ 大半実施済み。残は上表の整理のみ。

---

## 追補 (2026-07-09 同日): P2 修正フェーズ + 追加バグハント

### P2 修正フェーズ
P2-1〜10 の全件を修正済み (未コミット)。Codex 検収 3 ラウンドで追加 5 件
(normalize 系 shift 漏れ P2 / music_vst_shell P2 / vst3_deferred P3 / テスト強化 P3 /
非 windows cfg 漏れ P1) を検出し全対応。回帰テスト 6 本追加。フルテスト green。
詳細: codex-fix-review{,2,3}.md。

### 追加バグハント (Codex×3 + Claude 横断×1) の結果

**修正済み (このセッションで対応):**
- [P1] 壊れたファイルの巨大 duration → タイムラインが兆単位 row 確保で OOM/フリーズ
  → `TIMELINE_MAX_ROWS=100_000` キャップ + テスト (codex-hunt-core)
- [P2] NaN/Inf サンプルが解析 (rms/loudness/band/FFT) に伝播 → `finite_or_zero` を
  3 箇所に適用 + テスト (codex-hunt-core)
- [P2] 異常 BPM で `BeatGrid::from_bpm` が巨大確保 / 無限ループ → `MAX_BEATS` +
  進行ガード + テスト (codex-hunt-core)
- [P2] タグファセット通過判定に Audio が無く、音声だけタグ絞り込みをすり抜け →
  `facet_tag_filter_applies` に Audio 追加 + テスト (codex-hunt-misc)
- [P2] **P2-8/9 修正自体の欠陥**: detached 窓 close 時に bundle が drop されるだけで
  per-context worker pool (5〜14 スレッド) が condvar 待ちで永久残留 →
  `ViewerContextBundle` に Drop 実装 (cancel + notify) (claude-hunt-crosscut)
- [P3] keep-range/scroll 共有 atomic の文脈間汚染 (P2-8/9 の残穴) → 4 atomic も bundle 化
- [P3] `ComInitScope` が S_FALSE で CoUninitialize しない → SUCCEEDED 判定に修正
- [P3] music-core FFT の空/奇数長入力 panic (library API 経由のみ到達) → 境界ガード追加
- [P3・前方防御] `FacetItemKind` に `#[serde(other)] Unknown` を追加 (v2.4 以降からの
  ダウングレード耐性。ToolbarFacetFilterItem と同方針)

**報告のみ (要ユーザー判断):**
- [P2] **v2.2.0 ダウングレード非互換**: v2.3.0 で音声ファセットを有効化して保存すると、
  v2.2.0 は `FacetItemKind::Audio` を読めず設定 DB 全体が Corrupted → quarantine/bak
  巻き戻り。v2.2.0 側は出荷済みで直せない。**リリースノートに「v2.2.0 へ戻す場合の注意」
  を記載推奨** (claude-hunt-crosscut。他の新規永続化 7 キー + rating/keymap/メニューは
  ダウングレード安全と確認済み)
- [P2] KeyHold 割当 (FsZoomMode / SpacePan) がテンキー系物理キーで効かない/別キーで
  効く — `key_held_chord` が `KeyName::to_egui()` None を即 false にする非対称
  (codex-hunt-input。挙動繊細のため要実機検証つきで別途修正推奨)
- [P2] X リング中にモーダル/IME でブロックされると保持スティックが解除後に通常操作へ
  漏れる — neutral gate 欠落 (codex-hunt-input。同上)
- [P2] detached/DWM snapshot の HWND 採用条件が可視性を見ない — hidden/iconic viewport
  誤採用の可能性 (codex-hunt-misc。**BA-1/BA-4 = 凍結対象、リワークへ引き継ぎ**)
- [P2] audio decode EOF で swresample を flush せず末尾 PCM が解析から欠ける —
  意図的省略コメントありのため設計判断が必要 (codex-hunt-core)
- [P3] native nav preview がサムネ未キャッシュ時に旧動画フレームを見せ得る (仕様判断)
- [P3] 非 Windows ビルド破壊 2 系統 (music chrome 系 cfg)。非 Windows toolchain がなく
  検証不能のため報告のみ。**再発防止は CI に非 Windows cargo check 追加** (crosscut)

**問題なし確認**: settings 互換 (新規 7 キー)、fs_animation、logger、HUD hit-test、
panic ハント (新規 unwrap 69 件全照合)、エラー経路後始末、リソース teardown — 各レポート参照。

### 追補 (2026-07-11): ParkedLive / VST3 deferred BA-7 P2 3 件

発見: Codex 出荷前最終レビュー / 裏取り: Fable / 修正: Codex Sol。

- VST3 startup load 待ちの media open を `ViewerContextBundle` 所有へ移し、main / active
  detached / ParkedLive の各 context で pending を再開するよう修正。main 側 close/open による
  別 context の pending 消失も防止した。
- ParkedLive の直接 close と新メディアによる close に共通の bundle teardown を追加。最終再生
  位置の保存、path 所有での normalize scan cancel、最後の音楽 consumer close 時の global
  music 状態解放を行う。
- grid から同じ media を開いた場合は context 間で path 照合し、active は raise、ParkedLive は
  既存 activation 経路で前面化するよう修正。別 media の 1 本規則と teardown 付き close は維持。

### 追補 第 2 弾 (2026-07-11): ParkedLive 所有権境界 P2 5 件

発見: Codex 出荷前最終レビュー / 裏取り: Fable / 修正: Codex Sol。

- ParkedLive source-swap 中の activation で pending owner を mounted/active 所有へ移し、
  active poll が completion / timeout を引き継ぐよう修正。逆向き live-park も owner を
  window id へ移す。parked mount から生成可能な open / fast-swap / video-tile pending にも
  同じ owner を追加。
- close_fullscreen は実際に閉じる context の native pending だけを破棄し、別 ParkedLive /
  promoted active 窓所有分は温存。parked 窓 teardown では一致 owner の pending を明示破棄して
  presenter/output 残留を防止。
- preserve-main live-park のロード複合体へ requested / pending_finalize /
  texture_backlog を追加し、worker queue と重複投入防止状態を同じ context へ戻す。

- same-media 再クリックは表示面で raise 先を選び、動画→音声モードでは hidden presenter
  でなく egui 音楽 viewport を focus。VST host 表示中と通常動画は presenter raise を維持。
- 表示モード変更の passive 一括 close を共通 media teardown seam に通し、最終 resume 保存、
  normalize cancel、最後の音楽 consumer の global 状態解放を揃えた。

## 推奨アクション (優先順)

1. **出荷前修正 (小粒・低リスク・凍結対象外)**: P2-1 (ガード 1 条件) / P2-3 (clear 条件化) /
   P2-4 (版数カウンタ) / P2-5 (`fs_music_view_active` 分岐) / P2-10 (除外条件 1 つ)
2. **出荷前修正 (要設計判断)**: P2-6 (EOF 時 BufferReady) / P2-7 (SeekCompleted 再送) /
   P2-2 (shift 対象追加 + 削除ガード、リワークと調整)
3. **detached リワークへ引き継ぎ (凍結、パッチ禁止)**: P2-8 / P2-9 + BA-7 系 P3 群 →
   プラン §2 に従い、bundle × App-global ワーカー基盤の境界再設計をステージ項目化
4. **実機 smoke 追加**: detached 動画 → ♪/Z → exit / F11 / host 再生成 / EOF 継続 (P2-11)、
   0.1s 未満音声ファイル (P2-6 確認)
5. **docs**: architecture-overview 追記 (必須) ほか上表
