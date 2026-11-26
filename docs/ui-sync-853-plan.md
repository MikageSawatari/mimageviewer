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
- §1.853 の残り C・D・E (2026-11-26): 「1. 待ちの間のキーを捨てる (推奨)」。お気に入り・アドレス欄 Enter と同じ規則 (§6.1)。
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

#### 実 Shell テストのタイムアウトの原因 (2026-11-25 14:00 頃の観測)

設計担当が、ビルド済みのテスト実行体を直接動かし、待っている間に cdb を非侵襲で attach して観測した
(エクスプローラーへは、スレッドを止めない `-pvr` でスタックとハンドルだけを読んだ)。

- テストの `shell-paste` スレッドが止まっていたのは `InvokeCommand` ではなく、その前の `QueryContextMenu`
  (`invoke_shell_folder_background_verb` → `query_shell_context_menu`)。SHELL32 が項目のアイコンを引く途中、
  thumbcache の中で mutex `...\Explorer\iconcache_idx.db!IconCacheInit` を待っていた。
- この mutex の持ち主は explorer.exe (11-23 起動) のスレッド 0x5b98。このスレッドは別の mutex
  `iconcache_16.db!dfMaintainer` を待っている。その持ち主の explorer のスレッド 0x5cc4 は、event
  `iconcache_idx.db!rwWriterEvent` (アイコンキャッシュの読み書き錠) の解放を待ったまま止まっている。
- つまり、エクスプローラーのアイコンキャッシュの錠が解けない状態にある。アイコンを引くプロセスは、どれもここで待つ。
  10:37 の成功から 13:30 の失敗までの間に、この状態になったと見ている。読み手を握っているプロセスは特定していない。
  クリップボード (所有者・開いている窓とも無し) は関係しない。
- 製品への含意: この状態では、mIV の貼り付けも `QueryContextMenu` で返らない。§1.859 で worker に移したので
  UI は止まらないが、その後の貼り付け・ファイルの切り取り/コピーは「貼り付けが完了するまでお待ちください」で断られ続ける。
  §1.859 より前は、UI スレッドごと止まっていた。OS 側の異常なので、割り切りの方針どおり回避の仕組みは足さない。
- 失敗した実行では、テストの panic の後もプロセスが終わらなかった (Shell の中で止まったスレッドが残るため)。
  直接起動した分は設計担当が止めた。

### 5.11 master 取り込み後の 9 件

Remote 返却の 1 件は非同期完了前の参照だったため、既存の
`finish_startup_open_path_resolve_for_test` で完了後に検査する。RAR ナビゲーション 7 件の共通組み立てと
検索画像の sidecar シナリオは、Specific の一時フォルダで実際の初回フレームを通し、resolver と一覧の
hydration が終わってから detached 操作・検索一覧を組み立てる。本物の Desktop を走査しない。

検索入口の退役漏れは確認されなかった。`open_global_search` は `take_origin_for_search_entry` →
`retire_main_list_requests_for_surface_switch` → `retire_replaced_main_list_requests` →
`cancel_unresolved_open_for_navigation` を既に通る。検索前に resolver が保留されている場合の既定先・明示パスの
両方について、検索バーを開くだけで取消・pending 破棄・遅延結果の送信拒否・main 不変を確かめる状態遷移テストを追加。
今回の検索シナリオは resolver の受付より前に検索一覧を作っていたため、その退役境界では防げなかった。

設計担当から、fmt 後の関連 156 件では検索画像の sidecar シナリオだけが失敗したと報告を受けた。
1 回目は起動の list hydration、設計担当が起動を sidecar 有効化の前へ移した後の 2 回目は親の Checking relay の待ちで
10 秒の期限に達した。前回の「9 件ともテスト側」という記述は、この残件の解消を確認した結果ではなかった。

追補のコード調査: sidecar が有効なら空の実フォルダも `PreparingPageIndex` → `Quiescing` → `Checking` を通る。
無効なら空一覧は即時に表示を再開するため、設定順の変更で起動側の待ちを迂回できることは確認した。
一方、`poll_sidecar_restore` だけのループは Quiescing が待つ別 owner の完了回収を進めない。通常フレームには
spread-shift／bookmark の service、local-adjust／tag／rating 等の完了回収がある。
当該 2 実行で残っていた phase・writer は既存の失敗ログに無く、特定していない。

起動と検索一覧の hydration、親の relay 捕捉待ちを、既存 `ScenarioDriver::root` で実際の logic／UI フレームを回す形へ修正。
sidecar は起動前から有効に戻し、設定順で待ちを迂回しない。親の走査後、最初の sidecar poll より前に relay を取り付ける
順序は保持し、その restore の target context／generation が detached 親フォルダと一致すること、main の検索 origin・
一覧・generation が最後まで保たれることを追加検査する。起動時の restore は検索を開く前に終端を検査する。
再び期限に達した場合に phase／owner／writer の状態を残す診断もテストに加えた。親の relay の 10 秒は延長していない。

製品コード・detached 述語・viewport 経路は変更していない。今回の追補差分のコンパイル・テスト・独立レビューは未実施。
製品側の停止を確認した結果ではなく、修正後の通常フレーム進行での結果は設計担当の検証待ち。
今回も cargo・rustfmt・製品起動・git index 操作・コミットは行っていない。

d0bb9fa16 の検証: 設計担当がメイン PC で run-heavy 経由で流し、関連 156 件 (remote_return / multiwindow_ /
startup_default_ / startup_scan / section1285_ / section1473_history_menu_ / section1853) がすべて成功した。
独立レビュー (GPT-6.1 Sol / xhigh、実装とは別の会話) は ACCEPT、指摘なし。検索を開くと起動の解決待ちが退役する経路は
`take_origin_for_search_entry` (app.rs) → `retire_main_list_requests_for_surface_switch` → `retire_replaced_main_list_requests`
→ `cancel_unresolved_open_for_navigation` (startup_ops.rs) で、追加テストが既定の起動先と明示パスの両方で確かめている。

### 5.12 一覧の Ctrl+V の取りこぼし

観測者はサブ PC 担当の Claude、対象は master 14a448b3a。perf ログでは一覧の Ctrl+V 5 回中 1 回だけ
`shell_paste/submit` が出た。不発時は断りの通知もなく、V の押下と解放の間にフレームが走っていなかった。
成功時は Ctrl 押下 88.057 → submit のフレーム 88.073 → V 解放 88.105、失敗例は Ctrl 押下 231.107 →
次のフレームが V 解放 231.138。Ctrl+C は同じ状況で効いた。

コードの原因: `grid_clipboard_requests` は `GetAsyncKeyState` の Ctrl / V の現在レベルと V の下位 bit を
要求し、描画より先に V が離れると false になる。egui-winit は Ctrl+V を Key event にせず、ファイルだけの
クリップボードでは代わりのイベントも出さない。Ctrl+C の keymap 経路は subclass が配送時に stamp した edge を読む。

変更 (2026-11-25): 固定 Ctrl+V も ROOT の frame edge の `VK_V && ctrl && !repeat` を非消費で照合し、
後述の dispatch claim を通す。KeyAction と keymap.ini は変更しない。既定 chord の定義と `docs/keymap.ini.default` に
Ctrl+Shift+V / Ctrl+Alt+V の割り当ては無かったので、Shift / Alt を問わない旧来の意味を維持した。
旧判定は repeat を明示判定せず OS の transition bit 任せだった。新判定は repeat edge を明示的に除外する
(旧版で repeat が再発火したかの実機観測は無い)。

共通の `keyboard_input_permits` に合わせ、ROOT の配送済み edge は pass 終端の unfocused でも受ける。
TextInput / IME / modal / 別 owner、`grid_item_input_allowed` の block は優先し、固定 Ctrl+V にも
`keymap_owner_blocks_shortcuts` を通す。copy / cut の keymap edge も同じ focus 境界へ揃えた。
送信元の証拠が無い egui Copy / Cut semantic event は従来どおり処理時点の focus を要求する。
copy / cut → ほかのコピー → paste の処理順、1 frame の edge 寿命を維持した。
detached の述語・viewport 経路は変更していない。

test_script の `clipboard_paste` は同一時刻の Ctrl+V down/up を既存 synthetic timeline に積み、製品と同じ
ROOT frame edge の handler を通す形に移行。`SyntheticNavigationKey::V` は test / test-script 限定。
選択済み exact ROOT / backend の fresh validation、target 登録・focus、受付カウンタと待機期限は維持する。
実 OS のキー押下・GetAsyncKeyState consumer ACK・外部キー helper 接続と解放待ちは撤去し、runner の
ClipboardCapture 用 helper 起動も外した。旧 C# helper 本体と単独テストは package に残すが使わない。
NativeTopPanoramaClick のマウス helper と解放の interlock は変更しない。
簡素化: 新しい押下保留状態や再試行を足さず、既存の配送 owner と frame 寿命へ合流した。操作のモーダル化は不要。

回帰テストは `app::main_drop::shell_paste_tests::fixed_ctrl_v_tests` に 8 件追加:

- `shell_paste_ctrl_v_released_before_frame_submits_once_and_preserves_edge`
- `shell_paste_ctrl_v_layout_replay_after_completion_dispatches_once`
- `shell_paste_ctrl_v_keeps_shift_alt_and_routed_ownership_after_focus_loss`
- `shell_paste_ctrl_v_rejects_plain_v_repeat_and_key_up`
- `shell_paste_ctrl_v_text_modal_ime_and_focused_ui_block_routed_edges`
- `shell_paste_ctrl_v_ignores_foreign_edges_and_unfocused_egui_only_input`
- `shell_paste_ctrl_v_synthetic_tap_submits_after_timeline_release`
- `shell_paste_ctrl_v_keeps_copy_cut_precedence_and_semantic_focus_gate`

handler の OS クリップボード照会だけを `handle_clipboard_shortcuts_with` で注入し、Shell は既存 fake worker を使う。
最初のテストは ROOT の Ctrl down / V down / V up / Ctrl up をフレーム前に pending へ積み、処理時の V は解放のまま
`ShellPasteOrigin::CtrlV` の要求を検査する。判定だけを旧コードへ戻すと、実 OS の現在レベルを読んで要求が出ないため
落ちる構造 (この破壊検証は未実行)。synthetic tap のテストも V / Ctrl の現在レベル解放後の要求を検査する。
旧 transition 関数とテストを削除し、Ctrl 必須・release 後の受付・repeat 拒否を上記へ移した。
Copy / Cut semantic event のテストは `clipboard_semantic_requests_keep_copy_cut_separate_from_paste` として維持。
test_script には `clipboard_capture_paste_commands_are_one_released_ctrl_v_tap` を追加した。

独立レビュー (Sol / xhigh、設計担当が依頼) の 1 回目は P2 2 件で REJECT。コードで両方を確認して修正した。
P2-1: 非消費 edge は egui の再 pass でも残り、worker 完了・回収後なら同じ押下を再投入できた。
`key_input::claim_key_down_dispatch` に dispatch owner と既存 `KeyOperationId` の組で実行を claim する入口を追加。
claim は既存の backend frame 境界でだけ clear し、再 pass では保持する。edge は残し、別 handler の照合・消費を変えない。
Ctrl+V 専用の bool・時刻・App の pending field は追加しない。`request_discard` の前に初回処理を完了・poll し、
再 pass では照会も Shell 要求も増えず、次の backend frame の新しい押下は受ける handler 回帰を追加した。
`key_down_dispatch_claim_preserves_edges_and_other_owners` は owner / viewport の分離と edge の保持を検査する。
Ctrl+C / Ctrl+X の `pressed_action` → `pressed_chord` も同じ非消費 query を使い、再 pass の抑止は無かった。
既存の consume 系は frame から除去することで再発火を防ぐ。既存の copy / cut 等への横展開は今回行わない。

P2-2: `require_key_target` の snapshot は現在の前面の target であり、選択 ROOT と一致する保証が無かった。
選択 ROOT の HWND / viewport と `resolve_synthetic_routing_target` を投入前に照合し、不一致なら Down / Up を作らずエラーにする。
Down は診断 timeline の `DownForTarget` で配送先を拘束し、materialize までに前面が変わった場合も queue を取り下げて
`KeyTargetMismatch` にする (別 viewport へ配送せず、遅延再試行もしない)。投入前の拒否テストは
`clipboard_capture_paste_rejects_other_routing_targets_before_queueing`、配送時の拒否テストは
`synthetic_targeted_key_rejects_routing_changes_without_delivery_or_retry`。既存の synthetic tap handler 回帰も
target 指定 Down を通す形へ更新した。設計担当が追加した `ui_fullscreen.rs` の V の match 腕は保持する。

検証 (設計担当が run-heavy 経由で実行、2026-11-25):
- 通常構成 `test(/shell_paste|file_operation_selection_tests|clipboard_semantic|keyboard_input::|key_input::|grid_clipboard|clipboard_shortcut|copy_key_action|cut_files|copy_files|key_down_dispatch_claim|synthetic_targeted_key/)`:
  161 passed。
- `--features test-script` `test(/clipboard_capture|shell_paste|synthetic|test_script::/)`: 415 passed。
- 本体 lib の全件は未実行 (master 取り込み後に統括が流す)。旧判定へ戻して落ちることの破壊検証は未実行。
  ui-smoke スクリプト (`scripts/ui-smoke.ps1` 等) の実走は未実施 (構文解析のみ)。
- 独立レビュー (Sol / xhigh) の 2 回目は ACCEPT、P1〜P3 の指摘なし。
- 実機確認は未実施。

### 5.13 置換の画面を閉じた後の → と、中止の outcome

観測者はサブ PC 担当の Claude。master 38e8e43c5 の build-dev と perf ログによる報告:
一覧の Ctrl+V → Shell の「ファイルの置換またはスキップ」→「スキップ」の後、0.3 秒後の → が
2 回とも効かず、約 2 秒後の → は効いた。`native_menu/verb_invoke` (paste) は 167.573 に戻り、
167.576 の `input/modifier_probe` は `focused:false, permit:false`。
167.612〜168.503 は 890 ms フレームが無く (2 回目も 896 ms)、その間の → の key event は記録されず、
次の → (173.7) は `focused:true` で届いて選択が動いた。
同じ担当から、60 GB の貼り付けを進捗画面の × で中止しても `shell_paste/end` が `outcome:"ok"` だったと報告された。

事実と未特定の理由: source inspection では `src/app/main_drop.rs` が main HWND を service へ渡し、
`src/native_context_menu.rs` の `CreateViewObject(hwnd)` と `CMINVOKECOMMANDINFO.hwnd` の両方に使っている。
通常は Shell の画面を閉じると Windows が owner へ activation を返すはずだが、サブ PC 担当の Claude の
perf ログでは戻り時も `focused:false` で、約 0.9 秒 (2 回とも 890〜897 ms) 戻らなかった。
**理由は未特定、R3-3 は未修正**。候補は (a) Shell の進捗窓 (置換の画面の owner) が残って前面を持つ、
(b) main がまだ disabled、(c) その他。エージェントはメイン PC で画面を出して確認していない。

前面の返却は独立レビュー (別担当、GPT-6.1 Sol / xhigh、利用者からの共有) の REJECT を受けて外した。
P2-1: `GA_ROOTOWNER == main` は操作の由来を識別せず、main を owner にする native 動画の別 thread の
全画面窓 (`src/video/native_window_pump.rs`) やプロパティ等からも前面を奪い得る。
P2-2: `InvokeCommand` の戻りは Shell UI の役目の終了や転送完了の証拠ではなく、理由が未特定の強制返却は根本修正ではない。
P3: 従来のテストは本物の `InvokeCommand` 直前の採取境界を検査していなかった。
設計担当の判断で診断を先行し、サブ PC の再確認で `shell_paste/foreground_after_invoke` を見て
(a)(b)(c) を判別してから構造的な直し方を決める。

試して外した案 (現在の実装には残さない):

| コミット | 当時の判定の表の要約 | 外した理由 |
| --- | --- | --- |
| `0a065beed` | 前面 NULL または同じ process・別 thread なら main へ復帰。それ以外は保持 | 無関係の窓・前面の無い状態まで対象になる |
| `e0036a17a` | main で開始し、同じ process・別 thread・root owner が main の前面だけ復帰 | 所有系列は操作の由来を、呼び出しの戻りは Shell UI 終了を証明しない (P2-1・P2-2) |

現在の変更:

- `src/shell_paste.rs`: `SetForegroundWindow`、設定 API、復帰の判定・enum を削除。
  `ShellPasteCompletion` は呼び出し結果だけを持ち、読み取り専用の `ForegroundApi` で診断する。
- `src/native_context_menu.rs`: 本物の `invoke_canonical_verb` に対の `before_invoke` / `after_invoke` を置く。
  前者は `InvokeCommand` 直前、後者は成功・失敗どちらでも戻った直後で、エラーの整形・計測出力・menu 破棄より前。
  開始時の値は callback の戻り値で後者へ渡す。perf 無効時は前面を問い合わせず、出力もしない。
  bind/query の失敗で未呼び出しなら両 callback と診断 event は無い。
- worker の一行の event は `shell_paste/foreground_after_invoke`。値は `started_from_main`・
  `foreground_class`・`root_owner_class`・`root_owner_is_main`・`foreground_visible`・`main_enabled`・
  `same_thread`・`same_process`・`foreground_is_null`。前面 NULL の thread/process 比較は null。
- `shell_paste/end` の `focus_return` は削除。R3-2 の typed outcome は
  `invoked|failed|start_failed|disconnected` を維持し、選択要求の取り下げ・通知も維持する。
  `invoked` は呼び出し成功だけで、転送の成否・中止を含まない。Shell の paste 動詞からは中止を取得できず、
  `IFileOperation` への置換は行わない。
- ignored の実 Shell テストには前面操作を追加しない。3 入口は共通 service のままで、
  detached の述語・viewport・動画の処理は変更しない。

P3 の seam: 実処理が使う `with_background_menu` で bind/query 後だけ呼び出しへ進む。
`invoke_canonical_verb` の `InvokeCommand` 自体を closure 引数で差し替え、製品と同じ
`CMINVOKECOMMANDINFO` の構築と前後の callback を headless で通す。実 Shell・製品・画面は起動しない。
MenuGuard と Shell interfaces の破棄順序は popup → interfaces を維持する。

追加・置き換えたテスト (コードの記録であり、実行結果ではない):

```text
shell_paste_real_invocation_boundary_orders_observers_on_success_and_failure
shell_paste_real_background_boundary_skips_observers_on_bind_and_query_failure
shell_paste_has_no_foreground_mutation_path
shell_paste_handler_records_read_only_diagnostics_once_separately_from_end
shell_paste_handler_failed_invocation_records_diagnostics_and_failed_outcome
shell_paste_handler_disabled_diagnostics_do_not_query_foreground
shell_paste_outcome_perf_tags_describe_invocation
```

最後の outcome 対応テストは既存のまま維持。復帰の判定表・復帰 handler テストは削除した。
境界テストは成功・失敗の before → invoke → after の一回ずつの順序、bind/query 失敗時の callback 不在、
after が計測出力・menu 破棄より先であることを検査する。handler は 3 入口の診断の一回だけの発行、
値 (前面 NULL・main disabled・main 以外で開始も含む)、別 event と end の分離、outcome を検査する。
読み取り専用 adapter に設定 API はなく、worker・完了 handler に前面変更 API が無いこともソースで検査する。

