# docs/ — ドキュメント索引

修正作業の前に、関連するドキュメントを読んで全体設計を把握すること。

## 設計ドキュメント (これから作業する前に)

**迷ったらまず [architecture-overview.md](architecture-overview.md) から。**

| ドキュメント | 読むべきタイミング |
| --- | --- |
| [architecture-overview.md](architecture-overview.md) | 全体像の把握。レイヤー構造・モジュールマップ・永続化ストア一覧 |
| [display-pipeline.md](display-pipeline.md) | サムネイル表示・フルスクリーン描画を触るとき。**補正/AI/回転の適用順の決定版** |
| [async-architecture.md](async-architecture.md) | 並列処理・キャンセル・キャッシュ競合を触るとき。ワーカー一覧とテンプレ |
| [ui-responsiveness.md](ui-responsiveness.md) | UI スレッド同期 I/O で UI を止めないための設計方針。**新機能追加前にチェックリスト §4 を必ず見る** |
| [virtual-folders.md](virtual-folders.md) | ZIP/PDF 関連を触るとき。**通常画像との分岐チェックリスト** |
| [preset-and-adjustment.md](preset-and-adjustment.md) | 補正・プリセット・AI キャッシュを触るとき。**無効化ルールの早見表** |
| [search-architecture.md](search-architecture.md) | 検索 / インデクサ / タグを触るとき。**Ctrl+S/F/G の経路 + インデクサパイプライン + DB 責任分離** |
| [fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md) | フルスクリーン / 検索結果 / 動画タイルをまたぐ Ctrl+↑↓・境界ヒント・前後移動の統一仕様メモ |
| [keymap-spec.md](keymap-spec.md) + [key-customization-impl-plan.md](key-customization-impl-plan.md) | キーボード操作 / ショートカット / `consume_key` / `key_pressed` / native VK 判定を触るとき。新しいキー操作は keymap 対応要否を必ず確認 |

## 仕様・機能

