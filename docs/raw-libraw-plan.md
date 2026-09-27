# RAW の LibRaw 対応 — 設計書

- 状態: **設計 第1版 (実装前の独立レビュー待ち)**。2026-09-27 起案 (ClaudeCode Opus 5.5)
- 作業場所: worktree `C:\home\mimageviewer-raw` / branch `raw-libraw` (master `edbac5f37` から分岐)
- 引き継ぎ元: [raw-libraw-handoff.md](raw-libraw-handoff.md)。本書が完成したら handoff の内容は本書へ吸収済みとして削除してよい
- 実装: Codex GPT-6 Sol / xhigh に段ごとに委任。独立レビュー: 実装者とは別の GPT-6 Sol / xhigh

本書の事実の出どころは 3 種類だけにする。**(a) コードの参照** (`path:line`、HEAD `d77081018`)、
**(b) LibRaw 0.22.2 の公式ソース / 文書** (§18 に URL)、**(c) 明示の未計測・未確認**。
実行時の所要時間・メモリ・見た目について、誰かが観測した値は現時点で **1 つも無い**。
数値はすべて「未計測」または「公式係数からの推定」と書く。

---

## 1. 目的と範囲

**目的**: カメラ RAW を Microsoft Store の「Raw 画像拡張機能」なしで表示できるようにする。
LibRaw を本体に静的リンクし、**RAW 拡張子は WIC を一切通らない**形でリリースする。
HEIC / HEIF / AVIF / JXL / TIFF は従来どおり WIC のまま。

**範囲に入れる**:
- 対象拡張子は現行の 15 種そのまま: `dng cr2 cr3 nef nrw arw srf sr2 raf orf rw2 pef ptx rwl iiq`
  (`src/wic_decoder.rs:64-71`)。**DNG も LibRaw へ移す** (「RAW は全部 LibRaw」で説明を一本化)
- 一覧サムネイル、フルスクリーン、ZIP 内 RAW、書き出し / コピー / 外部ツール、類似画像、
  mIV Remote、360 度・比較・分析などの周辺機能での RAW の扱い
- 配布・ライセンス通知・対応ソース・マニュアル / 製品ページ / spec (S4)

**範囲に入れない (v1 の非目標)**:
- 現像パラメータの利用者設定 (WB・露出・ハイライト等の UI)。v1 は固定パラメータ (§5.3.4)
- 対象拡張子の追加 (`crw srw 3fr erf kdc mrw mos mef x3f dcr` 等)。§17 の未決事項に置く
- フル現像結果のディスクキャッシュ。メモリ上だけ (§7.8)
- RAW の EXIF 表示の改善 (メタデータパネルは現状の rexif のまま)
- Lightroom 等の `.xmp` サイドカーの解釈

## 2. 利用者の決定事項 (確定)

1. S4 (配布・ライセンス・文書) まで揃えてからリリースする。master への統合は完成後に一度
2. フルスクリーンは **常に「埋め込みプレビュー → フル現像に差し替え」**
3. RAW フル現像の先読みは **先 2 枚・前 1 枚**
4. RAW フル現像の並列数は **既定 3、設定で 1〜10**
5. **編集機能 (補正 / AI アップスケール / 消しゴム / モザイク等) はフル現像完了まで待つ**。プレビューにはかけない

用語の固定: 本書の「先 / 前」は **表示順 (現在の一覧・読書順) での進行方向 / 逆方向**。
既存設定の `prefetch_forward` (既定 12) / `prefetch_back` (既定 4) (`src/settings.rs:4226-4230`,
`6666-6671`) と同じ向き。handoff の「後 4・前 12」はこの `back 4 / forward 12` を指す。

## 3. 現状のコード (確認済みの事実)

### 3.1 RAW の入口は 1 つではない

RAW を画素に変える入口が 11 系統、寸法だけを読む入口が 4 系統ある。すべて「image crate → WIC →
Susie」の順で試しており、**拡張子で RAW を先に振り分ける箇所は無い**。
`wic_decoder::is_wic_supported_extension` はテスト以外から呼ばれていない (`src/wic_decoder.rs:74-77`)。

| # | 入口 | 用途 | 入力 | 位置 |
| --- | --- | --- | --- | --- |
| D1 | `canonical_image_loader::decode_canonical_image` | フルスクリーン / Remote AI / 類似候補プレビュー | path / bytes (ZIP) | `src/canonical_image_loader.rs:349-499`, 呼び出し `src/app.rs:60070`, `src/remote_ipc/container.rs:6647`, `src/similar_preview.rs:1400,1439` |
| D2 | `thumb_loader::load_one_cached` + `decode_zip_chain` | 一覧サムネイル / Remote サムネイル / **Remote ページ** | path / bytes | `src/thumb_loader.rs:3105`, `928-961`, `3374-3401` |
| D3 | `thumb_loader::decode_image_for_thumb` | 動画 sidecar サムネ / 本の並べ替えサムネ | path | `src/thumb_loader.rs:568-571` |
| D4 | `thumb_loader::build_and_save_one(_zip)` / `cache_ops` の ZIP ループ | キャッシュ一括作成 | path / bytes | `src/thumb_loader.rs:5758-5819,5919`, `src/app/cache_ops.rs:407,473-585`。**image crate だけで WIC に到達しない (RAW は現状も失敗)** |
| D5 | `books::decode_file_color_image` / `decode_bytes_color_image` | 製本の焼き込み / Ctrl+E / 一括書き出し / 外部ツールの TempEdited | path / bytes | `src/books.rs:1280-1315`, 呼び出し `export_dialog.rs:945`, `export_batch.rs:195`, `materializer.rs:938` |
| D6 | `similar_image::decode_full` | 類似索引の proxy | path + bytes | `src/similar_image.rs:257-301` |
| D7 | `App::start_pano_high_res_load` | 360 度の高解像度 | path | `src/app.rs:72028-72037` |
| D8 | `context_menu::copy_image_to_clipboard` / `copy_zip_image_to_clipboard` | 画像をコピー (worker thread 上) | path / bytes | `src/ui_dialogs/context_menu.rs:2370-2446` |
| D9 | `app/cache_ops.rs:41-53` | サムネイル画質サンプル | path / bytes | image crate だけ |
| D10 | `remote-web` の `image_support::decode_oriented` / `probe_image` | Remote の旧 `/api/image` `/api/image-info` (remote-web プロセス内) | path | `crates/remote-web/src/image_support.rs:32-51`, `161-325`。**本体とは別の WIC 実装と拡張子一覧を持つ** |
| D11 | `bin/bench_dupe.rs::decode_full` | ベンチ | path / bytes | `src/bin/bench_dupe.rs:1985-2000` |
| P1〜P4 | `fast_resize::probe_dims(_from_bytes)` ほか | 寸法の事前読み (`DimsOnly` / 一括編集 / メタ / Remote) | path / bytes | `src/fast_resize.rs:215,225`, `src/app/metadata_ops.rs:852,1521`, `src/remote_ipc/container.rs:2164-2193` |

**危険な既存挙動 (コードからの推論、実物では未確認)**: image 0.25 は既定で TIFF decoder を持つ。
magic から形式を推測する経路 (`load_from_memory`, `with_guessed_format`) は、TIFF 構造の RAW
(DNG / CR2 / NEF / ARW / PEF …) の IFD0 (多くは小さな埋め込み画像) を「成功」として返し得る。
したがって **RAW の振り分けは、どの decoder も試す前に拡張子で行う**必要がある (§5.1)。

向き: RAW の向きは rexif で読めず WIC で読んでいる (`src/thumb_loader.rs:593-606`)。bytes 版は
無いため **ZIP 内 RAW は現状いつも向き 1** (`src/thumb_loader.rs:3296-3298`)。

