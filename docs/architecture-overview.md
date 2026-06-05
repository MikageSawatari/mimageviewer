# アーキテクチャ概観

mimageviewer 全体の構造を俯瞰するための入口ドキュメント。**修正作業の前に必ず目を通すこと**。
個別の詳細は下の「関連ドキュメント」にある専用ページに任せる。

---

## 1. レイヤー構造

```
┌──────────────────────────────────────────────────────────────┐
│  UI 層 (eframe + egui, wgpu バックエンド)                     │
│   - メインビューポート: グリッド (ui_main.rs)                 │
│   - フルスクリーンビューポート (ui_fullscreen.rs)              │
│   - オーバーレイ: 補正 / 分析 / 消しゴム / 隠蔽 / メタデータパネル │
│   - ダイアログ群 (ui_dialogs/、Ctrl+E は export_dialog.rs)      │
└───────────────┬──────────────────────────────────────────────┘
                │ App の public メソッド経由で状態を更新
┌───────────────▼──────────────────────────────────────────────┐
│  アプリ状態層 (src/app.rs の App 構造体)                       │
│   - items / thumbnails / fullscreen_idx …                     │
│   - 各種キュー (reload_queue, heavy_io_queue)                  │
│   - 各種キャッシュ (fs_cache, edit_result_cache, final_composite_…) │
│   - 通信チャネル (tx/rx, cancel_token, scroll_hint)            │
└───────────────┬──────────────────────────────────────────────┘
                │ LoadRequest を push / ワーカースレッド spawn
┌───────────────▼──────────────────────────────────────────────┐
│  非同期ワーカー層                                              │
│   - サムネイルワーカー (通常/重 I/O の 2 系統)                 │
│   - フルスクリーンロードスレッド (1 画像ごとに spawn)          │
│   - PDF ワーカープロセス (--pdf-worker サブプロセス × 5)       │
│   - Susie 32bit ワーカープロセス (mimageviewer-susie32.exe × N)│
│   - TRT エンジンビルダー (--tensorrt-build サブプロセス、初回1回)│
│   - TRT 推論ワーカー (--tensorrt-infer-worker サブプロセス × 1)│
│   - AI 推論スレッド (ort + DirectML、TRT は別プロセス経由)     │
│   - VST3 host bridge プロセス (mimageviewer-vst3-host.exe × 1)│
│     [docs/vst3-integration.md](vst3-integration.md)            │
│   - 動画サムネイルスレッド, フォルダナビゲーション, etc.       │
└───────────────┬──────────────────────────────────────────────┘
                │ デコード結果を mpsc で返す
┌───────────────▼──────────────────────────────────────────────┐
│  データソース                                                  │
│   - ファイルシステム (画像 / ZIP / PDF)                        │
│   - image crate + turbojpeg (JPEG) + WIC (HEIC/AVIF/JXL/RAW)   │
│   - PDFium (pdfium-render、別プロセス)                         │
│   - SQLite DB 群 (catalog / rotation / adjustment / mask /     │
│     conceal / spread / pdf_passwords)                          │
│   - ONNX モデル (upscale / denoise / inpaint / classify)       │
└──────────────────────────────────────────────────────────────┘
```

**鉄則**: 上位レイヤーから下位レイヤーへの呼び出しは OK。逆方向（ワーカーが UI を直接触るなど）は禁止。
ワーカーから UI への通知は必ず mpsc チャネルで行う。

---

## 2. モジュールマップ

### コア

