# §1.372 補正編集のページ identity と viewer context 所有

2026-10-12、branch `next-fix`、初回調査基点 `f182037c2`、追補基点 `440fdf69b`。
状態: **独立レビューREVISEのP2 2件を設計へ反映・再レビュー待ち。§1.372 の製品コードは未変更**。
正本は [next-release-backlog.md §1.372](next-release-backlog.md)。実機での再現報告ではなくコード調査。

## 1. 問題と既存の保護

検索の結果を逐次採用すると、画像Aのslotへ画像Bが入り、Aは別slotへ移ることがある。
`replace_search_view_items` は読込済み編集のindex mapを消して新投影へ交換するが、
編集中セッションとUndoの対象を同じ所有境界で交換していない。
そのため終了時に古いindexから保存先を取り直す編集は、BへAの内容を書き得る。

現行コードを区別する。backlogの「補正Undoはidxのみ」は全種類に当てはまらない。

| 現行の資源／処理 | 既存契約と残る境界 |
| --- | --- |
| `PageAdjustmentTarget` (`app.rs`) | 論理page key・location・sidecar座標・製本判定をcaptureできる。`idx_hint`は一覧内の投影用で、context／items世代の所有証明ではない |
| 通常補正 `AdjustUndoScope::PageKey` (`undo_stack.rs`, `undo_ops.rs`) | 既に安定キーを記録し、Undo/Redoごとに現一覧で一括解決する。非residentはDBだけを戻す。これをidx-onlyとして作り直さない |
| `AdjustUndoScope::Page(usize)` | キー取得不可のlegacy退避先。新しい編集所有のfallbackとして広げない |
| `LocalAdjustmentChange` | idxのみ。適用時に `set_local_adjust_layers_for_idx(c.idx, ...)` が現在の一覧からキーを求めるため、別ページを更新し得る |
| `AdjustmentDragSession` | fs_idxとbefore。終端時の対象・live値がindex mapに依存する |
| local shape key／canvas／shape drag／brush | fs_idxとレイヤー／マスク／図形番号。beforeが別Optionのgestureもあり、確定時に現index mapの値を読む |
| `ViewerContextBundle` | items世代、補正map、local描画cache／pending、page-edit snapshotはcontext所有。上記interactionと `meta_undo` は交換対象ではない |
| `LocalAdjustWritePending` | enqueue時のkey／writer世代／sidecar座標を保持し、後続idx処理は同keyでgateする。元の保存対象保護は存在するが、snapshot volatile更新の所有contextが不足する |
| `PageEditSnapshot`／prepared projection | exact page key、write stamp、context／items世代／edit revisionによる採用fenceが存在する。local layersの全文はidx mapにあり、volatile keyだけでは未確定draftを復元できない |

`persist_local_adjust_shape_key_edit` と各 `persist_*`、`capture_local_adjustment_undo`、
`apply_local_adjustment_change_to_app`、`set_local_adjust_layers_for_idx` が同じ不変条件を破る経路。
見える症状のUndoだけをguardする修正では、タイマー確定・fold・遅い完了が残る。

## 2. 採用案と簡素化の比較

**ページの保存先は安定キー、編集中資源はcontext所有、index mapは投影だけ**とする。
検索streaming、閲覧位置のexact identity transfer、編集、Undo/Redoを維持する。

先に次の簡素化を検討した。

| 案 | 判断 |
| --- | --- |
| 毎回の検索採用で既存finish/clear境界を通しUndoを消す | 状態は減るが、通常の検索中に編集やUndoが勝手に終了する。リリース済み操作を失うので無承認では採用しない |
| 編集中だけ結果採用を止める／検索をmodalにする | 頻繁な検索結果更新で待機・解除操作が必要になる。通常利用を不便にするため採用しない |
| 旧idxごとのguard／追加pending群で対応 | beforeと対象、contextとmapの所有分裂が残り、別終端ごとの対策を増やすため不採用 |
| 既存bundleに単一のtyped編集ownerを移し、stable targetへ統一 | mount/depositのたびに引継ぎ状態を作る必要がなくなる。採用案 |

