# egui 0.37 更新 — 段6 検証台帳

設計担当 (ClaudeCode) が 2026-10-17 に実行した非対話の全体 gate。source は `4f95c926e` (段5、独立レビュー ACCEPT)。
toolchain は `rust-toolchain.toml` の Rust 1.99.0、`MSBUILDDISABLENODEREUSE=1`。live テスト・製品起動はしていない。

| 手順 | コマンド | 結果 | 秒 |
| --- | --- | --- | ---: |
| 整形 | `cargo fmt --check` | exit 0 | 17 |
| UI glyph | `python scripts/check_ui_glyphs.py` | exit 0 | 2 |
| release core | `cargo build --release --bin mimageviewer-core` | exit 0 | 941 |
| release remote | `cargo build --release -p mimageviewer-remote --bin mimageviewer-remote --features embedded-web-assets` | exit 0 | 44 |
| release EPUB worker | `cargo build --release -p epub-pdf-worker --bin mimageviewer-epub-pdf` | exit 0 | 20 |
| 本体 lib 全件 | `scripts/test-lib-full.ps1` | PASS: 13,448 passed、59 skipped (既存)、失敗 0 | 379 |
| 全体 | `scripts/test-full.ps1` | PASS (exit 0) | 923 |
| 確認用 build | `scripts/build-dev.ps1 -PreserveRuntime` | exit 0、`target/dev-runtime/mimageviewer-core.exe` | 347 |

release の 3 本は `test-full.ps1` の前提 (launcher の build.rs が内包対象 exe の存在を検査する) として作った。
test-full には vendor の非対話区分・MSRV 検査・core / Remote / EPUB が含まれる。live の eframe テストは段0a のとおり既定対象外 (`scripts/test-live-windows.ps1` で承認付き実行)。

自動 gate は成功。段7 の利用者実機確認と承認付き live 試験は未実施で、この台帳は PASS として扱わない。

## master 取り込み後の再確認 (2026-10-17 夜)

master `720565e35` を merge (`6540b982d`、コンフリクトなし)。master 由来のテクスチャ共有テストを 0.37 の API へ移行 (`147519d5e`)。
source `147519d5e` で、fmt・core check・全 test target のコンパイル・release core のビルド・`test-full.ps1` (PASS) は exit 0。

`test-lib-full.ps1` は 13,461 件中 2 件が失敗した。どちらも単独で再実行すると成功し、egui の変更が触れない経路のテスト:

- `video::tests::real_decoder_eof_full_loop_preserves_single_tail_burst_per_epoch` (実 decoder の音声 callback で末尾 burst が 2 回に分かれた)
- `app::section375_tests::section1375_search_root_delete_does_not_dispatch_physical_folder_reload` (実ファイル監視の最初の通知が来なかった)

実行時、利用者が確認用ビルドを起動して操作中で、別の作業ツリーでも native のビルドが走っていた。負荷によって実時計・実 OS 通知に依存するテストが
落ちる既知の傾向 (backlog §1.389) と同じ形と判断した。`build-dev.ps1 -PreserveRuntime` は、利用者が起動中の dev core を止めないため
作り直しを断った (スクリプトの意図どおり)。
