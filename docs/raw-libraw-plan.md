# RAW の LibRaw 対応 — 設計書

- 状態: **設計 第2版 (独立レビュー 1 回目の指摘を反映、再レビュー待ち)**。2026-09-27 (ClaudeCode Opus 5.5)
  - 第1版 `cb2953119` → GPT-6 Sol / xhigh 独立レビュー (P1×3 / P2×7 / P3×1、全件採用。対応表は §19)
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
- 対象拡張子: 現行の 15 種 `dng cr2 cr3 nef nrw arw srf sr2 raf orf rw2 pef ptx rwl iiq`
  (`src/wic_decoder.rs:64-71`) に、**`crw` (Canon の旧形式) と `srw` (Samsung NX) を加えた 17 種**。
  **DNG も LibRaw へ移す** (「RAW は全部 LibRaw」で説明を一本化)
- 一覧サムネイル、フルスクリーン、ZIP 内 RAW、書き出し / コピー / 外部ツール、製本、類似画像、
  mIV Remote、360 度・比較・分析などの周辺機能での RAW の扱い
- 配布・ライセンス通知・対応ソース・マニュアル / 製品ページ / spec (S4)

**範囲に入れない (v1 の非目標)**:
- 現像パラメータの利用者設定 (WB・露出・ハイライト等の UI)。v1 は固定パラメータ (§5.3.4)
- `3fr erf kdc dcr mrw mos mef` 等の追加。S1 で LibRaw の対応と CC0 サンプルの有無を調べてから利用者と相談する (§17)
- フル現像結果の保持 (ディスクにもメモリ LRU にも持たない)。開き直したら再現像する。
  AI アップスケールと同じ扱い (利用者決定 2026-09-27)
- RAW の EXIF 表示の改善 (メタデータパネルは現状の rexif のまま)
- Lightroom 等の `.xmp` サイドカーの解釈

## 2. 利用者の決定事項 (確定)

1. S4 (配布・ライセンス・文書) まで揃えてからリリースする。master への統合は完成後に一度
2. フルスクリーンは **常に「埋め込みプレビュー → フル現像に差し替え」**
3. RAW フル現像の先読みは **先 2 枚・前 1 枚**
4. RAW フル現像の並列数は **既定 3、設定で 1〜10**
5. **編集機能 (補正 / AI アップスケール / 消しゴム / モザイク等) はフル現像完了まで待つ**。プレビューにはかけない
6. **Remote はフル現像を使う** (本体と同じ結果にする) (2026-09-27)
7. **製本**: 無編集なら元の RAW ファイルをそのまま入れる。何か編集していれば焼き込む。
   PNG 等と同じ規則にする (2026-09-27)
8. 対象拡張子に `crw` / `srw` を加える (2026-09-27)
9. フル現像の保持はしない。再処理でよい (2026-09-27)
10. **カラー化・LUT が効いているページ**は、フル現像が終わるまで既存と同じ
    「サムネイルにカラー化・LUT を掛けた低解像度の代役」を出す (Q1 案A、2026-09-27)
11. **埋め込みプレビューが無い RAW** は、フルスクリーンでは「現像中」を出してフル現像を待つ。
    サムネイルだけ半分の解像度の現像 (以下 half 現像) で作る (Q2 案A、2026-09-27)

用語の固定: 「先 / 前」は **表示順 (現在の一覧・読書順) での進行方向 / 逆方向**。
既存設定の `prefetch_forward` (既定 12) / `prefetch_back` (既定 4) (`src/settings.rs:4226-4230`,
`6666-6671`) と同じ向き。handoff の「後 4・前 12」はこの `back 4 / forward 12` を指す。

決定 5 と 10 の関係: 10 の代役は**表示専用**である。サムネイル (RAW では埋め込みプレビューから作る) に
色処理を掛けて一時的に描くだけで、編集結果として保存・書き出し・AI 入力・比較には使わない。
これは今すべての画像形式で「最終表示待ちの間」に使っている既存の表示契約
([display-pipeline.md §2.5.3](display-pipeline.md)) であり、確定要件 R2 (白黒 → カラーの切り替わりを
見せない) を満たすために必要。決定 5 が禁じる「プレビューを入力にした編集処理」には当たらない。

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
独立レビューもこの判断を支持している (WIC 側の拒否だけでは足りない)。

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
  `fs_upload_backlog` 経由で 1 フレーム「現在ページ + 1 件」だけ upload する。
  **backlog は同じ idx の既存 entry を stage や要求を見ずに上書きする** (`src/app.rs:66444-66470`)
- Static を `fs_cache` へ入れるたびに `bump_input_generation_for_fs_cache_reload`
  (`src/app.rs:65164`) が `input_generation[idx]` を進め、消しゴム / 補正レイヤー / 隠蔽 / edit result /
  比較準備を失効させる。**差し替えの失効経路は既にある** (PDF の再レンダがこれを使っている)
- `FsCacheEntry` は `Static / Animated / Failed / Video` (`src/fs_animation.rs:80`)。
  **`Static` があれば、ほぼ全ての画素処理が自動で始まる** (edit result、`auto_apply_saved_mask`
  `src/app.rs:74476`、同期補正 `apply_sync_adjustment`、final composite、final AI、色検索、分析、
  パイプライン debug …)。「この画素は仮」という gate は PDF 用の `display_should_defer_final_ai`
  (`src/app.rs:69420-69437`) だけで、final AI しか止めない
- 読み込み状態 `fs_page_load_state` (`src/app.rs:66626-66642`) は `Failed` 以外の `fs_cache` entry を
  すべて表示可能とみなす。一方、ページ送りの完了判定は実際に texture が解決できるかを見る
  (`src/ui_fullscreen.rs:11111-11142`)。フォルダ移動の lock 解除 `poll_fs_nav_lock`
  (`src/ui_fullscreen.rs:11781-11806`) は `Static / Animated / Video` を「フル」と数え、カラー化が
  必要なページでは完成した final composite まで待つ
- `fs_zoom` は fit 倍率への乗数、fit は **texture 寸法**から計算する。`layout_source_size` を使うのは
  通過 rendition と PDF だけ (`src/ui_fullscreen.rs:35834-35846`)。Z ズームは `source_size` と
  texture 寸法の両方から別途 transform を解く (`src/ui_fullscreen.rs:12088-12125`)
- 編集 overlay の座標: 消しゴム / 隠蔽 / 補正レイヤーは raster の `pixels.size`、注釈と crop は
  `source_dims`。保存済みマスクは要求寸法へ拡縮して読む (`src/mask_db.rs:1444`)、補正レイヤーは
  寸法不一致時に resize (`src/app.rs:64478`)、crop は `source_size` 付きで拡縮 (`src/app.rs:63957-64017`)
- 編集モードの共通入口 `fullscreen_edit_mode_entry_allowed` (`src/ui_fullscreen.rs:34990`) は
  現在 360 度だけを拒否する。左パネルは `image_edit_tools_disabled_reason`
  (`src/ui_adjustment_panel.rs:9166`) の理由文字列で無効化する。crop 入口は共通入口を通らない
- 通過 rendition (`src/app.rs:73800-73908`) はサムネイルの texture / 画素に色処理を掛けた表示専用の代役
- viewer context の park は `fs_pending` を drain して cancel する (`src/app/viewer_context_registry.rs:1680-1682`)。
  context の drop は所有 work を cancel する (`:1304-1334`)。idx 空間の差し替えは
  `invalidate_idx_state_and_queues` (`src/app.rs:31082-31155`)、fullscreen close は別の clear 経路
  (`src/app.rs:61384-61428`) を持つ
- サムネイル worker は queue から 1 件取り出して `process_load_request` を同期実行する
  (`src/app.rs:38251-38281`, `38354-38373`)。worker が待つと、その worker の後続は止まる