| モジュール | 役割 |
| --- | --- |
| `main.rs` | エントリポイント。フォント設定、logger 初期化、eframe 起動、`--pdf-worker` サブコマンド分岐 |
| `lib.rs` | モジュール宣言のみ (ベンチマーク・テスト用に公開) |
| `app.rs` | `App` 構造体と `eframe::App` 実装。状態遷移の中心 |
| `app/native_video.rs` | Windows native video presenter から戻る overlay event / key / mouse / marker / VST3 操作の App 側処理 |
| `app/tests.rs` | `App` 周辺の unit test / App-level 状態機械テスト |
| `settings.rs` | 設定の永続化 API (`Settings::load` / `save`)。Phase 3 で SQLite 経路に切替 (= `settings_db::boot_settings_db` / `with_db_result` 経由)。旧 JSON 経路 (`try_load_with_recovery` / `rotate_backups` / `write_atomic` 等) は `#[allow(dead_code)]` で残置 (将来削除予定) |
| `settings_db.rs` | 設定永続化 SQLite バックエンド (`%APPDATA%/mimageviewer/settings.db`)。spec §5 の起動決定木 (`boot_settings_db`)、世代バックアップ (`SettingsDb::rotate_backups` で `bak1..bak10`)、JSON migration (`migrate_from_settings_json`)、quarantine (`quarantine_db_files`)、save 抑止フラグ (`save_suppressed`) を提供。詳細は [docs/settings-sqlite-migration.md](settings-sqlite-migration.md) |
| `auto_aspect_cache.rs` | サムネイル比率 Auto モードのフォルダ別前回確定値キャッシュ (`%APPDATA%/mimageviewer/auto_aspect_cache.db`)。再訪時に `auto_aspect.current` の初期値として使い、1:1 → 統計結果への切替ちらつきを減らす。キャッシュ管理ダイアログからフォルダ単位 / 古い行 / 全件をリセットできる |
| `data_dir.rs` | `%APPDATA%/mimageviewer/` のパス解決 |
| `logger.rs` | ファイルロガー (`mimageviewer.log`)。常時記録 + 16 MiB ローテーション |
| `diagnostics.rs` | 診断 zip 書き出し (`export_diagnostics_zip`)。logs ディレクトリのログ群 + システム情報をまとめてデスクトップに保存。環境設定「開発者」タブから呼ばれる |
| `stats.rs` | 読み込み統計の集計 |

### グリッド / サムネイル / フルスクリーン

| モジュール | 役割 |
| --- | --- |
| `ui_main.rs` | メイン画面のグリッド描画とクリック/ドラッグ処理 |
| `ui_fullscreen.rs` | フルスクリーンビューポート (`show_viewport_immediate`)。描画テクスチャの優先順位はここで決定 |
| `ui_fullscreen/draw_icons.rs` | フルスクリーン上部バー / 動画 HUD のボタン・アイコン描画 helper、ファイル情報文字列 builder |
| `export_dialog.rs` | Ctrl+E エクスポートのダイアログ状態・worker・ファイル名衝突回避。UI は base pixels / mask / preset を snapshot し、隠蔽合成・画像エンコード・メタデータ転記は worker が担当 |
| `ui_helpers.rs` | メニューバー、ツールバー、アドレスバー等の共通 UI |
| `grid_item.rs` | `GridItem` 列挙型と `ThumbnailState` (Pending/Loaded/Failed/Evicted) |
| `thumb_loader.rs` | サムネイル並列ロード (WebP キャッシュ生成含む) |
| `catalog.rs` | SQLite サムネイルキャッシュ (`%APPDATA%/mimageviewer/catalog.db`) |
| `folder_thumb_pins.rs` | 親コンテナ (Folder/ZipFile/PdfFile) の代表サムネ手動ピン DB (`%APPDATA%/mimageviewer/folder_thumb_pins.db`)。`apply_folder_thumb_pin` が cache key に `#pin:{source_id}` suffix を載せて pin の identity を表現、Video ピンは `seed_folder_video_pin_thumbs` で `video_pins` から WebP を catalog に seed する |

### 仮想フォルダ (ZIP/PDF) / フォーマット

