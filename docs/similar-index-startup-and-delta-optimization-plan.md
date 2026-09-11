# 類似索引：起動確認と変更時DB整理の次段階

2026-09-11。設計レビュー完了後、利用者の依頼により実装を開始。実装・性能検証は進行中で、完了扱いにしない。
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

共通変更箇所は主にsimilar_dbの候補取得・summary更新とsimilar_indexのscope受渡し・計測。全writerへの影響を設計確定し、親タスクのapp.rs等と重複編集しない。実装後は限定テスト→独立レビュー→全体gate→利用者用build。実機起動は利用者が行い、本番DBの直接変更・索引削除による検証はしない。設計文書だけの段階ではbuild不要だったが、実装後は確認buildを用意する。

## 設計レビュー記録

2026-09-11、実装調査 `implement_resume` と別担当 `review_resume`（双方GPT-5.6 Sol / xhigh）。独立レビューのP1 2件（child画像本の未観測prune、summary baseline）とP2 1件（RootRepair・長時間TX・migration検収）を上記へ反映し、限定再確認でP1/P2残件なし、設計draft承認。実装や性能結果の承認ではない。source変更・Cargo・アプリ操作・本番DB操作なし。

## 実装の区切りと進捗

利用者の実装依頼を受領。実装・テストは `implement_resume`、独立レビューは別担当 `review_resume`、rootは設計判断・文書・統合調整を所有する。両担当のモデル指定は引き続きSol/xhigh。親タスクのsidecar等とはsource所有を分離する。

| 区切り | 完了条件 | 状態 |
| --- | --- | --- |
| DB基盤 | 既知schemaの保持移行、scope候補取得、identity保護、summary baselineと小fixture | r5/r6実装・自動検証完了 |
| 索引接続・計測 | production Delta経路への接続、child scope回帰、種類別時間 | 実装・回帰済み、実機分類時間は未測定 |
| 性能・独立レビュー | 小K/大Nの候補数・query plan・時間・移行容量、独立指摘解消 | r5/r6承認済み、実機速度は未測定 |
| 全体gate・確認build | 最終sourceで全体テスト、実行中アプリを保持した確認build | r6全gate・確認build成功、実機確認待ち |

前提確認では既存schema 3の明示的保持migrationを追加する方針。schema番号だけを変更して既存再作成分岐へ流さない。既知の古いschema移行も回帰対象。実アプリ起動・停止・通常DBの操作は行わない。

初回core checkではDelta取消分岐の `ScanJobOutcome.watermark` 追随漏れ（E0063 1件）を補正し、再checkはexit0、33.04秒との実装担当報告。両回の出力原本はこの時点で未保存のため、原本付き最終検証とは分けて扱う。Directory/Loose/ImageBook/ZIP/PDFの累積worker wall/max計測は接続済み、migration・scope・summary回帰の追加中であり、source freeze・独立実装レビュー・最終gateはまだ。

最初の焦点回帰は `target/similar-index-startup-delta-optimization-20260911/validation-r1` に記録。rootも原本末尾を照合し、`delta-scoped.log` 6 passed（0.02秒）、`schema-v4.log` 2 passed（0.06秒）を確認した。scope候補件数、child保護、identity変更保護、cancel rollback、invalid summary、v2/v3保持移行と移行失敗後の再openを含む。既存テスト全体・大規模性能・実機改善まで確認できたという意味ではない。

### Production freeze r1

静的確認でドライブ直下のparent keyが `c:` と `c:/` で不一致になる境界を発見し、rootを保持する親key計算と回帰を追加。実行出力未保存の7件成功は参考記録にとどめ、最終ログ付き集合で再確認する。実 `run_delta` のparent-child画像本保護は `validation-r2/parent-child-book.log` で1 passed / 0 failed、0.02秒（compile1分32秒）をrootも確認した。