- 先読みの状況表示のドットは **AI 先読み専用** (`src/ui_fullscreen.rs:42649-42740`)

### 3.3 Remote

- Remote のページ (`/api/page`) は canonical loader ではなく `thumb_loader::process_load_request`
  を通る (`src/remote_ipc/container.rs:479-515,5631,5665-5677`)。`SourceOnly` は cache を迂回するだけで
  decode の段階を選ぶ仕組みではない。ページ生成の single-flight identity は `full_page` を含む
  (`:1075-1138`)
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
| 定義 | `LIBRAW_NODLL LIBRAW_BUILDLIB USE_ZLIB USE_JPEG`。`USE_X3FTOOLS` は付けない | `USE_ZLIB` は deflate DNG に必須 (`src/decoders/fp_dng.cpp`)。`USE_JPEG` は lossy DNG (compression 34892) と一部 Kodak に必須 (`src/metadata/identify.cpp:1269-1275`)。X3F は対象拡張子に無い |
| 既存の Rust crate | 使わない | `rsraw-sys` は MSVC で panic、`libraw-rs` は 0.20.1 で 2021 年停止、どれも 0.22 を同梱しない (crates.io、2026-09-27 時点) |

**新しい PE が増えない**。静的リンクなので launcher の埋め込み一覧、ポータブルの同梱一覧、署名対象、
VC runtime gate は変わらない (core が大きくなるだけ)。S4 はライセンスと文書が中心になる。

### 4.1 zlib と libjpeg (S1 で調達方法を決める。**両方とも必須**)

lossy DNG と deflate DNG は対象拡張子 `dng` の一部であり、どちらかを「非対応」にすると
利用者向け機能の削減になる。したがって **`USE_ZLIB` と `USE_JPEG` を付けたビルドで両形式の
フル現像が通ることを S1 の合格条件にする**。調達方法だけを S1 で選ぶ:

- **zlib**: workspace に C の zlib は無い (`flate2` + `miniz_oxide` だけ、`Cargo.lock:2209,4087`)。
  候補は `libz-sys` (static、cc で zlib を同梱ビルド) か、C ABI を出す `libz-rs-sys`
- **libjpeg**: `turbojpeg-sys` 1.2.0 が libjpeg-turbo を static でビルドしている
  (`Cargo.lock:7062-7083`)。その static lib が `jpeg_mem_src` を含む libjpeg API を公開し、ヘッダへ
  build.rs から到達できるなら再利用する。重複シンボル・ヘッダ不達なら、libjpeg-turbo を
  `crates/libraw-sys` 側で別にビルドする等の別手段を取る。**手段が見つからない場合は S1 を止めて
  設計担当へ返す** (勝手に lossy DNG を非対応にしない)

## 5. 全体構造

```
            ┌─────────── 拡張子の単一所有者 ───────────┐
            │ raw_format::is_raw_ext / RAW_EXTENSIONS   │  ← 全入口 D1〜D11, P1〜P4 が最初に問う
            └───────────────┬───────────────────────────┘
                            │ RAW
   ┌────────────────────────▼────────────────────────┐
   │ raw_decoder (safe Rust)                          │
   │  info()     : 開くだけ (寸法・向き・プレビュー一覧・非対応判定)   │
   │  preview()  : 埋め込みプレビュー (安い。executor を使わない)     │
   │  develop()  : 現像 (full / half)。RawDevelopExecutor の中だけで呼べる │
   └────────────────────────┬────────────────────────┘
                            │ extern "C" (自前の狭い ABI)
   ┌────────────────────────▼────────────────────────┐
   │ crates/libraw-sys : shim (C++) + LibRaw 0.22.2 静的  │
   └──────────────────────────────────────────────────┘

   RawDevelopExecutor (App-global、Arc) : 現像専用の worker thread 群 = 設定値 1〜10 (既定 3)
     job を submit → ticket (cancel / 昇格) を受け取り、結果は submit 側が渡した送り先へ届く
     ├ fullscreen の現像 (viewer context の RawPageStore が ticket を所有)
     ├ 書き出し / コピー / 外部ツール / 製本の焼き込み / Remote ページ / Remote AI
     └ プレビューの無い RAW のサムネイル用 half 現像、類似索引の half 現像
```

### 5.1 拡張子の単一所有と WIC 境界での拒否

- 新モジュール `src/raw_format.rs` が `RAW_EXTENSIONS` (17 種) と `is_raw_ext(&str)` /
  `is_raw_path(&Path)` を所有する。`folder_tree::SUPPORTED_EXTENSIONS` と
  `wic_decoder::WIC_SUPPORTED_EXTENSIONS` はこの一覧を参照して組み立て、**WIC 側の一覧から RAW を外す**。
  `settings::default_image_ext_priority` (`src/settings.rs:6847-6859`) にも `crw` / `srw` を RAW と同じ
  最後尾に足す
- **WIC の境界自体が RAW を拒否する**。`wic_decoder::decode_to_dynamic_image(path)` と
  `read_wic_orientation(path)` は RAW 拡張子なら WIC を呼ばずに `None` を返す (debug build では
  `debug_assert!` で呼び出し側の振り分け漏れを検出)。bytes 版
  `decode_to_dynamic_image_from_bytes` は拡張子を知らないので、**引数に拡張子 (または
  `WicSourceHint`) を必須にする**シグネチャ変更を行い、同じく拒否する
- 各入口 D1〜D11 と P1〜P4 は、**どの decoder も試す前に** `is_raw_ext` を問い、RAW なら
  `raw_decoder` へ行く。失敗時に image crate / WIC / Susie へ落とさない (型付きエラーで終端)
- `crates/remote-web` は本体 crate に依存できない。remote-web の `SUPPORTED_IMAGE_EXTENSIONS` から
  RAW を外す (§10.3)。本体側に「本体の `RAW_EXTENSIONS` と remote-web の一覧が交わらない」ことを
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
- narrow の `open_file(const char*)` は 1 byte ずつ wchar へ広げるだけで UTF-8 を解さない
  (`src/libraw_datastream.cpp:~706`)。**Rust からは必ず wide 版を使う**
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
`develop()` は `RawDevelopExecutor` の worker thread の中だけで呼べる (§5.4)。可視性で強制する
(`pub(in crate::raw)`)。

#### 5.3.2 寸法と向き

- `developed_dims` は `adjust_sizes_info_only()` (LibRaw API、Fuji SuperCCD の回転・非正方画素・flip を
  含めた出力寸法を計算する) で求める。公式文書は「この変更は繰り返せない」と書いている
  (API-CXX.html `adjust_sizes_info_only`、`src/utils/utils_libraw.cpp:417-446`) ので、`info()` は現像と
  別の LibRaw instance / 別の open で行い、同じ instance で続けて `dcraw_process` しない。
  **S1 の受入条件**: 全サンプルで `develop(Full).dimensions() == info.developed_dims` を unit test で確認する。
  一致しない形式があれば設計へ戻す (表示の差し替えと編集座標がこの値に依存するため)
- フル現像の出力 (`dcraw_make_mem_image` / `copy_mem_image`) は **flip 適用済み**
  (`src/postprocessing/mem_image.cpp:153-233`)。追加で回転しない
- 埋め込みプレビューは **flip 未適用** (`src/decoders/unpack_thumb.cpp` に flip 処理が無い)。
  `unpack_thumb_ex(i)` の生バッファを使い、`tflip` が既知 (≠0xffff) ならそれ、不明なら `sizes.flip` を
  自前で適用する。JPEG 内の EXIF Orientation は**読まない** (`dcraw_make_mem_thumb` は EXIF の無い
  JPEG に向きを挿入するため、そちらを使うと二重適用の経路が生まれる)。向きの正しさは縦位置サンプル
  (flip 5 / 6) の unit test で固定する
- ZIP 内 RAW も bytes から同じ向きを得る (現状の「ZIP 内 RAW は向き 1」は解消される)

