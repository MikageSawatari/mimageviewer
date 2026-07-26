対象範囲を `docs/archive/review-v2.3.0/brief.md` の形式で静的レビューしました。現状の working tree を正として見ています。

[P1] 不正または巨大な音声 duration でタイムラインが巨大 row 配列を確保し、OOM/フリーズする
- 場所: `src/audio_decode.rs:484`, `src/ui_fullscreen.rs:21627`, `src/ui_music_timeline.rs:182`, `src/ui_music_timeline.rs:694`
- シナリオ: 壊れた音声ファイルで FFmpeg duration が `i64::MAX` 相当になり、`probe.duration_secs = 9_223_372_036_854.775` 秒になる。再生側 duration が 0 の状態で音楽ビューを開くと、`timeline_row_count(duration, 1.0)` が兆単位の rows を返し、`TimelineTextureCache::ensure` が `Vec::resize_with(key.rows, ...)` を実行してアプリが固まる/落ちる。
  ユニット再現入力: `timeline_row_count(9_223_372_036_854.0, 1.0)` または `timeline_row_count(f64::MAX, 1.0)` が実用上の上限を超えないことを期待するテスト。
- 根拠: `probe_audio_file` は正の duration を上限なしで秒に変換し、`draw_fs_music_view` も finite/上限チェックなしで timeline duration に採用する。`timeline_row_count` は finite かつ正ならそのまま `ceil() as usize` し、cache 側がその rows 数で複数の Vec を resize する。
- 確度: 高

[P2] audio decode の EOF で swresample を flush しないため、末尾 PCM が解析から欠ける
- 場所: `src/audio_decode.rs:265`, `src/audio_decode.rs:340`, `src/audio_decode.rs:533`
- シナリオ: 44.1kHz stereo WAV、長さ 0.100 秒、最後の 441 samples だけ振幅 1.0 のインパルスを置く。`decode_audio_file_to_stereo_f32` で 48kHz に変換した結果、期待は約 4800 frames かつ末尾 600 frames 内にインパルスが残ること。現状は decoder EOF 後に resampler 内部 delay を flush しないため、末尾の transient が timeline/spectrum から消える可能性がある。ユーザー症状は、再生では聞こえる末尾音が波形・スペクトラム・ビート解析に出ない、特に極短音声で解析が短く見えること。
- 根拠: EOF では `decoder.send_eof(); drain_decoder(...)` までで、`ResampleContext` に空入力を流す final flush がない。コメントにも `Flush at EOF is intentionally omitted` とあり、resampler delay を破棄している。
- 確度: 中

[P2] 非有限 PCM が analysis 結果へ伝播し、NaN 表示/描画欠落を起こす
- 場所: `crates/music-core/src/analysis.rs:262`, `crates/music-core/src/analysis.rs:287`, `crates/music-core/src/analysis.rs:292`
- シナリオ: `analyze_stereo_timeline(&[NaN, 0.0, INFINITY, -INFINITY, 0.25, -0.25], 48000, default_config)` を呼ぶ。期待は decode 異常入力でも bin の `rms_l/rms_r/peak_l/peak_r/loudness_db/band_energy/transient` がすべて finite になること。現状は `l`/`r` を `is_finite` で正規化せず演算に入れるため、NaN が bin metrics に入り、hover の `NaN%` 表示や波形/色の欠落につながる。
- 根拠: sample 読み出し後に `clamp(-1.0, 1.0)` しているが、NaN は以降の `sum_l += l * l`、`peak_l.max(l.abs())`、band/chroma 系に伝播しうる。
- 確度: 高

[P2] `BeatGrid::from_bpm` が BPM 上限なしで beat を生成し、入力次第で長時間ループ/巨大 allocation になる
- 場所: `crates/music-core/src/beat.rs:49`
- シナリオ: `BeatGrid::from_bpm(60.0, 1_000_000.0, 0.0, 1.0)` で約 100 万 beat を 1 秒分に生成する。さらに `BeatGrid::from_bpm(1.0, f32::MAX, 0.0, 1.0)` のような入力では `beat_period` が極小になり、進捗しない/実用上終わらないループになり得る。手動 BPM、外部プリセット、キャッシュ復元などで任意 BPM が入る経路があると UI 操作で固まる。
- 根拠: finite かつ正の BPM だけを受け入れ、`while t <= duration_secs + beat_period * 0.5` で beat を push し続ける。BPM の現実的上限、最大 beat 数、`t` が進んでいることの検査がない。
- 確度: 中

問題なしとして見た観点:
- 空 PCM / sample_rate 0 は `analysis` と `SpectrumAnalyzer` 側で早期 return され、明確な除算ゼロは見つかりませんでした。
- `timeline_bin_at_time` / `timeline_bins_window_range` の `partition_point` は、通常の `analyze_stereo_timeline` 生成 bin では単調な start/end 前提を満たしており、境界時刻の off-by-one は見つかりませんでした。
- `MusicPcm` の progressive append/copy は `try_reserve` 失敗を `Err` として返し、未到達 frontier の window も `None` にしているため、通常の部分 decode 中表示では破綻しにくいです。
- decode worker の channel disconnect/cancel は通常経路では後始末されており、切断だけで UI thread が待ち続ける経路は見つかりませんでした。

テスト実行はしていません。今回は read-only 環境で、`cargo test` は `target` への書き込みが必要になるため静的レビューに限定しました。