| モジュール | 役割 |
| --- | --- |
| `zip_loader.rs` | ZIP 内の画像列挙、エントリバイト取得、先頭画像抽出。ネスト ZIP (ZIP in ZIP) はフラット展開し、内側 ZIP バイト列は 256MB LRU キャッシュに保持。画像判定は `folder_tree::is_recognized_image_ext` に委譲 (ネイティブ + WIC + Susie) |
| `pdf_loader.rs` | PDFium ワーカープロセスプール。ページ列挙・レンダリング |
| `pdf_passwords.rs` | PDF パスワードの DPAPI 暗号化永続化 |
| `wic_decoder.rs` | HEIC/AVIF/JXL/TIFF/RAW のデコード (Windows Imaging Component) |
| `save_with_metadata.rs` | JPEG/PNG/WebP のエンコードと EXIF/XMP/PNG text/WebP metadata の転記。Ctrl+E エクスポートから呼ばれ、出力は `create_new` で上書きしない |
| `susie_loader.rs` | Susie 画像プラグイン (`.spi`) のワーカープロセスプール。PI/MAG/Q0/PIC/MAKI 等レトロ画像のデコードをルーティング。32bit ワーカー exe は本体に `include_bytes!` で埋め込み、初回起動時に `%APPDATA%\mimageviewer\mimageviewer-susie32.exe` へ自動展開 |
| `archive_converter.rs` | 7z / LZH → 無圧縮 ZIP 変換 (sevenz-rust2 / delharc)。画像判定は `is_recognized_image_ext` 経由 (Susie 対応拡張子も含む) |
| `archive_cache.rs` | 変換済み ZIP のマッピング DB (`%APPDATA%/mimageviewer/archive_cache.db`)。元ファイルパス + mtime + size で lookup、変換後 ZIP は `archive_cache/<hash[..2]>/<hash>/*.zip` |
| `fs_animation.rs` | GIF / APNG アニメーションのフレーム展開 |
| `video_thumb.rs` | 動画サムネイル取得 (Windows Shell API) |
| `video/` | 動画インライン再生。`mod.rs` (VideoPlayer 公開 API) / `ffmpeg_loader.rs` (FFmpeg LGPL DLL が exe 同居しているか検証 — 展開は launcher が起動時に行い、ロードは Windows ローダが行う) / `decoder.rs` (avformat/avcodec/swscale デコード worker、`VideoDynamicState` で per-frame 状態を atomic 共有) / `audio.rs` (cpal/WASAPI 出力 + ring buffer + VST3 前段の time-stretch) / `audio_stretch.rs` (Signalsmith Stretch による pitch 維持の倍速音声処理) / `clock.rs` (AV マスタークロック)。FsCacheEntry::Video が VideoPlayer を所有。`VideoInfo.dynamic` は decoder thread / native presenter thread / UI で共有し、右パネルの「フレーム表示」「デインターレース」を動的更新する |
| `folder_tree.rs` | 深さ優先前順トラバーサル (Ctrl+↑↓ 用) |
| `panorama.rs` / `panorama_wgpu.rs` | 360 度パノラマビュー (Phase 1 + 1.5 + 2a)。`panorama.rs` は state / GPano XMP 検出 / 解像度ゲート / settle policy / CPU bilinear sampler / `render_settle_overlay`、`panorama_wgpu.rs` は equirect WGSL シェーダ + 8K base アップロード + settle overlay の alpha blend pipeline。詳細は [`docs/panorama-360-view-plan.md`](panorama-360-view-plan.md) |

### 補正 / 編集 / AI

| モジュール | 役割 |
| --- | --- |
| `adjustment.rs` | `AdjustParams` (輝度/コントラスト/γ/彩度/色温度…)、LUT 適用、オート補正 |
| `adjustment_db.rs` | フォルダ別プリセット・ページ別プリセットの SQLite 永続化 |
| `rotation_db.rs` | 非破壊回転の SQLite 永続化 |
| `audio_normalize_db.rs` | 動画音量ノーマライズの per-file 測定値 (integrated LUFS / true peak / 算出ゲイン) の SQLite 永続化 |
| `rating_db.rs` | レーティング (★1〜5) の SQLite 永続化 |
| `mask_db.rs` | 消しゴムマスクの SQLite 永続化 (1bit/pixel deflate 圧縮 + ベクタオブジェクト JSON) |
| `conceal_db.rs` | 隠蔽加工マスクの SQLite 永続化。`mask_db.rs` と同じビットマップ + ベクタ構造を使い、マスクスロットも管理 |
| `local-adjust-core` | 補正レイヤー合成 core。mIV 本体では消しゴム後・隠蔽加工前の非同期レイヤーとして使う |
| `local_adjust_db.rs` | 補正レイヤー配列をページ単位 JSON として `local_adjust.db` へ保存し、`mimageviewer.dat` に復元用バックアップをミラーする |
| `local_adjust_catalog.rs` | 補正レイヤー効果ピッカー用の効果一覧・検索・デフォルト効果生成 |
| `local_adjust_effect_ui.rs` | `local_adjust_lab` から移植した各 `LocalEffect` のパラメータ UI |
| `export_crop.rs` | 切り取り (crop) の矩形・アスペクト・ページ単位の切り出しと `export_crop.db` 永続化。通常表示は crop 外暗転 overlay のみで、実際の切り出しは Ctrl+S コピー / Ctrl+E 書き出しの最終段に行う |
| `spread_db.rs` | フォルダ別の見開きモード永続化 |
| `ai/` | ONNX Runtime (DirectML or TensorRT) によるアップスケール / デノイズ / Inpainting / 画像種別分類 / 補正レイヤー被写体選択。`AiBackend` で multi-EP 対応、TRT 用に `tensorrt_pack` (DLL pack 検出) と `tensorrt_builder` (子プロセスエンジンビルダー) を持つ。TRT 推論はメインから別プロセスへ shm + JSON IPC でルーティング (`trt_worker_pool` / `trt_worker_proto` / `trt_worker_runtime` / `trt_worker_shm`)。TensorRT 設定でも起動時には worker を自動起動せず、AI 処理が実際に必要になった最初のタイミングで遅延起動する。worker 死亡を検知したら自動 detach + DirectML フォールバック + UI バナー通知。U²-Netp 被写体選択は小型モデルとして in-process CPU 強制ロードで使う |
| `png_metadata.rs` | PNG の tEXt/iTXt/zTXt に埋め込まれた AI メタデータ読み取り |
| `exif_reader.rs` | EXIF 読み取り (rexif)。構造タグの抑止と Exif 2.3x 拡張タグの日本語名マップを持つ |
| `xmp_reader.rs` | XMP 読み取り (quick-xml)。`xtw:*` 名前空間のツイート情報を抽出し「XMP ツイート情報」パネルに表示 |