#### 5.3.3 使えるプレビューの定義

`thumbs_list` の中で、形式が **JPEG か BITMAP** で、実際にデコードできるものを「使えるプレビュー」とし、
その中で最大のものを選ぶ。H.265 (CR3 の HEIF プレビュー)、JPEG XL、LAYER、ROLLEI は選ばない。
**寸法の閾値は置かない** (第1版の「長辺 1024 未満は使わない」は根拠が無いので撤回)。

- CR3 の parser は一部の JPEG サムネイルを `twidth = theight = 0` で記録し
  (`src/metadata/cr3_parser.cpp:174-182`)、unpack もその寸法を書き戻さない
  (`src/decoders/unpack_thumb.cpp:203-217`)。**JPEG の寸法は JPEG ヘッダから求める**
  (`thumbs_list` の寸法は大きさの比較に使わない)。BITMAP は `unpack_thumb` 後の寸法を使う
- 使えるプレビューが 1 つも無い RAW は、決定 11 に従う: フルスクリーンは「現像中」、サムネイルは
  half 現像 (§8)。**half 現像の結果をフルスクリーンのプレビューとして出さない**
- JPEG のデコードは既存の TurboJPEG 経路を bytes で使う (サムネイル用途では DCT スケールも効く)
- 非対応形式 (`get_decoder_info().decoder_flags & LIBRAW_DECODER_UNSUPPORTED_FORMAT`、例: Nikon HE/HE*、
  JPEG XL DNG) は `info.develop_support = Unsupported` として open 直後に分かる。プレビューがあれば
  **プレビューだけを表示し、現像を要求しない** (§7.4 のメッセージを出す)。プレビューも無ければ
  そのページは読み込み失敗として終端する

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

### 5.4 `RawDevelopExecutor` (現像の実行枠)

第1版の「呼び出し側の thread が permit を待つ」形は、サムネイル worker や Remote の heavy worker を
待ちで塞ぐ (独立レビュー P2-7 / P2-8)。第2版では **現像専用の worker thread を executor 自身が持つ**。
呼び出し側は job を submit して即座に戻り、結果は submit 時に渡した送り先 (channel / continuation) へ
届く。**try_lock + sleep は使わない** (`Mutex + Condvar` の優先度キュー、PDF worker pool と同型)。

#### 5.4.1 枠と優先度

- worker 数 `N = settings.raw_develop_parallelism` (1〜10、既定 3)。設定変更は即時: 増やすときは
  thread を足し、減らすときは余分な thread が現在の job を終えてから退出する (実行中の現像は止めない)
- 優先度 3 段:
  - `High`: 表示中のページ (現在ページと見開き相方)、ページ送りの表示待ち target、利用者が今待っている
    書き出し / コピー / 外部ツール / 製本の焼き込み、Remote の前景ページ、Remote AI
  - `Normal`: フルスクリーンの先読み現像、プレビューの無い RAW のサムネイル用 half 現像 (可視範囲)、
    Remote の先読みページ
  - `Background`: 類似索引の half 現像
- 同時実行の上限: `N ≥ 2` のとき `Normal + Background` は `N - 1` まで (High 用に 1 枠を必ず空ける)、
  `Background` は 1 まで。`N = 1` のときは予約できないので次の規則にする:
  - 待機列は優先度順 → 受付順。worker が空いたら先頭を取る
  - High が待機列に入った時点で実行中が Normal / Background で、かつその job が
    「もう要らない」(現像窓の外、取消済み view、等) なら cancel する。要る job は止めない
  - 中断できない区間 (§5.4.3) にある job は cancel しても実際の終了まで枠を占有する。
    **High の待ち時間はその 1 job の残り時間を上限とする**。これを fake job のテストで固定する
- 先読みが表示対象になったら同じ ticket を High へ昇格する (取消 + 再投入はしない)。
  `FsPageLoadScheduler::promote_to_high` (`src/fs_page_load_scheduler.rs:212`) と同型

#### 5.4.2 取消

- 待機中の取消は即座に列から消える。実行中の取消は `Cancelling` として **worker が実際に終わるまで
  枠を占有する** (取消した現像を新しい現像が追い越して実行数が N を超えないように。
  `fs_page_load_scheduler.rs:383` と同じ規約)
- 取消の伝え方は shim の progress callback の戻り値 (非 0 で `LIBRAW_CANCELLED_BY_CALLBACK`)。
  callback は Rust の `AtomicBool` を読むだけ。decoder 内の `setCancelFlag` 対応箇所
  (`src/utils/utils_libraw.cpp:310-336`) も同じ flag から shim が立てる
- executor の job は他の permit (`FsPageLoadScheduler`、`GlobalIoSemaphore`、PDF pool) を**持たない**。
  submit する側も、結果を待つ間それらの permit を保持してはならない (デッドロックと優先度逆転の防止)。
  S2 で submit 箇所ごとに確認する

#### 5.4.3 中断の遅延 (公式ソースからの事実。時間は未計測)

- AHD は 512 行の帯ごとに callback があり、帯の途中で止まる (`src/demosaic/ahd_demosaic.cpp:293-355`)
- `unpack()` は開始と終了の 2 回しか callback しない (`src/decoders/unpack.cpp:32,513`)。
  中断フラグを見る decoder もあるが、**CR3 (`crx.cpp`)・Fuji 圧縮・Panasonic v8 は見ない**。
  これらは読み込みの途中では止まらない
- DCB / DHT / AAHD / X-Trans の demosaic には callback が無い。X-Trans (Fuji) は demosaic 中に止まらない

「ページ送りで途中中断」は **形式によって遅れる**。遅れは `Cancelling` の枠占有として正直に見え、
perf イベントで計測できるようにする (§12)。中断を速く見せるための時間窓や先行解放は入れない。

#### 5.4.4 進捗

- shim の progress callback が `(stage, iteration, expected)` を Rust へ渡し、Rust 側で 0〜100 の
  単調増加値へ写す (open/identify 0〜5、LOAD_RAW 5〜35、INTERPOLATE の帯 35〜85、残り 85〜100。
  区切りは S1 で実測して調整)。`unpack` は 2 回しか報告しないので、その区間は「読み込み中」と表示し
  数値を動かさない
- 値は job ごとの `Arc<AtomicU8>` (latest-value)。**channel に流さない**
  ([async-architecture.md §5.5.1](async-architecture.md))。UI は描画時に読むだけ

#### 5.4.5 メモリ

並列数は固定の設定値で決め、**実行時の空きメモリで変えない** (プロジェクト方針)。高い設定値での
メモリ不足は許容し、LibRaw のメモリ確保失敗は `RawError::OutOfMemory` として型付きで返す
(プロセスを落とさない)。`imgdata.rawparams.max_raw_memory_mb` は LibRaw 既定 (2048) のまま。
worker thread の stack は 1 MiB 以上 (LibRaw は 1 呼び出しで 130〜140 KB の stack を使う、API-notes.html)。

## 6. RAW ページの状態 (単一の持ち主)

第1版は現像の状態を `fs_cache` の variant・`raw_develop_pending`・`fs_upload_backlog` の 3 か所に
分けていた。これでは「現像済みなのに遅れたプレビューで上書き」(独立レビュー P1-1) のような
矛盾した組み合わせを禁止する持ち主がいない (P2-4)。第2版では **viewer context ごとの `RawPageStore`
が、RAW ページの段階遷移の唯一の持ち主**になる。

### 6.1 型

