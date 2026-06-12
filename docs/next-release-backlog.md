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

### 1.2 ネスト ZIP ツリー / アーカイブ

| 項目 | 場所 | 内容 / 対応方針 | 優先 |
| --- | --- | --- | --- |
| ソート変更後 cold reopen の代表サムネ stale | `app.rs` 非ピン ZipDir の `zipdir_cache_key` 経路 | 非ピン ZipDir の catalog キーが `zipdir:{dir_prefix}` で代表 entry を含まない。ソート変更で別代表になった2枚が同一 `(mtime, file_size)` のとき cold reopen で旧サムネが出る。キーに代表の discriminator を足す。**見た目のみ・低確率** | 見送り (v1.3.1 判断: 修正は cache key への sort 反映で、`zipdir_cache_key` の本番 7 箇所 (app.rs×6 + zip_tree `all_cache_keys`) を整合スレッディング必須 — 保存キーと `existing_keys` (delete_missing) が不一致だと「保存直後に GC → 毎回再生成」の perf 退行になりうる。既存 cache の一度無効化も伴う。cosmetic・低確率に対し工数/回帰リスク過大。**ユーザー側は P キーのピン留め (`folder_thumb_pins`) で任意の代表に固定できる回避策あり**。やる場合は既存テスト `all_cache_keys_zipdir_format_matches_materialize` を非デフォルト sort でも検証する形に拡張して整合を守る) |
| ZipDir サムネのキュー振り分け | `app.rs` `is_heavy_io` 判定 (≈ thumbnail queue routing) | ZipDir 要求は軽量キューだが worker 側で `ZipDirRepresentative` 解決時に ZIP 列挙 (重 I/O) する。`LoadRequest` の解決戦略でも振り分ける。**perf::event("thumb","zipdir_resolve") 計装を追加**(実測→振り分け再検討用)。振り分け本体は計測値を見て別途 | 計装のみ (v1.3.1) |
| 変換キャッシュ downgrade の UI 不可視 | `archive_cache.rs` `format_from_db` | v1.3.0 で `format="zip"` の行を v1.2.0 にダウングレードすると `list_all` が skip し cache-manager UI に出ない (auto-prune は効くので disk leak はなし)。未リリース機能なので影響軽微。コメント追記 or 未知 format の汎用表示 | P3 (unreleased) |
| `files_done` 非飽和加算 | `archive_converter.rs` `finish_image` | `bytes_written` は saturating だが `files_done: u32` は素の `+= 1`。>42 億エントリで panic/wrap。到達不能だが `saturating_add` で統一 | 対応済 (v1.3.1) |
| ネイティブ ZIP entry_name のサニタイズ | `zip_loader.rs` enumerate / `book_container_key` | ネイティブ (非変換) ZIP の entry に `../`・`.`・ドライブ文字が来ても converter (`normalize_entry_name`) と違い拒否しない。**`normalize_path` は字句的で `..` を解決しないため名前空間脱出・実パス衝突は起きない**ことを確認済み (= 実害なし) だが、converter 側と一貫させる防御的サニタイズを入れる余地あり | 見送り (v1.3.1 判断: 実害なし確認済。サニタイズは正規の特殊名エントリを取りこぼす回帰リスク) |

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

### 2.2 Rust クレート

- まず `cargo update` で互換範囲内 (semver マイナー/パッチ) を更新し、`cargo test` +
  検索 bench 回帰 (`scripts/check_bench_regression.py`) + perf smoke (`scripts/perf_smoke.sh`) を回す。
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
