# UI スレッドに残る同期処理 3 件 (§1.861・§1.859・§1.853)

状態: 設計 r1 (2026-11-24)。担当: 設計・検収 = ClaudeCode Opus 5.5 (ui-sync-853)、実装 = Codex GPT-6.1 Sol / high、
独立レビュー = GPT-6.1 Sol / xhigh。ブランチ `ui-sync-853` (`G:\home\mimageviewer-uisync853`)。

観測の出どころ: 下の時間はすべてサブ PC / サブ PC2 担当の perf ログ (next-release-backlog の各節)。本担当は実行時の
挙動を観測していない。コードの記述は master `361411c72` の読み取り。

まれな失敗 (worker の起動失敗・Shell の異常・ディスク I/O エラー等) は既定の割り切り (ログ + 通知)。再試行・回復の
仕組みは足さない。

---

## 1. §1.861 リンクを開く処理を UI スレッドから外す

### 1.1 同期の経路

- `ui_helpers::open_url(url)` (`src/ui_helpers.rs:3317`) → `external_links::open_url` → `opener::open` → `ShellExecuteW`。
  呼び出し 13 か所 (マニュアル 7 か所、更新通知、What's New、初回設定、ネットワーク data dir 通知、音楽パネル、
  native_video、metadata panel)。結果は全部ログだけ。
- 同型の入口 (同じ `opener::open` を UI から呼ぶ、結果はログだけか無視):
  - `ui_helpers::open_external_player(path)` (`ui_helpers.rs:3281`、呼び出し 6 か所: 外部プレイヤー・フォルダを開く)。
  - `ui_metadata_panel.rs:7733/7743/7779` の「元ツイートを開く」等 3 か所 (`let _ = opener::open(url)`)。
  - `ui_metadata_panel.rs:2948/7391` の「Google マップで開く」(`external_links::open_url` の Err で警告を出す)。
- 範囲外 (結果を画面へ返す別の作り。今回は変えない): `agent_integration.rs:611` の `codex://settings`、
  `external_tool.rs:2310` の OS 既定アプリ起動 (結果を `ExternalLaunchOutcome` で返す)、
  `filename_stack_script` の writer (既に worker)。

### 1.2 worker 化の形

- `open_recycle_bin_async` と同じ「短命の名前付き thread を 1 本立てて投げ放し」。UI は URL の検査
  (`normalize_http_url`、メモリだけ) と spawn だけを行う。
- worker の中で COM STA を初期化してから `opener::open` を呼ぶ (`ShellExecute` は COM を使う handler に委譲し得る。
  既存の `open_with::initialize_com_sta` を使う)。
- 共通の 1 関数 `external_links::spawn_open(target: OpenTarget, on_failure: Option<FailureSink>)` に集約し、
  `ui_helpers::open_url` / `open_external_player` / ツイートの 3 か所 / マップから呼ぶ。spawn 失敗はログ (+ マップは警告)。
- 起動の順序・重複は保証しない (人の操作の速さでしか来ない。2 回押せば 2 回開くのは従来どおり)。

### 1.3 完了の受け取り

- 一般の呼び出しは従来どおり失敗をログに残すだけ (URL をログに書く既存の挙動も維持。マップだけは書かない)。
- マップだけは失敗時に「Google マップを開けませんでした」を出していた。これを残すため、App が持つ受信口
  `external_open_failures: mpsc::Receiver<ExternalOpenFailure>` を 1 つ置き、root の既存 poll 群 (early return より前、
  `poll_file_organize` と同じ場所) で `try_recv` して `show_feedback_warning` する。worker は失敗時だけ送り、
  ROOT の repaint を要求する。`draw_exif_panel` は関数ポインタで開く代わりに「マップを開く」要求を返し、App が送信口を
  渡して spawn する (描画関数は App を持たないため)。
