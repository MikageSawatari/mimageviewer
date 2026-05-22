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

## 仕様・機能

| ドキュメント | 内容 |
| --- | --- |
| [spec.md](spec.md) | アプリ全体の仕様書 (設定項目・機能一覧) |
| [feature-expansion-ideas.md](feature-expansion-ideas.md) | v0.10 候補 3 機能 (キャプチャ保存 / 比較ビュー / 動画連続再生) + スクロールバー視認性改善 の詳細仕様。Codex 第 2 ラウンドレビュー反映済、実装状況と設計メモを併記 |
| [file-drag-drop-design.md](file-drag-drop-design.md) | グリッドからエクスプローラ等へファイルをドラッグ送出 (コピー) する機能の実装設計＋実装メモ。シェル `IDataObject` + `SHDoDragDrop` 方式。実装済み (2026-05、`src/file_drag.rs`)、残るは §8.2 の実機検証 |

## 設計メモ (特定領域の詳細)

| ドキュメント | 内容 |
| --- | --- |
| [catalog-design.md](catalog-design.md) | サムネイルキャッシュ DB の設計 |
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
| [tag-feature.md](tag-feature.md) | ハッシュタグ型タグ機能 (`dc:subject` に `#タグ` 書き込み + Ctrl+G 連携) の設計ドキュメント |
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
