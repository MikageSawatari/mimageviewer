# §1.464 フォルダツリーへのドラッグ&ドロップ・右クリック貼り付け / §1.821 一覧へのドロップの上書き — 設計

2026-11-19〜20 設計 (ClaudeCode)。worktree `G:\home\mimageviewer-treednd464`、branch `treednd-464`、基点 master `ca6686d93`。
要件の正本は [next-release-backlog.md §1.464・§1.821](next-release-backlog.md)。
**状態: 設計 r5 (独立レビュー r1〜r4 = REJECT を反映。仕組みを足すより組み合わせを減らした。§13)。実装前の再レビュー待ち。**

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

受け付けない場面: 行の上でない / 本棚の配下 / 明示の移動か既定で、先が元の親と同じ (字句。Explorer も同じフォルダへの既定のドロップでは何もしない) /
元フォルダ自身か配下 / §6.4 の述語 / §4.1。

### 4.3 OLE への返答と表示 (r1 P2-6・r2 P2-5)

送出の `dwOKEffects` は COPY のまま (Q2)。ツリー行の上では mIV の target は OLE へ COPY を返し、Drop で `InternalDrag.paths` から
`ShellTransferRequest` を作る (data object からは読まない)。送出元は mIV なので COPY と答えても削除は起きない。

**IDropSource は今の Shell 既定のまま** (`SHDoDragDrop` の `pdsrc = None`、generic image も今のまま。file-drag-drop-design §12 の async wrapper の
扱いも変えない)。表示は target 側だけで行う (Microsoft の DragDropVisuals サンプルと同じ形):

- mIV の target は `IDropTargetHelper` (`CLSID_DragDropHelper`) の DragEnter/Over/Leave/Drop を呼ぶ (自分の窓の上に drag image を描かせる)。
- ツリー行の上では data object へ `CFSTR_DROPDESCRIPTION` を `SetData` する: 予想が移動なら `DROPIMAGE_MOVE` + 「%1 へ移動」、
  コピーなら `DROPIMAGE_COPY` + 「%1 へコピー」、受け付けないなら `DROPIMAGE_NONE`。%1 はフォルダ名。変わったときだけ書く。
  行を離れたら・DragLeave・Drop の後は `DROPIMAGE_INVALID` に戻す (古い説明を残さない)。
- 説明文のテキストを出すため、`SHDoDragDrop` の前に `IDragSourceHelper2::SetFlags(DSH_ALLOWDROPDESCRIPTIONTEXT)` を同じ data object に対して呼ぶ
  (画像は作らない。`InitializeFrom*` は呼ばない)。テキストが出ない環境でも、バッジ (type) で移動かコピーかは分かる。

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
- Drop: `CF_HDROP` のパスで `ShellTransferRequest { intent: Explicit(Copy) }` を作り、**COPY を返す**。`PERFORMEDDROPEFFECT` 等は書かない。
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

副作用の無い述語 `shell_transfer_admission(&self, target) -> Result<(), String>` を 1 つ作り、`admission` と投入時の両方で使う。
見るもの: `FileOrganizeRequest` が `Hidden` (整理先の選択画面 `Selecting` もモーダルとして拒否、`Running` は消さない)、
`remote_session_blocks_local_control()`、終了要求 (shutdown・tray quit・root close)、`main_hwnd`、`viewer_mutation_modal_active()`、
`sidecar_restore_active()`、`common_modal_dialog_open`、`folder_pane_disabled()`、先が本棚の配下でないこと。

ツリーへの投入 `submit_shell_transfer(request)` は述語を**もう一度**見て、通ったときだけ `admit_book_append_followup_mutation()`
→ `transfer_items_async` → `Running`。通らなければ理由を通知して何もしない。整理先は今の `take_for_submission` で `Selecting` を一度消費してから、
同じ述語 (Selecting の判定を除いた部分) と投入を使う (条件を 2 か所に書かない)。`admission` の計算では `admit_book_append_followup_mutation` を呼ばない。

「同時に 1 本」は整理先とツリーへのドロップの間の保証。一覧への落下 (§8) とツリーの貼り付け (§7) は今の Ctrl+V・一覧への落下と同じく並行し得る
(全操作を横断する排他は作らない)。

## 7. 右クリック「貼り付け」(Q3、r1 P2-5・r2 P1-2/P1-3/P2-4/P2-6)

**Ctrl+V と同じ関数を、Ctrl+V と同じく UI スレッドで同期に呼ぶ**。r2 で worker 化に伴う問題 (クリック時と別のクリップボードの中身を貼り得る、
切り取りの data object の apartment、worker の OLE 初期化、選択要求の取り違え) が出たため、worker 化をやめて組み合わせごと消す。

