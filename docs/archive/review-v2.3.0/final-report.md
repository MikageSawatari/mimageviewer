# v2.3.0 出荷前 品質レビュー 統合レポート (確定版)

## 現在の出荷候補ステータス (2026-07-11、追補第 21 弾反映)

- 対象: `master @ 30b42707` + working tree (追補第 15〜21 弾、未コミット)。
- 追補とコミットの対応: 第1弾 = `bf1e2370`、第2弾 = `c3efcedc`、第3〜13b弾 =
  `30b42707`。第14弾は full composite の実現可能性調査だけで実装コミットなし。
  第15〜21弾は現在の working tree にあり、コミット / stage 前。
- 第16弾の `src/metadata_cleanup.rs` / `src/ui_dialogs/metadata_cleanup.rs` は新規ファイルで
  現在未追跡。モジュール宣言だけの欠落ではなく、次コミットで tracked 化する実装本体である。
- R1 第2波で新規検出された P1-1 / P2-1 / P2-2 は追補第7弾で修正済み。
  P1-1 の同期 `Path::exists` は候補解決 worker へ移し、UI thread の EOF / 手動送りから
  metadata I/O を除去した。
- 当初確定 P2-1〜10 は追補第1〜6弾までに修正済み。P2-11 はコード欠陥ではなく
  detached 動画×音声モードの実機 smoke 項目としてチェックリストで継続する。
- R1 第2波 P3-1 (音声の Ctrl+G 索引対象化) はユーザー裁定「v2.3.0 で対応」に従い、
  追補第8弾で実装済み。ID3 等の埋め込みメタデータは対象外で、ファイル名検索に限定する。
- 実機 FB で判明したタグビューの種類 dropdown の音声欠落は追補第9弾で修正し、
  kind フィルタ UI の横断監査も完了した。タグビューの選択は設定/DBへ永続化しない。
- 実機 FB の「`NumpadEnter` に KeyHold を割り当てると本体 Enter でも発動」は追補第10弾で
  修正した。Win32 extended bit の物理ラッチにより、Press / KeyHold / native / 割当キャプチャの
  全経路で本体 Enter とテンキー Enter を双方向に分離する。
- 第15〜20弾の最終自動検証結果は各追補節を正とする。現在の出荷判断は残る実機 smoke
  結果で確定する。

以下の 2026-07-09 の件数・総評・優先順位は当時のレビュー記録であり、現在ステータスは
本節と末尾「推奨アクション」を正とする。

---

実施 (当時): 2026-07-09 / 対象: `7eff5a9e` (v2.2.0) .. `01910684` (HEAD、362 コミット / +77k 行)
体制: Codex CLI ×3 セッション (性能 / レース / モード) + Claude (Fable) エージェント ×3 (同 3 観点)
+ 委任サブ調査 ×3 (音楽解析 / detached 共有基盤 / 動画エンジン・音声) + ドキュメント調査 ×1。
全 P1/P2 候補は検収側 (このセッション) が HEAD のコードを直接読んで裏取り済み。
Codex の当初 P1 は反証質問により**撤回** (P3 へ再分類) — 経緯は codex-race-followup.md。

ベースライン: `cargo test` フル実行 **green** (exit 0)。

素材: 同ディレクトリの codex-{perf,race,mode}.md / codex-race-followup.md /
claude-{perf,race,mode}.md / brief.md。

---

## 総評 (2026-07-09 当時)

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

## P2 確定指摘 (2026-07-09 当時: 11 件、対処優先順)

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

### 追補 第 3 弾 (2026-07-11): 角度 A/B 追加レビュー P2 4 件 + P3 1 件

発見: Codex Sol の read-only 追加レビュー (角度 A = 直近 2 コミット相互作用、
角度 B = 連続再生・スライドショー) / 裏取り: Fable / 修正: Codex Sol。

- **[P2 / A-1] 削除時の native pending idx shift 漏れ**:
  `remove_items_batch` で source-swap / open / fast-swap / video-tile の 4 種を
  owner=None (mounted 所有) の場合だけ old→new idx へ追随させた。owner=Some の
  ParkedLive pending は bundle の idx 空間なので不変。target 削除時は pending を畳み、
  source-swap の navigation preview と presenter を正規 teardown する。
- **[P2 / A-2] ParkedLive close の companion state 残留**:
  所有 fast/tile pending の破棄と同時に deferred navigation を破棄。tile companion は
  owner stamp を優先し、完成済み tile は閉じる bundle の表示動画 path、他の mounted /
  active / parked video context 不在を照合してから mode/state/reopen/deadline を消す。
  無関係な mounted tile state は温存する。
- **[P2 / A-3] player 作成前 deferred media の削除・rename 照合漏れ**:
  mounted fullscreen の表示 item が対象 Video/Audio で、VST3 deferred open または
  owner=None の native open pending と一致する場合も、player 一致時と同じ close 経路へ
  入れる。静止画の隣接再整合 UX は変更しない。
- **[P3 / A-4] removed-path passive close の teardown seam 迂回**:
  passive / ParkedLive bundle を retain-drop する前に共通 teardown を呼び、close 時点の
  最終 resume を保存する。rename はその後に既存の path migration が走るため、
  「最終位置を旧 path へ保存 → 新 path へ移行」の順序を固定した。
- **[P2 / B-1] EOF source-swap 中 F12 の設定・表示乖離**:
  案 a を採用。現在 context と owner が一致する source-swap pending 中は、通常モードと
  メディア別窓モードの共通 F12 入口でトグル全体を無視し、設定も反転しない。player 不在で
  placement だけ適用不能になる二相不整合を、小さな owner gate で防いだ。

「偶然防御」は c3efcedc でも生存しており、今回も owner の明示照合・破棄範囲限定で
さらに強化されたことを再確認した。

### 追補 第 4 弾 (2026-07-11): GUI レビュー P1/P2 の裁定と修正

対象: Codex GUI レビュー (c3efcedc) / 裏取り・検収: Fable / 修正: Codex Sol。

- **① [P1] live-park の部分 clone による main 文脈喪失 — 修正**:
  preserve-main 経路を空 bundle + allowlist clone から、`ViewerContextBundle` 全 field を
  destructure して「main/parked へ複製」「parked へ移送」「main に保持」の 3 群へ分類する
  split に置換した。player、fullscreen load/pending、fullscreen 一時 UI、動画音声/UI 状態だけを
  ParkedLive へ移し、ページ補正・マスク/隠蔽/注釈・補正レイヤー・見開き/読書方向・view-trim・
  サムネ補正・詳細列/tag prewarm・folder load 複合体は main が原本を保持する。items など EOF
  連続再生に必要な一覧 identity は従来どおり parked にも複製する。
- **② [P2] ダブルクリック same-media 前面化迂回 — 修正**:
  grid leaf open の共通前処理を追加し、Enter / ダブルクリック / ゲームパッド accept /
  stack 集約フラット open の全経路で activate-existing を park より先に実行する。同じ
  active/ParkedLive 動画・音声は既存窓を前面化して open を消費し、再起動しない。
- **③④ — 追補第 3 弾で対応済み**:
  native pending idx の owner-aware shift/target 削除 teardown、および ParkedLive close の
  tile/deferred-navigation companion teardown は追補第 3 弾 A-1/A-2/A-4 の修正を正とする。
- **⑤ — 既知 P3、detached リワーク送り**:
  bundle と App-global native runtime の所有境界をさらに一本化する構造変更は凍結対象であり、
  v2.3.0 の局所修正には含めない。付随指摘の `native_video_mode_switch` は owner stamp が無く、
  close 後も deadline まで font-atlas settle/native control を塞ぐ実害があるため、閉じる
  ParkedLive 群が唯一の video context と確定できる場合だけ teardown する。mounted / active /
  surviving parked video があれば無関係な切替を壊さないよう温存する。

