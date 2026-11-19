# §1.464 フォルダツリーへのドラッグ&ドロップ・右クリック貼り付け / §1.821 一覧へのドロップの上書き — 設計

2026-11-19〜20 設計 (ClaudeCode)。worktree `G:\home\mimageviewer-treednd464`、branch `treednd-464`、基点 master `ca6686d93`。
要件の正本は [next-release-backlog.md §1.464・§1.821](next-release-backlog.md)。
**状態: 設計 r2 (独立レビュー r1 = REJECT の P1×3・P2×5 を反映)。実装前の再レビュー待ち。**

要望の要旨 (5ch): >>444 ツリーへファイルをドロップできず、右クリックの貼り付けもできない。>>707 読み終えたファイルを
`_mita` フォルダへ D&D したい (内部ドラッグで「＋」カーソルなのに何も起きない)。固定先へ送る「ファイル整理先」(v4.4.0 出荷)
はこの要望の代替扱いにしない (backlog の記録)。

## 0. 利用者の決定 (2026-11-19、統括経由)

- Q1 修飾キーなしでツリーへ落としたときの既定は Explorer と同じ (同じドライブは移動、別のドライブはコピー。Ctrl でコピー、Shift で移動)。
- Q2 mIV の一覧から mIV の外へのドラッグは今までどおりコピーだけ。
- Q3 ツリー行の右クリックは「貼り付け」の 1 項目だけ。
- §1.821 一覧へのドロップが同名を確認なしで上書きする件は v5.5.0 で一緒に直す。ツリーと同じ経路 (IFileOperation、競合の確認は Windows の画面) に揃える。コミットは §1.464 と分ける。
- ドラッグ中の自動展開・自動スクロールはしない (報告として了解)。
- **未回答 (§11)**: Q4 外部からツリーへのドロップをコピーだけにすること、Q5 §1.821 でフォルダのドロップも受けるか。

## 1. 結論 (推奨構成)

1. **転送の実行は既存の整理先と同じ経路**。ツリー行へのドロップは `ShellTransferRequest` を作り、整理先の単一 request owner
   `FileOrganizeRequest::Running` へ入れる。検証・`IFileOperation`・完了処理 (`transfer_items_async` → `poll_file_organize`) を共有する。
2. **ドロップを受けるために ROOT の OLE drop target を mIV 自前の 1 個に置き換える** (winit は位置・修飾キーを渡さず常に COPY と答えるため)。
   一覧への落下は、新しい drop target から今の `handle_external_file_drop` へ渡す。
3. **mIV の一覧からツリーへ** (§4): Q1 の規則で移動/コピー。送出の許可効果は COPY のまま (Q2)。転送は mIV が `IFileOperation` で行い、
   OLE には COPY を返す。操作の表示はドロップの説明 (drag image のバッジ) で必ず正しく出す。
4. **外部 (エクスプローラー等) からツリーへ** (§5): **コピーだけ** (Q4)。外部の元に対して移動の完了を正しく知らせるには、
   Drop の中で同期に移動する (UI が止まる) か、元が非同期転送に対応している必要があり、Shell が作る data object は非同期を公開しない
   (file-drag-drop-design §12.2 の実測)。今の一覧への落下と同じ「COPY を返し、後で worker がコピー」の約束事に留める。
5. **ツリー行の右クリック「貼り付け」**は、Ctrl+V と同じ関数 `invoke_shell_folder_background_verb(hwnd, folder, Paste)` を、
   **STA の worker スレッドで**その行のフォルダへ呼ぶ (§7)。
6. **§1.821**: 一覧への落下の worker の中身を PowerShell `Copy-Item -Force` から整理先と同じ Shell 転送 (Copy) に替える (§8)。
7. 処理中はモーダルにしない (整理先 §5.1 と同じ判断、§9)。

## 2. 現状のコード事実