### UI オーバーレイ / ダイアログ

| モジュール | 役割 |
| --- | --- |
| `ui_adjustment_panel.rs` | 画像補正パネル (左端オーバーレイ)。プリセット切替・AI 設定・保存スロット。ヘッダーの 消しゴム / 補正レイヤー / 隠蔽加工 / 切り取り アイコンからそれぞれ `×` 付き独立左パネルを開き、エクスポートアイコンは `Ctrl+E` と同じダイアログへ合流する。補正レイヤーは `local_adjust_lab` と同じ左パネル + 右ツールパネル構成で、効果 UI、マスク編集、U²-Netp 被写体生成、クラシック領域分割生成を UI thread 外の worker 経由で扱う |
| `ui_crop.rs` | 切り取り (crop) モードの独立左パネル。比率選択 / 有効化・解除 / X・Y・W・H 数値入力、crop 外暗転 overlay、ハンドルドラッグ、見開きから Single への pivot を扱う。実際の切り出しは capture / export の最終段 (`export_crop.rs`) |
| `ui_analysis_panel.rs` | 画像分析パネル (右端オーバーレイ)。色情報・ヒストグラム |
| `ui_metadata_panel.rs` | メタデータパネル (AI メタデータ + EXIF + XMP ツイート情報) |
| `ui_erase.rs` | 消しゴムモード (筆 / 囲み / 直線 / 縦線 / 横線 / 矩形 / 楕円 → MI-GAN で inpaint) |
| `ui_conceal.rs` | 隠蔽加工モード (同じマスク編集 UI でモザイク / 塗りつぶし / ぼかしを合成) |
| `ui_dialogs/` | 環境設定・サムネイルキャッシュ管理・変換済みアーカイブキャッシュ管理 (`archive_cache_manager.rs`)・アーカイブ変換ダイアログ (`archive_convert.rs`)・お気に入り編集・スライドショー設定等 |
| `ui_dialogs/preferences.rs` | 環境設定ダイアログの状態、App 連携、ツリー / ページ dispatch |
| `ui_dialogs/preferences/pages.rs` | 環境設定の各 `page_*` 描画関数 |
| `ui_susie_diagnostic.rs` | Susie プラグイン診断パネルの描画。環境設定の「Susie プラグイン」ページから切り出し、`PoolStatus` 各バリアントごとにメッセージ・配色を出し分け。`egui_kittest` のスナップショットテスト対象 |
| `changelog_markdown.rs` | 更新履歴 (GitHub release body) の Markdown サブセット描画。バージョン更新ダイアログ (`ui_dialogs/update_notice.rs`) から呼ばれ、見出し / 箇条書き / `**強調**` / `` `コード` `` / `<kbd>キー</kbd>` を整形。`egui_kittest` のスナップショットテスト対象 |