拡張子一覧は静的で、WIC codec の有無を実行時に調べていない (`src/folder_tree.rs:86-94`,
`crates/remote-web/src/image_support.rs:6-10`)。製本のページ判定 `is_supported_book_image_path`
(`src/books.rs:2299-2318`) は RAW を含まない。同名 JPG がある RAW は `skip_duplicate_images`
(既定 ON、`src/settings.rs:7136`) で隠れる (`src/app/folder_scan.rs:970`)。これは維持する。

### 3.2 フルスクリーン読み込みの仕組み

- `start_fs_load_with_purpose` (`src/app.rs:59461`) が 1 ページ 1 thread を spawn し、App-global な
  `FsPageLoadScheduler` (総枠 6 / High 予約 2、`src/fs_page_load_scheduler.rs:12-13`) の permit を取ってから
  `decode_canonical_image` を呼ぶ。要求は viewer context ごとの `fs_pending`
  (`ItemsGenerationMap::with_discard`、`src/app.rs:16683`) が ticket ごと所有する
- 結果は `FsLoadResult` (`src/fs_animation.rs:25`) で返り、`poll_prefetch` (`src/app.rs:74032`) が
  `fs_upload_backlog` 経由で 1 フレーム「現在ページ + 1 件」だけ upload する
- Static を `fs_cache` へ入れるたびに `bump_input_generation_for_fs_cache_reload`
  (`src/app.rs:65164`) が `input_generation[idx]` を進め、消しゴム / 補正レイヤー / 隠蔽 / edit result /
  比較準備を失効させる。**差し替えの失効経路は既にある** (PDF の再レンダがこれを使っている)
- `FsCacheEntry` は `Static / Animated / Failed / Video` (`src/fs_animation.rs:80`)。
  **`Static` があれば、ほぼ全ての画素処理が自動で始まる** (edit result、`auto_apply_saved_mask`
  `src/app.rs:74476`、同期補正 `apply_sync_adjustment`、final composite、final AI、色検索、分析、
  パイプライン debug …)。「この画素は仮」という gate は PDF 用の `display_should_defer_final_ai`
  (`src/app.rs:69420-69437`) だけで、final AI しか止めない
- `fs_zoom` は fit 倍率への乗数、fit は **texture 寸法**から計算する (PDF と通過 rendition だけが
  `layout_source_size` を使う、`src/ui_fullscreen.rs:35830-35841`)。`Original` (100%) と
  拡大しない / 縮小しない制限は texel 数に依存する
- 編集 overlay の座標: 消しゴム / 隠蔽 / 補正レイヤーは raster の `pixels.size`、注釈と crop は
  `source_dims`。保存済みマスクは要求寸法へ拡縮して読む (`src/mask_db.rs:1444`)、補正レイヤーは
  寸法不一致時に resize (`src/app.rs:64478`)、crop は `source_size` 付きで拡縮 (`src/app.rs:63957-64017`)
- 編集モードの共通入口 `fullscreen_edit_mode_entry_allowed` (`src/ui_fullscreen.rs:34990`) は
  現在 360 度だけを拒否する。左パネルは `image_edit_tools_disabled_reason`
  (`src/ui_adjustment_panel.rs:9166`) の理由文字列で無効化する。crop 入口は共通入口を通らない
- 先読みの状況表示のドットは **AI 先読み専用** (`src/ui_fullscreen.rs:42649-42740`)。
  ページ読み込みのドットは無い
- 表示優先順位は `final_composite > edit_result > fs_cache > サムネイル` で、カラー化 / LUT が
  必要なページは `colorize_display_requires_final_effect` が生の `fs_cache` を出させない
  ([display-pipeline.md §2.3](display-pipeline.md))

### 3.3 Remote

- Remote のページ (`/api/page`) は canonical loader ではなく `thumb_loader::process_load_request`
  を通る (`src/remote_ipc/container.rs:479-514,5631`)
- Remote のページ生成は **保存済み編集 (消しゴム / 補正レイヤー / 隠蔽 / 注釈 / crop) と補正を
  画素へ適用する** (`src/remote_ipc/container.rs:834-1030`)
- Remote AI は canonical loader (`src/remote_ipc/container.rs:4402-4430`)

## 4. LibRaw の採用条件

| 項目 | 決定 | 根拠 |
| --- | --- | --- |
| 版 | **0.22.2** (2026-07-16 公開、bugfix-only) | GitHub release / Changelog.txt 1 行目 |
| ライセンス | **CDDL-1.0 を選択し、core へ静的リンク** | COPYRIGHT は LGPL-2.1 / CDDL-1.0 の選択制。CDDL §3.6 で Larger Work として結合可。義務は LibRaw 部分のソース提供の告知・ライセンス本文・notice 保持 (§14)。LGPL を選ぶと再リンク可能性のため DLL 分離が要る |
| GPL コード | 含まれない | README.demosaic-packs「GPL packs are abandoned」、0.22.2 の src/ に Lesser 以外の GPL 表記なし (調査時の grep) |
| ビルド | 新 crate `crates/libraw-sys` の build.rs が `cc` で C++ を直接コンパイル | 公式の CMake は無い (README.cmake: 2014 以降非サポート、LibRaw-cmake は unmaintained)。Makefile.msvc の `LIB_OBJECTS` をファイル一覧の正本にする |
| CRT | `/MT` (静的) | `.cargo/config.toml` の `+crt-static` を `cc` が自動で `/MT` にする (cc 1.2.67 `src/lib.rs:2115,2412`)。現 core は VC runtime DLL を import しておらず、この状態を保つ |
| OpenMP | **使わない** | MSVC の `/openmp` は `vcomp140.dll` を要し、同梱物にも `check-vcrt-pe-dependencies.ps1` の検出対象にも無い。ファイル単位で並列化する (§5.4) ので、ファイル内の並列は過剰になる。代償は CR3 / Fuji 圧縮 / AHD の単ファイル速度 (未計測) |
| 定義 | `LIBRAW_NODLL LIBRAW_BUILDLIB USE_ZLIB`、`USE_JPEG` は S1 で判断、`USE_X3FTOOLS` は付けない | `USE_ZLIB` は deflate DNG に必須 (`src/decoders/fp_dng.cpp`)。`USE_JPEG` は lossy DNG (34892) と一部 Kodak に必須 (`src/metadata/identify.cpp:1269-1275`)。X3F は対象拡張子に無い |
| 既存の Rust crate | 使わない | `rsraw-sys` は MSVC で panic、`libraw-rs` は 0.20.1 で 2021 年停止、どれも 0.22 を同梱しない (crates.io、2026-09-27 時点) |

**新しい PE が増えない**。静的リンクなので launcher の埋め込み一覧、ポータブルの同梱一覧、署名対象、
VC runtime gate は変わらない (core が大きくなるだけ)。S4 はライセンスと文書が中心になる。

### 4.1 zlib と libjpeg (S1 のスパイクで決める)

- **zlib**: workspace に C の zlib は無い (`flate2` + `miniz_oxide` だけ、`Cargo.lock:2209,4087`)。
  候補は `libz-sys` (static、cc で zlib を同梱ビルド) か、C ABI を出す `libz-rs-sys`。S1 で
  リンクできることと deflate DNG サンプルの現像を確認して決める
- **libjpeg (`USE_JPEG`)**: `turbojpeg-sys` 1.2.0 が libjpeg-turbo を static でビルドしている
  (`Cargo.lock:7062-7083`)。その static lib が `jpeg_mem_src` を含む libjpeg API を公開し、ヘッダへ
  build.rs から到達できるなら再利用する。重複シンボル・ヘッダ不達なら `USE_JPEG` を付けず、
  lossy DNG は **型付きの「非対応形式」** として扱う (プレビューだけ表示、§5.3.3)。
  黙って別経路へ落とさない