| 領域 | 事実 |
| --- | --- |
| ツリー | `src/ui_folder_pane.rs::render_folder_pane_tree`。行は実フォルダとドライブだけ (`folder_pane.rs:1356` が directory 分類の項目だけ列挙)。ZIP / PDF・コレクション・タグ・検索結果の行は無い。右クリックの処理は無い (`ui_folder_pane.rs:977` の非 Primary `clicked()` は擬似クリック)。毎フレーム `folder_pane.label_display` に label と arrow の rect (行全体ではない) と、ScrollArea の `inner_rect`・layer・pass が残る |
| ツリーの更新 | §1.686。`FolderPaneState::observe_accepted_folder(path)` (`folder_pane.rs:805`) は **path 自身の直下**を未確認にし、需要があれば再走査する。今の producer は main の受理済み rescan (`app.rs:28193`) だけ |
| ドラッグ送出 | `ui_main.rs:3268` 左ドラッグで実項目を集め、仮想項目の除外は `post_drag_toast`。`update_phases.rs:1728` `post_render_native_drag` がフレーム末尾 (描画・present の前) に `file_drag::start_file_drag` を同期で呼ぶ。`dwOKEffects = DROPEFFECT_COPY` (`file_drag.rs:254`)、IDropSource は Shell 既定。`shell_data_object_for_paths` は `SHParseDisplayName` に失敗したパスを除いて残りで data object を作る (`file_drag.rs:107`・`131`) |
| ドロップ受け | winit 0.30.13 `drop_handler.rs:79/106/133`: `CF_HDROP` があれば DragEnter/Over で COPY、Drop でパスだけ送る。`window.rs:1167–1204` で `OleInitialize` 後に `RegisterDragDrop`、`event_loop.rs:1260` の WM_DESTROY で `RevokeDragDrop`。eframe → `RawInput.dropped_files` → `update_phases.rs:438` → `handle_external_file_drop` (`app.rs:52580`)。ROOT の `dropped_files` の consumer はここだけ、`hovered_files` の consumer は無い |
| 跳ね返り | `update_phases.rs:1743`: effect が NONE 以外でカーソルが mIV の窓上なら `internal_drop_pending` を立て次の drop を捨てる。>>707 の「＋なのに何も起きない」と整合 (コード経路。実行での再現ではない) |
| 一覧への落下 | `handle_external_file_drop`: 検索結果・タグ表示中と `current_favorite_target()` が None (ZIP/PDF 表示中・本棚配下) を拒否。`file-drop-validate` worker でフォルダを skip (`partition_dropped_paths`)、`context_menu.rs:3321` `copy_paths_into_folder` が `Copy-Item -LiteralPath $f -Destination $dest -Force` (同名を確認なしに上書き)。受信は `drop_copy_pending: Vec<Receiver<CopyOutcome>>` (並行可) → `poll_file_drop_pending` が `pending_reload` |
| 整理先の転送 | `shell_file_ops.rs:70` `validate_transfer_request` (worker 内、全件検証、junction 解決、同じ親への移動・自身/配下を拒否)、`:150` STA の `IFileOperation`、`FOF_ALLOWUNDO | FOFX_ADDUNDORECORD` (`:188`)、全件予約後に 1 回実行 (`:219`) |
| 整理先の owner | `file_organize.rs:14` `Hidden / Selecting / Running`。`:441` `submit_file_organize` が Remote 所有・終了要求・HWND を見て `admit_book_append_followup_mutation` 後に起動。`:519` 完了で現在の実フォルダが元の親/先なら `Notified` を 1 回。common modal・sidecar 復元は見ていない |
| 共通の blocker | `viewer_mutation_modal_active()` (`app.rs:23732`) は sidecar 復元を含まない。sidecar は別 (`app.rs:23907`) |
| 貼り付け | Ctrl+V `app.rs:64139`: ファイルなら `request_post_operation_selection_for_added_items(folder)` → `invoke_shell_folder_background_verb` → 失敗で `cancel_post_operation_selection`、`resync_egui_modifiers_from_os`。この関数 (`native_context_menu.rs:1032`) は自分で STA を初期化 (`ComStaGuard::new`) し、bind・menu 構築・`InvokeCommand` を同期で行う。戻り値は verb 呼び出しの成否で転送完了ではない |
| 取り消し | mIV の Ctrl+Z は評価・タグ・補正と本への追加だけ (`undo_ops.rs:339`)。整理先の転送は `FOFX_ADDUNDORECORD` で Windows の取り消し履歴に載る |

