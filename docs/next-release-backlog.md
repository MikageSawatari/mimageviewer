# 次リリース検討バックログ (post-v1.3.0)

v1.3.0 のリリースレビュー (Claude マルチエージェント + Codex) で「実害が小さい / 防御的 /
要設計判断」と判断して **v1.3.0 では見送った項目** と、しばらく実施していない
**依存ライブラリの更新** を、次リリース以降の検討タスクとしてここで管理する。

運用ルール:

- 対応に着手したら該当行のステータスを `対応中` / `対応済 (vX.Y.Z)` に更新する。
- 完全に不要と判断したら理由を一言添えて削除する。
- 新たにレビューで見送った P3 等が出たらここへ追記する (恒久バックログ)。
- ライブラリ更新は CLAUDE.md「リリース手順チェックリスト Phase 2」とも整合させる。

---

## 1. コードレビュー由来の未対応項目 (P3 / 防御的 / 要判断)

優先度は v1.3.0 レビュー時点の評価。**いずれもリリースブロッカーではない**。

### 1.1 検索 / AI メタデータ

| 項目 | 場所 | 内容 / 対応方針 | 優先 |
| --- | --- | --- | --- |
| バイト列の二重フルパース | `ingest_text.rs` `build_per_source_from_bytes` | `build_searchable_from_bytes_with_origin` と `build_searchable_from_bytes` を別々に呼び、ZIP 内画像のチャンクを2回 inflate + 解析している。`_with_origin` 1回に統合 (`build_per_source_for_file` 同様)。ZIP ingest ホットパスの perf | 対応済 (v1.3.1) |
| zlib 解凍爆弾 | `png_metadata.rs` `decompress_zlib` | `read_to_string` に上限がなく、細工した zTXt/iTXt チャンクが無制限に膨らむ。**v1.2.0 以前からの既存**だが untrusted ファイルを読むので `Read::take(CAP)` で上限化。アーカイブ変換の `copy_capped` (v1.3.0) と同じ方針。**per-chunk 16 MiB + per-file 累積 32 MiB 上限**を実装 (回帰テスト付) | 対応済 (v1.3.1) |
| 既定許可の索引 | `png_metadata.rs` `parse_fooocus_json` / `sui_extra_data` | スキップリストが空のまま全スカラーキーを索引へ入れる default-allow 姿勢。既知のリークはないが、negative を未知キーへ置いた細工 JSON への防御として allow-list 方式が望ましい | 見送り (v1.3.1 判断: 既知リークなし。allow-list 化は正規の未知パラメータを索引から落とす回帰リスク) |
| EXIF UserComment の AI メタ誤分類 | `png_metadata.rs` `parse_user_comment_metadata` → `parse_a1111_as` (~1322) / `ingest_text.rs` (~415) | `parse_a1111_as` が非空テキストなら常に `Some` を返すため、通常の JPEG EXIF `UserComment` (カメラのコメント等) も AI メタとして解釈され、通常の EXIF 検索から抑制される。`Negative prompt:` / `Steps:` 等の A1111 シグネチャを要求してから AI メタ扱いにする。通常コメントの negative test 追加。**v1.3.0 の AI メタ拡充以来の既存挙動で v1.4.0 新規退行ではない** | P2 (v1.4.0 Codex re-review、既存) |

### 1.2 ネスト ZIP ツリー / アーカイブ