## 5. 全体構造

```
            ┌─────────── 拡張子の単一所有者 ───────────┐
            │ raw_format::is_raw_ext / RAW_EXTENSIONS   │  ← 全入口 D1〜D11, P1〜P4 が最初に問う
            └───────────────┬───────────────────────────┘
                            │ RAW
   ┌────────────────────────▼────────────────────────┐
   │ raw_decoder (safe Rust)                          │
   │  info()     : 開くだけ (寸法・向き・プレビュー一覧・非対応判定)  │
   │  preview()  : 埋め込みプレビュー (安い、permit 不要)           │
   │  develop()  : 現像 (full / half)、RawDevelopPermit が引数に必須 │
   └────────────────────────┬────────────────────────┘
                            │ extern "C" (自前の狭い ABI)
   ┌────────────────────────▼────────────────────────┐
   │ crates/libraw-sys : shim (C++) + LibRaw 0.22.2 静的  │
   └──────────────────────────────────────────────────┘

   RawDevelopScheduler (App-global、Arc) : 現像の実行枠 = 設定値 1〜10 (既定 3)
     ├ fullscreen の現像要求 (viewer context ごとの raw_develop_pending が ticket を所有)
     ├ 書き出し / コピー / 外部ツール / Remote AI / (Remote ページ) の現像
     └ プレビューの無い RAW のサムネイル用 half 現像、類似索引の half 現像
```

### 5.1 拡張子の単一所有と WIC 境界での拒否

- 新モジュール `src/raw_format.rs` が `RAW_EXTENSIONS` と `is_raw_ext(&str)` / `is_raw_path(&Path)` を
  所有する。`folder_tree::SUPPORTED_EXTENSIONS` と `wic_decoder::WIC_SUPPORTED_EXTENSIONS` はこの一覧を
  参照して組み立て、**WIC 側の一覧から RAW を外す**
- **WIC の境界自体が RAW を拒否する**。`wic_decoder::decode_to_dynamic_image(path)` と
  `read_wic_orientation(path)` は RAW 拡張子なら WIC を呼ばずに `None` を返す (debug build では
  `debug_assert!` で呼び出し側の振り分け漏れを検出)。bytes 版
  `decode_to_dynamic_image_from_bytes` は拡張子を知らないので、**引数に拡張子 (または
  `WicSourceHint`) を必須にする**シグネチャ変更を行い、同じく拒否する。
  これで「WIC を一切通らない」を入口ごとの注意ではなく WIC 側の不変条件にする
- 各入口 D1〜D11 と P1〜P4 は、**どの decoder も試す前に** `is_raw_ext` を問い、RAW なら
  `raw_decoder` へ行く (image crate の TIFF 誤成功を避けるため)。失敗時に image crate / WIC / Susie へ
  落とさない (型付きエラーで終端)
- `crates/remote-web` は本体 crate に依存できない。remote-web の `SUPPORTED_IMAGE_EXTENSIONS` から
  RAW を外すかどうかは §10.3。本体側に「本体の `RAW_EXTENSIONS` と remote-web の一覧の関係」を
  固定する unit test を置き、片側だけの変更を検出する

### 5.2 `crates/libraw-sys` と shim

- LibRaw のソースは `vendor/libraw/` (gitignore、他の vendor と同じ扱い)。
  `scripts/setup-libraw.sh` が公式 tarball `LibRaw-0.22.2.tar.gz` を **sha256 固定**で取得・展開し、
  tarball 自体も `vendor/libraw/` に残す (対応ソースとして配布するため、§14)。
  `check` モードは GitHub の最新 release tag と比較する (`setup-pdfium.sh check` と同型)
- LibRaw の C API ではなく、**自前の狭い C ABI** を C++ の shim (`crates/libraw-sys/shim/miv_libraw.cpp`)
  で定義し、Rust 側は手書きの `extern "C"` 宣言だけにする (bindgen は使わない)。理由:
  - wide path open (`open_file(const wchar_t*)`) と `open_datastream` / 中断フラグは C++ API にしかない
  - C++ 例外を FFI 境界の外へ出さない (`try { … } catch (...)` で全関数を包み、エラーコードへ)
  - 公開する面を必要最小にして、LibRaw の構造体 layout を Rust に写さない
- shim の関数 (案): `miv_raw_open_path(wchar*)` / `miv_raw_open_buffer(ptr,len)` / `miv_raw_info` /
  `miv_raw_preview_count` / `miv_raw_preview_info(i)` / `miv_raw_preview_extract(i)` /
  `miv_raw_develop(params, progress_cb, user)` / `miv_raw_copy_rgb(buf, stride)` / `miv_raw_free_*` /
  `miv_raw_close`。shim は LibRaw のソースを改変しない独立ファイルで、MIT とする
- build.rs: `CARGO_CFG_TARGET_OS != "windows"` なら何もコンパイルしない (ubuntu CI の `cargo check`
  で C++ を要求しない、`.github/workflows/ci.yml`)。Windows で `vendor/libraw` が無ければ
  ルート build.rs と同じ形式の枠付きメッセージ + 復旧手順 (`bash scripts/setup-libraw.sh` /
  `bootstrap-vendor.sh`) で止める
- Rust 側の非 Windows 実装は「非対応」を返す stub (既存 `wic_decoder` の非 Windows と同型)

### 5.3 `raw_decoder` (safe API)

#### 5.3.1 型

```rust
pub enum RawSource<'a> { Path(&'a Path), Bytes(&'a [u8]) }   // ZIP 内は Bytes (open_buffer)

pub struct RawInfo {
    pub developed_dims: [u32; 2],   // フル現像が返す寸法 (向き適用後)。§5.3.2
    pub flip: RawFlip,              // LibRaw sizes.flip
    pub previews: Vec<RawPreviewInfo>, // thumbs_list 由来。format / 寸法 / tflip
    pub develop_support: RawDevelopSupport, // Supported | Unsupported(RawUnsupportedReason)
    pub make_model: Option<String>, // 診断用
}

pub struct RawPreview { pub image: DynamicImage /* 向き適用済み */, pub info: RawPreviewInfo }

pub enum RawDevelopScale { Full, Half }

pub enum RawError {
    Io(..), Corrupt(..), Unsupported(RawUnsupportedReason), NoUsablePreview,
    OutOfMemory, Cancelled, TooLarge, Internal(i32 /* LibRaw error code */),
}
```

`info()` と `preview()` は open (ヘッダ解析) とプレビュー部分の読み出しだけで、画素の現像をしない。
`develop()` は `&RawDevelopPermit` を引数に取り、**permit を持たずに現像できない** (§5.4 を型で強制)。

#### 5.3.2 寸法と向き

- `developed_dims` は `adjust_sizes_info_only()` (LibRaw API、Fuji SuperCCD の回転・`half_size`・flip を
  含めた出力寸法を計算する) で求める。公式文書は「この変更は繰り返せない」と書いている
  (API-CXX.html `adjust_sizes_info_only`) ので、`info()` は現像と別の LibRaw instance / 別の open で行い、
  同じ instance で続けて `dcraw_process` しない。**S1 の受入条件**: 全サンプルで
  `develop(Full).dimensions() == info.developed_dims` を unit test で確認する。一致しない形式が
  あれば設計へ戻す (表示の差し替えと編集座標がこの値に依存するため)
- フル現像の出力 (`dcraw_make_mem_image` / `copy_mem_image`) は **flip 適用済み**
  (`src/postprocessing/mem_image.cpp:153-230`)。追加で回転しない
