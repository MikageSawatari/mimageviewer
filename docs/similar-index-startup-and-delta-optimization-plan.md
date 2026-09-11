# 類似索引：起動確認と変更時DB整理の次段階

2026-09-11。設計検討のみ。実装・性能検証は未実施。
前段の実装と検証は [incremental reconcile計画](similar-index-incremental-reconcile-plan.md) を参照。

## 要求と実測

利用者は、起動時の種類別計測と重複処理削減、およびファイル更新時のDB整理短縮を希望している。約1GBのメモリ使用は選択肢として許容するが、操作のたびに長時間CPU・I/Oを使わないことを優先する。検出精度、起動停止中の変更確認、失敗時の既存索引保護は維持する。

source `51caf188e`、記録HEAD `f1ae62b08` の通常ログによる観測：

| 処理 | 準備 | FS確認 | DB整理・公開 | 検索配列反映を含む総時間 |
| --- | ---: | ---: | ---: | ---: |
| 起動Full、約465万件 | 4.901秒 | 172.387秒 | 1.975秒 | 179.266秒 |
| コピー後Delta、1 scope / 328件 | 1.821秒 | 約0.018秒 | 24.808秒 | 26.654秒 |

起動のindexed=0、removed=0。既存署名を再利用してもFS確認が残る。Deltaは全件FS走査ではないが、`similar_db.rs::prune_scopes_transaction` が全item/container keyを列挙し、各itemを読んでからscope判定する。さらに `refresh_index_summary_count_transaction` は全件COUNTする。これらの個別所要時間は未計測であり、24.808秒の内訳を断定しない。

Deltaのremoved=2,108はDB itemとcontainer行の合計で、実ファイル削除数ではない。対象の内訳は現ログで不明。新方式との比較で削除候補と理由別件数を検証する。indexed=83もコピー数やdecode数ではなく登録・更新件数である。

証跡：`target/similar-startup-full-hotpath-20260911/live-20260911-1225/similar-events.txt`（SHA256 `FDC39659DFA3B6A59417C4CC452D58C68D8CC9523EEB5FA797CADE885E7AB40A`）、`live-delta-20260911-1240/events.txt`（同 `319C328FD78A3C8DD3E518001F3CD5676A7835D61C6E9EA25D3459EE68F74750`、同じ証跡親ディレクトリ）。追加の本番DB操作・実データ走査は行わない。

## 採用する順序

1. 起動FSとDelta DBを分類別に計測し、既存の集計ログに追加する。
2. Deltaの候補取得・整理・集計を変更範囲に限定する。DBを正本とし、ジョブ内の小さなメモリinventoryを併用する。
3. 起動時は計測で確認した重複metadata取得・列挙・openを削減する。署名再利用条件は弱めない。
4. 残るDBアクセスが支配的な場合に限り、常駐inventoryの採否を実アプリのメモリ・時間で判断する。

### 起動：計測と再利用

directory列挙、loose画像、画像フォルダー、ZIP、PDFを分け、列挙・metadata・既存判定・decodeとDB fallbackの時間/回数を集計する。並列workerの累積時間とjob wall時間は区別し、合算を経過時間と表示しない。長い単一書庫処理も完了時に測定値を残す。ログ書込みはDB lock外、パスごとの大量ログは出さない。

source調査ではSimilar内のmtime/sizeは既にDirEntry metadataからFileCandidateへ渡されており、loose/ZIP/PDFで再statしていない。この箇所に未実装の削減余地があるとは扱わない。残る具体的候補はmetadata/name walkerとSimilar Fullが別々に行うread_dir/statである。まず分類別計測で寄与を確認する。

共有するなら完成済み索引結果ではなく、watch登録後の同一root/config epochで得た `ObservedDirectoryBatch`（path/type/mtime/sizeと観測完了・error）をbounded queueで各consumerへ渡す設計が必要。metadata側のsidecar/video/audio、Similar側の本判定・ZIP/PDFページ列挙はそれぞれ維持する。除外条件、ActivityGate、cancel、遅いconsumer、drop/欠落、観測失敗をconsumerごとに扱い、不完全なconsumerだけprune-unsafeとrepairへ戻す。別観測間の永続cacheとは区別する。

