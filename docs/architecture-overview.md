# アーキテクチャ概観

## 本のitems生成境界（§1.242 G1）

`app/book_materialization`に既存ZIP階層、PDF/EPUB page row、folder listingの生成を集約する。
source enumeration、policy、raw manifest、cache/lease、既存PreparedPageIdentityIndexとadoptionは
従来のownerを保つ。ZIPは直下ページblockだけがsurface間で一致し、子containerの位置は従来どおり。
B2の`book_composition`はpure record・可視実列resolver・ページ指定編集を提供する。
保存するのは方向・表紙on/off・順序・hidden・ページoverrideだけで、組は通常の共通builderから派生する。
G2 part Bは単一workerの`book_composition.db`、lifecycle、metadata v11転送とread登録を追加する。
G3はraw manifestと保存構成からworkerで共通projectionと回転入力を作る。items・identity index・回転・構成binding・着地・facet列を採用してから提示許可を作る。回転memoのcold時も採用済projectionのページキー別入力を使い、UIでRotationDbを読まない。
G4の`app/book_composition_editor`はprocess共通の操作ownerと送信済みsave ticketを持ち、
draft/phase/viewは一つのViewerContextBundleに所属する。一時mount/兄弟描画ではownerを移さない。
`book_composition/editor`は純粋draft・undo/redo、`ui_composition_editor`はI/Oなしの
virtualized一覧と固定操作行を担当する。source準備/保存前照合は専用worker、thumbnailはeditor寿命の
独立worker/channel/cancelへ分け、背景viewerのitems/cache/queueを変更しない。
結果生成はsession workerが最新generationだけを採用し、編集中のundo/redoを止めない。
main「製本」menuとHUD overflowの入口は同じ開始受付へ通す。後者はキーより前のroot surface受付と
Eのframe-local wheel ownerを使う。保存完了ownerは各窓のreloadと共有resume intentの一回writeを分ける。
送信済保存票は所有窓の退役後もmetadata preview/import/exportとGeneric改名の開始を保護し、
同じ完了ownerの採用後だけmaintenanceへ進む。folder sidecar restoreの従来開始条件は変更しない。
§1.242はnext-bookで開発中、2026-10-13利用者決定の次のrelease向け。G4Fは2026-10-12に切り離し、
画像だけのfolderは構成対象外。folder閲覧・個別編集・sidecar v1は維持する。
画像folderのZIP変換は、process単位の `app/image_folder_zip_operation` が一つのmodalとRemote admission leaseを所有する。
先に受理した保存票・writerの完了を採用してから、既存sidecar/tag resourcesを単一workerへ移す。
`image_folder_zip_job` が既存FIFO/service owner経由で利用者データを複製し、Stored出力をno-clobber公開する。
元folderの補充だけをgeneration/key限定のbounded refreshで採用し、F12起点では元の閲覧を維持する。
一覧を持つ起点だけ親一覧を更新し、可視なZIPを選択する。Collectionは元の画像を指したまま。
親の受付済みpane scan ownerはoperationのParentScanへ移譲し、通常frameと応答を共有しない。
成功scanの実directory判定をprepared adoptionへ渡し、選択/完了票は既存hydration continuationが所有する。
監視reloadはmodal中の既存pendingへ留め、source補充refreshは集約面も含めitem/page keyで対象を決める。
取消・稀な失敗では元データを巻き戻さず、destination公開と複製を止める。詳細は [画像folder→ZIP設計](image-folder-to-zip-plan.md)。
共通model/storeのfolder値は保持するが、物理画像の提示・類似preview・Remoteはその構成行を消費しない。
Eのfolder scope/shiftは引き続き使用し、構成管理の表示や操作拒否へ流用しない。
G5は別stage。
明示targetは番号の採用前にidentityを捕捉し、表示候補だけをcontext所有の非同期permission ownerへ渡す。
列挙待ちの再開要求はsource完了境界が所有し、完了後の構成待ちだけ提示許可ownerへ移す。
構成pollは未完成のsource要求を消費せず、採用時にscope/context/旧世代を照合して継続する。
required HiddenもMissingと同じ所有context限定の初回open終端へ通し、detached session/runtimeを終了する。
mode-onlyは休眠bindingと表示分類を分離して読取り、方向列・親表紙・文書由来の既定を解決した後で分類を適用する。
構成解除・PC再入場・Remote再入場は同じ純粋resolverからmodeと方向を受け取り、Singleにも方向を保持する。
EPUBの解除はsourceが保持するReadTarget/世代leaseの方向、PDFは投影workerの列挙結果を使う。
通常のAutomatic再採用は構成解除causeを発行せず、snapshot復帰のsession shiftを保持する。
Checking/Hiddenはcached textureも描画に使わず、抽出結果の行生成/thumbnailに構成lookupを追加しない。
類似候補の長押しプレビューは既存preview ownerで候補のidentityをservice FIFOへ渡し、
workerで受け取った許可tokenをtexture採用前に照合する。連結でも通常の可視unit受付を共有する。
投影前の未保存trimと通常trim保存・reset、Remoteのtrim変更は同じservice FIFOへ送り、既存view_trim.dbへ保存する。
Remoteは要求ownerがworkerの保存結果を受け取ってから成功を返し、既存の左右連動切替と応答値を維持する。
キュー受付から完了まで既存Edit ledgerのguardを保持し、古い投影workerによる新しい編集の上書きを防ぐ。
保存キー`BookScopeKey`はラインEの所有であり、
このmodelはキーを定義せず、参照種類/直下階層の検証用`PageDomain`だけを受け取る。

itemsの採用時に排他的な`ProjectionRecipe`を確定する。抽出結果かどうかを現在のanchorや
synthetic pathから推定しない。recipeはViewerContextBundleのmount/forkとsnapshot保存列へ登録し、
物理本へのdrillと抽出rootへの復帰を区別する。A2の表示種別は既存spread/continuous ownerが担い、
新規openはSingle、一時表示の方向はsession snapshot、表示設定DBには結果用keyを生成しない。
連結方式（flow）はA2のpairing初期化と分離し、各入口の従来の採用規則を維持する。タグ・お気に入り・評価・サブ展開・Smart新規rootは既存の保存flow（旧Vertical表現を含む）または従来の分類に対応するdefaults（通常はNON_BOOKのPaged）を採用し、GlobalSearch・Smart保存root復帰・Collection root・固定snapshotの直接列差し替えは既存flowを維持する。単ページ化・再openだけを理由にflowを変更せず、連結読みの操作を維持する。旧Vertical mode行はflow列に別値が残っていても従来どおり縦連結へ復元し、旧行は変更しない。
元の本の列を保存する既存SnapshotStateは閲覧状態（mode/方向/flow/shift index/補助設定）も
一緒に保持し、固定解除の同じadoption内で復元する。BSで固定列へ戻る場合はA2のまま。
検索のback/BS・履歴の本への復帰は既存Book設定adoption、F12はcontext内snapshot/bundle、
Remoteは本の通常responseから復元し、A2のSingle/一時Spreadを本へ保存しない。


## Everything検索の所有（§1.374）

