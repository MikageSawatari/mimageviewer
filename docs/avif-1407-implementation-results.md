# AVIF 内蔵デコーダ 実装・検証記録

対象: `avif-1407`、基点 `797dbf905`。この作業ではコミット・push・製品起動を行っていない。
設計の正本は [avif-builtin-decoder-plan.md](avif-builtin-decoder-plan.md)。

## 実装

- libavif / dav1d / libyuv の static link と `/MT` の C shim。vendor の全 VERSION と3本のライブラリを検査する。
- `avif_decoder` は parse・寸法・RGBA予算・decode・色変換の型付き失敗（libavif code/diag）を返す。
  Rustで clap→irot→imir を適用。同じ画像ownerへ代入し、変換前rasterを各段で解放する。不正 clap のみを無視し、straight alpha の RGBA8 を返す。
- `image_decode::Decoded` が画像・使用decoder・向き適用済みをまとめて持つ。
  AVIFは内蔵→WIC→既存Susie。その他の形式とJPEG DCT高速経路を維持する。
  拡張子のない埋め込みbytesは既存バッファのヘッダからAVIFを識別する。
- 取消はnative前後・fallback前後。既存の非中断Susie IPCの扱いを呼出元ごとに保持する。
- 背景/一括/先読みは1 thread、需要のフルサイズ/clipboard/Remote AIは4 threads。
  logical CPU24、fullscreen最大6件、thumbnail/batch Autoは12 workersを確認した。
- キャッシュには形式/decoder版の共通無効化機構がなく、元形式不明の代表行もある。
  指示に従い永続schema・キャッシュ無効化は追加しない。既存WIC製AVIFサムネイルは再生成まで残る。

## 接続した呼出し箇所

| ファイル | 経路 |
| --- | --- |
| `src/canonical_image_loader.rs` | path / verified bytes / ZIP bytes、FallbackDecoder fake seam |
| `src/thumb_loader.rs` | decode_image_for_thumb / decode_zip_chain / load_one_cachedのpath / batch path・ZIP |
| `src/app.rs` | panorama高解像度source、fullscreen需要/先読みのworkload |
| `src/app/book_composition_editor_thumbnails.rs` | 背景thumbnailのworkload |
| `src/books.rs` | composite sourceのfile / ZIP bytes |
| `src/similar_image.rs` | file / encoded bytes proxy（既存のfile WIC→bytes WICも維持） |
| `src/ui_dialogs/context_menu.rs` | file / ZIP clipboard worker |
| `src/effetune/now_playing/artwork.rs` | bounded bytes、拡張子なし埋め込みAVIFも含む |
| `src/bin/bench_dupe.rs` | full decode / synthesis、二重EXIFの撤去 |
| `src/bin/bench_ai.rs` | file入力（従来のRAW扱いを保持） |
| `src/bin/bench_zip_seek.rs` | ZIP decode / 並列ZIP計測 |
| `src/bin/bench_raw.rs` | DNG専用WIC対照も共有helperを経由 |
| `src/remote_ipc/container.rs` / `thumbnail.rs` | canonical / thumb_loaderを通ることを確認。独立のdecode経路なし |

直接WIC呼出しを残したのは共有helper、canonical production seam、artworkのWIC対照unit testだけ。
棚卸しの範囲では同期UIからの新規到達はなく、既存worker配置を維持した。

## 自動検証

ビルド・テスト・Clippy実行に `MSBUILDDISABLENODEREUSE=1` を設定した。
このsandboxの既存TurboJPEG MSBuildはエラー0件のまま失敗したため、プロセス環境に
`CMAKE_GENERATOR=NMake Makefiles` を設定して検証した。リポジトリの設定は変えていない。