### 追補 第 5 弾 (2026-07-11): §1.7 linked 画像窓の media open 乗っ取り

実機報告 / 機構確定: Fable / 修正: Codex Sol。

- grid leaf open seam に opening item を渡し、media-window へ入る item のときだけ mounted
  detached still を先に passive snapshot へ handoff する。通常の linked still→still は
  従来どおり同じ窓を reuse し、§1.7 OFF の linked 窓 close 仕様も変更しない。
- ParkedLive の same-media activation も media handoff 専用 preserve を通す。§1.7 の F12
  linked still は閉じず passive に残り、既存メディア窓だけが active へ戻る。
- 別 media open は旧 ParkedLive を既存 teardown seam で close し、その runtime placement
  を settings seed へ保存してから新 window_id を allocate する。画像窓 ID は再利用せず、
  新メディア窓の位置・サイズは直前のメディア窓を継承する。
- 複数ウィンドウモードにも mounted still→media の同じ seam 欠落があり、既存の independent
  still preserve 条件を同じ分岐から利用して防止した。
- 実機ログで、ダブルクリック前の選択変更を sync_detached_viewer_to_selected が先に処理し、
  grid-open seam を通らず linked 画像窓へ media を開く cursor-follow 経路の取り残しを検出。
  §1.7 中は「現在表示中が media」に加えて「追従先 selected が media」でも sync を skip
  するようガードを拡張した。クリック・矢印・ゲームパッドの選択移動を同じ gate で保護し、
  非 §1.7 の linked 窓が動画へ追従する従来仕様は維持する。
- 追加実機ログで、media handoff により Parked になった linked still を次の画像 grid open が
  再利用せず、新しい画像窓を allocate して3窓化する取り残しを検出。handoff元runtimeの
  linked + Parkedをone-shot identityとして保持し、snapshotとruntimeが両方生存する一意候補
  だけを次のstill openで消費する。同じwindow_id・placement・HWND runtimeをResuming→Active
  へ戻すため、画像→動画→画像と画像→動画→動画→画像はいずれも「media 1 + still 1」を維持。
  close / 直接passive activate / mode一括closeではruntime removalまたはlinked解除により
  one-shotが失効し、stale IDは再利用しない。複数ウィンドウmodeのalways-new仕様は対象外。

### 追補 第 6 弾 (2026-07-11): MPEG-PS EOF位置のresume汚染

実機ログ / 機構確定: Fable / 修正・追加調査: Codex Sol。

- MPEG-PSでavformat durationが0または過大な場合、従来のposition対duration終端guardを
  EOF位置がすり抜け、終端秒がresumeとして保存されて再open直後にEOFへ戻るループを確認。
- resume保存判断へEngineActorのpublished EOF stateを追加。5秒周期、save-all、ParkedLive /
  passive teardown planの3経路すべてで、EOFならduration値に関係なく既存entryを削除する。
  既に終端値で汚染されたentryも、そのplayerがEOFへ到達した保存/closeで自動回復する。
- open-time resumeはVideoPlayerとEngineActorの両適用点を共通sanitizeへ集約。duration既知で
  末端5秒以内ならseekせず先頭から、正常な途中位置は従来どおり復元する。
- **P3 / 別件 — MPEG-PSのduration学習とシーク精度**: VideoInfoとnative HUD用duration atomicは
  avformat情報受領時に一度だけ設定される。decode EOF時はduration不明ならclock現在位置を
  EngineActorのEof freezeへ渡すが、VideoInfo/HUD durationへ書き戻す機構は無い。duration=0では
  seek barの尺が得られず、過大値では実在しない範囲へのseekになり得る。ファイル固有の推定尺
  補正はresume汚染と分離し、v2.3.0出荷前修正では深追いしない。

#### 追補 第 6 弾 続報: xhigh R1×4 + R2×3 (重複統合後 P2 6 件)

レビュー: Codex xhigh R1/R2 / コード照合・検収: Fable / 修正: Codex Sol。
R1-3 と R2-P2-3 は同じ stale bundle item 問題として統合した。

- **R1-1 音声モード resume 巻き戻り**: resume 位置選択を helper 化し、5 秒周期、
  `save_all_video_resume_positions`、ParkedLive/passive teardown plan の全経路へ適用した。
  純粋な動画→音声モードは音声 clock の `position()`、VST host 表示中と通常動画は
  `last_displayed_pts` 優先を維持する。bundle 側は bundle 自身の mode/VST state で判定する。
- **R1-2 終了時 detached resume 未収穫**: on_exit で mount 中 player を保存した後、active
  detached bundle と全 paused bundle を read-only teardown plan 化して resume を収穫する。
  bundle の drop/mount や playback teardown は行わない。
- **R1-3 = R2-P2-3 stale sibling path**: promoted/ParkedLive の items snapshot は main の
  delete/rename と同期せず、使用時検証を採用した。動画 EOF、動画→音声モード EOF、音楽 EOF、
  メディア窓の手動前後送りで Image/Video/Audio の実 path に `Path::exists` を行い、欠損候補を
  ログ 1 行付きでスキップする。ZIP/PDF 仮想 entry は existence check 対象外。全滅時は従来の
  folder-end/boundary 動作へ落とす。
- **R1-4 ParkedLive open timeout zombie**: detached host 待ちと decoder 待ちの両 timeout で
  owner=Some のとき mounted fullscreen を閉じない。mount 中 bundle を snapshot へ swap-back
  した後、owner 窓を共通 paused-media teardown seam 経由で close する。
- **R2-P2-1 main delete の誤 idx shift**: owner=None の native pending 4 種は active promoted
  media context が存在する間 shift しない。App-global normalize/continuous EOF state は active
  promoted または ParkedLive media window が存在する間 shift しない。ParkedLive pending は
  既存 owner=Some gate、mounted media は従来 shift を維持する。メディア窓 1 本規則により
  detached 所有中の mounted media state 併存はない。
- **R2-P2-2 ZIP 専用入口の promote 迂回**: `load_zip_as_folder` の items clear より前、still
  preserve/close より前に media promote を追加した。cache-hit 振替と入れ子 ZIP の再帰呼び出しは
  2 回目の `fullscreen_idx=None` で no-op となり、アドレスバー/お気に入り等の同入口を一括で守る。

追加 unit coverage は audio-mode save-all/teardown、on_exit 相当 active+paused 収穫、欠損候補
skip+仮想 entry 非検証、ParkedLive host timeout owner close、mounted/promoted/ParkedLive の idx
ownership、ZIP load 前 promote を対象とする。

### 追補 第 7 弾 (2026-07-11): R1 第 2 波 P1×1 + P2×2 + doc 整合

レビュー: Codex R1 第2波 / コード照合・検収: Fable / 修正: Codex Sol。

- **[P1-1] stale bundle path 検証の UI thread 同期 I/O**: 案 A (候補解決 worker) を採用。
  EOF / 手動前後送りは `items` と表示順から方向付き候補列だけを同期抽出し、実ファイルの
  `Path::exists` は cancel token 付き worker で順次実行する。結果は mpsc で返し、適用時に
  `items_generation` / `input_seq` / ParkedLive owner window の一致を検証する。新規要求、
  context close/load、worker loop の3箇所に cancel を置き、開始・完了・適用を perf 計装した。
  動画 EOF、動画→音声モード EOF、音楽 EOF、メディア窓の手動前後送りを同じ経路へ統合。