13:47 JSTの `freeze-production-r1` を独立レビューへ渡した。manifest SHA256 `1AA46689FEFBEC91E7D64CB873F76ED762144A2056F552BDC1407F33A21A5FA0`、patch SHA256 `6B158DCA5E959FE2FF3AD4C22E22F682CD0F131B9BC45DDFEA91F22EC338B297`。sourceはsimilar_db `8FBD978B36F1347AEE3380F80A992E1238F8F616D2D934636392A0EE444B6082`、similar_index `E0EDA60566D81692CCDDD8A5E20BB3CA01BAE53AD51FE9CE0D12AA678B4D09D9`。rootも実file hash一致を確認。性能harnessだけ別test子fileで追加できるよう分離し、production変更時はfreezeを更新する。

既存焦点回帰 `validation-r2` はsimilar-db-module 51 passed / 1 ignored、incremental-reconcile 32 passed、schema-v1 6 passed / 1 ignored、missing-subtree 1 passed、telemetry-safe-unsafe 1 passed、全exit0をrootも照合した。これらは一部集合が重なるため合算の独立件数とはしない。

### r1独立レビュー指摘への対応中

独立レビューでP1 2件：DeltaのcleanupがActivityGateより先でpause/cancel中もDB writeを始める点、scoped loaderの全行collectが巨大scopeで取消pollなしの長いread TXと一時memory増を作る点。r1を解除し、gateをcleanup/metadata/DB取得前へ移す実経路回帰、stream取得と定期取消確認・fallible growth・DB解放回帰を実装担当へ依頼した。P2としてprefix scope各種のquery plan/固定K増Nの証拠が未完で、性能harnessの検収へ追加する。その他のchild保護、identity/failed保護、summary CAS→Full修復、watermark/array ackはレビューで成立を確認。再確認は修正差分と性能証拠に限定する。

### r2静的freezeと検証開始

ActivityGateをDelta入口へ移し、scoped loaderをstream取得・4,096行ごとの取消確認・fallible growthへ変更。性能fixtureを固定K/2段階Nとparent/prefix loose/container/orphanの5 query plan、v3/v4容量・移行時間、旧新最終fingerprint比較へ拡張した。`freeze-r2/MANIFEST.txt` SHA256 `D4023752464DA20CF4B82EA54F967B7EB4305E9BC8719B0C7BD665AEF6D16629`、complete patch `0B418A7BD246A17DB7072B0BEEBBBE896017A43A850725ACA8F5500D162E8E50`。freeze時はrustfmt/diff-checkのみ成功、未compileとして記録。独立レビューへr1との差分を渡し、209からCargo枠返却後に限定テスト・性能検証を開始する。r1成功をr2の成功と読み替えない。

r2再レビューではActivityGate/stream処理を確認したが、production SQLのORDER BY/DISTINCTがrow返却前にscan/sortを行う余地と、EXPLAIN fixtureがその句を省いている不一致をP1とした。順序に意味がないinventory取得の不要な並べ替えを除去し、row step前poll、productionと共通のSQL定義によるquery plan検証へ修正する。P2としてmigrationのWALをcheckpoint後だけ測る問題を指摘し、checkpoint前DB/WALと後DBを分離する。r2は未承認。`validation-r2fix/activity-gate.log` のE0373は追加テストclosureの所有権に関するcompile失敗であり、原本を保持して修正する。

### r3と小性能測定で残る全体件数依存

`freeze-r3/MANIFEST.txt` SHA256 `A70B43D7A689309C2B1071DC6EF8166CA74A5F7D744F082AAA6C0565A85FCF23`、complete patch `08E284C50F277D5920376F50E6D804D598CA7C28E842905E58B69966910BF952`。r2との差分レビューを依頼。`validation-r3` のActivityGate実経路1 passed、delta-scoped 9 passed / 1 ignoredをrootも確認した。

`performance-small-r3/result.json` はK=8、N=4,096/32,768でcandidate両8・最終fingerprint一致。旧publish56.44/470.79msに対し、新load0.524/0.303ms・publish2.61/16.36ms。しかし新publishもNに追随する傾向があり、性能検収完了とはしない。rootはsubtree_looseのplanが `item_container_idx(container_key=?)` のみでprefix範囲を絞らない点と、publish/summaryのtemp joinが全itemを外側にし得る点を指摘し、大規模run前のquery plan/処理量確認を依頼した。SEARCH表記や返却候補数だけでは、内部の全件処理を排除した証拠にはならない。

