# RAW の LibRaw 対応 — 設計書

- 状態: **設計 第3版 (独立レビュー 2 回目の指摘を反映)**。2026-09-27 (ClaudeCode Opus 5.5)
  - 第1版 `cb2953119` → GPT-6 Sol / xhigh 独立レビュー 1 回目 (P1×3 / P2×7 / P3×1、全件採用)
  - 第2版 `ce97951ac` → 同セッションで再レビュー (P1×3 / P2×3 / P3×1、全件採用)。対応表は §19
  - 再レビューの判定: **S1 (LibRaw 単体のビルドと decoder) は着手してよい**。fullscreen と executor の
    指摘は S2 / S3 の統合前に直すこと、Remote は予定どおり別途 admission のレビューを受けること
- **S1 完了 (2026-09-27、`3d00650a0`)**: 独立レビュー (GPT-6 Sol / xhigh、実装者とは別セッション) が受け入れ判定。
  実測と判断の記録は [raw-libraw-s1-results.md](raw-libraw-s1-results.md)
- **S2a 完了 (2026-09-28、`a982704b9`)**: 全入口の RAW 振り分け・WIC 拒否・executor 接続・サムネイル受け渡し。
  独立レビュー (別セッション) が 3 回で受け入れ判定、`test-full.ps1` PASS。S2b (Remote) は §10.2 の方針決定待ち
- **S2b 完了 (2026-10-01、`14f96aea3`)**: Remote の RAW (表示ページだけ同期現像、先読みは skip、最後の 1 枚、
  サムネイルは half 現像なし)、protocol v62。独立レビュー (別セッション) が 6 回で受け入れ判定、`test-full.ps1` PASS。
  次は master の取り込み (利用者指示)、その後 S2c
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
  (`src/wic_decoder.rs:64-71`) に、**`crw` (Canon の旧形式)・`srw` (Samsung NX)・`3fr` (Hasselblad)・`erf` (Epson)・`kdc` / `dcr` (Kodak)・`mrw` (Minolta)・`mos` (Leaf) を加えた 23 種**。`mef` (Mamiya) は CC0 のサンプルが無く検証できないので入れない
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
8. 対象拡張子に `crw` / `srw` を加える (2026-09-27)。さらに `3fr` / `erf` / `kdc` / `dcr` / `mrw` / `mos` を加える (2026-09-29。`mef` はサンプルが無いので除外)
9. フル現像の保持はしない。再処理でよい (2026-09-27)。
   意味: **追加の保持 (LRU・ディスク) を作らない**。既存の keep set (`prefetch_back` / `prefetch_forward`、
   `src/app.rs:60569-60588`) の中で `fs_cache` に残る通常の保持はそのまま (AI の final cache と同じ扱い)
10. **カラー化・LUT が効いているページ**は、フル現像が終わるまで既存と同じ
    「サムネイルにカラー化・LUT を掛けた低解像度の代役」を出す (Q1 案A、2026-09-27)
11. **埋め込みプレビューが無い RAW** は、フルスクリーンでは「現像中」を出してフル現像を待つ。
    サムネイルだけ半分の解像度の現像 (以下 half 現像) で作る (Q2 案A、2026-09-27)
12. **現像の明るさ**は既定で「埋め込みプレビューの明るさに合わせる」。環境設定で「補正なし」へ切り替え
    られる (比較画像を見て利用者が選択、2026-09-27)
13. **Remote の RAW は「表示するページだけ、その場で現像」** (§10.2.2 の案B)。Remote では RAW の先読み現像を
    しない。最後に現像した 1 枚だけを Remote 用に保持する (決定 9 の例外、2026-09-29)。表示位置からの先読み
    現像は将来の拡張 (§10.2.3)
14. **Remote のサムネイルでは half 現像をしない** (決定 11 を PC のサムネイルに限定、2026-09-29)。使えるプレビューが
    あれば寸法にかかわらず使い、無ければ PC 側で作った catalog のサムネイル、それも無ければ既存の代替表示
15. 既知の差として受け入れる (2026-09-29): 見開きを含め、RAW を開くたびにフル現像を待つのは WIC 経路でも同じ
    構造だった (WIC も最初のフレームをフル解像度で同期にデコードする、`src/wic_decoder.rs:200`。所要時間は未計測)。
    WIC が埋め込みプレビューを返していた一部の DNG (S1 の Ricoh GXR 640×480、Pixel 4 XL 672×502) は、
    正しくフル現像するぶん遅くなる。Remote は RAW の先読みをしないので、Store 拡張を入れていた環境より
    めくりで待つ場面があり得る

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