- 取消: なし (ShellExecute は取り消せない。開くかどうかは押した時点で確定)。終了時は join しない。
- 別の窓: 受信口は App に 1 つ (process 共通の通知)。警告は従来と同じ `show_feedback_warning` (surface 指定なし)。

### 1.4 状態の組み合わせ

新しい状態は「マップ失敗の受信口」1 つだけ。待ち・世代・取消はない (投げ放しで、UI 側の状態が結果に依存しない)。
モーダル化は不要 (UI が待つ処理がない)。

### 1.5 計測とテスト

- worker で `perf::event("ext", "open_end", ...)` (`kind=url|player|tweet|map`、`ms`、`ok`)。`is_enabled` の外ガード。
- テスト: cfg(test) の opener 差し替えで、(1) `open_url` が呼び出しスレッドで opener を呼ばずに戻り、別スレッドで
  1 回だけ呼ぶ、(2) 不正 URL は spawn しない、(3) マップの失敗が root poll で警告になる、(4) 一般の失敗は通知を出さない。

---

## 2. §1.859 フォルダの「貼り付け」を UI スレッドから外す

### 2.1 同期の経路と同型の入口

`native_context_menu::invoke_shell_folder_background_verb(hwnd, folder, Paste)` (bind → `QueryContextMenu` →
`InvokeCommand`) を UI スレッドで呼ぶ入口が 3 つある。
1. ツリーの右クリック「貼り付け」: `ui_folder_pane.rs:1217` → `paste_tree_files_to_folder` (`app/main_drop.rs:317`)。
2. 一覧の Ctrl+V (ファイルのとき): `app.rs:64697` → `paste_files_to_folder`。
3. 一覧の右クリック「貼り付け」(mIV メニュー): `ui_dialogs/context_menu.rs:1947` (同じ処理の複製)。
3 つとも同じ問題なので同じ関数にまとめて worker へ移す。Windows の Shell メニュー (`show_native_context_menu`) の
中の動詞は OS のメニューの中で実行されるので範囲外。

既存の決定 ([folder-tree-dnd-plan.md](folder-tree-dnd-plan.md) §7、ui-responsiveness §4) は、r2 レビューで worker 化に
次の問題が出たため同期にしていた。今回の設計でそれぞれをどう消すかを示す。

| r2 の問題 | 今回の扱い |
| --- | --- |
| クリック時と別のクリップボードを貼り得る | UI がクリックの時点で `GetClipboardSequenceNumber` を控え、worker は `InvokeCommand` の直前に同じ値かを確かめる。違えば貼らずに「クリップボードの内容が変わったため貼り付けませんでした」。確認から Shell の読み取りまでの間の変化は残るが、Explorer の貼り付けと同じ幅 |
| 切り取りの data object の apartment | worker は使い捨てではなく、**アプリの寿命のあいだ生きてメッセージを回す STA thread 1 本** (`shell-paste`) にする。`OleInitialize` 済みで、Shell が `OleGetClipboard` で作る包み object と、Shell の転送 thread がそれへ持つ proxy は、この thread が生きて pump している限り有効 (今の UI スレッドで呼ぶ場合と同じ条件)。mIV 自身の切り取り (`LocalCutDataObject`) は UI スレッドの apartment にあり、Shell からの `SetData` (Paste Succeeded 等) は COM が UI スレッドへ運ぶ。UI スレッドは winit の message loop で pump しているので届く。UI スレッドはこの thread を待たない (join しない) ので、互いに待つ形は生じない |
| worker の OLE 初期化 | 上の常駐 thread の起動時に 1 回 `OleInitialize`。失敗したら以後の貼り付けを「貼り付けを開始できませんでした」で断る (ログ + 通知) |
| 選択要求の取り違え | `PostOperationSelection` に要求番号を持たせ、貼り付けの完了が失敗のとき「自分の番号のときだけ」取り下げる。間に別の操作が要求を置き換えていれば何もしない |

### 2.2 worker 化の形

