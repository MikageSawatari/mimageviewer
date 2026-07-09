# v2.3.0 出荷前レビュー: モード分岐の正しさ (D3)

レビュー対象: `7eff5a9e..01910684`。観点 = モード空間
{フル機能 / マルチウィンドウ / F12 detached (active・passive・parked)} ×
{グリッド / FS 画像 / native 動画 / 音楽ビュー / 動画→音声モード / 音声モード+VST GUI} ×
{定常 / 遷移中} の判別誤り・取りこぼし。

調査方法: brief 記載の統一述語 (`viewer_session_is_detached[_or_switching]` /
`detached_video_presentation_active_or_targeted` / `fs_music_view_active` /
`fullscreen_uses_video_ring_context` / `detached_active_window_alive_wanted`) の全呼び出し箇所を
rg で列挙し、`video_audio_mode` / `video_audio_vst` / `viewer_presentation` /
`active_detached_viewer_context` / `GridItem::Audio` の生読みと突き合わせ。入力系
(handle_fs_key_input / handle_native_video_key_event / ring / gamepad / 右ドラッグ / tray) の
ガード条件を個別に照合した。

---

## 指摘 (深刻度順)

### [P2] F12 detached (active) の動画から音声モードに入れる経路が全て開いており、brief の「未対応」と実装が矛盾。smoke 未カバーの組合せ

- 場所: src/app/native_video.rs:6858 (`enter_video_audio_mode`)、同 :2917 (HUD ♪ →
  `ToggleAudioMode` イベント)、同 :6477 (native Z キー)、同 :1302
  (EOF keep_audio_mode swap 完了 → 再入場)、src/video/native_presenter/overlay_draw.rs:2787
  (♪ ボタンは detached でも無条件描画)
- シナリオ: 動画を F12 で detached 表示 → native HUD の ♪ ボタン (または Z キー) →
  **detached ウィンドウ内で音声モード (hidden presenter + egui 音楽ビュー) に入れる**。
  brief の既知の残課題 #1 は「Inc7e 残作業で未対応」と明記しているが、コードは
  7e で意図的に解禁済み: `enter_video_audio_mode` のコメントに「フルスクリーン /
  ウィンドウ内 / **別ウィンドウ (F12 DetachedWindow) のいずれでも使える**」とあり、
  detached では `entry_target` 捕捉必須 + switch/host-switch in-flight ブロック +
  音声モード中 F12 の choke (src/app.rs:41753) というガード構成になっている。
- 根拠: `enter_video_audio_mode` に detached 拒否は無い (拒否条件は
  ①mode 済み ②非動画/非現 idx ③switch・swap・host-switch in-flight ④presenter 未確立
  ⑤音声トラック無し ⑥detached で entry_target 取得不可、のみ)。一方
  native_video.rs:6473 のコメントは「enter_video_audio_mode 内で … **detached** /
  switch 中などは弾かれる」と stale な記述のまま (7d 時代の名残)。さらに
  `docs/detached-rework-ship-checklist.md` の機能マトリクスには
  「音声ファイル F12 (F9)」「♪×F11 (V5)」「♪ park (P7)」はあるが、
  **「F12 detached 中の動画 → ♪/Z」の active detached × 音声モードのケースが無い**。
  hidden presenter (WS_CHILD、detached host にぶら下がる) × host 再生成
  (`try_resync_detached_video_host` は音声モード中 skip、native_video.rs:660-676) ×
  exit 時の SwitchPlacement という 3 者が重なる、リワークで最も壊れやすいクラスの
  組合せが検証網の外にある。
- 要求される対応 (どちらか): (a) 未対応が正なら入場ガード
  (`viewer_presentation==DetachedWindow` で ♪/Z を toast 付き拒否) が**欠けている**、
  (b) 対応済みが正なら brief/残課題リストと :6473 コメントを更新し、ship checklist に
  「detached 動画 → ♪ → exit / F11 / host 再生成 / EOF 継続」の smoke 行を追加する。
- BA マッピング: BA-7 (状態組合せが明示状態機械の外にあり、検証マトリクスからも漏れる)
- 確度: 経路の存在 = 高 (コードで確認)。「入ったら壊れる」かは未検証 = 中

