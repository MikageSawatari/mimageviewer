# §1.375 監視更新の準備結果と現在の表示を比較して再表示を省く

2026-10-13、`next-list`、R8。作業基点 `c903a369b`。R6の比較設計の基点は`918a51309`、R7は`84f31c346`。
**LISTrev7のREVISE（P2二件）を受けた設計担当決定を反映。代表の選択保持とcache変更・移行を撤回し、通常prepareの完全一致時だけinstallを省く。LISTrev10の独立設計再レビューはACCEPT（P1／P2なし）、2026-10-13利用者から実装指示を受けた。初回採用入力の共有境界が未確定のため設計判断待ち（§7）・新方式は未実装**。
R8の設計提出時は設計書とmanualだけを改訂した。実装工程では本書の所有境界と受入条件を維持し、§1.373のAppend／Undo契約を変更しない。
正本項目: [next-release-backlog.md §1.375](next-release-backlog.md#1375-フォルダで-delete-キーで削除すると正しく消えた一覧がもう一度丸ごと読み直されてちらつく--利用者の実機報告-2026-10-12)。

R3はACCEPTだったが、その後の実装調査で初期表示と自動代表の採用証拠が対応しない前提不足を確認した。
2026-10-12の案B、R4のprovenance配管、operation IDによる自己操作receipt照合は、
**2026-10-13の利用者決定で採用を撤回済み**。R8ではR5〜R7の代表保持契約も撤回する。
過去の設計は `ae2772c8e`／`84f31c346` の履歴を参照する。
§1.373のR4案1と、そのAppend／Undoのoperation ID・受付契約は変更しない。

## 1. 原因と新しい不変条件

Deleteの既存経路は `handle_delete_key` → 確認 → `start_delete_files` → delete worker →
`poll_delete_pending` → 成功pathだけ `remove_items_batch`。
終端は `current_folder_signature = None` にする。監視による後続走査は一致分岐を通れず、
`apply_external_rescan` → `load_folder_with_scan_owned` で一覧を丸ごと再採用する。
Delete後の表示と、後続走査が準備した表示を比べる境界がないことが、余分な再表示の原因である。
これはsource inspectionで確定した実行可能経路であり、利用者実機の時系列ログを得たという主張ではない。

`remove_items_batch` は成功行を除くだけで、非表示候補を追加しない。
`a.jpg + a.png` のJPG優先表示からJPGを削除した場合、PNGを再表示する必要がある。
したがって「自分のDeleteだから読み直さない」でも、raw走査の一致だけでも正しくない。

新しい不変条件は **通常の走査・表示準備と全行比較を既存prepare ownerのworkerで済ませ、
比較対象の表示世代／revisionが採用直前にも有効で、完全に同じ場合だけ一覧の入れ直しを省く**。
自分の操作か外部操作かは判定条件にしない。
一致しない／比較材料が足りない場合は従来の一覧採用へ流す。
時間窓で通知を捨てず、準備結果が違う外部変更を取り込む。

## 2. R8: 再表示を省いた結果として現在のサムネイルが残る

**設計担当決定 (2026-10-13)**: 利用者の「代表はCtrl+F5まで反映されなくても良い」は
更新を遅らせられるという許容であり、代表の選択を固定する保証要件ではない。
R5〜R7で足した自動代表の選択キー保持・選択付きpayload・leaf付きcache key・旧cache移行の
契約をすべて撤回する。表示比較のために代表選択ownerやcatalogの新しい形式を作らない。

### 2.1 表示と既存thumbnail lifecycle

監視更新でも通常の走査・表示準備を行い、§3の完全一致が確認できた場合だけ、
一覧install・世代更新・そのinstallに伴うサムネイル再要求を省く。
既存のitems／thumbnails／texture／要求ownerを入れ直さないので、表示中のサムネイルは
代表を含めそのまま残る。これはその時点の表示を置換しない効果であり、選んだleafを記録して
以後の画像を同じleafへ拘束する仕様ではない。

- 差分あり／一致を確認できない場合は、通常のprepared一覧を従来どおり採用する。
  代表も従来の選定・cache・要求経路に従うため、監視更新によって変わってよい。
- CPU／GPU eviction、画面外からの再読込、idle upgrade、サイズ変更、cache miss／再生成、
  先着済み要求の完了・RAW継続は既存のlifecycleを使う。その時点の通常の代表選定や
  共有cacheから画像が来て、代表が変わることを許容する。別contextの明示更新で共有cacheが
  更新された後も、同じファイルの代表を使い続ける保証はしない。
- 一致採用を理由に既存の進行中thumbnail要求を取り消したり、画像採用／保存終端を偽装したり
  しない。既存ownerによる後続採用や独立した再要求は継続する。比較中に表示値が変われば、
  §3.3のrevision照合で旧比較を棄却する。
- 一覧全件のCPU画素保持、GPU keep_range／upload予算、cache容量・evictionは変更しない。
  新しい保持キー・要求mode・payload field・cache lookup参照は追加しない。
- 自動代表についての「保持選択先が消失した場合だけ再選定」という例外は撤回する。
  保持機構がないため、ファイル消失／読込失敗／NoArt等は従来の通常選定・要求規則で扱う。
  消失検査や再選定のための専用状態・新request配送を足さない。

手動pinの指定／解除、pin先消失時の既存fallback、編集プレビュー等の明示操作は従来どおり。
保存されたpin設定を監視や明示更新で解除しない。手動固定の既存契約と、撤回した
自動代表の選択保持契約を混同しない。

Ctrl+F5（「最新の情報に更新」）は従来どおり一覧を更新し、自動代表を選び直す。
完全一致による省略は監視等の自動更新の採用境界に限り、明示更新まで省く設計にしない。
操作は既存 `KeyAction::GridReload`／同名メニューへ配送する。GridReloadは既定キーなし、
F5は★5であり、ここでのCtrl+F5は利用者の呼称／割当を指す。固定rawキー判定や
既定keymap変更は追加しない。F5をGridReloadへ割り当てた場合も同じ操作である。

### 2.2 状態と移行の組み合わせを減らす判断

LISTrev7のP2二件は、旧cacheの移行とcache-only経路、新payload keyとcatalogの
作成・掃除・失効の接続が未確定という指摘だった。保持保証を成立させるために
それらを追加するより、利用者が求めていない保証自体を外し、既存の表示／cache経路を使う。

| 案 | R8の採否と理由 |
| --- | --- |
| 通常prepareの完全一致時だけinstallを省く | 採用。現在の表示と既存要求がそのまま残り、一覧採用の判断一箇所で余分な再表示を防ぐ |
| 自動代表の選択キー保持、Stack用の保持対応キー、Exact／Discover要求、画像と選択付きpayloadの同時採用 | 不採用。選択と画像・要求・context・evictionの状態の組み合わせが増える。利用者の要件は更新遅延の許容であり、永続的な選択保証ではない |
| leaf付きpayload cache key、親lookupから新payloadへの参照、catalogの作成・掃除・失効の拡張 | 不採用。旧／新形式、共有cache、cache-only consumerと選択ownerの整合を新たに管理する必要が生じ、小さな一覧更新の修正がcache方式変更へ広がる |
| 選択先不明の旧cacheをmissにして移行／作り直し | 不採用。通常利用でcache作り直しの費用が出て、既存のcache-only候補をどう扱うかという組み合わせが増える。保持保証を撤回すれば移行は不要 |
| CPU画素の追加保持、context別の画素cache複製、新pending owner | 不採用。メモリと寿命・所有の状態を増やす。現在のthumbnail／cache予算を維持する |

現行cacheのkey・payload・形式・lookup、旧cacheのhit／miss、cache-onlyでの代表取得、
catalogの作成・掃除・失効は従来の契約のままにする。新形式への強制miss、全件移行、
代表を得るためだけの新規書庫openを追加しない。既存cache内の選定proof等の運用も変更せず、
本修正のための候補依存provenanceや自己操作operation IDの照合は復活させない。

## 3. 通知から走査・準備・比較・採用まで

### 3.1 更新を省かない入口

既存 `poll_current_folder_watch` → `check_external_folder_changes(Notified)` →
`start_external_rescan` → worker走査 → 表示準備 → 採用境界を使う。
**Notifiedで更新を検知したら、走査と一覧準備はいつもどおり実行**する。
raw signature、操作path、operation IDを根拠にprepareを飛ばさない。
復帰だけが契機のResumedは既存の安いmtimeふるいを維持し、Notifiedではふるいをbypassする。

Deleteの進捗modalは維持する。modal中に受け取った通知はdirtyに残し、終端後に準備へ配送する。
Delete終端自身も同じ通常更新を要求するため、通知が来なかった場合にも非表示候補を再投影する。
既存rescan ownerが同じ更新を準備中なら既存の再予約へ集約する。独自pendingやoperation IDを足さない。
順序が通知先着でもDelete終端先着でも、準備結果とその時点の表示の比較へ合流する。

種類採否、同名候補除去、動画派生判定、音声／動画補助画像、本判定、Stack、sort、
facet／rating、表示metadata・省略件数を既存の表示準備経路で作る。
必要な走査／DB read／sidecar readはworkerで行い、UIに新しい同期I/Oを追加しない。
初期producerをprovenance取得のために再設計する範囲は撤回する。
startup-diagnosticsのPhase B保留を今回の計画だけで完了扱いにしない。

### 3.2 比較する表示投影

hashだけの一致やGPU pixelの全件readbackで判定しない。
既存prepare ownerのworkerが、prepared resultと捕捉した採用表示投影の以下の値をexactに比較する。
GPU texture／CPU画素そのものを比較せず、既存の行・表示状態・更新要否を使う。
代表の保持選択キー・選定依存provenance／全件decodeは不要。
比較はviewport内のセルだけに限らず、一覧の全行と投影結果を対象にする。
画面外の行・順序が変わる結果を、今見える部分が同じという理由で省かない。

| 比較対象 | 一致に必要な値 |
| --- | --- |
| 行と表示投影 | context／sourceを含む既存の論理行key・種類・表示名、同名候補の除去、画像本判定、Stackの既存group key・代表・member構成・件数、facet／ratingによる可視性。新しい保持対応キーは作らない |
| 並び・表示layout | 実際の行順、sort条件／方向、採用済みsort metadata、画像比率等の一覧layoutに使う値 |
| 表示metadata | 更新日時・size・rating・tag／badge・pin表示など、画面／tooltipへ使う採用値。source stampだけで代用しない |
| サムネイルの表示状態（代表を含む） | 既存の寸法・layout／描画条件に影響する値、Pending／Loaded／NoArt／Failed等の採用表示状態と、通常のcache失効／内容更新による更新要否。再要求が必要なら差分にする。既存値で一致を確認できない場合も通常採用へ流し、保持選択キーや画像の由来証明は足さない |
| 通常画像等の内容更新 | Content通知で更新が必要な通常画像／ページは既存のthumbnail更新状態をprepared結果に含める。秒stampが同じでも古いLoadedとの一致にせず、既存cache失効／source要求へ配送し、同じcoarse cacheの読戻しで内容更新を落とさない |
| 一覧の件数表示 | 表示／総件数、省略chipとその内訳など、利用者に出す値。OS system件数だけの内部差分と表示差分を区別 |

省略chipの表示値が変わる場合も「完全一致」ではなく通常採用する。
今回、新しいchip-only採用方式や独自partial row installは追加しない。
内部の観測stamp／watch sequenceは見た目の値ではないので比較キーへ混ぜない。
新しい一覧の初期Pendingと現在のLoadedを機械的に比較して常に差分とするのではなく、
通常準備で判定した同じ行の既存表示状態・更新要否を投影する。一致時には現在の資源をそのまま使う。
一致を確認できない状態を旧Loadedへ黙って合わせることもせず、通常採用へ流す。

§1.314の実Folder mtime-onlyでは、観測値だけを進め、表示metadata・sort・代表要求stampは
旧採用値を準備結果へ使う。この例外を通常ファイル・ZIP/PDF本体へ広げない。
§2の更新遅延の許容を、ファイルのsize／mtime・件数等の表示更新を省く理由にしない。

### 3.3 worker比較と採用直前の有効性確認

UIには **同一／差分の判定と、差分の場合だけprepared採用結果** を返す。
全行の比較・比較用投影の組立て・hash計算・一覧cloneをUI poll／採用直前に置かない。
UIは元context・source・items generation・設定条件と、捕捉した表示投影revisionの有効性だけを確認する。
worker比較中にDelete成功行除去、sort／facet／metadata、thumbnail初回採用・寸法・更新要否などの
比較値が変われば、旧表示に対する「同一」も「差分」も既存stale拒否／再準備へ戻す。
拒否結果のdirty／cutoffは照合済みにしない。選択／check／scrollは比較値へ含めず、差分採用時に
その時点の状態を既存ownerで復元する。古いworker snapshotで利用者の操作を戻さない。

- 完全一致: 観測signature／親mtime／取り込んだ通知cutoffだけを同じownerで進める。
  items／世代／texture／表示metadata／sort／選択／check／scroll／AI・PDF保持画像を触らない。
  この採用に伴うサムネイル再要求も0回。同じ結果を再準備しても再表示0回。
  既存ownerによる進行中の画像採用・eviction／idle upgradeは従来どおり続ける。
- 一つでも表示値が違う／不明: prepared resultを既存一覧採用へ一回渡す。
  代表の選択保持は行わず、サムネイルも既存の通常採用・選定／cache規則を使う。
  必要な更新をした後、後続の同じ準備結果は新しい現在表示と一致し再表示を省く。
- 準備失敗・cancel・context不一致: 元表示を維持し、未照合dirtyは既存ownerに残す。
  空一覧を成功結果と偽らず、兄弟contextの購読・結果・ackを変更しない。
  I/O失敗はログ・通知して次の通常契機へ戻し、同じdirtyで毎フレームretryする仕組みは作らない。

新しいcomparison pending／二つ目の表示owner／初回専用installを作らない。
選択やscrollは準備結果が書き換える表示内容ではなく、既存採用ownerの復元対象とする。

### 3.4 コード照合と必要な共有投影の基盤

R6でコード照合した比較／共有投影の基盤を維持する。以下の変更は新方式に必要だが、今回未実装。
R7で追加した実選択付きproducer／cache／source契約はR8の実装前提から外す。

| 現コードの境界 | R8で既存owner内に必要な変更 |
| --- | --- |
| `ExternalRescanPending` はcontext・items_generation・scan_options・cancel・restartを持ち、workerはScannedDir／signatureと既存refresh causeを返す | 同じrescan／prepare ownerで走査→表示準備→比較まで行い、typed結果を同一／差分にする。別のcomparison pending／worker ownerは足さず、§1.373の独立したcauseも維持する |
| `App::prepare_scanned_folder_listing[_with_cache_keys]` は `&mut self` からmaterialize／DB read等を行い、PreparedScannedFolderListing.itemsとApp.itemsはVec | workerで使う設定・DB reader・分類／materializeをこの既存準備境界へ分離する。現関数が既に全準備workerであるとは扱わない。既存Stack／sidecar／page-edit準備の終端と合流してから比較する |
| `page_identity_index: Arc<PreparedPageIdentityIndex>`、StackView／SubfolderExpansionSnapshotは共有immutable値を持つが、全表示値を覆うsnapshotはない | 採用する行順・metadata・件数・既存thumbnail表示状態／更新要否のimmutable投影を準備workerで生成し、元ViewerContextBundle／既存一覧ownerに保持する。要求時はArc等の参照を安価に渡す。items全件のcloneでsnapshotを作らない |
| `items_generation` と `page_edit_revision`、PAGE_EDIT_WRITESのWriteStamp、rating_session_write_generation、Stack request sequence／設定照合がある | 既存の世代・revision／write stampを比較要求と結果の受理条件に使う。items_generationだけでは世代内thumbnail採用やmetadata変化を捕捉できないため、採用投影の参照identityも同ownerで更新し、有効性を照合する |

immutable投影は一覧全体の複製をUIで作るための別ownerではない。workerが作った採用値をそのまま
共有し、UIの単一行採用・除去等は変更した行／ブロックだけを更新し、未変更部分を共有する。
root参照identityをrevisionとして使い、全件cloneを伴う `Arc::make_mut(Vec)` や
毎フレームの全行snapshot再構築を行わない。この部分共有の基盤も現コードにあるとは主張しない。
その基盤が揃わなければ「UIで比較する」「UIで全件cloneする」形で接続せず、実装前提不足として止める。
thumbnail画素やegui textureを投影に保持・workerへ移す必要はない。

`ThumbPixels`／LoadRequestの選択先field、leaf付きcache namespace、catalog lookup参照・
移行／cleanup／失効の新接続は不要。cache-onlyの旧画像からleafを復元する基盤も要求しない。
Folder／書庫／PDF／Stack／動画・音声の既存通常準備とthumbnail表示状態を比較へ投影し、
cache hit・miss／source生成・RAW継続・drive seed・Remote／benchは既存契約を維持する。
画像／保存両終端やrequest IDの相関を変更せず、選定保証のためのproducer再設計はしない。

簡素化としてDeleteの既存modalと既存再予約を維持する。比較中の全一覧操作を新たにmodalで止める
方法は、通常の監視更新で入力を制限するため採用しない。既存の世代／revisionによる拒否と通常再準備を
用い、rollback・時間窓・operation自己ID・provenance・専用のcompletion待ちを追加しない。

## 4. 購読・context・通知契約の維持

R3のcontextごとの所有契約を維持する。App-global watcherを同folderのA/Bが共有して
ackする構造にはしない。既存ViewerContextBundleに、physical-listの購読／dirty／
external rescanと観測signatureを揃える。新しい単独pending ownerは作らない。

購読identityはcontext ID・subscription epoch・exact folder・listing options。
OS登録／初期scanは既存のsource準備worker内で、購読開始後にscanする。
購読前pre_scanを新購読のcutoff証明にせず、owner／cancel／dropを既存要求に載せる。
同folderのA/Bにも各自の購読を持ち、共有fanout／ack台帳は作らない。
本棚／ZIP／PDF／合成rootの既存購読範囲は変更しない。

| lifecycle | 維持する契約 |
| --- | --- |
| park／mount | 元bundleに購読・dirtyを残す。park中の通知も元identityへ記録し、再mountで走査・準備する |
| scan中の切替／世代変更 | 元ownerの結果を新contextへ採用しない。拒否した結果のcutoffはackしない |
| source終了／context破棄 | 元購読・workerと共有投影を解放。兄弟のdirty・表示・requestを触らない |
| 走査中の次通知 | restart予約とcutoff後のdirtyを残す。次の入力／新通知なしでも通常の再準備へ進む |
| coverage不明／watch error | mtimeふるいだけで無変更としない。通常の全走査・準備へ渡し、既定のログ・通知で扱う |

notifyのkind／pathをsaw_changeだけに潰さない。既存の購読dirty値にsequenceと対象path、
Nameのold/new、Content、typed directory、Unknownを有界に集約し、採用成功時だけcutoff以下を消費する。
後から同pathへ届いた通知を先の通知と一緒に消さない。時間TTLは追加しない。

| kind／対象 | 走査・準備と表示の扱い |
| --- | --- |
| Create／Remove／Modify(Name) | Namespace。全行投影を準備し現在表示と比較。Nameを一律Contentにしない |
| ファイルModify(Data/Metadata/Any) | 内容・属性変更を否定しない。通常のprepare・既存更新へ渡し、更新要否を表示比較へ含める。代表を含む画像更新も既存cache／要求規則に従い、raw一致だけで落とさない |
| 確定した実directoryのmtime-only | §1.314を維持し観測stampだけ進める。Deleteと混在しても親の表示値へmtimeを漏らさない |
| path不明／型不明／Other／watch error／overflow | Unknown。全走査・準備し、材料不足なら通常採用。raw一致を理由に更新を落とさない |
| Access | 既存どおり一覧mutation対象外 |

Unknownを「準備不要」にせず、既存更新が必要／一致を確認できない状態なら通常採用する。
通知を自己由来と認定しないので、外部Bの上書きや外部Cの追加がDeleteと同batchでも通常準備される。

## 5. 同型経路と今回の範囲

| 経路 | 扱い |
| --- | --- |
| 通常FolderのDelete／メニュー削除 | ごみ箱・完全削除の成功path除去後、同じ通常prepare・比較へ配送。Shellの成功判定は変更しない |
| ZIP/PDF本体の物理削除 | 通常の実file削除。行消失とcache／metadata purgeは既存workerを維持 |
| ZIP内画像／ZipDir／PDFページの削除 | 実fileのdrag sourceがなく既存の理由通知で拒否。書庫を書き換える削除を新設しない |
| 検索結果 | 実source削除は既存path再探索へ。検索rootの再クエリ／索引ownerは別の正当な更新であり、物理Folder比較で抑止しない |
| Collection root | 参照解除であり物理Deleteではない。actorのrevisionを握りつぶさない |
| Collection等の物理子 | 元のphysical-list ownerで比較。子の差分は親actor／source invalidationを従来どおり通知 |
| 改名／move／copy／D&D／Shell paste | 第一段の操作受付・終端は変更しない。後続watchの通常prepareが比較境界を通る範囲は共通。操作終端からの二重reload集約の完了は別段 |
| 本フォルダ | watcher購読は対象外でもResumed／先着rescanはある。Appendの自動観測配送は§1.373 R4案1を維持 |

第一段の受入対象はDeleteの余分な再表示。rename/move/copyを完了扱いにしない。
今回の比較は全操作にoperation IDを配管する前提を持たず、第一段のために本Undoへ変更を加えない。
§1.373の「同Append IDの自己refreshだけ失効除外」は独立した契約として残す。

### 5.1 不採用部品の撤去

未接続の `app/folder_reconcile.rs` を撤去する。DeleteReceipt／FileOperationId、
AppliedFolderListing／NeedsProjection、raw成功差分の減算、projection証拠、
代表provenance準備、未接続の新watch subscription helper／専用準備workerは使わない。
そのためだけに `folder_scan.rs` へ追加したRawFolderSnapshot／ObservedEntryStamp、
ScannedDirのraw／prepared payload、関連fixtureのfieldも外す。
既存の分類済みscan、AuxiliaryMediaCandidates、同名filter、sort・metadata、§1.314 signature、
音声inventory、表示用省略件数は維持する。製品へ接続済みのwatcherを削除する変更ではない。

旧部品の17件成功は旧方式の記録で、新方針の受入証拠に流用しない。
R5で未接続部品を撤去済み。R6〜R8は設計・manualだけの変更で、新方式の実装は行わない。

## 6. 実装と独立レビューの受入条件

実装予定範囲はappのexternal rescan要求／準備／採用、folder_scanの既存表示準備、
ViewerContextBundleの購読所有と共有表示投影、実Delete handlerとその回帰。
代表保持、thumbnail payload／要求mode／cache形式の変更・移行、catalogの新しい掃除／作成／失効接続、
新source provenance／cache proof DB／operation receiptは範囲外。
共有ファイルは一人のwriterが所有する。display-pipeline、thumbnail-request-lifecycle、
ui-responsiveness、architectureも実装時に該当契約を同時更新する。

追加予定回帰:

- Delete成功後とprepared行・投影・並び・metadata・既存thumbnail表示状態が完全一致:
  install／世代変更／そのinstall起因のサムネイル再要求・texture再uploadが0回。
  通知先着／終端先着、二度目の同じ通知でも同じ。現在のtextureと既存要求ownerが残る。
- JPG優先の同名PNG再露出、Folder/ZIP、PDF/EPUB、動画／音声sidecar、Stack member・facet、
  sort・badge・省略chipの表示差分は通常採用する。raw一致だけで省かない。
- Deleteと残存Bの内容上書き／外部C追加が同batch。ファイルmtimeが同秒・size同じでも、
  Content／Unknownを通常prepareへ渡し、既存の更新要否を比較へ含める。
  内容更新を否定できない状態を「完全一致」と偽らない。
- 一致によるinstall省略中は表示中の代表が残る。差分install、CPU／GPU eviction後の再読込、
  idle upgrade、サイズ変更、通常cache／sourceの再生成では通常の代表へ変わってよい。
  固定leafや選択付きpayloadを要求せず、CPU画素保持・全件事前探索・decodeを増やさない。
- 同folderのA/Bで、Bの明示GridReloadによる共有cache更新がAの一致採用の権限／dirtyを消さない。
  Aは一覧を入れ直さなければ表示中のtextureを維持するが、その後の通常再読込で代表が変わってよい。
  初回cache hit、cache-only、idle upgrade、RAW継続・遅着／保存終端は既存request相関を維持する。
  cache形式変更・旧cache強制miss・新payloadや移行／cleanup workerがないことを確認する。
- Stack代表削除後に2枚残る場合と1枚になりImage化する場合を対で検証。
  通常materializeが作った代表・member構成・件数・種類の差分を採用し、保持対応mapを作らない。
- 手動pin／解除・pin先消失時の既存fallback、編集、NoArt／Failed、初回Pending／要求中の対照。
  pin設定を解除せず、thumbnailの画像／保存両終端・取消・keep_range／upload予算は不変。
- §1.314の子.partによるmtime-onlyとDelete混在で親の追加install0回。表示日時・sort・
  代表要求stamp不変。ZIP/PDF／普通fileのmetadata変化をこの例外へ広げない。
- 同folderのA/B、A→B→A、park中内容上書き、先着結果／cutoff後通知、cancel／close／
  subscription再作成。Aの一致判定でBのdirty・表示・要求を消さない。
- worker比較中と採用直前の表示世代／revision変更のhandler回帰。Delete・sort／facet／metadata・
  thumbnail初回採用／寸法変更・既存更新要否の変化で同一／差分の両結果を拒否し、
  dirtyを残して通常再準備する。UI全行比較／全件cloneが0回であること、
  選択／check／scrollを旧snapshotで戻さないことも確認する。
  GridReloadのAction／メニュー／割当キーは従来の明示更新へ届き、自動比較で省略しない。

大量一覧の性能検証予定:

- 1万／10万行のsynthetic prepared投影で同一、末尾1行差分、全行差分、比較中revision変更を測定する。
  画面外差分も検出し、全行exact比較の仕事量がworker側にあることを確認する。
- UI enqueue／poll／有効性確認の時間・最大frame時間、workerのscan／prepare／compare時間、
  比較回数・再準備回数・install回数、snapshot共有／変更部分の割当量、CPU画像／GPU使用量を記録する。
  同一時のUI時間が行数比例で増えないこと、UI全件clone／snapshot再構築0回を受入条件とする。
- 既存の大規模一覧perfイベントと同じ行数／cold・warm条件で前後比較し、入力応答・scroll、
  eviction後の再要求・idle／trayのCPU、keep_range／upload予算が退行しないことを確認する。
  A/B切替・長い一覧巡回時の共有投影の割当量も記録する。全件の事前探索・decode0回、
  cache形式変更・移行起因の作り直し0回、既存cache予算不変を確認する。
  通常利用の目立つ遅延があれば実装を止めて設計担当へ相談する。
  headless／fake-inputを先に行い、アプリ計測は具体的suiteの利用者承認を得た別の検証枠で行う。
  今回は計測・試験を実行せず、R5の撤去検証をこの受入証拠へ流用しない。

**ここで止め、R8の独立設計再レビュー後に新方式の実装へ進む。**
まれな失敗は割り切り (ログ + 通知 [+ 作り直し])。新しいretry／rollback／回復journalは作らない。
今回の新方式の実装・回帰追加0件。R8では文書検証だけを行い、ビルド・テスト実行・性能計測・
アプリ起動・コミットはしない。§1.373 R7の既存実装は変更しない。

以下の§6.1〜6.3はR5〜R7当時の履歴であり、撤回した代表保持の仕様やR8の実行証拠ではない。

### 6.1 未接続部品撤去の検証記録 (2026-10-13)

- 旧folder_reconcile専用の17件を部品とともに削除。新方式のテスト追加0件・実装0件。
- targeted: folder_scan 45件、folder_mtime 6件、similar_navigation 33件、全件成功。
  合計84実行、重複除外83件、失敗0件。
- cargo fmt --all -- --check、通常／portable core cargo check、git diff --checkが成功。
  manualのHTML解析2ファイルと、変更文書・manualのUTF-8確認も成功。
- 初回の標準環境でのtargeted compileはTurboJPEGのnative buildでexit101、テスト実行0件。
  §1.353で構築済みの同じpinned static/SIMD依存と既存
  `target/nextlist-verify-env.ps1`を再利用して再実行した。依存版・Cargo設定は変更していない。
- test-full／build-dev／build-releaseは実行0回。新方式は設計レビュー前であり、
  今回は未接続部品の撤去だけを検証した。アプリ起動0回・コミット0件。

ログ: [folder_scan 45件](../target/section1375-r5-cleanup/folder_scan.log)、
[folder_mtime 6件](../target/section1375-r5-cleanup/folder_mtime.log)、
[similar_navigation 33件](../target/section1375-r5-cleanup/similar_navigation.log)、
[fmt](../target/section1375-r5-cleanup/fmt.log)、
[通常core check](../target/section1375-r5-cleanup/core-check.log)、
[portable core check](../target/section1375-r5-cleanup/portable-check.log)。
これらをR8の新しい表示比較方式の受入証拠とは扱わない。

### 6.2 R6文書改訂の検証記録 (2026-10-13)

- 指定4文書のUTF-8 strict decodeと、manual 2ファイルのHTMLParser解析が成功。
- 指定4文書の `git diff --check` が成功。
- コードは読取り照合のみ。新方式の実装・回帰追加・ビルド・テスト実行・性能計測・
  アプリ起動・コミットは0件。§6.1はR5撤去時の履歴であり、R6の実行結果ではない。
- R6の独立設計レビューと、§3.4の共有投影基盤・worker準備分離の実装は未実施。

### 6.3 R7文書改訂の検証記録 (2026-10-13)

- 今回の変更はこの設計書とmanualのgrid／shortcuts、計3ファイルだけ。
  UTF-8 strict decode 3ファイル、HTMLParser解析2ファイル、対象3ファイルの
  `git diff --check`が成功。設計の節順と、manual両方の画面外復帰／別窓更新／消失例外を照合した。
- §1.373 R7に属する既存13ファイルは、作業開始時のSHA-256とすべて一致。
  既存作業内容は`4c0567b23`へコミット済みだが、この文書改訂ではコミットしていない。
- 新方式の実装・回帰追加・cargo build／test・性能計測・アプリ起動・コミットは0件。
  R7の独立設計再レビューは未実施。§6.1の過去の試験結果をR7の受入証拠にしない。

### 6.4 R8文書改訂の検証記録 (2026-10-13)

- 変更はこの設計書とmanualのgrid／shortcuts、計3ファイルだけ。
  UTF-8 strict decode 3ファイル、HTMLParser解析2ファイル、対象3ファイルの
  `git diff --check`が成功。撤回した代表保持保証・消失例外が残っていないことと、
  通常の再読込／高画質化で代表が変わり得る説明、既存の手動固定規則の整合を確認した。
- コード・§1.373設計書・README・backlogは作業基点から変更なし。
  実装・回帰追加・cargo build／test・性能計測・アプリ起動・コミットは0件。
- この文書提出時点ではR8の独立設計再レビューは未実施だった。その後LISTrev10でACCEPT。
  §6.1〜6.3の過去の検証記録をR8の受入証拠にしない。

## 7. 実装前提照合: 初回採用入力を共有する境界 (2026-10-13)

LISTrev10のACCEPT後、§3.4を実装するために通常の初回表示とhydrationを照合した。
比較方式・代表保持の撤回を差し戻すものではなく、**最初の採用表示をworkerへ共有する
具体的な所有境界**が未確定であることを記録する。ここを決める前に、表示時期を変更したり
UIで全一覧を複製したりして接続しない。

### 7.1 現コードで確定した順序

通常の実Folderは `load_folder_with_scan_claimed` →
`prepare_scanned_folder_listing_with_cache_keys` → 可視採用 →
`install_prepared_scanned_folder_listing` → `hydrate_installed_main_list` →
`begin_sidecar_restore` → page identity準備 → 必要なsidecar取込 →
`resume_loading_items_after_sidecar` の順である。

- 通常prepareはUI側の `&mut App` でmaterialize／rating read／pin read等を行い、
  itemsとmetadataのVecを返す。`install_loading_items_inner` がこれらをAppへ移す。
- `begin_sidecar_restore` の既存 `PreparingPageIndex` workerは、採用後の
  `self.items.clone()` を入力とする。これは行と順序だけであり、採用済み
  image／sort metadataや最終的なtag／rating／page-edit表示値は含まない。
- sidecarの経路はreadonly準備だけではなく、flush／取込／cache更新を行う。
  取込前のDBを読むだけで一致とすると、必要な取込を省く可能性がある。
- 後からFS／DBを再読しても、先に実際に採用したmetadataの証明にはならない。
  新しく走査した値をそのまま「現在表示」とする初回bootstrapも不可である。

したがってpage-index workerに投影結果を一つ追加するだけでは、§3.2の完全一致を
満たせない。必要なのは、採用した値の共有入力と、既存hydration終端での確定である。

### 7.2 設計担当が確定する選択肢

| 選択肢 | 所有境界と費用 |
| --- | --- |
| 採用済み入力の共有storageを既存hydrationへ渡す | install→sidecar→表示の既存順序を維持する。行／採用metadataの共有入力を `SidecarLoadContinuation` に載せ、必要な取込後に同じhydration workerで最終投影を作る。世代内更新は§3.4の部分共有で扱う。どのstorageを正本にするか、既存Vec／slice consumerとの境界を先に確定する必要がある |
| 通常のsource準備を既存要求ownerのworker段へ移す | workerがprepared rowsと共有入力を生成してから既存可視採用へ渡す。通常startup／同期loadも含むcontinuation契約が必要で、現在の新しいPending行の表示時期が変わり得る。通常利用の待ち時間・表示変更は未承認なので、勝手に実装しない |

既存page-index worker用のUI item cloneを再利用する場合も、**追加cloneがないこと**と、
§3.4の「items全件のcloneでsnapshotを作らない」との整合を設計担当が明記する。
新しいmetadata全件cloneは採用しない。共有storage案でも、元の採用値を共有できなければ
この問題は解消しない。通常利用の表示順序を守れる共有storage案を先に具体化する。

初回だけ常に差分扱いにする案は、最初のDelete後の余分なinstallを残すので採らない。
sidecar有効時の一律比較省略、raw signatureだけの一致、UIの全件clone／比較も採らない。
新しいcomparison pending／初回専用install owner、代表保持・cache形式変更は追加しない。
本判断が済むまではstartup Phase Bの保留や§1.375の実装完了を宣言しない。

### 7.3 今回の到達点

- LISTrev10のP3に従い、README・backlog・ui-responsivenessの残ったR6の代表保持記述をR8へ同期。
  manualのgrid／shortcutsは既にR8の許容仕様であり、整合を確認した。
- 上記の初回入力境界について実装前に停止。未接続の抽出試作は製品コードへ残さない。
  §1.373のコード・設計・manualは変更しない。
- 新方式の実装回帰・性能計測・cargo test／check・build-dev・test-full・アプリ起動・コミットは0件。
  既存試験の成功記録を、この新方式の検証結果には流用しない。
- 今回の変更4文書のUTF-8 strict decode、既存manual 2ファイルのHTMLParser解析、
  `git diff --check` が成功。src／htdocs／scripts／Cargo設定のHEADとの差分は0件。