| 項目 | 場所 | 内容 / 対応方針 | 優先 |
| --- | --- | --- | --- |
| ソート変更後 cold reopen の代表サムネ stale | `app.rs` 非ピン ZipDir の `zipdir_cache_key` 経路 | 非ピン ZipDir の catalog キーが `zipdir:{dir_prefix}` で代表 entry を含まない。ソート変更で別代表になった2枚が同一 `(mtime, file_size)` のとき cold reopen で旧サムネが出る。キーに代表の discriminator を足す。**見た目のみ・低確率** | 見送り (v1.3.1 判断: 修正は cache key への sort 反映で、`zipdir_cache_key` の本番 7 箇所 (app.rs×6 + zip_tree `all_cache_keys`) を整合スレッディング必須 — 保存キーと `existing_keys` (delete_missing) が不一致だと「保存直後に GC → 毎回再生成」の perf 退行になりうる。既存 cache の一度無効化も伴う。cosmetic・低確率に対し工数/回帰リスク過大。**ユーザー側は P キーのピン留め (`folder_thumb_pins`) で任意の代表に固定できる回避策あり**。やる場合は既存テスト `all_cache_keys_zipdir_format_matches_materialize` を非デフォルト sort でも検証する形に拡張して整合を守る) |
| ZipDir サムネのキュー振り分け | `app.rs` `is_heavy_io` 判定 (≈ thumbnail queue routing) | ZipDir 要求は軽量キューだが worker 側で `ZipDirRepresentative` 解決時に ZIP 列挙 (重 I/O) する。`LoadRequest` の解決戦略でも振り分ける。**perf::event("thumb","zipdir_resolve") 計装を追加**(実測→振り分け再検討用)。振り分け本体は計測値を見て別途 | 計装のみ (v1.3.1) |
| 変換キャッシュ downgrade の UI 不可視 | `archive_cache.rs` `format_from_db` | v1.3.0 で `format="zip"` の行を v1.2.0 にダウングレードすると `list_all` が skip し cache-manager UI に出ない (auto-prune は効くので disk leak はなし)。未リリース機能なので影響軽微。コメント追記 or 未知 format の汎用表示 | P3 (unreleased) |
| `files_done` 非飽和加算 | `archive_converter.rs` `finish_image` | `bytes_written` は saturating だが `files_done: u32` は素の `+= 1`。>42 億エントリで panic/wrap。到達不能だが `saturating_add` で統一 | 対応済 (v1.3.1) |
| ネイティブ ZIP entry_name のサニタイズ | `zip_loader.rs` enumerate / `book_container_key` | ネイティブ (非変換) ZIP の entry に `../`・`.`・ドライブ文字が来ても converter (`normalize_entry_name`) と違い拒否しない。**`normalize_path` は字句的で `..` を解決しないため名前空間脱出・実パス衝突は起きない**ことを確認済み (= 実害なし) だが、converter 側と一貫させる防御的サニタイズを入れる余地あり | 見送り (v1.3.1 判断: 実害なし確認済。サニタイズは正規の特殊名エントリを取りこぼす回帰リスク) (v1.4.0 Codex 再指摘: 合成 rating/spread/pin キーへの影響を懸念。判断は据え置き) |
| 変換アーカイブ cache パス refresh の UI 同期 I/O | `app.rs` `refresh_converted_archive_cache_paths` (`install_new_items` 経由) → `archive_cache.rs` `peek` (SQLite + `zip_path.exists()`) | ナビゲーション経路で `ConvertibleArchive` アイテムごとに DB lookup + `exists()` の同期 FS I/O。変換アーカイブを多数含むフォルダ + 低速/ネットワークドライブで stall の可能性 (docs/ui-responsiveness.md)。scan/サムネ worker 経路へ移すか非同期 batch 化。**既存コードで v1.4.0 で未変更** | P2 (v1.4.0 Codex re-review、既存) |
| ZipDir をレーティングフィルタ下で開くと中身が空 | `ui_main.rs` (~3237) / `app.rs` (~16095) `maybe_suppress_rating_filter_for_opened_container` / `grid_item.rs` `container_path()` が ZipDir で `None` | ★フィルタ中にネスト ZIP の「本」(ZipDir) を開いても抑制経路が呼ばれず、中身が空/未レーティング表示になりうる。`rating_path_key` + 合成 book key を使う ZipDir 対応 suppression helper を `zip_nav_enter` 前に追加。**v1.3.0 のネスト ZIP ツリー以来の既存 UX 欠落で v1.4.0 新規退行ではない** | P2 (v1.4.0 Codex re-review、既存) |
| ZIP/PDF ページの R/L 回転が効かない | `app.rs` `get_rotation` / `apply_rotation`、`rotation_db.rs` | `R`/`L` キー、ホバーバー、右クリックメニューはいずれも `rotate_image_*` に到達するが、回転の取得/保存が `GridItem::Image` / `Video` の実パスだけを対象にしており、`ZipImage` / `PdfPage` は `None` / no-op になる。`display-pipeline.md` / `virtual-folders.md` では ZIP/PDF も回転対象なので実装漏れ。`page_path_key` 相当のページ単位文字列キーへ寄せ、通常画像の既存キー互換を保ちつつ ZIP entry / PDF page を保存できるようにする。`Image` / `ZipImage` / `PdfPage` の回転保存・取得・描画反映の回帰テストを追加。**v0.3 回転機能導入時からの既存不具合で、PDF アップスケール修正の退行ではない (2026-06-13 調査)** | P2 (user-visible virtual folder bug) |

### 1.3 フォルダツリーペイン