- **[P2-1] ParkedLive source-swap 失敗後の zombie bundle**: presenter-closed と decoder
  解放待ち timeout の owner=Some 分岐を `request_parked_live_media_close_after_poll` へ接続。
  mount 中 bundle を swap-back してから対象 owner 窓だけを共通 teardown seam で閉じる。
  mounted (owner=None) は従来どおり `close_fullscreen` を使う。
- **[P2-2] タグビュー音声の Folder 誤復元**: in-memory の `TagViewItemKind` に `Audio` を追加し、
  共通 `folder_tree::is_audio_ext` で Folder fallback より先に分類、結果を `GridItem::Audio` へ
  マップした。この型は DB/設定へ serialize されないため v2.2.0 永続化互換策は不要。
  既存種別フィルタには音声カテゴリがないため選択肢は増やさず、「すべて」では表示し
  「フォルダ」への混入だけを解消した。
- **[P3-1] 音声 Ctrl+G 索引**: 機能スコープのユーザー裁定待ち。追補第7弾では変更しない。
- **[P3-2] final report 整合**: 冒頭に現在ステータスを追加し、旧 HEAD / 総評 / 件数を
  当時記録と明示。推奨アクションを現在の残作業へ更新した。

追加 unit coverage は候補抽出 + 欠損/仮想候補解決、generation/input/owner gate、
source-swap の presenter-closed/timeout owner close 要求、音声 classify と GridItem 適用を対象とする。

### 追補 第 8 弾 (2026-07-11): P3-1 ユーザー裁定「音声 Ctrl+G 対応」

裁定: v2.3.0 で対応 / 実装: Codex Sol / 検収: Fable。

- 初期 `search_walker` と watcher 差分 `build_candidate_from_path` の双方を、既存共通
  `folder_tree::is_audio_ext` へ接続した。独自の拡張子一覧は追加せず、音声を
  `CandidateKind::Audio` → `IndexKind::Audio` として ingest する。
- 音声 ingest はファイルを読まず、共通正規化した basename だけを `name` へ格納する。
  ID3 の曲名・アーティスト・アルバム等は今回の対象外。Tantivy の既存 `kind` 文字列へ
  `audio`、`fts_meta.db.kind` の既存末尾へ整数 `5` を加えた。
- Ctrl+G の Flat / DrilledInto が共有する FS hit materialize 境界を作り、共通拡張子判定で
  `GridItem::Audio` へ復元する。結果セルの double-click / Enter は既存 Audio open 経路から
  音楽ビューへ入り、Ctrl+↑↓ の検索結果ナビにも音声を追加した。streaming rebuild 用の
  内容キーにも Audio を加え、選択・チェック追従を維持する。
- Ctrl+G の索引 kind ドロップダウンへ「音声ファイル」を追加。検索結果のスマートファセットは
  既存 `GridItem::Audio → FacetItemKind::Audio` 接続がそのまま機能し、回帰テストで固定した。

#### 既存索引・ダウングレード判断

- **full rebuild / schema version bump は不要**。`kind` は元から `STRING | STORED` で、`audio` は
  新しい term 値にすぎず schema は変わらない。既存ユーザーの DB に音声行がなくても、次回の
  supervisor 初期 scan で「FS にあり DB になし」と判定され、通常の差分 ingest に自然に入る。
  watcher はアップグレード後の新規追加・rename・更新を同じ Audio 候補として取り込む。
- **v2.2.0 へ戻しても Corrupted 級にはならない**。v2.2.0 の `IndexKind::from_i64` は未知の整数
  `5` を診断ログ付きで `Image` へフォールバックし、Tantivy の `kind="audio"` は既存 STRING
  schema で開ける。v2.2.0 walker は音声を FS 候補にしないため、初期 3-way diff で該当 DB 行を
  delete queue に落とし、Tantivy doc と SQLite 行を通常削除する。削除 commit 前の短い窓に
  kind 未指定検索を行うと音声 hit が旧 materialize の Image fallback で見える可能性はあるが、
  画像 decode に失敗するだけで DB 隔離・schema破損・設定 Corrupted には至らない。

#### 自動検証

- `cargo fmt` / `cargo fmt --check`: exit 0。
- `cargo test --bin mimageviewer-core`: exit 0、3374 passed / 17 ignored / 0 failed。
- `cargo run --release --bin bench_search -- --docs 50000 --json target/bench-audio-search/bench_new.json`:
  exit 0。続く `check_bench_regression.py` も exit 0、全10クエリが +30%以内、最大 +6.2%。
  初回計測は短時間クエリ3件が閾値を超えたが、同条件の再計測で解消し baseline は更新していない。
- `python scripts/check_ui_glyphs.py`: exit 0、危険グリフ 0。
- `.\scripts\build-release.ps1 -SkipVst3Bridge`: exit 0。core / launcher とも release build 成功。

追加 unit coverage は walker Audio 候補化、watcher 差分 Audio 分類、filename-only ingest、
Audio kind の保存値と検索フィルタ、Flat / DrilledInto の `GridItem::Audio` 復元、
音声の fullscreen target とファセット kind 判定を対象とする。

### 追補 第 9 弾 (2026-07-11): 種類フィルタ UI の音声対応 実機 FB + 横断監査

実機報告 / 裁定: ユーザー / 実装・監査: Codex Sol / 検収: Fable。

- **観測**: タグビュー (Ctrl+T) の「すべての種類」には音声が無く、動画を選ぶと
  音声は正しく除外される一方、音声だけへ絞る経路が無かった。
- **不変条件**: タグビューが `TagViewItemKind::Audio` を結果種別として扱うなら、
  同じ種類 dropdown から Audio を選択でき、Audio だけが通り、動画/フォルダへ混入しない。
- **原因と修正**: 追補第7弾で結果分類と `GridItem::Audio` 復元だけを追加し、
  UI 選択 enum `TagViewKindFilter` への Audio 追加を見送った取り残し。
  `TAG_VIEW_KIND_FILTER_CHOICES`、表示ラベル、`matches` 判定、実ファイルを使う検索テストへ
  Audio を一貫して追加した。件数表示はフィルタ済み `entries.len()` を既存どおり使う。

#### kind フィルタ UI 横断監査

| UI | 現状 | 判断 | 対応 |
|---|---|---|---|
| タグビュー (Ctrl+T) 種類 dropdown | Audio 結果分類はあるが選択肢だけ欠落 | 音声を扱うため必須 | **修正**: Audio 選択・ラベル・判定・回帰テストを追加 |
| Ctrl+G アイテム検索 種類 dropdown | `IndexKind::Audio` / 「音声ファイル」あり | 音声を扱うため必要、追補第8弾で対応済み | 変更なし |
| 共通スマートフィルタ「種類」(通常/詳細/サブ展開/検索・タグ結果/レーティング一覧) | `GridItem::Audio → FacetItemKind::Audio`、動的件数・ラベル・保存退避あり | 音声が一覧にあるビューでは必要 | 変更なし。動画/Folder への混入なし |
| Ctrl+S コンテナ検索「種別」 | フォルダ / ZIP / PDF のみ | コンテナ検索であり音声・動画は対象外 | 変更なし |
| レーティング一覧 | `RatingItemKind::Audio=9` と `GridItem::Audio` 復元、共通種類 facet が利用可能 | 音声レーティングを扱うため Audio 分類が必要 | 対応済み、変更なし |
| 読書履歴 | 画像本の Folder / ZIP / PDF / Archive のみ。動画・音声は記録対象外 | 「最近読んだ本」専用なので音声選択は不要 | 変更なし |
| サブ展開 | 画像 / 動画だけのフラット表示と UI に明記し、Audio は走査対象外 | 現仕様では音声を扱わない | 変更なし |
| 画像色フィルタ | `has_page_data` の画像/ZIP画像/PDFページと Stack のみ | 色を持たない音声は対象外 | 変更なし |
| 同名ファイル処理 | アーカイブ対フォルダ、動画対画像、画像拡張子間の明示ルール。Audio は除外しない | 音声を動画 companion / 画像扱いしてはならない | 既存の kind 限定を確認、変更なし |
| キャッシュ一括作成 | 画像 / ZIP画像 / PDFページの raster thumbnail 作成。kind 選択 UI は無い | raster thumbnail を持たない音声は対象外 | 変更なし |

