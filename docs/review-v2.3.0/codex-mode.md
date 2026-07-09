調査範囲 `7eff5a9e..01910684` について、`rg` で統一述語の呼び出しを列挙し、差分で追加された分岐を照合しました。深刻度順です。

[P2] 音声ファイルの音楽ビューでリング/ゲームパッドの「ウィンドウモード切替」が no-op になる
- 場所: [src/app/gamepad_input.rs](C:/home/mimageviewer/src/app/gamepad_input.rs:4633), [src/app/native_video.rs](C:/home/mimageviewer/src/app/native_video.rs:2305), [src/app/native_video.rs](C:/home/mimageviewer/src/app/native_video.rs:2125)
- シナリオ: 音声ファイルをフルスクリーン音楽ビューで開き、リング/ゲームパッドから `ToggleWindowMode` を実行する。
- 根拠: `fullscreen_uses_video_ring_context()` は `fs_music_view_active()` を含めて音楽ビューを `VideoFullscreen` 扱いにしますが、`apply_ring_toggle_window_mode()` は `VideoFullscreen` で常に `toggle_video_window_mode_for_input()` に入ります。この helper は `video_audio_mode` / `video_audio_vst` / detached しか見ず、純音声の `fs_music_view_active()` を見ないため、最終的に native video presentation 切替へ進み、`GridItem::Video` でないので早期 return します。キーボード F11 側は [src/ui_fullscreen.rs](C:/home/mimageviewer/src/ui_fullscreen.rs:10397) で `fs_music_view_active` を見て egui viewer 切替に入るため、入力経路間で挙動が食い違います。
- 確度: 高

[P2] 音楽ビューでリング/ゲームパッドから動画フレーム操作が通る
- 場所: [src/app/gamepad_input.rs](C:/home/mimageviewer/src/app/gamepad_input.rs:4539), [src/app/gamepad_input.rs](C:/home/mimageviewer/src/app/gamepad_input.rs:4766), [src/ui_fullscreen.rs](C:/home/mimageviewer/src/ui_fullscreen.rs:18511), [src/ui_fullscreen.rs](C:/home/mimageviewer/src/ui_fullscreen.rs:19458)
- シナリオ: 音声ファイルの音楽ビュー、または `video_audio_mode` 中に、リング/ゲームパッドから `VideoCapture` や `AddToBook` を実行する。
- 根拠: キーボード経路では音楽ビュー中の動画専用操作が抑止されていますが、リング経路では `VideoFullscreen` context のまま `save_video_frame_to_file()` / `add_current_video_frame_to_active_book()` を直接呼び、`fs_music_view_active()` でガードしていません。`fs_video_player()` は音声用 `VideoPlayer` も返すため、純音声では音声パスに対してフレーム抽出を試み、動画→音声モードでは音楽UI中に隠れた動画フレーム操作が通ります。
- 確度: 高

[P3] F12 host migration 中のリング close/minimize が detached-or-switching を見ず旧モード側へ寄る可能性
- 場所: [src/app/gamepad_input.rs](C:/home/mimageviewer/src/app/gamepad_input.rs:4658), [src/app/gamepad_input.rs](C:/home/mimageviewer/src/app/gamepad_input.rs:4707), [src/app.rs](C:/home/mimageviewer/src/app.rs:24775)
- シナリオ: F12 detached への host migration / native mode switch target が pending の遷移中に、リング/ゲームパッドから close または minimize を実行する。
- 根拠: `viewer_session_is_detached_or_switching()` なら detached 系セッションとして扱うべき遷移窓ですが、`apply_ring_close_fullscreen()` は `viewer_session_is_detached()` の生判定だけで focus 戻しを決め、`ring_minimize_target_hwnd()` も `viewer_presentation == DetachedWindow` / `active_detached_viewer_context` の直読みで target detached switch を見ません。統一述語との差は、`native_video_mode_switch.target_presentation == DetachedWindow` や `detached_video_host_switch_pending()` の間です。この窓では main/root 側へ focus/minimize が向く可能性があります。
- BA マッピング: BA-7（detached 状態の raw bool/Option 再実装による分岐漏れ）
- 確度: 中

[P3] `enter_video_audio_mode` 周辺のコメントが現在の detached 仕様と逆向き
- 場所: [src/app/native_video.rs](C:/home/mimageviewer/src/app/native_video.rs:6472), [src/app/native_video.rs](C:/home/mimageviewer/src/app/native_video.rs:6868)
- シナリオ: native key 経路の `VideoToggleAudioMode` を保守する際、コメントを根拠に detached では音声モードがブロックされると誤認する。
- 根拠: 呼び出し側コメントは「detached / switch 中などは `enter_video_audio_mode()` 内で弾かれる」と読めますが、実装側は `DetachedWindow` も usable とし、switch/pending だけを弾く設計になっています。挙動バグではありませんが、今回の観点では将来のモード分岐修正を誤誘導しやすいです。
- BA マッピング: BA-7（detached 判定の所有境界が曖昧になるリスク）
- 確度: 高

補足確認: F12 detached ウィンドウ中の `video_audio_mode` 進入自体は、現行コードでは明示的に許可されています。一方、`video_audio_mode` 中に F12 で detached 切替する経路は [src/app.rs](C:/home/mimageviewer/src/app.rs:41753) でブロックされ、switch/pending 中の進入も [src/app/native_video.rs](C:/home/mimageviewer/src/app/native_video.rs:6885) でブロックされています。brief の「Inc7e 未対応」を現行リリース仕様として扱うなら、ここは仕様/実装の不一致です。