```rust
// ViewerContextBundle の field。fs_pending / fs_cache と同じ context 所有。
struct RawPageStore {
    pages: ItemsGenerationMap<RawPageState>, // with_discard: 保持する ticket を cancel
    next_request_id: u64,
}

struct RawPageState {
    source: RawSourceIdentity,     // item key + file size + mtime (要求時に確定)
    developed_dims: Option<[u32; 2]>, // info 到着後
    stage: RawInstalledStage,      // fs_cache に何が入っているか (単調)
    preview: RawPreviewPhase,
    develop: RawDevelopPhase,
}

enum RawInstalledStage { Nothing, PreviewShown, PreviewAbsent, Developed } // 単調増加。逆行しない

enum RawPreviewPhase { Requested { request_id }, Done, Absent, Failed(RawError) }

enum RawDevelopPhase {
    Idle,                                          // 窓の外、または未要求
    Submitted { request_id, ticket, progress: Arc<AtomicU8> }, // 待機中 / 実行中
    Done,
    Blocked(RawDevelopBlocked),                    // Unsupported | Failed(RawError)。終端
}
```

- **`fs_cache` への書き込みは `RawPageStore::apply_result` だけが行う**。RAW の結果
  (プレビュー / 現像) は `FsUploadResult` に `RawResultTag { request_id, stage, source }` を付けて
  backlog へ入り、upload 時に `apply_result` を通る。`apply_result` は次を満たす結果だけを受け付ける:
  - items generation と viewer context が一致する (既存の `ItemsGenerationMap` と bundle 所有)
  - `source` が現在の `RawPageState.source` と一致する (同じ idx で別ファイル・上書き更新を拒否)
  - `request_id` がその段階の現在の要求と一致する
  - **段階が逆行しない** (`Developed` の後に届いたプレビューは捨てる)
- backlog の同 idx 置換 (`src/app.rs:66459-66470`) は RAW の tag を見て、**`Developed` の entry を
  プレビューで置き換えない**。完了順・backlog 内の順のどちらが入れ替わっても最終状態が
  `Developed` になることをテストで固定する
- 相互排他の状態はすべて enum で表す。bool / Option の組で表さない

### 6.2 `fs_cache` 上の表現

`fs_cache` に RAW 専用の variant を足す。**`Static` にはしない。**

```rust
FsCacheEntry::RawPreview {
    preview: Option<RawPreviewTexture>, // tex + pixels + preview_dims。使えるプレビューが無ければ None
    developed_dims: [usize; 2],         // フル現像の寸法 (向き適用後)。layout と編集座標の正本
    load_seq: u64,
}
```

理由: 既存の画素処理はほぼすべて `FsCacheEntry::Static` への一致で始まる (§3.2)。プレビューを
別 variant にすれば、それらは **プレビューに対して始まらない**。「Static だが仮」という属性を足すと、
すべての consumer にその属性の確認を足す必要があり、1 箇所でも漏れると決定事項 5 が破れる。
ただし variant を分けても、`_ =>` で Static 以外を受ける箇所、「Failed 以外は ready」とみなす箇所、
サムネイル経由の代役 (通過 rendition) は別途点検が要る (§7.7)。

現像の終端 (`Blocked`) は `RawPageStore` にだけ持ち、`fs_cache` の variant には持たない
(同じ事実を 2 か所に書かない)。フル現像の結果は `FsLoadResult::Static` として同じ upload 経路を通り、
`apply_result` が `RawPreview` を `Static` で置き換える。これで `bump_input_generation_for_fs_cache_reload`、
比較準備の失効、`FinalEffectSourceReload` の holdover 捕捉、`auto_apply_saved_mask` が既存どおり動く。

### 6.3 読み込み状態

`fs_page_load_state` (`src/app.rs:66626-66642`) の「Failed 以外は表示可能」を RAW で使わない。
RAW ページの状態は `RawPageStore` から次の 5 つへ写す:

| RAW の状態 | 意味 | `waiting_for_display()` |
| --- | --- | --- |
| `PreviewPending` | プレビュー要求中 | true |
| `PreviewShown` | プレビュー画素がある | false |
| `PreviewAbsent` | 使えるプレビューが無い。フル現像待ち | **true** (表示できる画素が無い) |
| `Developed` | Static がある | false |
| `Terminal` | プレビューも現像も得られない (失敗・非対応でプレビュー無し) | false (`LoadFailed` と同じ) |

`ensure_fs_page_load` は `PreviewPending` 以外でプレビューを再要求しない。

### 6.4 lifecycle (open / switch / park / close / cancel / error / 差し替え)

| 事象 | `RawPageStore` の動き |
| --- | --- |
| ページを開く / プレビュー窓に入る | `RawPageState` を作り `source` を確定。`fs_pending` に preview 要求 (既存 `FsPageLoadScheduler`)。`info()` の `developed_dims` を `DimsOnly` として先に送る |
| プレビュー到着 | `apply_result`: `stage = PreviewShown` (画素あり) / `PreviewAbsent` (無し)。`record_fs_cache_page_dims_for_spread` には `developed_dims` を渡す |
| 現像窓に入る (§7.2) | `develop = Submitted` (executor へ submit、ticket 保持)。`Blocked` / `Done` なら何もしない |
| 現像完了 | `apply_result`: `stage = Developed`、`develop = Done`。`into_gpu_raster` の 8192 clamp と 360 度の tee は既存の後処理を通す |
| 現像失敗 / 非対応 | `develop = Blocked(..)`。同じ items 世代・同じ `source` の間は再要求しない |
| 現像窓を出る | `Submitted` の ticket を cancel して `Idle` へ。`stage` は下げない (完了済みの Static は keep set から外れるまで残す。同寸法の JPEG と同じ) |
| プレビュー窓 (keep set) を出る | `fs_cache` の entry と一緒に `RawPageState` を削除 (保持中の ticket は discard hook で cancel) |
| 取消 (上記以外の cancel) | `Idle` へ戻す。`Blocked` にしない (窓に戻れば再要求) |
| ページ送り中 | §7.6 の admission に従う |
| viewer context の park | 既存の `fs_pending` drain と同じ場所で `RawPageStore` の全 ticket を cancel し `develop = Idle` (`src/app/viewer_context_registry.rs:1680-1682`)。park 中に完了した現像は捨てる。activate 後に現像窓を再評価して submit し直す |
| viewer context の mount / swap | bundle の `swap_field!` 一覧に `RawPageStore` を含める。結果の送り先 channel も context 所有なので、sibling context の結果を消化しない |
| viewer context の drop | discard hook で全 ticket を cancel (`src/app/viewer_context_registry.rs:1304-1334` と同じ場所) |
| idx 空間の差し替え (`invalidate_idx_state_and_queues`) / items generation の変更 | `RawPageStore` を clear (ticket cancel) |
| fullscreen close | `fs_cache` の clear と同じ経路で `RawPageStore` を clear |
| 同じ path のファイルが外部で上書きされた | 次の要求で `source` (size / mtime) が変わるので、古い結果は `apply_result` で拒否される |

これは detached 専用の bool / Option ではなく、既存の context-owned resource (`fs_pending` と同型) の
追加である。detached 憲法 §2-3 には当たらない。context 切替をまたぐ回帰テスト (§15) を必須にし、
[detached-rework-plan.md](detached-rework-plan.md) §11 に「context-owned resource を 1 つ追加した」と
記録する。

## 7. フルスクリーン

### 7.1 流れ

1. ページを開く → プレビュー (安い) → 表示
2. 現像窓にあれば executor が現像 → 完了で Static に置換
3. 置換で編集・AI・補正の各段が既存どおり始まる (決定 5: それまでは始まらない)

### 7.2 現像の窓

- 対象: 表示順 (`collect_image_indices()`、`src/ui_fullscreen.rs:27291`) で **現在ページ・見開き相方・
  先 2・前 1**。順序は既存の `interleaved_prefetch_targets` (`src/app/prefetch_policy.rs:161`) と同じ
  「+1, −1, +2」。現在ページと相方は High、他は Normal