### 検索 / インデクサ / タグ

**全体像は [search-architecture.md](search-architecture.md) を参照**。

| モジュール | 役割 |
| --- | --- |
| `search_query.rs` | 検索クエリのトークナイザ + マッチャ (AND / OR / NOT / `"..."` フレーズ、`MatchMode`)。3 モード共通 |
| `search_norm.rs` | `normalize_for_match` — ingest / クエリ / post-filter で共有する唯一の正規化関数 (lowercase) |
| `search_index_db.rs` | Ctrl+S 用 `search_index.db` (お気に入り配下のフォルダ / ZIP / PDF / 動画名) |
| `fts_index.rs` | Tantivy 0.26 ラッパ。`IndexDoc` / `Fields` / `QueryFilters` / `build_bigram_and_query` / `search_page`。bigram tokenizer + lower_caser |
| `fts_meta.rs` | `fts_meta.db` (SQLite) ラッパ。ファイル単位の管理メタ (path / mtime / size / status=Ok\|Failed / index_generation)。検索原文は持たない (Tantivy STORED に集約) |
| `ingest_text.rs` | `PerSourceText` (filename / exif / xmp_tweet / png_prompt / pdf_meta / video_meta / tags) のビルダー |
| `ingest_worker.rs` | メタ抽出 + Tantivy buffer + バッチ commit + commit 成功フレームでのみ SQLite を更新 (Tantivy First 書き込み順序) |
| `indexer_manager.rs` | 全お気に入りの `SupervisorHandle` 統括。Ctrl+G ワーカー spawn、App drop 時の停止 |
| `indexer_supervisor.rs` | メタ索引 supervisor (1 お気に入り 1 本)。初期スキャン + FsWatcher + ingest |
| `indexer_progress.rs` | Supervisor → UI への進捗 `ProgressReporter` |
| `search_walker.rs` | 起動時の再帰 walk + 3-way diff (FS と fts_meta.db の突き合わせ) |
| `search_watcher.rs` | notify-rs `ReadDirectoryChangesW` ラッパ + 500ms debounce + rename 正規化 |
| `name_index_supervisor.rs` | Ctrl+S 用 名前索引 supervisor (初期バルク + notify-rs 追従) |
| `name_bulk_indexer.rs` | Ctrl+S 初期バルクスキャンの本体 |
| `global_search.rs` | Ctrl+G streaming クエリワーカー (Searcher snapshot 固定 + ページング post-filter) |
| `global_search_ui.rs` | Ctrl+G 検索バー + drill-down ビュー + Aggregated / DrilledInto 集約 |
| `io_semaphore.rs` | `GlobalIoSemaphore` — UI / PDF / サムネ / インデクサ横断の I/O 同時実行制御 (Low/Normal/High) |
| `tag_ops.rs` | `#タグ` 要素の Bag 操作ヘルパ (add / remove / clear-hash-prefixed) |
| `tag_write_worker.rs` | UI → XMP 書き込み worker。書込み成功後に共有 `IndexWriter` 経由で即時 Tantivy 反映 |
| `xmp_writer.rs` | 既存メタを保持したままの `dc:subject` atomic 書換 (JPEG / PNG / WebP) |

### その他

| モジュール | 役割 |
| --- | --- |
| `gpu_info.rs` | GPU 情報取得 (VRAM サイズ等、キャッシュ容量の自動決定に使用) |
| `monitor.rs` | モニター情報取得 (DPI 等) |
| `open_with.rs` | 外部アプリで開く |
| `file_drag.rs` | グリッドからエクスプローラ等へのファイル D&D 送出 (シェル `IDataObject` + `SHDoDragDrop`)。`docs/file-drag-drop-design.md` |
| `os_theme.rs` | Windows の「アプリ用の色」(レジストリ) を検出し、egui::Visuals へ適用。初回起動時に `Settings::ui_theme` の初期値を決める |

---

## 3. データフロー (俯瞰)

詳細は [display-pipeline.md](display-pipeline.md) を参照。ここでは 1 画面分だけ:

```
ユーザー操作 (キー/マウス)
    │
    ▼
App::update() 内のハンドラ
    │  ├─ load_folder(path)         → フォルダ/ZIP/PDF 切替
    │  ├─ start_fs_load(idx)        → フルスクリーン画像ロード
    │  ├─ apply_rotation(idx)       → 回転の DB 更新
    │  └─ preset 切替 / 補正変更    → final pipeline cache クリア
    │
    ▼
各ワーカーに LoadRequest を投げる / テクスチャキャッシュを無効化
    │
    ▼
次フレームの poll_* / ensure_final_composite_*() 等で結果取り込み
    │
    ▼
ui_fullscreen.rs / ui_main.rs が「表示用テクスチャ」を選んで描画
    (final_composite_cache > edit_result_cache > fs_cache の優先順位)
```

**「どのテクスチャを表示するか」の決定ロジックは `ui_fullscreen.rs` に集中している**。
補正や AI を追加する時は、ここの選択順序を必ず確認すること。

---

## 4. 永続化ストア一覧

すべて `%APPDATA%/mimageviewer/` 配下。バックアップ対象。

| ファイル | 内容 | 書き込むモジュール |
| --- | --- | --- |
| `settings.db` (SQLite, 2026-05 移行) | アプリ全体設定・グローバルプリセット・保存スロット・お気に入り (`FavoriteEntry { id: Uuid, name, path, auto_index_{structure,metadata,thumbs} }`)・タグ定義 (`Vec<TagDef>`)・VST3 chain 設定 (大型 BLOB)。**SQLite トランザクション + `VACUUM INTO` で `settings.db.bak1..bak10` に世代スナップショット**。Corrupted 検出時は `.corrupted-<ts>-<seq>` 3 セット (main + WAL + SHM) で quarantine、bak1→bak10 を新→古で試行し復旧。バージョン跨ぎは初回 load で `settings.db.preupgrade-v<old>` を `VACUUM INTO` でスナップショット。**Transient I/O / 全復旧失敗時は `MAIN_UNREADABLE_THIS_SESSION` + `settings_db::SAVE_SUPPRESSED` で `Settings::save()` 完全 no-op 化** (= 残骸保護)。旧 `settings.json` は初回起動時に migration して `*.migrated-<ts>` にリネーム済み | `settings.rs` + `settings_db.rs` |
| `catalog.db` | サムネイル WebP キャッシュ (BLOB) + メタデータ | `catalog.rs` |
| `auto_aspect_cache.db` | Auto サムネイル比率のフォルダ別前回確定値。フォルダ再訪時はこの値を初期 `auto_aspect.current` にして、後続の実統計で必要なら既存ゲート (streak/cooldown 等) に従って補正する。サムネイルキャッシュ管理の削除操作と連動してリセットされる | `auto_aspect_cache.rs` + `app.rs` |
| `rotation.db` | 非破壊回転角 (0/90/180/270) | `rotation_db.rs` |
| `audio_normalize.db` | 動画ファイル単位のノーマライズ測定値 (integrated LUFS / true peak / 算出ゲイン)。主キー `(path_lower, file_size, mtime_ms, target_lufs_milli)`。環境設定 → 動画から全件クリア可能 | `audio_normalize_db.rs` |
| `rating.db` | レーティング (★1〜5、0 は未登録)。ページ単位 (画像/ZIP 内画像/PDF ページ) とコンテナ (フォルダ/ZIP/PDF 本体) を同一テーブルに格納。キー形式の違い (`::` の有無) で区別 | `rating_db.rs` |
| `search_index.db` | Ctrl+S 用。お気に入り配下のフォルダ/ZIP/PDF/動画名索引 | `search_index_db.rs` |
| `fts_index/` | Ctrl+G 用 Tantivy index (複数 segment + meta.json)。bigram 候補絞り込み | `fts_index.rs` → `ingest_worker.rs` / `tag_write_worker.rs` |
| `fts_meta.db` | ファイル単位の管理メタ (path / mtime / size / status=Ok\|Failed / index_generation)。検索原文は持たず Tantivy STORED に集約 | `fts_meta.rs` |
| `adjustment.db` | ページ個別補正 (`page_params`) とお気に入り標準補正 (`favorite_params`) | `adjustment_db.rs` |
| `mask.db` | 消しゴムマスク (deflate 圧縮 1bit/pixel + ベクタオブジェクト JSON) | `mask_db.rs` |
| `conceal.db` | 隠蔽加工マスク (deflate 圧縮 1bit/pixel + ベクタオブジェクト JSON) とマスクスロット | `conceal_db.rs` |
| `local_adjust.db` | 補正レイヤーのページ単位 JSON。中央 DB が authoritative で、`mimageviewer.dat` の `local_adjust_layers` はフォルダ移動時の復元用バックアップ | `local_adjust_db.rs` + `sidecar.rs` |
| `export_crop.db` | 最後段 crop のページ単位矩形。中央 DB が authoritative で、`mimageviewer.dat` の `export_crop` はフォルダ移動時の復元用バックアップ | `export_crop.rs` + `sidecar.rs` |
| `spread.db` | フォルダ別見開きモード | `spread_db.rs` |
| `folder_thumb_pins.db` | 親コンテナ (Folder/ZipFile/PdfFile) の代表サムネ手動ピン。container_key 主キー (= normalize_keep_drive 済みパス) で 1 行 1 コンテナ、source は kind + container 相対 rel + (zipentry の) entry / (pdfpage の) page。`apply_folder_thumb_pin` が cache key suffix `#pin:{source_id}` で identity を表現 | `folder_thumb_pins.rs` |
| `video_pins.db` | ユーザーがフルスクリーン HUD で指定した動画フレームの抽出 WebP。`(path, pin_pts_secs, thumb_webp, thumb_pts_secs)`。folder thumb pin の source が動画のときは `seed_folder_video_pin_thumbs` が起動時にこの WebP を catalog にミラー seed する。左ジャンプパネルのピン行もこの WebP を再利用する | `video_pins.rs` |
| `video_bookmarks.db` | 動画ブックマーク (pts / title / jump panel 用 WebP)。初回表示時に FFmpeg worker で取得したサムネを `thumb_webp` に保存し、次回以降は DB から復元する | `video_bookmarks.rs` |
| `video_chapter_thumbs.db` | 埋め込みチャプターの jump panel 用 WebP キャッシュ。path + file size + mtime + chapter start をキーにし、動画更新後は古いサムネを参照しない | `video_chapter_thumbs.rs` |
| `pdf_passwords` | PDF パスワード (DPAPI 暗号化) | `pdf_passwords.rs` |
| `pdfium.dll` | 初回起動時に exe から展開 | `main.rs` |
| `models/*.onnx` | 初回起動時に exe から展開 | `ai/model_manager.rs` |
| `mimageviewer.log` | **常時記録** (旧 `--log` ゲートは撤廃、`--log` 引数は no-op)。起動ごとに truncate し、前回分を `mimageviewer.log.prev` に退避。実行中は 16 MiB 超で `mimageviewer.log.bak` にローテーション | `logger.rs` |
| `logs/panic.log` | Rust panic フックが backtrace 付きで append。セッションを跨いで蓄積するため 4 MiB 超で `panic.log.bak` に 1 世代退避 | `main.rs` (`append_panic_log_entry`) |
| `logs/settings.log` | 設定復旧経路の永続診断ログ。**logger の初期化状態に依存しない独立 sink** なので起動ごく初期の復旧フェーズでも残る。SQLite open 時の primary code + extended code、bak 復旧の経路、quarantine、preupgrade snapshot、save 抑止のイベントが残る。再現が難しい設定リセット系報告の事後解析用 | `settings.rs` (`settings_diag_log`) + `settings_db.rs` (`log_diag`) |
| `logs/perf_events.jsonl` | 構造化イベントログ (JSON Lines)。`--perf-log` 引数または環境設定「開発者」タブの「性能ログを記録する」が ON のときだけ生成。起動ごとに rotate (`perf_events.1..4.jsonl`) | `perf.rs` |
| デスクトップ `mImageViewer_diag_<日時>.zip` | 環境設定「開発者」→「ログを zip にする」で生成する診断 zip。logs ディレクトリのログ群 + `system_info.txt` をまとめる | `diagnostics.rs` |

