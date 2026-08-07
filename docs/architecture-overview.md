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
| `main.rs` | `windows_subsystem` 属性と `mimageviewer::run()` 呼び出しだけを持つ薄い実行ファイル入口 |
| `lib.rs` | アプリの単一 crate root。全モジュール宣言、logger / eframe 起動、worker サブコマンド分岐を所有し、unit test・integration test・実行ファイルで同じコンパイル結果を共有する |
| `app.rs` | `App` 構造体と `eframe::App` 実装。状態遷移の中心 |
| `app/vram_accounting.rs` | `App` が所有する全 GPU テクスチャキャッシュを、実寸・mip chain・`TextureId` 重複排除で横断集計する。サブシステム別会計、モード判定、共有予算の参照、1 秒間隔の perf 計装を担当する |
| `app/folder_scan.rs` | 通常実フォルダの列挙と、1 物理フォルダ内に限定した同名メディア / コンテナ正規化の所有者。動画 + sidecar 画像、実フォルダ + ZIP/PDF/対応アーカイブ、ZIP + 変換元アーカイブ、画像拡張子優先度の規則を通常一覧・サブ展開・スマートフォルダで共有する |
| `app/native_video.rs` | Windows native video presenter から戻る overlay event / key / mouse / marker / VST3 操作の App 側処理。native Touch は render overlay 内で完結し、App の legacy mouse 操作へは再注入しない |
| `touch_input.rs` | 静止画 egui viewport と native video presenter が共有する、接点集合・所有・tap zone・pinch/pan/scroll の純粋な認識器 |
| `touch_debug.rs` | `MIV_TOUCH_DEBUG=1` の入力源診断。Win32 pointer/mouse source に加え、native presenter の stream 所有、座標変換、認識コマンド、promoted mouse 破棄を記録する |
| `app/recursive_snapshot_scan.rs` | 複数実フォルダを再帰列挙する snapshot view 共通 walker。cancel、深さ上限、reparse point 回避、重複 root 排除、`GlobalIoSemaphore` / `ActivityGate`、chunk sort をサブ展開とスマートフォルダで共有する |
| `app/subfolder_expansion.rs` | 現在地以下の画像 / 動画、ZIP/PDF 本体、設定上の画像フォルダ本を平坦化する一時 snapshot view。画像フォルダ本は通常一覧と判定述語を共有し、設定 OFF では画像を個別項目に保つ。共通 walker で走査し、prepare worker で metadata・コンテナピン・表示順を構築する |
| `app/top_level_grid_view.rs` | 通常一覧、検索、★固定、サブ展開、スマートフォルダ、閲覧履歴、レーティング一覧が共有する最上位 grid surface の単一 ownership と完全な復元 snapshot。スマートフォルダでは root の表示順、現在 entry、entry 内 current / Backspace stack を所有する。既存の個別 active flag は描画互換用に残す |
| `app/smart_folder.rs` | 保存済みスマートフォルダの複数ルール / root 走査、フォルダ / 画像 / 動画 / 音声 / ZIP / PDF / 対応アーカイブ収集、物理フォルダ単位の同名正規化、ルール OR 条件適用、現在の全体ソート順と保存済みグループ化単位による prepare、snapshot 履歴復元、進捗 / cancel / stale generation 破棄を担当する。同名動画 sidecar は full-path key の snapshot として表示へ渡す。prepare worker は exact key の編集状態、変換アーカイブ対応、catalog、固定代表を一括準備し、UI install に同期 DB I/O を残さない。ソートだけの再構築は同一 snapshot generation の正規化 key 配列・採用 entry index・sparse metadata と共有 catalog cache を再利用し、DB を再照会しない。metadata DB の書込完了境界で revision を進め、古い cache / 完了結果は論理的に失効させる。巨大な旧 cache、revision / tombstone / generation / 表示定義の不一致で採用しない完成済みprepare結果、cancel時のpending receiver内に到着済み結果と大件数確認待ちsnapshotは、type-erased payloadとして専用 workerへ渡しUIスレッド外で破棄する。検索 / ★固定との相互排他は入口と worker 完了時の両方で検証し、検索結果・検索由来 Snapshot・進行中 smart から別の最上位ビューへ直行するときは復元なしで元 `TopLevelGridRestore` を原子的に移譲する。実フォルダ entry を開いた後の scope は `TopLevelGridView::SmartFolder` に残し、親移動と Ctrl+↑/↓ を entry root 内へ制限する。削除 tombstone は scan/prepare 世代と照合し、成功 snapshot 採用まで破棄しない。通常一覧のソート順・サムネイル / 詳細表示は定義へ保存も上書きもしない |
| `app/tests.rs` | `App` 周辺の unit test / App-level 状態機械テスト |
| `settings.rs` | 設定の永続化 API (`Settings::load` / `save`)。Phase 3 で SQLite 経路に切替 (= `settings_db::boot_settings_db` / `with_db_result` 経由)。旧 JSON 経路 (`try_load_with_recovery` / `rotate_backups` / `write_atomic` 等) は `#[allow(dead_code)]` で残置 (将来削除予定) |
| `ui_fonts.rs` / `ui_font_catalog.rs` | v2.7.0 UI フォント基盤。前者は日本語の正立した選択 face + 記号 / 絵文字 / CJK fallback、動画・音声の固定サイズ HUD label 用の既定 family を含む `FontDefinitions`、実メトリクス由来の縦位置補正、atlas resync 用 cache を所有する。後者は worker からシステム font / TTC face を列挙し、日本語 coverage と upright style の判定、ユーザー font 取り込み、プレビュー raster を行う |
| `settings_db.rs` | 設定永続化 SQLite バックエンド (`%APPDATA%/mimageviewer/settings.db`)。spec §5 の起動決定木 (`boot_settings_db`)、世代バックアップ (`SettingsDb::rotate_backups` で `bak1..bak10`)、JSON migration (`migrate_from_settings_json`)、quarantine (`quarantine_db_files`)、save 抑止フラグ (`save_suppressed`) を提供。詳細は [docs/settings-sqlite-migration.md](settings-sqlite-migration.md) |
| `auto_aspect_cache.rs` | サムネイル比率 Auto モードのフォルダ別前回確定値キャッシュ (`%APPDATA%/mimageviewer/auto_aspect_cache.db`)。再訪時に `auto_aspect.current` の初期値として使い、1:1 → 統計結果への切替ちらつきを減らす。キャッシュ管理ダイアログからフォルダ単位 / 古い行 / 全件をリセットできる |
| `data_dir.rs` | `%APPDATA%/mimageviewer/` のパス解決 |
| `explorer_integration.rs` | Windows Explorer 連携。SendTo Known Folder (`FOLDERID_SendTo`) を解決し、ShellLink COM (`IShellLinkW` / `IPersistFile`) で per-user の `mImageViewer.lnk` を作成・削除・状態確認する。launcher から起動された core では `MIV_LAUNCHER_EXE_PATH` を優先して配布用 `mimageviewer.exe` を登録先にする |
| `keymap.rs` | キーボード割り当ての `KeyAction` / `Chord` / parser / Action 定義 / egui exact-match / native VK 判定 helper。現在の正本は `Settings.keymap` (`settings.db`) で、旧 `%APPDATA%/mimageviewer/keymap.ini` は初回起動時に 1 回だけ取り込んで `keymap.ini.imported*.bak` へ退避する。`keymap.ini.default` は Action 名と既定キーの参照として起動時に更新する。フレーム中にファイル I/O はしない |
| `key_input.rs` | Windows の subclass 済み HWND と `ViewportId` の単一 registry、送信元 HWND / viewport 付き `KeyEdge` queue、viewport 必須の consume / pressed / frame-state / Enter-held API を所有する。main は `ROOT`、fullscreen / detached は安定した viewer viewport ID を登録し、`WM_NCDESTROY` で対応と同じ HWND 由来の未処理 edge を同時に除去する。未登録 HWND は invariant 違反として記録し、互換側の `ROOT` だけへ配送する |
| `keyboard_input.rs` | viewport pass 単位の `KeyboardOwner` / `TextInputPhase` / `TextInputClaim` / `ShortcutPermit`、FS 固定 Esc / 矢印用 `FullscreenRawKeyPermit`、typed text-input claim と id snapshot だけを受ける純粋な所有者決定、pass cache、既存 App / Keymap 判定への互換投影を所有する。不純な snapshot 収集は `App::keyboard_ownership_snapshot` だけが担い、draft state を所有権に使わない。helper-managed field の focus が begin-pass key 処理で一時消失した pass は `FocusRecovery` が shortcut より優先する。FS raw permit は TextInput 全 phase を拒否し、非テキスト `FocusedUi` はスライダーに矢印を奪われないよう許可する |
| `egui_focus_policy.rs` | egui が `begin_pass` で先行決定する Tab focus traversal を、最初の widget 登録前に常にキャンセルする Context 共通ポリシー。Tab event 自体は keymap / TextEdit の owner が後段で処理する。`PlatformOutput::ime` を viewport ごとに前 pass から引き継ぎ、main / fullscreen / detached / native video overlay で同じ判定を使う |
| `ime_focus.rs` | 各 egui Context の input plugin が viewport 別 composition / 300ms grace の単一 owner となり、`Memory::begin_pass` 前に composing Esc press を除去し、Commit の無い composing `Disabled` へ空 preedit を補う。App / presenter は同 snapshot の read-only projectionを使う。日本語を入力できる single-line TextEdit の共通描画境界、キー処理由来の focus ownership 復帰、直前 pass の helper-managed focus contract、IME 候補選択中だけの Tab focus-lock、caller の focus request latch も所有する。helper field が focused または直前 pass で focused だった間の各 key press は、widget id の継続、egui focus の前後、keyboard owner / phase、左 side-panel close call site を通常 logger の `[text-input-key]` へ常時計装する。通常 record は process あたり 1 MiB で抑止し、id / focus / owner / close の異常 record は継続する。pointer による focus 移動は復帰せず、理由付き allowlist 以外の raw TextEdit を unit test で禁止する |
| `operation_customize_share.rs` | 操作カスタマイズ 3 点セット (`keymap` / `ring_shortcuts` / `menu_layout`) の共有 JSON、未知項目の警告付き正規化、実効チョード単位の差分計算を担う純ロジック |
| `settings_restore.rs` | 設定全体の世代復元に加え、過去世代を一時ディレクトリへ読み取り専用展開して操作カスタマイズだけを抽出し、取り込み前の `.mivkeys.json` 自動退避を管理 |
| `books.rs` | 製本機能。製本ルート直下の通常フォルダを本として扱い、`0001_元名.ext` のページ保存、通常画像/ZIP 内画像の無加工コピー、補正/PDF/動画フレームの焼き込み追加、2 パス temp rename による並べ替えフラッシュを担当 |
| `book_fs_journal.rs` | 製本の改名・並べ替え・別本移動を crash-safe な filesystem step plan として実行する。永続 temp 名、copy/move の SHA-256 identity、冪等な forward / rollback 判定を所有し、phase と進捗の SQLite 永続化は `book_bookmarks.rs` に委譲する |
| `reading_history_db.rs` | 閲覧履歴 (`%APPDATA%/mimageviewer/reading_history.db`)。ユーザーが開いた画像本 / 動画 / 音声を MRU として保持し、`reading-history-writer` で upsert / prune、メディア進捗更新、file metadata 補完を行う |
| `rename_key_migration.rs` | アプリ内リネーム後の path-keyed 永続データ移行と回復ジャーナル。未完了集合の正本は App の in-flight + FIFO queue + boot-retry に置き、ジャーナル書き出しは App-global の単一 latest-value worker が直列化する。起動時の初回 enqueue / poll 前だけ同期 load し、終了時は最新 revision の完了まで flush する |
| `metadata_cleanup.rs` | 明示操作の孤児メタデータ整理。`rename_key_migration::STORES` を正本に全 path-keyed DB を worker で走査し、親フォルダ到達可能な missing だけを確認後に transaction DELETE する。切断ドライブ、本棚、逆引き不能キーは非破壊側へ倒す。mIV 削除後の purge 最終失敗は `delete_purge_journal.json` に path 単位で残し、同じ孤児安全判定 + 共通 hard-purge を起動時 / idle 時にピンポイント再実行する |
| `logger.rs` | ファイルロガー (`mimageviewer.log`)。常時記録 + 16 MiB ローテーション |
| `diagnostics.rs` | 診断 zip 書き出し (`export_diagnostics_zip`)。logs ディレクトリのログ群 + システム情報をまとめてデスクトップに保存。環境設定「開発者」タブから呼ばれる |
| `stats.rs` | 読み込み統計の集計 |

