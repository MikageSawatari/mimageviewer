# §1.464 フォルダツリーへのドラッグ&ドロップ・右クリック貼り付け — 設計

2026-11-19 設計 (ClaudeCode)。worktree `G:\home\mimageviewer-treednd464`、branch `treednd-464`、基点 master `ca6686d93`。
要件の正本は [next-release-backlog.md §1.464](next-release-backlog.md)。**状態: 設計・実装前の独立レビュー待ち。利用者判断待ちの点は §10。**

要望の要旨 (5ch): >>444 ツリーへファイルをドロップできず、右クリックの貼り付けもできない。>>707 読み終えたファイルを
`_mita` フォルダへ D&D したい (内部ドラッグで「＋」カーソルなのに何も起きない)。固定先へ送る「ファイル整理先」(v4.4.0 出荷)
はこの要望の代替扱いにしない (backlog の記録)。

## 1. 結論 (推奨構成)

1. **転送の実行は既存の整理先と同じ経路に乗せる**。ツリー行へのドロップは `ShellTransferRequest` を作り、
   `FileOrganizeRequest::Running` (整理先の単一 request owner) へ直接入れる。検証・`IFileOperation`・完了処理
   (`shell_file_ops::transfer_items_async` → `poll_file_organize`) は整理先と共有する。新しい転送経路・worker・状態は作らない。
2. **右クリックの「貼り付け」は既存の Ctrl+V と同じ Shell の Paste verb** (`invoke_shell_folder_background_verb`) を、
   現在地の代わりにその行のフォルダへ向けて呼ぶ。切り取り (cut) の完了通知・競合確認は Shell が持つ。
3. **ドロップを受けるために、メインウィンドウ (ROOT) の OLE drop target を mIV 自前の 1 個に置き換える**。winit の
   drop target はカーソル位置も修飾キーも渡さず、効果を常に COPY と答えるので、ツリー行の判定と移動/コピーの区別ができない
   (§2)。一覧へのドロップ (既存の「現在のフォルダへコピー」) は、新しい drop target から同じ `handle_external_file_drop`
   へ渡し、挙動を変えない。
4. 移動/コピーの決め方は Windows Explorer に合わせる (§4)。**mIV の一覧から外 (エクスプローラー等) へのドラッグは
   今までどおりコピーのみ**のままにする。
5. 処理中はモーダルにしない (整理先 §5.1 と同じ判断、§7)。同時に走る転送は整理先と合わせて 1 本だけ。

## 2. 現状のコード事実

| 領域 | 事実 |
| --- | --- |
| ツリー | `src/ui_folder_pane.rs` の `render_folder_pane_tree`。行は実フォルダとドライブだけ (`folder_pane.rs::scan_real_subfolders` が `try_classify_dir_entry(..).is_directory()` の項目だけ列挙)。ZIP / PDF・コレクション・タグ・検索結果の行は無い。右クリックには何も割り当てていない (`resp.clicked() && !clicked_by(Primary)` は accesskit 等の擬似クリック)。各行の rect は毎フレーム `folder_pane.label_display` (`FolderPaneLabelDisplay { pass, layer, rect, rows }`) に残る |
| ツリーの更新 | §1.686 ([folder-tree-refresh-plan.md](folder-tree-refresh-plan.md))。`FolderPaneState::observe_accepted_folder(path)` → `request_recheck` が、その親の直下を「未確認」にし、需要があれば再走査する。消費側はこの 1 か所 |
| ドラッグ送出 | `file_drag::start_file_drag` (`SHDoDragDrop`、`dwOKEffects = DROPEFFECT_COPY` のみ、[file-drag-drop-design.md §6.4](file-drag-drop-design.md))。左ボタンのドラッグだけが起点。`post_render_native_drag` (`src/app/update_phases.rs`) がフレーム末尾で同期に呼ぶ。混在選択は実項目だけを送り、除外件数を `post_drag_toast` で後から知らせる (§5.4.2) |
| ドロップ受け | winit 0.30.13 の `FileDropHandler` (`platform_impl/windows/drop_handler.rs`)。`CF_HDROP` があれば DragEnter/Over で常に `DROPEFFECT_COPY`、Drop で `DroppedFile(path)` だけを送る。位置・修飾キーは渡さない。eframe → `RawInput.dropped_files` → `App::input_drop_and_activity` → `handle_external_file_drop` (現在の実フォルダへ PowerShell `Copy-Item -Force` でコピー、フォルダは skip) |
| mIV 内の跳ね返り | `SHDoDragDrop` の effect が NONE 以外で、カーソルが mIV の窓上なら `internal_drop_pending` を立て、次の `dropped_files` を捨てる。>>707 の「＋カーソルなのに何も起きない」はこれ (winit が COPY と答え、mIV が捨てる) |
| 整理先の転送 | `src/shell_file_ops.rs::transfer_items_async`。worker 内で `validate_transfer_request` (絶対パス・実在・実フォルダ・同じ親への移動拒否・フォルダ自身/配下拒否、junction 解決) と STA の `IFileOperation` (`FOF_ALLOWUNDO | FOFX_ADDUNDORECORD`、競合・進捗・エラーは Windows の画面) を行う |
| 整理先の request owner | `src/ui_dialogs/file_organize.rs::FileOrganizeRequest { Hidden, Selecting, Running }`。投入境界 `submit_file_organize` で Remote 所有・終了要求・HWND を見て、`admit_book_append_followup_mutation` の後に worker を起動。`poll_file_organize` が完了を受け、現在の実フォルダが元の親か先なら既存の外部変更確認 (`Notified`) を 1 回だけ要求する |
| 貼り付け | Ctrl+V は `handle_clipboard_paste_with` → ファイルなら `request_post_operation_selection_for_added_items(folder)` → `invoke_shell_folder_background_verb(hwnd, folder, Paste)`。貼り付け先は `current_favorite_target()` (実フォルダ表示中だけ、本棚の配下は除外) |
| 取り消し | mIV の Ctrl+Z (`UndoLastAction`) は評価・タグ・補正と本への追加だけ。整理先の移動/コピーは mIV では取り消さず、`FOFX_ADDUNDORECORD` で Windows の取り消し履歴に載せる (エクスプローラーの Ctrl+Z で戻せる) |