## 3. 範囲

| 対象 | 初版 |
| --- | --- |
| 落とし先 | ツリーの**画面に表示されている行** (実フォルダ・ドライブ)。一覧は既存のまま (§1.821 の中身の変更だけ) |
| 元 | mIV の一覧からのドラッグ (移動/コピー)、外部からの `CF_HDROP` (コピーだけ) |
| 右クリック | ツリー行に「貼り付け」1 項目 |
| やらない | ツリー行をドラッグの元にする、ドラッグ中の自動展開・自動スクロール・行の強調表示、ショートカット作成、外部からの移動、`CF_HDROP` の無いデータ、mIV 独自の取り消し |

自動展開・自動スクロール・強調表示を外す理由: mIV の一覧からのドラッグ中は `SHDoDragDrop` が `App::update` の中で止まり egui が描けない。
ドラッグ元で見え方を分けない。**落とし先の行を先に表示しておく** (マニュアルに書く)。操作と行は §4.3 のバッジで示す。

## 4. mIV の一覧からツリーへ

### 4.1 元パスの確定 (r1 P2-8)

ドラッグ開始時 (`ui_main.rs:3268` の payload 決定) の全パスと、仮想項目の除外の有無を `InternalDrag { paths, excluded_virtual }` として保持し、
`start_file_drag` が返す `failed_paths` (PIDL 化に失敗した件数) も記録する。**ツリーへの転送は `InternalDrag.paths` 全件を使う** (data object の縮んだ
`CF_HDROP` は使わない)。`excluded_virtual` か `failed_paths > 0` ならツリーでは受け付けない (整理先と同じ「対象外が混ざれば全体を実行しない」)。
外部へのドラッグの既存の挙動 (実項目だけ送り、除外を後で知らせる) は変えない。

### 4.2 操作の決め方 (Q1、r1 P2-7)

DragEnter / DragOver で、行・修飾キー・元パスから決める。UI スレッドなのでファイルシステムには触らない (字句だけ。最終確認は worker の
`validate_transfer_request`)。

| 押しているもの | 操作 |
| --- | --- |
| Shift | 移動 |
| Ctrl | コピー |
| なし | 元が全部、落とし先と同じドライブなら移動、1 件でも別ならコピー |
| Ctrl+Shift・Alt | 受け付けない (ショートカット作成はしない) |

「同じドライブ」は**パスのルート (ドライブ文字、UNC は `\\server\share`) を大文字小文字無視で比べる近似**。Explorer はボリュームで判定するので、
フォルダにマウントした別ボリューム・別表記で届く同じボリュームでは Explorer と既定が違い得る (マニュアルでは「同じドライブ」と書く)。
送出元は mIV 自身なので、preferred effect や元の許可効果による分岐は無い。

受け付けない場面: 行の上でない / 本棚の配下 / 移動で先が元の親と同じ (字句) / 元フォルダ自身か配下 / §6 の受付条件を満たさない / §4.1。

### 4.3 OLE への返答と表示 (r1 P2-6)

送出の `dwOKEffects` は COPY のまま (Q2)。ツリー行の上では mIV の target は OLE へ COPY を返し、Drop で `InternalDrag.paths` から
`ShellTransferRequest` を作る (data object からは読まない)。送出元は mIV なので COPY と答えても削除は起きない。