### グリッド / サムネイル / フルスクリーン

| モジュール | 役割 |
| --- | --- |
| `ui_main.rs` | メイン画面のグリッド描画とクリック/ドラッグ処理 |
| `ui_dialogs/smart_folder_editor.rs` | 名前だけのスマートフォルダ作成、現在の実フォルダ + facet 条件をルールとして追加する確認 UI、ルール / グループ化単位の管理 UI。場所 / AI / 画像色など保存対象外の条件を明示する。管理中のテキストはローカル draft に保持し、フォーカス離脱・選択変更・ダイアログ操作の確定境界でだけ Settings 保存と対応 worker / snapshot の無効化を行う。終了・トレイ退避の共通保存境界では有効 draft を Settings へ確定するが worker は開始せず、トレイ復帰時だけ遅延した副作用を1回適用する |
| `ui_fullscreen.rs` | フルスクリーンビューポート (`show_viewport_immediate`)。描画テクスチャの優先順位はここで決定 |
| `displayed_image_transform.rs` | ページ単位の実表示 transform の正本。fit / scale limit / trim / 90 度・free rotation / 通常または Z の zoom-pan から paint・hit・UV rect、source↔screen 写像、total scale を一度に解決する。`FullscreenPageLayout` はフレーム中に実際に描いた Single / Spread / Continuous の各ページ transform を paint 順で保持し、ルーペと範囲キャプチャへ共通 `hit_test` を提供する。見開き・連結読みの配置計算自体は `ui_fullscreen.rs` が担当する |
| `vendor/eframe` | eframe 0.33.3 の Windows hidden/minimized repaint scheduler に upstream PR #7905 を backport するローカルパッチ。不可視 HWND へ `ControlFlow::Poll` + OS redraw を要求せず direct UI pass で pending command を消化し、アプリが要求した repaint だけを `max(要求済み時刻, now + 100ms)` へ制限し、先の予定は早めない。要求が無ければ heartbeat を作らず sleep する。App / tray / detached の状態 ownership は変更しない |
| `vendor/egui-wgpu` | egui 0.33.3 の managed texture に opt-in GPU mipmap を追加するローカルパッチ。`TextureOptions::mipmap_mode` 指定時だけ完全な mip chain を確保・生成し、静止画の論理 `TextureHandle` / cache 構造は変えない。`Rgba8Unorm`生成器は比較callbackと360度パノラマの独自textureにも公開して共用する。上流の`LICENSE-MIT` / `LICENSE-APACHE`本文をcrate内に保持し、アプリ内表示と配布物同梱の正本にする |
| `ui_fullscreen/draw_icons.rs` | フルスクリーン上部バー / 動画 HUD のボタン・アイコン描画 helper、ファイル情報文字列 builder |
| `export_dialog.rs` | Ctrl+E エクスポートのダイアログ状態・worker・ファイル名衝突回避。UI は base pixels / mask / preset を snapshot し、隠蔽合成・画像エンコード・メタデータ転記は worker が担当 |
| `ui_helpers.rs` | メニューバー、ツールバー、アドレスバー等の共通 UI |
| `grid_item.rs` | `GridItem` 列挙型と `ThumbnailState` (Pending/Loaded/Failed/Evicted)。`GridItem::Stack { key, representative, count }` (v2.0.0) はファイル名スタックの畳んだ集約セル (ZipDir と同じ仮想コンテナ扱い = pin/snapshot/file-op/checkable/rating 対象外)。`arrange_grid_items` は実フォルダ / アーカイブ類 / 画像 / 動画・音声の設定行を全グリッド構築経路へ適用する単一チョークポイント |
| `filename_stack.rs` | ファイル名 prefix スタック (v2.0.0) の純ロジック。`StackMember`/`StackGroup`/`StackView` + `group_media` (末尾区切り文字の前でグループ化、動画は単独固定) / `materialize_aggregated` (集約グリッド) / `materialize_flat` (フラット読書フルスクリーン) / flat-index 写像 / `stack_jump_target` (Shift+↓↑)。I/O 無しで unit test 容易 |
| `filename_stack_ui.rs` | 上記の App グルー (bin-only)。トグル / 集約⇔フラットのビュー切替 (`swap_stack_view_items`) / `stack_try_open_from_grid` (集約セル → フラットフルスクリーン) / `stack_reconcile_after_fullscreen_close` (閉じたら集約へ戻す)。集約構築は `load_folder_with_scan` hook 経由。詳細は [filename-stack-plan.md](filename-stack-plan.md) |
| `thumb_loader.rs` | サムネイル並列ロード (WebP キャッシュ生成含む)。Folder 自動代表の再帰探索では子の非 Image pin を元ソースから生成せず、直上 catalog に完全一致の pin WebP がある場合だけ上位へ伝播する。再利用 WebP は完成済み cache origin として idle source upgrade を抑止する |
| `catalog.rs` | フォルダ単位の SQLite catalog。サムネイル WebP、PDF メタデータ、ZIP / 画像のみフォルダのページ数を保持する。ページ数 cache は種別・mtime・file size・判定設定 fingerprint の完全一致時だけ再利用する。再帰 pin の cache-only lookup 用に、DB が存在するときだけ schema 変更なしで read-only open する経路を持つ |
| `folder_thumb_pins.rs` | 親コンテナ (Folder/ZipFile/PdfFile/ConvertibleArchive) の代表サムネ手動ピン DB (`%APPDATA%/mimageviewer/folder_thumb_pins.db`)。`apply_folder_thumb_pin` が cache key に `#pin:{source_id}` suffix を載せて pin の identity を表現し、子の Folder / ZIP / PDF / ZipDir が持つ代表 pin を最終 leaf まで連鎖解決する。cascade の source_id は経路 hash + leaf identity。固定 leaf が Image / ZipEntry / PdfPage なら canonical page key も親要求へ渡し、編集 preview を優先する。Video ピンは `seed_folder_video_pin_thumbs` で `video_pins` から WebP を catalog に seed する。RAR/7z/LZH 変換キャッシュ閲覧中は元アーカイブパスを root key にする |