- 埋め込みプレビューは **flip 未適用** (`src/decoders/unpack_thumb.cpp` に flip 処理が無い)。
  `unpack_thumb_ex(i)` の生バッファを使い、`tflip` が既知 (≠0xffff) ならそれ、不明なら `sizes.flip` を
  自前で適用する。JPEG 内の EXIF Orientation は**読まない** (`dcraw_make_mem_thumb` は EXIF の無い
  JPEG に向きを挿入するため、そちらを使うと二重適用の経路が生まれる)。向きの正しさは縦位置サンプル
  (flip 5 / 6) の unit test で固定する
- ZIP 内 RAW も bytes から同じ向きを得る (現状の「ZIP 内 RAW は向き 1」は解消される)

#### 5.3.3 使えるプレビューの定義

`thumbs_list` から、形式が **JPEG か BITMAP** のものの中で最大のものを選ぶ。H.265 (CR3 の HEIF
プレビュー)、JPEG XL、LAYER、ROLLEI は選ばない。選んだものの長辺が
**`RAW_PREVIEW_MIN_LONG_EDGE = 1024`** 未満なら「使えるプレビューが無い」とする。
この判定はファイル内容だけで決まり、実行時の状態に依存しない。

- JPEG のデコードは既存の TurboJPEG 経路を bytes で使う (サムネイル用途では DCT スケールも効く)
- 非対応形式 (`get_decoder_info().decoder_flags & LIBRAW_DECODER_UNSUPPORTED_FORMAT`、例: Nikon HE/HE*、
  JPEG XL DNG) は `info.develop_support = Unsupported` として open 直後に分かる。プレビューがあれば
  **プレビューだけを表示し、現像を要求しない** (§7.4 のメッセージを出す)

#### 5.3.4 現像パラメータ (v1 は固定)

| param | 値 | 理由 |
| --- | --- | --- |
| `use_camera_wb` | 1 | カメラの WB。非 DNG では camera matrix も有効になる (`use_camera_matrix=1` の規則) |
| `output_color` | 1 (sRGB) | 表示・編集パイプラインは sRGB 8bit |
| `output_bps` | 8 | 同上。`ColorImage` は 8bit |
| `gamm` | `{1/2.4, 12.92}` | sRGB 曲線 (LibRaw 既定は BT.709) |
| `highlight` | 0 (clip) | カメラ JPEG に近い |
| `user_qual` | -1 (AHD。Fuji SuperCCD は PPG、X-Trans は Markesteijn) | 既定。AHD は中断の応答が最も良い (§5.4.3) |
| 明るさ | **S1 で決める** | 候補: 自動 (thr 0.01 = LibRaw 既定) / 自動 (thr 0.001 = 公式文書の推奨範囲) / 自動なし。サンプル全件でプレビューとの平均輝度差を測り、利用者の目視で決める。数値は未計測 |
| `half_size` | 用途で指定 | サムネイル・類似索引の代替だけ |

LibRaw の現像はカメラ JPEG と色・トーンが一致しない (README: production-quality rendering ではない、
ピクチャースタイルのトーンカーブもレンズ補正も無い)。**プレビュー → フル現像の差し替えで色は変わる。**
これは決定事項 2 が受け入れている差であり、§7.3 の R2 (白黒 → カラーを見せない) とは別の話として扱う。

### 5.4 `RawDevelopScheduler` (現像の実行枠)

`FsPageLoadScheduler` と同じ `Mutex + Condvar` + RAII permit の構造で、App-global に 1 つ置く
(detached を増やしても実行予算は増えない)。**try_lock + sleep は使わない。**

#### 5.4.1 枠と優先度

- 総枠 `N = settings.raw_develop_parallelism` (1〜10、既定 3)。設定変更は即時に上限だけ変え、
  実行中の現像は止めない
- 優先度 3 段:
  - `High`: 表示中のページ (現在ページと見開き相方)、利用者が今待っている書き出し / コピー /
    外部ツール、Remote の前景
  - `Normal`: フルスクリーンの先読み現像、プレビューの無い RAW のサムネイル用 half 現像、Remote の先読み
  - `Background`: 類似索引の half 現像
- `Normal + Background` の同時実行は `max(N - 1, 1)`、`Background` は最大 1。`N = 1` のときは
  High 予約が作れないので、High は待機列の先頭に並ぶだけ (実行中の Normal は止めない)
- 待機中の要求は優先度順 → 受付順。先読みが表示対象になったら同じ ticket を High へ昇格する
  (取消 + 再投入はしない)。`FsPageLoadScheduler::promote_to_high` と同型

#### 5.4.2 取消

- 待機中の取消は即座に列から消える。実行中の取消は `Cancelling` として **worker が実際に終わるまで
  枠を占有する** (取消した現像を新しい現像が追い越して実行数が N を超えないように。
  `fs_page_load_scheduler.rs:383` と同じ規約)
- 取消の伝え方は shim の progress callback の戻り値 (非 0 で `LIBRAW_CANCELLED_BY_CALLBACK`)。
  callback は Rust の `AtomicBool` を読むだけ

#### 5.4.3 中断の遅延 (公式ソースからの事実。時間は未計測)

- AHD は 512 行の帯ごとに callback があり、帯の途中で止まる (`src/demosaic/ahd_demosaic.cpp:293-355`)
- `unpack()` は開始と終了の 2 回しか callback しない (`src/decoders/unpack.cpp:32,513`)。
  中断フラグ (`setCancelFlag`) を見る decoder もあるが、**CR3 (`crx.cpp`)・Fuji 圧縮・Panasonic v8 は
  見ない**。これらは読み込みの途中では止まらない
- DCB / DHT / AAHD / X-Trans の demosaic には callback が無い。X-Trans (Fuji) は demosaic 中に止まらない

したがって「ページ送りで途中中断」は **形式によって遅れる**。遅れは `Cancelling` の枠占有として
正直に見え、perf イベントで計測できるようにする (§12)。中断を速く見せるための時間窓や
先行解放は入れない。

#### 5.4.4 進捗

- shim の progress callback が `(stage, iteration, expected)` を Rust へ渡し、Rust 側で 0〜100 の
  単調増加値へ写す (open/identify 0〜5、LOAD_RAW 5〜35、INTERPOLATE の帯 35〜85、残り 85〜100。
  区切りは S1 で実測して調整)。`unpack` は 2 回しか報告しないので、その区間は「読み込み中」と表示し
  数値を動かさない
- 値は要求ごとの `Arc<AtomicU8>` (latest-value)。**channel に流さない**
  ([async-architecture.md §5.5.1](async-architecture.md))。UI は描画時に読むだけ

#### 5.4.5 メモリ

並列数は固定の設定値で決め、**実行時の空きメモリで変えない** (プロジェクト方針)。高い設定値での
メモリ不足は許容し、LibRaw のメモリ確保失敗は `RawError::OutOfMemory` として型付きで返す
(プロセスを落とさない)。`imgdata.rawparams.max_raw_memory_mb` は LibRaw 既定 (2048) のまま。

## 6. 表示段階の型

### 6.1 `FsCacheEntry::RawPreview`

`fs_cache` に RAW 専用の variant を足す。**`Static` にはしない。**

```rust
FsCacheEntry::RawPreview {
    preview: Option<RawPreviewTexture>, // tex + pixels + preview_dims。使えるプレビューが無ければ None
    developed_dims: [usize; 2],         // フル現像の寸法 (向き適用後)。layout と編集座標の正本
    develop_blocked: Option<RawDevelopBlocked>, // Unsupported(..) | Failed(RawError) の終端。None = 未現像
    load_seq: u64,
}
```

