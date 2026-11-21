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
- bench と tantivy の生成コードの確認 (リリース手順 Phase 2 の 9.5) は [下の節](#bench-と-tantivy-の生成コード-リリース手順-phase-2-の-95)。

## bench と tantivy の生成コード (リリース手順 Phase 2 の 9.5)

### bench

`cargo build --release --features dev-tools --bin bench_search` (Rust 1.100.0、c01a6ab63 と同じソース) の後、
run-heavy の全件枠 (ほかの重い実行が終わるのを待ち、新しい実行を止める) の中で `bench_search --docs 50000` を 3 回測った。
`vendor/bench_baseline.json` と比べ、1 回目だけでも 3 回の最良値でも `OK: 全クエリで回帰なし` (exit 0)。baseline は上書きしていない。

| query | baseline | 1 回目 | 3 回の最良 |
| --- | ---: | ---: | ---: |
| en_ai_phrase | 1.35 ms | 1.46 ms (+8.5%) | 1.34 ms (-1.0%) |
| en_common | 16.85 ms | 14.96 ms (-11.2%) | 14.96 ms (-11.2%) |
| generic_jp | 25.07 ms | 19.24 ms (-23.3%) | 19.24 ms (-23.3%) |
| medium_jp | 7.20 ms | 6.41 ms (-11.0%) | 6.41 ms (-11.0%) |
| medium_jp_3 | 0.74 ms | 0.74 ms (-0.1%) | 0.74 ms (-0.1%) |
| medium_jp_and | 1.65 ms | 1.73 ms (+4.8%) | 1.63 ms (-1.1%) |
| rare_jp | 1.03 ms | 0.89 ms (-14.4%) | 0.84 ms (-18.8%) |
| rare_jp_and | 0.15 ms | 0.14 ms (-7.6%) | 0.14 ms (-7.6%) |
| super_generic | 0.01 ms | 0.00 ms (-24.2%) | 0.00 ms (-35.5%) |
| unique_id | 1.12 ms | 1.08 ms (-3.8%) | 1.08 ms (-3.8%) |

結果の JSON とログは `target/dependency-update-20261119/bench_*` (git 管理外)。

### 製品の生成コード

`CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_STRIP=false CARGO_PROFILE_RELEASE_DEBUG=line-tables-only
cargo build --release -p mimageviewer --bin mimageviewer-core --target-dir target/core-sym --locked` (18 分 43 秒、run-heavy 経由) の
core を、Codex (gpt-6.1-sol / high、cargo 等は実行させず dumpbin・llvm-pdbutil・python だけ) が前回 (2026-10-28) と同じ方法で照合した。
core の SHA-256 は `30b71d028be26b4490ddd885a5f6c84c7058bcb7cecf6274cd31e598370e46d2`、PDB は
`f0060a7e0f3fd2f91bf7d6aa20699769a10a65b27cda53a5152eb97344f6df8e`。PE の RSDS と PDB の GUID
`464F178C-25B0-CFF1-4C4C-44205044422E` / age 1 が一致。

| 関数 | 命令数 (前回 → 今回) | unpcklps / addps / divps | mulss / addss / divss |
| --- | ---: | --- | --- |
| score_out_of_line (Box) | 4 → 4 | 0 / 0 / 0 | 0 / 0 / 0 |
| score_out_of_line (TermScorer) | 33 → 33 | 0 / 0 / 0 | 1 / 1 / 1 |
| Intersection::score (Box, Box) | 39 → 39 | 0 / 0 / 0 | 0 / 3 / 0 |
| Intersection::score (TermScorer, Box) | 72 → 72 | 0 / 0 / 0 | 1 / 4 / 1 |
| block_wand_intersection | 前回・今回とも製品から除去 | — | — |

pruning → `into_box_scorer` → `intersect_scorers` の経路と、返す vtable (`0x156E34160` / `0x156E34298`) の score slot `+0x98` が
上の 2 種の `Intersection::score` を指すこと、両 score から helper への call が残ることも、実バイトで照合した (ICF の別名も確認)。
抽出した 4 関数の命令数と 6 命令の数は設計担当も数え直して一致した。
左右 2 語の BM25 をまとめる SIMD の展開は無く、§1.424 の防ぎは 1.100.0 でも効いている。
これは指定の採点経路の証拠であり、全 callee が scalar であることや実行性能の証明ではない。
証跡は `target/dependency-update-20261119/` (codegen-report.txt・dumpbin-scoring.json・verification.json・functions-1..4.txt・
routes-1..5.txt と抽出スクリプト。git 管理外)。

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

### wgpu 30.0.2 と静止画 shader の DXBC 契約

`src/gpu_still_shaders.rs` の契約テスト `still_bytecode_contract_tracks_locked_hal_and_naga` は、wgpu・wgpu-hal・wgpu-core の版を
固定して、更新時に静止画 NIS / Anime4K VL の DXBC (§1.776 案 B-A) の前提を見直させる。独立レビュー (Sol / xhigh) の指摘で、
30.0.1 → 30.0.2 の registry のソースを比べた (設計担当)。

- wgpu-hal: 変わったのは `src/metal/surface.rs` だけ。`dx12/` (固定 layout の register 配置・`load_shader`・`compile_fxc`) は同一。
- wgpu-core: `Global`・`CommandEncoder`・`CommandBuffer` の Send / Sync の明示実装と、map 中に destroy した buffer の解放を drop まで遅らせる修正。shader・passthrough の経路は同一。
- wgpu: webgpu backend と lib.rs の版表記。
- 実行時の naga は 30.0.1 のまま (`naga-types` だけ 30.0.2)。ビルド時の `naga_wgpu = "=30.0.1"` も変えない。

したがって DXBC の生成条件は変わらない。テストの期待値を 30.0.2 にし、`build/still_image_shaders.rs` 冒頭と
`docs/display-pipeline.md` の版の記述を合わせた。WGSL と DXBC の GPU 上の一致テストは既存のものをそのまま使う。

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

## 検証

どれも設計担当が run-heavy 経由で流した (2026-11-20 の利用者の決定どおり、Codex にはコンパイル・テストを流させていない)。
製品のバイナリはエージェントが起動していない。

- 本体 lib 全件 (`.\scripts\test-lib-full.ps1`、NEXTEST_TEST_THREADS=12、MSBUILDDISABLENODEREUSE=1):
  - c01a6ab63 のソース: **19,007 件成功 / 0 失敗 / 88 skipped** (1819.7 秒)。実行中に記録文書だけを書き換えたので、
    同じソースの再利用の記録は残っていない。
  - その前の 20c48491d のソース: 19,006 件成功 / 1 失敗 (DXBC 契約テスト。上の wgpu の節で対応) / 88 skipped (1712.9 秒)。
  - 2026-11-20 00:34 に始めた 1 回目 (20c48491d、当時は同時実行の上限なし) は、途中で PC 全体が固まって中断した。
    原因は統括が live kernel dump で調べている (テストの実行ファイルの中での NTFS のデッドロック)。12 並列の 2 回は固まらずに完走した。
- `.\scripts\build-dev.ps1`: 成功。`target\dev-runtime\mimageviewer-core.exe` に 8165 の pdfium.dll のバイト列が入り、8138 のものは入っていないことを照合した。
- `cargo fmt --check`・FFmpeg の check のテスト・`scripts\test-check-dependency-updates.ps1`・bench・生成コードの照合: 上の各節。
- 独立レビュー (gpt-6.1-sol / xhigh、read-only): 1 回目 REJECT (P2 wgpu の契約テスト、P3 記録の参照先)、2 回目 ACCEPT。
- test-full は統括が master で流す (各ブランチでは流さない)。

## 未確認 (実機)

mIV の自動テストは PDF の描画結果の画素も EXR の画素も比べない。利用者の実機で次を見てもらう必要がある。

- PDF: サムネイル (表紙)、ページ送り・見開き・拡大、図形・グラデーション・半透明・クリップの多い PDF、スキャン画像の PDF
  (CCITT Fax / Flate の修正が入った)、パスワード付き PDF、数百ページの PDF の速いページ送り。
- EXR (exr 1.74.3 で SIMD の実装が変わった) の表示。mIV は EXR を対応形式に入れていない
  (`src/folder_tree.rs` の `SUPPORTED_EXTENSIONS` に無い) ので、利用者には影響しない
  (exr crate は `image` の既定の機能として入っているだけ。2026-11-21 に設計担当がコードで確認)。
- 静止画の NIS / Anime4K VL (DX12 の DXBC 経路) が従来どおり表示される。

## Cargo 1.100 の build-dir 配置 (取り込み後に判明)

Cargo 1.100 (2026-11-12、CHANGELOG #17354) で中間成果物の配置が変わった。
旧配置に戻す設定は無い。`rust-toolchain.toml` は 1.100.0 に固定している。
設計担当が run-heavy で次を確認した。

- テスト実行体は旧 `target\debug\deps\<name>-<hash>.exe` から
  `target\debug\build\<package>\<hash>\out\<name>-<hash>.exe` へ移った。
- build script の `OUT_DIR` も旧 `target\<profile>\build\<package>-<hash>\out` から
  `target\<profile>\build\<package>\<hash>\out` へ移った。実例は
  `C:\home\mimageviewer\target\debug\build\mimageviewer/30505735908c35d5\out`。
  観測した hash は16桁 hexで、区切りに `/` が混ざる。
- `cargo test` は `0xc0000135 STATUS_DLL_NOT_FOUND` で止まった。FFmpeg は import library
  経由で exe 読み込み時に DLL が必要で、従来の `target\debug\deps\` への手動コピーでは
  新しい実行体の隣に置けなくなった。`target\debug\` に置くと通ることを確認した。
- `cargo check -p mimageviewer-launcher` は build script の旧形の照合で
  `Launcher build cannot resolve inputs from OUT_DIR: OUT_DIR package directory is not mimageviewer-launcher-<hash>`
  と出て停止した。

nextest は `target\debug` と `deps` を PATH に加える
([env-vars 文書](https://nexte.st/docs/configuration/env-vars/)) ので、上記の本体 lib 全件は通っていた。
Cargo が PATH に加える base output directory も `target\debug` であり、テスト用 DLL はそこへ配置する。

ソース確認では、本体 `build.rs` の `MIV_SOUND_CANDIDATE` 分岐も
`out.ancestors().nth(3)` を profile 名とみなしていた。新配置では package を指すため、
debug / dev-runtime の候補検証を誤って拒否する。

対応では `build_output_paths.rs` を本体と launcher で共有し、package を明示して
新しい `<profile_dir>/build/<package>/<nonempty ASCII hex hash>/out` だけを受け付ける。
絶対パス・親参照なしを維持し、旧形・未知の形は想定形と Cargo 配置変更の可能性を示して停止する。
launcher の埋込入力は従来どおり profile の親の `release`、候補許可は debug / dev-runtime だけとする。

`scripts/setup-test-dlls.ps1` が FFmpeg の6 DLLを `<target>/<profile-dir>/` へ実体コピーする。
profile 名の対応は native runtime ID と共通化し、同じ内容なら書かず、1本でも不足なら停止する。
`deps/` は変更しない。全件 lib・focused・full テストと build-dev から呼び、全件 lib では
native runtime ID の計算より先に準備する。直接 `cargo test` を打つ場合の手順は
[開発ビルドとテスト](development-build-and-test.md#テスト前の-ffmpeg-dll-配置) を参照。

この対応のビルド・テスト・整形は設計担当へ引き渡す。実装担当は Cargo・rustc・nextest・
MSBuild・cmake・cargo fmt を実行せず、製品 exe も起動していない。