### 仮想フォルダ (ZIP/PDF) / フォーマット

| モジュール | 役割 |
| --- | --- |
| `zip_loader.rs` | ZIP 内の画像列挙、エントリバイト取得、先頭画像抽出。ネスト ZIP (ZIP in ZIP) は再帰列挙し (表示は `zip_tree` でツリー化)、内側 ZIP バイト列は 256MB LRU キャッシュに保持。非 ZIP アーカイブ (RAR/7z/LZH) のエントリは `has_foreign_archives` フラグで検出して変換提案へつなぐ (v1.3.0)。読み戻しは literal フルネーム一致 → ネスト境界分割の順 (変換キャッシュのフラットエントリ対応)。画像判定は `folder_tree::is_recognized_image_ext` に委譲 (ネイティブ + WIC + Susie) |
| `pdf_loader.rs` | PDFium ワーカープロセスプール。ページ列挙・レンダリング |
| `pdf_passwords.rs` | PDF パスワードの DPAPI 暗号化永続化 |
| `wic_decoder.rs` | HEIC/AVIF/JXL/TIFF/RAW のデコード (Windows Imaging Component) |
| `save_with_metadata.rs` | JPEG/PNG/WebP のエンコードと EXIF/XMP/PNG text/WebP metadata の転記。Ctrl+E エクスポートから呼ばれ、出力は `create_new` で上書きしない |
| `susie_loader.rs` | Susie 画像プラグイン (`.spi`) のワーカープロセスプール。PI/MAG/Q0/PIC/MAKI 等レトロ画像のデコードをルーティング。32bit ワーカー exe は本体に `include_bytes!` で埋め込み、初回起動時に `%APPDATA%\mimageviewer\mimageviewer-susie32.exe` へ自動展開 |
| `archive_converter.rs` | RAR / 7z / LZH / (非 ZIP 入れ子入り) ZIP → 無圧縮 ZIP 変換 (unrar / sevenz-rust2 / delharc / zip)。入れ子アーカイブは一時ファイル経由で再帰展開し (深さ上限 8)、`"inner.rar/p01.jpg"` 形式のフラットなエントリ名で出力する (v1.3.0)。RAR はパスワード付きにも対応するが、入力パスワード自体は保存しない。画像判定は `is_recognized_image_ext` 経由 (Susie 対応拡張子も含む) |
| `archive_cache.rs` | 変換済み ZIP のマッピング DB (`%APPDATA%/mimageviewer/archive_cache.db`)。元ファイルパス + mtime + size で lookup、変換後 ZIP は `archive_cache/<hash[..2]>/<hash>/*.zip`。設定された容量上限がある場合は変換完了後に `last_access_at` の古い順で削除する。パスワード付き RAR 由来でもキャッシュ ZIP は暗号化されないため、管理 UI で `PW` と表示し削除可能にする。将来版/旧版由来の未知 `format` 行も `旧形式 / 不明` と raw format 値で表示し、同じ管理 UI から削除できる |
| `fs_animation.rs` | GIF / APNG / WebP アニメーションのフレーム展開 |
| `video_thumb.rs` | 動画サムネイル取得 (Windows Shell API) |
| `video/` | 動画インライン再生。`mod.rs` (VideoPlayer 公開 API) / `ffmpeg_loader.rs` (FFmpeg LGPL DLL が exe 同居しているか検証 — 展開は launcher が起動時に行い、ロードは Windows ローダが行う) / `decoder.rs` (avformat/avcodec/swscale デコード worker、`VideoDynamicState` で per-frame 状態を atomic 共有) / `audio.rs` (cpal/WASAPI 出力 + ring buffer + VST3 前段の time-stretch) / `audio_stretch.rs` (Signalsmith Stretch による pitch 維持の倍速音声処理) / `clock.rs` (AV マスタークロック)。FsCacheEntry::Video が VideoPlayer を所有。`VideoInfo.dynamic` は decoder thread / native presenter thread / UI で共有し、右パネルの「フレーム表示」「デインターレース」を動的更新する |
| `video/native_touch.rs` | presenter HWND 専用の薄い `WM_POINTER` アダプタ。HWND ごとの bounded stream ownership、client-pixel→points 共通変換、共有 `TouchRecognizer` への入力、先頭接点の pointer emulation、chrome command 写像を担当する。HUD HWND は Phase 3 まで対象外 |
| `folder_tree.rs` | 深さ優先前順トラバーサル (Ctrl+↑↓ 用) |
| `panorama.rs` / `panorama_wgpu.rs` | 360 度パノラマビュー (Phase 1 + 1.5 + 2a)。`panorama.rs` は state / GPano XMP 検出 / 解像度ゲート / settle policy / CPU bilinear sampler / `render_settle_overlay`、`panorama_wgpu.rs` は equirect WGSL シェーダ + 8K base アップロード + settle overlay の alpha blend pipeline。詳細は [`docs/panorama-360-view-plan.md`](panorama-360-view-plan.md) |

### 音楽ビュー / 音声再生 (v2.3.0)

音声ファイル (`GridItem::Audio`) は「映像なし動画」として `FsCacheEntry::Video` の headless
`VideoPlayer` で再生し、フルスクリーンには egui の音楽ビュー (DJ 風タイムライン +
スペクトラム) を描く。動画も HUD の ♪ / Z キーで「動画→音声モード」(presenter を hide して
音楽ビューで聴く hidden presenter 方式) にトグルできる。正本:
[music-integration-plan.md](music-integration-plan.md)、動画→音声モードは
[video-architecture.md](video-architecture.md) の該当節。

| モジュール | 役割 |
| --- | --- |
| `audio_decode.rs` | 音楽解析用の FFmpeg 音声デコード (48kHz stereo f32 固定)。全尺一括 + progressive 差分の 2 API。EOF で swresample の内部 delay を flush する (再生系の `video/decoder.rs` とは独立した解析専用デコーダ) |
| `crates/music-core` | 解析純ロジック (`analysis.rs` = タイムライン bin 化 + FFT/クロマ、`beat.rs` = BPM グリッド、`timeline.rs` / `effects.rs`)。I/O なしで unit test 容易 |
| `ui_music_timeline.rs` | 音楽ビュー中央の行分割波形タイムライン。row raster worker + 行テクスチャキャッシュ (`TimelineTextureCache`、解析の版数 = `music_analysis_version` で再ラスタ判定)。行数は `TIMELINE_MAX_ROWS` でキャップ |
| `ui_music_spectrum.rs` | 下段 108band スペクトラム + 鍵盤。専用 worker が共有 `MusicPcm` の窓を FFT (in-flight 1 件 coalesce) |
| `ui_music_panels.rs` | 音楽ビューの左右ホバーパネル (ブックマーク / ループ / 行秒数) と下 HUD (動画 native HUD とレイアウト一致) |
| `metadata_transfer.rs` / `ui_dialogs/metadata_transfer.rs` / `app/metadata_import_refresh.rs` | 実フォルダ単位の明示メタ情報移送。`mimageviewer.meta.miv` directory の原子的generation pointer + フォルダ単位JSON Lines shard、root-relative path・media kind・file size検証、再帰走査、評価 / タグ / ブックマーク / 見開き / 表示トリム / 回転 / 6種のページ編集 / 代表サムネ・動画ピンをworkerで移送する。変換済みRAR / 7z / LZH / nested-archive ZIPはsource container keyと環境固有cache ZIP page keyを相互変換する。export / preview / importはshardを逐次処理し、総項目数・総sidecarサイズの固定上限を持たない。importは15ストアをattached connectionへまとめ、外側transactionを256項目 / 64 MiB / 500 msで区切り、項目内SAVEPOINTで失敗を隔離する。UIは完全モーダルの確認・固定高進捗・キャンセルと未保存view-trimのメモリsnapshotだけを担当する。開始前にはmain / active detached / paused detachedのmetadata writerを静止・drainし、終了時にXMP readerをcontextごと再開する。DB更新中は既存cacheを安定snapshotとして保ち、終端時に影響contextのcompact keyとlegacy seed path snapshotを専用workerへ渡す。一括再取得したcacheは未タグの読込済みsentinelも含め、items世代照合後にcontextごと所有権を置換して表示集合とcontext所有XMP workerを再構築する。外部snapshot反映はApp-globalなfacet scope / suppressionを変更せず、bookmark presence更新も保持中の全contextへ在メモリ反映する |
| App の `music_*` 状態 | 解析ワーカー / `MusicPcm` / spectrum / timeline cache は **ViewerContextBundle に入れず global** (stage-audio §3.5: ParkedLive 音楽窓も同じ global を消費する)。表示ゲートの中央述語は `fs_music_view_active`、動画→音声モードの transient は `video_audio_mode` / `video_audio_vst` |