§1.371は利用者決定によりRating移動で既存のfinish/clearとrating履歴保持を使った。
本件でその決定や通常open/closeの履歴境界を撤回しない。検索の同一覧reindexが
別の画像への保存を作らないよう、現在の編集保存先を根本から固定する。

## 3. identity と世代の契約

以下は責務の形を示す設計用名称であり、型名やフィールド数はレビュー後に確定する。

- `PageEditOwner` は `ViewerContextId` と既存 `PageAdjustmentTarget` を持つ。
  targetのkey・永続化座標は編集開始時にcaptureする。標準補正は既存Favorite UUID／Globalでcaptureする。
- `ResolvedPageSlot` はowner context、現在の `items_generation`、exact key、indexの組。
  元の番号はヒントに過ぎず、世代が変わったら同じkeyを現投影から解決する。
  別key・別contextのslotへ書かない。同keyが複数slotにある場合は全投影を更新する。
- items世代は**居場所**の世代。Aの位置変更だけでAへのUndo／draftを失効させない。
  local描画世代は**描画内容**、writer世代は**同keyの保存要求順序**。
  これらを交換・流用しない。items世代をDB identityに追加しない。
  completionが参照する内容の版はownerの同keyの版であり、context全体の
  `page_edit_revision`（prepared採用fence）とは区別する。別keyの編集でAの保存完了を
  永久に未反映にしない。既存local描画世代／同keyのwrite generationをowner側へ揃え、
  同じ意味の新しいcounterや独立したpendingを重ねない。
- ページが一覧から外れても、確定済み変更／Undoの保存先はcaptureしたAのまま。
  非residentのDB更新にBのidxを使わない。未確定gestureはownerのdraftを保持し、
  明示終了時に既存の完了済み編集確定／未完gesture取消の区別で畳む。
- keyの取れない項目は既存の編集可能種類判定で入口を閉じる。新規idx fallbackは作らない。
  ZIP/PDF/製本ページのキー・sidecar規則は既存 `page_adjustment_target_for_idx` を再利用する。
  Remoteの非resident targetやFavorite/Global補正をviewer index前提へ変えない。

一括Undo／一覧採用で対象件数×全件走査を作らない。既存PageKeyの一括解決と
`PageKeyOrder` を再利用し、採用境界で一度だけ位置を解決する。新しい全件DB読取をUIへ追加しない。

## 4. 単一編集状態とcontextのライフサイクル

`ViewerContextBundle` がページ編集のinteractionを所有する。App上のmounted部分も
同じownerの投影として扱い、交換時に別contextの状態を借用しない。

通常ページdrag、標準drag、local shape key、canvas、shape drag、brushなどの
相互排他的な編集中状態は一つのenumへまとめる。各variant内にowner、before、
現在のdraftとgesture payloadを持ち、別Optionのbeforeやfield-presenceを状態判定に使わない。
完了済みキー編集と未完のラッソ／線の取消の区別をvariantの終端で表す。
レイヤー／マスク／図形番号もそのdraft文書内の座標として保持する。
編集対象に結び付くmode／選択レイヤー／図形／maskの状態もこのcontextのeditorに属する。
全体で共有するブラシ設定などの利用者設定までper-contextへ移して操作仕様を変えない。
単一enumへ束ねる前に実際の同時成立経路を確認し、合法な組を無断で除去しない。

| 遷移 | ownerの扱い |
| --- | --- |
| 編集開始／変更 | 現context・stable targetを一度captureし、そのdraftだけを変更。見た目は同keyのslotへ投影 |
| 同一覧reindex／stream採用 | 対象keyとdraftは維持し、slotだけ再解決。新DB snapshotでdraftを上書きせず、Undoを消さない |
| 普通のmount/deposit／park/resume | interactionとdraftをbundleと一緒に移す。非表示で入力を受けず、兄弟contextの終端処理をしない |
| split/fork／新context | 新ownerはIdle。描画用snapshotを複写する場合もgesture／beforeを複製しない。元ownerのparkならそのownerへ移す |
| 明示ページ移動／パネルfold／close／新source採用 | 所有中の編集だけを既存のfinish/cancel契約で畳む。次のページへbeforeやgestureを持ち越さない |
| Undo／Redoの受理／別の履歴操作の受理 | §4.1の共有履歴境界で完了済み保留を先に確定。Undo／Redoのpopや次操作のbefore captureより前。単なるmountとは区別 |
| 対象が非resident | draftはowner側にありBへ投影しない。明示終了で完了済み変更をAへ確定、未完gestureは取消 |
| context drop | 完了済み変更を既存writerへ配送してownerを破棄。保存workerはAppの既存寿命を使い、UI待機や通知待ちの延命を追加しない |
| App通常終了 | mountedと全at-rest／park中ownerを§4.2で畳んで配送し、その後に共有writerを停止。drop任せにしない |

