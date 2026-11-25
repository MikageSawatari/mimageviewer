# 依存物の更新 — 2026-11-26 (v5.8.0)

backlog §1.903 の記録。v5.7.0 のリリース時の依存物の確認 (リリース手順 Phase 2 の 6.3) で見つかった更新のうち、
利用者が v5.8.0 で行うと決めたもの (2026-11-25) を扱う。版・入力資産の同一性・変更点・検証の範囲を書き、
公開・配布の完了記録ではない。作業ブランチは `deps-903` (worktree `G:\home\mimageviewer-deps903`、master `c77d48f46` から)。
設計・検収は ClaudeCode。

## 結果

| 依存物 | 更新前 | 更新後 |
| --- | --- | --- |
| 通常 crate | master `c77d48f46` の Cargo.lock | `cargo update` の範囲で 23 件 (下記) |
| PDFium | `chromium/8165`, `160.0.8165.0` | `chromium/8179`, `160.0.8179.0` (2026-11-23 公開) |
| mIV Sound の元 (EffeTune Mixwright) | v0.17.0 (mIV Sound 0.17.0-miv.1) | v0.19.0 (mIV Sound 0.19.0-miv.1、候補 `b903e76a…7334`、未承認) |

手を入れている crate (`vendor/eframe`・`egui`・`egui-wgpu`・`egui-winit`・`epaint`・`libmimalloc-sys`・
`tantivy`、`crates/unrar-patched`) は、2026-11-25 の確認でどれも最新。Rust 1.100.0・FFmpeg・LibRaw・
ONNX Runtime・VST3 SDK も変えない (同じ確認で最新、FFmpeg は手元が公開中の最新より新しい)。

## 通常 crate

`cargo update` (manifest の版の制約の中) で 23 件を上げた。v5.7.0 の確認 (2026-11-25) の表は 22 件
(先頭 10 件だけ表示) で、その後に 1 件増え、ahash の行き先も 0.8.13 から 0.8.14 に進んでいた。
加わった・外れた package は無い。

全件: ahash 0.8.12 → 0.8.14、anyhow 1.0.104 → 1.0.105、cc 1.11.2 → 1.11.3、const-random 0.1.18 → 0.1.20、
const-random-macro 0.1.16 → 0.1.18、crc32fast 1.5.5 → 1.5.6、form_urlencoded 1.2.2 → 1.2.3、hashbrown 0.17.1 → 0.17.2、
litrs 1.0.1 → 1.1.0、pin-project 1.1.13 → 1.2.0、pin-project-internal 1.1.13 → 1.2.0、pin-project-lite 0.2.17 → 0.2.18、
rangemap 1.8.0 → 1.9.0、simd_helpers 0.1.0 → 0.1.1、toml 1.2.1 → 1.2.3、toml_edit 0.25.19 → 0.25.21、
toml_parser 1.1.5 → 1.1.7、url 2.5.8 → 2.5.9、walkdir 2.5.0 → 2.5.1、zcheapstr 1.2.0 → 1.4.0、
zerocopy 0.8.63 → 0.8.65、zerocopy-derive 0.8.63 → 0.8.65、zlib-rs 0.6.8 → 0.6.9。

依存の付け替え (Cargo.lock の差分から):

- ahash と const-random-macro が乱数の取得に getrandom 0.4.3 を使うようになった (それぞれ 0.3.4 と 0.2.17 から)。
  getrandom 0.4.3 は前から lock にあり、0.2.17・0.3.4 もほかの crate が使い続けるので package の増減は無い。
- pin-project-internal (proc-macro) が syn 3 を使うようになった。syn 3.0.10 も前から lock にある。
- simd_helpers が quote を使わなくなった。
- いずれも manifest の制約の中の minor・patch の更新で、mIV のコードの変更は要らない。
- `ort` (ONNX Runtime) は対象に入っていない (`2.0.0-rc.15`・`api-24` のまま)。

manifest の版の制約を超える更新 (`cargo update --verbose` の「behind latest」40 件。rusqlite 0.31 → 0.43、
windows 0.61 → 0.62、ureq 2 → 3、pdfium-render 0.8 → 0.9、ffmpeg-the-third 3 → 6、zip `=2.4.2` の固定など) は
今回の範囲外。どれも API の変更か、意図した固定 (zip、FFmpeg 7.1 に合わせた ffmpeg-the-third) で、
1 件ずつ移行の作業が要る。利用者が決めた 22 件 (今回 23 件) は manifest の範囲の更新だけを指す。

## PDFium

### 取得と資産の同一性

`gh release download chromium/8179 --repo bblanchon/pdfium-binaries --pattern pdfium-win-x64.tgz` で空のフォルダへ取得・展開し、
展開した内容をそのまま `vendor/pdfium/` に置いた (`setup-pdfium.sh` と同じ資産・同じ展開。置いた後に `diff -r` で全ファイル一致)。