### r4小測定で全体件数依存を除去

loose key用partial indexでexact/rangeを別々に引き、finalizerの集計/journal/deleteはtempのK行を外側に固定してitemをkey/rowid検索する方式へ変更。最初の小測定はquery plan等の検収で失敗した原本を保持し、条件を緩めず修正した。`performance-small-r4r2/result.json` はK8・N4,096/32,768、候補両8、旧新最終state一致。新load0.535/0.310ms、publish0.486/0.336ms、旧publish55.90/458.66ms。rootもJSON原本を照合した。

本番SQLのVM stepsはN両点でcount160・journal465・delete358と同一。fullscan_steps=7はtemp8行の走査で、item全体の走査ではない。sorts=0。query planもtemp外側→item key/rowid検索を示す。migrationの直接観測WAL704,552/5,397,232 bytesと報告peakを一致させ、checkpoint後0を別記録とした。小fixture結果であり、実機26.7秒がこの時間になると約束するものではない。大N測定・r4独立レビュー・最終gateはまだ。

r4の独立限定レビューは承認、blocking P1/P2なし。`freeze-r4/review-approval.md` SHA256 `B89DA9630C67FA58711207D2F58405690F9CB659BEEDFE730605F7E504D726B1`。partial index追加をschema v5とし、v4も保持移行する。source hashはDB `8B558E01683F4FA165E6AD3457AA0A25BE73D140CB0573721AD1DB13D906546D`、index `A12320D01277FFD7783C402CB3FC2FBA5DFBC8D3221D9C22D68AA1A3625B834A`、benchmark `80178D55CBCA9815DD557F531678DBAD26766D8F9CFB94EF7185B6E9F731A847`、complete patch `BE1497E16743C4F289EDB620C379BD208815616B512A7A86926AC7E0059AB57B`。

同freezeのdebug test exeでK8・N32,768/4,627,166の大N測定を `performance-full-r4` のfresh fixtureで開始。専用TEMP/TMP、APPDATA不使用。大規模migration・性能証拠と全体gate・実機は引き続き未確認として扱う。

### 大N証拠確認とr5移行補正

`performance-full-r4/result.json` SHA256 `BBA0C314CB906D629C5DF701A3FAF2D0FDD48C1042576DC6CA079B522F4182C9` をrootと独立reviewerが照合。K8/N32,768→4,627,166でcandidate両8、VM steps160/465/358、temp由来fullscan7、sort0が不変、最終state一致。load0.5487→0.3121ms、publish0.4308→0.3808ms、旧publish0.452→66.518秒。同host cacheのdebug合成比較であり、実機26.7秒への倍率外挿はしない。全行looseで、ZIP/PDF/画像本が混じる利用者の分布の代表ではない。

v3→v5移行は369.273秒、DB656,338,944→988,246,016 bytes、checkpoint前WAL778,399,872 bytes、temp観測peak167,258,400 bytes。移行中process sampled peak WS36,638,720 / Private57,978,880 bytesはこの検証processの値で、実アプリ総peakではない。初回移行が約6分を要したことと一時disk増を、通常Deltaの短縮と必ず分けて伝える。各fileのpeakは同時刻と限らず、単純合算を実測同時peakとはしない。

実装担当の追加確認で、v4→v5も既充填parent keyを再計算・UPDATEしていた点を発見。NULL行だけのbackfillとv4 UPDATE禁止trigger回帰へ最小補正するr5を承認し、r4を解除した。v3 fixtureは全parent未設定のため上記初回移行証拠は保持し、v4を別測定する。r5差分確認前は全体gateを開始しない。他の性能SQLや取消契約のレビューを最初からやり直さない。

### r5最終限定承認・全体gateへ

NULL行限定の補正後、v4 fixtureはitem/container/container_buildのparent列UPDATEを禁止するtriggerがあっても移行・再openが成功。`validation-r5` の移行2、DB module53（1 ignored）、index module77（5 ignored）が成功、core checkもexit0（12.39秒）。rootも原本を照合した。

