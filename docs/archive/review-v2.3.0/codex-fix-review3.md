結論: まだクリーンではありません。残り 1 件、非 Windows ビルドを壊す cfg 漏れがあります。

**指摘**
- [P1] [src/app.rs:19190](C:/home/mimageviewer/src/app.rs:19190)  
  `vst3_deferred_media_open` は field 定義が `#[cfg(windows)]` ですが（[src/app.rs:8247](C:/home/mimageviewer/src/app.rs:8247)）、`remove_items_batch` では cfg なしで参照されています。非 Windows では field が存在しないためコンパイルエラーになります。  
  ここは `#[cfg(windows)] { self.vst3_deferred_media_open = self.vst3_deferred_media_open.and_then(shift); }` にする必要があります。

**確認結果**
- `music_vst_shell` の shift / 対象削除時 teardown は OK。`fs_cache` take 前に `exit_music_vst_shell()` へ落としており、順序も妥当です。
- `normalize_state` の追随 / cancel とテスト追加は OK。
- `vst3_deferred_media_open` の shift 方針自体は OK。cfg だけ修正してください。
- 追加で shift 必須と判断する `remove_items_batch` 近傍の長命 idx-keyed state は見当たりません。

検証: `cargo test -q remove_items_batch_shifts_fullscreen_and_audio_mode_state` は pass しました。