# §1.263 固定の「ファイル整理先」へコピー・移動する設計案

2026-10-04。対象 worktree: `C:\home\mimageviewer-fileops`、branch: `next-file-ops`。
要件の正本は [next-release-backlog.md §1.263](next-release-backlog.md#1263-固定のファイル整理先へ選択項目をコピー移動する--444447-2026-09-21)。
2026-10-04 の独立レビュー後の利用者決定と補足 P2/P3 を反映。実装済み (レビュー前)。製品バイナリの起動、コミットは行わず、自動検証と利用者の実機確認を分ける。

## 1. 推奨する構成と不変条件

お気に入りとは別の `Settings.file_organize_destinations` を環境設定で編集し、グリッドの右クリックと既定キーなしの `KeyAction` から同じ整理先選択画面を開く。各行の「移動」「コピー」から Windows Shell に直接依頼する。クリップボードとお気に入りを変更せず、整理先へ一覧を移動しない。

既存の `shell_file_ops.rs` にある STA・`IFileOperation`・channel による非同期実行の形を拡張する。コピー／移動そのもの、競合確認、進捗、取消、アクセス拒否、部分失敗は Shell が担当する。mIV 専用の転送エンジン、常設 worker、進捗集計、失敗分類、再試行、ロールバック、操作ジャーナルは作らない。

- 対象は入口で確定した実ファイル／実フォルダ全件。カーソルとチェックを実行時に取り直さない。
- ZIP/PDF 内ページや合成セルを実ファイル、代表画像、外側コンテナへ勝手に置き換えない。
- ファイルとフォルダの混在は全件を渡す。対象外が混在するときも、黙って実項目だけ実行しない。
- 完了後は、現在表示中の実フォルダが元／先なら既存の外部変更再確認を起動するだけ。開始時 index による変更、消失項目の直接除去、チェック保持、保持一覧／viewer の専用後始末は行わない。
- 部分取消／失敗でも同じ再確認に渡す。全件成功と推測せず、エクスプローラーで移動した場合と同じ既存挙動に任せる。
- 設定、評価、タグ、編集、本棚、コレクションを、ファイル移動の「削除後処理」として消さない。

## 2. 現状のコード事実

以下の行番号はこの worktree の調査時点。設計判断と既存動作を区別する。

| 領域 | コード事実・再利用可否 |
| --- | --- |
| コピー／切り取りの右クリック入口 | `src/ui_dialogs/context_menu.rs:1633` の `MenuCommand::CutFiles / CopyFiles` は解決済み `target.shell_paths` を共通 clipboard 入口へ渡す。背景操作と項目操作は別。 |
| キー入口の選択 | `src/app.rs:50535` の `collect_shell_clipboard_paths` は checked 優先、なければ selected。実パス数と checked 数が違えば全体拒否する。`src/ui_dialogs/context_menu.rs:2119` の `collect_checked_indexed_paths` は `drag_source_path` を使い、削除用に index 降順で返す。整理先画面の表示順には `current_grid_order()` を使う。 |
| メニュー対象の確定 | `src/ui_dialogs/context_menu.rs:1251` は checked の RealOnly / VirtualOnly / Mixed を判定し、RealOnly だけ `shell_paths` を作る。単一右クリックは clicked item、キーは selected。チェックがある場合は clicked item を勝手に追加しない。 |
| clipboard 実行 | `src/app.rs:50589` が Copy/Cut の共通入口。`src/native_context_menu.rs:674` は Shell `IDataObject`、preferred drop effect、`OleSetClipboard` を使う。切り取り表示の observer は `src/app.rs:50623` で接続される。ここは UI 呼出しで、重いコピー実行 worker ではない。 |
| 現在地への貼り付け | `src/app.rs:50783` と `src/ui_dialogs/context_menu.rs:1603` は `request_post_operation_selection_for_added_items` の後、現在の実フォルダへ Shell の Paste verb を呼ぶ。`src/native_context_menu.rs:979` は背景 `IContextMenu` を構築して Invoke する同期 API で、戻り値は verb 呼出しの成否。転送全体の完了／全件成功を受信する API ではなく、UI を待たせない直接転送経路としてそのまま流用できない。 |
| 貼り付け後の選択 | `src/app.rs:63084`、`src/app.rs:63149`、`src/post_operation_selection.rs:100`。実行前との差分で追加項目を選ぶ。別フォルダへ移った場合や利用者が選択を変更した場合は追従をやめる。今回は整理先を開かないので、新規選択要求は原則不要。 |
| 外部 D&D のコピー | `src/ui_dialogs/context_menu.rs:2596` の `copy_paths_into_folder` は PowerShell worker。フォルダを除外し、同名ファイルを `-Force` で上書きする。`CopyOutcome` は失敗件数／先頭エラーを返す (`同:2575`)。ファイル＋フォルダと標準競合確認という要件に合わず、今回流用しない。 |
| 削除 worker | `src/app.rs:41057` → `src/delete_worker.rs:98` の spawn → `同:591` の `IFileOperation`。UI は `src/app.rs:41112` で `try_recv` し、成功 path を現在の items に引き直す (`同:41215`)。失敗の件数通知は `同:41192`。`DeletePending` は削除専用で、メタ purge・retry を含むためコピー／移動用に転用しない。 |
| 削除の Shell UI | `src/delete_worker.rs:575` のフラグは mIV の確認／進捗画面のため通常 Shell UI を抑止する。これを整理操作へコピーすると標準競合・エラー画面を失うので、フラグを流用しない。 |
| リネーム worker | `src/ui_dialogs/rename_item.rs:133` → `src/shell_file_ops.rs:15`。worker 内で STA を初期化 (`同:54`)、owner HWND を設定 (`同:91`)、`IFileOperation::PerformOperations` (`同:104`) を実行。UI の受信は `rename_item.rs:158`。この実行境界が直接コピー／移動に最も近い。 |
| リネーム完了 | `src/ui_dialogs/rename_item.rs:166` は abort を通知し、成功時に old path の viewer 解放、path-key 移行、smart snapshot 除去を行う。現在地が元の親なら `select_after_load / pending_reload` (`同:189`)。この old→new の DB 移行は clipboard 移動にはなく、フォルダ結合を含む整理移動に無条件で転用できない。 |
| 実フォルダの再同期 | `src/app.rs:21936` の watch、`同:22101` の外部変更確認、`同:22151` の worker 再走査、`同:22299` の適用。現在フォルダと一致しない結果を捨て、同じ listing は維持し、main viewer 閲覧中は既存の再読込へ保留する。watch の列挙は非同期だが `check_external_folder_changes` の親 metadata 確認は既存の同期 I/O。新たな全件 stat／走査を UI に足さない。 |
| 整理操作で使用しない後始末 | `src/app.rs:38092` の `remove_items_batch`、`src/app/subfolder_expansion.rs:2156`／`src/app/smart_folder.rs:8992` の snapshot 除去、`src/app/collection_grid.rs:888` の source 失効、`src/app.rs:38530` の viewer 解放は削除／リネーム等の明示的な変更経路。整理操作からは呼ばない。外部移動の再確認／再読込だけを使う (§5.4)。 |
| 対象種別 | `src/grid_item.rs:375` の `drag_source_path` は Folder / Image / Video / Audio / ZipFile / PdfFile / ConvertibleArchive の実パスだけ返す。`file_operation_path` (`同:354`) は Folder を含まず、こちらだけでは混在集合を狭めてしまう。 |
| 対象外理由 | `src/grid_item.rs:395` の `file_operation_refusal` が理由の正本。ZipImage / PdfPage、ZipDir、Stack、SearchContainer、CollectionPlaceholder を区別する。Stack の代表画像、SearchContainer の表示用 path は整理対象の実パスではない。 |
| お気に入り編集 UI | `src/ui_dialogs/favorites_editor.rs:352` は即時反映／保存で OK/Cancel がない。IME 名称欄 (`同:748`)、↑↓／削除 (`同:878`)、Vec swap (`同:1191`)、save (`同:1242`) の UI パターンだけ参考にする。索引・UUID・お気に入り標準設定の副作用は持ち込まない。 |
| 環境設定の確定 | `src/ui_dialogs/preferences.rs:2090` が一時コピーを作り、`同:1872` が OK 前に `overwrite_non_preferences_from` を通し、install (`同:1959`) と save (`同:2592`) を行う。お気に入りは non-preference として live から移される (`src/settings.rs:9822`)。整理先は環境設定の編集対象なのでこの移送に追加してはならない。 |
| 保存 | `src/settings.rs:9892` の save、`同:9897` の `save_checked`、`同:9926` の共通書込境界。既存保存は同期で、save は戻り値を捨て、失敗はログ (`同:9981`)。`src/settings_db.rs:12` と `同:892` の `save_full` は専用テーブルに分離しないフィールドを settings_kv の JSON 値として保存できる。新 DB／新テーブルは不要。 |

現行の「一覧が仮想か」を一律に拒否するのは誤り。検索、タグ、★、履歴、smart、サブ展開、コレクションでも実 `GridItem` は対象にできる。`current_favorite_target()` (`src/app.rs:21920`) は現在地を貼り付け先にするための判定で、今回の source 判定には使わない。表示用 `display_path` や `container_path` で実パス可否を代用しない。

## 3. 整理先の設定と環境設定 UI

### 3.1 データ・既定値

データ型は `FileOrganizeDestination { name: String, path: PathBuf }`、`Settings.file_organize_destinations: Vec<FileOrganizeDestination>`。順序は Vec の順そのものとし、別の order 値、UUID、favorite ID、索引フラグは持たない。`#[serde(default)]` と `Settings::default()` は空 Vec。5 件は想定用途であり、5 件の固定スロット制限にはしない。

settings_kv の同名キーへ保存する。旧 DB の欠落は空リスト。お気に入り、起動フォルダ、ツールバー、履歴から自動取り込みしない。未接続ドライブや存在しない先も、保存／起動時の sanitize で消さない。名称と順序は利用者が作った設定である。

### 3.2 編集画面

環境設定「フォルダ・ファイル」 (`PreferencesPage::Folder`、`src/ui_dialogs/preferences/pages.rs:8537`) に「ファイル整理先」セクションを追加する。既存のページ内 anchor／設定検索にも登録する。

- 表の行に表示名、実フォルダのパス、「参照…」、↑、↓、「削除」。末尾に「追加」。↑↓は端で disabled。ドラッグ並べ替えは初版に加えない。
- 追加は native folder picker で選び、末尾に追加する。既定名はフォルダ名（ルートはルートの表示名）。名称とパスは行内編集でき、「参照…」はその行のパスだけ置き換える。picker の取消は何も変えない。
- 編集は `PreferencesState.settings` だけに反映し、OK で live と DB へ確定、キャンセルで破棄する。「削除」は登録を外すだけで、ディスク上のフォルダを消さない。draft なので別の削除確認は増やさない。
- 名称空欄、パス空欄、相対パス、NUL を含むパスを入力エラーとして示し、修正まで OK を無効化する。環境変数／シェル引数の展開は行わない。ドライブ絶対パスと UNC を扱う。FS の存在確認はここで毎フレーム行わず、実行 worker で行う。
- 同名／同じ先の重複は保存を阻まない。名称とパスを併記するので識別できる。別途 ID 管理や重複修復は不要。
- お気に入りの即時 save をコピーしない。`overwrite_non_preferences_from` に追加せず、`prepare_preferences_settings_for_commit` → install の実経路を試験する。

既存の右ペイン solid scrollbar、`auto_shrink([false, false])` と利用可能幅を使う。長い UNC パスは折返し／全文 tooltip で確認でき、狭幅では編集欄と操作列を上下へ分ける。追加した登録による一覧／viewer の再ロードは不要。

保存失敗は既存 `save_checked` を利用してログ＋通知する。適用済みのメモリ値はそのセッションで使い、「保存できませんでした。再起動すると今回の変更が残らない可能性があります」と示す。自動再試行や旧設定へのロールバックは足さない。§8 の決定どおり、自動再試行・ロールバックは追加しない。既存の保存抑止／互換性保護を迂回しない。

## 4. 整理先選択ダイアログと入力

### 4.1 入口・対象

入口はグリッドの項目右クリックの静的 leaf「ファイル整理先…」一つと、既定キーなしの `GridOrganizeFiles` だけ（利用者決定）。メニューバー・ツールバー・「その他のフォルダ…」は初版に追加しない。コピー・移動の入口には整理先をメニューへ全列挙しない。閲覧用の場所▼サブメニューは §12 の追補。背景、ツリー、画像／動画 fullscreen、detached の viewer メニューにも新規入口を加えない。チェック優先、なければ右クリック項目／キーのカーソルを使い、同じ要求生成メソッドへ渡す。

`context_menu_model` の `MenuCommand`、stable `ContextMenuItemId`、ALL、文字列名、label、command→item 対応、可否、shortcut label、preview を揃える。native HMENU と egui fallback は同じ MenuNode を使う。§1.221 の右クリックカスタマイズで新 leaf の表示・並び・区切りが扱えるようにする。既存レイアウトに未知だった ID が加わっても既定位置に出ることを確認する。

全件の path／種別／表示名を入口時に捕捉する。対象外は `file_operation_refusal().message("移動 / コピー")` を使い、一覧上だけの項目を識別して理由を示す。**既存 Copy/Cut と同じ全体拒否に決定**：混在時は「対象外があるため実行しません。該当項目のチェックを外してください」と対象名・理由を表示する。実項目だけ続行する分岐は作らない。実ファイル＋実フォルダの混在は拒否しない。右クリックとキーのどちらも理由を通知でき、単なる無反応にしない。

### 4.2 表示と操作

既存 modal の規約に従う整理先選択画面に、対象件数と対象名（多い場合は折畳みの対象一覧）、次の表を表示する。

| 登録名 | パス | 操作 |
| --- | --- | --- |
| 保管 | `D:\写真\保管` | 移動　　コピー |
| 要確認 | `D:\写真\要確認` | 移動　　コピー |

登録順で並び、各行のボタン一回で選んだ操作を開始する。余計な mIV 確認は重ねず、競合確認は Windows に任せる。「移動」と「コピー」は十分に間隔を取り、色だけに頼らず文字・focus 枠で識別する。長いパスを確認でき、行の高さで他行のボタンと取り違えない。下部に「閉じる」。登録なしなら説明と「環境設定で登録…」を表示する。

選択画面の表示中だけ `common_modal_dialog_open / modal_dialog_block_reason` に含め、背面へのキーと pointer の漏れを防ぐ。登録画面へのリンクは選択画面を閉じ、対象 snapshot を破棄して環境設定を開く。戻るための再開状態や画面の live rebuild は作らない。整理先の行は開いた時の設定 snapshot とし、行 index から後で live 設定を引き直さない。

### 4.3 KeyAction と IME

Action は `GridOrganizeFiles`、ini 名は同名、説明「選択中またはチェック済みの実ファイル／実フォルダの整理先を選ぶ」。`KeyContext::Grid`、`KeyTrigger::Press`、既定 chord は空、repeat で再起動しない。

`src/keymap.rs` の enum / `ALL_ACTIONS` / `ini_name()` / `description()` / `context()` / `trigger()` / `default_chords()` と Grid handler を揃える。既存 Grid の keyboard owner、IME、text focus、modal、remote ownership の gate を保って `consume_action_no_repeat` で共有入口を呼ぶ。`command_catalog()` (`src/keymap.rs:2535`) から操作カスタマイズ、検索、競合表示、文脈ヘルプへ届くことを確認する。固定先ごとの動的 Action、直接移動／コピー Action、native 動画 VK 転送、RingAction は初版に追加しない。

`docs/keymap.ini.default` は `# GridOrganizeFiles = none` を生成結果に合わせ、`docs/keymap-spec.md` と保守手順を更新する。現行の保存正本は `Settings.keymap` であり、旧 keymap.ini を新たな設定 DB として扱わない。

名称／パスの single-line TextEdit は `crate::ime_focus` helper で描画する。Enter/Escape は `dialog_enter_pressed(ctx)` / `dialog_escape_pressed(ctx)` を使う。raw `key_pressed` で IME の確定／取消を奪わない。

- 環境設定の Enter は現在の検索／入力欄処理に従い、入力確定から整理操作を起動しない。Escape は既存の検索解除→キャンセル規約を維持する。
- 整理先画面は初期状態で移動を選ばない。↑↓で行、←→で「移動／コピー」を明示選択し、Enter は明示選択済みのボタンだけ実行する。Escape は閉じる。ダイアログローカルの入力として consume し、背面の keymap へ流さない。
- アプリ全体で Tab traversal が無効という現行仕様に合わせる (`docs/keymap-spec.md`「固定入力」)。新画面だけ Tab を再有効化しない。pointer と明示的な矢印選択で使え、フォーカス表示と読み順を UI 試験する。
- IME 中は画面の Enter/Escape 操作を行わない。Shell の native ダイアログ内キーは Windows の所有で、mIV 側から重ねて捕捉しない。

## 5. 実行境界と完了後の処理

### 5.1 割り込みを減らす案の比較

| 案 | 利点 | 追加負担・判断 |
| --- | --- | --- |
| 実行完了まで全体 modal | 一覧移動、履歴、窓切替、後続操作の組合せを消せる。 | 長い別ドライブ転送中も閲覧できず、複数の egui/native 窓と Remote を一括停止する所有境界が必要。Shell owner 指定だけで全窓 modal になるとは扱えない。 |
| 開始時 snapshot、転送中は従来の閲覧を許可 | clipboard 操作と同じく、移動先を開かず閲覧を続けられる。 | 完了後は現在の実フォルダの既存外部変更再確認だけ。保持 snapshot の整理操作専用反映、rollback／resume は不要。 |

**snapshot を採用する**。本機能は設定確定や本を開く変換と違い、転送先が現在の viewer 状態に依存しない。完了後をエクスプローラー移動相当へ揃え、一覧ごとの独自後始末も不要にした。全体 modal の新しい窓横断排他を足さず、長時間コピー中の既存閲覧を制限しない。選択画面は modal、Shell 実行は非同期と分ける。Shell が自分の確認画面により owner の入力を止める期間は Windows に従う。

App が持つ新しい状態は、この機能の一つの request owner（非表示／選択中／実行中を表す enum）だけにする。選択中は対象と整理先リスト、実行中は確定要求と receiver を所有する。show bool、pending Option、対象別 pending、新しい終了／Remote待機状態は増やさない。実行中に同じ機能を開き直す要求は通知して既存要求を維持し、同一フレームの pointer＋Enter による二回目の投入は enum の所有権消費により何も投入しない。他機能の状態機械を統合し直さない。

実装では選択中／実行中の payload を Box に置き、非表示時は割り当てない。enum は `2 * usize` 以下に検証する。設定 Vec は既存の環境設定 draft にも含まれるため、App の既存サイズ試験は上限を 110,000 から 110,128 バイトへ小幅に調整する。実測は payload を inline にした場合 110,088、Box 化後 110,024 バイト。新しい状態や、無関係な共有型の再編は増やさない。

### 5.2 Shell への直接依頼

1. 行のボタン／Enter は、worker 投入直前の共通入口一か所へ集約する。そこで request enum が選択中であること、`remote_session_blocks_local_control()` が false、終了要求がないことを確認する。終了判定には既存 `shutdown_requested`、tray の quit request、main/root の close request を使い、新しいフラグは足さない (`src/remote_ipc/ui.rs:977`、`src/tray_integration.rs:120`、`同:223`)。Remote 所有／終了要求なら選択中の要求を破棄して画面を閉じ、実行しなかった理由を通知する。選択中以外からは投入せず、既に実行中の要求を消さない。受理する場合だけ選択中の所有権を一度消費し、捕捉済みの対象・整理先・main owner HWND で実行中へ移す。pointer＋Enter の二回目には消費できる選択要求がない。HWND がなければ同じく通知して開始しない。選択画面を閉じ、独自進捗／取消画面は重ねない。
2. `shell_file_ops.rs` の既存 async 呼出しの形を拡張する。COM／Shell item の構築、パス検証、重い I/O、`PerformOperations` は STA worker の中だけ。UI から同期 join／待機を行わない。一般化は STA guard 等の必要な共通部分に留め、delete worker の purge／retry を統合しない。
3. 整理先を実ディレクトリとして確認し、全対象を queue できることを確認してから `CopyItem / MoveItem` を一つの `IFileOperation` に予約し、`PerformOperations` を一回呼ぶ。予約段階で失敗したら実行せず通知する。クリップボードを用いた cut→paste や PowerShell コピーには迂回しない。
4. リネームと同じ owner／undo 用フラグを基礎にし、競合・進捗・エラー UI を抑止する `FOF_NOCONFIRMATION / FOF_NOERRORUI / FOF_SILENT` を設定しない。自動上書き／自動改名を mIV の方針にしない。
5. `PerformOperations` の戻り値だけで全件成功としない。`GetAnyOperationsAborted` と Shell の実結果を尊重し、取消／失敗時も完了処理へ進める。受信と repaint 起床は既存 worker/channel の形を使い、全フレーム高速 repaint で待たない。

`IFileOperation` は STA のみで使用でき、標準の進捗・エラーダイアログを持つ。根拠: [Microsoft Learn: IFileOperation](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-ifileoperation)。

結果は操作終端の成否／中断だけで足りる。元／先の照合には要求にある source の親と整理先を使う。消失 path、保持 snapshot の実パス、旧→新の対応、item ごとの結果／競合後名は取得しない。`PostMoveItem` sink、完了後の存在確認 worker、保持一覧の収集を追加しない。Windows の処理が始まった後は Remote 接続や終了要求を理由に停止せず、mIV の途中中断／終了待機／再開機構を作らない。終了をまたぐ完了保証も加えない。

### 5.3 パス条件

| 条件 | 初版の扱い |
| --- | --- |
| 整理先消失・ドライブ未接続・実フォルダでない | worker の開始前検証でログ＋通知、操作を行わない。設定行を削除せず、フォルダを自動作成しない。検証後に消えたら Shell の標準エラーに従う。 |
| 対象の消失／予約不可 | 実行前なら全体を開始せず通知。開始後の外部変更は Shell に任せ、完了時は §5.4 の条件に合う現在の実フォルダだけ再確認する。 |
| 移動先が元の親と同じ | 当該対象名と「同じフォルダなので移動できません」を通知し、要求全体を実行しない。複数元の一部だけが該当しても黙って skip しない。コピーは同じ親でも Shell の同名処理へ渡し、mIV で上書きしない。 |
| Folder 自身／配下へ移動 | 実行前に全体拒否して理由を通知。コピーも自己再帰になる組合せを拒否する。 |
| 大文字小文字、区切り、末尾区切り、`.` / `..`、junction／symlink | worker で実在 path を正規化・解決し、ドライブ／UNC を保持した成分比較で同一／配下を判定する。文字列 prefix だけにしない。解決できなければ通知して開始しない。UI は FS probe しない。 |
| ファイルと Folder、同名 basename、親 Folder と子の同時選択 | 全対象をそのまま Shell に依頼し、集合をファイルだけ／親だけへ縮めない。途中で既に移った子や競合は Shell の標準画面に任せる。 |

`src/path_key.rs:34` のドライブ保持正規化は比較表記の参考になるが、実体解決や `..` の正規化は行わない。`src/folder_tree.rs:1236` の `path_eq` も小文字文字列比較だけなので、自己配下判定の安全性をそれだけで主張しない。検証後の外部変更に対する mIV 独自の監視／再試行は足さず、Shell を最終の実行主体とする。

同じ親への移動判定は `canonicalize(source.parent())` と整理先を比較する。junction 本体の置かれた親を、リンク先の親と取り違えない。自己配下判定は従来どおり `canonicalize(source)` を用い、Shell に渡す元パスは選択時のまま維持する。

### 5.4 完了後はエクスプローラーで移動した場合と同じ扱い

成功、途中取消、部分失敗、channel 終了で実行中の request を非表示へ戻し、必要な終端通知を行う。**完了時に現在表示している実フォルダが、要求した source の親フォルダのいずれか／整理先と一致すれば、既存の外部変更再確認を一回要求するだけ**。一覧を移動済みなら元の場所へ戻さず、無関係な現在地は再確認しない。コピーも同じ扱い。要求元の index／context を完了反映のために保持しない。

現在地は既存の `is_physical_folder_listing()`（通常scanのmarkerと最上位surfaceを照合、`src/app/content_identity_detection.rs:125`）で確認し、検索等に残っている `current_folder` を実フォルダ表示と誤認しない。この既存述語は変更しない。main update の通常 poll 境界から `check_external_folder_changes(ctx, ExternalChangeCheck::Notified)` に渡す (`src/app.rs:22101`、`同:84321`)。`Resumed` と違って `Notified` は親 mtime が不変でも worker 再走査へ進むため、整理先の同名置換も同じ確認入口で扱える (`同:22136`)。既存入口の検索／削除中等の抑止、viewer 閲覧中の適用保留、同一フォルダ走査中の再確認予約を迂回しない。UI で read_dir／対象ごとの stat を行わず、走査結果の選択／cache 処理も既存 consumer に任せる。

整理操作から `remove_items_batch`、snapshot 除去、検索保持ヒットの変更、コレクション失効、viewer 解放、チェック保持／復元、`AddedSince` 選択、他画面への再読込通知を行わない。Folder 結合や整理先置換にも専用処理を足さない。対象外の既存挙動を整理操作の新しい不変条件にしない。

既存の外部移動時の挙動は次のとおり。

| 場面 | 既存経路の事実 (file:line) |
| --- | --- |
| 現在の実フォルダ | `src/app.rs:21936` の watch が debounce 後に Notified を送り、`同:83829` の focus 復帰は Resumed を送る。`同:22151` の再走査 worker が列挙し、同一フォルダの走査中は restart_requested で再確認を予約する。 |
| 一覧切替／選択・チェック | `src/app.rs:22306` は現在地が違えば走査結果を捨てる。`同:22318` は同じ listing なら一覧を維持。変更時は `同:22401` の既存 reload へ渡す。再読込は checked を消し (`同:22349` のコメント)、残存カーソルを path で戻す (`同:22405`)。整理操作でチェック保持を保証しない。 |
| main viewer／開いた書庫 | `src/app.rs:22326` は main viewer が一覧を占有していれば folder_refresh_pending へ保留し、`同:37047` が一覧復帰時の再読込を処理する。書庫／PDF 自身は `同:22123` の metadata.is_dir 判定を通らず、そのページ一覧を整理操作から再列挙しない。 |
| 検索の保持ヒット／★固定／退避一覧 | `src/app.rs:22106` は global search／tag active なら外部再確認を抑止する。外部走査の適用は現在の folder の reload であり (`同:22401`)、保持ヒットや★保存一覧へ消失 path を配る処理ではない。`remove_items_batch` にあるサブ展開／smart snapshot 除去 (`同:38139`) は整理操作へ持ち込まない。退避状態の後日の復元もエクスプローラー移動時の既存動作のまま。 |
| コレクション／別窓の本／他画面の整理先 | 外部再確認から `src/app/collection_grid.rs:888` の全context source失効や `src/app.rs:38530` のviewer解放は呼ばれない。これらを使う明示削除 (`同:41072`、`同:41212`)／リネーム (`src/ui_dialogs/rename_item.rs:171`) と区別する。各画面の通常の読込／更新、保持表示、旧path不在時の挙動に任せ、整理先置換のcacheも専用失効しない。 |

**独立レビュー指摘1〜5（保持snapshot、検索保持ヒット、チェック保持、mainで開いた書庫、整理先置換の失効）は、エクスプローラー移動と同じ既存挙動として今回の専用対応・専用試験の対象外**。既存挙動が完全に即時収束するという主張ではない。旧項目や保持表示が残る場合も、既存の更新／開き直しに従う。

同名競合、権限、skip、取消、部分失敗の判断 UI は Shell。mIV の通知は「操作が中断されました。処理済みの項目は元に戻りません」「操作を完了できませんでした。フォルダの内容を確認してください」程度にする。abort は利用者取消と断定しない。[Microsoft Learn: GetAnyOperationsAborted](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperation-getanyoperationsaborted) は利用者またはシステムによる中断を含む。再走査の読取失敗は既存処理に任せ (`src/app.rs:22203`)、専用の再試行、途中状態保存、再起動後の再開を作らない。

## 6. Remote・detached・評価／タグ／編集の分担

### 6.1 Remote 接続・終了要求（レビュー指摘6）

選択画面を開く時の gate だけでは足りない。ボタン／Enter 共通の **worker 投入直前一か所**で、request enum・Remote 所有・終了要求を確認する (§5.2)。選択中に Remote が接続した場合は、既存の `remote_session_blocks_local_control()` で観測した時点で選択要求を破棄して画面を閉じ、通知してよい。閉じる観測が間に合わなくても投入境界で再確認する。新しい Remote 接続待機状態、排他 lease、worker cancel、途中停止、disconnect 後の要求再開は持たない。

Windows の処理開始後の Remote 接続は処理を止めない。Remote が旧pathを参照し得る点も外部移動と同じ。PC使用中のリモート接続は例外ケースとしてこの割り切りを採用する（2026-10-04 利用者決定）。終了要求を投入前に拒否する一方、開始後の通常終了／強制終了をまたぐ転送完了保証や保存済み要求の復旧は加えない。新しい Remote UI／IPC／Web UI は不要。

### 6.2 detached viewer

新規入口、viewport、window runtime 状態、context別の後始末は追加しない。同じ実体を表示する別窓、main の書庫、整理先を表示する他画面も §5.4 の外部移動相当の既存挙動に任せる。整理操作から viewer close／source invalidation を呼ばず、detached 述語／viewport 経路を変更しない。

### 6.3 評価・タグ・編集は既存の「編集内容の復元」が担当

**整理操作では評価・タグ・編集を移さない**（利用者決定）。旧pathのDB行も消さず、rename migration／copy_stores／purgeを整理操作から呼ばない。移動／コピー先の通常フォルダを開いたときに、内容一致を検出する既存の [edit-content-identity-plan.md](edit-content-identity-plan.md) の確認・復元経路で引き継ぐ。

コードで確認した範囲と、マニュアルに書く手順：

- 通常の物理フォルダの読込完了から検出を開始する (`src/app.rs:35134`、`src/app/content_identity_detection.rs:125`、`同:144`)。復元確認設定が有効で台帳が利用可能なときだけで、detached physical／検索等の合成一覧からは開始しない。
- 復元対象は画像、ZIP、PDF／EPUB、対応アーカイブ。Folder 自身、Video、Audio は内容一致の対象ではない (`src/content_identity.rs:156`)。Folderを丸ごと移した場合は、その中の対象ファイルを表示する通常フォルダを開く。ZIP/PDFは本体の内容一致によるページ状態の復元で、ページを独立ファイルとして整理する意味ではない。
- 対象を検出すると **フォルダ単位でまとめた「編集内容の復元」確認が1回表示され、利用者が「復元する」を選ぶ** (`src/app/content_identity_detection.rs:280`、`src/app/content_identity_restore.rs:70`、`src/ui_dialogs/content_restore.rs:290`)。ファイルごとの確認や整理操作自身の確認は追加しない。「整理操作一回につき必ず一回」ではなく、開いたフォルダの検出結果単位である。
- 復元 worker が既存storeコピーを使って評価・タグ・編集等を新pathへ複製する (`src/content_identity/restore.rs:185`)。復元先は台帳へ昇格記録され (`同:334`、`同:472`)、既知の復元先は次の検出対象から外れる (`src/content_identity.rs:325`)。一方「閉じる」／Escは拒否を記録しないので、次回開いたときに再確認され得る (`src/app/content_identity_restore.rs:115`)。「次から確認しない」は既存設定を無効にする。
- **★・タグ・編集を記録済みのファイルが復元の対象になる**。内容一致の記録がないファイルは復元されない。記録完了を確認できる専用の表示や待機手順は案内しない。整理操作のための待機や強制ハッシュも追加しない。
- この遡り記録は、補正・消しゴム・隠蔽・ローカル調整・注釈／comic・出力範囲のpresenceを使う (`src/app/content_identity_detection.rs:411`)。**未記録の全ファイルや評価／タグだけの旧項目まで、開くだけで必ず記録されるわけではない**。編集／表示状態の確定時には別の既存記録入口がある (`src/app.rs:69626`)。設定OFFではfolder-openの検出／遡り記録も行わない (`src/app/content_identity_detection.rs:147`)。整理操作のためにハッシュ記録を追加・強制・待機する処理は作らない。

この分担は既存復元機能の対象／条件内での引き継ぎであり、Folder／Video／Audioを含む整理可能な全種別に同じ保証を広げない。復元の対象外やOFF／未記録では、既存機能の制約をそのまま説明する。

### 6.4 設定の書き出し §1.317

整理先のパスはPC固有として環境設定の書き出し対象外にする予定。名前／順序だけの取り込みで操作先を失わないよう、整理先Vec全体を可搬exportから除外する案を推奨し、importで移行先の既存整理先を消さない。通常のsettings.db保存／バックアップとは区別する。

## 7. 絞ったテスト計画とマニュアル更新

実装の自動検証は以下に絞り、Shellの失敗分類や既存の外部移動挙動を整理操作専用に再検証するsuiteは作らない。

| 対象 | 確認する内容 |
| --- | --- |
| 現在フォルダの再確認 | fake完了で、現在の実フォルダがsourceの親／整理先なら既存Notified再確認を一回要求する。成功／中断／エラーでも同じ。別フォルダや仮想一覧には送らず、完了前の一覧切替後は完了時の現在地だけで判定。走査・適用自体は既存試験を再利用し、項目の直接除去やチェック保持を期待値にしない。 |
| 投入境界の拒否 | 選択画面を開いた後のRemote所有／shutdown／tray quit／root close requestで、ボタンとEnterが同じ境界から拒否し、workerを起動せず画面を閉じて通知。Remote接続の事前観測で閉じた要求も投入されない。実行中enumを二回目の拒否で消さない。開始後はRemote／終了による独自cancelを送らない。 |
| 二重投入 | pointer＋Enterが同フレームに来ても、選択中enumの所有権を一度だけ消費し、確定した一要求だけをworkerへ送る。既に実行中／非表示からの投入はゼロ。別のpending／sentinelを足さない。 |
| 設定の通し | 追加・表示名／path編集・削除・並べ替えを **編集→OK→開き直し**、さらにsettings.db再読込まで通す。キャンセルで元の値、全件削除で空、旧設定の欠落は空、不在先は保持。実際のpreferences OK mergeを通り、favorites／toolbarを変えない。保存の成否は既存 `save_checked` 境界を使用し、失敗時は適用済みメモリ値を保持して通知する。 |
| 選択解決・入口 | カーソル1件／checked優先、右クリック別項目、実ファイル＋Folder全件、各仮想／合成種別、実／仮想混在の理由付き全体拒否。右クリックと既定noneのKeyActionが同じ対象snapshotを作り、開始後の選択／設定変更で対象・整理先が変わらない。既存keymapインベントリ／default生成／IME・text focusの試験もこの入口の配線確認に再利用する。 |
| Shell の複数対象コピー／移動 | 使い捨て temp データのファイル＋フォルダを競合なしで `CopyItem / MoveItem` へ渡し、内容・元パスの存否を実際に確認する Rust テスト。Shell UI が出得る環境依存試験なので `#[ignore]` とし、製品バイナリは起動しない。実行コマンドは検証報告に記載する。 |
| UI snapshot | 本番の描画 helper を使い、環境設定の整理先編集を light／dark、整理先選択の登録済み／空を保存画像と比較する。対象一覧の展開は表示範囲の行だけを描画し、長いパスは一行に省略して tooltip に全文を表示する。新しい要求状態は追加しない。 |
| パス検証 | 元の親へのmove、同じ親へのcopy、Folder自身／配下、隣接prefix、別ドライブ、UNC、大小文字／区切り、`.`／`..`、junction／symlink、不在先。危険な組合せを通知して全体拒否し、検証とShell処理がUI threadにない。 |

レビュー指摘1〜5のための保持snapshot収集／消失確認、検索差替え、チェック保持、viewer close、別窓／他画面の整理先失効、folder結合後の独自整合試験は削除する。既存の外部移動より悪いクラッシュ・利用データ消失・UI停止を新規コードが起こさないかは通常のコード確認で報告し、それを既存挙動の対象外扱いで免除しない。

実装時は絞った `cargo test -p mimageviewer --lib <設定/選択/投入/再確認のfilter>` と既存keymap参照チェックから開始する。最終gate／fmt／UI glyphチェックと利用者用 `scripts/build-dev.ps1` のhandoffは [development-build-and-test.md](development-build-and-test.md) とリポジトリ手順に従う。不要になった専用後始末のための新規試験やライブmulti-window suiteは追加しない。

実装に伴って更新する場所：

- `spec.md`：右クリック1項目＋既定キーなしAction、対象／混在拒否、Windows標準処理、完了後は外部移動相当、投入前のRemote／終了拒否。
- `htdocs/mimageviewer/manual/settings.html`：整理先の登録・編集・順序・OK/Cancel、不在先、既存復元確認設定との分担。
- `htdocs/mimageviewer/manual/grid.html`：唯一の右クリック入口、checked優先、対象外理由。メニューバー／ツールバー導線は書かない。
- `htdocs/mimageviewer/manual/tut-file-ops.html`：登録→選択→移動／コピー、clipboardを使わず元一覧に留まる手順、外部移動相当の更新。**評価・タグ・編集は整理時に移さず、先の通常フォルダで既存「編集内容の復元」の一括確認から引き継ぐ**こと、★・タグ・編集を記録済みのファイルが復元対象となる条件を記載し、記録を待つ手順は書かない。「閉じる」は次回も確認され得ること、復元設定OFF／対象外種別／編集presenceのない未記録項目も正確に説明する。
- `htdocs/mimageviewer/manual/shortcuts.html`、`docs/keymap-spec.md`、`docs/keymap.ini.default`：既定noneのActionと割当。画面内入力はローカル操作。
- `docs/item-kind-capability-matrix.md`、`docs/README.md`、backlog：種別・索引・実装後の状態更新。既存復元機能の変更は行わないので、その新規設計や改修は混ぜない。

## 8. 決定済み事項と残るまれな失敗の扱い

2026-10-04の利用者決定を実装の前提とし、同じ事項を再確認しない。

| 決定 | 初版の扱い |
| --- | --- |
| その他のフォルダ | 追加しない。固定登録先だけ。 |
| 対象外混在 | 理由付き全体拒否。実項目だけ続行しない。 |
| 評価・タグ・編集 | 整理操作では移行／削除せず、先の通常フォルダで既存の編集内容復元に任せる。記録／対象条件を§6.3のとおり説明する。 |
| 入口 | 右クリックの1項目＋既定キーなしKeyActionだけ。メニューバー・ツールバーには追加しない。 |
| 完了後更新／レビュー1〜5 | 現在の元／先の実フォルダへ既存外部変更再確認だけ。保持一覧、検索、チェック、書庫、コレクション、別窓、他画面は外部移動相当。専用後始末なし。 |
| Remote・終了／レビュー6 | 投入直前一か所で拒否し選択要求を破棄。開始後は止めない。PC使用中のRemote接続は例外として割り切り、二重投入はrequest enumだけで防ぐ。 |

まれな失敗の推奨案は引き続き小さく扱う。設定保存失敗は既存DBを消さずログ＋通知し、そのセッションのメモリ値を使う（再起動で今回の編集が残らない可能性は伝える）。この点は利用者作成設定なので、永続保証が必要と判断された場合だけ追加仕組みを作る前に相談する。整理先消失／未接続は通知して何もしない。Shellの中断／部分失敗はWindowsの画面に任せ、突然終了／再走査失敗は通常の更新・開き直しに任せる。開いたmediaのhandle競合も外部移動と同じShellの使用中表示に従い、viewerを先に閉じたり独自再試行したりしない。

改訂時のソース確認では、採用する非同期Shell実行＋既存外部再確認により、リリース済みのエクスプローラー移動より悪いクラッシュ・利用データ消失・UI停止を起こすと確認できる経路は見つからなかった。これは設計時のソース確認であり、実装後の自動検証と実機確認は別に報告する。実装で新たな経路が見つかった場合だけ報告し、今回除いた保持状態の整合機構を自動的に戻さない。

## 9. 利用者の実機確認シナリオ (独立レビュー補足 P2)

使い捨ての元フォルダと整理先を用意し、画像ファイルと子フォルダを置く。製品バイナリは利用者自身が起動する。

1. 環境設定で先を登録し、名称・パスの変更、↑↓、削除を試す。OK 後の開き直しと再起動で保持され、キャンセルした編集は反映されない。
2. ファイルと子フォルダを同時にチェックしてコピー／移動する。コピーは元を残し、移動は元を消し、先にファイルとフォルダ内容が届く。元の一覧に留まり、既存の外部変更更新に従う。
3. 元と先に同名ファイルを異なる内容で作り、Windows の競合画面で置換・スキップをそれぞれ選ぶ。先に同名フォルダも作り、結合／競合確認が Windows の判断に従うことを確認する。
4. 大きな使い捨てファイル群を別ドライブへコピーし、Windows の進捗画面で取消する。完了済みの結果は巻き戻さず、独自の進捗・取消画面を重ねず、元／先の現在の実フォルダだけ既存更新で確認できる。
5. 競合・進捗・エラー画面がメインウィンドウを owner として前面に出て、終了後に操作できることを確認する。移動／コピーのボタン間隔と focus 表示、矢印で明示選択後の Enter、Escape、名称／パス欄の日本語 IME 確定・取消も確認する。
6. 作業前に任意の文字列をクリップボードへ置き、コピー／移動後にメモ帳へ貼り付けて同じ文字列であることを確認する。続けて Explorer で使い捨てファイルをコピーし、そのファイルのクリップボード内容も整理操作で変わらないことを確認する。
7. 画像と ZIP/PDF 内ページ等を混在チェックし、全体が理由付きで拒否され実ファイルだけ処理されないことを確認する。不在の登録先も登録は残り、実行時に通知される。
8. 復元確認が ON の状態で、★・タグ・編集を記録済みの画像を整理し、先の通常フォルダを開く。「編集内容の復元」の一括確認から復元し、先の既存編集を上書きしないことを確認する。OFF・未記録・Folder 自身・動画・音声には同じ復元を期待しない。

## 10. 実装・検証の記録 (2026-10-04)

変更ファイルは次のとおり。コミットと製品バイナリの起動は行わず、承認済み vendor／testdata は変更しない。

- 設定と環境設定: `src/settings.rs`、`src/ui_dialogs/preferences.rs`、`src/ui_dialogs/preferences/pages.rs`、`src/ui_dialogs/preferences/search_index.rs`。
- 選択・実行・完了: 新規 `src/ui_dialogs/file_organize.rs`、`src/ui_dialogs/mod.rs`、`src/app.rs`、`src/shell_file_ops.rs`。
- 入口のインベントリ: `src/context_menu_model.rs`、`src/ui_dialogs/context_menu.rs`、`src/keymap.rs`。
- 自動検証と画像: `src/app/tests.rs`、`src/lib.rs`、`tests/ui_snapshot.rs`。新規画像は `tests/snapshots/file_organize_destinations_light.png`、`file_organize_empty_dark.png`、`preferences_file_organize_light.png`、`preferences_file_organize_dark.png`。追加行を含む既存画像 `preferences_folder_edit_restore.png`、`preferences_context_menu_layout.png`、`preferences_context_menu_layout_open_with.png` も更新・目視確認する。
- 技術文書: 本書、`docs/README.md`、`docs/spec.md`、`docs/keymap-spec.md`、`docs/keymap.ini.default`、`docs/item-kind-capability-matrix.md`、`docs/next-release-backlog.md`。
- マニュアル: `htdocs/mimageviewer/manual/settings.html`、`grid.html`、`tut-file-ops.html`、`shortcuts.html`。

独立した実装レビューでは P1 なし。入口のキー repeat と、上下移動だけで Enter を確定させない点を修正し、追確認で未解決の P1/P2 なし。payload の Box 化と対象一覧仮想化の追確認も前提矛盾・P1/P2 なし。Cut／Copy の先頭配置は維持する。対象の展開表示は仮想化し、選択時の全パス snapshot と順序は保持する。Shell UI の競合・取消・owner/focus・クリップボード不変は §9 の利用者実機確認として残す。

実装時に確認した境界 (以下は実装後の行番号)：

| 前提 | 根拠 |
| --- | --- |
| 選択解決・混在拒否と、一要求の所有権 | `src/ui_dialogs/file_organize.rs:13`、`:33`、`:72`。選択と確定の snapshot はこの enum の payload だけに置く。 |
| 投入直前一か所で Remote／終了を拒否 | `src/ui_dialogs/file_organize.rs:247`。Remote の既存所有判定は `src/remote_ipc/ui.rs:977`。tray quit と root close は同じ投入境界で判定する。 |
| 整理対象／整理先の検証と Shell は worker 内 | `src/shell_file_ops.rs:27` の worker 内で `:70` のパス検証と `:150` の STA 処理を行い、`:201`／`:209` で全件予約、`:219` で実行する。完了後の既存再確認には §2 記載の親 metadata 確認が残るが、新しい全件 I/O／走査は UI に追加しない。 |
| 完了で現在の実フォルダに既存再確認だけ | `src/ui_dialogs/file_organize.rs:287` → `src/app/content_identity_detection.rs:125` の実フォルダ判定 → `src/app.rs:22117` の既存外部変更確認。再走査 worker は `src/app.rs:22165`。 |
| 環境設定の本番 OK・既存保存を通す | `src/ui_dialogs/preferences.rs:1944` の helper を `:2473` の本番 OK と `:3619` の通し試験から使用。`src/settings.rs:9959` の `save_checked` と `settings.db` の既存 `settings_kv` を使用する。 |
| 復元は既存機能の記録／対象条件に従う | `src/app/content_identity_detection.rs:147` の OFF gate、`:238` の遡り記録選別、`:411` の既存編集 presence。整理専用のハッシュ・待機・metadata 移行はない。 |

実 Shell テスト `shell_transfer_real_copy_and_move_file_and_folder` は、実装担当 (Codex) は未実行。設計担当 (ClaudeCode) が 2026-10-04 に実行し、1 passed (exit 0)。製品バイナリは起動していない。同名競合・取消・owner/focus・クリップボード不変は利用者の実機確認で、未確認。使い捨て temp データだけを作り、環境により Windows の画面が出るため明示実行の `#[ignore]` は維持する。通常の設定や製品プロセスは使用しない。

```powershell
cargo test -p mimageviewer --lib shell_transfer_real_copy_and_move_file_and_folder -- --ignored
```

初回実装完了時の自動検証 (exit code はコマンド終端の値)。追加レビュー修正後の追試は §11 に記録する：

| コマンド | exit code／結果 |
| --- | --- |
| `cargo fmt`、`cargo fmt --check` | 各 0 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 |
| `cargo test -p mimageviewer --lib file_organize` | 0、8 件成功。設定 DB 往復／Cancel、全体拒否、投入 gate、二重投入、Enter の明示選択、既存再確認を含む。 |
| `cargo test -p mimageviewer --lib shell_transfer` | 0、5 件成功・実 Shell 1 件 ignore。実行したのはパス検証だけ。 |
| `cargo test -p mimageviewer --lib keymap::tests` | 0、151 件成功。参照 ini の生成一致も含む。 |
| `cargo test -p mimageviewer --lib context_menu_model::tests` | 0、32 件成功。ID／ALL／旧カスタマイズの新項目補完を含む。 |
| `cargo test -p mimageviewer --lib history_transition_storage_keeps_app_stack_footprint_bounded -- --nocapture` | 0、1 件成功。App 110,024 バイト、request は `2 * usize` 以下。 |
| `cargo test --test ui_snapshot` | 0、65 件成功。新規 4 枚と更新した既存 3 枚を目視確認済み。 |
| `python scripts/check_ui_glyphs.py` | 0、危険 glyph なし。 |
| `git diff --check` | 0 |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 0、最終版の通常 profile core／Remote service／EPUB PDF worker をビルド。VCRT／PE 検査 runtime 4・PE 3 成功。成果物は起動せず利用者へ渡す。 |
| `cargo test --workspace --exclude mimageviewer-launcher --features pack-build-tools --no-fail-fast` | 0。最終版の全体試験成功 (メイン lib 10,234 件成功・52 件 ignore、関連 integration／snapshot／Remote／各 crate／doctest 成功)。launcher の埋め込み成果物不足だけを除いた補足試験であり、下記の通常全体 gate 成功とは扱わない。 |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | 1。launcher の build.rs が要求する `target/release/mimageviewer-core.exe`、`mimageviewer-remote.exe`、`mimageviewer-epub-pdf.exe` がなく、全体 gate は完了できない。承認済み vendor の検査は成功。代用品・stub の配置や release 工程の変更は行わない。 |

初回の launcher を除いた全体試験は exit 101 で、App サイズ上限と、追加項目を含む既存環境設定画像 3 枚の差分が失敗になった。Box 化と小幅上限調整、意図した画像の更新・目視確認で修正し、サイズ／関連 8 件／画像比較と、最終版の全体再試験は上記のとおり成功。無関係なテストや機能は削除していない。最終 log は `target/file-organize-workspace-final.log`、check は `target/file-organize-final-check.log`、関連 8 件は `target/file-organize-final-focused.log`、画像比較は `target/file-organize-ui-snapshot-final.log` に保存。製品バイナリは起動していない。

利用者用の起動コマンド:

```powershell
Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe
```

引数なしでは実利用中の `%APPDATA%\mimageviewer` を使い、設定・データを更新し得る。single-instance mutex を共有するため、インストール版／常駐 tray 版を終了してから利用者自身が起動する。§9 の確認は使い捨ての整理対象と整理先で行う。

## 11. 追加レビュー P2/P3 の修正・追試 (2026-10-04)

- P2: `src/shell_file_ops.rs:117` の同じ親への Move 判定は `canonicalize(source.parent())` を使う。自己配下判定は `canonicalize(source)` のまま、Shell の元パスも維持する。`:542` の junction 試験で、本体の同一親 Move、移動可能な別の実ファイルとの混在要求が全体拒否されることを確認する。リンク先の親への Move は許可し、元パスが junction 本体のままであることも検証する。
- P3: `src/ui_dialogs/preferences.rs:3692` の試験は、既存 `settings_db::set_save_suppressed(true)` と headless の本番 OK ボタン入力を使用する。適用済みメモリ値と開き直した draft の保持、既存 DB の全設定値不変、保存世代不変、失敗通知、抑止状態の維持を確認する。保存抑止は既存 App fixture の RAII／直列化ロックで隔離し、再試行・回復・新しい状態や本番 helper は追加しない。
- ignore 付き実 Shell テストは設計担当 (ClaudeCode) が実行し成功 (上記 §10)。利用者の実機確認は未実施。

今回追加で変更したファイルは `src/shell_file_ops.rs`、`src/ui_dialogs/preferences.rs`、本書だけ。初回の UI snapshot 等、今回変えていない経路の成功結果は再利用する。

| コマンド | exit code／結果 |
| --- | --- |
| `cargo fmt`、`cargo fmt --check` | 各 0 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0 |
| `cargo test -p mimageviewer --lib file_organize` | 0、9 件成功 (保存失敗の本番 OK 試験を含む)。 |
| `cargo test -p mimageviewer --lib shell_transfer` | 0、5 件成功・1 件 ignore (追加した junction 検証を含む)。 |
| `cargo test -p mimageviewer --lib ui_dialogs::preferences::tests` | 0、48 件成功。 |
| `python scripts/check_ui_glyphs.py` | 0、危険 glyph なし。 |
| `git diff --check` | 0 |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 0。修正版の確認用 core／Remote service／EPUB PDF worker をビルドし、VCRT／PE 検査 runtime 4・PE 3 成功。成果物は起動していない。 |

各 log は `target/file-organize-review-fixes-{focused,shell,preferences,check}.log`。コミットは行わない。

## 12. §1.263 追補: 整理先を開く (2026-10-05)

対象: `C:\home\mimageviewer-epubroot` / `next-organize-location`、base `4a4e75fd7`。

利用者決定: フォルダバー「場所▼」に「整理先 ▶」を追加する。登録順の名称とパス tooltip を
表示し、登録が空ならグループごと非表示。既定 ON の
`Settings.show_location_file_organize_destinations` で両 UI の表示を切り替える。
現行の場所表示項目は環境設定ではなくフォルダバー／場所▼の右クリックにあるため、
他の `show_location_*` と同じ「場所▼に出す項目」へ追加する。指定の環境設定にも、
「フォルダ・ファイル」のファイル整理先欄へ同じ表示チェックを追加し、OK／Cancel に従わせる。
ツールバーの既定に戻す操作も対象。
未リリースの機能なので設定移行は不要。serde 欠落時は true。§1.317 の環境設定転送 gate では
他の `show_location_*` と同じ表示／ツールバー状態として除外する。環境設定でも編集するため、
このフィールドは `overwrite_non_preferences_from` で live 値を上書きせず draft 値を確定する。

一覧の唯一の正本は `known_folders::LocationMenuEntry::FileOrganizeDestinations`。
ローカルの選択は他の場所項目と同じ `resolve_folder_bar_nav_path` → `AddressBarNav::Direct` の
同期解決を使う。到達不能なネットワーク先では OS が諦めるまで UI をブロックし得る
(2026-10-06 利用者了承、判断理由と既知例外は §13)。履歴、戻る stack、viewer context、
検索／★固定の入口 gate は既存場所移動に揃える。
存在しない先も登録から消さず、他の場所項目と同じ解決・移動経路に委ねる。
Remote Home は同じグループを `PlaceSummary` へ写像し、通常フォルダ route で開く。
Remote IPC は 65 → 66、両 exe は共有 crate の同一定数を参照して一緒にビルドする。
コピー・移動の実行、clipboard、ファイル変更の後始末には変更を加えない。

既存の単発 navigation と Home 更新を再利用するので、追加の非同期 owner、live rebuild、
rollback／resume は不要。表示切替は列挙時に読むだけで、menu 描画には登録先の存在確認を足さない。

場所 UI の PNG は次の 2 枚を新規追加し、どちらも目視確認した。

- `folder_bar_organize_destinations.png`: 本棚の後の「整理先 ▶」と、登録順の「要確認」「保管」を
  開いた状態。通常の場所メニューの行、親メニューとの配置、名称の読みやすさを確認する。
- `folder_bar_organize_destinations_toggle.png`: フォルダバー設定の「場所▼に出す項目」に
  既定 ON の「整理先」が入った状態。前後の既存チェックと設定項目が欠けずに収まることを確認する。

環境設定は次の既存 3 枚を更新し、すべて目視確認した。

- `preferences_file_organize_dark.png`: 暗色の整理先登録欄に表示チェックを追加。登録済み 2 行の
  名称・パス・参照／並べ替え／削除と追加ボタンが引き続き表示され、チェックの文字が欠けない。
- `preferences_file_organize_light.png`: 同じ登録欄の明色表示。チェック、名称・パス入力欄、操作の
  コントラストと配置が保たれている。
- `preferences_folder_edit_restore.png`: フォルダ・ファイルページ先頭へ表示チェックを追加したため、
  以降のセクションが 1 行分下へ移る。既存の編集内容復元／削除確認と、右端のスクロール領域が残る。

自動検証 (この未コミット差分、製品起動・コミットなし):

| コマンド | exit code／結果 |
| --- | --- |
| `cargo test -p mimageviewer --lib file_organize_destinations_location` | 0、3 件。共有一覧の空／OFF／順序、実メニューの pointer 選択 → `AddressBarNav::Direct`、メニュー閉じ、表示切替／保存値往復／既定リセット。PNG 生成後の比較は全体 gate で確認する。 |
| `cargo test -p mimageviewer --lib known_folders::tests` | 0、11 件。 |
| `cargo test -p mimageviewer --lib remote_ipc::collections::tests` | 0、37 件。名称／登録順／空／OFF、不在先の保持と既存 path guard を含む。 |
| `cargo test -p mimageviewer --lib settings_transfer::tests` | 0、16 件。全 440 設定の分類と除外値の保持。 |
| `cargo test -p mimageviewer-ipc --lib` | 0、64 件。Home の新グループと typed folder の JSON 往復、protocol 66 と不一致拒否を含む。 |
| `cargo test -p mimageviewer-remote --bin mimageviewer-remote` | 0、134 件成功・1 件 ignore。 |
| Web の `*.test.mjs` 10 ファイルを各 `node <file>` で実行 | 0、478 件。新グループの表示順・名称・パス tooltip、通常 folder と同じ route／戻り先を含む。sandbox では `node --test` の子 runner が `spawn EPERM` になるため、同じ `node:test` をファイル直接実行した。 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0。 |

環境設定の表示チェック追加後の関連試験／全体 gate／normal・portable check／
確認用 build の結果は下へ記録する。
利用者確認: 2 つの使い捨て整理先を登録し、場所▼ → 整理先で順序・tooltip・移動と戻る／進むを確認。
表示 OFF／登録なしでローカルと Remote Home の両方から消えることを確認する。
Remote はホーム更新後の名称・順序、tap によるフォルダ表示と Home へ戻る、不在先の既存エラーを確認する。

全体 gate の初回はコンパイル段階で exit 101。`rhai_codegen` の E0462 と `url`、`image`、
`tantivy` 等の rlib 不足で integration target を作れなかった。UI／テスト期待値で回避せず、
実体の `target/debug` がこの worktree 内で reparse point でないことを確認し、
`cargo clean -p mimageviewer --profile dev` と、不足報告のあった依存 package の同じ profile の
clean を行って再構築した。対象は生成済み dev/test cache のみで、`dev-runtime`／release と
検証 log、通常 APPDATA は維持した。最終追試は次のとおり成功。

| 最終差分の追試 | exit code／結果 |
| --- | --- |
| `cargo test -p mimageviewer --lib file_organize` | 0、17 件。環境設定の表示 OFF の OK → 保存 → DB 再読込、開き直し、Cancel で保存値を変えない経路を含む。 |
| `cargo test -p mimageviewer --lib ui_dialogs::preferences::tests` | 0、57 件。 |
| `cargo test -p mimageviewer --test ui_snapshot` | 0、88 件。上記の 3 枚だけを更新し、新規 2 枚を含め目視確認した。 |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | 0、PASS。本体 lib 10,603 件成功・52 件 ignore、integration／UI snapshot 88 件／Remote／各 crate／doctest、vendor egui・egui-wgpu・eframe の全必須試験成功。PNG 比較は UPDATE フラグなし。 |
| `cargo check -p mimageviewer --bin mimageviewer-core` | 0。 |
| `cargo check -p mimageviewer --bin mimageviewer-core --features portable` | 0。 |
| `cargo fmt --check` / `python scripts/check_ui_glyphs.py` | 0、UI glyph の問題 0 件。 |
| `git diff --check` | 0。 |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | 成功、DONE。normal feature set の core・Remote・EPUB worker を生成。同梱 runtime 4 件と PE 3 件の依存検査成功。起動・常駐アプリ停止は行っていない。 |

検証 log は `target/orgloc-*.log`。全体 gate の最終 log は `target/orgloc-full-final.log`。
確認用 build は `target/orgloc-build-dev.log`、依存検査は
`target/vcrt-pe-reports/dev-runtime.json`。通常 profile を使うため、利用者が起動する際は
インストール済み／tray 常駐の mImageViewer を先に終了する (single-instance mutex を共有)。
`Start-Process -FilePath .\target\dev-runtime\mimageviewer-core.exe` は通常の
`%APPDATA%\mimageviewer` を使用し、実設定・データを更新し得る。

## 13. 同期選択へ戻す利用者決定と P2 修正の保持 (2026-10-06)

対象: `next-organize-location-sync` / `1c49e9efa` の follow-up。
`d897257c6` の未コミット cherry-pick から、ローカルの場所選択を変更する部分を取り除く。
後続の失効・モーダル・終了境界の案はこの branch に取り込まない。

利用者決定: 整理先・QuickLocation・drive の選択解決は、元の Desktop / drive と同じ
`resolve_folder_bar_nav_path` → `AddressBarNav::Direct` の同期経路へ戻す。
非同期／モーダル案では後続移動、先行検索、native input と背景完了の境界についてレビュー指摘が
繰り返されたため、利用者は元の単純な振る舞いを選んだ。到達不能なネットワーク先は OS が諦める
まで UI がブロックされ得ることを明示的に了承した。これを ui-responsiveness §4 の既知例外に
記録する。停止時間・描画・応答の実機観測を主張するものではない。
場所選択専用の非同期要求、取消・確認 UI、入力遮断、背景完了保留は設けない。
本／書庫を開く既存の分類・変換など、他の通常ナビ処理は変更しない。

残す変更は次の 2 点:

1. Remote Home の整理先は `RemoteEntry` に登録名・パスをそのまま格納する。
   canonicalize / 存在確認をせず、開く要求に既存の存在・種別・Remote path guard を委ねる。
   登録順・不在先・名称・パス・wire shape・protocol 66 は維持する。
2. 整理先 submenu は画面高と 400px で制限した縦 ScrollArea を使う。
   開いている frame は描画後に raw / smooth delta と MouseWheel event を消費し、背面一覧へ通さない。
   100 件の末尾を実ホイールで表示し、通常の `Direct` として選択する headless test を維持する。

QuickLocation / DriveRoot の callback と App の通常 nav handler は初版と同じコードへ戻す。
organize destinations も同じ同期 helper を使い、親 fallback と履歴・戻る／進むを通常ナビへ委ねる。
`LocationMenuEntry` は引き続き両 UI の単一の正本。表示設定・§1.317 の分類・設定移行方針は維持する。
この機能は未公開のため settings migration は不要。変更は detached 述語や viewport 所有へ触れない。

PNG は cherry-pick の `folder_bar_organize_destinations_scrolled.png` を保持する。
100 件の末尾 (084〜099)、高さ制限、solid scrollbar の余白、名前の一行表示を示す画像であり、
同期解決へ戻す操作結果は描画を変えない。既存 menu / Preferences PNG も比較する。
製品起動・実機ネットワーク確認は行わず、検証 build と手動確認を利用者へ引き継ぐ。

手動確認: 多件数 submenu の末尾を選び、フォルダ表示・戻る／進む・tooltip を確認する。
整理先／Desktop 等／drive が通常の同期選択で動くこと、Remote Home が登録名・パスを保ち、
整理先を開くことを確認する。ネットワーク切断先の選択には UI 停止の既知制約がある。

自動検証 (最終未コミット差分、2026-10-06。Cargo は `CARGO_BUILD_JOBS=1`):

| command / 条件 | 結果 / log (`target/`) |
| --- | --- |
| `cargo test -p mimageviewer --lib file_organize_destinations` | exit 0、9 成功。即時 Direct の現在地・履歴、空／OFF／順序、Remote の登録パス保持、末尾選択・wheel 消費・menu／表示設定 PNG 比較。`orgloc6-narrow.log` |
| `cargo test -p mimageviewer --lib remote_ipc::collections::tests` | exit 0、37 成功。`orgloc6-remote-collections.log` |
| `cargo check -p mimageviewer --bin mimageviewer-core` | exit 0、normal。`orgloc6-normal-check.log` |
| 上記 check に `--features portable` | exit 0。`orgloc6-portable-check.log` |
| `cargo fmt --all -- --check` / `git -c core.safecrlf=false diff HEAD --check` / `python scripts/check_ui_glyphs.py` | 全て exit 0、危険 glyph 0。 |
| `.\scripts\test-full.ps1 -SuppressCrashDialogs` | exit 0、PASS。本体 10,605 成功・52 ignored、UI integration snapshot 88、Remote IPC 64、Remote-web 134 成功・1 ignored、vendor egui / egui-wgpu / eframe は 25 / 9 / 18 成功。その他 workspace・integration・doc test も成功。`orgloc6-full.log` |
| `.\scripts\build-dev.ps1 -PreserveRuntime` | exit 0、DONE。normal feature set の core・Remote service・EPUB PDF worker を生成。`orgloc6-build-dev.log` |

PNG 比較は期待画像を更新せずに実施した。新規 scrolled PNG の目視確認も行った。
製品起動・実機ネットワーク確認・commit は行っていない。
共有 Git 管理領域への書込制限で index の復元はできなかったため、commit 担当は最終作業ファイルを
再 stage する必要がある。検証は HEAD と index の差ではなく、最終作業ツリーに対して行った。
