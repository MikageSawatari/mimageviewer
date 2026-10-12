# §1.375 監視更新の準備結果と現在の表示を比較して再表示を省く

2026-10-13、`next-list`、確認基点 `ae2772c8e`。
**利用者の2026-10-13提案によるR5。独立設計レビュー提出用・表示採用の新方式は未実装**。
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

新しい不変条件は **通常の走査・表示準備を済ませ、採用直前の現在表示と完全に同じ場合だけ
一覧の入れ直しを省く**。自分の操作か外部操作かは判定条件にしない。
一致しない／比較材料が足りない場合は従来の一覧採用へ流す。
時間窓で通知を捨てず、準備結果が違う外部変更を取り込む。

## 2. 利用者決定: 監視更新では表示中の代表画像を保つ

**利用者決定 (2026-10-13)**: 削除・移動・内容変更などを監視で検知しても、既に表示した
代表画像は保つ。`Ctrl+F5`（「最新の情報に更新」）で初めて自動代表を選び直す。
画像の由来・選定依存・pin leaf・catalog revisionのprovenance照合は不要とする。

この変更は代表画像の表示に限る。行の追加・削除、同名候補の再露出、並び・表示metadata、
省略件数は通常どおり準備して反映する。代表の元画像が消えても、その代表行が残る間は
今の絵を使う。元画像の通常行は消え、実ファイルを開く処理も最新のsourceを扱う。

対象は物理一覧のFolder／ZIP／PDF／変換済み書庫等の代表サムネイルと、同じ一覧に出る
動画・音声の表紙／補助画像、Stackの代表表示。動画の再生中フレームは固定しない。
通常画像・ZIP内の画像・PDFページ本体の内容更新を代表保持の対象へ広げない。
行自体がなくなればその成果物を解放し、新しい行には通常の初回要求を行う。

手動pin設定／解除、編集プレビュー等の明示操作は既存の明示更新を維持する。
監視イベントをその明示操作と誤分類しない。Ctrl+F5で手動pin設定を解除する仕様ではない。
pin先が存在しない場合のfallbackも、明示更新時に既存規則で行う。

コード上の操作は既存 `KeyAction::GridReload`、メニュー「最新の情報に更新」と同じ境界へ配送する。
現基点のGridReloadは既定キーなし、F5は★5である。ここでのCtrl+F5は利用者の呼称／割当を
指し、固定rawキー判定や既定keymap変更を追加しない。F5をGridReloadへ割り当てた場合も同じ操作。

### 2.1 保持する成果物と遅延要求

保持するのは選ばれたsourceの証拠ではなく、**実際に採用したthumbnail画素と表示状態**。
既存 `ThumbnailState` の採用成果物とCPU画像／textureの所有へ組み込み、
別のrepresentative pending owner、操作receipt、provenance mapは作らない。
stable row keyで対応する生存行へ成果物を移す。pathだけではなく種類、archive prefix/page等を
含む既存row identityを使い、別の行へ移植しない。

- Loadedを保持する行は同じtexture・画素・寸法・表示originを使う。監視更新から新しい代表要求を
  起動しない。必要な行入れ直しでも、invalidateが採用成果物を捨てる前に同じownerで引き継ぐ。
- GPUの画面外evictionは維持する。再表示に使える採用済みthumbnail成果物をその行に残し、
  画面へ戻ったら同じ画素を再uploadする。変化したsource／共有catalogを読み直して
  別の代表を選ぶ経路には戻さない。既存CPU画素または既存cache画像の成果物を共有し、
  cache key／pathだけでなく実際に採用したimmutable画像を保持する。同じkeyのcacheが
  後から書き換わっても、保持画素を新しいcache画像へ差し替えない。
  元画像の全件保持、GPU texture全件常駐、provenance検証用I/Oは行わない。
- 保持費用は、既に採用した代表行ごとにthumbnail解像度の成果物一つ。履歴分を追加しない。
  同一行の監視更新を重ねても増えず、明示更新・行消失・source終了／context破棄で解放する。
  新しい代表用DB／disk journalは作らない。GPU keep_rangeとupload予算は変えない。