## 3. 範囲

| 対象 | 初版 |
| --- | --- |
| ドロップの落とし先 | ツリーの**表示されている行** (実フォルダ・ドライブ)。一覧・ほかの UI は既存のまま |
| ドラッグの元 | mIV の一覧からのドラッグ (既存の送出) と、エクスプローラー等の外部からのドラッグ (`CF_HDROP` を持つもの) |
| 右クリック | ツリー行の右クリックに「貼り付け」を 1 項目だけ出す (§10 Q3) |
| やらないこと | ツリー行をドラッグの元にする、ドラッグ中の自動展開・自動スクロール、ショートカット作成 (Alt / Ctrl+Shift)、ブラウザ等の `CF_HDROP` を持たないデータ (仮想ファイル・画像データ)、mIV 独自の取り消し、一覧へのドロップの挙動変更 |

自動展開・自動スクロールを外す理由: mIV の一覧からのドラッグ中は `SHDoDragDrop` が `App::update` の中で止まっていて
egui が描けない。ドラッグ元によって展開できる/できないが分かれる組み合わせを作らない。**落とし先の行を先に表示しておく**操作になる
(マニュアルに書く)。

## 4. 移動とコピーの決め方 (Windows Explorer に合わせる)

ドラッグ中 (DragEnter / DragOver) に、行・修飾キー・元パスから効果を決める。**ファイルシステムには触らない**
(UI スレッドなので、比較は字句だけ。最終確認は worker の `validate_transfer_request`)。

| 押しているもの | 効果 |
| --- | --- |
| なし | 元が全部、落とし先と同じドライブ (パスのルートが同じ。ドライブ文字・UNC の共有名を大文字小文字無視で比較) なら**移動**、1 件でも別なら**コピー** (§10 Q1) |
| Ctrl | コピー |
| Shift | 移動 |
| Ctrl+Shift・Alt | 受け付けない (ショートカット作成は mIV では行わない) |
| 右ボタンでのドラッグ | 離したときに native のメニュー「ここに移動」「ここにコピー」「キャンセル」(既定の項目は上の「なし」の規則で太字)。メニューは `TrackPopupMenuEx` で Drop の中で完結させ、egui の状態を作らない |

外部のドラッグ元が許していない効果 (`DragOver` に渡る `pdwEffect`) は選ばない。許していなければ受け付けない。

**受け付けない (効果 NONE) 場面**: 行の上でない / 行が本棚の配下 (Ctrl+V と同じ除外) / 移動で落とし先が元の親と同じ /
元フォルダ自身かその配下へ落とす / 整理先の転送が実行中 / §6 の受付条件を満たさない / mIV の一覧からのドラッグで、
仮想項目 (ZIP 内画像など) を除外して送った場合 (整理先と同じ「対象外が混ざれば全体を実行しない」)。

### 4.1 mIV の一覧からのドラッグ

