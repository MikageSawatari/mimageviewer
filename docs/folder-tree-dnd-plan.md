# §1.464 フォルダツリーへのドラッグ&ドロップ・右クリック貼り付け / §1.821 一覧へのドロップの上書き — 設計

2026-11-19〜20 設計 (ClaudeCode)。worktree `G:\home\mimageviewer-treednd464`、branch `treednd-464`、基点 master `ca6686d93`。
要件の正本は [next-release-backlog.md §1.464・§1.821](next-release-backlog.md)。
**状態: 設計 r5 (利用者から独立レビュー ACCEPT の報告あり)。A は `6f91f6058`。B は §14・§16 の判断を反映して実装。検証・引き渡しの記録は §15。コンパイル・テストは利用者指示により設計担当へ引き継ぐ。**

- 2026-11-22: §1.858 A 中断状態の常時照会、元の HRESULT と項目別ログ、処理済み件数による通知を追加。

要望の要旨 (5ch): >>444 ツリーへファイルをドロップできず、右クリックの貼り付けもできない。>>707 読み終えたファイルを
`_mita` フォルダへ D&D したい (内部ドラッグで「＋」カーソルなのに何も起きない)。固定先へ送る「ファイル整理先」(v4.4.0 出荷)
はこの要望の代替扱いにしない (backlog の記録)。

## 0. 利用者の決定 (2026-11-19〜20、統括経由)

- Q1 修飾キーなしでツリーへ落としたときの既定は Explorer と同じ (同じドライブは移動、別のドライブはコピー。Ctrl でコピー、Shift で移動)。
- Q2 mIV の一覧から mIV の外へのドラッグは今までどおりコピーだけ。
- Q3 ツリー行の右クリックは「貼り付け」の 1 項目だけ。
- §1.821 一覧へのドロップが同名を確認なしで上書きする件は v5.5.0 で一緒に直す。ツリーと同じ経路 (IFileOperation、競合の確認は Windows の画面) に揃える。コミットは §1.464 と分ける。
- ドラッグ中の自動展開・自動スクロールはしない (報告として了解)。
- Q4 外部 (エクスプローラー等) からツリーへのドロップはコピーだけ (Shift でもコピー)。マニュアルに「外から落としたときはコピーになる」と書く (2026-11-20)。
- Q5 §1.821 でフォルダのドロップも受け付ける (ツリーへのドロップと揃える)。更新履歴に「一覧へフォルダもドロップできるようにした」と「同じ名前の上書きの前に Windows の確認が出る」を書き、file-drag-drop-design §11.1 を直す (2026-11-20)。
- 期限内に入りきらなければ、外部からのドロップを後回しにする分け方でよい (統括 2026-11-20)。

## 1. 結論 (推奨構成)

1. **転送の実行は既存の整理先と同じ経路**。ツリー行へのドロップは `ShellTransferRequest` を作り、整理先の単一 request owner
   `FileOrganizeRequest::Running` へ入れる。検証・`IFileOperation`・完了処理 (`transfer_items_async` → `poll_file_organize`) を共有する。
2. **ドロップを受けるために ROOT の OLE drop target を mIV 自前の 1 個に置き換える** (winit は位置・修飾キーを渡さず常に COPY と答えるため)。
   一覧への落下は、新しい drop target から今の `handle_external_file_drop` へ渡す。
3. **mIV の一覧からツリーへ** (§4): Q1 の規則で移動/コピー。送出の許可効果は COPY のまま (Q2)。転送は mIV が `IFileOperation` で行い、
   OLE には COPY を返す。操作の表示は、Shell 既定の IDropSource のまま、mIV の target が書くドロップの説明 (drag image のバッジ) で出す。
4. **外部 (エクスプローラー等) からツリーへ** (§5): **コピーだけ** (Q4)。外部の元に対して移動の完了を正しく知らせるには、
   Drop の中で同期に移動する (UI が止まる) か、元が非同期転送に対応している必要がある。元ごとに分岐させず、今の一覧への落下と同じ
   「COPY を返し、後で worker がコピー」の約束事に留める。
5. **ツリー行の右クリック「貼り付け」**は、Ctrl+V と同じ関数 `invoke_shell_folder_background_verb(hwnd, folder, Paste)` を、
   Ctrl+V と同じく UI スレッドで、その行のフォルダへ呼ぶ (§7)。
6. **§1.821**: 一覧への落下の worker の中身を PowerShell `Copy-Item -Force` から整理先と同じ Shell 転送 (Copy) に替え、フォルダも受ける (§8)。
7. 処理中はモーダルにしない (整理先 §5.1 と同じ判断、§9)。

## 2. 現状のコード事実

以下は設計時の調査記録。A (`6f91f6058`) と B の更新後の経路・検証状況は §8・§15 を参照する。

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

### 4.1 元パスの確定 (r1 P2-8、r2 P3-9)

ドラッグ開始時 (`ui_main.rs:3268` の payload 決定) の全パスと仮想項目の除外の有無、そして `start_file_drag` の中で
`shell_data_object_for_paths` が返す `failed_paths` (PIDL 化に失敗した件数、`file_drag.rs:220`) を、**`SHDoDragDrop` を呼ぶ前に**
`InternalDrag { paths, excluded_virtual, failed_paths }` として共有状態 (§6.2) へ置く (`start_file_drag` に共有状態への書き込み口を渡す)。
**ツリーへの転送は `InternalDrag.paths` 全件を使う** (data object の縮んだ `CF_HDROP` は使わない)。`excluded_virtual` か
`failed_paths > 0` ならツリーでは受け付けない (整理先と同じ「対象外が混ざれば全体を実行しない」)。
外部へのドラッグの既存の挙動 (実項目だけ送り、除外を後で知らせる) は変えない。

### 4.2 操作の決め方 (Q1、r1 P2-7・r2 P2-8)

| 押しているもの | 操作の要求 |
| --- | --- |
| Shift | 移動 (明示) |
| Ctrl | コピー (明示) |
| なし | 既定 (ルートの予告で移動かコピー。下の説明) |
| Ctrl+Shift・Alt | 受け付けない (ショートカット作成はしない) |

ドラッグ中 (UI スレッド、I/O なし) は、**パスのルート (ドライブ文字、UNC は `\\server\share`) を大文字小文字無視で比べた予告**で表示する:
元が全部、先と同じルートなら「移動」、1 件でも違えば「コピー」。Drop で、この**予告した操作を要求に入れる** (`ShellTransferRequest.operation`)。

worker は**予告を上げない** (r3 P1-1)。予告がコピーなら必ずコピー。予告が移動で、それが既定 (修飾キーなし) によるものなら、検証の前に元と先の
ボリュームを `GetVolumePathNameW` → `GetVolumeNameForVolumeMountPointW` (取れなければ `GetVolumePathNameW` の結果。UNC は共有のルート) で比べ、
1 件でも違えば (または求められなければ) **コピーに下げる** (フォルダにマウントした別ボリュームで、Explorer と同じくコピーになる)。
Shift の明示の移動は下げない。要求は `operation` と `default_move_needs_volume_check: bool` を持つ (既定の移動のときだけ true)。

