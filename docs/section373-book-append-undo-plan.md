# §1.373 本棚への追加直後の一回取り消し

2026-10-13改訂、`next-list` / `ea060b66e` 設計基点、確認基点 `df20dba7b`。
**R7（2026-10-13設計担当決定）: 八store初期化を各画像の作成前へ移す。同じAppend workerの順序で公開境界を作り、stagingは追加しない。§7.10のfullは13,493成功でPASS。全実装のLISTrev8はREVISE（P2受付漏れ一件／P3冒頭の状態記載一件）。§7.11で対応実装・検証を完了し、今回のfullは一回完走、13,503成功／0失敗／63 ignoredでPASS。差分の独立再レビューは未完了**。
正本項目: [next-release-backlog.md §1.373](next-release-backlog.md#1373-本棚ツールバーの右クリックで本棚へ追加した操作を-ctrlz-で取り消す--利用者の要望-2026-10-11)。
アプリは起動していない。R4判断前の未検証試作は製品への接続を撤回し、`target`へ退役した。
R4の自動観測配送・ごみ箱Undo・UndoAppend限定排他は維持する。WIP `3e8178b31` の
通常の後続編集上書きは§7.7のR7で修正し、独立構造レビューの合意を得て実装を再開した。
§7.10までと、LISTrev8追加修正の§7.11の自動検証は完了。差分の独立再レビューと実機確認は未完了。
下記の試作退役・停止checkpointと中断記録は当時の履歴であり、現在のfull未成功を意味しない。

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
   Action (`KeyAction::UndoLastAction`、Global／Press、既定Ctrl+Z) に集約し、有効receiptがあれば
   この一回、なければ既存メタ Undoへ。消しゴム等の編集専用Undoが有効な面ではそちらを優先する。
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

### 3.2 失効 producer inventory (LISTrev8の受付監査を反映)

失効は操作の**受付成功から最初のdraft変更／job投入までの間**に行う。
`push_*_undo_entry` や非同期 completion だけでは遅い。受付共通 helper は
「UndoAppendならBusy、それ以外の後続操作ならreceipt失効／受付revision更新」を
行う。空target／stale index／DB不存在等、受付前の入力拒否はrevisionを進めない。
有効targetをDB操作へ受け付けた後の重複skip・書込み失敗は操作の結果であり、receiptを復活させない。
重複判定のためにUIの追加SELECTを行わず、既存のDB内判定を維持する。以下を実装・回帰の対象とする。

| producer群 | コード上の入口／捕捉位置 |
| --- | --- |
| タグ | `tag_ops.rs::request_tag_toggle_for_targets_impl`、set/add/remove/clear経路から共通 `submit_tag_jobs`。accepted targetsが確定して最初の投入より前。Undo確定／poll結果ではない |
| ★／解除 | `app.rs::write_user_rating_shared_deferred_from`、`write_user_ratings_shared_deferred_from`、container rating write を共通受付へ。grid/viewer／Remote／Undo由来を同じ境界で扱う |
| 色補正／標準／ローカル調整 | `set_page_params_for_target`／`set_page_params`、`set_local_adjust_layers_for_idx`、`ui_adjustment_panel.rs` の標準値変更、shape key／drag／brushの最初のdraft mutation。prepare-only hydrationと実editを区別 |
| 消しゴム／隠蔽／注釈／crop／rotation | mask／conceal／comic／crop等の最初のbrush・shape・text draft変更と、draftを経由しない確定入口 (`save_mask_with_sidecar`、`save_conceal_with_sidecar`、`save_comic_objects` 等)。保存helperだけに寄せない。readonly描画／decodeは除外 |
| 本しおり／タイトル | `add_current_book_bookmark` (:47419)、remove／set_title、browser削除、Remoteのbookmark branch。service投入前。`clear_book_bookmark_title_edit` はhydration／破棄だけなら非mutation |
| 代表ピン設定／解除 | 共通 `set_folder_thumb_pin`／`remove_folder_thumb_pin` → `media_metadata_mutation::user_folder_thumb_pin_db` → 既存受付helper。通常Folder／ZIP・nested ZIP・PDF／変換書庫、P／📌／context menu／ringはこの書込み境界に合流。ZIPのdirty消費や後続reloadに失効を依存しない |
| 動画／音声しおり | 共通 `user_video_bookmark_db` → 既存受付helper。nativeの `add_native_video_bookmark`、`handle_native_video_{delete_bookmark,set_bookmark_title,bulk_add_bookmarks,clear_all_bookmarks}_command`、eguiの `handle_video_input`、musicの `add_music_bookmark_at`／delete／rename／bulk_add／clear_allを接続。musicには動画音声モードも含む |
| 動画ピン設定／解除 | 共通 `user_video_pin_db` → 既存受付helper。`set_native_video_pin_with_webp` に通常HUD／選択tile／P／ring／menuが合流、解除は `handle_native_video_delete_pin_command`。target／DB確認後、書込み・dirty／pending更新前に受付 |
| 横断しおり一覧削除 | `delete_bookmark_browser_rows` でnonempty／既存pending検査後、共通受付→`bookmark_browser::spawn_delete`。選択／check／context menuのBook・Video・Audioを同じworker入口で覆う |
| 見開き／トリム／読書設定 | `app/spread_shift_owner.rs::set_live_spread_shift`／reset、fullscreenのspread／page-alone／final-cover設定、view trim永続化。setter／service投入前。通常read-only読書位置記録は後続ユーザー編集として失効させないが、未完了書込みは§3.1のbusyに含める |
| 本の操作 | `start_book_append_to_named`／create／rename／delete／transfer、`ui_main.rs::move_selected_book_reorder_group` 等の最初のreorder draft変更。flush受付より前。管理一覧のreadonly refreshは一般除外せず、source一覧を置換するものは下記境界へ |
| 実ファイル操作 | `start_delete_files`、rename dialog submit、file_organize submit、Shell paste・D&D copy の受付、native Shell操作へ制御を渡す時点。結果pollより前。ZIP/PDF内の拒否入力は操作でない |
| マスクのスロット保存 | `save_mask_to_slot`／`save_conceal_mask_to_slot` のmask／DB確認後、既存受付→`set_slot`。既存のslot読込・適用の受付も維持 |
| メタ Undo／Redo・編集専用 Undo | 通常のUndo/Redoが受理された時点。book receiptを消費するこの一回のUndoは別のmeta entryをpopしない |
| 一覧／context 終了 | `clear_meta_undo`／そのrating move版、主一覧採用／検索置換、quick-folder切替、viewer真のclose、A/B交換／context破棄の共通受付。後から古いappend完了が届いてもrevision/context不一致でreceiptを公開しない |
| Remote | acquire受付でsource contextのtoken失効、上記Remote write共通入口で受付revision更新。bookmark early branch、spread／rating／tag、Remote切断・context退役も含む。既存session ownershipを維持 |
| metadata import／復元・設定 | metadata transferのwrite受付、`start_content_identity_restore` のnonempty／pending検査後のworker投入前、本root変更／対象書込みに影響する設定変更。復元promptのUndo中の拒否はtake／設定保存前、dont-ask設定変更も既存受付へ。明示settings Restore／FullResetは `execute_pending_settings_restore` のpending破棄／worker投入前。表示を読むだけのworkerは失効しない |
| 明示リセット／履歴削除 | `reset_saved_rotations`、`remove_reading_history_entry_for_idx`／preferencesの全履歴削除、`clear_book_resume_async`。通常の位置recordは除外したまま、利用者の明示clearだけを受付へ。favorite view stateのremove／reset／clearも最初のmap・queue変更前。favorite削除は後段のdefault清掃に頼らずsettingsからの除去前に受付 |
| 本構成draft／保存・解除（master統合） | 有効なdraft編集・専用Undo／Redoの受理で既存受付を通す。保存／解除はRemoteのlocal admission内で、既存保存ticket登録／workerへの開始通知より前に受付。選択・preview・取消・無変更は除外。送信済ticketは既存writer-busy分類へ登録し、owner退役後も終端まで保持 |
| 構成済み本のlocal表示モード／読み方（merge review修正） | `persist_fullscreen_view_mode`／`persist_fullscreen_reading_flow`でscope／service確認後、既存受付→service enqueue。同値は既存の`apply_fullscreen_view_mode`／`set_reading_flow_for_fullscreen`で除外。receipt保持中は失効、Append中は遅着receiptの復活を拒否、UndoAppend中はenqueue・表示変更を拒否。新pending／ownerは追加しない |
| 画像folder→ZIP（master統合） | 有効manifestの受理後、既存ZIP worker入口のtrim移譲・portal／metadata投入前に受付。確認画面前にも補完importが走るためConvertボタンまで遅らせない。manifest探索／案内だけはreadonlyであり失効しない |
| 自分のcopy／refresh | `Append` intentに捕捉した同一operation IDから配送されたedit/semantic copyと一覧refreshだけを除外。source path一致／時間窓／「本処理中」だけでは除外しない。異なるaccepted mutationは必ず失効 |

master統合（2026-10-14）ではmetadata転送の`WaitingForWriters(Preview)`は従来のLoadingPreviewと
同じreadonlyとしてUndoのwriter-busyへ含めず、Transfer待ち以降の既存範囲を維持する。
Remoteの未回収reading writeはShiftClear／ViewSetting／ViewTrimを数え、resume readは除外する。

同じreceiptに対する失効は冪等。producerごとに別pending／復活用snapshotを置かない。
自分の刷新で一覧世代が変わっても同一operationの採用として扱い、後続の別更新は失効させる。

3 storeの共通入口は既存App-owned DBを借用して返すだけで、別の受付state／queue／ownerは持たない。
`admit_book_append_followup_mutation` がrevisionとUndoAppend限定Busyを一箇所で決める。
キー／HUD／menuごとの独立した失効印や、完了時の二重失効による症状修正は採らない。

機械的監査: `python scripts/check-book-append-mutation-admission.py`。
producer関数と最初の書込み／投入位置を列挙し、受付が先にあることと、3 storeの新しい直接書込みを検出する。
構成serviceの表示設定／trimの固有methodについてもproduction callerを全列挙し、未分類入口を失敗にする。
local mode／flow、book trim／page trim保存・解除、Remote mode／trimは共通受付を確認する。
構成draft／保存・解除は既存の専用受付・最初のticket投入の監査を維持する。
既存の`apply_view_trim_for_key_with_fallback`は旧MarginFitの自動移行、`begin_book_projection`は
受理済みtrimの採用前FIFO flushとして分類し、読取／採用で別のユーザーmutationを受付しない。
今回のruntime修正は2保存入口への接続だけに限定し、next-bookの組の作り直し・投影ownerには触れない。
ZIP／PDFの実handlerで同値／変更とreceipt・Append・UndoAppend・既存SourceMutation／Deleteを対照し、
worker停止中の受付revision、FIFO終端後の保存値、同一cursor、遅着Appendのreceipt復活拒否を検査する。
merge `cf90efc2e`の独立レビューP2への修正では、この3回帰（44ケース）がすべて成功した。
構成関連208件／§1.373関連76件を再検証し、重複を除く277件成功／失敗0／ignored 0。
fmtと通常／portable core checkも成功。fixtureの型参照だけを修正したため、両checkの成功を再利用する。
監査は607 Rust files／23,006関数を検査し、6つの構成view writerと2つの自動移行／flushを分類してPASS。
既存の構成draft／保存・解除とtrim／Remoteにも未分類のユーザーmutation入口は見つからなかった。
test-fullは利用者指定により実行しない。検証ログは`target/merge-review-cf90efc2e-final`に保存する。
`build-dev.ps1 -PreserveRuntime`はexit 0、core／remote／EPUB workerとruntime 4／PE 3の検査に成功。
source／監査scriptの検証前後SHA-256は一致し、`git diff --check`も成功した。
HEAD `cf90efc2e`を維持し、commit・mIVアプリ起動・coreの停止は行っていない。
自動補完の `persist_native_video_marker_thumbnail`／`tick_pending_pin_thumb_refresh` は
既に受理済みmarkerの再生成として明示除外する。readonly lookup／export／hydration、通常の読書位置record、
favorite view stateのautomatic capture／flush、migration／purge、テストfixture／seedも後続ユーザーmutationではない。
index・音量測定cacheの再生成／清掃や、対象metadataを変更しないお気に入り登録等の設定はこのinventoryの対象外。
Append自身のcopy／refreshの除外は同一operation IDのまま。監査の除外を自己操作の新しい照合へ広げない。

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
R4の独立設計レビュー前は製品への接続を停止した。現在はR4 ACCEPT後の実装中であり、
実ごみ箱操作の検証は引き続き別の明示了承済み検証枠で行う。

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

自動観測とreceiptの契約は維持する。R7ではmetadata／画像の順序だけを§7.7へ改訂し、
独立metadata worker／`AppendFilesWritten`の二段引渡しは撤去する。

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

R7では各destinationのmetadata初期化 → 画像作成を繰り返し → 最終構成照合の間は、
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
採用に付随して受理済みの書込みが残る場合だけ、§3.1に列挙した既存writerのBusy条件
(`rename_migration_writers_busy()`等)を使う。readonlyのrescan／prepareはこの判定に含めず、
既存ownerの取消・世代拒否で扱い、新しいBusy理由にはしない。古いAppend refreshの後着は
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

**R4提出時はここで停止した。R7の設計担当判断で書込み順序を確定し、実装・対象検証と確認用buildを完了した。全体gateの未成功は§7.9に記録する。ごみ箱方式の再承認は不要。**
R4提出時点では追加・実行テスト0件、build0件、アプリ起動0回、コミット0件で、退役試作は再適用しなかった。
これを現在の実装・検証の完了証拠として扱わない。

### 7.6 実装時の所有資格の具体化（2026-10-13）

`AppendOperation`の元contextと、一覧を採用するcontextは別の資格とする。
Rootの`poll_book_op_pending`がMainを投影していても、F12から受けた追加の通常終端は
既存`with_owner_viewer_context`で捕捉した元ownerへ配送する。一時mount自体では受付revisionを
失効させず、利用者による窓の切替・設定変更・後続mutationの受付で失効させる。
元窓が閉じられていればvisible採用とtokenだけを捨て、metadataコピーの結果、成功件数・
エラーのログと通常通知は終端へ残す。コピー完了で元画像や兄弟の編集draftを全消去しない。

既存refreshのpayload `BookAppendRefresh`にはoperation、採用先owner、exact folder、
items generation、既存の全scan optionsを載せる。元operationの生存／ID／受付revisionと、
採用先owner／folder／generation／optionsの一致を別々に照合する。fresh scanの全構成stampは
workerで確認し、これらの資格が失効した旧scanはtokenだけでなく表示採用も拒否する。
設定変更やDelete成功後の旧scanで行を戻さず、追加成功の通知は維持し、現在の通常refreshへ返す。
表示signatureが既存`same_listing`で一致する場合は観測signature・worker取得の親mtimeだけを進め、
一覧install・世代更新を行わない（§1.314を維持）。

現製品のwatch／Resumed／外部scan producerはMain updateから呼ばれ、scan pendingはApp-global。
本folderにはwatch購読がないため、Appendの観測はResumedと先着scanの採用入口を対象とする。
新しいdetached巡回・watch fanoutを作らない。元ownerが非投影／移動／park／dropなら既存の
stale/drop規則に従う。F12からの追加でMainが同じ対象本を表示している場合は、従来の追加成功更新を
維持し、Mainの既存scan ownerへ同operationのqualified refreshを渡す。元contextと異なるMainへの
採用でも、四つの資格（元operation、採用owner、exact folder、fresh構成stamp）が一致した
自己refreshだけは元receiptを失効させない。これは§1.375の自己操作照合ではなく、R4のAppend終端配送である。

Undo受付はその元contextで行う。readonly取消は同じ本の旧scan／同operationのdeferred要求と
元contextの既存readonly要求に限定し、無関係なMain・兄弟のscanを取り消さない。
fullscreen終了等でreceiptが失効してもdeferred更新は一般causeへ返す。Undoによる取消と、
通常失効後の最新表示更新を混同しない。新しいpending owner・Busy理由・時間窓は追加しない。

### 7.7 R7: metadata初期化をファイル公開前へ移す（設計担当決定、2026-10-13）

WIP `3e8178b31` の「追加中も通常の明示reload／openとタグ・★・編集を受け付ける」と、ファイル作成後に
八つのmetadata storeを独立workerで初期化する実装は、書込みの順序を保証できなかった。
これは通常操作で到達でき、まれな失敗の割り切り対象ではない。独立実装レビューもP1／REVISEと確認した。

具体的な順序は次のとおり。

1. `books::append_sources_with_cancel` が本フォルダへ番号付き画像を作成する。
2. 自動観測はAppend終端へ延期するが、明示reload／openは延期しない。作成済み画像が一覧に現れる。
3. その画像へ後続の★・タグ・編集を受け付け、DBへ保存する。receiptは失効するが、コピーは続く。
4. `start_book_append_metadata_terminal` の独立workerが後からコピー先metadataを初期化する。
   `RatingDb::copy_entry_key` の`ON CONFLICT ... UPDATE`、`TagsDb::copy_item_key`の
   destination削除／source再挿入等が、後続ユーザー編集の正本を上書きする。
5. DB書込みが逆順でも、古いimmutable copy reportをUI終端で採用すると、後続編集のcacheを戻し得る。

タグworkerのFIFOはそのworker内だけであり、同期rating書込みや他storeのコピーを含まない。
local-adjust fenceもhandoff前のそのworkerのjobを待つだけで、後続destination編集と八storeの
コピーを共通に序列化しない。受付revision／operation ID、copy reportへのgeneration cut／
受付key追加はtokenやcacheの保護には使えるが、正本DBの上書きを防ぐ十分条件ではない。
実装調査で見つかったexact destination cache更新は候補として残したが、全体の正しさの証明とは扱わない。

WIP `3e8178b31` の上記P1／P2に対し、設計担当ClaudeCodeが次のR7を決定した。
最初の調査で八storeすべてに`open_at`と同期copy／remove APIがあり、コピー先初期化に
別ownerのFIFOを必須とするstoreはないと確認した。独立の早期構造レビューもACCEPT。

| 方針 | 構造と通常操作への費用 |
| --- | --- |
| metadata完成まで新ページを公開しない（採用） | 各destinationについて、八storeの初期化を同じAppend workerで同期完了し、その後に画像を`create_new`する。ファイルの出現を公開点にし、stagingは作らない。明示reloadで見える途中ページも既にmetadata完成済みとなる |
| metadata初期化と後続destination書込みを序列化する（不採用） | 同期rating・別worker・他storeの通常書込みを広い共通ownerへ接続する必要がある。公開前に初期化すれば競合の組合せ自体を除けるため、新しい序列化owner／受付cutを導入する費用は不要 |

Appendの順序は以下で固定する。

1. 受付で既存local-adjust writerのsource fenceと有効storeの情報を捕捉する。待機は同じAppend
   workerだけで行い、UIに同期I/O／待ちを加えない。sourceタグの既存非同期FIFOについて、
   今回新しい受付時snapshot保証は追加しない。
2. 接続をAppend内で一回開き、各destinationの古いmetadataを初期化する。コピー対象のない値や
   source rowの不在も既存remove APIで空へ確定し、Full copy／Semantic copyの既存範囲を保つ。
3. そのdestinationの全store commitとepoch終端を完了してから`write_source`で画像を作る。
   全Appendを一つのactive epochにせず、公開済みページの明示閲覧・後続編集を妨げない。
4. 後続編集はファイル出現後なので、必ず初期化より後になる。画像ごとの未完成metadataと
   その画像へのユーザー編集の組合せがなくなる。独立`book-append-metadata` workerと
   `AppendFilesWritten`引渡しを撤去し、同じAppend ownerでreceiptの構成照合まで完了する。
5. copy reportは変更キー・エラー・DB所要時間だけを返す。★・タグ・六種presenceの古い値を
   UI終端からcacheへ書かず、新しいreport採用cut／資格台帳は作らない。

通常の初回読込／明示reloadで新ページを見つけた時点で、DB初期化は済んでいるため既存の
読込経路が現在値を取得する。mounted／parkedの後続編集cache・draftを終端で消去しない。
件数等の既存派生cacheは通常の失効へ戻す。
六つのApp-global編集存在集合には通常のphysical読込とは別のrollup用途があるため、変更キーだけを
既存`PageEditSnapshot::load_and_project_stable`で現在DBからworker再取得する。このreadonly準備は
copy reportと分離し、既存Append／refresh receiverが所有する。既存`PAGE_EDIT_WRITES`のstampで
通常staleを拒否し同じreceiverで再準備する。新しいgeneration／受付cut／pending owner／Busyは追加しない。
UI採用は対象keyのglobal存在集合だけに限定し、全体集合・contextの編集値／rating／tag cache・
draftを置換しない。既存の保存待ちlocal-adjust keyは現在の受付済みpresenceを保ち、完了consumerへ任せる。
readerのDB失敗はログ・通知して終端にし、まれな失敗のretry・回復待ちを追加しない。

初期化失敗ならその画像を作らず既存エラー通知へ返す。初期化後の画像作成失敗では、存在しない
destination keyの行を残す。既存orphan整理に任せ、新しいrollbackを作らない。
これは**既定の割り切り方針による**。既に成功したページとsourceは削除しない。
新しい番号付きファイルのkeyに残った古い行の上書きは設計担当承認済みである。
DB時間は接続開始と各画像の初期化を計測し、1／100／1000枚で旧store単位のcopy相当と比較する。
大量追加で通常操作の目立つ遅延が判明した場合は、最終完了扱いにせず設計担当へ返す。
§1.375は別の独立再レビュー対象で、R7では変更しない。

再設計の受入回帰では、制御channelでコピー前／コピー後／UI結果poll前の順序を固定する。
明示reloadで新ページを開き、★変更・0への解除、タグ追加・削除・全解除、六つの画像編集storeの
変更・解除を保存し、DB正本とmounted／parked cacheの両方で後続ユーザー操作が最後に残ることを
検査する。sourceの同時編集、別destination、無関係な兄弟のdraft、部分copy失敗も対照にする。
後着reportだけを抑止する試験を、正本の書込み順序の受入証拠として代用しない。

### 7.8 WIP `3e8178b31` 停止checkpointの検証記録（2026-10-13、R7以前の履歴）

- 対象`section1373`は50件成功・失敗0件。途中のfixture不備2件を修正して同じ50件を再実行した。
  新規Undo、receipt／構成、Shell実移動結果、後続入力、Remote先着順、F12→Main実採用、
  stale拒否、Delete前後のdeferred配送、exact destination cache更新を検査する。
  これらの成功を§7.7の正本／後着report上書きの解消証拠とは扱わない。
- 通常／portableのcore cargo check、cargo fmt --all -- --check、git diff --checkが成功。
- viewer_context_auditは違反0件。exact cache publication APIの正確なsignature一件を登録し、
  audit規則・検出範囲は変更していない。UI glyph検査は危険文字0件。
- 関連7文書のUTF-8 strict decode、manual 2ファイルのHTMLParser解析が成功。
- 今回のtest-full・build-release・build-devは0回。§7.7の設計判断で停止し、未完成の候補を
  最終検証・実装ACCEPT・利用者確認用binaryとして提出しない。アプリ起動0回・コミット0件。
- 全差分は50ファイル（実装／試験37、docs／manual 13）。主な境界は
  `src/books.rs`、`src/book_append_undo.rs`、`src/delete_worker.rs`、`src/app.rs`、
  `src/app/book_append_undo.rs`、`src/app/book_append_refresh.rs`、既存mutation／Undo入口、
  `src/app/viewer_context_registry.rs`と`tools/viewer_context_audit/src/lib.rs`。
  [全変更ファイルと状態](../target/section1373-checkpoint-summary.txt)を保持する。

ログ: [対象50件](../target/section1373-checkpoint-final.log)、
[通常core check](../target/section1373-checkpoint-core.log)、
[portable core check](../target/section1373-checkpoint-portable.log)、
[context audit](../target/section1373-checkpoint-audit-final.log)。
core check後のソース変更はfixture／audit登録のみで、製品codeは変えていない。

### 7.9 R7の実装・検証記録（2026-10-13）

八storeの同期APIを同じAppend workerへ接続し、各destinationについてmetadata初期化完了後に
画像を作成する。独立metadata workerと`AppendFilesWritten`は撤去した。copy reportは変更key・
エラー・DB時間だけを持ち、後続編集の値を終端でcacheへ上書きしない。現在DBからの六種presence
読込は既存receiver／epochへ接続し、保存待ちのlocal-adjust keyとmounted／parkedのcache・draftを保つ。
R7の独立実装レビューはACCEPT（P1／P2なし）。§1.375の製品code・設計・manualは変更していない。

WIP `3e8178b31`からの変更は13ファイル。実装・試験は`src/books.rs`、
`src/book_append_undo.rs`、`src/app.rs`、`src/app/book_append_refresh.rs`、
`src/app/book_append_undo.rs`、`src/app/viewer_context_registry.rs`、`src/app/tests.rs`、
`tools/viewer_context_audit/src/lib.rs`の8ファイル。文書は本書、`docs/README.md`、
`docs/architecture-overview.md`、`docs/compile-book-plan.md`、
`docs/next-release-backlog.md`の5ファイル。

- `section1373`は58件成功・失敗0件。制御channelでmetadata前／画像作成前／公開後／結果poll前の
  順序を固定し、八storeの後続追加・解除、sourceの後続変更、別destination、作成失敗のorphanを検査。
  mounted／parked cache、実Append終端の現在DB読込、stale再準備、保存待ちpresenceの保持も含む。
- `books::tests`は48件成功・失敗0件。上記との重複3件を除き、targetedは103種類・106実行が成功。
- 通常／portableのcore cargo check、cargo fmt --all -- --check、git diff --checkが成功。
  viewer_context_auditは違反0件、UI glyph検査は危険文字0件。退役したcache publication APIの
  audit登録だけを外し、auditの検出規則は変更していない。
- build-devは成功し、通常profileのcore／Remote／EPUB workerを生成した。build-releaseも成功し、
  core／Remote／EPUB worker／launcherとruntime・embedded PE検査が通った。両buildは
  `-PreserveRuntime`を使用し、常駐アプリ／通常profileのcacheを変更していない。
  アプリ起動0回・コミット0件。build後の変更は静的検査と文書だけで、製品codeは変えていない。

**全体検証の前提矛盾により、full成功として引き渡さない。** `test-full.ps1`は1回だけ実行した
（44分09秒、EXIT 101）。core以外のworkspaceは1,215件成功・失敗0件・ignored 7件。
coreの12,274件は最終集計に達していないため、その成功件数を推測しない。

- coreログには既存2件のFAILEDが出た。close／retireの静的検査はWIPで処理が共有helperへ移った後も
  wrapperだけを読んでいた。wrapperのManual委譲とretire前closure内の実completionを検査するよう
  `src/app/tests.rs`を追従し、従来のWindows素close禁止・残余context終端・呼出箇所数を維持。
  独立補完レビューはACCEPT、単独補完は1件成功・失敗0件。
- `visibility_external_rescan_restores_semantic_cursor_after_actual_hydration`は、変更を加えずに単独補完しても
  0.41秒で選択位置のassertion（`src/app/tests.rs:487`）に失敗した（0件成功・1件失敗）。5秒の
  hydration期限超過ではない。Appendを開始せず、通常load／rescan／sidecar hydrationだけを通る。
  R7のworker／presence reader／book pending guardとの直接交差がないことを独立読取で確認した。
  この通常一覧更新の回帰が成立しないことを前提矛盾として設計担当へ返し、未レビューの§1.375の
  更新処理や共通hydrationの所有をこのR7で変更しない。原因・修正方針の確定は未完了。
- coreの最後に既存HTTP期限試験だけが残り、試験用TCPサーバーの無期限`accept`待ちで終了できなかった。
  `src/clipboard_capture/fetch_tests.rs`のfixtureは期待回数の接続を受けるまで待つが、期限切れで
  後続接続が来なければ`server.join()`も終わらない。自分のharnessのexact executable pathを照合し、
  loopback待受1個／接続中clientなしを確認後、そのPIDだけを停止して残りworkspaceを完走させた。
  この中断を製品のcrashやテストPASSと扱わない。HTTP試験は無変更の単独補完で1件成功（1.43秒）。
  全体実行時のfixture停止を単独成功で置き換えず、Clipboardの製品code／fixtureを今回修正しない。
- workspace非成功のためfullから未到達だったvendor gateを各1回だけ個別補完。
  egui 25件、egui-wgpu 11件、eframe 18件が成功（合計54件・失敗0件）。全体再実行は0回。

対象58件＋books 48件の成功とは別に、個別補完3件は2件成功・1件失敗。fullを含まないこれらの
実行数は109件（108成功・1失敗）、重複を除いた種類数は106件。fullの既存FAILED二件については
harness中断前にpanic詳細が出なかったため、hydrationの上記assertionは単独補完の結果である。

同じSQLite DBに対し、接続をAppend中に再利用して実初期化時間を計測した。Fullは八storeに
値があるsourceのコピー、旧方式相当はstore単位のcopyと存在集合読込。通常追加はコピー元metadataの
mappingがなく、新しい番号付きdestinationの古い値を空へ初期化する場合である。

| 枚数 | R7 Full DB時間／枚 | 旧方式相当／枚 | 差／枚 | R7通常追加DB時間／枚 |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 41.472 ms | 17.426 ms | +24.046 ms | 0.466 ms |
| 100 | 12.615 ms | 13.249 ms | −0.634 ms | 0.227 ms |
| 1,000 | 12.480 ms | 11.735 ms | +0.745 ms | 0.219 ms |

接続開始はAppend一回につき4.760–6.529 ms。並行buildのあるPCでの非interactive計測で、
単発1枚は前の測定（R7 12.704 ms／旧11.896 ms）からも大きく変動した。100／1,000枚で見ると
通常追加の追加費用は約0.22 ms／枚（1,000枚で0.219秒）、Fullの旧方式との差は1,000枚で
0.745秒、DB部分の約6.3%。画像copy／decodeと実UIの体感時間はこの計測に含めない。
大量追加で新しい大幅な遅延を示す結果は得られていないが、実機操作の確認とは区別する。
metadata初期化後の作成失敗はログ・通知で閉じ、残ったorphanに新しいrollbackを作らない。
これは**既定の割り切り方針による**。

ログ: [対象58件・DB計測](../target/section1373-r7-targeted-complete.log)、
[books 48件](../target/section1373-r7-books.log)、
[通常core check](../target/section1373-r7-core.log)、
[portable core check](../target/section1373-r7-portable.log)、
[context audit（最終）](../target/section1373-r7-audit-final.log)、
[確認用build](../target/section1373-r7-build-dev.log)、
[release build](../target/section1373-r7-build-release.log)、
[full一回・未成功](../target/section1373-r7-full.log)、
[close静的検査補完](../target/section1373-r7-seam-followup.log)、
[hydration単独失敗](../target/section1373-r7-hydration-followup.log)、
[HTTP単独補完](../target/section1373-r7-http-followup.log)、
[egui](../target/section1373-r7-vendor-egui.log)、
[egui-wgpu](../target/section1373-r7-vendor-egui-wgpu.log)、
[eframe](../target/section1373-r7-vendor-eframe.log)。

### 7.10 R7追補: 通常rescanの選択復元退行（2026-10-13）

利用者の別worktreeでの切り分けは、`df20dba7b`で
`visibility_external_rescan_restores_semantic_cursor_after_actual_hydration`成功、
`3e8178b31`で同じ選択位置assertion失敗。間のdocs-only変更は原因ではなく、§1.373実装の退行とする。
§7.9の「前提矛盾」は切り分け前の記録であり、テストの前提が古いという判断は撤回する。

作用した差分は`apply_external_rescan`の`PreservePresentation`から`RefreshList`への変更。
Appendの自己refresh情報を運ぶために通常rescanも同じintentへ変更したが、既存hydrationの
操作選択／明示名前ヒントの復元後、`finish_main_list_open`が捕捉済みcursorをもう一度適用した。
その結果、正しく復元した高優先度の選択を旧cursorへ戻した。sidecar worker自体の変更ではない。
manual/full reload、Append終端fresh scan、deferred Append／一般Append refresh、
Collection／Smart／Rating等の物理子も同じhydrationを通るため、通常rescanだけの分岐では直さない。

不変条件は、受理された`resume_loading_items_after_sidecar`の既存選択復元で、
**操作結果 → 有効な明示名前ヒント → 同じlogical source／ZIP prefixのRefresh cursor → 履歴／読書位置**
を一度だけ解決すること。Refresh cursorは同じtyped intentからそこで消費し、後続の採用tailでは
再適用しない。自動cursorを`select_after_load`へ重ねて保存するrescan／reload処理は外し、
cursorと`rows_above`を既存Refresh要求に一緒に保持する。明示hintと操作要求は従来どおり優先し、
上位が採用された場合は下位cursorを消費だけする。明示hintが消失した場合はRefresh cursorへ戻る。

簡素化は、新しいrestore flag／pending／ownerや遅延を作らず、既存hydrationの選択確定点へ集約する。
hydrateを通らないZIP階層内の採用、RestoreList／ResumeListの明示target契約は維持する。
新しいUI同期I/O、待機、兄弟contextの失効は足さない。元の選択assertionは緩めず、manualと
一般Append refreshの実handler、明示hint消失、viewport anchor、次回reloadへのhint漏れを回帰に追加する。
§1.375の設計・manualはこの追補で変更しない。

追加の独立確認で、起動復元用`restorable_current_main_list()`をRefresh cursorの照合に流用すると、
★固定から開いたcaptured Folderの子が除外されることを確認した。これは通常の製品経路であり、
起動復元の資格と、現在のphysical listのカーソル復元資格は同じではない。
capture／consumeは共通のread-only physical source／prefix投影に揃える。current folder、logical alias、
ZIP treeのbackingとの対応をI/Oなしで照合し、context／generationは受理済みsidecar ownerが照合する。
startup recordのmain限定・snapshot除外は変更しない。★固定の実Folder子rescan回帰で選択に加え、
固定rootの世代・origin・rows／keysとparentのstartup recordが不変であることを確認する。
先の独立ACCEPTはこのfindingで撤回され、分離設計を再レビューしてACCEPT。最初のtest-fullは
この追加findingで中断したため成功証拠にはしない。修正後に最後まで通るfull gateを一回実行する。

最終差分の独立実装レビューはACCEPT、未解決指摘なし。capture拒否時はfallback targetから
cursor資格を復活させず、source不明／synthetic／ZIP backing不一致と有効sourceの対照も回帰に追加した。
元の選択期待は維持し、回帰4件追加。4入口×4条件の実hydrationと、拒否3条件＋有効対照を確認した。
最終targetedは全体ゲートと同じ`pack-build-tools`条件・`RUST_TEST_THREADS=2`で次のとおり成功。

| filter | 成功件数 |
| --- | ---: |
| folder_mtime_rescan_tests | 9 |
| startup_restore_tests | 73 |
| section1353 | 21 |
| book_append | 42 |
| post_operation_selection | 14 |
| sidecar_restore | 61 |
| section1373 | 58 |
| total_deadline_covers_slow_headers_drip_bodies_and_redirect_remaining_time | 1 |

合計279実行、重複除外237件、失敗0件。修正前の単独再現は0成功／1失敗として別に記録する。
fmt checkは最終コードで成功。後述のfull完走と区別し、過去の中断結果を成功件数へ加算しない。

最終通常／portable core check、`build-dev.ps1`が成功。確認用core／remote／EPUB workerの3 exeと
runtime 4ファイル／PE 3件の配置確認が成功し、アプリは起動していない。
最終`test-full.ps1 -SuppressCrashDialogs`は`RUST_TEST_THREADS=2`で**一回完走、exit 0／PASS**。
workspace 13,439件、vendor egui 25件／egui-wgpu 11件／eframe 18件、
合計13,493成功・0失敗・既存ignored 63件（doc-test等を含む）。追加findingで中断した一回は別記録であり、
製品テスト失敗や成功証拠として数えない。最終完走中のソース3ファイルのSHA-256は開始時と一致。
§1.375の設計書・manual変更0件、コミット0件。

追補ログ: [修正前の単独再現](../target/section1373-rescan-fix/baseline.log)、
[最終rescan 9件](../target/section1373-rescan-fix/folder_mtime_rescan_tests-final.log)、
[最終起動復元73件](../target/section1373-rescan-fix/startup_restore_tests-final.log)、
[最終§1.353 21件](../target/section1373-rescan-fix/section1353-final.log)、
[最終§1.373 58件](../target/section1373-rescan-fix/section1373-final.log)、
[最終sidecar 61件](../target/section1373-rescan-fix/sidecar_restore-final.log)、
[fmt](../target/section1373-rescan-fix/fmt-check.log)、
[通常check](../target/section1373-rescan-fix/core-check-final.log)、
[portable check](../target/section1373-rescan-fix/portable-check-final.log)、
[最終確認用build](../target/section1373-rescan-fix/build-dev-final.log)、
[中断full（成功証拠ではない）](../target/section1373-rescan-fix/test-full-interrupted.log)、
[最終full完走](../target/section1373-rescan-fix/test-full-final.log)、
[full件数集計](../target/section1373-rescan-fix/full-summary.json)。

### 7.11 LISTrev8追補: 後続mutationの共通受付漏れ（2026-10-13）

基点は`13f2fc3e9`（§1.373のコードはレビュー基点`c903a369b`と同一）。
全実装の独立レビューはREVISE、P1なし／P2一件／P3一件。
P2は代表ピンと動画しおり／ピンのユーザー書込みが既存受付helperへ届かず、
receiptが残ること。ZIPのピンはdirtyを消して帰るので後続reloadも失効を保証しない。
守るべき不変条件は、最初のユーザー書込み・draft変更・worker投入前に同じ受付を通り、
Append中はrevisionを進めて遅着receipt公開を拒否し、UndoAppend中だけ新規書込みを拒否すること。

§3.2のproducer inventoryを実書込み入口で監査し、指定群に加えて音声／動画音声モードのしおり、
egui動画の追加、横断しおり一覧の削除、マスクのslot保存、回転の全リセット、編集情報の復元、
履歴と読書位置の明示削除、favorite表示状態の明示リセット／削除、favorite削除の先行settings変更、
settings Restore／FullResetの投入に同型の漏れを確認して接続した。
automatic cache補完・通常読書位置record・migration／purgeは元操作やreadonly lifecycleに属するため、
これらを後続ユーザーmutationとして失効させない。対象storeを書かない設定まで排他を広げない。

代表ピン／動画ピン／メディアしおりは既存App-owned storeを返す借用入口で共通helperへ接続。
同じstoreへのnative・egui・音声・キー・HUD・menuの書込みを入口ごとのreceipt印へ分けず、
新しいpending／owner／I/Oは追加しない。その他は既存の共通mutation handlerを受付へ接続した。
通常SourceMutation／本削除中の受付規則は維持し、UndoAppend中の拒否と対で検査する。
P3は冒頭の状態を§7.10の完走記録とLISTrev8の現在の判定へ揃えた。過去の中断記録は維持する。

検証証跡は`target/section1373-listrev8/`へ集約する。今回の修正の検証・独立再レビューは、
§7.10の既存成功証拠と区別して報告する。アプリ起動・コミットは行わない。

変更ファイル（計16、§1.375設計書／manualは変更なし）:

| 範囲 | ファイル |
| --- | --- |
| 共通受付とhandler | `src/app.rs`、`src/app/media_metadata_mutation.rs`（新規） |
| 動画／音声 | `src/app/native_video.rs`、`src/ui_fullscreen.rs`、`src/ui_music_panels.rs` |
| 兄弟mutation | `src/app/book_resume_meter.rs`、`src/app/content_identity_restore.rs`、`src/ui_erase.rs`、`src/ui_conceal.rs`、`src/ui_dialogs/favorites_editor.rs`、`src/ui_dialogs/preferences.rs`、`src/ui_dialogs/rotation_reset.rs`、`src/ui_dialogs/settings_restore.rs` |
| 回帰／機械的監査／設計 | `src/app/book_append_producer_tests.rs`、`scripts/check-book-append-mutation-admission.py`（新規）、この設計書。動画／音声・復元ファイル内にも回帰を追加 |

回帰10件を追加。receipt ready／Append pending／UndoAppend pending／SourceMutation／本Deleteの
対照で、ZIPピンの実toggle、動画しおりの実handler、共有DB入口全書込みAPI、音声・動画音声モード、
横断削除worker投入、slot／回転／表示状態／読書位置reset、復元worker投入を検査した。
Append中は後続mutation後に実`poll_book_op_pending`へ古い成功結果を届け、receipt復活を拒否する。
Undo中はDB・map・pending・revision不変、未受付のempty／stale／DB不存在も対照にした。

| targeted filter（`pack-build-tools`／`RUST_TEST_THREADS=2`） | 成功件数 |
| --- | ---: |
| section1373 | 68 |
| folder_pin | 17 |
| native_video_bookmark | 1 |
| music_bookmark | 1 |
| content_identity_restore | 10 |
| favorite_view | 18 |
| settings_restore | 30 |
| folder_mtime_rescan_tests | 9 |

合計154実行、重複除外152件、失敗0件。
機械的監査は15 media writer／13兄弟入口／3共通受付／2 automatic除外、
srcの571 Rustファイル／22,313関数を走査してPASS。これは限定lexical監査であり、
Rustの型・全control flow・macro／関数を跨いだaliasの証明ではなく、handler回帰と併用する。
最終fmt checkと通常／portable core checkは成功。

検証開始時の二回は再利用build scriptに`G:\home\miv-bisect`が埋め込まれたため
LibRaw入力不足でexit101、実試験0件。現worktreeの入力実在を確認し、build scriptを再生成した。
追加回帰helperの型／メソッド名の初期compile失敗二回も実試験0件で、上の成功件数へ加算しない。
ツール側中断で停止したbuild-devは成功証拠にせず、状態・stdout／stderrを残す
非表示の検証jobでbuild-dev→test-fullを順次実行した。製品のpending ownerは追加しない。
指定Sol／xhighの独立差分レビューは二回ともmodel capacityエラーで未完了。
新たなACCEPTを主張せず、修正差分をcoordinatorの独立再レビューへ渡す。

最終`build-dev.ps1 -PreserveRuntime`は通常feature setでexit 0。core／remote／EPUB workerの
3 exeを確認し、VCRT runtime 4件／PE 3件の配置検証も成功。アプリは起動していない。
最終`test-full.ps1 -SuppressCrashDialogs`は`RUST_TEST_THREADS=2`で**一回完走、exit 0／PASS**。
workspace 13,449成功、vendor egui 25件／egui-wgpu 11件／eframe 18件、
合計**13,503成功・0失敗・既存ignored 63件**（doc-test等を含む）。
通常rescanの旧退行試験 `visibility_external_rescan_restores_semantic_cursor_after_actual_hydration` も成功。
最終差分check・§1.375設計書／manualの未変更確認が成功。実機確認と独立再レビューは別の未完了条件。
コミット0件。今回の成功証拠へ、初期compile失敗やツール中断の結果を加算しない。

証跡: [targeted68件](../target/section1373-listrev8/section1373-final.log)、
[機械的監査](../target/section1373-listrev8/mutation-audit.log)、
[fmt](../target/section1373-listrev8/fmt-check.log)、
[通常check](../target/section1373-listrev8/core-check.log)、
[portable check](../target/section1373-listrev8/portable-check.log)、
[build／full stdout](../target/section1373-listrev8/verification.stdout.log)、
[build／full stderr](../target/section1373-listrev8/verification.stderr.log)、
[終了状態](../target/section1373-listrev8/verification-state.json)、
[件数・ソースhash集計](../target/section1373-listrev8/verification-summary.json)。

### 退役記録と再開範囲（R7以前の履歴）

設計判断前に試作のApp fields／Undo入口／producer gate／typed refresh接続を撤回した。
撤回時の製品は既存の受付規則を維持した。その後、独立レビューACCEPTを受けてR4に沿って同じ既存ownerへ接続した。
退役記録は`target/section1373-integration-prototype.patch`と
`target/section1373-prototype/`の実ファイルcopy。これは未検証の途中差分であり、
製品仕様の正本・適用済み変更・検証証跡とは扱わない。

案の採否と費用は上記の利用者決定で確定済み。独立レビューでは§7.1–7.5の入口、先着scan、
終端照合、deferred refreshへのID引渡しを確認する。退役差分をそのまま製品へ戻さない。

退役前の worker／receipt／Shell 部分は独立コードレビューで重要所見なし。当時のApp接続は
未完成・未検証であり、この部分レビューを製品の§1.373全体 ACCEPTとは扱わなかった。
瞬間的な同stamp差替えは既定の割り切り方針による。撤回時の製品差分には新Undo入口を残さなかった。