| 検査 | 結果 / ログ |
| --- | --- |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 成功。所有権整理後も成功。`target/avif-check-final.log` |
| `cargo check -p mimageviewer --features dev-tools --bin bench_ai --bin bench_dupe --bin bench_raw --bin bench_zip_seek` | 4本成功。`target/avif-bench-check.log` |
| `cargo nextest run -p mimageviewer --lib -E 'test(/avif/)'` | 21/21成功。`target/avif-tests-pass.log`。後続69件にも全件含む |
| `cargo nextest run -p mimageviewer --lib -E 'test(/avif/) \| test(~canonical_image_loader::) \| test(~similar_image::) \| test(~effetune::now_playing::artwork::) \| test(~stats::) \| test(~wic_decoder::)'` | 69/69成功。`target/avif-regression.log`。所有権整理後は影響するAVIF全21件を後続22件で再確認 |
| AVIF＋埋め込みライセンスの最終focused `cargo nextest run -p mimageviewer --lib -E 'test(/avif/) \| test(~embedded_raw_notices_equal_tracked_files_with_bsd_and_ijg_attribution)'` | 所有権整理後も22/22成功。`target/avif-final-focused.log` |
| UI snapshot（Susie診断 / AVIFライセンス） | 更新11/11、通常比較11/11成功。`target/avif-ui-update.log` / `target/avif-ui-verify.log`。変更3枚＋新規2枚を目視確認（明暗・全文表示リンク・診断文と復帰履歴の収まり） |
| `cargo fmt` / `cargo fmt --check` | 成功（引数なしfmt後、check exit0）。`target/avif-fmt-final.log` |
| `cargo clippy -p mimageviewer --lib --tests` | 最終exit0。既存警告は下記。`target/avif-clippy-final.log` |
| `python scripts/check_ui_glyphs.py` | dangerous glyph 0 |
| `python scripts/check_site_cards.py` | 49 cards、超過0 |
| 3本のrootライセンスとvendor/licensesのbyte比較 | 全件一致（20,415 / 1,317 / 1,535 bytes） |
| `scripts/build-dev.ps1 -PreserveRuntime -WaitForOtherBuildsMinutes 0` | 成功。所有権整理後の最終core差分build 1m21s、Remote / EPUBも成功。`target/avif-build-dev-final.log`。通常feature、portableなし。起動していない |

AVIFは11種の正常fixtureと2種の不正fixtureを全件検査。path/bytes一致、色±16、alpha、
grid、irot/imir、最初のframe、gray、10/12bit、4:2:2/4:4:4、割当前予算、ZIP、thumbnail、
非正方形clap→rotation→mirror、取消、fake fallback順、二重EXIF抑止を検査した。
追加clap fixtureの必須属性フラグ不足による2失敗は試験データを修正して解消した。
全件 `test-lib-full.ps1` は依頼どおり実行していない。

## 制限・設計担当の確認事項

1. 既存now-playing bounded経路はWICで終わり、Susieは元から呼ばない。
   Susieには割当前RGBA予算のAPIがないため、内蔵→bounded WICを保持した。
   全経路にSusieを新設するなら別途worker/protocolの予算対応が必要。利用者へ解釈を確認中。
2. Clippyの全体警告ゼロは未達。初回は本体2,000件、lib test2,768件（1,788件重複）などの
   既存警告が出た。新設avif_decoder/image_decode/libavif-sysと差分行に警告はなかった。
   既存警告の一括整理は行っていない。最終実行も本体2,000件 / lib test2,768件（1,788件重複）、exit0。
3. `setup-libavif.sh` の既存skip条件はVERSIONのみ。同じVERSIONのままライブラリだけ失われると
   通常setup/bootstrapでは復旧せず、`bash scripts/setup-libavif.sh --force` が必要。
   依頼どおりsetupスクリプトは実行・変更していない。skip条件の改善候補として報告する。
4. 同じprofileの変更前exeがないため、exe増分と実画像での速度比較は未測定。今回coreは 335,876,608 bytes（dev-runtime、リソース埋め込みを含む）。
5. `bash -n scripts/bootstrap-vendor.sh` はsandboxのCreateFileMapping（Win32 error5）で
   Bash自体が起動できず未検証。追加した段は既存と同じrun_step形式。