### マルチウィンドウ / detached viewer (F12)

⚠️ 構造リワーク中 (凍結ルールあり)。正本:
[detached-rework-plan.md](detached-rework-plan.md) (§2 憲法 = BA-1〜BA-7)。
音声メディア窓は [archive/detached/detached-rework-stage-audio.md](archive/detached/detached-rework-stage-audio.md)。

| モジュール / 概念 | 役割 |
| --- | --- |
| `ViewerContextBundle` (app.rs) | ビューア文脈の状態束。active detached (独立 / Book) と parked live 窓は bundle swap で mount/unmount する。現行仕様に pin はない。通常画像の always-new grid open も open 前に `DetachedPhysical` active bundle へ昇格し、main の検索・選択・スクロールから分離する。仮想一覧由来でも backing `items` は複製せず、通常フォルダは bundle-owned worker scan、ZIP/PDF は既存 enumerate worker で完全な物理一覧を構築し、対象 leaf を新しい物理 index へ解決する。folder-nav pending、PDF/ZIP enumerate、nav lock generation、遅延 nav intent、holdover texture も context-owned。thumb channel / cancel_token / ワーカーキュー / keep-range atomic の「ロード複合体」も per-context (v2.3.0、bundle Drop が worker pool を畳む)。swap前後のApp-global rating session差分は各context cacheへ同期するが、ownership交換自体はnavigationではないためfacet scope / suppressionを変更しない |
| detached open request state (app.rs) | 通常画像のscan、ZIP/PDF enumerate、protected PDFのpassword request・session password・保存予約、明示leafの`Required` targetをbundle内に保持する。グリッドのFolderはmain-owned worker scanで「候補」を先に分類し、画像本と確定したcompleted scanだけをdetached contextへ移譲する。混在Folderはsession/runtimeを作らず通常main navigationへ戻す。password dialogはowner bundleをmountして再開する。空／error／必須target消失でviewportが未生成でも共通terminal closeがsession finishとwindow runtime削除を行う。実フォルダscan errorは空一覧と区別し、catalogへ適用しない |
| `ViewerSession` (`app/viewer_session.rs`) | 退避中 bundle の表示先・同期 stamp・独立 detached 状態・window ID を一括所有する。現在表示中の session は当面 `App` の既存フィールドへマウントし、`swap_with_mounted` で5項目を同時交換する |
| `DetachedWindowManager` (`app/detached_window_manager.rs`) | 窓ごとの HWND / placement / 状態遷移 (`Opening/Active/Parked/ParkedLive/Resuming/Closing`) と activation watcher を一元管理する。passive window は window collection / frozen 表示上の役割であり、runtime state 名ではない。`ViewerSession` の意味状態とは分離する |
| `dwm_transitions.rs` | DWM トランジション抑止 + UI スレッド窓 snapshot (HWND を生成イベントの before/after 差分で同定 = rect 一致捕捉の全廃、BA-1 根治) + 仮想デスクトップ移動 |
| `app/native_video.rs` | F12 host migration / source-swap / 動画→音声モード enter/exit など、native 動画 presenter と detached 窓の接続層 |

BA-1 の不変条件は geometry 非依存の HWND 所有である。detached host 登録は registry 化済みだが、
キー入力 subclass に rect 選択が残る具体的な仕様違反は
[v2.8.1 detached 監査](review-v2.8.1/s2-detached.md) に記録されており、後続リワークへ引き継ぐ。

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
| `spread_db.rs` | フォルダ別のページ構成・連結方式永続化 (単ページ / 見開き + ページ単位 / 縦連結 / 横連結。DB 名は互換のため `spread.db`) |
| `ai/` | ONNX Runtime (DirectML or TensorRT) によるアップスケール / デノイズ / Inpainting / 補正レイヤー被写体選択と、ヒューリスティックによる画像種別分類。`AiBackend` で multi-EP 対応、TRT 用に `tensorrt_pack` (DLL pack 検出) と `tensorrt_builder` (子プロセスエンジンビルダー) を持つ。TRT 推論はメインから別プロセスへ shm + JSON IPC でルーティング (`trt_worker_pool` / `trt_worker_proto` / `trt_worker_runtime` / `trt_worker_shm`)。TensorRT 設定でも起動時には worker を自動起動せず、AI 処理が実際に必要になった最初のタイミングで遅延起動する。worker 起動ハンドシェイクは provider DLL 初期化遅延を見込んで 45 秒待ち、transient な起動失敗は 1 回だけ silent retry する。worker 死亡を検知したら自動 detach + DirectML フォールバック + UI バナー通知。U²-Netp 被写体選択は小型モデルとして in-process CPU 強制ロードで使う |
| `png_metadata.rs` | PNG の tEXt/iTXt/zTXt に埋め込まれた AI メタデータ読み取り |
| `exif_reader.rs` | EXIF 読み取り (rexif)。構造タグの抑止と Exif 2.3x 拡張タグの日本語名マップを持つ |
| `xmp_reader.rs` | XMP 読み取り (quick-xml)。`xtw:*` 名前空間のツイート情報を抽出し「XMP ツイート情報」パネルに表示 |

### UI オーバーレイ / ダイアログ