#### 永続化・互換・snapshot

- `TagViewState.kind_filter` は `Settings` のフィールドでも serde 型でもなく、
  App のプロセス内 state にだけ保持される。settings.db / tags.db への保存・復元参照は無い。
  したがって v2.2.0 ダウングレード対策は不要。
- 永続化される共通 `FacetFilter.kinds` の Audio は、追補済みの
  `kind_audio_stash` 退避/復元により v2.2.0 互換を維持している。
- `tests/ui_snapshot.rs` は再利用可能な独立 UI と診断/Markdown描画だけを対象とし、
  App 内のタグビュー dropdown を含まない。snapshot PNG 更新は不要。

#### 自動検証

- `cargo fmt` / `cargo fmt --check`: exit 0。
- focused `cargo test --bin mimageviewer-core tag_view_`: exit 0、14 passed。
- `cargo test --bin mimageviewer-core`: exit 0、3374 passed / 17 ignored / 0 failed。
- `python scripts/check_ui_glyphs.py`: exit 0、危険グリフ 0。
- `.\scripts\build-release.ps1 -SkipVst3Bridge`: exit 0。core / launcher の release build 成功。
- `tests/ui_snapshot.rs` は対象 UI を含まないため更新・実行なし。

`docs/next-release-backlog.md` には、実機確認済みの duration 不明/不正 MPEG-PS の
シークバー不能を P3 / 次版送りとして追記した。

### 追補 第 10 弾 (2026-07-11): テンキー Enter の KeyHold が本体 Enter でも発動する実機 FB

実機報告 / 検収仮説: Fable / 実装・経路監査: Codex Sol / 検収: Fable。

- **観測と不変条件**: `FsZoomMode = NumpadEnter` で本体 Enter までズームを起動した。
  `Enter` と `NumpadEnter` は別 `KeySlot` なので、Press / KeyHold / native 動画 /
  操作カスタマイズのキャプチャの全経路で相互発火してはならない。逆方向の
  `FsZoomMode = Enter` もテンキー Enter で発動してはならない。
- **経路監査**: Win32 key edge、native `NativeVideoKeyEvent`、キャプチャ UI はいずれも
  `scan_code` と `extended` を保持し、既存 `matches_win32` / `from_win32` は
  extended=false を本体 Enter、true を `NumpadEnter` として正しく分離していた。
  一方、KeyHold held 判定だけは `GetAsyncKeyState(VK_RETURN)` を使用し、共有 VK の時点で
  区別を失っていた。高速タップ補完も本体 Enter 側では egui の Enter event を使うため、
  逆方向の誤発火余地があった。egui 0.33.3 の `physical_key` は
  `Option<egui::Key>` で、egui-winit が `Enter | NumpadEnter => Key::Enter` に畳むため
  scancode 代替にはならない。
- **根治設計**: main / fullscreen viewport の `WM_KEYDOWN/WM_KEYUP` edge から、
  `VK_RETURN` の extended=false/true を別々にラッチする。KeyHold held はこの物理ラッチ、
  高速タップ edge は同じ Win32 frame queue を使い、両経路を同じ物理 identity に統一した。
  フォーカス喪失 / 最終 subclass 破棄時は両ラッチを clear し、未配送 KeyUp の stale 状態を
  次フレームへ残さない。Enter 以外の固有 VK の held は従来どおり OS 直読みを維持する。
- **互換とキャプチャ**: `KeyName::parse` / settings 名 / 旧 `keymap.ini` の
  `Enter` / `NumpadEnter` 表記は変更していないため migration 不要。キャプチャ UI は既存の
  Win32 `from_win32(..., extended)` 経路を維持し、本体 / テンキーを別名で記録する。
- **回帰 coverage**: extended bit による chord の双方向不一致、main/numpad を同時押下しても
  独立する held latch、フォーカス喪失相当の latch clear を unit test 化。既存 keymap 85件
  (ini/settings round-trip、native VK、Numpad 名を含む) も green。

#### 自動検証と実機 smoke

- `cargo fmt`: exit 0。
- `cargo fmt --check`: exit 0。
- focused `cargo test keymap --bin mimageviewer-core`: exit 0、85 passed。
- focused `cargo test key_input --bin mimageviewer-core`: exit 0、3 passed。
- `cargo test --bin mimageviewer-core`: exit 0、3377 passed / 17 ignored / 0 failed。
- `.\scripts\build-release.ps1 -SkipVst3Bridge`: exit 0。core / launcher の release build 成功。
  既存 VST3 host と sha256 の削除は権限拒否 warning になったが、`-SkipVst3Bridge` の既存
  bridge 埋め込みと今回の core / launcher 生成は完了した。
- 実機未確認: `FsZoomMode = NumpadEnter` で本体 Enter がズームを起動せず、テンキー Enter
  だけが起動すること。本体 Enter の既定「一覧へ戻る」が維持されること。逆割当
  `FsZoomMode = Enter` でもテンキー Enter が起動しないこと。操作カスタマイズの
  「押して入力」が両 Enter を別表記で記録すること。

### 追補 第 11 弾 (2026-07-11): R1 第 3 波 P2 — media navigation resolver

レビュー / 検収: Fable / 実装・根因照合: Codex Sol。

- **観測と不変条件**: ParkedLive の EOF 候補解決中にメイン一覧の入力で App-global
  `input_seq` が進むと、別 context の結果が stale 破棄されていた。EOF 開始前に
  `video_continuous_last_eof` を記録済みなのに破棄時の解除が無く、同じ EOF は
  処理済みのまま永久停止した。また要求ごとの `media-nav-exists` spawn は、
  キャンセル不能な `Path::exists` 中の旧 thread を回収できず、遅延 NAS で連打回数分
  thread が累積した。context-local な新しい入力だけが古い結果を無効化し、EOF 結果を
  適用しなかった場合は同じ EOF を次 tick で再試行でき、存在確認 thread は App-global
  最大 1 本でなければならない。
- **stale 条件**: 結果適用は `items_generation`、ParkedLive
  `owner_window_id`、解決開始時の `fullscreen_idx` が現在の mounted bundle と一致する
  ことを必須にした。owner なしの mounted context だけは従来どおり `input_seq` 一致も
  要求し、後続の手動入力で古い手動送りを捨てる。owner ありの ParkedLive は main 入力で
  進む global `input_seq` を stale 条件に使わない。owner 不一致中は result channel を
  読まず pending と結果を保留し、該当 bundle が再 mount された時に検証・適用する。
- **EOF dedup rollback**: `VideoContinuousEof` /
  `VideoAudioModeContinuousEof` / `MusicContinuousEof` を共通 helper で
  `(fs_idx, seek_serial)` へ写像した。stale、後続要求による supersede、
  context close/load、resolver spawn/channel 失敗、apply 前の状態拒否のどれで捨てても、
  現在の latch がその key と一致する場合だけ `None` へ戻す。これにより新しい EOF key を
  誤って解除せず、EOF 由来だけを再試行可能にする。手動送り action は rollback 対象外。

#### 常駐 resolver

- `MediaNavigationResolver` は最初の要求時に `media-nav-resolver` thread を lazy 起動する。
  App は request sender / result receiver / 単調増加 request id を1組だけ保持する。
