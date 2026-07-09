# v2.3.0 出荷前レビュー: レースコンディション・並行性バグ (claude-race)

作成: 2026-07-09 / レビュアー: ClaudeCode (Fable)
対象: コミット範囲 `7eff5a9e` (= v2.2.0) .. `01910684` (= HEAD)
観点: 複数ウィンドウ競合 / 動画エンジン / 動画→音声モード hidden presenter / cpal 音声 /
音楽解析ワーカー / TOCTOU (brief.md 記載の 6 観点)

本レポートの指摘は HEAD の現行コードを直接 Read して file:line を確認済み。
シナリオはスレッド/イベントの時系列で記述する。

---

## 指摘一覧 (深刻度順)

### [P2] 音声モード exit 進行中に連続再生 EOF が発火すると exit 操作が消失し、旧動画の stale フレームが全画面に露出する

- 場所:
  - src/app.rs:45103-45108 (`handle_video_audio_mode_continuous_eof` の多重起動ガード — `video_audio_exit_pending` が入っていない)
  - src/app/native_video.rs:7043-7057 (`exit_video_audio_mode` の fast-show 経路 — `set_native_window_visible(true)` 送信 + `video_audio_exit_pending` セットのみ)
  - src/app/native_video.rs:7088-7096 (`poll_video_audio_exit_pending` — mode 不一致で pending を黙って破棄)
  - src/app/native_video.rs:869-875 (`defer_native_video_source_swap_until_decoder_free` — keep_audio_mode 時に `video_audio_mode = Some(target_idx)` へ進める)
  - src/app.rs:45707-45725 / 45754 / 45757 (poll_video 内の処理順: EOF dispatch → swap poll → exit poll)
- シナリオ (時系列):
  - T1 (フレーム N, draw): 連続再生 ON で音声モード再生中、曲の末尾間際にユーザーが音楽ビューの「動画に戻る」ボタンを押す。`exit_video_audio_mode` の fast-show 経路が presenter へ show コマンドを送り、`video_audio_exit_pending` をセット。`video_audio_mode` は presenter の再表示 confirm まで Some(old) のまま (設計どおり)。
  - T2 (フレーム N+1, poll_video): エンジンが EOF に到達。フレーム冒頭 snapshot は `video_audio_mode=Some(old)` なので EOF は `ContinuousEofKind::VideoAudioMode` に分類され、`handle_video_audio_mode_continuous_eof` が dispatch される。同関数のガード (app.rs:45103-45108) は `native_video_mode_switch` / `native_video_source_swap_pending` / `native_video_fast_swap_pending` / `video_tile_swap_pending` しか見ず、**`video_audio_exit_pending` を見ない**ため通過し、`source_swap_keep_audio_mode=true` で source-swap を開始する。
  - T3: `defer_native_video_source_swap_until_decoder_free` が旧 player から `take_native_output` し `fs_cache.remove(&from_idx)`、`video_audio_mode = Some(target_idx)` へ進める (native_video.rs:869-875)。
  - T4 (同フレームの app.rs:45757): `poll_video_audio_exit_pending` が `video_audio_mode (=Some(target)) != Some(pending.fs_idx (=old))` を検出して pending を黙って破棄 (native_video.rs:7093-7095)。**ユーザーの exit 操作はここで消失する**。
  - T5: 一方 T1 で presenter スレッドに送られた show コマンドは処理され、presenter ウィンドウが**可視化**される。swap は「presenter は hidden のまま SwitchSource は可視性を変えない」前提 (native_video.rs:758-760, 1293-1297) なので、swap 完了 (`enter_video_audio_mode` 内の `set_native_window_visible(false)`、native_video.rs:6967-6969) までの間 (decoder-free 待ち込みで数百 ms〜、deadline 上限 10 秒)、**旧動画の hold フレームが全画面 placement で音楽ビューの上に露出**する。この間 pending 側に output が退避されており UI から hide し直す経路が無い。
  - T6: swap 完了 → `open_fullscreen(target)` が mode を一旦 None → `enter_video_audio_mode(target)` で音声モード再確立 + re-hide。結果、ユーザー視点では「動画に戻るを押したのに、旧動画のフレームが一瞬 (〜数百 ms) フラッシュした後、次の曲が音声モードのまま始まる」= exit が無視される。