| 項目 | 場所 | 内容 / 対応方針 | 優先 |
| --- | --- | --- | --- |
| 毎フレーム `GetDriveTypeW`×最大26 | `folder_pane.rs` `refresh_drives` → `known_folders::available_drives` | ペイン表示中、`sync_to_active` から毎フレーム全ドライブ種別を列挙 + Vec alloc。通常はキャッシュ済みマウント情報で軽いが冗長。`last_drive_refresh` で 1〜2 秒 throttle、または明示トリガ (↻ / combo open) のみに。**`last_drive_refresh` で ~1.5s throttle を実装** | 対応済 (v1.3.1) |
| scan worker の perf 計装 + thread 構成 | `folder_pane.rs` `ensure_scan` / `scan_real_subfolders` | (1) `scan_real_subfolders` に `perf::event` を入れて低速共有でのスキャン悪化を `analyze_perf.py` で検知できるように (docs/ui-responsiveness.md §4)。(2) ノードごとに `thread::spawn` しているのを dispatcher/pool 方式に寄せるか検討 (現状は短命・cancel 付きで thread leak なし) | P3 (style/perf) |
| reparse-point / junction の再帰ガード | `folder_pane.rs` `scan_real_subfolders` / `push_visible_rows` の `seen` | `file_type().is_dir()` が junction も通し、render の `seen` は**正規化パス文字列**なので、異なる字句パスで到達するジャンクションループを手動展開で無限に降りられる (自動爆発はしない)。reparse-point を skip、または canonical / file-id 追跡 + 深さ上限。**`push_visible_rows` に表示深度上限 64 を実装** (上限超の行は描画せず手動展開も止まる。junction は引き続き辿れる) | 対応済 (v1.3.1) |
| フォルダペイン open scan の優先順位整理 | `app.rs` `poll_folder_pane_open` / `start_folder_pane_open` | worker 完了時に nav 優先順位判定の前で即 `load_folder_with_scan` へ流れるため、保留中クリックと同フレームの検索開始 / 別ナビが競合すると古いクリック先が一瞬または副作用込みで適用される余地がある。folder_pane open を通常の nav と同じ優先順位で裁定するか、高優先操作・検索開始時に `cancel_folder_pane_open` する | P3 (UX/priority) |

### 1.4 起動 / 単一インスタンス / Explorer 連携

| 項目 | 場所 | 内容 / 対応方針 | 優先 |
| --- | --- | --- | --- |
| 名前付きパイプにセキュリティ記述子なし | `single_instance.rs` `CreateNamedPipeW` (lpSecurityAttributes=None) | 既定 ACL なので同一マシンのローカルプロセスが「このパスを開け」を送れる (デコードは bounds-checked でメモリ安全)。単一ユーザーのデスクトップビューアでは低影響だが、ユーザー SID 限定の DACL + `PIPE_REJECT_REMOTE_CLIENTS` で攻撃面を縮小。受信パスの妥当性検証も検討。**`PIPE_REJECT_REMOTE_CLIENTS` を実装** (SMB 経由のリモート拒否)。SID 限定 DACL は v1.3.1 判断で見送り (単一ユーザーデスクトップでは過剰・unsafe 工数大) | 一部対応 (v1.3.1) |
| `open_startup_path` の UI スレッド FS stat | `app.rs` `open_startup_path` → `folder_tree::resolve_openable_path` | 転送パス activate 経路 (稼働中 UI) で `is_file`/`is_dir` + 親探索が走る。遅い/切断ネットワークパスで stall。worker 化 or 最低限 perf 計装。**perf::event("startup","open_path_resolve") 計装を追加**。worker 化本体は計測値を見て別途 | 計装のみ (v1.3.1) |
| `--version` / `-V` CLI フラグ未実装 | `main.rs` `main()` 冒頭 (`--pdf-worker` 等の特殊モード判定と同じ場所) | GUI アプリのため未知の引数は無視されて通常起動する。`mimageviewer-core.exe --version` がウィンドウを開いてしまい、CLI からバージョン確認できない (リリース時の版確認は版リソース `(Get-Item).VersionInfo.FileVersion` で代替できるが不便)。`env!("CARGO_PKG_VERSION")` を print して GUI を開かず即 exit する `--version`/`-V` (ついでに `--help`) を追加する。launcher 側にも同フラグを通す。**実装メモ**: GUI exe (`windows_subsystem="windows"`) は親コンソールを持たないため、`AttachConsole(ATTACH_PARENT_PROCESS)` + `WriteFile`(`GetStdHandle(STD_OUTPUT_HANDLE)` へ) で出力する。`WriteConsoleW` (console 専用) ではなく `WriteFile` を使うのは file/pipe リダイレクト (`--version > ver.txt`) でも拾えるため (`Win32_System_Console` feature を core / launcher の両 Cargo.toml に追加)。`--` 以降は位置引数扱いで option 走査を止める。core / launcher の両方で処理 | 対応済 (v1.3.1) |

### 1.5 補正 / AI