- worker は blocking `recv` 後に `try_iter` で mailbox を drain し、その時点の最後の要求だけを
  処理する。in-flight `Path::exists` は中断できないが、その間の新要求は同じ channel に溜まり、
  新しい OS thread を作らない。poll は pending の最新 request id と一致しない旧 response を捨てる。
- App drop で唯一の request sender が drop される。worker は in-flight I/O が戻った後、
  receiver disconnect で自然終了する。join は行わず、アプリ終了を OS I/O timeout で待たせない。
- soft timeout は追加しなかった。同期 `Path::exists` 自体を停止できないため、UI だけ先に
  「対象なし」へ落とすと、復帰を待って次候補を採用する既存動作を変える一方、後続 request の
  resolver 待ちは解消しない。今回の保証は UI 非 blocking と thread 数上限 1 であり、
  1 本が OS timeout まで遅れる点は残る。

#### 回帰 coverage と自動検証

- context-local stale predicate: generation / owner / fullscreen idx、mounted だけの input seq、
  ParkedLive での unrelated input seq 許容。
- stale EOF result の poll 破棄で dedup latch が解除されること、および video /
  動画音声モード / music の3 action が共通 rollback 対象であること。
- owner 不一致中は queued result を消費せず、owner 再 mount 後に適用すること。
- mailbox drain が最後の request だけを選ぶこと、旧 request id response を捨てて最新 response を
  適用すること、欠損実ファイル skip / 仮想 entry 非 stat の既存 coverage。
- `cargo fmt`: exit 0。
- `cargo test --bin mimageviewer-core`: exit 0、3383 passed / 17 ignored / 0 failed。
- `.\scripts\build-release.ps1 -SkipVst3Bridge`: exit 0。core / launcher の release build 成功。
  既存 VST3 host と sha256 の削除は権限拒否、停止済み Susie PID の再停止は not found warning
  だったが、今回の core / launcher 生成には影響しなかった。

### 追補 第 12 弾 (2026-07-11): 大量削除後の終了ハング

実機ログ解析 / 検収: Fable / 根因確定・実装: Codex Sol。

- **観測**: 約9,700ファイル削除後、IndexerManager dropの最初のsupervisor joinが
  170秒以上停止した。7 supervisorには先にsignal_stop済みで、main threadだけがjoin待ち、
  tray / VST3 heartbeatは生存していた。
- **根因**: watcher overflowのfull rescanまたは大量DebouncedChangeが入口になり得る一方、
  長時間停止を可能にした所有境界は search_walkerのGlobalIoSemaphore無期限acquire、
  ingest_workerの同acquire、およびFtsWriterDispatcherのreply無期限recvだった。
  さらにcancel検出後も保留delete batchをflushする経路があり、新しいwriter待ちへ入れた。
  よって仮説1 / 3が負荷の入口、仮説2が停止不能の直接原因である。
- **応答性修正**: walk entry / 3-way diff / delete / ingest loopへcheckpointを追加した。
  I/O permit取得とdispatcher batch応答は50ms timeoutでcancelを再確認する。
  cancel後は未flush batchをsubmitせず、event受信直後にも再確認してqueue済みwatcher eventを
  applyしない。既にsubmit済みのbatchはdispatcher内で完了し得るが、呼び出し側は待機を離脱する。
- **有界shutdown**: IndexerManager dropは全supervisor共通の4秒deadlineまでjoinし、
  超過handleをdetachしてjoined / detached数をログへ残す。final commitとdispatcherの最終所有は
  indexer-writer-finalizerへ移し、main threadはcommit / queue drain / dispatcher joinを待たない。
- **整合性**: cancel後に未submit batchを捨てても次回scanでFS差分として再検出される。
  submit済みbatchの応答を放棄してTantivyだけ更新された場合もSQLiteを先行更新しない
  Tantivy Firstを維持し、次回のFS / Tantivy / fts_meta 3-way diffで再投入・再削除される。
- **catalog migration**: source_width / source_heightはpragma_table_infoで存在確認してからALTERし、
  並行open競合時のduplicate columnだけを無言のidempotent successとした。1017行は
  CatalogDb openごとに既存列へALTERしていたためのログ増幅である。catalogはフォルダ単位DBで
  cold worker openもあるためopen回数だけでリークとは断定できず、今回のjoin停止原因でもないが、
  1セッション1017 openは高めなので将来のpath付き計測 / cache hit率監査候補とする。

#### 回帰coverage

- permit待機中のcancelで500ms以内に未取得return。
- blocked dispatcherの後ろにsubmitしたbatch待機がcancelで500ms以内にreturn。
- deadline消費済みのjoin seamがworkerをdetachして即return。
- cancel後のqueue済みchangeをdiscard。
- legacy catalogへ列を一度だけ追加し、2回目initが成功して列が重複しない。

#### 自動検証

- cargo fmt / cargo fmt --check: exit 0。
- focused回帰5件: exit 0。
- cargo test --bin mimageviewer-core: exit 0、3388 passed / 17 ignored / 0 failed。
- .\scripts\build-release.ps1 -SkipVst3Bridge: exit 0。core / launcher release build成功。
  既存VST3 host本体とsha256の削除はaccess denied warningだったが、SkipVst3Bridgeの
  既存bridge埋め込みと今回のcore / launcher生成には影響しなかった。

### 追補 第 13 弾・改 (2026-07-11): 全メタ hard purge / missing 非破壊の統一

実機 FB / 仕様裁定: ユーザー、検収: Fable、実装: Codex Sol。

- **差し替え経緯**: 当初の第13弾は rating だけを `deleted_at_ms` tombstone + prewarm unflag
  で扱ったが、タグ・補正・回転等と削除意味論が分裂し、ごみ箱復元時の挙動もストアごとに
  異なるため撤回した。`deleted_at_ms` は未コミット・未出荷列だったので schema 追加そのものを
  撤去し、既存ユーザーの `rating.db` に migration は不要。
- **トリガー分離**: (1) mIV の `delete_worker` が Shell 成功を確認した path は、全
  path-keyed メタを hard purge。(2) スキャン・検索・一覧復元・履歴 open の missing は、
  外付け切断 / NAS offline / 権限エラーを含み得るため表示除外・open 抑止だけで DB は非破壊。
- **共通ストア正本**: `rename_key_migration::STORES` に keep-drive / drive-stripped の正規化、
  DB / table / key column、rename の一意性を集約。rename と purge が同じ記述子を走査するため、
  新ストア追加時に対象がずれない。読書履歴は rename 時だけ raw path 更新を伴う専用処理、
  PDF password は SHA-256 key のため削除前に worker が配下 PDF path を列挙する専用処理。
- **削除連携**: Shell 成功 batch を UI へ送る前に worker 上で exact + `<key>/` +
  `<key>::` を `DELETE`。UI 完了ハンドラは rating / tags / rotation cache、タグ候補、
  folder count、編集済み presence set、動画 marker / resume、読書履歴 cache、PDF password
  in-memory store を同じ境界で整合させる。rename は従来 migration だけで、削除 purge と二重実行しない。
- **missing 横断監査**: rating view / count / folder counter / prewarm には missing 起因の
  DELETE は無かった。タグビューの `should_prune_missing_path` + `prune_items` を撤去し、結果から
  隠して行を保持するテストへ変更した。さらに読書履歴 open guard に missing 時の DB 削除が
  あったため、toast + open 抑止のみへ非破壊化した。Tantivy / FTS / catalog / archive cache の
  stale delete は再生成可能な索引・cache であり、ユーザーメタではないため対象外。

#### 回帰 coverage

