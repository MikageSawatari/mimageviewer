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

## 利用者の追加指定・実機枠

利用者は§1.209・§1.208・§1.210を次版で修正することを再指定し、離席中の
実機テストを明示許可した（「このあと、あす9時くらいまでは離席」）。
親は安全側に9/11 09:00 JSTを締切として伝え、3件を中心に合計60〜90分程度、
使い捨てportable/合成fixtureのみ、前面ウィンドウ・mouse/keyを使う検証へ限定した。
アプリの通常APPDATAや既存データは操作せず、検証窓外は非対話作業だけを続ける。
許可は実施結果ではない。修正・非対話gate・portable準備後に各scenarioを記録して実行する。

他セッションのバックログ追加§1.211〜§1.214は利用者のコミット依頼に従い
`6440856fe`で文書1ファイルだけ記録済み。§1.212の見送り、§1.211の再現待ちを維持し、
今回の3件の修正を差し置いて未確認の仕様変更を実装しない。

## 9/11 未明の実装区切り

§1.208は音声viewportへの入力所有権移行をtyped stateへまとめ、既存の一度限りの
Visible/Focus要求に対する `Some(true)` の確認後にpresenterを隠す変更を検証中。
LiveMediaのparkでcontext IDが変わる箇所、snapshotのindex再対応、取消・動画復帰も
同じ所有権へ追従させる。独立検収・実機確認はまだ完了していない。

§1.209はUI未配線のbatch import engineを先行実装した。合成430 fieldの初回計測は
約21 msだが、これは製品のUI停止解消を示す結果ではない。Appへの非同期配線には、
既存の編集・タグ・copy/move・rename workerとのDB書込競合を解消し、利用者の編集意図を
失わない調整が必要と判明した。UIのbusy timeout延長や、補正前画像を先に表示する変更は
採らない。Stage 1を限定検収し、Stage 2の影響範囲を設計書に整理してから組み込む。
§1.210は§1.208と同じファイルを使うため、その検証区切り後に実装する。

Stage 1は独立検収とlib 17件・integration 14件を通過し、`5db8df1ee`でmasterへ記録した。
最終430 field fixtureはloadを含め36.617 ms、edit family 1 transaction/1 commit。
製品への配線はなく、§1.209のUI停止修正完了とは扱わない。Stage 2は最低22 source file、
20〜35時間と見積もり、利用者へ範囲拡大の確認を提示中。返答前に配線を進めない。
外部sidecarを長期間write/delete禁止にする案は撤回し、commit直前再検証を復旧snapshotの
線形化点とする。既存中央DBを優先する仕様を維持し、内部編集の競合対策へ範囲を絞る。

dupe側Phase 1は独立branchの`98dc41f4b`で保存済みと引き渡された。
similar_index moduleは67成功/5 ignored、core check・glyph・fmt・diff成功、独立r3承認。
masterへのmerge・機能再有効化はしていない。Phase 2の照合高速化、性能A/B、全体gate、
最終snapshot・確認buildは未完了であり、この狭域成功だけで再有効化を判断しない。

§1.208・§1.210は最終コード/設計の独立検収を通過。§1.210の入力stateは
ViewerContextBundleが所有し、通常の描画mount/depositでは保持、別context・fork・close・
index-space変更では契約に従い退役する。焦点テストはreducer 6件、bundle 2件、snapshot 1件成功。
統合checkpointは `target/next-version-work/fullscreen-input-lifecycle-20260911/checkpoint-manifest.json`。
全体gate、確認build、実機結果は後続記録を参照し、checkpoint作成だけでは成功扱いにしない。

## 9/11 早朝の検証結果と引継ぎ

normal core check、fmt、UI glyph、viewer_context_audit、`test-full.ps1 -SuppressCrashDialogs`は成功。
本体libは8123成功/43 ignoredで、§1.208の最終pointer分類と§1.210の回帰を含む。
UI snapshot 48件、sidecar integration 14件、vendor egui/egui-wgpu/eframeも成功した。
normal `build-dev.ps1` と `prepare-portable-smoke.ps1 -TestScript`も成功。

- 統合記録: `target/next-version-work/fullscreen-input-lifecycle-20260911/integrated-verification-manifest.json`
  (SHA256 `E5EC9403D1700C8E3793B941EFED69A5F8B0A8A5B49909359FE6352E259897C6`)
- normal core: `target/dev-runtime/mimageviewer-core.exe`
  (SHA256 `CD0C1B058D0B106BD1F8F9FCBFBCA85AB81D194B2F1A5633323AE4CA2C5EC522`)
- portable core: `target/portable-smoke/mimageviewer.exe`
  (SHA256 `EA3CCF0FC0AE0EA44C7D1840C1499BDBABF2B059378A510C13EA80487C79E9FE`)

実機は環境不成立として閉じた。初回はnative動画にegui側の入力待機条件を要求したharnessが
操作前にtimeoutし、Skyの状態取得も応答しなかった。条件修正後の2回目も、返却Windowへの
screenshot-only取得が`timeout_ms=60000`指定でも約389秒応答せず、親が停止した。
両試行の外部click/keyは0件。§1.208は実機未検証、§1.210は未実行、VSTありも未検証であり、
製品PASS/FAILはいずれも判定しない。アプリは終了し、通常APPDATAを使用していない。
記録は `target/next-version-work/live-evidence/live-verification-summary.md`
(SHA256 `186D42BB21CA9FEA3DF8C4AB4595D91BBDAC8BCD225CF7A4E10125BAEE181FA9`)。