| 項目 | 場所 | 内容 / 対応方針 | 優先 |
| --- | --- | --- | --- |
| capture 再補正経路の sharpen | `capture.rs` `run_pixel_job` の re-adjust 分岐 | `effective_smart_sharpen` を経由せず raw `smart_sharpen` を適用。**本番呼出元なし (テスト専用) で latent**。AI アップスケール済みソースに繋ぐなら `output_is_ai_upscaled` を渡すか、テスト専用である旨のコメント | P3 (latent) |
| lazy-load layers の入場時同期 DB 読み | `app.rs` `ensure_local_adjust_layers_loaded` | フルスクリーン入場の初回フレームで `LocalAdjustDb::get_layers` を UI スレッド同期実行 (= 意図的な tradeoff、フォルダ open 一括 ~2.5s を回避)。数十 MB の単一ページで一過性 hitch の可能性。実測で問題が出たら worker 化 (read-only 経路は既に not-loaded を None 返ししている) | P3 (monitor) |
| legacy adjustment_cache の upscaled 誤判定 | `app.rs` (~26545) legacy `adjustment_cache` / `effective_smart_sharpen` | `ai_upscale_enabled` が true なら cache 済み AI 結果を一律「upscaled」扱いし、upscale が skip/失敗で denoise だけが cache を生んだ場合に smart sharpen が誤って skip される。AI cache entry に `used_upscale` を保存するか、最終 composite と同様 cache 出力寸法を source と比較する。**既存 latent** (1.7 で削除した `maybe_apply_adjustment` とは別 instance) | P3 (v1.4.0 Codex re-review、既存 latent) |

### 1.6 ドキュメント / マニュアル整合

| 項目 | 場所 | 内容 / 対応方針 | 優先 |
| --- | --- | --- | --- |
| ゲームパッド Select の説明ずれ | `docs/keymap-spec.md` / `docs/spec.md` / `htdocs/mimageviewer/manual/gamepad.html` | 実装 (`SpreadMode::next_in_spread_cycle`、単ページ→見開き4種→単ページ巡回) を正とし、3 ドキュメントを見開きモード巡回の説明へ同期。HTML マニュアル末尾の「縦読み」も除去 | 対応済 (v1.3.0) |

### 1.7 デッドコード / クリーンアップ

| 項目 | 場所 | 内容 |
| --- | --- | --- |
| `maybe_apply_adjustment` 削除 | `app.rs` | 呼出元ゼロ (コメント参照のみ)。legacy `adjustment_cache` の「アップスケール近似」(Codex 指摘) もこの dead fn 内。削除する | 対応済 (v1.3.1) |

---

## 2. 依存ライブラリ / SDK の更新

しばらく更新していないので、次リリース前に確認・更新する。手順は CLAUDE.md の各管理節
(「PDFium 管理」「ONNX Runtime 管理」「FFmpeg LGPL DLL 管理」「VST3 host bridge 管理」)
および「リリース手順チェックリスト Phase 2」に従う。

### 2.1 ネイティブ依存 (vendor)

| 対象 | 現行 | 確認 / 更新手順 | 注意点 |
| --- | --- | --- | --- |
| **PDFium** | 150.0.7843.0 | `bash scripts/setup-pdfium.sh check` → 必要なら `setup-pdfium.sh` | 更新後は **PDF 表示の動作確認が必須** (CLAUDE.md「リリース前チェック」)。毎週ビルドなので最新化しやすい |
| **ONNX Runtime DLL** | 1.24.2 (ort 2.0.0-rc.12 対応) | `ort-sys` の `build/download/dist.txt` の `ms@X.Y.Z` を確認 → `setup-ort.sh` の既定 VERSION を揃える → `bash scripts/setup-ort.sh` | ort クレートと DLL の C API バージョン一致が必須。`+crt-static` + `load-dynamic` の組合せは崩さない |
| **FFmpeg LGPL shared** | n7.1.4 (BtbN, 7.1) | `bash scripts/setup-ffmpeg.sh check` | メジャーが上がると DLL 名が変わる (例: avcodec-61→62)。**setup-ffmpeg.sh / `video/ffmpeg_loader.rs` / `build.rs` の3箇所**で DLL 名を揃える。GPL build は使わない。更新時は LGPL 通知 + ソース tarball 配置も更新 (docs/ffmpeg-lgpl-source-distribution.md) |
| **VST3 SDK / bridge** | (cmake ビルド済 exe) | C++ ソース変更がなければ再ビルド不要 | 更新したら商用プラグイン (Pro-Q 4 等) で実機確認。古い worktree の exe 流用は不可 (protocol 不一致でクラッシュ) |