検証 (設計担当が run-heavy で実行、544cb085e): 関連 lib (`shell_paste|main_drop|file_operation_selection_tests|native_context_menu|post_operation_selection`) 123 passed、
`--features test-script` の `shell_paste|clipboard_capture` 175 passed、実 Shell の ignored 2 件はそれぞれ単独で passed。
旧コードへ戻して落ちる破壊検証、lib 全件、ui-smoke、サブ PC・製品での確認は未実施。

独立レビュー (GPT-6.1 Sol / xhigh、read-only、実装とは別の担当、2026-11-25):
r1 (e0036a17a) REJECT。P2-1 `GA_ROOTOWNER == main` は操作の由来を識別しない (native 動画の全画面窓など main を owner に持つ別 thread の窓から奪い得る)、
P2-2 `InvokeCommand` の戻りは Shell の画面の役目が終わった証拠ではない、P3 採取境界のテストが本物の経路を通らない。
設計担当は前面の返却を外し、診断だけを残した (544cb085e)。r2 (544cb085e) ACCEPT、P1〜P3 なし。
承認の対象は診断と outcome の変更で、**R3-3 は未修正**。サブ PC の再確認で `shell_paste/foreground_after_invoke` を見て理由を判別してから直し方を決める。

**R3-3 の結論 (2026-11-26): 直さずに閉じる**。サブ PC の 3 回目 (v580c2 の D、サブ PC のエージェント) の記録で、閉じた直後の前面は Shell の進行状況の窓 (OperationStatusWindow) で、約 0.9 秒後に mIV へ戻ると → が効いた (候補 (a))。Explorer で同じ操作をした比べは、サブ PC では画面操作のツールが Explorer へキーを送れず未確認 (v580d の D)。利用者が自分の PC の Explorer で試した結果 (利用者の言葉、2026-11-26): 「スキップを押したあと、そのダイアログがまだしばらく残っており、下のウィンドウに戻るまでに1秒くらいはありました。その間の右キーは無視されますが、ダイアログが閉じてフォーカスが戻ればききました。」Explorer と同じ動きなので、mIV 側で前面を戻す仕組みは足さない (OS と挙動を一致させる方針)。


---

## 6. §1.853 の残り C・D・E (v5.9.0 予定、ブランチ `ui-scan-853b`)

状態: 設計 r7 確定 (2026-11-26、設計レビュー r7 で ACCEPT。r1〜r6 の指摘と、全画面の経路の利用者決定 (§6.1) を反映。§6.9〜§6.16)。担当: 設計・検収 = ClaudeCode Opus 5.5 (uiscan-853b)、実装 = Codex GPT-6.1 Sol / high、
独立レビュー = GPT-6.1 Sol / xhigh。作業ツリー `G:\home\mimageviewer-uiscan853b`。コードの記述は master `451ad09db` の読み取り。
本担当は実行時の挙動を観測していない。時間の数字はサブ PC 担当の perf ログ (§1.853、3004 件のネットワークフォルダで約 7.6 秒)。

まれな失敗 (走査の I/O エラー・worker の異常終了) は既定の割り切り (ログ + 通知)。再試行・回復の仕組みは足さない。

### 6.1 利用者の決定 (2026-11-26、統括経由)

設計担当が統括へ出した質問 (共有メモ `uiscan-853b-typeahead-question.md`) に、利用者は
「1. 待ちの間のキーを捨てる (推奨)」を選んだ (統括の伝達、AskUserQuestion、2026-11-26)。質問での説明は
「お気に入り・アドレス欄の Enter と同じ規則で、読み込み中に押したキーは押し直しが要る。速いローカルのフォルダでは
待ちがごく短く、影響は主に遅い共有フォルダや件数の多いフォルダ」。

したがって C・D・E の走査待ちは、§1.805 の「場所の移動待ち」(`GridItemInputBlock::DirectNavigationScan`) と同じ扱いにする。
走査の間は今の一覧を表示したまま一覧の項目入力 (キー・ポインタ・タッチ・リング・ゲームパッド・メニュー) を止め、
待ちの間に押したキーは新しい一覧へ渡さない。移動の入力 (ツリー・お気に入り・アドレス欄・戻る/進む等) は後勝ち。
走査が 500 ms を超えたら「移動先を読み込み中…」を出す。どれも既存の仕組みで、新しい規則は作らない。
2026-11-18 (§1.805 §3.2) に範囲から外した理由 (Enter 直後の先行入力) は、この決定で解消した。

2 つ目の決定 (2026-11-26、統括経由): 設計担当が「全画面から戻る経路と、読込の後に続きを運ぶ作りが要る経路は別項目」と
提案したのに対し、利用者は「全画面の経路も今回入れる」を選んだ (統括の伝達、AskUserQuestion。v5.9.0 が遅れ得ることを
示したうえでの選択)。同じ仕組みに乗る経路は一緒に、乗らないものは理由を添えて別項目にする。全画面の待ち方は
「設計の簡素化」を先に検討し、今の使い方が変わる点を統括へ返す (§6.12、共有メモ `uiscan-853b-fullscreen-close-proposal.md`)。

3 つ目の決定 (2026-11-26、統括経由): 全画面から戻る経路の待ち方は「Ctrl+↑↓ と同じ扱い (推奨)」(統括の伝達、AskUserQuestion)。
読み終わるまで全画面を出したままにし、読み込み中の表示は出さない。待ちの間のページ送り・ジャンプは受け付けない。拡大・回転・★は
今のページに効く。もう一度「閉じる」を押すと親の一覧を読み直す。
経緯: 設計担当は最初に「全部止めるモーダル + 500 ms 後に読み込み中の表示」を返し、利用者は一度それを選んだ。その後の設計レビュー r4 で、
全部止めるには専用の全画面 viewport・動画の HUD・focus guard・ゲームパッドの配り先 (別窓 F12 の操作まで巻き込む) に新しい止めが
要ると分かり、設計担当が「Ctrl+↑↓ と同じ扱い」へ改めた案を返した。統括が両案を比べて示し直し、利用者は改めた案を選んだ
(前の回答はこの回答で置き換え)。
お気に入り検索・タグ表示の BS と Ctrl+↑↓、スマートフォルダ・サブフォルダ展開を抜ける操作は同じ規則で一緒に入れ、★固定の移動は別項目
(番号は統括が master で取る)。

### 6.2 対象と対象外 (コードの棚卸し、master `451ad09db`)

同期走査点は 1 つ: `load_folder_with_scan_claimed_with_search_origin` (`src/app.rs` 32692 付近) の
`pre_scan = None` → `scan_directory_with_settings`。そこへ届く入口は 2 つ
(`load_folder_or_convert_archive_with_auto_fullscreen_classified_with_navigation_at_stage` 31658 付近と
`load_folder_with_scan_owned_with_search_origin` 31969 付近)。§1.805 の worker 分岐
(`direct_navigation_uses_worker_scan`) は前者の入口にだけあり、owner が `Navigation`・navigation あり・
印 `DirectoryScanRoute::Worker` のときだけ効く。

main の context で同期走査に届く入口 (今回の対象):

| 区分 | 入口 (生成点) | owner / intent / 全画面予約 | 読込後に使う状態 |
| --- | --- | --- | --- |
| C1 | 一覧のフォルダを開く: Enter (`accept_grid_container_open` → `grid_physical_navigation`、`collection_grid.rs` 1515・1603)、ダブルクリック (`ui_main.rs` 19510)、ゲームパッド A (`gamepad_input.rs` 10100) → post-render の `Direct` | Navigation / `container_open(auto)` / **`pending_auto_fs_open` を先に立てる** (`collection_grid.rs` 1546、`gamepad_input.rs` 10095) | **読込の前に閲覧履歴の帰路 (`note_reading_history_open`) と★フィルタの一時解除 (`maybe_suppress_rating_filter_for_opened_container`) を変える** (`collection_grid.rs` 1544、`gamepad_input.rs` 10094)。§6.3 の 8 で採用の時点へ移す |
| C2 | 親へ: BS・⬆・背景・ゲームパッド B (`physical_grid_parent_action`、`grid_parent.rs` 596) → `Direct(parent, ExplicitList)`。サブフォルダ展開の戻り・スマートフォルダの物理の親 (`subfolder_expansion.rs` 1998、`smart_folder.rs` 8843) | Navigation / ExplicitList | `select_after_load` (子の名前) を先に置く |
| C3 | コレクションの物理元: Enter・BS (`collection_grid.rs` 1592・1367) → `CollectionSource` | **CollectionGridPhysical** / 各 intent | Enter は C1 と同じ先行変更 (同じ `accept_grid_container_open`) |
| C4 | フォルダを開くダイアログ (`open_folder_nav` → `dispatch_favorite_navigation` → `Direct`、印なし) | Navigation / ExplicitList | なし |
| D1 | 本棚のルート `open_books_root` (`app.rs` 56021)、本の管理の「開く」(`location_jump.rs` 30) → `load_folder` | Navigation / ExplicitList | なし |
| D2 | お気に入り検索の BS・Ctrl+↑↓ (`favsearch_back` 35896、`favsearch_navigate_sibling` 35874)、タグ表示の BS (`tag_view_back` 35466) | Navigation / ExplicitList・`container_open(false)` | **読込の前に検索の積み上げを変える**。§6.3 の 9 で install の確定へ移す |
| D3 | スマートフォルダの取消・定義削除 (`smart_folder.rs` 8299・8373)、サブフォルダ展開の切替・終了 (`subfolder_expansion.rs` 1801・1986) | Navigation / ExplicitList | **読込の前に合成一覧の表示の状態を消す**。§6.3 の 10 で install の既存の後始末へ任せる |
| C5 | 全画面を閉じる即時経路の `Direct` / `CollectionSource` (`app.rs` 25208・25241): 「ページを直接開く」で閉じて親へ戻る (`close_return.rs` 407)・ブックマーク・コレクションの物理元の閉じ戻り・動画の背景での移動 (`update_phases.rs` 31〜46)・Everything の物理の親 | Navigation または CollectionGridPhysical | **全画面を閉じる処理が読込の install の中にある** (ページ一覧を見せないため)。§6.12 |
| D4 | 「ZIP を作成」完了時の親の読み直し (`ui_dialogs/archive_convert.rs` 1185) | Navigation / PreservePresentation | 出力名の `select_after_load` を先に置く |
| D5 | 読み直しの fallback (`reload_current_physical_list_with_intent`、`app.rs` 29068): main の実フォルダは既に staged の preflight (29049) で非同期。**採用済みの表示が実フォルダ**で `current_folder_last_mtime` が無いとき (メタデータの取得失敗、スマートフォルダが mtime を消した後 `smart_folder.rs` 8657) だけ同期。ZIP/PDF は正常でも mtime を持たない (`app.rs` 40715) が、走査点の前の早期 return で UI 走査は無い | Navigation・CollectionGridPhysical・RatingPhysical / RefreshList | **読込の直後に `address`・`archive_source_override` を書き換える** (29077〜29084) |
| E | 「元の場所へ」の ArchiveContainer (`app.rs` 61972〜62012): main で `classified_kind` が無く、`.zip`/`.pdf` 風の名前の実フォルダが今の一覧に Folder として出ているときだけ | Navigation / ExplicitList | なし |

対象外 (理由):

| 入口 | 理由 |
| --- | --- |
| トレイへ閉じる (`close_main_still_fullscreen_for_tray`、`startup_list_restore.rs` 429〜475) | 窓を隠す操作で、待ちの見え方が無い。隠れている間の poll と、再表示で一瞬全画面が見える組み合わせを作らないため、理由付きの例外 `TrayClose` で同期のままにする (止まるのは隠れた窓で、利用者が待つ前面の操作ではない) |
| ★固定の移動 `snapshot_load_and_open` (`snapshot_ops.rs` 2135)・`snapshot_navigate_grid_inner` (2870) | 読込の直後に `self.items` から対象の葉を探して全画面を開き、スライドショーを再開する (2136〜2195)。`snapshot_internal_nav` の guard と `items_generation` の照合 (2675) も同期を前提にしている。続きを型で運ぶ設計 (既存の `RequiredFullscreenTarget` が候補) が要る |
| 再生試験 `poll_play_test` (`app.rs` 34375) | 診断用の CLI。非同期にすると採用までの毎フレームに読込を出し直す |
| test-script の ReleaseChecks の「開く」(`release_checks.rs` 212) | 試験の自動操作。結果の bool を同期で返す契約 |
| main 以外の context (detached・投影中の別 context) の全入口 (表の C・D の各入口と、§1.805 の印付き入口の detached 側、`restore_view_return_origin` 35338、起動 resolver の detached lease の bookmark `startup_ops.rs` 2124・2275・2592) | detached viewer はリワーク中で凍結 (CLAUDE.md)。今回の分岐は main の述語だけで判定し、main 以外は従来どおり同期のまま |
| 既に `pre_scan` を持つ・ファイル/書庫だけの入口 (外部再走査、DFS、分類 worker、ツリー、JumpToPhysicalFolder、view-order refresh、起動 resolver の main、ZIP/PDF/変換書庫) | UI での走査が無い |

★固定の移動は、全画面のまま次のフォルダを読み、読み終わったら対象の画像で全画面を開き直してスライドショーを再開する
続きが要る。似た既存の `RequiredFullscreenTarget` (類似画像の移動、`snapshot_ops.rs` 2226〜2237・2466〜2513) は、
★固定を解除する (`dismiss_snapshot_without_restore`、2364)・スライドショーを再開しない (2396・2407)・対象が無いときの
代わり (最初の画像) を持たない (`DeferredFsTarget::Required`)・フォルダ項目を扱わない、の 4 点で違い、今回の仕組みに
乗らない。**別項目の候補として統括へ報告する** (番号は統括が master で取る)。C5・D2・D3 は、install の中の既存の確定点に
乗るので今回入れる (§6.3 の 9・10、§6.12)。

### 6.3 設計: 同期走査点で「同じ呼び出しを走査の後へ遅らせる」

§1.805 は生成点に印 (`DirectoryScanRoute::Worker`) を付けて入口ごとに worker へ回した。今回は対象が十数の生成点と
2 つの入口にまたがり、印で選ぶと付け忘れた入口が同期のまま残る。**判定を同期走査点そのものに置き、既定を反転する**
(CLAUDE.md「設計の簡素化」: 1 つの所有者・1 つの確定の時点)。

1. **判定の位置**: `load_folder_with_scan_claimed_with_search_origin` の `pre_scan = None` で
   `scan_directory_with_settings` を呼ぶ直前 (受付・拒否・claim・staged/PDF/ZIP の早期 return・test-script の数え上げを
   全部通った後)。次をすべて満たすときは走査せず、worker に走査を出して `FolderOpenOutcome::Classifying` を返す:
   - main の context (`main_folder_history_available()` かつ `!navigation_scope.is_detached_physical()`)
   - navigation がある (main ではこの関数の冒頭で `capture_physical_open_navigation` が捕捉する。main で None に
     なる経路が実在するなら、実装担当は設計担当へ返す。黙って同期にしない)
   - 呼び出しの走査経路が例外 (下の 4) でない
   - 権限が `VisibleInstallAuthority::Ordinary` (pre_scan = None で届く 2 入口はどちらも Ordinary。それ以外が来たら
     設計担当へ返す)
2. **預けるもの (`DirectNavigationContinuation`)**: 同期走査の後に `load_folder_with_scan_claimed_with_search_origin` が
   使う引数を、`pre_scan` 以外全部そのまま持つ: `path`・`owner`・`grid_effects`・`classified_kind`・`restore_intent`・
   `navigation` (test-script の `folder_load_recorded` を立てた後)・`stage`・`search_origin`・`tag_admission`。
   加えて**全画面の予約**: `pending_auto_fs_open` を `std::mem::take` で受け取り、要求が持つ。保留中に App の flag に
   残さないので、退役・取消・走査失敗で予約が次の無関係な読込へ漏れない (§1.805 §7 の `GridFolderCandidate` の既存の漏れと
   同じ形をここでは作らない)。`select_after_load` は navigation の `selection_after_load` に捕捉済みであることを
   実装担当がコードで確かめ、採用時に App の値を読まない (§5.4 の追補で起動の既定先に同じ問題があった)。
   `FolderOpenScanPurpose::DirectNavigation` をこの continuation を持つ形 (`DirectNavigation(Box<DirectNavigationContinuation>)`)
   に変える。
3. **完了**: 既存の pane pending の poll → 通常 nav 裁定 (`post_render_main_navigation`、より優先の nav が同じフレームに
   あれば負ける) → `main_list_navigation_is_current` の照合 (規則 3) はそのまま。owner が `CollectionGridPhysical` なら
   `collection_grid_physical_load_owner_is_current` も照合し、古ければ捨てる (`GridFolderCandidate` と同じ)。
   r2 (設計レビュー r1 P2-4): 今の §1.805 の完了は `open_direct_navigation_target_at_stage` へ再入して claim を通り直すので、
   claim 冒頭の `document_open_modal_admission_blocked()` (`app.rs` 32054。兄弟 context の PDF パスワード等も含む) が
   完了でも効いている。遅らせた呼び出しは claim を通らないので、**採用の直前に同じ述語を評価し、塞がっていれば今の再入と
   同じ結果 (要求を捨てる。通知も今と同じ) にする**。claim のその他の照合 (rating・Quick Folder・コレクションの owner の
   現在性) は、遅らせた呼び出しの中の `open_request_owner_is_current` が同じ判定をしているかを実装担当が確かめ、
   抜ける照合があれば同じく採用の直前に置く。claim の取消の副作用 (変換中・起動 resolver・ブックマークの退役) は
   受付で済んでいるので繰り返さない。
   r2 (設計レビュー r1 P2-5): **走査の設定と Susie の世代の照合**を §3.2 と共有する。worker は走査に使った
   `FolderScanPolicy` (Susie の対応拡張子の世代を含む、走査の前後で読む) を結果に付け、採用の直前に今の設定の policy と
   比べる。違えば結果を捨て、同じ pending を取り出さずに今の policy で走査だけをやり直す (§3.2 の「走査だけの続き」と同じ:
   continuation・cancel token・開始時刻はそのまま、受付は繰り返さない)。設定の変更・「プラグインを再読み込み」は通常の
   操作なので、割り切りではなく構造で扱う。§1.805 の入口 (お気に入り等) も同じ完了を通るので一緒に直る。
   採用では、予約を `pending_auto_fs_open` に戻した直後に、**預けた引数と `pre_scan = Some(scan)` で同じ
   `load_folder_with_scan_claimed_with_search_origin` を呼ぶ**。受付・claim はやり直さない (済んでいる。§1.805 の
   「Admission is complete」と同じ)。走査の失敗は今の同期の失敗と同じ処理 (ログ・予約の破棄・`cancel_folder_pane_open`・
   `release_fs_nav_lock`・「フォルダを読み取れませんでした」) にし、UI で走査し直さない。
4. **例外 (同期のまま)**: 走査経路の型を「既定 = main は worker」と「理由付きの UI 走査」の 2 値にする
   (例: `DirectoryScanRoute::{MainWorker, UiThread(UiThreadScanReason)}`、理由は `TrayClose`・`ConvertedCacheReload`
   (届く場合だけ)・`SnapshotLeafOpen`・`PlayTest`・`ReleaseCheck`)。今の
   `DirectoryScanRoute::UiThread` の 12 か所と `load_folder` 系の wrapper は既定になり、§6.2 の対象外の入口だけが理由を渡す。
   理由は「この入口はまだ続きを運べない」の印で、別項目で続きを運べるようにしたら外す。理由は 2 つの入口から同期走査点まで引数で運ぶ (App の flag で運ばない)。
   main 以外の context は経路の型にかかわらず従来どおり同期 (1 の述語で決まる)。