- `ShellPasteService` (process 共通、App が 1 つ持つ): 初回の貼り付けで `shell-paste` thread を起こす。thread は
  `OleInitialize` → `MsgWaitForMultipleObjects` で要求の到着とメッセージの両方を待ち、到着した要求を 1 件ずつ処理する
  (bind → query → 列の確認 → `InvokeCommand`)。既存の `verb_shell_bind` / `verb_query_shell` / `verb_invoke` の計測は
  そのまま thread 内で出る。結果は要求ごとの `mpsc::Sender` へ返し、ROOT の repaint を要求する。
- App の受付: 1 か所の関数 `start_shell_paste(ctx, folder, origin)` に 3 入口を集める。順序は従来の同期版と同じ:
  (ツリーだけ) 述語の再確認と本棚の拒否 → `admit_book_append_followup_mutation()` → 「貼り付け中」なら断る →
  先が今の実フォルダのときだけ選択要求を作る (番号を控える) → クリップボードの番号を控える → 要求を送る。
- 同時に 1 件。`shell_paste_request: Option<ShellPasteRunning { rx, selection_id, origin, started_at, perf_seq }>`。
  実行中の 2 回目の貼り付けは「貼り付けが完了するまでお待ちください」で断る (従来は UI が止まっている間の 2 回目が
  後で処理された。止まらなくなる代わりに断る。退行ではないと判断)。
- 整理操作との関係: ツリーの貼り付けは従来どおり `shell_transfer_admission` (整理・D&D 転送が実行中なら断る) を通す。
  逆に整理・D&D 転送の受付も貼り付け中は断る (`shell_transfer_admission` に 1 行足す。従来は貼り付け中に UI が止まって
  いたので受け付けられなかった)。Ctrl+V・一覧メニューは整理の実行中も従来どおり受け付ける (今の挙動を変えない)。

### 2.3 完了の受け取り

- root の poll (`poll_file_organize` の隣、early return より前) で `try_recv`。
  - `Ok`: repaint を要求 (従来と同じ。貼られたファイルの一覧への反映は既存のフォルダ変更検知と選択要求が担う)。
  - `Err(ClipboardChanged)`: 通知 (上の文言) + 自分の番号なら選択要求を取り下げ。
  - その他の `Err`: 従来どおりログだけ + 自分の番号なら選択要求を取り下げ。
  - `Disconnected` (thread の異常): ログ + 「貼り付けを完了できませんでした」+ 取り下げ。
- 完了時に `resync_egui_modifiers_from_os` を呼ぶ (従来は同期呼び出しの直後。Shell の画面がフォーカスを取った場合の
  修飾キーの食い違いを同じく直す)。

### 2.4 取消・終了・別の窓

- 取消: 利用者の取消は Shell の画面 (進捗・衝突) が持つ。mIV 側の取消はない (従来どおり)。
- 終了: thread も受信口も join・待機しない。終了の受付で実行中の貼り付けを待たない (従来も、Shell が転送 thread で
  進める部分は終了で止まり得た。D&D 転送と同じ割り切り)。
- 別の窓: ツリーは ROOT だけ。Ctrl+V・一覧メニューの選択要求は受付時の context の `post_operation_selection` に置かれる。
  完了時に別の context が投影されていれば、番号が一致しないので取り下げは何もしない (元の context の要求は既存の
  10 秒の期限で消える)。HWND は main の HWND (従来どおり)。

### 2.5 状態の組み合わせを減らす検討

- モーダル化: 不採用。貼り付けの呼び出しは通常 1 秒未満で、待ちの表示がちらつくだけになる。止める必要のある入力の
  組み合わせは「2 回目の貼り付け」と「整理・D&D 転送」だけで、受付で断れば消える。
- 使い捨て thread: 不採用 (上の apartment の問題)。常駐 thread 1 本 + 同時に 1 件で、要求の順序・apartment・初期化の
  組み合わせを消す。