- 共通 `STORES` の全 descriptor で exact / `<key>/` / `<key>::` hard purge と隣接 prefix 保持。
- rating hard delete 後の `count_by_stars` 即時減少。
- PDF password の exact / フォルダ配下 hash purge。
- タグ検索 missing の結果非表示 + `item_tags` / `tag_item_state` 保持。
- 削除完了時の rating / tags cache と補正・回転 presence set 整合。

#### 自動検証

- `cargo fmt` / `cargo fmt --check`: exit 0。
- `cargo test --bin mimageviewer-core`: exit 0、3393 passed / 17 ignored / 0 failed。
- `python scripts/check_ui_glyphs.py`: exit 0、dangerous glyph 0。
- `.\scripts\build-release.ps1 -SkipVst3Bridge`: exit 0。core / launcher release build成功。
  既存 VST3 host 本体と sha256 の削除は access denied warning だったが、SkipVst3Bridge の
  既存 bridge 埋め込みと今回の core / launcher 生成には影響しなかった。

### 追補 第 14 弾 (2026-07-11): 本棚追加時 full composite の実現可能性調査

実装: Codex Sol / 検収予定: Fable。

- **判定はケース C**。隠蔽 / 補正レイヤー / テキスト / 補正 / 回転は path キーの DB データと
  CPU 合成部品から headless 再構成できる。一方、消しゴムは `mask.db` に bitmap + shapes だけを
  保存し、MI-GAN / diffusion の確定画素はメモリ上の `erase_result_cache` にしか保持しない。
- Ctrl+E の最終合成は任意 path 用の純粋 API ではなく、表示 item の `idx`、`fs_cache`、各 edit
  result cache、`final_composite_cache`、`egui::Context` に接続されている。消しゴム結果が無い場合は
  `ensure_erase_result_texture` が保存マスクから MI-GAN を非同期再投入し、完了まで export を保留する。
- 任意 path の製本 worker で同じことを行うには AI runtime / model を新経路から利用する必要がある。
  モデル不在・ロード失敗では diffusion fallback になるため、編集確定時にユーザーが確認した画素と
  同一になる保証がない。よってマスクの存在だけで焼き込み対象にしても「フル最終 composite」の
  確定仕様を満たさない。
- 指示されたケース C の停止条件に従い、`BookPageSource`、`write_source`、焼き判定、UI、マニュアルは
  変更していない。選択肢は (1) 検証キー付き消しゴム確定 artifact の永続化、(2) 製本時の MI-GAN
  再推論と非同一可能性の仕様許容、(3) 消しゴムだけ表示後追加を要求する hybrid。詳細は
  `docs/compile-book-plan.md` の第 14 弾調査節を正本とする。
- 調査のみのため `cargo fmt`、テスト、release build は未実施（コード変更なし）。

### 追補 第 15 弾 (2026-07-11): 削除確認 Y/N と本棚未焼き込み編集の警告

実機 FB / 仕様裁定: ユーザー、実装: Codex Sol、検収予定: Fable。

- **削除確認の固定入力**: `show_delete_confirm_dialog` は Y = 削除、N / Esc = キャンセル、
  Enter = 無効とした。`dialog_escape_pressed(ctx)` と `ime_input_active()` は Window closure 前に
  capture し、IME 変換中は Y / N / Esc の確認 action を発火しない。Y / N / Esc の egui key event は
  ダイアログ描画時に consume するため、N / Esc で同 frame にダイアログが閉じても後段の
  `consume_key` / keymap dispatch へ漏れない。Delete action は既存ボタンと同じ reducer で path を
  take し、フラグ / targets / label を clear して `start_delete_files` へ渡す。
- **ビューポート確認**: この確認ダイアログを開くのはメイングリッドの Delete / context menu 経路だけで、
  `show_delete_confirm_dialog` も main viewport だけで描画される。現行 FS context menu は同ダイアログを
  開く削除項目を持たないため、fullscreen viewport 用の Y / N 配線追加は不要だった。
- **横展開候補 (今回は未変更)**: サムネイルキャッシュ全削除、アーカイブキャッシュ全削除、
  本マネージャの本削除、お気に入り削除、TensorRT パック削除、編集用追加ファイル削除の各確認は、
  confirm ボタンがマウス中心で Y / N 固定操作を持たない。cache 2 種は Esc キャンセル済み。
  destructive confirmation 全体の統一はユーザー裁定待ちとし、今回の対象を広げていない。
- **本棚警告トースト**: Grid / fullscreen / stack member の通常ページ追加前に、既存の path-key
  presence set で conceal / mask / local_adjust / comic の 4 種だけを exact 判定する。adjustment / rotation
  は対象外。複数ページに複数編集があっても単一 book worker の pending flagへ OR 集約し、Append 成功時の
  完了トースト 1 本へ「本棚のファイルには反映されない」警告を連結する。失敗時、動画フレーム、
  クリップボード追加では出さない。フル composite 焼き込み自体は backlog §1.14 の v2.4.0 対応のまま。
- **ドキュメント / smoke**: keymap 固定入力理由、spec、compile-book-plan、削除 / 製本マニュアルを更新し、
  `full-verification-checklist.md` に「Y / N / Esc / Enter / 背面漏れ」と本棚警告の実機 smoke を追加した。
  `tests/ui_snapshot.rs` には削除確認ダイアログの snapshot は無く、ボタン文言・配置も変更していないため
  snapshot 更新は不要。

#### 回帰 coverage と自動検証

- Y / N / Esc / Enter の action、IME 中の Y / N 無視、Delete worker 直前の path take / cancel state、
  Y / N / Esc consume 後に背面 KeyAction が発火しないこと。
- conceal / mask / local_adjust / comic 各 path key の warning pending と完了トースト、
  adjustment only / no edit の非発火、items に現れない stack member の path-key 判定と複数追加の 1 回集約。
- `cargo fmt` / `cargo fmt --check`: exit 0。
- `python scripts/check_ui_glyphs.py`: exit 0、dangerous glyph 0。
- focused tests: exit 0 (delete confirm 13 件 + book warning 3 件)。
- `cargo test --bin mimageviewer-core`: exit 0、3402 passed / 17 ignored / 0 failed。
- `cargo test --test ui_snapshot`: exit 0、15 passed / 0 failed (削除確認 snapshot は対象外、既存 snapshot 差分なし)。
- `.\scripts\build-release.ps1 -SkipVst3Bridge`: exit 0。core / launcher release build 成功。
  既存 VST3 host 本体と sha256 の削除は access denied warning だったが、SkipVst3Bridge の既存 bridge
  埋め込みと今回の core / launcher 生成には影響しなかった。

## 追補第16弾: 孤児メタデータ整理 (全ストア横断、明示トリガー)

- **入口**: 設定 → サムネイルキャッシュ管理 →「メタデータを整理…」。自動実行はしない。
- **worker**: `metadata-cleanup-scan` が正本 `rename_key_migration::STORES` の全行と実体を確認し、
  ストア別件数を返す。「整理する」確認後だけ `metadata-cleanup-delete` が DELETE する。
  scan/delete とも cancel + atomic 進捗を持ち、delete は descriptor 単位 transaction。削除直前にも
  実体と親を再判定する。
- **オフライン保護**: 物理実体が `try_exists() == Ok(false)` で、直上親が実在 directory の場合だけ
  orphan。親ごと見えない切断ドライブ / NAS / 権限エラーは保護して残す。製本用本棚配下も除外。
- **in-memory**: 完了結果の exact key で rating/tag/folder-pin cache と編集 presence set を更新し、
  `rating_counts_cache` / folder rating count を無効化。表示中の tag/rating 一覧は worker で再構築する。