- 未要求のPendingはplaceholderを保持し、初めて表示範囲へ来たときの通常の遅延探索を許す。
  要求中でまだ採用画像がない行も既存request ownerで完了させる。「既に表示した代表」の
  再選定と、初回生成を混同しない。NoArt／Failedの既採用終端も監視では保持する。
- idle upgrade／サイズ変更で、監視を契機に別の代表へ選び直さない。保持画素の表示倍率変更や
  再uploadは許す。Ctrl+F5では保持成果物を解放し、既存要求ownerから最新の選定を再開する。
  先着済みupgrade／cache検証結果も保持中の代表を別画素へ上書きしない。
  既存ThumbnailRequestsのcancel／image破棄／終端規則を使い、要求の終了を偽装しない。

`evict_grid_thumbnail` は現在CPU画素も消すため、Loadedのcloneだけでは最後まで保てない。
この成果物の寿命変更も今回の独立レビュー対象であり、未実装。
row内の採用値を保つ方式で、PC／Remote／catalog全体の代表選定を固定する旧§1.314のB1案は
復活させない。Remote・他contextはそれぞれの既存source／表示ownerを使う。

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
prepared resultと現在の採用値から、以下の表示値をexactに比較する。
textureそのものは2節で引き継いだ同一成果物を使うので、sourceのprovenance／全件decodeは不要。
比較はviewport内のセルだけに限らず、一覧の全行と投影結果を対象にする。
画面外の行・順序が変わる結果を、今見える部分が同じという理由で省かない。

| 比較対象 | 一致に必要な値 |
| --- | --- |
| 行と表示投影 | stable row key・種類・表示名、同名候補の除去、画像本判定、Stack member構成・件数、facet／ratingによる可視性 |
| 並び・表示layout | 実際の行順、sort条件／方向、採用済みsort metadata、画像比率等の一覧layoutに使う値 |
| 表示metadata | 更新日時・size・rating・tag／badge・pin表示など、画面／tooltipへ使う採用値。source stampだけで代用しない |
| 代表画像の状態 | 保持したLoaded成果物・寸法・origin、Pending／NoArt／Failed等の表示状態。GPU非常駐でも同じ保持成果物として比較 |
| 通常画像等の内容更新 | Content通知で更新が必要な通常画像／ページは既存のthumbnail更新状態をprepared結果に含める。秒stampが同じでも古いLoadedとの一致にせず、既存cache失効／source要求へ配送し、同じcoarse cacheの読戻しで内容更新を落とさない |
| 一覧の件数表示 | 表示／総件数、省略chipとその内訳など、利用者に出す値。OS system件数だけの内部差分と表示差分を区別 |

省略chipの表示値が変わる場合も「完全一致」ではなく通常採用する。
今回、新しいchip-only採用方式や独自partial row installは追加しない。
内部の観測stamp／watch sequenceは見た目の値ではないので比較キーへ混ぜない。

§1.314の実Folder mtime-onlyでは、観測値だけを進め、表示metadata・sort・代表要求stampは
旧採用値を準備結果へ使う。この例外を通常ファイル・ZIP/PDF本体へ広げない。
代表成果物の保持は2節の別の利用者決定であり、ファイルのsize／mtimeの表示更新を省かない。

### 3.3 採用直前の比較と結果

結果は元のcontext・source・items generation・設定／metadata条件が有効な場合だけ採用する。
worker開始時の旧画面ではなく、**採用直前の現在表示**と比べる。
その間のDelete成功行除去、選択変更、thumbnail初回採用も現在の採用値を使う。
準備条件が変わった結果は既存stale拒否／再準備へ戻し、dirtyを照合済みにしない。

- 完全一致: 観測signature／親mtime／取り込んだ通知cutoffだけを同じownerで進める。
  items／世代／texture／表示metadata／sort／選択／check／scroll／AI・PDF保持画像を触らない。
  同じ結果を再準備しても再表示0回。