- 既存の整理の owner (`FileOrganizeRequest::Running`) へ入れる案: 不採用。Ctrl+V が整理の実行中に使えなくなる
  (今の通常の使い方の退行)。受付の相互拒否だけを共有する。

### 2.6 計測とテスト

- `perf::event("shell_paste", "submit", ...)` (UI、`origin=tree|ctrl_v|grid_menu`)、`"end"` (poll、`ms` は受付から
  完了まで、`outcome`)。worker 内の既存 3 区間はそのまま。
- テスト (handler-level、Shell 呼び出しは差し替え): (1) 3 入口とも呼び出し側スレッドで Shell を呼ばずに戻り、要求が
  1 件出る、(2) 実行中の 2 回目と、実行中の整理・D&D の受付が断られる (逆も)、(3) 失敗の完了で自分の選択要求だけが
  消え、間に置き換わった要求は残る、(4) クリップボード番号の不一致で貼らずに通知、(5) 成功の完了で選択要求が残り
  repaint が出る、(6) thread の切断で通知、(7) ツリーの本棚拒否・述語の拒否は従来どおり受付で起きる。
  常駐 thread の OLE/pump は ignored の実 Shell テスト (一時フォルダ、ファイルの CF_HDROP をクリップボードへ置いて
  貼る) で確かめる (設計担当が run-heavy で流す。製品は起動しない)。

---

## 3. §1.853 UI スレッドのフォルダ走査

### 3.1 残っている経路 (sort-worker-849-plan §8.2 の棚卸しと照合)

共通の同期走査点は `load_folder_with_scan_claimed_with_search_origin` の `pre_scan = None` →
`scan_directory_with_settings`。そこへ `DirectoryScanRoute::UiThread` または `pre_scan = None` で届く経路:

| # | 経路 | 今回 |
| --- | --- | --- |
| A | 起動時: 引数・転送・agent・ブックマークの解決完了 (`apply_startup_open_path_resolve_result`、`startup_ops.rs:1948`) | **対象** |
| A' | 起動時の既定の場所: `open_default_startup_target` (前回の一覧 `open_previous_startup_list`、Desktop / 指定フォルダ) と、その fallback 呼び出し (起動 resolve の失敗、Remote 返却 `remote_ipc/ui.rs:4657`) | **対象** (同じ resolver に載せる) |
| B | 場所の移動: `dispatch_location_jump` / `dispatch_address_input` の `UiThread` (場所▼・既知フォルダ・整理先・アドレス入力の特殊な場所・ゲームパッドの場所 picker 等の `LocationJumpTarget::Physical`) | **統括の判断待ち** (§3.4) |
| C | `post_render_main_navigation` の既定 (一覧のフォルダを開く・親へ・コレクション物理元・フォルダを開くダイアログ・コンテキストの open) | 対象外 (§3.5) |
| D | `load_folder_or_convert_archive_with_auto_fullscreen_owned_at_stage` の他の呼び出し元 (通常の読み直し・全画面を閉じた後の Direct・コンテナを開く操作 等、約 20 か所) | 対象外 (§3.5) |
| E | `open_direct_navigation_target` の「元の場所へ」(ArchiveContainer、多くは pre_scan あり) | 対象外 (§3.5) |

### 3.2 A・A' の設計: 既存の起動 resolver の worker で走査まで済ませる

既に承認済みの [startup-diagnostics-plan.md](startup-diagnostics-plan.md) §6 (Phase B、未実装) を実装する形にする。
新しい pending・owner は作らない。