- 窓は定数 (`RAW_DEVELOP_FORWARD = 2` / `RAW_DEVELOP_BACK = 1`)。設定にしない (決定 3)
- プレビューの窓は既存の `prefetch_back` / `prefetch_forward` のまま
- 既存の「現在ページが読み込み中なら他の先読みを全部 cancel」(`src/app.rs:60623-60667`) は
  `FsPageLoadScheduler` の話であり、現像の窓には適用しない
- AI の先読み (`prefetch_final_ai`) は Static を要求するので、RAW では自然に現像窓の中だけになる。
  先読み AI のドット表示が「永久に未着」を出さないよう、AI 先読み対象の RAW ページを現像窓で
  切り詰める
- 見開きの相方も現像窓に含める (相方がプレビュー段のままだと、見開き全体の final 表示が揃わない)

### 7.3 色の扱い (R2、決定 10)

- `RawPreview` の表示は、既存の表示優先順位の **`fs_cache` 段** (生デコード結果の段) に置く。
  プレビュー画素そのものには補正・AI・カラー化・LUT を一切かけない (決定 5)
- カラー化 / LUT が有効で `colorize_display_requires_final_effect(idx)` が真のページでは、既存どおり
  生の `fs_cache` 段を出さず、**サムネイルから作る色忠実 rendition** (`src/app.rs:73800-73908`) へ落ちる
  (決定 10)。RAW のサムネイルは埋め込みプレビューから作るので、代役の画素の元はプレビューだが、
  これは表示専用で、保存・書き出し・AI・比較・edit cache には入らない
- 近モノクロ判定 (`MonochromeOnly`) は `edit_result_cache` の画素を見る。フル現像までは memo miss なので
  安全側の「待つ」になる (既存規約のまま)
- 見開きで片側だけ色処理待ちのときの扱い (左右とも rendition に揃える) も既存規約のまま
- プレビュー (カメラ JPEG) → フル現像 (LibRaw) の差し替えでは色・トーンが変わる (§5.3.4)。
  これは決定 2 が受け入れている差として扱い、時間窓や cross-fade で隠さない

### 7.4 編集の gate

- `fullscreen_edit_mode_entry_allowed` に型付きの拒否理由 `RawDevelopmentPending` /
  `RawDevelopmentUnavailable` を足す。消しゴム・補正レイヤー・隠蔽・注釈・SNS 分割のキー入口と
  左パネルのボタンは既にこの入口と `image_edit_tools_disabled_reason` を通るので、両方へ同じ述語を渡す
- **crop の入口 (`enter_export_crop_mode`、`src/ui_crop.rs:215`) は共通入口を通っていない**。
  RAW の crop も決定 5 の対象とし、共通入口を通すよう揃える (crop 自体の挙動は変えない)
- 補正スライダー・AI モデル選択は「設定を書く」操作であり、プレビュー段でも受け付ける。
  適用 (final composite) は Static が来るまで始まらないので、決定 5 は満たされる
- 拒否時の表示: 既存の no-op 表示 (`FsNavNoOpReason`) を使い「RAW の現像が終わると編集できます」
  / 非対応形式では「この RAW 形式は現像に対応していません (埋め込みプレビューを表示中)」。
  待ってから自動で入る (conceal の EnterMode 型の継続) は v1 では作らない
- 判定の述語は `RawPageStore` の `stage != Developed` から導く (`fs_cache` の variant を別に見ない)

### 7.5 差し替えで表示を飛ばさない

第1版は「既存の `layout_source_size` がそのまま効く」と書いたが、現行コードでそれを使うのは
通過 rendition と PDF だけで (`src/ui_fullscreen.rs:35834-35846`)、Z ズームは別に transform を解く
(`:12088-12125`)。第2版では **RAW プレビューを layout 寸法 = `developed_dims` の経路へ明示的に通す**:

- `draw_fs_image` の `use_source_layout` 判定に「RAW プレビュー段」を加える
- 見開き (`draw_fs_spread`)、連結読み、holdover capture、Z ズームの `ResolvedZTransform`、
  `Original` (100%) と拡大しない / 縮小しないの texel 基準、ナビゲータ、ルーペ、範囲キャプチャの
  hit-test を点検し、プレビュー段の transform が `developed_dims` 基準で解かれるようにする。
  判定は 1 つの述語 (`fs_page_layout_uses_source_size(idx, texture)`) に集め、各経路はそれを呼ぶ
- プレビュー texture は `developed_dims` の包含枠の中へ contain する
  (`resolve_fs_transform_in_layout_rect`)。`Original` はプレビュー段でも `developed_dims` 基準の 100% を
  指し、プレビューは拡大されて見える (解像度は変わってよい、
  [display-pipeline.md §2.5.1.1](display-pipeline.md))
- `DisplayedImageTransform` の `source_size` は常に `developed_dims`
  (`source_dims_for_idx` が RAW ページで `developed_dims` を返す)。注釈・crop の座標は最初から正しい
- テスト: プレビューと現像で寸法も縦横比も違う fake を用意し、Page / Width / Height / Original / Z の
  各モードで置換前後の paint rect と source↔screen 写像が一致すること (縦横比が違う場合は contain の
  差だけ)
- S1 でサンプル全件のプレビューと現像の縦横比差を表にする。許容できない差があれば設計へ戻す

### 7.6 ページ送り・フォルダ移動との関係

- ページ送りの完了判定 (`src/ui_fullscreen.rs:11111-11142`) は既存どおり「実際に texture が解決できたか」
  (`resolve_fs_display_tex(idx, true)`、サムネイルを含む) を使う。RAW では:
  - `PreviewShown` → プレビュー、色処理待ちなら rendition で settle
  - `PreviewAbsent` → サムネイルがあれば settle。無ければ target のサムネイル (half 現像) と
    フル現像の両方が必要になる
- 現像の admission は既存の `FsPageTurnWorkAdmission` に従う:
  - `All`: 現像窓の通常の submit
  - `NavigationTargetMaterializationOnly`: **target ページの現像と、target のサムネイル用 half 現像を
    High で許可する** (これが無いと、プレビューの無い target が settle できないまま止まる。独立レビュー P2-5)。
    target 外の現像は開始しない
  - `Deferred` (ready な rendition を描いている): 現像を開始しない
- フォルダ移動の lock (`poll_fs_nav_lock`) は既存規約を変えない。RAW の `PreviewShown` は
  「サムネイル相当の表示可能」として `has_thumb` 側と同じ扱いにする (lock 解除条件に RAW を足す)。
  カラー化 / LUT が必要なページは既存どおり完成した final composite まで待つので、RAW では
  **フル現像 + 最終合成の完了まで旧フォルダの holdover が残る** (数秒、未計測)。この間 target の
  現像は High で許可する。既存規約を RAW のために緩めない

### 7.7 `fs_cache` / 読み込み状態を読む consumer の点検

独立レビュー P2-9 のとおり、Static 限定の画素処理だけでなく、**readiness・lock 解除・通過 rendition・
final composite** の consumer も点検する。S3 の最初に次を grep で全列挙し、本表へ 1 件ずつ RAW の扱いを
記録してから実装する: `FsCacheEntry` の match で `_ =>` / `..` を使う箇所、`fs_page_load_state` /
`waiting_for_display` の利用箇所、`resolve_fs_display_tex` / `resolve_fs_processed_texture` の利用箇所、
`thumbnails` / `thumb_pixels` から画素を作る箇所。

