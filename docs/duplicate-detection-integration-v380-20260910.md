# v3.8.0 dupe / master 統合記録（2026-09-10）

## 許可・担当・固定入力

利用者は master 側親タスクを通じ、dupe を local master へ統合し、その状態のユーザー用確認ビルドを作るところまでを明示依頼。最終ハンドオフ後に master 編集・Cargo の所有権を当タスクへ移管した。公開・version・changelog・配布は ClaudeCode 担当のまま。push、アプリ起動、実データ操作、夜間UI入力は対象外。

- master: b003e6494c8c1c91d80ed11bfaeef0a9ef4b5622（親 dd997b538）。次版製品変更61pathを保存済み。
- dupe: duplicate-detection / cfb849e361e321d0e52878d9ec0ed3ffece20743 と検証済み staged/unstaged/untracked。
- 親: 設計判断・文書・進行。実装/git/test/build: implement_resume（Sol xhigh）。独立レビュー: review_resume（別Sol xhigh）。max/ultra不使用。
- master最終証跡: target/next-version-work/logs/video-seek-strip-dark-verification-ledger-20260910.md。独立承認、gate lib7696/34ignored・UI45・vendor等成功、build-dev成功。動画両strip最終版の実機表示は未確認。
- dupe最終証跡: target/similar-move-embedded-pump-fix-20260909/manifest.json、SHA256 588e1edbc09a970bf90a30a99d1e62cc89689d550b428643ddd391e1b06c15a7。独立承認、gate7963/0/43ignored、portable成功。利用者は移動・右パネル固定良好と確認。

## 保持するmaster側残変更

.gitattributes、AGENTS.md、CLAUDE.md、README.md（Claude仮changelog）、docs/development-build-and-test.md、docs/release-operations.md、htdocs/sitemap.xml、scripts/ui-smoke.ps1、src/gpu_anime4k_generated.rs（content diff0の生成/改行状態）。untrackedはdocs/brief-dupe-step5-incremental-array.md、docs/review-v3.5.0/、scripts/analyze_idle198_convergence.py、gen-sitemap-xml.py、test-ui-smoke-idle198.ps1、test_analyze_idle198_convergence.py、ui-smoke/generate_idle198_fixture.py、idle198-convergence.rhai。全addや削除を行わず、衝突時は先に保全して意味を統合する。

## 統合判断と検証境界

既存完了レビューを一からやり直さず、統合で変わるfullscreen描画/入力、viewerとthumbnail所有、画像変換/GPU、動画lifecycle、vendor egui ordered input/capture APIを重点確認する。§9.2横断一覧は保留。音声途切れの原因解消、元page-stallの完全解消、AHK互換、最終動画strip実機表示は過大に完了としない。

変更を保存→固定master取り込み→競合解決→別Sol独立レビュー→統合gate→local master反映→通常build-devの順。build-devには対象exe自動停止があるため実行前に正確なpathのprocessがないことを確認し、稼働中なら停止しない。通常profileの実行は利用者のみ。master編集権は確認build完了後に親タスクへ返却する。