> **v1.3.1 実施状況 (グループ D)**:
> - **PDFium**: 150.0.7843 → **151.0.7881** に更新済 (vendor ローカル)。⚠ **PDF 表示の手動動作確認が未実施** (リリース前必須)。
> - **FFmpeg**: n7.1.4-6 → **n7.1-latest** に更新済 (vendor ローカル、同 7.1 メジャーで DLL 名不変 = build.rs/loader 変更不要)。⚠ **動画再生の手動確認**と **LGPL ソース tarball の mikage.to 配置更新**が未実施 (リリース時)。
> - **ONNX Runtime**: ort-sys が ms@1.24.2 要求のまま = **据え置き** (更新不要)。
> - **VST3**: C++ ソース変更なし = 再ビルド不要。
> - ⚠ **perf smoke (`scripts/perf_smoke.sh`) 未実施** (GUI 手動操作要)。

### 2.2 Rust クレート

- まず `cargo update` で互換範囲内 (semver マイナー/パッチ) を更新し、`cargo test` +
  検索 bench 回帰 (`scripts/check_bench_regression.py`) + perf smoke (`scripts/perf_smoke.sh`) を回す。
  → **v1.3.1 で実施済**: `cargo update` (~108 crate)、全 bin テスト 2415 passed、bench 回帰なし。
    perf smoke は未実施 (上記)。メジャー更新は下表どおり全て **v1.4.0 へ先送り** (rc/メジャーは個別判断)。
- メジャー更新を個別に検討する主要クレート (現行バージョン):

  | クレート | 現行 | メモ |
  | --- | --- | --- |
  | `ort` | 2.0.0-rc.12 | **rc**。安定版が出ていれば追従 (対応 ONNX Runtime DLL バージョンを揃える) |
  | `pdfium-render` | 0.8 | PDFium DLL 側と合わせて確認 |
  | `ffmpeg-the-third` | 3 | FFmpeg DLL メジャーと整合 |
  | `image` | 0.25 | デコーダ周りの挙動差に注意 (サムネ品質テスト) |
  | `zip` | 2 | アーカイブ読み/変換の回帰テスト |
  | `sevenz-rust2` | 0.20 | 7z 変換 |
  | `delharc` | 0.6 | LZH 変換 |
  | `unrar` | 0.5.8 | RAR 変換。`unrar_sys` の更新有無も確認 |
  | `turbojpeg` | 1 | DCT スケールデコードのベンチ確認 |

- 更新後は `dumpbin /dependents` でリリース exe に `VCRUNTIME140.dll` 等が復活していないか確認
  (CLAUDE.md Phase 3 の依存 DLL 回帰チェック)。

---

## 3. 関連ドキュメント

- レビュー対応で実装済みの項目は git 履歴 (v1.3.0 リリースレビュー対応コミット) を参照。
- 機能追加アイデアは [feature-expansion-ideas.md](feature-expansion-ideas.md)、
  VST3 個別 TODO は [vst3-todo.md](vst3-todo.md) を参照。

---

## 4. 5ch feedback backlog after v1.3.0

Source thread: https://egg.5ch.io/test/read.cgi/software/1752914772/

### 4.1 Page number overlay

- Source: 743, 744.
- Request: Always show `(current page) / (total pages)` for comic-reader use.
- Planned response: Add a small bottom-right page number overlay. It should be possible to turn it off if it is distracting.
- Suggested default: ON.
- Display rules:
  - Single page: `12 / 180`.
  - Spread: `12-13 / 180`.
  - Continuous vertical / horizontal reading: show the page nearest the viewport center, e.g. `12 / 180`.
- Related detailed plan: [page-number-overlay-plan.md](page-number-overlay-plan.md).

### 4.2 Seek bar / page position without covering the image

- Source: 745.
- Request: The seek bar and page position currently appear only on mouse hover and cover the image. Add an option to show them constantly without overlaying the image.
- Proposed design:
  - Keep the current hover seek bar behavior as the default.
  - Add a lock/pin button at the edge of the fullscreen seek bar.
  - When locked, reserve a bottom HUD area for the seek bar/page position.
  - Fit the image into the remaining area above the locked seek bar, so the seek bar does not cover the image.
  - When unlocked, return to the current overlay-on-hover behavior.
- Implementation notes:
  - Avoid making the image area smaller unless the user explicitly locks the seek bar.
  - Persist the locked/unlocked state in settings.
  - Coordinate with the page-number overlay so the two do not overlap.
  - Check fullscreen and windowed/in-window modes separately.
- Priority: Medium. This is a larger layout change than the page-number overlay.

### 4.3 画像・動画ビューアの別ウィンドウ化