- **同じ worker で解決と走査**: `resolve_startup_open_path` (worker) の結果が実フォルダ (`OpenablePathKind::Directory`
  かつ `is_virtual_folder` でない) なら、同じ worker で走査し `StartupOpenPathResolveResult.folder_scan:
  Option<Result<ScannedDir, String>>` に入れる。走査の設定は受付時に UI がメモリから控える (`FolderScanSnapshot`:
  変換書庫・EPUB の扱い、隠しファイル、file-type visibility、長さ順の要否と cache dir)。この控えと走査関数の呼び方は
  `start_folder_open_scan_with_restore_and_navigation_at_stage` と共有する 1 つの helper に切り出す (複製しない)。
  取消は resolver の既存の cancel flag を走査にも渡す。
- **採用**: `apply_startup_open_path_resolve_result` は、走査済みならその `ScannedDir` を既存の
  `..._classified_with_navigation_at_stage(…, pre_scan = Some(scan), …)` に渡す。走査の失敗は従来の同期走査の失敗と
  同じ処理 (ログ + 「フォルダを読み取れませんでした」) に渡し、UI で再走査しない。worker が走査しなかった種類
  (ファイル・ZIP/PDF・変換書庫・RAR の証明待ち・detached へ渡すブックマーク) は今の経路のまま。
