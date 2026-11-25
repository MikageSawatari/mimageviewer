# UI スレッドに残る同期処理 3 件 (§1.861・§1.859・§1.853)

状態: 設計 r4 (2026-11-25、§2 のクリップボードを利用者決定 §0 で確定。§1・§3 は r3 のまま実装済み)。担当: 設計・検収 = ClaudeCode Opus 5.5 (ui-sync-853)、実装 = Codex GPT-6.1 Sol / high、
独立レビュー = GPT-6.1 Sol / xhigh。ブランチ `ui-sync-853` (`G:\home\mimageviewer-uisync853`)。

観測の出どころ: 下の時間はすべてサブ PC / サブ PC2 担当の perf ログ (next-release-backlog の各節)。本担当は実行時の
挙動を観測していない。コードの記述は master `361411c72` の読み取り。

まれな失敗 (worker の起動失敗・Shell の異常・ディスク I/O エラー等) は既定の割り切り (ログ + 通知)。再試行・回復の
仕組みは足さない。

## 0. 利用者の決定 (2026-11-24、統括経由)

設計担当が統括へ出した範囲の質問に、利用者が次の選択肢を選んだ (統括からの伝達のとおり)。

- §1.853: 「担当の案で進める (Recommended)」。worker にするのは (a) 起動時と (b) 場所の移動だけ。その待ちの間は、
  お気に入りと同じく一覧へのキーを止める。残りの経路 (§3.5 の C・D) と「元の場所へ」(E) は別の項目にする
  (番号は統括が master で取る)。
- §1.859: 「断るでよい (Recommended)」。貼り付けの処理中の 2 回目は、断って通知を出す。
- §1.861: 「案のとおり進めてください」。
- §1.859 のクリップボードの扱い (2026-11-25、設計レビュー r3 の後に統括経由で質問): 「準備中は mIV の切り取りを断る
  (Recommended)」。貼るのは Shell が読む時点のクリップボード。貼り付けの準備中は、mIV 自身のファイルの切り取り・
  コピーを断って通知を出す (§2.1・§2.2、設計 r4)。

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
  既存の `open_with::initialize_com_sta` (今は private) と、その戻り値の `ComApartmentGuard` (field は private のまま) を
  `pub(crate)` にして共有する)。
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
中の動詞は OS のメニューの中で実行されるので範囲外。r2 (設計レビュー r1 P2-4): Shell メニューの「貼り付け」は
`post_operation_selection` を作らず (`context_menu.rs:1340` は本への追記の受付だけ)、Shell 同士の並行はエクスプローラーで
2 つの貼り付けを続けた場合と同じく Shell が扱う。下の「同時に 1 件」は、mIV の 3 入口が持つ選択要求と受付の組み合わせを
減らすための制約で、Shell メニューの動詞を含む全貼り付けの直列化は約束しない。

既存の決定 ([folder-tree-dnd-plan.md](folder-tree-dnd-plan.md) §7、ui-responsiveness §4) は、r2 レビューで worker 化に
次の問題が出たため同期にしていた。今回の設計でそれぞれをどう消すかを示す。

| r2 の問題 | 今回の扱い |
| --- | --- |
| クリック時と別のクリップボードを貼り得る | **r4 (利用者決定 §0、2026-11-25)**: 貼るのは **Shell が `InvokeCommand` の中で読む時点のクリップボード**。mIV はクリップボードを読まず、照合もしない (r3 の中身の照合は撤回。CF_HDROP の無い形式・読み取りの失敗の扱いが増え続けたため、設計レビュー r3)。押してから Shell が読むまで (貼り付け先の bind・query。遅いネットワーク先では数秒) の間に**ほかのアプリ (エクスプローラー等) で**切り取った・コピーした物は、新しい物が貼られる。これは従来の同期版でも同じ (mIV が止まっている間もほかのアプリは動く)。従来なかったのは「準備中に mIV 自身で切り取る・コピーする」組み合わせだけなので、それを受付で断る: mIV のファイルの切り取り・コピーが必ず通る `invoke_shell_clipboard_verb_for_paths` (`app.rs:64455`。Ctrl+X / Ctrl+C の `invoke_shell_clipboard_verb` と、一覧の右クリックの「切り取り」「コピー」 `context_menu.rs:1802` の両方がここを通る) の先頭で、貼り付けの準備中なら「貼り付けが完了するまでお待ちください」と通知して何もしない (`reserve_clipboard_write_sequence` や `begin_local_write` より前の、読み取りだけの判定)。Windows の Shell メニューの中の「切り取り」「コピー」は OS のメニューの動詞で、ほかのアプリと同じ扱い (断らない)。画像・テキストのコピーはファイルを置かないので、貼り付けでファイルが作られることはなく、断らない。r1 の番号照合、クリック時の `IDataObject` を worker へ運ぶ案も採らない |
| 切り取りの data object の apartment | worker は使い捨てではなく、**アプリの寿命のあいだ生きてメッセージを回す STA thread 1 本** (`shell-paste`) にする。`OleInitialize` 済みで、Shell が `OleGetClipboard` で作る包み object と、Shell の転送 thread がそれへ持つ proxy は、この thread が生きて pump している限り有効 (今の UI スレッドで呼ぶ場合と同じ条件)。mIV 自身の切り取り (`LocalCutDataObject`) は UI スレッドの apartment にあり、Shell からの `SetData` (Paste Succeeded 等) は COM が UI スレッドへ運ぶ。UI スレッドは winit の message loop で pump しているので届く。UI スレッドはこの thread を待たない (join しない) ので、互いに待つ形は生じない |
| worker の OLE 初期化 | 上の常駐 thread の起動時に 1 回 `OleInitialize`。失敗したら以後の貼り付けを「貼り付けを開始できませんでした」で断る (ログ + 通知) |
| 選択要求の取り違え | `PostOperationSelection` に要求番号を持たせ、貼り付けの完了が失敗のとき「自分の番号のときだけ」取り下げる。間に別の操作 (新しいフォルダの作成 `new_folder.rs:101` など) が要求を置き換えていれば何もしない。番号は `request_post_operation_selection` が一か所で振る (全 producer 共通) |

### 2.2 worker 化の形

- `ShellPasteService` (process 共通、App が 1 つ持つ): 初回の貼り付けで `shell-paste` thread を起こす。thread は
  `OleInitialize` → `MsgWaitForMultipleObjects` で要求の到着とメッセージの両方を待ち、到着した要求を 1 件ずつ処理する
  (bind → query → `InvokeCommand`。r4 でクリップボードの読み取り・照合は外した)。既存の `verb_shell_bind` / `verb_query_shell` / `verb_invoke` の計測は
  そのまま thread 内で出る。結果は要求ごとの `mpsc::Sender` へ返し、ROOT の repaint を要求する。結果が表すのは
  **動詞の呼び出しの完了**で、転送の完了ではない (転送は Shell が別に進める。名前・ログも invocation と書く)。
- thread は App の lock や UI が持つ資源を持たずに Shell を呼ぶ。UI から thread への起動・終了の同期の握手はしない
  (起動は spawn だけ、要求は channel と event で送る)。