**移動をコピーと見せない**ことを必須にする。そのため送出側を次のように変える (外部へのドラッグの見た目も Explorer と同じ形に揃う):

- `start_file_drag` で、Shell 既定の IDropSource の代わりに mIV の IDropSource を渡す。`QueryContinueDrag` は標準どおり (Esc で取消、
  ボタンを離したら drop)。`GiveFeedback` は、data object の `CFSTR_DROPDESCRIPTION` が有効 (type が `DROPIMAGE_INVALID` 以外。
  mIV の target もエクスプローラーも書く) なら `SetCursor(IDC_ARROW)` して `S_OK` (カーソルは矢印、操作はバッジで示す)、
  そうでなければ `DRAGDROP_S_USEDEFAULTCURSORS` (今と同じ見た目)。説明文の読み取りは自分の data object への `GetData` だけで、I/O は無い。
  Shell 既定の IDropSource がしていた他の処理を取りこぼさないか、実装前にレビューで確かめる。
- 送出前に `IDragSourceHelper2::SetFlags(DSH_ALLOWDROPDESCRIPTIONTEXT)` と `InitializeFromWindow` で drag image を用意する
  (Microsoft Learn: SetFlags が無いと説明文は表示されない)。
- mIV の target は `IDropTargetHelper` (`CLSID_DragDropHelper`) の DragEnter/Over/Leave/Drop を呼び、ツリー行の上では data object へ
  `CFSTR_DROPDESCRIPTION` (`DROPIMAGE_MOVE` + 「%1 へ移動」/ `DROPIMAGE_COPY` + 「%1 へコピー」/ 受け付けないとき `DROPIMAGE_NONE`、
  %1 はフォルダ名) を `SetData` し、変えたら drag window へ `DDWM_UPDATEWINDOW` を送る。ツリー行を離れたら説明を `DROPIMAGE_INVALID` に戻す。
- 「移動」バッジが出るのは mIV のツリー行の上だけ。エクスプローラーの上では、エクスプローラーが書く説明 (コピー) が出る
  (`DSH_ALLOWDROPDESCRIPTIONTEXT` を立てたことによる見た目の変化。操作は今どおりコピー)。

実機確認 (§10) で「ツリー行の上の移動が移動として見える」を**出荷の条件**にする。出なければ移動を出荷しない判断を統括へ返す。

### 4.4 一覧の上・ツリーの空白

mIV 内のドラッグは、ツリーの行以外 (一覧・ツリーの空白) では NONE を返す (今の「＋なのに何も起きない」をやめる)。
`internal_drop_pending` / `cursor_over_window` の推定は不要になるので外す。

## 5. 外部からツリーへ (コピーだけ、r1 P1-3)

外部の元 (`InternalDrag` の印が無いドラッグ) がツリー行の上に来たら:

- 元の許可効果に COPY があり、`CF_HDROP` があれば COPY。無ければ NONE。修飾キー・preferred effect に関わらずコピー (移動は受けない。Shift でもコピー)。
  ドロップの説明は「%1 へコピー」(drag image は元が用意していれば表示される)。
- Drop: `CF_HDROP` のパスで `ShellTransferRequest { Copy }` を作り、**COPY を返す**。`PERFORMEDDROPEFFECT` 等は書かない。
  これは今の一覧への落下 (winit が COPY を返し、mIV が後でコピーする) と同じ約束事で、元は何も消さない。元が一時ファイルを Drop 後に消す
  アプリ (書庫ソフトなど) では、後のコピーが「見つからない」で失敗する (失われるものは無い。一覧への落下も同じ)。
- 外部からの移動を受けるには、Drop の中で同期に移動する (UI が止まる) か、元の非同期転送 (`IDataObjectAsyncCapability`) に完了を返す
  必要がある。Shell の data object は `GetAsyncMode` が FALSE (file-drag-drop-design §12.2)。初版では受けない (Q4)。

## 6. ROOT の drop target

