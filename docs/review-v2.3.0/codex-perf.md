レビュー結果です。`git diff 7eff5a9e..01910684` と HEAD の呼び出し経路を照合しました。

[P3] ビートグリッド描画が可視行ごとに全曲の beat/bar を毎フレーム線形走査する  
- 場所: [src/ui_music_timeline.rs](C:/home/mimageviewer/src/ui_music_timeline.rs:602), [src/ui_music_timeline.rs](C:/home/mimageviewer/src/ui_music_timeline.rs:1876)  
- シナリオ: 1時間超の音声で音楽ビューを再生し、複数行が可視になる。  
- 根拠: `draw_music_timeline` は可視行ごとに `draw_beat_grid` を呼び、`draw_beat_grid` は `analysis.beat_grid.beats` と `bars` 全体を毎回走査して行範囲外を `continue` している。beat/bar 数は曲長比例なので、描画パスが「可視行数 × 全曲 beat/bar 数」になる。`bins` 系は `partition_point` で範囲化されているが、beat/bar は同じ最適化がない。  
- 確度: 高

[P3] 音楽ブックマーク初回ロードが UI スレッド上の同期 SQLite SELECT になっている  
- 場所: [src/ui_fullscreen.rs](C:/home/mimageviewer/src/ui_fullscreen.rs:21396), [src/ui_music_panels.rs](C:/home/mimageviewer/src/ui_music_panels.rs:156), [src/ui_music_panels.rs](C:/home/mimageviewer/src/ui_music_panels.rs:163)  
- シナリオ: 音楽ビューを開く、またはファイル移動直後の最初の描画フレームで `%APPDATA%` 側の bookmark DB が cold / AV スキャン中 / 肥大化している。  
- 根拠: `draw_fs_music_view` から同期的に `ensure_music_bookmarks_loaded` → `reload_music_bookmarks` → `VideoBookmarkDb::list_marker_entries` に到達する。path 変化時のみで毎フレームではないが、§4 の SQLite チェック対象で、初回フレームのヒッチ要因になる。  
- 確度: 高

[P3] 音楽ビュー追加パスの主要同期区間に細粒度 perf 計装がない  
- 場所: [src/ui_fullscreen.rs](C:/home/mimageviewer/src/ui_fullscreen.rs:21372), [src/app.rs](C:/home/mimageviewer/src/app.rs:34237), [src/ui_music_timeline.rs](C:/home/mimageviewer/src/ui_music_timeline.rs:484), [src/ui_music_spectrum.rs](C:/home/mimageviewer/src/ui_music_spectrum.rs:361)  
- シナリオ: 音楽ビューでフレームヒッチが出ても、`frame_total` 以外で `poll_music_analysis`、bookmark DB、timeline texture upload、beat grid、spectrum update のどれが原因か分離できない。  
- 根拠: 変更範囲の音楽 UI 経路に `perf::event` / `emit_ms` がなく、§4 チェックリストの「該当区間に perf::event」を満たしていない。  
- 確度: 高

確認済み・問題なし:
- `src/audio_decode.rs` の FFmpeg open/probe/decode と `std::fs::metadata` は `run_music_analysis` ワーカー内で、UI スレッドから同期到達しない。
- `ctx.load_texture` は音楽 timeline row で 1 frame 1 row に制限されている。spectrum / panels 側に新規の毎フレーム texture upload は見当たらない。
- PCM 全走査、FFT、timeline 解析はワーカー側。spectrum は in-flight 1 件で、worker 側で最新リクエストへ coalesce している。
- cpal callback 共有 Mutex は重い VST/伸縮/limiter 処理を lock 外で実行しており、UI 側から長時間待つ新規構造は確認できない。
- detached parked live はメディア窓 1 本規則と `music_*` global 方針に沿っており、複数窓で同じ解析を重複実行する構造は確認できない。

テストは実行していません。今回はコードレビューのみです。