| モジュール | 役割 |
| --- | --- |
| `ui_adjustment_panel.rs` | 画像フルスクリーン左パネル。`画像補正 / 表示トリム / ブックマーク` の 3 タブを持つ。画像補正はプリセット切替・AI 設定・保存スロットを扱い、ヘッダーの 消しゴム / 補正レイヤー / 隠蔽加工 / 切り取り アイコンからそれぞれ `×` 付き独立左パネルを開く。ブックマークは現在の本のサムネイル一覧、ページ移動、DB 行削除を扱う。エクスポートアイコンは `Ctrl+E` と同じダイアログへ合流する。補正レイヤーは `local_adjust_lab` と同じ左パネル + 右ツールパネル構成で、効果 UI、マスク編集、U²-Netp 被写体生成、クラシック領域分割生成を UI thread 外の worker 経由で扱う |
| `ui_crop.rs` | 切り取り (crop) モードの独立左パネル。比率選択 / 有効化・解除 / 自動クロップ / X・Y・W・H 数値入力、crop 外暗転 overlay、ハンドルドラッグ、見開きから Single への pivot を扱う。実際の切り出しは capture / export の最終段 (`export_crop.rs`) |
| `ui_view_trim.rs` / `view_trim.rs` / `view_trim_db.rs` | 表示トリム (読みながら使う表示専用の余白カット)。フルスクリーン左ホバーパネルの `画像補正 / 表示トリム / ブックマーク` タブから、`トリムなし / 自動余白カット（本全体） / 手動設定` をラジオで選ぶ。`手動設定` のときだけスライダーの直上に `適用範囲：[本全体][このページ]` を出し、enabled なページ個別行の有無から選択を導出する。手動設定では単ページ / 見開き連動 / 見開き左右別の 0〜20% トリムを調整する。`draw_fs_image` / `draw_fs_spread` の fit 基準 bbox と描画 UV を変え、bbox 外は背景に落とす。見開き中央側のトリムは左右の見える端を gap に合わせて再配置し、操作判定は見える rect を使う。モード / 本全体設定は本キー、ページ個別設定は `page_path_key` で `view_trim.db` に保存する。表示解決は `idx` ごとにページ個別設定を優先し、無ければ本全体へ戻す。トリムなし / 自動余白カットはページ個別設定を参照しない。保存 / エクスポート / crop DB には影響しない |
| `book_bookmarks.rs` / `bookmark_browser.rs` | 本ページの安定 identity を `book_bookmarks.db` へ非同期保存し、既存 `video_bookmarks.db` と合わせた全メディア横断 read model を構築する。アプリ内の本 / ページのリネーム、製本ページの並べ替え・別本移動は旧 path → 新 path mapping を専用 transaction で identity へ反映する（missing 行保持のため共通 hard-purge store には含めない）。横断一覧は専用ダイアログを持たず `TopLevelGridSurface::Bookmarks` として通常の `App.items` / facet / details 表示へ載せ、ID・位置・登録日時・欠落状態を sidecar row に保持する。SQLite、存在確認、動画 WebP decode、ZIP / PDF 列挙、削除は worker 側で、ZIP / PDF の missing inventory は一覧構築中にコンテナ単位で共有する。削除は DB 行だけを対象にし、元メディアへ filesystem 操作を行わない。横断一覧から開く要求は単調増加 request ID と target identity を path resolver、RAR / 7z / LZH の `ArchiveConvertState`、viewer 待機まで運ぶ。元アーカイブ → 直接 RAR / 変換キャッシュ ZIP は owner 付き内部遷移とし、navigation / activation / cancel / disconnect / timeout は一致する owner と変換 receiver だけを終了して stale completion を表示へ適用しない |
| `ui_analysis_panel.rs` | 画像分析パネル (右端オーバーレイ)。色情報・ヒストグラム |
| `ui_metadata_panel.rs` | メタデータパネル (AI メタデータ + EXIF + XMP ツイート情報) |
| `ui_erase.rs` | 消しゴムモード (筆 / 囲み / 直線 / 縦線 / 横線 / 矩形 / 楕円 → MI-GAN で inpaint) |
| `ui_conceal.rs` | 隠蔽加工モード (同じマスク編集 UI でモザイク / 塗りつぶし / ぼかしを合成) |
| `ui_dialogs/` | 環境設定・サムネイルキャッシュ管理・変換済みアーカイブキャッシュ管理 (`archive_cache_manager.rs`)・アーカイブ変換ダイアログ (`archive_convert.rs`)・お気に入り編集・スライドショー設定等。アーカイブ変換は `ArchiveConvertState` が scan / password retry / convert 共通の cancel token と completion policy を所有し、state drop と競合 navigation で worker と receiver を同時に終了する。モーダル相当の表示状態は `App::common_modal_dialog_open` に集約し、`process_scroll` のポインタ直下 floating-layer guard と組み合わせてダイアログ内 wheel の背面グリッドへの伝播を防ぐ。TensorRT パック取得のような長時間ツール Window はモデルレスとし、表示中も閲覧を止めない |
| `native_name_dialog.rs` | 名前変更 / 新規フォルダ作成で共有する Windows 標準の単一行入力画面。メモリ上のダイアログテンプレートを同期モーダル表示し、IME・書記素編集・クリップボード・Undo を OS に委譲する。非 Windows では no-op stub |
| `ui_dialogs/preferences.rs` | 環境設定ダイアログの状態、App 連携、ツリー / ページ dispatch |
| `ui_dialogs/preferences/pages.rs` | 環境設定の各 `page_*` 描画関数 |
| `ui_susie_diagnostic.rs` | Susie プラグイン診断パネルの描画。環境設定の「ファイル処理 → Susie プラグイン」ページから切り出し、`PoolStatus` 各バリアントごとにメッセージ・配色を出し分け。`egui_kittest` のスナップショットテスト対象 |
| `changelog_markdown.rs` | 更新履歴 (GitHub release body) の Markdown サブセット描画。バージョン更新ダイアログ (`ui_dialogs/update_notice.rs`) から呼ばれ、見出し / 箇条書き / `**強調**` / `` `コード` `` / `<kbd>キー</kbd>` を整形。`egui_kittest` のスナップショットテスト対象 |
| `version_highlights.rs` (v2.0.0) | 更新後初回起動の「重要な変更点」(④)。exe 埋め込みテーブル + 純関数 `highlights_to_show` (またぎ累積、unit test) + 描画 `render`。ダイアログは `ui_dialogs/whats_new.rs`。`update_check` (更新前・ネット・全文) と別物で、更新後・オフライン・操作/既定変更の主要部分のみ。トリガは `last_seen_version` / `previous_last_seen_version` |

### 検索 / インデクサ / タグ

**全体像は [search-architecture.md](search-architecture.md) を参照**。

| モジュール | 役割 |
| --- | --- |
| `search_query.rs` | 検索クエリのトークナイザ + マッチャ (AND / OR / NOT / `"..."` フレーズ、`MatchMode`)。3 モード共通 |
| `search_norm.rs` | `normalize_for_match` — ingest / クエリ / post-filter で共有する唯一の正規化関数 (lowercase) |
| `search_index_db.rs` | Ctrl+S 用 `search_index.db` (お気に入り配下のフォルダ / ZIP / PDF / 動画名) |
| `fts_index.rs` | Tantivy 0.26 ラッパ。`IndexDoc` / `Fields` / `QueryFilters` / `build_bigram_and_query` / `search_page`。bigram tokenizer + lower_caser |
| `fts_meta.rs` | `fts_meta.db` (SQLite) ラッパ。ファイル単位の管理メタ (path / mtime / size / status=Ok\|Failed / index_generation)。検索原文は持たない (Tantivy STORED に集約) |
| `ingest_text.rs` | `PerSourceText` (filename / exif / xmp_tweet / png_prompt / pdf_meta / video_meta / sidecar、旧 tags は移行専用) のビルダー |
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
| `tags_db.rs` | `%APPDATA%/mimageviewer/tags.db`。`item_tags(item_key, tag, tag_key, applied_at)` / `tag_item_state` / `tag_meta`。mIV タグの正本。最初のタグ書き込み前に `tags.db.bak1..bak10` の世代バックアップをローテート。設定 ON 時だけ `mimageviewer.dat` に実ファイルタグをバックアップし、import 同期状態はタグ用に独立管理する |
| `tag_ops.rs` | UI からのタグ操作ファサード。6 種の実パス item を対象に all-or-nothing 付与/削除を決め、worker へ投入 |
| `tag_write_worker.rs` | UI → tags.db 更新 worker。通常タグ操作ではメディア本体 / XMP サイドカー / Tantivy へ書き込まない |
| `tag_legacy_xmp_worker.rs` | 旧バージョンが XMP `dc:subject` / 動画 `.xmp` に残した `#` タグを、ユーザー明示操作で `tags.db` へ union する worker。「取り込んで削除」では DB 反映成功後に `#` 要素だけを除去し、空殻になった動画 `.xmp` だけを削除する |
| `xmp_writer.rs` | 既存 XMP 書換 helper。タグでは旧 `dc:subject` 移行・明示除去系の補助に縮退、rating 書込みでは引き続き使用 |

### その他

| モジュール | 役割 |
| --- | --- |
| `gpu_info.rs` | GPU 情報取得 (VRAM サイズ等)。取得失敗時の 4 GiB fallback を含む共有 pool の入力を提供する |
| `vram_budget.rs` | mImageViewer 全体の GPU メモリ予算の単一所有者。永続設定の割合から共有 pool を求め、一覧 / フルスクリーンの 80% / 20% 配分、RGBA8 bytes / texels 換算、HIGH / LOW 水位を導出する。0% は無制限として明示的に扱う |
| `monitor.rs` | モニター情報取得 (DPI 等) |
| `open_with.rs` | 外部アプリで開く |
| `file_drag.rs` | グリッドからエクスプローラ等へのファイル D&D 送出 (シェル `IDataObject` + `SHDoDragDrop`)。`docs/file-drag-drop-design.md` |
| `os_theme.rs` | Windows の「アプリ用の色」(レジストリ) を検出し、egui::Visuals へ適用。初回起動時に `Settings::ui_theme` の初期値を決める |