これは共通走査機構の変更であり、Delta DB修正と混ぜない。計測後に影響先・所有権・統合方法を別設計として提示し、現状の局所的な重複削減だけでは足りない場合は、その境界を利用者へ説明してから実装する。

directory/root mtimeだけで再帰確認を省略しない。ZIP/PDFの現在性判定はページ数等も含むため、mtime/sizeだけで書庫列挙を省く案は採用しない。コンテナ・アイテム索引の観測結果は対象と情報が異なり、そのまま類似索引の完全性証明にはならない。USN等による停止中の変更追跡は今回の範囲外。CPU並列度増加による短縮も音声への影響があるため先行しない。

### 更新：scope限定DB処理

FS走査とDB候補取得が同じtyped scope planを使う。現在の `item_key` UNIQUE、`container_key` PRIMARY KEY、`item(container_key,page_index)` indexをまず評価し、親パスの検索index等が必要ならmigration費用を含め別途確定する。文字列全件走査をSQL関数やメモリループへ移しただけの実装は不可。

候補はloose itemとcontainerの `source_parent_key` 列および親検索indexで、looseにはpartial indexを検討する。既存prefix indexだけでDirectoryContentsを検索すると、root直下では全subtreeを候補にしてしまうため、恒久解とはしない。新列のbackfillとindex構築は初回全行writeになり得る。使い捨てfixtureでmigration時間・WAL・DB容量増を測り、通常起動の短縮とは分けて報告する。

- DirectoryContents：looseは直下、破壊対象containerはdirectory自身の画像本と実際に観測した直下file container（ZIP/PDF）。直下directoryの画像本は再帰なしでは観測していないため、parent一致だけで削除対象にしない。列挙で存在確認したchild directoryを明示保護し、消失・再分類は対応するchild scope/RemovedPrefix/Subtreeで確認して扱う。ページはitem keyの文字列形式ではなくowner経由で取得する。
- Subtree/RootRepair/RemovedPrefix：区切りを考慮したprefix範囲。似た名前、大小文字、末尾区切り、drive root、SQLの `%` / `_`、包含・重複scopeを正規化規約どおり扱う。
- 対象scopeだけの `DeltaScopedInventory` を作り、FS確認中はDB mutex/transactionを保持しない。小Kでは短いread transactionを目標とするが、巨大RootRepairでは保証しない。取消確認とDB mutex/TX占有時間の測定を設計に含める。ActivityGate待ちは取得前に行う。
- 最終transactionでloose更新、container完了、prune、item_change、summaryを原子的に公開する。取消・古いconfig epoch・観測不完全時の保護契約を維持する。
- snapshot後に作成・変更されたitem/owner/generationを古い判定で削除しない。既存Fullのidentity再照合と同等の保証を、最終transactionの現在行で行う。
- failed/password containerの旧Completeとmemberを保護する。book↔loose、rename、delete、root除外・重複rootも同じ所有境界で扱う。

summaryは「現hash versionで、looseまたはComplete owner」という既存の可視条件を維持し、重複除去したaffected setの変更前後差分で更新する。単純なinsert/delete件数ではcontainer状態変化を数え落とす。`SummaryBaseline { store identity, hash_version, through_change_seq, registered_items }` 相当を永続化し、可視行を変える全commitが同じtransactionでcountとseqを進める契約を作る。欠落・旧schema・version/store/seq不一致はinvalid baselineとして、通常Delta内の同期全COUNTではなくActivityGate下の明示repair/Fullへ戻す。契約で検出できない任意の破損まで差分だけで発見できるとはしない。Fullの全COUNTは監査・修復に残す。

Full/purge/container replace/store切替を含む全writerを棚卸ししてから変更する。prefill単独は公開itemでないためsummary変更なし、password変更は直接writerではなくFull要求として分類する。