- App の受付: 1 か所の関数 `start_shell_paste(ctx, folder, origin)` に 3 入口を集める。r3 (設計レビュー r2 P2-4):
  読み取りだけの拒否判定を先に全部済ませ、副作用のある受付はその後にする。順序: 「貼り付け中」なら断る →
  (ツリーだけ) 述語の再確認と本棚の拒否 (`shell_transfer_submission_admission_with`、読み取りだけ) →
  `admit_book_append_followup_mutation()` (本への追記の取り消し記録を消す副作用あり。ここより前で断れば記録は残る) →
  先が今の実フォルダのときだけ選択要求を作る (番号を控える) → 要求を送る。
- 同時に 1 件。`shell_paste_request: Option<ShellPasteRunning { rx, selection_id, origin, started_at, perf_seq }>`。
  実行中の 2 回目の貼り付けは「貼り付けが完了するまでお待ちください」で断る (従来は UI が止まっている間の 2 回目が
  後で処理された。止まらなくなる代わりに断る。退行ではないと判断)。
- 整理操作との関係: ツリーの貼り付けは従来どおり `shell_transfer_admission` (整理・D&D 転送が実行中なら断る) を通す。
  逆に整理・D&D 転送の受付も貼り付け中は断る (`shell_transfer_admission` に 1 行足す。従来は貼り付け中に UI が止まって
  いたので受け付けられなかった)。Ctrl+V・一覧メニューは整理の実行中も従来どおり受け付ける (今の挙動を変えない)。

### 2.3 完了の受け取り

- root の poll (`poll_file_organize` の隣、early return より前) で `try_recv`。
  - `Ok`: repaint を要求 (従来と同じ。貼られたファイルの一覧への反映は既存のフォルダ変更検知と選択要求が担う)。
  - `Err`: 従来どおりログだけ + 自分の番号なら選択要求を取り下げ。
  - `Disconnected` (thread の異常): ログ + 「貼り付けを完了できませんでした」+ 取り下げ。
- 完了時に `resync_egui_modifiers_from_os` を呼ぶ (従来は同期呼び出しの直後。Shell の画面がフォーカスを取った場合の
  修飾キーの食い違いを同じく直す)。

### 2.4 取消・終了・別の窓

- 取消: 利用者の取消は Shell の画面 (進捗・衝突) が持つ。mIV 側の取消はない (従来どおり)。
- 終了: thread も受信口も join・待機しない。終了の受付で実行中の貼り付けを待たない (従来も、Shell が転送 thread で
  進める部分は終了で止まり得た。D&D 転送と同じ割り切り)。
- 別の窓: ツリーは ROOT だけ。`post_operation_selection` は App に 1 つ (context の bundle には入っていない、
  `app.rs:17073`。消費は `apply_post_operation_checks` が同じ field を take する)。r2: 番号は App 共通の field に対して
  照合するので、完了までの間に別の producer (新しいフォルダ等) が置き換えた要求は消さない。context の切り替え・
  Quick Folder の切り替えで要求がどう扱われるかは既存の `decide` (フォルダ一致と 10 秒の期限) のまま変えない。
  HWND は main の HWND (従来どおり)。

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
  1 件出る、(2) 実行中の 2 回目が通知付きで断られ、実行中の整理・D&D の受付も断られる (逆に整理の実行中はツリーの
  貼り付けが断られ、Ctrl+V・一覧メニューは受け付ける)、(3) 失敗の完了で自分の選択要求だけが消え、間に実際の
  新しいフォルダの完了が置き換えた要求は残る、(4) 成功の完了で選択要求が残り repaint が出る、(5) thread の切断で通知、
  (6) ツリーの本棚拒否・述語の拒否は従来どおり受付で起きる、(7) 選択要求の番号が全 producer で一意に振られ、既存の
  take / 戻しの経路で番号が保たれる、(8) 断られた 2 回目の貼り付けで本への追記の取り消し記録と revision が変わらない、
  (9) r4: 貼り付けの準備中は、Ctrl+X / Ctrl+C と一覧の右クリックの「切り取り」「コピー」が通知付きで断られ、
  クリップボードの書き込み (`reserve_clipboard_write_sequence`・`begin_local_write`・Shell の動詞) が起きない。完了後は
  従来どおり受け付ける。画像のコピーは準備中も受け付ける。
- ignored の実 Shell テスト (一時フォルダ、設計担当が run-heavy で流す。製品は起動しない): (a) ファイルの CF_HDROP を
  クリップボードへ置いて常駐 thread で貼る、(b) **mIV の切り取り**: 別の pump する STA thread (UI 役) が既存の
  `LocalCutDataObject` 包みを `OleSetClipboard` し、常駐 thread の貼り付けの `InvokeCommand` が戻った後に、Shell からの
  完了の `SetData` が UI 役の apartment で観測され、元が移動することを確かめる (両 thread とも pump を続ける)。
- 実機で見る項目 (統括が手配): 衝突の確認・取消の Windows の画面が、別 thread から main の窓を持ち主にして正しく前面に
  出て、閉じた後に mIV の入力が戻ること。

---

## 3. §1.853 UI スレッドのフォルダ走査

### 3.1 残っている経路 (sort-worker-849-plan §8.2 の棚卸しと照合)

共通の同期走査点は `load_folder_with_scan_claimed_with_search_origin` の `pre_scan = None` →
`scan_directory_with_settings`。そこへ `DirectoryScanRoute::UiThread` または `pre_scan = None` で届く経路:

| # | 経路 | 今回 |
| --- | --- | --- |
| A | 起動時: 引数・転送・agent・ブックマークの解決完了 (`apply_startup_open_path_resolve_result`、`startup_ops.rs:1948`) | **対象** |
| A' | 起動時の既定の場所: `open_default_startup_target` (前回の一覧 `open_previous_startup_list`、Desktop / 指定フォルダ) と、その fallback 呼び出し (起動 resolve の失敗、Remote 返却 `remote_ipc/ui.rs:4657`) | **対象** (同じ resolver に載せる) |
| B | 場所の移動: `dispatch_location_jump` / `dispatch_address_input` の `UiThread` (場所▼・既知フォルダ・整理先・アドレス入力の特殊な場所・ゲームパッドの場所 picker 等の `LocationJumpTarget::Physical`) | **対象** (§3.4、利用者決定 §0) |
| C | `post_render_main_navigation` の既定 (一覧のフォルダを開く・親へ・コレクション物理元・フォルダを開くダイアログ・コンテキストの open) | 対象外 (§3.5) |
| D | `load_folder_or_convert_archive_with_auto_fullscreen_owned_at_stage` の他の呼び出し元 (通常の読み直し・全画面を閉じた後の Direct・コンテナを開く操作 等、約 20 か所) | 対象外 (§3.5) |
| E | `open_direct_navigation_target` の「元の場所へ」(ArchiveContainer、多くは pre_scan あり) | 対象外 (§3.5) |

### 3.2 A・A' の設計: 既存の起動 resolver の worker で走査まで済ませる

既に承認済みの [startup-diagnostics-plan.md](startup-diagnostics-plan.md) §6 (Phase B、未実装) を実装する形にする。
新しい pending・owner は作らない。