理由: 既存の画素処理はほぼすべて `FsCacheEntry::Static` への一致で始まる (§3.2)。プレビューを
別 variant にすれば、それらは **プレビューに対して始まらない**。「Static だが仮」という属性を足すと、
すべての consumer にその属性の確認を足す必要があり、1 箇所でも漏れると決定事項 5 が破れる。
variant を分ければ漏れは「`_ =>` で Static 以外を受けている箇所」に限られ、S3 でその箇所を列挙して
点検する (§7.7)。

フル現像の結果は通常の `FsLoadResult::Static` として同じ upload 経路を通り、`RawPreview` を
`Static` で**置き換える**。これで `bump_input_generation_for_fs_cache_reload`、比較準備の失効、
`FinalEffectSourceReload` の holdover 捕捉、`auto_apply_saved_mask` がすべて既存どおり動く。

### 6.2 読み込み状態

`FsPageLoadState::DisplayReady` の出どころに `RawPreviewOnly` を足す:
`DisplayReady(LiveCache | RetainedPdfFinalAi | RawPreviewOnly)`。

- `waiting_for_display()` は `RawPreviewOnly` で false (プレビューは「表示できる」)。したがって
  `ensure_fs_page_load` はプレビューを再要求しない
- ページ送りの navigation sequence は、プレビュー (またはサムネイル) の提示で settle する。
  **sequence がフル現像を待つことは無い** ([display-pipeline.md §2.5.2](display-pipeline.md) の
  循環待ちを作らない)
- `RawPreview { preview: None }` のページは表示候補が無いので、描画は既存の fallback (色忠実
  rendition → サムネイル → 「読込中」) に落ちる

### 6.3 現像要求の所有

`ViewerContextBundle` に `raw_develop_pending: ItemsGenerationMap<RawDevelopPendingValue>` を足す
(`with_discard` で ticket を cancel)。`fs_pending` と同じく:

- mount / park / activate の `swap_field!` 一覧、`set_items_generation` の retag に含める
- 世代切替・clear・retain drop・置換で ticket を cancel する
- 結果は同じ context の `fs_upload_backlog` へ `FsLoadResult::Static` として入る。sibling context の
  結果を消化しない

`RawDevelopPendingValue { ticket, rx, progress: Arc<AtomicU8>, load_seq }`。

1 ページに対して「プレビュー用の `fs_pending`」と「現像用の `raw_develop_pending`」が同時に存在し得る。
両者は別の scheduler の permit を取るので、互いの枠を塞がない。現像の結果が先に届いても
(プレビューより速いことは通常無いが)、upload は Static を入れるだけで正しい。

これは detached 専用の bool / Option ではなく、既存の context-owned resource (`fs_pending` と同型) の
追加である。detached 憲法 §2-3 には当たらない。ただし context 切替をまたぐ回帰テスト (§15) を必須にし、
[detached-rework-plan.md](detached-rework-plan.md) §11 に「context-owned resource を 1 つ追加した」と
記録する。

## 7. フルスクリーン

### 7.1 lifecycle

| 事象 | 起きること |
| --- | --- |
| ページを開く / 先読み窓に入る | `start_fs_load` が RAW を検出し、canonical loader を `RawStage::Preview` で呼ぶ worker を既存 `FsPageLoadScheduler` の permit で走らせる。`info()` の `developed_dims` を `DimsOnly` として先に送る (header probe の代わり、§9 P1) |
| プレビュー到着 | `FsCacheEntry::RawPreview` を upload。`record_fs_cache_page_dims_for_spread` には `developed_dims` を渡す (見開きの縦横判定がプレビューの寸法でぶれない) |
| 現像窓に入る (§7.2) | `ensure_raw_develop(idx)` が `raw_develop_pending` に ticket を作り、worker は `RawDevelopScheduler` の permit を取ってから `develop(Full)` |
| 現像完了 | `FsLoadResult::Static { source_dims = developed_dims }` を既存の後処理 (`into_gpu_raster` の 8192 clamp、360 度の tee) に通して upload。`RawPreview` を置換 |
| 現像失敗 / 非対応 | `RawPreview.develop_blocked` に終端を書く。再要求しない (再訪時も同じ items 世代なら再試行しない)。取消は失敗ではない (窓に戻れば再要求) |
| 現像窓を出る | 待機中・実行中の現像を cancel。**完了済みの Static は keep set から外れるまで残す** (既存の JPEG と同じ扱い、§7.8) |
| ページ送り (held) | 現像要求は `FsPageTurnWorkAdmission::All` のときだけ開始する (先読みと同じ扱い)。sequence 中は開始しない |
| fullscreen を閉じる / フォルダ移動 | `raw_develop_pending` の discard hook で全 ticket を cancel |
| 別の viewer context へ切替 | bundle swap で `raw_develop_pending` も一緒に交換。切り替え前の context の現像は継続し、その context の backlog へ入る |

### 7.2 現像の窓

- 対象: 表示順 (`collect_image_indices()`、`src/ui_fullscreen.rs:27291`) で **現在ページ・見開き相方・
  先 2・前 1**。順序は既存の `interleaved_prefetch_targets` (`src/app/prefetch_policy.rs:161`) と同じ
  「+1, −1, +2」。現在ページと相方は High、他は Normal
- 窓は定数 (`RAW_DEVELOP_FORWARD = 2` / `RAW_DEVELOP_BACK = 1`)。設定にしない (決定事項 3)
- プレビューの窓は既存の `prefetch_back` / `prefetch_forward` のまま
- 既存の「現在ページが読み込み中なら他の先読みを全部 cancel」(`src/app.rs:60623-60667`) は
  `FsPageLoadScheduler` の話であり、現像の窓には適用しない。現在ページが `RawPreviewOnly` なら
  「読み込み中」ではない (§6.2)
- AI の先読み (`prefetch_final_ai`) は Static を要求するので、RAW では自然に現像窓の中だけになる。
  先読み AI のドット表示が「永久に未着」を出さないよう、AI 先読み対象の RAW ページを現像窓で
  切り詰める

### 7.3 色の扱い (R2)

- `RawPreview` の表示は、既存の表示優先順位の **`fs_cache` 段** (生デコード結果の段) に置く。
  補正・AI・カラー化・LUT はプレビューに一切かけない (決定事項 5)
- カラー化 / LUT が有効で `colorize_display_requires_final_effect(idx)` が真のページでは、既存どおり
  生の `fs_cache` 段を出さず、**サムネイルから作る色忠実 rendition** へ落ちる。これは今日すべての
  画像で起きていることで、RAW 専用の処理ではない。プレビュー自体には何も適用しない
  ([display-pipeline.md §2.5.1.1](display-pipeline.md) R2、利用者の確定要件)
- 近モノクロ判定 (`MonochromeOnly`) は `edit_result_cache` の画素を見る。フル現像までは memo miss なので
  安全側の「待つ」になる (既存規約のまま)
- プレビュー (カメラ JPEG) → フル現像 (LibRaw) の差し替えでは色・トーンが変わる (§5.3.4)。
  これは決定事項 2 が受け入れている差として扱い、時間窓や cross-fade で隠さない

### 7.4 編集の gate

- `fullscreen_edit_mode_entry_allowed` に型付きの拒否理由 `RawDevelopmentPending` /
  `RawDevelopmentUnavailable` を足す。消しゴム・補正レイヤー・隠蔽・注釈・SNS 分割のキー入口と
  左パネルのボタンは既にこの入口と `image_edit_tools_disabled_reason` を通るので、両方へ同じ述語を渡す
- **crop の入口 (`enter_export_crop_mode`、`src/ui_crop.rs:215`) は共通入口を通っていない**。
  RAW の crop も決定事項 5 の対象とし、共通入口を通すよう揃える (crop 自体の挙動は変えない)
- 補正スライダー・AI モデル選択は「設定を書く」操作であり、プレビュー段でも受け付ける。
  適用 (final composite) は Static が来るまで始まらないので、決定事項 5 は満たされる