実機後にbacklogと本台帳だけを更新した。統合manifestに記録された製品source 7件は
再照合して差異0であり、成功済みgate/buildは再利用する。Windows native変更の製品commitは
AGENTS.mdの実機確認条件に従い利用者の確認待ち。対象差分とbuildを保持し、再開時に
♪/Zの再生継続、右クリックメニューを閉じた後の再出現、長押し/F12を確認する。

類似検索側は小DB11件の同値・取消等の回帰も成功し、大規模合成DBのA/B測定へ進む。
実DB・通常APPDATA・アプリは使わず、9/11 09:00 JSTまでに大規模実行を終了する枠を引き渡した。
masterへのmerge・再有効化は依然未実施。

## 9/11 朝の類似検索検収と統合準備

dupe branch `codex/similar-index-incremental-reconcile` は `b7102a53a` まで完了。
差分通知化、Full job 限定の照合 inventory、進捗表示を独立検収し、全体 gate と
確認用 build が成功した。本体 lib は8120成功/44 ignored、UI snapshotは48件成功。
このビルドはmasterの未コミット§1.208・§1.210を含まない。

合成4,627,166件、UTF-8 key 80/120 bytes、16 workersの同一release実行系で、
旧照合約67.6〜71.9秒に対しinventory方式約4.48〜5.21秒。AB/BAの順序を変えて
結果一致を確認した。ファイル走査・decode・アプリ全体の起動時間の測定ではない。
120 bytesケースの新方式process private peakは約1137 MiBで旧方式約1619 MiBより低いが、
実アプリ全体の最大メモリと実watcher下の収束は未検証。終了直後の残留量を
inventoryの常駐やリークの証拠とは扱わない。

性能検収記録はdupe側の
`target/similar-index-incremental-phase2-benchmark-harness-r4r1-20260911/`、
最終gate/build記録は `target/similar-index-incremental-final-gate-20260911/`。
測定とbuildのプロセスは終了済み。

次の区切りとして、dupe側でmaster `aa034d578` の確定済み変更を統合するよう指示した。
`Paused` capabilityと`Option<SimilarIndexManager>`を維持し、休止中の通常名前索引、
Enabledテスト経路の差分通知・watch bootstrap・password更新・進捗表示を整合させる。
実装と別担当の独立レビュー、統合影響の焦点テスト・全体gate・確認buildを行う。
masterの未コミット差分、実データ、実アプリには触れず、再有効化とmasterへのmergeは
後続判断とする。既に有効な大規模性能測定は繰り返さない。

## 利用者の確認追記

2026-09-11、利用者から§1.210の右クリックメニューが再出現しなくなり、F12も動作するとの
報告を受けた。別窓でのメニューを閉じた後の非再出現と長押しの確認範囲を案内した。
§1.208は通常profileにVSTが設定されているため、未設定のportable確認版を別途準備する。
この時点では§1.208を実機確認済みとは扱わない。

続いて利用者がVST未設定portableで「♪はうまく動くようになりました。OK」と確認した。
確認exe SHA256は `8823E8AC36BA8CEF248123349708D6B9EDC0E37CF3013C79B5794F92CDEAE776`。
§1.208の報告再現経路と§1.210のメニュー再出現は利用者確認済みとして記録し、
成功済み自動gateと同一sourceの製品差分をコミットする。F12長押しやVSTありの全経路まで
実機確認したとは扱わない。

このportable準備では内部build-portable.ps1の既定Stop-Process -Forceが開発版core 8件と
Remote 1件を停止する事故があった。利用者へ報告済み、再起動なし、未保存状態への影響未確認。
今後は `build-portable.ps1 -PreserveRuntime` → `prepare-portable-smoke.ps1 -SkipBuild` とし、
稼働アプリを停止しない。記録は同checkpoint directoryの `manual-portable-vst-unset-build.json`。

## 2026-09-11 統合後の完了状況

§1.209 は `9c9df532c` と `06a256a24` で実装。復元はworkerで行い、短時間ならmodalを描かず、100msを超える場合に復元中表示を出す。別窓native動画の通常操作は維持。全窓native入力排他の追加案は利用者合意で撤回。原本・未保存owner保護は維持する。
統合gateはmain 8266成功/0失敗/45 ignored、他workspace/integration/doc/vendorも成功。実アプリでの新しい復元確認は未実施。

§1.217 サムネイル画質dialog自己拡張、§1.214 件数overlay半透明化、§1.215 同一ピンタグ再クリックで閉じる、は `108931b9d` で実装・独立承認済み。28frame寸法安定、明暗theme可読性、通常/スマートフォルダ復帰等の重点回帰に成功。完了3節はbacklogから除去し本記録へ移した。
同commitの全gateはmain 8272成功/0失敗/45 ignored、全integration/doc/vendor成功、fmt成功、glyph0。`build-dev.ps1 -PreserveRuntime`成功。core SHA256 `3C568D002FFA375DAA9AED462787A75F44B14C57B79DD875239ECD11943ED6C3`。アプリ起動・停止は行っておらず、UI3件の利用者実機確認は未実施。

類似検索・コンテナ索引の製品変更は `12e9b8008`、`2b03c73b6` までmasterへ統合済み。実機測定の最新記録はdupeのdoc-only `54c36d6b6` / `a691aee59` を参照。類似差分27〜61ms、コンテナ同件数108秒→15.4秒/56秒→9.9秒。前後のキャッシュ条件は統制していない。

残る今回の製品実装は動画メモリ消費の§1.190と列幅§1.211。自動余白カット§1.216は利用者指定で後続版へ延期し、設計を保持する。実機自動検証は次回以降リリース前に約20分の枠を想定、診断版を事前準備し具体suiteの了承後に実行する。