独立レビューで旧pruneの欠陥候補を確認した。`discover_directory(..., false)` はchild directoryを再帰確認しないが、旧DB pruneはparent一致のchild containerまでcoveredにする。この不一致を高速化で継承しない。実機removed=2,108との因果は未確認。親の無関係なfile変更では既存child画像本を保持し、child削除/内部変更では対応scopeで退役・更新する回帰を必須とする。

大量削除はset-based化してもjournalの意味を変えない。watermarkとarray ackが揃う前にCompleteにしない。巨大なRootRepairは対象自体が大きいので、通常の小Deltaと測定を分ける。

## メモリ案の比較

| 案 | 利点 | 費用・判断 |
| --- | --- | --- |
| DB indexによる範囲限定 | 常駐コピー不要、小更新が全DB件数に依存しにくい | index追加時の構築・容量・migrationを検証 |
| 全件常駐inventory | 判定のDBアクセスを減らせる | 起動ロード、全writerとの整合、prefix/owner index、allocator保持、他cacheとの合算が必要 |
| 範囲限定DB＋ジョブ内inventory | 一度読んだ対象をメモリで再利用し、所有期間を限定できる | 最終transactionでidentity再照合が必要。優先案 |

過去の4,627,166行syntheticでは、キー80/120byte時の新Full inventory方式のプロセスpeak Privateは約921/1,137MiB。これはinventory単体の増分ではなく、実アプリの常駐量でもない。終了直後はallocator保持もあった。既存SearchSnapshot約212MiB、画像cache、Full、array rebuildの同時peakは別途実測が必要。約1GB許容を常駐設計の確定予算・上限と解釈しない。

常駐化する場合も全件memory scanは避け、正規化キー・親・ownerから限定検索できる構造が必要。DB commit成功後にだけ同じgenerationへ進め、rollback/失敗/外部変更/store切替時の再構築条件を定義する。memoryを正本にして永続化とのずれを黙って許容しない。今回この構造変更は実装しない。

## 実装前提と検収

実装担当Sol/xhighと別の独立レビュー担当Sol/xhighが前提を確認する。矛盾があれば設計に戻す。レビューは旧方式との同値だけでなく、旧方式の削除・scope設計自体の正しさも確認する。

検収は小scope Kを固定して全DB Nを増やすsyntheticで、SQL query planのindex利用・候補行数・読込回数・各段階wall/CPU/I/O・Private/WS peakを比較する。通常DeltaでN件列挙/N回point read/全件COUNTが残らないこと。実機の秒数目標はこの測定後に決め、未測定の短縮倍率は約束しない。

scope matrix、snapshot後追加/owner変更、failed保護、book↔loose、rename、cancel rollback、hash/config変更、重複scope、array ackを回帰で確認する。旧新の最終rows/container state/summary/watermarkおよび重複を隠さないitem_change multisetを比較する。journal順序が契約に関係する場合は順序も検証する。

既知の旧scope欠陥は旧方式との同値を正解にせず、期待する保持・削除を独立したfixtureで定義する。summary旧baseline移行、commit/rollback、Complete/Failed遷移、purge、store置換、seqとwatermark不一致も検証する。parent index migrationはrollback・中断後旧schemaからの再開・WAL/空き容量・正規化version変更を含める。

共通変更箇所は主にsimilar_dbの候補取得・summary更新とsimilar_indexのscope受渡し・計測。全writerへの影響を設計確定し、親タスクのapp.rs等と重複編集しない。実装後は限定テスト→独立レビュー→全体gate→利用者用build。実機起動は利用者が行い、本番DBの直接変更・索引削除による検証はしない。本変更は設計文書のみのため今回buildは不要。

## 設計レビュー記録

2026-09-11、実装調査 `implement_resume` と別担当 `review_resume`（双方GPT-5.6 Sol / xhigh）。独立レビューのP1 2件（child画像本の未観測prune、summary baseline）とP2 1件（RootRepair・長時間TX・migration検収）を上記へ反映し、限定再確認でP1/P2残件なし、設計draft承認。実装や性能結果の承認ではない。source変更・Cargo・アプリ操作・本番DB操作なし。