- 新モジュール `src/raw_format.rs` が `RAW_EXTENSIONS` (23 種) と `is_raw_ext(&str)` /
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
(`pub(in crate::raw)`)。外部 (bin を含む) から現像する手段は executor の公開 API
(`submit` → ticket → 結果) だけにする。S1 の `bench_raw` もこの API を使う (§16)。

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
  `unpack_thumb_ex(i)` の生バッファを使い、次の規則で向きを決めて自前で適用する:
  - `tflip` が **0 でも 0xffff でもない**ならそれを使う。それ以外は `sizes.flip` を使う。
    **`tflip = 0` は「向き不明」と区別できない**: LibRaw は TIFF IFD を 0 で初期化し、Orientation タグが
    あるときだけ `t_flip` を書き (`src/metadata/tiff.cpp:631`、Orientation 1 も 0 になる)、それをそのまま
    `thumbs_list` へ写す (`tiff.cpp:2398`)。S1 で Nikon Df の縦位置 NEF (`sizes.flip = 6`、プレビューの
    IFD に Orientation 無し) が第2版の規則では横倒しになった (実装担当の報告、2026-09-27)
  - 向きを当てた後のプレビューの縦横 (長辺が幅か高さか) を `developed_dims` と比べ、両方が正方形に
    近くない (長辺と短辺の差が長辺の 5% 超) のに縦横が食い違えば、**そのプレビューは使えない**
    (`RawError` ではなく「使えるプレビューが無い」扱いの型付き理由 `OrientationMismatch`)。
    横倒しの絵を出さないための検査で、180 度の食い違いはこの検査では分からない (既知の限界)JPEG 内の EXIF Orientation は**読まない** (`dcraw_make_mem_thumb` は EXIF の無い
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
| 明るさ | **既定 = プレビューに合わせる** (決定 12)。設定で「補正なし」へ切替 | 自動補正なし (`bright = 1`) で現像し、リニア光の輝度中央値を埋め込みプレビューと比べた倍率を LibRaw の `bright` (sRGB ガンマの前に掛かる線形倍率) として出力時に掛ける。demosaic はやり直さない。倍率は 1/8〜8 に制限。使えるプレビューが無い / 中央値が 0 のときは自動補正 (thr 0.001) で代用。S1 の 20 サンプルで倍率 1.00〜3.48 (中央値 2.10)、制限・代用ともに 0 件、プレビューとの平均輝度差 0.022 (自動 0.001 は 0.066)。詳細は [raw-libraw-s1-results.md](raw-libraw-s1-results.md) |
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
  - `Normal`: フルスクリーンの先読み現像、プレビューの無い RAW のサムネイル用 half 現像 (可視範囲、§8)、
    Remote の先読みページ
  - `Background`: 類似索引の half 現像
- 同時実行の上限: `N ≥ 2` のとき `Normal + Background` は `N - 1` まで (High 用に 1 枠を必ず空ける)、
  `Background` は 1 まで。`N = 1` のときは予約できないので次の規則にする:
  - 待機列は優先度順 → 受付順。worker が空いたら先頭を取る
  - High が待機列に入った時点で実行中が Normal / Background で、かつその job が
    「もう要らない」(現像窓の外、取消済み view、等) なら cancel する。要る job は止めない
  - 中断できない区間 (§5.4.3) にある job は cancel しても実際の終了まで枠を占有する。
    保証するのは**順序**だけ: 実行中の 1 job が終わった次に、待機列の先頭である High が必ず取られる。
    LibRaw には中断点の無い区間があるので、待ち時間の上限は保証しない。これを「中断できない fake job の
    最中に High と Normal が来たとき、次に High が実行される」テストで固定する
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

| RAW の状態 | 意味 | `waiting_for_display()` | プレビュー要求を出すか (`needs_load_request`) | fullscreen に出してよい画素 |
| --- | --- | --- | --- | --- |
| `PreviewNotRequested` | `RawPageState` はあるが生きた preview 要求が無い (park 後など) | true | **出す** | サムネイル (既存 fallback) |
| `PreviewPending` | 生きた preview 要求がある | true | 出さない | サムネイル (既存 fallback) |
| `PreviewShown` | プレビュー画素がある | false | 出さない | プレビュー。色処理待ちなら色忠実 rendition (§7.3) |
| `PreviewAbsent` | 使えるプレビューが無い。フル現像待ち | **true** | 出さない | **無し**。サムネイル・rendition も出さず「RAW 現像中」だけ (決定 11) |
| `Developed` | Static がある | false | 出さない | 既存の表示優先順位 |
| `Terminal` | プレビューも現像も得られない (失敗・非対応でプレビュー無し) | false (`LoadFailed` と同じ) | 出さない | 既存の失敗表示 |

`waiting_for_display()` (「まだ表示できない」) と `needs_load_request` (「要求を出すべき」) は
別の問いなので別の関数にする。既存の `ensure_fs_page_load` は `NeedsLoad` のときだけ要求を出す
(`src/app.rs:8003-8012`, `66824-66834`)。RAW では `PreviewNotRequested` だけを `NeedsLoad` に写す。

`PreviewAbsent` でサムネイルを出さないのは決定 11 のため。サムネイルは half 現像から作るので、
fullscreen に出すと「half 現像をプレビュー代わりに出す」(利用者が選ばなかった案B) と同じになる。
カラー化 / LUT の有無にかかわらずこの規則が優先する。

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
| viewer context の park | 既存の `fs_pending` drain (`src/app/viewer_context_registry.rs:1680-1682`) と同じ場所で、`RawPageStore` の全 ticket を cancel し `develop = Idle`。**preview も**: `Requested` なら request_id を失効させて `PreviewNotRequested` へ戻す (drain された `fs_pending` の要求はもう生きていない)。park 中に届いた結果は request_id 不一致で捨てる。activate 後は `needs_load_request` と現像窓を再評価して出し直す |
| viewer context の mount / swap | bundle の `swap_field!` 一覧に `RawPageStore` を含める。結果の送り先 channel も context 所有なので、sibling context の結果を消化しない |
| viewer context の drop | discard hook で全 ticket を cancel (`src/app/viewer_context_registry.rs:1304-1334` と同じ場所) |
| idx 空間の差し替え (`invalidate_idx_state_and_queues`) / items generation の変更 | `RawPageStore` を clear (ticket cancel) |
| fullscreen close | `fs_cache` の clear と同じ経路で `RawPageStore` を clear |
| 同じ path のファイルが外部で上書きされた | 次の要求で `source` (size / mtime) が変わるので、古い結果は `apply_result` で拒否される |
| **items generation を変えずに、ある idx の `fs_cache` を直接消す経路** | 例: detached viewer の同期で同じ idx の item key が変わったとき、generation を進めずに `fs_cache` / `fs_pending` / backlog を消す (`src/app.rs:51660-51673`)。`RawPageState` が `Developed` のまま残ると「表示可能だが texture が無い」になり、要求も出ない。**idx 単位の破棄を 1 つの操作 `discard_fs_page(idx)` に集め、`fs_cache` / `fs_pending` / backlog / `fs_margin_bbox_cache` / `RawPageStore` を一緒に消す**。S3 の最初に `fs_cache.remove` / `retain` / `clear` の全箇所 (HEAD で 31 箇所) を列挙し、idx 単位のものをこの操作へ、全体のものを `RawPageStore::clear` と対にする |

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
- `PreviewAbsent` のページには rendition も出さない (§6.3。サムネイルが half 現像由来のため、決定 11 が優先)
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

- ページ送りの完了判定 (`src/ui_fullscreen.rs:11111-11142`) は既存どおり「実際に texture が解決できて
  提示されたか」を使う。RAW では §6.3 の「fullscreen に出してよい画素」だけが解決対象になる:
  - `PreviewShown` → プレビュー、色処理待ちなら rendition の提示で settle
  - `PreviewAbsent` → サムネイル・rendition では settle しない。**`Developed` の提示 (色処理待ちなら
    完成した final composite) か、`Terminal` で settle する** (決定 11)
- 現像の admission は既存の `FsPageTurnWorkAdmission` に従う:
  - `All`: 現像窓の通常の submit
  - `NavigationTargetMaterializationOnly`: **target ページのフル現像を High で許可する**
    (これが無いと、プレビューの無い target が settle できないまま止まる。独立レビュー P2-5)。
    target 外の現像は開始しない。target のサムネイル用 half 現像は fullscreen の settle に使わないので、
    ここでは要求しない
  - `Deferred` (ready な rendition を描いている): 現像を開始しない
- 決定 11 の帰結: **プレビューの無い RAW が並ぶフォルダでキーを押しっぱなしにすると、1 ページごとに
  フル現像を待つ**。ページの並びは飛ばさない (R1) ので、ページ送りの速さは現像時間で決まる
- フォルダ移動の lock (`poll_fs_nav_lock`、`src/ui_fullscreen.rs:11781-11806`) は、ページ送りと
  **同じ「表示してよい画素が実際に提示されたか」の述語**で解除する (独立レビュー 2 回目 P1-2)。
  既存の lock は色処理が必要なページで完成した final composite まで待つが、RAW の `PreviewShown` で
  それを待つと、決定 10 で出すはずの rendition の代わりに旧フォルダの holdover が数秒残る。
  RAW では:
  - `PreviewShown`: 色処理不要ならプレビュー、必要なら rendition が提示された frame で解除。
    生のプレビュー画素を色処理前に出さない R2 の gate は維持する
  - `PreviewAbsent`: `Developed` (色処理が必要なら完成した final composite) の提示、または `Terminal` で解除
  - 非 RAW のページの解除条件は変えない
  lock 解除とページ送りの settle が別々の条件式を持つと、片方だけ待ち続ける (既存の
  `fs_display_bypasses_final_pipeline` を描画側と lock 側で共有している理由と同じ、
  [display-pipeline.md §2.3](display-pipeline.md))。RAW の述語は 1 つの関数にし、両方から呼ぶ

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
- プレビューの無いページ (`PreviewAbsent`) は、サムネイルの有無にかかわらず、既存の中央の「読込中」の
  位置に「RAW 現像中 NN%」を出す (決定 11。サムネイルは描かない)
- 既存の先読み状況の行 (`draw_fs_prefetch_status_row`、AI 用) と同じ部品・同じ設定
  (`fullscreen_prefetch_status_visible`) で、**「RAW 現像」行**を足す。ドットは現像窓の
  各ページの `Ready / Active / Missing` (既存の `FsPrefetchPageState`)

### 7.9 メモリ上のキャッシュ

- フル現像の結果は `fs_cache` の Static としてだけ持つ。ディスクにも保持 LRU にも持たない (決定 9)
- 現像窓を出ても、keep set (`prefetch_back` / `prefetch_forward`) の中なら Static は残る。これは
  同じ寸法の JPEG と同じ扱いで、RAW だけプレビューへ降格して解放する処理は作らない。
  決定 9 が除いているのは追加の保持 (LRU・ディスク) であり、この通常の保持ではない
- keep set を出た後、または fullscreen を閉じて開き直した後は、現像はやり直しになる

## 8. サムネイル

- D2 (`load_one_cached` / `decode_zip_chain`) で RAW を最初に振り分け、`preview()` を使う。JPEG の
  プレビューは既存の TurboJPEG DCT スケール経路へ bytes で渡す。向きは §5.3.2
- プレビューを使う条件: 使えるプレビューがあり、その長辺が `min(要求された display_px, developed の長辺)`
  以上。満たさなければ half 現像にする。判定は要求とファイル内容だけで決まる
- half 現像は **サムネイル worker の上で待たない** (独立レビュー P2-7)。また **executor の枠では
  現像だけを行い**、縮小・WebP 化・cache 保存・統計・完了通知はサムネイル worker に戻す
  (枠は RAW 現像専用で数が少ないため。独立レビュー 2 回目 P2-5)。流れ:
  1. サムネイル worker が「使えるプレビューが無い」と判定したら、executor へ half 現像を submit し、
     **`ThumbMsg` を 1 通も送らずに**次の要求へ進む。`requested[idx]` は立ったまま (二重要求を防ぐ)
  2. 現像が終わったら executor の thread は、画像を載せた後続要求 (`LoadRequest` に
     `RawHalfDeveloped { image, developed_dims }` の source を付けたもの) を**既存のサムネイル queue へ
     入れて**枠を返す
  3. サムネイル worker がその後続要求を通常どおり処理する: 縮小、向き (現像結果は flip 適用済み)、
     `CacheDecision` と cache 保存、統計、**第 1 シグナル (画像) と第 2 シグナル (`finalized`)、
     `gen_done`**。既存の 2 段通知 (`src/thumb_loader.rs:3554-3573`, `3693-3707`、UI 側
     `src/app.rs:38758-38790`) をそのまま使う
- half 現像の ticket は **idx ごとに**持つ。所有者は viewer context のサムネイル状態
  (`requested` と同じ所有境界) で、`raw_thumb_develop: ItemsGenerationMap<Ticket>` とする:
  - keep range が動いて範囲外になった idx の ticket は cancel し、既存の `canceled: true` の `ThumbMsg`
    を UI へ出して `requested` を抜く (既存の STALE 取消と同じ扱い、[async-architecture.md §3.4](async-architecture.md))
  - フォルダ移動・items generation の変更・context の park / drop では map ごと cancel する
  - 後続要求が queue に入った時点で ticket は map から外す (以後は通常の要求と同じ扱い)
- 優先度: 可視範囲は Normal、それ以外の keep range の先読みは submit しない (可視に入ったら submit する)。
  fullscreen の settle には half 現像を使わないので (§7.6)、High の half 現像は無い
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
| D1 の permit 境界 (S2a で確定) | フルスクリーンの worker は `FsPageLoadScheduler` の permit を取ってから `decode_canonical_image` を呼ぶ (`src/app.rs:59859`, `60070`)。**RAW の現像待ちをこの permit の中に置かない**。canonical loader を「source の解決 (ZIP entry の読み出し・verified bytes)」と「decode」の 2 段 API に分け、worker は source 解決までを permit の中で行い、RAW で現像が要る場合は **permit を明示的に drop してから** executor へ submit して結果を待つ。permit の役割 (書庫の読み出し・通常 decode の同時実行数の制限) は変えない。取消は fs ticket の cancel flag を executor の job と共有して伝える (待っている worker は結果 channel の受信で起きる。sleep / 定期確認は使わない)。**`FsPageLoadTicket::cancel()` は、scheduler に request の記録が残っているかどうかにかかわらず、ticket が所有する cancel flag を必ず立てる** (S2a で判明: permit を drop すると記録が消え、`cancel_request` は記録が無いと flag を立てずに戻る (`src/fs_page_load_scheduler.rs:383`) ため、permit 解放後の取消が現像へ届かなかった)。scheduler の枠の会計 (Waiting の除去、Running → Cancelling) は記録があるときだけ行う。現像待ちの間 `FsPageLoadScheduler` の running には数えない。現像の同時実行は executor の枠が別に制限する。プレビュー (S3) は安いので permit の中で行ってよい。S2a の暫定: フルスクリーンは表示ページ High / 先読み Normal で Full を待つ (S3 で preview → develop に置き換える) |
| D3 | Preview (サムネイル用途、§8 の条件) |
| D4 / D9 | サムネイルと同じ (§8) |
| D5 書き出し / 外部ツール | Full 現像 (High)。書き出しは `SrcFormat::Other` でメタデータ転記を拒否している (`src/save_with_metadata.rs:40-45,200`) ので、RAW からの EXIF 転記は現状どおり無し |
| D5 製本 (決定 7) | **無編集の RAW は元ファイルをそのままコピー** (`src/books.rs:1185-1198` の既存の無加工コピー経路)。編集のある RAW は Full 現像 (High) してから既存の焼き込み経路。本のページ判定 `is_supported_book_image_path` (`src/books.rs:2299-2318`) に RAW の全拡張子を加え、コピーした RAW をページとして数える。本の中の RAW も通常の RAW と同じ表示 (プレビュー → フル現像) |
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

#### 10.2.1 現状 (2026-09-27 の調査、HEAD `d78e20301`)

- **remote-web の HTTP worker は IPC の往復のあいだ塞がる**。12 本の `remote-http-*` が要求を同期で処理し
  (`crates/remote-web/src/main.rs:108-133`)、`api_page` は `IpcAdmission` の permit を持ったまま IPC 応答を待つ
  (`crates/remote-web/src/http.rs:3683-3697`)。`IpcAdmission` は待機列の無い try-semaphore で、全体 6 / heavy 4 /
  prefetch 3 (prefetch は各上限の 1 つ手前まで)。取れなければ 503 `ipc_busy` + `Retry-After: 1`
  (`http.rs:222-395`, `3845-3870`)。ページの IPC 締め切り `PAGE_RESPONSE_TIMEOUT` は 10 分
  (`crates/remote-web/src/ipc_client.rs:43`)。締め切りを過ぎても core の仕事は止まらない (`:40-42`)
- **core の heavy worker もページ生成のあいだ塞がる**。source の decode は single-flight が起こす専用 thread で
  走るが、heavy worker はその完了を Condvar で待つ (`src/remote_ipc/container.rs:1349-1378`)。
  heavy worker 数は設定の並列数 (最小 1、`src/remote_ipc/pipe.rs:973-984`)。heavy queue は
  Foreground / Interactive / Prefetch の 3 lane で、prefetch は `active < worker_count - 1` のときだけ取り出す
  (`src/remote_ipc/heavy_queue.rs:205-223`)
- **取消は別経路**: ブラウザの `abort()` は fetch を止めるだけで、実際の取消は `POST /api/page/demand` の
  `release` が `page_jobs` の cancel token を立てる (`crates/remote-web/web/app.js:1052-1078`、
  `src/remote_ipc/page_jobs.rs:258-284`)。昇格も同じ経路 (`promote`)。heavy worker は page job の token を
  render へ渡す (`pipe.rs:1106-1119`)
- **worker を返して後で続ける前例は、ページ経路には無い**。AI / 書庫変換の long job は「開始 → job id を即返す →
  ブラウザが poll」で worker を持たない (`src/remote_ipc/ai_job.rs:232-417`、`archive_job.rs:1100-1226`)。
  long job は `SessionOperation` を job の終わりまで持ち、session の drain を開いたままにする
- **session drain**: `begin_drain` が全 operation の cancel flag を立て、`try_finish_drain` は operation が
  0 になるまで待つ (`src/remote_ipc/session.rs:701-789`)。ページの `Work` は `SessionOperation` を運ぶので、
  実行中・待機中のページも drain を開いたままにする
- 同じ source を何度も要求する: 補正プレビューのスライダー、端末の画質 (target_px) 違い、見開きの自動
  トリムの相方。JPEG ならやり直しても安いが、RAW は 1 回 0.3〜13 秒 (S1 実測、release)

#### 10.2.2 設計 (第3案 = 利用者決定 13 の案B。第1案・第2案は却下、§20.1 / §20.2。第3案の初回レビュー対応済み、§20.3)

**方針: Remote の RAW は「表示するページだけ、その場で現像」する。Remote では RAW の先読み現像をしない。**
表示ページの要求は既存のページ生成と同じく同期で応答する (PDF や大きな画像と同じ形)。

**(1) RAW の依存を先に列挙する (page_inner の入口)**

- `page_inner` は、要求ページの source と、見開きの自動トリムの相方の source (scoped partner が読むもの、
  `src/remote_ipc/container.rs:3837-3881`, `5404-5430`) を、**どちらの読み込みも始める前に** 解決し、
  RAW で現像が要るもの (= (3) の cache に無いもの) の集合 `raw_deps` を作る
- 要求の実効優先度は、この時点で page job の registry から読み直す (`effective_page_priority`、
  `src/remote_ipc/pipe.rs:1108-1119` の dispatch 時の snapshot ではなく、決定の直前の値)
- 実効優先度が Prefetch で `raw_deps` が空でなければ、**何も読み込まずに** `MediaErrorCode::RawPrefetchSkipped`
  を返す (scoped partner も起動しない)。空なら通常どおり生成する (cache に当たる再要求など)
- 実効優先度が Foreground なら、`raw_deps` を (2) の RAW single-flight で現像してから通常の生成へ進む
  (相方も同じ flight を使う)
- **要求内の pin**: (1) で cache に当たった RAW も、(2) で現像した RAW も、その要求の間は `Arc` で保持する
  (`RawDepPins`)。要求ページと scoped partner の source 読み込みは、cache を引き直さずに pin を使う。
  cache (1 件) は要求をまたぐ再利用のためだけにあり、要求の途中で他の完了に追い出されても影響しない
  (設計レビュー第3案 2 回目 P1-1)
- 実効優先度は、`page_inner` に page job の優先度 accessor (registry から現在値を読む関数) を渡して、上の決定の
  直前に読む (`src/remote_ipc/pipe.rs:1108-1119` の dispatch 時の値ではなく)

**(1b) 書庫の代表の選び方 (S2b 実装中に確定、2026-10-01)**

- 代表の **特定** (identification) は、ZIP の中央ディレクトリの情報だけで行う: 代表の順序、画像の拡張子、
  暗号化の有無、対応する圧縮方式。入れ子 ZIP の器は、既存の上限付き・取消可能な入れ子 ZIP の cache で展開してよい
  (S2b 以前から JPEG の書庫でも行っていた処理)。**RAW entry の中身 (payload) は、先読みの skip 判定と AI の capacity
  確保より前には読まない・展開しない**
- **代表の順序は、アドレスの種類ごとに既存の loader と同じにする** (非 RAW の書庫の表示を変えないため):
  書庫 root の `File` は **中央ディレクトリの index 順** (`src/zip_loader.rs:1727` の既存 loader)、`ZipDirectory`
  等の既存の並べ替えを使う経路はその並べ替え。第1版の (1b) に書いた「名前順」は既存 loader と食い違う誤りで、
  独立レビューの指摘で訂正した (2026-10-01)
- 候補は **安定した位置 (cursor)** で表す: 入れ子の書庫の連なりを含めた entry の index。「次の候補」はこの cursor を
  単調に進める。名前で探し直さない (同名・大小文字違い・区切り文字違いの entry があっても同じ候補を読み直さない)
- Remote AI は、ページとしての有効性 (`ZipDirectory` 等の非ページの拒否) を **source の準備より前** に確かめる。
  無効な要求で RAW の現像や capacity の確保をしない
- payload の読み込みは、前景なら (2) の flight の中で、AI なら capacity 確保の後で行う。読めなかった場合
  (破損など) は、**同じ要求の中で代表の順序の次の候補へ進む** (既存の loader が「読めない entry を飛ばして次を
  表示する」のと同じ結果にする)。次の候補が RAW なら同じ規則を繰り返す

**(2) RAW single-flight (`RemoteRawFlights`、App-global、Remote のページと Remote AI が共有)**

- key (`RemoteRawIdentity`): 正規化 path + **高精度 mtime (100ns 単位の FILETIME をそのまま)** + file size +
  ZIP entry (+ 入れ子 prefix) + 明るさの値。target_px は含めない。秒精度の mtime は使わない
  (同じ秒・同じ size の差し替えを見逃すため、`src/ui_helpers.rs:911-916`)
- 1 key の状態は enum: `Submitting { flight_id, waiters }` / `InFlight { flight_id, ticket, waiters }` /
  `Cancelling { flight_id }` / `Done { flight_id, result: Arc<..>, waiters }`。`Done` の結果は waiter が全員
  受け取るまで flight が保持し (per-flight の `Arc`)、最後の waiter が受け取った時点で flight を消す。
  (3) の cache へは完了時に別途 `Arc` を置く (cache が先に置き換わっても waiter は flight から受け取れる)
- **submit の順序**: lock の中で `Submitting` を登録 → lock の外で executor へ submit → lock の中で
  `Submitting` を `InFlight` に置き換える。submit より先に完了 callback が来た場合 (即時完了・executor 停止中の
  即時エラー) は、callback が `Submitting` の flight_id を見て `Done` / 失敗へ進め、後から来た ticket の登録は
  捨てる。submit が Err を返した場合は、その flight を失敗で終わらせ waiter へ typed error を返す。
  submit 前の完了のテストを置く (P2)
- **すべての状態遷移は flights の lock の中で決め、executor の `RawTicket::cancel` / `promote_to_high` は
  lock の外で呼ぶ** (queued job の cancel は完了 callback を同期で呼び、callback が lock を取るため。
  `src/raw/executor.rs:53-63`, `267-287`)
- 状態ごとの join / leave (すべて lock の中で遷移を決め、ticket 操作は lock の外):

  | 状態 | join (waiter 追加) | leave (waiter 離脱、最後の 1 人) | ticket / 完了の到着 |
  | --- | --- | --- | --- |
  | 無し | `Submitting{waiters:1}` を作り submit | — | — |
  | `Submitting{waiters, cancel_requested}` | waiters+1。`cancel_requested` を戻す | `cancel_requested = true` (ticket はまだ無い) | ticket 到着: `cancel_requested` なら `Cancelling` にして lock 外で即 cancel、でなければ `InFlight`。完了が先に到着: `Done` / 失敗へ (後から来た ticket は、その job がもう終わっているので何もしない) |
  | `InFlight{ticket, waiters}` | waiters+1 (前景なら lock 外で ticket を High へ) | `Cancelling` にして lock 外で ticket を cancel | 完了: `Done` へ |
  | `Cancelling` | 新しい flight_id で `Submitting` を作り直して submit (古い完了は flight_id 不一致で捨てる) | — | 古い完了: 捨てる |
  | `Done{result, waiters}` | 結果をそのまま使う (waiters は増やさない) | waiter の lease が減るたびに数え、0 で flight を消す | — |

- submit が Err を返した場合は、その flight を失敗で終わらせ、waiter へ typed error を返す
- `Done` の結果は typed: `Ok(Arc<DevelopedRaw>)` / `Err(RawError)`。waiter が 0 の `Done` は直ちに消す
- **waiter は participant lease** で表す。lease は結果の受け取り・エラー・取消 (起床前・起床後のどちらでも) の
  いずれで終わっても Drop で waiters を 1 減らし、`Done` なら 0 で flight を消す。完了通知の直後に取消された
  waiter でも結果を pin し続けない (既存 source waiter の形、`src/remote_ipc/container.rs:1359-1370`)。テストする
- **仕事の会計は key と別に flight_id で持つ** (`outstanding: map<flight_id, FlightWork>`)。key の現在の flight は
  `Cancelling` の置き換えで新しい flight_id に移るが、古い flight の executor job は完了 callback (または submit の
  失敗) が来るまで `outstanding` に残る。`outstanding` からの削除のたびに capacity 待ちへ `notify_all` する
- テスト: ticket 到着前の取消、submit 中の join、即時完了・即時失敗、`Cancelling` 中の join
- 完了 callback: lock の中で「key の現在の状態が同じ flight_id か」を確かめ、同じときだけ `Done` に進めて
  結果を (3) へ publish し、待っている waiter を起こす (`notify_all`)。違う (= 新しい flight に置き換わった) なら結果を捨てる。
  既存 source single-flight の pointer 照合 (`src/remote_ipc/container.rs:1382-1421`) と同じ考え方
- waiter は flights の Condvar で待つ。page job / AI job の取消 (release・接続断・drain・service stop・AI の
  supersede) は `AtomicBool` を立てるだけで Condvar を起こさない (`src/remote_ipc/page_jobs.rs:258-280`,
  `342-377`) ので、**既存の source single-flight と同じく上限付きの `wait_timeout` (50ms) で待ち、起きるたびに
  token を確認する** (`src/remote_ipc/container.rs:1351-1378` と同じ形。try_lock + sleep ではない)。
  取消を確認したら flight を離脱して `Cancelled` で返る。取消から離脱までの遅れは最大でこの間隔
  (設計レビュー第3案 2 回目 P1-2。第3案初回版の「poll しない」は撤回)
- 取消された flight の結果は、その時点でまだ waiter がいるときだけ publish する

**(3) 最後に現像した 1 枚の cache (利用者決定 13)**

- App-global に **1 件だけ**。値は `Arc` の現像済みラスタ (flip 適用済み 8bit RGB、45MP で約 135 MB)
- (2) の完了で置き換える。PC のフルスクリーンからは使わない。Remote service の停止で消える

**(4) 明るさの設定**

- 明るさは Remote のページ生成が参照する live な `AdjustmentRenderSettings` (`src/settings_db.rs:408-422`) に
  加えて publish する。**1 回のページ生成・AI job の中では、最初に読んだ 1 つの値を decode と全 cache key に
  使う** (途中で読み直さない)。Remote AI の decode 経路が起動時 snapshot を読んでいる箇所
  (`src/remote_ipc/container.rs:4124-4145`, `4449-4479`) も live 値に揃える
- RAW の source のときは、source の識別に **高精度 mtime** (100ns 単位) を使う。現行の source / composite /
  自動トリムの key は秒精度 (`src/remote_ipc/container.rs:5714-5719`, `1025-1052`, `5865-5906`) なので、RAW の
  ときはこれらの key にも同じ高精度の値を入れる (同じ秒・同じ size の差し替えで古い composite に当たらない)。
  Remote AI も同じ: prepared identity と native / 結果の key、**完了時の再確認** (`src/remote_ipc/container.rs:4120-4133`,
  `4414-4440`) を同じ高精度の値で行う。差し替えのテストを AI にも置く
- RAW の source のときだけ、明るさを次の key に含める: (2) の `RemoteRawIdentity`、source single-flight
  (`container.rs:1063-1073`)、ページ composite cache (`:1023-1033`)、**自動トリム bbox cache**
  (`RemoteAutoTrimCacheKey`、`:1046-1052`)、Remote AI の結果 identity (`:1594-1617`)、AI native cache key
  (`:1577-1592`)
- ブラウザ側の page resource cache は変えない: 明るさは本体側の設定で、本体で設定を変えるには Remote の
  操作権を本体へ戻す必要があり、再接続時のセッション取得で端末の cache は破棄される
  ([web-remote-plan.md §12.16](web-remote-plan.md) の「本体側の変更はセッション取得で検知する」契約)

**(5) 先読みの「スキップ」(wire / remote-web / Web UI)**

- `MediaErrorCode::RawPrefetchSkipped` を加え、protocol version を上げる
- remote-web はこれを **HTTP 204 (本文なし) + ヘッダ `X-mIV-Page-Skip: raw-prefetch`** に写す。503 系の
  「一時的に混雑」とは別物で、再試行の対象にしない。204 にも通常のページ応答と同じ session / remote state
  generation のヘッダを付け、`Cache-Control: no-store` にする
- Web の `fetchPageResource` (`crates/remote-web/web/app.js:16068-16107`) は現在 204 を成功として画像の
  identity ヘッダを要求し空の blob を作る。**session と remote state generation の検証は従来どおり先に行い**、
  その後で skip ヘッダを判定して、画像の identity / blob の処理だけを飛ばし typed な skip を `runJob` へ返す。
  検証に通らない 204 は既存の session エラー / 古い generation として扱う (テストする)
- Web UI (`crates/remote-web/web/page-coordinator.mjs` と `app.js` の `PageDemandAdapter.runJob`):
  - job の結果に `SKIPPED` を足す (FAILED / ABORTED と別)。SKIPPED の resource key は、その表示計画の
    間は再び先読み対象にしない (reconcile が同じ先読みをすぐ出し直さない。
    `page-coordinator.mjs:246-285`, `389-435`)
  - SKIPPED の job に表示の需要 (display member) が付いていた、または返答までに前景へ昇格していた場合は、
    **ただちに同じ resource の前景要求を出す** (表示グループは pending のまま、失敗にしない)。
    coordinator は skipped の job を消し、表示需要があれば新しい前景 job を作る。表示計画の範囲の
    「先読みしない」印は resource key に残す。昇格が応答の前か後かの両方をテストする
  - telemetry は `page_prefetch` の `skip` として記録し、`failed` にしない (`app.js:1139-1152`)
- 前景へ昇格した後に届いた `RawPrefetchSkipped` (core が (1) で読んだ時点ではまだ Prefetch だった場合) も
  同じ規則で前景要求になる

**(6) 前景の admission**

- 現行の `IpcAdmission` は heavy の最後の 1 枠を先読みからしか守っていない (`crates/remote-web/src/http.rs:292-317`)。
  サムネイルは通常の heavy permit を使う (`:3530-3557`)
- **サムネイルに専用の admission class を設け、heavy の最後の 1 枠を使えないようにする** (先読みの class を
  流用しない。流用すると先読みの別枠まで消費する)。ブラウザ側のサムネイルの同時取得はもともと 3 本なので
  (`crates/remote-web/web/app.js:153-156`、`command-core.mjs:2527-2533`)、通常の Web のサムネイルの並列度は下がらない
- **それでも枠は埋まり得る** (サムネイルが先に入った後に前景 3 本、前景 4 本など)。そこで保証する性質を
  「前景ページが 1 回で permit を取れる」ではなく、**「まだ表示に必要なページを、枠の混雑で最終失敗にしない」**
  にする: Web の前景ページ要求は、**混雑の 503 (`ipc_busy` / `admission_busy` / `raw_busy`)** に対しては、
  その job に**表示の需要がある間は**再試行回数の上限 (`FOREGROUND_ADMISSION_RETRY_LIMIT`、
  `crates/remote-web/web/app.js:1080-1116`) に数えずに再試行を続ける (間隔は既存の backoff、上限 2000ms)。
  **それ以外の 503 (core の `MediaErrorCode::Busy` = `miv_media_error` を含む) は、従来どおり上限 3 回の再試行**
  (`crates/remote-web/src/http.rs:3986-4003`、`app.js:1103-1116`)。非 503 のエラーも従来どおり。両方の規則をテストする
- **「表示の需要がある」の判定は job の priority や abort signal では行わない**。coordinator は表示需要と先読み計画の
  需要のどちらかがある間 job を保持し、昇格は降格しない (`crates/remote-web/web/page-coordinator.mjs:214-229`,
  `346-348`, `389-435`) ので、それでは表示が離れた後も再試行が続き得る。coordinator に「この resource に保留中の
  表示需要があるか」を返す関数を足す。**最後の表示需要が消えた時点で、coordinator は前景の wire job に対して
  即座に cancel / release の effect を出す** (fetch の途中でも backoff の待ちの途中でも abort される)。先読み計画が
  まだその resource を望んでいれば、別の新しい (上限付きの) 先読み job を作る。`runJob` は再試行のたびにも
  同じ関数で需要を確かめる (競合の保険)
- 観測: 需要付きの再試行は 1 回ごとに失敗として記録しない。混雑で待った回数と時間を 1 件の telemetry
  (`page_congestion`) として、job の終わりに記録する
- これは RAW に限らず全ページの挙動の変更 (改善) になる。テスト: サムネイルが先に入って前景 3 本が埋めた状態の
  4 本目、前景 4 本の状態の新しい表示ページ、需要が消えたら再試行が止まる

**(7) Remote のサムネイル (`/api/thumb`、利用者決定 14)**

- **half 現像をしない**。対象は 2 経路とも: 通常ファイルの `ThumbnailEngine` (`src/remote_ipc/thumbnail.rs:200-215`)
  と、ZIP entry / コンテナの `ContainerEngine::thumbnail` (`src/remote_ipc/container.rs:3726-3749`, `5808-5827`)。
  フォルダ / ZIP の代表が RAW の場合も、代表を解決した後の RAW に同じ規則を当てる
- 規則: 使えるプレビューがあれば **寸法にかかわらず使う** (PC のサムネイルの「プレビューの長辺が要求寸法以上」
  (`src/thumb_loader.rs:676-684`) は Remote では使わない。Remote 用の判定を分ける)。無ければ catalog に
  PC 側で作ったサムネイルがあればそれ (**cache-only の lookup**: cache miss で元ファイルを現像しない)。それも
  無ければ typed な `ThumbnailErrorCode::NoThumbnail` を返す (`/api/thumb` の応答は `ThumbnailResponse` /
  `ThumbnailErrorCode`、`crates/remote-ipc/src/lib.rs:3041-3055`。`MediaErrorCode` ではない)。コンテナ経路の
  内部 loader が media error を使う箇所では、`NoThumbnail` への明示の変換を置く
- `NoThumbnail` は新しい code (protocol version を上げる)。remote-web は既存のサムネイル失敗と同じ HTTP 応答
  (Web は既存の代替表示) に写し、ログでは区別する。2 経路ともテストする
- これでサムネイル要求は RAW の現像を待たないので、10 秒の IPC 締め切りと executor の待ち行列の問題は生じない
- S2a の「half 現像が要る RAW は Unsupported」を、この規則に置き換える

**(8) Remote AI**

- AI job は自分の thread を持つ long job で、(2) の flight に waiter として参加する (前景扱い、High)。
  取消は AI job の cancel flag。AI runtime の資源を取るのは source の decode の後 (`container.rs:4138-4145`,
  `4190-4193`) なので、待ちのあいだ AI の資源を持たない

**(9) メモリと仕事量の上限**

- Remote 由来の仕事に **固定の上限** を置く: `REMOTE_RAW_FLIGHT_LIMIT = 6`。数えるのは (2) の `outstanding`
  (flight_id 単位。`Cancelling` の置き換えで古い job がまだ動いている分も数える)。**新しい submit はすべて**
  (新しい key、`Cancelling` の置き換え、同じ key の作り直し) この上限の確認を通る
- 上限に達したとき:
  - 前景のページ要求は、core が typed な **`MediaErrorCode::RawCapacity`** を返す。remote-web はこれを
    503 + 専用の error 名 `raw_busy` + `Retry-After` に写す (既存の `MediaErrorCode::Busy` の `miv_media_error`
    とも、admission の `ipc_busy` とも区別する)。Web は (6) の需要付き再試行の対象にする
  - Remote AI は、**owner ごとに 1 つだけの capacity 待ち枠**で待つ。同じ owner の新しい AI job が来たら古い待ちを
    起こして `Cancelled` で返させる (supersede と同じ向き)。待ちは (2) と同じ上限付き wait で、起きるたびと
    capacity を取る直前に cancel flag を確かめる。**capacity を取るまでは ZIP の source bytes を読まない**
    (待機中の AI は大きな source を抱えない)。接続断・drain・service stop は cancel flag で待ちを終わらせる
- S2c の先読みの flight は別枠 (S2c で上限を決め、S2b の上限と合わせた全体の上限を定義する) とし、この上限を消費しない
- cache は S2b では 1 件。flight の owner と cache は、S2c が「優先度付きの submit」と「件数 + 合計サイズの
  capacity policy」を差し込めるよう、submit の優先度と cache の上限を引数 (policy) として持つ
- 同時の peak は「executor の並列数 × 1 回の現像のメモリ (§12 の推定) + 最大 6 件の outstanding (ZIP の source bytes を
  含む) + cache + flight が保持中の結果 + 既存の composite cache」。すべて固定の値で決まる

**(10) テスト**

- (1) 先読み: 要求ページまたは見開きの相方のどちらかが cache に無い RAW なら、何も読まずに Skipped を返す
  (scoped partner を起動しない)。両方 cache にあれば生成する。決定直前に前景へ昇格していれば現像する
- (2) flight: 同じ key の前景・相方・画質違い・AI が 1 回の現像を共有する、最後の waiter の離脱で Cancelling、
  Cancelling 中の新しい join が新しい flight になり古い完了が cache を上書きしない、queued cancel の同期
  callback で deadlock しない、mtime の 1 秒未満の差し替えで別 key になる
- (3)(4) cache の置き換え、明るさ変更で RAW identity・source・composite・自動トリム・AI・AI native の key が変わる、
  1 回の生成の中で明るさを読み直さない
- (5) remote-web: Skipped → 204 + skip ヘッダ、再試行しない。Web UI (node テスト): SKIPPED で同じ先読みを
  出し直さない、表示需要があれば即前景要求、昇格後の Skipped も前景要求、group が失敗にならない、telemetry が skip
- (6) 飽和: 前景 RAW 3 本 + サムネイル多数の最中の新しい表示ページが、表示の需要がある間に最終的に完了する。
  需要が消えたら fetch / backoff の途中でも即座に cancel / release される。ページ移動で古い前景が cancel され permit を返す
- (7) プレビューのある RAW / 無い RAW で catalog あり / なしの各結果、cache-only で現像しないこと。
  通常ファイル・ZIP entry・フォルダ代表の 3 経路。小さいプレビューも Remote では使うこと、`NoThumbnail` の写像
- 追加 (第3案 2 回目): 見開きで要求ページと相方の 2 つの RAW を現像した後に別の完了で cache が置き換わっても、
  その要求は pin で正しく生成する / 取消の起床 (待機中・現像中・接続断・drain) / submit 前の完了 /
  同じ秒・同じ size の差し替えで composite と自動トリムに古い値が当たらない / サムネイルが最後の heavy 枠を使えない
  / 前景 RAW 3 本 + サムネイル多数 + 新しい表示ページ
- 追加 (第3案 4 回目): 同じ key の取消と作り直しを上限の近くで繰り返しても outstanding が上限を超えない /
  `RawCapacity` → `raw_busy` の写像と、それが需要付き再試行になること / 既存の `miv_media_error` の 503 は
  従来どおり / 昇格済みの job が再試行の合間に表示を失ったら cancel + release し、先読み計画があれば先読み job を
  作り直す / 表示の需要がある間は混雑が続いても最終失敗にならず、需要が消えたらすぐ止まる / AI の supersede を
  連続させても capacity 待ちが owner ごとに 1 つ・source bytes を読まない / 接続断・drain・stop で AI の待ちが終わる
- 追加 (第3案 3 回目): flight の状態表の各セル (ticket 到着前の取消、submit 中の join、Done 中の join) /
  `REMOTE_RAW_FLIGHT_LIMIT` 到達時の Busy と AI の待ち / AI の supersede を連続させても flight が上限を超えない /
  AI の高精度 mtime の差し替え / 認証・generation の通らない 204
- 既存の page / admission / coordinator / thumbnail のテストが通ること

#### 10.2.3 表示位置からの先読み現像 (S2c、S2b の直後に実施)

写真のスライドショーのようにゆっくりめくる用途を快適にするため、次を足す (利用者の提案 2026-09-29。なるべく早く実装する方針)。S2b は (2) の flight と (3) の cache をこの拡張で再利用できる形で作る。

- 本体が「Remote で最後に前景で表示された RAW ページ」の表示順の前後 (**先 2・前 1**、PC と同じ) を
  自分で先に現像し、cache に置く (cache の上限は件数と合計サイズの固定値)
- 需要の管理は「最新の前景位置」1 つだけ: 次の前景要求が来たら、窓の外になった現像を取り消す。
  第2案で問題になった要求ごとの需要追跡・再要求は要らない
- 期待できる効果 (S1 の実測からの見込み、未確認): 1 ページを見ている時間が現像時間 (一般的な機種で約 0.5〜2.5 秒、
  X-T4 は約 13 秒) より長ければ、めくった瞬間に表示される。速くめくると追いつかない

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
- `raw_brightness: RawBrightnessSetting` (`MatchPreview` 既定 / `None`)。環境設定の同じページに
  「RAW の明るさ: プレビューに合わせる / 補正なし」として置く (決定 12)。変更時は RAW の現像済み
  Static を keep set ごと失効させて再現像する。サムネイルは設定の影響を受けない: 埋め込みプレビュー由来の
  ものは当然として、プレビューの無い RAW の half 現像サムネイルも **設定にかかわらず自動補正 (thr 0.001)** で
  作る (S2a で判明: ZIP / フォルダの代表サムネイルは代表 entry を決める前に catalog を引く
  (`src/thumb_loader.rs:1261`, `1546`) ので、設定値を catalog のキーに入れると見つけられない)。
  「プレビューに合わせる」ではプレビューの無い RAW のフル現像も同じ自動補正になるので一致し、差が出るのは
  「補正なし」を選んだときのプレビューの無い RAW だけ (サムネイルの方が明るい)。
  S3 で設定 UI と失効経路を実装し、「編集 → OK → 効果」までを通しでテストする
- それ以外の現像パラメータの設定は v1 では作らない

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
  - プレビューの無いページのページ送り (target のフル現像が High で走り、`Developed` の提示で settle する。
    **サムネイルがあってもサムネイル・rendition は提示されない**)
  - カラー化 / LUT が有効な RAW でプレビュー画素が生で提示されないこと (rendition だけが提示される)
  - カラー化 / LUT が有効な RAW へのフォルダ移動で、rendition の提示時に lock が解除されること
    (旧フォルダの holdover が final composite まで残らない)。ページ送りの settle と同じ述語を使うこと
  - `N = 1` で中断できない現像中のページ送り
  - **detached の同期で同じ idx の item key が変わる経路** (`src/app.rs:51660-51673`) で `RawPageStore` も
    破棄され、新しい item の要求が出ること
  - park 中に preview / 現像の結果が届いても捨てられ、activate 後に `PreviewNotRequested` から要求が出直すこと
- サムネイルの half 現像: 2 段通知 (`finalized`) と `gen_done` が既存どおり届くこと、executor の枠が
  縮小・cache 保存の前に返ること、スクロールで keep range 外になった idx の ticket が cancel され
  `canceled` 通知で `requested` が抜けること
- 編集 gate: `RawPreview` で消しゴム・補正レイヤー・隠蔽・注釈・SNS 分割・crop の入口 (キーと左パネル) が
  拒否され、Static 後に通ること (handler-level)
- 表示: §7.5 の置換前後の transform 一致
- 実機確認 (利用者が行う): 大きな RAW フォルダのページ送り、差し替えで表示が飛ばないこと、
  進捗表示、編集が現像後に使えること、Remote、製本。エージェントは製品バイナリを起動しない

## 16. 段階

| 段 | 内容 | 受入条件 |
| --- | --- | --- |
| **S1** | `crates/libraw-sys` (shim + cc ビルド、`USE_ZLIB` / `USE_JPEG`)、`setup-libraw.sh`、`raw_decoder` (info / preview / develop / 中断 / 進捗)、**`RawDevelopExecutor` の本体 (枠・優先度・取消・進捗・`submit` API。App との接続は S2)**、明るさの決定 (§5.3.4)、`bench_raw` (executor の `submit` 経由で現像する)、サンプル manifest | §15 の `raw_decoder` テストが緑。**lossy DNG と deflate DNG のフル現像が通る**。全サンプルの寸法一致・向き・プレビューと現像の縦横比差・所要時間・中断遅延の表を本書へ記録。core が VC runtime DLL を import しないこと (`check-vcrt-pe-dependencies.ps1`)。非 Windows の `cargo check` は合格条件にしない (Windows 専用ソフト。ubuntu の CI job は 2026-09-29 に利用者判断で廃止)。`3fr erf kdc dcr mrw mos mef` の対応とサンプルの有無の報告 |
| **S2** | `raw_format` と WIC 境界の拒否、`RawDevelopExecutor` の App への接続 (設定値・App-global 所有)、入口 D1〜D11・P1〜P4 の振り分け、サムネイル (worker を塞がない half 現像)、ZIP 内 RAW、類似索引、書き出し / コピー / 外部ツール / 製本、Remote (§10.2 の詳細設計を独立レビューしてから) | 入口ごとの回帰テスト、executor テスト。`is_raw_ext` を通らずに RAW を decode する経路が無いことを grep 手順と test で示す |
| **S2c** | Remote の表示位置からの先読み現像 (§10.2.3、利用者の要望で S2b の直後に実施)。S2b の RAW flight と cache を再利用し、cache を「窓の件数 + 固定の合計サイズ」へ広げる | 別途、S2b 完了後に §10.2.3 を詳細化して設計レビュー |
| **S3** | `RawPageStore`、`FsCacheEntry::RawPreview`、読み込み状態、現像窓、差し替えの layout (§7.5 の全経路)、色の gate、ページ送り / フォルダ移動、編集 gate、進捗表示と先読み行、設定 UI、§7.7 の consumer 点検 | `RawPageStore` の状態遷移テスト (§15 の全項目)、context 分離テスト、UI スナップショット (進捗表示と設定)。`build-dev.ps1` で利用者の実機確認 |
| **S4** | ライセンス文書・バージョン情報・対応ソース・マニュアル・製品ページ・spec・readme・リリースチェックリスト・bootstrap | 文書差分のレビュー。`build-dist.ps1 -NoSign` 相当で同梱物に `LIBRAW-LICENSE.txt` が入ること |

各段は委任前にコミットし、この worktree で codex を同時に 2 本走らせない。master は各段の区切りで
このブランチへ取り込む (app.rs は競合しやすい)。統合は S4 完了後に一度。
S1 の実測 (寸法・向き・中断・codec・形式) は S2 / S3 へ進む前の gate とする。

## 17. 未決事項

### 17.1 利用者の判断が要るもの

1. ~~追加拡張子~~ → 決定 8 (6 種を追加、`mef` は除外)
3. ~~Remote 用の cache~~ → 決定 13 (案B、最後の 1 枚だけ)
2. ~~明るさのパラメータ~~ → 決定 12 (プレビューに合わせる + 補正なしへの切替)

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

## 20. 独立レビュー 2 回目 (同セッション、2026-09-27) の対応

第2版に対する判定: 前回の P1-1 / P2-6 / P2-9 / P2-10 / P3-11 は解決、P1-2 / P1-3 / P2-4 / P2-5 / P2-7 / P2-8 は
一部解決。新しい指摘 7 件をすべて採用した (コードで確認)。

| 指摘 | 判断 | 反映先 |
| --- | --- | --- |
| P1 プレビューの無い RAW で half 現像のサムネイルが fullscreen に出る (決定 11 違反) | 採用 | §6.3 (出してよい画素の列)、§7.3、§7.6 (settle と admission)、§7.8、§15 |
| P1 フォルダ移動の lock が final composite まで待ち、決定 10 の rendition を旧フォルダの holdover が隠す (`src/ui_fullscreen.rs:11795-11806`) | 採用 | §7.6 (ページ送りと同じ述語で解除)、§15 |
| P1 generation を変えずに同じ idx の `fs_cache` を消す経路が `RawPageStore` を素通りする (`src/app.rs:51660-51673`) | 採用 (コードで確認。`fs_cache` の直接操作は 31 箇所) | §6.4 (`discard_fs_page(idx)` に集約)、§15 |
| P2 park 時の preview 要求の再開が未定義、`waiting_for_display` と要求要否の混同 | 採用 | §6.3 (`PreviewNotRequested` と `needs_load_request`)、§6.4 |
| P2 サムネイルの executor 経路が既存の 2 段通知と idx ごとの取消を欠く | 採用 (コードで確認) | §8 (現像だけを枠で行い後続要求で queue へ戻す、idx ごとの ticket) |
| P2 S1 の bench が executor 経由の現像規則と矛盾 | 採用 | §5.3.1、§16 (executor 本体を S1 へ) |
| P3 `N = 1` の待ち時間上限の記述、決定 9 と keep set の関係 | 採用 | §5.4.1 (順序の保証だけ)、§2 決定 9、§7.9 |

### 20.1 Remote の詳細設計レビュー (2026-09-27、第1案)

第1案 (2 段 job で heavy worker を手放し、continuation で再投入、remote-web は変えない) は
「S2b に着手できない」と判定された (P1×6 / P2×2)。主な指摘: 再投入と completion guard の所有遷移が原子的でない、
executor の取消・停止・再投入拒否で reply が返らない経路がある、先読み + サムネイルで前景の permit が枯れる、
サムネイルの wire に優先度も長い締め切りも無い、明るさが既存の source / composite / AI の key に無い、
LRU が決定 9 と衝突する。第2案 (§10.2.2) は「本体で待たず、現像中を即返して再要求させる」形に組み直し、
reply と permit の問題を構造的に除いた。LRU は必須になるので利用者の決定を仰ぐ。

### 20.2 Remote の詳細設計レビュー (2026-09-27、第2案)

第2案 (現像中を即返して再要求、要求から切り離した owner と需要追跡、2 件 LRU) も「着手できない」と判定された
(P1×6 / P2×2)。主な指摘: 要求をまたぐ job の持ち主が無い、ブラウザが job を捨てても release が届かない経路がある、
古い完了が新しい現像を上書きし得る、再要求前に cache から追い出され得る、見開きの相方が RAW だと待ちが生じる、
明るさが live 設定と AI native key に無い、失敗記録と待機の上限が無い。利用者は案B (決定 13) を選び、
第3案 (§10.2.2) は Remote の RAW 先読み現像をやめて同期の既存形に戻した。

### 20.3 Remote の詳細設計レビュー (2026-09-29、第3案の初回)

P1×5 / P2×3。すべて採用し §10.2.2 を具体化した: 先読みの判定は見開きの相方を含む RAW の依存を先に列挙して
から行う ((1))、Skipped の Web 側の状態遷移と昇格の競合 ((5))、前景の admission ((6))、Remote サムネイルの
half 現像をやめて締め切りの問題を消す (利用者決定 14、(7))、flight の状態遷移を lock の中で決め cancel を lock の外で
呼び古い完了を捨てる ((2))、明るさを自動トリムと AI にも含め 1 回の処理で読み直さない ((4))、mtime を高精度にする、
Remote 由来の待ち行列の上限 ((9))、テスト ((10))。ブラウザ側 cache の無効化は、明るさが本体側の設定で
セッション取得を必ず挟むため既存契約で足りると判断した。

### 20.4 Remote の詳細設計レビュー (2026-09-29、第3案の 2 回目)

前回 8 件のうち 2 件解決 (Remote サムネイルの締め切り、明るさとブラウザ cache)。残りと新規 (P1×2 / P2×5 / P3×1) を
すべて採用: 要求内の pin と per-flight の結果保持 ((1)(2))、取消の起床を上限付き `wait_timeout` にする ((2)、
既存 source single-flight と同じ形)、submit と完了の順序 ((2))、204 の fetch 経路 ((5))、サムネイルも heavy の
最後の枠を使えない ((6))、高精度 mtime を下流の key にも ((4))、待ち行列の上限の根拠 ((9))、サムネイルの 2 経路と
`NoThumbnail` ((7))、優先度 accessor ((1))。

### 20.5 Remote の詳細設計レビュー (2026-09-29、第3案の 3 回目)

前回 8 件のうち 3 件解決 (pin と結果保持、取消の起床、優先度 accessor)。残りと新規 (P1×2 / P2×3 / P3×1) を採用:
flight の全状態の join / leave 表 ((2))、前景の保証を「表示に必要なページを混雑で最終失敗にしない」に改め Web の
前景再試行を需要がある間は上限なしにする ((6))、サムネイルの専用 admission class ((6))、`ThumbnailErrorCode::NoThumbnail`
((7))、AI の高精度 mtime と完了時の再確認 ((4))、固定の flight 上限 `REMOTE_RAW_FLIGHT_LIMIT` ((9))、204 の検証順
((5))、S2c のための priority と capacity policy ((9))。

### 20.6 Remote の詳細設計レビュー (2026-09-29、第3案の 4 回目)

前回の 11 項目のうち 8 件解決。残り (P1×3 / P2×1 / P3×1) を採用: flight_id 単位の仕事の会計とすべての submit への
上限適用 ((2)(9))、表示需要の判定を coordinator の関数で行い需要が消えたら前景 job を cancel して先読みを作り直す
((6))、上限到達を `RawCapacity` → `raw_busy` の専用 code にする ((6)(9))、AI の capacity 待ちを owner ごとに 1 つにし
source bytes を読む前に capacity を取る ((9))、テストと telemetry の言い直し ((6)(10))。

### 20.7 Remote の詳細設計レビュー (2026-09-29、第3案の 5 回目)

残り 5 件のうち 3 件解決。P1 (既存の core `Busy` の上限付き再試行を残す)、P2×2 (表示需要が消えたら coordinator が
即座に cancel / release、participant lease で `Done` の pin を必ず外す)、P3 (古いテストの削除) を採用した。

### 20.8 S2b 実装の独立レビュー (2026-10-01、3 回目)

JPEG だけの入れ子書庫の先読みと AI の待ちは解決。代表を選ぶ処理が「読めるか」の確認で RAW の payload を
丸ごと読んでいた (skip / capacity の前) 点を P2 として採用し、§10.2.2 (1b) を定めた。

### 20.9 S2b 実装の独立レビュー (2026-10-01、4 回目)

payload を読む前の判定は解決。新しい P2×3 を採用: 同名候補の再読込ループ → 安定した cursor、書庫 root の代表順序を
既存 loader と同じ中央ディレクトリ順に戻す (第1版 (1b) の「名前順」は設計の誤り)、AI はページの有効性を source 準備より前に確かめる。

### 20.10 S2b 実装の独立レビュー (2026-10-01、5〜6 回目)

候補の順序・cursor・AI の事前検証、読めない literal entry からの入れ子へのフォールバックを解決し、受け入れ判定。
非 RAW の全アドレス種別で pre-S2b loader との差異なし、RAW 理由の先読み skip / capacity 待ちが非 RAW に及ばないことを確認。