inputもownerに属する。readerが同じAを表示し続ける再配置ではAのgestureを継続する。
一覧外でもretained readerがAを表示しているなら、その安定target／geometryで操作する。
実際に表示ページがBへ移る場合は、旧Aのfinish/cancelを**表示交換前**に終え、
Bのpointer／キー入力をAのgestureへ配送しない。gridのselectedだけがBへ移る場合と区別する。
非residentの保持は元の保存先やUndoを失わないためであり、別ページの入力を受ける許可ではない。

`meta_undo` は既存の共有履歴とclear境界を維持する。全履歴をper-contextへ分割して
レーティング／タグの操作順を変えない。local undoはstable targetへ変換し、
通常PageKeyと同様に非residentでも元のページへ配送する。元context情報は
draftと描画投影の所有判定に用い、contextが閉じたことを理由に永続UndoをBへ付け替えない。

### 4.1 共有Undoへの受理順序（レビューP2-1）

現行の `apply_meta_undo`／`apply_meta_redo` はstackを先にpopし、形状キー編集は
`persist_local_adjust_shape_key_edit` がidle／終端で保存と履歴登録を行う。この順序を変更する。
**次の履歴操作を受け付ける前に、先行する完了済み保留編集を安定targetで確定する。**
メモリ上の終端と既存writerへのenqueueだけで順序を決め、writerの完了は待たない。

「完了済み保留」は、キーの一押し／反復で既に適用された値、既存persist対象の
shape drag／canvas drag／brushの有効な変更など、現行finishが保存する内容をいう。
まだページ内容になっていないラッソの途中点や未完成の新しい線／図形は取消対象であり、
保存要求もUndo entryも作らない。既存のfinish/cancel区別をtyped variantの終端に移す。

1. Undo／Redoは既存の入力・IME・tag-busy等の受理判定を通った後、**can_undo／can_redoの
   判定とpopより前**に、mountedとpark中を含む全ownerの完了済み保留を確定する。
   beforeとdraftをownerからtakeしてIdleへ移し、変更があれば保存enqueueと通常のUndo登録を
   一度だけ行う。受理contextの未完gestureは取消し、その後で既存Undo／Redoを適用する。
   他contextの未完gestureはこの共有履歴整理だけでは取消さず、別keyのdraftを触らない。
   受理されなかったキーにはこの境界の副作用を発生させない。
2. Rating／Tag／通常補正／local補正など**別の履歴操作の開始時**にも同じ確定境界を通す。
   次操作のbefore capture・メモリ変更・worker投入より前に行い、履歴push時だけの後処理に
   しない。同一ownerの同じキー反復だけは既存の一連の編集として継続し、別対象／別操作なら
   先行runを閉じる。park自体はdraftを保持できるが、Bで最初の履歴操作を開始するときに
   park中Aの完了済み保留を先に登録する。新contextで保留runを作る前にもこの境界を通る。
3. 各 `push_*_undo_entry` の呼出しを棚卸しし、同期操作は上記開始境界と対にする。
   Tag等の既存非同期操作は**投入時**に先行補正を畳み、結果の履歴登録は既存の
   pending／成功確認へ残す。遅い結果のpushで、その投入後に始めた補正runを
   先行編集として畳み直さない。既存tag-busy規則を拡大する待機・poll・保存barrierは加えない。
   補正保存のcompletionは履歴を積まず、確定時に登録したentryを再登録しない。