| ドキュメント | 内容 |
| --- | --- |
| [spec.md](spec.md) | アプリ全体の仕様書 (設定項目・機能一覧) |
| [feature-expansion-ideas.md](feature-expansion-ideas.md) | v0.10 候補 3 機能 (キャプチャ保存 / 比較ビュー / 動画連続再生) + スクロールバー視認性改善 の詳細仕様。Codex 第 2 ラウンドレビュー反映済、実装状況と設計メモを併記 |
| [next-release-backlog.md](next-release-backlog.md) | **次リリース検討バックログ**。未対応の P2/P3・要判断項目、ユーザー要望、依存ライブラリ更新、リリース前確認だけを恒久管理。完了した項目はこのファイルから削除する |
| [detached-viewer-implementation-plan.md](detached-viewer-implementation-plan.md) | 画像・動画を共通の別ウィンドウビューアとして扱う設計・実装メモ。F12 別ウィンドウモード、別ウィンドウ中 F11 無効、×/Esc/Enter/右クリックで session close、メイン一覧カーソルとの双方向同期、動画 native presenter の `NativeVideoPlacement::DetachedViewerChild` 化、close-to-tray 時の再生継続、ClaudeCode レビュー反映メモを整理 |
| [details-view-and-filter-plan.md](details-view-and-filter-plan.md) | **Ph1〜Ph4 + Ph5 画像/動画/作成日時遅延列まで実装済み**。ファイル選択画面の詳細表示モード (サムネ無しで名前/サイズ/日付＋★/タグ/編集フラグを行表示) ＋ Excel オートフィルタ風スマートフィルタの設計。現状は列セクションの詳細切替、右クリック列表示メニュー、`details_order` による列ヘッダ 3 トグルソート、種類/拡張子/★/タグ/日付/サイズ/状態の共通 `FacetFilter`、遅延列 worker / 進捗表示、作成日時列、画像解像度列、動画長さ/解像度/コーデック列まで実装済み。EXIF/PDF/アーカイブ系の追加遅延列は後続 |
| [shell-file-operations-context-menu-plan.md](shell-file-operations-context-menu-plan.md) | **一部実装済み**。Windows Shell の `IFileOperation` とネイティブ右クリックメニューへ寄せるファイル整理機能の実装計画。A/B クイックフォルダと実ファイル/実フォルダの Shell 標準右クリックメニューは実装済み。標準の上書き確認・進捗 UI、仮想 ZIP/PDF アイテム向けの native custom menu は後続 |
| [key-customization-plan.md](key-customization-plan.md) | **設計履歴**。キー操作カスタマイズの調査・設計。現状の 3 入力経路 (egui consume / key_pressed / Win32 VK)・hold ジェスチャ・コンテキスト分割を調査し、フル版と簡易版 (テキスト ini / GUI なし / 競合検知なし) を比較。§8 が簡易版の確定設計 (入力パターン分類・複数チョード) |
| [key-customization-impl-plan.md](key-customization-impl-plan.md) | **実装済みメモ**。簡易版 (テキスト ini / GUI なし / 競合検知なし) の手順書と実装判断。`src/keymap.rs` の型・コメントアウト済み `keymap.ini` / `keymap.ini.default` 生成・ini 仕様 (`Action.1` 形式)・exact match ヘルパー・native 動画転送対応・エッジケース規則・`KeyAction` インベントリ (付録 A)・キー変換ホワイトリスト (付録 B) |
| [file-drag-drop-design.md](file-drag-drop-design.md) | グリッドからエクスプローラ等へファイルをドラッグ送出 (コピー) する機能の実装設計＋実装メモ。シェル `IDataObject` + `SHDoDragDrop` 方式。実装済み (2026-05、`src/file_drag.rs`)、残るは §8.2 の実機検証 |
| [auto-thumb-aspect-plan.md](auto-thumb-aspect-plan.md) | サムネイル比率の自動選択 (`thumb_aspect_auto`) の設計と実装計画。`log(ratio)` の中央値 → 最近接バケット方式 + 6 段ゲート (min_samples / 連勝継続 / cooldown / 切替上限 / 入力 idle / log 距離マージン)。実装済み (2026-05、`src/auto_aspect.rs`) |
| [local-adjustment-layer-v1.1.0-plan.md](local-adjustment-layer-v1.1.0-plan.md) | **Codex 案**。v1.1.0 候補の画像補正ピボット計画。全体補正の強化、手描き/グラデーション/範囲/セグメンテーション生成マスク、マスク反転付きの部分補正レイヤーを、消しゴム後・隠蔽加工前の非破壊レイヤーとして追加する設計 |
| [local-adjust-filter-candidates.md](local-adjust-filter-candidates.md) | 補正レイヤーへ追加していくフィルタ候補リスト。イラスト用途を主眼に、効果選択 UI 方針、優先度、実装難易度、詳細設計を整理 |
| [speech-bubble-tool-design.md](speech-bubble-tool-design.md) | **Codex 案**。漫画 / AI イラスト投稿向けの吹き出し・セリフ入れツール設計。補正レイヤーとは分け、隠蔽加工後・crop 前に載せる前提で、テキスト、尾、縦書き、IME、保存、書き出しを整理 |
| [speech-bubble-text-tool-plan.md](speech-bubble-text-tool-plan.md) | **Claude 案** (上記 Codex 案と対。独立検証で結論一致)。同じ吹き出し・テキスト注釈ツールを実コードの型 / 関数 (`resolve_fs_processed_texture` / `ensure_conceal_texture` / `export_page_pixels_for_idx` / `page_path_key` / `conceal_db`・`local_adjust_db` パターン) に接続して設計。レンダリング基盤 (cosmic-text + 縦書き自前レイアウト + 共有レイアウトエンジン)、キャッシュ無効化表、機能リサーチ + 競合比較、フェーズ分けが厚い。縦中横の詳細は Codex 案を正とする |
| [editing-add-on-download-spec.md](editing-add-on-download-spec.md) | miV 本体マージ後に実装する編集用追加パックの仕様。オノマトペ向け OFL フォント、被写体分離モデル、初回利用時のダウンロードモーダル、保存先、manifest、ライセンス表示、TensorRT pack との分離方針を整理 |
| [portable-build-plan.md](portable-build-plan.md) | **v1.1.0 候補 (設計のみ)**。loose-deps ポータブル版 zip の設計。`portable` cargo feature で native 依存 (pdfium/onnx/susie/vst3/ffmpeg/models) を include_bytes せず exe 隣から解決し、実行時展開ゼロ・launcher 不要にする。C ドライブ圧迫と AV 誤検知の同時解消が狙い。集約モジュール `native_assets` + data_dir 検出 + mutex 名分離 + パッケージング + メンテ保証 (CI guard) を整理 |
| [comic-lab-validation-checklist.md](comic-lab-validation-checklist.md) | `tools/comic_lab` / `crates/comic-core` の実機検証チェックリスト。縦書き約物、IME、フォント、しっぽ、装飾、メッセージウィンドウ、本体統合時の P0 を整理 |
| [ai-suggested-mask-v1.1.0-plan.md](ai-suggested-mask-v1.1.0-plan.md) | **Codex 案**。v1.1.0 候補の AI 提案マスク設計案。標準の顔検出 + ユーザー指定 ONNX 検出モデルを、消しゴム / 隠蔽加工のマスクオブジェクト生成に接続する。バッチ生成を v1.1.0 に含め、ShapeMeta / モデル登録 UI を提案 |
| [auto-mask-detection-plan.md](auto-mask-detection-plan.md) | **Claude 案** (上記 Codex 案と対). 同じ v1.1.0 自動マスク機能を実コードの型/関数 (`Shape`@mask_db.rs / `commit_conceal_shape`@ui_conceal.rs / `runtime.rs` / `ai_upscale` worker) に接続して設計。標準=MIT 同梱 YuNet、追加=BYO (`DetectorProfile`+`OutputFormat`、deepghs サイドカー自動読取)。v1.1.0 は現ページ対象・一括は将来フェーズ。検証済みライセンス表付き |
| [ai-metadata-parser-expansion-plan.md](ai-metadata-parser-expansion-plan.md) | **v1.3.0 実装済み**。AI 生成メタデータパーサの形式拡充 (NovelAI / InvokeAI / SwarmUI / Fooocus 系 / JPEG EXIF UserComment)。NovelAI 誤判別 + Negative prompt 索引混入を consumed_keys 方式で修正し、JSON 生成メタデータを汎用 JSON より優先解釈。INDEX_VERSION=9 で再構築 |
| [nested-zip-tree-plan.md](nested-zip-tree-plan.md) | **v1.3.0 候補 (設計確定/実装着手前)**。ネスト ZIP を現在のフラット展開からツリーナビへ変える設計。`entry_name` を不変に保ち表示層のみ追加 (DB 移行ゼロ)、Ctrl+G ドリルダウンを流用して内側 ZIP/サブフォルダを階層移動。items が現在の本だけになるので見開きペアリングが本ごとにリセットされ相性問題を解消。Claude/Codex 独立合意の設計を固定 |
| [final-smart-sharpen-plan.md](final-smart-sharpen-plan.md) | **v1.3.0 実装済み (設計経緯メモ)**。画像補正パネルに 1 本スライダーの最終段スマートシャープを追加する計画。AI モデルではなく既存 `SmartSharpen` 系の計算式を final pipeline に入れ、サムネイル非反映、CPU 並列化、post_filter との併用を前提に整理。実装の正本は [preset-and-adjustment.md §2.6](preset-and-adjustment.md) |