5. **§1.805 の分岐を 1 つにまとめる**: `direct_navigation_uses_worker_scan` の早い分岐と `DirectoryScanRoute::Worker` の
   印は、1 の判定に吸収して撤去する (同じ意味を 2 か所に書かない)。お気に入り・アドレス欄 Enter・最近開いたフォルダの
   完了も 3 の「預けた同じ呼び出し」になる。§1.805 の分岐が今やっている受付時の後始末
   (`retire_replaced_main_list_requests_with_startup_intent(Some(Pane), …)`、DFS の `folder_nav_pending` の取消、
   `clear_pending_folder_nav_steps`、全画面の DFS holdover の `Superseded` 終了) は、1 の判定で worker を出す直前に同じ順で
   行う。`AddressBarNav::WorkerFavorite`・`AddressEnterPhysical`・`RecentFolder` の値は、走査経路以外の意味
   (位置・履歴の扱い) を持つなら残し、走査経路の印としての役目だけを外す。§1.805 の既存テストは期待値を変えずに通ること
   が、このまとめの受け入れ条件。
6. **D5 (読み直しの fallback) は staged の preflight に寄せる**: r3 (設計レビュー r2 P2-2): 対象は**採用済みの表示が
   実フォルダ** (ZIP/PDF でも変換キャッシュでもない。`archive_source_override` が無い) で `current_folder_last_mtime` が
   無い main の読み直しだけ。その場合だけ、既存の `start_physical_history_transition_classified_at_stage` (mtime がある
   場合と同じ経路) へ渡す。ZIP/PDF・変換キャッシュの読み直しは既存の経路のまま変えない (キャッシュのパスを staged へ
   渡すと論理 source と表紙 pin のキーが変わる、`app.rs` 39079・41001)。変換キャッシュの読み直しが走査点に届くなら
   (実フォルダのキャッシュ)、理由付きの例外 `ConvertedCacheReload` で同期のままにする。読込の直後に
   `address`・`archive_source_override` を書き換える同期前提 (29077〜29084) は、main ではこの経路を通らなくなる。
   staged の経路が mtime 無しを扱えない理由がコードにあれば、実装担当は設計担当へ返す (同期のまま残さない)。
   main 以外は従来どおり。
7. **呼び出しの外で状態を変える入口の点検**: 実装担当は、既定が worker になる入口 (§6.2 の対象の表) のすべてについて、
   呼び出しの前に App の状態 (表示中の合成一覧の owner・検索の積み上げ・アドレス・選択のヒント・全画面) を変えていないか、
   後に読込の結果を前提にした処理をしていないかを確かめる。D2・D3・C1 と同じ形 (呼び出しの外の状態の変更) のものが
   見つかったら、install の確定点へ移せるかを書いて設計担当へ返す (黙って worker にしない)。

8. **C1・C3 のフォルダを開く操作の先行変更を、ZIP/PDF と同じ採用時の確定へ移す** (r3、設計レビュー r2 P2-1):
   `accept_grid_container_open` (と同じ処理を持つゲームパッドの入口 `gamepad_input.rs` 10094) は、フォルダのとき読込の前に
   閲覧履歴の帰路と★フィルタの一時解除を変えている。待ちが拒否される後続で取り消されると、旧一覧に一時解除が残り、
   閲覧履歴からの帰路が消える。ZIP/PDF は既にこれらを `GridVirtualOpenEffects` に捕捉し、採用 (install) の中の
   `commit_grid_virtual_open_effects` (`app.rs` 37646、呼び出しは 32783) で確定している。**フォルダも同じ型の効果を捕捉して
   nav の値 (`Direct` / `CollectionSource` の型付きの持ち物) に入れ、読込の `grid_effects` として渡す**。先行の
   `note_reading_history_open`・`maybe_suppress_rating_filter_for_opened_container` の呼び出しはフォルダでは外す。
   r4 (設計レビュー r3 P2-1): フォルダでは `suppress_facet_filter = false` にする (true だと loader が `book_prefix = Some("")`
   にして通常フォルダの履歴を `FacetScope::Book` で作り、子フォルダを包含しなくなる。`app.rs` 32757・25656、
   `facet_navigation.rs` 106)。facet の絞り込みの扱いは今のフォルダと同じ (ZIP/PDF だけが true)。
   r4 (設計レビュー r3 P2-2): ★フィルタの一時解除は、**受付の時点で今の idx 版の判定 (`get_rating(idx)`、session の書き込みと
   表示中の cache を優先) で決めた結果**を効果に入れる (例: `rating_suppression: Option<(anchor, saved)>`、App を変えずに計算)。
   採用時の確定は、その決定を `rating_filter_suppressed_at` が空のときにだけそのまま入れ、通知もそこで出す。DB の値で
   判定し直さない。ZIP/PDF の既存の path 版の扱いは変えない (同じ型に 2 つの決め方を混ぜないよう、フォルダの決定は
   別の field にする)。
   全画面の予約も `effects.auto_fullscreen` で運び、読込の中で prepare (予約を消費する `app.rs` 33066) の直前に
   `pending_auto_fs_open` へ立てる。フォルダの入口で App の flag を先に立てるのはやめる (2 の「予約を要求が持つ」と同じ
   所有者にまとめる。2 の `take` は、効果を持たない入口が flag を立てていた場合の受け皿として残すか、flag を立てる入口が
   他に無いことを確かめて外すかを実装担当が判断し、記録する)。同期のまま残る経路 (main 以外・例外) でも、効果は同じ
   install の中で確定するので挙動は同じ。`capture_main_folder_list_order` は ZIP/PDF でも先行で行っている既存の扱いなので
   変えない。

9. **D2 (検索の BS・Ctrl+↑↓) の積み上げを install の確定へ移す** (r4): install の中の
   `commit_staged_search_navigation_effects` (`app.rs` 39335〜39370、呼び出しは 32792) は既に、お気に入り検索なら
   `nav_stack.push(path)` + アドレス更新、タグ表示なら `TagViewNavAdmission` (`Descend` / `ListOrderRow` で置き換え) で
   積み上げを確定している。**戻る・兄弟への置き換えもこの確定に型で渡す**: お気に入り検索の bool を型
   (例: `FavsearchNavAdmission::{Descend, Back, Sibling}`) に、タグ表示に `TagViewNavAdmission::Back` を足し、
   `favsearch_back`・`favsearch_navigate_sibling`・`tag_view_back` は読込の前に積み上げを変えない (選択のヒントと
   `cancel_pending_folder_nav` は今のまま先に行う)。`tag_view_back` の Ignored/Refused の巻き戻し (35472〜35478) のうち、
   積み上げとアドレスの巻き戻しは先に変えないので不要になる。r5 (設計レビュー r4 P3-5): **選択のヒントの巻き戻しは残す**
   (`select_after_load` は先に置くので、navigation が捕捉する前の即時の拒否 (modal、`app.rs` 31189) で次の読込へ漏れない
   ようにする)。お気に入り検索の BS も同じく、即時の拒否ではヒントを戻す。積み上げが空になって検索結果へ戻る枝 (`execute_favsearch` / `execute_tag_view`) は
   読込ではないので今のまま。`tag_view_back` の行き先が ZIP/PDF/変換書庫のとき (staged・分類の経路) も同じ型付きの戻りを
   確定時に適用する。運べない経路があれば設計担当へ返す。今の `favsearch_back` は pop の後の読込で同じ path をもう一度
   push している可能性がある (棚卸しの所見、未確認)。実装担当はテストで確かめ、型にした後の積み上げが今の利用者に
   見える結果 (戻り先・アドレス・次の BS の行き先) と同じであることを確かめる。今と違う結果になるなら返す。
10. **D3 (合成一覧を抜ける操作) は、読込の前の後始末をやめて install の既存の後始末に任せる** (r4): install の
   `install_loading_items_inner` は、別の一覧を採用するとき既にサブフォルダ展開 (`app.rs` 44891〜44907) とスマートフォルダ
   (44908〜44920、`preserve_smart_folder_session` の条件 44845〜44858 を除く) の状態を片付ける。`restore_cancelled_smart_folder_prior`・
   `forget_smart_folder_definition`・サブフォルダ展開の切替/終了は、機能自体の取消・定義の無効化 (pending の取消・
   `invalidate_smart_folder_definition`・削除済みパスの掃除) だけを先に行い、表示の状態の後始末
   (`clear_smart_folder_view_state`・`clear_subfolder_expansion_view_state`) は外す。実装担当は、install の後始末が今の
   先行の後始末と同じ field を片付けること、これらの帰り先で `preserve_smart_folder_session` が真にならないことを確かめる。
   待ちが取り消されたときは、合成一覧とその状態がそろったまま残る (行だけ残って状態が消える、を作らない)。定義を削除した
   スマートフォルダが待ちの取消で表示に残った場合、その一覧の操作が無効化済みの定義を既存の扱い (無効化の経路) で扱うことを
   確かめる。
   r5 (設計レビュー r4 P2-1): 状態を残したまま普通の読込を出すと、帰り先がスマートフォルダの entry やその配下のとき
   スマート専用の分岐 (`app.rs` 31932、entry 自身も対象 `top_level_grid_view.rs` 129) へ入り直し、抜けられない。**抜ける操作は
   既存の型付きの「一時表示から出る」navigation (`MainHistoryOperation::Direct(DirectNavigationPurpose::ExitTemporaryView {..})`)
   として出す**。この navigation はスマートの分岐を通らず (`app.rs` 31444・31930 の条件)、採用時に
   `dismiss_transparent_view_for_admitted_jump` (26285〜26310、呼び出しは 26199) がスマート・検索・タグの状態を片付ける。
   サブフォルダ展開を抜ける操作も同じ型にする。実装担当は、同じ子への帰還でも `preserve_smart_folder_session` が働かないこと、
   取消のときに表示中の session が残ること (採用で初めて終わること) を確かめる。

### 6.4 待ちの間の入力・表示

- 入力: 走査中は 3 の pending (`FolderOpenScanPurpose::DirectNavigation`) があるので、既存の
  `direct_navigation_scan_pending()` → `GridItemInputBlock::DirectNavigationScan` がそのまま効く (§1.805 §4.4 の規則 1〜3、
  Esc・スタック切替キーの扱いも同じ)。新しい停止理由・述語は作らない。
- 表示: 走査中は今の一覧のまま。500 ms を超えたら既存の「移動先を読み込み中…」(`DIRECT_NAVIGATION_WAIT_GRACE`)。
- 全画面から戻る経路 (C5) の待ちは §6.12。

### 6.5 計測

- worker の既存 `nav/folder_open_scan` (`ms`・`purpose`・`folders`・`media`・`failed`・`cancelled`) に、continuation から
  `owner` (`navigation|collection|rating`)・`intent` (restore_intent の種類)・`auto_fs` (予約の有無)・
  `rescan` (設定の不一致で走査し直した回数) を足す。生成点の名前は同期走査点では分からないので記録しない。
- 採用時の `nav/lf_scan` は `pre_scanned=true`。`scripts/analyze_perf.py` の nav の内訳に新しい extras を足す。
- UI スレッドの同期走査の cfg(test) 計数 (§1.805 で足したもの) を使い、対象の入口で 0 回であることをテストで確かめる。

### 6.6 不変条件とテスト

テストは既定と `--features test-script` の両方で流す (v5.8.0 の §1.853 では test-script 側の数え上げの期待値の誤りが
全件まで見つからなかった)。

1. §6.2 の対象の各入口 (C1〜C5・D1〜D5・E) で、dispatch したフレームと採用のフレームの UI 同期走査が 0 回。worker 完了 +
   poll で、今の同期経路と同じ一覧・選択 (子の名前・出力名)・履歴・アドレス・カーソルが採用される。
2. 全画面の予約: 画像フォルダを Enter で開く (自動全画面が有効) と、採用後に全画面が開く。待ちの間に別の移動で退役・
   走査失敗・`Disconnected`・コレクションの owner の失効・bundle の破棄のどれでも、予約と選択のヒントは次の無関係な読込に
   残らない。
3. 入力: 待ちの間は一覧の項目入力が止まり (Enter・↓・ダブルクリック・ゲームパッド)、待ちの間に押したキーは採用後の一覧に
   届かない。ツリー・お気に入り・アドレス欄・戻るは後勝ちで、遅れて完了した旧要求は採用されない。500 ms 未満では
   待機表示を出さない。
4. modal: main の走査待ちの間に兄弟 context が document modal (PDF パスワード等) に入ったら、完了は今の §1.805 の再入と
   同じ結果になる (採用しない)。
5. 走査の設定 (r3、設計レビュー r2 P3): 2 つに分けて確かめる。(a) 設定の適用が今の一覧の更新を始める変更
   (種類の表示の変更 `app.rs` 29384・29464 など) では、更新の開始 (`app.rs` 62310) が待ちの要求を退役させ、旧要求は
   復活しない (§1.805 の規則 2 のまま)。(b) 更新を始めない変更 (隠しファイル・書庫/EPUB の扱い・「プラグインを再読み込み」
   のうち、実装担当が (a) に当たらないと確かめたもの) では、要求が生きたまま policy が不一致になるので、古い結果を採らず
   同じ要求のまま今の設定で走査し直して採用する (受付・test-script の要求数・履歴は 1 回分)。お気に入りの入口でも同じ。
6. D5: mtime の無い main の実フォルダの読み直しが staged の経路で採用される。ZIP/PDF・変換キャッシュの読み直しは
   経路が変わらない (論理 source・アドレス・`archive_source_override`・表紙 pin のキーが従来どおり)。
6a. C1・C3: ★付きのフォルダ・閲覧履歴の一覧のフォルダを開いて待つ間に、拒否される後続 (表示対象外の ZIP をアドレス欄で
   指定する等) で取り消すと、★フィルタの一時解除も閲覧履歴の帰路も変わっていない。採用されたときは今と同じに確定する。
   ゲームパッド A でも同じ。★の判定は受付時の値 (session の書き込みを含む) で、待ちの間に兄弟 context が★を変えても
   採用時に判定し直さない。facet が有効な親 → 子フォルダ → 孫フォルダ → 戻る、の経路と復元が今と同じ。
6b. D2: お気に入り検索の BS・Ctrl+↑↓、タグ表示の BS が、今と同じ戻り先・アドレス・次の BS の行き先になる。待ちの間に
   拒否される後続で取り消すと、積み上げもアドレスも変わっていない。タグ表示の行き先が ZIP のときも同じ。
6c. D3: スマートフォルダの取消・定義削除、サブフォルダ展開の切替/終了が今と同じ一覧と状態になる。待ちが取り消されると、
   合成一覧と状態がそろって残る。
6d. C5 は §6.12 のテスト。
7. E: `.zip` という名前の実フォルダへの「元の場所へ」が worker で走査される。
8. 例外: トレイへ閉じる・★固定の移動・再生試験・ReleaseChecks は同期のまま (計数 1 回)。main 以外の context の各入口も同期のまま。
9. 兄弟 context・lifecycle: detached を投影している間は main の待ちで入力を止めない (§1.805 と同じ述語)。sidecar 復元中の
   完了は保留 (既存)。Quick Folder の A→B→A の往復、待ちの間の DFS・saved-group は §1.805 の規則のまま。
10. test-script の要求数: 受付時の 1 回だけ (採用の再入で数え直さない)。取消された受理済み要求も 1 回。
11. §1.805 の既存テストのうち、**意図して期待値を変えるもの**は次の 2 種だけ。実装担当は該当するテストを名前で列挙し、
    変える理由をテストのコメントと記録に書く: (a) 全画面の予約を要求が持つようになったこと (例:
    `direct_navigation_worker_admission_does_not_take_global_fullscreen_reservation`、`direct_navigation_worker_scan.rs` 302)、
    (b) 印の無い入口が同期で採用される前提 (例: `direct_navigation_worker_preserves_unmarked_presentation_intents`、同 1251。
    一覧・履歴・intent の期待値は保つ)。それ以外の §1.805 のテストは期待値を変えずに通ること。
12. 既存テストのうち、対象の入口の直後に同期採用を前提にしていたもの (§1.805 §3.2 の 39 件: history_parent_1473・
    ratingnav649・same_position_double_click・section1328 等) は、worker 完了 + `post_render_main_navigation` を回す helper を
    通す形に直す。**一覧・選択・履歴の期待値は変えない**。変えないと通らないものは、実装担当が理由を添えて設計担当へ返す。

### 6.7 文書の更新 (実装時)

- [ui-responsiveness.md](ui-responsiveness.md): 冒頭の §1.853 の節に C・D・E を追記。対象外 (C5・D2・D3・★固定の移動・診断・
  main 以外) を明記。
- [async-architecture.md](async-architecture.md): 物理フォルダの事前走査の行に、同期走査点の既定の反転と continuation。
- [direct-navigation-worker-scan-plan.md](direct-navigation-worker-scan-plan.md) §7: 一覧側の後続候補を本書 §6 で実施したと記録。
- [detached-rework-plan.md](detached-rework-plan.md) §11: 判定は main の述語だけで、main 以外の context の経路は変えないこと。
- htdocs のマニュアルに「読み込み中のキー」に触れた記述があれば合わせる (無ければ変えない)。
- [video-architecture.md](video-architecture.md): native の対象を変える入口を共通の受付へまとめたこと (§6.12 の 2)。
- [display-pipeline.md](display-pipeline.md): 全画面から戻る待ちの `CloseReturn` sequence (holdover の保持と終わり方)。
- next-release-backlog の状態行は統括が更新する (本担当は変えない)。

### 6.8 状態の組み合わせを減らす検討

- 採用: 判定を同期走査点 1 か所に置き、入口ごとの印をやめる。付け忘れの入口が残らず、§1.805 の分岐も吸収して
  完了の経路が 1 つになる。
- 採用: 待ちの間の入力は既存の「場所の移動待ち」(停止理由 1 つ) に乗せる。新しい停止理由・入力の保留は作らない
  (利用者決定 §6.1)。
- 採用: 全画面の予約は App の flag から要求の持ち物へ移す。flag と pending の 2 か所で同じ予約を持たない。
- 不採用: モーダル化。移動の入力 (ツリー・お気に入り・戻る) まで止める理由がない (§1.805 §3 の D と同じ)。
- 採用: 呼び出しの外で変えていた状態 (フォルダを開く効果・検索の積み上げ・合成一覧の後始末・全画面の close) は、新しい
  保留を作らず、install の中の既存の確定点 (`commit_grid_virtual_open_effects`・`commit_staged_search_navigation_effects`・
  `install_loading_items_inner` の後始末・`change_main_context_for_visible_grid…`) に寄せる。取消時の巻き戻しは作らない。
- 採用: 全画面の待ちは、既存の「全画面のまま次のフォルダを待つ」(Ctrl+↑↓) の sequence に乗せる (§6.12)。全部止めるモーダルは
  入力の面ごとに新しい止めが要るので採らない (r4 の検討)。
- 不採用: ★固定の移動まで今回広げること。続きの型が既存の `RequiredFullscreenTarget` と 4 点で違う (§6.2)。

### 6.9 設計レビュー r1 (GPT-6.1 Sol / xhigh、read-only、30ee454c7、2026-11-26): REJECT

