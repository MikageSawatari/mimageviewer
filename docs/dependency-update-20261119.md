# 依存物の更新 — 2026-11-19

backlog §1.816 の記録。Rust・PDFium・通常 crate を上げ、FFmpeg の更新確認の誤表示を直した。
版・入力資産の同一性・変更点・検証の範囲を扱い、公開・配布の完了記録ではない。
作業ブランチは `deps-816` (worktree `G:\home\mimageviewer-deps816`)。設計・検収は ClaudeCode、
FFmpeg の確認スクリプトの実装は Codex (gpt-6.1-sol / high)。

## 結果

| 依存物 | 更新前 | 更新後 |
| --- | --- | --- |
| Rust | 1.99.1 | 1.100.0 (`0ef1bc386` 2026-11-09、cargo `3acc79c0c`) |
| PDFium | `chromium/8138`, `159.0.8138.0` | `chromium/8165`, `160.0.8165.0` (2026-11-16 公開) |
| 通常 crate | master `ca6686d93` の Cargo.lock | `cargo update` の範囲で 71 件 (下記) |
| FFmpeg | `n7.1.5-16-g9a4bb2c579` | 据え置き (公開中の最新 n7.1 資産 `n7.1.5-12` より手元が新しい) |
| EffeTune Mixwright (mIV Sound の元) | v0.17.0 | 据え置き (v0.18.1 あり。派生版の作り直しと VirusTotal の検査が要るので次の版で別に扱う) |

手を入れている crate (`vendor/eframe`・`egui`・`egui-wgpu`・`egui-winit`・`epaint`・`libmimalloc-sys`・
`tantivy`、`crates/unrar-patched`) と、zip の `=2.4.2` 固定は対象外のまま。

## PDFium

### 取得手順

```bash
bash scripts/setup-pdfium.sh check   # Latest release: chromium/8165
bash scripts/setup-pdfium.sh         # vendor/pdfium/ へ展開
```

別に同じ資産を空のフォルダへ取得・展開し、展開済みの vendor と全ファイルを比べた。

### 資産の同一性

| 対象 | bytes | SHA-256 |
| --- | ---: | --- |
| 公式 `pdfium-win-x64.tgz` (`chromium/8165`) | 4,043,420 | `ca7338ae7779b8b1b82f4580305c4d762d214f8df1d54b47647fe3cc75989ded` |
| `vendor/pdfium/bin/pdfium.dll` | 7,846,912 | `31d249b8535dd5f351ae7f4b714dc4bb1d76b6069b321a309bc99271dd65f566` |

- アーカイブの SHA-256 は GitHub のリリース資産に記載の digest と一致した。
- 資産内の全ファイルが展開済みの `vendor/pdfium/` とバイト単位で一致した (`diff -r`)。
- DLL の FileVersion / ProductVersion はともに `160.0.8165.0`。

### 8138 との差

ファイル構成は 8138 と同じ。中身が変わったのは次の 3 つだけ。

- `bin/pdfium.dll`
- `VERSION` (`MAJOR=160`、`BUILD=8165`)
- `PDFiumConfig.cmake` (`PDFium_VERSION` の行だけ)

`include/` の全ヘッダ、`lib/pdfium.dll.lib`、`LICENSE`、`licenses/` の全ファイル、`args.gn` はバイト単位で同一。
したがって:

- **API・ABI**: 公開ヘッダと import library が同一なので、コードの変更は要らない。
- **ライセンス**: `python scripts/generate-pdfium-license.py` で作り直した `PDFIUM-LICENSE.txt` の差分は
  先頭行の版 (`159.0.8138.0` → `160.0.8165.0`) だけ。`--check` は成功。FreeType は上流で 36 commit 進んだが、
  同梱の `licenses/freetype.txt` は変わっていない。

上流の変更 (8152・8165 のリリースノートの commit 一覧から、表示に関わるもの):