**パスキーの正規化**: Windows は大文字小文字非区別なので、すべての DB は **小文字化 + バックスラッシュ→スラッシュ** に正規化してから格納する。新しい DB を追加するときも同じ規約に従う (`rotation_db.rs` / `adjustment_db.rs` を参照)。

### フォルダ側サイドカー (`mimageviewer.dat`)

`adjustment.db` (ページ個別補正)、`mask.db` (消しゴムマスク)、`conceal.db` (隠蔽加工マスク)、`local_adjust.db` (補正レイヤー)、`export_crop.db` (最後段 crop) のバックアップとして、各ユーザーフォルダの直下に `mimageviewer.dat` を置く (Hidden + System 属性付きの JSON)。中央 DB が authoritative で、フォルダを丸ごと別ドライブへ移動した際に中央のパスキーが無効化されるケースの復旧経路。設定トグル (`sidecar_backup_enabled`、デフォルト ON) で ON/OFF できる。補正レイヤーは各エントリの `local_adjust_layers` 配列、最後段 crop は `export_crop` として保存し、中央 DB に既存エントリがある場合はインポート時に上書きしない。書き込むモジュール: `sidecar.rs`。詳細は [preset-and-adjustment.md](preset-and-adjustment.md) §9 と [virtual-folders.md](virtual-folders.md) §6 を参照。