`performance-full-r5-v4/result.json` SHA256 `9EF65E46C99FFF9585DE3A82F9706B918607556D6CB684221FB8F09DBDF79544` はN4,627,166/K8でv4→v5 open3.359秒＋checkpoint0.021秒、load0.392ms＋publish0.319ms、candidate8・最終state一致・VM steps不変。v3の全parent未設定条件で測った369秒とは別条件。v4追加index移行もWAL168,849,992 bytes / temp観測peak165,483,100 bytesを要しており、ゼロ負荷とはしない。

`freeze-r5/MANIFEST.txt` SHA256 `2EEB917852CD17AA3B7F275328BB0240918DE4CC00996858C15169FE0FB98AE1`。sourceはDB `168CA99B2BA1406F9BC6B364256273F29A19ED081AF5CB5ABC77845768BFB8DE`、index `A12320D01277FFD7783C402CB3FC2FBA5DFBC8D3221D9C22D68AA1A3625B834A`、benchmark `52A14711DF3E6A6A5B8A060F6ECE7C056DBC19927D89E41C9DD36582182AAD91`。complete patch `7FB527AD9D5602D1A97A1EBB3709187078FC8F6C922AB597565A0927D2845634`。

独立レビューはr4との差分と大N証拠を承認し、blocking P1/P2なし、全体gate開始可。`freeze-r5/review-approval.md` SHA256 `C0FA7E9EAD2F907A214794B12EE42A3E996F0E3DC6238A4ED004D526867A9549`。親209とCargo枠を調整し、同sourceの全体gateを指示した。実機の初回移行・更新速度、起動FS分類実測は引き続き未確認。

### r5全体gate成功と残る入口cleanup

全体gateは14:56:48.905–15:09:27.807 JST、exit0 / timeoutなし。本体8,191 passed / 45 ignored（353.55秒）、UI48（4.21秒）、vendor egui25 / egui-wgpu9 / eframe15を含む全段階成功。証跡 `fullgate-r5`、stdout SHA256 `4B75C65AA532E6943B51FD92116360BCDF7AC7DCACFF500F8EE8B9B19DD52E82`、stderr `39E9314416F7B7203AB177474E8B7467D87EB0109D8B672770A8047DE25C2357`。rootも原本とsource固定を確認。Cargo枠を209へ返却し、buildは未開始。

最終説明の監査で、毎Delta入口の既存 `cleanup_incomplete` → `load_items_for_container_state(Building)` が全itemを外側にするSQLだった点を確認。rootと独立reviewerが既存合成DBへPython SQLite3.49.1でread-only EXPLAINし、container0でも `SCAN i → SEARCH c` となる計画を確認した。bundled SQLiteの計測は追加回帰で確認する。この時点で成立した性能説明はscope inventory/publish/finalizer部分のN比例排除であり、通常Deltaの準備全体のN比例排除ではない。

r5を承認・全gate成功のまとまりとして保存し、追加chunkで共通cleanupのmember選択をBuilding container外側→member index取得へ直す。Complete/loose保持、delete+journal順、staging掃除と単一TXを維持する。Full/Delta入口は取消確認付き経路、失敗・取消後の後始末は既存の無条件経路として所有を区別する。新schema追加は不要とし、container側の走査が残る点を明記する。追加差分は限定レビュー・焦点回帰・再gate後に確認buildへ進める。

### r6入口cleanupの限定レビュー

r5は `e614d6b97` に保存。r6 precompile freezeのmanifest SHA256は `4CC4ED8B88213607BCDA1B834F87EAC79AE8936CCB3FAA34E5AFE26444B73CB4`、DB `1070857F7E8F85454FA7FDAFD5C0576125C06CE03122E71C17FDD6058509319E`、index `7E5F1E71EC0901B3DAA5EFEC458732D8B02C54FA88F204282BE383EFCF2A1F93`。schemaは変更しない。