- 拒否時の表示: 既存の no-op 表示 (`FsNavNoOpReason`) を使い「RAW の現像が終わると編集できます」
  / 非対応形式では「この RAW 形式は現像に対応していません (埋め込みプレビューを表示中)」。
  待ってから自動で入る (conceal の EnterMode 型の継続) は v1 では作らない

### 7.5 差し替えで表示を飛ばさない

- `RawPreview` を描くときは、texture 寸法ではなく **`developed_dims` を layout 寸法**にする。
  既存の通過 rendition と PDF が使う `layout_source_size` / `resolve_fs_transform_in_layout_rect`
  (`src/ui_fullscreen.rs:35830-35841`) と同じ仕組みで、プレビュー texture は `developed_dims` の
  包含枠の中へ contain する
- これで fit (Page / Width / Height)、`fs_zoom` の乗数、`fs_pan`、Z ズーム、`Original` (100%) の
  基準がプレビュー段とフル現像段で同じになる。`Original` はプレビュー段では `developed_dims` 基準の
  100% を指し、プレビューは拡大されて見える (解像度は変わってよい、
  [display-pipeline.md §2.5.1.1](display-pipeline.md))
- プレビューと現像の縦横比がわずかに違う機種がある場合 (未確認)、contain による余白の差が差し替え時に
  出る。S1 でサンプル全件の縦横比差を表にし、許容できない差があれば設計へ戻す
- `DisplayedImageTransform` の `source_size` は常に `developed_dims`
  (`source_dims_for_idx` が `RawPreview.developed_dims` を返す)。注釈・crop の座標は最初から正しい

### 7.6 進捗表示

- 左下の読み込みラベル (`src/ui_fullscreen.rs:24033-24066`、「読込中...」/「PDF 再レンダリング中...」)
  と同じ場所に、現在ページの状態を出す: `RAW 現像待ち` (permit 待ち) / `RAW 現像中 NN%` /
  `RAW 読み込み中` (unpack 区間) / 非対応・失敗のメッセージ
- 既存の先読み状況の行 (`draw_fs_prefetch_status_row`、AI 用) と同じ部品・同じ設定
  (`fullscreen_prefetch_status_visible`) で、**「RAW 現像」行**を足す。ドットは現像窓の
  各ページの `Ready / Active / Missing` (既存の `FsPrefetchPageState`)

### 7.7 `fs_cache` の画素を読む consumer の扱い

`RawPreview` を新設するので、以下を S3 で 1 件ずつ確認する。**Static 限定で始まるものは変更不要**
(プレビューでは始まらない) で、表示・幾何の consumer だけを `RawPreview` 対応にする。

| consumer | 位置 | RAW プレビュー段での扱い |
| --- | --- | --- |
| 表示優先順位 `resolve_fs_processed_texture` | ui_fullscreen.rs | fs_cache 段に RawPreview を含める (§7.3) |
| 元画像ホールド | ui_fullscreen.rs:10516,10526 | プレビューを出してよい (表示だけ) |
| ルーペ | ui_fullscreen.rs:39806 | 表示 texture を使うのでプレビューで可 |
| 自動余白カットの bbox | ui_fullscreen.rs:35594 | プレビュー画素から計算してよい (正規化 bbox)。キーは `(load_seq, Arc ptr)` なので現像後に再計算される |
| 見開きの寸法記録 | ui_fullscreen.rs:16304 | `developed_dims` を記録 |
| edit result / 消しゴム / 補正レイヤー / 隠蔽 / `auto_apply_saved_mask` / 同期補正 / final composite / final AI / AI 先読み | app.rs 各所 | Static 限定。変更不要 (始まらない) |
| 書き出し / コピー / 比較 pin (capture 経由) | ui_fullscreen.rs:44518 | `complete` な final composite を要求するので自然に待つ。案内文「最終合成の完了後に再実行してください」はそのまま |
| 比較 pin (グリッド選択から) | ui_fullscreen.rs:43838, app.rs:35391 | Static / Animated だけ受ける。RawPreview なら現像を High で要求して `compare_pin_load_pending` を継続 |
| 360 度 | app.rs:71857,72832 | Static の `source_dims` で判定。現像後に判定される。高解像度 tee は現像結果にも適用 (D7 は §9) |
| 分析パネル / ヒストグラム | ui_fullscreen.rs:24088 | Static 限定。プレビュー段は「RAW の現像待ち」と表示 |
| 色検索のパレット | app/color_filter.rs:106 | Static 限定。キャッシュはファイル identity なので、プレビューから抽出しないことが重要 (変更不要を確認) |
| パイプライン debug 出力 | pipeline_debug.rs:284,661,697 | Static 限定 |
| `fs_static_has_alpha` / 縮小警告 / AI トースト / `poll_fs_nav_lock` | app.rs:61518 ほか | S3 で個別確認 (`_ =>` 分岐があれば列挙) |
| Ctrl+E 書き出しダイアログ | export_dialog.rs:960 | 自前で decode する (D5)。現像は §9 |

`FsCacheEntry` を match している箇所のうち `_ =>` / `..` で Static 以外をまとめて受けている箇所を
S3 の最初に grep で全列挙し、本表に追記してから実装する。

### 7.8 メモリ上のキャッシュ

- フル現像の結果は `fs_cache` の Static としてだけ持つ。ディスクには保存しない
- 現像窓を出ても、keep set (`prefetch_back` / `prefetch_forward`) の中なら Static は残る。これは
  同じ寸法の JPEG と同じ扱いで、RAW だけプレビューへ降格して解放する処理は作らない
- fullscreen を閉じて開き直すと、現像はやり直しになる (保持 LRU は v1 では作らない。§17)

## 8. サムネイル

- D2 (`load_one_cached` / `decode_zip_chain`) で RAW を最初に振り分け、`preview()` を使う。JPEG の
  プレビューは既存の TurboJPEG DCT スケール経路へ bytes で渡す。向きは §5.3.2
- 使えるプレビューが無い RAW は `develop(Half)` を `RawDevelopScheduler` の Normal で行う
  (サムネイル worker は permit 待ちでブロックする。件数はフォルダ内の該当ファイル数だけ)
- 報告する `source_dims` は `developed_dims`。これで一覧の縦横比・見開き判定・詳細表示の解像度列が
  フル現像と一致する
- `CacheDecision::should_cache` に RAW 専用の規則は足さない (RAW はファイルが大きく既存の
  `size_threshold` に掛かる見込み。未確認)
- キャッシュ一括作成 (D4)・画質サンプル (D9) は現状 RAW を扱えていない。同じ振り分けを入れて
  サムネイルと同じ結果にする
- 類似索引への prefill (`src/thumb_loader.rs:3512-3520`) は、`Other` 形式では「decode 長辺 ≥ source_dims
  長辺」でないと拒否される (`src/similar_index.rs:6722-6737`)。§9 の D6 の方針に合わせて、RAW 用の
  形式区分を足し、プレビューからの prefill を受け入れる

## 9. その他の入口

