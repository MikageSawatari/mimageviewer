# §1.373 本棚への追加直後の一回取り消し

2026-10-13改訂、`next-list` / `ea060b66e` 設計基点、確認基点 `df20dba7b`。
**利用者決定 (2026-10-12): R4案1を採用・設計確定。独立設計レビュー提出用、製品未接続**。
正本項目: [next-release-backlog.md §1.373](next-release-backlog.md#1373-本棚ツールバーの右クリックで本棚へ追加した操作を-ctrlz-で取り消す--利用者の要望-2026-10-11)。
アプリは起動していない。利用者の実装指示で試作を開始したが、R4の判断が必要になった時点で
製品への接続を撤回し、未検証の試作を`target`へ退役した。今回その判断を設計へ反映し、
独立レビュー前に再接続しない。

`001a0dabf` の独立設計レビューは **REVISE**。本改訂は coordinator 判断を優先して
P1-1 の過剰な同定を撤去し、P1-2 の既存 book operation owner への統合、P2 の失効入口を
確定した再レビュー用。ごみ箱方式は利用者決定済み (2026-10-12)。

R2 (`1c313a2ec`) の **REVISE** を反映。新規Busy条件を `UndoAppend` に限定し、
ごみ箱への実移動は `PostDeleteItem` の結果で判定する。既存本操作の受付規則は変えない。

R3 **ACCEPT** 後の試作で判明した通常経路について、利用者がR4案1を決定した。
**追加中だけ対象本フォルダの自動観測をAppend終端へ配送し、外部変更の表示が数秒遅れることは
了承済み**。追加全体のmodal化や既存本操作の新規Busyは導入しない。

## 1. 正本の前提と実装の差異

`rg -n 'book_membership|membership_db' src Cargo.toml` は一致 0 件。
`compile-book-plan.md` §11 は「設計のみ・未実装」であり、この基点には登録済みバッジ、
membership DB、同じ source の重複 skip がない。バックログ §1.373 の「更新している」前提は
この worktree の実装と一致しない。§11 の実装済み扱いへの変更や、新 DB のついでの実装はしない。
他ブランチにその機能があるなら、統合後の実コードで仕様・削除境界を再レビューする。

既存の追加は `ui_main.rs:11340` → `add_grid_selection_to_named_book`
(ui_fullscreen.rs:46926 付近) → `start_book_append_to_named` (app.rs:49377)
→ `append_pages_at_with_raw` (books.rs:780) → `BookAppendSummary`
→ `poll_book_op_pending` (app.rs:49828)。チェック優先、Stack は各 member に展開、
Image／ZIP entry／PDF page 等を `BookPageSource` に固定する。
通常 byte copy／必要な headless 合成の source 選択は維持する。
`bake_stage.rs` は焼き込み段の語彙を所有し、ファイル追加の成否・membership は所有していない。

worker は番号付きページを末尾に書き、summary は件数、最初の path、編集／semantic copy
mapping しか返さない。全出力の identity は返らない。途中の write が失敗すると Err になり、
既に作ったページがあっても summary は届かない。この例外に rollback は追加しない。
`poll_book_op_pending` は metadata copy、件数、必要な一覧更新を行う。

Undo は `undo_stack.rs` の軽量 metadata の undo/redo スタックで、ファイル操作を含めない。
`undo_ops.rs:209` は同期的に entry を適用して redo へ積むため、書庫の read／Shell削除／
hashing をそこへ直接足せない。現在のメタ Undo キーは `handle_meta_undo_keys` (:335) の
`consume_fixed_chord`。消しゴム／隠蔽／テキストには個別 `KeyAction` と Undo がある。

## 2. 提案する操作仕様

全履歴・Redo・番号再計算を持つファイル Undo は作らず、利用者が許容した
**最後の本追加一回だけの token** とする。本棚右クリックと、同じ append に到達する
Ctrl+B／メニュー／viewer 追加は同じ成功 receipt を使う。別本 transfer は対象外。

- 成功した一回の追加 (複数ページ・Stack member を含む) を Ctrl+Z 一回で取り消す。
  元画像、元 ZIP／PDF、既存本ページ、本フォルダ自体は削除しない。
- コピー追加を Redo で再実行しない。metadata Undo/Redo の既存履歴は保持する。
- 対象本への再追加、並べ替え、移動、コピー、削除、改名、後段編集、タグ／★／しおり等の
  mutation が入ったら token を失効させる。別の通常 Undo 操作が入った場合も失効し、
  Ctrl+Z はその新しい操作を既存 stack から戻す。
- カーソル／check／scroll の変更だけでは失効しない。別場所への移動・source一覧の置換・
  viewer の通常 Undo 終了境界では失効。アプリ終了で捨て、永続化しない。
- 外部で本の構成／ファイルが変わった場合、検証で検出し「本の内容が変わったため追加を
 取り消せません」と通知して token を消費する。**その Ctrl+Z で古い metadata Undo を
 続けて実行しない**。一回の入力が二つの意味を持たないようにする。
- 全 skip (将来統合される場合)、失敗、partial write の Err は token を作らない。
  ログと通知、本を開いて残ったページを整理する既存導線で扱う。再試行や部分 Undo は作らない。

### 削除方式 (利用者決定済み)

利用者決定は **今回追加したコピーだけをごみ箱へ移す**。完全削除案は撤回済み。
元ファイル・既存ページ・本フォルダは削除しない。既存 delete worker の Shell recycle helper を
再利用し、ごみ箱に送れない保存先では完全削除に自動 fallback せず、取り消せないと通知する。
現行一般 Delete の nuke warning／完全削除許容の flags をそのまま Undo へ流用しない。
ごみ箱必須の設定で拒否できる共通 Shell helper に限定して再利用する。
**利用者決定 (2026-10-12): ごみ箱へ移す形で確定**。

成功はpath消失／`PerformOperations`の全体HRESULTだけでは判定しない。
各exact sourceに結び付いた `IFileOperationProgressSink::PostDeleteItem` の `hrDelete` が
実操作の成功を表し、`psiNewlyCreated` がごみ箱内itemを返したものだけをrecycle成功に数える。
callback欠落／失敗／NULL item (完全削除) はごみ箱成功と報告しない。
成功分のpurgeもこの結果に合わせる。一般Deleteの成功規則は今回変更しない。
根拠: [Microsoft PostDeleteItem仕様](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperationprogresssink-postdeleteitem)。

### receipt 同定の費用とまれな競合 (P1-1への回答)

出力は既存の create-new／上書き禁止規則を維持し、worker が実際に作った全 path を記録する。
作成した handle からサイズ・完全精度の更新時刻・object identity を取得できる出力経路では
それを使う。既存 write_source／capture 保存が handle を閉じる経路は、書込直後に worker で
path + サイズ + 更新時刻を採取すれば足りる。object identity は全経路の必須条件にしない。
新しい hash 全件計算、全件 handle 保持 API、private staging、多段検証は作らない。

書込完了→receipt 確定、照合→Shellごみ箱移動の短い隙間に、外部プロセスが同名・同じ bytes／
同じ stamp の別物へ差し替える競合は、coordinator が「通常の使い方では起きない」と判断した。
**既定の割り切り方針による**。この瞬間の差し替えを防ぐ API／復旧機構は追加しない。
通常の後続編集／改名／外部追加・削除・上書きは、本構成と receipt stamp の照合で拒否する。
この区別を、すべての外部変更が構造的に排除されるという保証へ広げない。

## 3. 所有と受付・完了

単純化の検討: 追加全体を新たに modal にすると、現在許されている別の閲覧操作を制限する。
その変更は採らない。追加 worker は既存 `book_op_pending` に載せ、後続操作が起きたら
取り消し token を失効させる。取消対象の追跡・再開・移動先への追従はしない。
新しい Undo の検証／削除中だけ modal にして、追加・編集・移動・履歴・窓切替を入れない。
OS の外部変更は modal で排除できないので、§2 の stamp 照合で通常変更を検査する。

**処理中 owner は既存 `book_op_pending: Option<BookOpPending>` だけ**。
前版の `BookAppendUndoState`／別 receiver／Undoing／quiescing state を撤去する。
既存 `BookOpIntent` に `Append` (operation ID・source context・受付 revision・captured target folderを固定) と
`UndoAppend` (消費した receipt を保持する source mutation intent) を載せ、
`start_book_op` と `poll_book_op_pending` の同じ要求／終端へ配送する。
別の pending field や「Undo中」bool は追加しない。
直後一回の receipt は本操作の既存管理に一つだけ保持する値で、実行中 owner ではない。
後続 mutation の受付 revision は単調な比較値であり、新しい処理 state machine を作らない。

receipt は本棚 root／本 folder、全 created paths と各サイズ・更新時刻 (取れればobject identity)、
成功後の本全体の構成 stamp、source context、operation ID、受付 revision を持つ。
UI の後から変わった settings／active_book_name で出力名を作り直さない。
初期／事後 snapshot に今回の append で説明できない差分があれば追加は成功しても token は
作らない。元の append・edit／semantic copy・成功 UI の機能は維持する。

1. append の受理時に前 token を退役。既存 Undo/Redo の履歴は捨てない。
2. 追加中の後続 mutation／Undo boundary は受付 revision を進める。成功が遅れて来ても古い revision の
   token を復活させない。ファイル追加と通常成功 UI は既存どおり反映する。
3. metadata copy の受付／完了も同じ append の終端へ含め、成功した自分の内部 copy や同じ
   operation の一覧 refresh を「他操作」と誤分類しない。read-only一覧準備を一般の失効
   除外にせず、operation ID の同一性で限定する。
4. Ctrl+Z の新操作は `KeyAction` へ配送する。既存メタ Undo の固定 Ctrl+Z の入口も同じ
   Action (`UndoLastAction` 仮称) に集約し、有効receiptがあればこの一回、なければ既存メタ Undoへ。
   Redoと編集専用 Undo の優先順、IME／dialog gate は維持する。新しい raw `Key::Z` 判定は作らない。
5. Undo の受付は次節の既存本操作 gate で行い、`UndoAppend` pending を同期的に設定する。
   検証・ごみ箱移動・purge はその worker が一回の操作として行う。
6. worker は root／本folder／直下出力範囲 (reparse を辿らない) と、追加後の本全体の構成・
   各 receipt の stamp を一度照合する。全対象が一致してから Shell recycle を開始する。
   二件目が不一致なら一件目も Shell へ渡さない。handle を保持する検証 API は作らない。
   Shell 実行に渡すのは receipt の exact paths だけ、folder丸ごと／glob／番号範囲削除はしない。
7. worker terminal は実際に消した exact paths を返す。既存 metadata purge／presence／
   read cache／book count 更新の成功 path 境界を再利用する。全部終端後 receipt は消費済み、Redoなし。
   UI thread に read_dir、hash、delete、DB全照会を入れない。

普通の外部変更・後続操作は失効／stamp 照合で拒否し、§2 の瞬間的な差し替えだけは割り切る。
書込・DB・削除のまれな失敗は割り切り (ログ + 通知 [+ 作り直し])。自動 rollback／retry／
partial resume は追加しない。削除途中のまれな失敗時も成功 path だけを反映し、残りを通知する。
失敗を理由にユーザーの本構成・しおり・設定を作り直さない。

### 3.1 既存の本操作／source mutation の受付境界 (P1-2)

`UndoAppend` は `BookOpIntent::blocks_rename_migration()` の source mutation に含める。
この既存移行保護と、新規の通常操作Busyは別判定にする。新Busy／modal／Remote write拒否は
`matches!(pending.intent, BookOpIntent::UndoAppend { .. })` の場合だけ。
既存 `SourceMutation`／本削除／改名等がpendingでも、新helperを理由にタグ／★／管理画面closeを
拒否しない。それらはreceiptを失効させ、従来の各受付条件で実行する。
受付条件は一箇所に揃え、対象viewerの既存解放→最終busy検査→receipt 消費→
`start_book_op` の pending 設定を同じ
UI呼出しで行う。metadata producer、Remote、context変更はこの既存 pending の intent を
参照する。dialog が見えているかだけを排他条件にしない。

| 境界／段階 | 契約 |
| --- | --- |
| Undo受付前 | 別 book op、delete、book_reorder の flush／transfer、migration in-flight、cleanup、Remote Active／Acquiring があれば受け付けない。receipt がまだ有効であれば「処理中」と通知し、その入力で古い meta Undo を実行しない |
| 既に受理済みの書込み | `rename_migration_writers_busy()` の tag未回収 batch、rating、local_adjust未完了／未回収、edit_preview、spread_shift、book_resume、bookmark pending に加え、Remote pending書込み完了を確認。対象viewerの解放がflushを発行した場合も、その後に再検査する。未解放surface／未完了writerがあれば Undoを受け付けず、workerを開始しない。新しい fence／drain待ち state を作らない。自分の append に必要な copy 終端が終わるまで成功 receipt を公開しない |
| Undo worker開始 | `book_op_pending` を設置した後、worker で既存 `acquire_delete_epub_guard` を exact generated paths に取得。既存 delete-worker と同じ EPUB→shift maintenance の順序。検証より前に取得し、全 Shell chunk と成功分 purge の終端まで保持 |
| ローカル後続mutation | §3.2 の共通受付で UndoAppend pending だけを新規Busy判定。Undo中は受理・worker投入を行わない。Ctrl+Z連打は新しい操作を開始しない |
| Remote acquire | 現行 `LocalAiRemoteBarrierSnapshot.book_op_pending` (app.rs:10243) が Undo を含む。pending が残る間は引継ぎを完了しない。Undo前に Acquiring ならローカルUndoを受け付けない |
| Remote write | `apply_pending_remote_write` (remote_ipc/ui.rs:2413) の bookmark early branch／`apply_remote_write` より前にUndoAppend限定gateを適用。Undo中は既存 Busy 応答で終え、書込みを後で再生する新queueは作らない。受理済みRemoteジョブは前段のbusy検査で排除。他の本intentは既存Remote受付規則を維持 |
| 汎用metadata移行 | `rename_migration_source_changes_pending` (app.rs:45960) が Undo intent を見る。queued移行は開始しない。既に走った移行は受付前busyで排除。token失効をworker完了時へ遅らせない |
| context close／A/B／navigation | Undo pending の間は新しい切替／close要求を受理しない (既存modal受付で理由通知、専用延期stateなし)。対象生成物のreadonly解放だけは受理前の既存 removed-path 経路で行い、同一operation IDに限定する。既に編集draftがあるならtoken失効済みであり、解放による保存を自分のappendの除外にしない。元contextが消えても新contextへreceiptを移さない |
| 終端 | Shell成功分だけ guarded purge→worker result→UIの成功path更新まで pending を保つ。`poll_book_op_pending` は受信直後に take して受付を開けず、UI終端後に一度退役。Remote引継ぎ／次移行はその後 |
| 正常終了／強制終了 | 既存本操作の終了判定に同じ pending として含める。別の永続Undoや終了時resumeを作らない。途中でプロセスが失われる場合は既定のまれな失敗方針 |

maintenance lease は **既存受付済みmetadata workerの完了確認の代用ではない**。
leaseだけ取って後から旧queued書込みが削除pathへ着地する形は不可。
一方、busyなwriterをUIで同期wait／sleepする形も不可。今回の短いUndo操作だけを既存境界で排他にする。

### 3.2 失効 producer inventory (P2、実装前の確定範囲)

失効は操作の**受付成功から最初のdraft変更／job投入までの間**に行う。
`push_*_undo_entry` や非同期 completion だけでは遅い。受付共通 helper は
「UndoAppendならBusy、それ以外の後続操作ならreceipt失効／受付revision更新」を
行う。空target／入力拒否等の非操作はrevisionを進めない。以下を実装・回帰の対象とする。

| producer群 | コード上の入口／捕捉位置 |
| --- | --- |
| タグ | `tag_ops.rs::request_tag_toggle_for_targets_impl`、set/add/remove/clear経路から共通 `submit_tag_jobs`。accepted targetsが確定して最初の投入より前。Undo確定／poll結果ではない |
| ★／解除 | `app.rs::write_user_rating_shared_deferred_from`、`write_user_ratings_shared_deferred_from`、container rating write を共通受付へ。grid/viewer／Remote／Undo由来を同じ境界で扱う |
| 色補正／標準／ローカル調整 | `set_page_params_for_target`／`set_page_params`、`set_local_adjust_layers_for_idx`、`ui_adjustment_panel.rs` の標準値変更、shape key／drag／brushの最初のdraft mutation。prepare-only hydrationと実editを区別 |
| 消しゴム／隠蔽／注釈／crop／rotation | mask／conceal／comic／crop等の最初のbrush・shape・text draft変更と、draftを経由しない確定入口 (`save_mask_with_sidecar`、`save_conceal_with_sidecar`、`save_comic_objects` 等)。保存helperだけに寄せない。readonly描画／decodeは除外 |
| 本しおり／タイトル | `add_current_book_bookmark` (:47419)、remove／set_title、browser削除、Remoteのbookmark branch。service投入前。`clear_book_bookmark_title_edit` はhydration／破棄だけなら非mutation |
| 見開き／トリム／読書設定 | `app/spread_shift_owner.rs::set_live_spread_shift`／reset、fullscreenのspread／page-alone／final-cover設定、view trim永続化。setter／service投入前。通常read-only読書位置記録は後続ユーザー編集として失効させないが、未完了書込みは§3.1のbusyに含める |
| 本の操作 | `start_book_append_to_named`／create／rename／delete／transfer、`ui_main.rs::move_selected_book_reorder_group` 等の最初のreorder draft変更。flush受付より前。管理一覧のreadonly refreshは一般除外せず、source一覧を置換するものは下記境界へ |
| 実ファイル操作 | `start_delete_files`、rename dialog submit、file_organize submit、Shell paste・D&D copy の受付、native Shell操作へ制御を渡す時点。結果pollより前。ZIP/PDF内の拒否入力は操作でない |
| メタ Undo／Redo・編集専用 Undo | 通常のUndo/Redoが受理された時点。book receiptを消費するこの一回のUndoは別のmeta entryをpopしない |
| 一覧／context 終了 | `clear_meta_undo`／そのrating move版、主一覧採用／検索置換、quick-folder切替、viewer真のclose、A/B交換／context破棄の共通受付。後から古いappend完了が届いてもrevision/context不一致でreceiptを公開しない |
| Remote | acquire受付でsource contextのtoken失効、上記Remote write共通入口で受付revision更新。bookmark early branch、spread／rating／tag、Remote切断・context退役も含む。既存session ownershipを維持 |
| metadata import／復元・設定 | metadata transferのwrite受付、content-identity復元の採用、本root変更／対象書込みに影響する設定変更。表示を読むだけのworkerは失効しない |
| 自分のcopy／refresh | `Append` intentに捕捉した同一operation IDから配送されたedit/semantic copyと一覧refreshだけを除外。source path一致／時間窓／「本処理中」だけでは除外しない。異なるaccepted mutationは必ず失効 |

同じreceiptに対する失効は冪等。producerごとに別pending／復活用snapshotを置かない。
自分の刷新で一覧世代が変わっても同一operationの採用として扱い、後続の別更新は失効させる。

## 4. 並べ替え・membership・実装範囲

Undo 対象は末尾追加だけ。番号の詰め直しは行わない。既存ページ名／順序は unchanged。
並べ替え dialog の draft を変更する時点で token を失効 (flush 完了まで待たない)。
並べ替え中には既存 dialog 入力 ownership を優先し、本追加 Undo を実行しない。

membership が別工程で導入された場合は、同じ receipt の `created paths` に対応する登録行だけ
削除し、その worker 完了後にバッジを更新する。source 全体／本全体の provenance を消さない。
現基点には該当 DB がないため、今回の実装に membership schema やバッジは追加しない。
この前提差を coordinator が採否判断する。

予定実装: `books.rs` (出力receipt／既存intent・result)、`delete_worker.rs` (ごみ箱必須helper／
既存guarded purge共通経路)、
`app.rs` (book operation intent / terminal)、`undo_ops.rs`、`keymap.rs`、
`ui_main.rs` / `ui_fullscreen.rs`／`ui_adjustment_panel.rs` のmutation入口、`tag_ops.rs`、
`app/spread_shift_owner.rs`、`remote_ipc/ui.rs`、file operation dialog受付、
context採用／終了の既存所有箇所。`book_fs_journal.rs` の全件handle新APIは対象外。
`undo_stack.rs` の metadata entry にファイル削除は追加しない。
文書は compile-book、keymap spec／plan、architecture、必要なら virtual-folders を同時更新。
§3.2 のinventoryを確定範囲とし、実装中に別の直接書込み入口が判明したら同じ受付へ接続し、
根拠と追加範囲をレビューへ戻す。completionだけのtoken失効で代用しない。

## 5. 回帰・受入

予定 (未実行): 単枚／複数／Stack／ZIP／PDF／合成の一回 undo、元と既存ページが残ること、
二度目の Ctrl+Z と Redo、古い metadata 履歴、後続タグ／編集／別追加／並べ替え draft／
navigation／context切替、遅延成功結果、同じ operation の refresh、外部追加／同名再作成／
内容上書き／本改名／reparse、二件目のstamp不一致ならShell呼出し0件、途中write失敗、purge成功path。
ごみ箱非対応なら完全削除呼出し0件、receiptにないpathへの削除要求0件。
PostDeleteItemの成功＋ごみ箱item／失敗／NULL／未callbackをfake sinkで検査し、pathが消えた
だけのケースを成功にしない。実ごみ箱確認は別の了承済み検証枠で行い、今回アプリは起動しない。
UndoAppend中はタグ／★／Remote書込みが拒否され、既存の本改名・削除・SourceMutation中は
管理画面close後の別画像タグ／★が従来どおり受理される対のhandler回帰を追加する。
§3.2 のproducerを各群最低一件、受付直後・非同期完了前に失効することを検査。
Remote acquire先着／Undo先着、queued Remote bookmark／spread書込み、既存tag/rating/
local-adjust/bookmark等の未完了writer、queued／in-flight migration、context close／A/B、
worker result受信→UI purge反映の間も同じbook pendingで受付が閉じることを検査。
対象viewer解放が書込みを発行した場合の受付拒否も検査する。
瞬間的な同bytes別object差し替えを必須試験／新APIの動機にしない (**既定の割り切り方針による**)。
membership は統合された場合に専用回帰を追加。

検証所有者: root実装担当が集約。targeted、fmt、通常／portable core check、項目末尾 full gate 一回。
§1.373の試作にcargo／fmt／build検証は行っていない。アプリ起動・コミットなし。
下記R4の独立設計レビュー前に製品への接続・実ごみ箱操作の検証を進めない。

## 6. 退役した実装試作 (2026-10-12、製品未接続・未検証)

- `BookAppendSummary.undo_files` はworkerで実際に書いた全pathと完全精度のmtime／size、
  前後の直下構成照合から作る。説明できない外部差分や採取失敗は追加成功を保ちreceiptだけ作らない。
- 追加のファイル書込み結果は`AppendFilesWritten`として同じ`book_op_pending`へ戻す。
  pollの同UI呼出しで既存local-adjust FIFO fenceを発行し、そのreceiverをmetadata copy workerへ
  渡す。既存の成功受信時点に受理済みの書込みcutを維持し、UIでwaitしない。
  receiverの交代中も同intent／operation ID／context／revisionを保持する。
  `copy_append_metadata_at`が既存8storeのfull-edit／semantic copyを完了してから
  `Append`終端を返し、UIは返されたpresence／cacheだけを更新する。
  copy失敗は生成ページを残して通知し、receiptを公開しない。
- `book_append_undo` はreceiptとworker検証・ごみ箱移動・成功pathだけのguarded purgeを所有。
  root／本folderと祖先のreparseを拒否し、全対象の照合が終わってからShellへexact pathsを渡す。
  新hash計算・handle保持・rollback・retry・部分resumeは追加しない。
- `delete_worker::recycle_required` は既存COM chunk helperのごみ箱必須policyを使う。
  `FOFX_RECYCLEONDELETE`／`FOF_NOERRORUI`／`FOFX_EARLYFAILURE`で要求し、
  per-item sinkは完全削除の`PreDeleteItem`を拒否する。各exact sourceの`PostDeleteItem`
  成功かつnon-null recycle itemだけを成功分へ含める。path消失、全体HRESULT、callback欠落を
  ごみ箱成功の代わりにしない。一般Deleteのflags／成功規則は従来どおり。
- `KeyAction::UndoLastAction` (Global／Press、既定Ctrl+Z) がappend receiptとメタUndoの
  共通入口。ファイル操作をmetadata `UndoStack`へ積まず、追加のRedoはない。
- worker／pure回帰は出力範囲、全件検証先行、外部構成変更、初期／事後差分、空receipt、
  部分成功purge、callback成功／失敗／NULL／欠落を対象にする。受付とcontextの回帰はApp側で集約。

## 7. R4: 追加中の対象本への自動観測をAppend終端へ配送 (2026-10-12利用者決定、2026-10-13改訂)

### コードから判明した通常経路

`App::check_external_folder_changes`は、現在の実folderのmtime、またはwatcherのNotifiedを
受けて`start_external_rescan`を呼ぶ。既存の抑止は一般Deleteの`delete_pending`であり、
追加の`book_op_pending`は抑止に含まれない。既に受理済みのscan完了も
`poll_external_rescan`のowner/context/items-generation照合を通り、
`apply_external_rescan`が内容signatureの変更を見て既存のreload／fullscreen後reloadへ配送する。
なお本棚配下は `ensure_current_folder_watch` → `current_favorite_target` では購読対象外だが、
復帰契機の `check_external_folder_changes(Resumed)` はその判定を使わない。
本フォルダに自動観測がないという前提にはできない。今回その購読範囲を広げる変更はしない。

そのため、**現在の本へclipboard／capture等を追加する通常操作**では、今回の書込み自身が
folder変更を通知し、`AppendFilesWritten`受信→metadata copy worker→`Append`終端より先に
一般rescan／source一覧置換が受理・採用され得る。R3 §3.2では一般source置換を受付時に
receipt revisionの失効境界としている。このまま接続すると、通常の追加なのに自分の通知で
受付revisionが進み、成功receiptを公開できない、または直後に失効する経路が残る。
これは実行観測ではなく、上記のsource inspectionから判明した順序である。

単に「本操作pending中」「同じpath」「短い時間窓」を理由に一般rescanを自分の更新とする
方法は採らない。同時に外部で追加／削除／改名／上書きされた変更を自分のappendと誤分類する。
callback／検証を変更して末尾のCtrl+Zだけを通す症状パッチもしない。

### 比較した候補と利用者決定

1. **captured target folderを既存`Append` intentへ追加し、同folderへの自動観測をappend終端へ
   配送する案**。要求入口と先着済みscanの採用入口の両方で、exact folderとoperation IDを
   照合する。metadata copy終端でreceiptの全構成stampも照合し、説明できない外部差分は
   tokenを作らず一般refreshへ返す。専用pending ownerは追加しない。
   **案1を採用 (利用者2026-10-12)**。対象folderの外部変更表示が追加中の数秒だけ遅れる費用も
   了承済み。これは自動観測の配送変更であり、R3の「新規排他はUndoAppendだけ」を維持する。
2. **観測を延期せず、既存のsource観測ownerがappendの自己差分を証明する案**。
   初期の全物理構成と、各scanのreceipt差分を同operationへ相関させる必要がある。
   §1.375の案Bは具体化・独立レビュー前で未接続であり、その基盤を暗黙の前提にしない。
   案2は採用しない。
3. 追加全体のmodal化は§3で検討して退けた案。通常の閲覧・移動を制限するため、
   今回の実装担当だけで変更しない。

### 7.1 captured targetと観測の所有

`start_book_append_to_named`の受付時に既存の本名／root検証規則で固定したexact folderを、
既存 `BookOpIntent::Append` のoperation ID・source context・受付revisionと一緒に保持する。
workerも同じ固定targetへ書く。後のsettings／active_book_nameからtargetを再計算せず、
workerの結果folderがcaptureと異なればreceiptを作らない。reparse／root内直下の検査はworkerで行う。
pendingの設置は最初の書込みより前であり、最初の通知がFilesWrittenより先でも所有先がある。

Append intentはそのoperationへ配送した自動観測も保持する。値は観測元context、exact folder、
要求契機／通知evidence、cutoff／sequence (購読がある場合)、先着scanのidentityをまとめた
boundedな記録とし、別のreceiver／App pending／「append監視停止」boolを足さない。
記録の上限超過・coverage不明は全構成照合が必要という値へまとめる。通知を自己由来と断定する
台帳ではない。§1.375の新購読ownerが未実装でも、既存rescanのowner/context/generationを使って
同じ配送境界を実装できる設計とする。

### 7.2 要求入口と採用入口

| 入口 | 照合と配送 |
| --- | --- |
| 自動check／rescan要求 | 既存source判定後、mtimeの復帰ふるい／新scan起動より前に、処理中Appendのcaptured folderとのexact一致を確認する。一致した要求だけをそのAppend IDへ配送して記録する。OS通知に元からIDがあったとは扱わない。Resumedでも最終全構成照合は行うため、親mtime不変の内容上書きをふるいだけで除外しない |
| 既に受理済みrescanの完了 | 元のcontext／generation／scan optionsの有効性を確認し、signature更新・一覧置換・receipt失効・fullscreen後refresh予約より前に、現在のAppendのexact folderとIDへ照合する。一致すれば「このAppend終端で照合する観測」として引き渡し、古いscan payloadを終端snapshotとして採用しない。再走査予約も同じintentへ引き渡す |
| 異なるfolder／失効したcontext／異なるAppend ID | 自分のappendの更新とは扱わない。既存の通常採用／stale拒否規則へ戻す。source contextが違う観測はその元ownerを保持し、別contextへ結果やackを移さない |
| 明示GridReload、navigation、設定変更、他のmutation | 自動観測延期の対象外。R3 inventoryどおり受付revisionを進め、通常操作を受理する。処理中Appendのreceiptは復活させない |

先着scanはAppend以前に受理されているので、元の要求にAppend IDがあることを要求しない。
**採用入口で現pendingのexact targetへ照合して引渡し先IDを固定する**。引渡し済みの完了／
予約が別のAppendへ遅延して届いた場合、その古いIDでは一致しない。同pathを根拠に付け替えない。
同じ本を表示する別contextの観測も、それぞれのowner identityを保持したまま終端へ集める。
兄弟のdirtyは一つのcontextの採用でackせず、終端後に各ownerへ最新の通常観測を返す。

### 7.3 metadata copy終端の全構成照合

ファイル書込み → `AppendFilesWritten` → metadata copy → 最終構成照合の間は、
**同じ `book_op_pending` のAppend intent／operation IDを保持する**。pollでreceiverを交代するとき
pendingを一旦消して自動採用の入口を開けない。全構成の列挙・stamp取得・DB copyはworkerで行う。

1. workerは既存の本フォルダ作成・安全検査後、最初のページ書込み前に全構成snapshotを採る。
   新規作成した空の本も対象にできるが、Undoで本フォルダ自体は消さない。
   表示filter前の全直下entry (隠しfile、
   非表示形式、sidecarも含む) のexact name／種別／size／完全精度mtimeを記録する。
   本のページとして扱わないsubdirectoryも除外せず存在・stampを比較するが、reparseを辿る
   再帰探索はしない。取り消す範囲は今回作った直下fileだけであり、既存subdirectoryの中を消さない。
2. 実際にcreate-newで作った各pathのreceipt stampを記録し、metadata copyもそのIDで完了させる。
   own copyが物理sidecarを作成／変更する場合は、そのexact effectを同じworker結果へ含める。
   既存fileの不明差分を「metadata copyのはず」と推測しない。DB内のcopyは物理構成差分に混ぜない。
3. metadata copy終端で全直下構成を新しく採り、`before + exact created files + exact own effects`
   と比較する。created files自身のstampも再照合する。追加／削除／改名／同秒・同sizeの上書き、
   種別変更など説明できない差分が一件でもあればtokenを作らない。途中scanの一致や親mtimeで
   代用しない。snapshot取得不成立もtokenなし・ログと通知とする。
4. 一致し、source context／受付revisionも受理時のままなら成功receipt候補を返す。
   後続タグ／★／draft／navigation等が受理されていたら、物理構成が一致してもtokenを公開しない。
   追加成功の件数・metadata反映・通常の成功通知は維持する。

通常の外部差分はここで検出する。既存subdirectory内を含む後続ユーザー操作の受付はR3の失効
境界が担当し、Undo時もreceiptの全構成・出力stampを再検査する。瞬間的な同bytes／同stampの
差替えを排除する新handle保持・hash・stagingは作らない (**既定の割り切り方針による**)。

### 7.4 終端refresh・receipt公開・解放

説明できる自己差分だけなら、追加成功の既存refreshへ同じAppend ID・exact folder・ownerを
載せる。`pending_reload`のboolや `folder_refresh_pending: PathBuf` だけでは自己操作の印に
ならないため、既存refresh要求のtyped payloadを拡張し、要求からprepared result・採用までIDを
運ぶ。一般refreshは別causeのままにし、path一致だけで自己操作の失効除外へ入れない。

現在の対象一覧へ即時採用できる場合は、freshな終端scan/prepareをそのownerで採用し、
signature／観測stampを更新してからreceiptを公開・Appendを退役する。採用直前にもAppend ID、
source context、受付revisionを確認する。このprepared resultの全構成stampも§7.3の説明済み
構成と照合し、metadata終端snapshot後の外部差分を自己refreshに取り込んでtokenを作らない。
一致しなければ一般refreshへ戻し、tokenなしとする。最終scan開始のcutoffより後の観測は未照合として
元ownerへ戻す。行が不変なら既存の無変更採用を使い、余分な一覧installを作らない。

fullscreen等で一覧採用を延期する場合は、既存のdeferred refresh要求へIDとownerを引き渡して
Appendを終える。画面を強制的に閉じたり、閲覧終了までBookOpPendingを保持したりしない。
後のrefreshはその時点のfresh scanを使い、receipt候補の全構成stampと同じかをworkerで確認する。
同IDの説明済みrefreshだけを失効除外にし、不明差分ならreceiptを失効・通常更新へ戻す。
deferred予約だけを理由にCtrl+Zの新しいBusy条件は追加しない。receiptが有効ならR3の受付と
worker検証を使い、Undoに伴う既存readonly解放・refresh取消／世代失効を同じownerで処理する。
採用workerが既に走っている場合は§3.1の既存busy判定を使う。古いAppend refreshの後着は
消費済みreceiptやUndo後の世代へ採用せず、取り消したページを旧scanで表示へ戻さない。
本配下では `consume_folder_refresh_pending` の `current_favorite_target` が拒否するため、
Append causeの要求はお気に入り可否ではなくcaptured physical sourceの既存refresh配送を使う。
detachedの述語／viewportを変更する設計にはせず、既存閲覧後配送のownerと条件を維持する。

説明できない外部差分はreceipt候補を破棄し、記録した観測を元ownerの一般refreshへ返す。
latest内容を一回準備して採用し、external causeをAppend IDの自己除外へ偽装しない。
別folderへ移動／park／context破棄済みなら強制的に戻さず、元ownerの通常dirty／drop規則を使う。
context境界で失効したreceiptを新contextへ移さない。

Append終端後は自動観測の延期を終了する。遅延して届いた通知を旧IDに結び付けたり、時間窓で
捨てたりしない。既存の無変更比較なら一覧置換が起きずtokenも失効させないが、差分を採用する
通常source置換はR3どおり失効する。Undo時の全構成照合も省略しない。
metadata copy失敗・部分write・worker切断はtokenなし、生成済みfileは残しログと通知。
延期した観測は一般refreshへ解放し、失敗を理由にdirtyを消したりrollbackを始めたりしない。

### 7.5 受入回帰と独立レビュー停止点

追加予定のhandler回帰 (未実行):

- 現在の本への単枚／clipboard／capture追加。自動観測がFilesWrittenより先、metadata中、
  正常終端後の三順序。自己差分だけなら一回Undoが可能で、余分なsource失効をしない。
- Appendより先に始まったscanが書込み中に完了。payloadを採用せず同ID終端へ配送し、
  restart予約も失わない。旧Appendの引渡し結果を次の同folder Appendへ採用しない。
- 外部追加／削除／改名／同秒・同size上書きとown appendが混在。親mtime一致でも最終構成で
  拒否し、追加成功を保持、token0件・一般refresh1回。旧metadata Undoを同入力で実行しない。
- FilesWritten後・metadata終端前・refresh採用前の後続タグ／★／draft／navigationでreceipt復活0件。
  他folderの自動更新と通常閲覧を拒否しない。新BusyはUndoAppendだけという対の試験を維持。
- fullscreen後refresh、対象本の別context／park／drop、captured本名と後のactive_book_name差異。
  owner・IDが一致した自己refreshだけ除外し、別contextのdirtyを消さない。
- cutoff後の通知、copy失敗・scan不成立・worker切断で観測を一般経路へ返す。permanent delete0件、
  PostDeleteItemの実移動結果による成功pathだけのpurgeというR3回帰も維持。

**ここで止め、R3からの所有・配送差分を独立設計レビューへ提出する。再承認の利用者質問は不要。**
今回追加・実行テスト0件、build0件、アプリ起動0回、コミット0件。退役した試作は再適用していない。

### 退役記録と再開範囲

設計判断前に試作のApp fields／Undo入口／producer gate／typed refresh接続を撤回した。
現製品は既存の受付規則を維持する。独立レビュー後、R4に沿って同じ既存ownerへ接続し直す。
退役記録は`target/section1373-integration-prototype.patch`と
`target/section1373-prototype/`の実ファイルcopy。これは未検証の途中差分であり、
製品仕様の正本・適用済み変更・検証証跡とは扱わない。

案の採否と費用は上記の利用者決定で確定済み。独立レビューでは§7.1–7.5の入口、先着scan、
終端照合、deferred refreshへのID引渡しを確認する。退役差分をそのまま製品へ戻さない。

退役前の worker／receipt／Shell 部分は独立コードレビューで重要所見なし。App接続は
未完成・未検証であり、この部分レビューを製品の§1.373全体 ACCEPTとは扱わない。
瞬間的な同stamp差替えは既定の割り切り方針による。今回の製品差分には新Undo入口を残さない。