P1×1・P2×4・P3×1。設計担当がコードで照合し、すべて妥当と判断して r2 で直した。detached の境界は「構造的修正であって
症状パッチではない」に **AGREE** (main の既存 context／scope 述語で走査の責務を分けるため。§11 への記録は必要)。
追加の同期走査の入口は無く、対象外の分類は概ね妥当、D5 の staged の経路は mtime を要さず 3 つの owner を扱える、
選択のヒントは navigation が既に `take()` している (`app.rs` 25912)、という所見もあった。

| 指摘 | 対応 (r2) |
| --- | --- |
| P1 C5 は全画面を閉じる処理が読込の中にあり、`Classifying` では全画面が残って採用もされない | C5 を対象外にし、待ちの間の全画面の扱いを統括へ質問 (§6.2) |
| P2-2 D2 の積み上げの先行変更が、拒否される後続での取消で一覧と食い違う | D2 を対象外 (§6.2)。採用の時点へ移す作りは別項目 |
| P2-3 D3 は表示中の合成一覧の状態を先に消す | D3 を対象外 (§6.2)。同上 |
| P2-4 claim の再実行を外すと完了時の document modal の拒否が消える | 採用の直前に同じ述語を評価し、今の再入と同じ結果にする (§6.3 の 3) |
| P2-5 走査の設定・Susie の世代の照合が無い | §3.2 の policy 照合と走査だけのやり直しを共有 (§6.3 の 3) |
| P3-6 「§1.805 の既存テストは期待値を変えない」が新仕様と矛盾 | 意図して変える 2 種を明記 (§6.6 の 11)。兄弟投影・sidecar・Quick Folder・取消/切断のテストを追加 (§6.6 の 2・9) |


### 6.10 設計レビュー r2 (同じ担当の継続、06e53e169、2026-11-26): REJECT

r1 の 6 件は方針上解消と判定 (C5 は統括判断まで同期)。新たに P2×2・P3×1。設計担当がコードで照合し、r3 で直した。

| 指摘 | 対応 (r3) |
| --- | --- |
| P2-1 C1・C3 のフォルダを開く操作が、閲覧履歴の帰路と★フィルタの一時解除を先に変える (`collection_grid.rs` 1544、`gamepad_input.rs` 10094) | ZIP/PDF と同じ `GridVirtualOpenEffects` に捕捉し、install の中の `commit_grid_virtual_open_effects` で確定する。全画面の予約も同じ効果で運ぶ (§6.3 の 8) |
| P2-2 D5 の「mtime が無い」には ZIP/PDF・変換キャッシュも入り、staged へ渡すと論理キーが変わる | 採用済みの表示が実フォルダのときだけに限定。変換キャッシュが走査点に届くなら理由付きの例外 (§6.3 の 6) |
| P3 設定の適用による更新が旧要求を退役させる場合と、要求が生きたまま policy が不一致になる場合が混ざっている | 2 つに分けて記述・テスト (§6.6 の 5) |

### 6.11 設計レビュー r3 (同じ担当の継続、33d08a3eb、2026-11-26): REJECT

r2 の 3 件のうち D5 の限定と設定変更の分割は解消と判定。全画面の予約の他の参照・コレクションの owner の事前照合・main 以外も
同じ install 境界を使う方針は妥当との所見。新たに P2×2。設計担当がコードで照合し、r4 で直した。

| 指摘 | 対応 (r4) |
| --- | --- |
| P2-1 フォルダに ZIP/PDF と同じ効果を使うと `suppress_facet_filter=true` になり、通常フォルダの履歴が `FacetScope::Book` になる | フォルダでは false と明記し、facet の親→子→孫→戻るの検査を追加 (§6.3 の 8、§6.6 の 6a) |
| P2-2 ★の判定を採用時に path 版 (DB) でやり直すと、受付時の idx 版 (session の書き込み・cache 優先) と食い違う | 受付時の idx 版の決定を別の field で運び、採用時に判定し直さない (§6.3 の 8) |

### 6.12 C5 全画面から戻る経路: 全画面の Ctrl+↑↓ の待ちと同じ扱い (利用者決定 §6.1、r6)

利用者が選んだ案 (2026-11-26、§6.1 の 3 つ目の決定。共有メモ `uiscan-853b-fullscreen-close-proposal.md` の「改めた案」)。r4 の「全部止めるモーダル」は
設計レビュー r4 で、専用の全画面 viewport の入力 (`ui_fullscreen.rs` 28906・35595)・native HUD の output event
(`native_video.rs` 6212・6833)・root の focus guard (`app.rs` 65578)・App 共通のゲームパッドの解放 (`gamepad_input.rs` 4745、
前面の detached の操作まで途切れる) に新しい止めが要ると分かり、取り下げた (§6.13)。

今の作り: 「閉じる」で親の一覧 (L1) へ戻る経路は、その場では全画面を閉じず `pending_return_to_parent` を立て、同じフレームの
入力の合流点 (`app.rs` 103668〜103683、もう 1 か所 103814) で `Direct(親)` / `CollectionSource` の読込を出す。全画面を閉じるのは
読込の install の中 (`install_loading_items_inner` 45042〜45048 → `change_main_context_for_visible_grid_for_destination`
71740〜71777 → `close_fullscreen_content_for_destination` 84669)。閉じる要求の時点で `finish_fullscreen_navigation_for_true_close`
(`snapshot_ops.rs` 2271〜2294) がスライドショー・ナビの sequence を終え、holdover も消える (`release_fs_nav_lock`、
`ui_fullscreen.rs` 14902)。読込が失敗・拒否されると全画面のまま (`app.rs` 25206〜25231、再試行なし)。

**見え方とキー (利用者決定)**:

- 閉じる操作の後、親の一覧を worker で読み終わるまで全画面 (今のページ) を出したまま。読み終わったら今と同じ install の中で
  全画面を閉じて親の一覧に替わる。ページ一覧は今どおり見せない。読み込み中の表示は出さない (全画面の Ctrl+↑↓ の待ちと同じ。
  `fs_page_wait_indicator_visible` は `FolderItems` の待ちでは偽、`ui_fullscreen.rs` 4801)。
- 待ちの間のページ送り・ジャンプ・スクロールは受け付けない (押し直しが要る)。拡大・回転・★などは今のページに効く。もう一度
  「閉じる」を押すと、閉じる要求をやり直し (true-close の後始末 → 同じ親への読込を受け付け直し、前の待ちは退役)、結果は同じ
  親の一覧。
- 読めなかった・別の要求 (Remote・起動の転送・agent 等の受付) に退役させられたとき: 全画面のまま。失敗は今と同じ通知。
- 不採用: 先に全画面を閉じて待つ (ページ一覧が見え、今の作りに反する)。全部止めるモーダル (上記)。

**仕組み (既存の sequence に乗せる)**:

1. 受付: 今の入力の合流点で出す読込が、§6.3 の 1 の判定で worker になる。continuation に「main の全画面から戻る」印
   (例: `closes_main_fullscreen: bool`、受付時に main の全画面が出ていたか) を持たせる。新しい pending は作らない。
2. 待ちの間の全画面: r6 (設計レビュー r5 P2-1): **true-close の後始末より前に**、今描いている表示単位を捕捉する
   (表示中の holdover があればそれを先に、無ければ今の items/cache から。`ui_fullscreen.rs` 13036 の既存の注意と同じ順。
   `capture_fs_nav_holdover` 15045 は今の items/cache から取るので、後始末の後では旧画像を失う)。捕捉した表示単位は、
   既存の型付きの close continuation (`FullscreenCloseContinuation` の `Live` 等) に描画資源としてだけ持たせ、旧 sequence の
   操作の権限は引き継がない。worker を出した直後 (§6.3 の 5 の受付時の後始末、DFS holdover の `Superseded` 終了の**後**) に、
   その表示単位を `previous` にして `FsNavigationSequenceTarget::FolderItems` の sequence を、この待ち専用の目的
   (例: `FsNavigationPurpose::CloseReturn`) で始める (`begin_fs_folder_navigation_sequence` 14088)。移動先が未準備のまま閉じても
   黒や loading を挟まない。捕捉できる表示単位が無い (まだ描けていない) ときは、既存の gap surface (`draw_fs_navigation_gap_surface`) のとおり。
   r7 (設計レビュー r6 P2-2): **描画は Ctrl+↑↓ と変える**。Ctrl+↑↓ の `FolderItems` は `previous` を捕捉時の倍率・位置・回転で
   描き続ける (`ui_fullscreen.rs` 14809・29733・52720・52780) が、C5 の待ちでは items は採用まで変わらず、利用者決定で拡大・回転は
   今のページに効く。そこで `CloseReturn` の目的のときは、**今のページが描ける (準備済み) なら通常の描画で描き** (拡大・回転・位置の
   変更が見える)、描けないときだけ捕捉した表示単位を代わりに描く。対象を変える入力の止め (lock・`blocks_new_target`) は sequence の
   まま効かせる。描画の選び方は目的で分け、Ctrl+↑↓ の描画は変えない。
   実装の注意 (設計レビュー r7 P3): 今の描画の選択 (`ui_fullscreen.rs` 14849 付近) は、選んだ結果に応じて holdover・sequence を
   変える。`CloseReturn` では、通常の描画へ切り替えても sequence と lock は終えない (終えるのは 4・4a の終端だけ)。「準備済み」は
   見開き全体を描ける条件で判定する (既存の表示単位の規則 `ui_fullscreen.rs` 30263)。切り替えた後もページ送りが断られることをテストする。
   ページ送り・ジャンプ・スクロールは、既存の `fs_nav_is_locked()`・`fs_navigation_sequence_blocks_new_target()` の判定
   (全画面・専用 viewport・page overview・in-page scroll・動画の側にある) でそのまま止まる。新しい入力の止めは作らない。
   閉じる操作は Ctrl+↑↓ の待ちと同じく止めない。
   r6 (設計レビュー r5 P2-4): native 動画の Home/End は `open_native_video_fullscreen_from_navigation` を直接呼び
   (`native_video.rs` 13215)、lock の判定を通らずに選択を変えて fast swap を始める (15875)。前後移動は `fs_nav_is_locked()` を
   見ているので、**native の対象を変える入口 (Home/End・HUD からの対象指定) を、既存の sequence/lock を評価する共通の受付へ
   まとめ**、選択と swap を変える前に断る。これは今の Ctrl+↑↓ の待ちの間にも同じ迂回がある (既存の不具合) ので、一緒に直る。
   実装担当は native の対象を変える入口を列挙する。r7 (設計レビュー r6 P3): この共通受付は**新しい入力の受付**だけに置く。
   同じ helper は受付済みの非同期の結果や EOF の続き (`app.rs` 101922・102079) からも呼ばれるが、それらは受付済みの続きなので
   審査し直さない (受付の型で区別する)。待ちの無いときの Home/End・HUD の操作と、swap 中の後続入力の既存の保持は変えない。
3. 完了の poll: 全画面のフレームは通常の navigation の末尾 (`post_render_main_navigation`) に届かない (embedded の
   early-return 103809〜103870、native の背景 `update_phases.rs` 31〜46)。既存の `poll_main_embedded_required_fullscreen_folder_open`
   (`app.rs` 62636) が `RequiredFullscreenTarget` だけを回収している場所で、上の印の `DirectNavigation` も回収し、§6.3 の 3 の
   採用 (modal・policy の照合 → 遅らせた同じ呼び出し) を行う。native の背景の経路にも同じ回収を置く。全画面では一覧の nav 源が
   無く、全画面の移動の入力は 2 で止まっているので、同じフレームの裁定は通常の末尾と同じ規則で足りる (実装担当は、全画面の
   フレームで同じ pending を奪い合う別の nav が無いことを確かめる)。
4. 終端: r6 (設計レビュー r5 P2-2): install が呼ぶ content だけの close は `ViewerExited` を出さず、`FolderItems` の sequence は
   目的によらず「viewer が続く (閉じて開き直す)」と読まれる (`app.rs` 12693)。そのままだと、通常の終了で行う情報パネルの reset と
   viewport を作り直す方針が飛ばされる (85080・85195。固定した情報パネルが次の全画面へ持ち越される)。**採用が確定した境界
   (遅らせた呼び出しが install に入る直前) で、同じ要求の `CloseReturn` sequence を `ViewerExited` で終えてから** content の
   teardown へ進む。採用以外の終端 (走査の失敗・退役・`Disconnected`・`cancel_folder_pane_open`) では、同じ要求が始めた sequence を
   `Superseded` で終え、全画面のまま入力が戻る。どちらも sequence の終わりは pending の purpose 別の後始末 1 か所に置く
   (入口ごとに書かない)。
4a. 待ちの間に全画面が別の経路で閉じたとき (r6、設計レビュー r5 P2-3): Backspace はページ一覧への close
   (`close_fullscreen_to_page_list`、`startup_list_restore.rs` 423)、focus guard は `close_fullscreen` (`app.rs` 65591) を呼ぶ。今の
   true-close の後始末が取り消す pane の走査は `RequiredFullscreenTarget` だけ (`snapshot_ops.rs` 2276、`app.rs` 62519) なので、
   放っておくと L2 を出した後に残った走査が親へ移してしまう。**新しい close を受け付ける境界で、その context の C5 の要求も
   退役させる** (`RequiredFullscreenTarget` と同じ扱い。置き場所は下の r7 の段落のとおり)。採用のときは pending を取り出した後に
   install が全画面を閉じるので、この退役に当たらない (自分の採用を消さない)。二度目の「閉じる」は、この退役の
   後に新しい要求として受け付けられる (5)。
   r7 (設計レビュー r6 P2-1): 退役を置くのは**新しい close の受付**の境界 (true-close の後始末、Backspace のページ一覧への close の
   受付、focus guard の close の受付) だけにする。**受付済みの close の teardown では繰り返さない**: native の presentation 切替中の
   content の close は `TerminalClose` を積んで戻り (`app.rs` 84684)、後の effect が content の close へ再入する
   (`native_video.rs` 3166)。その間に agent・起動の転送の受付 (`update_phases.rs` 542、`app.rs` 103125) が新しい C5 の要求を
   受け付けていると、遅れた close がそれを退役させてしまう。既存コードも、この effect は「予約の退役は済んでいる」としている
   (`app.rs` 84715)。共通の content-close の入口に退役を置かない。
5. もう一度「閉じる」: `handle_fullscreen_close_request` は今どおり true-close の後始末 (sequence の終了、4a の退役) から始まり、
   `pending_return_to_parent` の受付 → 合流点の読込が新しい要求になる。予約・選択のヒントは前の要求と
   一緒に消え、新しい要求が受付時のものを持つ (閉じる要求ごとに子の名前を置き直す今の `take_close_return_navigation` のまま)。
6. 待ちの間に始まり得る他の要求 (Remote の取得、起動の転送、agent の open、ブックマーク): 既存の受付の境界が待ちを退役させ
   (4 の後始末で sequence も終わる)、その要求の採用が全画面を扱う (§1.805 の規則 2 と同じ)。
7. 待ちの間の Backspace・focus guard による close は 4a のとおり待ちを退役させ、今の close の結果 (ページ一覧等) になる。

**テスト**: (a) 「ページを直接開く」の本を閉じる・ブックマーク・コレクションの物理元の閉じ戻りで、閉じたフレームに UI 走査 0 回、
待ちの間は全画面が出たまま (`fullscreen_idx` が残る)、描画は閉じる前の表示単位の holdover (移動先が未準備のまま閉じた場合も)、
完了で今と同じ親の一覧・選択になり、その間のフレームで一覧が L2 にならない。(b) 待ちの間の →・PageDown・ジャンプが何も
起こさず、採用後の一覧にも届かない。(c) 走査失敗・Remote 等による退役で全画面のまま、sequence が終わってページ送りが戻る。
(d) もう一度「閉じる」で前の待ちが退役し、新しい待ちの完了で同じ親の一覧になる。(d2) 待ちの間の Backspace・focus guard の close の
後に遅い結果が来ても採用されない。(d4) native の presentation 切替中の遅れた `TerminalClose` の effect より前に受け付けた新しい
C5 の要求が、その effect で退役しない。(d5) 待ちの間の拡大・回転・位置の変更が描画に出る (今のページが準備済みのとき)。今のページが
未準備で閉じたときは捕捉した表示単位が出て、準備ができたら通常の描画に替わる。(d6) 待ちの無いときの native の Home/End・HUD の対象
指定と、swap 中の後続入力の保持が今と同じ。(d3) 採用のフレームで `CloseReturn` の sequence が `ViewerExited` で終わり、固定した情報パネルが
次の全画面へ持ち越されない (通常の close と同じ)。(e) 動画の全画面から戻る経路 (native の背景) の完了と、待ちの間の動画側の
前後移動・Home/End・HUD の対象指定の止め (handler-level)。Ctrl+↑↓ の待ちの間の native Home/End も止まる。(f) detached を投影している間は main の待ちの影響を受けない。(g) 全画面の Ctrl+↑↓ の
既存テストが変わらない。

### 6.13 設計レビュー r4 (同じ担当の継続、20ae1c90e、2026-11-26): REJECT

r3 の 2 件は解消と判定。新たに P2×4・P3×2。設計担当がコードで照合し、r5 で直した。

| 指摘 | 対応 (r5) |
| --- | --- |
| P2-1 D3 で状態を残して普通の読込を出すと、帰り先がスマートの entry・配下のときスマートの分岐へ入り直す | 抜ける操作を既存の `ExitTemporaryView` の navigation で出し、採用時の `dismiss_transparent_view_for_admitted_jump` に片付けさせる (§6.3 の 10) |
| P2-2 C5 の true-close の後始末が holdover を消し、待ちの間に黒/loading を挟む | worker を出した後に holdover を取り直し、Ctrl+↑↓ と同じ sequence を始める (§6.12 の 2) |
| P2-3 root のゲートだけでは全画面の入力を止められない (専用 viewport・native HUD・focus guard) | 全部止めるモーダルをやめ、既存の Ctrl+↑↓ の待ちの止めに乗せる。統括へ案を改めて返した (§6.12) |
| P2-4 sidecar のジェスチャ終了の流用は detached のゲームパッド操作まで壊す | sidecar のゲートを流用しない (§6.12) |
| P3-5 D2 の巻き戻しを全部外すと、選択のヒントが即時の拒否で漏れる | ヒントの巻き戻しは残す (§6.3 の 9) |
| P3-6 native 動画では待機表示の描画に届かない | 待機表示を出さない案に変えた (Ctrl+↑↓ の待ちと同じ。§6.12) |

### 6.14 設計レビュー r5 (同じ担当の継続、aa1276045、2026-11-26): REJECT

r4 の D3・sidecar の共用・D2 の選択のヒントは解消と判定 (待機表示は新しい仕様で対象外)。新たに P2×4 (どれも C5)。設計担当が
コードで照合し、r6 で直した。

| 指摘 | 対応 (r6) |
| --- | --- |
| P2-1 true-close の後で取り直しても、holdover だけが持っていた旧画像は失われている | true-close の前に表示単位を捕捉し、close continuation で描画資源として運んで新しい sequence の `previous` にする (§6.12 の 2) |
| P2-2 install の content の close は `ViewerExited` を出さず、`FolderItems` は閉じて開き直すと読まれる (情報パネルの reset 等が飛ぶ) | 採用の確定時に `CloseReturn` の sequence を `ViewerExited` で終えてから teardown (§6.12 の 4) |
| P2-3 待ちの間の Backspace・focus guard の close で走査が残り、L2 の後に親へ移る | 全画面を閉じる既存の境界で C5 の要求も退役させる (§6.12 の 4a) |
| P2-4 native の Home/End が lock を迂回する | native の対象を変える入口を、sequence/lock を評価する共通の受付へまとめる (§6.12 の 2。Ctrl+↑↓ の待ちの既存の迂回も直る) |