送出の `dwOKEffects` は COPY のまま変えない (エクスプローラー等へ落としたときは今までどおりコピーだけ)。
mIV 自身の drop target はドラッグが mIV 内のものだと知っている (§5.2 の印) ので、ツリー行の上では上表の規則で操作を決め、
OLE へは COPY を返す。転送は data object ではなく `IFileOperation` で mIV が行うので、送出側の COPY 限定と矛盾しない。
どちらの操作になるかは、ドロップの説明 (`CFSTR_DROPDESCRIPTION`、「%1 へ移動」「%1 へコピー」、%1 はフォルダ名) と
ドラッグ画像 (`IDropTargetHelper`) で示す。このドラッグ中は egui が描けないので、行の強調表示はしない (外部ドラッグでも
しない。表示の有無をドラッグ元で分けない)。**カーソル形状が COPY の「＋」のままになるかは実機確認項目** (§9)。

一覧の上に落としたとき: 今は winit が COPY と答えて mIV が捨てる。新しい drop target は mIV 内のドラッグなら一覧の上では NONE を返す
(「＋」を出さない)。`internal_drop_pending` / `cursor_over_window` の推定は不要になるので外す。

### 4.2 外部からの移動を安全に受ける (元の削除を起こさせない)

外部のドラッグ元に MOVE を返すと、元が「最適化されていない移動」と判断して**元ファイルを自分で消す**ことがある。mIV の転送は
Drop から戻った後に worker で始まるので、元が先に消すと失われる。Shell の「最適化された移動」の手順に従い、移動を受けたときは
Drop で `CFSTR_PERFORMEDDROPEFFECT = DROPEFFECT_NONE`、`CFSTR_LOGICALPERFORMEDDROPEFFECT = DROPEFFECT_MOVE` を
data object に書き、`*pdwEffect = DROPEFFECT_NONE` を返す (元に削除させない。実際の移動は `IFileOperation::MoveItem` が行う)。
根拠: Microsoft Learn「Handling Shell Data Transfer Scenarios」の Optimized move。**レビューで手順の正しさを確かめること**。
コピーは `DROPEFFECT_COPY` を返す。async capability (`StartOperation`) は使わない (data object から中身を読まないため)。

## 5. ROOT の drop target

### 5.1 置き換え

ROOT の HWND を得た最初のフレーム (`main_hwnd` を設定する所) で `RevokeDragDrop(hwnd)` → `RegisterDragDrop(hwnd, mIV の target)`。
winit の handler は使われなくなるが winit が所有したまま (終了時の `RevokeDragDrop` も winit が行う)。OLE の初期化は winit が
済ませているので重ねない。**detached viewer・フルスクリーン等の他の viewport の drop target は変えない** (detached 述語・viewport
経路に触れない)。COM object は `#[implement(IDropTarget, Agile = false)]` (UI の STA 専用)。

### 5.2 App との受け渡し (再入を作らない)

callback は UI スレッドで次の 2 通りに呼ばれる: (a) 外部ドラッグ中、winit のメッセージ処理の中 (App::update の外)、
(b) mIV 内のドラッグ中、`post_render_native_drag` の `SHDoDragDrop` の中 (App::update の中で App は可変借用中)。
どちらでも **callback は App に触れない**。`Rc<RefCell<MainDropShared>>` だけを介する (RefCell の借用は COM 呼び出しをまたがない)。

`MainDropShared` の中身:

- App が毎フレーム書く `snapshot`: その pass の `label_display` から作った「行の rect (論理座標) とパスの組」、`pixels_per_point`、
  受付可否 (§6、`Accept` / `Refuse(理由)`)。label_display が今の pass でなければ行は空。
- mIV 内ドラッグの印 `internal: Option<InternalDrag { excluded_virtual: bool }>`。`start_file_drag` の直前に立て、戻った直後に外す。
- callback が書く `drops: Vec<TreeDrop>` (一覧への落下は `GridFiles(paths)`、ツリー行は `Transfer(ShellTransferRequest)`)。
  外部ドラッグのときは保存した `egui::Context` へ `request_repaint`。

App は `input_drop_and_activity` (既存の `dropped_files` を読む場所) で `drops` を取り出す:
`GridFiles` は今の `handle_external_file_drop` へ (sidecar 復元中は従来どおり捨てる)、`Transfer` は §6 の投入境界へ。
mIV 内ドラッグのときは `post_render_native_drag` が `SHDoDragDrop` から戻った直後に同じ処理を呼ぶ (次フレームを待たない)。

