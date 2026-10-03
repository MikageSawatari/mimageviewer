# §1.263 固定の「ファイル整理先」へコピー・移動する設計案

2026-10-04。対象 worktree: `C:\home\mimageviewer-fileops`、branch: `next-file-ops`。
要件の正本は [next-release-backlog.md §1.263](next-release-backlog.md#1263-固定のファイル整理先へ選択項目をコピー移動する--444447-2026-09-21)。
本書は設計担当へ返す提案であり、実装済み・設計合意済みではない。この段では本書だけを作成し、製品コード、他の文書、コミット、実アプリ起動は扱わない。

## 1. 推奨する構成と不変条件

お気に入りとは別の `Settings.file_organize_destinations` を環境設定で編集し、グリッドの右クリックと既定キーなしの `KeyAction` から同じ整理先選択画面を開く。各行の「移動」「コピー」から Windows Shell に直接依頼する。クリップボードとお気に入りを変更せず、整理先へ一覧を移動しない。

既存の `shell_file_ops.rs` にある STA・`IFileOperation`・channel による非同期実行の形を拡張する。コピー／移動そのもの、競合確認、進捗、取消、アクセス拒否、部分失敗は Shell が担当する。mIV 専用の転送エンジン、常設 worker、進捗集計、失敗分類、再試行、ロールバック、操作ジャーナルは作らない。

- 対象は入口で確定した実ファイル／実フォルダ全件。カーソルとチェックを実行時に取り直さない。
- ZIP/PDF 内ページや合成セルを実ファイル、代表画像、外側コンテナへ勝手に置き換えない。
- ファイルとフォルダの混在は全件を渡す。対象外が混在するときも、黙って実項目だけ実行しない。
- 完了結果で開始時の item index を操作しない。実パスで現在の一覧に引き直し、別一覧の選択を奪わない。
- 部分取消／失敗でも、実際に変わったファイルシステムを再同期する。全件成功と推測しない。
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
| 消失項目と選択の再整合 | `src/app.rs:38092` の `remove_items_batch` は item / thumbnail / metadata をまとめて詰め、index を移し、消えたカーソルは次の残存項目、末尾なら末尾へ寄せる (`同:38243`)。サブ展開／smart snapshot も更新する (`同:38139`)。削除の DB purge 部分と切り離して再利用できる。 |
| 合成一覧の参照失効 | `src/app/subfolder_expansion.rs:2156`、`src/app/smart_folder.rs:8992` は消失 path の snapshot 除去。`src/app/collection_grid.rs:888` は Exact / Tree scope で全 viewer context の該当 source presentation を失効させる。コレクションの登録行を削除する処理ではない。 |
| 閲覧中の同一実体 | `src/app.rs:38530` の `release_viewer_surfaces_for_removed_paths` は該当 path／配下を参照する viewer だけ既存 close 経路へ渡す。無関係な窓を一括終了する入口ではない。 |
| 対象種別 | `src/grid_item.rs:375` の `drag_source_path` は Folder / Image / Video / Audio / ZipFile / PdfFile / ConvertibleArchive の実パスだけ返す。`file_operation_path` (`同:354`) は Folder を含まず、こちらだけでは混在集合を狭めてしまう。 |
| 対象外理由 | `src/grid_item.rs:395` の `file_operation_refusal` が理由の正本。ZipImage / PdfPage、ZipDir、Stack、SearchContainer、CollectionPlaceholder を区別する。Stack の代表画像、SearchContainer の表示用 path は整理対象の実パスではない。 |
| お気に入り編集 UI | `src/ui_dialogs/favorites_editor.rs:352` は即時反映／保存で OK/Cancel がない。IME 名称欄 (`同:748`)、↑↓／削除 (`同:878`)、Vec swap (`同:1191`)、save (`同:1242`) の UI パターンだけ参考にする。索引・UUID・お気に入り標準設定の副作用は持ち込まない。 |
| 環境設定の確定 | `src/ui_dialogs/preferences.rs:2090` が一時コピーを作り、`同:1872` が OK 前に `overwrite_non_preferences_from` を通し、install (`同:1959`) と save (`同:2592`) を行う。お気に入りは non-preference として live から移される (`src/settings.rs:9822`)。整理先は環境設定の編集対象なのでこの移送に追加してはならない。 |
| 保存 | `src/settings.rs:9892` の save、`同:9897` の `save_checked`、`同:9926` の共通書込境界。既存保存は同期で、save は戻り値を捨て、失敗はログ (`同:9981`)。`src/settings_db.rs:12` と `同:892` の `save_full` は専用テーブルに分離しないフィールドを settings_kv の JSON 値として保存できる。新 DB／新テーブルは不要。 |

現行の「一覧が仮想か」を一律に拒否するのは誤り。検索、タグ、★、履歴、smart、サブ展開、コレクションでも実 `GridItem` は対象にできる。`current_favorite_target()` (`src/app.rs:21920`) は現在地を貼り付け先にするための判定で、今回の source 判定には使わない。表示用 `display_path` や `container_path` で実パス可否を代用しない。

## 3. 整理先の設定と環境設定 UI

### 3.1 データ・既定値

提案名は `FileOrganizeDestination { name: String, path: PathBuf }`、`Settings.file_organize_destinations: Vec<FileOrganizeDestination>`。順序は Vec の順そのものとし、別の order 値、UUID、favorite ID、索引フラグは持たない。`#[serde(default)]` と `Settings::default()` は空 Vec。5 件は想定用途であり、5 件の固定スロット制限にはしない。

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

保存失敗は既存 `save_checked` を利用してログ＋通知する案。適用済みのメモリ値はそのセッションで使い、「保存できませんでした。再起動すると今回の変更が残らない可能性があります」と示す。自動再試行や旧設定へのロールバックは足さない。これは利用者が編集した設定を失い得るため、§8 で判断を求める。既存の保存抑止／互換性保護を迂回しない。

## 4. 整理先選択ダイアログと入力

### 4.1 入口・対象

グリッドの項目右クリックに静的 leaf「ファイル整理先…」を一つ追加する。整理先をメニューへ全列挙しない。背景、ツリー、画像／動画 fullscreen、detached の viewer メニューには新規入口を加えない。チェック優先、なければ右クリック項目／キーのカーソルを使い、同じ要求生成メソッドへ渡す。

`context_menu_model` の `MenuCommand`、stable `ContextMenuItemId`、ALL、文字列名、label、command→item 対応、可否、shortcut label、preview を揃える。native HMENU と egui fallback は同じ MenuNode を使う。§1.221 の右クリックカスタマイズで新 leaf の表示・並び・区切りが扱えるようにする。既存レイアウトに未知だった ID が加わっても既定位置に出ることを確認する。

全件の path／種別／表示名を入口時に捕捉する。対象外は `file_operation_refusal().message("移動 / コピー")` を使い、一覧上だけの項目を識別して理由を示す。**初版の推奨は既存 Copy/Cut と同じ全体拒否**：混在時は「対象外があるため実行しません。該当項目のチェックを外してください」と対象名・理由を表示する。実項目だけ続行する分岐は作らない。実ファイル＋実フォルダの混在は拒否しない。右クリックとキーのどちらも理由を通知でき、単なる無反応にしない。

### 4.2 表示と操作

既存 modal の規約に従う整理先選択画面に、対象件数と対象名（多い場合は折畳みの対象一覧）、次の表を表示する。

| 登録名 | パス | 操作 |
| --- | --- | --- |
| 保管 | `D:\写真\保管` | 移動　　コピー |
| 要確認 | `D:\写真\要確認` | 移動　　コピー |

登録順で並び、各行のボタン一回で選んだ操作を開始する。余計な mIV 確認は重ねず、競合確認は Windows に任せる。「移動」と「コピー」は十分に間隔を取り、色だけに頼らず文字・focus 枠で識別する。長いパスを確認でき、行の高さで他行のボタンと取り違えない。下部に「閉じる」。登録なしなら説明と「環境設定で登録…」を表示する。

選択画面の表示中だけ `common_modal_dialog_open / modal_dialog_block_reason` に含め、背面へのキーと pointer の漏れを防ぐ。登録画面へのリンクは選択画面を閉じ、対象 snapshot を破棄して環境設定を開く。戻るための再開状態や画面の live rebuild は作らない。整理先の行は開いた時の設定 snapshot とし、行 index から後で live 設定を引き直さない。

### 4.3 KeyAction と IME

提案 Action は `GridOrganizeFiles`、ini 名は同名、説明「選択中またはチェック済みの実ファイル／実フォルダの整理先を選ぶ」。`KeyContext::Grid`、`KeyTrigger::Press`、既定 chord は空、repeat で再起動しない。

`src/keymap.rs` の enum / `ALL_ACTIONS` / `ini_name()` / `description()` / `context()` / `trigger()` / `default_chords()` と Grid handler を揃える。既存 Grid の keyboard owner、IME、text focus、modal、remote ownership の gate を保って `consume_action` で共有入口を呼ぶ。`command_catalog()` (`src/keymap.rs:2535`) から操作カスタマイズ、検索、競合表示、文脈ヘルプへ届くことを確認する。固定先ごとの動的 Action、直接移動／コピー Action、native 動画 VK 転送、RingAction は初版に追加しない。

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
| 開始時 snapshot、転送中は従来の閲覧を許可 | clipboard 操作と同じく、移動先を開かず閲覧を続けられる。 | path による完了反映と既存 watch／snapshot 更新を再利用する必要がある。開始時 index に戻す必要はなく、rollback／resume は不要。 |

**推奨は snapshot**。本機能は設定確定や本を開く変換と違い、転送先が現在の viewer 状態に依存しない。既存の外部変更再同期も、別フォルダへ移った結果を捨てる形を持つ。全体 modal の新しい窓横断排他を足すより小さく、長時間コピー中の既存閲覧を制限しない。選択画面は modal、Shell 実行は非同期と分ける。Shell が自分の確認画面により owner の入力を止める期間は Windows に従う。

App が持つ新しい状態は、この機能の一つの request owner（非表示／選択中／実行中を表す enum）だけにする。選択中は対象と整理先リスト、実行中は確定要求と receiver を所有する。show bool、pending Option、対象別 pending の組合せを増やさない。実行中に同じ機能を二重投入した場合は通知して既存要求を維持する。他機能の既存状態機械をこの段で統合し直さない。

### 5.2 Shell への直接依頼

1. 行のボタンで操作種別と整理先を確定し、捕捉済みの対象・整理先 path・main owner HWND を worker へ渡す。入口の viewer context ID は要求の来歴として保持し、window ID や開始時 index を UI 結果適用先の代用にしない。HWND がなければ通知して開始しない。選択画面を閉じ、転送中は mIV の独自進捗／取消画面を重ねない。
2. `shell_file_ops.rs` の既存 async 呼出しの形を拡張する。COM／Shell item の構築、パス検証、重い I/O、`PerformOperations` は STA worker の中だけ。UI から同期 join／待機を行わない。一般化は STA guard 等の必要な共通部分に留め、delete worker の purge／retry を統合しない。
3. 整理先を実ディレクトリとして確認し、全対象を queue できることを確認してから `CopyItem / MoveItem` を一つの `IFileOperation` に予約し、`PerformOperations` を一回呼ぶ。予約段階で失敗したら実行せず通知する。クリップボードを用いた cut→paste や PowerShell コピーには迂回しない。
4. リネームと同じ owner／undo 用フラグを基礎にし、競合・進捗・エラー UI を抑止する `FOF_NOCONFIRMATION / FOF_NOERRORUI / FOF_SILENT` を設定しない。自動上書き／自動改名を mIV の方針にしない。
5. `PerformOperations` の戻り値だけで全件成功としない。`GetAnyOperationsAborted` と Shell の実結果を尊重し、取消／失敗時も完了処理へ進める。受信と repaint 起床は既存 worker/channel の形を使い、全フレーム高速 repaint で待たない。

`IFileOperation` は STA のみで使用でき、標準の進捗・エラーダイアログを持つ。根拠: [Microsoft Learn: IFileOperation](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nn-shobjidl_core-ifileoperation)。

最小の結果は「操作終端の成否／中断」と「移動後に存在しないことを worker が確認できた元 path」。移動元の存在確認は worker 上で行い、不明なものは一覧から消さない。**これは移動成功の分類ではなく、一覧から取り除ける実体の確認**で、消えた path の DB purge は行わない。初版では per-file のエラー表、独自成功件数、Shell の競合後名の推測を作らない。

フォルダ結合で親が残った場合は親全体を消失扱いにしない。中の一部だけ移った場合も表示中の該当実フォルダは既存再走査へ渡す。snapshot 一覧に残る子項目の整合のため、要求した Folder 配下の**保持中の表示／復元 snapshot に含まれる実パス**も完了時の worker 確認対象とする。再帰的にディスク全体を列挙せず、既存の在メモリ path を使う。もし既存 snapshot の所有境界からこの対象を小さく取得できないなら、§8 の境界として設計担当へ戻し、独自の snapshot 回復管理を先に作らない。

item ごとの実移動結果が必要と判明した場合、Shell の `PostMoveItem` 通知を最小限読む選択肢はあるが、初版に先取りしない。通知には衝突後の実名と実結果があり、queue の戻り値とは異なる。根拠: [Microsoft Learn: PostMoveItem](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperationprogresssink-postmoveitem)。これはエラー分類を自作する理由にはならない。

### 5.3 パス条件

| 条件 | 初版の扱い |
| --- | --- |
| 整理先消失・ドライブ未接続・実フォルダでない | worker の開始前検証でログ＋通知、操作を行わない。設定行を削除せず、フォルダを自動作成しない。検証後に消えたら Shell の標準エラーに従う。 |
| 対象の消失／予約不可 | 実行前なら全体を開始せず通知。開始後の外部変更は Shell に任せ、完了時に実際の一覧を再同期する。 |
| 移動先が元の親と同じ | 当該対象名と「同じフォルダなので移動できません」を通知し、要求全体を実行しない。複数元の一部だけが該当しても黙って skip しない。コピーは同じ親でも Shell の同名処理へ渡し、mIV で上書きしない。 |
| Folder 自身／配下へ移動 | 実行前に全体拒否して理由を通知。コピーも自己再帰になる組合せを拒否する。 |
| 大文字小文字、区切り、末尾区切り、`.` / `..`、junction／symlink | worker で実在 path を正規化・解決し、ドライブ／UNC を保持した成分比較で同一／配下を判定する。文字列 prefix だけにしない。解決できなければ通知して開始しない。UI は FS probe しない。 |
| ファイルと Folder、同名 basename、親 Folder と子の同時選択 | 全対象をそのまま Shell に依頼し、集合をファイルだけ／親だけへ縮めない。途中で既に移った子や競合は Shell の標準画面に任せる。 |

`src/path_key.rs:34` のドライブ保持正規化は比較表記の参考になるが、実体解決や `..` の正規化は行わない。`src/folder_tree.rs:1236` の `path_eq` も小文字文字列比較だけなので、自己配下判定の安全性をそれだけで主張しない。検証後の外部変更に対する mIV 独自の監視／再試行は足さず、Shell を最終の実行主体とする。

### 5.4 完了後の再同期・通知

- 成功、途中取消、部分失敗、channel 終了のいずれでも request owner を終端へ戻す。receiver を捨てただけで未実行に戻したり、同じ要求を再投入したりしない。終了をまたぐ再開はなし。起動時は実ファイルシステムの通常読込を正とする。
- 実フォルダは既存 watch／external rescan に乗せる。完了時の明示再確認も既存 worker 再走査入口へ渡し、UI で read_dir しない。完了時に現在見ている実フォルダが変更対象の元／先ならそのフォルダを確認し、無関係な現在地には reload／選択変更を送らない。移動先を開く処理は入れない。
- コピーだけなら整理操作から元一覧の選択・チェックを変更しない。同じ親へのコピーで出力も見える場合は通常 watch が新項目を反映し、整理操作から `AddedSince` の選択要求は新規発行しない。この場合の reload に伴う checked の扱いも既存再同期の仕様に従い、コピー専用の選択復元状態は足さない。
- 移動で消失確認した実パスは main グリッドの context が投影されている完了処理境界で、その時点の一覧へ引き直し、`remove_items_batch` の既存選択規則を使う。残った対象／チェックを整理操作から全解除しない。別の一覧へ移った後も、その一覧に同一の消失実体があれば正当に再整合するが、開始時 index／カーソルを復元しない。別 viewer が一時的に mount されている経路から main の index 操作を行わず、既存の main update の poll 境界に置く (`src/app.rs:84321`)。
- サブ展開／smart の復元 snapshot は既存の path 除去を使う。コレクションは `invalidate_collection_grid_sources` を使い、参照行を消さず missing presentation を既存 prepare へ任せる。検索や★等の snapshot に「成功したはず」と未確認項目を tombstone しない。途中結果と最終結果の順序で消失項目を再導入しないことを回帰試験する。
- 移動済み実体を参照する viewer の解放には、リネーム完了と同じ `release_viewer_surfaces_for_removed_paths` を使う。Shell の取消に備えて実行前に全 viewer を閉じる案は採らない。閲覧 handle が移動を妨げる場合の標準エラーと仕様選択は §8 に記載する。
- 同名競合、権限、skip、取消、部分失敗の説明／選択は Shell の標準画面を唯一の判断 UI にする。mIV は必要時に「操作が中断されました。処理済みの項目は元に戻りません」「操作を完了できませんでした。フォルダの内容を確認してください」程度の終端通知とログを残す。abort は利用者取消と断定しない。[Microsoft Learn: GetAnyOperationsAborted](https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperation-getanyoperationsaborted) は利用者またはシステムによる中断を含む。
- 再走査／消失確認自体が失敗したときは「一覧を確認できません。最新の情報に更新するか、開き直してください」と通知し、確認できた変更だけ反映する。自動 retry／失敗状態の保存／回復の回復は増やさない。

## 6. Remote・detached・保存データへの影響

**mIV Remote**: 新しい整理先 UI／操作／パス配布、IPC、Web UI は追加しない。Remote session 中の PC 操作制限は既存 gate を維持する。ただし PC が移動した実ファイルを Remote が後で参照すれば、通常の外部移動と同様に旧パスが不在となり得る。「Remote に影響なし」は新機能の提供範囲についてで、実ファイルへの副作用がないという意味ではない。

**detached viewer**: 新しい入口、viewport、window runtime 状態、設定の live rebuild は不要。main グリッドからの実ファイル移動が同じ実体を表示する窓に達する点は既存リネーム／削除と同じで、消失確認した対象だけ既存 close／source invalidation に渡す。無関係な context の items、channel、cache、選択は触らない。既存 helper を呼ぶ設計と、detached 述語／viewport 自体を変更する設計を区別し、後者が必要なら [detached-rework-plan.md §2](detached-rework-plan.md#2-憲法-全ステージ共通の不変条件禁止事項-最重要) に従い設計担当・独立 reviewer の合意と §11 の記録を別途行う。本段で他文書へ記録しない。

**path-key の利用データ**: 現行 Shell clipboard 移動はリネームのような mIV DB old→new 移行を呼ばない。本案も初版は同じファイル操作の範囲とし、評価、タグ、ページ編集、ブックマーク、読書位置、コレクション参照の自動複製／移行／purge は加えない。既存データを消さず、移動後の新パスへ追従する保証はしない。この利用者から見える制約は §8 の判断事項であり、保証が必要なら既存 rename migration の適用可能性を別途確認する。フォルダ結合／一部移動を単一 Tree rename と見なす実装は行わない。

**設定の書き出し §1.317**: 整理先のパスは PC 固有として書き出し対象外にする予定。名前／順序だけを取り込んでも操作先を失うため、初版の整理先 Vec 全体を環境設定の可搬 export 対象外にする案を推奨する。import で移行先 PC の既存整理先を消さない。通常の settings.db 保存／既存バックアップとは区別し、§1.317 の実装時に除外表へ明記する。

## 7. テスト計画と実装時の文書更新

この段では試験を実施しない。実装時は以下を確認する。製品起動を伴わない試験を先に行い、ライブ検証は具体的な suite と所要時間を示して利用者の明示了承後に disposable portable で行う。

| 層 | 確認する内容 |
| --- | --- |
| 設定 unit / SQLite 通し | 欠落フィールドが空、名称／Unicode path／順序の保存読込、空リスト、重複、不在 path の維持、既存 favorites／toolbar 不変。 |
| 環境設定 handler / kittest | **編集→OK→開き直し**で追加・名称／パス変更・削除・並べ替えが全て保持される。さらに保存した DB を再読込して同じ結果。キャンセル→開き直しは元の値、全件削除→OK は空。`overwrite_non_preferences_from` を含む実際の OK 入口を通し、model の直代入だけで済ませない。 |
| 設定失敗 | `save_checked` false の注入で通知とメモリ値／既存 DB の扱いを検証。通常 profile を壊して再現しない。auto retry や「保存成功」表示が出ない。 |
| 選択解決 | カーソル1件、checked複数、Folder＋Image＋Video＋Audio＋ZIP/PDF／変換元書庫、右クリック別項目でもchecked優先。各対象外種別と実／仮想混在で理由付き全体拒否、clipboard／ファイル／設定不変。仮想一覧内の実ファイルは可。 |
| パス検証 | 元の親への move、同じ親への copy、Folder自身／配下、隣接prefix `a` と `ab`、別ドライブ同名、UNC、大小文字、区切り、`.` / `..`、junction／symlink、不在先。FS probe と列挙が UI にない。 |
| Keymap | Action インベントリ、生成 default、既定 none、カスタム割当／明示 none／exact modifiers／repeat、command catalog／競合／ヘルプ。text focus、IME、modal、Remote制限で発火しない。入口が同じ対象 snapshot を作る。 |
| 完了 handler（fake Shell） | 1件／複数件の copy / move、全取消、部分変更、terminal error／disconnect。完了前に対象・選択・整理先設定・一覧を変更しても開始時要求は不変。別一覧の同じ index を消さず、取消時も確認済み消失だけ反映し、残ったcheckedを保持。folder結合後に親が残る場合も確認。 |
| context / lifecycle | 開く・一覧切替・履歴/A/B・close・終了・新規要求、sourceと同じ実体を表示するmain/active/passive窓、無関係な窓。source snapshotへの戻り、途中の検索結果差替え、コレクションmissing、smart／サブ展開復元で消失項目が復活しない。取消／copyが無関係なviewerを閉じない。 |
| UI snapshot | 環境設定セクション、登録なし／5件／多数、長い日本語名／UNC、対象外理由、狭い画面、ライト／ダーク。移動とコピーの距離、focus枠、矢印選択と未選択Enter、安全なEsc。 |
| 隔離 Windows Shell suite | 一件／混在複数のcopy/move、同一／別ドライブ、ファイル／Folderの同名競合（置換・skip・両方保持）、Shell取消／部分失敗、先消失、開いたmediaのhandle競合、転送中の一覧切替、folder結合と選択された親子。clipboard text／cut状態が変更されないことも確認。使うファイルとフォルダは全て使い捨て、上書き先も試験データに限定。 |

実装時の自動 gate は `cargo test -p mimageviewer --lib <設定/選択/完了のfilter>`、keymap と該当 snapshot から始める。共有一覧／context を触るため最終は `scripts/test-full.ps1`、fmt、UI文字変更の `python scripts/check_ui_glyphs.py`。コマンド行列は [development-build-and-test.md](development-build-and-test.md) を正とし、旧 keymap 計画の bin-only 記述をそのまま採らない。確認可能な実装完成後は `scripts/build-dev.ps1` で利用者用 binary を作るが、本段ではビルドしない。

実装時に更新する場所（本段では変更しない）:

- `spec.md`：登録先、対象集合、混在拒否、Windows標準処理、selection／実行中navigation、path-keyデータの範囲。
- `htdocs/mimageviewer/manual/settings.html`：「フォルダ・ファイル」に登録・編集・順序・OK/Cancel・不在先。
- `htdocs/mimageviewer/manual/grid.html`：項目右クリックの一つの入口、checked優先、対象外理由。
- `htdocs/mimageviewer/manual/tut-file-ops.html`：固定先を登録→対象をチェック→行の移動／コピー、clipboardを使わず元一覧に留まる手順、競合／途中取消／部分移動、同じ場所・自己配下、メタ情報追従の判断結果。
- `htdocs/mimageviewer/manual/shortcuts.html`（既存ショートカット説明）：既定キーなしと操作カスタマイズからの割当。画面内キーはローカル操作として説明。
- `docs/keymap-spec.md`、`docs/keymap.ini.default`：Actionと生成参照。`docs/item-kind-capability-matrix.md`：整理操作の種別表。`docs/README.md`：設計索引。backlogは実装／検証後に状態更新。

## 8. 利用者判断事項・設計担当へ返す境界

推奨案は判断を先取りした実装許可ではない。まれな失敗に追加機構が必要なら、まず以下の見え方で割り切れるか確認する。

| 判断事項 | 条件・頻度／割り切った場合の見え方 | 推奨案／増える仕組み |
| --- | --- | --- |
| 「その他のフォルダ…」を初版に入れるか | 用途はほぼ固定の約5先。都度違う先が必要なときに限って不足する。 | **初版は入れない**。既存cut/pasteは残る。入れるなら一時先picker＋操作種別選択が増え、登録先中心の画面と動線が二つになる。 |
| 実／仮想の混在 | ZIP/PDFページ等をチェックに含めた場合。通常の実項目だけなら追加手順なし。 | **既存Copy/Cut同様、理由付き全体拒否**。実項目だけ続行する案は、除外対象の確認と明示同意が増える。 |
| 実行中の一覧移動 | 大きいFolderや別ドライブコピーで長くなると通常に起こる。 | **snapshotで移動可**。完了時のpath再整合を再利用。全体modalは全窓／Remoteの排他と閲覧停止の費用がある。 |
| 設定保存失敗 | disk full、DB書込不能などまれ。新規編集が再起動後に残らない可能性がある（利用者が作った設定）。 | **ログ＋通知、今のセッションはメモリ値を使用**。既存DBを消さずauto retry／rollbackなし。編集を絶対失わせない要求なら未保存draft保持／終了時確認等が増えるため、先に相談する。 |
| 整理先消失・接続切れ | 先を手動削除、USB／NAS未接続など環境依存。登録名は残り、今回だけ実行されない。 | **通知して何もしない**。自動再接続／再試行／代替先／再作成を持たない。 |
| 再同期の失敗・突然終了 | 読取不能、プロセス終了などまれ。Shell処理済み分は残り、一覧が古い場合は更新／開き直しが必要。 | **通知＋通常の開き直し**。操作ログ／再開／逆移動／回復状態は作らない。 |
| 開いた動画等のhandle | 同じ実体を閲覧しながらmoveすると起こり得る。Shellが使用中として拒否／再試行を案内する場合がある。 | **Shellに任せ、成功分だけ既存viewer後処理**。全対象のviewerを先に閉じる案は取消時も閲覧を失う。必要なら閲覧中操作の仕様を先に判断し、勝手に機能制限しない。 |
| 評価・タグ・編集等の新パスへの追従 | 設定済み項目をmoveすれば日常的に起こる。旧pathのDB行は消さないが、新pathへ自動適用される保証はない。 | **初版は既存Shell移動相当**として制約を明記する案。ただし追従が整理機能の必須条件なら別の coherent chunk として設計する。folder結合・部分移動・衝突名を含むpath移行が必要で、単一Tree renameの流用では済まない。これはまれな失敗扱いにしない。 |
| folder結合／保持snapshot内の子の整合 | 複数元や横断一覧でFolderを整理すると起こり得る。消えた子が戻り先snapshotに残ってはいけない。 | **既存snapshotの実pathをworkerで確認し既存除去へ渡す**。取得／反映が窓横断の新機構を要求するなら実装前に境界を報告する。通常操作を制限する代案や、先に全体的なstate machineを作る案は採らない。 |

設計担当の確認点は、snapshot方式、混在拒否、設定失敗時の見え方、path-key利用データの範囲と、既存snapshot再同期への接続可能性。実装前の独立レビューではこの構成自体を確認し、合意していない窓横断機構／メタ移行／回復機構を追加しない。