### 6.15 設計レビュー r6 (同じ担当の継続、05effb7b2、2026-11-26): REJECT

r5 の 4 件への方針は反映済みと判定。新たに P2×2・P3×1。設計担当がコードで照合し、r7 で直した。

| 指摘 | 対応 (r7) |
| --- | --- |
| P2-1 4a の退役を共通の content-close に置くと、遅れた `TerminalClose` の再入が新しく受け付けた C5 の要求を退役させる | 退役は新しい close の受付の境界だけに置き、受付済みの teardown では繰り返さない (§6.12 の 4a) |
| P2-2 `FolderItems` は `previous` を捕捉時の変換で描き続け、待ちの間の拡大・回転が見えない | `CloseReturn` では今のページを通常の描画で描き、描けないときだけ捕捉を代わりに描く (§6.12 の 2) |
| P3 native の共通受付が受付済みの続き (非同期の結果・EOF) まで審査し直すおそれ | 新しい入力の受付だけに置き、待ちの無いときの挙動の維持テストを追加 (§6.12 の 2、テスト d6) |

### 6.16 設計レビュー r7 (同じ担当の継続、f0963a999、2026-11-26): ACCEPT

r6 の P2×2・P3×1 は設計上解消、追加の P1/P2 は無しと判定。detached の境界は「構造的修正であって症状パッチではない」に
**AGREE** (走査の判断・close の新しい受付・採用済みの teardown・描画資源の所有を、それぞれの境界へまとめている)。
P3×2 は実装の注意として反映した: 4a に残っていた古い置き場所の記述を置き換えた。通常の描画への切り替えと sequence の終わりを
分ける注意を §6.12 の 2 に足した。

### 6.17 実装の記録

2026-11-26、`ui-scan-853b`、基点 `30cdc0774`。§6 の C・D・E の実装差分。
**コンパイル・Rust テスト・nextest・ビルド・製品起動は行っていない。独立実装レビューも未実施。**
設計担当の共有 heavy-job runner へ検証を渡す。commit / stash / reset / checkout / branch switch は行っていない。
ファイルの担当を分けた GPT-6.1 Sol / high の実装担当へ一部を委譲した。親のモデルをその設定へ変更したという意味ではない。

最終の静的確認: 変更／追加した Rust 96 ファイルへ単体ごとの `rustfmt --edition 2024 --check --config skip_children=true`。
95 ファイルが初回成功、残る新 C5 テストの整形差分を適用して当該 1 ファイルも再確認成功。
`git diff --check` 成功、`python scripts/check_ui_glyphs.py` は危険な UI 字形 0 件。
これは型検査・コンパイル・テストの成功を意味しない。

| 変更ファイル | 設計項目と実装 |
| --- | --- |
| `src/app.rs` | §6.3(1–5): 共通走査点の既定反転、boxed continuation、modal／owner 再照合、同じ loader の再開、要求所有の選択・予約、理由付き例外。§6.3(6・8・9): D5 staged、Folder 効果・型付き検索帰路の確定。§6.12: close 捕捉・採用・取消 |
| `src/app/folder_scan.rs`、`src/app/startup_ops.rs` | §6.3(3): 起動／pane 共通の policy・Susie 前後照合。不一致は同じ要求・token・開始時刻を保つ受信口交換 |
| `src/app/update_phases.rs` | §6.3(3・5・8): 通常 nav 裁定・continuation 採用・生成点の worker 印の撤去。§6.12(3): native 背景／embedded 完了回収 |
| `src/app/collection_grid.rs`、`src/app/gamepad_input.rs`、`src/ui_main.rs`、`src/global_search_ui.rs` | §6.3(8): GridFolder carrier、受付時の idx ★判定・履歴・auto-fullscreen の効果。Folder の facet suppression は false、Rating の固有 staged 着地は維持 |
| `src/app/list_order_navigation.rs` | §6.3(9): 検索の Descend／Back／Sibling とタグ Back |
| `src/app/smart_folder.rs`、`src/app/subfolder_expansion.rs` | §6.3(10): ExitTemporaryView の採用まで表示状態を保持、機能 worker 取消・定義無効化だけを受付で行う |
| `src/app/close_return.rs`、`src/app/snapshot_ops.rs`、`src/app/startup_list_restore.rs`、`src/ui_fullscreen.rs` | §6.12: true-close 前の表示単位、request ID に属する CloseReturn、通常描画でも lock 維持、新 close 受付だけで退役。§6.3(4): SnapshotLeafOpen／TrayClose |
| `src/app/native_video.rs` | §6.12(2): 新規入力の共通受付と受付済み continuation の区別 |
| `src/app/release_checks.rs` | §6.3(4): ReleaseCheck の理由付き同期契約（PlayTest は app.rs、変換キャッシュの実フォルダ更新は ConvertedCacheReload） |
| `src/app/location_jump.rs`、`src/app/drive_jump.rs`、`src/app/collection_navigation.rs` | §6.3(5): 生成点の worker 走査印を既定へ吸収、場所・履歴の意味を持つ nav 値は維持 |
| `scripts/analyze_perf.py` | §6.5: purpose／owner／intent／auto_fs／rescan 別の内訳、旧ログの欠損値も扱う |
| 下記のテストファイル・共有 fixture | §6.6・§6.12: worker 完了と実 post-render／fullscreen poll を通す採用待ち、回帰検査 |
| `docs/ui-responsiveness.md`、`docs/async-architecture.md`、`docs/direct-navigation-worker-scan-plan.md`、`docs/detached-rework-plan.md`、`docs/display-pipeline.md`、`docs/video-architecture.md`、本書 | §6.7: 境界・例外・待ち方・既存設計合意・未検証の記録 |

実装時の照合:

- 未走査で入る二入口は Ordinary。main の navigation は既存 capture で得られ、選択は capture 時に App から take 済み。
  採用で App の選択ヒントを読み直さない。owner の読み取り検査は snapshot・rating・Quick Folder・collection・bookmark の照合を含む。
- 起動・書庫等、App の全画面 flag を立てる既存入口が残るため §6.3(2) の take を残した。Folder の入口は effects が予約を持つ。
- D5 は既存の `current_sort_reload_is_physical_directory` と logical override 不在で実フォルダに限定する。
  変換キャッシュの実ディレクトリは理由付きの同期、ZIP/PDF の早期 return は維持。
- SnapshotLeafOpen の続きは別項目。RequiredFullscreen の `SnapshotTarget::Fs` は上流で既存 worker に分岐する。
  横断ブックマーク一覧の Opening は従来の synthetic Bookmarks 復元であり、物理親の C5 に偽装しない。
- D3 の entry 自身／同じ子への帰還も ExitTemporaryView。削除定義が取消で表示に残る場合は、既存の定義不在検査が refresh／child adoption を拒否する。
- D2 の返却事項（§6.3(9) の「今と違う結果になるなら返す」）: 基点の `favsearch_back` は pop の後の loader が同じ親を push し、
  `[root, child]` が `[root, root]` になる。Sibling も先の `[next]` への置換の後に loader が push して `[next, next]` になる。
  次の BS の行き先を変えないため、今回の typed Back／Sibling はこの成功時の既存結果を採用時へ移す。
  重複を除いて一段ずつ戻す既存不具合の修正は、設計担当／利用者の判断へ返す（質問は未回答、黙って変更していない）。
  `commit_staged_search_navigation_effects` の Back／Sibling 分岐と新しい
  `section1853_favorite_back_defers_effects_and_preserves_released_duplicate_parent`／
  `section1853_favorite_sibling_defers_replace_and_preserves_released_duplicate_root` が証拠。
  D2 の non-main は §6.2 の対象外規則に従って従来の同期順を維持する。
- マニュアルの読み込み中のキーに関する既存記述を検索したが、変更すべき記述は見つからず htdocs は変更していない。
  `docs/next-release-backlog.md` は統括担当のため変更していない。

意図した既存期待値の変更（最終の一覧・選択・履歴・intent は保持）:

- §6.6(11a): `direct_navigation_worker_admission_does_not_take_global_fullscreen_reservation` を
  `direct_navigation_worker_admission_takes_and_retires_global_fullscreen_reservation` へ改名。App flag は false、予約は要求が所有。
- §6.6(11b): `direct_navigation_worker_unmarked_producers_keep_synchronous_adoption` を
  `direct_navigation_worker_unmarked_producers_scan_off_caller_and_keep_adoption` へ改名。
  `direct_navigation_worker_preserves_unmarked_presentation_intents` と collection の
  `direct_navigation_sync_preserves_collection_physical_owner`（新名 `direct_navigation_worker_preserves_collection_physical_owner`）も
  同期走査 1 回の前提を 0 回＋完了待ちへ変更。
- 同じ予約の所有変更: `section1335_bookshelf_compiled_folder_direct_read_and_explicit_list_restart_differ`、
  `section1335_rating_folder_auto_request_records_the_actual_settled_physical_list` と共有 `open_selected_image_folder_book`。
  同じ auto 値を GridFolder.effects で検査し、App に先行予約が無いことを確認する。
- §6.3(8)・§6.6(6a): `ratingnav649_folder_enter_name`、`ratingnav649_folder_enter_date_desc`、
  `ratingnav649_folder_double_click_name`、`ratingnav649_folder_double_click_date_desc`、
  `ratingnav649_folder_gamepad_name`、`ratingnav649_folder_gamepad_date_desc`、
  `ratingnav649_enter_and_pointer_folder_receipt_preserve_history_return`。
  先行 history／★解除の値を effects へ移し、待ちの App は旧状態、採用後は従来の結果。
- carrier を Direct／CollectionSource／RatingSource から GridFolder へ変えた箇所は、同じ path・owner・auto 値を検査する。
  その他の変更は、同期採用の前提を共通の完了 helper へ移すか、型の引数を補ったもの。
- C5 の描画資源を持つ LiveWithPresentation を、既存の Live と同じ pending として許容する型の補完は
  `close_return_real_fullfeature_media_is_shared_until_promote_or_live_fork` と
  `egui037_wheel_frame_recognized_live_return_keeps_original_page_and_return_owner` の既存の pending 検査にも適用した。

共有 runner へ渡す exact nextest filter（この担当では未実行）:

```powershell
$scan853Filter = 'test(section1853_) | test(direct_navigation_worker_) | test(close_return_) | test(grid_folder_) | test(grid_collection_folder_) | test(ratingnav649_) | test(section1335_) | test(section1328_) | test(section1339_) | test(history_parent_) | test(same_position_double_click) | test(g1_) | test(tag_view_) | test(favsearch) | test(native_backdrop_) | test(native_target_) | test(fullscreen_) | test(folder_pane_) | test(section1285_)'
cargo nextest run -p mimageviewer --lib -E $scan853Filter
cargo nextest run -p mimageviewer --lib --features test-script -E $scan853Filter
# 共通入口の既定変更なので focused 後に lib 全件も両構成で必要。
cargo nextest run -p mimageviewer --lib -E 'all()'
cargo nextest run -p mimageviewer --lib --features test-script -E 'all()'
```

設計担当は既存 run-heavy／ledger と `scripts/test-lib-full.ps1`／`scripts/test-full.ps1` の運用に載せる。
native 実機・snapshot 生成と確認・perf 計測・検証用 binary のビルドも設計担当に残る。
この担当から normal profile の製品を起動しない。

ファイル別の追加・変更テスト／共有 helper 名（同期採用待ちだけの移行を含む）は以下に記録する。


#### 初回コンパイル後の型修正（2026-11-26）

設計担当が `5f0ab660f` で既定 feature の lib test をコンパイルし、26 エラーを報告した。
提供された `test-default.log` 全文を読み、今回の型変更の追従漏れを修正した。
実装担当自身は cargo／nextest／ビルド／テスト／製品起動／コミットを行っていない。

- `snapshot_ops.rs`: 同期例外の `DirectoryScanRoute`／`UiThreadScanReason` の import を補った（6 エラー）。
- `image_folder_zip_operation.rs`: modal が所有する PaneNavigation の返信から、型付き結果の `scan` を取り出す（2 エラー）。
  DirectNavigation の policy 採用経路は変更しない。
- `app.rs`・`tests.rs`・`startup_ops.rs`: 偽の pane 返信を作る helper を `cfg(test)` の App メソッドへ集約し、
  現在の App の policy を付ける。親テスト module の import 依存と、裸の `io::Result` を送る箇所を修正した（6 エラー）。
  mutable な pending を借りたまま差し替える fixture は、先に policy を捕捉する。
- `location_jump.rs`: 完成候補を組み立てる fixture は返信の `scan` を取り出して成功／失敗を検査する（6 エラー）。
- `tag_view_open_supersession_tests.rs`: 返信全体を保持・再送する fixture は `scan.scan.is_ok()` を検査し、policy も保持する（3 エラー）。
- `ui_scan_853_tests.rs`: App の所有型に合わせ `ArchiveConvertState` を Box に入れる（1 エラー）。
- `close_return.rs`・`ui_fullscreen.rs`: registry の二つの ID メソッドの可視性は維持したまま、
  close-return owner が main であるかの述語だけを owner 側から描画側へ提供する（2 エラー）。判定条件は同じ。

テストの期待する一覧・選択・履歴・成功／失敗は変更していない。ログの本体 115 警告は、
診断行と import の使用箇所を基点 `30cdc0774` と照合し、今回追加由来と判断するものは見つからなかったため既存警告を残した。
`test-script` 側も、走査要求の数え上げ・`folder_load_recorded` の二つの initializer・ReleaseCheck・
親一覧の modal scan 返信を静的に点検した。今回の修正には feature 固有の field を追加していない。
両 feature 構成のコンパイル成功とテスト実行結果は、設計担当の runner での確認が必要。
今回変更した Rust 10 ファイルは単体ごとの `rustfmt --edition 2024 --check --config skip_children=true` に成功。
`git diff --check` も成功した。これらはコンパイル・テストの結果ではない。

#### テスト実行後の修正 (r1)

設計担当が `616f04b35`（型修正＋cargo fmt）で関連 nextest を実行し、既定構成は
1,046 件中 1,030 成功・16 失敗、`--features test-script` は 1,051 件中 1,035 成功・同じ 16 失敗だった。
両構成のコンパイル成功も設計担当の結果である。提供された `test-both-2.log` の両構成の失敗と
入力・採用経路を照合して、以下を修正した。実装担当自身はコンパイル・テストを実行していない。

| ファイル・失敗テスト | 根因と修正 |
| --- | --- |
| `smart_folder.rs` / `g1_smart_real_handler_reopen_restores_legacy_flow_and_book_return` | 最初の実フォルダの走査を採用せず Book recipe を検査していた。初期読込を既存の worker 完了＋post-render＋hydration helper へ変更。Book と帰路の期待値は維持 |
| `snapshot_ops.rs` / `g1_snapshot_a2_deactivation_preserves_source_book_presentation` | 同じ初期採用待ち不足。元 Book を採用してから snapshot を作る。元の表示状態を保存・復元する期待値は維持 |
| `subfolder_expansion.rs` / `g1_subfolder_real_handler_reopen_restores_legacy_flow_and_book_return` | 同じ初期採用待ち不足で初期 Single を検査していた。元 Book の採用を待つ。RtlCover・flow・帰路の期待値は維持 |
| `filename_stack_ui.rs` / `g1_subfolder_stack_book_reopen_preserves_vertical_flow_and_original_book` | 展開終了の D3 帰還だけ採用前だった。終了後に同じ worker 完了 helper を追加。元 Book・Vertical の期待値は維持 |
| `startup_ops.rs` / `section1285_reload_keeps_unresolved_initial_request_without_user_navigation` | resolver の模擬結果が `folder_scan=None` のため、その続きが走査待ちになった。resolver poll 後に worker 採用を待つ。reload が resolver を取り消さない検査と最終行き先は維持 |
| `startup_ops.rs` / `section1285_untouched_initial_resolver_still_opens_its_target` | 同じ模擬結果の採用待ち不足。worker 完了を待ち、行き先と完了 receiver の退役の期待値を維持 |
| `tag_view_list_order_tests.rs` / `tag_list_order_folder_then_archive_returns_through_folder_before_results` | 共通 settle が DirectNavigation を poll せず idle と判定し、Back の積み上げ確定前に検査していた。通常 post-render の poll と pending の終了条件を追加。2 段の戻り先・積み上げ・run の期待値は維持 |
| `tests/history_parent_1473.rs` / `section1473_v490_folder_enter_producer_boundary` | 「現行 helper は v4.9.0 と同一」という歴史的 fixture の前提が、§6.3(8) の効果捕捉で失効した。当時の `Direct(path, container_open(false))` payload を明示する。歴史的 producer の最終帰路の期待値は維持し、現行 Enter の効果は現行テストで検査 |
| `tests/startup_restore.rs` / `section1335_rating_folder_auto_request_records_the_actual_settled_physical_list` | 製品側の退行。新しい Folder effects の auto 予約を staged 採用で一律適用し、従来の Rating Folder の一覧採用を全画面へ変えていた。`commit_physical_history_transition` で Rating owner＋Folder payload の既存方針を保つ。要求内の予約の保持を追加検査し、最終一覧と起動復元先の期待値は維持 |
| `collection_grid.rs` / `grid_folder_enter_and_gamepad_defer_visible_effects_until_adoption` | fixture が画像フォルダ用の子設定だけを有効化し、必須の親設定を無効のままにしていた。`auto_fullscreen_zip_pdf=true` も設定。効果と旧表示不変の期待値は維持 |
| `tests/search_scan_853_tests.rs` / `section1853_tag_back_survives_classification_and_staged_zip_adoption` | ZIP は分類 owner ではなく staged PhysicalHistory owner が戻り意図を持つ。その要求を検査する。最終 ZIP と積み上げの期待値は維持 |
| 同 / `section1853_tag_back_intent_is_owned_by_pdf_and_conversion_classification` | PDF は直接の列挙 owner、RAR/7z は分類 owner。各実 owner の Back を検査する。積み上げの先行変更なしの期待値は維持 |
| 同 / `section1853_search_back_classifies_suffix_directories_before_virtual_dispatch` | ZIP の staged preflight と EPUB/7z の分類を同一 owner と誤認していたため、各要求を検査する。また未分類・表示済み Folder 証明の無い PDF 名の実フォルダは基点 `30cdc0774` でも直接 PDF 経路に入る未対応ケースだった。この新規テストの成功対象から PDF 名の実フォルダだけを除外し、ZIP/EPUB/7z の一覧・選択・積み上げ・要求数の期待値を維持。PDF ファイルの Back 所有と準備済み PDF 採用は別の既存追加テストで検査する |
| 同 / `section1853_favorite_sibling_keeps_suffix_directory_classification_and_single_count` | ZIP 名の実フォルダは分類ではなく staged preflight。実際の要求を検査する。最終一覧・重複を含む既存積み上げ・test-script の要求数は維持 |
| 同 / `section1853_temporary_view_exit_shares_folder_preflight_and_keeps_exit_authority` | ZIP 名の帰還先は staged preflight が ExitTemporaryView を所有する。その要求を検査する。採用までの session 保持と採用後の終了の期待値は維持 |
| `tests/ui_scan_853_tests.rs` / `section1853_collection_descendant_close_keeps_physical_owner_through_parent_wait` | fixture が collection actor の非同期起動直後に mutation を出し、C5 に到達する前に `Starting` で失敗していた。通常の App consumer で Ready を待ってから fixture を作る。C5 の owner・選択・全画面・帰還先の期待値は維持 |