- **同じ worker で解決と走査**: `resolve_startup_open_path` (worker) の結果が `OpenablePathKind::Directory` なら、
  同じ worker で走査し `StartupOpenPathResolveResult.folder_scan: Option<Result<ScannedDir, String>>` に入れる。
  r2 (設計レビュー r1 P2-5): 判定は resolver が実体で確かめた `Directory` だけを使い、拡張子の述語
  (`is_virtual_folder`) で外さない (`photos.zip` という名前の実フォルダも worker で走査する)。要求したファイルの
  親フォルダへ解決した場合も同じ。走査の設定は受付時に UI がメモリから控える (`FolderScanPolicy`:
  変換書庫・EPUB の扱い、隠しファイル、file-type visibility、長さ順の要否と cache dir)。この控えと走査関数の呼び方は
  `start_folder_open_scan_with_restore_and_navigation_at_stage` と共有する 1 つの helper に切り出す (複製しない)。
  取消は resolver の既存の cancel flag を走査にも渡す。
- **走査の段階の印**: worker は走査に入る直前に、pending と共有する `scan_phase` (開始時刻を持つ atomic 1 つ) を立てる。
  下の入力の停止・待機表示・ブックマークの期限は、すべてこの 1 つの述語 `main_startup_scan_elapsed()` (main の
  resolver で、detached lease の無い要求、かつ走査の段階) から読む。解決だけの段階 (今もある待ち) の挙動は変えない。
- **採用**: `apply_startup_open_path_resolve_result` は、走査済みならその `ScannedDir` を既存の
  `..._classified_with_navigation_at_stage(…, pre_scan = Some(scan), …)` に渡す。走査の失敗は従来の同期走査の失敗と
  同じ処理 (ログ + 「フォルダを読み取れませんでした」) に渡し、UI で再走査しない。worker が走査しなかった種類
  (ファイル・ZIP/PDF・変換書庫・RAR の証明待ち・detached へ渡すブックマーク) は今の経路のまま。
  RAR の証明不足の分類経路、要求したファイルの選択・hydration の続き (`select_requested_file` / `DirectMedia`)、
  既定の場所の `auto_fullscreen=false`、activation の保留・Remote の延期は今の順序のまま。
- **走査の設定の照合** (r2、設計レビュー r1 P2-3。r3 で r2 P2-2・P2-3 を反映): 結果は走査に使った `FolderScanPolicy`
  を持つ。policy には r3 で **Susie の対応拡張子の世代** (`susie_loader` に世代番号を置き、起動時の読み込み完了と
  `reload` で進める) を加える (走査は `is_recognized_image_ext` で live の Susie を見るため)。worker は走査の前後で
  世代を読み、変わっていれば「混ざった」結果として扱う。
  poll は結果を受け取ったとき、**pending を取り出す前に**今の設定から作った policy と比べる。違えば (走査中に
  隠しファイル・種類の表示・書庫/EPUB の扱い・Susie を変えた) その結果を捨て、**同じ pending を取り出さずに**、
  解決済みの行き先に対する「走査だけの続き」の worker を今の policy で出し、pending の受信口だけを差し替える
  (cancel token・owner・navigation・lease・保留中の activation・診断 span・走査の段階の開始時刻はそのまま)。
  解決・受付 (`start_startup_open_request_with_navigation` の admission) は繰り返さない。新しい要求としては扱わない
  (退役・履歴・test-script の数を変えない)。走査の段階は最初の走査の開始から最後の結果まで続くので、ブックマークの
  期限にも数えない。設定の変更が普通の操作なので割り切りではなく構造で扱う。
  ツリー・§1.805 の pre_scan も同じ穴を持つが、リリース済みの経路なので今回は変えず、別項目の候補として報告する。
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
- **待ちの間の入力** (r2、設計レビュー r1 P2-2・P3-8): **走査の段階だけ**を §1.805 の「場所の移動待ち」と同じにする
  (一覧の項目入力を止め、走査が 500 ms を超えたら「移動先を読み込み中…」、移動の入力は後勝ち)。従来 UI が止まって
  いたのはこの段階だけなので、利用者決定 (§0) の範囲に収まる。解決の段階 (今も非同期で、一覧を操作でき、別の
  ブックマークも選べる) は止めない。`grid_item_input_block` の `DirectNavigationScan`、`location_navigation_pending`、
  `direct_navigation_wait_overlay_delay` の経過時間の 3 か所が同じ `main_startup_scan_elapsed()` を読む。
  この述語は要求の owner だけでなく、今投影している context が main であること (`app.rs:31180` 付近の
  `main_folder_history_available` と同じ判定) も見る (drive jump の `drive_jump_pending` と同じ形。兄弟 context の
  回帰は `drive_jump_tests.rs:900` に倣う)。ブックマークの期限の免除は、その pending が同じブックマークの
  `request_id` を持つときだけ。
  既存の 400 ms の「パスを確認しています…」通知は解決の段階だけに出す (走査の段階に入っていれば出さず、待機表示に任せる)。
  detached lease のあるブックマークの待ちでは何も止めない。
- **ブックマークの 45 秒の期限** (r2、設計レビュー r1 P2-6): `poll_bookmark_book_open` の `Resolving` の期限
  (`app.rs:55643`) は解決の段階の期限として残し、走査の段階のあいだは数えない (変換中の
  `archive_convert_owns_bookmark_request` と同じ扱い)。従来も同期の走査中は期限が働かなかった。遅い共有フォルダの正常な
  走査を期限で取り消さない。
- **Phase B のうち今回やらないもの**: 「最初の通常の present の後に dispatch する」起床の契約 (§6.1 の Deferred wake)。
  解決・走査が UI スレッドを止めなくなれば、dispatch の時点を遅らせなくても最初の画面は描ける。今の dispatch の時点
  (`update_phases.rs:533` 等) は変えない。startup-diagnostics-plan §10 に「Phase B の off-UI 部分は §1.853 で実装、
  Deferred wake は残り」と記録する。Phase A の診断 span (`InitialTargetResolve` / `InitialTargetScan` /
  `InitialTargetAdopt`) は、非同期になった実際の区間 (worker の解決・worker の走査・UI の採用) を囲むように付け直す。

### 3.3 計測

- worker で `perf::event("nav", "startup_resolve_scan", ...)`: `resolve_ms`、`scan_ms`、`folders`、`media`、
  `outcome` (`scanned|not_directory|failed|cancelled`)、`source` (`initial|activation|agent|bookmark|default`)。
- 採用側の既存 `nav/lf_scan` は `pre_scanned=true` になる。`scripts/analyze_perf.py` の startup/nav 内訳に足す。

### 3.4 B (場所の移動)

§1.805 の `DirectoryScanRoute::Worker` (`DirectNavigation`) をそのまま使い、`dispatch_location_jump` /
`dispatch_address_input` の検索の外の `Physical` を `Worker` にする。§1.805 と同じく、待ちの間の一覧の項目入力が止まる
(お気に入り・アドレス欄 Enter で利用者が 2026-11-18 に了承した扱いの拡張。2026-11-24 の利用者決定で採用、§0)。

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
  (§1.805 の cfg(test) 計数)。採用後の一覧・選択・履歴・カーソル復元が従来と同じ。拡張子付きの名前の実フォルダ
  (`x.zip` / `x.pdf` / `x.epub` という名前のフォルダ) と、要求した画像の親フォルダへの解決も worker で走査する。
- 走査中の設定変更: 走査を止めた状態で隠しファイル・種類の表示・書庫/EPUB の扱い・Susie の世代を変え、採用では
  古い結果を使わず同じ要求のまま今の設定で走査し直す (履歴・要求数・診断の終端は 1 回分、解決は繰り返さない)。
  ブックマークで「走査が 45 秒を超える → 設定の不一致 → 走査し直し」でも取り消されない。