GPU テクスチャの利用許容量は `vram_budget.rs` の共有 pool だけから導出する。サムネイル側と
フルスクリーン側が物理 VRAM を別々に取得して独立上限を持つことは禁止し、AI 完了結果の保持
`Arc<ColorImage>` は GPU ではなく CPU メモリ用の独立 LRU とする。

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
| `settings.db` (SQLite, 2026-05 移行) | アプリ全体設定・グローバルプリセット・保存スロット・お気に入り (`FavoriteEntry { id: Uuid, name, path, auto_index_{structure,metadata,thumbs} }`)・タグ定義 (`Vec<TagDef>`)・VST3 chain 設定 (大型 BLOB)。**SQLite トランザクション + `VACUUM INTO` で `settings.db.bak1..bak10` に世代スナップショット**。`schema_meta.app_version` は open 時でなく正常な `save_full` の commit 時に更新し、各 snapshot の保存元版を保持する。物理的な Corrupted 検出時だけ `.corrupted-<ts>-<seq>` 3 セット (main + WAL + SHM) で quarantine、bak1→bak10 を新→古で試行し復旧する。保存元版が現バイナリより新しい、または未知の設定 enum / field がある場合は `IncompatibleSettings` とし、main と backup chain を変更せず save 抑止する。復元 UI は bak1..bak10 と `settings.db.preupgrade-v<old>` の保存元版・互換性を一覧表示する。**Transient I/O / Incompatible / 全復旧失敗時は `MAIN_UNREADABLE_THIS_SESSION` + `settings_db::SAVE_SUPPRESSED` で `Settings::save()` 完全 no-op 化**し、初回設定等を抑止して設定復元または終了の保護モーダルを表示する (= 残骸保護)。旧版の継続利用は他の永続DBまで読み取り専用にできないため許可しない。旧 `settings.json` は初回起動時に migration して `*.migrated-<ts>` にリネーム済み | `settings.rs` + `settings_db.rs` |
| `Pictures\mimageviewer\books\...` (既定、設定可) | 製本した本の実体。DB ではなく通常フォルダ + `0001_元名.ext` 画像ファイルのみ。`Settings.book_root` で変更でき、Ctrl+S/Ctrl+G の自動索引対象外 | `books.rs` + `ui_main.rs` + `ui_fullscreen.rs` |
| `Settings.keymap` / `keymap.ini.default` | キーボード割り当て設定。GUI 編集の正本は `settings.db` 内の `Settings.keymap`。旧 `keymap.ini` が残っている環境では初回起動時に読み込み、同じ内容を `Settings.keymap` へ移してから `keymap.ini.imported*.bak` へリネームする。以後 `keymap.ini` は通常読み込み対象外。`keymap.ini.default` は現在バージョンの Action 名と既定キーを確認する参照ファイルとして更新される。競合は拒否せず warning として扱う | `keymap.rs` + `settings.rs` |
| `catalog.db` | フォルダ単位のサムネイル WebP キャッシュ (BLOB) + PDF メタデータ + ZIP / 画像のみフォルダのページ数 cache。ページ数取得は詳細遅延 worker が `GlobalIoSemaphore` 配下で行い、cache 障害時は表示自体を失敗させず元コンテナから再取得する | `catalog.rs` + `app/metadata_ops.rs` |
| `auto_aspect_cache.db` | Auto サムネイル比率のフォルダ別前回確定値。フォルダ再訪時はこの値を初期 `auto_aspect.current` にして、後続の実統計で必要なら既存ゲート (streak/cooldown 等) に従って補正する。サムネイルキャッシュ管理の削除操作と連動してリセットされる | `auto_aspect_cache.rs` + `app.rs` |
| `rotation.db` | 非破壊回転角 (0/90/180/270) | `rotation_db.rs` |
| `audio_normalize.db` | 動画ファイル単位のノーマライズ測定値 (integrated LUFS / true peak / 算出ゲイン)。主キー `(path_lower, file_size, mtime_ms, target_lufs_milli)`。環境設定 → 動画・音声 → 動画から全件クリア可能 | `audio_normalize_db.rs` |
| `rating.db` | レーティング (★1〜5、0 は未登録)。ページ単位 (画像/ZIP 内画像/PDF ページ) とコンテナ (フォルダ/ZIP/PDF 本体) を同一テーブルに格納。キー形式の違い (`::` の有無) で区別 | `rating_db.rs` |
| `search_index.db` | Ctrl+S 用。お気に入り配下のフォルダ/ZIP/PDF/動画名索引 | `search_index_db.rs` |
| `fts_index/` | Ctrl+G 用 Tantivy index (複数 segment + meta.json)。bigram 候補絞り込み。旧 `tags` STORED は tags.db 移行専用 | `fts_index.rs` → `ingest_worker.rs` |
| `fts_meta.db` | ファイル単位の管理メタ (path / mtime / size / status=Ok\|Failed / index_generation)。検索原文は持たず Tantivy STORED に集約 | `fts_meta.rs` |
| `adjustment.db` | ページ個別補正 (`page_params`) とお気に入り標準補正 (`favorite_params`) | `adjustment_db.rs` |
| `mask.db` | 消しゴムマスク (deflate 圧縮 1bit/pixel + ベクタオブジェクト JSON) | `mask_db.rs` |
| `conceal.db` | 隠蔽加工マスク (deflate 圧縮 1bit/pixel + ベクタオブジェクト JSON) とマスクスロット | `conceal_db.rs` |
| `local_adjust.db` | 補正レイヤーのページ単位 JSON。中央 DB が authoritative で、`mimageviewer.dat` の `local_adjust_layers` はフォルダ移動時の復元用バックアップ | `local_adjust_db.rs` + `sidecar.rs` |
| `export_crop.db` | 最後段 crop のページ単位矩形。中央 DB が authoritative で、`mimageviewer.dat` の `export_crop` はフォルダ移動時の復元用バックアップ | `export_crop.rs` + `sidecar.rs` |
| `view_trim.db` | 表示専用トリム。`view_trim_books` に本ごとの基本適用モード / 本全体設定、`view_trim_pages` にページ個別設定を JSON で保存する。手動設定では enabled なページ行そのものが永続する「このページ」選択を表し、ページ表示時に本全体より優先される。出力用 crop とは独立し、コピー / 保存 / Ctrl+E には焼き込まない | `view_trim_db.rs` + `ui_view_trim.rs` |
| `comic.db` | Ctrl+T テキスト注釈のページ単位 JSON。吹き出し・テキスト・ウィンドウ・スタンプ配置の正本で、ユーザー画像スタンプは配置先の注釈に `Embedded` として埋め込む | `comic_db.rs` + `ui_text.rs` + `sidecar.rs` |
| `comic_user_stamps.db` | Ctrl+T スタンプピッカーのユーザー画像履歴。配置時の長辺 1024px 上限 PNG を再利用用 MRU として保持する。履歴から選んでも配置先には `Embedded` をコピーするため、履歴削除は既存注釈に影響しない | `comic_user_stamps.rs` + `ui_text.rs` |
| `spread.db` | フォルダ別表示モード (ページ構成: 単ページ / 見開き、連結方式: ページ単位 / 縦連結 / 横連結) | `spread_db.rs` |
| `book_resume.db` | 本 (フォルダ/ZIP/PDF) ごとの最後に読んだページ index。再起動を跨いで読書位置を復元する (動画 `video_resume_positions` の画像本版)。`open_fullscreen` で記録、自動オープン時に「続きから」開く / 通常オープン時はグリッド選択を復元 | `book_resume_db.rs` |
| `reading_history.db` | ユーザー操作で開いた画像フォルダ / ZIP / PDF / 変換アーカイブと動画・音声ファイルの MRU。既存 kind 文字列を維持し、`video` / `audio` と専用の `media_position_ms` / `media_duration_ms` を追加する。未知 kind は行ごと読み飛ばす。保持件数は 1..=1000 | `reading_history_db.rs` + `App::record_reading_history` |
| `folder_thumb_pins.db` | 親コンテナ (Folder/ZipFile/PdfFile/ConvertibleArchive) の代表サムネ手動ピン。container_key 主キー (= normalize_keep_drive 済みパス) で 1 行 1 コンテナ、source は kind + container 相対 rel + (zipentry の) entry / (pdfpage の) page。`apply_folder_thumb_pin` と変換アーカイブの `archivethumb:*#pin:*` が cache key suffix `#pin:{source_id}` で identity を表現し、cascade 時は途中コンテナを含む経路 hash を source_id に加える | `folder_thumb_pins.rs` |
| `edit_preview_cache.db` | 非破壊編集結果を一覧へ戻す派生 preview の対応表 / LRU。page source の mtime + size に加え、ZIP/PDF 親代表から同じ preview を安全に読むため container size も保持する。WebP 本体は `edit_preview_cache/` 配下 | `edit_preview_cache.rs` |
| `video_pins.db` | ユーザーがフルスクリーン HUD で指定した動画フレームの抽出 WebP。`(path, pin_pts_secs, thumb_webp, thumb_pts_secs)`。folder thumb pin の source が動画のときは `seed_folder_video_pin_thumbs` が起動時にこの WebP を catalog にミラー seed する。左ジャンプパネルのピン行もこの WebP を再利用する | `video_pins.rs` |
| `video_bookmarks.db` | 動画ブックマーク (pts / title / jump panel 用 WebP)。初回表示時に FFmpeg worker で取得したサムネを `thumb_webp` に保存し、次回以降は DB から復元する | `video_bookmarks.rs` |
| `video_chapter_thumbs.db` | 埋め込みチャプターの jump panel 用 WebP キャッシュ。path + file size + mtime + chapter start をキーにし、動画更新後は古いサムネを参照しない | `video_chapter_thumbs.rs` |
| `pdf_passwords` | PDF パスワード (DPAPI 暗号化) | `pdf_passwords.rs` |
| `pdfium.dll` | 初回起動時に exe から展開 | `lib.rs` (`run`) |
| `models/*.onnx` | 初回起動時に exe から展開 | `ai/model_manager.rs` |
| `mimageviewer.log` | **常時記録** (旧 `--log` ゲートは撤廃、`--log` 引数は no-op)。起動ごとに truncate し、前回分を `mimageviewer.log.prev` に退避。実行中は 16 MiB 超で `mimageviewer.log.bak` にローテーション | `logger.rs` |
| `logs/panic.log` | Rust panic フックが backtrace 付きで append。セッションを跨いで蓄積するため 4 MiB 超で `panic.log.bak` に 1 世代退避 | `lib.rs` (`append_panic_log_entry`) |
| `logs/settings.log` | 設定復旧経路の永続診断ログ。**logger の初期化状態に依存しない独立 sink** なので起動ごく初期の復旧フェーズでも残る。SQLite open 時の primary code + extended code、bak 復旧の経路、quarantine、preupgrade snapshot、save 抑止のイベントが残る。再現が難しい設定リセット系報告の事後解析用 | `settings.rs` (`settings_diag_log`) + `settings_db.rs` (`log_diag`) |
| `logs/perf_events.jsonl` | 構造化イベントログ (JSON Lines)。`--perf-log` 引数または環境設定「開発者」タブの「性能ログを記録する」が ON のときだけ生成。起動ごとに rotate (`perf_events.1..4.jsonl`) | `perf.rs` |
| デスクトップ `mImageViewer_diag_<日時>.zip` | 環境設定「開発者」→「ログを zip にする」で生成する診断 zip。logs ディレクトリのログ群 + `system_info.txt` をまとめる | `diagnostics.rs` |