- 根拠: 対称の防御が他所には存在する。`exit_video_audio_mode` は swap 進行中の exit を明示拒否 (native_video.rs:7006-7018) し、`enter_video_audio_vst` は `video_audio_exit_pending.is_some()` を明示拒否する (native_video.rs:7254-7257)。EOF ハンドラだけこの状態を考慮していない。poll_video 内の順序 (EOF dispatch が app.rs:45707、exit poll が 45757) により、同一フレーム内で exit poll が先に完了して防げる可能性も無い。
- 発生頻度: exit クリックから presenter show confirm まで (通常 1〜3 フレーム、deadline 400ms) の窓に EOF が重なる必要がある。「曲の終わり際に動画表示へ戻ろうとする」操作は自然なので、低頻度だが実使用で起こりうる。
- 確度: 高 (ガード欠落と pending 破棄経路はコードで確認。stale フレーム露出の持続時間は presenter スレッドの show 処理タイミング依存で、ここだけ推定を含む)

### [P2] detached / マルチウィンドウ中のグリッド削除完了が、共有 `fs_cache.clear()` で再生中 player を破棄し、`fullscreen_idx` / `video_audio_mode` の idx シフト漏れで stale idx 参照になる

- 場所:
  - src/app.rs:18900-19065 (`remove_items_batch` — `selected` / `search_filter` / 補正マップ等は shift するが、**`fullscreen_idx` と `video_audio_mode` / `video_audio_vst` / `video_audio_mode_entry_target` は shift もクリアもしない**)
  - src/app.rs:18658 (`invalidate_idx_state_and_queues` 内の `self.fs_cache.clear()` — 再生中 `VideoPlayer` ごと drop)
  - src/app.rs:19095-19192 (`poll_delete_pending` — 削除 worker 完了時に `remove_items_batch` を呼ぶ。フルスクリーン/音声モードのガード無し)
  - src/app.rs:24804-24805 (`viewer_session_blocks_main_window` — detached 中はメイン窓グリッドが完全に操作可能 = 削除コンテキストメニュー到達可能。src/ui_dialogs/context_menu.rs:1712)
- シナリオ (時系列):
  - T1: detached viewer (F12) で動画または音声モードの曲を再生中。detached-rework の設計どおりメイン窓グリッドは操作可能。
  - T2: ユーザーがメイン窓グリッドで別ファイル数件を右クリック → 削除。`start_delete_files` が削除 worker を spawn。
  - T3 (数百 ms 後、UI スレッド): worker 完了 → `poll_delete_pending` → `remove_items_batch` → `invalidate_idx_state_and_queues` → **`fs_cache.clear()`**。active コンテキストの `fs_cache` は detached viewer と共有なので、detached 窓で再生中の `VideoPlayer` (presenter/音声スレッド込み) がその場で drop され、**再生が突然停止**する (削除対象と無関係なファイルの再生でも)。
  - T4: `remove_items_batch` は `fullscreen_idx` / `video_audio_mode` を shift しないため、削除 idx が視聴中 idx より前だった場合、両者は「ずれた idx」を指したまま残る (両者一貫してずれるので mode 述語の相互整合は保たれるが、指す item が変わる)。フルスクリーン再ロード経路が items[stale idx] = **別のアイテム**を detached 窓に表示する。音声モードでは `video_audio_mode=Some(stale)` が残り、shift 先が別の動画なら音楽ビューがその別動画に付いたまま再確立されうる (`fs_music_view_active` の防御 ui_fullscreen.rs:21347-21356 は「item が Video か」しか確認できず、**別の Video へのずれは検出不能**)。
  - ユーザー視点の症状: グリッドで無関係なファイルを消しただけで、(a) detached 窓の再生が停止・presenter が消える、(b) 復帰後に隣のアイテムへ視聴位置が「ズレる」、(c) ズレた idx に対するレーティング等の idx ベース操作が**意図しないファイルに適用**されうる。