- 兄弟 context: detached を投影している間は main の走査待ちで入力を止めない。
- 段階: 解決の段階では入力を止めず (従来どおり)、走査の段階に入ると止まる。待機表示は走査が 500 ms を超えてから。
  400 ms の通知は解決の段階だけ。detached のブックマークの待ちは止めない。
- ブックマーク: 走査の段階が 45 秒を超えても取り消されず、解決の段階の 45 秒の期限は従来どおり (時計は差し替え)。
- RAR の証明不足・activation の保留・Remote の延期と返却・要求したファイルの選択の続きが従来と同じ。
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

### 5.1 設計レビュー r1 (GPT-6.1 Sol / xhigh、read-only、5a9d19a4b、2026-11-24): REJECT

P2×6・P3×2。設計担当がコードで照合し、すべて r2 で設計を直した。

| 指摘 | 対応 (r2) |
| --- | --- |
| P2-1 クリップボード番号の照合は遅延レンダリングで誤って断る | 照合をやめ、同一性はまれな組み合わせとして割り切る (§2.1 の表) |
| P2-2 resolver の待ち全体を止めると、今ある解決中の操作 (別のブックマーク等) まで止まる | 走査の段階だけを止める (§3.2) |
| P2-3 走査中の設定変更で古い走査が採られる | 走査の設定を照合し、違えば同じ要求のまま走査し直す (§3.2) |
| P2-4 Shell メニューの貼り付けは単一実行の外 | 選択要求を持たない Shell 同士の並行は Shell の扱い。約束の範囲を書き直した (§2.1) |
| P2-5 `is_virtual_folder` で実フォルダを外すと同期走査が残る | resolver の `Directory` だけで判定 (§3.2) |
| P2-6 ブックマークの 45 秒の期限が遅い走査を取り消す | 走査の段階は期限に数えない (§3.2) |
| P3-7 選択要求は App に 1 つ (context ごとではない) | 前提を直し、番号を一か所で振る (§2.1・§2.4) |
| P3-8 待機表示と入力停止の述語が別 | 1 つの述語に揃え、400 ms の通知と整理 (§3.2) |

ほか: 常駐 STA は apartment の寿命の設計として成立 (レビュー担当の所見)。切り取りの完了 `SetData` の ignored 実テストと、
別 thread を持ち主にした Shell の画面の実機確認を足した (§2.6)。

### 5.2 設計レビュー r2 (同じ担当の継続、cf1d91516、2026-11-24): REJECT

r1 の P2-2・P2-4・P2-5・P3-7・P3-8 は解消と判定。新たに P2×4・P3×1。設計担当がコードで照合し、すべて r3 で直した。

| 指摘 | 対応 (r3) |
| --- | --- |
| P2-1 準備中に別の物を切り取ると、それが貼られる (通常の操作順で起き得る) | 受付直後と呼び出し直前にクリップボードの中身 (パス一覧と切り取り/コピー) を比べ、違えば断って通知 (§2.1) |
| P2-2 設定の不一致での出し直しが、期限・受付・診断の寿命を壊す | pending を取り出さずに、走査だけの続きで受信口を差し替える (§3.2) |
| P2-3 policy に Susie の対応拡張子が入っていない | Susie の世代を policy に加える (§3.2) |
| P2-4 2 回目の貼り付けを断る前に、本への追記の取り消し記録を消してしまう | 読み取りだけの拒否を先に済ませる順序に変えた (§2.2) |
| P3-5 COM の guard の型も公開が要る | 型も `pub(crate)` に (§1.2) |

実装時の注意 (レビュー担当): 述語は今投影している context が main かも見る。ブックマークの期限の免除は同じ request_id のときだけ (§3.2 に反映)。
注: この回は `codex exec resume` のため sandbox が workspace-write で起動した (resume は sandbox を指定できない)。
レビュー担当はファイルを変えていないと報告し、作業ツリーにも変更は無かった。

### 5.3 設計レビュー r3 (同じ担当の継続、read-only、8e3b11ea2、2026-11-25): REJECT (§2 のクリップボードのみ)

r2 の P2-2・P2-3・P2-4・P3-5 は解消、P2-1 は一部解消と判定。§1 と §3 に未解決の指摘は無い。新たな指摘は §2 の
クリップボードの照合だけ: P2-1 CF_HDROP の無い形式 (`FileGroupDescriptorW`・`Shell IDList Array`、
`clipboard_capture/data.rs:70`) を照合できない、P2-2 照合の読み取り失敗が黙って貼り付けを止める、P3-3 残る幅が
「数 ms」とは言えない。照合で塞ぐと形式・失敗の組み合わせが増え続けるため、§2.1 のクリップボードの扱いは
利用者の判断を統括経由で求めた (2026-11-25)。判断が出るまで §2 は実装しない。

§3 の実装時の注意 (レビュー担当): 受信口を差し替えるときは、解決済みの payload (要求したファイルの準備を含む) を
全部保つ。走査し直しの取消・`Disconnected` の変種もテストする。`Disconnected` は §6.2.1 の終端 (ログ + 通知) にし、
今の起動時の既定の場所への fallback (`startup_ops.rs:935`) を戻さない。Susie の世代は、対応拡張子の公開と同時に進める
(`susie_loader` の既存の `INIT_GENERATION` は処理の入口で進むもので、公開された拡張子の版ではない)。

### 5.4 実装の記録（2026-11-25、§1・§3 のみ、未コミット）

§2 は判断待ちのためコードも対応文書も変更していない。コンパイル・Rust テスト・cargo fmt/check/clippy・
ビルド・製品起動は利用者の指示により未実行。独立実装レビューは未実施。

追補（2026-11-25、`79cc5bf58` 後）: 設計担当から run-heavy の関連 nextest 448 件中446件成功・2件失敗の報告を受け、コードを照合して修正した。`location_and_address_plain_physical_dispatch_scan_off_caller_and_adopt_once` は Quick Folder が有効でも未使用の `folder_nav_back_stack` を数えていたテスト前提の誤りで、`src/app/location_jump.rs` の同テストを active な履歴の参照へ変更し、slot 無し/A の両方と再 poll 後の履歴 snapshot 不変を検査する。`startup_default_specific_worker_preserves_cursor_only_at_the_same_place` の「同じ場所なら b を復元する」期待は §3.2 どおりで、実装側では App へ置いたヒントが `adopt_main_list_navigation` の `navigation.selection_after_load` 公開で上書きされていたため、`src/app/startup_ops.rs` で既定先のヒント（別の場所なら None）を採用要求自身へ渡すよう修正した。`src/app/startup_default_tests.rs` の同テストには競合する受付前ヒントを置き、置換/クリアと消費を追加検査した。再受付・新しい状態・同期 I/O は追加していない。この修正のコンパイル・Rust テストは未実行で、cargo・git の commit/stash/reset/checkout・製品起動も行っていない。

