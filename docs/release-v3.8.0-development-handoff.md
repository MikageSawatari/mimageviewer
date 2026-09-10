# v3.8.0 開発側から公開担当への引き継ぎ

更新: 2026-09-10。公開準備・署名・配布・公開は既定どおりClaudeCode Opusが担当する。
本書は開発の検証状態を渡す記録であり、公開完了や未実施ゲートの免除を示さない。

## 統合対象と現状

- 利用者は表紙補助機能の実機確認で問題なしと報告し、masterへの統合とリリース準備を明示依頼した。
- master統合commit: `e394e5c6d`。feature先端 `e5d754e3f`、検証済み製品source `bc8aa8c21`。
  feature側の後続commitは検証記録のみ。独立Solによるmasterとの意味衝突レビューは重要指摘なし。
- 統合は競合なし。マージ前から存在した別所有のtracked dirty 9 pathはファイルhash不変。
  AGENTS/CLAUDE/README・公開運用文書・sitemap・Idle198のui-smoke等を勝手に取り込んでいない。
- 表紙あり見開きで先頭と末尾が単ページの場合、末尾へ表紙を追加表示する。全体既定ON、
  本ごとに全体に従う/ON/OFF。連結・F12・Remoteを含み、ページ数・シーク・読書位置は維持。
  詳細と実機記録は [設計・検証記録](final-cover-spread-plan.md)。
- 先に統合済みの類似画像・本検索、および動画プレビュー、ページ送り停止、余白色、
  マウスシーク、Vキー、動画ストリップ描画、類似パネルの旧画像保持の修正も対象。
  経緯・検証条件は [開発台帳](next-version-development.md) を参照する。

READMEの未コミット更新履歴案はまだv3.7.1表記であり、v3.8.0の確定版ではない。
公開担当は既存案を保持しつつ類似検索・表紙補助を含む最終案に整理し、Phase 0の利用者確認を行う。
実際の公開日に合わせて日付を確定する。版番号・マニュアル・Release bodyの変更は公開担当に残す。

## 検証の再利用と統合後の確認

feature全gate: main8074成功/43ignored、UI48、IPC55、remote-web118/1ignored、vendor25/9/15成功。
独立レビューと利用者の本体/Remote/連結/F12/ずらし確認は完了。初回失敗と根因修正の記録も保持。
証拠: `C:/home/mimageviewer-dupe/target/final-cover-spread-full-gate-r2-20260910/MANIFEST.txt`。

master統合後の検証は完了。全gateは9,032成功・0失敗・49ignored、test-script featureのcore check、
fmt、glyph（検出0）も成功。結果は `target/next-version-work/merge-final-cover-20260910/manifest.json`
（SHA256 `58E8B22BCE46193DE39CA99162DFB37AA929584AE48EA23A553CF18914134BCD`）。
検証対象はe394e5c6d、検証中にHEADがf8f675144へ進んだが差分は文書のみでbuild入力は不変。
通常確認buildも9/10 19:52 JSTに成功し、成果物は未起動。master出力resident0を確認してから実行し、
既存アプリの停止は行っていない。core SHA256 `E7835E14A52D9E2387D856F3F5B37A25570A77784BB511E40ED9445ED351AC4A`、
Remote SHA256 `55D69A88C782ECB230230ED04F9B18F09921B102488DB15E04EF9FE9D5E45364` を親が照合済み。
成果物は `C:/home/mimageviewer/target/dev-runtime/`。利用者が確認する場合は既存・トレイ常駐mIVを終了し、
repo rootで `Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe`。
通常APPDATA profileを使い実設定/dataを更新し得る。エージェントは起動しない。
製品source変更がなければ既完了の独立レビュー・狭域テスト・利用者確認を全面再実施しない。

## 実機自動テストの状態

| 対象 | 状態・範囲 |
| --- | --- |
| 複数窓PDF | 9/10 10:16の隔離実アプリ検証成功 |
| 静止画ストリップdrag | 隔離実アプリで成功。egui合成pointerであり実OS dragではない |
| 動画MouseMove | 実OS入力の配送・処理確認に成功 |
| 上HUD hover | 実OS moveと実Response観測に成功。クリック成功とは別 |
| 上HUD click→100% zoom | 実装・独立レビュー・非対話検証・隔離build完了、実機未実施 |
| zoom wheel/pan/負の入力条件・thumbnail pixel | 自動化の残作業。完了扱いにしない |
| Idle198収束 | 索引作成負荷と分離できず未実施 |

成功4件の証拠は `target/next-version-work/logs/ui-smoke-default-desktop-suite-20260910.json`。
これはpre-merge sourceの限定シナリオ証拠で、統合版全機能の実機保証ではない。
旧portableのsource fingerprintは `8e9e229255695cd6547a127c89f686c698547a8d44e91ac7942204d1150d0854`。
統合sourceの追加実機を行う場合は再prepareする。9/10 18:30の操作枠は終了し、clickへの具体了承も未取得。
新しい実機操作は具体内容・時間・使い捨てdataを提示して了承後に行う。

利用者との最新合意: 今回は追加の実機自動テストを見送り、既実施結果と表紙補助の手動確認を再利用する。
未実施項目をPASSへ変更しない。ClaudeCodeの公開作業中にidle-healthとperf smokeは従来どおり実施し、
PC操作の時間枠は別途確認する。この見送りは性能計測や配布工程の免除ではない。

## 公開前に残るもの

- 開発側の統合後ゲートと確認buildは完了。配布用buildは公開担当の工程に残る。
- CLAUDE.md Phase 0以降の最終更新履歴確認、v3.8.0メタデータ、配布build・署名・配布物確認・公開。
- §9.6 perf smoke / §9.7 idle-healthは今回未完了。自動化開発の残作業とは分けて扱う。
  v3.7.0限定の省略了承は今回へ流用しない。通常profileを起動する旧スクリプトを
  エージェントがそのまま実行してよいという意味ではなく、既存の実データ保護要件を維持する。
- 未コミットの他所有変更は公開担当と範囲を確認し、まとめてstageしない。