| 対象 | bytes | SHA-256 |
| --- | ---: | --- |
| 公式 `pdfium-win-x64.tgz` (`chromium/8179`) | 4,046,589 | `836a77c9053f624839c66010cddea7b675628a36b8831d2330178bb169d241b6` |
| `vendor/pdfium/bin/pdfium.dll` | 7,849,984 | `b9324fe77556753e24443b8ff1d03b39a3aa01ccd6e56e7debb82922438e8965` |

- アーカイブの SHA-256 は GitHub のリリース資産に記載の digest と一致した。
- DLL の FileVersion / ProductVersion はともに `160.0.8179.0`。

### 8165 との差

ファイル構成は 8165 と同じ。中身が変わったのは次だけ。

- `bin/pdfium.dll`、`VERSION` (`BUILD=8179`)、`PDFiumConfig.cmake` (`PDFium_VERSION` の行だけ)
- `include/cpp/fpdf_deleters.h`・`include/cpp/fpdf_scopers.h`: C++ 用の補助ヘッダに `FPDF_XOBJECT` の deleter / `ScopedFPDFXObject` が加わった。
  mIV は C API を `pdfium-render` 経由で動的に呼ぶだけで、この C++ ヘッダは使わない。

C API のヘッダ (`include/*.h`)、`lib/pdfium.dll.lib`、`LICENSE`、`licenses/` の全ファイル、`args.gn` はバイト単位で同一。したがって:

- **API・ABI**: コードの変更は要らない。
- **ライセンス**: `python scripts/generate-pdfium-license.py` で作り直した `PDFIUM-LICENSE.txt` の差分は先頭行の版
  (`160.0.8165.0` → `160.0.8179.0`) だけ。`--check` は成功。FreeType は上流で 16 commit 進んだが、同梱の `licenses/freetype.txt` は変わっていない。
  内部部品の増減も無く、配布文書に帰属文を書く条件 (FreeType・IJG) も変わらない。

上流の変更 (8179 のリリースノートの commit 一覧 `chromium/8165..chromium/8179` から、表示に関わるもの):

- BGRA → マスクの合成処理を 16 bit 幅の lane に (8165 の SIMD 化の続き)
- FreeType の更新 (16 commit)
- `FindTextlineFlowOrientation()` の float の桁あふれの修正 (テキストの向きの判定)
- `ObservedPtr` の仮想関数の削除、`CPDF_Document` の破棄通知の分離 (内部構造)
- XFA・JavaScript (V8) まわりの修正 (mIV の PDFium は XFA / V8 なし)、テスト・ビルドの整理

mIV の自動テストは描画結果の画素を比べないので、表示の確認は実機で行う (下の「未確認」)。

### vendor の置き場所と取り込み

`vendor/` は git 管理外。8179 の DLL はこの worktree の `G:\home\mimageviewer-deps903\vendor\pdfium\` にだけある。
`src/pdf_loader.rs` が DLL を `include_bytes!` で埋め込むので、ブランチを取り込んだ checkout で vendor が 8165 のままビルドすると、
`PDFIUM-LICENSE.txt` (8179) と埋め込み DLL (8165) が食い違う。About のテストが `generate-pdfium-license.py --check` で止めるので気付けるが、
取り込み時に次のどちらかで揃える。

```bash
# (a) 取り込む checkout で取り直す (上の SHA-256 と照合する)
bash scripts/setup-pdfium.sh
sha256sum vendor/pdfium/bin/pdfium.dll
# (b) この worktree から実体コピーする (junction にしない)
rm -rf vendor/pdfium && cp -r /g/home/mimageviewer-deps903/vendor/pdfium vendor/pdfium
python scripts/generate-pdfium-license.py --check
```

`C:\home\mimageviewer_vendor_backup\` への退避は不要 (PDFium は再取得できる。backup の対象は models・vst3-host・vlm-worker と mIV Sound の承認物)。

## mIV Sound の元 (EffeTune Mixwright) 0.19.0

詳しい記録は [派生版計画 §21](effetune-fork-plan.md#21-0190-miv1-への更新-2026-11-26)。ここには要点だけを置く。

- 上流 v0.17.0 → v0.19.0 (Mixwright 5 commit、EffeTune 2.17.0 → 2.19.0)。派生版 `G:\home\miv-sound` のブランチ `miv-0.19`
  (merge `42c7d49`、修正 `346c8b0`・`801ca2a`、未 push)。submodule は effetune `0ffa5c7`、choc `8af031c`。
- 利用者の決定 (2026-11-26、統括経由): 歌詞は派生版の方式を残す (上流の `EffeTune.NowPlayingLyrics` と ProcessContext の時計は派生版から外した。
  mIV 本体と vst3-host は変えない)。Headphone EQ は AutoEQ の取得も含めて出す (CSP の connect-src に
  `https://raw.githubusercontent.com/jaakkopasanen/AutoEq/` だけを足す)。Distortion Correction は追加の一覧から外す (保存済みは読める)。
  Live Audio Send/Receive は従来どおり外す。0.19 → 0.17 へ戻したときの互換は保証しない。