- 根拠: `remove_items_batch` の shift 対象一覧 (app.rs:18950-19040) と `invalidate_idx_state_and_queues` のクリア対象一覧に上記フィールドが無いことを確認。app.rs:18735-18737 自身が「新しい idx-keyed 状態を足したらここと remove_items_batch の shift 群の両方に追加すること」と規定しており、v2.3.0 で新設された `video_audio_mode` 系フィールドはこの規約から漏れている。
- 補足 (範囲の切り分け): 「削除で `fs_cache` が全クリアされ `fullscreen_idx` が shift されない」こと自体は v2.2.0 以前からの挙動の可能性が高い。ただし (1) 本範囲で detached が live-park 化され「グリッド操作しながら別窓で視聴」が一級フローになったこと、(2) `video_audio_mode` / `video_audio_vst` という新規 idx-keyed 状態が shift 規約から漏れたこと、の 2 点が新規/悪化分。
- BA マッピング: BA-1〜BA-7 のどれにも直接対応しない (detached の HWND/placement 前提ではなく items 世代管理の問題)。リワーク凍結対象外の共有ロジック側として報告。
- 確度: 中 (コード経路は全て確認済み。削除確認ダイアログ〜worker 完了の間に UI 側で別のガードが働く可能性を網羅的には排除できていない点、および v2.2.0 時点の再現性を実測していない点で「中」)

### [P3] poll_video の EOF 種別分類がフレーム冒頭 snapshot 依存で、同フレーム内の ToggleAudioMode 処理と食い違う (♪ ボタンが EOF フレームで無視される)

- 場所:
  - src/app.rs:45445 (`video_audio_mode` のフレーム冒頭 snapshot)
  - src/app.rs:45620-45640 (fs_cache ループ内での `ContinuousEofKind` 分類 — snapshot を使用)
  - src/app.rs:45664-45706 (native events 処理 — この中で `enter_video_audio_mode` が走り mode が変わりうる。ToggleAudioMode ハンドラは src/app/native_video.rs:2917-2923)
  - src/app.rs:45707-45725 (分類済み `continuous_eof_events` の dispatch — **native events 処理後**に stale な kind で実行)
- シナリオ (時系列):
  - T1 (poll_video 冒頭): 連続再生 ON の通常動画再生中。snapshot `video_audio_mode=None`。fs_cache ループでエンジン EOF を検出し、`ContinuousEofKind::Video` として積む。同ループで presenter からの native events も drain されており、その中に直前のユーザー操作 = HUD「音声モード」ボタン (`ToggleAudioMode`) が入っている。
  - T2 (native events 処理): `enter_video_audio_mode` が成立 (この時点で swap pending はまだ無いのでガード通過)。presenter が hide され `video_audio_mode=Some(idx)` になる。batch 打ち切りガード (app.rs:45703-45705) は**残り native events** しか打ち切らない。
  - T3 (EOF dispatch): 積んであった kind=Video のまま `handle_video_continuous_eof` が実行され、次動画を**可視動画として**開く経路 (`open_native_video_fullscreen_from_navigation_with_options`) に入る。fast-swap の defer 経路は `keep_audio_mode=false` なので `video_audio_mode` は Some(old) のまま `fullscreen_idx` だけ target へ進み、swap 完了時の `open_fullscreen` が mode を None に落とす。
  - ユーザー視点の症状: 動画終了の瞬間に ♪ ボタンを押すと、押下が無視されて次の動画が通常の映像表示で開く。T2〜swap 完了の間は「mode=Some(old) かつ presenter hidden かつ fullscreen_idx=target」の不整合ウィンドウがあり、1〜数フレームの黒/未描画が出うる。
- 根拠: 分類 (45630) と dispatch (45707) の間に native events 処理 (45683) が挟まる構造。実機バグ 2026-07-04 (app.rs:45477-45479 コメント) の修正は snapshot 分類までで、「同フレーム内で mode が遷移した場合の再分類/dispatch 側ガード」は無い。逆方向 (VideoAudioMode 分類後に mode が消える) は `handle_video_audio_mode_continuous_eof` 冒頭の再チェック (app.rs:45098) で防御済みだが、Video 分類 → mode 成立方向の再チェックは `handle_video_continuous_eof` に無い (app.rs:45008-45026)。
- 発生頻度: EOF 検出と ♪ ボタン押下 event が同一 poll_video に同居する 1 フレーム窓。極めて稀。壊れ方も「操作が 1 回無視される」に留まる。
- 確度: 中 (経路はコードで確認。`handle_video_continuous_eof` → fast-swap 経路が presenter hidden 状態でどう見えるかは推定を含む)