4. idle／release／foldと共有履歴境界は同じowner終端を使う。低水準の終端はownerをtakeして
   安定target・before・after・summaryを返し、Appが保存と履歴へ配送する。配送から再び
   共有境界を呼ぶ再帰を作らない。before==afterは既存のno-opで、Redoを消さない。
   履歴を消す既存open／Rating境界では、先に必要な保存を配送してから既存clear／retainを行う。

これにより `Aのキー編集→idle前にCtrl+Z` は、Aのentryを登録→そのentryをUndo→Redoへ移動の順。
消費済みownerへ後のtimer／fold／completionが来ても新たな保存・履歴pushを作らず、
Undo結果を上書きせずRedoも消さない。Redo受理前に実際の新規保留変更があれば、
まずそれを新操作として登録するので既存Redoは通常のpush規則で消える。変更なしなら維持する。
`Aをpark→Bの操作→Aをresume／idle` はA登録→B登録の順となり、Aの再登録はない。

入力ownerの切替だけで全draftを捨てる案や履歴順のtimestamp／予約slot／新queueを足す案を
採らず、次の履歴操作の開始境界へ確定を揃える。完了済みで未登録の補正runを複数contextに
積み重ねない不変条件をこの入口で作るため、HashMap列挙順で履歴順を決めない。
park中の未完gestureが再開して有効な変更を受け取る場合も、新しい入力受理境界を通す。
通常のmount／検索の同一覧再配置／read-only openだけでは共有履歴境界を発火しない。

### 4.2 App終了の全owner確定とwriter停止（レビューP2-2）

現行 `persist_window_state_and_flush` はmountedの `commit_local_adjust_pending_edits` を呼んでから
`finish_local_adjust_writes`→`flush_all_sidecars`→`sidecar::wait_for_pending_writes` と進む。
bundleへinteractionを移した後は、**全ownerから要求を作り切る前にwriterを停止しない**。
App通常終了の既存経路を次の順序へ揃える。

1. 通常終了の受理後は既存終了経路内で新しい編集入力／idle確定の配送を終える。
   mounted ownerとregistry内の全at-rest／park中ownerを、所有表に従い重複なく一度ずつ巡回する。
   at-restを画面へmountして確定せず、各ownerのtargetとdraftから終端要求を取り出す。
   同一覧から消えたページや非表示のwindowも対象に含める。
2. 各ownerの完了済み保留をtakeし、capture済みtarget・文書・保存世代・originから既存の
   保存要求を作って、まだ生きている共通writerへ全てenqueueする。通常／標準補正も既存の
   保存経路へ渡し、設定値の反映後に既存settings保存を行う。未完の線／ラッソ等は
   取消して保存しない。共有Undoへの確定は§4.1の終端を再利用し、dropで二重配送しない。
3. **全contextの配送が終わってから**既存 `finish_local_adjust_writes` の
   `shutdown_and_collect`（drain→stop→join）を一度だけ呼ぶ。各completionのcaptured key・
   sidecar座標・originで既存mirrorへ反映する。originを破棄した場合でもmirrorの配送先は
   request／completionに残し、現在mountedのidxや最新pendingから取り直さない。
4. 全completionを反映した後、既存 `flush_all_sidecars` でdirtyなmirrorをqueueし、
   終了に既にある `sidecar::wait_for_pending_writes` を通してからowner／Appを破棄する。
   既存sidecar所有者もこのflush前にdropしない。既存待機の順序を利用し、新しいworker・
   barrier・timeout・完了待ち・終了中の回復処理を作らない。

全ownerの終端はUI同期DB読取／新規同期I/Oを行わず、既存保存APIへの要求配送である。
トレイ退避（`PersistScope::ProcessKeepsRunning`）は通常終了（`ProcessIsStopping`）と区別し、
既存の確定・到着済み結果poll・
sidecar queueだけを行い、writer停止／join／sidecar完了待ちへ拡張しない。
正常終了のDB／sidecar保存を実worker fixtureで検証し、まれな保存失敗は既定のログ＋通知に留める。

## 5. 保存・projection・非同期完了

保存APIをtarget基準に統一する。idx版はその場でcaptureする入口だけとし、
既にcaptureしたsession／Undoの終端はidx版へ戻らない。
副作用（DB書込、sidecar、badge、cache、smart folder通知）は既存writerの境界を再利用する。
UI同期SQL／I/O、待機、sleep、repaint追加で順序を調整しない。