| consumer | 位置 | RAW プレビュー段での扱い |
| --- | --- | --- |
| 表示優先順位 `resolve_fs_processed_texture` | `src/ui_fullscreen.rs:9358-9392,9571-9604` | fs_cache 段に RawPreview を含める。現在は Static 以外で fall through するので明示の分岐を足す (§7.3) |
| 読み込み状態 `fs_page_load_state` | `src/app.rs:66626-66642` | RAW は §6.3 の写像を使う |
| ページ送りの完了判定 | `src/ui_fullscreen.rs:11111-11142` | 変更不要 (texture 解決で判定)。admission は §7.6 |
| フォルダ移動の lock 解除 `poll_fs_nav_lock` | `src/ui_fullscreen.rs:11781-11806` | §7.6 |
| 通過 rendition / 色忠実 rendition | `src/app.rs:73800-73908` | 表示専用として使う (決定 10)。edit cache・書き出しへ流れないことをテストで確認 |
| 元画像ホールド | `src/ui_fullscreen.rs:10516,10526` | プレビューを出してよい (表示だけ) |
| ルーペ | `src/ui_fullscreen.rs:39806` | 表示 texture を使うのでプレビューで可。座標は §7.5 の transform |
| 自動余白カットの bbox | `src/ui_fullscreen.rs:35594` | プレビュー画素から計算してよい (正規化 bbox)。キーは `(load_seq, Arc ptr)` なので現像後に再計算される |
| 見開きの寸法記録 | `src/ui_fullscreen.rs:16304` | `developed_dims` を記録 |
| edit result / 消しゴム / 補正レイヤー / 隠蔽 / `auto_apply_saved_mask` / 同期補正 / final composite / final AI / AI 先読み | `src/app.rs` 各所 | Static 限定。変更不要 (始まらない) を確認 |
| 書き出し / コピー / 比較 pin (capture 経由) | `src/ui_fullscreen.rs:44518` | `complete` な final composite を要求するので自然に待つ。案内文「最終合成の完了後に再実行してください」はそのまま |
| 比較 pin (グリッド選択から) | `src/ui_fullscreen.rs:43838`, `src/app.rs:35391` | Static / Animated だけ受ける。RAW なら現像を High で submit して `compare_pin_load_pending` を継続 |
| 比較の source 待ち | `src/ui_fullscreen.rs:38242` | `fs_pending` だけでなく RAW の現像中も待つ |
| 360 度 | `src/app.rs:71857,72832` | Static の `source_dims` で判定。現像後に判定される。高解像度 tee は現像結果にも適用 (D7 は §9) |
| 分析パネル / ヒストグラム | `src/ui_fullscreen.rs:24088` | Static 限定。プレビュー段は「RAW の現像待ち」と表示 |
| 色検索のパレット | `src/app/color_filter.rs:106` | Static 限定。キャッシュはファイル identity なので、プレビューから抽出しないことが重要 (変更不要を確認) |
| パイプライン debug 出力 | `src/pipeline_debug.rs:284,661,697` | Static 限定 |
| `fs_static_has_alpha` / 縮小警告 / AI トースト / hover bar の寸法 | `src/app.rs:61518`, `src/ui_fullscreen.rs:26448`, `27789` | 個別確認 |
| Ctrl+E 書き出しダイアログ | `src/export_dialog.rs:960` | 自前で decode する (D5)。現像は §9 |

### 7.8 進捗表示

- 左下の読み込みラベル (`src/ui_fullscreen.rs:24033-24066`、「読込中...」/「PDF 再レンダリング中...」)
  と同じ場所に、現在ページの状態を出す: `RAW 現像待ち` (executor の待機列) / `RAW 現像中 NN%` /
  `RAW 読み込み中` (unpack 区間) / 非対応・失敗のメッセージ。表示内容は `RawPageStore` から導く
- プレビューの無いページ (`PreviewAbsent`) は、サムネイルが無い間は既存の中央の「読込中」の位置に
  「RAW 現像中 NN%」を出す (決定 11)
- 既存の先読み状況の行 (`draw_fs_prefetch_status_row`、AI 用) と同じ部品・同じ設定
  (`fullscreen_prefetch_status_visible`) で、**「RAW 現像」行**を足す。ドットは現像窓の
  各ページの `Ready / Active / Missing` (既存の `FsPrefetchPageState`)

### 7.9 メモリ上のキャッシュ

- フル現像の結果は `fs_cache` の Static としてだけ持つ。ディスクにも保持 LRU にも持たない (決定 9)
- 現像窓を出ても、keep set (`prefetch_back` / `prefetch_forward`) の中なら Static は残る。これは
  同じ寸法の JPEG と同じ扱いで、RAW だけプレビューへ降格して解放する処理は作らない
- fullscreen を閉じて開き直すと、現像はやり直しになる

## 8. サムネイル

- D2 (`load_one_cached` / `decode_zip_chain`) で RAW を最初に振り分け、`preview()` を使う。JPEG の
  プレビューは既存の TurboJPEG DCT スケール経路へ bytes で渡す。向きは §5.3.2
- プレビューを使う条件: 使えるプレビューがあり、その長辺が `min(要求された display_px, developed の長辺)`
  以上。満たさなければ half 現像にする。判定は要求とファイル内容だけで決まる
- half 現像は **サムネイル worker の上で待たない** (独立レビュー P2-7)。worker は executor へ job を
  submit し、`ThumbMsg` を送らずに次の要求へ進む。job の完了時に executor の thread が同じ `ThumbMsg`
  を既存の送り先へ送る。これで可視サムネイルの処理が RAW の現像待ちで止まらない
  - job は submit 元の cancel token (フォルダ単位の `cancel_token`) と要求の items generation を持ち、
    既存の `ThumbMsg` の世代検査を通る。フォルダ移動では cancel される
  - 優先度: 可視範囲は Normal、ページ送りの target は High (§7.6)、それ以外の先読みは submit しない
    (可視に入ったら submit する)
  - 1 フォルダで submit する数は keep range の中に限られる (既存の keep range が上限になる)
- 報告する `source_dims` は `developed_dims`。これで一覧の縦横比・見開き判定・詳細表示の解像度列が
  フル現像と一致する
- `CacheDecision::should_cache` に RAW 専用の規則は足さない (RAW はファイルが大きく既存の
  `size_threshold` に掛かる見込み。未確認)
- キャッシュ一括作成 (D4)・画質サンプル (D9) は現状 RAW を扱えていない。同じ振り分けを入れて
  サムネイルと同じ結果にする (half 現像が要る RAW は executor の Background で)
- 類似索引への prefill (`src/thumb_loader.rs:3512-3520`) は、`Other` 形式では「decode 長辺 ≥ source_dims
  長辺」でないと拒否される (`src/similar_index.rs:6722-6737`)。RAW 用の形式区分を足し、プレビューからの
  prefill を受け入れる (§9 D6)

## 9. その他の入口

| # | 方針 |
| --- | --- |
| D1 canonical loader | `CanonicalDecodeOptions` に **既定値の無い** `raw_stage: RawStage::{Preview, Full}` を足す。Preview は `CanonicalDecodeResult::RawPreview { preview: Option<..>, developed_dims, develop_support }` を返す。Full は executor へ submit して結果を待つ (canonical loader を呼ぶ thread は他の permit を持たないこと、§5.4.2)。fullscreen の先読み / 表示は Preview、Remote AI は Full、類似候補プレビューは Preview (比較用の表示なので) |
| D3 | Preview (サムネイル用途、§8 の条件) |
| D4 / D9 | サムネイルと同じ (§8) |
| D5 書き出し / 外部ツール | Full 現像 (High)。書き出しは `SrcFormat::Other` でメタデータ転記を拒否している (`src/save_with_metadata.rs:40-45,200`) ので、RAW からの EXIF 転記は現状どおり無し |
| D5 製本 (決定 7) | **無編集の RAW は元ファイルをそのままコピー** (`src/books.rs:1185-1198` の既存の無加工コピー経路)。編集のある RAW は Full 現像 (High) してから既存の焼き込み経路。本のページ判定 `is_supported_book_image_path` (`src/books.rs:2299-2318`) に RAW 17 種を加え、コピーした RAW をページとして数える。本の中の RAW も通常の RAW と同じ表示 (プレビュー → フル現像) |
| D6 類似索引 | プレビュー (§8 と同じ条件を満たさなければ half 現像を Background で)。**フル現像はしない** |
| D7 360 度高解像度 | Full 現像 (High)。RAW の 360 度は稀なので、既存経路へ Full を差すだけ |
| D8 画像コピー | Full 現像 (High)。既に worker thread 上 |
| D10 remote-web 旧経路 | §10.3 |
| D11 bench | Full 現像。ベンチ内で自前の executor を作る |
| P1 `DimsOnly` | RAW は header probe を使わず `info().developed_dims` を送る |
| P2〜P4 寸法の事前読み | RAW は `info().developed_dims`。image crate の probe に RAW を渡さない |

