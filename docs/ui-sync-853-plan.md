# UI スレッドに残る同期処理 3 件 (§1.861・§1.859・§1.853)

状態: 設計 r3 (2026-11-24、設計レビュー r1・r2 の指摘を反映、§5.1・§5.2)。担当: 設計・検収 = ClaudeCode Opus 5.5 (ui-sync-853)、実装 = Codex GPT-6.1 Sol / high、
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
| クリック時と別のクリップボードを貼り得る | **r3: 中身の照合** (設計レビュー r1 P2-1・r2 P2-1)。常駐 thread は要求を受け取った直後 (bind より前。単一実行なので待ち行列は無く、クリックから数 ms) に、クリップボードの `CF_HDROP` のパス一覧と `Preferred DropEffect` (切り取り / コピー) を読んで控える。bind・query の後、`InvokeCommand` の直前にもう一度読み、**中身**が同じかを比べる。違えば貼らずに「クリップボードの内容が変わったため貼り付けませんでした」(ログ + 通知)。遅い貼り付け先の準備中に別の物を切り取った場合 (レビュー r2 の例) は、ここで断る。遅延レンダリングで番号が進んでも中身は同じなので誤って断らない (r1 の番号照合は採らない)。読み取りは UI スレッドでは行わない。残る幅は「クリックから受付直後の読み取りまで」と「直前の読み取りから Shell の読み取りまで」の数 ms ずつで、まれな組み合わせとして割り切る。クリック時の `IDataObject` を worker へ運ぶ案は、r2 時の apartment の問題を戻すので採らない |
| 切り取りの data object の apartment | worker は使い捨てではなく、**アプリの寿命のあいだ生きてメッセージを回す STA thread 1 本** (`shell-paste`) にする。`OleInitialize` 済みで、Shell が `OleGetClipboard` で作る包み object と、Shell の転送 thread がそれへ持つ proxy は、この thread が生きて pump している限り有効 (今の UI スレッドで呼ぶ場合と同じ条件)。mIV 自身の切り取り (`LocalCutDataObject`) は UI スレッドの apartment にあり、Shell からの `SetData` (Paste Succeeded 等) は COM が UI スレッドへ運ぶ。UI スレッドは winit の message loop で pump しているので届く。UI スレッドはこの thread を待たない (join しない) ので、互いに待つ形は生じない |
| worker の OLE 初期化 | 上の常駐 thread の起動時に 1 回 `OleInitialize`。失敗したら以後の貼り付けを「貼り付けを開始できませんでした」で断る (ログ + 通知) |
| 選択要求の取り違え | `PostOperationSelection` に要求番号を持たせ、貼り付けの完了が失敗のとき「自分の番号のときだけ」取り下げる。間に別の操作 (新しいフォルダの作成 `new_folder.rs:101` など) が要求を置き換えていれば何もしない。番号は `request_post_operation_selection` が一か所で振る (全 producer 共通) |

### 2.2 worker 化の形

- `ShellPasteService` (process 共通、App が 1 つ持つ): 初回の貼り付けで `shell-paste` thread を起こす。thread は
  `OleInitialize` → `MsgWaitForMultipleObjects` で要求の到着とメッセージの両方を待ち、到着した要求を 1 件ずつ処理する
  (中身を控える → bind → query → 中身の再照合 → `InvokeCommand`)。既存の `verb_shell_bind` / `verb_query_shell` / `verb_invoke` の計測は
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
  - `Err(ClipboardChanged)`: 通知 (上の文言) + 自分の番号なら選択要求を取り下げ。
  - その他の `Err`: 従来どおりログだけ + 自分の番号なら選択要求を取り下げ。
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
  (9) クリップボードの中身の照合: 準備中に中身が変わると貼らずに通知、パス一覧と効果が同じなら (番号だけ進んでも) 貼る
  (読み取りは差し替え)。
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