座標: `POINTL` (画面座標の物理 px) → `ScreenToClient(main_hwnd)` → `/ pixels_per_point` → 行の rect と比較。

callback の中でしてよいこと: `CF_HDROP` の取り出し (`GetData` + `DragQueryFileW`。DragEnter で 1 回、パスはその場で保持)、
行の線形探索、字句比較、`IDropTargetHelper` の呼び出し、説明文の `SetData` (変わったときだけ)、右ドラッグのメニュー。
ファイル I/O・`canonicalize`・App の状態変更はしない。

### 5.3 一覧への落下を変えない

一覧 (ツリー行以外) の上: 外部ドラッグで `CF_HDROP` があれば COPY、Drop で `GridFiles(paths)`。winit の今の挙動と同じ
(位置で分けないのも同じ)。mIV 内ドラッグなら NONE (§4.1)。`handle_external_file_drop` の中身 (PowerShell のコピー、フォルダ skip、
検索結果・ZIP/PDF 表示中の拒否) は変えない。

## 6. 受付条件と投入境界

drop target が見る受付可否 (毎フレームの snapshot) は表示用の先回りで、**最終判断は App の 1 か所**で行う:
`submit_tree_transfer(request)` が整理先の `submit_file_organize` と同じ条件を見る —
`FileOrganizeRequest` が `Hidden` であること、`remote_session_blocks_local_control()` が false、終了要求なし、`main_hwnd` あり、
`viewer_mutation_modal_active()` が false、`admit_book_append_followup_mutation()`。通れば
`transfer_items_async` を起動して `Running` に入れる。通らなければ理由を通知して何もしない。整理先と同じ helper に切り出して共有する
(条件を 2 か所に書かない)。

snapshot の `Refuse` にする条件: 上の条件に加え、モーダル (`common_modal_dialog_open`)・egui の popup が開いている、
`folder_pane_disabled()`、ツリーが表示されていない。

## 7. 長い処理・取り消し・同名の衝突

- **モーダルにしない**。整理先 §5.1 と同じ判断: 転送先は表示中の viewer 状態に依存せず、完了後は「エクスプローラーで動かした
  場合」と同じ既存の外部変更確認に任せる。長い別ドライブのコピー中も閲覧を止めない。進捗・取消は Windows の画面。
  割り込みの組み合わせは「同時に 1 本」(整理先と共有する `Running`) で減らす。実行中のドロップは受け付けず (§4)、
  右クリックの「貼り付け」は Ctrl+V と同じく Shell が別に扱う。
- **取り消し**は整理先と同じ。mIV の Ctrl+Z では戻さない。Windows の取り消し履歴に載るので、エクスプローラーで Ctrl+Z すれば戻せる。
  本への追加の取り消しは `admit_book_append_followup_mutation` で失効する (整理先・Ctrl+V と同じ)。
- **同名の衝突**は Windows の画面 (置換・スキップ・両方残す)。mIV は自動上書き・自動改名をしない。
- **評価・タグ・編集**は整理先と同じく移さない。先の通常フォルダを開いたときの既存「編集内容の復元」に任せる (整理先 §6.3)。
- まれな失敗は既定の割り切り (ログ + 通知)。

## 8. 完了後の更新 (一覧・ツリー・サムネイル)

`poll_file_organize` の完了処理を共有し、1 つだけ足す:

1. 一覧: 既存どおり、現在の実フォルダが元の親か先なら `check_folder_changes_with_cause(Notified, Operation)` を 1 回。
2. **ツリー (追加)**: 先のフォルダと元の親それぞれに `folder_pane.observe_accepted_folder(path)` を呼ぶ。§1.686 の再確認の消費側に
   「操作の完了」という producer を 1 つ足すだけで、node が無ければ何もしない。整理先の完了にも同じく効く (整理先でフォルダを動かした
   ときのツリーの取り残しも直る)。
3. サムネイル・キャッシュ・タグ等: 専用の後始末はしない (整理先と同じ。外部移動と同じ扱い)。
4. 右クリックの貼り付け: Ctrl+V と同じく、先が現在の実フォルダなら `request_post_operation_selection_for_added_items` を要求する
   (それ以外は要求しない)。完了は Shell が持つので、ツリーは先のフォルダの再表示・再展開 (§1.686 (c)) と一覧の監視に任せる。

## 9. テストと実機確認

自動テスト (製品バイナリは起動しない):

