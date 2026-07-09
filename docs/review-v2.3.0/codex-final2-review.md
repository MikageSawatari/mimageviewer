結論: クリーン。今回の 2 件について P1/P2/P3 の指摘はありません。

`draw_native_navigation_preview` は `src/video/native_presenter/mod.rs:6090` の `navigation_preview.is_some()` 分岐内だけで呼ばれており、通常再生の `preview == None` では到達しません。`navigation_preview` 自体も `src/app/native_video.rs:762` で navigation swap 時だけ設定され、`src/video/mod.rs:2266` / `src/video/mod.rs:2460` / `src/video/mod.rs:4062` の経路でクリアされます。したがって `src/video/native_presenter/overlay_draw.rs:3031` の常時黒塗りはプレビュー表示中に限定され、通常再生には影響しない構造です。

`src/version_highlights.rs:248` の must_read 文言と `docs/review-v2.3.0/release-note-drafts.md:8` の注意書きも、表示専用・ユーザー向け表現として問題ありません。確認として `cargo test --lib version_highlights::tests::embedded_table_contains_v2_3_0_entry -- --exact` は通過、対象コードの `git diff --check` も問題なしです。

補足: `docs/review-v2.3.0/release-note-drafts.md` は現在 `??` の未追跡ファイルなので、コミット対象にする場合は `git add` が必要です。