## 設計メモ (特定領域の詳細)

| ドキュメント | 内容 |
| --- | --- |
| [catalog-design.md](catalog-design.md) | サムネイルキャッシュ DB の設計 |
| [ai-region-segmentation-retrospective.md](ai-region-segmentation-retrospective.md) | `local_adjust_lab` で試した SAM / SAM2 領域分割の失敗メモ。v1.1.0 では AI 領域分割を見送り、クラシック領域分割へ集中する判断の背景 |
| [thumbnail-memory-redesign.md](thumbnail-memory-redesign.md) | サムネイルメモリ管理の再設計経緯 |
| [dpi-multimonitor-issue.md](dpi-multimonitor-issue.md) | マルチモニター DPI 問題の調査記録 |
| [pdf-issues.md](pdf-issues.md) | PDF サポートの既知問題 |
| [screenshot-howto.md](screenshot-howto.md) | 製品ページ用スクリーンショット手順 |
| [e2e-smoke-test.md](e2e-smoke-test.md) | E2E スモークテストのチェックリスト |
| [test-video-generation.md](test-video-generation.md) | `testimage/movie/test_*fps_*p_sync.mp4` (FFmpeg testsrc2 + sine ビープ) の再生成手順 |
| [ui-snapshot-policy.md](ui-snapshot-policy.md) | egui_kittest によるスナップショットテストの運用方針 |
| [keymap-spec.md](keymap-spec.md) | キー / マウス操作仕様。フルスクリーン横断の詳細は [fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md) も参照 |
| [bench-scroll-report.md](bench-scroll-report.md) | スクロール性能ベンチマーク結果 |
| [perf-investigation-handoff.md](perf-investigation-handoff.md) | パフォーマンス調査の進行中メモ (AI アップスケール優先度 / スクロール中の重複エンキュー抑制) |
| [plan-v0.7.0.md](plan-v0.7.0.md) | v0.7.0 実装計画 + 完了ステータス + リリース直前チェックリスト |
| [search-expansion-design.md](search-expansion-design.md) | 検索システムの仕様選択理由と背景資料 (Tantivy スキーマ・ZIP ingest 負荷制御・UI drill-down・streaming プロトコル)。**v5 までの旧設計 (二段整合性 / SQLite 内 norms) を含む** — 現行 v6 設計は [search-architecture.md](search-architecture.md) §4.2 を参照 |
| [search-bench-results.md](search-bench-results.md) | Tantivy + bigram プロトタイプ計測結果 (50 万件規模まで) |
| [search-test-plan.md](search-test-plan.md) | 検索・notify-rs 監視・キー操作の自動テスト整備計画 |
| [search-container-item-redesign.md](search-container-item-redesign.md) | 検索を「コンテナ検索 (Ctrl+S) / アイテム検索 (Ctrl+G)」モデルへ整理する再設計案。Ctrl+G 一覧/集約ビュー・動画索引除外・mtime 追加・Ctrl+F の構造アイテム絞り込み |
| [tag-feature.md](tag-feature.md) / [tag-catalog-redesign-plan.md](tag-catalog-redesign-plan.md) | mIV タグ機能。現行は `tags.db` 正本 + facet 絞り込み、旧 `dc:subject` タグは移行対象 |
| [video-architecture.md](video-architecture.md) | 動画インライン再生サブシステムの設計指針と内部構造 (D3D11VA HW デコード + DX12 zero-copy interop + CPU fallback)。**Phase 2 (DComp / NVIDIA VSR) 撤回の経緯も巻末に記載** |
| [playback-speed-design.md](playback-speed-design.md) | 動画倍速再生機能の仕様。Signalsmith Stretch 採用、AvClock 中心の速度配線、音声 PTS/PDC/queue 秒数の扱い、HUD UI、検証計画 |
| [dcomp-native-presenter-integration-plan.md](dcomp-native-presenter-integration-plan.md) | DirectComposition native video presenter prototype を本番 fullscreen path へ統合するための段階計画。1080p120/165Hz 対応、egui overlay 分離、DPI/resize/VST owner 課題を整理 |
| [dcomp-overlay-egui-technical-brief.md](dcomp-overlay-egui-technical-brief.md) | DirectComposition native presenter Phase C overlay で egui-wgpu をどう載せるかの技術選択メモ |
| [codex-native-overlay-redraw-cadence-brief.md](codex-native-overlay-redraw-cadence-brief.md) | Phase C native egui overlay の redraw cadence / render_ms を soak で確認するための計測ブリーフ |
| [codex-native-presenter-copy-spike-brief.md](codex-native-presenter-copy-spike-brief.md) | Production native DComp presenter の `copy_ms` / `fence_wait_ms` spike を per-present trace soak で切り分ける計測ブリーフ |
| [ffmpeg-lgpl-source-distribution.md](ffmpeg-lgpl-source-distribution.md) | FFmpeg LGPLv3-or-later build の配布時チェックリスト、対応ソース、同梱外部ライブラリの確認メモ |
| [codex-video-upscale-resumable-segments-design.md](codex-video-upscale-resumable-segments-design.md) | Offline video upscale の resumable segment / persistent queue 設計 |
| [codex-video-upscale-resumable-segments-phasec-implementation-review.md](codex-video-upscale-resumable-segments-phasec-implementation-review.md) | Offline video upscale Phase C/D/E 実装レビュー依頼メモ |
| [video-engine-redesign.md](video-engine-redesign.md) | エンジン側 (`AvClock` / `EngineActor` / `MasterClock` / `AudioBookkeeping`) のリデザイン経緯と各 Phase 詳細。Phase 8.K の pacing 仕様、Phase 9 の 3-thread 分離、Phase 9.A〜9.G の追加修正 (wall-rate cap / cpal warmup silence / forward seek backward+preroll / perf overlay seek freeze 等) を網羅 |
| [audio-normalize-scan-bench.md](audio-normalize-scan-bench.md) | 音量ノーマライズ初回スキャン待ち時間の実測用 CLI (`normalize_scan_bench`) と、HDD 上の動画で逐次 / 並列スキャンを比較するときの読み方 |
| [vst3-integration.md](vst3-integration.md) | VST3 プラグイン統合 (v0.9.0+) — C++ bridge プロセス + Rust IPC、audio-pump からの bridge 経由、プラグイン GUI のクロスプロセス attach、チェーン編集 UI、再生中 VST3 パネル、後段 safety limiter |
| [settings-sqlite-migration.md](settings-sqlite-migration.md) | 設定永続化を `settings.json` から `settings.db` (SQLite) に移行する spec。transient NotFound による設定消失事故の構造的解消 + VST3 BLOB の dirty-skip による I/O 浪費解消。4 ラウンドの Codex review 反映済み |

---

## ドキュメント更新ルール

コード修正時は以下も同時に更新する (CLAUDE.md の指示に従う):

- 機能追加・変更・削除 → `spec.md` と `htdocs/mimageviewer/` を更新
- 設計レベルの変更 (キャッシュ構造・ワーカー構成・新しい永続ストレージなど)
  → 該当する設計ドキュメント (上記の「設計ドキュメント」セクション) を更新

**設計を変えたのに設計ドキュメントを放置しない**。このドキュメントが腐ると、
将来の自分 (または AI) が同じ罠を踏む。