| 変更ファイル | 設計節と実装内容 |
| --- | --- |
| `src/external_links.rs`、`src/open_with.rs`、`src/ui_helpers.rs` | §1.2・§1.5。HTTP(S) の在メモリ検査、共通 spawn、短命 COM STA worker、失敗のログと map 通知、計測、opener 差し替え |
| `src/ui_metadata_panel.rs`、`src/app/external_open.rs` | §1.3。tweet 3 入口、EXIF 描画から map 要求を返す経路、map 受付と root 通知 poll、描画/handler テスト |
| `src/app.rs`、`src/app/update_phases.rs` | §1.3 と §3.2。App 共通の map 受信口と早期 return 前の poll、resolver の走査 payload/段階、同じ bookmark request ID の期限免除、既存 pane 走査の共通 policy 利用 |
| `src/app/startup_ops.rs`、`src/app/startup_list_restore.rs` | §3.2・§3.3・§5.3。既存 resolver の解決＋走査、typed Default、Previous/カーソルの継続、同期 fallback 撤去、設定照合後の受信口交換、payload/lease/held/時計/診断の保持、Remote 取消時の入力種別、worker 計測 |
| `src/app/folder_scan.rs`、`src/susie_loader.rs` | §3.2。共通 `FolderScanPolicy`、Susie pool と対応拡張子世代の同時公開、publication lock 外の旧 pool 終了 |
| `src/app/location_jump.rs`、`src/filename_stack_ui.rs` | §3.2・§3.4。検索外 Physical の Worker、入力停止・場所待ち・500 ms 表示猶予を同じ main 走査述語へ接続 |
| `scripts/analyze_perf.py` | §3.3。startup/nav の worker 解決・走査時間と source/outcome/件数の内訳 |
| `src/app/startup_scan_tests.rs`、`src/app/startup_scan_policy_tests.rs`、`src/app/startup_default_tests.rs` | §3.7・§5.3。実 worker と制御した受信口による handler/状態遷移テスト |
| `src/app/tests.rs`、`src/app/tests/startup_restore.rs`、`src/app/tests/direct_navigation_worker_scan.rs`、`src/app/rar_archive_cache_tests.rs` | §3.7。既定起動/Previous の worker 完了＋poll helper、B の producer を同期テストから worker テストへ移動。既存の採用結果の期待値は維持 |
| `src/app/subfolder_expansion.rs`、`src/app/browser818_presentation_tests.rs` | §3.7。既存の制御した resolver fixture に新しい payload/段階の初期値を追加 |
| `docs/ui-responsiveness.md`、`docs/async-architecture.md`、`docs/startup-diagnostics-plan.md`、`docs/next-release-backlog.md`、本書 | §4。§1・§3 の実装範囲、検証待ち、Phase B の Deferred wake 残件を記録。貼り付けの既知例外は維持 |
| `docs/detached-rework-plan.md` §11 | main 投影と detached lease の所有境界を使う走査述語の到達範囲を記録。viewport/placement/既存 detached 分岐は変更しない |

設計との差・コード照合で明確になった点:

- §1.5 の extras `kind=url|player|tweet|map` は `perf::event` の予約キーで、イベント名 `open_end` と衝突する。
  分類キーだけ `target_kind` とした。`ext/open_end` は維持する。
- §3.7 の「走査の失敗で一覧は変わらない」は、明示 InitialStartup の失敗について既存コードと読み分けが要る。
  従来も loader の `Ignored` → `NotOpenable` → 既定先へ一度だけ fallback するので、この既存挙動は維持した。
  失敗した同じ場所を UI で再走査することはない。Default 自身の失敗と spawn 失敗/切断は終端とする。
- Default の RAR は従来の owned loader の error/dialog 契約を保ち、明示起動先だけが持っていた resolver 側の
  巻拒否通知へ変更しない。RAR 証明不足の明示要求は既存の分類継続へ渡す。
- Default は `StartupResolveInput::Default` とし、pending には typed `input_kind` を保持する。Remote 取得時に
  placeholder の空パスを明示要求として延期しないためで、Default は既存の返却時の「採用済み一覧を優先、空なら既定先」へ戻す。
  Agent の source も同じ型で保持し、走査だけの継続でも計測上 Activation と取り違えない。
- Default の解決済みの行き先が現一覧と同じなら、捕捉済み navigation の history operation だけを
  `SameLocation` とする。従来の default loader と同じ履歴の扱いを保ち、受付/証明を取り直さない。
- 走査だけの継続の計測は `resolve_ms=0`。元 payload の `elapsed_ms` は保持し、解決を再実行した時間として重複集計しない。
- Desktop のテストは実 known-folder パスの解決までとし、一覧の採用は隔離した一時フォルダの実 scan payload で検査する。
  利用者の Desktop 内容の列挙や、製品バイナリによる描画 snapshot は行っていない。

実行した非 cargo の点検: `rustfmt --edition 2024 --check --config skip_children=true`、`git diff --check`、
`python scripts/check_ui_glyphs.py` (危険 glyph 0)、`scripts/analyze_perf.py` の構文検査と合成ログの startup/nav 内訳。
これらはコンパイル・Rust テスト・実機の動作/性能の証拠ではない。

残件: 設計担当の共有 runner でコンパイルと下記テスト、独立実装レビュー、必要な実機計測。
ツリー・§1.805 の既存 pre_scan の設定照合は範囲外の残件候補、Phase B の Deferred normal-present wake も未実装のまま。

追加・更新したテスト名（すべて Rust 実行は未実施）:

新規 (40 件):

`src/app/external_open.rs`

```text
external_open_map_failure_repaints_root_and_root_poll_warns
external_open_general_failure_does_not_publish_notification
external_open_invalid_map_request_warns_without_calling_shell
```

`src/app/folder_scan.rs`

```text
section1853_policy_snapshot_tracks_all_scan_inputs
section1853_policy_scan_preserves_visibility_and_cancellation
```

`src/app/location_jump.rs`

```text
location_and_address_plain_physical_dispatch_scan_off_caller_and_adopt_once
```

`src/external_links.rs`

```text
external_open_url_returns_before_shell_and_opens_once_on_worker
external_open_invalid_urls_never_spawn
external_open_player_and_tweet_use_the_same_worker_boundary
```

`src/susie_loader.rs`

```text
section1853_extensions_generation_advances_only_with_publication
```

`src/app/startup_default_tests.rs`

```text
startup_default_previous_worker_restores_same_folder_cursor_and_rows
startup_default_previous_missing_folder_uses_nearest_ancestor_without_old_cursor
startup_default_same_folder_keeps_existing_history_without_an_extra_departure
startup_default_specific_worker_preserves_cursor_only_at_the_same_place
startup_default_desktop_and_previous_desktop_fallback_keep_typed_list_continuations
startup_default_drives_reading_history_and_previous_drive_list_do_not_scan
startup_default_explicit_not_openable_falls_back_once_and_default_failures_are_terminal
startup_default_remote_return_redispatches_asynchronously_without_empty_explicit_request
startup_default_remote_return_preserves_an_already_adopted_local_folder
startup_default_late_tree_favorite_and_dfs_requests_retire_the_old_default
```

`src/app/startup_scan_policy_tests.rs`

```text
section1853_hidden_setting_rescans_same_request_before_adoption
section1853_visibility_setting_rescans_same_request_before_adoption
section1853_archive_setting_rescans_same_request_before_adoption
section1853_epub_setting_rescans_same_request_before_adoption
section1853_susie_publication_rescans_same_request_before_adoption
section1853_mixed_recognition_rescans_even_with_equal_policy
section1853_scan_continuation_keeps_pending_identity_and_full_resolved_payload
section1853_scan_continuation_cancel_discards_late_result
section1853_scan_continuation_disconnected_is_terminal_without_default_fallback
```

`src/app/startup_scan_tests.rs`