- 効果の決定 (純関数): 修飾キー × 同じ/別ドライブ × 元の許可効果 × 同じ親・自身/配下・本棚配下・除外ありの mIV 内ドラッグ。UNC・大文字小文字・末尾区切り。
- 座標 → 行の判定: `pixels_per_point` 1.0/1.5、行の端、ScrollArea の外側 (inner_rect で切った rect)、古い pass の label_display は空。
- 投入境界: Running 中・Remote 所有・終了要求・モーダル中は worker を起動しない。整理先と同じ helper を通ることを両方の入口から確かめる。
- 最適化移動の手順: 偽の data object で、移動の Drop が `PERFORMEDDROPEFFECT=NONE`・`LOGICAL=MOVE` を書き、NONE を返すこと。コピーは COPY。
- 一覧への落下: `GridFiles` が `handle_external_file_drop` へ届き、mIV 内ドラッグでは届かないこと。sidecar 復元中は捨てること。
- 完了後: 先と元の親に `observe_accepted_folder` が呼ばれ、現在フォルダの外部変更確認が従来どおり 1 回だけであること。
- 右クリック: 行の右クリックで「貼り付け」を出し、本棚配下・Remote 所有では出さない/無効。クリックで、その行のフォルダへ Paste verb が呼ばれる (verb 呼び出しは差し替え可能な形で検査)。
- 既存の実 Shell テスト (`shell_transfer_real_copy_and_move_file_and_folder`、`#[ignore]`) は変えない。

実機確認 (サブ PC、使い捨てデータ):

1. 一覧から同じドライブのツリー行へドラッグ → 移動 (既定)。Ctrl でコピー、Shift で移動。別ドライブは既定でコピー。説明文とカーソルの見え方。
2. エクスプローラーから同じドライブ/別ドライブのツリー行へドラッグ。移動で元が消え、先に届く (元が先に消えて失われないこと)。右ドラッグのメニュー。
3. 同名の衝突で Windows の画面が出る。置換・スキップ。大きなコピーを取消。エクスプローラーで Ctrl+Z して戻る。
4. 一覧へのドロップ (エクスプローラー → 一覧) が今までどおりコピーされる。mIV 内で一覧の上に落としても何も起きず、「＋」も出ない。
5. ツリー行の右クリック → 貼り付け (エクスプローラーでコピー/切り取りしたファイル)。切り取りは元が消える。
6. 一覧とツリーの更新: 元フォルダ表示中にフォルダを動かすと、一覧とツリーの両方から消え、先の展開済みの行の下に出る。

## 10. 利用者の判断が要る点 (統括経由)

- **Q1 既定の操作**: 修飾キーなしのとき (a) Explorer と同じ (同じドライブは移動、別ドライブはコピー) **[推奨]**、(b) 常にコピー (Shift で移動)、(c) 常に移動。
  >>707 の用途 (読み終えたファイルを `_mita` へ) は同じドライブなら (a) で既定の移動になる。
- **Q2 mIV の外へのドラッグ**: 一覧からエクスプローラー等へのドラッグは今までどおりコピーのみ **[推奨・変えない]**。
  変えると、移動を既定にするアプリへ落としたときに原本が移動する (§6.4 の理由が残る)。
- **Q3 ツリー右クリックの範囲**: (a) 「貼り付け」だけ **[推奨]**、(b) Windows のフォルダの右クリックメニュー全体 (大きい・別設計)。
  クリップボードが画像だけのとき、Ctrl+V は画像をファイルとして保存するが、ツリーの貼り付けは (a) ではファイルだけにする。
- 外して報告する既存の課題 (今回は直さない): 一覧へのドロップは `Copy-Item -Force` で**同名ファイルを確認なしに上書き**する
  (file-drag-drop-design §11.1)。同じ Shell の転送に揃えれば衝突確認が出るが、リリース済みの挙動を変えるので別項目で判断する。

## 11. 文書の更新 (実装時)

- マニュアル: `htdocs/mimageviewer/manual/grid.html` (ツリーの節) と `tut-file-ops.html`。修飾キー、行を先に表示しておくこと、取り消しは
  エクスプローラーで行うこと、評価・タグは先で「編集内容の復元」。
- 設計: 本書、[file-drag-drop-design.md](file-drag-drop-design.md) (§11 受け取りの drop target が mIV 自前になったこと、
  `internal_drop_pending` の撤去)、[folder-tree-refresh-plan.md](folder-tree-refresh-plan.md) (操作完了の producer)、
  [file-organize-destinations-plan.md](file-organize-destinations-plan.md) (request owner の共有)、`docs/README.md`、`docs/spec.md`。
- キー操作は追加しない (右ボタン・D&D・修飾キーは固定入力。`docs/keymap-spec.md` の固定入力に理由を書く)。