**パスキーの正規化**: Windows は大文字小文字非区別なので、すべての DB は **小文字化 + バックスラッシュ→スラッシュ** に正規化してから格納する。新しい DB を追加するときも同じ規約に従う (`rotation_db.rs` / `adjustment_db.rs` を参照)。

### フォルダ側サイドカー (`mimageviewer.dat`)

`adjustment.db` (ページ個別補正)、`mask.db` (消しゴムマスク)、`conceal.db` (隠蔽加工マスク)、`local_adjust.db` (補正レイヤー)、`export_crop.db` (最後段 crop)、`comic.db` (Ctrl+T 注釈) のバックアップとして、各ユーザーフォルダの直下に `mimageviewer.dat` を置く (Hidden + System 属性付きの JSON)。中央 DB が authoritative で、フォルダを丸ごと別ドライブへ移動した際に中央のパスキーが無効化されるケースの復旧経路。設定トグル (`sidecar_backup_enabled`、デフォルト ON) で ON/OFF できる。補正レイヤーは各エントリの `local_adjust_layers` 配列、最後段 crop は `export_crop`、Ctrl+T 注釈は `comic` として保存し、中央 DB に既存エントリがある場合はインポート時に上書きしない。書き込むモジュール: `sidecar.rs`。詳細は [preset-and-adjustment.md](preset-and-adjustment.md) §9 と [virtual-folders.md](virtual-folders.md) §6 を参照。

この自動 sidecar は日常的な編集バックアップであり、明示的な持ち運び用 manifest ではない。
設定が OFF でも `mimageviewer.meta.miv` の export / import は独立して動作する。

### 明示メタ情報転送 (`mimageviewer.meta.miv`)

v2.7.0では安定化のためメニュー入口を一時非表示にしたが、v2.8.0の継続開発では
`metadata_transfer::UI_ENABLED`を有効化して再公開する。

`ファイル > メタ情報をエクスポート / インポート` は、自動バックアップの
`mimageviewer.dat` と独立した versioned bundle directory を実フォルダ直下へ作る。v7 は評価、
タグ（名前・適用時刻と旧XMP seedを制御する決定状態）、サムネイル付き動画・音声ブックマーク /
本ブックマーク、見開き・表示トリム・回転、ページ補正 / マスク /
部分補正 / crop / 注釈、フォルダ代表サムネ・動画ピンを対象にする。crop矩形は通常DB /
sidecarと同じ元画像ピクセル座標を保持する。ZIP / PDF のページ stateと ZIP 内の本 state は
物理コンテナ配下の相対キーで持つ。RAR / 7z / LZHと、入れ子非ZIPを含んで変換されたZIPは、
ページ側とコンテナ側で独立したoriginをmanifestの`virtual_key_base` / `container_key_base`へ保存する。
ページ情報はexport元のarchive cache DBで有効なsource/cache対応も確認し、cache ZIP側のページkeyを
相対化してimport先data directoryから決まるcache ZIP keyへ復元する。見開き・連結方式・綴じ方向と
本単位表示トリムは実行時の`nav.tree.zip_path`と同じく変換後cache ZIP基点なので、実際に検出した
source/cache originを移送先の決定的cache keyへ対応付ける。一方、代表サムネピンは
`zip_pin_root_path`由来の元アーカイブsource keyを維持する。この非対称は実行時DB key体系に合わせたものとする。
未リリースの開発ビルドだけが作成したv1〜v6 bundleは移行せず、v7だけを受け入れる。
前回の未コミット検証で作成されたv6も、必須fieldを黙って補わずversion gateで明示的に拒否する。
archive cache DB自体がない場合と対象行がない場合だけを未変換扱いにし、open / query失敗は
ページメタ情報を黙って欠落させないようexport全体のエラーにする。
cache ZIPと`converted_archives`行を管理画面削除・容量pruneで失っても、cache pathはsource pathと
data directoryから決定的に再生成できるため、source/cache両prefixを全ページメタ情報familyの
scope、および`spreads` / `view_trim_books`のコンテナscopeへ入れる。cache側だけに行が残る場合は
`ConvertedCache`として移送し、source/cache双方に行がある場合はページ・コンテナそれぞれのorigin内で
DB間の走査順による暗黙mergeをせず、対象アーカイブ付きの競合エラーにする。
この競合は同じアーカイブを直接閲覧と変換cache経由の両方で編集した可能性を利用者へ示し、
どちらかを推測して統合せずexport全体を中止する。
一方、直接閲覧RAR / CBRは`ConvertibleArchive`として列挙されてもsource RAR keyを参照するため、
いずれか一方でページ行を実際に検出した場合は、そのoriginを有効cache行や拡張子からの推測より
優先してmanifestの`virtual_key_base`へ保存する。

bundle は `mimageviewer.meta.miv/manifest.json` を現在generationの小さいpointerとし、
`generations/<id>/shards/<folder-hash>.jsonl` にrootと各サブフォルダの直下項目を1recordずつ
保存する。bundleは別環境へコピーするための成果物なので、WindowsのHidden + System属性を付けず、
旧ビルドが作成したbundleも再exportの公開時にこの2属性だけをbest-effortでクリアする。隠したままでは
ExplorerのCtrl+Aコピーや`xcopy`（`/H`なし）等でbundleだけが漏れ得るためである。mIV自身の通常一覧、
サブフォルダ展開、スマートフォルダ、フォルダペイン、検索・名前索引からの除外は
`mimageviewer.meta.miv`と一時directoryの名前に基づき、Windows属性や隠しファイル表示設定に依存しない。
日常バックアップの自動sidecar `mimageviewer.dat`は従来どおりHidden + System属性を維持する。
再帰exportはフォルダ境界をまたぐ4096物理項目ごとにDB scopeを作り、深さ優先で
逐次serializeするため、全項目や単一フォルダの全子要素を1つの`Vec`やJSONへ集めず、
総項目数と総bundleサイズに固定上限を設けない。防御上限は
manifest 1 MiB、shard header 256 KiB、単一物理項目record 256 MiBに限定する。
完成前generationはpointerから参照されず、全shardのflush / sync後に`manifest.json`だけを
原子的に置換する。キャンセル・書込失敗では既存pointerを維持し、公開済み内容を壊さない。
crashで残った未公開generationは、次回exportの開始時と公開成功後に現pointerのgenerationを
保護しながらbest-effortで回収する。