- 候補 `b903e76a…7334` (未承認)、ctest 139 件すべて成功。独立レビューは 2 回目で ACCEPT。
- mIV 側: `config/miv-sound.rs` を 0.19.0-miv.1 / v0.19.0 にし、説明書 (snapshot `third_party/effetune-docs/0ffa5c70`)・
  privacy.html (日英、Headphone EQ の通信と保存)・マニュアル `effetune.html` を更新した。製品ページのカードは今回も成り立つので変えていない。
- **承認 (`-Approve`) まで、このブランチは `MIV_SOUND_CANDIDATE` を付けないとビルドできない** (config は 0.19、`vendor/miv-sound` は 0.17 のまま)。
  承認で `vendor/miv-sound`・`third_party/miv-sound/0.19.0-miv.1/` と `C:\home\mimageviewer_vendor_backup\miv-sound-0.19.0-miv.1\` ができる。
- **公開前の VirusTotal (リリース手順 12.6) が要る**: `mIV Sound.vst3` (DLL と webview の資源) が変わる。vst3-host は変わらない。

## 検証

どれも設計担当が run-heavy 経由で流した (Codex にはコンパイル・テストを流させていない)。製品のバイナリはエージェントが起動していない。

### crate の更新 (4b478e6db)

- `cargo check --workspace --all-targets --exclude mimageviewer-launcher`: 成功。エラーなし、更新した crate に関する非推奨の警告なし。
- 本体 lib 全件 (`.\scripts\test-lib-full.ps1`、NEXTEST_TEST_THREADS=12): **20,784 件中 20,782 件成功 / 2 件失敗 / 106 skipped** (834.5 秒)。
  失敗はどちらも `app::tests::still_window_mode_key_tests` のリモート閲覧の受け入れのテスト
  (`remote_acquire_retires_startup_resolves_and_rejects_new_activations`、`remote_session_acquire_closes_mounted_active_and_parked_live_media_at_position`)
  で、lease を返した直後の `poll_remote_session` の後も `AcquiringRemote` のままで `RemoteActive` にならなかった。
  同じソースで `still_window_mode_key_tests::remote_` の 14 件を単独で 3 回流すと 3 回とも全件成功した。master の直近の全件
  (統括、2026-11-26 00:18 開始、同じ 20,784 件) は全件成功している。負荷の高い全件の中でだけ出る時間依存の失敗と見ているが、
  統括の判断: 既知の §1.847 (リモート取得のテストが混雑時に 1 回の poll で完了しない、テストの組み方の問題) と同じ群。
  crate・PDFium を取り込んだ後の master の全件 (20,796 件) は全件成功し、この 2 件は出なかった (統括、2026-11-26)。
- 独立レビュー (gpt-6.1-sol / xhigh、read-only): 1 回目 REJECT (P2: Windows 向けにはビルドされない pin-project 1.2.0・zcheapstr のソースが手元に無く未確認、
  P3: zlib-rs 0.6.9 の不正な zlib ヘッダでの assert は mIV の flate2 の使い方からは到達しない)。`cargo fetch` の後の 2 回目で ACCEPT。
  レビューが挙げた挙動の変化: ahash のハッシュ値が変わる (mIV は ahash の値を保存していない)、url がパスの `^` を `%5E` にする、
  zcheapstr 1.3 で serde の表現が変わった (Linux の accesskit 経由だけで Windows ではビルドされない)。どれも mIV のコードの変更は要らない。

### PDFium 8179

- `python scripts/generate-pdfium-license.py --check`: 成功。
- `cargo nextest run -p mimageviewer --lib -E 'test(/pdf/) | test(/pdfium/) | test(/about/)'`: 609 件すべて成功 (About の `pdfium_notices_match_all_vendor_files_and_version` を含む)。

## 未確認 (実機)

mIV の自動テストは PDF の描画結果の画素を比べない。利用者 (サブ PC) の実機で次を見てもらう必要がある。

- PDF: サムネイル (表紙)、ページ送り・見開き・拡大、文字の多い PDF (縦書き・横書き)、図形・半透明・クリップの多い PDF、スキャン画像の PDF、
  パスワード付き PDF。About の PDFium の版が `160.0.8179.0`。
