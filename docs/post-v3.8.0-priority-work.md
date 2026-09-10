# v3.8.0 公開後の優先修正

2026-09-11、利用者が公開完了を報告し、以下を優先して開発するよう依頼。
着手時masterは `922bf1a86`。他所有の未コミット変更は保持する。

## 範囲・担当

| 対象 | 実装・検証所有者 | 初期段階 |
| --- | --- | --- |
| §1.208 動画Fullscreen→音声で終了 | Sol / smoke_next_plan | 根因・各presentationのfocus所有境界を調査 |
| §1.209 初回sidecar importでUI47秒停止 | Sol / sidecar_import_fix | 同期I/O・DB transaction・編集競合を調査 |
| §1.210 右クリックメニュー再表示ループ | Sol / smoke_next_plan（§1.208後） | 作業中の追加backlogを確認、未実装 |
| 類似索引の全周回反復・照合性能・進捗 | dupe側タスク「引き継ぎ状況を整理」 | 独立branchでPhase 1実装・狭域検証中 |
| CI non-Windows cfg | Sol / smoke_next_plan | app.rsの先行小修正 |
| CI viewer context audit | 親が整理、別SolがA4 API独立検収 | 監査失敗と誤判定を区別 |

親は設計・進行管理、独立Sol / next_independent_reviewが重要設計と完成差分を検収する。
実装とレビューはSol/xhigh、max/ultraは使わない。旧detached文書のClaudeCode検収指定は
AGENTS.mdの開発担当移行規則に従い親と独立レビューへ対応付ける。
公開作業・版番号更新は本chunkに含めない。

app.rsは先行CI修正の間smoke_next_planのみ編集し、sidecar担当はread-only。
両修正の設計と必要ファイルが判明した時点で所有権を引き渡す。同一ファイルは同時編集しない。
dupe側は別worktreeのみ編集し、自動merge・類似機能再有効化はしない。
重いCargo/full gate/build/大規模benchmarkの開始時は担当間で資源を調整する。

## 守る条件と完了条件

- §1.208: presenterの可視状態とセッション生存・focus所有を混同しない。MainWindow /
  Fullscreen / detached・複数窓、VST有無のenter/exitを棚卸しし、正常なfocus離脱動作も維持する。
  新しい時間猶予・追加repaint・フォーカス強奪で症状を消さない。detached §2に構造合意後、§11へ記録する。
- §1.209: UIでsidecar読取・逐次DB書込を待たない。既存編集を上書きせず、部分失敗を成功登録しない。
  worker完了は対象/世代と照合する。フォルダ移動・取消・終了・エラー・別窓でも所有境界を維持する。
  transaction粒度と既存行非上書きは同時編集を含めて検証する。
- CI: 非WindowsとWindowsの両方で参照が整合すること。A4の公開APIは意味を検収し、
  ベースラインへの機械的追加で済ませない。A6のテスト判定修正で製品側の違反検出を弱めない。
- 狭域回帰と独立検収後、変更範囲に適した全体ゲート・確認buildを実施する。
  成功済み検証はsource/条件が有効なら再利用。未実施の実機検証は成功扱いにしない。
- 新しい実機起動・入力は具体的scope/時間/dataを提示して明示了承後のみ。
  通常APPDATAを使うagent起動、実設定の操作、稼働中アプリ停止は行わない。

## 着手時の証跡

§1.208/§1.209の公開時調査は `96c0c10a1` と next-release-backlog.md。
CI run `34500939632`（HEAD `922bf1a86`）の失敗ログを
`target/ci-34500939632-failed.log` に保存。cfg漏れはapp.rsの
native_video_mouse_seek_holds field/initializerとpoll_similar_preview_workers_in_all_contexts呼出。
A4はContextRef::similar_panel、App::poll_similar_preview_workers_in_all_contexts、
App::invalidate_final_cover_spread_display_in_parked_contextsの3 API。
A6は親modがcfg(test)のsimilar_navigation_tests / similar_preview_context_testsを
ファイル単位の監査が製品コードとして読む点を調査中。

§1.209の初期コード照合では各storeのget→autocommit setと、失敗時にもsync mtimeを登録する
経路が見つかった。worker化だけではget/set間のユーザー編集を上書きし得るため、
transaction内の条件付き挿入と成功記録の扱いまで設計対象とする。修正・検証は未完了。

## CIの設計判断

独立Solと親が以下に合意し、smoke_next_planへ実装・検証を委任した。

- field/initializerのnative_video型はWindows専用条件を対称にする。preview pollは
  非Windowsでもmounted single contextの完了・取消drainを維持する。Windowsの順序は変更しない。
- A4 `similar_panel` は同じcontextの確定park時の需要保持とAtRest資源計上に必要。
  raw bundleや可変参照を外へ出さず、bundleが生存期間を所有するので登録可能。
- A4 `poll_similar_preview_workers_in_all_contexts` はmounted＋AtRestだけをin-placeでpollし、
  swap/mountや別contextの取消をしない。consumerがapp内のみなので公開範囲を
  `pub(in crate::app)`へ狭めて登録する。
- A4 `invalidate_final_cover_spread_display_in_parked_contexts` はapp外のpreferencesから必要。
  全体設定変更時にFollowGlobalのAtRest表示派生物だけを退役し、明示ON/OFFを維持するため
  現在のcrate公開範囲で登録可能。
- A6は2子ファイルに実コンパイル境界であるinner cfgを明示し、auditがfile.attrsの
  cfg(test)含意も認識するようにする。通常製品fileでtest helperを呼ぶ違反は引き続き検出する。
  parent mod graphの新構築や52件の個別allowlist化は不要。

CI小chunkは6pathの凍結差分を独立Solが最終承認。Windows core check / fmt / diff check成功、
audit tests 34/34・audit run違反0、non-Windows shadow成功。shadowはWindows hostでの近似検証で、
正式Ubuntu CIの再実行成功とは区別する。GitHubへのpush/CI再実行は未実施。
証拠: `target/next-version-work/logs/ci-non-windows-audit-20260911/verification-summary.json`
（SHA256 `6C6718DE92605AB16D890E4899BD6B9F182FEF52B82238FFC199F797F3291EE6`）。
Windows製品挙動は変更せず、非Windowsのcompileと監査の修正なので、この小chunk単独の
Windows確認buildは省略し、続くP1製品修正の確認buildへまとめる。

作業中のHEAD `5e7c87092` は他担当が§1.210を追加した文書commit。
製品ソースは変わっていない。操作不能になるP1として同じ優先台帳に加え、§1.208の後に扱う。
メニューmodal loopのrelease取落しは未測定の仮説、levelから新しい押下を作る経路は
コード上の事実として区別する。新規pressなしの再武装を止め、短押し・長押し・ring/gesture・
edit modeと動画経路の正しい入力処理は維持する。