| ストア | 判断 |
| --- | --- |
| rating / adjustment(page) / mask / conceal / local-adjust / comic / export-crop / tags(item×2) / rotation / view-trim(page) / video pins / video bookmarks | keep-drive path または `container::entry` の物理 container を判定して整理対象 |
| adjustment.sidecar-sync / tags.sidecar-sync | `folder_key` のフォルダ実体で判定 |
| folder thumbnail pins | 通常 folder/file はその実体、ZIP 内本の合成 key は最初の archive component を物理 container として判定 |
| book-resume / spread / view-trim(book) | drive-stripped key からオンラインの正しい drive を一意に逆引きできないため対象外 |
| PDF passwords | SHA-256 key だけで元 PDF path を逆引きできないため対象外 |
| reading-history | missing でも「以前読んだ記録」というユーザー価値と既存 spec の非破壊方針を優先して残す |
| 製本用本棚配下 | 通常削除フローと別管理のため全ストアで対象外 |

自動 coverage は、親あり missing / 親ごと missing / 実在 path、ストア別集計、folder/hash/drive-stripped/
本棚判断、delete、途中 cancel rollback、整理後の rating count / presence set 無効化を追加した。

検証結果:

- `cargo fmt` / `cargo fmt --check`: exit 0。
- `python scripts/check_ui_glyphs.py`: exit 0、dangerous glyph 0。
- focused metadata cleanup tests: 7 passed / 0 failed。
- `cargo test --bin mimageviewer-core`: exit 0、3409 passed / 17 ignored / 0 failed。
  初回並列実行では既存の ZIP 本 rating undo test が 1 回だけ揺れたが、単独再実行と全体再実行は成功。
- `cargo test --test ui_snapshot`: exit 0、15 passed / 0 failed。既存 snapshot に新ダイアログは
  含まれず、既存画像差分なしのため PNG 更新なし。
- `.\scripts\build-release.ps1 -SkipVst3Bridge`: exit 0。core / launcher release build 成功。
  既存 VST3 host と sha256 の削除は access denied warning だが、既存 bridge 埋め込みと生成物には影響なし。

## 追補第17弾: 削除 purge 失敗の永続再試行 + フレーキーテスト堅牢化

- **失敗の所有境界**: `delete_worker` は Shell 成功 path の hard purge を初回 + 3 回試し、
  なお `PurgeReport.errors` が残る場合だけ `delete_purge_journal.json` へ path 単位で永続化する。
  フォルダ削除時の PDF password 用に、削除前に列挙した配下 PDF path も entry に保持する。
- **孤児整理との関係**: journal は全ストア scan を自動実行せず、削除成功済み path だけを
  `delete-purge-retry` worker でピンポイント処理する。ストア正本と DELETE は第13/16弾と同じ
  `rename_key_migration::STORES` / `purge_removed_paths_at` を再利用し、再試行前には第16弾と同じ
  「親へ到達可能 + path 不在」を確認する。同名 path が再作成済み、または親ごと見えない場合は
  新メタの誤削除を避けて entry を残す。起動時、1秒入力 idle 後、失敗時は10秒 backoff後に再実行し、
  成功 entry だけ journal から atomic に消し込む。
- **sidecar**: `SidecarFile::flush()` を成否 bool にし、削除 root をメモリ上で除いても flush が失敗した
  場合は purge rows に加算せず error とする。この error も DB lock と同じ journal 再試行対象になる。
- **フレーク特定**: `fts_writer_dispatcher::interactive_preempts_queued_background` は500ms sleep中に
  4 jobを投入し、submit側スレッドの elapsedを比較していた。dispatcherが正しい順で処理しても
  parallel負荷で完了側スレッドの再スケジュール順が逆転し得るため、test-only `TestBlock` / `TestMark`
  で queue `(interactive=1, background=3)` を確定後に解放し、dispatcher自身の処理イベント順を
  検証する決定的テストへ置換した。製品 job / dispatcher 動作は変更していない。
- **負荷時マージン**: `io_semaphore` / dispatcher cancel / `indexer_manager` の待機 deadline は、
  条件待ち・cancel pollingという意味論を維持したまま、フル並列CPU競合用に10〜20秒へ拡大した。

自動 coverage:

- purge 最終失敗を注入 → 初回 + 3 retry → journal 作成。
- journal entry + 孤児 rating → retryでDB行削除 → journal消し込み。
- sidecar flush失敗 → rowsへ非加算 + error。
- dispatcher queueのInteractive先行をsleep / elapsedなしでイベント順検証。

検証結果:

- `cargo fmt` / `cargo fmt --check`: exit 0。
- focused purge / sidecar / dispatcher / cancellable tests: exit 0。
- `io_semaphore` / `fts_writer_dispatcher` / `indexer_manager`: 20反復ずつ、計60 cargo test runが
  すべて exit 0。`cargo test --lib` フル並列も5反復すべて exit 0。
- `cargo test --bin mimageviewer-core`: exit 0、3412 passed / 17 ignored / 0 failed。
- `.\\scripts\\build-release.ps1 -SkipVst3Bridge`: exit 0。core / launcher release build成功。
  既存VST3 host本体とsha256の削除は従来同様 access denied warningだが、既存bridge埋め込みと
  今回の core / launcher生成には影響なし。

## 追補第18弾: 削除確認ラベル + 重要な変更点 + 更新履歴

- **削除確認ラベル**: 固定入力を画面上でも明示するため、ボタンを「削除[Y]」と
  「キャンセル[N]」へ変更した。Y / N / Esc の割り当て、Enter 無効、IME 中の抑止、
  背面 KeyAction への漏出防止は第15弾のまま変更していない。
- **更新後の重要な変更点**: v2.3.0 の `must_read` に、ファイル削除時はレーティング・タグ・
  補正・回転なども消え、ごみ箱から戻しても復元されないことを追加した。過去の削除で残った
  データの整理経路と、取り外し中の外付け / 接続不能ネットワークドライブを対象外にする保護も
  同じ画面で案内する。
- **更新履歴**: README.md と `release-note-drafts.md` に、削除時のデータ消去と
  「設定 → サムネイルキャッシュ管理 → メタデータを整理…」の整理機能を追記した。
- **実文言 / snapshot**: 起動点は `cache_manager.rs` の「サムネイルキャッシュ管理」内にある
  「メタデータを整理…」で一致した。削除確認ダイアログは既存 snapshot に収載されておらず、
  `version_highlights` の snapshot も固定 fixture のため PNG 更新は不要。

### 第18弾 実機 smoke

- [ ] 削除確認ボタンが「削除[Y]」/「キャンセル[N]」と表示される。
- [ ] 更新後初回起動で、削除するとレーティング・タグ・補正なども消える重要なお知らせが出る。

### 第18弾 自動検証

- `cargo fmt`: exit 0。
- `cargo test --bin mimageviewer-core`: exit 0、3412 passed / 17 ignored / 0 failed。
  途中、既知の `undo_restores_zip_book_container_rating_key` が 1 回だけ揺れたが、単独再実行と
  bin 全体再実行は成功した。
- `cargo test --lib version_highlights::`: exit 0、15 passed / 0 failed。
- `python scripts/check_ui_glyphs.py`: exit 0、dangerous glyph 0。
- `cargo test --test ui_snapshot`: exit 0、15 passed / 0 failed。既存 PNG 差分なし。
- `.\scripts\build-release.ps1 -SkipVst3Bridge`: exit 0。core / launcher release build 成功。
  既存 VST3 host 本体と sha256 の削除は access denied warning だったが、既存 bridge 埋め込みと
  今回の core / launcher 生成には影響しなかった。

## 追補第19弾: App テストの削除 purge-retry 隔離

