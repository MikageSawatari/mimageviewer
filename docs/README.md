# docs/ — ドキュメント索引

修正作業の前に、関連するドキュメントを読んで全体設計を把握すること。

## 設計ドキュメント (これから作業する前に)

**迷ったらまず [architecture-overview.md](architecture-overview.md) から。**

| ドキュメント | 読むべきタイミング |
| --- | --- |
| [architecture-overview.md](architecture-overview.md) | 全体像の把握。レイヤー構造・モジュールマップ・永続化ストア一覧 |
| [display-pipeline.md](display-pipeline.md) | サムネイル表示・フルスクリーン描画を触るとき。**補正/AI/回転の適用順の決定版** |
| [async-architecture.md](async-architecture.md) | 並列処理・キャンセル・キャッシュ競合を触るとき。ワーカー一覧とテンプレ |
| [virtual-folders.md](virtual-folders.md) | ZIP/PDF 関連を触るとき。**通常画像との分岐チェックリスト** |
| [preset-and-adjustment.md](preset-and-adjustment.md) | 補正・プリセット・AI キャッシュを触るとき。**無効化ルールの早見表** |

## 仕様・機能

| ドキュメント | 内容 |
| --- | --- |
| [spec.md](spec.md) | アプリ全体の仕様書 (設定項目・機能一覧) |

## 設計メモ (特定領域の詳細)

| ドキュメント | 内容 |
| --- | --- |
| [catalog-design.md](catalog-design.md) | サムネイルキャッシュ DB の設計 |
| [thumbnail-memory-redesign.md](thumbnail-memory-redesign.md) | サムネイルメモリ管理の再設計経緯 |
| [dpi-multimonitor-issue.md](dpi-multimonitor-issue.md) | マルチモニター DPI 問題の調査記録 |
| [pdf-issues.md](pdf-issues.md) | PDF サポートの既知問題 |
| [screenshot-howto.md](screenshot-howto.md) | 製品ページ用スクリーンショット手順 |
| [e2e-smoke-test.md](e2e-smoke-test.md) | E2E スモークテストのチェックリスト |
| [ui-snapshot-policy.md](ui-snapshot-policy.md) | egui_kittest によるスナップショットテストの運用方針 |
| [bench-scroll-report.md](bench-scroll-report.md) | スクロール性能ベンチマーク結果 |
| [perf-investigation-handoff.md](perf-investigation-handoff.md) | パフォーマンス調査の進行中メモ (AI アップスケール優先度 / スクロール中の重複エンキュー抑制) |

---

## ドキュメント更新ルール

コード修正時は以下も同時に更新する (CLAUDE.md の指示に従う):

- 機能追加・変更・削除 → `spec.md` と `htdocs/mimageviewer/` を更新
- 設計レベルの変更 (キャッシュ構造・ワーカー構成・新しい永続ストレージなど)
  → 該当する設計ドキュメント (上記の「設計ドキュメント」セクション) を更新

**設計を変えたのに設計ドキュメントを放置しない**。このドキュメントが腐ると、
将来の自分 (または AI) が同じ罠を踏む。