- ツリー行の右クリックで egui のメニュー (`os_theme` の helper) に「貼り付け」1 項目。行は右クリックした時点のパスで確定する。
  有効なのは、クリップボードにファイルがある (`IsClipboardFormatAvailable(CF_HDROP)`、メニューを開いたとき 1 回) かつ §6.4 の述語が通るとき。
  無効の理由はツールチップで示す。画像だけのクリップボードは対象外。
- 実行: 今の Ctrl+V の処理 (`app.rs:64139` 付近の「ファイルなら」の枝) を、貼り付け先のフォルダを引数に取る 1 つの関数に切り出し、
  Ctrl+V とツリーの両方から呼ぶ: (ツリーは述語を再確認) → `admit_book_append_followup_mutation()` → 先が現在の実フォルダのときだけ
  `request_post_operation_selection_for_added_items(folder)` → `invoke_shell_folder_background_verb(hwnd, folder, Paste)` →
  `resync_egui_modifiers_from_os` → 失敗ならログと、**この呼び出しで選択要求を作ったときだけ** `cancel_post_operation_selection`。
  同期なので、間に別の選択要求は入らない。
- 応答性: Shell の bind・`InvokeCommand` は UI スレッドで行う (Ctrl+V と同じ)。ツリーの行は列挙できたフォルダだが、その後に切断された
  ネットワーク先では OS が諦めるまで UI が止まり得る。整理先を開く同期選択 (file-organize-destinations-plan §13、利用者了承) と同じ既知の例外として
  ui-responsiveness §4 に記録する。転送そのものは Shell が持つ (戻り値は verb 呼び出しの成否)。
- Ctrl+V 自体の挙動は変えない (関数の切り出しだけ)。

## 8. §1.821 一覧へのドロップ (Q5 = フォルダも受ける)

- 原因 (コード): `copy_paths_into_folder` (`context_menu.rs:3321`、呼び出しは `app.rs:52665` の 1 か所) が `Copy-Item ... -Force` で、
  同名のファイルを確認なしに上書きする。
- 直し方: `handle_external_file_drop` の受付 (検索結果・タグ表示中・ZIP/PDF 表示中・本棚配下の拒否、`admit_book_append_followup_mutation`) と
  `file-drop-validate` worker・`drop_copy_pending` の並行受信はそのまま。worker の中のコピーを、整理先と同じ検証 + `IFileOperation` Copy
  (`shell_file_ops` の同期関数を worker から呼ぶ。`operation: Copy`) に替える。同名の衝突・進捗・取消・エラーは Windows の画面。
  結果は今の `CopyOutcome` に写す (中断 → 「操作が中断されました。処理済みの項目は元に戻りません」、失敗 → 全件失敗の通知)。
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

- 操作の決定 (純関数): 修飾キー × ルートの同じ/別 × 内部/外部 × 外部の許可効果 × 同じ親・自身/配下・本棚配下・`excluded_virtual`・`failed_paths`。UNC・大文字小文字・末尾区切り。
- 既定の解決 (worker): 予告がコピーなら必ずコピー (r3 の反例 `D:\src` → `C:\MountD\dst` で同じボリュームでもコピー)。予告が既定の移動なら、ボリューム一致で移動、不一致・取得失敗でコピー。Shift の移動は下げない。ボリュームの取得は差し替え可能な形にして検査。
- 当たり判定: 表示済みの pass の番号の形だけを使うこと。「表示は A、次の pass の形は B、present が省略された (番号が進まない) 後の次のフレームの描画前」で A を使う回帰、
  `request_discard` による 2 pass のフレーム (描かれなかった 1 pass 目の形を使わない)、番号の形が無ければ NONE、ppp 1.0/1.5、行全体の rect の端、
  `TreeNonTarget` (インデント・空白・スクロールバー・重なったレイヤーで外した行) が NONE で一覧へ流れないこと、大きさ・DPI 不一致で NONE。
- vendor egui-wgpu: present が返ったときだけ番号を記録し、Outdated・skip では記録しないこと (既存の `winit_tests.rs` の形で、GPU が要る部分は既存の扱いに合わせる)。
- 受付の述語: snapshot と投入で同じ述語。Running・Selecting・Remote・終了・モーダル・sidecar・本棚で worker を起動しない。整理先とツリーの両方の入口。snapshot の作成で本への追加の取り消しが失効しないこと。
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
