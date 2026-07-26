検収結果: 指摘は 1 件です。

**指摘**
- [P2] [src/app.rs](C:/home/mimageviewer/src/app.rs:19139)  
  `remove_items_batch` の idx shift 対象に `normalize_state` / `normalize_ui_states` / `normalize_auto_scan_suppressed` / `last_loop_pos` が残っています。特に `start_normalize_scan_inner` は再生停止と preroll suspend 後に `fs_idx` を保持し、完了時はその `fs_idx` で `fs_cache` を検証します（[native_video.rs](C:/home/mimageviewer/src/app/native_video.rs:3953), [native_video.rs](C:/home/mimageviewer/src/app/native_video.rs:4045)）。  
  そのため、動画 idx 1 の音量解析中に idx 0 を削除すると、動画本体は idx 0 に温存されますが解析状態は idx 1 のままになり、完了時に stale 扱いで再生再開・preroll suspend 解除・gain 反映が落ちる可能性があります。`last_loop_pos` も同じ idx-keyed 状態なので、残存アイテム規則に合わせて shift または clear が必要です。

**OK**
- P2-1: OK。`video_audio_exit_pending` 中の continuous EOF swap 抑止は意図に合っています。
- P2-3: OK。parked live 音楽窓が global music state を使う場面だけ clear を避け、動画 open 側は従来どおり clear されます。
- P2-4: OK。`music_analysis_version` 経由になっており、直接代入漏れは見当たりません。
- P2-5: OK。音声ファイル音楽ビューの ring/gamepad 窓切替は F11 と同じ egui viewer 経路に揃っています。
- P2-10: OK。`GridItem::Audio` 除外で不可視 zoom latch は閉じています。
- P2-2: 上記 P2 指摘あり。それ以外の `fullscreen_idx` / video audio 系 / EOF 系 shift と `FsCacheEntry::Video` 温存方針は妥当です。
- P2-6: OK。EOF 到達時の `BufferReady` は短尺音声の固着対策として自然で、通常長尺の ready 条件は維持されています。
- P2-7: OK。`SeekCompleted` の lane Full 時 pending 退避と serial 不一致破棄は意図どおりです。
- P2-8/P2-9: OK。thumb channel / token / queues の bundle swap、swap 末尾 clear 撤去、detached mount 中の crate-global 副作用抑止、rx drain は整合しています。main 側 worker/queue を壊す残存経路は差分上は見当たりません。
- cfg(windows) / 非 Windows: OK。今回確認した範囲では非 Windows 側を壊す参照は見当たりません。

テストは read-only 環境のため実行していません。差分と関連コードの静的レビュー結果です。