## 10. mIV Remote

### 10.1 サムネイル

本体と同じ D2 経路なので、§8 がそのまま効く。

### 10.2 ページ (`/api/page`) — フル現像 (決定 6)

Remote のページは本体と同じ結果にするため、**フル現像**を使う (独立レビューも同じ判断)。
プレビューに保存済み編集を掛ける案は、決定 5 に反するので採らない。

必要な設計 (S2 の Remote 部分を組み込む前に、ここを詳細化して独立レビューを受ける):

- **段階を明示して運ぶ**: Remote のページ生成は `thumb_loader::process_load_request` を通る
  (`src/remote_ipc/container.rs:479-515,5665-5677`)。`SourceOnly` は段階の選択ではないので、
  RAW の Full 段を要求に明示的に載せ、ページ生成の single-flight identity (`:1075-1138`) にも含める
- **heavy worker と HTTP worker を現像待ちで塞がない**: RAW ページの job は 2 段に分ける。
  1 段目 (heavy worker) は executor へ submit して worker を返す。現像完了で 2 段目 (編集適用・JPEG 化)
  を同じ job identity と優先度のまま heavy queue へ再投入する。前景の lease と先読みの昇格
  ([web-remote-plan.md §14](web-remote-plan.md)) は job identity で引き継ぐ
- **締め切り**: remote-web 側の `PAGE_RESPONSE_TIMEOUT` を超える現像では、ブラウザ側の再要求で同じ
  job に相乗りできること、需要が無くなった job (lease が 0) は executor の ticket を cancel すること
  (孤児の現像を残さない)
- **HTTP worker**: remote-web 側で IPC 応答を待つ HTTP worker が現像時間ぶん占有されないか
  ([web-remote-plan.md §9.5](web-remote-plan.md)) を確認する。占有されるなら、応答待ちを
  `IpcAdmission` の既存の上限に入れる等の対策を同じ詳細設計に含める

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
| 製本済みの本 | 済 | 既存の本に RAW ページは無い (ページ判定が RAW を含まなかったため)。判定に RAW を加えても既存の本の並びは変わらない。本フォルダに手で置かれた RAW があれば、新たにページとして数えられる (S2 で本の並び順の扱いを確認) |
| 新設定 `raw_develop_parallelism` | 未 | 未リリースなので移行処理は不要 |

## 12. 性能とメモリ (すべて未計測)

- 所要時間: WIC で RAW を開く時間、LibRaw のプレビュー抽出・フル現像・half 現像の時間は **誰も
  測っていない**。S1 でサンプル全件について `info / preview / develop(Full) / develop(Half)` の時間と、
  中断要求から終了までの時間を計測するベンチ (`src/bin/bench_raw.rs`) を作り、結果を本書に記録する
- メモリの目安 (**公式文書の画素当たり係数からの推定**、API-notes.html "Memory Usage"):
  raw 2 B/px + 後処理 8 B/px + 8bit 出力 3 B/px、ハイライト復元等で一時 6〜8 B/px。
  24MP で約 0.3〜0.5 GB、45MP で約 0.6〜1.0 GB、100MP で約 1.3〜2.1 GB。既定の並列 3 で 100MP を
  同時に現像すると約 4〜6 GB。これに `fs_cache` の RGBA (45MP で約 180 MB/枚) と GPU texture が加わる