- 出典: 745.
- 要望:
  - Leeyes / MangaMeeya のように、ファイル選択ウィンドウと画像・動画表示ウィンドウを分離したい。
  - 想定される挙動は「フルスクリーンを一時的に別窓化する」ではなく、1 回開いた画像・動画ビューアウィンドウが常駐し、ESC で一覧へ戻ってもビューアウィンドウ自体は残る形。
  - ビューア側で前後の画像・動画やフォルダへ移動した場合、メインウィンドウ側のサムネイル一覧カーソルも常に同じ項目へ追従する。
  - 見開き表示では、主カーソルに加えて、同時に開かれている相方ページをサブカーソルとして一覧側にも表示できると望ましい。
- 設計方針:
  - 実装計画: [detached-viewer-implementation-plan.md](detached-viewer-implementation-plan.md)。
  - `fullscreen_idx` を「閉じたら終わる一時的なフルスクリーン状態」として拡張し続けるより、常駐する単一の `ViewerSession` を導入する方が自然。
  - `ViewerSession` は少なくとも以下を持つ:
    - `current_idx`: ビューアの現在項目。メイン一覧カーソル同期の正本。
    - `secondary_idx`: 見開き表示の相方ページ。通常は `None`。
    - `presentation`: `MainWindow` / `DetachedWindow` / `Fullscreen` などの表示形態。
    - `visible`: ビューアセッションが開いているかどうか。detached mode 設定とは分けて扱う。
  - 「別ウィンドウモード」は別機能ではなく、同じ `ViewerSession` の `presentation == DetachedWindow` として扱う。
  - フルスクリーン表示、メインウィンドウ内表示、別ウィンドウ表示は、同じビューア状態を異なる場所・サイズで見せるだけにする。
  - ただし初期仕様では、別ウィンドウモードの `DetachedWindow` は常に通常ウィンドウ形式とし、別ウィンドウからのフルスクリーン化は行わない。
  - 別ウィンドウの F11 フルスクリーン化は、複数ディスプレイ・保存位置復元・モニター構成変更・動画 native presenter が絡み、予期しないモニターで全画面化するリスクが高い。必要になった場合は、将来の明示的な「このモニターで全画面表示」機能として別途設計する。
- メインウィンドウ同期:
  - ビューアの `current_idx` が変わるすべての経路で、メイン側の `selected` / `scroll_to_selected` / 直近選択項目の記録を同時に更新する。
  - メインウィンドウが表示されている別ウィンドウモードでは、カーソル移動が即時に見えるよう root viewport の repaint を要求する。
  - フルスクリーンでメインウィンドウが見えない場合も、論理状態としては常に `selected` とスクロール目標を更新する。戻った瞬間に正しい位置を表示できるようにする。
  - ただし、見えないメイン一覧を毎フレーム重く描画する必要はない。同期すべきなのは選択状態・スクロール目標・フォルダ状態であり、不要なサムネイルロードやフォルダスキャンは増やさない。
  - 入力中・ドラッグ中・ダイアログ表示中でも、ビューアからメインへの現在項目同期は止めない。同期を遅延させると、スライドショーや連続再生中に「いまどこを表示しているか」が分かりにくくなるため。
  - 同期時にメインウィンドウを前面化したり、キーボードフォーカスを奪ったりしない。必要なのは状態更新と repaint 要求であり、OS の window activation は行わない。
  - メイン側のダイアログ・ドラッグ・編集操作は、操作開始時の対象 `idx` / path を保持する。途中でビューア側の自動送りにより `selected` が変わっても、進行中の操作対象を差し替えない。
  - もしドラッグ中の自動スクロールが実用上邪魔になる場合でも、止めるのは物理スクロールだけに限定する。`selected` / `secondary_idx` の論理同期は即時に行い、ドラッグ終了時にスクロール位置だけ再同期する。
- ナビゲーションとフォルダ移動:
  - 静止画フルスクリーンの前後移動、ジャンプ、連続読書、Ctrl+↑↓ のフォルダ移動、動画 native presenter 側の前後移動・遅延移動を、共通の「ビューア現在項目を変更してメイン一覧へ同期する」ヘルパーへ寄せる。
  - 現状の静止画フルスクリーン経路は移動時点で `selected` を更新しているが、動画 native presenter 側の直接移動は閉じるタイミングの同期に寄りがちなので、この要件では明示的な即時同期が必要。
  - フォルダ移動では、結果のフォルダロードが完了した時点でメイン一覧も新フォルダへ切り替え、ビューアの現在項目と同じ項目へスクロールする。