---

## 5. Phase 区分 (現状)

| Phase | 内容 | 状況 |
| --- | --- | --- |
| 1 | コアビューワー (グリッド・フルスクリーン・設定永続化) | ✅ |
| 1.5 | サムネイルカタログ (SQLite + WebP) | ✅ |
| 2 | AI アップスケール / デノイズ / Inpaint (ONNX + DirectML) | ✅ (Real-ESRGAN/Real-CUGAN/NMKD-Siax, RealPLKSR, MI-GAN は消しゴムから利用) |
| 3 | お気に入り・ツールバー・ZIP・WIC・動画・アニメーション | ✅ |
| 3.5 | 画像補正プリセット (フォルダ別 4 種 + グローバル + 保存スロット 10) | ✅ |
| 3.6 | 消しゴム (Lasso/ブラシ → MI-GAN) | ✅ |

---

## 6. 関連ドキュメント

| ドキュメント | 読むべきタイミング |
| --- | --- |
| [display-pipeline.md](display-pipeline.md) | サムネイル表示やフルスクリーン描画を触るとき。**補正・AI・回転がどこで適用されるかの決定版** |
| [async-architecture.md](async-architecture.md) | 並列処理・キャンセル・キャッシュ競合を触るとき。ワーカー構成の一覧 |
| [virtual-folders.md](virtual-folders.md) | ZIP/PDF 関連を触るとき。**通常画像パスと分岐する箇所のチェックリスト** |
| [preset-and-adjustment.md](preset-and-adjustment.md) | 補正・プリセット・AI キャッシュを触るとき。無効化ルールの早見表 |
| [local-adjustment-layer-v1.1.0-plan.md](local-adjustment-layer-v1.1.0-plan.md) | 補正レイヤー / ローカル調整 / レイヤー合成を触るとき。本体統合タスクリストと cache 方針 |
| [local-adjust-filter-candidates.md](local-adjust-filter-candidates.md) | 補正レイヤー効果候補、効果ピッカー、効果追加方針を触るとき |
| [search-architecture.md](search-architecture.md) | 検索 / インデクサ / タグ関連を触るとき。**Ctrl+S/F/G の経路とインデクサパイプラインの全体像** |
| [spec.md](spec.md) | 機能仕様・設定項目の正式な定義 |
| [catalog-design.md](catalog-design.md) | サムネイルキャッシュ DB の詳細設計 |
| [thumbnail-memory-redesign.md](thumbnail-memory-redesign.md) | サムネイルメモリ管理の背景経緯 |

---

## 7. 修正時のチェックリスト

1. **触る機能の doc を必ず先に読む** (上の表から該当ページを選ぶ)
2. **通常画像 / ZIPImage / PdfPage の 3 分岐を忘れない** — ZIP/PDF 対応漏れは頻出バグ
3. **サムネイル経路とフルスクリーン経路の両方で整合性を保つ** — 片方だけ修正すると表示が食い違う
4. **テクスチャキャッシュの無効化タイミング** — 補正・AI・回転を変更したら正しいキャッシュをクリアしているか確認 (`preset-and-adjustment.md`)
5. **ドキュメント同時更新** — CLAUDE.md の「コード修正時のドキュメント同時更新」セクションに従う