編集中の全文はownerのdraftが正本。preparedなdurable snapshotとindex mapは読み取り投影。
採用時は既存 `PAGE_EDIT_WRITES` stamp／context／items世代／edit revisionのfenceを維持し、
その上へ同keyのdraftを投影する。volatile keyだけ残してlayersを捨てない。
ローカル編集の全文を複数mapの独立した正本として持たず、既存layers mapを投影へ位置付ける。
非activeだが保存待ちの値は、既存write ownerの文書とrevisionに沿って扱い、
旧prepared値が最新メモリを戻さないことを試験する。検索採用自体はDBへ書き込まない。

App共有のlocal writerは増設しない。各request/completionにenqueue時のkey・世代・
sidecar座標・表示更新のowner contextとedit revisionを対応させる。
旧completionのoriginをkeyごとの最新pendingから借りない。completionはcaptured keyの正本／mirrorへ配送する。
遅い旧世代のCommittedも既存の順序付きmirror契約を維持する。最新completionだけが
対応する保存pendingを畳む。volatile解除／cache更新は所有contextの同key・同revisionへ行い、
現在mountedの別contextのsnapshotを変更しない。
同ページを表示する他contextへの更新は、永続変更の同key publicationとして明示的に行う。
兄弟の別keyのdraft・cache・workerは触らない。read-only open/closeはmutation-style invalidationを発行しない。
同keyの別contextにactive draftがある場合、そのdraftはpublicationで上書きしない。
表示中draftは各ownerで独立し、明示確定した文書は既存の同key writer順序で保存する。
自動merge／rollbackは加えない。この正常到達する二窓経路も、保存順と各draft保持を固定した試験で検証する。

保存失敗は既定のログ＋通知で扱い、retry・rollback・回復worker・新たな完了待ちを作らない。
既存page-edit reconcileの仕組みは本件で増設しない。renameでkey自体が変わる経路は
既存の明示編集終端・key付替え境界へ接続し、一般一覧reindexと同一視しない。

## 6. §1.371のgrid cursor ownerとの関係

`GridClickSelectionAnchor { index, items_generation }` と `apply_grid_cursor_move` は
selected／Shift起点の所有者。検索採用はreader、selected、anchorを各stable content keyでremapする。
編集ownerはこの選択状態から独立する。選択がBへ変わっても、先に開始したAの編集先はA。
カーソル／readerの移動結果から旧sessionの保存キーを取り直さない。
一覧採用ではitems・page-edit投影を交換し、readerのexact transferとcursorのpaired remapを
既存順序で行った上で編集draftを同keyへ投影する。二度目の近傍選択を編集側に追加しない。
Rating集合owner／grid cursor owner／編集ownerのそれぞれに一つの決定境界を保つ。
§1.371のRating用finish/clear境界は、この新編集ownerのfinishを呼ぶように接続し、
rating-only Undo保持と遅延openの既存契約を維持する。

## 7. 実装棚卸しと受入試験

実装候補は `app.rs`（target／保存／completion）、`undo_stack.rs`、`undo_ops.rs`、
`ui_adjustment_panel.rs`（全gesture／timer／fold）、`app/viewer_context_registry.rs`、
`global_search_ui.rs`、`app/page_edit_snapshot.rs` とそのhandler tests。
必要に応じ既存local writerのrequest/completionを拡張するが、新workerは作らない。
入口はslider／reset／copy/paste／preset／bulk、shapeのKeyAction、canvas／brush／mask、
Undo/Redo、idle確定。出口はrelease／別shape開始／fold／navigation／close／取消／drop／失敗。
通常フォルダ、検索、Rating、Collection、ZIP/PDF、製本、Remoteの同型入口を確認する。
Smart／Collectionの実adopt handlerは、同一覧再配置ならdraft再投影、新source採用なら
所有editorのfinish/cancelという区別で接続する。cursor変更だけで終端を代用しない。