---

## 問題なしと確認できた設計ポイント (検収の裏取り用)

自分で該当コードを読み、防御が入っていることを確認したもの:

1. **native presenter イベントの stale 照合**: `handle_native_video_output_event` は
   fullscreen_idx 照合 (native_video.rs:2732) + source_epoch 照合 (2840-2871、NavigateItem のみ
   意図的 bypass) の二段。source swap 中は退避中 native_output の committed 世代で close を
   gate する (`drain_native_video_source_swap_pending_events`、native_video.rs:913-932)。
2. **poll_video の native events batch 打ち切り**: close 成立 / music_vst_shell 離脱 /
   video_audio_vst 離脱 / audio mode 突入の 4 契機で残イベントを破棄し、破棄済み presenter
   由来イベントの誤適用を防ぐ (app.rs:45679-45705)。
3. **音声モード enter/exit の swap 相互ブロック**: enter は `native_video_mode_switch` /
   `native_video_source_swap_pending` / `detached_video_host_switch_pending` 中を拒否
   (native_video.rs:6885-6890)、exit は swap 中を拒否 (7013-7018)。F12 トグルは
   `video_audio_mode.is_some()` で一括拒否 (tests.rs:17703 で固定)。
4. **exit の saw_hidden シード**: show が 1 poll より先に処理されて hidden→false 遷移を
   UI が観測し損ねる race を、exit 時点の hidden 値シードで吸収 (native_video.rs:7019-7028,
   7112-7121)。deadline 超過は detach+attach+seek フォールバックで復帰保証 (7132-7139)。
5. **`fs_music_view_active` の stale-idx 防御**: `video_audio_mode == Some(fs_idx)` 単独ではなく
   `fullscreen_idx` 一致 + item が `GridItem::Video` であることを要求 (ui_fullscreen.rs:21347-21356)。
   ただし上記 P2-2 のとおり「別の Video へずれた」ケースは原理的に検出できない。
6. **音声モード EOF swap の intent 維持**: swap pending 中の追加ナビでも
   `audio_mode_after_swap` が false へ潰れない (native_video.rs:771-807)。keep_audio_mode の
   one-shot (`source_swap_keep_audio_mode`) は同期呼び出し内で消費されリークしない
   (app.rs:45138-45140 → native_video.rs:761 を同一コールスタックで確認)。
7. **music_bookmarks の path 照合**: 曲切替直後に前曲のブックマークで区間ループを組まないよう
   `music_bookmarks_loaded_for` (path キー) で gate (app.rs:45155-45169, 45506-45512)。
8. **parked bundle の状態隔離**: live-park の bundle swap は `items` / `fs_cache` /
   `fullscreen_idx` / `video_audio_mode` 系を丸ごと swap し (app.rs:10560-10745)、parked 窓の
   再生状態がメイン文脈の folder 遷移と混ざらない構造になっている (削除経路は上記 P2-2 のとおり例外)。

---

## 未回収の観点

並行調査を委任した以下の観点は、期限内に結果を回収できなかった。本レポートには
含まれていない (別途フォローが必要):

- 未回収: 音楽解析ワーカー (crates/music-core / MusicPcm progressive streaming / 解析結果 LRU) の結果適用とファイル切替の競合 (brief 観点 5)
- 未回収: detached 複数窓の共有 SQLite / キャンセルトークン・世代カウンタの窓間取り違え / worker 結果の window_id 照合 (brief 観点 1 の大部分。ただし本レポート P2-2 が観点 1 の一部をカバー)
- 未回収: 動画エンジン内部 (EngineActor のイベント順序 / seek 中 EOF / audio_bookkeeping の atomic 合成) と cpal コールバック共有状態 (brief 観点 2・4)