独立Sol/xhighレビューではblocking P1/P2なし。共通SQLをBuilding container外側→member index検索とし、item_id順の削除journalを維持。入口だけ取消可能とし、失敗後の無条件回収を保持する。SQLite progress handlerはRAIIでcommit/error/rollback前に解除する。静的承認時点では未compileであり、保持・journal順、処理途中取消のrollbackとhandler再利用、固定K/増Nの実SQL plan/VM、新旧Full/Delta取消経路の実行を承認条件とする。性能説明はglobal item scanの除去に限定し、container走査・対象memberの一時保持とsortは残る。

`validation-r6` は新規cleanup3、Full/Delta activity2、DB56（1 ignored）、index77（5 ignored）が成功。core check38.25秒、fmt/diff/glyphもexit0。完全修飾名のexactテスト1件で、bundled SQLiteのproduction SQLはN4,096/32,768・K8とも `SCAN c → SEARCH i USING COVERING INDEX item_container_idx`、VM88・fullscan0・sort1と一致した。sortは対象8件のitem_id順である。誤った短いexact指定による0件ログは非証拠として保持し、1件実行ログと区別した。

最終 `freeze-r6/MANIFEST.txt` SHA256 `6794A97536E9C0D685A40B44752B7E747908A3968D6E4A6852246DD1C995095B`、DB `11F7B5E443C3E209BBD46327D339BA6B170A092828094E13967DDDE3133DBAAC`、index `7E5F1E71EC0901B3DAA5EFEC458732D8B02C54FA88F204282BE383EFCF2A1F93`。precompile後はテストの計画出力とrustfmtのみ。最終bytesを全体gateで再compileする。

独立レビューは最終差分・実行証拠を限定照合し、r6 blocking P1/P2なし、全体gate継続可と承認。`freeze-r6/review-approval.md` SHA256 `9D15C7C939A97A3E8F646CE25537206AD3FABB314E6B8866063E4257E18D12EC`。全体gateは15:39:39 JST開始、jobs1、通常アプリや実DBを操作せず実行する。

### r6全体gate成功

15:39:39–15:52:26 JST、exit0、`[test-full] PASS`。本体8,194 passed / 45 ignored（377.67秒）、UI snapshot48（3.99秒）、vendor egui25 / egui-wgpu9 / eframe15を含め全段階成功。rootも原本と最終source hash一致を確認した。証跡 `fullgate-r6`、stdout SHA256 `B4841EBE65FCD0B509DFC3FE13A20ACC952EFEC929CCCD566F9363E563DA5431`、stderr `B919430B887254B136AF3E1E04F2B9893570B45F4B6B8F845ED234F7CCEA33FE`。実機時間はまだ未検証であり、確認buildを作成して利用者へ渡す。

### 確認buildの引き渡し

r6 sourceは `ae0b9f7ee` に保存。`build-dev.ps1 -PreserveRuntime` は15:53:21–16:03:42 JST、jobs1、normal feature、exit0。開始時のe614d6b97＋差分とae0b9f7eeのsource hashは同じ。開始・終了時とも対象dev-runtimeアプリ0、終了時Cargo/rustc/link0、アプリ起動・停止・実DB操作なし。証跡は `build-r6`。

`target/dev-runtime/mimageviewer-core.exe` は310,144,512 bytes、2026-09-11 16:03:40 JST、SHA256 `56803B1F9E01AA1F6FE93F4C36FA0FCBDCBD28087E11B3C5E140424F83095547`。Remoteは変更なしで再利用、SHA256 `4456A614C8D40E9DF08C3BEACF208ACA86743EAB812D2CEE3B02CE0C33A2C3AA`。rootも実file hashを照合した。

利用者の手動確認は、常駐版を終了後、通常profileの上記coreを起動し、初回移行・Full完了を待ってから監視対象へ1件コピーしDeltaの各phaseを測る。既存v3からの初回移行は合成約463万行で約6分を要したため、通常更新の時間と分ける。実機の移行・起動分類時間・Delta短縮は未検証。全件フォルダ確認の共通化は実測後の別設計であり、今回実装済みとはしない。