- perf 計装 (`--perf-log`): `raw/info`, `raw/preview`, `raw/develop_begin|end|cancel|fail`
  (所要時間・寸法・形式・scale・優先度)、`raw/executor_*` (待機・実行・取消中・cancel→終了 ms、
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
  (`scripts/build-portable.ps1` の同梱一覧)、`installer/readme.txt` / `readme_portable.txt` の「ライセンス」節。
  zlib / libjpeg を新たに同梱ビルドする場合は、その notice も同じ文書群へ加える
- アプリ内「バージョン情報」(`src/ui_dialogs/about.rs`) の第三者一覧に `LibRaw (CDDL-1.0)`、
  同梱版、対応ソースへのリンク (`https://mikage.to/mimageviewer/libraw-0.22.2-source.tar.gz`)。
  版は build.rs が `vendor/libraw/VERSION` から焼き込む (FFmpeg の `MIV_FFMPEG_BUILD_ID` と同型)
- 対応ソース: 無改変の公式 tarball をそのまま mikage.to に置く (sha256 を tracked の `.sha256` に記録)。
  shim は MIT で本リポジトリにあり、GitHub で公開済み。手順は新規 `docs/libraw-source-distribution.md`
- 製品ページ `htdocs/mimageviewer/index.html` のライセンス節に LibRaw を追加。
  `htdocs/mimageviewer/manual/formats.html:113-131` の「DNG は標準、他は Store 拡張が必要」を
  「RAW は内蔵」へ書き換え、対応拡張子 (crw / srw を含む) と非対応形式 (Nikon HE/HE*、JPEG XL 圧縮 DNG) を
  書く。`docs/spec.md:1330` も更新
- リリースチェックリスト (CLAUDE.md Phase 2) に「`bash scripts/setup-libraw.sh check`」と
  「対応ソースの配置」を追加。`scripts/bootstrap-vendor.sh` に LibRaw の取得を追加

## 15. テスト方針

- **サンプル**: raw.pixls.us の **CC0 のファイルだけ**を使う (同サイトには CC BY-NC-SA の 146 件が
  混ざるので license URL で厳密に絞る)。git には入れず、tracked の manifest
  `tests/raw-samples.json` (URL・sha256・形式・期待される性質) を正本に、
  `scripts/setup-raw-samples.ps1` が `vendor/raw-samples/` へ取得する。Windows でサンプルが無い場合、
  RAW の統合テストは **skip ではなく失敗**させ、復旧手順を出す (検証できない分岐を残さない)
- 選ぶサンプル (機能で選ぶ): CR3 / CR3 CRAW / CR2 / CRW / 実機の NEF / NEF HE* (非対応の陰性) /
  ARW 圧縮 / ARW lossless / RAF (X-Trans、圧縮) / ORF / RW2 / PEF / RWL / IIQ / SRW /
  スマートフォンの DNG (小さいプレビュー → half 現像経路) / deflate DNG / lossy DNG / 縦位置 (flip 5 と 6) /
  `twidth = 0` で記録される CR3 の JPEG サムネイル
- `raw_decoder`: 全サンプルで `develop(Full)` 寸法 == `developed_dims`、プレビューの向き、非対応判定、
  中断 (callback 経由で `Cancelled`)、壊れたファイル (切り詰め) で `Corrupt` を返し落ちない
- WIC 境界: RAW path / RAW 拡張子の bytes を渡すと WIC を呼ばずに `None`
- 入口 D1〜D11・P1〜P4: TIFF 構造の RAW (DNG / CR2) を渡して、decoder 経路が LibRaw であることを
  型付きの decode source で確認する (image crate の IFD0 誤成功の回帰)
- `RawDevelopExecutor` (fake job で LibRaw 不要): 枠数、High 用の 1 枠、`N = 1` の規則、昇格、待機取消、
  実行中取消の枠占有、**中断できない fake job の最中に High が来たときの待ち時間の上限**、
  設定変更 (増減) の即時反映、submit 元が permit を持たないこと
- `RawPageStore` の状態遷移 (**提示された画素まで検証する**):
  - preview → develop → Static 置換
  - **develop が先に完了し、preview が後から届く** (backlog 投入順・完了順の両方の入れ替え) → 最終が Developed
  - 同じ idx で別ファイル・同じ path の上書き (`source` 不一致) の結果を拒否
  - 現像窓の出入りでの cancel、非対応の終端、失敗後に再要求しない、取消後は再要求する
  - **park 中の完了を捨て、activate 後に再 submit する**、context drop での cancel
  - **別 viewer context の現像結果が sibling の `fs_cache` へ入らない**
  - プレビューの無いページのページ送り (target の half 現像と現像が High で走り settle する)
  - カラー化 / LUT が有効な RAW でプレビュー画素が生で提示されないこと (rendition だけが提示される)
  - `N = 1` で中断できない現像中のページ送り
- 編集 gate: `RawPreview` で消しゴム・補正レイヤー・隠蔽・注釈・SNS 分割・crop の入口 (キーと左パネル) が
  拒否され、Static 後に通ること (handler-level)
- 表示: §7.5 の置換前後の transform 一致
- 実機確認 (利用者が行う): 大きな RAW フォルダのページ送り、差し替えで表示が飛ばないこと、
  進捗表示、編集が現像後に使えること、Remote、製本。エージェントは製品バイナリを起動しない

## 16. 段階

| 段 | 内容 | 受入条件 |
| --- | --- | --- |
| **S1** | `crates/libraw-sys` (shim + cc ビルド、`USE_ZLIB` / `USE_JPEG`)、`setup-libraw.sh`、`raw_decoder` (info / preview / develop / 中断 / 進捗)、明るさの決定 (§5.3.4)、`bench_raw`、サンプル manifest | §15 の `raw_decoder` テストが緑。**lossy DNG と deflate DNG のフル現像が通る**。全サンプルの寸法一致・向き・プレビューと現像の縦横比差・所要時間・中断遅延の表を本書へ記録。core が VC runtime DLL を import しないこと (`check-vcrt-pe-dependencies.ps1`)。ubuntu CI の `cargo check` が通ること。`3fr erf kdc dcr mrw mos mef` の対応とサンプルの有無の報告 |
| **S2** | `raw_format` と WIC 境界の拒否、`RawDevelopExecutor`、入口 D1〜D11・P1〜P4 の振り分け、サムネイル (worker を塞がない half 現像)、ZIP 内 RAW、類似索引、書き出し / コピー / 外部ツール / 製本、Remote (§10.2 の詳細設計を独立レビューしてから) | 入口ごとの回帰テスト、executor テスト。`is_raw_ext` を通らずに RAW を decode する経路が無いことを grep 手順と test で示す |
| **S3** | `RawPageStore`、`FsCacheEntry::RawPreview`、読み込み状態、現像窓、差し替えの layout (§7.5 の全経路)、色の gate、ページ送り / フォルダ移動、編集 gate、進捗表示と先読み行、設定 UI、§7.7 の consumer 点検 | `RawPageStore` の状態遷移テスト (§15 の全項目)、context 分離テスト、UI スナップショット (進捗表示と設定)。`build-dev.ps1` で利用者の実機確認 |
| **S4** | ライセンス文書・バージョン情報・対応ソース・マニュアル・製品ページ・spec・readme・リリースチェックリスト・bootstrap | 文書差分のレビュー。`build-dist.ps1 -NoSign` 相当で同梱物に `LIBRAW-LICENSE.txt` が入ること |

各段は委任前にコミットし、この worktree で codex を同時に 2 本走らせない。master は各段の区切りで
このブランチへ取り込む (app.rs は競合しやすい)。統合は S4 完了後に一度。
S1 の実測 (寸法・向き・中断・codec・形式) は S2 / S3 へ進む前の gate とする。

## 17. 未決事項

### 17.1 利用者の判断が要るもの

1. `3fr erf kdc dcr mrw mos mef` 等の追加 (S1 の調査結果を見て相談)
2. 明るさのパラメータ (S1 の比較結果を見て目視で)

### 17.2 設計レビュー・S1 で詰めるもの

- zlib / libjpeg の調達方法 (§4.1)
- `adjust_sizes_info_only()` の寸法が全形式で現像結果と一致するか (§5.3.2)
- プレビューと現像の縦横比差 (§7.5)
- 進捗の区切り (§5.4.4)
- Remote ページの詳細設計 (§10.2、S2 の Remote 部分の前に独立レビュー)

## 18. 出典

- LibRaw 0.22.2 ソース一式: https://github.com/LibRaw/LibRaw/archive/refs/tags/0.22.2.tar.gz
  (本文の `src/...` / `doc/...` / `libraw/...` の LibRaw 側の行番号はこの tarball 内。
  本リポジトリの `src/...` と区別するため、LibRaw 側は §4・§5 の LibRaw の文脈でだけ使う)
- リリース: https://github.com/LibRaw/LibRaw/releases/tag/0.22.2 / https://www.libraw.org/download
- API: https://www.libraw.org/docs/API-CXX.html / API-datastruct.html / API-notes.html / Install-LibRaw.html
- ライセンス: COPYRIGHT / LICENSE.CDDL / LICENSE.LGPL (上記 tarball)、CDDL-1.0 §3.6: https://opensource.org/license/CDDL-1.0
- LibRaw-cmake の状態: https://github.com/LibRaw/LibRaw-cmake
- MSVC OpenMP の再頒布: https://learn.microsoft.com/en-us/cpp/build/reference/openmp-enable-openmp-2-0-support
- サンプル: https://raw.pixls.us/ / https://raw.pixls.us/json/getrepository.php?set=all
- Rust crate の状態: crates.io (`libraw-rs`, `rsraw`, `libraw_rs_vendor`)、2026-09-27 時点
- Microsoft の Raw 画像拡張が LibRaw ベースであること: Store の説明・報道 (handoff §4)。未検証の二次情報

## 19. 独立レビュー 1 回目 (GPT-6 Sol / xhigh、2026-09-27) の対応

| 指摘 | 判断 | 反映先 |
| --- | --- | --- |
| P1-1 遅れたプレビューがフル現像を上書きする (`src/app.rs:66459-66470`) | 採用 (コードで確認) | §6.1 `RawPageStore::apply_result`、段階の単調性、backlog の置換規則、§15 |
| P1-2 R2 とプレビューの両立が未定義 | 採用。利用者判断 (決定 10) | §2 決定 10、§7.3、§7.7、§15 |
| P1-3 プレビューが無い場合が未定義、CR3 の `twidth = 0` | 採用。利用者判断 (決定 11)。閾値 1024 を撤回、JPEG 寸法はヘッダから | §5.3.3、§6.3、§7.8、§8 |
| P2-4 状態が 3 か所に分散、lifecycle 未定義 | 採用 | §6 全体 (単一の持ち主、lifecycle 表) |
| P2-5 プレビュー無しで ready 扱い、target の現像が admission で止まる | 採用 | §6.3、§7.6 |
| P2-6 layout の主張が現行コードと合わない (Z ズーム含む) | 採用 (コードで確認) | §7.5 |
| P2-7 permit 待ちがサムネイル worker を塞ぐ、`N = 1` の方針 | 採用。permit 方式を executor 方式へ変更 | §5.4、§8 |
| P2-8 Remote A の段階と admission が未設計 | 採用 (A は利用者決定 6) | §10.2 (S2 の Remote 前に詳細設計を独立レビュー) |
| P2-9 consumer 点検の漏れ (readiness / lock / rendition) | 採用 (コードで確認) | §7.7 |
| P2-10 `USE_JPEG` を省くと lossy DNG の機能削減 | 採用 | §4、§4.1、§16 S1 の受入条件 |
| P3-11 敵対的な順序・identity のテスト | 採用 | §15 |