- ESC / close の挙動:
  - `DetachedWindow` の `×` / `Esc` / `Enter` / 右クリック / `Alt+F4`: ビューアセッションを終了する。動画の場合は再生停止。別ウィンドウモード設定は維持し、次の open 操作で再表示する。
  - `Fullscreen` は初期仕様では同一ウィンドウ表示側だけの表示形態とする。`Fullscreen` で ESC: `ViewerSession` は維持したまま、`presentation` を `MainWindow` または直前の同一ウィンドウ表示形態へ戻す。
  - `MainWindow` 表示で ESC: 一覧表示へ戻す。ただし、ビューアセッションを残すか非表示にするかは設定・仕様として明確化する。
  - メインウィンドウの最小化とは連動しない。アプリ終了時は別ウィンドウも終了するが、close-to-tray 設定で実際にはアプリ終了しない場合は detached 動画再生を継続する。
- 動画固有の注意点:
  - 静止画は既存の egui viewport / fullscreen 描画を流用しやすい。
  - 動画は native presenter / HWND 系の処理があるため、画像と同じ常駐ビューアウィンドウにどう紐づけるかが最大のリスク。
  - 初期実装では、画像は egui viewport に描画し、動画は native presenter を viewer window の位置・サイズへ追従させる方式が現実的。
  - 長期的には、画像・動画のどちらも同じ `ViewerSession` にぶら下がり、表示形態の切替だけで windowed / fullscreen を行える状態を目指す。
- UI 詳細:
  - 見開き時の `secondary_idx` はサムネイル一覧でサブカーソルとして描画する。主カーソルとは見た目を分け、詳細表示モードでの表現も決める。
  - キーボード操作は新規操作を `KeyAction` / keymap 経由にする。ビューアにフォーカスがある場合とメイン一覧にフォーカスがある場合の入力先を明確にする。
  - F12 を同一ウィンドウ表示 / 別ウィンドウ表示の切替キー候補にする。現行の既定ショートカットでは F12 は使っていないため、実装時は `KeyAction` と keymap へ追加する。
  - F11 は既存どおり同一ウィンドウ表示時の「ウィンドウ内表示 ⇔ 全画面表示」切替に限定する。別ウィンドウモード中の F11 は無効、または軽い通知だけで何もしない。
  - メイン側から別ウィンドウへ伝播するのは、表示対象を切り替える操作だけにする。タグ編集、検索条件変更、ソート、単なるスクロール、設定変更、サムネイルサイズ変更などは別ウィンドウの表示内容へ影響させない。
  - 別ウィンドウ側の前後移動、フォルダ移動、スライドショー、動画連続再生などで `current_idx` が変わった場合は、メイン側で別操作中でも現在項目同期を反映する。ただしフォーカスは奪わず、進行中のメイン側操作対象も変更しない。
  - 別ウィンドウは owner 付き window ではなく独立 top-level window とし、常に最前面にはしない。明示 open / F12 切替など必要なタイミングでのみ前面へ出す。
  - ゲームパッド、マウスホイール、ドラッグ & ドロップ、ウィンドウの最小化/最大化/閉じる挙動は個別に仕様化する。
  - 別ウィンドウの位置・サイズは設定へ保存する候補。復元時は現在のモニター構成に対して画面外へ出ないよう補正する。
- 推奨フェーズ / 工数感:
  - 詳細は [detached-viewer-implementation-plan.md](detached-viewer-implementation-plan.md) の Phase 0〜5 を正とする。
  - ユーザー体験上、動画だけ別挙動にすると分かりにくいため、動画 native presenter の `DetachedWindow` 対応も初回実装範囲に含める。
  - 複数の独立ビューアウィンドウを同時に開く設計は、単一 `fullscreen_idx` 前提を大きく崩すため別課題。必要なら `ViewerSession` を複数持つ設計として 1〜2 か月級の改修を見込む。
- 優先度: Medium / v1.4.0 後の実装候補。画像・動画の同等対応まで含めるため大きめの UI アーキテクチャ変更。

### 4.4 Jump forward/backward by a fixed amount

- Source: 745.
- Request: Add key operations that jump forward/backward by a configured amount, such as fixed 10 pages or 10% of total pages.
- Status: 対応済み (next patch). `Shift+Left/Right` は keymap action として維持しつつ、設定を `fullscreen_jump_mode` + `fullscreen_jump_percent` + 既存 `fullscreen_fixed_jump_count` に拡張。既定は 10%。固定ページ数にも切替可。
- Proposed design:
  - Add commands such as `jump_forward_large` and `jump_backward_large`.
  - Make them available in `keymap.ini`.
  - Add settings for jump mode:
    - Fixed page count.
    - Percentage of total pages.
  - Initial default candidate: 10 pages or 10%.
- Notes:
  - Clamp to first/last page.
  - For spread mode, land on a valid display index while respecting existing spread pairing behavior.
  - For continuous reading, update scroll position to center the target page.