### 6.1 置き換え (r1: 構造上の衝突なしと確認済み)

ROOT の HWND を得た最初のフレームで `RevokeDragDrop(hwnd)` → `RegisterDragDrop(hwnd, mIV の target)`。OLE の初期化は winit が済ませている。
winit の WM_DESTROY の `RevokeDragDrop` が mIV の target も外す。**登録に失敗したらログ + 通知** (ドロップを受けられない旨) で終え、成功したふりをしない。
detached viewer・フルスクリーン等の他の viewport の drop target は変えない (detached 述語・viewport 経路に触れない)。
COM object は `#[implement(IDropTarget, Agile = false)]`、IDropSource も同様。

### 6.2 App との受け渡し

callback は UI スレッドで (a) winit のメッセージ処理の中 (外部ドラッグ)、(b) `SHDoDragDrop` の中 (mIV 内ドラッグ、App::update の中)、
(c) 右ドラッグ等の native メニューや Shell の画面のメッセージループの中、に呼ばれ得る。**callback は App に触れない**。
`Rc<RefCell<MainDropShared>>` だけを介し、**RefCell の借用を COM 呼び出し・メッセージループをまたいで持たない**。

`MainDropShared`:

- `drawn: DropGeometry` — App が update の最後に書く、そのフレームの形 (§6.3)。
- `presented: DropGeometry` — App が update の最初に `drawn` から移す (前の update は描画・present 済み)。
- `in_update: bool` — update の最初に立て、最後に下ろす。callback は `in_update` なら `presented`、そうでなければ `drawn` を使う
  (どちらも「いま画面に出ている形」)。
- `admission: DropAdmission` — App が毎フレーム書く受付可否 (§6.4 の述語の結果)。
- `internal: Option<InternalDrag>` — `start_file_drag` の直前に立て、戻った直後に外す。
- `feedback: Option<TreeFeedback>` — target が書き、IDropSource の `GiveFeedback` が読む。
- `drops: Vec<DropRequest>` — target が書く。`GridFiles(paths)` / `TreeTransfer(ShellTransferRequest)`。外部ドラッグなら `request_repaint`。

App の消費: `update_phases.rs` の今の `dropped_files` の場所で `drops` を取り出す。mIV 内ドラッグは `post_render_native_drag` が
`SHDoDragDrop` から戻った直後に同じ処理を呼ぶ。`GridFiles` は `handle_external_file_drop` (sidecar 復元中は今と同じく捨てる)、
`TreeTransfer` は §6.4 の投入境界へ。

### 6.3 形と当たり判定 (r1 P1-1・P1-2)

`DropGeometry` は、表示された最終 pass の `label_display` から作る:

- `pane_rect` (ツリーのパネル全体、ヘッダを含む)、`tree_clip` (ScrollArea の `inner_rect`)。
- 行ごとに**行全体の rect** (`draw_folder_pane_row` が返す row_rect を `tree_clip` で切ったもの) とパス。行の rect は今の label/arrow だけでなく
  row_rect も `FolderPaneLabelDisplay` に記録する。
- 行が他のレイヤー (popup・メニュー・モーダル・ツールチップ以外の Area) に隠れていないか: 作る時点で行の中心と四隅を
  `ctx.layer_id_at` で見て、ツリーのレイヤーでなければその行は対象外。
- `client_size_px` と DPI (`GetDpiForWindow`)、`pixels_per_point`。

callback: `POINTL` → `ScreenToClient` → `/ pixels_per_point`。**今の `GetClientRect` の大きさと DPI が形と違えば、どこでも NONE**
(リサイズ・DPI 変更から次の表示までの間に、古い形で判定しない)。点の分類:

| 分類 | 扱い |
| --- | --- |
| `TreeRow(path)` (`tree_clip` 内で行全体の rect に入り、隠れていない) | §4・§5 |
| `TreeNonTarget` (`pane_rect` 内でそれ以外: 空白・インデント・スクロールバー・隠れた行・ヘッダ) | NONE。一覧の経路へ流さない |
| `ExistingDropArea` (`pane_rect` の外) | 外部ドラッグは今の一覧への落下 (COPY、Drop で `GridFiles`)。mIV 内ドラッグは NONE |
| 形が無い・古い (ツリー非表示のフレームを含む) | ツリーが表示されていなければ `pane_rect` は空なので全部 `ExistingDropArea`。大きさ・DPI 不一致は全部 NONE |

### 6.4 受付の述語と投入境界 (r1 P2-4)

副作用の無い述語 `shell_transfer_admission(&self, target) -> Result<(), &str>` を 1 つ作り、snapshot の `admission` と投入時の両方で使う。
見るもの: `FileOrganizeRequest` が `Hidden` (整理先の選択画面 `Selecting` もモーダルとして拒否、`Running` は消さない)、
`remote_session_blocks_local_control()`、終了要求 (shutdown・tray quit・root close)、`main_hwnd`、`viewer_mutation_modal_active()`、
`sidecar_restore_active()`、`common_modal_dialog_open`、`folder_pane_disabled()`、先が本棚の配下でないこと。

投入 `submit_shell_transfer(request)` は述語を**もう一度**見て、通ったときだけ `admit_book_append_followup_mutation()` (本への追加の取り消しを失効させる)
→ `transfer_items_async` → `Running`。通らなければ理由を通知して何もしない。整理先の `submit_file_organize` もこの述語と投入を使うように揃える
(条件を 2 か所に書かない)。snapshot の作成では `admit_book_append_followup_mutation` を呼ばない。

「同時に 1 本」は整理先とツリーへのドロップの間の保証。一覧への落下 (§8) とツリーの貼り付け (§7) は今の Ctrl+V・一覧への落下と同じく並行し得る
(全操作を横断する排他は作らない)。

## 7. 右クリック「貼り付け」(Q3、r1 P2-5)

- ツリー行の右クリックで、egui のメニュー (固定パネルの規則どおり `os_theme` の helper) に「貼り付け」1 項目。行は右クリックした時点のパスで確定する。
  有効なのは、クリップボードにファイルがある (`IsClipboardFormatAvailable(CF_HDROP)`、メニューを開いたとき 1 回) かつ §6.4 の述語が通るとき。
  無効の理由はツールチップで示す。画像だけのクリップボードは対象外 (Ctrl+V の画像保存はツリーでは行わない)。
- 実行: 述語を再確認 → `admit_book_append_followup_mutation()` → 先が現在の実フォルダなら Ctrl+V と同じく
  `request_post_operation_selection_for_added_items(folder)` → **STA の worker スレッド**で
  `invoke_shell_folder_background_verb(hwnd, folder, Paste)` (関数は自分で STA を初期化するので同じ関数を使う)。結果は channel で受け、
  失敗ならログ・通知と `cancel_post_operation_selection`、`resync_egui_modifiers_from_os`。UI スレッドで Shell の bind・`InvokeCommand` を行わない
  (未接続のネットワーク先でも UI を止めない)。結果は verb 呼び出しの成否で、転送の完了は Shell が持つ (Ctrl+V と同じ)。
- 切り取りの完了通知 (PasteSucceeded 等) は Shell の paste が持つ。mIV の切り取りの data object は UI の STA にあり、worker からの呼び出しは
  COM の marshaling で UI スレッドへ届く (UI は止めていないので配送される)。
- 貼り付けの実行中の二重実行: 1 本の `Option<Receiver>` (`tree_paste_pending`) で持ち、実行中は「貼り付け」を無効にする。
- Ctrl+V 自体 (UI スレッドで同期) は今回変えない (別項目候補)。

## 8. §1.821 一覧へのドロップ