今回、既存テストの最終一覧・選択・履歴・表示の期待値は変更していない。新規テストの誤った
中間 owner の検査と PDF 名の実フォルダの対象範囲を上記のとおり是正した。
新しい設計返却事項はない。D2 のお気に入り検索の重複 stack に関する前記の返却は、既存動作を維持したまま継続する。

このログで確認できた本変更由来の `unused_must_use` 3 件も修正した。
`app.rs` の理由付き読込／D3 の void wrapper と `startup_list_restore.rs` の TrayClose は、
結果の通知・失敗処理を loader に委ね、戻った直後の状態確定をしない呼び出しなので `let _ =` を明示する。
既存警告を消すための lint 抑制は追加していない。
今回変更した Rust 13 ファイルは単体ごとの `rustfmt --edition 2024 --check --config skip_children=true` に成功し、
`git diff --check` も成功した。修正後の両構成のコンパイル・テスト成功は未確認である。

設計担当へ渡す r1 focused filter（両構成で未実行、従前の関連／全件 gate も引き続き必要）:

```powershell
$scan853R1Filter = 'test(g1_) | test(section1285_) | test(tag_list_order_) | test(section1473_v490_folder_enter_producer_boundary) | test(section1335_rating_folder_auto_request_records_the_actual_settled_physical_list) | test(grid_folder_enter_and_gamepad_defer_visible_effects_until_adoption) | test(search_scan_853_tests) | test(section1853_collection_descendant_close_keeps_physical_owner_through_parent_wait)'
cargo nextest run -p mimageviewer --lib -E $scan853R1Filter
cargo nextest run -p mimageviewer --lib --features test-script -E $scan853R1Filter
```

#### テスト実行後の修正 (r2)

2026-11-27。設計担当が `d2768a9bb` で `scripts/test-lib-full.ps1` を実行した結果、
既定構成は 20,862 件中 20,829 成功・33 失敗、`--features test-script` は 21,024 件中
20,989 成功・同じ 33 件＋2 件の計 35 失敗（両構成とも ignored 106 件）だった。
提供された `full-fails.txt`・`full-panics.txt`、各 run の実ログ（`stdout.log` は空で、診断は
`stderr.log`）を読み、下表の入力・採用・復元の境界をコードで照合した。
実装担当自身はコンパイル・cargo・nextest・ビルド・製品起動・コミットを行っていない。

| ファイル・失敗テスト（module 接頭辞省略） | 根因と修正 |
| --- | --- |
| `browser818_presentation_tests.rs` / `browser818_presentation_activation_restore_materialized_keeps_request_seed` | 未走査の模擬 resolver 応答から Directory worker が始まるため、sidecar はまだ始まっていなかった。走査採用だけを完了し、新しい復元 owner を残して従来の seed を検査 |
| `app.rs` / `saved_reveal_preserves_a_staged_classification_without_reserving_navigation` | 初期 source の読込が未採用のまま次の分類へ進んでいた。source のみ完了 helper へ変更し、後続 candidate の pending／request ID は維持 |
| `collection_grid.rs` / `failed_independent_folder_load_keeps_collection_surface_and_history_unchanged` | main 未走査要求の受付を同期 I/O の失敗と誤認。§6.3(1) の `Classifying` 受付→worker 失敗通知→元 surface／履歴不変を検査 |
| 同 / `failed_independent_scan_keeps_collection_root_surface_and_items_atomic` | 同じ受付／完了の混同。worker の失敗を待ち、元 root・items・世代・アドレス等の atomic な保持の期待を維持 |
| `app.rs` / `disabling_memory_rebuilds_actual_items_with_common_view_order` | 最初の favorite の採用前に実 items の順序を検査していた。初期読込を完了 helper へ変更。無効化／再有効化後の実 rebuild・順序の検査は維持 |
| 同 / `moving_between_favorites_and_common_context_keeps_actual_order_in_sync` | 各移動の採用前に favorite の sort を検査していた。3 回の読込を完了 helper へ変更し、各実順序の期待を維持 |
| 同 / `reset_projects_layout_fields_and_leaves_the_common_quality_alone` | 初期 favorite 未採用で Details が適用されていなかった。初期読込だけ完了を待ち、reset 後の layout・quality・世代の期待は維持 |
| 同 / `reset_rebuilds_actual_items_with_common_view_order` | 初期 favorite の走査待ちで rows が未採用。初期読込を完了 helper へ変更し、reset による実 rows の再構築は従来どおり検査 |
| `gamepad_input.rs` / `list_order_gamepad_folder_accept_replaces_stale_snapshot_before_navigation` | Folder の carrier を旧 `Direct` として検査していた。同じ path を持つ `GridFolder` を検査し、凍結順の置換と採用前の source の期待は維持 |
| `grid_parent.rs` / `section1473_router_tag_children_use_owner_and_zip_moves_only_one_level` | tag の Folder Back の採用待ち不足。実 post-render を通す完了 helper を追加し、戻り先と stack の期待を維持 |
| `list_order_everything_tests.rs` / `list_order_everything_parent_uses_the_moved_row_and_nested_book_returns_to_its_folder_first` | inner ZIP→実フォルダの Back が未採用。完了 helper を追加し、元 row／drill の親／次の帰路の期待を維持 |
| `menubar_search_jump_tests.rs` / `menubar_plain_books_root_opens_during_render_without_deferred_candidate` | render 中の受付を同期採用として検査していた。render 中に scan が始まることと、同じ完了経路での採用を検査。最終行き先は維持。実態に合わせ `menubar_plain_books_root_starts_during_render_and_adopts_without_deferred_candidate` へ改名 |
| `multiwindow_scenario_tests.rs` / `multiwindow_scenario_a_folder_sidecar_restore_reaches_paint` | main 以外の同期 load に完了 helper を付け、検査対象の復元まで消費していた。元の `load_folder` に戻し、実 restore→paint の検査を維持 |
| `release_checks.rs` / `release_pin_refresh_must_finish_before_a_following_open`（test-script のみ） | pin dirty の消費が D5 の Physical preflight になり、まだ復元が始まっていなかった。preflight のみ採用し、新 sidecar を残して後続 open の拒否を検査。許可後の open は完了を待つ |
| `smart_folder.rs` / `epub_smart_modal_blocks_superseding_open_until_cancel` | modal 取消後の next folder の採用待ち不足。読込の完了を待ち、modal 中の拒否と取消後の行き先の期待は維持 |
| `snapshot_ops.rs` / `browser870_snapshot_child_pdf_round_trip_restores_directory_source` | snapshot が初期 Directory の採用前の Other source を捕捉していた。初期読込を完了 helper へ変更し、元 Directory source／ページ構成の復元は維持 |
| `startup_ops.rs` / `browser818_open_owner_initial_activation_and_agent_keep_intent_through_restore` | 手動 reply が path 解決だけで、実 worker が付ける Directory scan を欠いていた。実 worker と同じ走査処理で reply を構築し、復元開始時点の request seed を維持 |
| 同 / `direct_media_activation_can_supersede_other_folder_and_fixed_snapshot_keeps_lock` | 同じ模擬 reply の欠落で余分な走査待ちになり、Loaded 後の明示 media 継続へ届かなかった。worker 相当の reply に補正し、置換／固定の期待は維持 |
| 同 / `excluded_external_media_startup_and_activation_keep_projected_parent_and_siblings` | 同じ欠落で明示 media の一時的な表示継続が未実行。worker 相当の reply に補正し、除外対象の直接閲覧と兄弟順の期待は維持 |
| 同 / `section1419_activation_and_bookmark_retire_earlier_main_requests` | B の模擬 reply が新しい走査を発行し、それを A の遅延完了として検査していた。B に実 worker の走査結果を付ける。A の退役と最終 owner の期待は維持 |
| 同 / `section1419_snapshot_bookmark_classification_continuation_is_held_until_admission` | B と再開 A の模擬 resolver 完了が同じ走査結果を欠いていた。main のみ実 worker 相当にし、detached の非走査／保留と採用順の期待は維持 |
| `tests/close_nav_801.rs` / `close_nav_801_ai_queue_held_does_not_block_pdf_return_handler` | C5 の親 scan 採用前に親を検査していた。完了を待ち、mutex 保持中に handler／採用が完了する検査と最終親を維持 |
| 同 / `close_nav_801_inference_sessions_held_does_not_block_pdf_return_handler` | 同じ待ち不足。実完了 seam を通し、AI sessions の lock を取らない検査を維持。同期 Loaded を表していた受付の診断期待だけ Classifying に変更 |
| 同 / `close_nav_801_remote_state_held_blocks_pdf_return_handler_at_named_lock` | 同じ親 scan 採用待ち不足。完了を待ち、既存 Remote lock の観測と最終親を維持 |
| `tests.rs` / `direct_page_parent_return_preserves_parent_scroll_history` | C5 の親 scan 未採用。完了 helper を追加し、親・選択・scroll 240・reveal の期待は維持 |
| 同 / `embedded_container_close_request_allows_parent_return_to_be_consumed` | 同じ未採用。既存 dispatcher 後に完了を待ち、親と fullscreen 終了／要求の消費の期待は維持 |
| 同 / `file_organize_destinations_direct_handler_adopts_folder_and_history` | Direct 受付直後の同期採用を仮定。完了 helper を追加し、行き先と history の最終期待を維持 |
| 同 / `history_transition_storage_keeps_app_stack_footprint_bounded` | 製品側の storage 退行。CloseReturn の描画 payload が navigation／owner に inline で伝播し App が 114,448 bytes になっていた。描画 payload を request ID と同じ所有者内の Box に移す。110,000 の上限を維持し、presentation 自体のサイズ上限も追加 |
| 同 / `mixed_path_history_replay_accepts_old_spelling_and_restores_the_same_folder` | Direct の採用前で back stack が空だった。完了を待ってから旧 spelling を設定し、正規化された復元先／stack の期待は維持 |
| 同 / `subfolder_expansion_button_exits_immediately_when_view_is_on` | D3 の終了を同期採用として検査。完了を待ち、合成一覧の終了／dialog の最終期待は維持 |
| 同 / `section1705_local_filter_and_empty_bar_allow_history_keys_and_dispatch` | 完了 helper が検査対象の初期 sidecar まで消費していた。走査採用だけを待ち、実 modal→復元終了→history dispatch の期待を維持 |
| `tests/scroll_661.rs` / `row_anchor_resume_keeps_middle_selected_row_in_both_renderers` | resume の helper が sidecar を先に完了していた。走査採用だけを待ち、実 hydration の選択優先処理と両 renderer の row anchor の期待を維持 |
| `global_search_ui.rs` / `caption_drill_branch_identity_lives_with_installed_rows` | Folder の carrier が旧 Direct という前提。同じ branch path の GridFolder を検査し、検索終了後も行が開ける契約を維持 |
| `test_script.rs` / `rating_folder_back_key_pointer_reveals_and_double_clicks_actual_large_grid_without_focus`（test-script のみ） | 同じ carrier の前提。選択済み key と同じ path の GridFolder を検査し、実 grid の選択／描画／pointer の期待は維持 |
| `ui_main.rs` / `section1804_stack_pending_background_click_keeps_explicit_deselection` | 初期 folder の未採用で stack script の開始に届かなかった。走査採用後に既存の復元を完了し、pending 中の明示 deselection の期待は維持 |

共通 helper `finish_direct_navigation_scan_before_hydration_for_test` は既存の post-render／fullscreen seam で
走査採用までを待ち、採用が始めた sidecar は消費しない。既存の full helper はこれに復元待ちを加える。
startup の新しい reply fixture は実 worker の `scan_startup_resolved_target` を呼び、main 以外は走査しない。
意図して scan の無い応答を扱う既存 §1.285 テストは維持する。

missing path の即時 bool を戻すための同期 metadata/read_dir は追加しない。§6.3(1) は未走査 main で
`Classifying` を返す設計で、bool wrapper も元から Loaded／Classifying を受付成功とする。
`saved_group_actions`・`collection_navigation`・`book_append_refresh` の成功後処理は走査済みの引数、
変換書庫の成功後処理は別の ZIP/cache 経路と行き先照合、snapshot の実フォルダ fullscreen は上流の
Required worker に属することを点検した。main 未走査の true を同期 install 成功として後処理する
製品 caller は見つからなかった。wrapper に「受付結果であり採用完了を保証しない」と明記した。

既存の最終一覧・選択・履歴・表示の期待値と App サイズ上限は変更していない。
変更した中間期待は、未走査 main の即時失敗→非同期失敗、Folder の Direct→GridFolder carrier、
同期受付の Loaded 診断→Classifying、および上表の採用／復元待ちである。

利用者から指定された P3 の補強（§6.12 d5／d6）も追加した:

- `src/app/native_video.rs` / `native_unblocked_home_end_and_hud_choose_real_targets_from_middle_item`:
  3 項目の中央から Home／End と HUD 前後を実 handler に渡し、要求する移動と採用先・選択を検査する。
  端の項目は実在の画像にし、decoder／native presenter／HWND を起動せずに通常の着地まで進める。
- `src/ui_fullscreen/close_return_tests.rs` /
  `close_return_wait_renders_live_zoom_pan_and_rotation_while_rejecting_page_targets`:
  CloseReturn 中に通常 fullscreen を headless 描画し、現在ページの texture mesh の頂点と paint transform の一致、
  zoom・pan・rotation を順に変えた実描画の変化を検査する。待ちの lock が残り、ページ送り／jump は拒否されることも検査する。
- 同ファイルの既存 `close_return_captures_visible_previous_before_true_close_cleanup` は、捕捉を clone して元を drop した後も
  同じ request ID と描画資源で sequence を始められる検査を追加し、Box 化後の所有を補強した。

新しい返却事項はない。既存のお気に入り検索の重複 stack の判断待ちは、従来結果を維持したまま継続する。
修正後のコンパイル・両構成のテスト成功、および App の実サイズは runner での再確認が必要。
変更した Rust 22 ファイルは単体ごとの `rustfmt --edition 2024 --check --config skip_children=true` に成功し、
`git diff --check` も成功した。これらはコンパイル・テストの成功証拠ではない。

設計担当へ渡す r2 focused filter（実装担当では未実行）:

```powershell
$scan853R2Filter = 'test(browser818_) | test(saved_reveal_preserves_a_staged_classification_without_reserving_navigation) | test(failed_independent_) | test(favorite_view_state_tests) | test(list_order_gamepad_) | test(section1473_router_tag_children_) | test(list_order_everything_parent_) | test(menubar_plain_books_root_) | test(multiwindow_scenario_a_) | test(release_pin_refresh_) | test(epub_smart_modal_) | test(browser870_snapshot_child_) | test(direct_media_activation_) | test(excluded_external_media_) | test(section1419_) | test(close_nav_801_) | test(favorite_adjustment_defaults_tests) | test(file_organize_destinations_) | test(history_transition_storage_) | test(mixed_path_history_) | test(subfolder_expansion_button_exits_) | test(section1705_local_filter_) | test(row_anchor_resume_) | test(caption_drill_branch_) | test(rating_folder_back_key_pointer_) | test(section1804_stack_pending_) | test(native_) | test(close_return_)'
cargo nextest run -p mimageviewer --lib -E $scan853R2Filter
cargo nextest run -p mimageviewer --lib --features test-script -E $scan853R2Filter
# focused と従前の関連 gate の後、同じ source で全件を両構成とも確認する。
.\scripts\test-lib-full.ps1
.\scripts\test-lib-full.ps1 -Features test-script
```

#### テスト実行後の修正 (r3)

2026-11-27、設計担当が `de9b6a308`（r2＋cargo fmt）で lib 全件を再実行した。
既定は 20,864 件中 20,861 成功・3 失敗、test-script は 21,026 件中 21,022 成功・4 失敗。
最新 2 run（`20261126T170315449Z-ed5b5df1`、`20261126T173114087Z-480a17be`）の診断を読んだ。
`stdout.log` は空で、panic とサイズの実測値は `stderr.log` にある。
実装担当自身はコンパイル・cargo／nextest・テスト・ビルド・製品起動・コミットを行っていない。

| 失敗テスト | 根因・修正 |
| --- | --- |
| `history_transition_storage_keeps_app_stack_footprint_bounded` | r2 後も App は既定 110,448 bytes／test-script 110,464 bytes。CloseReturn 内側の Box 化だけでは、navigation ごとの search／close／選択と grid effects の増分が残った。`MainListNavigationPayload` にこの 3 値をまとめ、既存の inline 選択ヒントも含め `Option<Box<…>>` へ移す。Grid の履歴帰路と受付時の★解除決定もそれぞれ `Option<Box<…>>` にする。空の navigation payload は持たず、同じ要求が全効果を所有する。App の 110,000 上限は維持し、各 field の pointer-size と navigation／effects の型別上限も追加 |
| `browser818_presentation_activation_restore_materialized_keeps_request_seed` | r2 fixture の `folder_scan=None` では loader が Classifying を返すため、startup の Loaded 限定の明示ファイル継続に入らず、sidecar の viewer request が無い。実 resolver は main Directory に走査結果と policy を付けてから返すため、この fixture を実 worker の応答の受信・確認・再送へ修正。seed／窓／選択の最終期待は維持 |
| `browser870_snapshot_child_pdf_round_trip_restores_directory_source` | 最後の `load_folder(folder)` だけ採用待ちがなく、直前に操作した PDF の Ltr のまま検査していた。それ以前の Directory source 復帰・親 DB 未保存・PDF DB の Ltr 保存はログ上通過。最終読込の pending と採用前 Ltr を確認し、実完了 helper 後に元の Single を検査する。最終期待は変更しない |
| `release_pin_refresh_must_finish_before_a_following_open`（test-script のみ） | 製品の受付順の退行。既存 `prepare_visible_install` の `sidecar_restore_active() || book_append_undo_in_progress()` が worker 分岐より後に残り、先に Classifying を返していた。この既存の在メモリの拒否条件を共通 helper にし、未走査 main Directory の分岐で予約取得・旧要求退役・worker 起動より前にも適用。可視 install 時の再確認は維持。即時 false の期待は変更せず、worker を発行していない検査も追加 |

最後の拒否は scan の I/O エラーや走査した rows の内容によるものではなく、既存の可視採用の busy 条件である。
最新指示の「走査だけを遅らせ、受付拒否は遅らせない」に従い、その条件を受付側でも評価する。
document modal 全体や main 以外へ対象を広げず、受付後に復元が始まった場合の既存 poll の保留は変えない。
両構成でこの境界を検査する `direct_navigation_worker_busy_install_refuses_before_scan_admission` を追加した。
新たな返却事項はない（従前のお気に入り検索の重複 stack の判断待ちは継続）。

64-bit Windows のサイズ影響（修正前 App は設計担当ログの実測、下記の型／field は layout からの見積もりで未実測）:

| 移動する field | r2 の inline 見積もり | r3 の inline | 影響 |
| --- | ---: | ---: | --- |
| navigation の選択＋CloseReturn＋検索 admission | 32＋16＋48＝96 bytes | payload pointer 8 bytes | navigation 1 値あたり約 88 bytes 減。型全体は約 768→680 bytes、§6 前の約 704 bytes より小さい見込み |
| grid の `rating_suppression` | 32 bytes | 8 bytes | effects 1 値あたり約 24 bytes 減 |
| grid の `reading_history_return_from` | 32 bytes | 8 bytes | effects 1 値あたり約 24 bytes 減。上と合わせ effects は約 72→24 bytes（§6 前は約 40 bytes） |

App 所有 pending へ埋め込まれた複数の値にこの削減が効く。App 全体の削減量と上限通過は再実行で測る。
既存サイズテストへ実 field の 1 pointer 検査、`MainListNavigation <= 704`、`GridVirtualOpenEffects <= 40`、
診断用の各型・payload サイズ出力を追加した。閾値を引き上げる変更はしていない。
変更した Rust 13 ファイルの単体 `rustfmt --edition 2024 --check --config skip_children=true` と
`git diff --check` は成功。これはコンパイル・テストや修正後サイズの測定ではない。

変更ファイルと設計項目:

- `app.rs`・`close_return.rs`・`startup_ops.rs`・`update_phases.rs`: §6.3(2・3・8・9)／§6.12 の要求所有を維持した storage。
  busy admission の共通化と worker より前の拒否も `app.rs`。
- `collection_grid.rs`: §6.3(8) の idx 判定を再評価せず、捕捉時に Box へ包み採用で元の値を消費。
- `browser818_presentation_tests.rs`・`snapshot_ops.rs`・`release_checks.rs`: 上表 3 テストの修正。
- `tests.rs`: サイズ検査の補強と `main_container_open_via_gamepad_defers_reading_history_return_until_adoption` の値比較の型追従。
- `tests/direct_navigation_worker_scan.rs`: 新規 busy admission 回帰と既存の captured effects／選択比較の型追従。
- `list_order_ratingnav649_tests.rs`・`tests/search_scan_853_tests.rs`・`tests/ui_scan_853_tests.rs`:
  Box の中身／navigation accessor を通して、従来の値をそのまま比較する。最終期待は変更しない。

設計担当へ渡す r3 filter（未実行）:

```powershell
$scan853R3Filter = 'test(history_transition_storage_keeps_app_stack_footprint_bounded) | test(browser818_presentation_activation_restore_materialized_keeps_request_seed) | test(browser870_snapshot_child_pdf_round_trip_restores_directory_source) | test(release_pin_refresh_must_finish_before_a_following_open) | test(direct_navigation_worker_) | test(grid_folder_) | test(ratingnav649_) | test(search_scan_853_tests) | test(ui_scan_853_tests) | test(close_return_) | test(section1335_) | test(section1853_)'
cargo nextest run -p mimageviewer --lib -E $scan853R3Filter
cargo nextest run -p mimageviewer --lib --features test-script -E $scan853R3Filter
# 関連 gate の後の全件確認も従来どおり必要。
.\scripts\test-lib-full.ps1
.\scripts\test-lib-full.ps1 -Features test-script
```

<!-- scan853-test-inventory -->

#### `src/app/native_video.rs`

```text
native_home_end_and_hud_targeting_share_folder_wait_admission
native_wait_gate_keeps_unblocked_inputs_and_swap_delta_coalescing
native_unblocked_home_end_and_hud_choose_real_targets_from_middle_item
```

#### `src/app/book_composition_permission_tests.rs`

```text
resume_notice_follows_materialized_open_into_its_detached_context
```

#### `src/app/book_preferred_rating_tests.rs`

```text
book_preferred_loaded_extension_named_folders_do_not_write_or_record_undo
```

#### `src/app/book_resume_meter_tests.rs`

```text
book_resume_meter_copied_data_dir_uses_existing_cache_save_restore_key
book_resume_meter_deleted_cache_reconversion_and_source_change_resume_same_key
```

#### `src/app/browser818_activation_tests.rs`

```text
exercise_activation_with_history
```

#### `src/app/browser818_presentation_tests.rs`

```text
browser870_browser818_linked_and_multiple_open_single_with_context_owned_saving
```

#### `src/app/close_return_tests.rs`

```text
close_return_real_fullfeature_media_is_shared_until_promote_or_live_fork
enter_source
```

#### `src/app/collection_grid.rs`

```text
assert_collection_descendant_open_round_trip
assert_collection_native_outer_complete_temporary_origin
collection_child_backspace_back_forward_round_trip_uses_keyboard_history
collection_grid_error
direct_navigation_worker_preserves_collection_physical_owner
direct_navigation_worker_rejects_collection_owner_staled_after_admission
g1_collection_real_handler_reopen_preserves_vertical_flow_and_book_return
grid_collection_folder_carries_owner_without_applying_effects
grid_folder_enter_and_gamepad_defer_visible_effects_until_adoption
grid_folder_rating_suppression_uses_admitted_session_value_at_commit
poll_real_history_load
rating_child_backspace_back_forward_round_trip_uses_keyboard_history
real_cached_epub_direct_warm_placeholder_commits_before_worker_result
real_epub_ab_switch_changes_slot_only_after_prepared_adoption
real_epub_ordinary_back_and_forward_replay_adopts_one_outer_point
```

#### `src/app/collection_navigation.rs`

```text
ready_collection_child_preflight_waits_for_listing_identity_or_surface_switch
ready_direct_classifier_waits_for_listing_identity_or_surface_switch
```

#### `src/app/drive_jump_tests.rs`

```text
drive_jump_classified_directories_keep_worker_scan_and_original_navigation
```

#### `src/app/folder_pane_reclick_stage2_tests.rs`

```text
reclick_stage2_aggregate_fixture
reclick_stage2_fixture
```

#### `src/app/folder_pane_reclick_tests.rs`

```text
folder_pane_reclick_does_not_replace_pending_history_navigation
folder_pane_reclick_same_pass_ready_order_refresh_is_replaced
reclick_fixture
reclick_pending_pane_scenario
reclick_single_scenario
```

#### `src/app/folder_pane_refresh_tests.rs`

```text
folder_pane_label_req40_completed_navigation_collapses_prior_ancestors
folder_pane_reported_delete_is_reflected_after_main_list_acceptance_without_tree_input
frame_end_tree_toggle_scenario
label_double_click_scenario
navigation_tree_fixture
scrollable_tree_click_fixture
```

#### `src/app/gamepad_input.rs`

```text
epub_modal_blocks_ring_and_keyboard_history_dispatch
```

#### `src/app/list_order_everything_tests.rs`

```text
commit_row
finish_landing
landing_fixture
list_order_everything_admission_requires_current_outer_run_and_accepted_dataset_run
list_order_everything_internal_open_physical_book_reports_unavailable_from_real_key_handler
retained_parent_internal_open_repro
```

#### `src/app/list_order_ratingnav649_tests.rs`

```text
folder_enter_auto_open
ratingnav649_enter_and_pointer_folder_receipt_preserve_history_return
ratingnav649_gamepad_extension_named_folder_keeps_existing_receipt
ratingnav649_smart_pointer_consumed_open_does_not_fall_through_to_secondary_click
scenario
settle
```

#### `src/app/list_order_tests.rs`

```text
list_order_cancelled_pdf_then_existing_folder_open_still_resolves_frozen_order
list_order_capture_uses_folder_listing_name_and_date_sorts_both_ways
```

#### `src/app/list_order_virtual_tests.rs`

```text
configure_fixture
finish_reopen
```

#### `src/app/location_jump.rs`

```text
location_and_address_plain_physical_dispatch_scan_off_caller_and_adopt_once
location_jump_everything_physical_child_exits_to_drive_list_with_pre_search_history
```

#### `src/app/menubar_search_jump_tests.rs`

```text
menubar_plain_favorite_uses_existing_direct_navigation_and_history
```

#### `src/app/multiwindow_scenario_tests.rs`

```text
multiwindow_rating_folder_reload_keeps_sibling_order_and_generation
rating_publication_reaches_main_and_two_detached_without_reordering_siblings
run_scenario_a
```

#### `src/app/native_backdrop_navigation_tests.rs`

```text
native_backdrop_plain_favorite_and_direct_keep_immediate_loader_cleanup
```

#### `src/app/rar_archive_cache_tests.rs`

```text
load_settled_folder
rar_archive_cache_completed_probe_cancelled_before_adoption_keeps_new_folder
```

#### `src/app/search_nav_book_tests.rs`

```text
search_nav_book_retained_result_child_records_saved_origin_route_and_real_back_restores_it
settle_target
```

#### `src/app/search_nav_boundary_tests.rs`

```text
fixture
poll
settle
```

#### `src/app/search_nav_entry_inventory_tests.rs`

```text
physical_parent_case
```

#### `src/app/section375_c1_tests.rs`

```text
section1375_c1_promoted_physical_reader_ignores_actual_change_and_late_restart
```

#### `src/app/section375_tests.rs`

```text
fixture
section1375_automatic_origin_survives_match_diff_restart_resume_and_defer
section1375_delete_during_initial_stack_extraction_or_grouping_resumes_only_after_success
section1375_delete_hidden_candidate_and_no_visible_idx_still_reconcile
section1375_delete_mixed_directory_mtime_observes_without_display_sort_stamp_change
section1375_direct_reader_both_forks_rebind_only_saved_physical_basis
section1375_favorite_effective_projection_key_matches_worker_and_first_equal
section1375_physical_folder_zip_pdf_epub_video_audio_delete_equal
section1375_same_second_file_overwrite_is_observed_by_actual_worker
section1375_search_root_delete_does_not_dispatch_physical_folder_reload
section1375_stack_flat_and_retained_aggregate_rebind_physical_record_for_equal
section1375_stack_flat_delete_rebuilds_retained_singleton_metadata_and_maps
section1375_stack_late_hydration_keeps_automatic_import_policy_and_group_cursor_owner
section1375_stack_member_changes_and_equal_facet_details_keep_projection
section1375_stack_retained_read_failure_keeps_flat_until_ordinary_reload
section1375_stack_self_delete_equal_opens_remaining_stack_after_image_or_folder
section1375_stack_zero_success_keeps_view_and_pending_switch
section1375_tag_and_edit_sidecar_policy_off_on_explicit_and_open
```

#### `src/app/sidecar_restore.rs`

```text
visibility_detached_folder_auto_first_display_completes_or_terminates_its_owner
```

#### `src/app/similar_navigation_tests.rs`

```text
control_required_scan
controlled_scan_channel
detached_required_scan_failure_without_a_page_exits_the_password_wait_owner
disconnected_required_scan_uses_the_same_terminal_failure_boundary
embedded_similar_move_update_pump_does_not_consume_a_passive_context_scan
embedded_similar_move_update_pump_leaves_non_required_scan_for_normal_tail
image_scan
send
```

#### `src/app/smart_folder.rs`

```text
section1853_cancelled_smart_origin_fallback_keeps_visible_owner_until_install
section1853_deleted_smart_return_wait_keeps_owner_and_rejects_deleted_definition
```

#### `src/app/startup_default_tests.rs`

```text
startup_default_and_explicit_global_search_entry_retire_pending_main_open
startup_default_late_tree_favorite_and_dfs_requests_retire_the_old_default
startup_default_remote_return_preserves_an_already_adopted_local_folder
startup_default_same_folder_keeps_existing_history_without_an_extra_departure
```

#### `src/app/startup_ops.rs`

```text
section1285_deferred_modal_target_is_replaced_by_explicit_local_navigation
section1285_reload_keeps_unresolved_initial_request_without_user_navigation
section1285_remote_retains_deferred_target_through_operations_and_local_reload
section1285_same_location_explicit_open_replaces_deferred_target_but_reload_keeps_it
```

#### `src/app/startup_scan_policy_tests.rs`

```text
fixture
```

#### `src/app/startup_scan_tests.rs`

```text
fixture
```

#### `src/app/subfolder_expansion.rs`

```text
g1_subfolder_real_handler_reopen_restores_legacy_flow_and_book_return
section1853_subfolder_exit_and_toggle_keep_visible_state_until_return_install
```

#### `src/app/tag_view_list_order_tests.rs`

```text
tag_list_order_adoption_replaces_only_matching_run_and_row
tag_list_order_grid_request_supersedes_direct_navigation_scan
```

#### `src/app/tag_view_open_supersession_tests.rs`

```text
fixture
tag_view_open_supersession_sibling_edge_keeps_real_current_view_order_refresh
tag_view_open_supersession_tag_accepts_and_retires_direct_navigation_scan
```

#### `src/app/tests.rs`

```text
a_ring_sort_order_reloads_the_window_the_ring_was_opened_in
accepted_epub_retires_pending_dfs_after_classification_but_ignore_keeps_it
archive_suffix_directories_open_as_folders_under_ignore
auto_fullscreen_image_folder_ignores_mixed_folder_after_load
auto_fullscreen_image_only_folder_opens_page_after_load
auto_fullscreen_image_only_folder_preserves_grid_intent_for_always_new_detached
bookmark_direct_open_retires_pending_history_preflight
detached_pdf_password_owner_blocks_new_main_open_without_changing_sibling
direct_epub_conversion_resumes_deferred_surface_adoption
direct_epub_modal_or_cold_enumeration_preserves_source_through_history_request
direct_pdf_password_continuation_keeps_source_until_retry_or_cancel
direct_pdf_rows_and_surface_commit_with_enumeration_result
ensure_final_composite_never_leaves_stuck_incomplete_entry
epub_batch_completion_refreshes_grid_and_applies_same_name_pdf_priority
failed_rating_read_opens_and_reloads_with_name_order_and_ordinary_prewarm
file_organize_destinations_direct_handler_adopts_folder_and_history
folder_history_back_and_forward_restore_reading_history_without_folder_load
folder_history_back_restores_preserved_subfolder_expansion_snapshot
folder_to_drive_list_records_back_target_and_restores_both_directions
idle_upgrade_recheck_keeps_waking_inside_margin_window
ignored_archive_file_does_not_retire_pending_direct_pdf_owner
installed_prepared_cache_replays_a_write_after_read_even_if_seen_advanced
later_rating_chunk_failure_installs_the_standard_order_cache_and_filter
leaving_smart_folder_session_discards_result_and_synthetic_return_rescans
letterbox_real_grid_folder_icon_touch_cancel_and_double_tap
letterbox_real_grid_folder_icons_own_inner_area_with_setting_on_and_off
listed_epub_directory_opens_as_folder_under_ignore
main_folder_candidate_fallback_updates_reading_history_at_adoption
main_window_title_hides_synthetic_path_during_search_close_and_restores_adopted_path
mixed_path_history_replay_accepts_old_spelling_and_restores_the_same_folder
mutation_refresh_rating_child_and_backing_alias_keep_parent_chain_and_history
navigation_cancels_archive_bookmark_scan_and_drops_late_result
navigation_cancels_book_request_after_resolver_completion
nested_smart_convertible_history_keeps_logical_kind_and_cache_alias
nested_smart_pdf_history_reopens_as_pdf_after_source_session_is_gone
newer_open_classification_cancels_old_result_without_adopting_it
no_resident_smart_pdf_history_back_rebuilds_root_then_restores_container
no_resident_smart_pdf_history_cancel_restores_prior_folder_and_pop_snapshot
no_resident_smart_pdf_missing_root_row_rolls_back_history
no_resident_smart_pdf_password_cancel_rolls_back_history_after_root_prepare
normal_navigation_cancels_normal_archive_scan
normal_navigation_cancels_slow_bookmark_resolve_and_ignores_late_result
offscreen_parked_root_delete_is_not_resurrected_by_history_back
park_terminates_classification_history_pdf_and_zip_without_adopting
pdf_password_prompt_blocks_new_direct_open_but_its_retry_proceeds
phase_b_reading_history_opened_folder_resolves_masked_member_before_paint
physical_rating_reload_restores_the_exact_selected_path
poll_folder_pane_open_returns_ready_without_loading
quick_folder_none_startup_pdf_password_cancel_exit_reload_preserves_ownership
realmachine50_foreign_zip_offer_transfers_main_origin_and_cancel_does_not_handoff
same_folder_reload_preserves_history_with_archive_dialog
same_name_test_folder_reports_its_folded_and_unsupported_entries
section1335_direct_zip_exit_restart_keeps_explicit_parent_list
section1339_address_bar_zip_enter_keeps_copied_path_across_row_refresh
section1339_cold_pdf_success_retires_ready_retained_warm_verification_once
section1339_direct_password_retry_keeps_same_navigation_until_single_adoption
section1339_direct_warm_pdf_adopts_facet_and_history_before_verification_once
section1339_foreign_zip_conversion_cancel_and_cached_hydration_keep_same_location
section1339_main_pane_admission_retires_classification_even_if_pane_is_cancelled
section1339_main_pane_admission_retires_rating_navigation_even_if_pane_is_cancelled
section1339_new_rating_admission_retires_old_classification_before_new_request_cancel
section1339_new_smart_admission_retires_each_old_typed_request_before_smart_cancel
section1339_parent_failure_and_supersession_do_not_leak_child_selection_hint
section1339_plain_main_converted_archive_preserves_source_until_owned_cached_zip_adoption
section1339_plain_zip_grid_handler_adopts_only_usable_rows_and_reload_keeps_manual_restore
section1339_quick_native_proof_survives_rows_but_rejects_switch_epoch
section1339_rating_zip_backspace_back_forward_restashes_live_parent
section1339_reported_rating_zip_fullscreen_bs_toolbar_back_forward
section1339_smart_surface_survives_real_pdf_verification_but_row_request_expires
section1339_successful_parent_backspace_selects_the_exact_departed_child
section1339_zip_nested_level_history_reentry_uses_the_prepared_fresh_root_route
section1339_zip_same_location_reload_reconciles_prepared_prefix_and_manual_restore
section1353_disappeared_level_uses_parent_facet_and_new_backspace_stack
section1353_disappeared_level_with_root_collapse_restores_actual_parent_facet
section1353_grid_reload_key_preserves_zip_level_cursor_facet_and_history
section1353_refresh_cancel_and_stale_results_keep_the_current_location
section1353_staged_folder_refresh_reexecutes_submitted_search_on_new_rows
section1705_alt_history_from_focused_empty_local_input_moves_and_retires_focus
section1705_local_filter_and_empty_bar_allow_history_keys_and_dispatch
smart_convertible_zip_late_result_cannot_replace_a_new_folder
smart_folder_drill_parent_search_snapshot_and_history_round_trip_keep_scope
smart_fullscreen_folder_nav_carries_continuation_into_nested_zip_and_cancel
snapshot_out_of_scope_load_is_refused_before_main_state_effects
snapshot_refusal_before_both_claim_sites_preserves_running_archive_open_owner
snapshot_refusal_preserves_conflicting_bookmark_open_owner
snapshot_refusal_preserves_unresolved_startup_open_owner
staged_and_direct_epub_modals_block_new_opens_on_same_path
staged_epub_ask_stale_error_and_cancel_keep_old_location
staged_epub_ignore_retires_only_request_and_convert_policy_has_typed_child
staged_epub_publication_resumes_same_request_and_adopts_once
staged_epub_save_pdf_is_explicit_destination_without_popping_epub_replay
staged_smart_folder_superseded_child_keeps_new_folder_navigation
staged_smart_history_root_commits_once_and_does_not_suppress_next_navigation
successful_rating_load_installs_complete_zero_cache_without_ui_prewarm
test_script_counts_delegated_smart_start_once_through_adoption
test_script_counts_delegated_zip_start_once_through_adoption
test_script_counts_direct_archive_owner_without_recounting_cache_hydration
test_script_counts_history_back_once_through_adoption
test_script_counts_superseded_refresh_starts_before_completion
test_script_counts_the_actual_refresh_folder_request
test_script_does_not_count_zip_foreign_conversion_continuation
tree_sort_change_cancellation_rejects_old_order_result_but_keeps_exact_pane_open
visible_thumb_adjustment_4k_175pct_timing
warm_pdf_direct_open_absent_from_current_rows_adopts_placeholders
warm_pdf_direct_open_commits_placeholder_and_history_failure_keeps_source
warm_pdf_navigation_absent_from_rows_adopts_before_worker
warm_pdf_verification_keeps_placeholder_thumbnail_workers_or_rebuilds_them
warm_pdf_verifier_survives_failed_or_cancelled_cold_direct_candidate
```