Everything ファイル名検索（§1.374、実装中・検証待ち）は `everything_ipc`
（pure codec / Windows不可視reply-window transport）と`everything_results`
（対応形式/pathの検査とworker stat）を追加し、一覧のownerは既存 `GlobalSearchState` と
`TopLevelGridView::Search` に置く。`SearchSource`、排他的pending `SearchProducer`、
accepted `SearchAccepted` / `EverythingPosition` がdraft・取得中・表示中の位置を区別する。
`SearchPageEditPrepare` がimmutable datasetからsort/★/tag/編集情報を準備し、既存adoptionへ渡す。
通常thumbnail・物理本open/back・F12 fork/park/promoteを利用し、検索専用の第二items/cache/worker群を作らない。
Everything本体/SDK DLLは同梱せず、同PCで起動中の既定clientへ直接IPCする。
IPCのHWND/send/pump/packetは検索ごとの`everything-ipc` runが一括所有し、検索prepare workerは
完了event/取消eventで待つ。native sendが遅くても取消は先に返し、旧runは有限deadlineで自然退役する。
[遅いsendと取消の契約](everything-search-plan.md#17-wm_copydataの遅い送信と取消2026-10-12)。
raw10,000件・明示送信・完了後一括採用・mainの取消可能modalを使う。
新設定field/DB/schema/検索履歴/Remote APIは追加せず、query/result一覧は永続化・診断ログへ保存しない。
PDF共有poolの失効は先行改修 `4510346f5` / master `c1e66f589` のviewer-context ownerを使う。
最新実装の独立レビューと自動/実機検証の受入は未完了。
[仕様・履歴・検証台帳](everything-search-plan.md#14-実装記録2026-10-12検証待ち)。

## PDF render epochのviewer-context所有（§1.377）

未リリースPDF-EPOCH-OWNERSHIP: PDF renderの失効権限は非Cloneの
`PdfRenderEpochOwner` としてApp投影/ViewerContextBundleが一つずつ所有する。
worker/Jobはimmutable ticketだけを持ち、失効とViewer優先昇格はそのowner/要求世代に限定する。
registryの値移譲はidentityを保持し、forkはfresh owner。共有pool内maintenanceがpruneを担当し、
UIはatomic＋非blocking wakeだけ。fullscreen本体・独立backgroundは明示Unscoped。
[後継設計](pdf-render-epoch-ownership-plan.md)。

mimageviewer 全体の構造を俯瞰するための入口ドキュメント。**修正作業の前に必ず目を通すこと**。
個別の詳細は下の「関連ドキュメント」にある専用ページに任せる。

一覧の種類・拡張子の採否は `file_type_visibility` が一つのpure policyとして所有する。
一覧producerは同名除去・本判定・集約・代表選定より前にこのpolicyを適用し、facetは
その後の表示条件として組み合わせる。PCは準備workerにimmutable snapshotを渡し、
Remoteは次の一覧・読出し要求から最新設定を使う。読書位置の論理ページidentity mapは
既存sidecar hydration ownerまたは各一覧producerのworkerで最終行順と対で準備し、
各viewer contextで所有する。退避した親一覧・Collection root・固定一覧にも同じimmutable
対応表を保持し、行削除は共有する論理identityを再探索せず整数slotだけを変換する。
本内ブックマークの投影rows・container/採用済みpolicy key・typed request・編集draftも
既存ViewerContextBundleが所有する。共有DB actorの応答は既存context tableで要求元へ
配送し、worker結果はそのcontextの世代と採用済みpolicyで採用する。通常のmount/depositは
保持し、登録変更は同じ本を投影するownerを失効させる。異なるpolicyの兄弟reader間で
要求を共有・置換しない。変換書庫の再読込は受入済みの物理cacheと既存Collection/Rating
return ownerを保持する。共有open admissionの種類判定だけは、同じcontextが保持する
current_folderと元書庫overrideの対応から論理sourceを使う。元書庫がなくても既存のF5
cache再読込を維持し、別のpathや兄弟contextへaliasを適用しない。
集約一覧の`ArchiveVisibilityProjection`は元書庫参照と物理cache参照を型で区別する。
元書庫のcurrentnessを確認するのは前者だけで、後者の実在memberは実cacheのmanifest／
legacy規則が可視性を所有する。物理パスの通常／Windows verbatim表記は`path_key`の
I/Oなしhelperで比較し、`archive_cache::is_under_cache_root`とcache対応表の照合で共有する。
代表ページとRAW先読みはこのcache-root判定を使い、既存DBキーと公開addressは保持する。
来歴と状態遷移の正本は [表示するファイル種類](file-type-visibility-plan.md)。
動画・音声の補助画像は表示行とは独立した`AuxiliaryMediaCandidates`として既存走査で保持する。
sourceの種類採否と派生動画判定後、producer既存のsort／走査順で採用し、画像種類の非表示で
採用候補を変えない。候補metadataを既存signatureへ含め、追加I/Oやcontext ownerを作らない。

実Folderの自動観測は、前回走査の軽い行記録、購読、dirty／cutoff、走査中要求を
同じViewerContextBundleで所有する。既存rescan workerで種類採否・同名除去・順序を投影して
前回記録と比較し、一致なら一覧install・世代更新・install起因のthumbnail再要求を省く。
採用時はcontext／exact source／一覧世代／投影条件／記録revisionを照合する。
forkはimmutableな行記録だけを継承し、購読・dirty・要求を共有しない。
自動originは既存RefreshList intentからhydrationまで運び、sidecar取込だけを外す。
明示GridReload／通常openは従来の取込とreloadを行う。
契約と受入記録は [§1.375 R10](section375-operation-reconcile-plan.md)。


検索索引の所有判定は `metadata_ownership`、metadata の構成変更は
`metadata_reconfiguration`、名前索引の root 所有は `name_index_manager` に集約する。
UI は snapshot 提出と進捗参照を行い、停止・join・DB cleanup は各 manager worker が担う。
起動確認の省略には既存の `fts_meta.db`・`search_index.db`・`similar.db` 内の追加テーブル
`scanned_once` を使う。fts/name は root 単位、similar は有効 root 集合全体で1つの完走印を持ち、
永続ストアの追加や保存先の変更はない。

live一覧のcursor／Shift起点は `App::move_grid_cursor(target, GridCursorIntent)` が所有する。
入力callerは明示Move/Extendまたは受理no-opのExplicitKeep、自動採用callerはAutomaticを
必ず指定する。明示入力は同context・一覧のrating履歴、pending復帰、ring内Grid originの
復帰権限を失効させてからcursorを公開する。クリックpolicyはローカルコピーから行先を
計算し、このownerへ渡す。identity remap／contextの値転送は対を保つ既存非navigation経路。
詳細と全caller分類は [評価filterのfullscreen整合](rating-filter-fullscreen-plan.md)。

ページ補正の編集内容・選択レイヤー・gestureは `ViewerContextBundle` が所有する。
`PageEditOwner` の安定した `PageAdjustmentTarget` が保存先を決め、items番号は描画投影だけに使う。
同一覧の採用時にownerのslotを再解決し、入力中はその小さなslot集合へdraftを投影する。
App共有のUndoへ次操作を受理する前に全contextの完了済み保留を確定する。
local writerの最新受付済み全文を新規runのbeforeに使い、completionはrequestごとのoriginへ配送する。
localの受付／最新成功完了は同keyのidle contextへsnapshot presenceとshared pending由来の
volatileを反映する。非resident ownerも対象とし、同keyの未確定draftは上書きしない。
通常Pageも受付成功を同keyの全contextへbatch反映し、parked代表サムネの更新通知を
表示中contextだけで消費せず、既存退去境界で全所有contextへ配送する。
通常終了は全contextの要求配送→既存writer停止→completion mirror→sidecar flushの順。
詳細は [ページ補正の所有設計](page-adjustment-owner-plan.md)。

---

### 音声サムネイル (§1.347、2026-10-08)

`audio_album_art` は先頭 ID3v2 タグ限定の bounded PIC/APIC reader、`audio_thumbnail` は worker stat・選定・decode・catalog を共通化する。動画の同名 sidecar discovery / 設定を Audio に拡張し、一覧出所は既存 `video_thumb_overrides` に統一する。Local / Remote は共通 I/O owner と catalog admission を使い、再生 decoder は変更しない。Remote IPC は統合版 v70（§1.345 v68＋master v67）、永続コレクションも snapshot / navigation 準備で thumbnail address を確定する。Web の NoArt / Failed は DOM 外の一覧 owner が持ち、明示 refresh で epoch と HTTP URL を更新する。[詳細設計](audio-album-art-plan.md)。


## 1. レイヤー構造

```
┌──────────────────────────────────────────────────────────────┐
│  UI 層 (eframe + egui, wgpu バックエンド)                     │
│   - メインビューポート: グリッド (ui_main.rs)                 │
│   - フルスクリーンビューポート (ui_fullscreen.rs)              │
│   - オーバーレイ: 補正 / 分析 / 消しゴム / 隠蔽 / メタデータパネル │
│   - ダイアログ群 (ui_dialogs/、Ctrl+E は export_dialog.rs)      │
└───────────────┬──────────────────────────────────────────────┘
                │ App の public メソッド経由で状態を更新
┌───────────────▼──────────────────────────────────────────────┐
│  アプリ状態層 (src/app.rs の App 構造体)                       │
│   - items / thumbnails / fullscreen_idx …                     │
│   - 各種キュー (reload_queue, heavy_io_queue)                  │
│   - 各種キャッシュ (fs_cache, edit_result_cache, final_composite_…) │
│   - 通信チャネル (tx/rx, cancel_token, scroll_hint)            │
└───────────────┬──────────────────────────────────────────────┘
                │ LoadRequest を push / ワーカースレッド spawn
┌───────────────▼──────────────────────────────────────────────┐
│  非同期ワーカー層                                              │
│   - サムネイルワーカー (通常/重 I/O の 2 系統)                 │
│   - RAW 現像 executor (App 共有、優先度付き 1〜10 threads)     │
│   - フルスクリーンロードスレッド (1 画像ごとに spawn)          │
│   - PDF ワーカープロセス (--pdf-worker、設定 3〜10、既定 5)     │
│   - EPUB → PDF 変換プロセス (1 冊につき 1 子プロセス)          │
│   - 外部受け渡し実体化ワーカー (ZIP/PDF/編集焼き込み + 起動)   │
│   - Susie 32bit ワーカープロセス (mimageviewer-susie32.exe × N)│
│   - TRT エンジンビルダー (--tensorrt-build サブプロセス、初回1回)│
│   - TRT 推論ワーカー (--tensorrt-infer-worker サブプロセス × 1)│
│   - AI 推論スレッド (ort + DirectML、TRT は別プロセス経由)     │
│   - VST3 host bridge (ユーザー VST と音響調整で最大 ×2)    │
│     [docs/vst3-integration.md](vst3-integration.md)            │
│   - 動画サムネイルスレッド, フォルダナビゲーション, etc.       │
└───────────────┬──────────────────────────────────────────────┘
                │ デコード結果を mpsc で返す
┌───────────────▼──────────────────────────────────────────────┐
│  データソース                                                  │
│   - ファイルシステム (画像 / ZIP / PDF)                        │
│   - image crate + turbojpeg (JPEG) + WIC (HEIC/AVIF/JXL/TIFF)  │
│   - LibRaw (RAW の preview / half / Full)                      │
│   - PDFium (pdfium-render、別プロセス)                         │
│   - SQLite DB 群 (catalog / rotation / adjustment / mask /     │
│     conceal / spread / pdf_passwords)                          │
│   - ONNX モデル (upscale / denoise / inpaint / classify)       │
└──────────────────────────────────────────────────────────────┘
```

**鉄則**: 上位レイヤーから下位レイヤーへの呼び出しは OK。逆方向（ワーカーが UI を直接触るなど）は禁止。
ワーカーから UI への通知は必ず mpsc チャネルで行う。

本追加の直後一回取消（[§1.373、R7実装・検証中](section373-book-append-undo-plan.md#77-r7-metadata初期化をファイル公開前へ移す設計担当決定2026-10-13)）も、
実行中のownerは既存の `book_op_pending: Option<BookOpPending>` 一つだけ。
`BookOpIntent::Append` がoperation ID・source context・受付revision・対象folderを捕捉し、
ファイル追加、edit/semantic metadata copy、必要な一覧refreshの終端まで同じ要求を保持する。
本rootや追加先の後続変更は元の出力先を付け替えず、受付時にreceiptを失効させrevisionを進める。
処理の遅い成功が古いrevisionのreceiptを復活させることはない。
R7では同じAppend workerが各destinationの八metadata storeを同期初期化し、commit／epoch終端後に
画像を作成する。ファイルの出現が公開点で、後続ユーザー編集は常に初期化より後になる。
copy reportの値でcontext cacheを上書きせず、global存在集合は既存stable readerで現在DBから
worker再取得し、既存Append receiverが対象keyだけ更新する。context値・draftは変更しない。

source contextは取消receiptの資格を決める。refreshの一覧採用資格は別に、各受け手の
`BookAppendRefresh.adoption_owner`・items世代・folder・scan設定と同一operationへの所属で決める。
同じ本を表示する別contextも既存のscan/refresh ownerで更新し、一覧採用先へreceiptを移さない。
受付済みのmetadata copyや自分のrefreshだけをoperation同一性で区別し、通常のsource置換や
後続mutationは失効境界を通す。追加中の対象folderの自動観測はAppend終端へ配送するR4方針であり、
別のpending receiver、Undo中bool、refresh再開ownerは設けない。

`UndoAppend` は同じ本操作ownerでworkerの検証・ごみ箱移動・成功pathのmetadata purge・UI終端を保持する。
新しいローカルmutation／Remote書込みのBusyはこのintentだけを条件とし、既存のSourceMutationや
本改名・削除中の通常タグ／評価の受付は変更しない。Ctrl+Zは `UndoLastAction` からreceiptを先に扱い、
処理中・不一致による拒否でも同じ入力を古いmetadata Undoへ渡さない。ファイル取消をmetadataの
`UndoStack`へ積まず、追加のRedoも作らない。検証・Shell操作・DB purgeはworker上で行う。

F11中の固定chrome一時抑制は `ui_helpers::{ViewerChromeSurface, ResolvedViewerChrome}` の純投影で解く。
保存ロックはSettings / viewer contextが所有し、eguiの正確なwindow bindingとnative coreのplacement・既存host identityが
実際の表示先を所有する。nativeのraw policyとhost factを一つのsnapshotで運び、適用先でdraw / layout / hitを揃える。
独自のplacement遷移・世代・復元stateは設けない。詳細は [決定仕様](fullscreen-locked-chrome-suppression-plan.md)。

指定サイズfit（§1.342）は `fullscreen_specified_scale` の純粋計算を各 viewer の幾何へ渡す。
canonical aspect と AI 前の物理原寸を分けた型付き descriptor を live / frozen / capture が共有し、
PDF は検証済みページ metadata から固定原寸を解く。描画 raster は sampling にだけ採用し、
fit 基準へ戻さない。連結の基準 unit と PDF の絶対需要比較は既存 viewer bundle と一緒に
移送する。共通指定値と favorite 専用値は既存 overlay / 保存 worker を使い、欠落継承だけで
専用値を書き込まない。実装・検証状況は [指定サイズfit計画 §15](fullscreen-specified-scale-plan.md#15-実装接続と検証結果2026-10-09) を参照。

紙めくり（§1.341、2026-10-11）はA0の実機不合格を受け、R7設計のStage 1を実装する。
`ui_fullscreen/page_turn.rs` の `PageTurnPresentation` を既存 viewer bundleで所有し、
各頁のnormal geometryとlayoutの背を `page_turn_animation.rs` のpure book-leaf projectionへ渡す。
通常prepareの全unitをflat/leafのどちらか一度だけ同passへ提出する。部分leaf receiptは頁ごとの
可視性だけを表し、ready/navigation完了にはcanonical flatの採用を必要とする。割込み時の
一つの解決済み操作は同じpresenterのCompletingで保持し、close→採用通知/commit→fresh snapshotの
共通境界へ渡す。commit後の媒体/描画owner変更は旧canvas/footerを終了して既存経路へ戻す。
navigation/load admission、配送先とstable operation ID、F12の既存動画拒否は通常ownerを維持する。
旧素材のleaseは実draw batchまで保持し、App共有のweak一覧はparked/in-flightを含むVRAM会計だけを担う。
既定OFF、時間は利用者評価用の仮値。次の次の版へ延期し、実装の独立完了レビュー・実機/性能受入れは別gate。
範囲と検証の正本は [紙めくり計画 §19](page-turn-animation-plan.md#19-stage-1--通常頁の実装と早期motion評価2026-10-11)。

---

## 2. モジュールマップ

§1.165 B1（2026-10-21、実装・自動検証済み・独立完成レビュー／実機未確認）は crop 済みの編集 preview を
部分素材として扱う。収録範囲は生成時の実整数 pixel bounds から求めて preview と同時保存し、
現在 crop から復元しない。一覧は crop 後の比率を使い、部分素材のページ構成・座標基準には
crop 前の正規寸法を供給する。正規座標の元寸法、実 raster 寸法、PDF page-box の layout 寸法は別物である。
PDF の既知 page-box cache 値も preview の独立 nullable layout 列へ同時保存し、preview hit から
復元する。旧行／通常画像は `None`。layout のための追加 PDF IPC や preview の待機を加えない。
正準座標の `CanonicalPageDimensions` と100%表示の `DisplayOriginalDimensions` を分ける。
通常完成素材はGPU clamp後／AI後の実texture、部分代役は生成時のcrop前rasterを100%基準にし、
`FullscreenFitInput::Raster` でcapture／replayへ保持する。crop samplerの丸めは幾何へ逆流させない。
見開きの左右構成と指定方式の基準は共通のレイアウト寸法境界から選び、片側だけ crop の場合も
通常 thumbnail の texel 数を外側の構成へ使わない。既存4方式の全面素材の単ページ sizing は保つ。
指定倍率の通常rasterだけは単ページ・見開き・連結読みで同じA/O resolverへ統一する。
本画像未到着時の100%基準は生成由来のcrop前raster、またはproducer共通のGPU clamp予測値であり、
thumbnail実寸やclamp前の正準座標寸法を流用しない。AI後もAI前raster基準を維持する。
100%と方式別layoutの所有者は素材自身の `MaterialDisplayBasis` とする。producer selectorは
handleとdescriptorを持つResourceを返し、保持presentationの選択はPayloadとして別に行う。
Loaded／派生cache／通過素材／消しゴム下地から保持・captureまでdescriptorを継承し、
fit時のidx探索やtexture IDからのproducer逆引きは行わない。Single／Spread／Continuousの
旧4方式の契約差と指定A/Oは一つの不変descriptorから射影し、live fit設定は含めない。
Unknown の旧行は一覧で保持する一方、現在 crop ありの
fullscreen／通過／animation には共通 typed policy で使わない。詳細は
[表示パイプライン §1.3.1／§2.1.1](display-pipeline.md#131-非破壊編集プレビューキャッシュ)。

素材の由来は `ThumbnailState::Loaded`／CPU 下地から既存の `FullscreenPaintResource`、
`FullscreenPresentationResource`、paint payload へ値として継承する。共有純粋 helper
`source_region_texture_quad` が coverage・表示要求・素材内 native ROI を合成し、全画像 transform
上の頂点と texture UV を返す。ページ全体の下地と `PageMaterial::PartialTexture` の画像 domain を
分けて、通常描画／紙めくり／passive snapshot 再描画／navigator／ルーペ／パノラマ navigator／
保持素材復元へ接続する。capture 済み素材を現在の thumbnail metadata で置換しない。
既存 resource owner に載せることで新しい idx 別 map、preview 失効の同期化、全 context reset、
専用 worker を増やさない。まれな保存失敗は既定の割り切りとし、多段の復旧処理を追加しない。

### コア

| モジュール | 役割 |
| --- | --- |
| `main.rs` | `windows_subsystem` 属性と `mimageviewer::run()` 呼び出しだけを持つ薄い実行ファイル入口 |
| `lib.rs` | アプリの単一 crate root。全モジュール宣言、logger / eframe 起動、worker サブコマンド分岐を所有し、unit test・integration test・実行ファイルで同じコンパイル結果を共有する |
| `app.rs` | `App` 構造体と `eframe::App` 実装。非描画の`logic`と、root Uiを受ける`ui`へ状態遷移を接続する |
| `app/update_phases.rs` | service・input・render・post-renderの既存順序を所有。表示採用とGPU uploadはUI側へ残す |
| `app/close_return.rs` / `app/root_lifecycle.rs` | teardown前に捕捉したClose帰還を存続mainの既存navigation ownerへ移し、root Closeをlogic直後のbackend終了判定へ接続する |
| `crates/startup-diagnostics` | launcher/core共通の常時起動timeline。UIは固定容量のmemoryへ非blockingで公開し、独立writerがJSONLを保存する。UI heartbeatと独立した限定watchdog、同runの引継ぎ、保存先分類を所有する。詳細は [startup-diagnostics-plan.md](startup-diagnostics-plan.md) §3・§10 |
| `ui_startup.rs` | 起動stage/経過時間と「検索の準備中」の純粋描画。Indexerの5秒overlay上限・worker継続・単一採用はAppの `IndexerInit` が所有する |
| `raw_format.rs` | 23 種の RAW 拡張子の単一リスト。フォルダ列挙はこれを含み、WIC はこの集合を拒否する |
| `details_name_colors.rs` | 詳細一覧の名前だけの六分類・設定・既定/カスタム色・コントラストを純粋に導出。I/Oや行状態cacheを持たず、選択/チェックは共通色、強めは固定色。設定UIと描画が背景・検査を共有する |
| `raw/{raw_decoder,executor,brightness}.rs` | LibRaw の安全な info / preview / Full・half 現像、固定明るさ処理、優先度と取消を持つ App 共有の現像 executor。RAW は各入口で他の画像デコーダより先に分岐する |
| `crates/libraw-sys` | vendored LibRaw との Windows FFI 境界と native build。非 Windows は safe API の Unsupported を返す |
| `cut_clipboard.rs` | Windows の現在の file clipboard を App 単位で観測し、実ファイル / 実フォルダの cut 表示 snapshot を所有する。message-only window の通知 thread と OLE reader threadを分離し、UI は正規化済み `Arc<HashSet>` を可視項目の実パスと照合するだけにする。mIV の cut data object は private token と Shell の完了 format を同じ reducer へ返し、古い通知・読取・callback が新しい clipboard を上書きしない。viewer context やファイル / DB には cut 状態を保存しない |
| `app/viewer_context_registry.rs` | main / detached / parked viewer context の唯一の bundle 保管先。`ViewerContextId`、`ContextResidence`、window binding の双方向表と、mount / build / fork / retire / promote の 5 transaction を所有する。`App` の viewer field 群は常に registry が選んだ 1 context の投影であり、active / parked は bundle の保管場所ではなく detached window runtime state が表す。窓 ID は App / bundle に保存せず、mounted binding（build 中は非公開の予約）から導出するため、一覧差し替えや表示終了で所有窓が変わらない |
| `app/vram_accounting.rs` | `App` が所有する全 GPU テクスチャキャッシュを、実寸・mip chain・`TextureId` 重複排除で横断集計する。サブシステム別会計、モード判定、共有予算の参照、1 秒間隔の perf 計装を担当する |
| `app/folder_scan.rs` | 通常実フォルダの列挙と、1 物理フォルダ内に限定した同名メディア / コンテナ正規化の所有者。動画 + sidecar 画像、実フォルダ + ZIP/PDF/対応アーカイブ、ZIP + 変換元アーカイブ、画像拡張子優先度の規則を通常一覧・サブ展開・スマートフォルダで共有する |
| `same_name_folder_book.rs` | 同名の実folder/本の「本を表示 / フォルダを表示 / 両方表示」を適格な候補へ適用するpure policy。通常一覧・Smart・サブ展開・Ctrl+↑↓/DFS・Remoteで共用し、再帰探索を刈らない。旧`skip_zip_if_folder_exists`のON→本表示/OFF→両方表示を、Settings・Remote subset・preferences transferの共通decoderで読み替える |
| `app/startup_list_restore.rs` | 「前回終了した場所」の明示一覧 record と唯一の reducer。main の採用済み物理一覧／本／Drive の適格性を投影し、明示一覧採用、既存一覧の表示要求受理、同じ一覧の cursor 捕捉を分ける。直接読書・内部 load・合成 root・F12 表示先切替・independent viewer は target を更新しない。既存 typed request が intent を運び、ZIP prefix を含む restore hydration は最初の materialization に渡す。詳細と検証状況は [起動復元の所有設計](startup-restore-target-plan.md) |
| `app/native_video.rs` | Windows native video presenter から戻る overlay event / key / mouse / marker / VST3 操作の App 側処理。native Touch は render overlay 内で完結し、App の legacy mouse 操作へは再注入しない |
| `touch_input.rs` | 静止画 egui viewport と native video presenter が共有する、接点集合・所有・tap zone・pinch/pan/scroll の純粋な認識器 |
| `touch_debug.rs` | `MIV_TOUCH_DEBUG=1` の入力源診断。Win32 pointer/mouse source に加え、native presenter / HUD の stream 所有、座標変換、認識コマンド、promoted mouse 破棄を source 別に記録する |
| `app/recursive_snapshot_scan.rs` | 複数実フォルダを再帰列挙する snapshot view 共通 walker。cancel、深さ上限、reparse point 回避、重複 root 排除、`GlobalIoSemaphore` / `ActivityGate`、chunk sort をサブ展開とスマートフォルダで共有する |
| `app/subfolder_expansion.rs` | 現在地以下の画像 / 動画、ZIP/PDF 本体、設定上の画像フォルダ本を平坦化する一時 snapshot view。画像フォルダ本は通常一覧と判定述語を共有し、設定 OFF では画像を個別項目に保つ。共通 walker で走査し、prepare worker で metadata・コンテナピン・表示順を構築する |
| `app/top_level_grid_view.rs` | 通常一覧、検索、★固定、サブ展開、スマートフォルダ、閲覧履歴、レーティング一覧が共有する最上位 grid surface の単一 ownership と完全な復元 snapshot。スマートフォルダでは root の表示順、現在 entry、entry 内 current / Backspace stack を所有する。既存の個別 active flag は描画互換用に残す |
| `app/smart_folder.rs` | 保存済みスマートフォルダの複数ルール / root 走査、フォルダ / 画像 / 動画 / 音声 / ZIP / PDF / 対応アーカイブ収集、物理フォルダ単位の同名正規化、ルール OR 条件適用、現在の全体ソート順と保存済みグループ化単位による prepare、snapshot 履歴復元、進捗 / cancel / stale generation 破棄を担当する。同名動画 sidecar は full-path key の snapshot として表示へ渡す。prepare worker は exact key の編集状態、変換アーカイブ対応、catalog、固定代表を一括準備し、UI install に同期 DB I/O を残さない。ソートだけの再構築は同一 snapshot generation の正規化 key 配列・採用 entry index・sparse metadata と共有 catalog cache を再利用し、DB を再照会しない。metadata DB の書込完了境界で revision を進め、古い cache / 完了結果は論理的に失効させる。巨大な旧 cache、revision / tombstone / generation / 表示定義の不一致で採用しない完成済みprepare結果、cancel時のpending receiver内に到着済み結果と大件数確認待ちsnapshotは、type-erased payloadとして専用 workerへ渡しUIスレッド外で破棄する。検索 / ★固定との相互排他は入口と worker 完了時の両方で検証し、検索結果・検索由来 Snapshot・進行中 smart から別の最上位ビューへ直行するときは復元なしで元 `TopLevelGridRestore` を原子的に移譲する。実フォルダ entry を開いた後の scope は `TopLevelGridView::SmartFolder` に残し、親移動と Ctrl+↑/↓ を entry root 内へ制限する。削除 tombstone は scan/prepare 世代と照合し、成功 snapshot 採用まで破棄しない。通常一覧のソート順・サムネイル / 詳細表示は定義へ保存も上書きもしない |
| `collection_store/{runtime,db,model,text,export_all}.rs` | `collection.db` の単一 actor と immutable snapshot。名前・参照・手動順 / 通常ソート / シャッフルを revision 付きで管理し、10,000 件の追加上限、起動時検証と実変更前の世代バックアップ、単一 read transaction の全件書き出しを所有する。UI は actor の結果を非同期で受ける |
| `ui_dialogs/collections.rs` / `app/collection_grid.rs` / `app/collection_navigation.rs` | 作成・追加・取り込み・書き出しの単一 UI operation、Collection root の表示と戻り先、最新有効順への navigation を分担する。準備と実 target の確認は worker に置き、別 viewer context の選択・スクロール・worker に触れない |
| `collection_store/prepare.rs` | PC と Remote が共有する prepared 順序・表示設定 key の判定と、物理参照の分類。PC の同 revision / 設定での再利用と Remote の session 内再利用を同じ契約へ揃え、実際の移動先は採用前に再確認する |
| `remote_ipc/persistent_collections.rs` | 認証済み Remote session に対して公開可能な永続コレクションを読み取り専用で準備し、route・媒体別位置・seek・path guard を検証する。session と明示更新を含む有界 cache は prepared / full wire facts に限り、実 target と認証は要求ごとに再確認する |
| `app/tests.rs` | `App` 周辺の unit test / App-level 状態機械テスト |
| `remote_ipc/collections.rs` | mIV Remote のドライブ一覧・閲覧履歴・ブックマーク・本棚・レーティング・スマートフォルダを既存 read model から構築する。collection 内の通常画像は実親フォルダごとに catalog を 1 回 read-only open して source dimensions を一括参照し、本体共通の見開き builder から address-based `page_groups` を返す。collection には永続 spread key を作らず、Remote session の指定と non-book 既定だけを解決する。 |
| `crates/remote-web/web/app.js` (collection reading) | コレクションの通常画像は親フォルダへ遷移せず取得済み一覧を viewer 文脈に保ち、core の address-based `page_groups` をコンテナと共通の install 経路へ渡す。コレクション画像 hash は種別・任意 ID・画像 path を保持し、再読込時は一覧を再評価して同じ画像を復元する。 |
| `remote_ipc/ai_job.rs` | mIV Remote AI の job registry、13 状態、ページ別 `Ready` / `NotApplicable` outcome、typed terminal、10 分 result / metadata 保持と tombstone、idempotent request identity、executor trait を所有する。job ごとの `RemoteAiJobLease` が `SessionOperation` を executor 終端まで保持して disconnect / supersede / background expiry の drain barrier に参加し、fake executor test が GPU 無しで lifecycle を固定する |
| `remote_ipc/service.rs` | 保存済み `remote_service_enabled` の即時切替、`tailscale serve --bg <port>` の明示操作、PIN hash を保つ全端末の認証失効、`mimageviewer-remote.exe` の所有を専用 worker に閉じ込める。UI thread は希望状態を channel へ送るだけで、認証ファイル I/O、外部コマンド、spawn / kill / wait を行わない。実行ファイルは core と同じディレクトリから解決し、同じ `--data-dir`、共有既定値から本体が決めた `--port`、管理用 marker を渡す。serve 設定成功時と全端末失効時は child 再起動で remote-web に新しい状態を読み直させる。stdout は Bearer token を含むため破棄し、stderr の起動失敗・版不一致だけを型付き診断へ変換する。core 自身が spawn した `Child` だけを終了し、外部起動プロセスは所有しない |
| `crates/remote-web/web/app.js` | mIV Remote SPA。`gridHash` ごとの scroll / 明示復元アンカーを 64 件 LRU で持つ。復元アンカーは未操作でも 0 を持つ操作カーソルと分離し、一覧 DOM の破棄時保存、親復帰、画像・動画 viewer のページ更新を同じ復元経路へ集約する。一覧の並べ替え候補・ラベル・現在値は core の `SortOrder` 応答だけから描画し、本として固定されたページ順や固定コレクションでは理由付きで操作を無効化する。画像 viewer / 補正 panel / 本単位の表示トリム panel に加え、「端末の設定」で選ぶ端末別の静止画画質（高品質 8192 / 標準 4096 / 軽量 2048 / 最軽量 1024）、同一 target の前方 3 / 後方 1 ページ先読み、server-owned AI model catalog の選択、現在の見開きだけの自動 AI 開始、phase / page / stage / tile 進捗、取消、foreground polling と backoff、background 停止、recoverable 復帰を所有する。画像 viewer は要求位置と描画位置を別値で持ち、タイトル・ページ番号・シークは要求位置から即時導出し、描画位置が追いつくまで暫定表示にする。ページと AI result は JPEG を decode し、完成 result は Ready ページをすべて decode してから見開き DOM を一度だけ差し替え、NotApplicable ページの元画像を保持する。単ページ・見開き・補正プレビュー・AI result は、core が画素生成境界で確定した response identity を blob 化・cache 格納・DOM 差替えより前に要求 identity と照合し、不一致を表示せず通常段 telemetry に両 identity を残して自動再取得しない |
| `crates/remote-web/web/document-double-tap.mjs` | 文書全体の browser double-tap pair 観測を単一 owner に集約する。`command-core.mjs` の時間差・距離による純粋な pair 判定を使うが、成立した 2 打目を含むどの `touchend` も止めない。button / link / input / 素の要素を分ける selector 表は持たず、移動 gesture と複数接点だけを pair 候補から外す。任意 callback には直前 tap からの時間・距離、認識有無、常に false の抑止・除外値、`cancelable` を固定形式で渡し、telemetry と相関 policy は app 側が所有する。browser zoom の制限は viewport meta が所有する |
| `crates/remote-web/web/service-worker.js` / `offline.html` | standalone 起動時の接続不能案内。保存対象は利用者画像を含まない案内 HTML 1 件だけ。top-level navigation を network-first で試し、通信失敗または 5xx のときだけ案内を返す。JS / CSS / 認証 / API / thumbnail は fetch handler の対象外で、既存 asset-token 更新通知とも分離する |
| `settings.rs` | 設定の永続化 API (`Settings::load` / `save`)。Phase 3 で SQLite 経路に切替 (= `settings_db::boot_settings_db` / `with_db_result` 経由)。旧 JSON 経路 (`try_load_with_recovery` / `rotate_backups` / `write_atomic` 等) は `#[allow(dead_code)]` で残置 (将来削除予定) |
| `context_menu_model.rs` / `ui_dialogs/context_menu.rs` / `native_context_menu.rs` | grid / fullscreen の右クリック項目を `MenuNode` の単一定義から構築し、通常は Win32 `HMENU`、構築失敗時だけ egui へ描画する。可否判定済み tree に `ContextMenuLayoutSettings` を適用し、stable ID を持つ静的 leaf だけを同一階層内で表示・並べ替え、直前の separator は内部で継承 / 表示 / 非表示を保持する。環境設定「右クリックメニュー」は固定の確認場面を同じbuilderへ通し、継承値を実際の表示 / 非表示として案内する。場面選択は保存せず、対象外・非表示の項目には表示可能な別場面もbuilder結果から示す。並べ替えは actual ID とnode payloadを一緒に移し、継承separatorは移動先slotのsection、明示separatorはitemへ追従する。外部ツール群、Open With submenu、関連付けアプリ群は元位置と内部順を保つ固定枠で、Windows Shell 群も従来どおり末尾に置く。未知 / 重複 / 親違い ID は無視し、新 ID は canonical 位置へ補完してから separator と空 submenu を正規化する。仮想項目と仮想混在選択は mIV 項目だけの native menu とし、実項目の Shell 項目は既定で末尾の「Windows のメニュー」へまとめる。`QueryContextMenu` はそのサブメニューの `WM_INITMENUPOPUP` まで遅延し、設定で従来の同階層表示へ戻せる。外部ツールは登録済みの全ツールを平坦に並べ、フォルダー背景とコンテナー項目では対象だけをコンテナー 1 件へ切り替える。ZIP / PDF page では一時ファイルを渡す tool を有効にし、`OriginalFile` の tool だけを理由付きの disabled 項目にする (隠さない)。native menu は同じ window subclass の `WM_MENUSELECT` から command ID を追跡して理由の tracking tooltip を出し、egui fallback も disabled hover で同じ理由を出す。実体化不能な項目を含む集合は部分実行せず全体を拒否する |
| `external_tool.rs` | 外部ツール登録モデル (`ExternalTool`、安定 `ExternalToolId`、起動型と対象ポリシー)、発火面 + checked + `current_grid_order()` の純 resolver、Grid 専用固定スロット、右クリック、表示時点 snapshot のピッカー、コンテナー背景を所有する。単一の準備 owner が一覧 index / 所有 context / items generation / typed source identity を保持し、表示順の列挙・Stack 展開・編集 snapshot を 1 frame 最大 128 entry / 2ms に分割する。RealFile はページ探索をせず、旧内部入口のページ照合も要求ごとの source map で O(M+N) にする。所有 context の変更・終了だけが準備を取消し、別 context を無効化しない。Stack / 見開き展開後の正確な件数で確認・上限を判定し、実体化は既存 worker が行う。具体的 source 確定後は明示 cancel / supersede と materializer generation の契約を維持し、frame tail の UI launch ACK 後だけ起動する。引数は先に分割し、`{files}` を N 個の `OsString`、`{file_list}` を起動ごとの固定形式リストのパスへ展開する。リストの検証・書込み・close は worker、成功時だけリストと参照メディアを process 所有へ移す。見開きは読み順の 2 件または左右 source / 編集 context の `MergedSpread`、動画 CurrentFrame は要求時点の提示済み PTS から実体化する。Executable + Batch の長さ検査と既存 Association / OsDefault の意味を維持する |
| `materializer.rs` | 外部プロセスや将来の D&D / file clipboard が使える汎用の実ファイル化境界。実画像 / ZIP entry / PDF page / 動画フレーム / `MergedSpread` と `TempEdited` / `TempOriginal` / `OriginalFile` から、元 path、元 bytes のコピー / 展開、または PNG を決める。**出力の形は編集の有無で変えない** (`TempEdited` は常に焼き、`TempOriginal` は常に元バイト列)。動画フレームは `video::screenshot::capture_frame` (クリップボードコピーと同じ helper) で worker 上に decode する。見開きは `MaterializePageEdits::Spread` の左右を worker 上で個別に decode・選択段まで合成してから結合する。補正 DB の read-only snapshot、`CompositeSource` + `BakedEditSnapshot`、PDF 長辺、AI を含む段選択を処理する。単ページは source / 出力 stamp と完全な編集 fingerprint で process 内再利用し、見開きは不完全な単一 stamp を作らず常に再合成する。`create_new` で atomic claim した request-owned RAII temp lease と process-owned reuse の分離、成功時 handoff、世代 cancel、dead-PID / 起動前から存在する現 PID 名の孤児回収、終了時 cleanup、basename sanitize と reparse point 非追従を所有する。リスト成果物は画像 cache key / stamp を持たず、API 自身で directory 初期化後に一意名を予約し、UTF-8 BOM なし・CRLF・厳密な絶対パスを行単位で書く。keep_temp は成功したメディアとリストの終了時削除に適用する |
| `content_identity.rs` / `content_identity/restore.rs` / `app/content_identity_detection.rs` / `app/content_identity_restore.rs` / `ui_dialogs/content_restore.rs` | 物理ファイルの size / 先頭 hash / 全体 hash 台帳と、通常の物理フォルダを開いたときだけ動く 3 段照合。schema 作成 / upgrade は `PRAGMA user_version` の単一 gate が担い、失敗は空 index でなく typed `Unusable` state + 利用者通知にする。`has_restorable_content` が復元元と検出 hash cache を分離し、段 0 の size 索引には復元元だけ、全 ledger key 集合には cache 行も載せる。folder-open backfill は 5 種の edit presence があり全 ledger key 集合に無い物理ファイルを容器単位で dedup し、`ViewingState` として記録する。backfill と段 1 / 2、metadata、SQLite は Low-priority worker に閉じ、folder 切替で cancel する。設定 OFF では検出と backfill の両方を開始せず、編集確定時の A1 記録だけは継続する。同一 size / mtime の再訪は cache hash を使ってファイルを読まない。検出結果は folder key + items generation で current 判定し、非モーダル復元ウィンドウが folder 単位で保持する。復元 worker は選択候補すべての mapping を集約して共通 `STORES` を 1 回だけ走査し、identity 更新と runtime state 読み出しも batch 化する。完了 report の sidecar mirror / presence / cache invalidation を UI owner へ適用し、物理フォルダが current のままなら rename と同じ `current_folder` prefix から idx-keyed page edit state を再 hydrate し、検索・snapshot 等へ遷移済みなら clear のみにする。`comic_docs` の no-row sentinel、`rotation_cache` の `None`、materialize 済み edit-preview thumbnail は実際の destination page key だけを未読 / 再要求へ戻す。同期 sidecar flush は行わない。ウィンドウ描画と背面入力 block は同じ fullscreen-aware 可視性述語を共有する |
| `ui_fonts.rs` / `ui_font_catalog.rs` | v2.7.0 UI フォント基盤。前者は日本語の正立した選択 face + 記号 / 絵文字 / CJK fallback、動画・音声の固定サイズ HUD label 用の既定 family を含む `FontDefinitions`、実メトリクス由来の縦位置補正、atlas resync 用 cache を所有する。後者は worker からシステム font / TTC face を列挙し、日本語 coverage と upright style の判定、ユーザー font 取り込み、プレビュー raster を行う |
| `settings_db.rs` | 設定永続化 SQLite バックエンド (`%APPDATA%/mimageviewer/settings.db`)。spec §5 の起動決定木 (`boot_settings_db`)、世代バックアップ (`SettingsDb::rotate_backups` で `bak1..bak10`)、JSON migration (`migrate_from_settings_json`)、quarantine (`quarantine_db_files`)、save 抑止フラグ (`save_suppressed`) を提供。詳細は [docs/settings-sqlite-migration.md](settings-sqlite-migration.md) |
| `auto_aspect_cache.rs` | サムネイル比率 Auto モードのフォルダ別前回確定値キャッシュ (`%APPDATA%/mimageviewer/auto_aspect_cache.db`) と Collection ID 専用 table。Collection 側は App 全体のメモリ map と単一非同期 SQLite actor が読み書き・整理を所有し、root prepare の期限付き Get から初回実 rows 描画前に復元する。キャッシュ管理は両 table の件数・古い行・全件を合算し、個別フォルダ削除はフォルダ側だけを対象にする |
| `data_dir.rs` | build flavor の既定値と `--data-dir` からデータ保存先を一元解決する。設定読込後の起動案内向けに UNC を文字列判定し、Windows の割り当てドライブは `GetDriveTypeW` でネットワーク上かを判定する。案内を抑止した保存先との同値比較も所有する |
| `explorer_integration.rs` | Windows Explorer 連携。SendTo Known Folder (`FOLDERID_SendTo`) を解決し、ShellLink COM (`IShellLinkW` / `IPersistFile`) で per-user の `mImageViewer.lnk` を作成・削除・状態確認する。launcher から起動された core では `MIV_LAUNCHER_EXE_PATH` を優先して配布用 `mimageviewer.exe` を登録先にする |
| `keymap.rs` | キーボード割り当ての `KeyAction` / `Chord` / parser / Action 定義 / egui exact-match / native VK 判定 helper。現在の正本は `Settings.keymap` (`settings.db`) で、旧 `%APPDATA%/mimageviewer/keymap.ini` は初回起動時に 1 回だけ取り込んで `keymap.ini.imported*.bak` へ退避する。`keymap.ini.default` は Action 名と既定キーの参照として起動時に更新する。フレーム中にファイル I/O はしない |
| `key_input.rs` | Windows の subclass 済み HWND と `ViewportId` の単一 registry、送信元 HWND / viewport 付き `KeyEdge` queue、viewport 必須の consume / pressed / frame-state / Enter-held API を所有する。main は `ROOT`、fullscreen / detached は安定した viewer viewport ID を登録し、`WM_NCDESTROY` で対応と同じ HWND 由来の未処理 edge を同時に除去する。未登録 HWND は invariant 違反として記録し、互換側の `ROOT` だけへ配送する。テスト用の typed navigation timeline もここだけが所有し、ROOT input plugin で同じ materialization から viewport-routed `KeyEdge` と egui key event を fan-out する。level 読み取りは `PhysicalKeySlot` chokepoint を通し、timeline 非 armed 時は従来どおり OS 状態を返す |
| `keyboard_input.rs` | viewport pass 単位の `KeyboardOwner` / `TextInputPhase` / `TextInputClaim` / `ShortcutPermit`、FS 固定 Esc / 矢印用 `FullscreenRawKeyPermit`、typed text-input claim と id snapshot だけを受ける純粋な所有者決定、pass cache、既存 App / Keymap 判定への互換投影を所有する。不純な snapshot 収集は `App::keyboard_ownership_snapshot` だけが担い、draft state を所有権に使わない。helper-managed field の focus が begin-pass key 処理で一時消失した pass は `FocusRecovery` が shortcut より優先する。FS raw permit は TextInput 全 phase を拒否し、非テキスト `FocusedUi` はスライダーに矢印を奪われないよう許可する |
| `egui_focus_policy.rs` | 共有 `egui-ime` の全域 Tab policy を再エクスポートする本体の入口。最初の widget 登録前に focus traversal を常に取り消し、Tab event は keymap / TextEdit の owner に残す。本体の登録順・既存テスト・viewport ごとの IME 出力引継ぎは変更しない |
| `crates/egui-ime` | egui のみ依存する入力補正。各 Context の単一 plugin が viewport 別 composition を保持し、非空 Commit のない取消へ空 preedit を補う。viewport 消滅で当該状態を破棄し、focus 喪失で shortcut claim を終了しつつ取消の終端待ちを保持する。Windows の allow_ime が true のとき、egui-winit は Process press を遮断する。本家 0.34.0 #7967 の HashSet を物理キーごとの Hidden / Forwarded に置換し、配送済みは後続の Process repeat でも保持する。release は allow_ime にかかわらず状態を除去して Hidden のみ遮断、focus 喪失で全状態を消す。single-line helper、viewport / widget 別の次 pass 限りの retry、直前 pass の focus contract と read-only snapshot / pending event 最終状態の投影を所有する。focus 復帰は key ownership と分離し、pointer 無し・他 widget focus 無し・composing または Enter / Escape 無しで判定する。初回 focus 用の呼び出し側 latch は書き換えない。矢印 lock とホストが選ぶ全域 Tab policy も共有する。ホストの診断・keyboard owner・logger は知らない。設計・移植記録は [IME 計画](ime-process-keys-plan.md) |
| `ime_focus.rs` | 共有 `egui-ime` の本体向け facade。既存の helper 引数・戻り値と App / presenter の参照入口を維持し、helper 描画前の診断開始と描画後の widget 観測を接続する。focused または直前 pass で focused だった helper field の key press に、widget id、focus 前後、keyboard owner / phase、side-panel close call site を `[text-input-key]` へ記録する。文字マスク・Sensitive redaction、通常 record の process あたり 1 MiB 予算と異常 record の継続は本体が所有する。理由付き allowlist 以外の本体 raw TextEdit を unit test で禁止し、共有クレートも構築境界と fixture の件数を検査する。全域 Tab policy の登録、App の viewport 入口と owner 決定は従来の本体に残る |
| `operation_customize_share.rs` | 操作カスタマイズ (`keymap` / `ring_shortcuts` / 上部 `menu_layout` / 右クリック `context_menu_layout` / gamepad) の共有 JSON、未知項目の警告付き正規化、実効チョード単位の差分計算を担う純ロジック |
| `settings_transfer.rs` | 環境設定の持ち運び。全 Settings field の一つの分類 policy から転送対象 projection / 検証 / draft 適用を生成し、形式 v1 の JSON だけを処理する。App 内の環境設定・操作カスタマイズの大きな draft はそれぞれ Box に収納し、既存の寿命を保つ。設定の復元画面が一つの転送状態を所有し、worker は App / DB を持たない。確定済み設定を書き出し、取り込みは検証成功後に環境設定の draft へ反映し、既存 OK で確定する。分類・所有・既存 OK の限界は [settings-export-import-plan.md](settings-export-import-plan.md) |
| `settings_restore.rs` | 設定全体の世代復元に加え、過去世代を一時ディレクトリへ読み取り専用展開して操作カスタマイズだけを抽出し、取り込み前の `.mivkeys.json` 自動退避を管理 |
| `books.rs` | 製本機能と source-based headless compositor。製本ルート直下の通常フォルダを本として扱い、`0001_元名.ext` のページ保存、通常画像/ZIP 内画像の無加工コピー、補正/PDF/動画フレームの焼き込み追加、2 パス temp rename による並べ替えフラッシュを担当する。`CompositeSource` + `BakedEditSnapshot` の適用順、Ctrl+E の conceal / crop override、合成寸法予測は単枚・一括・製本・外部ツールで共有する |
| `book_append_undo.rs` / `app/book_append_undo.rs` | §1.373の追加出力receipt・本全体の構成と出力stampの照合・exact generated pathsだけのごみ箱取消を担当する。App側は既存BookOpPendingの受付／metadata copy／Undo終端へ接続し、後続mutationの受付revisionでreceipt資格を管理する。ごみ箱移動成功は各pathのPostDeleteItem結果で判定し、成功分だけ既存guarded purgeへ渡す |
| `app/book_append_refresh.rs` | §1.373 R4の自動観測配送と同一operationのrefresh annotation。source contextによる取消資格と、adoption owner・items世代・scan設定による一覧採用資格を分離し、既存folder scan / refreshのownerで完了結果を配送する |
| `clipboard_capture/` | App が process に 1 つ所有する画像・HTML の取り込み。`mod.rs` が不変設定・最新通知・検出時の HTML snapshot・typed event と手動要求を所有し、`native.rs` が listener / reader / 自動画像保存 worker、`popup.rs` が非アクティブ Win32 / GDI 小窓を担当する。`data.rs` は形式分類・上限付き画像デコード・ファイル名・保存公開・Zone.Identifier、`html.rs` は CF_HTML の context / fragment 分離と候補抽出、`fetch.rs` は App 所有 `CaptureFetchService` の固定 4 worker、session ごとの Agent / resolver / pool、原本ハッシュ・一時予算・背景保存・cleanup を所有する。既定保存先は起動後に背景解決し、通知は reader 上で 300 ms 合流して sequence / 世代を検査する。DIB 解釈は製本と共有し、寸法制限は取り込み入口だけで適用する。切り取り表示や viewer bundle から独立する。詳細は [clipboard-capture-plan.md](clipboard-capture-plan.md) |
| `ui_dialogs/clipboard_capture.rs` | App 所有の `CaptureSelection` と `Fetching` → `Saving` のモーダル。監視小窓・Ctrl+V の HTML snapshot を同じ受付へ集約し、現在の実フォルダ / 自動取り込みの月別フォルダを intent で区別する。原本内容による統合・最小サイズ・選択・取得失敗の表示・1 frame 最大 2 枚の texture upload・保存中の閉じる操作拒否を担当し、背景サービスの完了をメインのトーストへ返す。最小サイズは live Settings へ反映し、既存の設定保存境界で永続化する |
| `window_activation.rs` | トレイ・2 重起動・取り込み event を受理した App が共有するメイン HWND の前面化。小窓 thread は前面化しない。配置スロットの lock を外してから、非表示・通常の最小化・表示中を分けて復帰する。App の `sync_after_restore` と viewer の経路は既存の所有者に残す |
| `book_fs_journal.rs` | 製本の改名・並べ替え・別本移動を crash-safe な filesystem step plan として実行する。永続 temp 名、copy/move の SHA-256 identity、冪等な forward / rollback 判定を所有し、phase と進捗の SQLite 永続化は `book_bookmarks.rs` に委譲する |
| `reading_history_db.rs` | 閲覧履歴 (`%APPDATA%/mimageviewer/reading_history.db`)。ユーザーが開いた画像本 / 動画 / 音声を MRU として保持し、`reading-history-writer` で upsert / prune、メディア進捗更新、file metadata 補完を行う |
| `rename_key_migration.rs` | アプリ内リネーム後の path-keyed 永続データ移行と回復ジャーナル、および content-identity 復元用の非破壊 copy。rename / hard purge / copy は同じ `STORES` descriptor を正本とし、copy は `unique: true` のストアだけを `INSERT OR IGNORE` で複製する。copy の generic SQL は store 非依存のまま、`rating.source_path`、`reading_history.path`、edit-preview の所有 WebP + path 列を per-store fixup する。descriptor transaction の成功境界は path-keyed snapshot の共通 mutation effect も生成し、`edit_origin.file_key` の rewrite / delete 完了時は App が旧 content-identity index を Loading で gate して Low-priority cancellable worker から全件再読込する。未完了 rename 集合の正本は App の in-flight + FIFO queue + boot-retry に置き、ジャーナル書き出しは App-global の単一 latest-value worker が直列化する。起動時の初回 enqueue / poll 前だけ同期 load し、終了時は最新 revision の完了まで flush する |
| `metadata_cleanup.rs` | 明示操作の孤児メタデータ整理。`rename_key_migration::STORES` を正本に全 path-keyed DB を worker で走査し、親フォルダ到達可能な missing だけを確認後に transaction DELETE する。切断ドライブ、本棚、逆引き不能キーは非破壊側へ倒す。mIV 削除後の purge 最終失敗は `delete_purge_journal.json` に path 単位で残し、同じ孤児安全判定 + 共通 hard-purge を起動時 / idle 時にピンポイント再実行する |
| `logger.rs` | ファイルロガー (`mimageviewer.log`)。常時記録 + 16 MiB ローテーション |
| `diagnostics.rs` | 診断 zip 書き出し (`export_diagnostics_zip`)。logs ディレクトリのログ群 + システム情報をまとめてデスクトップに保存。環境設定「開発者」タブから呼ばれる |
| `stats.rs` | 読み込み統計の集計 |

2026-08-09 以降の `crates/remote-web/web/app.js` は、前方 3 / 後方 1 の要求解像度を維持したまま
先読みを同時最大 2 件にし、最大見開き 2 + 前方 3 + 後方 1 を 8 MiB/ページで保持できる
48 MiB 下限を持つ。DOM 適用境界は `page_display` telemetry に要求ページ、seek label、
候補 / 適用 HTTP request ID、非適用理由を path と session capability なしで記録する。

### グリッド / サムネイル / フルスクリーン

| モジュール | 役割 |
| --- | --- |
| `ui_main.rs` | メイン画面のグリッド描画とクリック/ドラッグ処理 |
| `ui_dialogs/smart_folder_editor.rs` | 名前だけのスマートフォルダ作成、現在の実フォルダ + facet 条件をルールとして追加する確認 UI、ルール / グループ化単位の管理 UI。場所 / AI / 画像色など保存対象外の条件を明示する。管理中のテキストはローカル draft に保持し、フォーカス離脱・選択変更・ダイアログ操作の確定境界でだけ Settings 保存と対応 worker / snapshot の無効化を行う。終了・トレイ退避の共通保存境界では有効 draft を Settings へ確定するが worker は開始せず、トレイ復帰時だけ遅延した副作用を1回適用する |
| `ui_fullscreen.rs` | フルスクリーンビューポート (`show_viewport_immediate`)。描画テクスチャの優先順位はここで決定。見開きのcanonical `SpreadDisplayUnit`はpage列と形成理由（paired / 本当に相方がない / 横長境界 / 非ペア境界）を同じcache tokenで所有し、共通compositionだけが端の単ページ配置を解決する。paged、F12、連結読み、holdover、Remoteはtyped placementを消費し、縦横を再判定しない |
| `seek_ruler.rs` | 静止画 / 本のページ数と動画 / 音楽の duration から、トラック幅に収まる 1 / 2 / 5 系の目盛り位置と大小区分を生成する純ロジック。3 面の共通寸法・低コントラスト色も所有する |
| `page_dims.rs` | 一度判明したページ寸法を GPU texture の生存期間から分離して保持する generation 付き CPU cache。`ViewerContextBundle` 所有で、同じ idx の別 items へ寸法を誤適用しない |
| `source_coverage.rs` | §1.165 B1 の素材の収録範囲。`SourceCoverage::{FullSource, Known(EditPreviewGeometry), Unknown}` が通常素材・生成由来が既知の編集 preview・旧行を区別する。正規座標の元寸法、切り出し前 raster 寸法、実整数 pixel bounds を別に持ち、現在 crop と独立した UV を供給する。Unknown の代役可否は共通 typed policy 一か所で解決する |
| `fullscreen_specified_scale.rs` | `FullscreenFitInput` が legacy fit と指定値＋`FullscreenFitBasis` を型で区別する。A（canonical aspect）と O（AI 前の物理原寸）を正規化し、任意軸の％ / logical px、合成 unit の実効 gap、既存倍率制限と指定方式だけの自動 5 倍上限を純粋計算する。PDF の O は page metadata と既存 Original 長辺 / native cap から解き、現在 raster へ依存しない |
| `displayed_image_transform.rs` | identity に依存しない `DisplayedImageGeometry` が fit descriptor / scale limit / trim / 90 度・free rotation / 通常または Z の zoom-pan から paint・hit・UV rect、source↔screen 写像、total scale を一度に解決する。指定方式の `layout_full_image_rect` は A/O 由来の安定した fit 枠で、実 raster の縦横比丸めは枠内の contain と sampling へ閉じる。ページは `DisplayedImageTransform` が page idx を付け、類似候補はページへ偽装せず geometry を利用する。見開き端の単ページは `SingletonSpreadPlacement` と同じページ寸法の仮想 2-slot canvasをlayoutにだけ持ち、page/sourceを増やさず既存 4 方式・SpecifiedScale と Z を解く。`FullscreenPageLayout` は各 viewer の最後に描いた Single / Spread / Continuous の transform を paint 順で保持し、`ViewerContextBundle` とともに交換する。navigator・ルーペ・範囲コピーはその viewer の layout を参照する。見開き・連結読みの配置計算自体は `ui_fullscreen.rs` が担当する |
| `vendor/egui` | egui 0.37.0の判定済みpointer release列、viewport別IME中断、寿命を持つ承認済み退役commandを所有。連続clickは上流の距離6points条件を採用する。実discard再pass直前の生成済みdeltaを、Contextに対応する既存backend rendererへ一度だけ移譲する。原本記録と差分は`vendor/upstream-provenance.json`とcrate内`PATCHES.md`を参照 |
| `vendor/eframe` | eframe 0.37.0。非表示・最小化時のlogicとFull UI pass、surface描画を別判定にする。承認済み退役をpass外で完結し、immediate子を含むdeltaは既存共有rendererで描画またはresource消費する。`max_texture_side`同期など既存差分も保持。App/tray/detachedの終了判断は既存ownerへ残す。正式経路と回帰は[egui更新計画](egui-0.37-update-plan.md)と各`PATCHES.md`を参照 |
| `vendor/egui-wgpu` | egui-wgpu 0.37.0 / wgpu 30のmanaged textureにopt-in GPU mipmapを追加するローカルパッチ。`TextureOptions::mipmap_mode`指定時だけ完全なmip chainを生成し、静止画の論理`TextureHandle` / cache構造は変えない。map化deltaの同一texture内順序を保持し、旧set/partial→submission→後発freeを完結させる。`Rgba8Unorm`生成器は比較callbackと360度パノラマにも共用。WindowsではPainterが全viewportのHWNDにdevice上限の外寸制約を所有し、初回取り付けと迂回resizeで実窓を補正・client寸法を再取得する。外寸制約はconfigureへ渡す物理client寸法をdeviceのmax_texture_dimension_2d以下に保つための保守的な設計で、再configure／Lost再作成も同じ不変条件に従う。device要求上限の既定8192は変更しない（§1.395）。上流license本文をcrate内に保持する |
| `ui_fullscreen/draw_icons.rs` | フルスクリーン上部バー / 動画 HUD のボタン・アイコン描画 helper、ファイル情報文字列 builder |
| `export_dialog.rs` | Ctrl+E エクスポートのダイアログ状態・worker・ファイル名衝突回避。UI は source identity / 編集 snapshot / preset を固定し、元画像の decode、選択段までの合成、見開き結合、画像エンコード、メタデータ転記は worker が担当 |
| `ui_helpers.rs` | メニューバー、ツールバー、アドレスバー等の共通 UI |
| `grid_item.rs` | `GridItem` 列挙型と `ThumbnailState` (Pending/Loaded/Failed/Evicted)。`GridItem::Stack { key, representative, count }` (v2.0.0) はファイル名スタックの畳んだ集約セル (ZipDir と同じ仮想コンテナ扱い = pin/snapshot/file-op/checkable/rating 対象外)。`arrange_grid_items` は実フォルダ / アーカイブ類 / 画像 / 動画・音声の設定行を全グリッド構築経路へ適用する単一チョークポイント |
| `filename_stack.rs` | ファイル名 prefix スタック (v2.0.0) の純ロジック。`StackMember`/`StackGroup`/`StackView` + `group_media` (末尾区切り文字の前でグループ化、動画は単独固定) / `materialize_aggregated` (集約グリッド) / `materialize_flat` (フラット読書フルスクリーン) / flat-index 写像 / `stack_jump_target` (Shift+↓↑)。I/O 無しで unit test 容易 |
| `filename_stack_ui.rs` | 上記の App グルー (bin-only)。トグル / 集約⇔フラットのビュー切替 (`swap_stack_view_items`) / `stack_try_open_from_grid` (集約セル → フラットフルスクリーン) / `stack_reconcile_after_fullscreen_close` (閉じたら集約へ戻す)。集約構築は `load_folder_with_scan` hook 経由。詳細は [filename-stack-plan.md](filename-stack-plan.md) |
| `thumb_loader.rs` | サムネイル並列ロード (WebP キャッシュ生成含む)。Folder 自動代表の再帰探索では子の非 Image pin を元ソースから生成せず、直上 catalog に完全一致の pin WebP がある場合だけ上位へ伝播する。再利用 WebP は完成済み cache origin として idle source upgrade を抑止する |
| `catalog.rs` | フォルダ単位の SQLite catalog。サムネイル WebP、PDF メタデータ、ZIP / 画像のみフォルダのページ数、動画・音声の長さ / 解像度 / コーデック / 確定読取失敗 (`video_meta`) を保持する。ページ数 cache は種別・mtime・file size・判定設定 fingerprint の完全一致時だけ再利用する。fingerprint はフォルダ走査が使うネイティブ対応拡張子集合も含み、保存済みの拡張子優先度が変わらなくても形式追加時に再取得する。動画・音声は mtime / file size 一致で再利用し、取消 / timeout は保存しない。再帰 pin の cache-only lookup 用に、DB が存在するときだけ schema 変更なしで read-only open する経路を持つ |
| `folder_thumb_pins.rs` | 親コンテナ (Folder/ZipFile/PdfFile/ConvertibleArchive) の代表サムネ手動ピン DB (`%APPDATA%/mimageviewer/folder_thumb_pins.db`)。`apply_folder_thumb_pin` が cache key に `#pin:{source_id}` suffix を載せて pin の identity を表現し、子の Folder / ZIP / PDF / ZipDir が持つ代表 pin を最終 leaf まで連鎖解決する。cascade の source_id は経路 hash + leaf identity。固定 leaf が Image / ZipEntry / PdfPage なら canonical page key も親要求へ渡し、編集 preview を優先する。Video ピンは `seed_folder_video_pin_thumbs` で `video_pins` から WebP を catalog に seed する。RAR/7z/LZH 変換キャッシュ閲覧中は元アーカイブパスを root key にする |

### 仮想フォルダ (ZIP/PDF) / フォーマット

| モジュール | 役割 |
| --- | --- |
| `zip_loader.rs` | ZIP 内の画像列挙、エントリバイト取得、先頭画像抽出。ネスト ZIP (ZIP in ZIP) は再帰列挙し (表示は `zip_tree` でツリー化)、内側 ZIP バイト列は 256MB LRU キャッシュに保持。非 ZIP アーカイブ (RAR/7z/LZH) のエントリは `has_foreign_archives` フラグで検出して変換提案へつなぐ (v1.3.0)。読み戻しは literal フルネーム一致 → ネスト境界分割の順 (変換キャッシュのフラットエントリ対応)。画像判定は `folder_tree::is_recognized_image_ext` に委譲 (ネイティブ + WIC + Susie) |
| `canonical_image_loader.rs` | fullscreen と remote AI が共有する静止画 canonical decoder。通常 file / verified bytes / ZIP・CBZ entry（nested ZIP を含む）を typed source で受け、RAW は拡張子で先に LibRaw へ分岐し、その他は image crate → WIC → Susie の順に試す。ソース解決とデコードを分け、fullscreen の RAW Full 待機前には fs scheduler permit を返す。EXIF 適用、GIF/APNG/WebP の既存 Animated 分類、通常 static の 8192 clamp を一箇所に置く。panorama 用 native tee は呼び出し側の従来位置に残し、raster PDF は `pdf_loader` の canonical renderer が担当する |
| `pdf_loader.rs` | PDFium ワーカープロセスプール。ページ列挙・レンダリング。EPUB の世代解決と使用は本ごとの読取リースを保持し、キャッシュの即時削除と順序付ける |
| `pdf_passwords.rs` | PDF パスワードの DPAPI 暗号化永続化 |
| `wic_decoder.rs` | HEIC/AVIF/JXL/TIFF のデコード (Windows Imaging Component)。RAW 拡張子は path / bytes の入口で拒否する |
| `save_with_metadata.rs` | JPEG/PNG/WebP のエンコードと EXIF/XMP/PNG text/WebP metadata の転記。Ctrl+E エクスポートから呼ばれ、出力は `create_new` で上書きしない |
| `susie_loader.rs` | Susie 画像プラグイン (`.spi`) のワーカープロセスプール。PI/MAG/Q0/PIC/MAKI 等レトロ画像のデコードをルーティング。32bit ワーカー exe は本体に `include_bytes!` で埋め込み、初回起動時に `%APPDATA%\mimageviewer\mimageviewer-susie32.exe` へ自動展開 |
| `archive_converter.rs` | RAR / 7z / LZH / (非 ZIP 入れ子入り) ZIP → 無圧縮 ZIP 変換 (unrar / sevenz-rust2 / delharc / zip)。入れ子アーカイブは一時ファイル経由で再帰展開し (深さ上限 8)、`"inner.rar/p01.jpg"` 形式のフラットなエントリ名で出力する (v1.3.0)。RAR はパスワード付きにも対応するが、入力パスワード自体は保存しない。画像判定は `is_recognized_image_ext` 経由 (Susie 対応拡張子も含む) |
| `archive_cache.rs` | 変換済み ZIP のマッピング DB (`%APPDATA%/mimageviewer/archive_cache.db`)。元ファイルパス + mtime + size で lookup、変換後 ZIP は `archive_cache/<hash[..2]>/<hash>/*.zip`。設定された容量上限がある場合は変換完了後に `last_access_at` の古い順で削除する。パスワード付き RAR 由来でもキャッシュ ZIP は暗号化されないため、管理 UI で `PW` と表示し削除可能にする。将来版/旧版由来の未知 `format` 行も `旧形式 / 不明` と raw format 値で表示し、同じ管理 UI から削除できる |
| `epub_cache.rs` | EPUB 専用ストア。`<data_dir>/epub_cache.db` の予約 ID・不変世代・現在・内部廃止世代の 4 表を持ち、PDF は `epub_cache/<hash[..2]>/<hash>/<stem>.g<ID>.pdf` に置く。管理画面の削除は背景 worker が PDF 文書を解放してその場で実行し、使用中の対象は残す。内部廃止世代と未完了予約の回収には起動時の `.alive` 削除ゲートを維持する。削除対象の root と各親ディレクトリをハンドルで検査して再解析ポイントを拒む。ZIP 変換キャッシュとは独立する |
| `epub_convert.rs` / `crates/epub-pdf-worker` | 背景スレッド専用 EPUB → PDF 変換ランナーと WebView2 子プロセス。core は自分の exe の隣から `mimageviewer-epub-pdf.exe` を探す (`MIV_EPUB_PDF_WORKER` で上書き可能)。書き込み共有を拒むハンドルから一時コピーと内容同定ハッシュを作り、子を suspended で起動して `KILL_ON_JOB_CLOSE` の Job Object に割り当てる。取消・異常終了で子プロセス群を止め、PDF 検証後に世代ファイルを上書きなしで公開して `epub_cache` に記録する。内部廃止世代は読込中に固定され、管理画面で物理削除に成功した世代は固定表から外す |
| `fs_animation.rs` | GIF / APNG / WebP アニメーションのフレーム展開 |
| `video_thumb.rs` | 動画サムネイル取得 (Windows Shell API) |
| `video/` | 動画インライン再生とリモート時計なし配信。`mod.rs` (VideoPlayer 公開 API) / `ffmpeg_loader.rs` (FFmpeg LGPL DLL が exe 同居しているか検証 — 展開は launcher が起動時に行い、ロードは Windows ローダが行う) / `decoder.rs` (avformat/avcodec/swscale デコード worker、`VideoDynamicState` で per-frame 状態を atomic 共有) / `audio.rs` (cpal/WASAPI 出力 + ring buffer + VST3 前段の time-stretch) / `clockless_transcode.rs` と `stream/session.rs` (再生時計から独立した H.264/AAC/fMP4 generation、App の共有 bridge と世代別 DSP 調停) / `audio_stretch.rs` (Signalsmith Stretch による pitch 維持の倍速音声処理) / `clock.rs` (AV マスタークロック)。FsCacheEntry::Video が VideoPlayer を所有し、remote headless player は metadata / thumbnail だけを提供する。`VideoInfo.dynamic` は decoder thread / native presenter thread / UI で共有し、右パネルの「フレーム表示」「デインターレース」を動的更新する |
| `video/native_touch.rs` | presenter / HUD HWND が共有する薄い `WM_POINTER` アダプタ。HWND ごとの bounded stream ownership、client-pixel→points 共通変換、source typed event、共有 `TouchRecognizer` への入力、先頭接点の pointer emulation、chrome command 写像を担当する。HUD source は OS hit-test を正本として geometry 再分類を行わず widget passthrough に固定する |
| `folder_tree.rs` | 深さ優先前順トラバーサル (Ctrl+↑↓ 用) |
| `panorama.rs` / `panorama_wgpu.rs` | 360 度パノラマビュー (Phase 1 + 1.5 + 2a)。`panorama.rs` は state / GPano XMP 検出 / 解像度ゲート / settle policy / CPU bilinear sampler / `render_settle_overlay`、`panorama_wgpu.rs` は equirect WGSL シェーダ + 8K base アップロード + settle overlay の alpha blend pipeline。詳細は [`docs/panorama-360-view-plan.md`](panorama-360-view-plan.md) |

### 音楽ビュー / 音声再生 (v2.3.0)

音声ファイル (`GridItem::Audio`) は「映像なし動画」として `FsCacheEntry::Video` の headless
`VideoPlayer` で再生し、フルスクリーンには egui の音楽ビュー (DJ 風タイムライン +
スペクトラム) を描く。動画も HUD の ♪ / Z キーで「動画→音声モード」(presenter を hide して
音楽ビューで聴く hidden presenter 方式) にトグルできる。正本:
[music-integration-plan.md](music-integration-plan.md)、動画→音声モードは
[video-architecture.md](video-architecture.md) の該当節。

| モジュール | 役割 |
| --- | --- |
| `audio_decode.rs` | 音楽解析用の FFmpeg 音声デコード (48kHz stereo f32 固定)。全尺一括 + progressive 差分の 2 API。EOF で swresample の内部 delay を flush する (再生系の `video/decoder.rs` とは独立した解析専用デコーダ) |
| `crates/music-core` | 解析純ロジック (`analysis.rs` = タイムライン bin 化 + FFT/クロマ、`beat.rs` = BPM グリッド、`pitch` = 埋め込み重みを使う HarmNet L 基音推定、`timeline.rs` / `effects.rs`)。I/O なしで unit test 容易 |
| `ui_music_timeline.rs` | 音楽ビュー中央の行分割波形タイムライン。row raster worker + 行テクスチャキャッシュ (`TimelineTextureCache`、解析の版数 = `music_analysis_version` で再ラスタ判定)。行数は `TIMELINE_MAX_ROWS` でキャップ |
| `ui_music_spectrum.rs` | 下段 108band スペクトラム + 88 鍵の基音推定。専用 worker が共有 `MusicPcm` の窓を FFT し、HarmNet の状態を PCM 同一性・絶対フレーム番号で継続／助走する。棒グラフと確率は同じ結果チャネル (in-flight 1 件 coalesce)。音楽 PCM／spectrum は App 全体で 1 つ |
| `ui_music_panels.rs` | 音楽ビューの左右ホバーパネル (ブックマーク / ループ / 行秒数) と下 HUD (動画 native HUD とレイアウト一致) |
| `metadata_transfer.rs` / `ui_dialogs/metadata_transfer.rs` / `app/metadata_import_refresh.rs` | 実フォルダ単位の明示メタ情報移送。`mimageviewer.meta.miv` directory の原子的generation pointer + フォルダ単位JSON Lines shard、root-relative path・media kind・file size検証、再帰走査、評価 / タグ / ブックマーク / 見開き / 表示トリム / 回転 / 6種のページ編集 / 代表サムネ・動画ピンをworkerで移送する。変換済みRAR / 7z / LZH / nested-archive ZIPはsource container keyと環境固有cache ZIP page keyを相互変換する。export / preview / importはshardを逐次処理し、総項目数・総sidecarサイズの固定上限を持たない。importは15ストアをattached connectionへまとめ、外側transactionを256項目 / 64 MiB / 500 msで区切り、項目内SAVEPOINTで失敗を隔離する。UIは完全モーダルの確認・固定高進捗・キャンセルと未保存view-trimのメモリsnapshotだけを担当する。開始前にはmain / active detached / paused detachedのmetadata writerを静止・drainし、終了時にXMP rating readerをcontextごと再開する。DB更新中は既存cacheを安定snapshotとして保ち、終端時に影響contextのcompact keyを専用workerへ渡す。一括再取得したcacheは未タグの読込済みsentinelも含め、items世代照合後にcontextごと所有権を置換して表示集合とcontext所有rating readerを再構築する。旧XMPタグの自動seedは2026-09-12に撤去した。外部snapshot反映はApp-globalなfacet scope / suppressionを変更せず、bookmark presence更新も保持中の全contextへ在メモリ反映する |
| App の `music_*` 状態 | 解析ワーカー / `MusicPcm` / spectrum / timeline cache は **ViewerContextBundle に入れず global** (stage-audio §3.5: ParkedLive 音楽窓も同じ global を消費する)。表示ゲートの中央述語は `fs_music_view_active`、動画→音声モードの transient は `video_audio_mode` / `video_audio_vst` |

### マルチウィンドウ / detached viewer (F12)

⚠️ 構造リワーク中 (凍結ルールあり)。正本:
[detached-rework-plan.md](detached-rework-plan.md) (§2 憲法 = BA-1〜BA-7)。
音声メディア窓は [archive/detached/detached-rework-stage-audio.md](archive/detached/detached-rework-stage-audio.md)。

| モジュール / 概念 | 役割 |
| --- | --- |
| `ViewerContextBundle` (app.rs) | ビューア文脈の状態束。active detached (独立 / Book) と parked live 窓は bundle swap で mount/unmount する。現行仕様に pin はない。通常画像の always-new grid open も open 前に `DetachedPhysical` active bundle へ昇格し、main の検索・選択・スクロールから分離する。仮想一覧由来でも backing `items` は複製せず、通常フォルダは bundle-owned worker scan、ZIP/PDF は既存 enumerate worker で完全な物理一覧を構築し、対象 leaf を新しい物理 index へ解決する。folder-nav pending、PDF/ZIP enumerate、nav lock generation、遅延 nav intent、holdover texture、ページ寸法の generation 付き CPU cache も context-owned。thumb channel / cancel_token / ワーカーキュー / keep-range atomic の「ロード複合体」も per-context (v2.3.0、bundle Drop が worker pool を畳む)。swap前後のApp-global rating session差分は各context cacheへ同期するが、ownership交換自体はnavigationではないためfacet scope / suppressionを変更しない |
| detached open request state (app.rs) | 通常画像のscan、ZIP/PDF enumerate、protected PDFのpassword request・session password・保存予約、明示leafの`Required` targetをbundle内に保持する。グリッドのFolderはmain-owned worker scanで「候補」を先に分類し、画像本と確定したcompleted scanだけをdetached contextへ移譲する。混在Folderはsession/runtimeを作らず通常main navigationへ戻す。password dialogはowner bundleをmountして再開する。空／error／必須target消失でviewportが未生成でも共通terminal closeがsession finishとwindow runtime削除を行う。実フォルダscan errorは空一覧と区別し、catalogへ適用しない |
| `ViewerSession` (`app/viewer_session.rs`) | 退避中 bundle の表示先・同期 stamp・独立 detached 状態・window ID を一括所有する。現在表示中の session は当面 `App` の既存フィールドへマウントし、`swap_with_mounted` で5項目を同時交換する |
| `DetachedWindowManager` (`app/detached_window_manager.rs`) | 窓ごとの HWND / placement / 状態遷移 (`Opening/Active/Parked/ParkedLive/Resuming/Closing`) と activation watcher を一元管理する。passive window は window collection / frozen 表示上の役割であり、runtime state 名ではない。`ViewerSession` の意味状態とは分離する |
| `dwm_transitions.rs` | DWM トランジション抑止 + UI スレッド窓 snapshot (HWND を生成イベントの before/after 差分で同定 = rect 一致捕捉の全廃、BA-1 根治) + 仮想デスクトップ移動 |
| `app/native_video.rs` | F12 host migration / source-swap / 動画→音声モード enter/exit など、native 動画 presenter と detached 窓の接続層 |

★固定の `snapshot`、評価 filter の一時解除 anchor、Ctrl+S / Ctrl+G 由来の synthetic
subfolder restore payload は同じ `ViewerContextBundle` の mounted projection に属する。
live-media fork はこの三者を top-level surface と一緒に複製し、fresh / materialized physical
context は空から始める。snapshot が `items` を直接交換するときは、位置対応する
`thumbnails` / `image_metas` と Details の order / prewarm / metadata pending も同じ generation
境界で交換・失効・再構築する。

BA-1 の不変条件は geometry 非依存の HWND 所有である。detached host 登録は registry 化済みだが、
キー入力 subclass に rect 選択が残る具体的な仕様違反は
[v2.8.1 detached 監査](review-v2.8.1/s2-detached.md) に記録されており、後続リワークへ引き継ぐ。

### 補正 / 編集 / AI

| モジュール | 役割 |
| --- | --- |
| `adjustment.rs` | `AdjustParams` (輝度/コントラスト/γ/彩度/色温度…)、LUT 適用、オート補正 |
| `ai/runtime.rs` | GUI process 全体で一つの `Arc<AiRuntimeInitOwner>` が DirectML runtime constructor を UI thread 外で一度だけ実行し、`Dormant / Initializing / Ready(Arc<AiRuntime>) / Failed(Arc<AiError>)` を所有する。App / Remote / materializer と各 AI worker はこの terminal snapshot または cancel-aware waiter だけを使う。spawn failure / panic も Failed を publish して全 waiter を起こし、process 内再試行や別 constructor を作らない。`ort` rc.12 は upstream `17ed727` の local backportで dynamic load 初期化失敗時の ORT API 再入を除去する |
| `ai/final_pipeline.rs` | fullscreen と remote AI が共有する effective upscale / denoise model 選択、native 寸法 size gate、model load、denoise → upscale 実行順の正本。任意の progress sink を受け、local は no-op、remote は model / tile phase を job progress へ投影する。model load failure 時の既存 fallback と cancel 境界を共有し、本体と remote の適用判定・出力順を分岐させない |
| `final_composite.rs` | 静止画のページ個別 > 現在地標準 > global 解決、`FinalCompositePlan`、tone → smart sharpen → colorize → Creative LUT → post_filter の共有 CPU executor。元画像・編集結果の materialize、final AI、cache / worker / GPU upload は App / remote adapter 側に残す |
| `adjustment_db.rs` | ページ個別補正・お気に入り標準補正と、お気に入り UUID ごとの表示状態を `adjustment.db` に永続化。表示状態は起動時に全件ロードし、削除済み UUID を prune する |
| `favorite_view_state.rs` | お気に入り別表示状態の 500ms debounce と SQLite 書き込み worker。メモリ正本は即時更新し、DB commit だけを UI thread 外で UUID ごとにまとめる。単一直列 queue が Set / Remove / Clear の順序を保証し、終了時は在庫を drain する |
| `edit_bundle.rs` | ページ個別補正、消しゴム、隠蔽加工、補正レイヤー、切り取り、注釈の 6 系統を `PageEditBundle` として snapshot 化し、対象寸法への変換と 6 DB の attached transaction による全置換を担う。空 bundle も同じ経路で各行を DELETE する |
| `edit_bundle_app.rs` | 単一ページの編集内容コピー / 貼り付けと、DB commit 成功後の sidecar、presence set、表示・比較・編集 preview cache、および置換前の対象ページ編集 Undo の無効化を担う App 接続層 |
| `edit_bundle_bulk.rs` | チェック優先・カーソル fallback の一括対象解決、貼り付け / 7 種リセットの確認・進捗・キャンセル、および対象ごとに `edit_bundle_app` の runtime commit を再利用する逐次適用を担う。回転だけは bundle 外として UI thread で `set_image_rotation` へ渡す |
| `rotation_db.rs` / `rotation_cache.rs` | 非破壊回転の SQLite 永続化と context 所有の回転 memo。静止画シークの未読値は pending reader が非同期取得し、cache と要求寿命を一緒に交換・失効・破棄する |
| `audio_normalize_db.rs` | 動画音量ノーマライズの per-file 測定値 (integrated LUFS / true peak / 算出ゲイン) の SQLite 永続化 |
| `rating_db.rs` | レーティング (★1〜5) の SQLite 永続化 |
| `mask_db.rs` | 消しゴムマスクの SQLite 永続化 (1bit/pixel deflate 圧縮 + ベクタオブジェクト JSON) |
| `conceal_db.rs` | 隠蔽加工マスクの SQLite 永続化。`mask_db.rs` と同じビットマップ + ベクタ構造を使い、マスクスロットも管理 |
| `local-adjust-core` | 補正レイヤー合成 core。mIV 本体では消しゴム後・隠蔽加工前の非同期レイヤーとして使う |
| `local_adjust_db.rs` | 補正レイヤー配列をページ単位 JSON として `local_adjust.db` へ保存し、`mimageviewer.dat` に復元用バックアップをミラーする |
| `local_adjust_catalog.rs` | 補正レイヤー効果ピッカー用の効果一覧・検索・デフォルト効果生成 |
| `local_adjust_effect_ui.rs` | `local_adjust_lab` から移植した各 `LocalEffect` のパラメータ UI |
| `export_crop.rs` | 切り取り (crop) の矩形・アスペクト・矩形作成時の基準ラスタ寸法、ページ単位の切り出しと `export_crop.db` 永続化を所有する。通常表示は crop 外暗転 overlay のみで、実際の切り出しは Ctrl+S コピー / Ctrl+E 書き出しの最終段に行う。旧行の nullable 基準寸法は最初の利用可能ラスタで遅延移行する |
| `spread_db.rs` | フォルダ別のページ構成・連結方式・綴じ方向、末尾表紙補助、見開き端の単ページ配置を独立tableで永続化する。端の配置は `全体設定に従う / 配置する / 中央表示` の3状態で、既存spread rowをmaterializeしない（DB 名は互換のため `spread.db`） |
| `ai/` | ONNX Runtime (DirectML or TensorRT) によるアップスケール / デノイズ / Inpainting / 補正レイヤー被写体選択と、ヒューリスティックによる画像種別分類。`AiBackend` で multi-EP 対応、TRT 用に `tensorrt_pack` (DLL pack 検出) と `tensorrt_builder` (子プロセスエンジンビルダー) を持つ。TRT 推論はメインから別プロセスへ shm + JSON IPC でルーティング (`trt_worker_pool` / `trt_worker_proto` / `trt_worker_runtime` / `trt_worker_shm`)。`trt_worker_lifecycle` が process 共通の typed state、revision、retry budget、pool identity、request route、background reaper を所有し、通常表示 / prefetch / book / materializer / video / Remote を一つの demand gate へ合流させる。TensorRT 設定でも起動時には worker を自動起動せず、対象 AI 処理が必要になった最初のタイミングで遅延起動する。worker 起動失敗は protocol から owner と UI notice まで `ProcessSpawn / Transport / Timeout / Protocol / ParentSetup / RuntimeInit / CommandRejected` の型を保ち、最初の3種だけ exact episode で1回再試行する。決定的失敗は manual / backend / pack rearm まで terminal、推論中 death は最大3回だけ自動回復し、全要求は同じ pool identity の route lease を使う。runtime init rejection は元の DirectML fallback reason を保持して即時表示する。backend off / uninstall / exit は先に route を失効し、最後の lease 後の子 shutdown / wait を background reaper が行う。U²-Netp 被写体選択は小型モデルとして in-process CPU 強制ロードで使う |
| `png_metadata.rs` | PNG の tEXt/iTXt/zTXt に埋め込まれた AI メタデータ読み取り |
| `exif_reader.rs` | EXIF 読み取り (rexif)。構造タグの抑止と Exif 2.3x 拡張タグの日本語名マップを持つ |
| `xmp_reader.rs` | XMP 読み取り (quick-xml)。`xtw:*` 名前空間のツイート情報を抽出し「XMP ツイート情報」パネルに表示 |

### UI オーバーレイ / ダイアログ

| モジュール | 役割 |
| --- | --- |
| `ui_adjustment_panel.rs` | 画像フルスクリーン左パネル。`画像補正 / 表示トリム / ブックマーク` の 3 タブを持つ。画像補正はプリセット切替・AI 設定・保存スロットを扱い、ヘッダーの 消しゴム / 補正レイヤー / 隠蔽加工 / 切り取り アイコンからそれぞれ `×` 付き独立左パネルを開く。ブックマークは現在の本のサムネイル一覧、ページ移動、DB 行削除を扱う。エクスポートアイコンは `Ctrl+E` と同じダイアログへ合流する。補正レイヤーは `local_adjust_lab` と同じ左パネル + 右ツールパネル構成で、効果 UI、マスク編集、U²-Netp 被写体生成、クラシック領域分割生成を UI thread 外の worker 経由で扱う |
| `ui_crop.rs` | 切り取り (crop) モードの独立左パネル。比率選択 / 有効化・解除 / 自動クロップ / X・Y・W・H 数値入力、crop 外暗転 overlay、ハンドルドラッグ、見開きから Single への pivot を扱う。実際の切り出しは capture / export の最終段 (`export_crop.rs`) |
| `ui_view_trim.rs` / `view_trim.rs` / `view_trim_db.rs` | 表示トリム (読みながら使う表示専用の余白カット)。フルスクリーン左ホバーパネルの `画像補正 / 表示トリム / ブックマーク` タブから、`トリムなし / 自動余白カット（本全体） / 手動設定` をラジオで選ぶ。`手動設定` のときだけスライダーの直上に `適用範囲：[本全体][このページ]` を出し、enabled なページ個別行の有無から選択を導出する。手動設定では単ページ / 見開き連動 / 見開き左右別の 0〜20% トリムを調整する。`draw_fs_image` / `draw_fs_spread` の fit 基準 bbox と描画 UV を変え、bbox 外は背景に落とす。見開き中央側のトリムは左右の見える端を gap に合わせて再配置し、操作判定は見える rect を使う。モード / 本全体設定は本キー、ページ個別設定は `page_path_key` で `view_trim.db` に保存する。表示解決は `idx` ごとにページ個別設定を優先し、無ければ本全体へ戻す。トリムなし / 自動余白カットはページ個別設定を参照しない。mIV Remote の Page worker は DB を read-only で開き、container address・物理表示 slot・見開き相手から同じキーと `None` / `Book` / `Page` / `Auto` の bbox を解決して、端末用 JPEG の resize 前だけ crop する。Remote の変更要求は既存要求ownerがreceiptを待ち、composition serviceのFIFO workerから既存 `ViewTrimDb` へ正規の `ViewTrimBookState` のまま保存し、core の clamp と共通 generation 無効化を通す。Remote からページ個別設定は作らない。Auto の見開き相手は待たず raw decode し、上下を本体と同じ規則で調停する。保存 / エクスポート / crop DB には影響しない |
| `book_bookmarks.rs` / `bookmark_browser.rs` | 本ページの安定 identity を `book_bookmarks.db` へ非同期保存し、既存 `video_bookmarks.db` と合わせた全メディア横断 read model を構築する。アプリ内の本 / ページのリネーム、製本ページの並べ替え・別本移動は旧 path → 新 path mapping を専用 transaction で identity へ反映する（missing 行保持のため共通 hard-purge store には含めない）。横断一覧は専用ダイアログを持たず `TopLevelGridSurface::Bookmarks` として通常の `App.items` / facet / details 表示へ載せ、ID・位置・登録日時・欠落状態を sidecar row に保持する。SQLite、存在確認、動画 WebP decode、ZIP / PDF 列挙、削除は worker 側で、ZIP / PDF の missing inventory は一覧構築中にコンテナ単位で共有する。削除は DB 行だけを対象にし、元メディアへ filesystem 操作を行わない。横断一覧から開く要求は単調増加 request ID と target identity を path resolver、RAR / 7z / LZH の `ArchiveConvertState`、viewer 待機まで運ぶ。元アーカイブ → 直接 RAR / 変換キャッシュ ZIP は owner 付き内部遷移とし、navigation / activation / cancel / disconnect / timeout は一致する owner と変換 receiver だけを終了して stale completion を表示へ適用しない |
| `ui_analysis_panel.rs` | 画像分析パネル (右端オーバーレイ)。色情報・ヒストグラム |
| `ui_metadata_panel.rs` | 右情報パネル (AI メタデータ + EXIF + XMP ツイート情報 + 類似画像)。類似タブ・照会表示・サムネイル要求と完了channelは `ViewerContextBundle` が所有する。可視カードとhoverページ帯の需要だけを描画後に処理し、workerは4枠、GPU uploadは1フレーム1件、派生キャッシュは512件以内に保つ。要求IDと内容stampを照合して退避・取消後の古い完了を拒否する |
| `ui_erase.rs` | 消しゴムモード (筆 / 囲み / 直線 / 縦線 / 横線 / 矩形 / 楕円 → MI-GAN で inpaint) |
| `ui_conceal.rs` | 隠蔽加工モード (同じマスク編集 UI でモザイク / 塗りつぶし / ぼかしを合成) |
| `ui_dialogs/` | 環境設定・サムネイルキャッシュ管理・変換済みアーカイブ管理と EPUB 変換キャッシュ管理の独立したダイアログ (`archive_cache_manager.rs`)・アーカイブ変換ダイアログ (`archive_convert.rs`)・お気に入り編集・スライドショー設定・ネットワーク上のデータ保存先に関する起動案内等。アーカイブ変換は `ArchiveConvertState` が scan / password retry / convert 共通の cancel token と completion policy を所有し、state drop と競合 navigation で worker と receiver を同時に終了する。モーダル相当の表示状態は `App::common_modal_dialog_open` に集約し、`process_scroll` のポインタ直下 floating-layer guard と組み合わせてダイアログ内 wheel の背面グリッドへの伝播を防ぐ。TensorRT パック取得のような長時間ツール Window はモデルレスとし、表示中も閲覧を止めない |
| `native_name_dialog.rs` | 名前変更 / 新規フォルダ作成で共有する Windows 標準の単一行入力画面。メモリ上のダイアログテンプレートを同期モーダル表示し、IME・書記素編集・クリップボード・Undo を OS に委譲する。非 Windows では no-op stub |
| `ui_dialogs/preferences.rs` | 環境設定ダイアログの状態、App 連携、ツリー / ページ dispatch。環境設定と操作カスタマイズの draft は App が `Option<Box<PreferencesState>>` で所有し、ページ追加による App のスタックサイズ増大を避ける |
| `ui_dialogs/preferences/pages.rs` | 環境設定の各 `page_*` 描画関数 |
| `ui_susie_diagnostic.rs` | Susie プラグイン診断パネルの描画。環境設定の「ファイル処理 → Susie プラグイン」ページから切り出し、`PoolStatus` 各バリアントごとにメッセージ・配色を出し分け。`egui_kittest` のスナップショットテスト対象 |
| `changelog_markdown.rs` | 更新履歴 (GitHub release body) の Markdown サブセット描画。バージョン更新ダイアログ (`ui_dialogs/update_notice.rs`) から呼ばれ、見出し / 箇条書き / `**強調**` / `` `コード` `` / `<kbd>キー</kbd>` を整形。`egui_kittest` のスナップショットテスト対象 |
| `version_highlights.rs` (v2.0.0) | 更新後の「重要な変更点」(④)。exe 埋め込みテーブル + 純関数 `highlights_to_show` (またぎ累積、unit test) + 描画 `render`。ダイアログは `ui_dialogs/whats_new.rs`。`update_check` (更新前・ネット・全文) と別物で、更新後・オフライン・操作/既定変更の主要部分のみ。`whats_new_acknowledged_version` から未読を選び、利用者が閉じたときだけ保存。main の起動通知は `startup_notice_viewer_open` で3モードの閲覧中を保留し、描画とモーダル判定を同じ visible helper へ揃える。詳細は [変更点表示計画 §2](version-highlights-plan.md#2-トリガと既読記録-12322026-10-19) |

### 検索 / インデクサ / タグ

**全体像は [search-architecture.md](search-architecture.md) を参照**。

| モジュール | 役割 |
| --- | --- |
| `search_query.rs` | 検索クエリのトークナイザ + マッチャ (AND / OR / NOT / `"..."` フレーズ、`MatchMode`)。Ctrl+S/F/G共通、Everythingは通さない |
| `search_norm.rs` | `normalize_for_match` — ingest / クエリ / post-filter で共有する唯一の正規化関数 (lowercase) |
| `search_index_db.rs` | Ctrl+S 用 `search_index.db` (お気に入り配下のフォルダ / ZIP / PDF / 動画名) |
| `fts_index.rs` | Tantivy 0.26 ラッパ。`IndexDoc` / `Fields` / `QueryFilters` / `build_bigram_and_query` / `search_page`。bigram tokenizer + lower_caser |
| `fts_meta.rs` | `fts_meta.db` (SQLite) ラッパ。ファイル単位の管理メタ (path / mtime / size / status=Ok\|Failed / index_generation)。検索原文は持たない (Tantivy STORED に集約) |
| `ingest_text.rs` | `PerSourceText` (filename / exif / xmp_tweet / png_prompt / pdf_meta / video_meta / sidecar、旧 tags は移行専用) のビルダー |
| `ingest_worker.rs` | メタ抽出 + Tantivy buffer + バッチ commit + commit 成功フレームでのみ SQLite を更新 (Tantivy First 書き込み順序) |
| `indexer_manager.rs` | 全お気に入りの `SupervisorHandle` 統括。アイテム索引または別バージョン索引が有効なお気に入りごとに 1 本だけ立て、Ctrl+G ワーカー spawn、App drop 時の停止を担う |
| `indexer_supervisor.rs` | お気に入り単位の共有 supervisor。初期メタ走査 + FsWatcher + ingest を担い、同じ watcher の変更通知を別バージョン索引の差分照合にも渡す |
| `indexer_progress.rs` | Supervisor → UI への進捗 `ProgressReporter` |
| `search_walker.rs` | 起動時の再帰 walk + 3-way diff (FS と fts_meta.db の突き合わせ) |
| `search_watcher.rs` | notify-rs `ReadDirectoryChangesW` ラッパ + 500ms debounce + rename 正規化 |
| `name_index_supervisor.rs` | Ctrl+S 用 名前索引 supervisor (初期バルク + notify-rs 追従) |
| `name_bulk_indexer.rs` | Ctrl+S 初期バルクスキャンの本体 |
| `global_search.rs` | Ctrl+G streaming クエリワーカー (Searcher snapshot 固定 + ページング post-filter) |
| `global_search_ui.rs` | Ctrl+G 検索バー + drill-down ビュー + Aggregated / DrilledInto 集約 |
| `similar_image.rs` / `similar_index.rs` | 画像・ZIP/PDFページの共通Proxy生成と、別バージョン索引のscheduler・非同期照会。お気に入り対象変更とwatcher通知を集約し、索引は全体16/同一volume8、操作中1/1、明示pause中0/0で走査する。本はstagingへ全ページを書いてからCompleteを原子的に公開する。索引中も公開済みsnapshotを保持し、storeの単一SimilarArrayOwnerがbase I/O・変更履歴適用・公開・pruneを直列処理する。単体照会はbase/deltaの線形検索後にSQLiteでorigin/hitのitem ID・revision・署名を検証し、origin key + query scope revision + publication revisionの最大6件cacheを使う。NotIndexedの完成結果と現在Running由来のPreparing表示は分ける。prefillとpurgeはscheduler存続中の同じDB ownerで直列化する。本照会のcommon判定・世代整合の未修正点は[レビュー修正計画](duplicate-detection-review-fixes-20260907.md)を参照 |
| `similar_array_owner.rs` | managerの固定data_dirごとに一つの検索配列owner。初期load/rebuild/refresh/compaction/replace・snapshot/revision採用・store照合後のhistory pruneを一つのworkerで所有する。構成変更でloadを取消さず、同store/seqの公開済み事実からackを返す。UIは要求と短い状態参照、shutdownはclose通知だけを行う。実I/O失敗は公開済み配列を保持して通知し、次回プロセス起動へ回す |
| `similar_search_array.rs` | SQLiteから派生する不変base/deltaとsuperseded mask。similar.baseは1行48byteのitem ID・PDQ・品質・revisionとheaderの適用済みseqを保存する。ロード時にstore ID、hash/proxy/形式version、行数・ファイル長・item ID順序、本文SHA-256を検証する。DBの最新状態への追随は、その後の変更履歴適用が担う。変更履歴をdeltaへ適用し、更新/削除されたbase行をmaskする。compactionは新baseを非同期構築してcutoff後のdeltaを保ち、利用中snapshotを変更しない。旧rowid/key hash方式のsimilar.compactは移行前の形式で、現行検索には使わない |
| `similar_db.rs` | 別バージョン検索の正本similar.db（schema 3）。明示AUTOINCREMENTのitem_id、revision、PDQ-256/品質、寸法・形式・保存場所を保持する。公開行の追加・署名更新・削除と同じtransactionでitem_changeへ変更連番を記録する。本のstaging/Complete公開、favorite OFFのpurge、prefill、ページ順修復を所有する。ページ順versionは変更履歴と別に管理される |
| `io_semaphore.rs` | `GlobalIoSemaphore` — UI / PDF / サムネ / インデクサ横断の I/O 同時実行制御 (Low/Normal/High) |
| `tags_db.rs` | `%APPDATA%/mimageviewer/tags.db`。`item_tags(item_key, tag, tag_key, applied_at)` / `tag_item_state` / `tag_meta`。mIV タグの正本。最初のタグ書き込み前に `tags.db.bak1..bak10` の世代バックアップをローテート。設定 ON 時だけ `mimageviewer.dat` に実ファイルタグをバックアップし、import 同期状態はタグ用に独立管理する |
| `tag_ops.rs` | UI からのタグ操作ファサード。6 種の実パス item を対象に all-or-nothing 付与/削除を決め、worker へ投入 |
| `tag_write_worker.rs` | UI → tags.db 更新 worker。通常タグ操作ではメディア本体 / XMP サイドカー / Tantivy へ書き込まない |
| `xmp_writer.rs` | XMP ratingなど現役metadataの書換 helper。旧XMPタグの自動seedと手動移行は廃止済み |

### その他

| モジュール | 役割 |
| --- | --- |
| `gpu_info.rs` | GPU 情報取得 (VRAM サイズ等)。取得失敗時の 4 GiB fallback を含む共有 pool の入力を提供する |
| `vram_budget.rs` | mImageViewer 全体の GPU メモリ予算の単一所有者。永続設定の割合から共有 pool を求め、一覧 / フルスクリーンの 80% / 20% 配分、RGBA8 bytes / texels 換算、HIGH / LOW 水位を導出する。0% は無制限として明示的に扱う |
| `monitor.rs` | モニター情報取得 (DPI 等) |
| `open_with.rs` | Windows の関連付けアプリ列挙と EXE 選択。列挙結果は実行パスではなく `IAssocHandler::GetName()` の handler ID として保持する。起動 worker 内で対象拡張子の handler を再列挙し、既存の Shell `IDataObject` 生成経路を介して `IAssocHandler::Invoke` を呼ぶ。利用者が明示選択した EXE だけを process spawn へ渡す |
| `file_drag.rs` / `file_drag/async_data_object.rs` | グリッドから外部へのファイル D&D 送出。Shell `IDataObject` を送出専用 STA 委譲ラッパーで保持し、`IDataObjectAsyncCapability` で非同期を交渉する。1ドラッグ1 owner、App へ完了を戻さず、終了時に転送を待たない。共有 Shell helper / クリップボード経路は従来どおり。`docs/file-drag-drop-design.md` §12 |
| `os_theme.rs` | Windows の「アプリ用の色」(レジストリ) を検出し、egui::Visuals へ適用。初回起動時に `Settings::ui_theme` の初期値を決める |

GPU テクスチャの利用許容量は `vram_budget.rs` の共有 pool だけから導出する。サムネイル側と
フルスクリーン側が物理 VRAM を別々に取得して独立上限を持つことは禁止し、AI 完了結果の保持
`Arc<ColorImage>` は GPU ではなく CPU メモリ用の独立 LRU とする。

---

## 3. データフロー (俯瞰)

詳細は [display-pipeline.md](display-pipeline.md) を参照。ここでは 1 画面分だけ:

```
ユーザー操作 (キー/マウス)
    │
    ▼
App::ui() 内の入力ハンドラ
    │  ├─ load_folder(path)         → フォルダ/ZIP/PDF 切替
    │  ├─ start_fs_load(idx)        → フルスクリーン画像ロード
    │  ├─ apply_rotation(idx)       → 回転の DB 更新
    │  └─ preset 切替 / 補正変更    → final pipeline cache クリア
    │
    ▼
各ワーカーに LoadRequest を投げる / テクスチャキャッシュを無効化
    │
    ▼
次フレームの poll_* / ensure_final_composite_*() 等で結果取り込み
    │
    ▼
ui_fullscreen.rs / ui_main.rs が「表示用テクスチャ」を選んで描画
    (final_composite_cache > edit_result_cache > fs_cache の優先順位)
```

**「どのテクスチャを表示するか」の決定ロジックは `ui_fullscreen.rs` に集中している**。
補正や AI を追加する時は、ここの選択順序を必ず確認すること。

---

## 4. 永続化ストア一覧

アプリ管理のストアは通常 `%APPDATA%/mimageviewer/` 配下 (data-dir の指定時はその場所)。
本の実体と利用者が指定して書き出すファイルは別の保存先を持つ。

`settings.db` の追加表 `video_audio_track_choices` は動画・音声ファイルごとの明示的な音声トラック選択を保持する。
再生位置の表とは独立し、`Settings::save()` 時に既知列だけを書き込む。

§1.335 の `startup_list_restore` は既存 Settings 全体保存の一つの record として `settings.db` に
置き、専用 DB／journal／別の pending 正本は作らない。明示一覧の target と対応 cursor を対で
保存し、旧 `last_folder` と cursor は初回だけ移行する。Preferences OK は live record を保持し、
設定 transfer は利用データとして除外する。新しい field の存在が移行済みの証拠になる。

| ファイル | 内容 | 書き込むモジュール |
| --- | --- | --- |
| `settings.db` (SQLite, 2026-05 移行) | アプリ全体設定・グローバルプリセット・保存スロット・お気に入り (`FavoriteEntry { id: Uuid, name, path, auto_index_{structure,metadata,thumbs,similar} }`)・タグ定義 (`Vec<TagDef>`)・VST3 chain 設定 (大型 BLOB)。**SQLite トランザクション + `VACUUM INTO` で `settings.db.bak1..bak10` に世代スナップショット**。`schema_meta.app_version` は open 時でなく正常な `save_full` の commit 時に更新し、各 snapshot の保存元版を保持する。物理的な Corrupted 検出時だけ `.corrupted-<ts>-<seq>` 3 セット (main + WAL + SHM) で quarantine、bak1→bak10 を新→古で試行し復旧する。保存元版が現バイナリより新しい、または未知の設定 enum / field がある場合は `IncompatibleSettings` とし、main と backup chain を変更せず save 抑止する。復元 UI は bak1..bak10 と `settings.db.preupgrade-v<old>` の保存元版・互換性を一覧表示する。**Transient I/O / Incompatible / 全復旧失敗時は `MAIN_UNREADABLE_THIS_SESSION` + `settings_db::SAVE_SUPPRESSED` で `Settings::save()` 完全 no-op 化**し、初回設定等を抑止して設定復元または終了の保護モーダルを表示する (= 残骸保護)。旧版の継続利用は他の永続DBまで読み取り専用にできないため許可しない。旧 `settings.json` は初回起動時に migration して `*.migrated-<ts>` にリネーム済み | `settings.rs` + `settings_db.rs` |
| `collection.db` / `collection.db.bak1..bak10` | 名前付きコレクションの定義・登録元パス・手動順・シャッフル設定。既存 DB は起動時に整合を確認し、そのsessionの最初の実変更前に一度世代バックアップを試行する。未編集の空DBは最初の成功変更後から対象とする。v1/未版管理DBのschema移行前はバックアップ必須で、失敗時は移行を止める。通常のバックアップ失敗は記録して変更を続ける。設定リセット・メタ情報の移行には含めない | `collection_store/{runtime,db}.rs` |
| `Pictures\mimageviewer\books\...` (既定、設定可) | 製本した本の実体。DB ではなく通常フォルダ + `0001_元名.ext` 画像ファイルのみ。`Settings.book_root` で変更でき、Ctrl+S/Ctrl+G の自動索引対象外 | `books.rs` + `ui_main.rs` + `ui_fullscreen.rs` |
| `Settings.keymap` / `keymap.ini.default` | キーボード割り当て設定。GUI 編集の正本は `settings.db` 内の `Settings.keymap`。旧 `keymap.ini` が残っている環境では初回起動時に読み込み、同じ内容を `Settings.keymap` へ移してから `keymap.ini.imported*.bak` へリネームする。以後 `keymap.ini` は通常読み込み対象外。`keymap.ini.default` は現在バージョンの Action 名と既定キーを確認する参照ファイルとして更新される。競合は拒否せず warning として扱う | `keymap.rs` + `settings.rs` |
| 利用者指定の `preferences.mivprefs.json` (推奨名) | 現在の確定済み環境設定の移行可能な項目だけを持つ UTF-8 JSON、format_version=1。data-dir 外にも保存可。正本 DB・全設定バックアップではなく、利用データ・パス・接続・操作カスタマイズは含めない。設定の復元画面から取り込み、検証成功後に draft へ反映し既存 OK で保存 | `settings_transfer.rs` + `ui_dialogs/settings_restore.rs` + `ui_dialogs/preferences.rs` |
| `catalog.db` | フォルダ単位のサムネイル WebP キャッシュ (BLOB) + PDF メタデータ + ZIP / 画像のみフォルダのページ数 cache + 動画・音声の `video_meta`。ページ数・メディア情報の取得は既存詳細遅延 worker が `GlobalIoSemaphore` 配下で行い、cache 障害時は表示自体を失敗させず元ファイルから再取得する | `catalog.rs` + `app/metadata_ops.rs` |
| `video_tile_thumbs.db` | 動画タイル / resume サムネイルの WebP と、粗い全尺波形の量子化 chunk。新しい波形 chunk は stream index を含む追加テーブルへ保存し、既定トラックだけ旧テーブルを互換読みする。path、stream index、動画 mtime / size、bin 幅・総 bin 数、format version の完全一致時だけ再利用し、窓解析と raster は保存しない。サムネイルキャッシュ管理のファイル単位・フォルダ単位・全件削除を共有する | `video/tile_thumb_cache.rs` + `video/seek_strip_wave.rs` |
| `content_identity.db` | 物理ファイルの size / 先頭 64 KiB hash / 全体 hash と、復元元か検出 cache だけかを表す `has_restorable_content`、復元辞退組を保持する。schema は `PRAGMA user_version` の単一入口で作成 / upgrade し、unversioned A1 行へ列を足す migration の default は復元元を表す `1`。期待 schema を用意できない session は typed `Unusable` とし、空台帳として処理を続けない | `content_identity.rs` + `app/content_identity_detection.rs` |
| `auto_aspect_cache.db` | Auto サムネイル比率のフォルダ別前回確定値と、UUID table に置く Collection root 別前回確定値。後続の実統計は既存ゲート (streak/cooldown 等) に従って補正する。Collection の DB I/O は単一 actor に限定し、prepare worker が最大 100 ms 待つ optional Get、UI はメモリ値だけを扱う。サムネイルキャッシュ管理の全件・期限整理は両 table を対象にする | `auto_aspect_cache.rs` + `app.rs` |
| `rotation.db` | 非破壊回転角 (0/90/180/270) | `rotation_db.rs` |
| `audio_normalize.db` | 動画ファイル単位のノーマライズ測定値 (integrated LUFS / true peak / 算出ゲイン)。主キー `(path_lower, file_size, mtime_ms, target_lufs_milli)`。環境設定 → 動画・音声 → 動画から全件クリア可能 | `audio_normalize_db.rs` |
| `rating.db` | レーティング (★1〜5、0 は未登録)。ページ単位 (画像/ZIP 内画像/PDF ページ) とコンテナ (フォルダ/ZIP/PDF 本体) を同一テーブルに格納。キー形式の違い (`::` の有無) で区別 | `rating_db.rs` |
| `search_index.db` | Ctrl+S 用。お気に入り配下のフォルダ/ZIP/PDF/動画名索引 | `search_index_db.rs` |
| `fts_index/` | Ctrl+G 用 Tantivy index (複数 segment + meta.json)。bigram 候補絞り込み。旧 `tags` STORED は tags.db 移行専用 | `fts_index.rs` → `ingest_worker.rs` |
| `fts_meta.db` | ファイル単位の管理メタ (path / mtime / size / status=Ok\|Failed / index_generation)。検索原文は持たず Tantivy STORED に集約 | `fts_meta.rs` |
| `similar.db` / `similar.base` | 別バージョン検索用。`similar.db` が唯一の正本で、item ID・revision・PDQ-256・品質・寸法・保存場所とCompleteな本generation、変更履歴を保存する。`similar.base` は1行48byteの不変baseとheaderを持つ派生配列で、store ID・形式version・順序・SHAを検証する。`similar.compact` は旧形式で現行検索には使わない。追加・変更・削除は既存favorite watcherから差分照合を要求する | `similar_db.rs` + `similar_search_array.rs` + `similar_index.rs` |
| `adjustment.db` | ページ個別補正 (`page_params`)、お気に入り標準補正 (`favorite_params`)、お気に入り別表示状態 (`favorite_view_states`)。表示状態の行は独自状態を持つこと自体を表し、既存テーブルを変更せず追加テーブルとして共存する | `adjustment_db.rs` || `mask.db` | 消しゴムマスク (deflate 圧縮 1bit/pixel + ベクタオブジェクト JSON) | `mask_db.rs` |
| `conceal.db` | 隠蔽加工マスク (deflate 圧縮 1bit/pixel + ベクタオブジェクト JSON) とマスクスロット | `conceal_db.rs` |
| `local_adjust.db` | 補正レイヤーのページ単位 JSON。中央 DB が authoritative で、`mimageviewer.dat` の `local_adjust_layers` はフォルダ移動時の復元用バックアップ | `local_adjust_db.rs` + `sidecar.rs` |
| `export_crop.db` | 最後段 crop のページ単位矩形と、その座標を作成した基準ラスタ寸法 (`source_width` / `source_height`)。中央 DB が authoritative で、`mimageviewer.dat` の `export_crop` は同じ自己記述設定を持つフォルダ移動時の復元用バックアップ | `export_crop.rs` + `sidecar.rs` |
| `view_trim.db` | 表示専用トリム。`view_trim_books` に本ごとの基本適用モード / 本全体設定、`view_trim_pages` にページ個別設定を JSON で保存する。手動設定では enabled なページ行そのものが永続する「このページ」選択を表し、ページ表示時に本全体より優先される。出力用 crop とは独立し、コピー / 保存 / Ctrl+E には焼き込まない | `view_trim_db.rs` + `ui_view_trim.rs` |
| `comic.db` | Ctrl+T テキスト注釈のページ単位 JSON。吹き出し・テキスト・ウィンドウ・スタンプ配置の正本で、ユーザー画像スタンプは配置先の注釈に `Embedded` として埋め込む | `comic_db.rs` + `ui_text.rs` + `sidecar.rs` |
| `comic_user_stamps.db` | Ctrl+T スタンプピッカーのユーザー画像履歴。配置時の長辺 1024px 上限 PNG を再利用用 MRU として保持する。履歴から選んでも配置先には `Embedded` をコピーするため、履歴削除は既存注釈に影響しない | `comic_user_stamps.rs` + `ui_text.rs` |
| `spread.db` | フォルダ別表示モード (ページ構成、連結方式、綴じ方向、末尾表紙補助、見開き端の単ページ配置) | `spread_db.rs` |
| `spread_shift.db`（§1.364 D1–D3、未リリース） | 論理本scopeごとの相対PageRef。KeepDriveのroot＋実効ZIP prefix、一冊一行。schema v1。新規storeのためmigrationなし、不正schema/JSONは原本を保って書込停止。既存spread設定・resumeとは独立 | `book_scope.rs`、`spread_shift/{model,service,store}.rs`、`app/spread_shift_owner.rs` |
| `book_composition.db`（§1.242 G2、未リリース） | `book_compositions(book_key PRIMARY KEY, revision, schema_version, composition_json)`、schema 2。一行のJSONに検証domainと方向・表紙・順序・hidden・overrideを持つ。未公開旧units移行なし。KeepDrive共有BookScopeKey、local revision、単一writer・immutable snapshot・worker-only maintenance。新storeのmigrationなし。未知/破損は原本保持・書込停止。STORESのrename/purge/copy/probe/cleanup、内容ID Edit観測、明示metadata v11専用family、確認済み完全resetへ登録。Folder sidecarはG4F | `book_composition/{store,service}.rs` |
| `book_resume.db` | コンテナpathとraw items indexで位置を復元。記録時のHUD読み順ordinal/totalをnullable列に保存し、通常Folder/ZIP/PDF一覧の下端メーターに常に左から右へ表示。旧2列DBへのALTERと一覧全行読込は既存writerで実行。App共通メモリmapを記録受付時にも更新し、描画はmap参照のみ。動画/音声の同じ帯は既存video_resume_positionsと取得済みvideo_metaを読み、別の位置map/DB列は作らない。Remoteは補助列NULL (IPC不変) | `book_resume_db.rs`, `app/book_resume_meter.rs`, [設計・割り切り](book-resume-meter-plan.md) |
| `reading_history.db` | ユーザー操作で開いた画像フォルダ / ZIP / PDF / 変換アーカイブと動画・音声ファイルの MRU。既存 kind 文字列を維持し、`video` / `audio` と専用の `media_position_ms` / `media_duration_ms` を追加する。未知 kind は行ごと読み飛ばす。保持件数は 1..=1000 | `reading_history_db.rs` + `App::record_reading_history` |
| `folder_thumb_pins.db` | 親コンテナ (Folder/ZipFile/PdfFile/ConvertibleArchive) の代表サムネ手動ピン。container_key 主キー (= normalize_keep_drive 済みパス) で 1 行 1 コンテナ、source は kind + container 相対 rel + (zipentry の) entry / (pdfpage の) page。`apply_folder_thumb_pin` と変換アーカイブの `archivethumb:*#pin:*` が cache key suffix `#pin:{source_id}` で identity を表現し、cascade 時は途中コンテナを含む経路 hash を source_id に加える | `folder_thumb_pins.rs` |
| `edit_preview_cache.db` | 非破壊編集結果を一覧へ戻す派生 preview の対応表 / LRU。page source の mtime + size と親代表用 container size、crop 後の一覧寸法を保持する。§1.165 B1 は released v6 を維持し nullable `source_coverage_json`／`layout_dims_json` / `metadata_content_hash` を加法追加する。旧行/WebP を残し、新生成は実 pixel bounds と正規座標/raster 寸法、独立したPDF page-box寸法を同じ upsert で保存する。WebP 本体は `edit_preview_cache/` 配下で、各 row が item-key hash directory の下地 + 注釈 layer を単独所有する | `edit_preview_cache.rs` + `source_coverage.rs` |
| `video_pins.db` | ユーザーがフルスクリーン HUD で指定した動画フレームの抽出 WebP。`(path, pin_pts_secs, thumb_webp, thumb_pts_secs)`。folder thumb pin の source が動画のときは `seed_folder_video_pin_thumbs` が起動時にこの WebP を catalog にミラー seed する。左ジャンプパネルのピン行もこの WebP を再利用する | `video_pins.rs` |
| `video_bookmarks.db` | 動画ブックマーク (pts / title / jump panel 用 WebP)。初回表示時に FFmpeg worker で取得したサムネを `thumb_webp` に保存し、次回以降は DB から復元する | `video_bookmarks.rs` |
| `video_chapter_thumbs.db` | 埋め込みチャプターの jump panel 用 WebP キャッシュ。path + file size + mtime + chapter start をキーにし、動画更新後は古いサムネを参照しない | `video_chapter_thumbs.rs` |
| `pdf_passwords` | PDF パスワード (DPAPI 暗号化) | `pdf_passwords.rs` |
| `pdfium.dll` | 初回起動時に exe から展開 | `lib.rs` (`run`) |
| `models/*.onnx` | 初回起動時に exe から展開 | `ai/model_manager.rs` |
| `mimageviewer.log` | **常時記録** (旧 `--log` ゲートは撤廃、`--log` 引数は no-op)。起動ごとに truncate し、前回分を `mimageviewer.log.prev` に退避。実行中は 16 MiB 超で `mimageviewer.log.bak` にローテーション。非blocking facade、単一writerが各batchを直ちにflush | `logger.rs` / `log_writer.rs` |
| `logs/panic.log` | Rust panic フックが backtrace 付きで append。セッションを跨いで蓄積するため 4 MiB 超で `panic.log.bak` に 1 世代退避 | `lib.rs` (`append_panic_log_entry`) |
| `logs/settings.log` | 設定復旧経路の永続診断ログ。**logger の初期化状態に依存しない独立 sink** なので起動ごく初期の復旧フェーズでも残る。SQLite open 時の primary code + extended code、bak 復旧の経路、quarantine、preupgrade snapshot、save 抑止のイベントが残る。再現が難しい設定リセット系報告の事後解析用 | `settings.rs` (`settings_diag_log`) + `settings_db.rs` (`log_diag`) |
| `logs/perf_events.jsonl` | 構造化イベントログ (JSON Lines)。`--perf-log` 引数または環境設定「開発者」タブの「性能ログを記録する」が ON のときだけ生成。起動ごとに rotate (`perf_events.1..4.jsonl`)。単一writer、250ms周期flushと完了checkpoint | `perf.rs` / `log_writer.rs` |
| デスクトップ `mImageViewer_diag_<日時>.zip` | 環境設定「開発者」→「ログを zip にする」で生成する診断 zip。workerの完了barrierで両ログの固定prefixを取得し、補助ログ + `system_info.txt` + `delivery_manifest.json` をまとめる。過去dropは証拠不完全と知らせ保存 | `diagnostics.rs` / `log_writer.rs` |

**パスキーの正規化**: Windows は大文字小文字非区別なので、すべての DB は **小文字化 + バックスラッシュ→スラッシュ** に正規化してから格納する。新しい DB を追加するときも同じ規約に従う (`rotation_db.rs` / `adjustment_db.rs` を参照)。

§1.364 D1の`BookScopeKey`/coverage codecは`book_scope.rs`が共通owner。
Gの意味分類`BookScope`はG3でconstructorへadaptする。shift保存はAppの単一workerだけが書き、
共通storeの移動・purge・cleanup・metadata転送は同workerのFIFOをfenceして接続を閉じるmaintenance leaseを使う。
EPUB保護が必要な処理は対象範囲を先に確保してからshift leaseを取得し、内側のcopy/purgeには
取得済みownerを渡す。同階層PDF保存の長い変換中は既存の本保護だけを維持する。
copy/probeの`LogicalBookFamily`は指定descriptorだけにroot exact＋境界付きslash範囲を有効にする。
相対payloadは本rootの改名で不変、製本の旧→最終ページmappingはworkerで更新し、scope外移動はMissing保持。
本管理のrename/delete成功は捕捉済みfolderを専用workerのroot移行/hard purgeへ登録する。
PageRefは保存表記を維持し、照合時だけ共通のcase/slash正規化を使う。
settings単体reset、履歴/resume/cache削除、自動sidecarにはshiftを混ぜない。
明示metadata v10のlogical `spread_shift_family`は物理cache由来の`container_state`とは別に搬送する。
D1はstore/完了契約と登録まで。D2の接続差分では`app/spread_shift_owner.rs`の
`ViewerSpreadShift`がscope、`ViewerShift::None | Ordinary | Book`、操作receipt、
fenced resume、identity/nav投影を一つのviewer ownerへ集約する。通常一覧のずらしはOrdinaryで
sessionだけ、本はPageRefと`SpreadShiftState`で扱う。旧raw anchor fieldは独立の正本にせず、
`spread_shift_anchor_idx()`が現在解決済みの投影だけを既存display-unit resolverへ渡す。
items削除・snapshotのslot変換はlive解決位置をremapし、identity/nav列が変わればPageRefを再解決する。
snapshot化ではdurable scopeを切り離し、その時点のnumeric pairingをOrdinaryのsession調整として保持する。
成功済みページ改名のmappingはpure helperで同じ本のlive PageRefへ反映する。
root改名のdurable移行はD1のworkerが所有し、mounted/at-restのlive ownerのscopeは旧rootを保持する。
新rootは改めて開いたcontextが使い、旧contextのSet/Clearを新rowへ読み替えない。

`ViewerContextBundle`がこのownerを交換・保持する。F12のcloneはその時のlive調整を複製し、
保存replyはcontext/items/operation sequenceとscopeが一致する操作ownerへだけ配送する。
at-rest ownerにも同じreceipt判定を使い、保存完了はページ移動や兄弟unitの失効を起こさない。
ON/OFFは全体設定の世代を進め、全ownerの現表示をsession扱いにし、ON化元のBookに
live調整がある場合だけ保存する。base spread setterの同期契約は維持し、成功後のQ3 resetだけを
shift Clearへ接続する。HUDと環境設定転送には全体boolを、metadata転送には本別PageRefを渡す。
この明示設定変更で`session_only`はretainedのpending receiverを取り消し、受入済みsessionを保つ。
表示済みの無関係な兄弟窓へ旧保存値を復元しない。fresh-openのCommon依存は世代不一致だけで
破棄せず、`refresh_pending_book_shift_memory_policy`が捕捉済みtarget/context/items/open/scopeと
元のresume cutoffを維持して、新settings世代・現在のshift受付cutoffで読取依存を再受付する。
再受付するのはshift依存だけで、元のresume receiverと取得済み結果は保持する。
maintenance epochの進行でも失効するのはshift recipeだけで、元のresume依存は保持する。
Commonはその固定読取点を満たして調整なしで初回採用を完了し、同期の非fence SELECTへ戻さない。
読取完了のCaptured/Expiredを型で区別し、失効を保存row不在と解釈しない。retainedの
受入済みsessionとprojectionは失効時に保持する。maintenance資格は対象scopeのstampで照合し、
別本のroot改名・変更なしlease・通常writeはそのscopeのreadを失効させない（設計§11.8）。
明示importの書込み前取消・エラーも一括失効にせず、maintenance ownerの再読込した実変更scopeへ
集約する。無関係なfresh Commonの捕捉済みsaved shiftはそのまま採用できる（設計§11.9）。
resumeのroot資格・キーは既存producerの`at_root()`・`current_folder`を共用する。
これは設定変更に伴う通常遷移であり、retryやまれな失敗の復旧ではない。

新規openのRestoreは既存`sidecar_restore::Common`の`ViewerBookRestore`として受け、最終identity/navへ
shiftを解決してから同じcontinuationの位置復元・first-displayへ進む。
retained ONの`begin_retained_shift_readoption`は受入済みidentity indexを再利用し、
読取待ちはbundle内の`ViewerSpreadShift.retained: RetainedReadoption::Idle | Reading`が所有する。
global Commonとは別contextの読取を併存させ、既存のtarget-context semantic gate
(`sidecar_restore_blocks_projected_context`)と復元dialogを対象窓だけに使う。兄弟窓の入力は止めない。
`ContextAsyncOwner::RetainedSpreadShift`が既存async inventoryでpollとcancel-on-parkを扱う。
parkはreceiverをdropして受入済みsessionを保ち、次の再採用がfresh cutoffを捕捉する。
forkは`fork_accepted`でaccepted layoutだけを複製し、receiverと元contextのwrite受付を引き継がない。
保存待ち/失敗の調整は新contextではsession扱いにする。OFFはlive調整をそのまま保持する。
fresh-openの`FencedResume`はopen continuationの初回採用完了時に終了し、後のsession内参照を固定しない。
retainedとimportの復元では
`adopt_shift_layout_without_navigation`が現在の実ページを含むunitへ正規化するだけで、
共有resume/historyを書かない。metadata importは捕捉FIFO読取をContextRequest workerで待ち、
context/items世代の採用判定後にcurrent scope + ON + Automaticの一致で調整を採用する。
shiftの読取対象はmanifestのsection flagではなく、import commitで実際に変更したscope集合に限る。
root改名では参照元・items・prepared identity index・pending load・addressとshift scopeを旧rootに保つ。
main一覧の旧path再読込・F12の旧items保持はmaster `02079f95d`の挙動を維持し、live追従や自動reopenを
加えない。同一本の未完了openを待つquery・製本管理の待機wrapperも置かず、受付済みopenは旧scopeを保つ。
D1の単一FIFOが受理済みshift write/read/root操作を整列する。旧contextのSet/Clearが旧keyを
再作成・解除し得る点は利用者了承済みの割り切りで、新rowへ書き換えない。
snapshotのanchor remapはfullscreenの有無によらずitems交換で行う。最新決定は設計§11.6。
OFFではそのshift読取結果を使わない。

D3のRemoteはApp所有serviceのread handleを`SessionHandle`へ登録し、ContainerEngine workerがcloneして使う。
shift専用のDB reader/writerは追加せず、保存済みlogical scope/relative PageRefをfull本のidentityへ解決し、
共通Automatic builderのoptional shiftへ渡す。完全応答では同じrulesと列のPC unitに一致する。
応答budgetで切り詰めてもfull-scopeの基点解決は維持するが、bounded group生成は既存の契約を保つ。
切詰め境界のgroup差はshiftなしのmasterでも存在し、既存cap・ページアクセスを変えず、
省略ページの全寸法走査や境界group削除を追加しない。Webは既存PageGroupをそのまま描く。
PCのviewer-local live調整を借用しない。記憶OFF・forceSingle・orientationも保存行を書き換えない。
acquireは受理済みshift/resumeの完了とsnapshot公開を既存取得continuationで待つ。
SetSpreadのbase setterは同期のまま、Q3のshift Clearだけをsequence-ownedな同request continuationで待ち、
commit成功後にwrite success/generation/group cacheを公開する。UI threadでDB完了をblockしない。
Remoteのnudge/記憶設定UIは増やさない。wire/address/slot/roleの契約を変えないためIPC v69・Web asset tokenを維持する。
変換書庫のRemote baseは元書庫のlogical keyを保持し、PCのcache側baseへの移行を行わない。
この既存差と切詰め境界のmaster証明・D3検証記録は設計§11.10と`target/D-1364-d3-msg.txt`を参照する。
D3時点ではG3は未統合だった。2026-10-11のG3は共有C1–C12を既存admission/restore ownerへ接続し、
strict IPC v70のmode-only入口を追加した。構成editor固有のC7/C9はG4で接続する。
本記述は実装の所有境界を示し、自動検証・独立完了レビュー・利用者確認の完了を主張しない。

### フォルダ側サイドカー (`mimageviewer.dat`)

`adjustment.db` (ページ個別補正)、`mask.db` (消しゴムマスク)、`conceal.db` (隠蔽加工マスク)、`local_adjust.db` (補正レイヤー)、`export_crop.db` (最後段 crop)、`comic.db` (Ctrl+T 注釈) のバックアップとして、各ユーザーフォルダの直下に `mimageviewer.dat` を置く (Hidden + System 属性付きの JSON)。中央 DB が authoritative で、フォルダを丸ごと別ドライブへ移動した際に中央のパスキーが無効化されるケースの復旧経路。設定トグル (`sidecar_backup_enabled`、デフォルト ON) で ON/OFF できる。補正レイヤーは各エントリの `local_adjust_layers` 配列、最後段 crop は矩形と基準ラスタ寸法を含む `export_crop`、Ctrl+T 注釈は `comic` として保存し、中央 DB に既存エントリがある場合はインポート時に上書きしない。書き込むモジュール: `sidecar.rs`。詳細は [preset-and-adjustment.md](preset-and-adjustment.md) §9 と [virtual-folders.md](virtual-folders.md) §6 を参照。

この自動 sidecar は日常的な編集バックアップであり、明示的な持ち運び用 manifest ではない。
設定が OFF でも `mimageviewer.meta.miv` の export / import は独立して動作する。

### 明示メタ情報転送 (`mimageviewer.meta.miv`)

v2.7.0では安定化のためメニュー入口を一時非表示にしたが、v2.8.0の継続開発では
`metadata_transfer::UI_ENABLED`を有効化して再公開する。

`ファイル > メタ情報をエクスポート / インポート` は、自動バックアップの
`mimageviewer.dat` と独立した versioned bundle directory を実フォルダ直下へ作る。v7 は評価、
タグ（名前・適用時刻と空タグを含む決定状態）、サムネイル付き動画・音声ブックマーク /
本ブックマーク、見開き・表示トリム・回転、ページ補正 / マスク /
部分補正 / crop / 注釈、フォルダ代表サムネ・動画ピンを対象にする。crop は通常DB /
sidecarと同じ矩形と基準ラスタ寸法の組を保持する。ZIP / PDF のページ stateと ZIP 内の本 state は
物理コンテナ配下の相対キーで持つ。RAR / 7z / LZHと、入れ子非ZIPを含んで変換されたZIPは、
ページ側とコンテナ側で独立したoriginをmanifestの`virtual_key_base` / `container_key_base`へ保存する。
ページ情報はexport元のarchive cache DBで有効なsource/cache対応も確認し、cache ZIP側のページkeyを
相対化してimport先data directoryから決まるcache ZIP keyへ復元する。見開き・連結方式・綴じ方向と
本単位表示トリムは実行時の`nav.tree.zip_path`と同じく変換後cache ZIP基点なので、実際に検出した
source/cache originを移送先の決定的cache keyへ対応付ける。一方、代表サムネピンは
`zip_pin_root_path`由来の元アーカイブsource keyを維持する。この非対称は実行時DB key体系に合わせたものとする。
未リリースの開発ビルドだけが作成したv1〜v6 bundleは移行せず、v7だけを受け入れる。
前回の未コミット検証で作成されたv6も、必須fieldを黙って補わずversion gateで明示的に拒否する。
archive cache DB自体がない場合と対象行がない場合だけを未変換扱いにし、open / query失敗は
ページメタ情報を黙って欠落させないようexport全体のエラーにする。
cache ZIPと`converted_archives`行を管理画面削除・容量pruneで失っても、cache pathはsource pathと
data directoryから決定的に再生成できるため、source/cache両prefixを全ページメタ情報familyの
scope、および`spreads` / `view_trim_books`のコンテナscopeへ入れる。cache側だけに行が残る場合は
`ConvertedCache`として移送し、source/cache双方に行がある場合はページ・コンテナそれぞれのorigin内で
DB間の走査順による暗黙mergeをせず、対象アーカイブ付きの競合エラーにする。
この競合は同じアーカイブを直接閲覧と変換cache経由の両方で編集した可能性を利用者へ示し、
どちらかを推測して統合せずexport全体を中止する。
一方、直接閲覧RAR / CBRは`ConvertibleArchive`として列挙されてもsource RAR keyを参照するため、
いずれか一方でページ行を実際に検出した場合は、そのoriginを有効cache行や拡張子からの推測より
優先してmanifestの`virtual_key_base`へ保存する。

bundle は `mimageviewer.meta.miv/manifest.json` を現在generationの小さいpointerとし、
`generations/<id>/shards/<folder-hash>.jsonl` にrootと各サブフォルダの直下項目を1recordずつ
保存する。bundleは別環境へコピーするための成果物なので、WindowsのHidden + System属性を付けず、
旧ビルドが作成したbundleも再exportの公開時にこの2属性だけをbest-effortでクリアする。隠したままでは
ExplorerのCtrl+Aコピーや`xcopy`（`/H`なし）等でbundleだけが漏れ得るためである。mIV自身の通常一覧、
サブフォルダ展開、スマートフォルダ、フォルダペイン、検索・名前索引からの除外は
`mimageviewer.meta.miv`と一時directoryの名前に基づき、Windows属性や隠しファイル表示設定に依存しない。
日常バックアップの自動sidecar `mimageviewer.dat`は従来どおりHidden + System属性を維持する。
再帰exportはフォルダ境界をまたぐ4096物理項目ごとにDB scopeを作り、深さ優先で
逐次serializeするため、全項目や単一フォルダの全子要素を1つの`Vec`やJSONへ集めず、
総項目数と総bundleサイズに固定上限を設けない。防御上限は
manifest 1 MiB、shard header 256 KiB、単一物理項目record 256 MiBに限定する。
完成前generationはpointerから参照されず、全shardのflush / sync後に`manifest.json`だけを
原子的に置換する。キャンセル・書込失敗では既存pointerを維持し、公開済み内容を壊さない。
crashで残った未公開generationは、次回exportの開始時と公開成功後に現pointerのgenerationを
保護しながらbest-effortで回収する。

preview / importは各shardをbounded line readerで逐次読み、folder hash、direct-child配置、
shard / entry総数、record単位validationを検証する。importはDBを触る前に現generationの全shardを
streaming preflightし、重複pathはRAM上の全件`HashSet`ではなく一時SQLite indexで検出する。
各file recordにはsize / mtimeを保存するが、移送性能を優先してファイル内容digestは作らない。
importは相対path・entry kind・media kind・sizeが一致すれば同一項目として扱い、mtimeだけが変わる
コピーと、同一sizeで内容が変わったファイルのどちらにも適用する。media kindも明示し、
画像への時刻bookmark、音声へのvideo pin、
動画へのpage edit、通常fileへのvirtual pageなどの組み合わせをpreflightで拒否する。
移送先環境で拡張子から再導出したmedia kindとの差は項目単位で判定し、Susieプラグイン構成だけで
変動する`Image`と`OtherFile`間はそのまま適用する。それ以外の差はbundle全体を中断せず、該当項目だけを
`KindMismatch`としてスキップし、previewと完了結果へ件数を表示する。
Susieが静的テーブル上の動画 / 音声 / ZIP / PDF / 変換アーカイブ拡張子を画像として主張した場合は
`Image`とその静的種別の差になるため免除せず、件数表示付きの項目スキップへ倒す。実在する主要Susie
拡張子は静的テーブルと衝突せず、環境差だけで変動する通常形が`Image`と`OtherFile`間だからである。
2回目の走査では対象pathを再検証し、256物理項目 / 実record 64 MiB / 500 msのいずれかで
外側transactionをcommitする。各物理項目はSAVEPOINTで隔離し、1件の失敗が同じbatchの
他項目を巻き戻さない。走査途中のI/O・parse・validationエラーと明示キャンセルでは現在batchも
commitして部分完了結果を返す。プロセス異常終了時は現在batchだけがrollbackされ、完了済みbatchは
保持される。
manifestに記載された物理項目だけを上書きし、未記載項目を保持する。適用時は既存
`mimageviewer.dat` の mtime を編集 / タグ sidecar sync table に記録し、明示 import 後の
フォルダ再ロードで古い自動バックアップが欠落行を復活させないようにする。
ZIP / `ConvertibleArchive`のrating / tag / page-state familyは、manifestで選択されたoriginに
かかわらずsource pathと決定的cache pathの両prefixを同一DELETE文で消去してから、選択originへ
復元する。これにより直読みRARへ戻した後も旧cache値を表示・再exportしない。
`spreads` / `view_trim_books`も同様にsource/cache両コンテナprefixを消去してから
`container_key_base`で選択したoriginへrootとnested bookを復元する。`folder_thumb_pins`はこの削除対象へ
混ぜず、従来どおりsource keyだけを上書きする。
同一フォルダのsync upsertはsectionごとに1回へ集約し、file種別上存在し得ないDB familyは
削除・挿入処理自体を省く。family削除はpath indexを利用できるexact + range条件で行う。
dirty な自動 sidecar のserialize / temp書き込み / renameはimport workerの前処理で行い、
失敗時はDB更新を開始しない。項目の全15ストアはbundled SQLiteのATTACH上限を20へ拡張して
単一transactionへ参加させる。通常WALの`tags.db`はAppのidle connectionをworkerへ移して閉じる
だけでなく、App-globalなtag write workerの既存jobをdrainして保持接続の解放ACKを待ってから、
import中だけDELETE journalへ切り替える。解放要求中に到着したタグjobは捨てずにworker内へ保留し、
importの成功 / キャンセル / 失敗 / 開始失敗後に要求を下ろして、次jobの直前にWAL接続を遅延再open
して処理する。接続解放ACKには上限を設け、到達しなければimportを開始しない。他の参加DBを含め
WAL / MEMORY / OFFを検出した場合は
適用前に拒否し、SQLite super-journalによるbatch内の全参加DB atomicityを維持する。
manifest 由来の画像本の相対ページは provenance と canonical container を一覧から
metadata / thumbnail / fullscreen worker まで保持し、実 I/O では開いた同一ハンドルの
final path を再検証してから、そのハンドル由来の metadata / bytes だけを利用する。
archive member / nested container の identity は大小文字を区別せず、`\`を`/`へ統一してから
重複検査・DB書込へ共用する。import中は値単位の表示差分を送らず、既存UI cacheを安定snapshotとして
保持する。成功・キャンセル・部分成功の終端で変更sectionを1回だけ確定し、影響するmain /
detached / parked contextの現在項目キーを3 ms / 2048項目のフレーム予算でcompact snapshot化する。
専用refresh workerがrating / tag / page state / container state / pinをSQLiteからbatch取得し、
UIは要求構築時に焼き込んだViewerContextIdのresidenceがMounted / AtRestで、かつitems generationが
一致する完成cacheだけをswapする。Retiredへの遅延結果は正常な破棄、Unknownはidentity不変条件違反として
診断して破棄する。不一致時は
モーダルを維持して再snapshot・再取得する。bookmark / media cacheは影響contextごとに1回だけ
無効化し、全体page badge集合もworker生成snapshotへ置換する。folder pinのlookup identityは
通常ロードと共通化し、current folder / 現在のZIP本 / ZipDirのliteral-effective aliasを含める。
cache swap後のvisible/facet/details/selection再計算と代表サムネ再materializeは各contextをmount
している間に行う。pin refreshは通常ナビゲーションの`load_folder`を通さず、影響thumbnailだけを
evictし、不要になった`#pin:` catalog行の削除とvideo pin WebPのseedを同じcontextで行う。古いpin
snapshotを持つin-flight結果はcontext所有のthumbnail worker channel / queueを交換して到達不能にし、
items generation・fullscreen・media sessionを変更しない。video pinは現DBにWebPがない項目も再生成
対象へ含め、削除を通常フレーム抽出へ戻す。終端refreshはimport本体と別のcancel tokenを持ち、
App終了時はDB chunk間で中断する。
page state import時は永続edit preview cacheの破棄完了を非同期ACKで待ち、main / active detached /
paused detachedのmaterialized previewを失効させる。各contextではimport前後のどちらかに編集が
あるthumbnailも再要求するため、編集削除後の旧WebPを残さない。
再帰exportでjunction / symlink directoryを追わない場合は、除外数と先頭pathを完了画面へ表示する。
項目SAVEPOINT失敗は「一部未反映」として先頭path・理由を表示し、全件は常時
`mimageviewer.log`へ記録する。import本体はmanifest / preflight / DB open / target verify /
SQL apply / commit（合計・最大）の時間、終端refreshはcontext / item数と時間を同ログへ記録する。
PDFの仮想項目はruntime keyと同じ`page_<u32>`だけを許可し、評価のkind / page numberもkeyと
一致させる。export元DBのJSON化されたマスク・隠蔽図形などを厳密にparseし、破損値は空の状態へ
変換せずストア名と項目keyを伴うエラーにする。
性能ログを有効にした場合は同じ境界を`logs/perf_events.jsonl`の`metadata_import`イベントにも残す。
進捗path欄は3行分を固定確保し、折り返しによってキャンセルbuttonの位置を動かさない。
実装と境界条件は `metadata_transfer.rs`、モーダルと writer drain は
`ui_dialogs/metadata_transfer.rs` が所有する。

---

## 5. Phase 区分 (現状)

| Phase | 内容 | 状況 |
| --- | --- | --- |
| 1 | コアビューワー (グリッド・フルスクリーン・設定永続化) | ✅ |
| 1.5 | サムネイルカタログ (SQLite + WebP) | ✅ |
| 2 | AI アップスケール / デノイズ / Inpaint (ONNX + DirectML) | ✅ (Real-ESRGAN/Real-CUGAN/NMKD-Siax, RealPLKSR, MI-GAN は消しゴムから利用) |
| 3 | お気に入り・ツールバー・ZIP・WIC・動画・アニメーション | ✅ |
| 3.5 | 画像補正プリセット (フォルダ別 4 種 + グローバル + 保存スロット 10) | ✅ |
| 3.6 | 消しゴム (Lasso/ブラシ → MI-GAN) | ✅ |

---

## 6. 関連ドキュメント

| ドキュメント | 読むべきタイミング |
| --- | --- |
| [display-pipeline.md](display-pipeline.md) | サムネイル表示やフルスクリーン描画を触るとき。**補正・AI・回転がどこで適用されるかの決定版** |
| [async-architecture.md](async-architecture.md) | 並列処理・キャンセル・キャッシュ競合を触るとき。ワーカー構成の一覧 |
| [virtual-folders.md](virtual-folders.md) | ZIP/PDF 関連を触るとき。**通常画像パスと分岐する箇所のチェックリスト** |
| [preset-and-adjustment.md](preset-and-adjustment.md) | 補正・プリセット・AI キャッシュを触るとき。無効化ルールの早見表 |
| [local-adjustment-layer-v1.1.0-plan.md](local-adjustment-layer-v1.1.0-plan.md) | 補正レイヤー / ローカル調整 / レイヤー合成を触るとき。本体統合タスクリストと cache 方針 |
| [local-adjust-filter-candidates.md](local-adjust-filter-candidates.md) | 補正レイヤー効果候補、効果ピッカー、効果追加方針を触るとき |
| [search-architecture.md](search-architecture.md) | 検索 / インデクサ / タグ関連を触るとき。**Ctrl+S/F/G の経路とインデクサパイプラインの全体像** |
| [music-integration-plan.md](music-integration-plan.md) | 音楽ビュー / 音声再生 / 動画→音声モードを触るとき (Inc 履歴含む正本) |
| [video-architecture.md](video-architecture.md) | 動画 HUD・native presenter・動画→音声モードを触るとき |
| [detached-rework-plan.md](detached-rework-plan.md) | detached viewer / F12 / 複数ウィンドウを触るとき。**§2 憲法 (BA-1〜7) 必読、凍結ルールあり** |
| [spec.md](spec.md) | 機能仕様・設定項目の正式な定義 |
| [catalog-design.md](catalog-design.md) | サムネイルキャッシュ DB の詳細設計 |
| [archive/performance-refactoring/thumbnail-memory-redesign.md](archive/performance-refactoring/thumbnail-memory-redesign.md) | サムネイルメモリ管理の背景経緯 |

---

## 7. 修正時のチェックリスト

1. **触る機能の doc を必ず先に読む** (上の表から該当ページを選ぶ)
2. **通常画像 / ZIPImage / PdfPage の 3 分岐を忘れない** — ZIP/PDF 対応漏れは頻出バグ
3. **サムネイル経路とフルスクリーン経路の両方で整合性を保つ** — 片方だけ修正すると表示が食い違う
4. **テクスチャキャッシュの無効化タイミング** — 補正・AI・回転を変更したら正しいキャッシュをクリアしているか確認 (`preset-and-adjustment.md`)
5. **ドキュメント同時更新** — [public-wording-policies.md](public-wording-policies.md) の「コード修正時のドキュメント同時更新」セクションに従う

§1.333の最後まで視聴した動画・音声は、Settingsの `video_watched_to_end` 集合とsettings.db同名の追加tableで記録する。既存再生位置表・再開動作は保持し、集合から本体一覧へ満タンを描く。詳細はbook-resume-meter-plan.md §11。


### 一時通知の時間ポリシー（§1.402）

`feedback.rs` が情報/警告/エラーの型と表示時間の倍率を所有する。
App の発行境界で寿命とnative fadeを確定して、既存 toast state と native transport へ渡す。
標準は従来の面別の時間・fadeを維持し、nativeのlinger/fadeの既存不一致は今回修正しない。
動画内部の発行元は種類を明示し、既存 NativeChromeSnapshot から選択値を読む。
設定は既存 settings.db KV の追加項目で、旧項目欠落時は標準へ移行する。
新しい worker・channel・detached context state は持たない。
[時間・例外・棚卸しの詳細](notification-duration-policy.md)。

## AVIF 内蔵デコード（§1.407）

`crates/libavif-sys` の C shim が libavif の構造体 ABI を閉じ込め、既存の静的
libavif / dav1d / libyuv をリンクする。safe Rust APIが予算検査・RGBA8確保・表示変換と
画素を確保しない寸法解析を所有し、core / Remote / EPUBで共用する。`avif_decoder` は本体のperf/ログ層。`image_decode` は形式選択と WIC 接続、使用デコーダと向き状態を共有する。
worker・queue・context owner・永続 schema は追加しない。
