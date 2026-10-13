# §1.375 監視更新の準備結果と現在の表示を比較して再表示を省く

2026-10-13、`next-list`、R7。R6コード照合基点 `918a51309`（R5の基点は `ae2772c8e`）。
**LISTrev6のP2一件を反映したR7。選択キーと画像の同時採用、leafを含むcache identityを具体化。独立設計再レビュー提出用・表示採用の新方式は未実装**。
今回のコード照合は`3e8178b31`と既存の§1.373 R7作業内容（`4c0567b23`と同一）の読取りだけで行い、§1.373の変更は行わない。
正本項目: [next-release-backlog.md §1.375](next-release-backlog.md#1375-フォルダで-delete-キーで削除すると正しく消えた一覧がもう一度丸ごと読み直されてちらつく--利用者の実機報告-2026-10-12)。

R3はACCEPTだったが、実装調査で初期表示と自動代表の採用証拠が対応しない前提不足を確認した。
2026-10-12の案B、R4のprovenance配管、operation IDによる自己操作receipt照合は、
**今回の利用者決定で採用を撤回**する。過去の設計は `ae2772c8e` の履歴を参照する。
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

## 2. 利用者決定: 監視更新では代表ファイルの選択を保つ

**利用者決定／再レビュー訂正 (2026-10-13)**: 監視更新では既に採用した代表ファイルの
選択キーだけを保つ。`Ctrl+F5`（「最新の情報に更新」）で自動代表を選び直す。
ただし、選択したファイル自体が削除・移動等で存在しなくなった場合は、監視更新でも再選定する。
設計担当判断: 消失時の再選定は「削除等をCtrl+F5まで反映しなくてもよい」という利用者の許容内の改善であり、消えた選択先を読みに行って失敗する状態を避けるため、例外を維持する。
画像の由来・選定依存・pin leaf・catalog revisionのprovenance照合は不要とする。

この変更は代表画像の表示に限る。行の追加・削除、同名候補の再露出、並び・表示metadata、
省略件数は通常どおり準備して反映する。画素を固定する仕様ではなく、同じ選択を既存cache／
要求経路で表示する。選択先の内容変更やcache再生成で画素が変わることは許す。
候補の追加・並べ替え・catalog変更だけでは別のファイルへ選び直さない。

対象は物理一覧のFolder／ZIP／PDF／変換済み書庫等の代表サムネイルと、同じ一覧に出る
動画・音声の表紙／補助画像、Stackの代表表示。動画の再生中フレームは固定しない。
通常画像・ZIP内の画像・PDFページ本体の内容更新を代表保持の対象へ広げない。
行自体がなくなれば選択キーを解放し、新しい行には通常の初回要求を行う。

手動pin設定／解除、編集プレビュー等の明示操作は既存の明示更新を維持する。
監視イベントをその明示操作と誤分類しない。Ctrl+F5で手動pin設定を解除する仕様ではない。
pin先が存在しない場合は監視更新でも既存規則でfallbackするが、保存されたpin設定は解除しない。

コード上の操作は既存 `KeyAction::GridReload`、メニュー「最新の情報に更新」と同じ境界へ配送する。
現基点のGridReloadは既定キーなし、F5は★5である。ここでのCtrl+F5は利用者の呼称／割当を
指し、固定rawキー判定や既定keymap変更を追加しない。F5をGridReloadへ割り当てた場合も同じ操作。

### 2.1 選択キーと既存要求の寿命

保持する値は、既存の代表要求ownerが実際に採用したファイル／書庫内entry／PDFページを
再要求できる選択キーだけ。選定理由・候補の依存関係・catalog proofは保持しない。
生存行への対応はcontext・source・種類を含む論理行identityで行い、別の行へ移さない。
別のrepresentative pending owner、操作receipt、provenance map、DB／disk journalは作らない。

- Loadedの既存textureは使える間そのまま使うが、画面外／trayのGPU・CPU evictionとcache予算を
  変更しない。`evict_grid_thumbnail` のCPU解放も維持し、追加のthumbnail画素保持を行わない。
  再表示は保持した選択キーを既存cache／source要求へ渡す。同じ選択先のcache更新は許すが、
  親Folderだけの共有cacheから別の選択先の画像を取り込まない（§2.3–2.4）。
- idle upgrade・サイズ変更・cache missも同じ選択を使う。sourceの存在確認／読出しは既存workerで
  行い、実ファイル／entry／ページの消失が確認された場合だけ通常の再選定へ戻す。
  単なる読込失敗を消失と見なさない。UIに同期stat／decodeを追加しない。
- 未要求Pendingと未選定の要求中は通常の初回選定を行い、採用された選択を同ownerへ残す。
  NoArt／Failedを永久固定せず、画像なし終端と要求の寿命は既存規則を維持する。
  先着結果は既存request ID／世代で採用・破棄し、終了を偽装しない。
- Ctrl+F5は保持選択を更新し、既存要求ownerから最新選定へ進む。手動pin／編集の明示操作は
  従来どおり反映する。行消失・source終了／context破棄でそのownerの選択を解放する。
  キー保持は通知反復で増えず、CPU画素・GPU textureを一覧全件分常駐させない。

R5のimmutable採用画素保持・eviction寿命変更案は撤回する。PC／Remote／catalog全体の
代表選定を固定する旧§1.314のB1案も復活させない。各contextは既存のsource／表示ownerを使う。

### 2.2 Stackの論理行identity

Stackの保持対応キーは **source／context + Stackグループkey + Stack種別** とする。
`GridItem::Stack { key, representative, count }` の `key` を使い、サブ展開では既存の
親フォルダscope付きgroup keyを使う。代表pathやmember件数はidentityへ含めず、表示値として比較する。
`src/grid_item.rs::perf_key` のStackは `stack::<representative path>` であり、代表path由来の
`ItemId` とともに、この保持対応キーには使わない。計装／要求用identityの用途と分離する。

同グループの代表を削除して2枚以上残ればStackは同じ論理行として存続し、消失例外で代表を再選定する。
一方、1枚になれば `StackView::materialize_aggregated` は通常Imageを作るため、旧Stackの選択は
引き継がない。Imageの通常要求へ戻る。同名keyの別source／兄弟contextへも保持選択を移さない。

### 2.3 R7: 選択キーは画像の実採用で確定する

現在の`ThumbPixels`（`src/thumb_loader.rs:567`）には画像・origin・寸法等だけがあり、
通常cache hit（同`load_one_cached`、2735行付近）も実際の選択先を配送しない。
`folder_thumb_cache_key`は親・sort・depth等を区別するが、選んだleafを含まない。
このままでは、Aがaを保持中にBの明示更新が同じcache rowをbの画像へ更新すると、
Aのeviction後に「保持キーa／表示画像b」を作れる。request IDだけの一致では防げない。

R7の不変条件は **採用表示の選択キーと、そのキーのsourceから得た画像を一組として採用し、
以後の要求も同じsourceに限定する**。選択キーは再要求できる正規のsource addressで、
実画像、書庫内entry、PDFページ、動画／音声の表紙等の種類も区別する。Folder再帰／pin／
変換済み書庫では、既存のsource address／alias解決に合わせ、親Folderの名前だけをleafにしない。
既存exact catalog代表を候補として読む場合も、実際に選んだsourceのaddressをresolverから返す。
通常画像等と代表要求の違いはtyped payload／既存request種別で表し、画像と選択キーを別channelや
独立したpendingへ分けない。以下の型名は概念名であり、実装時は既存型と揃える。

| 境界 | 必要な契約 |
| --- | --- |
| 要求作成 | 同じ既存thumbnail requestに`Discover`（初回／明示再選定）または`Exact(selected_leaf)`（保持中）を載せる。request ID・context・source・items generation・既存設定条件をそのまま捕捉する |
| source生成 | resolver／decoderが実際に読んだsource addressを画像・寸法と同じpayloadに載せ、同じrequest IDで返す。親要求の想定キーや、生成後に改めて選んだ候補で画像をラベル付けしない。Exactはleafを直接要求し、親の自動chooserを再実行しない |
| cache hit | 実際に読んだimmutable cache entryから画像・選択キー・寸法を一度に取り出す。decode後に共有mapを読み直してキーだけ取らない。Exactでは保持キーと一致するentryだけを使う |
| backlog／RAW継続 | 選択キーは`ThumbPixels`相当の画像payloadと一緒に既存ownerを流れる。RAW ticket／follow-upは解決済みsourceと同じrequest IDを保持し、親Folderへ戻って選定しない。preview／half／fullの品質差を別leafへの切替にしない |
| UI画像採用 | 既存のrequest相関・設定／世代・source受理判定とExactのキー一致を通った画像だけ、texture／寸法と選択キーを同じ採用処理で更新する。キーは既存bundleの採用表示row record（§3.4の共有投影）に置く。受信／enqueue／cache保存／Finalizedだけでは更新しない |
| 破棄・失効 | keep外破棄、cancel、stale／遅着、別contextへの誤配送、upload未採用は表示選択を変更しない。SourcePixelsの画像採用とFinalizedの保存終端は§1.360どおり別の終端であり、Finalizeは選択キーを更新しない |

代表のProvisionalPixelsを使う経路は、その画像の実選択が確定し、継続も同じleafである場合だけ
この契約へ参加する。検証前の別sourceに置換され得るseedを保持選択の根拠にしない。
ドライブの暫定seed等、この監視更新の対象外の経路は既存のtyped lifecycleを維持する。
NoArt／Failedで初回キーを捏造しない。保持中の単なる失敗でも別leafへ切り替えない。

選択先の消失をworkerで確認した場合は、現在のExact要求を既存規則で終端にし、同ownerから
新しいrequest IDのDiscoverを受け付ける。Exact(a)の途中でbを読んでaとして返すfallbackはしない。
明示GridReload／pin変更も既存の明示受付からDiscover／新しい指定先の要求へ進む。
新しい画像が採用されるまでは旧画像に随伴する旧採用キーを保ち、新しい結果を採用する時点で
一緒に置換する。evictionは画素／textureだけを解放し、採用rowの小さな選択キーは残す。
row消失・source終了／context破棄はそのrowを解放し、兄弟ownerのキーを変更しない。

### 2.4 R7: leafを含むcache identityで食い違う状態を減らす

先に次の簡素化を検討し、**実際の選択leafを含むcache keyを採用**する。

| 案 | 採否と理由 |
| --- | --- |
| 親・sort・depthだけのmutable cacheに選択metadataを足し、読戻しごとに照合 | 不採用。正しい照合とExact sourceへのmiss配送で防げるが、Bがbを保存するたびA用aが同じrowから失われ、eviction／upgradeで再生成が増える。「親cacheはb、保持はa」という状態と全consumerの個別guardを残す |
| source／描画条件のnamespaceに実selected leafを加えたpayload key | 採用。aとbの画像を別keyへ保存するので、Bのb保存がA用aを上書きする組合せがなくなる。同じleafの内容／品質更新は許し、既存cache予算・evictionを維持する |
| contextごとの画素保存・cache丸ごと複製・新しい代表pending owner | 不採用。画素保持の撤回と共有cache／既存ownerの契約に反し、選択と画素の独立した寿命を増やす |

payload keyは **既存のsource／描画条件namespace + selected leafの正規address**。
Stackのnamespaceはsource＋group key＋種類で、代表path由来perf_key／ItemIdを使わない。
contextは保持rowの所有identityへ含めるが、同じsource／leaf／描画条件のpayload cacheを
共有するためだけにcontext別の画像を複製しない。addressの種類と区切りを一意に符号化し、
basenameだけで別source・書庫entry・PDFページを衝突させない。

- producerは実際の画像payloadに付いた選択キーから保存keyを作る。要求したaのkeyへ実bの
  画像を書かない。catalog／in-memory entryには選択キーと画像・寸法を同じ更新で保存する。
  読戻しでもkeyのaddressとentryの選択キーが一致することを確認する。
- 初回のDiscoverにもcache hitを残す。既存の親・sort・depth等のlookup namespaceには、
  最新の選定結果の**leaf payload keyへの参照だけ**を既存catalog内に持たせる。画像本体を
  二重保存せず、別のcache proof DB／provenance map／ownerを作らない。
  workerは参照からleaf entryを解決し、そのentryの画像＋キーの一組を同request IDで返す。
  参照が途中でbへ替わっても、一度捕捉したaのpayloadをbのキーでラベル付けしない。
- 保持済みExact(a)は上記の「最新の選定」参照を読まず、aのpayload keyだけを使う。
  aのcacheがevict／missなら通常のExact(a) source要求へ進む。aが存在する限り、cache missや
  idle SourceOnlyを親chooserによるbへの再選定理由にしない。
- 旧parent cache等、**選択先を特定できない画像はR7代表のcache miss**とする。
  現在の候補を選び直して旧画素へそのキーを付けず、provenanceや画素照合で旧選択を推測しない。
  既存の画像／page等のキー自身がexact sourceを一意に表すcacheは、その既存address規則を
  明示して配送できる。単なるFolder・ZIP/PDF親keyをentry／pageの選択証明にはしない。
- 旧未知cacheの再生成は既存lazy需要に限り、初期全Folder探索・全件decode・UI I/Oを足さない。
  CacheOnly／既存archive候補の「新規書庫を開かない」制限を解除せず、miss／判定待ちの
  typed終端も維持する。既存cache-only候補がactual addressを返せる基盤も§3.4の必須条件とする。
  それを満たせず既存候補を黙って飛ばす／代表機能を削る実装には進まず、前提不足として報告する。
  一度の旧cache missに伴うcold化費用も性能検証に含め、通常利用の目立つ遅延は設計担当へ相談する。

これは「その画像の選択先」という小さなキーの結合だけであり、候補一覧、選定依存、catalog revision、
自己操作のoperation IDを照合する方式ではない。同じleafに対する共有cache更新は通常の
cache／source経路で表示してよい。CPU画素保持、cache予算やeviction方針の変更はしない。

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
GPU texture／CPU画素そのものを比較せず、代表選択と表示状態を使う。provenance／全件decodeは不要。
比較はviewport内のセルだけに限らず、一覧の全行と投影結果を対象にする。
画面外の行・順序が変わる結果を、今見える部分が同じという理由で省かない。

| 比較対象 | 一致に必要な値 |
| --- | --- |
| 行と表示投影 | context／sourceを含む論理行key・種類・表示名、同名候補の除去、画像本判定、Stackグループkeyとmember構成・件数、facet／ratingによる可視性 |
| 並び・表示layout | 実際の行順、sort条件／方向、採用済みsort metadata、画像比率等の一覧layoutに使う値 |
| 表示metadata | 更新日時・size・rating・tag／badge・pin表示など、画面／tooltipへ使う採用値。source stampだけで代用しない |
| 代表画像の状態 | 採用した選択キー・寸法・layoutに影響する値、Pending／Loaded／NoArt／Failed等の表示状態。通常のcache／内容更新で再要求が必要なら差分にする。GPU常駐有無を画素固定の証拠にしない |
| 通常画像等の内容更新 | Content通知で更新が必要な通常画像／ページは既存のthumbnail更新状態をprepared結果に含める。秒stampが同じでも古いLoadedとの一致にせず、既存cache失効／source要求へ配送し、同じcoarse cacheの読戻しで内容更新を落とさない |
| 一覧の件数表示 | 表示／総件数、省略chipとその内訳など、利用者に出す値。OS system件数だけの内部差分と表示差分を区別 |

省略chipの表示値が変わる場合も「完全一致」ではなく通常採用する。
今回、新しいchip-only採用方式や独自partial row installは追加しない。
内部の観測stamp／watch sequenceは見た目の値ではないので比較キーへ混ぜない。

§1.314の実Folder mtime-onlyでは、観測値だけを進め、表示metadata・sort・代表要求stampは
旧採用値を準備結果へ使う。この例外を通常ファイル・ZIP/PDF本体へ広げない。
代表選択の保持は2節の別の利用者決定であり、ファイルのsize／mtimeの表示更新を省かない。

### 3.3 worker比較と採用直前の有効性確認

UIには **同一／差分の判定と、差分の場合だけprepared採用結果** を返す。
全行の比較・比較用投影の組立て・hash計算・一覧cloneをUI poll／採用直前に置かない。
UIは元context・source・items generation・設定条件と、捕捉した表示投影revisionの有効性だけを確認する。
worker比較中にDelete成功行除去、sort／facet／metadata、thumbnail初回採用・寸法・選択先などの
比較値が変われば、旧表示に対する「同一」も「差分」も既存stale拒否／再準備へ戻す。
拒否結果のdirty／cutoffは照合済みにしない。選択／check／scrollは比較値へ含めず、差分採用時に
その時点の状態を既存ownerで復元する。古いworker snapshotで利用者の操作を戻さない。

- 完全一致: 観測signature／親mtime／取り込んだ通知cutoffだけを同じownerで進める。
  items／世代／texture／表示metadata／sort／選択／check／scroll／AI・PDF保持画像を触らない。
  同じ結果を再準備しても再表示0回。
- 一つでも表示値が違う／不明: prepared resultを既存一覧採用へ一回渡す。
  行入れ直しのときも、生存する代表行の選択キーを2節の契約で引き継ぐ。
  必要な更新をした後、後続の同じ準備結果は新しい現在表示と一致し再表示を省く。
- 準備失敗・cancel・context不一致: 元表示を維持し、未照合dirtyは既存ownerに残す。
  空一覧を成功結果と偽らず、兄弟contextの購読・結果・ackを変更しない。
  I/O失敗はログ・通知して次の通常契機へ戻し、同じdirtyで毎フレームretryする仕組みは作らない。

新しいcomparison pending／二つ目の表示owner／初回専用installを作らない。
選択やscrollは準備結果が書き換える表示内容ではなく、既存採用ownerの復元対象とする。

### 3.4 コード照合と必要な共有投影の基盤

R6基点`918a51309`と今回読取り照合した作業ツリーの現状を、R7の実装前提と区別する。
以下の基盤は新方式に必要だが、今回未実装。共有投影だけでなく、実選択を返す全producerと
cache／source consumerを接続してから、保持選択の採用を有効にする。

| 現コードの境界 | R7で既存owner内に必要な変更 |
| --- | --- |
| `ExternalRescanPending` はcontext・items_generation・scan_options・cancel・restartを持ち、workerはScannedDir／signatureのみ返す | 同じrescan／prepare ownerで走査→表示準備→比較まで行い、typed結果を同一／差分にする。別のcomparison pending／worker ownerは足さない |
| `App::prepare_scanned_folder_listing[_with_cache_keys]` は `&mut self` からmaterialize／DB read等を行い、PreparedScannedFolderListing.itemsとApp.itemsはVec | workerで使う設定・DB reader・分類／materializeをこの既存準備境界へ分離する。現関数が既に全準備workerであるとは扱わない。既存Stack／sidecar／page-edit準備の終端と合流してから比較する |
| `page_identity_index: Arc<PreparedPageIdentityIndex>`、StackView／SubfolderExpansionSnapshotは共有immutable値を持つが、全表示値を覆うsnapshotはない | 採用する行順・metadata・件数・代表選択のimmutable表示投影を準備workerで生成し、元ViewerContextBundle／既存一覧ownerに保持する。要求時はArc等の参照を安価に渡す。items全件のcloneでsnapshotを作らない |
| `items_generation` と `page_edit_revision`、PAGE_EDIT_WRITESのWriteStamp、rating_session_write_generation、Stack request sequence／設定照合がある | 既存の世代・revision／write stampを比較要求と結果の受理条件に使う。items_generationだけでは世代内thumbnail採用やmetadata変化を捕捉できないため、採用投影の参照identityも同ownerで更新し、有効性を照合する |
| `ThumbPixels`には実選択先がなく、source結果とcache hitは画像・origin・寸法等だけを返す | source resolverが実際に読んだleafのaddressを同じ画像payloadへ載せる。cache hitも一つのentryから画像・実選択キー・寸法を取り出す。同じ既存request IDで配送し、画像とキーの独立channel／pendingを作らない |
| Folder cache keyは親・sort・depth等で、選択leafを区別しない。既存entryだけからactual leafを特定できるとは限らない | §2.4のleaf付きpayload key／同entryの選択値へ接続する。Discoverの既存catalog lookupはpayloadへの参照だけにし、Exactはその参照を使わない。選択不明cacheはmiss、既存exact source keyから導出できる場合はaddress規則を明示する |
| `LoadRequest`／RAW follow-upは元Folder等の要求を保持し、idle upgrade／SourceOnlyでsourceを読み直す | 同じ要求ownerにDiscover／Exactを載せ、解決済みleafを継続へ渡す。cache miss・サイズ変更・RAW preview／half／fullで親chooserに戻らない。確認済み消失は旧要求を終端にして新request IDで再選定する |
| `poll_thumbnails`は既存request相関・backlog・upload予算・SourcePixels／Finalizedの終端を管理するが、採用選択を持たない | 既存の画像採用を唯一の確定境界とし、texture／寸法と実選択キーを原子的に採用表示rowへ反映する。Exact不一致・遅着・破棄・保存終端では選択を更新しない。request解放後もrowのキーだけ残し、兄弟bundleへ移さない |

immutable投影は一覧全体の複製をUIで作るための別ownerではない。workerが作った採用値をそのまま
共有し、UIの単一行採用・除去等は変更した行／ブロックだけを更新し、未変更部分を共有する。
root参照identityをrevisionとして使い、全件cloneを伴う `Arc::make_mut(Vec)` や
毎フレームの全行snapshot再構築を行わない。この部分共有の基盤も現コードにあるとは主張しない。
その基盤が揃わなければ「UIで比較する」「UIで全件cloneする」形で接続せず、実装前提不足として止める。
thumbnail画素やegui textureを投影に保持・workerへ移す必要はない。

実装前に接続確認するproducer／consumerは、Folderの直接画像・子Folder・pin、ZIP／PDF／
変換済み書庫の既存cache候補とsource生成、動画／音声のsidecar・埋込表紙、Stack、編集preview、
memory／catalog cache hit、cache-only miss、idle SourceOnly、RAWの各品質／follow-up、
backlog／画像採用／保存終端である。既存cache-only書庫候補もactual source addressを返す必要がある。
Remote／bench等の共有型consumerと対象外のdrive seedは既存の要求寿命を維持し、
UIのrequest ID照合をidxだけの配送へ緩めない。選択の分からない画像を暫定的に採用する接続、
既存候補を省く接続、新規書庫を開く接続では基盤不足を埋めない。

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
| source終了／context破棄 | 元購読・worker・保持選択と共有投影を解放。兄弟のdirty・代表・requestを触らない |
| 走査中の次通知 | restart予約とcutoff後のdirtyを残す。次の入力／新通知なしでも通常の再準備へ進む |
| coverage不明／watch error | mtimeふるいだけで無変更としない。通常の全走査・準備へ渡し、既定のログ・通知で扱う |

notifyのkind／pathをsaw_changeだけに潰さない。既存の購読dirty値にsequenceと対象path、
Nameのold/new、Content、typed directory、Unknownを有界に集約し、採用成功時だけcutoff以下を消費する。
後から同pathへ届いた通知を先の通知と一緒に消さない。時間TTLは追加しない。

| kind／対象 | 走査・準備と表示の扱い |
| --- | --- |
| Create／Remove／Modify(Name) | Namespace。全行投影を準備し現在表示と比較。Nameを一律Contentにしない |
| ファイルModify(Data/Metadata/Any) | 内容・属性変更を否定しない。通常画像等を既存更新へ渡す。代表選択は2節の仕様で保持し、選択先の内容更新は既存cache／要求経路で反映する |
| 確定した実directoryのmtime-only | §1.314を維持し観測stampだけ進める。Deleteと混在しても親の表示値へmtimeを漏らさない |
| path不明／型不明／Other／watch error／overflow | Unknown。全走査・準備し、材料不足なら通常採用。raw一致を理由に更新を落とさない |
| Access | 既存どおり一覧mutation対象外 |

Unknownでも2節の代表選択保持と消失例外は守る。Unknownを「準備不要」にする仕様ではない。
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
R5で未接続部品を撤去済み。R6／R7は設計・manualだけを変更し、比較／代表選択保持の新実装は行わない。

## 6. 実装と独立レビューの受入条件

実装予定範囲はappのexternal rescan要求／準備／採用、folder_scanの既存表示準備、
ViewerContextBundleの購読所有と共有表示投影、thumbnail代表選択キー／明示refresh、
実Delete handlerとその回帰。新source provenance／cache proof DB／operation receiptは範囲外。
共有ファイルは一人のwriterが所有する。display-pipeline、thumbnail-request-lifecycle、
ui-responsiveness、architectureも実装時に該当契約を同時更新する。

追加予定回帰:

- Delete成功後とprepared行・投影・並び・metadata・代表が完全一致: 追加install／世代変更／
  texture再upload0回。通知先着／終端先着、二度目の同じ通知でも同じ。
- JPG優先の同名PNG再露出、Folder/ZIP、PDF/EPUB、動画／音声sidecar、Stack member・facet、
  sort・badge・省略chipの表示差分: 通常採用が必要であり、raw一致だけで省かない。
- Deleteと残存Bの内容上書き／外部C追加が同batch。ファイルmtimeが同秒・size同じでも
  Content／Unknownを通常prepareへ渡し、通常画像の更新を落とさない。
- 選択先が存在する候補追加／並べ替え／cascade変更: watcherでは同じ選択キーを保持し、
  明示GridReloadで最新選定。選択先上書き・cache miss／再生成・GPU／CPU evict→再表示は
  同じ選択を既存要求へ渡し、古い画素保持を要求しない。
- 代表の選択先削除／移動・pin先消失: watcherでも消失例外で再選定／既存fallback。
  pin設定は解除せず、通常の内容更新・読込失敗を消失として扱わない。
- Stack代表削除後に2枚残る場合と1枚残る場合を対で検証。前者はsource／context＋group keyで
  同じStackに対応して消失例外の再選定、後者はImage化し旧Stack選択を引き継がない。
  代表path由来perf_key／ItemIdが変わる対照と、同名group keyの別source／兄弟contextも含む。
- 行が変わるwatch full install、NoArt／Failed、初回Pending／要求中、idle upgrade、
  サイズ変更、手動pin／編集の対照。初期全Folder探索0回、GPU keep_range／upload予算不変、
  選択キーは同じ行への通知反復で増えず、明示更新／dropで解放。CPU／GPU evictionと
  ThumbnailRequestsの保存／画像両終端・取消規則は不変。
- §1.314の子.partによるmtime-onlyとDelete混在で親の追加install0回。表示日時・sort・
  代表要求stamp不変。ZIP/PDF／普通fileのmetadata変化をこの例外へ広げない。
- 同folderのA/B、A→B→A、park中内容上書き、先着結果／cutoff後通知、cancel／close／
  subscription再作成。Aの一致判定でBのdirty・代表・要求を消さない。
- Aがaを採用済み、Bの明示GridReloadが同じsource／描画条件の共有cacheへbを保存するhandler回帰。
  異なる画素のfixtureで、AのCPU／GPU eviction後はaのleaf付きcache hit、aのcache miss時は
  Exact(a) sourceだけを使うことを確認する。保持キーa／画像bを作らず、Bの選択／cache・dirtyは不変。
- 初回Discoverのcache hitはentryの実選択キーと画像を同request IDで配送し、受信／backlog待ちでは
  キー未確定、実画像採用で両方確定。keep外破棄・保存Finalizedだけではキーを確定しない。
  旧parent cacheの選択不明画像を現在候補のキーでラベル付けせず、既存lazy miss終端へ流す。
  lookup参照捕捉後にBが参照をbへ変えても、捕捉したaの画像とキーが対応することを確認する。
- Exact(a)のidle upgrade・サイズ変更・RAW preview／half／full／follow-up中に、より優先されるbを
  追加する。cache／sourceの実選択はすべてaのままとし、parent chooser再実行0回を確認する。
  選択先a自体の内容／品質・寸法更新は反映し、CPU画素保持を増やさない。
- 同idxでも旧request ID／世代／contextの遅着、cache keyとentry addressの不一致、Exact(a)へ
  actual(b)を返す結果を実採用handlerで拒否する。拒否でキー／画像を別々に書き換えない。
  aの確認済み消失では新IDで再選定し、旧aのRAW結果／Finalizedが新選択を戻さないことも確認する。
- worker比較中と採用直前の表示世代／revision変更のhandler回帰。Delete・sort／facet／metadata・
  thumbnail初回採用／寸法変更で同一／差分の両結果を拒否し、dirtyを残して通常再準備する。
  UI全行比較／全件cloneが0回であること、選択／check／scrollを旧snapshotで戻さないことも確認する。
  GridReloadのAction／メニュー／割当キーが同じ明示更新へ届くことを確認する。

大量一覧の性能検証予定:

- 1万／10万行のsynthetic prepared投影で同一、末尾1行差分、全行差分、比較中revision変更を測定する。
  画面外差分も検出し、全行exact比較の仕事量がworker側にあることを確認する。
- UI enqueue／poll／有効性確認の時間・最大frame時間、workerのscan／prepare／compare時間、
  比較回数・再準備回数・install回数、snapshot共有／変更部分の割当量、CPU画像／GPU使用量を記録する。
  同一時のUI時間が行数比例で増えないこと、UI全件clone／snapshot再構築0回を受入条件とする。
- 既存の大規模一覧perfイベントと同じ行数／cold・warm条件で前後比較し、入力応答・scroll、
  eviction後の再要求・idle／trayのCPU、keep_range／upload予算が退行しないことを確認する。
  leaf付きcacheのlookup／hit／miss・source呼出し回数、旧選択不明cacheからの一度のcold化費用、
  A/B切替・長い一覧巡回時のcache bytes／選択キーbytesも記録する。全件の事前探索・decode0回、
  既存cache予算不変を確認し、通常利用の目立つ遅延があれば実装を止めて設計担当へ相談する。
  headless／fake-inputを先に行い、アプリ計測は具体的suiteの利用者承認を得た別の検証枠で行う。
  今回は計測・試験を実行せず、R5の撤去検証をこの受入証拠へ流用しない。

**ここで止め、R7の独立設計再レビュー後に新方式の実装へ進む。**
まれな失敗は割り切り (ログ + 通知 [+ 作り直し])。新しいretry／rollback／回復journalは作らない。
今回の新方式の実装・回帰追加0件。R7では文書検証だけを行い、ビルド・テスト実行・性能計測・
アプリ起動・コミットはしない。§1.373 R7の既存実装は変更しない。

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
これらを新しい表示比較・代表保持方式の受入証拠とは扱わない。

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