### [P2] 音楽ビュー (音声ファイル) で Z キーが ZipPla 全画面ズームをラッチする — `fs_zoom_mode_context_ok` が Audio / 音楽ビューを除外していない

- 場所: src/ui_fullscreen.rs:2142 (`fs_zoom_mode_context_ok`)、:9386
  (`update_fs_zoom_mode_keys` 呼び出し — 音楽ビューのキーゲートより**前**)、
  :2151 (`!matches!(items.get(fs_idx), Some(GridItem::Video(_)))` — Video のみ除外)
- シナリオ: 音声ファイルをフルスクリーン再生 (音楽ビュー) → Z を押して離す
  (動画→音声モードの Z トグルの癖で押すのが自然な操作) →
  `update_fs_zoom_mode_keys` が `take_key_hold_edges(FsZoomMode)` で Z イベントを消費し、
  `fs_zoom_aiming` → `fs_zoom_active` がラッチされる。音楽ビュー自体は
  `draw_fs_music_view` 経路なので見た目の変化がほぼ無く、ユーザーは何も起きて
  いないと思う。その後:
  - Z ホールド中のホイールが `adjust_fs_zoom_factor` に吸われる
    (src/ui_fullscreen.rs:11556-11559、`should_handle_fullscreen_wheel` は音楽ビューを見ない)
  - `fs_zoom_mode_engaged()` により `cursor_in_panel` 判定が無効化される (:11275)
  - `fs_zoom_active` は **viewer context bundle のフィールド** (swap_field 対象) なので
    セッション内で残存し、↑↓で画像へナビした瞬間に ZipPla ズームモードの画像表示に
    なる (open_fullscreen 内部ナビは `fs_zoom_reset` を呼ばない。reset は
    close_fullscreen = app.rs:35710 のみ)
