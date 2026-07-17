# キー / マウス操作仕様 (静止画 vs 動画)

mimageviewer のフルスクリーン操作におけるキー / マウス アサインの整理。
画像 (静止画 / アニメーション GIF / PDF / ZIP 内画像) と動画でアサインが
異なる項目を一覧化し、整合性 / 不整合を明らかにする。

フルスクリーン境界ヒント、Ctrl+S/F/G 検索スコープ、動画タイルモードまで含めた
横断仕様と現状差分は [fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md)
を参照すること。

一部のキーボード操作は設定メニュー「操作カスタマイズ…」で上書きできる。設定の正本は
`Settings.keymap` (`settings.db`) で、旧 `%APPDATA%\mimageviewer\keymap.ini`
が残っている場合は初回起動時に 1 回だけ取り込んで
`keymap.ini.imported*.bak` へ退避する。Action 名・書式・固定扱いの入力は
[keymap.ini.default](keymap.ini.default) と、コマンド設定画面の表示を正とする。マウス、ゲームパッド、
OS/egui clipboard、D&D、IME 確定、右クリックメニューは keymap 対象外。
マウスとゲームパッドも原則 keymap 対象外だが、例外として、マウス右ドラッグの
フリック、マウス戻る / 進む / ホイールクリック、ゲームパッド <kbd>X</kbd> リングは
`Settings.ring_shortcuts` で厳選アクションだけを差し替えられる。
これは `KeyAction` / コマンド設定の完全カスタマイズではなく、マウス・ゲームパッド用の
固定入力レイヤーで扱う。
レーティングは専用の `[Rating]` グループ (`RatingItem1..5/Clear`、
`RatingContainer1..5/Clear`) で、グリッド / 画像フルスクリーン / 動画フルスクリーンが
同じ割り当てを共有する。v2.2.0 からは物理キー寄りの `KeySlot` を正本にし、通常の数字キーとテンキー数字を別キーとして扱う。互換性のため、従来の数字キー既定操作は `1` と `Numpad1` のように両方を既定割り当てにする。
本体 <kbd>Enter</kbd> と `NumpadEnter` も別スロットであり、押下操作、KeyHold、native 動画、
操作カスタマイズの「押して入力」の全経路で相互に発火しない。Windows では両者が
`VK_RETURN` を共有するため、Win32 `WM_KEYDOWN/WM_KEYUP` の extended bit を物理種別の
正本にする。egui 0.33 の `physical_key` は両者を同じ `egui::Key::Enter` へ畳むため、
この 2 スロットの照合には使わない。旧 `keymap.ini` と現行設定の表記は従来どおり
`Enter` / `NumpadEnter` で、migration や破壊的な表記変更は行わない。
OS 予約ショートカット (例: Alt+F4 / Alt+Tab / Win キー系) は keymap では上書きできない。
`Esc` と修飾なし矢印ナビゲーションは、モード脱出と閲覧の最低限の固定入力として keymap 対象外にする。
Enter / Backspace / Home / End / PageUp / PageDown などの閲覧操作は文脈ごとの `KeyAction`
として扱い、コマンド設定・競合検出・ヘルプ表示に載せる。マウス戻る / 進む / ホイールクリック、OS クリップボードや
Shell 連携 (コピー / パスコピー / フォルダを開く) は入力経路が異なるため固定入力レイヤーに残す。
標準キーを持たないが割り当て可能な操作は `keymap.ini.default` に
`# Action = none` として列挙される。コマンド設定または旧 `keymap.ini` 移行でキー名を指定すると割り当てられ、
`Action = none` を明示した場合は無効化として扱う。
お気に入りの前後移動 / 1〜20 番を開く、現在位置のルートディレクトリを開く、
`C:\`〜`Z:\` を開く、`C:`〜`Z:` の最後の場所へ切り替える、場所▼の固定項目
(ドライブ一覧 / 読書履歴 / ★1〜★5 / 本棚フォルダ / デスクトップ / ピクチャ / ダウンロード)
、ピン留めタグ 1〜20 の付与/解除も標準キーなしのサムネイル一覧 Action として割り当てられる。設定名は
`GridFavorite...` / `GridOpenFavorite...` / `GridOpenCurrentDriveRoot` / `GridOpenDrive...` /
`GridSwitchDrive...` /
`GridOpenLocation...` / `GridTogglePinnedTag...` とし、競合判定とヘルプ表示でも `Grid` 文脈として扱う。
同時に有効になり得る Action へ同じキーを割り当てた場合や、予約扱いの
Escape / 修飾なし矢印キーへ割り当てた場合は起動時に警告ログを出すが、
設定自体は読み込み、現行 dispatch の優先順を変えない。
サムネイル一覧、通常の画像フルスクリーン、動画フルスクリーン、消しゴム / 隠蔽加工 / 切り取り / テキスト注釈 / 補正レイヤーモードでは、既定 <kbd>?</kbd> の `HelpShowContextShortcuts` で現在の文脈で使える
ショートカット一覧を表示する。keymap 化済み操作は現在のコマンド設定の実割り当てから
表示し、Esc / 修飾なし矢印など固定扱いの操作は固定キーとして補助表示する。
キー未設定または明示無効化中の割り当て可能操作は、操作カスタマイズ画面で確認する。
`HelpShowContextShortcuts` もコマンド設定で変更でき、ヘルプ内の固定キー欄や native 動画 overlay の表示は変更後のキーを表示する。動画フルスクリーンは egui 経路と Windows native
動画 overlay の両方で対応する。既定の `?` は設定ファイル上も `?` と書けるが、内部的には `Shift+/` と同等に扱う。
消しゴム / 隠蔽加工パネルの描画・消去ボタンとツールボタンは、先頭 chord が
修飾なし単独キーの場合だけ `筆 [B]` のような compact 表示を実割り当てから作る。
修飾付き chord や未割り当ての場合、パネル上の compact キー表記は省略する。
メニューバーのフォルダを開く / 検索 / タグビュー項目など一部のメニュー表示と hover text は、
先頭 chord を `現在地フィルタ (Ctrl+F)` のように表示する。
★フィルタボタン / スマートフィルタ内の★項目 / フォルダバー右側のコンテナ★ tooltip も、
`RatingItem*` / `RatingContainer*` の実割り当てから `F1〜F6` / `Shift+F1〜F6` などの
表示を作る。
静止画フルスクリーンの上部ホバーバー tooltip と、表示モード / ズーム・フィット
popup の shortcut 表記は、`分析ツール [Shift+Z]` や `パネル表示 [I / Tab]`
のように実割り当てから作る。native 動画 overlay の top bar / bottom HUD / jump panel /
seek hover thumbnail も KeyAction 由来の shortcut 表記を実割り当てから作る。
グリッドの <kbd>Backspace</kbd> 親フォルダ移動、Enter / Home / End / PageUp / PageDown
ナビゲーション、グリッド / フルスクリーンの <kbd>F11</kbd> 系ウィンドウ切り替えも
keymap 対象にする。`Esc` / 修飾なし矢印ナビゲーション、Ctrl+ホイールなど固定扱いの入力は従来どおり。

**動画→音声モード / 音楽ビュー**: 音声ファイル (`GridItem::Audio`) の再生と、動画再生中に映像を
消して音声だけ聴くモードは、どちらも `[FsVideo]` のキーをそのまま流用する。映像 ↔ 音声の切り替えは
`VideoToggleAudioMode` (既定 <kbd>Z</kbd>)。`VideoLoop` / `VideoBookmark` / `VideoMarkerPrev` /
`VideoMarkerNext` は音楽ビューでは音楽のループ / ブックマーク / ブックマーク移動へ翻訳される。
動画専用の `VideoTileMode` / `VideoCapture` / `VideoFrameStep` / `VideoPin` は音楽ビューでは無効
(キャプチャパレット / コマ送り UI を出さない)。`[Rating]` グループは静止画・動画と同じく
音楽ビューでも使える。左右パネル表示モードの <kbd>I</kbd> / <kbd>Tab</kbd> は 3 面で同じ
`FsToggleMetadata` を使い、native 動画も effective chord を App 側へ転送する。
Windows の egui 経路では、KeySlot でショートカットを所有した押下を egui event queue からも
同時に除去する。ただし egui 0.33 は UI 実行前の `begin_pass` で <kbd>Tab</kbd> から focus
方向を確定するため、event 除去だけでは traversal を止められない。各 egui Context に登録した
`on_begin_pass` ポリシーが、viewport ごとに直前 pass の `PlatformOutput::ime` と focus ID の
`TextEdit::load_state` を参照し、実際の TextEdit 編集中でない <kbd>Tab</kbd> の focus 方向を
最初の focusable widget 登録前に
`FocusDirection::None` へ戻す。event 自体はこの段階では残し、後段の Keymap が通常どおり
consume する。no-repeat の <kbd>Tab</kbd> は repeat event も発火させず除去する。
`wants_keyboard_input()` の gate はこの消費より先に維持し、TextEdit / IME やフォーカス中 UI が
キーボードを所有している間は KeySlot と egui event の双方を残す。

開発者向けメモ: 新しいキーボード操作を追加・変更するときは、ユーザーから明示されて
いなくても keymap 対応要否を確認する。通常ショートカットは `KeyAction` に追加し、
`docs/key-customization-impl-plan.md` の「新しいキー操作を追加するとき」に従って
`docs/keymap.ini.default` まで更新する。固定扱いにする入力は、この文書の該当節に理由を残す。

## 固定入力 / KeyAction 対象外の整理

操作カスタマイズの一覧に出すものは、原則として `KeyAction` 化された離散ショートカットだけにする。
次の入力は OS / フォーカス中 UI / モード内ドラッグ状態に強く依存するため、現時点では固定入力として扱う。
固定入力へ同じキーを割り当てた場合は競合警告やヘルプで補助表示することはあるが、固定入力そのものは
コマンド設定で差し替えない。

| 分類 | 固定扱いの入力 | 理由 / 備考 |
|---|---|---|
| OS 予約 | <kbd>Alt</kbd>+<kbd>F4</kbd>、<kbd>Alt</kbd>+<kbd>Tab</kbd>、<kbd>Win</kbd> キー系など | Windows 側が先に処理する。mIV の keymap では上書きしない |
| フォーカスローカル UI | テキスト入力、IME 変換、コンボボックス、リスト、フォルダツリー、製本並べ替えダイアログ内の矢印 / Enter / Esc / PageUp / PageDown / Home / End など | その UI 部品の中だけで意味を持ち、グローバルショートカットとして外へ漏らさない |
| モーダル削除確認 | <kbd>Y</kbd> = 削除、<kbd>N</kbd> / <kbd>Esc</kbd> = キャンセル、<kbd>Enter</kbd> = 無効 | 誤操作防止のため keymap 対象外の固定入力。ダイアログ表示中に Y / N / Esc を消費し、背面の KeyAction へ漏らさない。IME 変換中は Y / N / Esc の確認操作を行わない |
| 最低限の脱出 / 閲覧ナビ | <kbd>Esc</kbd>、修飾なし矢印キー | モード脱出とページ / 一覧移動の最後の手段として残す。Enter / Backspace / Home / End / PageUp / PageDown などは文脈ごとの `KeyAction` 化対象 |
| UI 表示倍率 | egui 既定の <kbd>Ctrl</kbd>+<kbd>+</kbd> / <kbd>-</kbd> / <kbd>0</kbd> | `Settings.ui_scale_factor` と main Context の `zoom_factor` を単一の正本にするため、main / native presenter の両 egui Context で `zoom_with_keyboard=false`。表示倍率は設定メニュー「スケーリング」だけから変更し、KeyAction 対象外とする |
| サムネイル一覧の範囲選択 | <kbd>Shift</kbd>+矢印キー | グリッド選択カーソルの移動とチェック追加が一体になった固定操作。Grid 文脈で同じキーを割り当てた場合は予約キー警告を出す |
| Shell / クリップボード / D&D | Shell コピー / 切り取り / 貼り付け、Shell 右クリックメニュー、外部アプリへのドラッグ送出 | Windows Shell やクリップボードイベントの経路を優先するため、キーボードコマンドとは分ける |
| 編集ツール内のドラッグ状態 | 消しゴム / 隠蔽加工 / 補正レイヤーの選択後の矢印 / <kbd>Ctrl</kbd>+矢印、<kbd>[</kbd> / <kbd>]</kbd>、<kbd>Ctrl</kbd>+<kbd>[</kbd> / <kbd>]</kbd>、ハンドル操作中の <kbd>Shift</kbd> / <kbd>Alt</kbd>、テキスト注釈の四隅ハンドルドラッグ中の <kbd>Ctrl</kbd> / <kbd>Shift</kbd>、切り取り / テキスト注釈のドラッグやホイールなど | 選択中オブジェクト、ドラッグ中の形状、パネルフォーカスに依存するモード内操作。テキスト注釈の <kbd>Ctrl</kbd>（中心対称）と <kbd>Shift</kbd>（縦横比固定）は離散ショートカットではなく、マウスドラッグ中だけ幾何制約を切り替える修飾なので keymap 対象外。フルスクリーンキャンバスでは egui の修飾状態が stale になり得るため、両方ともドラッグ中の各フレームで OS から直接読む。操作カスタマイズ画面では消しゴム / 隠蔽 / 切り取り / テキスト / 補正レイヤーの通常コマンドは「編集モード」としてまとめるが、これらの微調整キーは固定入力のまま |
| 360 度パノラマ表示中 | 左ドラッグの yaw/pitch、ホイールの FOV、上バーの 360 解除ボタン | パノラマ表示中だけの連続操作。<kbd>V</kbd> の 360 度パノラマモード切替 (`FsPanorama`) は `KeyAction` であり、コマンド設定で変更できる |
| 動画の修飾なし左右 | <kbd>←</kbd> / <kbd>→</kbd> の 5 秒シーク、およびタイル中の左右カーソル移動 | 修飾なし矢印は固定ナビゲーションとして残す。<kbd>Alt</kbd> 付き左右は固定シーク扱いにせず、割り当てた `KeyAction` を優先する。<kbd>Shift</kbd> / <kbd>Ctrl</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd> 付きの動画シーク / フレーム送りは `KeyAction` 化済み |

補足: 画像分析モード起動の既定 <kbd>Shift</kbd>+<kbd>Z</kbd> は `FsImageAnalysis`、ルーペの
<kbd>Shift</kbd> 押しっぱなしは `FsLoupeHold`、360 度パノラマ切替の <kbd>V</kbd> は
`FsPanorama` として `KeyAction` 化済み。これらは固定入力ではなく、操作カスタマイズで変更または解除できる。

## グリッドビュー (フルスクリーン外) 共通

| キー | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | 現在のサムネイル一覧コンテキストで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いのナビゲーションキーは別枠で表示する |
| <kbd>Backspace</kbd> | 親フォルダへ。Action: `GridParentFolder`。ドライブルート (`C:\` など) ではドライブ一覧へ戻り、元ドライブを選択状態にする。検索 (Ctrl+S / Ctrl+G) 中は検索仮想階層を 1 段ドリルアップ、最上位 (集約ビュー / 結果一覧) では no-op。タグビュー (Ctrl+T) 中は、検索結果から開いたフォルダ / ZIP / PDF / 変換アーカイブを 1 段戻り、検索結果一覧では no-op (検索を閉じるには <kbd>Esc</kbd> / 検索バーの <kbd>×</kbd> / <kbd>Ctrl</kbd>+<kbd>G</kbd>・<kbd>Ctrl</kbd>+<kbd>S</kbd>・<kbd>Ctrl</kbd>+<kbd>T</kbd> 再押下)。Ctrl+F フィルタ中は、フィルタを実行したフォルダだけ親移動を no-op にする。検索結果から子フォルダへ入った後は通常どおり親へ戻れる |
| <kbd>Enter</kbd> | 選択アイテムを開く。Action: `GridOpenSelected`。別ウィンドウセッションが同じ項目を既に表示中の場合は再オープンせず、必要に応じて別ウィンドウを前面化する |
| 既定キーなし (`GridOpenSelectedAsPage` / `GridOpenSelectedAsList`) | 選択中の ZIP/PDF/対応アーカイブを、全体設定に関係なく「ページを開く」または「一覧を開く」で明示的に開く。右クリックメニュー、リングショートカット、マウスジェスチャ候補にも同じ操作を出す |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> | 選択中の動画を外部プレイヤーで開く。Action: `GridOpenExternalPlayer` |
| <kbd>Alt</kbd>+<kbd>↑</kbd> | 親フォルダへ。Action: `GridParentFolder` の別既定割り当て (<kbd>Backspace</kbd> と同じ。Explorer 慣習に合わせた代替ショートカット。ドライブルートではドライブ一覧へ戻る。Ctrl+F フィルタ元フォルダでは no-op) |
| <kbd>Alt</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | フォルダ履歴を戻る / 進む。Action: `GridHistoryBack` / `GridHistoryForward` (フォルダバーの ←/→ と同じ。検索中は無効。ドライブ一覧でも履歴 target があれば有効) |
| 既定キーなし (`GridOpenCurrentDriveRoot`) | 現在位置のルートディレクトリへ移動する。通常フォルダ、ZIP/PDF/変換アーカイブ内では `effective_folder()` のドライブ root / UNC share root を対象にする。検索中・★固定中は無効 |
| 既定キーなし (`GridOpenDriveC..Z`) | 対応する `C:\`〜`Z:\` のドライブルートを直接開く |
| 既定キーなし (`GridSwitchDriveC..Z`) | アクティブな A/B クイックフォルダスロットごとに覚えている、そのドライブの最後の場所へ切り替える。未訪問または最後の場所が存在しない場合は対象ドライブ root へフォールバックする |
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> | ツリー順で前のフォルダへ (DFS pre-order、画像なしフォルダは skip_limit までスキップ)。Action: `GridTreeFolderPrev`。検索中は前のヒットフォルダへ移動 (`global_search_ctrl_nav` / `favsearch_ctrl_nav`)。★固定 中は snapshot 内の前 entry へ |
| <kbd>Ctrl</kbd>+<kbd>↓</kbd> | ツリー順で次のフォルダへ (DFS pre-order)。Action: `GridTreeFolderNext`。検索中は次のヒットフォルダへ移動。★固定 中は snapshot 内の次 entry へ |
| <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> | 前 / 次の兄弟フォルダへ。Action: `GridSiblingFolderPrev` / `GridSiblingFolderNext`。同じ親の直下だけを対象にし、空フォルダも skip せず、子や祖先の兄弟には入らない。検索中は無効。★固定 中は snapshot 内の前/次 image-like entry へ (Folder/Zip/Pdf entry は skip) |
| <kbd>Home</kbd> / <kbd>End</kbd> | サムネイル一覧の先頭 / 末尾へ移動する。Action: `GridMoveFirst` / `GridMoveLast` |
| <kbd>PageUp</kbd> / <kbd>PageDown</kbd> | サムネイル一覧を 1 ページ分前 / 次へ移動する。Action: `GridPagePrev` / `GridPageNext` |
| <kbd>F1</kbd>〜<kbd>F5</kbd> | レーティング 1〜5。ドライブ一覧中は無効 |
| <kbd>F6</kbd> | レーティング解除。ドライブ一覧中は無効 |
| <kbd>F7</kbd> / <kbd>F8</kbd> | 消しゴムマスクスロット 1 / 2 をチェック済み画像へ一括適用 (チェックがなければ選択中の 1 枚)。Action: `GridApplyErase1/2` |
| <kbd>F9</kbd> / <kbd>F10</kbd> | 隠蔽マスクスロット 1 / 2 をチェック済み画像へ一括適用 (チェックがなければ選択中の 1 枚)。Action: `GridApplyConceal1/2` |
| <kbd>Shift</kbd>+<kbd>F7</kbd> / <kbd>Shift</kbd>+<kbd>F8</kbd> | チェック済み画像 / 選択中画像から消しゴムマスクを削除。Action: `GridDeleteEraseMask` |
| <kbd>Shift</kbd>+<kbd>F9</kbd> / <kbd>Shift</kbd>+<kbd>F10</kbd> | チェック済み画像 / 選択中画像から隠蔽マスクを削除。Action: `GridDeleteConcealMask` |
| <kbd>P</kbd> | 選択中アイテムを現在のコンテナの代表サムネに固定 / 解除 (toggle、フォルダバー 📌 の左クリックと同等)。ZIP 内の ZipDir も通常フォルダと同じ cascade で子の pin に追従する。pin 不能アイテム / 検索アグリゲート / zip_nav のない変換キャッシュ状態では silent no-op。**動画フルスクリーンの P と合わせて P = Pin に統一** |
| <kbd>F</kbd> | 左側のフォルダツリーペインの表示 / 非表示を切り替える。表示時は現在フォルダへツリーカーソルを移す。非表示にする時、ツリーカーソルが別フォルダへ動いていれば <kbd>Enter</kbd> 相当でそのフォルダへ移動してグリッドへ戻る (動いていなければ単に閉じる) |
| 既定キーなし (`GridToggleStackMode`) | コマンド設定でキーを割り当てると、フォルダバーの「スタック」と同じスタック表示トグルを実行する。実フォルダまたはサブ展開ビューで有効 |
| <kbd>T</kbd> | 選択中アイテムへタグを付ける/外すダイアログを開く |
| <kbd>Ctrl</kbd>+<kbd>T</kbd> | タグビューを開く / 閉じる。`tags.db` のタグから候補を表示し、選んだタグを持つフォルダ・画像・動画・音声・ZIP/PDF/対応アーカイブを検索結果グリッドに表示する。「すべての種類」プルダウンで画像・動画・音声など結果の種類を絞れる。フルスクリーン中はテキスト注釈モードの <kbd>Ctrl</kbd>+<kbd>T</kbd> を優先する |
| 既定キーなし (`GridTogglePinnedTag1..20`) | コマンド設定でキーを割り当てると、ピン留めタグ 1〜20 番を選択中またはチェック済みアイテムへ付与/解除する。ピン留めタグの番号は「ピン留めタグの管理」画面の番号列と同じ |
| <kbd>X</kbd> | 選択中の画像 / ZIP 内画像 / PDF ページを比較スロットへピン留め / 同じ画像なら解除 |
| <kbd>Ctrl</kbd>+<kbd>B</kbd> | 選択中またはチェック済みの画像 / ZIP 内画像 / PDF ページを追加先の本へ追加する。画像・ページ以外が選択に混在している場合は一部追加せず全体を拒否する |
| <kbd>Space</kbd> | 選択中アイテムをチェック ON/OFF。画像 / 動画 / ZIP・PDF 本体 / 変換前アーカイブ / ZIP 内画像 / PDF ページが対象 (**フォルダとドライブ一覧はチェック対象外**) |
| <kbd>Ctrl</kbd>+<kbd>A</kbd> | 表示中のチェック可能なアイテムを全選択 |
| <kbd>Alt</kbd>+<kbd>1</kbd>〜<kbd>9</kbd> / <kbd>Alt</kbd>+<kbd>0</kbd> | サムネイル列数を 1〜9 / 10 列に切り替え。詳細表示中はサムネイル表示へ戻してから列数を適用 |
| <kbd>Alt</kbd>+<kbd>-</kbd> | サムネイル表示 / 詳細表示を切り替え |
| <kbd>Ctrl</kbd>+<kbd>C</kbd> / <kbd>X</kbd> | チェック済み、または選択中の実ファイル / 実フォルダを Windows Shell のコピー / カット verb へ渡す。ZIP/PDF 内ページなど仮想項目が含まれる場合は実ファイルだけを部分コピーせず、トーストで通知して中止する |
| <kbd>Ctrl</kbd>+<kbd>V</kbd> | Windows Shell の背景ペースト verb で、クリップボードのファイル / フォルダを現在の実フォルダへペースト。ZIP/PDF/検索結果グリッドなど実フォルダ以外では無効 |
| <kbd>Delete</kbd> | チェック済み、または選択中の実ファイル / 実フォルダを削除 (通常はゴミ箱。ZIP/PDF 内ページなど仮想項目は対象外) |
| <kbd>F11</kbd> | メインウィンドウを最大化 ⇔ 元のサイズに復元する (`toggle_main_window_maximized` → `ViewportCommand::Maximized`)。Action: `GridToggleMaximize`。フルスクリーン中は F11 が window/全画面切替 (フルスクリーン共通表参照) なので、この最大化トグルは通常 (グリッド) 表示時のみ |
| <kbd>F12</kbd> | 画像・動画ビューアの別ウィンドウモード ON/OFF を切り替える。静止画 / ZIP画像 / PDFページは detached viewport、動画は同じ detached viewport の child native presenter で表示する |
| マウス左ドラッグ | グリッドのセルを掴んでエクスプローラ等へファイル D&D 送出 (コピー)。複数チェック選択時はその実パス群をまとめてドラッグ。フォルダ / ZIP・PDF 本体 / 変換前アーカイブも対象。ZIP/PDF 内画像 (仮想フォルダ) とドライブ一覧は対象外 |
| マウス戻る / 進む / ホイールクリック | `Settings.ring_shortcuts.mouse_buttons_grid` に従い、物理戻る / 進むボタンとホイールクリックを個別に割り当てる。新規環境と既定リセットは戻る / 進むがフォルダ履歴、ホイールクリックは未割り当て。既存環境は初回ダイアログで標準 / 従来どおりを選ぶまで、戻る / 進むのみ従来互換の Ctrl+↑ / Ctrl+↓ 相当 |
| エクスプローラ等からのドロップ | mIV ウィンドウへファイルをドロップすると現在表示中のフォルダへコピー (**フォルダは v1.1.0 で一旦無効化・skip**)。ZIP/PDF / 検索結果グリッドなど実フォルダ以外を表示中は拒否 |
| グリッド空白の右クリック | 現在の実フォルダの Windows Shell 背景メニューを表示する。mIV 先頭項目の「貼り付け」および Shell 側の Paste はどちらも Shell `paste` verb を使い、新しいフォルダ作成やペースト後の一覧更新は表示中フォルダ watcher が拾う |

Windows では、Shell のコピー / 切り取り / 貼り付けや native context menu の
モーダル処理中に Ctrl/Shift/Alt の KeyUp が egui へ届かないことがある。そのため
mIV は各フレームの入力処理前と Shell 復帰直後に Win32 の実キー状態を読み、
`i.modifiers` / `raw.modifiers` を同期する。個別の `Event::Key` に記録された
modifiers はイベント発生時点の情報として残し、離散ショートカットの event 判定は
書き換えない。

## フォルダツリーペイン

フォルダツリーペインはツールバーの `ツリー` ボタン、またはグリッド側の <kbd>F</kbd> で表示する左ペイン。ここで扱うツリーは実ファイルシステムのディレクトリのみで、ZIP / PDF / 変換アーカイブなどの仮想フォルダは表示しない。ZIP / PDF 表示中は、その親実フォルダを現在位置として同期する。

ツリー内の矢印 / Enter / Esc は、テキスト入力やリストボックス操作と同じフォーカスローカルな固定入力として扱う。グローバルショートカットではないため `KeyAction` には追加しない。ツリー側にフォーカスがある間は、同じキーがグリッド選択移動やレーティング操作へ漏れないようグリッド側のキー処理を止める。

| キー | 動作 |
|---|---|
| <kbd>↑</kbd> / <kbd>↓</kbd> | 展開されているツリー行の中でカーソルを前 / 次へ移動 |
| <kbd>←</kbd> | カーソル行が展開中なら閉じる。閉じている場合は親フォルダへ移動 |
| <kbd>→</kbd> | カーソル行が閉じていれば展開。展開済みで子フォルダが読み込み済みなら先頭の子フォルダへ移動 |
| <kbd>Enter</kbd> | カーソル位置の実フォルダを右側ペインで開き、フォーカスをグリッド / 詳細一覧へ戻す |
| グリッド側の <kbd>Esc</kbd> | フォルダツリー表示中、かつ検索バー・ダイアログ・フルスクリーンなどの優先処理がない場合、ツリー側へフォーカスを移す |

★固定 (Snapshot Lock) 中はフォルダ間移動を禁止するため、フォルダツリーペイン全体を disabled とし、クリック / キー操作を受け付けない。

## 製本 並べ替えダイアログ

製本の並べ替えダイアログ内の PageUp/PageDown/Home/End は、テキスト入力やリストボックス操作と同じフォーカスローカルな固定入力として扱う。グローバルショートカットではないため `KeyAction` には追加しない。カーソル表示と Space 選択は v1.7.0 初期版では持たず、キー操作はサムネイル一覧のスクロールだけを行う。

| キー | 動作 |
|---|---|
| <kbd>PageUp</kbd> / <kbd>PageDown</kbd> | サムネイル一覧を 1 画面弱、上 / 下へスクロール |
| <kbd>Home</kbd> / <kbd>End</kbd> | サムネイル一覧の先頭 / 末尾へスクロール。予約ナビゲーションとして固定扱い |

## フルスクリーン共通 (画像 / 動画とも)

| 入力 | 動作 |
|---|---|
| <kbd>Esc</kbd> | フルスクリーン解除。**環境設定 `auto_fullscreen_zip_pdf` が ON で ZIP/PDF/変換アーカイブ内のページを表示している場合は、ページ一覧 (L2) を経由せず親フォルダの一覧 (L1) へ直帰**。追加設定 `auto_fullscreen_image_folders` も ON で、画像のみ通常フォルダを直接ページ表示している場合も同じ (`handle_fullscreen_close_request` → `pending_return_to_parent` → 入力ナビ合流点が親へナビ) |
| <kbd>Enter</kbd> | (画像) フルスクリーン解除。Action: `FsClose`。グリッドで Enter / ダブルクリックで開く動作とトグル成立。`auto_fullscreen_zip_pdf` ON のコンテナページ、または追加設定 ON の画像のみ通常フォルダでは親直帰も Esc と同じ / (動画) 再生・一時停止トグル。Action: `VideoPlayPause` |
| <kbd>Space</kbd> | (画像) 選択 (チェック) トグル — スライドショー再生中なら停止 / (動画) 再生・一時停止トグル |
| <kbd>Backspace</kbd> | フルスクリーンを 1 段閉じてグリッドビューへ戻る。Action: `FsBackToList`。ZIP/PDF/変換アーカイブ内のページ、または追加設定 ON の画像のみ通常フォルダでは、そのコンテナ / フォルダのページ一覧 (L2) を表示 (= Esc/Enter の「L1 へ直帰」と対をなす) |
| <kbd>Home</kbd> / <kbd>End</kbd> | フルスクリーン中の先頭 / 末尾の項目へ移動する。Action: `FsJumpFirst` / `FsJumpLast`。動画 native presenter 経路でも App 側へ転送する |
| <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> | 前 / 次の兄弟フォルダへ。同じ親の直下だけを対象にし、移動先に image-like があればフルスクリーンを維持して先頭 image-like を開く。なければ一覧へ戻る。切り離した detached 窓 / always-new 窓では、メイン bundle との境界をまたがないため無効化し案内だけ出す |
| マウスホイール | 前 / 次のファイル。縦/横連結モードでは連結方向へスクロール |
| マウス戻る / 進む / ホイールクリック | `Settings.ring_shortcuts.mouse_buttons_image` / `mouse_buttons_video` に従い、物理戻る / 進むボタンとホイールクリックを個別に割り当てる。画像フルスクリーンでは Home/End 相当の先頭 / 末尾移動、全画面ズームモードも候補に含む。ウィンドウ最小化は通常フルスクリーン / detached viewer / native video で利用でき、動画は最小化後も再生を継続する。画像 / 動画フルスクリーンのマウスボタン候補では `C:\`〜`Z:\`、お気に入り、読書履歴、★一覧などの場所移動系は表示しない。新規環境と既定リセットは戻る / 進むがフォルダ履歴、ホイールクリックは未割り当て。従来どおりを選んだ既存環境は戻る / 進むのみ Ctrl+↑ / Ctrl+↓ 相当。ホイールクリックは 500ms 以内かつドラッグしきい値以下の短クリックだけ発火し、中ボタンドラッグズームとは分離する。全画面ズームモードへ割り当てた場合、Z キー長押し時の照準表示はスキップして現在のカーソル位置でズーム状態へ入る |
| マウス左クリック | (画像) ページめくり。LTR では右半分クリックで次 / 左半分クリックで前、RTL では左半分クリックで次 / 右半分クリックで前 / (動画) 再生・一時停止トグル |
| <kbd>F1</kbd>〜<kbd>F5</kbd> / <kbd>F6</kbd> | 表示中アイテムへレーティング 1〜5 / 解除 |
| <kbd>Shift</kbd>+<kbd>F1</kbd>〜<kbd>F5</kbd> / <kbd>Shift</kbd>+<kbd>F6</kbd> | 現在のコンテナへレーティング 1〜5 / 解除 |
| <kbd>F11</kbd> | ウィンドウ内表示 ⇔ 全画面表示 を切り替え (右上 × の左のトグルボタンと同等)。Action: `FsToggleWindowMode`。静止画は egui 経路 (`toggle_still_window_mode` = 設定 flip のみ)、動画は native presenter 経路 (`toggle_video_window_mode` = presenter rebuild)。Windows native 動画ウィンドウにフォーカスがある場合も現在の keymap 割り当てで App 側へ転送し、S キーの動画タイル表示中もタイル状態を維持したまま切り替える。別ウィンドウ表示中は、真の fullscreen API ではなく装飾なしでモニター全体を覆う仮想フルスクリーンをトグルする。消しゴムモード中は無効化 |
| <kbd>F12</kbd> | 画像・動画・音声ビューアの別ウィンドウモード ON/OFF を切り替える。Global action として keymap 対象。native 動画ウィンドウにフォーカスがある場合も App 側へ転送する。静止画のフルスクリーン編集モード中、IME 変換中、ダイアログ操作中は発火させない。設定「画像/動画を別ウィンドウで開く」が ON の間は、静止画 / ZIP画像 / PDFページでの F12 切替は無効化し、動画 / 音声表示中だけ現在のメディアをメイン / detached へ一時 migration する。F11 のウィンドウ内表示 / 全画面表示の選択は F12 とは独立して保持し、F12 OFF 時は直前の F11 状態へ戻る |
| <kbd>Ctrl</kbd>+<kbd>B</kbd> | (画像) 現在ページを追加先の本へ追加 / (動画) 現在の再生フレームを画像として追加先の本へ追加 |

「画像/動画を別ウィンドウで開く」設定で開いた always-new 窓では、消しゴム・隠蔽加工・テキスト注釈・切り取り・補正レイヤー・マスクスロット適用/削除などの編集系ショートカットを無効化する。通常 F12 の linked 別ウィンドウでは従来どおり有効。全体の色調補正、ポストフィルタ、パノラマ、分析などの表示系操作は連動なし窓でも有効。

## フルスクリーン編集モード共通 (静止画)

消しゴム / 隠蔽加工 / 切り取り / テキスト注釈 / 補正レイヤーでは、ツール固有の描画・選択・ハンドル操作中でも、以下の閲覧操作を共通で使えるようにする。パネル上で開始した操作はパネル UI を優先し、画像上で開始した Space パンや中ボタンズームは途中でパネル上を横切っても継続する。

| 入力 | 動作 |
|---|---|
| <kbd>Space</kbd>+左ドラッグ | 一時パン。進行中の描画 / 図形 / crop / 注釈ドラッグは途中でパンへ切り替えず、現在の操作を完結させる |
| マウスホイール | 画像上ではズーム。スクロール可能なツールパネル上ではパネルスクロールを優先 |
| <kbd>Ctrl</kbd>+マウスホイール | ズーム。ツールパネル上でも同じ |
| ホイール押し込み+上下ドラッグ | 中ボタンドラッグズーム。パネル上で開始した場合は無視 |
| 右 <kbd>Ctrl</kbd> 押しっぱなし | 元画像表示。補正 / AI / 消しゴム / 補正レイヤー / 隠蔽 / 注釈を一時的に外す。補正レイヤーの <kbd>Ctrl</kbd>+<kbd>Shift</kbd> は選択レイヤーバイパス表示を優先 |

## ゲームパッド操作

ゲームパッドは閲覧専用の固定割り当て。編集、削除、レーティング、チェック切り替え、
エクスポートは対象外。マウス戻る / 進む / ホイールクリックは環境設定の
「マウスボタン」、<kbd>X</kbd> リングは「リングショートカット」で差し替えられるが、
`keymap.ini` では扱わない。画像・動画フルスクリーンのリングには、キーボード側の
`FsToggleWindowMode` と同じウィンドウ / 全画面切替、<kbd>F12</kbd> 相当の別ウィンドウ ON/OFF、
ウィンドウ最小化も割り当てられる。

detached viewer とメイングリッドが同時に表示される場合、ゲームパッド全体と
<kbd>X</kbd> リングの現在コンテキストは OS の前面ウィンドウを優先して決める。
前面 HWND を一時的に確定できない場合だけ、最後にゲームパッドまたは割り当て済み
マウスボタンを操作した面へ追従する。マウス戻る / 進む / ホイールクリックはイベントを
受け取った面を発火面として固定し、メイングリッドからの操作が detached viewer の項目へ
誤って適用されないようにする。<kbd>X</kbd> の値ピッカーも発火面だけに表示する。中ボタンの
短クリック判定は開始面を保持し、同時表示中の別面に入力がないフレームで取り消さない。
これらは `KeyAction` ではなく固定入力レイヤーの責務である。

| 入力 | 動作 |
|---|---|
| 方向パッド / 左スティック | グリッドでは選択移動。詳細一覧では上下が 1 行移動、左右が表示行数ぶん前後にスキップ。画像ではページ送り。縦連結では上下がスクロール、左右がページ送り。横連結では左右がスクロール、上下がページ送り。左スティックは連結方向を連続スクロール。動画では左右がシーク / タイルカーソル移動、上下が前後ファイル移動 |
| <kbd>A</kbd> / <kbd>B</kbd> | 決定・開く・再生 / 戻る・閉じる。グリッドでの <kbd>B</kbd> は <kbd>Backspace</kbd> / <kbd>Alt</kbd>+<kbd>↑</kbd> と同じ親移動で、ドライブルートではドライブ一覧へ戻る |
| <kbd>LB</kbd> / <kbd>RB</kbd> | Ctrl+↑ / Ctrl+↓ と同じ前 / 次フォルダ移動 |
| <kbd>LT</kbd> / <kbd>RT</kbd> | グリッドでは連続スクロール、画像ではズームアウト / ズームイン、動画では連続シーク |
| 右スティック上下 | 画像フルスクリーンのズーム。LT/RT より速め |
| <kbd>Select</kbd> | グリッドでは場所リストを開き、ドライブ一覧 / 読書履歴 / 本棚フォルダ / 既知フォルダ / ドライブを上下で選んで <kbd>A</kbd> で移動する。画像フルスクリーンでは見開きモードを巡回切り替え (単ページ / 見開き 左→右 / 左→右(表紙あり) / 右→左 / 右→左(表紙あり))。動画フルスクリーンではブックマーク / チャプター / ピンの一覧を開き、上下で選んで <kbd>A</kbd> で移動する |
| <kbd>X</kbd>+方向パッド / 左スティック | リングショートカット。<kbd>X</kbd> を押しながら方向を選び、<kbd>X</kbd> を離すと現在コンテキスト (グリッド / 画像フルスクリーン / 動画フルスクリーン) のスロットに設定された一発アクションを実行する。左スティックは軽く触れた程度では方向確定せず無方向扱い。方向なしで離すと専用ピッカーパネルを開き、方向で行選択/値変更、<kbd>A</kbd> で一覧を開くか一覧項目を選択、<kbd>X</kbd> / <kbd>B</kbd> で確定して閉じる。<kbd>B</kbd> でリング選択もキャンセルできる。<kbd>X</kbd> リング中とピッカー表示中の方向入力は通常ナビゲーションへ流さない |
| <kbd>Y</kbd> | グリッドではフォルダツリーペインを開閉する。表示時は現在フォルダへツリーカーソルを移す。非表示にする時、ツリーカーソルが別フォルダへ動いていれば <kbd>A</kbd>/<kbd>Enter</kbd> 相当でそのフォルダへ移動してグリッドへ戻る (動いていなければ単に閉じる)。動画では S キー相当。画像では Y+左右で Ctrl+左右相当の見開き 1 ページずらし、Y+上下で Home/End 相当の先頭 / 末尾移動 |
| フォルダツリー表示中の方向パッド / 左スティック / <kbd>A</kbd> | ツリーカーソルを上下左右に移動し、<kbd>A</kbd> で選択フォルダを開く。決定後はツリーを自動で閉じ、グリッドへ戻る |
| <kbd>Y</kbd>+方向パッド左右 (動画) | J / K キーと同じ前 / 次のチャプター・ブックマーク・ピン移動 |
| <kbd>Start</kbd> | お気に入り一覧を開く。上下で選んで <kbd>A</kbd> で移動し、件数が多い場合はスクロールバーで続きが分かる |

## 画像 フルスクリーン

| キー | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | 画像フルスクリーンで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いのナビゲーションキーは別枠で表示する |
| <kbd>I</kbd> / <kbd>Tab</kbd> | 左右パネルの表示モード (通常ホバー / クリック表示) を切り替える。Action: `FsToggleMetadata`。画像フルスクリーン、native 動画、egui 音楽ビューで共通。native 動画では effective chord を presenter から App へ転送する |
| <kbd>←</kbd> / <kbd>→</kbd> | 前 / 次のファイル (見開き中は前 / 次の見開き = 2 ページ送り)。最後 / 最初の見開きでは 1 回目の入力で境界案内を表示する |
| <kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次のファイル (= 一般慣例で左右と同義)。見開きの境界判定も左右と共通。縦連結では縦スクロール、横連結では前 / 次ファイル。スライドショー中もフォルダ内移動は再生を止めない |
| <kbd>Shift</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 通常はプレーン <kbd>↑</kbd>/<kbd>↓</kbd> と同義 (前 / 次ファイル)。**ファイル名スタックのフラット読書中 (v2.0.0)** は「前 / 次のスタックの先頭画像へジャンプ」(= 今の投稿の残りをスキップ。`Shift+↑` はスタック途中なら現スタック先頭、先頭なら前スタック先頭)。端では no-op (次フォルダは <kbd>Ctrl</kbd>+<kbd>↓</kbd>)。スタックジャンプ部分の Action: `FsStackJumpPrev` / `FsStackJumpNext`。動画再生中は音量 (下表参照) |
| 既定キーなし (`FsPagePrev` / `FsPageNext`) | コマンド設定でキーを割り当てると、前 / 次のページへ移動する。矢印ナビゲーションは固定扱いのまま残しつつ、明示的にページ送りへ割り当てたい場合の Action |
| <kbd>Shift</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 現在の表示順で、環境設定の「ページジャンプ量」ぶん前 / 次へジャンプ。Action: `FsFixedJumpPrev` / `FsFixedJumpNext`。既定は全ページの 10%。固定ページ数にも切替可。見開き中は最低 2 ページ進む。左右の意味は通常の左右ページ送りと同じく RTL で反転。動画は対象外 |
| <kbd>PageUp</kbd> / <kbd>PageDown</kbd> | 縦/横連結モードでは画面単位で連結方向へスクロール。通常のページ単位表示では、環境設定の「ページジャンプ量」ぶん前 / 次へジャンプする。Action: `FsFixedJumpPrevNoRtl` / `FsFixedJumpNextNoRtl`。こちらは PageUp/PageDown へ割り当てる用途のため RTL でも前 / 次の意味を反転しない。修飾なし矢印ナビゲーションは文脈依存の固定入力として残す一方、メタデータパネル・ページジャンプ・Home/End 先頭末尾移動など副作用が明確な操作は KeyAction 化して競合検出対象にする |
| <kbd>Ctrl</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 見開きの「1 ページずらし」(現在の表示ユニット先頭を軸に見開きを 1 ページぶんずらす。空白/欠落ページでの綴じずれ補正。1 回押すごとに必ず 1 ページ動く)。結果はセッション内の一時アンカーとして保持し、`spread_db` には保存しない。Single モードでは前 / 次ファイル。RTL では左右の意味を反転。リング / マウス / パッド用の操作候補には、この左右版に加えて PageUp/PageDown と同じく RTL でも前 / 次を反転しない「前 / 次」版を用意する |
| <kbd>0</kbd> 〜 <kbd>7</kbd> | <kbd>1</kbd>〜<kbd>5</kbd>: ページ構成切替 (<kbd>1</kbd>: 単ページ / <kbd>2</kbd>: 見開き 左開き / <kbd>3</kbd>: 見開き 左開き+表紙単独 / <kbd>4</kbd>: 見開き 右開き / <kbd>5</kbd>: 見開き 右開き+表紙単独)。<kbd>6</kbd>: 連結方式をページ単位 → 縦連結 → 横連結で循環。<kbd>7</kbd>: 横方向 左→右 / 右→左を切替。<kbd>0</kbd>: ズーム/フィットをページ全体 → 横幅フィット → 縦幅フィット → 100%原寸で循環。余白カットは左パネルの表示トリムで設定する。見開き中は表紙あり/なしを保ったまま左開き / 右開きも連動して切り替える。ZIP の作品区切り表示上でも有効。ホバーバーの表示モード/フィットボタンからも切替可 |

連結モード (縦連結 / 横連結) は画像 / ZIP 内画像 / PDF ページの通常閲覧用。比較、360度パノラマ、分析、消しゴム、隠蔽加工、テキスト注釈、補正レイヤーなどの編集・解析系モードはページ単位モードでのみ起動する。
| <kbd>R</kbd> / <kbd>L</kbd> | 右 / 左 90° 回転 |
| <kbd>Z</kbd> (ホールド/短押し) | ZipPla 風 全画面ズームモード (v2.0.0)。**押している間**は画像全体を表示してズーム範囲の枠を出し、カーソルで枠を移動・ホイールで枠サイズ (= 倍率) を変える。**離す**と枠の範囲を画面いっぱいに拡大し、以後マウス移動で表示範囲をパン (元画像範囲内のみ、余白なし)。**ズーム確定後のホイールは通常どおり前後ページ移動** (ZipPla 準拠)、倍率を変えたいときは <kbd>Ctrl</kbd>+ホイール。**もう一度 <kbd>Z</kbd> で元の表示へ戻る**。短押しでもその場でズーム。既定倍率は単ページ=cover (縦長で横幅目一杯) / 見開き=単ページ幅の約 1.2 倍。ズーム中は左右の補正/メタデータパネルを抑止し、パンは上下のホバー領域へ入る前に画像の上端・下端へ到達する (操作帯方式)。PDF はズーム倍率で高解像度へ再レンダ、表示トリム中はトリム後範囲のみパン。前後ページへ移動してもズーム状態・倍率はセッション内で維持する。単ページ・見開きの通常閲覧で動作 (連結 / パノラマ / 動画 / 分析モードでは無効)。現行ルーペ (<kbd>M</kbd> / <kbd>Shift</kbd>) とは別物 |
| ホイールクリックに全画面ズームモードを割り当て | 短クリックでズーム状態を ON/OFF する。Z キーの押下中照準表示は出さず、現在のカーソル位置でズーム状態へ入る。ズーム状態中は中ボタン押し込み + 上下ドラッグで倍率を変えられる |
| <kbd>Shift</kbd>+<kbd>Z</kbd> | 画像分析モード (旧 <kbd>Z</kbd>。Z を全画面ズームへ明け渡したため移動) |
| <kbd>S</kbd> | スライドショー 再生 / 停止。ホバーバーの ▶ ボタンは停止中のみページ送り間隔・連結読みスクロール設定・末尾動作を設定するポップアップを開き、再生中の一時停止ボタンは即停止する。動画はスキップして継続。縦/横連結読み中は待機時間ごとに設定割合だけ短時間スクロールし、終端で通常の末尾動作へ合流する。フォルダ内移動 (矢印 / ホイール / クリック / Home / End) では止まらず、Ctrl+↑↓ のフォルダ移動・S・Space・Esc で止まる |
| <kbd>M</kbd> | ルーペ トグル。360 度パノラマモード中とテキスト注釈モード中は表示しない。テキスト注釈モードに入ってもトグル状態は保持する |
| <kbd>Shift</kbd> (押しっぱ) | ルーペ。360 度パノラマモード中とテキスト注釈モード中は表示しない。Action: `FsLoupeHold` |
| <kbd>G</kbd> | ピクセルグリッド表示 ON/OFF (ユーザーズームが等倍より拡大中、かつ高倍率時のみ画像ピクセル境界を表示) |
| <kbd>B</kbd> | 透過背景色サイクル (黒 → 白 → 市松)。AI アップスケール時は黒 ↔ 白の 2 段 (市松は出力に焼き込まれるため不可) + 背景変更時に `clear_adjustment_caches` を呼び背景別 `(idx,bg)` 結果を表示し直す (idx キーの派生キャッシュ取り違えによる固着防止)。透過 (alpha) の無い画像では無効化してトースト案内 |
| <kbd>E</kbd> | 消しゴムモード開始 / 確定 |
| <kbd>Ctrl</kbd>+<kbd>G</kbd> | 補正レイヤーモードを開始する。Action: `FsLocalAdjustMode`。画像フルスクリーンの操作カスタマイズ対象。モード終了は <kbd>Esc</kbd> |
| <kbd>Ctrl</kbd>+<kbd>M</kbd> | 隠蔽加工モード開始 / 終了 |
| <kbd>F7</kbd> / <kbd>F8</kbd> | 消しゴムマスクスロット 1 / 2 を現在ページに即適用 |
| <kbd>F9</kbd> / <kbd>F10</kbd> | 隠蔽マスクスロット 1 / 2 を現在ページに即適用 |
| <kbd>Shift</kbd>+<kbd>F7</kbd> / <kbd>Shift</kbd>+<kbd>F8</kbd> | 現在ページの消しゴムマスクを削除 |
| <kbd>Shift</kbd>+<kbd>F9</kbd> / <kbd>Shift</kbd>+<kbd>F10</kbd> | 現在ページの隠蔽マスクを削除 |
| <kbd>Ctrl</kbd>+<kbd>E</kbd> | 現在の表示結果を別ファイルへエクスポート |
| <kbd>Ctrl</kbd>+<kbd>Alt</kbd>+<kbd>Shift</kbd>+<kbd>D</kbd> | 画像パイプラインのデバッグ出力 (`%APPDATA%\mimageviewer\debug-pipeline\...` に段階別 PNG と manifest を保存) |
| <kbd>P</kbd> | 現在表示中アイテムを現在のコンテナの代表サムネに固定 / 解除 |
| <kbd>X</kbd> | 現在表示中を比較スロットへピン留め / 同じ画像なら解除 |
| <kbd>C</kbd> | 比較スロットのピン画像と現在画像をトグル表示 |
| <kbd>Shift</kbd>+<kbd>C</kbd> | Wipe 比較を ON/OFF (左=ピン、右=現在) |
| <kbd>Alt</kbd>+<kbd>C</kbd> | 差分比較を ON/OFF (RGB チャンネルごとの差分を色付きで強調表示) |
| <kbd>U</kbd> / <kbd>Shift</kbd>+<kbd>U</kbd> / <kbd>Alt</kbd>+<kbd>U</kbd> | AI モデル 次 / 前 / リセット。標準キーなしの `FsAiModelAuto` / `FsAiModelRealEsrganX4Plus` / `FsAiModelRealEsrganAnime6B` / `FsAiModelRealCugan4x` / `FsAiModelNmkdSiax4x` / `FsAiModelRealEsrGeneralV3` にキーを割り当てると、任意の AI モデルへ直接切り替えられる |
| <kbd>T</kbd> / <kbd>Shift</kbd>+<kbd>T</kbd> / <kbd>Alt</kbd>+<kbd>T</kbd> | ポストフィルタ 次 / 前 / 標準 (リセット)。標準キーなしの `FsPostFilter...` 系 Action にキーを割り当てると、任意のポストフィルタへ直接切り替えられる |
| <kbd>V</kbd> | 360 度パノラマモード トグル (360 候補画像のみ) |
| <kbd>Ctrl</kbd>+<kbd>S</kbd> | 現在画像 / アニメーション現在フレーム / ZIP 内画像 / PDF ページをキャプチャ保存フォルダへ保存 |

## 消しゴムモード

| 入力 | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | 消しゴムモードで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いの操作は別枠で表示する |
| <kbd>E</kbd> / <kbd>Esc</kbd> | 補完を実行して終了。選択中オブジェクトがあるときの <kbd>Esc</kbd> はまず選択解除 |
| <kbd>S</kbd> / <kbd>B</kbd> / <kbd>L</kbd> / <kbd>I</kbd> / <kbd>V</kbd> / <kbd>H</kbd> / <kbd>R</kbd> / <kbd>O</kbd> | 選択 / 筆 / 囲み / 直線 / 縦線 / 横線 / 矩形 / 楕円ツール |
| <kbd>D</kbd> / <kbd>F</kbd> | 描画 / 消去モード切替 |
| <kbd>Space</kbd>+左ドラッグ | 一時パン |
| <kbd>Ctrl</kbd>+マウスホイール | ズーム。ツールパネル上でも同じ |
| マウスホイール | 画像上ではズーム。ツールパネル上ではパネルスクロール |
| 矢印 / <kbd>Ctrl</kbd>+矢印 | マスクまたは選択オブジェクトを 1px / 10px 移動 |
| <kbd>[</kbd> / <kbd>]</kbd>, <kbd>Ctrl</kbd>+<kbd>[</kbd> / <kbd>]</kbd> | マスクまたは選択オブジェクトを ±0.1° / ±1° 回転 |
| <kbd>Enter</kbd> | 多角形マスクの頂点列を確定。Action: `EraseConfirmPolygon` |
| <kbd>Shift</kbd>+ハンドル | 端点角度・回転角をスナップ、矩形/楕円の角リサイズを等比化 |
| <kbd>Alt</kbd>+ハンドル | 矩形/楕円を中心固定でリサイズ |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> | マスク編集 Undo |
| <kbd>Del</kbd> | 選択中オブジェクトを削除 |

## 隠蔽加工モード

| 入力 | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | 隠蔽加工モードで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いの操作は別枠で表示する |
| <kbd>Ctrl</kbd>+<kbd>M</kbd> | 隠蔽加工モード終了 |
| <kbd>Esc</kbd> | 選択中オブジェクトがあるときは選択解除、なければ隠蔽加工モード終了 |
| <kbd>T</kbd> | 隠蔽タイプを順に切替 |
| <kbd>G</kbd> | ピクセルグリッド表示 ON/OFF (ユーザーズームが等倍より拡大中、かつ高倍率時のみ画像ピクセル境界を表示) |
| <kbd>1</kbd>〜<kbd>4</kbd> | プリセット 1〜4 を呼び出し |
| <kbd>D</kbd> / <kbd>F</kbd> | 描画 / 消去モード切替 |
| <kbd>S</kbd> | 選択ツール |
| <kbd>B</kbd> | 筆ツール |
| <kbd>L</kbd> | 囲みツール |
| <kbd>I</kbd> | 直線ツール |
| <kbd>V</kbd> | 縦線ツール |
| <kbd>H</kbd> | 横線ツール |
| <kbd>R</kbd> | 矩形ツール |
| <kbd>O</kbd> | 楕円ツール |
| <kbd>Space</kbd>+左ドラッグ | 一時パン |
| <kbd>Ctrl</kbd>+マウスホイール | ズーム。ツールパネル上でも同じ |
| マウスホイール | 画像上ではズーム。ツールパネル上ではパネルスクロール |
| 矢印 / <kbd>Ctrl</kbd>+矢印 | 選択オブジェクト、またはオブジェクト全体を 1px / 10px 移動 |
| <kbd>Shift</kbd>+ハンドル | 端点角度・回転角をスナップ、矩形/楕円の角リサイズを等比化 |
| <kbd>Alt</kbd>+ハンドル | 矩形/楕円を中心固定でリサイズ |
| <kbd>Enter</kbd> | 多角形マスクの頂点列を確定。Action: `ConcealConfirmPolygon` |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> | マスク編集 Undo |
| <kbd>Del</kbd> | 選択中オブジェクトを削除 |

## 補正レイヤーモード

| 入力 | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | 補正レイヤーモードで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いの操作は別枠で表示する |
| <kbd>Q</kbd> | 補正レイヤー直前の元画像表示 ON/OFF |
| <kbd>W</kbd> | 選択中レイヤーのマスク表示 ON/OFF |
| <kbd>D</kbd> / <kbd>F</kbd> | 手動マスクの描画 / 消去モード切替 |
| <kbd>B</kbd> / <kbd>A</kbd> / <kbd>G</kbd> | 筆 / 境界筆 / すき間塗りツール |
| <kbd>L</kbd> / <kbd>P</kbd> | 囲み / 多角形ツール |
| <kbd>S</kbd> | 選択ツール |
| <kbd>I</kbd> / <kbd>V</kbd> / <kbd>H</kbd> | 直線 / 縦線 / 横線ツール |
| <kbd>R</kbd> / <kbd>O</kbd> | 矩形 / 楕円ツール |
| <kbd>Space</kbd>+左ドラッグ | 一時パン |
| <kbd>Esc</kbd> | 編集中の図形操作を解除。解除対象がなければ補正レイヤーモード終了 |
| <kbd>Enter</kbd> | 多角形マスクの頂点列を確定。Action: `LaConfirmPolygon` |
| <kbd>Del</kbd> | 選択中の図形マスクを削除 |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> | 多角形入力中は頂点を戻す。それ以外は補正レイヤー操作を Undo |
| <kbd>Ctrl</kbd>+<kbd>Y</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> | 補正レイヤー操作を Redo |
| 矢印 / <kbd>Ctrl</kbd>+矢印 | 選択中の図形マスクを 1px / 10px 移動 |
| <kbd>[</kbd> / <kbd>]</kbd>, <kbd>Ctrl</kbd>+<kbd>[</kbd> / <kbd>]</kbd> | 選択中の図形マスクを ±0.1° / ±1° 回転 |
| <kbd>Shift</kbd>+ハンドル | 端点角度・回転角をスナップ、矩形/楕円の角リサイズを等比化 |
| <kbd>Alt</kbd>+ハンドル | 矩形/楕円を中心固定でリサイズ |
| <kbd>Ctrl</kbd>+マウスホイール | ズーム。ツールパネル上でも同じ |
| マウスホイール | 画像上ではズーム。ツールパネル上ではパネルスクロール |

## 切り取りモード

| 入力 | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | 切り取りモードで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いの操作は別枠で表示する |
| <kbd>Ctrl</kbd>+<kbd>E</kbd> | 切り取りを確定し、現在の表示結果を別ファイルへエクスポートする |
| <kbd>Esc</kbd> | 切り取りモードを終了する |
| <kbd>Space</kbd>+左ドラッグ | 一時パン |
| マウスドラッグ | 切り取り範囲を作成、移動、またはリサイズする |
| <kbd>Ctrl</kbd>+マウスホイール | ズーム。ツールパネル上でも同じ |
| マウスホイール | 画像上ではズーム。ツールパネル上ではパネルスクロール |

## テキスト注釈モード

| 入力 | 動作 |
|---|---|
| <kbd>?</kbd> (既定) | テキスト注釈モードで使えるショートカット一覧を表示する。Action: `HelpShowContextShortcuts`。keymap 化済み操作は現在読み込まれている割り当て済みのものを表示し、固定扱いの操作は別枠で表示する。本文入力や検索欄などが keyboard focus を持つときは入力を優先する |
| <kbd>Ctrl</kbd>+<kbd>T</kbd> | テキスト注釈モードを確定または終了する |
| <kbd>Ctrl</kbd>+<kbd>Z</kbd> | テキスト注釈編集を Undo。本文入力欄が keyboard focus を持つときは TextEdit の Undo を優先する |
| <kbd>Ctrl</kbd>+<kbd>Y</kbd> / <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>Z</kbd> | テキスト注釈編集を Redo。本文入力欄が keyboard focus を持つときは TextEdit の Redo を優先する |
| <kbd>Space</kbd>+左ドラッグ | 一時パン。注釈の移動中は現在のドラッグを継続する |
| <kbd>Esc</kbd> | 選択を解除する。未選択ならテキスト注釈モードを終了する。本文入力中は TextEdit / IME を優先する |
| <kbd>Del</kbd> / <kbd>Backspace</kbd> | 選択中の注釈を削除する。本文入力中はテキスト入力を優先する |
| ドラッグ | 画像上で注釈を選択、移動、またはハンドル編集する |
| <kbd>Shift</kbd>+四隅ハンドル | ドラッグ開始時の縦横比を維持して拡大・縮小する。<kbd>Ctrl</kbd> と併用すると中心固定の等比リサイズ。番号バッジとスタンプは <kbd>Shift</kbd> なしでも常に縦横比を維持する |
| <kbd>Ctrl</kbd>+四隅ハンドル | ドラッグ開始時の中心を固定した対称リサイズ。<kbd>Shift</kbd> と併用可能 |
| マウスホイール / <kbd>Ctrl</kbd>+マウスホイール | 画像上ではズーム。パネル上ではスクロールまたはズーム |
| 中ボタン+上下ドラッグ | ドラッグ開始位置を中心にズームする |
| 右 <kbd>Ctrl</kbd> | 押している間だけテキスト注釈を外した元画像を表示する |

## 動画 フルスクリーン (Phase 7.H 適用後)

| キー / 入力 | 動作 | 備考 |
|---|---|---|
| <kbd>?</kbd> (既定) | 現在の動画フルスクリーンコンテキストで使えるショートカット一覧を表示 | Action: `HelpShowContextShortcuts`。egui 経路と Windows native 動画 overlay の両方で対応 |
| <kbd>Space</kbd> / <kbd>Enter</kbd> | 再生 / 一時停止トグル | Action: `VideoPlayPause`。動画 HUD 2 段化リデザイン (Phase 1) で Space を再生/停止に変更 (旧: 選択トグル)。チェックしたい場合は Esc で一覧へ戻る |
| 既定キーなし | 動画フルスクリーンを閉じて一覧へ戻る | Action: `VideoCloseFullscreen`。<kbd>Esc</kbd> は固定入力として残しつつ、操作カスタマイズで追加の閉じるキーを割り当てられる |
| <kbd>Backspace</kbd> | 一覧へ戻る | Action: `FsBackToList`。画像フルスクリーンと同じ。native presenter 経路でも App 側へ転送する |
| <kbd>Home</kbd> / <kbd>End</kbd> | 先頭 / 末尾の項目へ移動 | Action: `FsJumpFirst` / `FsJumpLast`。native presenter 経路でも App 側へ転送する |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> | 外部プレイヤー起動 | Action: `VideoExternalPlayer` |
| <kbd>←</kbd> / <kbd>→</kbd> | 5 秒シーク (デフォルト) | |
| <kbd>Shift</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 1 秒シーク (細かい) | Action: `VideoSeekBackSmall` / `VideoSeekForwardSmall` |
| <kbd>Ctrl</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 30 秒シーク (大きい) | Action: `VideoSeekBackLarge` / `VideoSeekForwardLarge` |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 1 フレーム戻る / 進む | Action: `VideoFrameStepBack` / `VideoFrameStepForward` |
| <kbd>←</kbd> / <kbd>→</kbd> (タイル中) | タイルカーソルを前 / 次へ移動 | seek しない。現在位置より後の最初のタイルを時刻ラベル込みで強調表示 |
| <kbd>Ctrl</kbd>+<kbd>←</kbd> / <kbd>→</kbd> (タイル中) | タイルカーソルを 1 行分移動 | 列数分だけ前 / 次へ移動 |
| <kbd>Space</kbd> / <kbd>Enter</kbd> (タイル中) | タイルカーソル位置から再生 | S / Esc で閉じた場合は再生位置を変更しない |
| <kbd>P</kbd> (タイル中) | タイルカーソル位置のサムネイルを代表フレームとしてピン留め | マウス hover ではカーソルを動かさない。マウス操作はタイルクリックだけが seek として反応する |
| <kbd>↑</kbd> / <kbd>↓</kbd> | **前 / 次のファイル** (画像と同じ、マウスホイールと同じ) | Phase 7.H |
| <kbd>Shift</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 音量を dB フェーダー目盛りの 1/4 幅で上下 | |
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 現在コンテキストの前 / 次フォルダまたは検索結果へ移動 | native presenter 経路でも有効。切り離した detached 窓 / always-new 窓ではメイン一覧を動かさず、案内だけ出す |
| <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> | 前 / 次の兄弟フォルダへ | 同じ親の直下だけを対象にし、空フォルダも skip しない。検索中は無効。切り離した detached 窓 / always-new 窓では無効 |
| <kbd>M</kbd> | ミュート トグル | |
| <kbd>L</kbd> | ループ再生 トグル | 連続再生 ON 中は無効化し、「連続再生中はループ無効」を表示 |
| <kbd>B</kbd> | ブックマーク追加 (現在位置 🔖) | |
| <kbd>S</kbd> | タイルモード ON/OFF | |
| HUD 連続再生ボタン | オフ → 連続再生 → 連続再生 + ループを循環 | ループ再生とは排他。アプリ再起動時は OFF |
| <kbd>Ctrl</kbd>+<kbd>S</kbd> | 現在フレームをキャプチャ保存フォルダへ保存 | v0.10 MVP。egui / native presenter 両経路で有効 |
| <kbd>X</kbd> / <kbd>C</kbd> / <kbd>Shift</kbd>+<kbd>C</kbd> / <kbd>Alt</kbd>+<kbd>C</kbd> | 比較ビュー対象外のため silent no-op | native presenter 経路でも passthrough しない |
| <kbd>P</kbd> | 現在再生位置をピン留め (= HUD 📌 ボタンと同等)。タイル中はタイルカーソル位置をピン留め | v0.9.x、グリッドの P (folder_thumb_pin toggle) と統一した「P = Pin」 |
| <kbd>F</kbd> | Perf / フレームレート オーバーレイ トグル | v0.9.x、以前は P。P を Pin に再割り当てしたため F (Frames) へ移動 |
| <kbd>Esc</kbd> (タイル中) | タイルモード解除 | |
| マウス左クリック | 再生 / 一時停止トグル (HUD/パネル除く) | |
| マウスホイール | 前 / 次のファイル | 画像と同じ |
| <kbd>Ctrl</kbd>+ホイール (タイル中) | 列数切替 (4/6/10/16/20/26/30) | 上部バーの 3x3 / 5x5 アイコンボタンでも同じ操作可 |

## 不整合の解消 (Phase 7.H 適用後)

| 入力 | 画像モード | 動画モード | 状態 |
|---|---|---|---|
| <kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次ファイル | 前 / 次ファイル | ✅ 揃った |
| マウスホイール | 前 / 次ファイル | 前 / 次ファイル | ✅ 揃った |
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次フォルダまたは検索結果 | 前 / 次フォルダまたは検索結果 | ✅ 揃った。切り離した detached / always-new では両方 no-op |
| <kbd>Shift</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次ファイル (= ↑↓ と同義)。スタックのフラット読書中は前 / 次スタックへジャンプ (v2.0.0) | 音量を dB フェーダー目盛りの 1/4 幅で上下 | ⚠ 残った差異 (許容、動画プレイヤー慣例) |
| <kbd>←</kbd> / <kbd>→</kbd> | 前 / 次ファイル | 5 秒シーク | ⚠ 動画プレイヤー慣例 (mpv/VLC/YouTube) で許容 |
| マウス左クリック | ページめくり | 再生 / 一時停止 | ⚠ 動画プレイヤー慣例で許容 |

## ★固定 (Snapshot Lock) 中の挙動

★ filter ツールバー右端の `[★固定]` ボタンで現在の絞り込み結果を一時 snapshot 化して
凍結する機能 (v1.1.0+、設計: [star-lock-snapshot-design.md](star-lock-snapshot-design.md))。
snapshot 中のキー操作は以下のように再定義される:

| 入力 | snapshot 中の挙動 |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | snapshot 内の前/次 entry へ (= 混合 nav: Folder/Image/Video 全部対象、Folder entry は中の最初の image を fullscreen で開く) |
| <kbd>Ctrl</kbd>+<kbd>PageUp</kbd> / <kbd>PageDown</kbd> | snapshot 内の前/次 **image-like** entry のみへ (Folder/Zip/Pdf entry は skip) |
| スライドショー末尾 | snapshot 内の次 playable entry へ自動遷移 (= ★5 folder 巡回の主用途) |
| ★ filter ボタン | 操作可能、ただし top-level grid 表示は snapshot のまま凍結 (= captured folder の中身には作用する) |
| <kbd>Backspace</kbd> / <kbd>Alt</kbd>+<kbd>↑</kbd> / <kbd>Alt</kbd>+<kbd>←</kbd>/<kbd>→</kbd> / フォルダツリー / お気に入りクリック / フォルダパス入力 | 無効、toast「スナップショット中は他のフォルダに移動できません」 |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> / <kbd>Ctrl</kbd>+<kbd>S</kbd> / <kbd>Ctrl</kbd>+<kbd>G</kbd> / <kbd>Ctrl</kbd>+<kbd>T</kbd> | snapshot 自動解除 + 検索 mode 起動 (= scope mutual exclusion) |
| `[★固定]` ボタン再クリック | snapshot 解除 (= 元のフォルダ表示に戻る) |

snapshot 末尾到達時は `FsBoundaryHint::NoImageFolder` で boundary hint を表示。

## 設計メモ

- 動画モードで ↑↓ をファイル移動に再アサインする方針は、旧 egui 経路では
  `handle_video_input` がプレーン ArrowUp/ArrowDown を consume せず後段へ流すことで
  実現していた。現行 Windows native presenter 経路では
  `app/native_video.rs::handle_native_video_key_event` が plain ↑↓ を直接
  `navigate_native_video_fullscreen` に流している。
- Ctrl+↑↓ も同じ思想で native key handler から
  `handle_fullscreen_ctrl_nav_context` へ流し、フォルダ / Ctrl+S / Ctrl+G の
  スコープ解決を画像系と共有する。
- **Shift+↑↓ のスタックジャンプ (v2.0.0) は 2026-06 の Phase 4 初期実装で
  `KeyAction::FsStackJumpPrev` / `FsStackJumpNext` 化した。** 同じ Shift+↑↓ は
  ① 非スタック時のプレーン ↑↓ エイリアス (ページ送り)、② 動画再生中の音量、
  ③ スタックのフラット読書中のスタックジャンプ、の 3 用途をコンテキストで出し分ける。
  `ui_fullscreen.rs` では動画の `VideoVolume*` / `VideoNextFile` / `VideoPrevFile`
  を先に扱い、非動画かつスタックフラット時だけ `FsStackJump*` を見る。これにより
  native 動画経路と App 側の縦方向 keymap 解決を近づけつつ、通常の Esc /
  plain 矢印ナビゲーションは固定扱いのまま残す。Enter / Backspace / Home / End は
  文脈ごとの `KeyAction` として扱う。
- **ZipPla 風 全画面ズームは `KeyAction::FsZoomMode` (KeyHold トリガ、既定 <kbd>Z</kbd>) として扱う**。
  「押している間=照準 (枠表示) / 離す=ズーム確定 / ズーム中の押下=解除」というホールド + トグルの
  ハイブリッドだが、`KeyHold` アクション基盤 (Shift ルーペ `FsLoupeHold` / 編集モードの Space パンと
  同じ枠組み) に乗せてカスタマイズ可能にした。押下状態は `keymap.key_held_action` (OS 直読み =
  フルスクリーンビューポートで stale な egui key_down を回避) で取り、高速タップ (idle からの同
  フレーム押下+離し) は `keymap.take_key_hold_edges` で補完する。Windows では held と edge の
  両方を Win32 物理入力へ統一し、`Enter` / `NumpadEnter` は extended bit 別の押下ラッチを
  参照する。その他の固有 VK は従来どおり `GetAsyncKeyState` を使う (Codex P2)。
  状態 (`fs_zoom_active` / `fs_zoom_aiming` / `fs_zoom_factor`) は settings に保存せずアプリ
  セッション内のみ保持し、グリッドへ戻ると解除 (倍率は維持)。
- **画像分析モードは `FsAnalysis` → `FsImageAnalysis` へ改名し、既定 chord を <kbd>Shift</kbd>+<kbd>Z</kbd>
  へ移動**した (v2.0.0)。改名により、旧バージョンの `keymap.ini` に残る `FsAnalysis = …` の割当ては
  **未知アクションとして無視され**、全員が新既定 (Z=ズーム / Shift+Z=分析) へ移行する (旧 Z=分析の
  カスタムと固定ズームが衝突するのを避けるための clean break)。分析を別キーへ割り当てていた場合も
  新名 `FsImageAnalysis` で設定し直す。`FsZoomMode` と `FsImageAnalysis` はどちらも `keymap.ini` で
  カスタマイズ可能。
- 5/1/30 秒シークの粒度は動画プレイヤー一般の慣例 (mpv: ←→=5s, Shift+←→=1s,
  ←/→ alone in YouTube=5s, J/L=10s) を踏襲しつつ、modifier で粒度切替できる
  ようにした。
- 既に先頭 / 末尾に居て ←→ シークが動かない場合は、シークを発行せず
  「動画先頭です」「動画末尾です」のトーストを出す (詳細は
  [video-architecture.md](video-architecture.md) の seek HUD 節)。
- タイルモード中の ←→ は修飾キーの有無に関係なくタイルカーソル移動を優先する。
  Shift は無視し、Ctrl が含まれる場合だけ 1 行分移動にする。
- 音量は HUD 下部の dB フェーダーをマウスでドラッグして調整可能。
  キーボードでは Shift+↑↓ で -∞/-60/-40/-20/-10/-5/0/+6/+12/+18dB の
  目盛り間を 1/4 幅ずつ移動し、plain ↑↓ アサインは廃止 (= プレーン ↑↓ を
  ファイル移動に譲るため)。