- Priority: Medium. Useful and relatively contained if implemented through the existing navigation command layer.

### 4.5 Filename-oriented selection mode discoverability

- Source: 745, 747.
- Request: A mode to select by filename instead of thumbnails.
- Status: 対応済み扱い (existing detailed view covers the request).
- Current state:
  - Covered by the existing detailed view mode.
  - User was informed that the toolbar column/detail mode and `Alt+-` switch to detailed view.
- Decision:
  - Do not add a separate filename-selection mode for now.
  - Reopen only if later feedback says detailed view is insufficient.
- Priority: 対応済み / no implementation required.

### 4.6 Customize information shown under the selection cursor

- Source: 745.
- Request: Customize the information shown under the file selection cursor. Current full path is redundant when the folder bar is visible; user wants update time and file size as possible fields.
- Proposed design:
  - Add checkboxes for displayed fields:
    - File name.
    - Full path.
    - Resolution.
    - File size.
    - Modified time.
  - Consider presets instead of too many checkboxes if the preferences page becomes crowded.
- Notes:
  - For archive entries, file size and modified time may be missing or archive-entry-specific.
  - Keep formatting compact to avoid noisy grid UI.
- Priority: Medium-low.

### 4.7 Show selection cursor initially inside ZIP/archive views

- Source: 745, 747.
- Request: When opening ZIP, the file selection cursor should be visible initially, matching the folder behavior.
- Current response: Acknowledged in thread; fix promised.
- Priority: High for next patch. Small UX consistency bug.

### 4.8 Microsoft Defender false positive follow-up

- Source: 739, 742.
- Issue: v1.3.0 ZIP package is detected by Microsoft Defender as Trojan, while installer and standalone exe can be downloaded.
- Current response: Reproduced locally and submitted to Microsoft as a false-positive analysis request.
- Follow-up:
  - Track Microsoft analysis result.
  - If Microsoft confirms false positive, mention it in the next release note or thread reply if needed.
  - If detection persists, consider repackaging the ZIP and resubmitting.
  - Code signing may help SmartScreen reputation over time, but Defender malware detections still require false-positive submission.
- Priority: Release hygiene / support.

### 4.9 v1.4.0 large-jump feedback

- Source: 756, 758, 759.
- Current state: v1.4.0 added `Shift+Left/Right` large page jump with a fixed default of 10 pages.
- Status: 対応済み (next patch). 既定を 10% に変更し、右綴じでは通常の左右ページ送りと同じく方向を反転。見開き中は最低 2 ページ進む。
- Next-version fixes promised in thread:
  - Add percentage-based jump amount, e.g. jump by 10% of total pages, because fixed 10 pages is too small for thick books and too large/noisy for thin books.
  - Respect reading direction / binding direction for `Shift+Left/Right`; the current implementation appears reversed for right-bound manga.
- Proposed design:
  - Extend the jump setting from a fixed page count to a mode + value pair:
    - Fixed pages.
    - Percentage of total pages.
  - Clamp the computed jump amount to at least 1 page and within the book bounds.
  - `Shift+Left/Right` should reuse the same logical previous/next direction mapping as normal page navigation, then apply the larger step. Do not hard-code physical left/right directly to index +/-.
- Priority: High for the next patch because both items were acknowledged in thread.

### 4.10 v1.4.0 seek-bar / cursor / spread-label feedback

- Source: 760.
- Requests:
  - When the bottom seek bar is pinned/fixed, the mouse cursor does not seem to auto-hide. Fix this so the cursor still hides after inactivity.
  - Add a setting for mouse cursor auto-hide delay. The current behavior appears to be around 3 seconds; user wants a shorter delay.
  - For spread display in right-bound manga, the bottom-right page number should match the visual page arrangement on screen, e.g. `3,2 / 200` instead of numeric `2-3 / 200`.
  - The page number shown on the seek bar currently shows only one page such as `2/200`, `4/200`, `6/200`; it should use the same spread-aware label as the bottom-right overlay.
- Status: 対応済み (next patch). `fullscreen_cursor_hide_delay_secs` を追加し、既定 1.0 秒 / 設定範囲 0.1〜5.0 秒に変更。固定シークバー表示はカーソル可視維持理由にせず、ページ番号は右下 overlay / 下部 seek bar で同じ見開き対応 formatter を使う。
- Proposed design:
  - Treat mouse movement/interaction, not pinned HUD visibility, as the reason to keep the cursor visible.
  - Persist `fullscreen_cursor_hide_delay_secs`.
  - Default to 1.0 second.
  - Use the shared formatter for both the bottom-right overlay and the seek-bar page label.
- Priority: High for cursor auto-hide bug and spread-label consistency; medium for exposing the cursor-hide delay setting.