```text
section1853_initial_activation_agent_and_bookmark_directories_scan_only_on_worker
section1853_archive_suffix_directories_keep_directory_worker_classification
section1853_requested_image_parent_scans_off_caller_and_keeps_file_selection
section1853_resolve_and_scan_share_stage_predicate_and_visual_grace
section1853_main_scan_does_not_block_independent_mounted_grid_or_detached_owner
section1853_bookmark_scan_timeout_exemption_requires_same_request_and_scan_stage
section1853_spawn_failure_and_disconnected_initial_are_terminal_without_default_fallback
section1853_activation_spawn_failure_and_disconnect_resume_exact_held_resolver
section1853_policy_rescan_spawn_failure_is_terminal_without_synchronous_retry
section1853_detached_bookmark_resolver_worker_does_not_scan_or_block_main
section1853_bookmark_over_deadline_keeps_timeout_exemption_through_policy_rescan
```

本体・fixture 初期値を直接更新 (26 件):

`src/app/browser818_presentation_tests.rs`

```text
browser818_admission_snapshot_refusal_focuses_main_from_resolved_request
browser818_presentation_activation_restore_materialized_keeps_request_seed
```

`src/app/tests.rs`

```text
snapshot_refusal_preserves_unresolved_startup_open_owner
refused_snapshot_activation_resumes_held_bookmark_resolver
cached_converted_archive_keeps_bookmark_owner_through_cache_zip_load
startup_open_path_resolve_shows_delayed_toast
startup_open_path_resolve_cancels_previous_pending
normal_navigation_cancels_slow_bookmark_resolve_and_ignores_late_result
detached_grid_archive_claim_supersedes_slow_bookmark_resolve
replacing_bookmark_a_with_b_cannot_cancel_or_apply_over_b
quick_folder_none_startup_pdf_password_cancel_exit_reload_preserves_ownership
ignored_epub_bookmark_resolver_keeps_refusal_feedback
detached_media_bookmark_resolve_cancel_preserves_main_and_target_until_exact_retirement
remote_acquire_retires_startup_resolves_and_rejects_new_activations
section1335_direct_zip_exit_restart_keeps_explicit_parent_list
```

`src/app/tests/direct_navigation_worker_scan.rs`

```text
direct_navigation_worker_dispatch_has_zero_caller_scans_and_adopts_once
direct_navigation_worker_unmarked_producers_keep_synchronous_adoption
```

`src/app/tests/startup_restore.rs`

```text
section1335_drive_and_other_startup_modes_retain_their_explicit_dispatch
section1335_first_app_startup_migrates_missing_record_and_second_startup_keeps_new_owner
section1335_previous_zip_missing_or_recapsed_prefix_rejects_old_same_name_cursor
section1335_departing_same_items_page_open_captures_list_cursor_before_page_navigation
section1335_synthetic_install_captures_changed_physical_departure_without_manual_capture
section1335_startup_cursor_rows_survive_grid_page_acceptance_before_first_scroll_layout
section1335_bookshelf_compiled_folder_direct_read_and_explicit_list_restart_differ
```

`src/ui_metadata_panel.rs`

```text
gps_maps_panel_click_uses_current_photo_and_spread_side
gps_maps_panel_hides_invalid_missing_and_filtered_locations
```

以下はテスト本体の期待値を変えず、更新した共有 helper (`exit_and_restart` / `restart_previous_from_saved_settings` /
`section1285_resolver` / `section1285_result` / `section1285_resolver_result` / RAR の `open_entry`) を使う既存テスト。
filter を選ぶ際の関連範囲として列挙する。これらも Rust 実行は未実施。

`src/app/rar_archive_cache_tests.rs`

```text
rar_archive_cache_first_volume_normal_open_reuses_live_cached_zip
rar_archive_cache_first_volume_smart_open_reuses_live_cached_zip
rar_archive_cache_first_volume_history_open_reuses_live_cached_zip
rar_archive_cache_first_volume_rating_open_reuses_live_cached_zip
rar_archive_cache_first_volume_collection_open_reuses_live_cached_zip
rar_archive_cache_later_volume_normal_open_refuses_first_cache
rar_archive_cache_later_volume_smart_open_refuses_first_cache
rar_archive_cache_later_volume_history_open_refuses_first_cache
rar_archive_cache_later_volume_rating_open_refuses_first_cache
rar_archive_cache_later_volume_collection_open_refuses_first_cache
rar_archive_cache_later_volume_startup_restore_refuses_first_cache
rar_archive_cache_later_volume_bookmark_handler_refuses_first_cache
rar_archive_cache_later_volume_detached_grid_handler_refuses_first_cache
rar_archive_cache_later_volume_direct_without_cache_is_refused
```

`src/app/startup_ops.rs`

```text
browser818_open_owner_same_path_new_request_cancels_only_old_receiver
browser818_open_owner_remote_suspension_keeps_initial_identity
realmachine50_cancelled_bookmark_resolve_does_not_handoff_live_owner
realmachine50_stale_bookmark_resolve_does_not_handoff_live_owner
section1419_candidate_main_continuation_keeps_provisional_activation
section1419_retired_held_detached_context_does_not_panic_on_refusal_or_replacement
section1419_failed_activation_does_not_resume_the_retired_pane
section1285_new_main_requests_retire_startup_and_activation_resolvers
section1473_history_menu_retires_startup_even_through_transparent_search
section1285_reload_keeps_unresolved_initial_request_without_user_navigation
section1285_fix_and_unfix_list_retire_older_main_resolvers
section1285_remote_retains_deferred_target_through_operations_and_local_reload
section1285_main_collection_keeps_detached_bookmark_even_under_activation
section1285_classification_handoffs_preserve_later_activation
section1285_candidate_main_join_retires_older_startup_and_activation
section1285_accepted_pane_join_preserves_later_activation
section1285_explicit_zip_level_moves_retire_unresolved_main_opens
section1285_zip_root_back_without_a_move_keeps_unresolved_open
section1285_untouched_initial_resolver_still_opens_its_target
```

`src/app/subfolder_expansion.rs`

```text
section1285_subfolder_scan_handoff_preserves_later_activation
```

`src/app/tests.rs`

```text
quick_folder_restart_after_b_exit_preserves_a_workspace
quick_folder_restart_after_a_exit_preserves_b_workspace
quick_folder_restart_after_none_exit_keeps_both_workspaces
quick_folder_restart_after_none_collection_exit_keeps_both_workspaces
quick_folder_restart_after_none_smart_folder_exit_keeps_both_workspaces
```

`src/app/tests/startup_restore.rs`

```text
section1335_explicit_book_list_and_direct_read_restart_choose_different_lists
section1335_back_to_list_acceptance_captures_viewed_page_instead_of_grid_selection
section1335_quit_immediately_after_transition_acceptance_restores_accepted_book_list
section1335_nested_zip_previous_rebuilds_effective_prefix_and_back_stack
section1335_converted_archive_records_logical_source_and_keeps_it_on_restart
section1335_tray_direct_book_quit_while_hidden_restarts_at_preserved_parent_list
section1335_collection_ctrl_down_zip_adoption_restores_with_sidecars_on_and_off
section1335_video_ring_close_accepts_current_folder_list_for_exit_and_restart
section1335_extra_video_native_window_close_rejects_stale_then_restores_folder_list
section1335_extra_video_native_close_fullscreen_rejects_stale_then_restores_folder_list
section1335_extra_video_parked_native_closes_preserve_main_list_restore
section1335_extra_video_egui_close_key_restores_folder_list
section1335_extra_video_normal_parent_return_still_restores_parent_list
section1335_tray_image_folder_book_direct_quit_restarts_parent_and_preserves_resume
section1335_tray_image_folder_book_backspace_quit_restarts_explicit_book_list
```

