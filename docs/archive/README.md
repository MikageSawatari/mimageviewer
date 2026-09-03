# 完了した作業の記録

ここには、実装・検証・レビューが完了した計画書、brief、findings、リリース記録を
領域別に保存しています。現行仕様の正本ではありませんが、設計判断や検収の経緯を
後から確認できるように残しています。現行の正本と進行中の計画は
[`docs/README.md`](../README.md) を参照してください。

## 領域一覧

- `ai/` — AI メタデータ、マスク、TensorRT 検証
- `comic/` — 漫画・注釈ラボと UI 検収
- `detached/` — 別ウィンドウ viewer の段階実装、調査、findings
- `editing/` — 消しゴム、部分補正、隠蔽、最終シャープ
- `folders-archives/` — フォルダ表示、ZIP/PDF、製本ナビゲーション
- `performance-refactoring/` — 性能調査、応答性監査、リファクタリング
- `release/` — 過去版の計画、リリース本文、安定性レビュー
- `review-v2.3.0/`、`review-v2.7.0/`、`review-v3.5.0/` — 版ごとのレビュー資料一式
  (v3.5.0 は出荷前の 8 巡ぶん。追跡するのは Markdown だけなので、findings が参照する
  probe スクリプトとログはリポジトリに入っていない)
- `search-metadata/` — 検索プロトタイプ、タグ、sidecar メタデータ
- `ui-input/` — UI 改修、キー入力移行、操作設計
- `video/` — 動画・音楽再生、native presenter、offline upscale
- `vst3/` — VST3 bridge、障害調査、レビュー記録