| 非対話回帰 | 必須のassert |
| --- | --- |
| A idx0を編集→検索採用でB idx0／A idx1→Undo／Redo | AのDB／sidecarだけがbefore/afterへ変わる。B不変。通常PageKeyとlocal双方 |
| Aが一覧から消える→Undo／Redo／終了 | 元のAへ永続化し、Bの表示／保存／beforeを変えない |
| slider／shape key／canvas／shape／brush中に複数stream採用 | ownerとbefore/draft維持。timer／release／foldでAへ一度確定、完成済み編集を失わない |
| 旧prepared snapshotがdraft開始前／保存enqueue前に作られる | 既存stamp不一致拒否と最新draft overlayにより値を戻さない |
| 2 contextの異なるkey／同key、mount/deposit／park/resume／fork／close | 読取だけの交換は相手のgestureを確定／取消しない。別履歴操作の受理は§4.1で完了済み保留だけを先に確定。同key更新はpublication、無関係なdraftは不変 |
| Aのキー編集保留→idle前の実Undo handler→idle／fold／completion→Redo | Aを一度登録してそのentryをUndo。AのDB／sidecarはbefore、後続timerで戻らず、Redo entry維持。Redoでafterへ。B不変 |
| 既存Redo＋新規保留変更／変更なし／未完ラッソ→実Redo handler | 新規変更は先に登録し通常pushで旧Redoを消す。no-opはRedoを維持。未完gestureは保存・履歴登録せず受理contextで取消 |
| Aをpark→BでRating／通常補正／local補正／Tag投入→Aをresume／idle | 実handlerでAの確定・enqueue・履歴pushがBのbefore capture／変更／投入より先。stackはA→B、Aの再pushなし。Tag結果到着時に後発draftを先行確定しない |
| park中Aのキー編集保留＋mounted Bの未完線／ラッソ→通常終了 | 実終了handler＋実workerで全context要求配送→writer stop→completion mirror→sidecar flushの順。Aの正しいDB key／sidecarへ保存、Bの未完gestureは保存しない。二重enqueueなし |
| mounted／at-rest双方の完了済み値、一覧外A、origin drop後の終了completion | 全ownerを一度回収してからshutdown。capture済み座標へDB／sidecar保存し、別contextのidxを使わない。既存ProcessKeepsRunningはworkerを停止・待機しない |
| completion前にreindex／別context mount／origin close | captured key／sidecarへ保存。volatile／cache更新はownerと最新revisionだけ。旧Committed mirrorも維持 |
| A保存待ちの間に同contextのBを編集 | Aの最新保存完了はAのvolatileだけを解除できる。context全体revisionの変更だけで拒否しない。Bのdraftは不変 |
| 同key複数slot／同idx別key／items世代だけの変化 | keyごとに全投影更新、idx再利用は保存先にならない、reindexだけではUndo無効化しない |
| Rating自動移動、検索・通常open/close、明示navigation、gesture取消 | 既存履歴境界とfinish/cancel仕様を維持。次sessionへbeforeを持ち越さない |
| キー操作とpointer操作の等価入口 | キーは既存KeyAction経由。shape key idle、フォーカス／IME入力保護、Undo順序を維持 |

pure state-transitionと実handler／使い捨てDB fixtureを組み合わせる。UI snapshotは
既存のpanel表示が変わる場合だけ更新する。通常／portable core check、fmt、対象tests、
項目末一度のtest-full、確認用buildが実装後のgate。設計段階の試験実行は0件。

## 8. レビュー境界

この変更は編集内容という利用者データと、非同期・複数windowの所有境界に関わる。
**本書で止め、独立設計レビューと設計責任者の合意後に実装する。**
`440fdf69b`の独立レビューは§1.369実装ACCEPT、§1.372設計REVISE（P2 2件）。
本追補は§4.1／§4.2と実handler受入条件へ2件を反映した設計修正であり、製品実装は0件。
今回の試験実行は0件（文書のみ）。`git diff --check`で差分形式を確認する。
新しい操作制限や通常利用の遅延を必要とする実装案が出た場合は、実装前に再相談する。
detachedの述語／viewport／HWND制御は本設計の対象に追加しない。bundleの所有統合を
実装する際は [detached-rework-plan.md §2](detached-rework-plan.md) の構造レビューを守り、
触れた範囲と合意を同書§11へ記録する。設計書の存在をその合意の代わりにしない。