結果として、実行が移動になるのは「予告が移動」のときだけで、表示と実行の食い違いは「予告は移動、実際はコピー」の向きに限られる
(元が消える側には転ばない)。同じボリュームを別の名前 (`D:\` と `C:\MountD\`) で見ている場合、Explorer は既定で移動するが mIV はコピーになる
(安全側の違いとしてマニュアルに書かない細部。設計書にだけ残す)。送出元は mIV 自身なので、preferred effect や元の許可効果による分岐は無い。

受け付けない場面: 行の上でない / 明示の移動か既定で、先が元の親と同じ (字句。Explorer も同じフォルダへの既定のドロップでは何もしない) /
元フォルダ自身か配下 / §6.4 の述語 / §4.1。
本棚の配下はドラッグ中の判定に含めず、投入時に拒否する (§6.4・§16)。

### 4.3 OLE への返答と表示 (r1 P2-6・r2 P2-5)

送出の `dwOKEffects` は COPY のまま (Q2)。ツリー行の上では mIV の target は OLE へ COPY を返し、Drop で `InternalDrag.paths` から
`ShellTransferRequest` を作る (data object からは読まない)。送出元は mIV なので COPY と答えても削除は起きない。

**IDropSource は今の Shell 既定のまま** (`SHDoDragDrop` の `pdsrc = None`、generic image も今のまま。file-drag-drop-design §12 の async wrapper の
扱いも変えない)。表示は target 側だけで行う (Microsoft の DragDropVisuals サンプルと同じ形):

- mIV の target は `IDropTargetHelper` (`CLSID_DragDropHelper`) の DragEnter/Over/Leave/Drop を呼ぶ (自分の窓の上に drag image を描かせる)。
- ツリー行の上では data object へ `CFSTR_DROPDESCRIPTION` を `SetData` する: 予想が移動なら `DROPIMAGE_MOVE` + 「%1 へ移動」、
  コピーなら `DROPIMAGE_COPY` + 「%1 へコピー」、受け付けないなら `DROPIMAGE_NONE`。%1 はフォルダ名。変わったときだけ書く。
  行を離れて落とせない場所 (一覧の空白・ツリーの行以外・同じ一覧など) へ動いたら `DROPIMAGE_NONE` (§1.844。`INVALID` では
  Shell の既定の「＋ コピー」が出ることを §1.836 のサブ PC 確認で観測)。`DROPIMAGE_INVALID` は、受け付ける外部からの一覧への
  ドロップと、DragLeave・Drop の後のリセット (古い説明を残さない) だけに使う。正本の詳細は file-drag-drop-design §12.3.1。
- 送出側は何も足さない (`IDragSourceHelper2::SetFlags` は helper 自身の設定で data object と結び付かないため使わない。§14)。
  Shell の generic image のまま、target が書く説明のバッジ (type) で移動かコピーかを示す。テキストが出るかは実機確認で見る。

**出荷条件**: 実機確認 (§10 の 1) で「ツリー行の上の移動が、移動のバッジ (またはカーソル) として見え、コピーの＋だけにはならない」。
満たさなければ、mIV の一覧からツリーへの**既定と Shift の移動を出荷せず** (どちらもコピーの扱いにする)、統括へ判断を返す。
custom IDropSource は作らない (r2 P2-5: 画像の表示状態を source が正しく知る手段が無く、通常のカーソルを壊し得る)。

### 4.4 一覧の上・ツリーの空白

mIV 内のドラッグは、ツリーの行以外 (一覧・ツリーの空白) では NONE を返す (今の「＋なのに何も起きない」をやめる)。
`internal_drop_pending` / `cursor_over_window` の推定は不要になるので外す。

## 5. 外部からツリーへ (コピーだけ、Q4)

外部の元 (`InternalDrag` が無いドラッグ) がツリー行の上に来たら:

- 元の許可効果に COPY があり、`CF_HDROP` があれば COPY。無ければ NONE。修飾キー・preferred effect に関わらずコピー (Shift でもコピー)。
  説明は「%1 へコピー」(drag image は元が用意していれば表示される)。
- Drop: `CF_HDROP` のパスで `ShellTransferRequest { operation: Copy, default_move_needs_volume_check: false, ... }` を作り、**COPY を返す**。`PERFORMEDDROPEFFECT` 等は書かない。
  今の一覧への落下 (winit が COPY を返し、mIV が後でコピーする) と同じ約束事で、元に削除を頼まない。元が一時ファイルを Drop 後に消す
  アプリでは、後のコピーが「見つからない」で失敗する (失われるものは無い。一覧への落下も同じ)。
- 外部からの移動を正しく受けるには、Drop の中で同期に移動する (UI が止まる) か、元が非同期転送に対応している場合だけ完了を後で返す
  必要がある (file-drag-drop-design §12.2 の観測は mIV の生成経路での結果で、外部の元一般の性質ではない)。元ごとに分岐させず、初版では受けない (Q4)。

## 6. ROOT の drop target

### 6.1 置き換え (r1: 構造上の衝突なしと確認済み)

ROOT の HWND を得た最初のフレームで `RevokeDragDrop(hwnd)` → `RegisterDragDrop(hwnd, mIV の target)`。OLE の初期化は winit が済ませている。
winit の WM_DESTROY の `RevokeDragDrop` が mIV の target も外す。**登録に失敗したらログ + 通知** (ドロップを受けられない旨) で終え、成功したふりをしない。
detached viewer・フルスクリーン等の他の viewport の drop target は変えない (detached 述語・viewport 経路に触れない)。
COM object は `#[implement(IDropTarget, Agile = false)]`。

### 6.2 App との受け渡し

callback は UI スレッドで (a) winit のメッセージ処理の中 (外部ドラッグ)、(b) `SHDoDragDrop` の中 (mIV 内ドラッグ、App::update の中)、
(c) native メニューや Shell の画面のメッセージループの中、に呼ばれ得る。**callback は App に触れない**。
`Rc<RefCell<MainDropShared>>` だけを介し、**RefCell の借用を COM 呼び出し・メッセージループをまたいで持たない**。

`MainDropShared`:

- `geometries: VecDeque<(u64, DropGeometry)>` — App が**各 pass の最後**に、その pass の形 (§6.3) を **pass の番号**
  (`ctx.cumulative_pass_nr() + 1` = その pass が終わった後の完了 pass 数) 付きで積む。表示済みの番号より古いものは捨てる (残すのは数個)。
- **表示済みの pass の番号は描画側から受け取る** (r3 P1-2・r4 P1-1): vendor の egui-wgpu (`winit.rs` の `render_state.queue.present(output_frame)` の直後、
  既存の root present observer と同じ位置) で、ROOT の present が実際に返ったときだけ、`ctx.cumulative_pass_nr_for(ROOT)` (= 描いた最後の pass の番号) を
  ctx の temp data に記録する。公開 helper `egui_wgpu::last_presented_pass(ctx, viewport) -> Option<u64>` で読む。Surface の `Outdated`・skip・capture だけの
  ときは記録しない (present していないので)。パッチは `vendor/egui-wgpu/PATCHES.md` に 1 行足す。
- callback は `last_presented_pass` の番号の形だけを使う。その番号の形が無い (まだ 1 度も描いていない・古すぎる) ときは NONE。
  画面に出ている形そのものなので、`in_pass` や 2 つの形の一致は要らない。callback から ctx を読むのは `ctx.data` の 1 回だけで、
  App の pass 中 (mIV 内ドラッグ) でも ctx のロックは他に持たれていない (closure の中で ctx を入れ子に使わない)。
- `admission: Result<(), String>` — App が毎 pass 書く受付可否 (§6.4 の述語の結果)。
- `internal: Option<InternalDrag>` — §4.1。`SHDoDragDrop` の前に立て、戻った直後に外す。
- `drops: Vec<DropRequest>` — target が書く。`GridFiles(paths)` / `TreeTransfer(ShellTransferRequest)`。外部ドラッグなら `request_repaint`。

App の消費: `update_phases.rs` の今の `dropped_files` の場所で `drops` を取り出す。mIV 内ドラッグは `post_render_native_drag` が
`SHDoDragDrop` から戻った直後に同じ処理を呼ぶ。`GridFiles` は `handle_external_file_drop` (sidecar 復元中は今と同じく捨てる)、
`TreeTransfer` は §6.4 の投入境界へ。

### 6.3 形と当たり判定 (r1 P1-1・P1-2、r2 P1-1・P2-7)

`DropGeometry` は、その pass の `label_display` から作る:

- `pane_rect` (ツリーのパネル全体、ヘッダを含む)、`tree_clip` (ScrollArea の `inner_rect`)。
- 行ごとに**行全体の rect** (`draw_folder_pane_row` が返す row_rect (`ui_folder_pane.rs:938`) を `tree_clip` で切ったもの) とパス。
  `FolderPaneLabelDisplay` に row_rect も記録する。
  row_rect は開閉矢印から末尾 widget までを覆い、その左の深さ用インデントは含めない (表示・行高は変えず、インデントは下表の `TreeNonTarget`)。
- **上に重なるレイヤーとの交差で行を外す**: その pass で見えているレイヤーのうち、ツリーのレイヤーより手前にあるもの (popup・メニュー・モーダル・
  ツールチップ・他の Area) の rect を集め、交差する行は対象外 (行全体を外す。点のサンプリングはしない)。
- `client_size_px`、DPI (`GetDpiForWindow`)、`pixels_per_point`。

callback: `POINTL` → `ScreenToClient` → `/ pixels_per_point`。**今の `GetClientRect` の大きさと DPI が表示済みの形と違えば、どこでも NONE**。点の分類:

| 分類 | 扱い |
| --- | --- |
| `TreeRow(path)` (`tree_clip` 内で、対象の行全体の rect に入る) | §4・§5 |
| `TreeNonTarget` (`pane_rect` 内でそれ以外: 空白・インデント・スクロールバー・外した行・ヘッダ) | NONE。一覧の経路へ流さない |
| `ExistingDropArea` (`pane_rect` の外) | 外部ドラッグは今の一覧への落下 (COPY、Drop で `GridFiles`)。mIV 内ドラッグは NONE |
| 形が無い (ツリー非表示) | `pane_rect` が空なので全部 `ExistingDropArea`。大きさ・DPI 不一致は全部 NONE |

### 6.4 受付の述語と投入境界 (r1 P2-4)

副作用の無い述語 `shell_transfer_admission(&self, ctx) -> Result<(), String>` を 1 つ作り、`admission` と投入時の両方で使う。
見るもの (本棚の配下は §16 により投入の境界でだけ見る): `FileOrganizeRequest` が `Hidden` (整理先の選択画面 `Selecting` もモーダルとして拒否、`Running` は消さない)、
`remote_session_blocks_local_control()`、終了要求 (shutdown・tray quit・root close)、`main_hwnd`、`viewer_mutation_modal_active()`、
`sidecar_restore_active()`、`common_modal_dialog_open`、`folder_pane_disabled()`。本棚パスの解決は含めない。

ツリーへの投入 `submit_shell_transfer(request)` は述語を**もう一度**見て、次に `book_root_path()` を一回呼んで先が本棚自身・配下でないことを確かめる。
本棚なら「本棚の中へは移動・コピーできません」と通知して何もしない。通ったときだけ `admit_book_append_followup_mutation()`
→ `transfer_items_async` → `Running`。通らなければ理由を通知して何もしない。整理先は今の `take_for_submission` で `Selecting` を一度消費してから、
同じ述語 (Selecting の判定を除いた部分) と投入を使う (条件を 2 か所に書かない)。`admission` の計算では `admit_book_append_followup_mutation` を呼ばない。
行の形・ドラッグ中のバッジ・メニューの有効判定は本棚も通常の行と同じ。本棚の拒否は入力を確定した境界に限るため、既定パスの同期解決を毎 pass に増やさない。

「同時に 1 本」は整理先とツリーへのドロップの間の保証。一覧への落下 (§8) とツリーの貼り付け (§7) は今の Ctrl+V・一覧への落下と同じく並行し得る
(全操作を横断する排他は作らない)。

## 7. 右クリック「貼り付け」(Q3、r1 P2-5・r2 P1-2/P1-3/P2-4/P2-6)

**Ctrl+V と同じ関数を、Ctrl+V と同じく UI スレッドで同期に呼ぶ**。r2 で worker 化に伴う問題 (クリック時と別のクリップボードの中身を貼り得る、
切り取りの data object の apartment、worker の OLE 初期化、選択要求の取り違え) が出たため、worker 化をやめて組み合わせごと消す。

- ツリー行の右クリックで egui のメニュー (`os_theme` の helper) に「貼り付け」1 項目。行は右クリックした時点のパスで確定する。
  有効なのは、クリップボードにファイルがある (`IsClipboardFormatAvailable(CF_HDROP)`、メニューを開いたとき 1 回) かつ §6.4 の述語が通るとき。
  無効の理由はツールチップで示す。画像だけのクリップボードは対象外。
- 実行: 今の Ctrl+V の処理 (`app.rs:64139` 付近の「ファイルなら」の枝) を、貼り付け先のフォルダを引数に取る 1 つの関数に切り出し、
  Ctrl+V とツリーの両方から呼ぶ: (ツリーは述語を再確認し、`book_root_path()` を一回呼んで本棚自身・配下なら上記文言で拒否) → `admit_book_append_followup_mutation()` → 先が現在の実フォルダのときだけ
  `request_post_operation_selection_for_added_items(folder)` → `invoke_shell_folder_background_verb(hwnd, folder, Paste)` →
  `resync_egui_modifiers_from_os` → 失敗ならログと、**この呼び出しで選択要求を作ったときだけ** `cancel_post_operation_selection`。
  同期なので、間に別の選択要求は入らない。
- 応答性: Shell の bind・`InvokeCommand` は UI スレッドで行う (Ctrl+V と同じ)。ツリーの行は列挙できたフォルダだが、その後に切断された
  ネットワーク先では OS が諦めるまで UI が止まり得る。整理先を開く同期選択 (file-organize-destinations-plan §13、利用者了承) と同じ既知の例外として
  ui-responsiveness §4 に記録する。転送そのものは Shell が持つ (戻り値は verb 呼び出しの成否)。
- Ctrl+V 自体の挙動は変えない (関数の切り出しだけ)。
  Ctrl+V は既存の `current_favorite_target()` が操作時に本棚を除外済みなので、共通貼り付け関数でその解決を繰り返さない。

## 8. §1.821 一覧へのドロップ (Q5 = フォルダも受ける)

- 原因 (コード): `copy_paths_into_folder` (`context_menu.rs:3321`、呼び出しは `app.rs:52665` の 1 か所) が `Copy-Item ... -Force` で、
  同名のファイルを確認なしに上書きする。
- 直し方: `handle_external_file_drop` の受付 (検索結果・タグ表示中・ZIP/PDF 表示中・本棚配下の拒否、`admit_book_append_followup_mutation`) と
  `file-drop-validate` worker・`drop_copy_pending` の並行受信はそのまま。worker の中のコピーを、整理先と同じ検証 + `IFileOperation` Copy
  (`shell_file_ops` の同期関数を worker から呼ぶ。`operation: Copy`) に替える。同名の衝突・進捗・取消・エラーは Windows の画面。
  結果は `CopyOutcome` に写す (実行後の中断・失敗は §8.1 の共通 notice、実行前の `Err` は従来の全件失敗通知)。
  `copy_paths_into_folder` と PowerShell の経路は削除する。
- **フォルダも受ける (Q5)**: `partition_dropped_paths` によるフォルダの skip と「フォルダのドロップは現在無効です」の通知をやめる。
  フォルダを自身や配下へコピーする組み合わせは `validate_transfer_request` が全体を拒否する (未使用の `dir_copy_would_recurse` は削除)。
  file-drag-drop-design §11.1 の「v1.1.0 で無効化」の記述を直す。
- mIV 内のドラッグが一覧に落ちても何もしない (§4.4) のは今と同じ結果。
- 更新履歴の文案: 「エクスプローラーから一覧へファイルをドロップしたとき、同じ名前のファイルがあると確認せずに上書きしていたのを、
  Windows の確認画面 (置き換え・スキップなど) を出すようにしました。あわせて、一覧へフォルダもドロップできるようにしました。」

実装 A: `shell_file_ops::transfer_items_blocking` に検証と STA 転送を集約し、`transfer_items_async` も共有する。
`handle_external_file_drop` の worker は `main_hwnd` と全パスを `file_drag::copy_dropped_paths_blocking` へ渡し、結果を `CopyOutcome` に写す。
旧 PowerShell コピー・フォルダ skip・未使用の再帰判定を削除。既存の受付・並行受信・`pending_reload` は維持する。

### 8.1 §1.858 A Shell 転送・名前変更の終端契約

予約が全件成功したら `PerformOperations` を一回実行し、成功・失敗にかかわらず直後に `GetAnyOperationsAborted` を必ず呼ぶ。
実行の元の HRESULT と、中断照会の `Result<bool, HRESULT>` は独立して保持する。照会失敗を false にせず、実行のエラーも上書きしない。
検証・COM 構築・項目生成・予約の失敗は従来どおり `Err(String)` で実行しない。

| 分類 | 条件 (上から優先) |
| --- | --- |
| Interrupted | 中断照会が `Ok(true)`、または実行 HRESULT が `0x80270000` / `0x800704C7` |
| Completed | 実行が成功 HRESULT かつ中断照会が `Ok(false)` |
| Failed | 上記以外 (中断照会失敗も含む) |

Interrupted と別の実行失敗が同時にある場合、その HRESULT も表示する。中断の原因は利用者・Shell の画面・システムのいずれとも断定しない。
名前変更の新旧パスの事後確認と移行は Completed の場合だけ行う。操作、予約の時機、検証、通常の
`FOF_ALLOWUNDO | FOFX_ADDUNDORECORD` は維持し、自動再試行・rollback は追加しない。

`MoveItem / CopyItem / RenameItem` に同じ操作の `IFileOperationProgressSink` を渡す。Advise の登録・解除状態を増やさず、
`PostMoveItem / PostCopyItem / PostRenameItem` で操作種別、元の parsing name、先フォルダ、新名、HRESULT、作成後の parsing name を記録する。
callback は失敗の HRESULT を受けても常に S_OK を返す。フォルダの子の callback も全件集計し、詳細は先頭50件と省略数に限定する。
`COPYENGINE_S_PENDING` (`0x0027000B`) は未処理の途中報告であり、ログ用 pending だけを増やし、確定結果の processed / failed / cancelled / skipped には数えない。
後続の同じ元の確定 callback で一件を数える。確定報告が来なければ未処理として数えない。元パスをキーにした管理表は作らず、途中報告も詳細行へ残す。
`COPYENGINE_S_NOT_HANDLED` (`0x00270003`) / `COPYENGINE_S_MERGE` (`0x00270006`) も結合開始の途中報告としてログ用 merged のみを増やす。
作成後 item が既存の先フォルダを指していても、親を processed / skipped に数えず、子の callback で結果を数える。
根拠は [Wine の shlfileop.c `notifications6`](https://github.com/wine-mirror/wine/blob/master/dlls/shell32/tests/shlfileop.c) が示す親の報告→子の処理の順序。
中断 HRESULT (`0x80270000` / `0x800704C7`) は cancelled、その他の失敗 HRESULT は failed、その他の成功 HRESULT は
callback の作成後 item (`psiNewlyCreated`) が非 null なら processed、null なら skipped に数える。
parsing name の取得成否ではなく item の有無を使い、詳細上限を超えた callback と名前変更も同じ規則で数える。
最初の実失敗 (中断コードを除く) HRESULT は詳細上限に関係なく `first_failed_hresult` に保持する。
省略数は確定結果の合計ではなく、pending / merged も含む callback 総数から詳細保持数を引いて求める。
2026-11-22 設計担当の Windows 11 / NTFS 実 Shell 観測では、完了した同一ボリューム Move が
`COPYENGINE_S_DONT_PROCESS_CHILDREN` (`0x00270008`) と作成後 item を返したため、成功コードの列挙で処理済みを判定しない。
また、使用中フォルダの Move は試験用 UI 抑止時に実行 HRESULT が S_OK、中断照会が `Ok(true)`、項目 callback が失敗だった。
確定結果は確定 callback の件数、pending / merged は途中報告の件数であり、要求した source 件数とは限らない。まれな記録失敗はログで終え、回復機構は作らない。

2026-11-23 sub-PC2 の実アプリ確認 (c4db2bbd3) では、Windows の使用中画面を閉じる操作の後、同じ元の pending → 中断の2報告を観測した。
pending を確定結果から除き、中断を実失敗と分ける。名前変更も同じ規則。

結合時の本機の生 callback 列は ignored の `shell_op_858_real_merge_into_existing_folder_records_callbacks` で確認する。
正常 Move、子ファイルを FILE_SHARE_READ だけで開いた Move、正常 Copy、空フォルダ Move の4ケースを出力し、特定の結合コードの出現は要求しない。
2026-11-23 に開発機 (Windows 11、NTFS、同じボリューム、エラー画面を出さない試験用フラグ) で流した結果 (設計担当が run-heavy で実行、製品は起動していない):
- 正常 Move: 親 `m` が `0x00270003` + 先 item あり、子 `a.txt` が `0x00270008` + 先 item あり → merged=1、processed=1。
- 子を開いたままの Move: 親 `m` が `0x80270027` + 先 item なしの 1 報告だけ → failed=1、processed=0、中断状態 true。
- 正常 Copy: 親 `m` も子 `a.txt` も `0x00000000` + 先 item あり → processed=2。
- 空フォルダ `e` を既存の `dst\e` へ Move: `e` が `0x00270008` + 先 item ありの 1 報告 → processed=1、元 `src\e` は消えた。結合コードは出なかった。

残る限界: Copy の結合では、既存の親フォルダも `S_OK` + 先 item で報告されるため、新しく作ったフォルダと区別できず processed に数える。
子のコピーが止まった場合は、何も増えていなくても「一部の項目は処理済みです。フォルダの内容を確認してください」になり得る。
これは「確認を促す」側への誤りなので、追加の判定 (フォルダの事前存在の確認など) は足さない。逆向きの誤り (処理済みがあるのに「ありません」) は作らない。

空フォルダの Move 結合は、子の callback がなくても元フォルダが削除され得るため、merged のみでも「移動・コピーした項目はありません」とは断定しない。
空フォルダの追加ケース (d) は、元 `src\e` の残存／消失を生 callback とともに出力し、どちらも許容する。件数は failed=0 だけを要求する。
開発機では上記のとおり結合コードが出なかったが、別の環境 (Wine の実装が示す経路) で merged のみになる場合に備えて、中立の通知を残す。

ログ形式 (`prefix` は `shell_transfer` / `shell_rename`、id はプロセス内共通の連番、文字列は Debug 表記):
項目行は終端でまとめて書くため、ログの時刻は終端時刻であり callback の発生時刻ではない。

```text
{prefix}: start id={id} requested={Copy|Move|Rename} resolved={Copy|Move|Rename} sources={count} destination={path:?}
{prefix}: item id={id} kind={Copy|Move|Rename} source={Option<String>:?} destination={Option<String>:?} new_name={Option<String>:?} hr=0x{HRESULT:08X} created={Option<String>:?}
{prefix}: item id={id} ... and {omitted} more
{prefix}: end id={id} classification={Completed|Interrupted|Failed} perform=0x{HRESULT:08X} abort={Ok(true)|Ok(false)|Err(0xHHHHHHHH)} processed={count} failed={count} cancelled={count} skipped={count} pending={count} merged={count} first_failed_hresult={0xHHHHHHHH|None}
```

開始行は全件予約後、実行直前に出す。requested は volume check 前、resolved は後の操作。名前変更は両方 Rename、先は元の親。
詳細と終端は中断照会後に出す。UI は以下の短い共通通知を使う (Completed は通知なし)。

- Interrupted、処理済みあり: 「操作が中断されました。一部の項目は処理済みです。フォルダの内容を確認してください」
- Interrupted、処理済みなし・merged あり (名前変更以外): 「操作が中断されました。フォルダの内容を確認してください」
- Interrupted、処理済みなし・merged なし: 「操作が中断されました。移動・コピーした項目はありません」。名前変更は merged の有無にかかわらず「名前の変更は中断されました」。
- Interrupted、実失敗項目あり (`failed > 0`): 2行目に `失敗した項目: {n} 件 (Shell: 0x{hr:08X})` (最初の実失敗 HRESULT) を追加する。
  中断コードだけなら見出しのみで、pending / merged / cancelled の件数や失敗行を出さない。
- Failed: 「操作を完了できませんでした。フォルダの内容を確認してください」。処理済みありなら同じ行に「一部の項目は処理済みです」を続ける。
  次行は `Shell: 0xHHHHHHHH、中断状態: 中断あり / 中断なし / 照会失敗 0xHHHHHHHH`。
  最初の実失敗 HRESULT が実行 HRESULT と異なる場合は `、項目: 0xHHHHHHHH` を理由行の末尾へ追加する。中断コードはこの項目理由に使わない。
  Interrupted でも非 cancel の実行失敗または照会失敗があれば、この理由行を併記する (項目失敗行があればその後)。

整理先・ツリーは全結果で従来の tree observe + 現在実フォルダの再確認、外部から一覧へのコピーは全結果で従来の `pending_reload` を維持する。
実行後の結果を「全要求が成功／失敗」と推測して `CopyOutcome` の要求件数へ当てはめず、共通 notice で知らせる。

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

- 操作の決定 (純関数): 修飾キー × ルートの同じ/別 × 内部/外部 × 外部の許可効果 × 同じ親・自身/配下・`excluded_virtual`・`failed_paths`。UNC・大文字小文字・末尾区切り。本棚は操作確定の境界でのみ拒否する。
- 既定の解決 (worker): 予告がコピーなら必ずコピー (r3 の反例 `D:\src` → `C:\MountD\dst` で同じボリュームでもコピー)。予告が既定の移動なら、ボリューム一致で移動、不一致・取得失敗でコピー。Shift の移動は下げない。ボリュームの取得は差し替え可能な形にして検査。
- 当たり判定: 表示済みの pass の番号の形だけを使うこと。「表示は A、次の pass の形は B、present が省略された (番号が進まない) 後の次のフレームの描画前」で A を使う回帰、
  `request_discard` による 2 pass のフレーム (描かれなかった 1 pass 目の形を使わない)、番号の形が無ければ NONE、ppp 1.0/1.5、行全体の rect の端、
  `TreeNonTarget` (インデント・空白・スクロールバー・重なったレイヤーで外した行) が NONE で一覧へ流れないこと、大きさ・DPI 不一致で NONE。
- vendor egui-wgpu: present が返ったときだけ番号を記録し、Outdated・skip では記録しないこと (既存の `winit_tests.rs` の形で、GPU が要る部分は既存の扱いに合わせる)。
- 受付の述語: snapshot と投入で同じ述語。Running・Selecting・Remote・終了・モーダル・sidecar で worker を起動しない。整理先とツリーの両方の入口。snapshot の作成で本への追加の取り消しが失効しないこと。毎 pass の述語・行の形・メニュー有効判定では `book_root_path()` を呼ばず、投入時は一回だけ呼び本棚自身・配下を拒否すること。
- 外部ドロップの返答: 偽の data object で、ツリー行への外部 Drop が COPY を返し、`PERFORMEDDROPEFFECT` 等を書かないこと。Shift でもコピーの要求になること。
- 内部ドロップ: `InternalDrag.paths` 全件で要求を作ること (data object の `CF_HDROP` を読まない)。`failed_paths > 0` で受け付けない。
- 一覧への落下: `GridFiles` が `handle_external_file_drop` に届き、内部ドラッグでは届かないこと。sidecar 復元中は捨てること。
- 完了後: 先と元の親に `observe_accepted_folder`、現在フォルダの `Notified` は従来どおり 1 回。
- 右クリック: 「貼り付け」の有効/無効、その行のフォルダで verb が呼ばれること (呼び出しは差し替え可能な形で検査)、現在フォルダのときだけ選択要求、
  失敗のとき自分が作った選択要求だけ取り消すこと (現在フォルダ以外への失敗で既存の要求を消さない)。Ctrl+V が同じ関数を通ること。
- §1.821: worker が Shell 転送を使い、`CopyOutcome` へ正しく写すこと (中断・失敗)。フォルダが skip されないこと、フォルダを自身の配下へ落とすと全体を拒否すること。実 Shell は既存の `#[ignore]` テストの形で 1 件。

実機確認 (サブ PC、使い捨てデータ):

1. 一覧から同じドライブのツリー行へドラッグ → 移動 (既定)。Ctrl でコピー、Shift で移動、別ドライブは既定でコピー。**ツリー行の上で移動のバッジ
   (または移動のカーソル) が出て、コピーの＋だけにならないこと (出荷条件、§4.3)**。ツリー行を離れると今と同じ見た目に戻る。エクスプローラーへのドラッグは今どおりコピー。
2. エクスプローラーからツリー行へドラッグ → コピー (Shift でもコピー)。元は残る。
3. 同名の衝突で Windows の画面 (置換・スキップ)。大きなコピーを取消。エクスプローラーで Ctrl+Z して戻る。
4. エクスプローラーから一覧へドロップ: 同名があると Windows の確認画面が出る。フォルダもコピーされる (§1.821)。mIV 内で一覧の上に落としても何も起きず、＋も出ない。
5. ツリー行の右クリック → 貼り付け (エクスプローラーでコピー/切り取りしたファイル)。切り取りは元が消える。
6. 一覧とツリーの更新: 元フォルダ表示中にフォルダを動かすと一覧とツリーの両方から消え、展開済みの先の行の下に出る。
7. ツリーのスクロール直後・窓のリサイズ直後に落としても、見えている行に入る (ずれた行に入らない)。

## 11. 利用者の判断

Q1〜Q5 はすべて回答済み (§0)。残る判断は §4.3 の出荷条件を満たさなかった場合だけ。

## 12. 文書の更新 (実装時)

- マニュアル: `grid.html` (ツリーの節)・`tut-file-ops.html`。修飾キーと「同じドライブ」、外から落としたときはコピーになる、行を先に表示しておくこと
  (ドラッグ中の自動展開・自動スクロールはしない)、取り消しはエクスプローラー、評価・タグは先で「編集内容の復元」、一覧へのドロップの確認画面。
- 設計: 本書、[file-drag-drop-design.md](file-drag-drop-design.md) (§5〜6 IDropSource と drag image、§11 受け取りの drop target と Shell 転送、
  `internal_drop_pending` の撤去)、[folder-tree-refresh-plan.md](folder-tree-refresh-plan.md) (転送完了の producer)、
  [file-organize-destinations-plan.md](file-organize-destinations-plan.md) (受付の述語と owner の共有)、`docs/README.md`、`docs/spec.md`、
  [ui-responsiveness.md](ui-responsiveness.md) (ツリーの貼り付けの同期 Shell 呼び出しを既知の例外に追記)。
- キー操作は追加しない (右ボタン・D&D・修飾キーは固定入力。`docs/keymap-spec.md` の固定入力に理由を書く)。

## 13. レビューの経過と、減らした組み合わせ

- r1 (c3eedb5d8) REJECT: 表示前の形で判定、ツリーの空白が一覧のコピーへ流れる、外部の移動の完了を先に知らせる (P1×3)、ほか P2×5。
- r2 (8576098d7) REJECT: 複数 pass で表示前の形が残る、非同期貼り付けがクリック時と別のクリップボードを貼る・切り取りの data object の apartment (P1×3)、
  custom IDropSource の条件不足・遮蔽のサンプリング漏れ・選択要求の取り違え・ボリューム近似 (P2)。
- r3 で**仕組みを足す代わりに組み合わせを消した**もの (CLAUDE.md「設計の簡素化」):
  - 外部からの移動を受けない (Q4)。外部の元との完了の約束事 (async・最適化移動) を扱わない。
  - 貼り付けを worker にしない。Ctrl+V と同じ同期の呼び出しにし、クリップボードの所有・apartment・選択要求の順序の組み合わせを消す
    (代わりに切断されたネットワーク先での UI 停止を既知の例外にする)。
  - custom IDropSource を作らない。表示は target の drop description だけ。表示されなければ移動を出荷しない (§4.3)。
  - 自動展開・自動スクロール・行の強調表示をしない。
  - 遮蔽は点ではなく行全体で外す。形はフレームの最初の pass でだけ入れ替える。
- r3 (b238cc97c) REJECT: 字句の予告がコピーで実行が移動になる反例 (同じボリュームの別名)、App から戻った後・描画前の callback で未表示の形を使う (P1×2)。
- r4 で直したもの: worker は予告を上げない (下げるだけ)。形の一致の規則 (r4 案) は破棄。
- r4 (a4f40baec) REJECT: present が省略されるフレーム (Surface Outdated 等) があり、フレームの開始は描画済みの証拠にならない (P1×1)。
- r5 で直したもの: 描画側 (vendor egui-wgpu) が present の返った pass の番号を記録し、当たり判定はその番号の形だけを使う。推定の仕組み (in_pass・一致規則・追加 repaint) を全部外した。

## 14. B の実装着手と設計照合での停止 (2026-11-20)

基点 `6f91f6058`、未コミットの中間差分。利用者の指示により、コンパイル・テスト・ビルドは一切実行していない。B の fmt・glyph 検査も未実施。

- §4.3 の「同じ data object に対して `IDragSourceHelper2::SetFlags`」は、ローカルの windows 0.61.3 の生成 API と一致しない。`SetFlags(&self, dwflags: u32)` は helper 自身の設定だけで、data object を受け取るのは `InitializeFromWindow` / `InitializeFromBitmap`。独立に作った helper を保持して SetFlags だけ呼んでも、既存 Shell data object と関連付ける処理がない。
- [Microsoft の仕様](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl/nn-shobjidl-idragsourcehelper2) も SetFlags を InitializeFrom* より前に呼ぶ手順を示す。一方、[DragDropVisuals の `_GetDataObject`](https://github.com/microsoft/Windows-classic-samples/blob/main/Samples/Win7Samples/winui/shell/appplatform/DragDropVisuals/DragDropVisuals.cpp) は Shell item の枝では BindToHandler を使い、独立した SetFlags は使わない。generic Shell image に target の drop description が表示されないと判明したわけではない。
- 設計担当の判断が必要: 無関係な SetFlags を省き generic Shell image と target description を維持して実機の出荷条件へ進むか、InitializeFrom* を使う設計に改訂するか。後者は現在の「画像は作らない・InitializeFrom* は呼ばない」という採択方針に関わるため、実装者は選ばない。

保存済みのコードは `src/main_drop_target.rs`、`src/app/main_drop.rs` の骨格、`src/file_drag.rs`、`src/shell_file_ops.rs`、`src/app.rs`、`src/folder_pane.rs`、`src/lib.rs`、`vendor/egui-wgpu/src/{presented_pass.rs,lib.rs,winit.rs}` と `PATCHES.md`。型の初期化・既存 literal・呼出接続・native submodule・UI・テストは未完了で、コンパイル可能な完成差分ではない。
文書の下書きはマニュアル `grid.html` / `tut-file-ops.html`、本書、`file-organize-destinations-plan.md`、`folder-tree-refresh-plan.md`、`keymap-spec.md`、`spec.md`、`ui-responsiveness.md`。残る文書の更新と §10 のテスト追加は設計判断後に続ける。製品バイナリ・実 Shell・実 GUI は起動しておらず、コミットもしていない。

設計担当の判断 (2026-11-20): **無関係な SetFlags を省き、generic Shell image と target の drop description のまま進める** (§4.3 を改訂)。
`InitializeFrom*` で画像を作る案は採らない。表示の正否は §4.3 の出荷条件 (実機確認) で判断し、満たさなければ移動を出荷しない。

## 15. B の実装の記録 (2026-11-20、`6f91f6058` 上の未コミット差分)

§14 の停止後、利用者から設計担当の決定と続行指示を受領。独立した `IDragSourceHelper2::SetFlags` を除き、generic Shell image と target の drop description を使う。外部からの移動、custom IDropSource、自動展開・自動スクロール・強調表示、貼り付け worker は追加していない。

### 変更ファイルと設計節との対応

| 節 | ファイル | 実装 |
| --- | --- | --- |
| §6.2・§10 | `vendor/egui-wgpu/src/{presented_pass.rs,lib.rs,winit.rs,winit_tests.rs}`、`PATCHES.md` | `last_presented_pass` と `present_if_available`。present が返った後だけ viewport ごとの完了 pass 番号を公開。None・未帰還・2 pass・viewport 分離の fake-frame テスト |
| §4〜6・§10 | `src/main_drop_target.rs`、`src/main_drop_target/{native.rs,tests.rs,native/tests.rs}` | ROOT の STA-bound `IDropTarget`、表示済み形の ring、純粋な操作決定、サイズ・DPI・行／空白の分類、CF_HDROP と表示 helper、説明の変更時更新と INVALID、失敗通知。偽の data object・表示 helper で COM callback を検査 |
| §4.1・§4.4 | `src/file_drag.rs` | `with_internal_drag` で Shell 解決前の全パス・仮想除外・解決失敗件数を modal 呼び出し中だけ共有。外への COPY と既存 async data object を保持 |
| §4.2 | `src/shell_file_ops.rs` | 要求に `default_move_needs_volume_check` を追加。`resolve_default_move` は検証前に既定 Move だけボリュームを調べ、異なる／求まらない場合 Copy へ下げる。差し替えた照会関数で回帰を記述 |
| §6.1〜6.4・§7・§10 | `src/app/{main_drop.rs,main_drop_tests.rs,update_phases.rs}`、`src/app.rs`、`src/lib.rs` | 初回 HWND で登録、pass 末尾の形、共有受付と投入、DropRequest 消費、Ctrl+V と共通の同期貼り付け。旧 `internal_drop_pending` / `cursor_over_window` を撤去 |
| §6.3・§7・§10 | `src/folder_pane.rs`、`src/ui_folder_pane.rs`、`src/ui_folder_pane_drop_tests.rs` | 行全体と panel の形、インデント除外、前面 layer との交差行除外、行を移動・選択しない右クリック「貼り付け」。形式照会はメニューを開いた時に一回。閉鎖・非表示・無効化でメニューの保持を終える |
| §6.4・§9 | `src/ui_dialogs/file_organize.rs` | Selecting を一度消費して共通投入へ。完了時に先と各元の親へ `observe_accepted_folder`、現在フォルダの `Notified` 一回を維持 |
| 型追従 | `src/app/{tests.rs,section375_c1_tests.rs}` | 既存の明示要求に新しい field の false を指定 |
| §12 | `docs/{README.md,file-drag-drop-design.md,file-organize-destinations-plan.md,folder-tree-refresh-plan.md,keymap-spec.md,spec.md,ui-responsiveness.md}`、本書、`htdocs/mimageviewer/manual/{grid.html,tut-file-ops.html}` | 送受信・共有 owner・同期貼り付けの既知の例外・固定入力と、利用者向け操作説明を更新 |

描画の寿命は入力の `label_display` の退役から分けた。`update_frame` 直後に可視行だけの `CapturedMainDropGeometry` を pass 内のローカル値へ捕捉し、`finish_folder_pane_presentation` 後も保持する。最後に後描画の前面 layer と交差する行を外して公開するため、描画後の hide でも画面に出たツリーが一覧領域に誤分類されず、show は未描画の形を先取りしない。新しい App の pending や永続状態は追加していない。

独立レビュー追補 (2026-11-20、`930bdaa8e` 後、範囲修正は §18): ROOT の `keyboard_ownership_snapshot` はツリーの貼り付けメニューの正確な ID の open／保持済み Esc の所有だけを既存 `KeyboardOwner::Modal` へ統合し、`input_main_grid_shortcuts` 冒頭も Modal だけを拒否する。一般の popup・TextInput の所有、各枝のテキスト入力条件は維持する。
通常の ROOT 入力→一覧ショートカット→ツリー描画の順で、Esc、矢印→Enter、Ctrl+A／Ctrl+O／F、閉鎖後の再開と先描画 close 時の所有維持を検査する回帰 4 件を記述した (未実行)。
製品ページのフォルダツリー紹介を移動・コピー・右クリック貼り付けと `manual/grid.html#folder-tree` の案内へ更新した。

### 実施した検査と静的レビュー

- `cargo fmt` (引数なし): exit 0。
- `python scripts/check_ui_glyphs.py`: exit 0、危険なグリフ 0 件。
- `git diff --check`: exit 0 (Git の LF/CRLF 予告だけ)。
- 新規のテスト記述は native 側 17 件 (vendor 2 件を含む)、App 側 18 件、UI 側 12 件 (§18 までの追補を含む)。**追補の実行はしていない**。
- 独立した GPT-6.1 Sol / xhigh が API 署名・型・fixture・所有境界を最終静的レビューし、ソースレビューとして ACCEPT (未解決 P1/P2 なし)。§16 の P2 は、設計担当の判断に沿って本棚の判定を操作の境界へ移して解消した。コンパイル・実行の成功を示すレビューではない。

### 設計担当が実行するテスト

以下は **実行候補の filter** であり、成功結果ではない。B ではコンパイル・test・nextest・build・build script を一切実行していない。

- core の追加・直接関連経路と既存ツリー／完了経路の回帰: `-E 'test(/main_drop|folder_tree_drop|native_drag_shared_payload|shell_transfer|file_organize|file_drop|folder_pane|section375_c1/)'`
- vendor egui-wgpu (`--features winit` の graph): `-E 'test(/presented_pass_/)'`。新規 2 件は GPU 不要の fake-frame 試験。実 Surface acquire の Outdated 等から非 present 経路へ到達することは配置の静的確認だけで、実 GPU での実行証拠ではない。

新たに実 Shell・実 GUI に依存する試験は追加していない。既存 `shell_transfer_real_copy_and_move_file_and_folder` と A の `dropped_paths_worker_real_copy_file_and_folder` は `#[ignore]` のまま、未実行。製品バイナリも起動していない。

### snapshot と実機で残る確認

PNG は変更していない。既存の見た目の照合対象は `folder_pane_location_light` / `folder_pane_location_dark` / `folder_pane_refresh_light` / `folder_pane_refresh_dark`。行の allocation・描画・行高は維持しており、既存 PNG の変更は想定しない。メニューの新規撮影候補は `folder_tree_paste_menu_enabled_light` / `folder_tree_paste_menu_enabled_dark` / `folder_tree_paste_menu_disabled_no_files` / `folder_tree_paste_menu_disabled_admission` (現時点の既存 snapshot 名ではない)。
移動バッジの出荷条件、drop description のテキスト、Windows の競合・進捗・取消画面、エクスプローラーの Ctrl+Z、実 DPI・リサイズ・スクロール直後の位置、実クリップボードとの貼り付けは §10 の実機確認に残す。コンパイル結果も未確認で、設計担当からのエラー連絡があれば同じセッションで修正する。

## 16. 既定本棚パスの同期 I/O — 投入時の判定へ変更 (2026-11-20)

最終監査で `shell_transfer_target_is_books` → `Settings::books_root_path` → `books::default_books_root` → `capture::default_output_dir` → `pictures_root` → `SHGetKnownFolderPath(FOLDERID_Pictures, KF_FLAG_DEFAULT)` を確認した。`book_root=Some` はメモリ値だけだが、None の既定では存在確認を含む API へ到達するため、新しい毎 pass／行判定から UI の同期 I/O を増やしてしまう。独立レビューでも未解決 P2 とされた。

既存の `BookShelfOwner.snapshot` は本棚を開いた時だけの値で、常用の正本にはできない。`known_folders` も同期解決であり、クリップボードの `DestinationResolution` は初回 pass 後に開始される worker なので、常に解決済みとも扱えない。USERPROFILE の推測先・`KF_FLAG_DONT_VERIFY` への変更・未解決時に黙って対象を許可する回避策は採らない。

利用者へ以下の三案を質問したが、いずれも採らない判断を受領した (末尾)。以下は比較の記録:

1. 推奨: 正規の既定本棚パスを `data_dir::init` 後・egui 構築／GUI worker 開始前に一度解決し、全 books consumer がランタイムの共通値を読む。受付待ちを増やさないが、実行中の Windows「ピクチャ」保存先変更は次回起動で反映する仕様になる。明示 `Settings.book_root` の変更は即時反映を維持する。
2. 既存の保存先 resolver の結果を共有するよう拡張し、未解決中の転送受付を保留する。ただし整理先にも新しい一時制限が加わる。
3. この点を設計担当へ戻す。

起動前 pin を選ぶなら変更先は `books` の既定 root owner と `lib.rs` の初期化境界になる。テストは通常の起動を通らず `data_dir` override もあるため、一時パスをプロセス全体へ固定しない扱いも必要になる。この案は不採用で、既定 root owner・起動経路・resolver は変更しない。

設計担当の判断 (2026-11-20): **1〜3 のどれも採らず、本棚の配下の判定を毎 pass の `admission` と行の判定から外す**。
本棚の配下かどうかは、投入の境界 (`submit_shell_transfer` と、ツリーの「貼り付け」を押した時) でだけ `book_root_path()` を呼んで確かめる
(今の Ctrl+V・一覧へのドロップが `current_favorite_target()` で操作のときに 1 回呼ぶのと同じ時点・同じ関数。新しい同期 I/O の入口は増えない)。
ドラッグ中は本棚の行にも移動/コピーのバッジが出るが、落とすと「本棚の中へは移動・コピーできません」と通知して何もしない。
「貼り付け」の有効/無効の判定にも本棚の判定は入れない (押した時に断る)。起動前の pin・resolver の共有・受付の保留は作らない。
§4.2 の「受け付けない場面」と §6.4 の述語から「先が本棚の配下」を外し、投入の境界の再確認に移す。

実装では `shell_transfer_admission`・行の形・メニューの有効判定から本棚パスの解決を除去した。`shell_transfer_submission_admission_with` を転送投入とツリーの貼り付けで共有し、既存の `book_root_path()` を操作時に一回だけ呼ぶ。Ctrl+V は既存の確認を保ち、共通の貼り付け処理で重ねて解決しない。起動前の pin・cache・resolver の共有・受付の保留は追加していない。

回帰テストには、未設定の本棚でも pass の受付・形の公開で解決が 0 回、ツリー・整理先・貼り付けの各投入で解決が 1 回となること、本棚自身・配下の拒否では worker・Shell・選択要求・本への追加の取り消し状態に副作用を与えないことを記述した。メニューは本棚の行でも有効になり、押した時に通知して拒否する。いずれも未実行で、設計担当の検証へ引き継ぐ。

## 17. 設計担当の nextest 結果に対する fixture 修正 (2026-11-20)

設計担当より、core の filter `main_drop|folder_tree_drop|native_drag_shared_payload|shell_transfer|file_organize|file_drop|folder_pane|section375_c1|dropped_paths` でコンパイル成功、263 件中 260 件成功・3 件失敗の報告を受領した。設計担当が修正した MouseWheel の `phase` / `source`、`IAgileObject` の import、vendor の `Id::unique` は維持する。

- snapshot を作る二つのテストは `items` が 1 件、`thumbnails` が 0 件の不正な fixture だった。`src/app/tests.rs` の既存 `seed_snapshot_context` を全 OS の共有テスト helper として公開し、`src/app.rs` の test-only 再 export を介して `src/app/main_drop_tests.rs` と `src/ui_folder_pane_drop_tests.rs` で使う。項目・サムネイル・メタデータ・可視 index を揃え、製品の debug assert は変更しない。
- メニュー退役のテストの exit 2 (行が消える場合) は `FolderPaneState::default()` を空状態と誤認していた。実際は実機のドライブを取得して行を持つため、既存の空行時の閉鎖経路を通らなかった。テストでドライブ候補と選択ドライブを明示的に空にし、行が空であることを確認してから描画する。chrome 非表示・ツリー非表示の二経路は既存の finish 処理が直接閉じるため、フレーム待ちや製品コードの変更は不要。失敗時に exit 番号が分かる assert メッセージも追加した。

修正後は `cargo fmt` (引数なし)・`python scripts/check_ui_glyphs.py`・`git diff --check` がいずれも exit 0 (危険グリフ 0 件、Git の LF/CRLF 予告のみ)。コンパイル・テストは実装者側では実行しておらず、再実行は設計担当へ引き継ぐ。
独立した GPT-6.1 Sol / xhigh による追補の静的レビューは ACCEPT、未解決 P1/P2 なし。テストの実行成功を示すものではない。

## 18. 貼り付けメニューの入力所有を狭める追補 (2026-11-20)

未コミットの P2 修正案に含めた全 ROOT popup の Modal 化と `shortcuts_blocked_by_text_input` による一覧ショートカットの一括 return は撤回した。`folder_pane_paste_menu_owns_keyboard` は ROOT の貼り付けメニュー ID の `Popup::is_id_open` と既存 `popup_owned_escape_at_pass_start` だけを見る。`keyboard_ownership_snapshot` でこのメニューだけを Modal に加え、`input_main_grid_shortcuts` 冒頭は pass owner が Modal の場合だけ return する。

一覧右クリックの既存経路は `context_menu_idx` → `modal_dialog_block_reason_including_menus` (`grid_context_menu`) → `common_modal_dialog_open` → `any_dialog_open` → `keyboard_ownership_snapshot` の Modal → `shortcuts_blocked_by_text_input` → `handle_keyboard` の return。ツリーのメニューも同じ Modal と後段の return を使う。Menu/ComboBox/補完等のほかの egui popup の分類には触れない。履歴候補は `ui_history_field` の Area と HistoryPopupState であり、従来の TextInput／pending focus／履歴キー claim が所有する。

`input_main_grid_shortcuts` の全枝を `930bdaa8e` と照合した結果:

| 枝 | テキスト入力中の変更前→変更後 | ツリーの貼り付けメニュー表示中 |
| --- | --- | --- |
| 保存位置 clear | 個別 focus 条件と keymap 消費条件を維持 | 許可されていた背面処理を Modal で停止 |
| 最前面切替 | 同上 | 同上 |
| F12 の表示モード切替 | 同上 | 同上 |
| Ctrl+F Local 検索 | search_shortcut_availability と pressed_action の条件を維持 | 同上 |
| 全選択・選択解除 | 個別 focus 条件と keymap 判定を維持 | 同上 |
| pin・本追加・collection 追加・選択 export | 個別 focus・grid item・viewer 条件と keymap 消費を維持 | 同上 |
| Ctrl+S Container 検索 | 個別 focus 条件と pressed_action を維持 | 同上 |
| Everything 検索 | 従来の shortcut_permit と受付・消費を維持 | 同上 |
| Ctrl+G Metadata 検索 | search_shortcut_availability と pressed_action を維持 | 同上 |
| Ctrl+T タグ表示 | 個別 focus 条件と pressed_action を維持 | 同上 |
| T タグ付与 | 個別 focus・grid item 条件と pressed_action を維持 | 同上 |
| Ctrl+O フォルダ入力 | 個別 focus 条件と pressed_action を維持 | 同上 |
| 詳細表示・列数 | 個別 focus 条件と pressed_action を維持 | 同上 |

実キーの Ctrl+F は元から `pressed_action` → `pressed_chord` → `keymap_owner_blocks_shortcuts` により TextInput の Focused／FocusRecovery／ImeComposing では拒否される。アドレス・Container・Metadata・タグ等にはさらに検索入口の個別拒否がある。コメントと起動 helper の相互排他試験は「focused 実キーも許可する」証拠にはならない。入力欄がキーを所有しない状態からの Container→Local 切り替えと、入力中の従来の拒否を新しい実キーテストで固定する。PendingFocus の互換動作と履歴による別の予約判定も変更しない。

`ime_focus::record_keyboard_owner` は診断記録だけで変換状態を変えず、ほかの popup の TextInput／FocusedUi 等の分類を維持する。`dialog_enter_pressed` の IME 判定、`dialog_escape_pressed` の IME／既存 pass-start Escape 所有、IME helper は変更していない。`gamepad_dispatch_allowed` 末尾も共通 shortcut 判定を参照するが、追加の拒否はツリーの貼り付けメニューのときだけで、ほかの popup／TextInput／IME には従来の判定が残る。`common_modal_dialog_open` と他 viewport の分岐は変更しない。

追加: `main_drop_unrelated_popup_keeps_text_input_owner_and_ime_dialog_keys`、`main_drop_ctrl_f_keeps_existing_focused_input_rejection_and_unfocused_search_switch`。先描画 close の回帰は `main_drop_tree_paste_menu_keyboard_owner_survives_early_close_and_resumes_next_pass` へ範囲を修正した。
既存の `history_field_arrows_pick_and_plain_enter_are_exclusive`、`history_field_escape_closes_only_history_and_restores_plain_escape`、`history_field_ime_does_not_navigate_pick_or_dismiss`、`section1756_pending_history_text_and_paste_enter_reach_the_editor_before_submit`、`facade_observes_ime_restoration_after_the_shared_contract_is_refreshed` を履歴・IME の回帰として引き継ぐ。いずれも今回の修正後は実装者側で未実行。
引数なしの `cargo fmt`、glyph 検査、site cards 検査 (53 件・超過 0)、`git diff --check` は exit 0。追補の独立静的レビューは ACCEPT、未解決 P1/P2 なし。コンパイル・テスト・製品起動・コミットは行っていない。