- **Fable 適用分を保持**: `App::new_for_test` は本番既定 `true` の
  `delete_purge_retry_needed` をテスト harness だけ `false` に戻す。`App::update` を回す
  並列テストが process-global `data_dir` override 越しに別テストの DB を purge しない契約を維持した。
- **journal coverage**: retry の意味論は App update に依存せず、
  `metadata_cleanup::tests::delete_purge_journal_retries_confirmed_orphan_and_clears_itself` が
  「journal enqueue は DB 非変更 → 到達可能な orphan を再 purge → 成功 entry 消し込み」を直接検証する。
- **反復中に見つけた追加隔離漏れ**: 初回確認 run で
  `undo_restores_zip_book_container_rating_key` が `rating_db == None` となり再発した。原因は第16弾の
  `metadata_cleanup_result_invalidates_counts_and_presence_sets` だけが `setup_app()` を通らず
  `App::default()` を直接生成し、別 App test の process-global `data_dir` override 上で同じ
  `rating.db` schema open と競合していたこと。製品コードや Fable 適用行は変えず、このテストも
  共有 lock + RAII cleanup 付き `phase_c_support::setup_app()` に統一した。
- **反復結果**: 修正後の `cargo test --bin mimageviewer-core` 既定並列を20回反復し、
  全 run が 3415 passed / 17 ignored / 0 failed (合計 68,300 passed)。undo 系と
  `fts_writer_dispatcher::interactive_preempts_queued_background` は全 run に含まれ、再発なし。

## 追補第20弾: 削除時メタ purge の末尾1回化

- **purge 所有境界**: `delete_worker` は最大100件の Shell recycle chunk ごとに
  `DeleteMsg::Batch` を送り、進捗を従来どおり更新する。同時に Shell 成功 path と削除前 PDF candidate
  を worker 内へ蓄積し、全チャンク終了後に `purge_removed_paths_at` を成功 path 全体へ1回だけ呼ぶ。
  1400件なら SQLite store open は従来の約14 × 16回から約16回へ減る。
- **cancel / UI 整合**: chunk 境界の cancel では、すでに recycle 成功した path だけを末尾 purge する。
  `Done` は purge と、最終失敗時の journal 永続化が終わってから送るため、UI の items 除去と
  rating/tag/rotation 等の in-memory clear は永続 purge より後になる。進捗先行は維持するが、
  purge 完了前に UI state を消す protocol 変更は行わなかった。
- **PDF 走査スキップ**: worker 起動時に `pdf_passwords.json` を1回だけ読み、保存 entry が0件
  (空 / 不在 / 壊れた JSON) なら `collect_pdf_paths_for_delete` 自体を呼ばず、フォルダの
  `read_dir` 再帰を丸ごと省く。entry がある場合だけ従来どおり削除前に列挙する。
- **journal / perf**: 初回 + 最大3 retry と `delete_purge_journal.json` の意味は維持した。
  `perf::event("delete", "metadata_purge", ..., "worker_tail")` に purge ms、論理 attempt、
  SQLite DB open attempt、成功 path、PDF path、削除 row、最終 error 数を記録する。
- **回帰 coverage**: 201 path / 3 chunk でも purge closure 1回、1 chunk成功後 cancel は
  成功済み100件だけ purge、空 PDF password store は collector 0回、Shell失敗 pathは非 purge、
  purge最終失敗はjournal化、を `delete_worker::worker_tests` で固定した。
- **自動検証**: `cargo fmt` / `cargo fmt --check` exit 0。focused delete worker は
  10 passed / 0 failed、journal / dispatcher / ZIP本 rating undo の各 focused test は green。
  full bin と20回反復の結果は追補第19弾のとおり。
- **release build**: `.\scripts\build-release.ps1 -SkipVst3Bridge` exit 0。core / launcher の
  release build 成功。既存 VST3 host 本体と sha256 の削除は access denied warning だったが、
  `-SkipVst3Bridge` の既存 bridge 埋め込みと今回の生成物には影響しない。

## 追補第21弾: 削除 purge の index-fast 化

- **根因 / 修正境界**: 共通 `purge_store` は削除キーごとに exact + folder prefix +
  container prefixをOR接続し、列への `substr` 適用で全表scanになっていた。exactを最大500件の
  `DELETE ... IN (...)` batchへ分離し、2種prefixは
  `col >= prefix AND col < next(prefix)` のrange DELETEへ変更した。1ストア1transaction、
  初回+最大3 retry、journal、`Done`後送の順序は維持した。孤児整理はexact DELETEのみで
  同型の関数条件を持たないため変更していない。
- **upper / COLLATE**: `prefix_upper_bound` は最後のUnicode scalarを1つ進めた排他的上限を作る。
  hard purgeのprefixは `/` / `:` 終端なので通常は `key/` → `key0`、
  `key::` → `key:;` となる。次scalarが作れないsurrogate境界 / `char::MAX` は旧
  `substr` へ安全にfallbackする。全 `STORES` の対象列は宣言上既定BINARYで、PRIMARY KEY、
  `item_tags(item_key, tag_key)` の左端、または `idx_video_bookmarks_path` の索引を持つ。
  範囲境界は従来と同じkeep-drive / drive-strippedの小文字化・slash統一キーから構築する。
- **正しさ / planner coverage**: 旧substr版とrange版へexact file、folder配下、
  container配下、隣接prefix、`%` / `_` / 日本語を投入し、削除row数と残存集合の一致を固定した。
  `EXPLAIN QUERY PLAN` はIN / rangeとも `SEARCH`、`SCAN`なしを確認した。
- **perf**: debug unit testの27,000行PRIMARY KEY table + 1,000削除キーで、exact 2 batch +
  空のprefix range 2,000本を含むpurge本体は **6.9 ms**。5秒上限の回帰テストを追加した。
- **focused検証**: `cargo fmt` exit 0。
  `cargo test --bin mimageviewer-core rename_key_migration::tests:: -- --nocapture` は
  15 passed / 0 failed、exit 0。
- **全体検証 / build**: `cargo fmt --check` / `git diff --check` はexit 0。
  `cargo test --bin mimageviewer-core` は3419 passed / 17 ignored / 0 failed、exit 0。
  `.\scripts\build-release.ps1 -SkipVst3Bridge` はexit 0でcore / launcher release build成功。
  既存VST3 host本体とsha256の削除は従来同様access denied warningだったが、
  `-SkipVst3Bridge` の既存bridge埋め込みと今回の生成物には影響しない。

## 推奨アクション (現在、優先順)

1. **ユーザー裁定**: 第14弾の消しゴム確定画素を artifact 永続化 / 製本時に再推論 / hybrid の
   どれで扱うかを確定する。裁定までは full composite 焼き込みを実装しない。
2. **実機 smoke**: 追補第13弾・改の「mIV削除で★件数が即減り、補正/タグ/回転も消える /
   同 path へごみ箱復元後もメタは戻らない / 外付け切断中のタグ検索はDB非破壊」、
   追補第17弾の「削除中DB lockでpurge失敗 → lock解除・再起動でjournal再試行し旧メタ消去」、
   追補第12弾の数千ファイル削除直後の終了、追補第10弾の本体 /
   テンキー Enter 双方向分離、checklist の P2-11 継続項目、
   NAS/切断ドライブ相当の EOF/次送り (連打でも resolver thread 最大 1、ParkedLive 中の
   main 入力後も EOF 再試行)、
   音声タグ結果、タグビューの種類 = 音声、MP3 の Ctrl+G ファイル名 hit → 音楽ビュー再生、
   種類ファセットの音声絞り込みを確認する。
3. **将来の detached リワーク**: bundle × App-global runtime のさらなる所有境界一本化は、
   凍結中の構造課題として既存リワーク計画へ引き継ぐ。