6. 実機の透過・色・ZIP・Remote確認、独立レビュー、全件ゲートは設計担当/利用者側。

## 利用者による確認

インストール版・tray常駐版を先に終了する（single-instance mutex共用）。
このビルドは `dev-runtime` という最適化profileで、通常版と同じ
`%APPDATA%\mimageviewer` を使い、実利用中の設定・データ・キャッシュ・ログを更新し得る。
この作業ではバイナリを起動していない。

リポジトリルートで利用者が実行:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

- `testdata/avif/quadrant_alpha_444.avif` の左半分の透過、各quadrantの色、irotの左上が青になる向きを確認。
- `target/avif-verification-fixtures.zip` を開き、ZIP内でも同じ表示になることを確認。
  正常な11 fixtureのみをStored ZIPへ格納し、CRC検査済み。不正2ファイルは含まない。
- mIV Remoteで同じZIP/画像のthumbnailとページを確認する。
- 既存のWIC製サムネイルは再生成まで残るため、今回の新しいfixture/ZIPを使う。

## 変更ファイル

- `.gitattributes`
- `Cargo.lock`
- `Cargo.toml`
- `DAV1D-LICENSE.txt`
- `LIBAVIF-LICENSE.txt`
- `LIBYUV-LICENSE.txt`
- `README.md`
- `crates/libavif-sys/Cargo.toml`
- `crates/libavif-sys/build.rs`
- `crates/libavif-sys/shim/miv_avif.c`
- `crates/libavif-sys/src/lib.rs`
- `docs/architecture-overview.md`
- `docs/avif-1407-implementation-results.md`
- `docs/avif-builtin-decoder-plan.md`
- `docs/display-pipeline.md`
- `docs/native-dependencies.md`
- `docs/project-reference.md`
- `docs/release-checklist.md`
- `docs/spec.md`
- `docs/virtual-folders.md`
- `htdocs/mimageviewer/index.html`
- `htdocs/mimageviewer/manual/formats.html`
- `htdocs/mimageviewer/manual/getting-started.html`
- `htdocs/mimageviewer/manual/troubleshooting.html`
- `htdocs/mimageviewer/migrate-leeyes.html`
- `htdocs/mimageviewer/migrate-mangameeya.html`
- `htdocs/mimageviewer/migrate-massigra.html`
- `htdocs/mimageviewer/migrate-photos.html`
- `installer/mimageviewer.iss`
- `installer/readme.txt`
- `installer/readme_portable.txt`
- `scripts/bootstrap-vendor.sh`
- `scripts/build-portable.ps1`
- `src/app.rs`
- `src/app/book_composition_editor_thumbnails.rs`
- `src/avif_decoder.rs`
- `src/bin/bench_ai.rs`
- `src/bin/bench_dupe.rs`
- `src/bin/bench_raw.rs`
- `src/bin/bench_zip_seek.rs`
- `src/books.rs`
- `src/canonical_image_loader.rs`
- `src/effetune/now_playing/artwork.rs`
- `src/image_decode.rs`
- `src/lib.rs`
- `src/remote_ipc/container.rs`
- `src/similar_image.rs`
- `src/stats.rs`
- `src/thumb_loader.rs`
- `src/ui_dialogs/about.rs`
- `src/ui_dialogs/context_menu.rs`
- `src/ui_dialogs/mod.rs`
- `src/ui_dialogs/stats_dialog.rs`
- `src/ui_susie_diagnostic.rs`
- `src/wic_decoder.rs`
- `testdata/avif/README.md`
- `tests/snapshots/avif_license_information_dark.png`
- `tests/snapshots/avif_license_information_light.png`
- `tests/snapshots/susie_diagnostic_ready_after_recovery.png`
- `tests/snapshots/susie_diagnostic_ready_with_plugins.png`
- `tests/snapshots/susie_diagnostic_ready_with_plugins_dark.png`
- `tests/ui_snapshot.rs`