### 5.5 設計 r4 (2026-11-25、§2 のクリップボードのみ)

利用者決定 (§0) を受けて、§2.1 の表の 1 行目・§2.2・§2.3・§2.6 の (9) を直した。r3 の中身の照合 (`ClipboardChanged`)
は撤回し、mIV 自身のファイルの切り取り・コピーを貼り付けの準備中に断る受付 1 か所に置き換えた。§1・§3 は変えていない。

### 5.6 設計レビュー r4 (同じ担当の継続、read-only、18d6d2fd2、2026-11-25): ACCEPT (§2)

指摘なし。ファイルの切り取り・コピーの入口 (キー、カスタムキー、全画面、ゲームパッド・リング・ジェスチャ) はすべて
`invoke_shell_clipboard_verb_for_paths` に集まり、断る位置は書き込みの予約・切り取り監視・Shell の動詞より前と確認された。
Remote・agent にファイルのクリップボード操作は無く、ドラッグはクリップボードを使わない。

### 5.7 実装の記録（2026-11-25、§2、未コミット）

§2.1〜2.4 を実装。3 入口は `start_shell_paste` に集約し、読み取りだけの拒否を先に行う。
常駐 STA は初回投入で起動し、OLE 初期化とメッセージポンプを保ち、要求別の動詞の呼び出し結果を root poll へ返す。
UI は起動・呼び出し・終了を待たず、クリップボードの読み取り・照合は追加していない。
番号付き選択要求、実行中の相互拒否、ファイルの切り取り・コピーの先頭ガードと §2.6 の計測を追加した。

| 設計節 | 変更ファイル | 内容 |
| --- | --- | --- |
| §2.1〜2.4・§2.6 | `src/shell_paste.rs`、`src/lib.rs` | 常駐 `shell-paste` STA、channel / event / メッセージポンプ、要求別の完了と ROOT repaint、join なし。テストでは App ごとの fake worker を使う |
| §2.1〜2.4・§2.6 | `src/app.rs`、`src/app/main_drop.rs`、`src/app/update_phases.rs`、`src/ui_dialogs/context_menu.rs` | 3 入口の共通受付、整理・D&D との受付、共通ファイル切り取り・コピーの先頭ガード、root poll、修飾キー同期、perf |
| §2.1・§2.3・§2.4 | `src/post_operation_selection.rs`、`src/app.rs` | 共通受付だけで番号を発行、take / 戻しで保存、失敗完了は自分の番号だけを取り下げる |
| §2.6 | `src/app/shell_paste_tests.rs`、`src/app/main_drop_tests.rs`、`src/ui_folder_pane_drop_tests.rs` | handler-level / 状態遷移の追加と既存同期テストの worker 完了 + poll への移行。期待値は維持 |
| §2.6 | `src/shell_paste_native_tests.rs`、`src/cut_clipboard.rs`、`src/ui_dialogs/context_menu.rs` | ignored の実 Shell テスト、既存の完了通知の thread 観測と clipboard 書き込み前の副作用を検査する test-only の観測点 |
| §4（§2 の分） | `docs/ui-responsiveness.md`、`docs/async-architecture.md`、`docs/folder-tree-dnd-plan.md`、`docs/next-release-backlog.md`、本書 | 同期呼び出しの既知の例外を置き換え、常駐 STA の理由と寿命・受付・検証待ちを記録 |

追加テスト（通常 13 件、実 Shell ignored 2 件）:

```text
shell_paste_runs_off_caller_and_reuses_worker_after_invocation
shell_paste_submission_and_drop_do_not_wait_for_invocation
shell_paste_tree_ctrl_v_and_grid_menu_invoke_once_off_caller_and_keep_origin
shell_paste_duplicate_preserves_undo_receipt_revision_and_exact_selection
shell_paste_running_refuses_tree_transfer_and_organize_before_mutation
shell_paste_organize_running_refuses_tree_but_preserves_ctrl_v_and_grid_menu
shell_paste_failed_completion_retires_only_its_selection_and_new_folder_completion_survives
shell_paste_success_keeps_selection_repaints_root_and_resyncs_modifiers_once
shell_paste_disconnected_completion_warns_and_retires_its_selection
shell_paste_tree_refusals_resolve_books_only_after_predicate_and_keep_selection
shell_paste_selection_ids_are_unique_and_survive_wait_apply_take_restore
shell_paste_pending_file_cut_copy_have_no_write_side_effects_and_completion_restores_admission
shell_paste_pending_preserves_grid_and_fullscreen_image_copy_dispatch
shell_paste_native_hdrop_copy_uses_resident_sta
shell_paste_native_local_cut_completes_on_original_sta
```

既存テストの変更（3 件）:

```text
main_drop_paste_selection_belongs_only_to_the_current_real_folder
main_drop_paste_to_other_folder_creates_no_selection_and_ctrl_v_uses_the_same_helper
main_drop_books_rejection_and_default_resolution_are_shared_by_all_operation_boundaries
```

共有 `TreeDropTest` fixture の非同期化が適用される既存テスト（12 件）:

```text
folder_tree_drop_secondary_click_keeps_navigation_cursor_and_selection_and_pastes_to_captured_row
folder_tree_drop_paste_disabled_reasons_and_wheel_do_not_leak
folder_tree_drop_menu_rechecks_admission_before_paste
folder_tree_drop_books_menu_stays_enabled_until_operation_time_refusal
folder_tree_drop_menu_is_retired_when_chrome_or_tree_is_hidden_or_rows_disappear
folder_tree_drop_row_includes_arrow_label_and_trailing_area_but_excludes_leading_indent
folder_tree_drop_visible_empty_pane_keeps_header_and_blank_area_out_of_grid_route
folder_tree_drop_two_pass_release_keeps_the_first_menu_path_and_clipboard_query
folder_tree_drop_disabled_pane_retires_the_menu_but_keeps_visible_panel_geometry
folder_tree_drop_menu_escape_uses_normal_keyboard_route_without_changing_background_focus
folder_tree_drop_menu_arrows_and_enter_use_normal_keyboard_route_without_opening_tree_folder
folder_tree_drop_menu_blocks_grid_shortcut_phase_and_resumes_after_escape
```

`PostOperationSelection::new` の番号引数だけを補った既存テスト（11 件、期待値の変更なし）:

```text
a_created_folder_is_found_wherever_the_sort_put_it
a_paste_is_recognised_by_what_was_not_there_before
a_name_the_shell_renamed_on_collision_is_still_picked_up
moving_to_another_folder_before_it_finishes_leaves_that_list_alone
an_output_hidden_by_the_filter_is_waited_for_rather_than_forced_into_view
a_paste_that_produced_nothing_is_dropped_instead_of_waiting_forever
a_long_paste_keeps_selecting_the_files_as_they_arrive
the_request_lets_go_once_the_user_moves_the_selection
a_request_that_has_not_applied_yet_owns_nothing_and_keeps_waiting
a_single_output_is_still_ours_with_nothing_checked
waiting_does_not_expire_while_files_keep_arriving
```