- **既定の場所 (A')**: `open_default_startup_target` を同期の解決・読込から、同じ resolver への「既定の場所」要求に
  変える (startup-diagnostics-plan §6.1 の typed input)。worker が `known_folders::startup_folder` (Previous は
  `startup_list_restore` の target、祖先への遡上、Desktop の fallback を含む) を実行し、実フォルダなら続けて走査する。
  採用は今の同期版の UI 側の処理 (Previous の `RestoreList { target, cursor }`、それ以外のカーソル名の
  `select_after_load` / 行位置の設定と失敗時の巻き戻し、ドライブ一覧・閲覧履歴の分岐) を同じ順序で行い、loader に
  pre_scan を渡す。ドライブ一覧・閲覧履歴のモードは今のまま (走査しない)。
- **既定の場所への fallback**: 明示の起動先が開けなかったときの既定の場所 (`startup_ops.rs:937/1040/1134/1149`) と
  Remote 返却 (`remote_ipc/ui.rs:4657`) も、同じ非同期の既定の場所要求にする。Explicit → Default の 1 回だけで、
  Default 自身の失敗は終端 (再帰しない)。
- **spawn 失敗・切断**: startup-diagnostics-plan §6.2.1 のとおり、UI スレッドでの同期 fallback (`startup_ops.rs:770` 付近)
  を全経路で外し、ログ + 「場所を開く準備ができませんでした。もう一度開いてください」で終える。
- **退役**: 既存の `cancel_unresolved_open_for_navigation` / `retire_replaced_main_list_requests` がそのまま resolver を
  退役させる (§1.285・§1.419 の規則を変えない)。後から来た移動に、遅れて着いた既定の場所が上書きしない。
- **待ちの間の入力**: main の resolver の待ちは、走査を含むと長くなり得る。待ちの間は §1.805 の「場所の移動待ち」と同じに
  する (一覧の項目入力を止め、500 ms を超えたら「移動先を読み込み中…」、移動の入力は後勝ち)。`grid_item_input_block`
  の `DirectNavigationScan` の理由に main の resolver の待ちを加える (detached lease のあるブックマークは除く)。
  起動直後は一覧が空なので実質の差はない。起動中の mIV へ別のフォルダを渡したとき (activation) だけ、待ちの間の
  キーが旧一覧に効かなくなる (従来は UI が止まり、キーは新しい一覧で処理されていた)。
- **Phase B のうち今回やらないもの**: 「最初の通常の present の後に dispatch する」起床の契約 (§6.1 の Deferred wake)。
  解決・走査が UI スレッドを止めなくなれば、dispatch の時点を遅らせなくても最初の画面は描ける。今の dispatch の時点
  (`update_phases.rs:533` 等) は変えない。startup-diagnostics-plan §10 に「Phase B の off-UI 部分は §1.853 で実装、
  Deferred wake は残り」と記録する。

### 3.3 計測

- worker で `perf::event("nav", "startup_resolve_scan", ...)`: `resolve_ms`、`scan_ms`、`folders`、`media`、
  `outcome` (`scanned|not_directory|failed|cancelled`)、`source` (`initial|activation|agent|bookmark|default`)。
- 採用側の既存 `nav/lf_scan` は `pre_scanned=true` になる。`scripts/analyze_perf.py` の startup/nav 内訳に足す。

### 3.4 B (場所の移動) — 統括の判断待ち

§1.805 の `DirectoryScanRoute::Worker` (`DirectNavigation`) をそのまま使い、`dispatch_location_jump` /
`dispatch_address_input` の検索の外の `Physical` を `Worker` にする。§1.805 と同じく、待ちの間の一覧の項目入力が止まる
(お気に入り・アドレス欄 Enter で利用者が 2026-11-18 に了承した扱いの拡張)。判断が出るまで実装しない。

### 3.5 対象外にした経路と理由

- C・D は、一覧のフォルダを開く (Enter・ダブルクリック)・親へ・読み直し・全画面を閉じた後の移動を含む。worker 化すると、
  走査待ちの間に押したキー (Enter 直後の矢印など) が捨てられる (§1.805 §3.2 で利用者が 2026-11-18 に範囲から外した
  理由と同じ退行)。先行入力の扱いを利用者が決めてから別項目で扱う。
- E は多くの場合 pre_scan を持ち、未走査に届く条件がまれ。C と同じ判断に含める。

### 3.6 状態の組み合わせを減らす検討

- 新しい pending を作らず、既存の起動 resolver (1 つの owner) に走査を載せる。退役・世代・採用は既存のまま。
- 既定の場所も同じ resolver に載せ、`open_default_startup_target` の同期経路を消す (2 つの経路を 1 つにする)。
- モーダル化: 不採用。遅い共有フォルダの起動先から、別の場所へ移る操作まで止める理由がない (startup-diagnostics-plan
  §6.2 末尾と同じ判断)。

### 3.7 テスト

- 起動・activation・agent・ブックマークの実フォルダ: dispatch したフレームと採用のフレームで UI の同期走査が 0 回
  (§1.805 の cfg(test) 計数)。採用後の一覧・選択・履歴・カーソル復元が従来と同じ。
- 既定の場所: Previous (同じ場所のカーソル復元、消えたフォルダの祖先、Desktop fallback)、Desktop、指定フォルダ、
  ドライブ一覧、閲覧履歴。明示の起動先の失敗 → 既定の場所が 1 回だけ。Remote 返却。
- 退役: 解決・走査中の別の移動 (ツリー・お気に入り・DFS) が勝ち、遅れた結果は採用されない。
- spawn 失敗: UI で解決・走査せず、通知で終わる。走査の失敗: 従来と同じ通知、一覧は変わらない。
- 待ちの間: main の項目入力が止まり、移動の入力は通る。detached のブックマークは止めない。
- 既存テストのうち、起動の同期採用を前提にしたものは、worker 完了 + poll を回す helper へ直す (期待値は変えない)。

---

## 4. 文書の更新 (実装時)

- [ui-responsiveness.md](ui-responsiveness.md): 上部に本件の節。§4 の「ツリーの右クリック『貼り付け』は … 既知の例外」の
  段落を書き換える。
- [async-architecture.md](async-architecture.md): `shell-paste` 常駐 thread、起動 resolver の走査、§1.861 の投げ放し。
- [folder-tree-dnd-plan.md](folder-tree-dnd-plan.md) §7: 今回 worker 化したことと理由。
- [startup-diagnostics-plan.md](startup-diagnostics-plan.md) §10: Phase B の実施範囲。
- next-release-backlog の §1.861・§1.859・§1.853 の状態。

## 5. 記録

(設計レビュー・実装・テストの記録をここへ足す。受けていないレビューの結果を書かない。)