- 一つでも表示値が違う／不明: prepared resultを既存一覧採用へ一回渡す。
  行入れ直しのときも、生存する代表行の成果物を2節の契約で引き継ぐ。
  必要な更新をした後、後続の同じ準備結果は新しい現在表示と一致し再表示を省く。
- 準備失敗・cancel・context不一致: 元表示を維持し、未照合dirtyは既存ownerに残す。
  空一覧を成功結果と偽らず、兄弟contextの購読・結果・ackを変更しない。
  I/O失敗はログ・通知して次の通常契機へ戻し、同じdirtyで毎フレームretryする仕組みは作らない。

新しいcomparison pending／二つ目の表示owner／初回専用installを作らない。
選択やscrollは準備結果が書き換える表示内容ではなく、既存採用ownerの復元対象とする。

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
| source終了／context破棄 | 元購読・worker・保持代表を解放。兄弟のdirty・代表・requestを触らない |
| 走査中の次通知 | restart予約とcutoff後のdirtyを残す。次の入力／新通知なしでも通常の再準備へ進む |
| coverage不明／watch error | mtimeふるいだけで無変更としない。通常の全走査・準備へ渡し、既定のログ・通知で扱う |

notifyのkind／pathをsaw_changeだけに潰さない。既存の購読dirty値にsequenceと対象path、
Nameのold/new、Content、typed directory、Unknownを有界に集約し、採用成功時だけcutoff以下を消費する。
後から同pathへ届いた通知を先の通知と一緒に消さない。時間TTLは追加しない。

| kind／対象 | 走査・準備と表示の扱い |
| --- | --- |
| Create／Remove／Modify(Name) | Namespace。全行投影を準備し現在表示と比較。Nameを一律Contentにしない |
| ファイルModify(Data/Metadata/Any) | 内容・属性変更を否定しない。通常画像等を既存更新へ渡す。代表画像だけは2節の仕様で保持する |
| 確定した実directoryのmtime-only | §1.314を維持し観測stampだけ進める。Deleteと混在しても親の表示値へmtimeを漏らさない |
| path不明／型不明／Other／watch error／overflow | Unknown。全走査・準備し、材料不足なら通常採用。raw一致を理由に更新を落とさない |
| Access | 既存どおり一覧mutation対象外 |

Unknownでも2節の代表保持は守る。Unknownを「準備不要」にする仕様ではない。
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
本turnでは未接続部品の撤去と設計・manualだけを変更し、比較／代表保持の新実装は行わない。

## 6. 実装と独立レビューの受入条件

実装予定範囲はappのexternal rescan要求／準備／採用、folder_scanの既存表示準備、
ViewerContextBundleの購読所有、thumbnail採用成果物／eviction／明示refresh、
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
- 代表の元画像削除・移動・上書き、pin先消失／cascade変更: watcherでは旧成果物を保持し、
  明示GridReloadでは最新選定／既存pin fallback。GPU evict→再表示でも旧画素を使う。
- 行が変わるwatch full install、NoArt／Failed、初回Pending／要求中、idle upgrade、
  サイズ変更、手動pin／編集の対照。初期全Folder探索0回、GPU keep_range／upload予算不変、
  保持成果物は同じ行への通知反復で増えず、明示更新／dropで解放。
- §1.314の子.partによるmtime-onlyとDelete混在で親の追加install0回。表示日時・sort・
  代表要求stamp不変。ZIP/PDF／普通fileのmetadata変化をこの例外へ広げない。
- 同folderのA/B、A→B→A、park中内容上書き、先着結果／cutoff後通知、cancel／close／
  subscription再作成。Aの一致判定でBのdirty・代表・要求を消さない。
- 比較直前に現在表示が変わるhandler回帰。worker開始時の旧表示との一致だけで省かない。
  GridReloadのAction／メニュー／割当キーが同じ明示更新へ届くことを確認する。

**ここで止め、R5の独立設計レビュー後に新方式の実装へ進む。**
まれな失敗は割り切り (ログ + 通知 [+ 作り直し])。新しいretry／rollback／回復journalは作らない。
今回の新方式の回帰追加0件。アプリ起動・コミットはしない。

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