設計と違えた点: 実 Shell テスト (b) は `InvokeCommand` の戻りを受信した後も両 STA を生かし、
実際の `SetData` 完了通知の到着 apartment とファイルの移動を検査するが、通知が戻りより後に発生したこと自体は要求しない。
速い転送では戻り前にも通知が届き得るため、OS の時機による不安定な期待値を避けた。
製品の寿命・完了の契約に変更はない。戻り後の callback を必ず発生させる条件の検証は残る。
コンパイル・Rust テスト・実 Shell テスト・製品での衝突画面と取消の確認は未実施。
利用者の制約に従い cargo・製品起動・commit / stash / reset / checkout は行っていない。
変更した Rust ファイルへの直接の `rustfmt --check`、`git diff --check`、
`python scripts/check_ui_glyphs.py` は成功（危険な UI 字形 0 件）。これはコンパイル・テストの結果ではない。

独立実装レビュー r1 の P2-1・P2-2 対応: `src/native_context_menu.rs` の `ClipboardDataObject` を `Agile = false` とし、STA 専用の inner `IDataObject` への委譲を所有 apartment に戻す。クリップボードを使わず fake inner と STA 間 marshal proxy で検査する `clipboard_data_object_does_not_implement_iagile_object`、`clipboard_data_object_marshaling_delegates_getdata_and_setdata_to_owner_sta` を追加。`src/app.rs` と `src/app/startup_ops.rs` は、同じ request_id のメディアブックマークについて走査中の30秒期限を免除し、policy 再走査を含む連続した走査時間を採用時に一度だけ pending の起点へ加算する。既存の activation 保留による期限延長と走査時間の重複は二重に控除しない。本は既存の `begin_page_wait` が採用後の `AwaitingPage` に新しい起点を作るため変更不要と確認した。`src/app/startup_scan_tests.rs` に、時計を渡して検査する `section1853_media_bookmark_scan_timeout_exemption_requires_same_request_and_scan_stage`、`section1853_media_bookmark_policy_rescans_credit_scan_once_before_player_wait`、`section1853_media_bookmark_scan_adoption_never_credits_another_request`、`section1853_media_bookmark_activation_hold_and_scan_credit_do_not_overlap`、`section1853_book_bookmark_page_wait_starts_fresh_after_scan` を追加（既存テストの期待値変更なし）。cargo・コンパイル・Rust テスト・製品起動は未実施、commit / stash / reset / checkout は行っていない。

時刻差し替えテストの修正: `startup_scan_tests.rs` と `startup_scan_policy_tests.rs` の本ブランチで追加したテストから、`Instant::now()` を過去へ引く設定を除いた。起点を現在時刻に置き、期限・再走査・表示猶予は未来の観測時刻を `_at` helper に渡して検査する。走査の印だけが必要なケースは経過時間を作らず開始済みの phase を使う。本の poll と走査時間・表示猶予の取得にも観測時刻を渡す seam を設け、通常の呼び出しは現在時刻を渡す。ブランチ開始前からあるテストと期待する結果は変更せず、cargo・Rust テスト・製品起動は未実施。

### 5.8 独立実装レビュー (GPT-6.1 Sol / xhigh、read-only、設計・実装とは別の担当、2026-11-25)

- r1 (d3e0e7aed): REJECT、P2×2。P2-1 `ClipboardDataObject` が agile (FTM) のままで、別 STA の貼り付けから Shell の
  STA 専用 object へ直接委譲され得る → `Agile = false` と marshal の単体テスト。P2-2 動画・音声ブックマークの 30 秒の期限が
  走査の段階を免除していない → 同じ request の走査中は免除し、走査時間を採用時に 1 回だけ起点へ足す。いずれも設計担当が
  コードで確かめて正しいと判断し、7cad8585d で直した。
- r2 (7cad8585d): ACCEPT-WITH-CHANGES。r1 の 2 件は解消。P3×1 (新しいテストが `Instant::now()` から過去を引き、起動直後の
  環境で underflow し得る) → 基準を「今」に置き、`_at` の seam に未来の時刻を渡す形に直した (次のコミット)。
- 設計担当が run-heavy で流した関連の filter: 最終 818 件すべて成功。本体 lib 全件は流していない (統括が master で流す)。
  ignored の実 Shell テスト 2 件 (`shell_paste_native_*`) はデスクトップのクリップボードを置き換えるため、メイン PC では
  流していない。

### 5.9 実 Shell テスト (2026-11-25、統括の依頼)

ignored の `shell_paste_native_hdrop_copy_uses_resident_sta` と `shell_paste_native_local_cut_completes_on_original_sta` を、
設計担当が開発機 (メイン PC) で run-heavy 経由・1 件ずつ (`--run-ignored only --test-threads 1`) 流し、2 件とも成功した
(c89d38eb0)。製品は起動していない。衝突・取消の Windows の画面の確認 (§2.6 の実機で見る項目) は未実施。

### 5.10 master との merge (v5.7.0 後)

Susie の公開状態は master の `PoolState` / `replace()` / `pool_snapshot()` / `pool_revision()` に統一し、
本ブランチの `PublishedPool` / `publish()` / `extensions_generation()` を廃止した。起動走査の前後照合と
`FolderScanPolicy` は同じ pool revision を使う。版は master の `checked_add` を維持し、pool と同じ lock 内で
更新する。初期化・reload とも旧 pool の Drop は write lock を離してから行う。二つの publication テストは
認識規則・revision・保持済み Arc の関係を確認する一つの `PoolState` テストにまとめた。

ツリーの worker は本ブランチの `scan_policy.scan()` を残し、policy に UI 側で捕捉した
`page_count_fingerprint: Option<i64>` を加えた。等価比較にも含め、走査後は master の
`collect_folder_listing_metadata_facts` で長さ・ページ数の事実を一緒に収集する。起動 Directory・既定先・
前回の一覧の復元も同じ policy を通す。場所移動・アドレス入力の Worker 経路と、自動 merge 済みの
分類・DFS/兄弟移動・外部変更・物理履歴 preflight は、事実を持つ `ScannedDir` を既存の採用経路へ渡す。
master の同期 loader は catalog を読まないまま維持した。policy の fingerprint 比較・事実収集と、
ページ数の認識設定変更時に同じ起動要求を再走査する回帰テストを追加した。

`async-architecture.md` は本ブランチの二節と master の一括エクスポートの透かしの節を残し、
Susie の記述を統合後の名前へ揃えた。本作業では cargo・rustfmt・製品起動・コミット操作は行っていない。

merge 後 (3d85f11fa) の検証は設計担当がメイン PC で run-heavy 経由で行った。関連の filter
(§1.853/§1.859/§1.861、起動・場所移動・アドレス入力・物理履歴、§1.883 の `page_badge_883` / `page_sort_883` /
ページ数・一覧ソート・`folder_scan`・`duration_762` 等) は 406 件すべて成功した。解消の差分の独立レビュー
(GPT-6.1 Sol / xhigh、実装とは別の会話) は ACCEPT、指摘なし。
実 Shell テスト 2 件は、merge 後・merge 前 (e62b93173、10:37 には成功) のどちらでも 13:30 以降は
`InvokeCommand` が 30 秒で返らず失敗した。merge は shell_paste 系のファイルに触れておらず、merge 前のコミットでも
同じ失敗になるので、merge による退行ではなく開発機側の状態の変化と見ている。クリップボードを開いている窓・所有者は無く、
explorer は応答し、画面のロックも無かった。原因は未特定。
