# §1.375 監視再走査の行signatureとsidecar stampで再表示を省く

2026-10-13、`next-list`、R9。改訂基点 `aa26c9b29`。
**設計担当ClaudeCodeの§7判断を反映。比較相手を「採用表示全体」から「その一覧を作った走査の記録」へ変更し、比較対象をファイル行の投影と取込対象sidecarのstampへ絞る。R9は独立設計レビュー提出用・未実装**。
今回の変更はこの設計書だけ。コード・manual・索引文書・§1.373は変更せず、cargo build／testは実行しない。
R8のLISTrev10 ACCEPTはR8の記録であり、R9の承認／実装証拠には流用しない。
正本項目: [next-release-backlog.md §1.375](next-release-backlog.md#1375-フォルダで-delete-キーで削除すると正しく消えた一覧がもう一度丸ごと読み直されてちらつく--利用者の実機報告-2026-10-12)。

案B・自己操作operation ID・provenanceによる照合は2026-10-13の利用者決定で撤回済み。
R5〜R7の代表選択保持・leaf付きcache key・選択付きpayload・旧cache移行もR8で撤回済み。
R9はそれらを復活させない。§1.373 R4案1のAppend／Undo operation IDと受付契約は独立して維持する。
過去の設計は `ae2772c8e`／`84f31c346`／`13f2fc3e9`／`aa26c9b29` の履歴を参照する。

## 1. 原因とR9の不変条件

Deleteは `handle_delete_key` → 確認 → `start_delete_files` → delete worker → `poll_delete_pending` →
成功pathだけ `remove_items_batch`。終端の `current_folder_signature = None` により、後続watch scanは
既存signature一致分岐を通れず、`apply_external_rescan` → `load_folder_with_scan_owned` で一覧を再採用する。
これはsource inspectionで確定した実行可能経路であり、利用者実機の時系列ログを取得したという主張ではない。

R9の比較相手は画面のitems／DB cache／thumbnailそのものではない。
**同じ走査層が作った前回の行signatureと今回の行signature、前回と今回の取込対象sidecar stampが
ともに一致し、元owner・source・世代・投影条件が有効なら、一覧install・世代更新・install起因の
サムネイル再要求・sidecar取込を省く。いずれかが違う／不明なら従来どおり再読込する**。

`remove_items_batch` は現在の行を除くだけで、隠れた候補を追加しない。
JPG優先の `a.jpg + a.png` からJPGを削除したとき、残ったPNGの再露出は通常走査・投影で検出する。
成功Deleteの既存行除去だけを前回記録へ反映し、走査前に非表示候補の繰上げまで反映済みと偽らない。
自己操作か外部操作かを通知の分類条件にせず、時間窓で通知を捨てない。

## 2. 比較対象を絞る理由と撤回事項

### 2.1 DB由来値は既存publicationで扱う

タグ・★・編集・しおり等のDB由来値は、mIV内の通常操作では既存publication／cache更新で表示へ反映する。
その値をwatch比較用snapshotへ複製する必要はない。具体的な確認とsibling例外は§7.2に列挙する。
publicationの確認ができない場面だけ通常再読込へ倒し、新しいDB更新通知台帳・共有表示storageは作らない。

監視再読込で反映すべき通常の外部ツールの画像・動画等の変更は、ファイルの更新日時／サイズ／種類／名前を
走査した行signatureへ反映する。opt-inのXMP rating書込み（既定OFF）も元ファイルの更新日時を変える。
取込対象sidecarの変更は、そのファイルのstampを別に比較する。
この二つの経路と既存publicationを前提に、DB由来の表示値を比較対象から外す。

**R8の「raw signatureだけの一致は不採用」「採用表示全体をexact比較する」は、この根拠で取り消す。**
R9は既存のraw hashを無変更で使うという意味ではない。行の投影・順序とsidecar stampを含む、
同じ走査層の比較記録にする。DB値・GPU画素・採用thumbnail状態までの完全一致は保証しない。
秒precisionの表示mtimeだけで内容更新を判定せず、既存metadataから得られる更新時刻の精度を記録する。
更新日時・サイズを意図的に維持する外部書換えはstampでは証明できない制限として明記する。
追加の全件decode・画像hash・回復機構は作らない。

### 2.2 代表と既存thumbnail lifecycle

一致してinstallを省けば、表示中のサムネイルは代表を含めそのまま残る。
同じ自動代表を使い続ける保証ではなく、差分reload、eviction後の再読込、idle upgrade、サイズ変更、
cache miss／再生成、RAW継続・既存要求の遅着では、通常の選定・共有cacheから別の代表へ変わってよい。
利用者の「Ctrl+F5まで代表の変更を反映しなくてもよい」は許容であり、固定保証ではない。

代表選択キー／Stack保持identity、Exact／Discover要求、選択付きpayload、leaf cache key、旧cache移行、
CPU画素の追加保持は不採用。手動pin・解除・pin先消失の既存fallbackは維持し、pin設定は勝手に消さない。
画像／保存両終端・request ID、CPU／GPU eviction、keep_range／upload予算、catalog形式は変更しない。
thumbnailの通常採用だけではR9の行signatureを失効させず、既存要求を継続する。

### 2.3 状態の組み合わせを減らす

| 案 | 判断 |
| --- | --- |
| 走査の行signature＋取込対象sidecar stamp | 採用。比較入力が同じ走査層に揃い、DB／thumbnailの採用状態を共有しなくてよい |
| R8の全表示snapshot／部分共有storage／初回採用入力の共有化 | 撤回。DB publicationとsidecar stampで範囲を分ければ不要 |
| 初回準備をworkerへ移して新しいPending行の表示を遅らせる | 不採用。通常の表示順序を変えずに済む |
| 初回だけ常に差分、sidecar設定ONなら常に差分 | 不採用。初回の記録も通常loadの走査で作り、最初のDeleteから省略できるようにする |
| 代表保持・cache方式変更／移行、自己操作ID・provenance・追加pending owner | 不採用。通常利用の費用と所有・移行の組み合わせを増やす |

## 3. 走査記録・比較・採用

### 3.1 前回の記録を作る入口

既存の `ScannedDir` を生成／消費する通常loadとpre_scanで、走査層の軽い比較記録を作る。
既存folder-list owner／ViewerContextBundleが、採用したsourceの前回記録を持つ。
App.itemsをcloneしたsnapshotやDB由来metadataの共有storageから記録を作らない。
signature生成を既存の走査／投影の仕事へ揃え、UIに追加の全件clone・同期I/Oを置かない。

初回のsidecar stampは走査で観測した値を使い、既存hydrationの実取込sourceとの対応を維持する。
後から読んだ「最新stamp」を取込済みと偽らず、hydrationが未完了／sourceが変わった場合は
既存ownerの終端・世代拒否で通常再読込へ戻す。新しいfirst-install ownerは作らない。
既存load経路の記録可否と不足fieldは§7.1に列挙する。

### 3.2 軽いsignatureに含める範囲

| 入力 | R9の契約 |
| --- | --- |
| ファイル行 | sourceに対する行key／名前・種類・サイズ・更新日時。通常の同名候補除去、種類採否、本判定、動画／音声の補助画像などの既存投影を使う |
| 行の順序 | 現在の並び・フィルタ投影後の順序と行数。read_dir順ではなく、同じ投影条件で走査層が作った順序を記録する |
| Stack／展開 | 既存のグループ化・member変化が行／種類／順序へ及ぼす差分。保持用identityやpixel比較は作らない。走査層だけで同じ投影を証明できないモードは通常再読込へ倒す |
| 取込対象sidecar | exact sourceに対応するファイルごとの存在／サイズ／更新時刻。現行の編集・タグ取込は `sidecar::SIDECAR_FILENAME`（`mimageviewer.dat`） を共用する。取込対象の追加がある場合は同じ実際の取込入口から列挙し、任意の全 `*.dat` を探索しない |
| 実Folderのmtime | §1.314により観測signatureだけへ含め、行の比較ではmtime-onlyを無視する。表示日時・sort・代表要求stampへ漏らさない |

既存 `FolderScanSignature` はfull／listingのhashで、path順へsortするため表示順非依存、
秒mtimeであり、sidecar stampもない。R9の契約を満たす記録へ接続する必要がある（未実装）。
行signatureはプロセス内の軽い比較値であり、暗号学的な同一性保証や永続cache形式を新設しない。
DB由来rating／tag／編集／しおり／pin値、thumbnailのPending／Loaded／NoArt／Failed、
画像寸法・layout、GPU／CPU画像、省略chip／tooltip内訳は比較対象にしない。
R8の「全行表示値・省略chipまで一致」の受入条件も撤回する。

DB依存sort／facetが行の投影へ効く場合は、既存の投影条件・publicationで確認できる範囲だけ記録へ使う。
その入力を走査層へ揃えられないモードや条件変更は、既存のmode／設定／世代の判定で通常再読込へ倒す。
採用済みDB値を共有する新しい仕組みでこの範囲を広げない。

### 3.3 Delete終端の記録更新

成功pathの既存行除去と、前回走査記録の同じ行の除去を同じownerで扱う。
走査層の純粋signature更新を既存除去の仕事へ揃え、UIからFS／DBを再読しない。
失敗path・取り消したpathや他contextの記録は除去しない。現行のsignature全面無効化はこの境界で置き換える。
順序signatureも、残った行の相対順を維持した除去として更新する。
独立したDeleteReceipt・operation ID・成功を待つ追加ownerは作らない。

前回記録から除くのは実際に反映した行だけで、隠れたPNG／ZIP／EPUB等の露出を先取りしない。
現在行に成功idxがない場合でも、成功physical sourceに対する終端後の通常走査は要求する。
Delete後は通知の有無に依存せず同じrescanを要求し、既存のrestart予約へ集約する。
再走査で異なる候補や外部追加が見えれば通常reloadへ進む。
sidecar stampはDelete成功を理由に最新値へ書き換えない。違えば通常の取込を含むreloadで扱う。

### 3.4 監視workerと採用資格

`poll_current_folder_watch` → `check_external_folder_changes(Notified)` → `start_external_rescan` の
既存ownerを使い、workerが走査・行投影・sidecar stamp取得・signature比較を行う。
一致判定のために全表示prepare／DB cache／thumbnail stateの共有を要求しない。
差分時は走査済み結果を既存の通常load／hydrationへ渡し、取込も従来どおり行う。
UIで再走査・全行比較・snapshot cloneを行わず、既存の表示順序を保つ。

UIは元context／exact source／items generation／走査・並び・フィルタ条件と、要求が捕捉した前回記録の
有効性を採用直前に確認する。Deleteの記録更新、source切替、sort／facetの変更で旧要求は拒否する。
thumbnail単独採用など、比較範囲から外した値の変化のために新しい表示revisionを作らない。

- 一致し、§7.2の例外なし: 観測signature／親mtime／取り込んだ通知cutoffだけを元ownerで進める。
  install・世代更新・install起因thumb再要求・sidecar取込は0回。選択・check・scroll・既存要求を維持する。
- 行／sidecar stampが違う、投影・stampが不明、既存publicationの反映資格を確認できない:
  通常reloadへ渡す。現行sidecar importの厳密なsource検証・取込規則を緩めない。
- 取消・stale・scan失敗: 元表示を維持し、cutoffをackしない。未照合dirtyを元ownerに残す。
  まれなI/O失敗はログ・通知で閉じ、毎フレームretry／rollbackを新設しない。

明示 `KeyAction::GridReload`／「最新の情報に更新」は常に通常reloadへ配送し、この省略を通さない。
GridReloadは既定キーなし、F5は★5。Ctrl+F5は利用者の呼称／割当で、rawキー判定や既定keymap変更はしない。

## 4. §1.314とcontext／購読／通知の契約

R8のcontextごとの購読・dirty／cutoff契約は維持する。
現行App-global watcher／external rescanを、既存physical-list bundleの観測記録と同じownerへ揃える。
新しい単独pending owner、共有fanout／ack台帳、時間TTLは作らない。
購読identityはcontext ID・subscription epoch・exact folder・listing options。

| lifecycle | 契約 |
| --- | --- |
| 登録と初期coverage | OS登録／登録後scanを既存source／rescan workerで行う。UIへ同期登録を追加せず、購読前pre_scanをcutoff証明にしない |
| park／mount | 元bundleに購読・dirty・前回記録を保持。park中通知も元ownerへ残し、mount後に比較する |
| A/B同folder、A→B→A | 各contextが独立した記録・購読を持つ。Aの一致でBのdirty／記録／要求をackしない |
| 走査中の通知 | kind／path／sequenceを有界dirtyへ残す。cutoff後は既存restartで再走査し、次の新通知を必要としない |
| source終了／context破棄 | 元購読・worker・前回記録だけを解放し、兄弟を失効させない |
| stale／失敗 | 未照合cutoffは消費しない。watch error／coverage不明は通常再読込へ渡す |

kind／pathをsaw_changeだけへ潰さない。Accessは既存どおり除外し、
Create／Remove／Modify(Name old/new)はNamespace、file Modify(Data／Metadata／Any)は内容・属性変更、
対象不明／Other／watch error／overflowはUnknownとして保持する。
WindowsのModify(Any)でも、対象が確定した実directoryのmtime-onlyとfile変更を区別する。
通知を自己由来と認定せず、Deleteと外部変更が同batchでも走査する。

実Folder mtime-onlyは観測stampだけ進める。親一覧の表示日時・sort・代表要求stampは旧採用値を維持する。
この例外を普通file・ZIP／PDF本体へ広げず、sidecar stamp差分やUnknownもdirectory-onlyと偽らない。
Deleteとの混在でも、残った実Folderのmtime差分が追加installを発生させないことを検査する。
Resumedの既存親mtimeふるいは維持し、Notifiedはふるいをbypassする。

## 5. 同型経路と範囲

| 経路 | 扱い |
| --- | --- |
| 通常FolderのDelete／メニュー削除 | ごみ箱／完全削除の成功行除去・前回記録更新・終端rescanへ合流 |
| ZIP／PDF本体の物理削除 | 通常の実file削除。cache／metadata purge・Shell成功判定は既存どおり |
| ZIP内画像／ZipDir／PDFページの削除 | 既存の理由通知で拒否。書庫を書き換える削除を新設しない |
| 検索root／Collection root | 再クエリ・actor revisionは別の正当な更新であり、物理Folder比較で抑止しない |
| Collection・Smart・★一覧等の物理子 | 元のphysical-list ownerの走査記録で扱う。synthetic rootの記録を流用しない |
| 改名／move／copy／D&D／Shell paste | 第一段の終端集約は対象外。後続watchの比較境界は共通だが、第一段完了をこれらの修正完了とはしない |
| 本フォルダ | 既存watch購読範囲を広げず、Resumed／先着rescanと§1.373 R4自動観測配送を維持 |

第一段の受入範囲はDeleteの余分な再表示。
未接続folder_reconcile部品・RawFolderSnapshot・DeleteReceipt等はR5で撤去済みで、復活させない。
§1.373のcause／operation資格／Undo失効除外を一般watchのsignature一致へ置き換えない。

## 6. R9の受入回帰・性能検証計画

今回は計画だけで、実装・回帰追加・性能計測・cargo build／test・アプリ起動・コミットは0件。

- 実Delete handlerの成功行除去後、行signature／sidecar stampが一致すれば追加install・世代更新・
  install起因thumb要求・sidecar取込が0回。通知先着／終端先着／通知なし／反復通知を検査する。
- JPG→PNG、Folder／ZIP、PDF／EPUB、動画／音声補助画像、Stack 3→2／1枚Image化、sort／facet差分。
  前回記録の成功除去が非表示候補を先取りせず、今回の行投影差分を通常reloadへ渡すことを確認する。
- Deleteと外部B上書き／C追加が同batch。fileの更新時刻／サイズ・種類・名前の差分を検出する。
  同一秒内の更新も利用可能なmetadata精度で区別し、stamp維持書換えの制限を明記する。
- sidecarの作成・変更・削除、Missing／読取り不明、設定ON／OFF、scan中変更／pending writer。
  sidecarだけが変わった場合も通常取込へ進み、同じstampで不要な取込を再実行しない。
- 通常load／pre_scanの最初の記録から最初のDeleteを試験し、「初回だから差分」を使わない。
  ZIP／PDF／検索root等の対象外、物理子のsource帰属と、初回stampの不足経路も検査する。
- ★publication／park後mount、本しおりの全context失効、tag完了、page-edit epochを対照検査する。
  §7.2のsibling／snapshotなし等の例外だけを通常reloadへ倒し、新しいpublication機構を作らない。
- §1.314の子.partによる実Folder mtime-only＋Delete混在、typed directoryとfile Modify(Any)の対。
  観測だけが進み表示mtime・sort・代表要求stampが不変であることを確認する。
- A/B同folder、A→B→A、park中変更、cutoff後通知、scan中切替／取消／close／再購読。
  採用直前の元記録・世代・投影条件変更では両結果を拒否し、他ownerのdirtyを消さない。
- 明示GridReloadのAction／メニュー／割当キーは、signature一致でも通常reload／代表再選定へ届く。
  一致省略中の既存thumbnail要求・画像／保存両終端・RAW継続／cache-only／eviction／idle upgradeも維持する。

1万／10万行でscan・行投影・signature生成／比較を測定する。末尾差分、全行差分、Delete除去、
条件変更、sidecarだけの差分を含め、UI enqueue／poll／資格確認とworker時間・割当量・install回数を記録する。
UIの全表示snapshot作成・全件cloneは0回、新しい共有表示storage・全件decode・cache移行も0件。
既存初回表示・scroll・idle／tray・CPU／GPU予算を前後比較し、通常利用に目立つ遅延があれば止める。
headless／fake-inputを先に行い、実アプリの計測は別途具体的なsuiteへの利用者了承を必要とする。
実装完了時はtargeted、fmt、通常／portable core check、build-dev、test-fullを既存検証方針で行う。

**設計書で止め、R9の独立設計レビュー後に実装する。**
まれな失敗は既定で割り切り（ログ＋通知［＋作り直し］）。新しいretry／rollback／回復journalは作らない。
以下§6.1〜6.4はR5〜R8当時の履歴で、R9の受入証拠ではない。

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

## 7. 初回採用入力の共有を不要にする判断 (R9、2026-10-13)

R8実装調査では通常の可視採用後にsidecar hydrationが始まり、そのworkerには行だけが渡るため、
全表示値を共有するstorageか、初回prepare／Pending行の表示順序の変更が必要になり停止した。
当時の記録は `aa26c9b29` の本節を参照する。
**設計担当判断によりR9は比較範囲を走査記録へ絞る。二つの案は採らず、§7の共有境界の判断待ちは解消する。**

### 7.1 初回記録を作れる経路と現行の不足

| 経路／コード | 現行とR9の接続 |
| --- | --- |
| 通常load／startup／pre_scan: `load_folder_with_scan_claimed`、`prepare_scanned_folder_listing_with_cache_keys` | 全てScannedDirを消費し、消費前に既存signatureを生成する。初回の行記録もこの走査／投影層で作れ、items cloneや初回だけの強制差分は不要 |
| Smartの実Folder: `install_scanned_folder_listing` | 同じprepare境界を使う。前回記録のownerはadopt先physical-listであり、Smart rootではない |
| Collection／★等の物理子、DFS／pane／historyのpre_scan | 通常Folder prepareへ合流する。採用したscanの記録だけを元ownerへ渡し、古いpre_scan／却下候補を基準にしない |
| Stack／展開／DB依存sort・facet | 元のphysical scanはあるが、現行signatureに最終順序／フィルタ投影がない。既存投影と同じ行記録を作れる条件だけ比較し、資格不明なmodeは通常reloadへ倒す |
| ZIP／PDF／変換済み書庫／検索・Collection等のsynthetic root | 通常Folder ScannedDirを持たない。物理Folder watchの省略対象外で、既存のreload／query／actor ownerを維持 |
| sidecar stamp | 現行ScannedDirは `mimageviewer.dat` のstampを保持しない。既存scanのentry metadata／既存sidecar workerの観測を記録へ接続する。hidden除外等より前に取込対象を識別し、追加UI I/Oを作らない |
| 現行signatureとDelete | 現行hashは表示順非依存・秒stamp・sidecarなしで、Deleteは全面無効化する。R9の行投影記録／成功行除去へ接続する変更が必要で、現コードで完了済みとはしない |

通常physical Folderの初回scanがそもそも存在しない経路は、今回のコード照合では確認されなかった。
記録を作れないのはsynthetic／仮想rootと、現行の投影／sidecar stamp fieldの不足であり、
全通常Folderを初回差分扱いにする根拠にはしない。stamp不明／scan失敗は正常なMissingと区別する。
sidecar設定OFFのfamilyは取込を省略している既存契約に揃え、設定変更時は通常の明示更新へ進む。

### 7.2 DB publicationのコード照合と通常reloadの例外

| DB由来値／経路 | 確認とR9の扱い |
| --- | --- |
| ★、Remote rating、park→mount | `write_user_rating_shared_deferred_from`／`rating_session_writes`／`publish_current_context_rating_writes` とmountのpublication境界がある。watch再読込を唯一の反映経路とはしない |
| 現在のcontextのタグ | `tag_ops::poll_tag_write_results` が成功後のtag列を `set_tags_cache_entry` へ反映し、必要なfacetをrebuildする。sidecar stampを変えるミラーも既存経路 |
| 同じsourceを持つsiblingのタグ | tags_cacheはBundle別、通常tag完了はmount中cacheだけを更新し、mountにはratingと同じtag publicationがない。**受け手がsiblingで、既存情報でそのtag cacheへの反映を確認できない場面は通常reload**。新しいtag epoch／fanoutを追加しない |
| 編集の既存snapshotあり | `poll_page_edit_reconciliation` はPAGE_EDIT_WRITESのstampを見て既存workerへ再読を要求する。比較用DB snapshotを作らず、このpublicationを維持 |
| 通常Folderの編集snapshotなし／sibling | `poll_page_edit_reconciliation` はsnapshot／stampなしでreturnする。通常hydrationにはDBから値を再取得する経路があり、siblingでその更新が届いたと確認できない場合にreloadで補われ得る。**既存snapshotのpublication資格がなく同source siblingの編集反映を確認できない場面は通常reload** |
| 代表pinのsibling cache | `set_folder_thumb_pin`／`remove_folder_thumb_pin` はmount中folder_pin_mapを更新する。Bundle別のsibling mapへのpublicationを一律保証できない。**同source siblingのpin cacheが反映済みと確認できない場面は通常reload**。自動代表保持を復活させない |
| 本しおり | `poll_book_bookmarks`／`invalidate_current_book_bookmarks_for_container` がmatching contextのprojectionを失効させ、既存serviceへ再読する。watch依存ではない |
| metadata import | `metadata_import_refresh::ContextRequest/ContextResult` がcontext・世代付きの既存読込み／cache反映を持つ。未完了／資格不明な要求をsignature一致で完了扱いにしない |

上のsibling例外は「DB値は全contextへ必ず既に反映済み」と断定しないための保守的な通常reload条件。
既存のcontext／source／snapshot／pending等だけで確認し、反映資格が不明なら通常経路へ戻す。
同じsourceを持たないsiblingまで巻き込まない。単にDBがある、sidecar設定ON、thumbnailが変わった、
という理由で全Folderの省略を無効にしない。§1.314のdirectory-mtime-onlyと、DB反映の資格不明は
別の判定として扱い、例外を実Folder mtime-onlyの不要なinstallへ広げない。

今回の読取りで確認したのは上記のpublication境界と資格不足であり、
「watchがDB変更の唯一の反映経路である」ことを実機で再現したという主張ではない。
siblingへのタグ／編集／pinの反映保証不足は列挙して報告し、当該場面だけ通常reloadへ倒す。
実装時に同型の唯一経路が追加で見つかれば同表へ追記し、個別条件を通常reloadへ足す。
本項目のために新しいpublication／受領台帳／表示storageを作らない。

### 7.3 R9文書改訂の検証

変更は本設計書1ファイル。R9の実装・回帰追加・性能計測・cargo build／test・アプリ起動・コミットは0件。
README／backlog／ui-responsiveness／manualは今回の指定範囲外なので変更しない。
そこに残るR8の全表示比較説明は、R9の設計レビュー後の実装工程で同期する必要がある。
§1.373の既存コード・設計・manualには触れない。
本書1ファイルのUTF-8 strict decode・節構造の確認と `git diff --check` が成功。
指定範囲外のcode／manual／索引文書／§1.373設計のHEADとの差分は0件。