#### `src/app/tests/archive_inner_book_s1.rs`

```text
section1675_snapshot_zipdir_keeps_explicit_list_navigation
section1676_collection_live_and_terminal_close_preserve_owner_and_back_out
section1676_physical_history_roundtrip_does_not_restore_list_choice
section1676_subfolder_origin_keeps_existing_physical_backout_after_chosen_close
settle
settle_rating
settle_smart
```

#### `src/app/tests/bookmark_build_adoption.rs`

```text
bookmark_build_folder_adoption_retires_worker_and_preserves_audio_sidecar
bookmark_build_late_owner_proof_rejects_held_reply_after_folder_adoption
```

#### `src/app/tests/browser870.rs`

```text
browser870_directory_books_and_three_favorite_guards_do_not_remember
browser870_live_root_switch_controls_saving_before_reload_and_retains_old_rows
browser870_load_preserves_non_presentation_preferences
browser870_metadata_completion_preserves_live_presentation_and_imports_other_values
browser870_non_directory_and_replacement_do_not_inherit_directory_provenance
```

#### `src/app/tests/ctrl_down_close_1678.rs`

```text
section1678_direct_main_open_keeps_branch_list_but_wrapper_opens_page
section1678_main_and_linked_ctrl_down_end_in_branch_list
```

#### `src/app/tests/direct_navigation_worker_scan.rs`

```text
capture_destination_existing_opens_like_pane_navigation
capture_destination_missing_warns_without_replacing_current_listing
capture_destination_other_scan_errors_keep_generic_warning
direct_navigation_worker_admission_after_real_dfs_poll_retires_sequence
direct_navigation_worker_admission_takes_and_retires_global_fullscreen_reservation
direct_navigation_worker_dfs_admission_cancels_only_replaceable_scan
direct_navigation_worker_dispatch_has_zero_caller_scans_and_adopts_once
direct_navigation_worker_history_navigation_replaces_pending_scan
direct_navigation_worker_history_wait_keeps_released_escape_stack_toggle
direct_navigation_worker_original_navigation_is_not_recaptured
direct_navigation_worker_pane_retirement_does_not_retain_older_direct_scan
direct_navigation_worker_preserves_unmarked_presentation_intents
direct_navigation_worker_busy_install_refuses_before_scan_admission
direct_navigation_worker_quick_folder_round_trip_retires_old_epoch
direct_navigation_worker_ready_scan_yields_to_same_frame_navigation_inputs
direct_navigation_worker_rejected_address_input_cancels_prior_scan_in_actual_frame
direct_navigation_worker_rejected_filtered_backspace_cancels_prior_scan
direct_navigation_worker_route_excludes_virtual_and_sibling_contexts
direct_navigation_worker_snapshot_refusal_keeps_existing_owner_and_history
direct_navigation_worker_stale_failure_and_disconnect_preserve_listing
direct_navigation_worker_successful_pdf_claim_outside_frame_retires_scan
direct_navigation_worker_uncached_pdf_acceptance_retires_scan_before_pdf_completion
direct_navigation_worker_unmarked_producers_scan_off_caller_and_keep_adoption
section1853_policy_rescan_keeps_same_continuation_token_clock_and_admission
section1853_reasoned_ui_scan_routes_keep_synchronous_adoption
section1853_worker_continuation_commits_captured_effects_and_selection_once
section1853_worker_error_and_disconnect_retire_reservation_and_selection
section1853_worker_ready_rejects_sibling_document_modal_without_adoption
```

#### `src/app/tests/everything_close_return.rs`

```text
open
```

#### `src/app/tests/facet_835.rs`

```text
image_fixture
```

#### `src/app/tests/facet_835/ranges.rs`

```text
media_fixture
```

#### `src/app/tests/file_type_visibility_fixes.rs`

```text
assert_excluded
visibility_bookmark_actor_routes_same_book_results_to_different_context_policies
visibility_bookmark_cache_reopen_uses_accepted_policy_and_restores_rows
visibility_bookmark_context_bundles_keep_concurrent_policies_completions_and_close_scoped
visibility_bookmark_mutation_invalidates_matching_bundles_and_preserves_unrelated_owner
visibility_bookmark_projection_rejects_same_context_late_container_completion
visibility_settings_ok_snapshot_reader_close_and_reopen_reload_origin
visibility_shared_archive_admission_uses_only_the_mounted_matching_source_pair
```

#### `src/app/tests/g1_extracted_handlers.rs`

```text
exercise_entrance
```

#### `src/app/tests/grid_edge_buttons_1855.rs`

```text
harness
```

#### `src/app/tests/history_parent_1473.rs`

```text
matrix
nested_rating_history_case
section1473_bookmark_entrance_direct_exit_must_not_cycle_parent
section1473_bookmark_entrance_restores_stable_key_and_scroll_then_its_parent
section1473_both_markers_truncated_equal_revisit_keeps_entrance_destination
section1473_equal_entrance_revisit_via_rating_keeps_complete_back_forward
section1473_expansion_entrance_owns_complete_restore
section1473_folder_bar_second_parent_keeps_physical_ancestors
section1473_folder_descendant_does_not_resurrect_expired_book_return
section1473_marker_presence_selects_back_or_direct_without_losing_entrance_position
section1473_other_owned_entrances_survive_history_and_book
section1473_other_synthetic_roots_parent_inventory
section1473_rating_exit_keeps_its_own_complete_origin
section1473_reading_history_root_parent_never_opens_internal_data
section1473_returned_history_back_candidate_consumes_self_marker_first
section1473_search_transfers_complete_canonical_origin
section1473_smart_and_collection_physical_entrances_keep_native_root_and_origin
section1473_snapshot_exit_cancel_keeps_visible_root_and_ends_fixed_scope
section1473_snapshot_exit_restores_complete_native_canonical_owners
section1473_snapshot_keeps_input_specific_admission_and_parent_owner
section1473_snapshot_same_path_history_has_distinct_root_and_exits_to_its_captured_visit
section1473_snapshot_subset_cannot_overwrite_ordinary_entrance_position
settle
```

#### `src/app/tests/history_parent_lifecycle_1473.rs`

```text
history_fixture
```

#### `src/app/tests/merge_adoption.rs`

```text
folder_adoption_prunes_audio_absence_on_its_new_worker_queue
reading_shell_with_audio
reading_source_reply_after_reselect_cannot_fill_new_folder
```

#### `src/app/tests/physical_parent_1672.rs`

```text
section1672_all_inputs_use_the_same_preview_and_exact_jump_selection
section1672_history_and_search_books_exit_to_physical_list_without_old_return
section1672_history_descendant_jump_does_not_resurrect_expired_return
```

#### `src/app/tests/plain_folder_spread.rs`

```text
plain_folder_spread_import_refresh_limits_opt_in_and_preserves_sibling_context
plain_folder_spread_install_uses_scan_and_does_not_persist_defaults
plain_folder_spread_requires_a_scan_fact_and_normalizes_only_the_new_default
plain_folder_spread_saved_values_and_legacy_flow_win
```

#### `src/app/tests/rating_filter_parent_return_1721.rs`

```text
new_descendant_sibling_unrelated_and_startup_entries_still_suppress
round_trip
same_starred_folder_reload_does_not_suppress_after_badge_restore
synthetic_search_and_smart_origins_allow_new_physical_entry
unrated_child_parent_action_and_same_folder_reload_keep_explicit_filter
zip_book_reload
```

#### `src/app/tests/rating_filter_return_1671.rs`

```text
same_backing_reload_and_switch_use_the_adopted_source
scenario
settle
unadopted_canceled_and_failed_pdf_leave_the_converted_book_unchanged
unadopted_pdf_password_terminal
```

#### `src/app/tests/same_position_double_click.rs`

```text
check_same_position
settle
```

#### `src/app/tests/scroll_661.rs`

```text
adopt_input_destination_before_first_layout
row_anchor_after_list_adoption
```

#### `src/app/tests/search_scan_853_tests.rs`

```text
section1853_conversion_phase_uses_captured_search_back_instead_of_new_descend
section1853_favorite_back_defers_effects_and_preserves_released_duplicate_parent
section1853_favorite_sibling_defers_replace_and_preserves_released_duplicate_root
section1853_favorite_sibling_keeps_suffix_directory_classification_and_single_count
section1853_search_back_classifies_suffix_directories_before_virtual_dispatch
section1853_search_back_failure_or_replacement_keeps_old_stack_and_address
section1853_search_back_immediate_refusal_restores_hint_without_stack_or_address_change
section1853_search_back_keeps_ignored_file_classification_without_stack_effects
section1853_tag_back_intent_is_owned_by_pdf_and_conversion_classification
section1853_tag_back_is_preserved_by_prepared_direct_pdf_adoption
section1853_tag_back_pops_only_at_adoption_and_next_back_requeries
section1853_tag_back_survives_classification_and_staged_zip_adoption
section1853_temporary_view_exit_shares_folder_preflight_and_keeps_exit_authority
```

#### `src/app/tests/search_thumb_1741.rs`

```text
section1741_everything_zip_generation_reuses_parent_catalog_and_folder_selection
```

#### `src/app/tests/section1328_session.rs`

```text
section1328_session_reading_deep_zip_parent_does_not_return_to_history
section1328_session_subfolder_prepare_uses_destination_sort_and_category_order_for_new_and_return
section1328_session_zip_bs_preserves_physical_and_expansion_parent_meaning
session1328_settle
```

#### `src/app/tests/section1795.rs`

```text
finish_stack_entry
open
section1795_off_bookshelf_root_starts_first_but_compiled_book_keeps_positions
section1795_off_stack_load_hints_survive_final_aggregation
section1795_off_startup_cursor_remains_independent
section1804_stack_hidden_load_hints_do_not_request_deselection
section1804_stack_internal_install_keeps_departure_recording
```

#### `src/app/tests/sort_hold_838.rs`

```text
(fixture ??module)
images
```

#### `src/app/tests/sort_hold_838/gamepad.rs`

```text
direct_replacement_blocks_item_rating
media_fixture
```

#### `src/app/tests/sort_hold_838/lifecycle.rs`

```text
direct_navigation_replacement
sort_hold_838_scan_error_releases_gate_without_replacing_rows
```

#### `src/app/tests/sort_hold_838/watch_849.rs`

```text
sort_worker_849_watch_and_refresh_resume_after_navigation_replacement
```

#### `src/app/tests/sort_hold_838/worker_849.rs`

```text
conversion_fixture
```

#### `src/app/tests/startup_restore.rs`

```text
book_env
direct_tray_book_env
direct_tray_image_folder_book_env
extra_video_close_env
open_selected_image_folder_book
section1335_bookshelf_compiled_folder_direct_read_and_explicit_list_restart_differ
section1335_collection_and_rating_folder_carriers_keep_owner_and_open_intent
section1335_collection_root_child_and_descendant_use_adopted_physical_location
section1335_collection_root_return_from_page_b_retains_departing_list_page_a
section1335_departing_same_items_page_open_captures_list_cursor_before_page_navigation
section1335_favorite_reading_sibling_fallbacks_preserve_last_accepted_physical_cursor
section1335_filtered_search_with_retained_physical_path_keeps_parent_cursor_on_back_and_tray
section1335_flat_filename_stack_acceptance_never_persists_member_name_as_aggregate_cursor
section1335_grid_folder_candidate_classifies_mixed_and_image_only_without_main_commit_leak
section1335_native_back_to_list_caller_rejects_repeat_and_accepts_explicit_press
section1335_pending_grid_candidate_setting_change_uses_completed_scan_main_intent
section1335_rating_folder_auto_request_records_the_actual_settled_physical_list
section1335_reading_empty_normal_folder_then_inner_zip_keeps_restore_at_each_seam
section1335_required_virtual_zip_exact_page_success_and_missing_page_keep_parent_record
section1335_similar_exact_page_move_preserves_accepted_departure_before_physical_materialization
section1335_single_image_startup_and_required_page_scan_do_not_adopt_parent_lists
section1335_subfolder_scan_reinstall_and_read_close_preserve_parent_cursor
section1335_synthetic_install_captures_changed_physical_departure_without_manual_capture
section1335_tray_converted_book_returns_logical_source_parent_and_preserves_reading_resume
section1335_tray_direct_book_with_unrelated_saved_parent_keeps_book_list_and_record
section1335_tray_image_folder_book_setting_changed_in_f12_returns_parent_and_keeps_resume
section1335_tray_persistence_and_restore_keep_reading_session_and_parent_list
section1335_video_ring_close_accepts_current_folder_list_for_exit_and_restart
section1795_off_image_folder_book_resumes_and_closes_to_same_list
settle_book
```

#### `src/app/tests/typing/search.rs`

```text
seed_images
```

#### `src/app/tests/ui_scan_853_tests.rs`

```text
section1853_archive_named_directory_context_jump_scans_off_caller
section1853_collection_descendant_close_keeps_physical_owner_through_parent_wait
section1853_completed_bookmark_reader_close_uses_ordinary_parent_wait
section1853_converted_cache_reload_keeps_logical_source_and_explicit_sync_route
section1853_created_sibling_zip_completion_keeps_output_selection_with_worker
section1853_delayed_terminal_close_effect_preserves_new_close_return_request
section1853_fullscreen_close_error_and_replacement_keep_reader_and_release_lock
section1853_fullscreen_close_keeps_reader_until_parent_install
section1853_grid_folder_facet_parent_child_grandchild_and_back_stay_physical
section1853_main_close_wait_does_not_block_projected_detached_context
section1853_missing_mtime_real_folder_reload_uses_staged_refresh
section1853_native_backdrop_seam_adopts_ready_fullscreen_close_return
section1853_page_list_and_focus_close_retire_late_parent_reply
section1853_parent_dialog_and_books_root_dispatch_scan_off_caller
section1853_retired_context_bundle_drops_request_owned_fullscreen_and_selection
section1853_second_fullscreen_close_retires_first_wait_and_adopts_same_parent
section1853_sidecar_restore_keeps_ready_scan_owned_until_restore_terminal
section1853_visibility_apply_retires_old_scan_before_current_list_refresh
```

#### `src/app/viewer_context_registry.rs`

```text
rating_request_reaches_park_terminal
```

#### `src/filename_stack_ui.rs`

```text
failed_rating_stack_keeps_name_order_across_saved_sort_and_rebase
g1_subfolder_stack_book_reopen_preserves_vertical_flow_and_original_book
hold_stack_grouping_completion
pending_stack_rebase_keeps_prepared_sort_across_global_sort_changes
physical_rating_request_reaches_async_stack_grouping_without_re_reading_sort_keys
rating_read_failure_notice_survives_successful_async_stack_grouping
visibility_settings_ok_retires_extracting_and_grouping_on_worker_scan_path
```

#### `src/global_search_ui.rs`

```text
g1_global_search_producer_new_open_reopen_and_return_preserve_flow_policy
global_search_label_unapplied_physical_child_keeps_metadata_breadcrumb_on_everything_switch
grid_scroll_session_search_renews_at_first_rows_and_drill_not_stream_reprojection
```

#### `src/global_search_ui/everything/tests.rs`

```text
cold_folder_video_pin_seed_cascades_and_retargets_thumbnail_owner_on_return
folder_activation_parent_chain_gamepad_return_research_and_close_keep_one_owner
section1431_drive_list_search_initializes_thumbnail_owner_and_restores_icons
section1741_prepare_owner_replaced_before_ready_retries_with_current_search_pool
section1741_search_from_folder_replaces_pool_once_for_both_producers
```

#### `src/global_search_ui/thumbnail_owner_tests.rs`

```text
section1741_metadata_prepare_owner_replaced_before_ready_retries_and_adopts
section1741_nonsearch_pool_capture_never_borrows_outgoing_resources
section1741_search_pool_adoption_leaves_sibling_context_unchanged
```

#### `src/remote_ipc/ui.rs`

```text
remote_static_page_progress_is_the_next_pc_open_position
```

#### `src/ui_dialogs/archive_convert/rar_cache_tests.rs`

```text
request_open
```

#### `src/ui_dialogs/favorites_editor.rs`

```text
favorite_cover_auto_crop_combo_saves_and_reloads_loaded_books_with_scoped_requests
favorite_cover_auto_crop_delete_active_view_starts_one_order_refresh
```

#### `src/ui_dialogs/preferences.rs`

```text
assert_grid_background
folder_cell_background_preferences_confirm_applies_saves_and_paints
ignoring_open_epub_on_preferences_ok_does_not_select_same_named_item_in_next_book
unrated_position_survives_preferences_ok_save_reopen_and_orders_folder
```

#### `src/ui_dialogs/preferences/transfer.rs`

```text
preferences_transfer_busy_background_menu_and_toolbar_pointer_handlers
```

#### `src/ui_fullscreen/close_return_tests.rs`

```text
close_return_captures_visible_previous_before_true_close_cleanup
close_return_does_not_change_ctrl_folder_navigation_draw_policy
close_return_live_render_keeps_fallback_and_input_lock_until_terminal
close_return_wait_renders_live_zoom_pan_and_rotation_while_rejecting_page_targets
close_return_page_and_jump_handlers_discard_new_targets_without_replay
close_return_terminal_only_releases_its_own_request_and_exit_resets_panel
close_return_waits_for_every_page_in_current_spread
```

#### `src/ui_fullscreen/page_turn/plain_folder_regression_tests.rs`

```text
folder_fixture
```

#### `src/ui_fullscreen/wheel_frame_tests.rs`

```text
egui037_wheel_frame_recognized_live_return_keeps_original_page_and_return_owner
```

#### `src/ui_main.rs`

```text
physical_parent_folder_menu_toggle_controls_toolbar_and_resets_to_hidden
rating_and_duration_sort_controls_dispatch_one_reload_for_selected_order
section1339_toolbar_projects_pending_cursor_and_allows_reverse_cancel
section1684_folder_bar_native_quick_slots_and_history_keep_whitespace
section1705_history_buttons_allow_local_filter_and_empty_bar
section207_production_layout_all_controls_and_positions_keep_input_usable
section207_reported_controls_keep_left_input_right_order
section207_snapshot_label_keeps_input_minimum_order_and_panel_bounds
section292_both_dropdowns_share_sort_and_facet_popup_consumes_wheel
section292_locked_facet_sort_keeps_refresh_and_subfolder_grouping_usable
```

#### `src/ui_main/toolbar_bg_menu_1876_tests.rs`

```text
section1876_row_two_pinned_tag_secondary_click_keeps_tag_action
```
