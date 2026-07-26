結論: `normalize_ui_states` / `normalize_auto_scan_suppressed` / `last_loop_pos` / `normalize_state` の修正自体は意図どおりです。ただし、この近傍にはまだ shift すべき idx-keyed state が残っています。

**指摘**
- [P2] [src/app.rs:19119](C:/home/mimageviewer/src/app.rs:19119)  
  `music_vst_shell: Option<MusicVstShell>` の `fs_idx` が shift 対象に入っていません。音声ファイルの VST shell 中に前方アイテムを削除すると、`fullscreen_idx` と `fs_cache` は新 idx に移る一方で `music_vst_shell.fs_idx` だけ旧 idx のままです。`tick_music_vst_shell` はその idx で `fs_cache` を見ますし、`exit_music_vst_shell` もその idx から native output を外します（[native_video.rs:6744](C:/home/mimageviewer/src/app/native_video.rs:6744), [native_video.rs:6816](C:/home/mimageviewer/src/app/native_video.rs:6816)）。  
  対象が残存するなら shift、対象自体が削除されたなら `fs_cache` を take する前に `exit_music_vst_shell()` 相当の teardown が必要です。

- [P3] [src/app.rs:19119](C:/home/mimageviewer/src/app.rs:19119)  
  `vst3_deferred_media_open: Option<usize>` も shift されていません。VST3 startup load 中に音声/動画 open が `vst3_deferred_media_open = Some(idx)` で保留され、その間に前方削除が入ると、`fullscreen_idx` は shift 済みでも deferred idx が旧値のまま残ります。再開側は `fullscreen_idx == Some(idx)` を要求するため、保留 open が失われます（[app.rs:13157](C:/home/mimageviewer/src/app.rs:13157), [app.rs:29816](C:/home/mimageviewer/src/app.rs:29816), [app.rs:29850](C:/home/mimageviewer/src/app.rs:29850)）。残存なら shift、削除対象なら clear が妥当です。

- [P3] [src/app/tests.rs:18622](C:/home/mimageviewer/src/app/tests.rs:18622)  
  拡張テストは `normalize_ui_states` / `normalize_auto_scan_suppressed` / `last_loop_pos` の残存 shift は見ていますが、`normalize_state.fs_idx` の追随と対象削除時 cancel は未検証です。今回のバグの核心に近いので、dummy `NormalizeScanState` で「前方削除で fs_idx 更新」「対象削除で state None + cancel true」まで見ると再発防止として強くなります。

**OK**
- 追加された normalize / loop state の shift 実装は OK。`filter_map(shift)` で削除対象を落とし、残存対象だけ新 idx に移しています。
- `normalize_state` の実装方針は OK。残存時は `fs_idx` を追随、対象削除時は cancel して state を畳む流れになっています。
- `invalidate_idx_state_and_queues` との順序も OK。温存対象を先に shift /退避し、その後で汎用 cache と queue を clear しています。
- 今回の狭いテスト `cargo test -q remove_items_batch_shifts_fullscreen_and_audio_mode_state` は 1 件 pass しました。