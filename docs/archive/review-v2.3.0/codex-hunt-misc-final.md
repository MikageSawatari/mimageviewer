[P2] タグファセット通過判定に `Audio` が入っておらず、音声がタグ絞り込みをすり抜ける
- 場所: `src/app/metadata_ops.rs:198`, `src/app.rs:31631`, 関連: `src/ui_main.rs:7590`
- シナリオ: 音声ファイル A/B があり、A だけにタグ `work` を付与する → タグファセットで `work` を選ぶ → 期待は A だけ表示だが、B を含む音声ファイルがタグ条件を通過する。`タグなし` 絞り込みでも同様に音声が正しく除外されない。
- 根拠: `tag_item_path` / `tag_ops` / タグ件数側は `GridItem::Audio` をタグ対象として扱っている一方、`facet_tag_filter_applies` は `Image | Video | ZipFile | PdfFile | ConvertibleArchive` のみで `Audio` を含まない。`passes_facet_filter` はこの関数が false の item ではタグ条件自体を評価しない。
- 確度: 高

[P2] detached/DWM snapshot の HWND 採用条件が可視性を見ず、未登録の hidden/iconic egui viewport を detached host として誤採用し得る
- 場所: `src/dwm_transitions.rs:318`, `src/app.rs:1080`, `src/app.rs:1099`
- シナリオ: UI スレッド上に hidden/minimized/stale な egui viewport (`class_name == "Window Class"`) が 1 つ残っている状態で detached window の登録 fallback に入る → `select_detached_unclaimed_hwnd` がそれを唯一の未登録 viewport と見なして採用 → native video host resync / focus / close が実際の可視 detached 窓ではなく古い HWND に向く。
- 根拠: snapshot は `visible` / `iconic` / `rect_ok` を収集しているが、`select_detached_created_hwnd` / `select_detached_unclaimed_hwnd` は `!is_main && class_name == EGUI_VIEWPORT_CLASS` だけで選別している。既存テストも visible=true の entry だけをモデル化しており、hidden/iconic 除外を保証していない。
- BA マッピング: BA-1(HWND 誤同定), BA-4(viewport identity)
- 確度: 中

[P3] サムネなし native navigation preview が全画面ヒット領域を持つのに背景を描かず、旧動画フレームを新項目の preview として見せ得る
- 場所: `src/video/native_presenter/overlay_draw.rs:3008`, `src/video/native_presenter/overlay_draw.rs:3030`, `src/video/native_presenter/mod.rs:5488`
- シナリオ: 動画から別動画へ移動し、移動先の preview thumbnail が未キャッシュ → preview top bar は新ファイル名を出し、HUD region は全画面になる → しかし背景は透明のままなので、下の DComp video visual の直前フレームが見える。ユーザーには「新ファイル名 + 旧動画映像」に見える。
- 根拠: `has_thumbnail` のときだけ `full_rect` を黒塗りする一方、`ui.interact(full_rect, ...)` と HUD region は thumbnail 有無に関係なく全画面。コードコメント上は黒点滅回避の意図があるため、仕様判断が必要。
- 確度: 中

**取り下げ**
- 音声レーティング Undo の `rating_meta_for_key_and_source` Audio 分岐漏れは、通常の音声レーティング設定と Undo キャプチャが `rating_meta_for_idx` を通り `RatingItemKind::Audio` を保持するため、今回の確定指摘からは外しました。

**問題なし確認**
- `src/video/native_window.rs`
- `src/video/native_presenter/hud_window.rs`
- `src/fs_animation.rs`
- `src/logger.rs`
- `src/settings.rs`
- `src/ui_dialogs/preferences/pages.rs`
- `src/ui_metadata_panel.rs`
- `src/ui_helpers.rs`
- `src/app/subfolder_expansion.rs`
- `src/app/folder_scan.rs`
- 補助確認: `src/tag_ops.rs`, `src/rating_db.rs`, `src/rating_view.rs`, `src/undo_ops.rs`, `src/app/native_video.rs`

コードレビューのみで、追加探索・ビルド・テスト実行はしていません。