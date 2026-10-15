# §1.375 前回走査と今回走査の行signatureで再表示を省く

2026-10-13夜、`next-list`、R10。改訂基点 `37dcc8b9c`。
**LISTrev12のR9 REVISE（P2二件／P3一件）に対する利用者決定を反映。比較は前回／今回の走査の
行signatureだけとし、sidecar stampと別context／RemoteのDB反映資格を判定から外す。
fork時の前回記録の継承・新contextの所有資格・独立購読を明記。LISTrev13の独立設計再レビューは
ACCEPT（P1／P2なし）、利用者の実装指示に従い実装・独立コードレビュー・自動検証完了（2026-10-14）**。
R10提出時は設計書とmanualだけを変更した。実装工程では§6の回帰・性能検証を行い、
§1.373の既存Append／Undo契約を維持する。アプリ起動・コミットは行わない。
R8 ACCEPTや旧検証をR10の受入証拠にしない。
実装基点`a73ea3750`の独立レビューはREVISE（P2一件／P3二件）。修正と今回の最終検証は§8.4に記録し、
§8.2の初回実装検証とは区別する。
正本項目: [next-release-backlog.md §1.375](next-release-backlog.md#1375-フォルダで-delete-キーで削除すると正しく消えた一覧がもう一度丸ごと読み直されてちらつく--利用者の実機報告-2026-10-12)。

案B・自己操作operation ID・provenanceによる照合、代表選択保持・leaf付きcache key・選択付きpayload・
旧cache移行は撤回済みで、復活させない。§1.373 R4案1のAppend／Undo operation IDと受付契約は維持する。
過去の設計は `ae2772c8e`／`84f31c346`／`13f2fc3e9`／`aa26c9b29`／`37dcc8b9c` の履歴を参照する。

## 1. 原因とR10の不変条件

Deleteは `handle_delete_key` → 確認 → `start_delete_files` → delete worker → `poll_delete_pending` →
成功pathだけ `remove_items_batch`。終端の `current_folder_signature = None` により後続watch scanは
既存signature一致分岐を通れず、`apply_external_rescan` → `load_folder_with_scan_owned` で一覧を再採用する。
これはsource inspectionで確定した経路であり、利用者実機の時系列ログを得たという主張ではない。

**比較相手は採用表示全体ではなく、その一覧を作った前回走査の記録。同じ走査層が作る行signature
（名前・種類・サイズ・更新日時・投影後の順序）が同じならinstall・世代更新・install起因thumb再要求を
省き、違う／不明なら通常再読込へ進む。sidecar stamp・DB cache・thumbnail状態は判定に使わない。**
採用直前に元owner・source・世代・走査／投影条件・前回記録の有効性を確認する。

成功Deleteの既存行除去だけを前回記録へ反映する。JPG優先の `a.jpg + a.png` からJPGを削除した場合、
前回記録にPNGの露出まで先取りしない。今回の走査・投影でPNGが現れれば差分として通常再読込する。
自己操作か外部操作かを通知の判定条件にせず、時間窓で通知を捨てない。

## 2. R10の許容と簡素化

### 2.1 別context／RemoteのDB反映を保証しない

**利用者決定（2026-10-13夜）**: 監視起点の更新では、別context・Remote由来の補正・タグ・pin等の
DB値の反映を保証しない。Ctrl+F5／「最新の情報に更新」または開き直しで反映する。
書込み元が別sourceへ移動・終了した後も同じ許容とする。

R9 §7.2の「siblingの反映資格が不明なら通常再読込」は撤回し、そのための条件を設けない。
TAG_WRITES／PAGE_EDIT_WRITES等を追加のinstall資格証明に使ったり、受領台帳・publication機構・
共有表示storageを新設したりしない。既存の★session publication、snapshotがある編集のepoch再照合、
本しおりのmatching-context失効、metadata importのcontext／世代付き反映はそのまま続ける。

根拠: これらのDB値は通常のmIV操作では既存publication／cache更新で反映する。別context／Remoteの
値が届かない場合はDeleteしなくても現行で届かず、Delete後の一覧再読込が偶然補っていた。
この偶然の更新は維持要件にしない、という利用者判断である。行が変わって通常再読込した結果として
DB値が更新される場合はあるが、それを監視起点の保証にはしない。

外部ツールの通常の画像・動画変更はfileの名前・種類・サイズ・更新日時を行signatureへ反映する。
opt-in XMP rating書込み（既定OFF）も元fileの更新日時を変える。
既存metadataから得られる更新時刻の精度を記録し、秒precisionの表示値だけで内容更新を判定しない。
更新日時・サイズを意図的に維持する外部書換えはstampで検出できない制限を残し、
追加の画像hash／全件decode／回復機構は作らない。

### 2.2 sidecarは比較せず、自動取込もしない

**利用者決定**: `mimageviewer.dat` 等の取込対象sidecarは従来どおりファイル行の対象にせず、
stampも比較しない。外部で変更したsidecarはCtrl+F5／「最新の情報に更新」または開き直しで取り込む。
**監視起点の更新ではsidecar取込をしない。行差分があり一覧を再読込する場合にも、この自動取込をしない。**

mIV自身が書いたsidecarの内容は既存の編集／cache更新で表示済み。
編集終了のbackup flushやDelete workerのhard purgeがsidecarを書き換えても、
それだけでは行signatureに差分を作らず、追加installを起こさない。
R9のsidecar stamp比較・初回stamp取得・stamp資格の照合・差分による無条件reloadを全て撤回する。

sidecar由来通知を自己操作と認定して捨てる方式にはしない。通知は元ownerのdirtyへ残し、
通常走査で行signatureを比較する。同じbatchに外部Bの上書き／Cの追加があれば、行差分は取り込む。
sidecarを直近stampへ無条件に進める方式も使わず、比較用stamp自体を持たない。
監視originは既存の更新cause／intentから既存hydration入口まで配送し、sidecar取込だけを実行対象から外す。
page identity準備・既存DB読込み・通常表示のhydrationは維持し、全体設定を一時OFFにしない。
R10比較を使うwatchのdefer／restartと同じ自動物理rescan経路（Resumedを含む）にもこの取込範囲を揃える。
明示GridReload／通常openの既存取込と、その厳密なsource検証・設定ON／OFFの規則は変更しない。

### 2.3 代表と既存thumbnail lifecycle

一致してinstallを省けば表示中のサムネイルは代表を含め残るが、同じ代表を使い続ける保証ではない。
差分reload、eviction後再読込、idle upgrade、サイズ変更、cache miss／再生成、RAW継続・遅着では
既存選定・共有cacheに従い代表が変わってよい。「Ctrl+F5まで変更を反映しなくてもよい」は許容である。
手動pin・解除・pin先消失の既存fallbackは維持し、pin設定は消さない。
画像／保存両終端・request ID、CPU／GPU eviction、keep_range／upload予算、catalog形式は変更しない。

### 2.4 状態の組み合わせを減らす

| 案 | 判断 |
| --- | --- |
| 前回／今回の走査の行signatureだけで比較 | 採用。同じ走査層の入力に揃え、DB／thumbnailの採用状態を共有しなくてよい |
| R9のsidecar stamp比較／別context・RemoteのDB反映資格／通常reload例外 | 撤回。通常flush・hard purgeで再表示が増え、書込み元の移動／終了後まで資格管理が広がる。利用者の許容でこの管理自体を不要にする |
| 全表示snapshot／部分共有storage／初回採用入力の共有化 | 不採用。比較範囲から外した値の共有は不要 |
| 初回worker準備でPending行の表示を遅らせる／初回だけ常に差分 | 不採用。通常scanの記録とfork時の継承で初回から比較できる |
| 代表保持・cache変更／移行、operation ID・provenance、追加pending owner | 不採用。通常利用の費用と所有・移行の組み合わせを増やさない |

## 3. 前回記録・比較・採用

### 3.1 記録の入口と範囲

通常load／pre_scanは既存 `ScannedDir` の走査・投影層で軽い行記録を作り、
成功採用したsourceの記録だけを既存folder-list owner／ViewerContextBundleに持たせる。
App.items／DB cache／thumbnailをcloneしたsnapshotを作らず、UIに追加の全件clone・同期I/Oを置かない。
forkは新scanを消費しないため、元一覧の前回記録を継承する（§7.3）。初回だけの強制差分を使わない。

| 入力 | R10の契約 |
| --- | --- |
| ファイル行 | sourceに対する行key／名前・種類・サイズ・更新日時。同名除去・種類採否・本判定・動画／音声補助画像の既存投影を使う |
| 順序 | 現在の並び・フィルタ投影後の行順と行数。read_dir順ではなく、同じ投影条件で走査層が作った順序 |
| Stack／展開 | 既存グループ化・member変化が行／種類／順序へ及ぼす差分。保持用identityを作らない |
| 実Folder mtime | §1.314により観測signatureだけへ含め、行比較ではmtime-onlyを無視。表示mtime・sort・代表要求stampへ漏らさない |

sidecarファイル・そのstamp、DB由来rating／tag／編集／しおり／pin値、thumbnail状態／寸法／画像、
省略chip／tooltip内訳は比較対象にしない。画像の補助sidecarとして選ばれる通常Imageのstampは
既存の行／source投影規則を維持する（取込用 `mimageviewer.dat` と混同しない）。
DB依存sort／facetが行へ効く部分は既存の行投影・条件を使い、DB値全体の反映保証へ範囲を広げない。
旧 `FolderScanSignature` は表示順非依存のhash・秒mtimeだった。R10実装では既存entry metadataから
利用可能な精度のstampを得て、同じmaterializeの投影後行記録へ接続する（§8.1）。
プロセス内signatureであり、永続cache形式・暗号学的保証は作らない。

### 3.2 Delete成功行の更新

成功pathの既存行除去と前回記録の同じ行の除去を同じownerで扱う。
走査層の純粋signature更新を既存除去の仕事へ揃え、UIからFS／DBを再読しない。
失敗／取消pathや他contextの記録は除去せず、残った行の相対順を維持する。
現行のsignature全面無効化はこの境界で置き換える。非表示候補の露出は前回記録へ先取りしない。

成功physical sourceに対する終端後の通常scanは、通知の有無や現在行の成功idxの有無に依存せず要求する。
同じownerの既存restartへ集約し、DeleteReceipt・操作ID・追加ownerを作らない。
sidecar hard purgeの結果を比較基準へ反映する必要はなく、sidecar内容・stampは判定外である。
前回記録の更新は元ownerだけに作用し、fork済み兄弟の記録はその兄弟の走査で更新する。

### 3.3 worker比較と受理

既存 `poll_current_folder_watch` → `check_external_folder_changes(Notified)` → `start_external_rescan` の
ownerでworkerがscan・行投影・signature生成／比較を行う。UIで全行比較・snapshot clone・追加I/Oをしない。
元context／exact source／items generation／走査・並び・フィルタ条件と捕捉した前回記録の有効性を
採用直前に確認する。Delete記録更新・source切替・sort／facet変更で旧要求を拒否する。
thumbnail採用や別context／RemoteのDB書込みだけで新しい比較revision／reload条件を作らない。

- 一致: 観測signature／親mtime／取り込んだcutoffだけを元ownerで進める。
  install・世代更新・install起因thumb再要求は0回。選択・check・scroll・既存要求を維持。
- 行差分／行記録が不明: 走査済み結果を通常一覧採用へ渡す。自動originによるsidecar取込はしない（§2.2）。
- 取消・stale・scan失敗: 元表示を維持しcutoffをackしない。未照合dirtyを元ownerに残す。
  まれなI/O失敗はログ・通知で閉じ、毎フレームretry／rollbackを新設しない。

明示 `KeyAction::GridReload`／「最新の情報に更新」は常に通常reload・代表再選定・設定に応じた
sidecar取込へ配送し、この省略を通さない。GridReloadは既定キーなし、F5は★5。
Ctrl+F5は利用者の呼称／割当で、rawキー判定・既定keymap変更はしない。

## 4. §1.314・context／購読・dirty／cutoff

現行App-global watcher／external rescanを、既存physical-list bundleの観測記録と同じownerへ揃える。
購読identityはcontext ID・subscription epoch・exact folder・listing options。
新しい単独pending owner、共有fanout／ack台帳、時間TTLは作らない。

| lifecycle | 契約 |
| --- | --- |
| 登録と初期coverage | OS登録→登録後scanを既存source／rescan workerで行う。UI同期登録を足さず、登録前scanを新購読のcutoff証明にしない |
| park／mount | 元bundleに購読・dirty・前回記録を保持。park中通知も元ownerへ残しmount後に比較 |
| fork | 前回行記録の内容を継承し新ownerへ束縛。watcher／rx／dirty／cutoff／pendingは共有せず、新購読を登録後scanで開始（§7.3） |
| A/B同folder、A→B→A | 各contextの独立記録・購読を維持し、Aの一致でBをackしない |
| 走査中の通知 | kind／path／sequenceを有界dirtyへ残し、cutoff後は既存restartで再走査。次の新通知を必要としない |
| source終了／context破棄 | 元購読・worker・記録だけを解放し、兄弟を失効させない |
| stale／失敗／coverage不明 | 未照合cutoffを消費しない。通常走査へ渡し、行記録を確定できないなら通常reload |

Accessは除外。Create／Remove／Modify(Name old/new)、file Modify(Data／Metadata／Any)、Unknown／
watch error／overflowのkind・pathをsaw_changeへ潰さず保持する。WindowsのModify(Any)でも実directoryとfileを区別。
通知kindだけでsidecar変更を行差分とみなさず、完了した走査の行signatureで判断する。
自己由来の判定やsidecar通知の先捨てはしない。

実Folder mtime-onlyは観測値だけ進め、親一覧の表示mtime・sort・代表要求stampは旧採用値を維持する。
普通file・ZIP／PDF本体へこの例外を広げず、Unknownをdirectory-onlyと偽らない。
Deleteと混在しても実Folder mtime-onlyだけで追加installしない。
Resumedの親mtimeふるいは維持、Notifiedはbypassする。

## 5. 同型経路と範囲

| 経路 | 扱い |
| --- | --- |
| 通常FolderのDelete／メニュー削除 | ごみ箱／完全削除の成功行除去・前回記録更新・終端scanへ合流 |
| ZIP／PDF本体の物理削除 | 実file削除。cache／metadata purge・Shell成功判定は既存どおり |
| ZIP内画像／ZipDir／PDFページの削除 | 既存理由通知で拒否。書庫の書換え削除は新設しない |
| 検索root／Collection root | 再query・actor revisionは別の正当な更新で、物理Folder比較で抑止しない |
| Collection・Smart・★一覧等の物理子 | 元physical-listの走査記録で比較し、synthetic root記録を流用しない |
| 改名／move／copy／D&D／Shell paste | 第一段の終端集約は対象外。後続watchの行比較は共通 |
| 本フォルダ | 既存購読範囲・Resumed／先着rescan、§1.373 R4の自動観測配送を維持 |

第一段の受入範囲はDeleteの余分な再表示。R5で撤去済みfolder_reconcile・RawFolderSnapshot・
DeleteReceipt等は復活させない。§1.373のcause／operation資格／Undo失効除外も変更しない。

## 6. R10の受入回帰・性能検証計画

LISTrev13 ACCEPT後の実装・検証では以下を受入条件とし、実行結果は§8へ記録する。

- 実Delete handlerの成功行除去後、行signature一致なら追加install・世代更新・thumb再要求0回。
  通知先着／終端先着／通知なし／反復通知、最初のDeleteから検査する。
- JPG→PNG、Folder／ZIP、PDF／EPUB、動画／音声補助画像、Stack 3→2／1枚Image化、sort／facet差分。
  非表示候補を前回記録へ先取りせず、今回の行差分を通常採用する。
- Delete＋残存B上書き／C追加の同batch、同一秒更新、末尾行差分、行の順序差分。
  利用可能な更新時刻の精度とstamp維持書換えの既定の制限を確認する。
- 編集backup既定ONで、編集終了flushのsidecar通知だけでは追加install／世代／thumb要求0回。
  sidecar項目を持つ編集済み画像のDeleteで、hard purgeによるsidecar書換え後も余分なinstall0回。
- 外部sidecarの作成・変更・削除だけの通知では行一致を保ち、自動取込0回。
  外部sidecar変更と行追加が同batchなら行は採用するがsidecar取込0回。
  Ctrl+F5／開き直しでは設定に従い通常取込する。scan中変更・pending writerでも自動取込を復活させない。
- LISTrev13 P3: defer→一覧復帰、走査中通知によるrestart、Resumedの各実handler経路で、
  行一致／行差分の両方を検査する。自動sidecar取込は0回、その直後の明示GridReload／通常openは
  編集・タグそれぞれの設定に従い取込む、という対照試験にする。通常Folderのdefer終端が
  `reload_current_folder_preserving_override`へ直接合流する現経路も自動originを維持する経路へ接続する。
  originをrestart／deferred request／hydrationに運び、全体設定の一時変更やpage identity準備の省略はしない。
- 別contextで補正・タグ・pinを変更し、書込み元を同sourceに残す／別sourceへ移動／終了する場合と、
  Remote補正、sidecar OFFを検査。受け手の行一致は同じ省略へ進み、DB反映資格によるreload例外を設けない。
  反映されないDB値は明示GridReload／開き直しで取得し、既存★publication・snapshot編集・本しおり反映は維持。
- 通常load／pre_scanの初回記録、両fork関数からの「fork後の最初の更新／Delete」を検査。
  新ownerのsource・世代・投影条件、独立登録→確認scan、登録までの外部変更、元ownerのdirty不変を確認。
- §1.314の子.part mtime-only＋Delete混在、directoryとfile Modify(Any)、sidecar通知との混在。
  観測だけが進み、表示mtime・sort・代表要求stampを変えない。
- A/B、A→B→A、park中変更、cutoff後通知、scan中切替／取消／close／再購読。
  旧owner／記録／世代／条件の結果を採用せず、兄弟のdirty・要求を消さない。
- 明示GridReloadのAction／メニュー／割当キーは、signature一致でも通常reloadへ届く。
  一致中のthumbnail・画像／保存両終端・RAW継続・cache-only・eviction／idle upgradeは既存契約を維持。
- install省略後、Delete終端が失効させるrating cache等を含め、描画・scroll中の同期DB query数も前後比較。
  省略を理由にUIでDB再取得が増える変更を入れない。

1万／10万行のscan・行投影・signature生成／比較、末尾差分・全行差分・Delete除去・fork直後を測定する。
UI enqueue／poll／資格確認・最大frame時間、worker時間・割当量・再走査／install回数を記録。
UI全件clone／全表示snapshot、新しい共有表示storage・全件decode・cache移行は0件。
初回表示・scroll・idle／tray・CPU／GPU予算が退行し、通常利用に目立つ遅延が出たら止める。
headless／fake-inputを先に実行し、実アプリ計測は別途具体的suiteへの利用者了承を必要とする。
実装完了時はtargeted、fmt、通常／portable core check、build-dev、test-fullを既存方針で行う。

**LISTrev13で設計ACCEPT。P3の対照試験を含め、この契約で実装・検証する。**
まれな失敗は既定で割り切り（ログ＋通知［＋作り直し］）。新retry／rollback／回復journalは作らない。
以下§6.1〜6.5は過去の記録で、R10の受入証拠ではない。

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

### 6.5 R9文書改訂の記録（履歴）

`37dcc8b9c` の改訂は設計書1ファイル。UTF-8・節構造・diff確認が成功し、
実装・回帰追加・cargo build／test・性能計測・アプリ起動・コミットは0件だった。
その後LISTrev12でREVISE（P1なし／P2二件／P3一件）。sidecar stampとDB反映資格の設計はR10で撤回する。

## 7. LISTrev12と利用者決定への対応（R10）

### 7.1 P2二件を条件追加ではなく許容で閉じる

| finding | 利用者決定とR10の対応 |
| --- | --- |
| P2-1: 書込み元の移動／終了後の別窓、Remote補正などは現在のsibling判定では網羅できない | 別context／RemoteのDB値の反映は監視更新で保証しない。Delete後reloadが偶然反映した値は維持要件にせず、Ctrl+F5／開き直しで取得。R9の通常reload例外・DB反映資格判定は撤回 |
| P2-2: backup flush／Delete hard purgeでsidecar stampが変わり再表示が増える | sidecarを行対象・stamp判定から除外。監視originの取込を実行せず、外部変更は明示更新／openで取込。通知の先捨て・自己由来認定・stampの無条件更新で対処しない |
| P3: 既存一覧のforkは新しいScannedDirを消費しない | 前回走査記録を継承し新ownerへ束縛。独立購読の登録後scanで新coverageを確立。「fork後の最初の更新／Delete」を回帰へ追加 |

R8の共有storage／初回Pending表示順序変更は不要のまま。
R9の「raw signatureだけの一致は不採用」の撤回も維持し、R10ではさらにsidecar stampを外す。
新しいDB更新台帳、TAG_WRITES／PAGE_EDIT_WRITESを使った資格管理、source移動／終了を追跡するownerを作らない。

### 7.2 コード照合と初回記録の入口

| 経路 | R10の接続 |
| --- | --- |
| 通常load／startup／pre_scan: `load_folder_with_scan_claimed`／`prepare_scanned_folder_listing_with_cache_keys` | ScannedDirを消費して既存signatureを生成する境界で初回行記録を作れる。items cloneや初回強制差分は不要 |
| Smartの実Folder: `install_scanned_folder_listing`、Collection／★等の物理子、DFS／pane／history pre_scan | 同じFolder prepareへ合流し、成功採用した元physical-listの記録を保持。古い候補を基準にしない |
| 両fork関数 | 現行 `current_folder_signature` は `duplicate_for_parked!` に分類済み。R10の前回行記録も明示継承し、コピーした内容へ新ownerの資格を束縛する |
| Stack／展開 | 元physical scanの行記録と、既存投影後の順序との対応を維持。別sourceのsynthetic記録を流用しない |
| ZIP／PDF／変換済み書庫／synthetic root | 通常Folder scanを持たず物理Folder watchの省略対象外。既存reload／query／actor ownerを維持 |

`tag_ops::poll_tag_write_results` はmount中tags_cacheを更新し、`poll_page_edit_reconciliation` は
snapshot／stampなしでreturnする。pin操作もmount中folder_pin_mapを更新する。
Remote補正は指定addressのDB keyへ書く。この反映境界の不足はR9で検出しLISTrev12で移動／終了後まで
指摘されたが、R10では§2.1の利用者許容として記録する。現context／source／pendingから資格を推定しない。
既存の★session／mount publication、本しおりのcontext失効、metadata importの反映は変更しない。

### 7.3 forkの継承・資格・購読

対象は `split_current_context_preserving_main_grid` と、それを使う
`split_materialized_physical_context_for_detached_scope`。forkは新scanなしで既存一覧を引き継ぐため、
前回行signatureの内容・対応source・投影条件を一覧と同時に継承する。

- 元contextは自分の前回記録・購読・dirty／cutoffを保持する。新contextは独立した記録の資格を持ち、
  新しいcontext ID・採用世代・exact physical source・有効な投影条件へ束縛し直す。
  同じ数値のitems generationや同folderだけを根拠に元contextの完了を採用しない。
- mutable watcher／rx／dirty／cutoff／走査中request／cancel tokenはclone・共有しない。
  fork前のpending結果を新contextに付け替えず、元要求は元ownerの既存lifecycleだけで処理する。
- 新contextの購読は既存worker上でOS登録→確認scanの順に始める。
  継承した前回記録は比較の基準として有効だが、新購読のcutoffを証明しない。
  登録まで／登録中の変更も確認scanで拾い、最初の走査が一致すればinstallを省く。
- 新ownerがまだpark中なら既存の購読開始／mount契機を使い、開始時の確認scanを省かない。
  元ownerのdirtyを新ownerにackさせず、close／dropも各ownerだけに作用する。
- materialized physical forkはその後source・世代・visible_indicesを束縛し直す。
  継承recordを新physical scopeの同じ行投影へ対応させ、sourceが違う親全体のsignatureをそのまま使わない。
  元の軽い走査記録から適切なscopeを対応付け、資格が確定していないrecordで一致を宣言しない。
  virtual／synthetic rootから物理子を作る場合もこのsource対応を検証し、「forkだから初回は常に差分」を使わない。

これは§1.375のphysical-list観測所有をforkの既存分類へ揃える設計であり、
detachedのviewport／geometry／focus述語を変更する設計ではない。
実装時は両fork経路で最初の通知・最初のDelete・元ownerの保留通知を対で検査する。

### 7.4 R10文書改訂の検証（提出時の履歴）

変更は本設計書とmanualのgrid／shortcuts、計3ファイルだけ。
README／backlog／ui-responsivenessの旧R8索引は今回の指定範囲外で変更せず、
R10レビュー後の実装工程で同期する。§1.373のコード・設計・manualには触れない。
実装・回帰追加・性能計測・cargo build／test・アプリ起動・コミットは0件。
改訂3ファイルのUTF-8 strict decode、manual 2ファイルのHTMLParser解析、
R10の節構造・両fork経路の記載確認、`git diff --check`が成功。
コード・Cargo設定・索引文書・§1.373設計のHEADとの差分は0件。

## 8. R10実装と検証

### 8.1 実装境界

`FolderRowRecord`は既存走査／materializeが採用した物理file-rowを記録する。
種類採否、同名winner、補助画像、既存rating sortと表示block順までが記録の境界で、
名前・種類・サイズ・更新時刻・補助画像stampをscalarとして保持する。
Stackはこの同じ入力と既存separator／script／sort条件の決定的なgroup化なので、
入力と条件が同じならmember／種類／順序が変わらず既存group表示を維持できる。
facet／detailsはitems配列ではなく既存visible_indices／details_orderの投影を保持する。
file由来の投影を変える名前・サイズ・更新時刻は行差分、投影条件変更は要求資格の変更として扱う。
DBだけのfacet／details反映保証は§2.1の許容から追加しない。

通常のScannedDir producerは一回のread_dirによる直下走査で、materializeの除外／並び替えは
sourceを合成しない。Appの通常／pre_scan／external worker、Remoteのfolder／本棚も、走査元の
exact folderをmaterializeへ渡す。この既存不変条件から親scope keyを一回作り、初回記録のUI側で
行ごとにPath::parentを解析しない。行配列は投影件数分を先に確保し、構築中の拡張コピーを避ける。
複数sourceの手製ScannedDirはテスト用入力で、
通常producerの契約へ追加しない。forkのscope対応・資格検証は別途維持する。

前回記録はimmutable Arc、要求はそのrecordとrevisionを捕捉する。
成功Deleteの除去だけを元ownerの記録へ反映し、workerが今回の投影と比較する。
非表示候補の露出は今回投影だけが決める。実Folderの観測stampは先に新しい値で生成し、
一覧のprepareにだけ旧採用のdirectory表示／sort stampを渡して§1.314を維持する。
EqualではUIに全走査payloadを送らず、retirement workerで旧記録を解放する。
Differentではworkerで準備した投影を既存loadへ渡し、UIで同じ投影をやり直さない。

既存RefreshList intentへ自動originを載せ、sidecar hydration入口で取込だけを外す。
defer／restart／Resumed、Appendからの自動配送も同じoriginを維持する。
Stackの非同期group採用後に行う二度目のhydrationも既存GroupingSourceでpolicyを配送する。
既存InternalHydrationに入れ、既に消費したcursor／Append receipt／local searchは再適用せず、
遅着groupの選択は既存StackSelectionTargetの解決を維持する。
watch／dirty／cutoff／pendingは既存bundleへ移し、closeの既存cancel境界に接続する。
forkは記録だけを継承し、元購読を共有しない。代表cache／payloadの変更は0件。
direct readerからのforkでmainへ保存済みbasisを戻す場合は、既存DirectMedia source labelに
捕捉したbasis generationと、record source／snapshot origin／mainのsourceの一致を確認して
新main世代へ記録を束縛する。対応しない記録を有効化する全scopeの世代書換えはしない。
取消はExternalRescanPendingの共通入口で先にatomic cancelを立て、receiver／proofと
未pollの準備済み結果を既存retirementへ渡す。採用時に不要になったraw scanも同じworkerで解放する。
前回記録の世代は既存installが成功して世代を進めた後に束縛する。
Stackのflat／aggregate切替でも、切替前に同じphysical source・StackView・採用世代との対応を確認し、
成功切替後の世代へ束縛する。sourceが違う記録や既に古い記録を切替だけで有効化しない。
検索等のsynthetic rootには、既存の物理子／root配送資格をwatch登録・走査・Delete終端の入口で
適用する。検索前に保持したfolder／mtimeだけを根拠にrootを物理Folderへ置き換えない。
従来FolderScanSignatureは秒精度のhash tuple streamを完全維持する。精密比較はFolderRowRecordが
担当し、既存signatureへ精密stampを追加するUI passを作らない。Append／Undoの完全構成stampは
既存の別契約のままにする。§1.314の保持処理はdirectoryのmtimeだけを置き換え、size差分を消さない。

### 8.2 検証記録

2026-10-13〜14、確定source 17ファイルのfreezeと独立レビューを照合済み。コード判定はACCEPT、
P1／P2なし。追加回帰28件を含む§1.375 filterは29件成功。

| targeted filter | 成功数 |
| --- | ---: |
| `signature_from_scan` | 2 |
| `folder_mtime` | 10 |
| `section1375` | 29 |
| `folder_scan` | 50 |
| `multiwindow_scenario` | 36 |
| `completed_export_refresh` | 1 |
| `a_watch_rescan_during_main_viewer` | 1 |
| `external_rescan` | 4 |
| `current_folder_watch` | 1 |
| `section1373` | 68 |
| `sidecar_restore` | 61 |
| `viewer_context_registry` | 80 |
| `section1353` | 21 |
| `filename_stack_ui` | 24 |
| `remove_items_batch` | 15 |
| `delete_pending` | 2 |

16 filterは405実行／重複除外391件、失敗0・ignored 0。
最適化constructor計測の1実行を加え406実行／391件。全コマンドexit 0。
`cargo fmt --all -- --check`、通常／portable core check、glyph checker（問題0）が成功。
コマンド・exit・時間は[最終targeted state](../target/section1375-r10/full-fixes/targeted-state.json)、
handler回帰は[section1375 stdout](../target/section1375-r10/full-fixes/section1375.stdout.log)を参照。

性能は全行に実経路と同じ精密stampを持つ合成入力を使い、constructorを5回測定した。
`dev-runtime`（opt-level=2）の結果:

| 行数 | 初回記録 median／max | workerの投影＋記録 | workerの全行比較 | 実行配列capacityのbytes |
| --- | ---: | ---: | ---: | ---: |
| 10,000 | 0.659／0.972 ms | 14.675 ms | 0.052 ms | 960,000 |
| 100,000 | 8.353／10.949 ms | 139.539 ms | 0.717 ms | 9,600,000 |

改修前の100k constructorはmedian 17.970／max 19.780 msで受入にせず、単一走査sourceの親key再利用・
配列の事前確保で費用を削減した。通常loadの既存signatureへの不要な精密lookup／hash拡張も撤去した。
旧測定は[attempt5](../target/section1375-r10/targeted-attempt5/record-opt2.stderr.log)、
確定測定は[record-opt2](../target/section1375-r10/full-fixes/record-opt2.stderr.log)。
debugの同constructorは100k median 62.336／max 69.763 ms。最終値は区切り文字の共通identity修正を含む。
debugを実製品の受入性能として扱わず、同じconstructor・stamp条件の最適化結果と区別する。

実filesystemに作った10k／100k JPEG行のdebug worker測定は、scan 61.044／645.627 ms、
投影＋記録56.177／574.892 ms。Equal UI pollは0.304／0.551 ms。
未poll Differentのcancel／replace／stale retirementは100kで0.097／0.604／0.299 ms、
UI上のrecord destructor実行0回。forkのrecordはArc共有、Delete除去履歴の全件cloneは0回。
数値は[section1375 stderr](../target/section1375-r10/full-fixes/section1375.stderr.log)。
既存の32回連続remove処理も測定したが、その処理全体を新しい比較費用とはみなさない。

実Delete終端→自動走査→Equalの後、headless grid描画と3箇所のscrollで、ratingの全read／point read、
tag、rotationの同期DB read差分はそれぞれ0回。両方式で必要なfooter／全体件数の一度の読取りは
同じ既存cacheをwarmしてから測定した。新しいUI同期I/Oや全一覧snapshotで抑止していない。
実アプリの最大frame／初回一覧の総時間は未計測で、上記constructor単体・headlessメソッドの測定と
混同しない。アプリ起動は0回。

検証途中の失敗も履歴へ保存した。install前のrecord世代束縛は成功install後へ修正し、
Stack flat／aggregate復帰の回帰を追加。既存hashの失敗は精密化の撤去で既存assertを維持。
folder sizeの失敗はmtime保持がsizeまで0にしていた実装を修正し、全raw候補を表示する旧fixtureを
設定と実materializeへ整合させた。`real_changes`の各assertは削除・緩和していない。
新しい失敗のない最終targeted結果と、これら途中の結果を合算しない。

最初のfull gateではworkspace 13,471成功／3失敗／63 ignored（lib内3件）が出た。
一覧復帰のexport／watchの2件は旧physical-history helperだけを待っていたテストの完了境界を、
実external-rescan ownerのpollへ接続した。元の画像存在・世代・mtime assertionは維持し、
二度目のconsumeが新しい走査も作らないことを追加確認する。
検索画像の実失敗は、初回scanの `/` 表記と採用後scanの `\` 表記を生OsStr hashで別keyにしたもの。
baseline・source・世代・投影・stampは同じなのにrow keyだけが違うことをtraceで確定した。
共通のrow／精密stamp／成功Delete keyをallocation-free・losslessのPath hashへ統一し、
実scan二回とDeleteの区切り表記回帰を追加する。ドライブ・表示名のcaseは混同しない。
検索画像の既存paint assertionは変更しない。

同型inventoryで新しいDetachedPhysical観測の実差分が閲覧ページを閉じる採用経路も確認した。
自動Differenceの既存deferを `viewer_session_blocks_main_window()` または
`DetachedPhysical && fullscreen_idx.is_some()` という一覧所有資格へ接続する。
Main一覧と別窓動画／音声の共存は従来の採用経路を維持し、fullscreen有無だけの一括抑止はしない。
元bundleの既存FolderRefreshRequestだけを使い、一覧復帰で消費、context終了で破棄する。
新pending・再open・猶予時間は足さず、R10のcontext／終了契約内の構造修正として独立レビューで合意。
実workerの差分を使う「独立readerはdefer／Mainの別窓media中は採用」の対照回帰を追加する。

最初の確認用`build-dev.ps1 -PreserveRuntime`はexit 0、通常機能セットのcore／remote／EPUB workerと
VCRT（runtime 4／PE 3）が成功。上述のfull失敗を修正したsourceでtargeted・fmt・両core checkを
再実行した。更新後のbuild-devはexit 0、最終full gateは13,531成功／失敗0／既存ignored 63件、PASS。
full実行は計2回（最初は3件失敗、修正後の完走は1回）。最初の失敗ログは保存し、成功証拠に含めない。
[最終state](../target/section1375-r10/final-after-fixes/final-state.json)と
[stdout](../target/section1375-r10/final-after-fixes/final.stdout.log)／[stderr](../target/section1375-r10/final-after-fixes/final.stderr.log)に記録する。
コミット0件。manualは提出済みR10と整合する。

### 8.3 変更ファイル

| 範囲 | ファイル |
| --- | --- |
| 走査・比較・採用・Delete／watch | `src/app.rs`, `src/app/folder_scan.rs`, `src/app/top_level_grid_view.rs` |
| origin／hydration／Append接続 | `src/app/startup_list_restore.rs`, `src/app/sidecar_restore.rs`, `src/app/book_append_refresh.rs`, `src/filename_stack_ui.rs` |
| context／fork／保存済みbasis | `src/app/viewer_context_registry.rs`, `src/app/startup_ops.rs`, `src/snapshot.rs`, `src/app/smart_folder.rs` |
| 生存cache／テスト用query計数 | `src/rotation_cache.rs`, `src/rating_db.rs`, `src/tags_db.rs` |
| 回帰・fixture | `src/app/section375_tests.rs`, `src/app/tests.rs`, `src/app/similar_navigation_tests.rs` |
| 設計・索引・所有境界 | `docs/section375-operation-reconcile-plan.md`, `docs/README.md`, `docs/next-release-backlog.md`, `docs/ui-responsiveness.md`, `docs/architecture-overview.md`, `docs/async-architecture.md`, `docs/detached-rework-plan.md` |

source 17ファイル（新規回帰module 1を含む）・docs 7ファイル。
manualのgrid／shortcutsは提出済みR10の記述と照合し、追加変更はない。
§1.373の設計書・manual、Cargo設定、ビルドscriptは変更しない。

### 8.4 a73ea3750 独立実装レビューの修正（2026-10-14）

P2は、成功Deleteが表示行と観測の除去記録だけを進め、StackViewのメンバー／aggregate→flat対応表と
保持した集約投影を進めない所有境界の不整合だった。Equalは正しくinstallを省くため旧対応表が残る。
成功集合の確定後、既存delete workerで元のgrouping／rating順snapshotから成功分だけを除去し、
同じsurvivor候補から両対応表を再構築する。UIで全件clone・sort・DB読込みは行わない。
保持投影がある場合はmaintenance lease解放後に既存Stack candidate読込みを行う。
2枚Stack→単独Imageでは元の集約snapshotに生存画像のpage keyがないため、単純なindex remapを採らない。
削除取消・部分失敗でも成功分だけを準備する。既存DeletePending／modalを同じ完了通知まで保持し、
Stackの背景採用とclose reconcileは削除終端まで延期する。完了受理でcontext／generation／元view Arcを
確認し、StackView／保持投影の交換と表示行の除去を同一frameで行う。兄弟の共有Arcは変更しない。
旧payloadは既存retirement workerへ渡す。古いownerの結果は採らず、同sourceの置換済み一覧には
観測不明から通常の準備を行う。保持投影の読込み失敗はログ＋通知と既存の一覧復帰時refreshで扱う。
読込み失敗時は旧集約を復帰させず、flat survivorを保ったまま観測不明＋既存の一覧復帰時refreshへ渡す。
未採用のflat列をaggregate modeに変えて既存Recoveringへ流す案は、対応表の前提が違うため採らない。
新pending owner・自己操作ID・通知の時間窓・Equal側の強制installは追加しない。
この削除完了境界とworker配置は独立のCodex設計確認で合意した。

同型の初回分類（Extracting／Grouping／そのRetry）にも、成功Deleteで入力世代が変わった後に
旧要求だけがstaleとなり、Equalが分類の再開を省く経路がある。表示済みStackと分け、
初期要求のcontext／generation／件数／sequence／sourceとサブ展開snapshot Arcを削除前に照合する。
成功visible idxがある場合だけ旧要求を取消・既存retirement workerへ渡す。
通常Folderは未完成consumerの観測資格を不明に戻し、既存終端rescan→Differentで分類を再準備する。
サブ展開は物理rescanを使わず、削除後のcurrent itemsと既存snapshot／tombstoneから、
元のhydration intentを持つ既存分類入口を再開する。全件clone・sortはUIで行わない。
成功ゼロは元の初期要求を保持する。新pending ownerや完了後のmode切替を追加しない。
この初期入口とサブ展開の同型も独立の設計確認で合意し、両段階の成功／取消回帰を追加した。

回帰は単独Image／先頭Folderの自己Delete→実worker Equal→残存Stackを開く対照と、
flat member削除→単独Image復帰時の編集・★・対応表を含む。
P3 cutoff試験は最初のworker結果を固定後に外部ファイル変更と後続通知を入れ、最初の受理では
captured cutoffだけをackしdirtyが残ること、次の走査で追加ファイルが表示されることを検査する。
P3保持試験は本番worker→Equal pollを通し、非既定の選択・check・scroll・進行中thumbnail要求と
同世代の遅着画像の採用を直接検査する。旧§1.314のテスト専用早期returnには依存しない。
新規回帰8件（handler 7件／純粋reducer 1件）と既存cutoff回帰の強化を行った。
最終sourceの関連テストは272件成功／失敗0（§1.375 36、Stack reducer 28、Stack UI 24、
Delete worker 17、行除去15、rescan 4、context 80、§1.373 68）。
`cargo fmt --check`、通常／portableのcore `cargo check`、UI glyph検査はすべて成功。
`build-dev.ps1 -PreserveRuntime`は通常機能セットのcore／remote／EPUB workerを生成しexit 0、
VCRT検査もruntime 4／PE 3で成功した。共有Delete終端を変更したため`test-full.ps1`を今回1回実行し、
13,539件成功／失敗0／既存ignored 63件、PASS。
最終source 5ファイルのSHA-256は検証前後で一致する。独立の変更境界確認でも追加P1／P2はない。
結果は[最終state](../target/section1375-review-final/state.json)、
[full stdout](../target/section1375-review-final/full.stdout.log)／
[stderr](../target/section1375-review-final/full.stderr.log)に保存した。
今回の変更はsource 5ファイル（`app.rs`／`section375_tests.rs`／`delete_worker.rs`／
`filename_stack.rs`／`filename_stack_ui.rs`）とdocs 3ファイル（本書／`filename-stack-plan.md`／
`async-architecture.md`）。manual／§1.373設計／ビルドscriptは変更せず、コミット・アプリ起動は0件。

### 8.5 masterとの統合（2026-10-14）

利用者報告の独立ACCEPT `7a7041412`へ、設計担当が`master e5f3905ff`をno-commit mergeした。
16ファイルの競合を両側の意図を保持して解消した。indexへのadd、merge commit、アプリ起動は行わない。
既存設計の接続を維持し、新pending ownerや通常Appendの排他を追加しない。

| 競合ファイル | 解消方針 |
| --- | --- |
| `docs/README.md` | §1.353／§1.373／§1.375とmaster側の設計索引をすべて併記 |
| `docs/async-architecture.md` | context別Folder観測とEverything／PDF／本構成の非同期契約を併記 |
| `docs/detached-rework-plan.md` | 両側の§11変更記録を原文保持し、既存context所有の統合を記録 |
| `src/app.rs` | modal条件を併合し、typed refresh originとdefer、観測世代とrecipe、安定targetの受付→保存→publicationを保持 |
| `src/app/folder_scan.rs` | masterの共通生成境界への移設を採用し、走査記録生成も移設先へ引継ぎ |
| `src/app/spread_shift_owner.rs` | masterの同値no-op／実効permissionを保持し、実変更前に既存mutation受付 |
| `src/app/startup_ops.rs` | fork用basis世代、recipe、閲覧状態の保存をすべて保持 |
| `src/app/viewer_context_registry.rs` | FolderObservationとBookCompositionReadを両登録し、観測資源とPDF ownerの独立移譲／終端を保持 |
| `src/filename_stack_ui.rs` | recipeとhydration intentを併せて配送し、Stack採用後に観測世代を再束縛 |
| `src/global_search_ui.rs` | accepted dataset退役、更新時drill保持、Everything優先配送とmetadata検索の完了時照合を両立 |
| `src/remote_ipc/ui.rs` | UndoAppend限定拒否とcomposition資格照合を併合し、新しいreading書込みのenqueue前へ既存受付を接続 |
| `src/tag_ops.rs` | 非空tagの可用性・mutation受付後、masterの先行ページ編集確定を維持 |
| `src/ui_adjustment_panel.rs` | stable draftの変更／非同期producerへ既存受付を接続し、PreparedOnlyとno-opは失効させない |
| `src/ui_dialogs/metadata_transfer.rs` | append writer分類とmasterの公開helperを保持し、readonly Preview待ちとTransfer待ちを区別 |
| `src/ui_text.rs` | append受付回帰2件とfont-card高さ／終端到達回帰をすべて保持 |
| `src/undo_ops.rs` | UndoAppend拒否を先に行い、先行編集確定→履歴判定→通常Undo／Redo受付の順序を保持 |

マーカー外の接点も照合する。`book_materialization`へ精密stamp／row recordを移し、
投影fingerprintはmasterの同名Folder／book policyへ追随する。ZIPのfacet用実効階層は
`ZipNavLevel.display`から取得し、masterのraw book scopeを置き換えない。
Remoteの未回収reading書込みは新vectorの全write variantを数え、readonly resume readは含めない。
安定local target、slider、保留編集Undo、Remote早期writeの統合回帰を追加する。
本構成draft／save／解除とZIP変換の新producerも既存§1.373受付へ接続し、
新owner・追加の待機・全windowの再初期化で統合を回避しない。

追加回帰は10件（補正／Remote 4、本構成／ZIP変換5、ZIP display／raw scope分離1）。
旧fixtureも新しい監視sequence・search producer・stable target・snapshot basis世代へ追随し、
期待値やsnapshot基準は緩めていない。初回の型／fixtureコンパイル失敗は修正後に再検証した。
fmt、通常／portableのcore checkは成功。後述の格納修正後にも両checkを実行し、
`target/merge-master-7a7041412-heap`の成功を確認した。最後の変更は`cfg(test)` fixtureの型参照だけのため、
両checkの結果を再利用する。
関連テストは延べ1,104件成功／失敗0／既存ignored 2件、UI snapshotは176件成功／失敗0。
mutation受付の機械監査はmedia writer 15／その他writer 16／accessor 3／maintenance除外2でPASS、
UI glyph検査も成功した。

指定の`test-full.ps1`は今回1回だけ実行し、exit 101。本体libのAppサイズ回帰で113,200 bytes
（上限110,000）が判明し、同libの`capture_region_focus_and_escape_terminals_preserve_event_order`で
`STATUS_STACK_OVERFLOW`となった。libは中断したため成功件数を推定せず、残りworkspace targetの
完走と区別する。ログは`target/merge-master-7a7041412-complete/full.{stdout,stderr}.log`に保存した。
上限・thread stack・試験期待値は緩めない。既存の書庫／EPUB変換stateをBoxへ移す格納修正を
独立確認し、owner／modal／receiver／lease／取消・終端を変えず、変換開始時の1 allocationへ寄せる。
新owner・state・待機は追加しない。修正後は失敗targetのlibと変換／contextを再検証し、
full scriptは再実行せず、それ以外の有効なworkspace結果を再利用する。
修正後のAppは108,072 bytesで既存上限110,000を満たし、同じcapture試験も成功した。
この2件の再検証では通常stackと期待値を変更していない。
lib全体の再検証は12,782件成功／3件失敗／既存ignored 57件で完走した。
失敗3件はすべてEffeTune native hostの共通Hello／Ready入口（`bridge.rs:2033`）で、
worktreeの旧vendor hostが現ソースidentityと一致していなかった。master worktreeの成果物は
現ソースhash `04e94b0d62c8d4eb0128a708aec1a70c7ecb2c7639d9899fc54f621098a869ad`と一致するため、
既存手順でhostと公式CRT4本を実ファイルでコピーし、コピー前後のSHA-256とコピー先の一致を確認した。
この復元で製品ソース・テスト期待値・通常APPDATAは変更しない。
host PEのsize／SHA-256は[復元記録](../target/merge-master-7a7041412-heap-final/vst-host-restored.json)に保存した。
復元後の当該3件は成功した。未実行だったvendorはegui 25／egui-wgpu 11／eframe 18件すべて成功。
full初回で完走した他のworkspace targetは1,246件成功／既存ignored 7件で、格納のみの修正のため再利用する。
子processが出した重複summary 2件とtargeted再実行の重複は集計へ加えない。
これらとlibの成功12,782＋復元後3件を合わせ、gate構成targetは14,085件成功／未解決失敗0／既存ignored 64件。
full scriptの初回exit 101はそのまま記録し、script全体のPASSへ置き換えない。script実行回数は1回のまま。
修正後のlib・vendor・native hostのログはそれぞれ
[heap再検証](../target/merge-master-7a7041412-heap-final/state.json)と
[native／vendor検証](../target/merge-master-7a7041412-native-final/state.json)に保存した。
`build-dev.ps1 -PreserveRuntime`はexit 0で通常機能セットのcore／remote／EPUB workerを生成し、
VCRT／PE検査もruntime 4／PE 3で成功した。mIVアプリ起動は0件。
検証前後のsource／script 1,471ファイルのSHA-256は一致し、競合マーカーは0件。
indexは要求どおり16ファイル／48 unmerged entriesのまま、HEAD `7a7041412`と
MERGE_HEAD `e5f3905ff`は変更していない。add／commitは0件。
結果は[native／vendor／build-dev最終state](../target/merge-master-7a7041412-native-final/state.json)と
[最終source／index照合](../target/merge-master-7a7041412-native-final/final-source-state.json)に保存した。
`git diff --check`は既存CRLFを改行として扱う設定で成功。独立の接続確認にも追加P1／P2はなかった。
