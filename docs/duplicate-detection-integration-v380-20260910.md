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

## 保存と競合解消

dupeを5c864176e（既存R2 index exact）、f83161f5d（検証済みproduct/source/vendor）、5eb5e782a（文書）へ保存しclean確認。master b003e6494取込みは5file/14block競合。app/native_video/displayed_image_transform/ui_fullscreen/detached計画について双方の仕様を統合した。masterのstable anchor・canonical pages・mouse hold・native入力・余白色と、dupeのtyped chrome/purpose/password/history/trace・per-context比較ownerを保持。ImagePaintQuadはdupe geometryへ統合。

独立レビューのP2は移動診断のidentity/終端境界。masterのDisplay identity（generation/anchor）へ判定を統一し、cancel owner境界でnavigation_supersededをtakeしてからreleaseする。通常release理由と一回性を維持。同anchor/FolderItemsは保持、別anchor/stale generationは終了。統合5callerを一巡した。

製品check成功。master側テストのtyped fixture不足を修正。similar focusedは初回32成功/1失敗だったが、失敗はmaterialized-failure fixtureのfullscreen_idx未設定で、masterの実表示anchor契約を満たしていなかった。Some(anchor)で実表示失敗を再現し、製品のholdover/close条件を変更せず単独成功、独立reviewも妥当と判断。名前にgreenを含む初回失敗logは成功証拠と扱わない。

最終focused: similar33、topology10、target10、mouse hold3、raw XButton5、paint quad2、detached backstop2、underlay5、seek margin1、video display mode4成功。ログはtarget/duplicate-master-integration-20260910/logs。source固定、独立最終承認と統合全体gateは次の区切り。

最終追補: master由来のset_items_generationによるDisplay invalidationもSuperseded共通終端へ統一。同generation保持・新generationでtrace一回を既存remove-items回帰へ追加し1/1、navigation_target全10件再検証成功。master追加のowner破棄/直接releaseはcommon cancelとgeneration setterの2箇所で、追加phase rebindはowner/purpose保持で終端ではないことを独立reviewが一巡確認。同種追加P1/P2なし。初回型名修飾不足によるtest compile failureは修正済みで製品test failureとは区別する。

## 初回統合gateと回帰fixture修正

初回test-fullはexit101、本体8028 passed / 1 failed / 43 ignored。唯一similar_book_visit_commits_only_after_its_exact_spread_destination_is_liveが失敗。ui_fullscreen側でsimilar_navigation_tests filter外であり、focused33成功から並列依存とは判断しない。fixtureはfullscreen_idx=1とDisplay anchor=2が不一致。実Similar routeはdestination target_idxをanchorにするためSome(2)が正規と親・実装・独立Solが照合。fullscreen_idxだけを修正し、製品observer/all-Live/exact destination/履歴確定条件は不変。単独1/1、similar-book2/2、ordinary-history1/1、fmt成功。追加review所見なし。初回gateは失敗記録として保持し、別ログで最終gateを再実行する。

## 統合最終gate成功

最終test-fullはexit0 / PASS、本体8029 passed / 0 failed / 43 ignored、UI snapshot48、vendor egui25/egui-wgpu9/eframe15成功。実native出力はtarget/duplicate-master-integration-20260910/logs/test-full-integration-final.log。初回失敗ログとは分離。既存成功計測の無条件再実行は行わず、統合fixture修正後に必要な全体gateを実行した。ここから固定統合結果を保存し、master残dirtyを保持して反映、正確な対象processを確認して通常build-devへ進む。