- 原因 (コード): `copy_paths_into_folder` (`context_menu.rs:3321`) が `Copy-Item ... -Force` で、同名のファイルを確認なしに上書きする。
- 直し方: `handle_external_file_drop` の受付 (検索結果・タグ表示中・ZIP/PDF 表示中・本棚配下の拒否、`admit_book_append_followup_mutation`) と
  `file-drop-validate` worker・`drop_copy_pending` の並行受信はそのまま。worker の中のコピーだけを、整理先と同じ検証 + `IFileOperation` Copy
  (`shell_file_ops` の同期の内部関数を worker から呼ぶ) に替える。同名の衝突・進捗・取消・エラーは Windows の画面になる。
  結果は今の `CopyOutcome` に写す (中断 → 「中断しました。処理済みの項目は元に戻りません」、失敗 → 全件失敗の通知)。PowerShell の経路は削除する。
- フォルダ: Q5 の回答待ち。既定 (a) は今どおり skip して通知、(b) なら skip をやめる (`partition_dropped_paths` を通さない)。
- mIV 内のドラッグが一覧に落ちても何もしない (§4.4) のは今と同じ結果。
- 更新履歴の文案: 「エクスプローラーから一覧へファイルをドロップしたとき、同じ名前のファイルがあると確認せずに上書きしていたのを、
  Windows の確認画面 (置き換え・スキップなど) を出すようにしました。」

## 9. 長い処理・取り消し・衝突・完了後の更新

- **モーダルにしない**。整理先 §5.1 と同じ判断 (転送先は表示中の viewer 状態に依存せず、完了後は外部で動かした場合と同じ既存の更新に任せる。
  長いコピー中も閲覧を止めない。進捗・取消は Windows の画面)。組み合わせは「整理先とツリーへのドロップで同時に 1 本」で減らす。
- **取り消し**: 整理先と同じ。mIV の Ctrl+Z では戻さない。Windows の取り消し履歴に載る (エクスプローラーで Ctrl+Z)。§1.821 のコピーも同じフラグなので同じ。
- **同名の衝突**: Windows の画面。mIV は自動上書き・自動改名をしない。
- **評価・タグ・編集**: 整理先と同じく移さない (先の通常フォルダで既存の「編集内容の復元」)。
- 完了後 (`poll_file_organize` の共有の完了処理):
  1. 一覧: 今どおり、現在の実フォルダが元の親か先なら `Notified` を 1 回。
  2. ツリー (追加): 先のフォルダと元の親ごとに `folder_pane.observe_accepted_folder(path)` (path 自身の直下を再確認)。§1.686 の消費側に
     「main の転送の完了」という producer を足す (folder-tree-refresh-plan に追記)。整理先の完了にも効く。
  3. サムネイル・キャッシュ・タグ: 専用の後始末なし (整理先と同じ、外部で動かした場合と同じ)。
- まれな失敗は既定の割り切り (ログ + 通知)。

## 10. テストと実機確認

自動テスト (製品バイナリは起動しない):

- 操作の決定 (純関数): 修飾キー × 同じ/別ドライブ × 内部/外部 × 外部の許可効果 × 同じ親・自身/配下・本棚配下・`excluded_virtual`・`failed_paths`。UNC・大文字小文字・末尾区切り。
- 当たり判定: 形 `drawn` / `presented` の選び分け (`in_update`)、**表示済みの形で A の位置、update 中の新しい形では B が同じ位置にある場合に A を返す**回帰、
  ppp 1.0/1.5、行全体の rect の端、`TreeNonTarget` (インデント・空白・スクロールバー・隠れた行) が NONE で一覧へ流れないこと、大きさ・DPI 不一致で NONE。