- 根拠: `fs_zoom_mode_context_ok` の除外リストは 見開き/パノラマ/分析/編集/比較/**Video**
  のみ。**動画→音声モードは item が Video なので偶然守られ、純粋な音声ファイル
  (`GridItem::Audio`) だけがすり抜ける**非対称 — 「モード判別を誤って特定の組合せだけ
  壊れる」の典型。Codex 7c/7e が `fs_music_view_active` に集約した音楽ビュー判定を、
  この述語だけ経由していない。修正は `|| self.fs_music_view_active(fs_idx)` を除外条件に
  足すだけで済む見込み (音楽ビューの Z no-op consume (:9749) は既存のまま機能する)。
- 確度: コード経路 = 高。実機での見え方 (どこまで悪目立ちするか) = 中

### [P3] リング / ゲームパッドの「ウィンドウ / 全画面切り替え」が音声ファイル音楽ビュー (非 detached) で無反応

- 場所: src/app/gamepad_input.rs:4634 (`apply_ring_toggle_window_mode` の
  `VideoFullscreen` 分岐 → `toggle_video_window_mode_for_input`)、
  src/app/native_video.rs:2348 (`toggle_video_window_mode`) →
  :2128 (`switch_native_video_viewer_presentation` の `GridItem::Video` ガードで silent return)
- シナリオ: 音声ファイルの音楽ビュー (フル機能 1 ウィンドウ) で、リングショートカット /
  ゲームパッド / 右ドラッグの ToggleWindowMode を実行 → 何も起きない。
  同じ操作が **F11 キー (ui_fullscreen.rs:10401 `!current_is_video || fs_music_view_active`
  → `toggle_egui_viewer_window_mode_for_input`) と音楽 chrome の window ボタン
  (:21805) では動く**ので、入力手段によって挙動が割れる。
- 根拠: 音楽ビューのリング context は `fullscreen_uses_video_ring_context` により
  VideoFullscreen になった (2026-07 実機 FB 対応) が、VideoFullscreen 側の
  ToggleWindowMode 実装は `toggle_video_window_mode_for_input` のままで、音声ファイルは
  `video_audio_vst` (None) → `video_audio_mode` (None) → detached (No) の choke を
  すべて素通りして native 動画用の placement switch (item Video ガードで no-op) に落ちる。
  `fs_music_view_active` 分岐の追加漏れ (VideoLoop / VideoBookmark / VideoMarkerPrev/Next /
  VideoTileMode は 9a952d26 で対応済み、ToggleWindowMode だけ漏れ)。
  detached の音声は :2337 の detached 分岐 (borderless toggle) で偶然動くため、
  「非 detached の音声ファイルだけ」壊れる組合せ。
- 確度: 高

### [P3] リング VideoCapture / VideoExternalPlayer が音楽ビューで非対応のまま露出している

- 場所: src/app/gamepad_input.rs:4539 (`RingActionId::VideoCapture`)、:4612
  (`RingActionId::VideoExternalPlayer`)
- シナリオ:
  - 音声ファイル音楽ビューでリングの VideoCapture → `save_video_frame_to_file`
    (ui_fullscreen.rs:18568) は player.error 無し・info 有りで通過し、音声ファイルに
    対して動画フレームキャプチャワーカーが走る (失敗系 / 無意味な結果)。
  - 動画→音声モードでは hidden presenter の hold フレーム (映像は detach 時点で固着)
    がキャプチャされる。7-③④ で native HUD からキャプチャパレット / コマ送りを
    非表示にした整合が、リング経路には及んでいない。
  - VideoExternalPlayer は `GridItem::Video` ガードで音声ファイルは silent no-op
    (音声も外部プレイヤーで開けて良いはずなので、無反応はどちらにせよ不親切)。
- 根拠: 同ファイル内の VideoLoop / VideoBookmark / VideoMarker / VideoTileMode には
  `fs_music_view_active` 分岐・無効化 toast (Codex P2/P3 対応) があるのに、この 2 つには無い。
- 確度: 中 (経路は確実、save_video_frame_to_file の音声入力時の実挙動は未追跡)

### [P3] native VK 経路の detached Enter/Esc 固定分岐がタイルモード / ノーマライズモーダルと衝突し、keymap も迂回する (範囲外由来・残存)

- 場所: src/app/native_video.rs:6051-6060
  (`viewer_session_is_detached() && matches!(key.virtual_key, 0x0D | 0x1B) → close_fullscreen`)
- シナリオ (detached 動画のみ発生、非 detached は正常):
  1. detached 動画で S (タイル一覧) を開いて Esc → 非 detached では
     `close_video_tile_mode` がタイルだけ閉じるが、detached ではこの分岐が先に
     マッチして**ウィンドウごと閉じる**。Enter でのタイル再生開始 (:6103) も同様に
     close に化ける。
  2. detached 動画で仮 gain 適用前のノーマライズスキャン (モーダル) 中に Esc →
     非 detached では scan cancel、detached では close_fullscreen
     (モーダルガード :6045 は Esc を素通しし、その直後にこの分岐が奪う)。
  3. VK 0x0D / 0x1B ハードコードなので keymap で Enter/Esc を remap しても
     detached だけ旧挙動 (keymap-spec の「固定扱いは理由を文書化」ルールにも未記載)。
  4. 判定が `viewer_session_is_detached()` (or_switching でない) なので、placement
     switch 進行中 (deadline 最大 5 秒) は presentation 旧値のままキー解釈が行われる。
- 根拠: この分岐は rating キー判定・`matches_vk_action(VideoCloseFullscreen/VideoPlayPause)`・
  0x1B の tile/normalize 分岐 (:6117-6125) より**前**に置かれている。
  commit 1bc9cf99 (v2.2.0 以前) 由来で今回範囲の新規ではないが、stage-audio /
  リワークで detached 動画の使用頻度が上がり露出が増えた。凍結ルールに従い
  修正パッチは提案せず報告のみ。
- BA マッピング: BA-7 (遷移・サブモードが flag/順序依存で表現され、分岐順で挙動が決まる)
- 確度: 中 (コード順序から確実だが、detached でタイル/ノーマライズを使う実機確認は未実施)

### [P3] `enter_video_audio_mode` 呼び出しサイトの stale コメント (detached 拒否と記載)

- 場所: src/app/native_video.rs:6473 「enter_video_audio_mode 内で音声トラック無し /
  **detached** / switch 中などは弾かれる」
- 内容: 実装は detached を弾かない (P2 第 1 項参照)。7d 時点の記述が 7e の解禁後も
  残っており、次にこのコードを触るセッションが「detached は入れない前提」で
  ガードを組む事故の種になる。brief の既知の残課題 #1 も同じ前提のまま。
- 確度: 高

### [P3] 「?」ショートカット一覧が音楽ビューで動画専用行 (タイル / コマ送り / キャプチャ等) をそのまま表示する

- 場所: src/ui_dialogs/context_shortcuts.rs:344 (`current_shortcut_help_context` —
  `GridItem::Video|Audio → FsVideo` の item 種別直読み)、:387 (`video_help_includes_row` は
  Compare 系しか除外しない)
- シナリオ: 音声ファイルの音楽ビューで ? → 「動画フルスクリーン」コンテキストとして
  VideoTileMode (S)・VideoFrameStep・VideoCapture (Ctrl+S) など音楽ビューでは
  無効・無意味な行が並ぶ。`fs_music_view_active` を見ていないので
  動画→音声モードでも同様。表示のみの問題。
- 確度: 高 (表示内容)、影響は軽微

---

## 低確度の「怪しい組合せ」 (確度明記)

### [P3・確度低] トレイ格納が ParkedLive / passive detached 窓を一切考慮しない

- 場所: src/tray_integration.rs:118/221/229/243 —
  detached の生存判定がすべて `viewer_session_is_detached_or_switching()`
  (= **active セッションのみ**) で、`parked_live_media_window_exists()` /
  `detached_image_windows` (passive) を見る箇所が無い。
- シナリオ: ON モードで動画/音声を再生 → PDF を開いてメディア窓が ParkedLive に
  なった状態で、メイン窓を × でトレイ格納 → main は隠れるが parked メディア窓は
  画面に残って再生継続。`keep_detached_viewer_alive=false` 側の分岐に落ちるので
  heartbeat は「suspended」と記録され (watchdog 用途なので実害はない)、
  `release_gpu_resources` の思想 (「トレイ中は重いリソースを解放」) とも食い違う。
  active detached は明示的に keep-alive するのに parked だけ「たまたま生き残る」
  非対称で、意図した仕様かコードからは判別できない。
- BA マッピング: BA-7 に近い (「窓が生きているべきか」の述語が active セッション
  にしか存在せず、parked/passive が状態機械の外)
- 確度: 低 (実害の有無はトレイ + ON モードの実機確認が必要)

### [P3・確度低] `viewer_session_blocks_main_window` の否定形が「切替中」を含まない

- 場所: src/app.rs:24804 (`fullscreen_idx.is_some() && !viewer_session_is_detached()`)
- シナリオ: detached 動画 → F12 で main へ戻す placement switch 進行中
  (presenter 無応答なら deadline 最大 5 秒)、presentation はまだ DetachedWindow なので
  blocks=false になり、メイン窓のグリッドがキー入力 (矢印・Ctrl+A 等) を受ける。
  ユーザーが切替の完了を待たず連打したキーが「旧モードの分岐 (グリッド)」に入る窓。
  F12 再押下は `toggle_detached_viewer_mode` の in-flight ガードで守られているが、
  他のグリッドキーは素通り。実害はグリッド選択が動く程度。
- 確度: 低 (通常の切替は 1〜数フレームで完了し窓が極小)

### [P3・確度低] `poll_video` の `is_music_mode` が `video_audio_mode == Some(idx)` 直読みで staleness ガード (fullscreen_idx 一致 + item Video 確認) を省略

- 場所: src/app.rs:45499 / :45610 / :45630 / :45736
- 内容: `fs_music_view_active` は Codex 7c レビューで「stale index で別アイテムを
  音楽ビュー扱いしない」ための `fullscreen_idx == Some(fs_idx)` + item 種別確認を
  持つが、poll_video 内の 4 箇所は `video_audio_mode == Some(*idx)` +
  VST 除外だけで判定している。`video_audio_mode` は close/open/park/EOF-swap で
  クリア・付替えされるため現状の到達経路では破綻を見つけられなかったが、
  fs_cache に複数動画が残る将来変更で「別 idx の動画がループ/resume/ EOF 判定だけ
  music 扱いになる」座りの悪さがある。既知の設計判断 (snapshot 化のための直読み)
  なら OK。
- 確度: 低 (現状の実害経路は未発見)

---

## 確認して問題なしと判断した点 (再調査不要の記録)

- **F12 choke point**: 音声モード中の F12 は `toggle_detached_viewer_mode` 冒頭
  (app.rs:41753) の一括ガードで egui fs / native VK / main grid / ring
  (`RingActionId::ToggleDetachedViewer`) の全入口が止まる。VST サブモード
  (fs_music_view_active=false) も `video_audio_mode.is_some()` 判定なので取りこぼさない。
- **F11 choke point**: `toggle_video_window_mode_for_input` (native_video.rs:2305) が
  VST 中 → exit+egui toggle、音声モード中 → egui toggle、detached → borderless toggle を
  一元処理。native VK / HUD / ring / gamepad が同関数に合流する。
- **ParkedLive の入力遮断**: `native_video_output_event_allowed_while_parked_live` 系
  フィルタで parked native HUD のボタン (♪ 含む) はすべて activation 要求化され
  実行されない。left click は down/up ペア一致でのみ activation。
- **park の状態移送**: `video_audio_mode` / `video_audio_vst` / `video_audio_mode_entry_target` /
  `video_audio_exit_pending` / `pending_detached_video_host_switch` は
  ViewerContextBundle の swap_field 対象で、parked 窓の音声モードは bundle が所有
  (fix3/fix7 系の設計どおり)。`native_video_source_swap_pending` は
  `parked_live_window_id` タグ + owner-context poll (fix7-3) で分離。
- **close 時の後始末**: `close_fullscreen` (app.rs:35687-35694) が
  `video_audio_mode` / `video_audio_vst` / entry_target / exit_pending を一括クリア。
  session close は `handle_fullscreen_close_request` またはkeep_alive_cleanup
  (ui_fullscreen.rs:6082) が拾う。リング flick / 右ドラッグ状態も close_fullscreen +
  `reset_detached_pause_foreground_modes` (park) で cancel され、fix9 の
  「他 surface の flick を消さない」変更に対する掃除境界は揃っている。
- **enter/exit の in-flight 対称ガード**: enter は mode switch / source swap /
  host-switch pending を拒否、exit は swap 進行中拒否 + exit_pending 二重起動拒否、
  `enter_video_audio_vst` は exit_pending 拒否 + main fullscreen 限定 (fix12) +
  presenter 不在拒否。
- **VST の fix12 制限**: `vst3_playback_ui_context_is_main_fullscreen` が
  music chrome (`music_chrome_should_show_vst`) / native HUD
  (`native_video_vst3_controls_available` + poll_video の `set_native_vst3_available`) /
  出力イベント (`Ev::ToggleVst3Gui` の toast 拒否) の 3 経路で一貫。
- **リング context**: `current_ring_shortcut_context` / `current_right_drag_context` とも
  `fullscreen_uses_video_ring_context` (= 動画 or `fs_music_view_active`) を経由し、
  音楽ビューは VideoFullscreen 文脈で統一 (9a952d26 の設計どおり)。
- **EOF 振り分け**: poll_video の ContinuousEofKind (AudioFile / VideoAudioMode / Video)
  は VST 中を Video 側へ倒す判定を含めて一貫。

## 統一述語 vs 生読みのインベントリ要約 (質問 1)

- `fs_music_view_active` 迂回の生 `video_audio_mode == Some(idx)` 読み: 定義コメントの
  「3 概念分離」(表示ゲート / 解析ソース / ファイル種別) に沿った意図的なものが大半
  (`fs_music_source_for_idx`, keep-nav 分岐, poll_video snapshot)。逸脱として上記
  P2 第 2 項 (`fs_zoom_mode_context_ok` が概念分離のどれにも属さず Video 直読み) と
  P3 (`current_shortcut_help_context` の item 直読み) を検出。
- `viewer_session_is_detached()` (非 switching) 直読み 40 箇所超はほぼ描画/内部遷移用で、
  入力系の逸脱は P3 (native Enter/Esc 分岐) と `viewer_session_blocks_main_window`
  (低確度) のみ。
- `matches!(viewer_presentation, DetachedWindow)` の生読み 7 箇所は
  fullscreen_viewport_id / sync_detached_video_child_presenter_rect 等の
  detached 機構内部で、統一述語との食い違いは検出せず。