preview / importは各shardをbounded line readerで逐次読み、folder hash、direct-child配置、
shard / entry総数、record単位validationを検証する。importはDBを触る前に現generationの全shardを
streaming preflightし、重複pathはRAM上の全件`HashSet`ではなく一時SQLite indexで検出する。
各file recordにはsize / mtimeを保存するが、移送性能を優先してファイル内容digestは作らない。
importは相対path・entry kind・media kind・sizeが一致すれば同一項目として扱い、mtimeだけが変わる
コピーと、同一sizeで内容が変わったファイルのどちらにも適用する。media kindも明示し、
画像への時刻bookmark、音声へのvideo pin、
動画へのpage edit、通常fileへのvirtual pageなどの組み合わせをpreflightで拒否する。
移送先環境で拡張子から再導出したmedia kindとの差は項目単位で判定し、Susieプラグイン構成だけで
変動する`Image`と`OtherFile`間はそのまま適用する。それ以外の差はbundle全体を中断せず、該当項目だけを
`KindMismatch`としてスキップし、previewと完了結果へ件数を表示する。
Susieが静的テーブル上の動画 / 音声 / ZIP / PDF / 変換アーカイブ拡張子を画像として主張した場合は
`Image`とその静的種別の差になるため免除せず、件数表示付きの項目スキップへ倒す。実在する主要Susie
拡張子は静的テーブルと衝突せず、環境差だけで変動する通常形が`Image`と`OtherFile`間だからである。
2回目の走査では対象pathを再検証し、256物理項目 / 実record 64 MiB / 500 msのいずれかで
外側transactionをcommitする。各物理項目はSAVEPOINTで隔離し、1件の失敗が同じbatchの
他項目を巻き戻さない。走査途中のI/O・parse・validationエラーと明示キャンセルでは現在batchも
commitして部分完了結果を返す。プロセス異常終了時は現在batchだけがrollbackされ、完了済みbatchは
保持される。
manifestに記載された物理項目だけを上書きし、未記載項目を保持する。適用時は既存
`mimageviewer.dat` の mtime を編集 / タグ sidecar sync table に記録し、明示 import 後の
フォルダ再ロードで古い自動バックアップが欠落行を復活させないようにする。
ZIP / `ConvertibleArchive`のrating / tag / page-state familyは、manifestで選択されたoriginに
かかわらずsource pathと決定的cache pathの両prefixを同一DELETE文で消去してから、選択originへ
復元する。これにより直読みRARへ戻した後も旧cache値を表示・再exportしない。
`spreads` / `view_trim_books`も同様にsource/cache両コンテナprefixを消去してから
`container_key_base`で選択したoriginへrootとnested bookを復元する。`folder_thumb_pins`はこの削除対象へ
混ぜず、従来どおりsource keyだけを上書きする。
同一フォルダのsync upsertはsectionごとに1回へ集約し、file種別上存在し得ないDB familyは
削除・挿入処理自体を省く。family削除はpath indexを利用できるexact + range条件で行う。
dirty な自動 sidecar のserialize / temp書き込み / renameはimport workerの前処理で行い、
失敗時はDB更新を開始しない。項目の全15ストアはbundled SQLiteのATTACH上限を20へ拡張して
単一transactionへ参加させる。通常WALの`tags.db`はAppのidle connectionをworkerへ移して閉じる
だけでなく、App-globalなtag write workerの既存jobをdrainして保持接続の解放ACKを待ってから、
import中だけDELETE journalへ切り替える。解放要求中に到着したタグjobは捨てずにworker内へ保留し、
importの成功 / キャンセル / 失敗 / 開始失敗後に要求を下ろして、次jobの直前にWAL接続を遅延再open
して処理する。接続解放ACKには上限を設け、到達しなければimportを開始しない。他の参加DBを含め
WAL / MEMORY / OFFを検出した場合は
適用前に拒否し、SQLite super-journalによるbatch内の全参加DB atomicityを維持する。
manifest 由来の画像本の相対ページは provenance と canonical container を一覧から
metadata / thumbnail / fullscreen worker まで保持し、実 I/O では開いた同一ハンドルの
final path を再検証してから、そのハンドル由来の metadata / bytes だけを利用する。
archive member / nested container の identity は大小文字を区別せず、`\`を`/`へ統一してから
重複検査・DB書込へ共用する。import中は値単位の表示差分を送らず、既存UI cacheを安定snapshotとして
保持する。成功・キャンセル・部分成功の終端で変更sectionを1回だけ確定し、影響するmain /
detached / parked contextの現在項目キーを3 ms / 2048項目のフレーム予算でcompact snapshot化する。
専用refresh workerがrating / tag / page state / container state / pinをSQLiteからbatch取得し、
UIはitems generationとdetached window identityが一致する完成cacheだけをswapする。不一致時は
モーダルを維持して再snapshot・再取得する。bookmark / media cacheは影響contextごとに1回だけ
無効化し、全体page badge集合もworker生成snapshotへ置換する。folder pinのlookup identityは
通常ロードと共通化し、current folder / 現在のZIP本 / ZipDirのliteral-effective aliasを含める。
cache swap後のvisible/facet/details/selection再計算と代表サムネ再materializeは各contextをmount
している間に行う。video pinは現DBにWebPがない項目も再生成対象へ含め、削除を通常フレーム抽出へ
戻す。終端refreshはimport本体と別のcancel tokenを持ち、App終了時はDB chunk間で中断する。
page state import時は永続edit preview cacheの破棄完了を非同期ACKで待ち、main / active detached /
paused detachedのmaterialized previewを失効させる。各contextではimport前後のどちらかに編集が
あるthumbnailも再要求するため、編集削除後の旧WebPを残さない。
再帰exportでjunction / symlink directoryを追わない場合は、除外数と先頭pathを完了画面へ表示する。
項目SAVEPOINT失敗は「一部未反映」として先頭path・理由を表示し、全件は常時
`mimageviewer.log`へ記録する。import本体はmanifest / preflight / DB open / target verify /
SQL apply / commit（合計・最大）の時間、終端refreshはcontext / item数と時間を同ログへ記録する。
PDFの仮想項目はruntime keyと同じ`page_<u32>`だけを許可し、評価のkind / page numberもkeyと
一致させる。export元DBのJSON化されたマスク・隠蔽図形などを厳密にparseし、破損値は空の状態へ
変換せずストア名と項目keyを伴うエラーにする。
性能ログを有効にした場合は同じ境界を`logs/perf_events.jsonl`の`metadata_import`イベントにも残す。
進捗path欄は3行分を固定確保し、折り返しによってキャンセルbuttonの位置を動かさない。
実装と境界条件は `metadata_transfer.rs`、モーダルと writer drain は
`ui_dialogs/metadata_transfer.rs` が所有する。

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
| [music-integration-plan.md](music-integration-plan.md) | 音楽ビュー / 音声再生 / 動画→音声モードを触るとき (Inc 履歴含む正本) |
| [video-architecture.md](video-architecture.md) | 動画 HUD・native presenter・動画→音声モードを触るとき |
| [detached-rework-plan.md](detached-rework-plan.md) | detached viewer / F12 / 複数ウィンドウを触るとき。**§2 憲法 (BA-1〜7) 必読、凍結ルールあり** |
| [spec.md](spec.md) | 機能仕様・設定項目の正式な定義 |
| [catalog-design.md](catalog-design.md) | サムネイルキャッシュ DB の詳細設計 |
| [archive/performance-refactoring/thumbnail-memory-redesign.md](archive/performance-refactoring/thumbnail-memory-redesign.md) | サムネイルメモリ管理の背景経緯 |

---

## 7. 修正時のチェックリスト

1. **触る機能の doc を必ず先に読む** (上の表から該当ページを選ぶ)
2. **通常画像 / ZIPImage / PdfPage の 3 分岐を忘れない** — ZIP/PDF 対応漏れは頻出バグ
3. **サムネイル経路とフルスクリーン経路の両方で整合性を保つ** — 片方だけ修正すると表示が食い違う
4. **テクスチャキャッシュの無効化タイミング** — 補正・AI・回転を変更したら正しいキャッシュをクリアしているか確認 (`preset-and-adjustment.md`)
5. **ドキュメント同時更新** — CLAUDE.md の「コード修正時のドキュメント同時更新」セクションに従う