- Lab の ICC プロファイルの扱いの修正、画像に Pattern 色空間を指定した不正な PDF の拒否
- FreeType の更新 (36 commit)、BGRA → マスクの合成処理の SIMD 化
- CCITT Fax・Flate (predictor 付き) のデコーダの Rewind で状態を戻し切れていなかった不具合の修正 (スキャン PDF に関係する)
- AES-192 の鍵を拒否 (PDF の仕様に無い鍵長。AES-128・AES-256 は従来どおり)
- 循環参照の検出の書き直し、`CheckSimpleLinePath` の float の桁あふれの修正
- XFA・JavaScript・保存まわりの修正 (mIV の PDFium は XFA / V8 なしで、保存しない)

mIV の自動テストは描画結果の画素を比べないので、表示の確認は実機で行う。

### vendor の置き場所と取り込み

`vendor/` は git 管理外。新しい DLL はこの worktree の `G:\home\mimageviewer-deps816\vendor\pdfium\` にだけある。
統括の checkout (`C:\home\mimageviewer\vendor\pdfium`) とほかの worktree は 8138 のまま (junction で共有していない)。
`src/pdf_loader.rs` が DLL を `include_bytes!` で埋め込むので、ブランチを取り込んだ checkout で
vendor が 8138 のままビルドすると、`PDFIUM-LICENSE.txt` (8165) と埋め込み DLL (8138) が食い違う。
About のテストが `generate-pdfium-license.py --check` で止めるので気付けるが、取り込み時に次のどちらかで揃える。

```bash
# (a) 取り込む checkout で取り直す (上の SHA-256 と照合する)
bash scripts/setup-pdfium.sh
sha256sum vendor/pdfium/bin/pdfium.dll
# (b) この worktree から実体コピーする (junction にしない)
rm -rf vendor/pdfium && cp -r /g/home/mimageviewer-deps816/vendor/pdfium vendor/pdfium
python scripts/generate-pdfium-license.py --check
```

`C:\home\mimageviewer_vendor_backup\` への退避は不要 (PDFium は再取得できる)。

## Rust 1.100.0

`rust-toolchain.toml` の `channel` を `1.100.0` にした (`Cargo.toml` の MSRV のコメントも)。
workspace の `rust-version` (MSRV) は変えていない。`cargo update` は MSRV に合う版を選ぶ (「Rust 1.95 compatible」)。

- `cargo fmt --check`: 1.100 の rustfmt でも差分なし。
- 警告: `cargo check --workspace --all-targets --exclude mimageviewer-launcher --message-format short` を
  同じ Cargo.lock で 1.99.1 (`--target-dir target/check-1991`) と 1.100.0 の両方で流し、位置を除いた警告文を突き合わせた。
  ソースの警告は両方とも 595 件で、内容も同一 (どちらにしか無いものは 0 件)。
  1.100 で増えたのは cargo の新しい lint `cargo::unused_dependencies` の 4 件だけで、次のとおり解消した。
  - 本体の `kamadak-exif`: 使っていない (EXIF は `rexif`)。依存から外した (Cargo.lock から `kamadak-exif`・`mutate_once` が消える)。
  - `tools/comic_lab` の `ab_glyph`、`tools/music_lab` の `serde_json`: 使っていない。外した。
  - `crates/libraw-sys` の `turbojpeg-sys`: build.rs がヘッダと `turbojpeg-static.lib` を使う link 専用の依存。
    外さずに `use turbojpeg_sys as _;` で使っていることを示した。
- bench と tantivy の生成コードの確認 (リリース手順 Phase 2 の 9.5) は下の節。

## 通常 crate

`cargo update` (manifest の制約の中) で 67 件を上げ、4 件が加わり 7 件が外れた。
主なもの: wgpu 一式 30.0.1 → 30.0.2 (vendor の egui-wgpu が使う。patch 版)、exr 1.74.2 → 1.74.3
(SIMD の実装が `pulp` から `multiversion` に変わり、`pulp`・`raw-cpuid`・`reborrow`・`defmt` 系が外れた)、
jiff 0.2.38 → 0.2.46、toml 1.1.8 → 1.2.1、regex 1.13.1 → 1.13.2、cc 1.10.0 → 1.11.2、
rustls-webpki 0.103.16 → 0.103.17、flate2 1.1.10 → 1.1.11、lz4_flex 0.14.0 → 0.14.2、notify 8.2.1 → 8.2.2。

全件: arc-swap 1.9.5、async-executor 1.14.1、async-lock 3.5.0、bit_field 0.10.4、bstr 1.14.0、bumpalo 3.20.4、
bytes 1.12.2、cc 1.11.2、crc32fast 1.5.5、encoding_rs 0.8.44、errno 0.3.15、exr 1.74.3、fixedbitset 0.5.9、
flate2 1.1.11、itoa 1.0.19、jiff 0.2.46 (jiff-core 0.1.8・jiff-static 0.2.46・jiff-tzdb 0.1.10)、js-sys 0.3.107、
kamadak-exif 0.6.2 (後で依存から外した)、libbz2-rs-sys 0.2.7、libloading 0.9.1、log 0.4.35、lz4_flex 0.14.2、mio 1.2.5、
naga-types 30.0.2、nix 0.31.4、notify 8.2.2、notify-types 2.3.1、pollster 1.0.3、regex 1.13.2 (regex-automata 0.4.19・
regex-syntax 0.8.12)、rustls-pki-types 1.16.0、rustls-webpki 0.103.17、serde_spanned 1.1.3、smallvec 1.16.6、syn 3.0.10、
toml 1.2.1 (toml_datetime 1.1.3・toml_edit 0.25.19・toml_writer 1.2.0)、unicode-properties 0.1.5、unicode-script 0.5.9、
utf8_iter 1.1.0、uuid 1.29.3、wasm-bindgen 0.2.130 一式・wasm-bindgen-futures 0.4.80・web-sys 0.3.107、webbrowser 1.2.7、
webpki-roots 1.0.12、wgpu 30.0.2 一式 (wgpu-core・wgpu-hal・wgpu-types・wgpu-naga-bridge・wgpu-core-deps-*)、winnow 1.0.5、
zcheapstr 1.2.0、zeroize 1.9.2。加わった: multiversion 0.8.1・multiversion-macros 0.8.1・target-features 0.1.6・
utf_types 1.0.0。外れた: defmt 1.2.0・defmt-macros 1.2.0・defmt-parser 1.1.0・pulp 0.22.3・pulp-wasm-simd-flag 0.1.1・
raw-cpuid 11.6.0・reborrow 0.5.5。

manifest の版の制約を超える更新 (`cargo update --verbose` の「behind latest」) は今回の範囲外。

## FFmpeg の更新確認の誤表示

`bash scripts/setup-ffmpeg.sh check` は資産名を文字列で比べていたので、公開中の最新の n7.1 資産
(`autobuild-2026-07-31-14-10` の `ffmpeg-n7.1.5-12-g1fdbca85aa`) より手元の `n7.1.5-16` のほうが新しいのに
「New version available」と出し、`check-dependency-updates.ps1` の表もそれを写していた。check なしで実行すると
古い -12 を取得して格下げするところだった。

major・minor・patch (省略時は 0)・コミット数を数値で比べるようにした。手元が新しいときは
`Up to date. Current is newer than the newest published versioned asset: ...`、取得時は格下げを拒否する。
数値が同じで名前が違う・解析できないときは推測せず失敗にする。オフラインのテストは
`bash scripts/tests/test-setup-ffmpeg.sh` (16 件)、表の判定は `scripts/test-check-dependency-updates.ps1`。
2026-11-19 の実際の check は `Up to date. Current is newer ...` を出した。