| # | 方針 |
| --- | --- |
| D1 canonical loader | `CanonicalDecodeOptions` に **既定値の無い** `raw_stage: RawStage::{Preview, Full(&RawDevelopPermit)}` を足す。Preview は `CanonicalDecodeResult::RawPreview { preview: Option<..>, developed_dims, develop_support }` を返す。Full は `Static { source_dims = developed_dims }`。fullscreen の先読み / 表示は Preview、現像 worker と Remote AI は Full、類似候補プレビューは Preview (比較用の表示なので) |
| D3 | Preview (サムネイル用途) |
| D4 / D9 | サムネイルと同じ (§8) |
| D5 書き出し / 製本焼き込み / 外部ツール | Full 現像 (High)。書き出しは `SrcFormat::Other` でメタデータ転記を拒否している (`src/save_with_metadata.rs:40-45,200`) ので、RAW からの EXIF 転記は現状どおり無し |
| D6 類似索引 | プレビュー (無ければ Half 現像を Background で)。**フル現像はしない** |
| D7 360 度高解像度 | Full 現像 (High)。RAW の 360 度は稀なので、既存経路へ Full を差すだけ |
| D8 画像コピー | Full 現像 (High)。既に worker thread 上 |
| D10 remote-web 旧経路 | §10.3 |
| D11 bench | Full 現像。permit はベンチ内で自前の scheduler を作る |
| P1 `DimsOnly` | RAW は header probe を使わず `info().developed_dims` を送る |
| P2〜P4 寸法の事前読み | RAW は `info().developed_dims`。image crate の probe に RAW を渡さない |

製本への追加は §17 の未決事項 (RAW を本にどう入れるか)。

## 10. mIV Remote

### 10.1 サムネイル

本体と同じ D2 経路なので、§8 がそのまま効く。

### 10.2 ページ (`/api/page`) — **利用者判断が要る**

Remote のページ生成は保存済み編集と補正を画素へ適用する (§3.3)。選択肢:

- **A: フル現像を使う** (現像 permit を High / Normal で取得)。PC の表示・編集結果と一致し、
  決定事項 5 と矛盾しない。代償は 1 ページ数秒 (未計測) と、Remote の page worker が permit を
  待つ間 heavy queue の worker を占有すること。HTTP worker 枯渇
  ([web-remote-plan.md §9.5](web-remote-plan.md)) を避けるため、Remote の admission に
  「現像待ちの job は worker を占有しない」形 (permit 取得を別 thread にし、完了で job を再投入する等)
  を設計する必要がある
- **B: 埋め込みプレビューを使う** (無ければ Half 現像)。速い。端末の画質 (1024〜8192) に対し、
  多くのカメラのプレビューは十分な寸法を持つ (未確認)。ただし **プレビューへ保存済み編集と補正を
  かけることになり**、決定事項 5 の「プレビューにはかけない」を Remote では破る。PC で見る色
  (LibRaw 現像) と Remote で見る色 (カメラ JPEG) が違う

設計担当の推奨は **A**。理由は、Remote は既に「PC と同じ編集結果を見せる」契約で作られており
(§3.3)、B はその契約を RAW だけ崩すため。A の admission 設計は S2 の Remote 段で独立レビューを受ける。

### 10.3 remote-web の旧経路 (D10)

remote-web プロセスは LibRaw を持たないので、旧 `/api/image` `/api/image-info` は RAW を
**非対応として返す** (remote-web の一覧から RAW を外す)。`web/app.js:8769,8799` が旧経路を使うのは
`address` を持たない entry だけなので、S2 で「RAW の entry が必ず `address` を持つか」を確認する。
持たない経路が見つかった場合は、その entry に `address` を付ける側を直す。

### 10.4 Remote AI

canonical loader の Full (High)。

## 11. 既存データとの互換

| データ | リリース済みか | 扱い |
| --- | --- | --- |
| `catalog.db` の RAW サムネイル (WIC 由来) | 済 | そのまま使う。source_dims が WIC と LibRaw で違う場合、アイドル高画質化の判定が 1 回走り得るだけ |
| RAW に保存済みのマスク / 補正レイヤー / 隠蔽 / 注釈 / crop | 済 (Store 拡張がある環境、または DNG で WIC が開けた環境で作られたもの) | 寸法が変わっても既存の拡縮経路 (§3.2) で読まれる。**WIC の出力寸法と LibRaw の `developed_dims` が一致するかは未確認**。MS の拡張も LibRaw ベース (Store の説明・報道) なので一致する見込みだが、縦横比が違う場合は位置がずれる。S1 で手元に WIC 出力がある DNG だけでも比較し、結果を本書に記録する。移行処理は作らない |
| `similar.db` の RAW の行 | 済 | proxy の入力が WIC のフル画像からプレビューへ変わる。PDQ は縮小に強いので既存行は有効なまま扱い、再計算は通常の差分照合 (mtime / size) に任せる。形式 version は上げない (S2 で `similar_image` の proxy version の意味を確認し、上げる必要があれば判断を戻す) |
| 新設定 `raw_develop_parallelism` | 未 | 未リリースなので移行処理は不要 |

## 12. 性能とメモリ (すべて未計測)

- 所要時間: WIC で RAW を開く時間、LibRaw のプレビュー抽出・フル現像・Half 現像の時間は **誰も
  測っていない**。S1 でサンプル全件について `info / preview / develop(Full) / develop(Half)` の時間と、
  中断要求から終了までの時間を計測するベンチ (`src/bin/bench_raw.rs`) を作り、結果を本書に記録する
- メモリの目安 (**公式文書の画素当たり係数からの推定**、API-notes.html "Memory Usage"):
  raw 2 B/px + 後処理 8 B/px + 8bit 出力 3 B/px、ハイライト復元等で一時 6〜8 B/px。
  24MP で約 0.3〜0.5 GB、45MP で約 0.6〜1.0 GB、100MP で約 1.3〜2.1 GB。既定の並列 3 で 100MP を
  同時に現像すると約 4〜6 GB。これに `fs_cache` の RGBA (45MP で約 180 MB/枚) と GPU texture が加わる
- perf 計装 (`--perf-log`): `raw/info`, `raw/preview`, `raw/develop_begin|end|cancel|fail`
  (所要時間・寸法・形式・scale)、`raw/scheduler_*` (待機・実行・取消中・cancel→終了 ms、
  `fs.scheduler_*` と同じ項目)

## 13. 設定

- `raw_develop_parallelism: u8` (既定 3、1〜10)。環境設定の「ファイル処理」系ページ (PDF ワーカー数の
  近く) に置く。変更は即時反映 (§5.4.1)
- 現像パラメータの設定は v1 では作らない

## 14. 配布・ライセンス (S4)

- 静的リンクなので **新しい DLL / exe は無い**。launcher・ポータブル・署名・VC runtime gate の一覧は
  変更しない (確認として `check-vcrt-pe-dependencies.ps1` を通す)
- 同梱するライセンス文書: `LIBRAW-LICENSE.txt` = CDDL-1.0 本文 + LibRaw の COPYRIGHT
  (DCB / FBDD の BSD 表記を含む)。インストーラ (`installer/mimageviewer.iss`)、ポータブル
  (`scripts/build-portable.ps1` の同梱一覧)、`installer/readme.txt` / `readme_portable.txt` の「ライセンス」節
- アプリ内「バージョン情報」(`src/ui_dialogs/about.rs`) の第三者一覧に `LibRaw (CDDL-1.0)`、
  同梱版、対応ソースへのリンク (`https://mikage.to/mimageviewer/libraw-0.22.2-source.tar.gz`)。
  版は build.rs が `vendor/libraw/VERSION` から焼き込む (FFmpeg の `MIV_FFMPEG_BUILD_ID` と同型)
- 対応ソース: 無改変の公式 tarball をそのまま mikage.to に置く (sha256 を tracked の `.sha256` に記録)。
  shim は MIT で本リポジトリにあり、GitHub で公開済み。手順は新規 `docs/libraw-source-distribution.md`
- 製品ページ `htdocs/mimageviewer/index.html` のライセンス節に LibRaw を追加。
  `htdocs/mimageviewer/manual/formats.html:113-131` の「DNG は標準、他は Store 拡張が必要」を
  「RAW は内蔵」へ書き換え、非対応形式 (Nikon HE/HE*、JPEG XL 圧縮 DNG) を書く。
  `docs/spec.md:1330` も更新
