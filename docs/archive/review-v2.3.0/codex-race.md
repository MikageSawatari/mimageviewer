[P1] `NativeVideoSourceSwapPending` が別 ParkedLive 窓の更新で所有者だけ差し替わる  
場所: `src/app/native_video.rs:771`, `src/app/native_video.rs:803`  
時系列: UI thread で `poll_parked_live_detached_windows` が窓Aを swap-in → 窓Aの動画 EOF/ナビで source-swap pending を作成し、`native_output` は窓A presenter、`parked_live_window_id=Some(A)` になる → pending 完了前に窓Bを swap-in → 窓Bでも動画 EOF/ナビが走る → `native_video_source_swap_pending.is_some()` 分岐が既存 pending の `target_idx/target_path` と owner を `Some(B)` に更新する。ただし保持中の `native_output` は窓Aのまま。以後 owner gate は窓Bとして通り、窓A presenter を窓B player に attach/commit し得る。  
根拠: pending は App グローバル 1 本で、更新時に既存 owner との一致チェックがない。`source_swap_owner_after_update(Some(8), Some(7)) == Some(8)` もテストで固定されている。  
BA mapping: BA-4 / BA-7。window_id identity を completion 側だけで見ており、enqueue/update 側の所有境界が reducer 化されていない。  
確度: 高。

[P2] 音声モードから動画へ戻る pending 中に EOF 継続が exit を追い越す  
場所: `src/app.rs:45101`, `src/app.rs:45632`, `src/app/native_video.rs:7088`  
時系列: ユーザーが音楽ビューの「動画へ戻る」を押す → `exit_video_audio_mode` は presenter show を投げるが、再表示確認まで `video_audio_mode=Some(fs_idx)` と `video_audio_exit_pending=Some(...)` を維持する → 次フレームの `poll_video` で、exit pending の poll より前に EOF 判定が走る → `video_audio_mode == Some(idx)` なので `ContinuousEofKind::VideoAudioMode` に分類される → `handle_video_audio_mode_continuous_eof` は `video_audio_exit_pending` を見ず、keep-audio source-swap を開始して `video_audio_mode=Some(next_idx)` に進める → その後 `poll_video_audio_exit_pending` は `video_audio_mode != Some(old_fs_idx)` で pending を捨て、ユーザーの「動画へ戻る」が失われる。  
根拠: EOF handler の多重起動 guard は `native_video_mode_switch/source_swap/fast_swap/tile` のみで、exit pending が含まれていない。  
確度: 高。

[P3] 音楽解析 worker が同一 path の外部更新を decode 後に再検証しない  
場所: `src/app.rs:5270`, `src/app.rs:5361`, `src/app.rs:34244`  
時系列: UI が `path` の解析 worker を起動 → worker が decode 前に `fresh_meta` を stat → その後、同一 path のファイルが外部プロセスで置換/上書きされる → worker は decode/解析結果を `TimelineComplete { meta: fresh_meta }` として送る → UI は path 一致だけで採用し、LRU も pre-decode の meta で登録する。  
根拠: 適用側の照合は `music_analysis_path == pending.path` のみで、decode 後の size/mtime 再検証や generation はない。通常の「別 path へ切替」は pending receiver を捨てるため保護されているが、同一 path 更新 TOCTOU は残る。  
確度: 中。

EngineActor / audio callback / SQLite 共有 DB については、この範囲で同等確度の追加指摘は見つけていません。