- 受付の述語: snapshot と投入で同じ述語。Running・Selecting・Remote・終了・モーダル・sidecar・本棚で worker を起動しない。整理先とツリーの両方の入口。snapshot の作成で本への追加の取り消しが失効しないこと。
- 外部ドロップの返答: 偽の data object で、ツリー行への外部 Drop が COPY を返し、`PERFORMEDDROPEFFECT` 等を書かないこと。Shift でもコピーの要求になること。
- 内部ドロップ: `InternalDrag.paths` 全件で要求を作ること (data object の `CF_HDROP` を読まない)。`failed_paths > 0` で受け付けない。
- 一覧への落下: `GridFiles` が `handle_external_file_drop` に届き、内部ドラッグでは届かないこと。sidecar 復元中は捨てること。
- 完了後: 先と元の親に `observe_accepted_folder`、現在フォルダの `Notified` は従来どおり 1 回。
- 右クリック: 「貼り付け」の有効/無効、worker で verb が呼ばれること (呼び出しは差し替え可能な形で検査)、現在フォルダのときだけ選択要求、失敗で取消。
- §1.821: worker が Shell 転送を使い、`CopyOutcome` へ正しく写すこと (中断・失敗)。実 Shell は既存の `#[ignore]` テストの形で 1 件。

実機確認 (サブ PC、使い捨てデータ):

1. 一覧から同じドライブのツリー行へドラッグ → 移動 (既定)。Ctrl でコピー、Shift で移動、別ドライブは既定でコピー。**ツリー行の上で「フォルダ名 へ移動」の
   バッジが出て、＋のカーソルにならないこと (出荷条件)**。ツリー行を離れると今と同じ見た目に戻る。エクスプローラーへのドラッグは今どおりコピー。
2. エクスプローラーからツリー行へドラッグ → コピー (Shift でもコピー)。元は残る。
3. 同名の衝突で Windows の画面 (置換・スキップ)。大きなコピーを取消。エクスプローラーで Ctrl+Z して戻る。
4. エクスプローラーから一覧へドロップ: 同名があると Windows の確認画面が出る (§1.821)。mIV 内で一覧の上に落としても何も起きず、＋も出ない。
5. ツリー行の右クリック → 貼り付け (エクスプローラーでコピー/切り取りしたファイル)。切り取りは元が消える。
6. 一覧とツリーの更新: 元フォルダ表示中にフォルダを動かすと一覧とツリーの両方から消え、展開済みの先の行の下に出る。
7. ツリーのスクロール直後・窓のリサイズ直後に落としても、見えている行に入る (ずれた行に入らない)。

## 11. 未回答 (統括経由で利用者へ)

- **Q4 外部からツリーへのドロップはコピーだけ (Shift でもコピー)** [推奨]。理由は §5。移動はエクスプローラー側で行う。
  ほかの案: 外部からの移動も受ける代わりに、移動が終わるまで mIV の画面が止まる。
- **Q5 §1.821 でフォルダのドロップも受けるか**: (a) 今どおりファイルだけ [推奨] / (b) 受ける。

## 12. 文書の更新 (実装時)

- マニュアル: `grid.html` (ツリーの節)・`tut-file-ops.html`。修飾キーと「同じドライブ」、外部からはコピーだけ、行を先に表示しておくこと
  (ドラッグ中の自動展開・自動スクロールはしない)、取り消しはエクスプローラー、評価・タグは先で「編集内容の復元」、一覧へのドロップの確認画面。
- 設計: 本書、[file-drag-drop-design.md](file-drag-drop-design.md) (§5〜6 IDropSource と drag image、§11 受け取りの drop target と Shell 転送、
  `internal_drop_pending` の撤去)、[folder-tree-refresh-plan.md](folder-tree-refresh-plan.md) (転送完了の producer)、
  [file-organize-destinations-plan.md](file-organize-destinations-plan.md) (受付の述語と owner の共有)、`docs/README.md`、`docs/spec.md`、
  [ui-responsiveness.md](ui-responsiveness.md) (貼り付けの worker 化)。
- キー操作は追加しない (右ボタン・D&D・修飾キーは固定入力。`docs/keymap-spec.md` の固定入力に理由を書く)。