- リリースチェックリスト (CLAUDE.md Phase 2) に「`bash scripts/setup-libraw.sh check`」と
  「対応ソースの配置」を追加。`scripts/bootstrap-vendor.sh` に LibRaw の取得を追加

## 15. テスト方針

- **サンプル**: raw.pixls.us の **CC0 のファイルだけ**を使う (同サイトには CC BY-NC-SA の 146 件が
  混ざるので license URL で厳密に絞る)。git には入れず、tracked の manifest
  `tests/raw-samples.json` (URL・sha256・形式・期待される性質) を正本に、
  `scripts/setup-raw-samples.ps1` が `vendor/raw-samples/` へ取得する。Windows でサンプルが無い場合、
  RAW の統合テストは **skip ではなく失敗**させ、復旧手順を出す (検証できない分岐を残さない)
- 選ぶサンプル (機能で選ぶ): CR3 / CR3 CRAW / CR2 / 実機の NEF / NEF HE* (非対応の陰性) / ARW 圧縮 /
  ARW lossless / RAF (X-Trans、圧縮) / ORF / RW2 / PEF / RWL / IIQ / スマートフォンの DNG
  (小さいプレビュー → Half 現像経路) / deflate DNG / lossy DNG (`USE_JPEG` の判断) / 縦位置 (flip 5 と 6)
- unit / 統合テスト:
  - `raw_decoder`: 全サンプルで `develop(Full)` 寸法 == `developed_dims`、プレビューの向き、非対応判定、
    中断 (callback 経由で `Cancelled`)、壊れたファイル (切り詰め) で `Corrupt` を返し落ちない
  - WIC 境界: RAW path / RAW 拡張子の bytes を渡すと WIC を呼ばずに `None`
  - 入口 D1〜D11・P1〜P4: TIFF 構造の RAW (DNG / CR2) を渡して、decoder 経路が LibRaw であることを
    型付きの decode source で確認する (image crate の IFD0 誤成功の回帰)
  - `RawDevelopScheduler`: 枠数、High 予約、昇格、待機取消、実行中取消の枠占有、設定変更の即時反映
    (`fs_page_load_scheduler` のテストと同型、fake job で GPU / LibRaw 不要)
  - フルスクリーン状態遷移: preview → develop → Static 置換、現像窓の出入りでの cancel、
    非対応の終端、page-turn sequence が現像を待たないこと、
    **別 viewer context の現像結果が sibling の `fs_cache` へ入らないこと** (context 切替をまたぐ回帰)
  - 編集 gate: `RawPreview` で消しゴム・補正レイヤー・隠蔽・注釈・SNS 分割・crop の入口が拒否され、
    Static 後に通ること
- 実機確認 (利用者が行う): 大きな RAW フォルダのページ送り、差し替えで表示が飛ばないこと、
  進捗表示、編集が現像後に使えること、Remote。エージェントは製品バイナリを起動しない

## 16. 段階

| 段 | 内容 | 受入条件 |
| --- | --- | --- |
| **S1** | `crates/libraw-sys` (shim + cc ビルド)、`setup-libraw.sh`、`raw_decoder` (info / preview / develop / 中断 / 進捗)、zlib / libjpeg の決定 (§4.1)、明るさの決定 (§5.3.4)、`bench_raw`、サンプル manifest | §15 の `raw_decoder` テストが緑。全サンプルの寸法一致・向き・縦横比差・所要時間・中断遅延の表を本書へ記録。core が VC runtime DLL を import しないこと (`check-vcrt-pe-dependencies.ps1`)。ubuntu CI の `cargo check` が通ること |
| **S2** | `raw_format` と WIC 境界の拒否、`RawDevelopScheduler`、入口 D1〜D11・P1〜P4 の振り分け、サムネイル (Half 現像の代替を含む)、ZIP 内 RAW、類似索引、書き出し / コピー / 外部ツール、Remote (§10 の決定に従う、admission 設計は独立レビュー) | 入口ごとの回帰テスト、scheduler テスト。`is_raw_ext` を通らずに RAW を decode する経路が無いことを grep 手順と test で示す |
| **S3** | `FsCacheEntry::RawPreview`、読み込み状態、`raw_develop_pending`、現像窓、差し替えの layout、色の gate、編集 gate、進捗表示と先読み行、設定 UI、§7.7 の consumer 点検 | フルスクリーン状態遷移テスト、context 分離テスト、UI スナップショット (進捗表示と設定)。`build-dev.ps1` で利用者の実機確認 |
| **S4** | ライセンス文書・バージョン情報・対応ソース・マニュアル・製品ページ・spec・readme・リリースチェックリスト・bootstrap | 文書差分のレビュー。`build-dist.ps1 -NoSign` 相当で同梱物に `LIBRAW-LICENSE.txt` が入ること |

各段は委任前にコミットし、この worktree で codex を同時に 2 本走らせない。master は各段の区切りで
このブランチへ取り込む (app.rs は競合しやすい)。統合は S4 完了後に一度。

## 17. 未決事項

### 17.1 利用者の判断が要るもの

1. **Remote のページ** (§10.2): A (フル現像、推奨) / B (埋め込みプレビュー)
2. **製本への RAW の追加**: 現状は無編集なら RAW ファイルをそのままコピーするが、本のページ判定
   (`is_supported_book_image_path`) が RAW を含まないため、コピーした RAW がページとして数えられない
   (§3.1)。案: (a) RAW を本に入れるときは常にフル現像して画像として焼き込む (推奨。本を開くたびに
   現像しない) / (b) 本のページ判定に RAW を加え、本の中でも LibRaw で表示する
3. **対象拡張子の追加** (`crw srw 3fr erf kdc mrw mos mef dcr` 等): v1 に含めるか。含めると
   「Store 拡張で開けていたのに開けなくなった」形式が減る一方、サンプルと検証が増える
4. **フル現像の保持**: fullscreen を閉じて開き直すと再現像になる。保持 LRU (PDF の retained page cache と
   同じ容量枠) を v1 に入れるか

### 17.2 設計レビュー・S1 で詰めるもの

- zlib / libjpeg の調達 (§4.1)
- 明るさのパラメータ (§5.3.4)
- `adjust_sizes_info_only()` の寸法が全形式で現像結果と一致するか (§5.3.2)
- プレビューと現像の縦横比差 (§7.5)
- 進捗の区切り (§5.4.4)
- `RAW_PREVIEW_MIN_LONG_EDGE = 1024` の妥当性 (§5.3.3)

## 18. 出典

- LibRaw 0.22.2 ソース一式: https://github.com/LibRaw/LibRaw/archive/refs/tags/0.22.2.tar.gz
  (本文の `src/...` / `doc/...` / `libraw/...` の行番号はこの tarball 内)
- リリース: https://github.com/LibRaw/LibRaw/releases/tag/0.22.2 / https://www.libraw.org/download
- API: https://www.libraw.org/docs/API-CXX.html / API-datastruct.html / API-notes.html / Install-LibRaw.html
- ライセンス: COPYRIGHT / LICENSE.CDDL / LICENSE.LGPL (上記 tarball)
- LibRaw-cmake の状態: https://github.com/LibRaw/LibRaw-cmake
- MSVC OpenMP の再頒布: https://learn.microsoft.com/en-us/cpp/build/reference/openmp-enable-openmp-2-0-support
- サンプル: https://raw.pixls.us/ / https://raw.pixls.us/json/getrepository.php?set=all
- Rust crate の状態: crates.io (`libraw-rs`, `rsraw`, `libraw_rs_vendor`)、2026-09-27 時点
- Microsoft の Raw 画像拡張が LibRaw ベースであること: Store の説明・報道 (handoff §4)。未検証の二次情報
