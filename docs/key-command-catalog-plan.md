# キー操作コマンドカタログ化計画

> ステータス: **Phase 6 menu layout editor まで実装済み / ClaudeCode レビュー済み。native 動画 overlay ヘルプ重なり補正もレビュー済み。v2.2.0 重要変更点 entry 追加済み / ClaudeCode レビュー済み。旧 `keymap.ini` → `Settings.keymap` 一回移行済み。設定メニュー「操作カスタマイズ」独立ダイアログ移設 + コマンド一覧整理スライス実装中** (2026-06-25)。
> 既存の簡易 keymap 実装は [key-customization-impl-plan.md](key-customization-impl-plan.md)、
> 現行キー仕様は [keymap-spec.md](keymap-spec.md) を正とする。本書はその次段階として、
> 「デフォルト未割り当ての操作にもキーを割り当てられる」状態へ進めるための段階計画。
>
> **改訂メモ (2026-06-24, ClaudeCode / Codex レビュー反映)**: Phase 1 は `CommandId` /
> `CommandSpec` の本格導入を**見送り**、既存 `KeyAction` の小さな拡張に絞る (空 `default_chords`
> の許可・`GridToggleStackMode` 追加・`none` 未割り当て行の生成・表示用 helper まで)。
> `CommandId` / `CommandScope` / 衝突判定は、実際に必要になる Phase 2 以降へ後ろ倒しする。
> §3.1 / §4 / §6 / §7 はこの方針で書き直してある。
>
> **Phase 1 実装メモ (2026-06-24, Codex)**: `KeyAction::GridToggleStackMode` を既定未割り当てで追加し、
> 空 `default_chords()`、`# Action = none` 生成、`effective_chords()` /
> `first_chord_label()` / `compact_single_key_label()`、グリッド側キーハンドラへの最小配線まで実装。
> `CommandId` / `CommandSpec` / scope 衝突判定は未導入のまま。ClaudeCode レビュー済み。
>
> **GUI 正本化メモ (2026-06-25, Codex)**: コマンド設定 GUI に備え、キー割り当ての正本を
> `Settings.keymap` (`settings.db`) に移す。旧 `%APPDATA%\mimageviewer\keymap.ini` がある環境では
> 初回起動時に 1 回だけ読み込み、同じ override を settings.db へ保存してから
> `keymap.ini.imported*.bak` へ退避する。以後 `keymap.ini` は通常読み込み対象外とし、
> `keymap.ini.default` は Action 名と既定キーの参照ファイルとして残す。
>
> **コマンド設定 GUI 初期スライス実装メモ (2026-06-25, Codex)**:
> 環境設定「表示 → コマンド」として初期実装した後、実機確認で狭い環境設定ペインでは編集 UI /
> 競合表示 / Esc 挙動が扱いにくいことが分かったため、設定メニュー「操作カスタマイズ…」の
> 独立した大きめのダイアログへ移設する。
> `Settings.keymap` の上書きを GUI から編集できるようにする。
> 一覧は `KeyAction` 全体を対象にし、Action 名 / 説明 / コンテキストで絞り込める。
> 1 コマンド最大 3 キーを入力でき、「押して入力」で通常キー系の chord を取り込める。
> 割り当て解除・既定復帰・全体既定化を提供する。
> `BindingConflict` は保存禁止にせず警告として表示し、競合一覧から各コマンドの編集欄へ移動できる。
> OK 反映時に runtime `Keymap` と native 動画転送用 shortcut snapshot も更新する。
> 右ドラッグ mode 4 文脈化、リングショートカット、マウスジェスチャも同じ「操作カスタマイズ」
> ダイアログにまとめる。現スライスでは既存の編集部品を大きなタブ式ダイアログへ移し、
> 「コマンド一覧」タブはキーボード / リング / マウス / ゲームパッド割り当ての一覧表示にし、
> キー編集本体は「キーボード」タブへ分離する。キーボードタブは `KeyContext` ごとの文脈タブを持ち、
> 競合一覧や一覧の編集ボタンから移動したときは該当文脈を自動選択する。
> 簡易キーボード図は F13〜F24 を F1〜F12 の上段に出し、Ctrl / Shift / Alt 切替、hover で
> 実割り当て表示、click で選択中コマンドの空きキー欄へ chord 入力する。
> キーボード図 / ゲームパッド図などの高度なビジュアル編集は後続で仕様を詰める。
>
> **Phase 2 初期実装メモ (2026-06-24, Codex)**: `KeyContext` を `CommandScope` として再利用し、
> `CommandSpec` / `BindingPolicy` / active scope 隣接表 / `BindingConflict` を `keymap.rs` に追加。
> ユーザー override が絡む同一 chord の Hard / ActiveOverlap / TriggerMismatch と、
> Esc / Enter / 修飾なし矢印キーの Reserved を起動時 warning として出す。設定拒否や dispatch 変更はしない。
>
> **Phase 3 初期実装メモ (2026-06-24, Codex)**: グリッド側に残っていた F7-F10 /
> Shift+F7-F10 のマスク一括適用・削除を `GridApplyErase1/2`、
> `GridApplyConceal1/2`、`GridDeleteEraseMask`、`GridDeleteConcealMask` として
> `KeyAction` 化。既定キーと実行順は従来どおりで、dispatch resolver はまだ導入しない。
>
> **Phase 4 初期実装メモ (2026-06-24, Codex)**: フルスクリーン縦方向だけを対象に、
> `FsStackJumpPrev/Next` と `VideoPrevFile/NextFile` の局所 resolver 配線を追加。
> `Keymap::resolve_first_action_for_chord` で active scope + priority の純粋判定をテストする。
> Esc / Enter / plain 矢印全体の自由化や、全面 dispatch resolver 置換はまだ行わない。
> ClaudeCode レビュー後、`FS_IMAGE_ACTIVE_SCOPES` / `FS_VIDEO_ACTIVE_SCOPES` を共有定数化し、
> resolver テストと実 dispatch が同じ active scope 定義を参照するようにした。
>
> **Phase 5 初期実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `Keymap::compact_action_label()` を追加し、消しゴム / 隠蔽加工パネルの
> 描画・消去ボタンとツールボタン (`筆 [B]` など) を実 keymap から表示するようにした。
> 修飾付き chord や未割り当てでは compact 表示を省略する。この時点ではメニュー表示、
> native 動画 overlay、ツールチップのフル表記は後続に残した。
>
> **Phase 5 メニュー表示スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `Keymap::first_chord_action_label()` を追加し、メニューバーの検索 / タグビュー項目と、
> 詳細表示・タグビュー・現在地フィルタの一部 hover text を先頭 chord 表示に追従させた。
> native 動画 overlay と、フルスクリーンホバーバー等の広範な shortcut 表示は後続。
>
> **Phase 5 フルスクリーンホバーバー表示スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `Keymap::first_chord_bracket_label()` / `chord_list_bracket_label()` を追加し、静止画
> フルスクリーンの上部ホバーバー tooltip と、表示モード / ズーム・フィット popup の
> shortcut 表記を実 keymap から表示するようにした。`Esc` / `F11` は固定扱いのまま。
> native 動画 overlay は当時後続に残した（後続スライスで対応済み）。
>
> **Phase 5 native 動画 overlay 表示スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `NativeOverlayShortcutLabels` を `NativeOverlayMetadata` に載せ、App 側で現在の
> `keymap.ini` effective chord から作った表示用 snapshot を native presenter へ渡すようにした。
> 動画 top bar / bottom HUD / jump panel / seek hover thumbnail の KeyAction 由来 shortcut 表記を
> この snapshot から表示する。`Esc` / `F11`、Ctrl+Shift+←/→、Ctrl+ホイールなど固定扱いの
> 入力は従来どおり。
>
> **Phase 5 レーティング tooltip 表示スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `Keymap::first_rating_chord_label()` / `rating_chord_summary_label()` を追加し、★フィルタ
> ボタン / スマートフィルタ内の★項目 / フォルダバー右側のコンテナ★ tooltip に出る
> `F1〜F6` / `Shift+F1〜F6` 表記を実 keymap 由来にした。既定割り当ては従来どおり範囲表記へ
> 畳み、カスタム時は `1:Alt+F1 / ...` のように明示する。
>
> **Phase 7 コンテキストヘルプ基盤スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `CommandDisplayRow` と `Keymap::command_display_rows_for_active_scopes()` を追加し、呼び出し側が
> 渡した active scope に対して、`CommandSpec` と実 keymap 由来の shortcut label 一覧を取得できる
> 純ロジックを用意する。未割り当て / `none` の Action は通常非表示、必要な場合だけ空 shortcut
> 行として含められる。`?` ヘルプ UI、dispatch、メニュー構成変更はまだ行わない。
>
> **Phase 7 グリッドヘルプ初期スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> グリッド文脈に限り、固定 `?` キーで「ショートカット」ダイアログを開く。Keymap 化済み操作は
> `GRID_ACTIVE_SCOPES` + `CommandDisplayRow` から実 effective chord を表示し、Enter / Backspace /
> 矢印 / F11 など当面固定扱いのグリッド操作は補助行として表示する。テキスト入力・IME・既存
> ダイアログ・フルスクリーン中は誤発火させない。画像 / 動画フルスクリーン、編集モード、
> `?` 自体の KeyAction 化は後続。
>
> **Phase 7 グリッドヘルプ未設定表示スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> グリッドヘルプ内で `include_unassigned=true` の `CommandDisplayRow` を使い、実 shortcut が
> 空の操作を「キー未設定 / 無効化中」として別枠表示する。これにより `GridToggleStackMode`
> のような既定キーなしだが割り当て可能な操作も、ヘルプから発見できる。通常の shortcut 一覧には
> 引き続き実 effective chord がある操作だけを表示する。
>
> **Phase 7 画像フルスクリーンヘルプスライス実装メモ (2026-06-24, Codex / ClaudeCodeレビュー反映済み)**:
> `?` ヘルプを通常の画像フルスクリーンにも広げる。フルスクリーン viewport 内で同じ
> ダイアログを描き、`FS_IMAGE_ACTIVE_SCOPES` (`Global` / `FsCommon` / `Rating` /
> `FsImage`) の実 effective chord と固定キーを表示する。テキスト入力・IME・モーダル中は
> 既存の fullscreen key guard に従い、編集モードと native 動画は別スライスに残す。
> ClaudeCode レビュー反映として、`FsStackJumpPrev/Next` と重複する固定キー行を削除し、
> main/root fallback 経路にもモーダル早期 return を追加した。
>
> **Phase 7 消しゴム / 隠蔽加工ヘルプスライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> 消しゴム / 隠蔽加工モード中の固定 `?` で同じ「ショートカット」ダイアログを開く。
> 通常フルスクリーンの `Global` / `FsCommon` 操作は編集モード中に発火しないため、
> 表示対象は `Erase` / `Conceal` scope の KeyAction に限定し、Esc / Enter / 矢印 /
> ホイールなどの固定扱い入力は補助行に分ける。補正レイヤー、動画フルスクリーン、
> テキスト注釈は後続スライスで対応済み。
>
> **Phase 7 切り取りヘルプスライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> 切り取りモード中の固定 `?` で同じ「ショートカット」ダイアログを開く。表示対象は
> `Crop` scope の KeyAction (`CropExecute` / `CropSpacePan`) に限定し、Esc / ドラッグ /
> ホイールなどの固定扱い入力は補助行へ分ける。補正レイヤー、動画フルスクリーン、
> テキスト注釈は後続スライスで対応済み。
>
> **Phase 7 補正レイヤーヘルプスライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> 補正レイヤーパネル中の固定 `?` で同じ「ショートカット」ダイアログを開く。表示対象は
> `LocalAdjust` scope の KeyAction に限定し、通常フルスクリーンの `Global` / `FsCommon`
> 操作は編集モード中に混ぜない。Esc / Enter / Delete / 矢印 / ブラケット / Undo・Redo /
> マウス操作などの固定扱い入力は補助行へ分ける。パネル内のテキスト入力や数値入力が
> keyboard focus を持つときは `?` を奪わない。動画フルスクリーンとテキスト注釈は
> 後続スライスで対応済み。
>
> **Phase 7 egui 動画ヘルプ初期スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> egui 経路の動画フルスクリーンでも固定 `?` で同じ「ショートカット」ダイアログを開く。
> 表示対象は `FsVideo` scope と、動画中にも実際に届く `Rating` / `FsCommon` の一部
> (`FsToggleMetadata`, `FsCtrlNav*`, `FsSibling*`) に限定する。`VideoCompare*` は動画では
> silent no-op として消費するだけなので、ヘルプには出さない。Esc / Backspace / シーク /
> Home / End / F11 / ホイールなどの固定扱い入力は補助行へ分ける。native 動画 overlay 上で
> 直接開くヘルプは後続スライスに残す。
>
> **Phase 7 native 動画 overlay ヘルプスライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> Windows native presenter 上でも `?` の Text イベントで動画フルスクリーン用ショートカット
> ヘルプを開く。App 側で `NativeOverlayShortcutHelp` snapshot を作って `NativeOverlayMetadata`
> に載せ、presenter は keymap を直接参照せず所有済み文字列だけを描画する。表示対象は
> egui 動画ヘルプと同じく、動画中に実際に有効な `FsVideo` / 一部 `FsCommon` / `Rating` /
> `ToggleDetachedViewerMode` に限定し、`VideoCompare*` は出さない。ヘルプ表示中は
> presenter 内で Esc を閉じる操作として消費し、App 側へのキー・Text・ホイール転送を抑止する。
> 実機補正として、`?` は `WM_CHAR` の ASCII / 全角 `？` に加え、`WM_CHAR` が届かない
> native ウィンドウ環境向けに `Shift+VK_OEM_2` KeyDown でも開く。
> ヘルプは native overlay 内のモーダルとして扱い、表示中は右メタデータパネル / 左ジャンプパネルの
> hover 表示を抑止する。右上の閉じるボタンへカーソルを移動したときに右パネルが重なって
> 閉じられなくなる実機症状への補正。
>
> **Phase 7 テキスト注釈ヘルプスライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> テキスト注釈モード中の固定 `?` で同じ「ショートカット」ダイアログを開く。表示対象は
> `Text` scope の `TextConfirm` / `TextUndo` / `TextRedo` / `TextSpacePan` と、Esc /
> Delete / Backspace / ドラッグ / ホイール / 右Ctrl などの固定扱い入力。本文やフォント検索など
> TextEdit が keyboard focus を持つときは `?` を奪わない。
>
> **Phase 7 ヘルプキー KeyAction 化スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `HelpShowContextShortcuts` を `Global` scope の `KeyAction` として追加し、既定を `?`
> (内部的には `Shift+/`) にする。egui 側の各ヘルプ dispatch、ヘルプ固定キー欄、native 動画
> overlay の Text / KeyDown fallback はこの Action の effective chord を参照する。
> ユーザーが `HelpShowContextShortcuts = F1` や `none` に変更した場合も、入力判定と表示が一致する。
>
> **Phase 6 メニュー command catalog 基盤スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `MenuCommandId` / `MenuCommandSpec` / `TopMenuId` を `keymap.rs` に追加し、まず既に
> shortcut 表示が keymap 追従している top menu 項目 (現在地フィルタ / コンテナ検索 /
> アイテム検索 / タグビュー) だけを catalog 化する。`Keymap::menu_command_label()` から
> 既存と同じラベルを作るように `render_menubar()` を差し替えるが、メニュー構成・クリック処理・
> 保存形式・UI はまだ変更しない。
>
> **Phase 6 ファイルメニュー静的項目 catalog スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `MenuCommandId::ALL` を追加して enum ⇄ catalog の drift をテストで検知する。ファイルメニューの
> 静的項目 (フォルダを開く / 読書履歴 / 現在地フィルタ / キャプチャ保存フォルダ / ゴミ箱 / 終了)
> を `MenuCommandSpec` へ追加し、`render_menubar()` のラベル取得を catalog 経由にする。
> `フォルダを開く…` は既存 `GlobalOpenFolder` と紐付け、表示上の shortcut も keymap 追従にする。
> クリック処理・メニュー構成・保存形式は変更しない。
>
> **Phase 6 お気に入りメニュー静的項目 catalog スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> お気に入りメニューの静的項目 (`このフォルダを追加…` / `編集`) を `MenuCommandSpec` へ追加し、
> `render_menubar()` のラベル取得を catalog 経由にする。動的なお気に入り一覧は catalog 対象外のまま。
> enabled 判定・hover tip・クリック処理・メニュー構成・保存形式は変更しない。
>
> **Phase 6 タグメニュー静的項目 catalog スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> タグメニューの固定ラベル項目 (`ピン留めタグの管理…`) を `MenuCommandSpec` へ追加し、
> `render_menubar()` のラベル取得を catalog 経由にする。件数付きのタグ付け / 旧 XMP 操作、
> ピン留めタグ由来の動的一覧とサブメニューは catalog 対象外のまま。クリック処理・メニュー構成・
> 保存形式は変更しない。
>
> **Phase 6 UI-only 静的メニュー項目まとめ catalog スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `TopMenuId::Books` / `Video` / `Settings` / `Help` を追加し、製本 / 動画 / 設定 / ヘルプ
> メニューの固定ラベル leaf 項目を `MenuCommandSpec` へ追加する。`render_menubar()` はラベル取得だけを
> catalog 経由にし、enabled 判定・hover text・クリック処理・メニュー構成・保存形式は変更しない。
> 件数付き項目、動的一覧、サブメニュー本体、状態でラベルが変わる `更新を確認…` は catalog 対象外のまま。
>
> **Phase 6 menu catalog drift hardening スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `TopMenuId::ALL` と `menu_commands_for_parent()` を追加し、後続のメニュー構成保存・編集 UI が使う
> parent 別 catalog 取得口を用意する。`TopMenuId` / `MenuCommandId` は `KeyAction` と同様に
> `include_str!` ベースの enum ⇄ `ALL` drift テストで守る。描画・クリック処理・保存形式は変更しない。
>
> **Phase 6 menu layout settings model スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `MenuLayoutSettings` / `MenuCommandOrderSettings` / `ResolvedMenuLayout` を追加し、
> stable name 文字列ベースの保存モデルと resolver を `keymap.rs` に置く。未知 top menu / command は
> 読み飛ばし、保存順に無い既定メニュー・新規コマンドは catalog 既定順で補完する。非表示は
> `hidden_commands` で表現し、全コマンドが非表示になった top menu は resolver 出力から落とす。
> まだ `Settings` へは接続せず、描画・クリック処理・保存形式は変更しない。
>
> **Phase 6 menu layout Settings 永続化スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `Settings.menu_layout: MenuLayoutSettings` を追加し、`settings_kv` 経由の SQLite roundtrip に
> 乗せる。欠落時は空 `MenuLayoutSettings` になり、resolver が catalog 既定順へ補完するため
> 既存ユーザーのメニュー表示は変わらない。未リリースの新規 `settings_kv` フィールドなので
> DB schema migration は不要。描画・クリック処理・編集 UI はまだ変更しない。
>
> **Phase 6 menu layout visibility render スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `render_menubar()` で `resolve_menu_layout(&settings.menu_layout)` を読み、各 top menu が
> resolver 出力に残っている場合だけ描画し、catalog 化済み固定 leaf 項目は
> `ResolvedTopMenu.commands` に含まれる場合だけボタンを出す。動的なお気に入り一覧、タグ一覧、
> レーティング一覧、サムネイル列数 / 比率 / ソート順、更新確認など catalog 外の項目は
> 従来どおり描く。初期接続ではメニュー内順序と top menu 順序の反映はまだ行わず、表示 ON/OFF
> だけを描画に接続する。
>
> **Phase 6 menu layout top menu order render スライス実装メモ (2026-06-24, Codex / ClaudeCode レビュー済み)**:
> `render_menubar()` の top menu 描画を `ResolvedMenuLayout.menus` の順に行うようにし、
> `top_menu_order` の並び替えをメニューバー上の top menu 順へ反映する。各 top menu 内部の
> 固定 leaf 項目は引き続き `ResolvedTopMenu.commands.contains(...)` による表示 ON/OFF だけを
> 使い、メニュー内 command order と編集 UI は後続スライスに残す。クリック処理、enabled 判定、
> hover 表示、動的 / catalog 外項目の描画は変更しない。
>
> **Phase 6 menu layout command order render スライス実装メモ (2026-06-25, Codex / ClaudeCode レビュー済み)**:
> 各 top menu 内の catalog 化済み固定 leaf 項目を `ResolvedTopMenu.commands` の順に描画する。
> 登録済みお気に入り一覧、本一覧、タグ付け / ピン留めタグ一覧、サムネイル列数 / 比率 /
> ソート順、ツールバー submenu、更新確認など catalog 外の動的ブロックは、既定表示を保つため
> 既存の近い固定項目にアンカーし、アンカー項目が非表示の場合は同 top menu 内にフォールバック
> 表示する。クリック処理、enabled 判定、hover 表示、shortcut label、保存形式は変更しない。
> 編集 UI は後続。
>
> **Phase 6 menu layout editor スライス実装メモ (2026-06-25, Codex / ClaudeCode レビュー済み)**:
> 環境設定の表示カテゴリへ「メニュー構成」ページを追加し、`Settings.menu_layout` を
> `PreferencesState.settings` の一時コピー上で編集できるようにする。top menu と固定 leaf 項目の
> 表示 / 非表示、上下移動、既定化、非表示項目の全表示を提供し、OK / 適用時は既存の
> 環境設定保存フローに乗せる。保存値は引き続き stable name 文字列で、未知 ID は UI 表示時に
> 読み飛ばす。登録済みお気に入り一覧やタグ一覧などの動的ブロックは個別編集対象外。
>
> **native 動画 overlay ヘルプ重なり補正メモ (2026-06-25, Codex / ClaudeCode レビュー済み)**:
> native 動画 overlay で `?` ヘルプを開いた状態で右上の閉じるボタンへカーソルを移動すると、
> 右 edge-hover パネルが上に出て閉じられない問題を修正した。`shortcut_help_open` 中は
> right panel / jump panel の visibility 判定を false にし、ヘルプ modal の描画・入力領域を
> 優先する。ヘルプを閉じた後は従来どおり edge-hover パネルが復帰する。
>
> **v2.2.0 重要変更点 entry 追加メモ (2026-06-25, Codex / ClaudeCode レビュー済み)**:
> `src/version_highlights.rs` に v2.2.0 向け entry を追加し、`?` コンテキストヘルプ、
> keymap 追従ラベル、環境設定「表示 → メニュー構成」をまとめて告知する。Cargo.toml が
> まだ 2.1.0 の間に未来の entry がヘルプメニューから表示されないよう、再表示 fallback は
> 現行版以下の最新 entry を選ぶ。

## 1. 背景と狙い

現行の `src/keymap.rs` は `KeyAction` を中心に、既に多くのキーボード操作を
`keymap.ini` で差し替えられる。ただし、これは「既にショートカットとして設計された
キー操作」の一覧であり、アプリ全体の操作カタログではない。

そのため、たとえばファイル名スタック表示トグルのように UI ボタンはあるが既定キーが
ない操作は、ユーザーが後から `keymap.ini` でキーを割り当てる余地がない。

最終的には以下を目指す。

- アプリ内のユーザー操作を安定したコマンド ID で棚卸しする。
- デフォルトでは未割り当ての操作も `keymap.ini` から割り当てられるようにする。
- メニュー / ツール / ツールチップのキー表記を、実際の割り当てから表示できるようにする。
- 衝突判定を、全アプリ一律ではなく「同時に有効になり得るスコープ」単位で扱う。
- 将来のメニュー構成カスタマイズは、コマンド ID の並びとして保存できるようにする。

ただし、最初のリリースでは **UI 表示変更やメニュー構成変更は行わない**。まずは
「catalog 化」と「ini で割り当てられる範囲の拡張」を安全に進める。

## 2. 非目標

初期フェーズでは以下を扱わない。

- GUI のキー割り当てエディタ。
- メニュー構成のカスタマイズ。
- メニューやツール名のショートカット表記の全面更新。
- Esc / Enter / 矢印キーの全面自由化。
- OS clipboard、D&D、右クリックメニュー、IME 確定、通常マウス操作、ゲームパッドの
  `keymap.ini` 化。
- 既存の入力 dispatch を一気に中央集権化すること。

特に最後が重要で、初期フェーズでは **catalog は作るが、既存 dispatch の消費順は極力維持する**。
バグが入りやすいのはコマンド一覧そのものではなく、`consume_key` / `key_pressed` /
native VK 判定の優先順を変える部分である。

## 3. 基本方針

### 3.1 Catalog first, dispatch later

最初に「この操作はコマンドである」という台帳を作る。既存の `KeyAction` はすぐ捨てず、
コマンドに紐づくキー割り当て ID として使い続ける。

> **Phase 1 では `CommandId` / `CommandSpec` を導入しない。** これらが価値を持つのは
> メニュー構成カスタマイズ (Phase 6) や動的ラベル (Phase 5)、scope 衝突判定 (Phase 2) で
> 「keymap.ini の Action 名から切り離した安定 ID」が要るようになってからである。1 エントリ
> しか無い段階で `KeyAction` と 1:1 の第二 ID 空間を作ると、(a) どちらが正本か曖昧になり、
> (b) 既存の `ALL_ACTIONS ⇄ KeyAction` ドリフト検知 (keymap.rs の
> `all_actions_inventory_matches_key_action_enum`) に加えて `CommandId ⇄ KeyAction` の
> 同期テストまで増える。よって **Phase 1 は既存 `KeyAction` の拡張だけ**で進め、catalog 型は
> Phase 2 以降に後送りする (§6 参照)。

#### Phase 1 で実際に作る最小形

`CommandId` は使わず、`KeyAction` 側に「既定未割り当て」を表現できるようにするだけ。

- `KeyAction::default_chords()` が**空の `ChordList` を返せる**ようにする
  (現状は keymap.rs の `all_actions_have_unique_names_and_parse_back` テストが非空を
  強制しているため、ここを「空 = 既定未割り当て」を許す形に緩める)。
- 新規操作 `GridToggleStackMode` を**通常の `KeyAction`** として追加し、既定 chord を空にする。
- 表示用 helper (`effective_chords` / `first_chord_label` 相当) を keymap.rs に足す。
  Phase 1 では UI から呼ばなくてよい (生成・テスト用)。

#### 将来形 (Phase 2 以降で導入する catalog 型、参考)

scope 衝突判定やメニュー構成を実装する Phase で、以下のような `CommandSpec` を導入する想定。
**Phase 1 ではこの型を作らない**ことに注意 (将来の到達点の記録)。

```rust
// Phase 2 以降。Phase 1 では未導入。
pub enum CommandId {
    GlobalLocalSearch,
    GridAddToActiveBook,
    GridToggleStackMode,
    OpenRecycleBin, // UI / メニューだけに存在し当面 keymap 対象外の操作の例
}

pub struct CommandSpec {
    pub id: CommandId,
    pub label: &'static str,
    pub scope: CommandScope,
    pub category: CommandCategory, // 種別 (閲覧 / 編集 / 動画 など)。導入 Phase で確定する
    pub key_action: Option<KeyAction>,
    pub binding_policy: BindingPolicy,
}
```

導入時の役割分担 (Phase 2 以降):

- `CommandId`: UI、メニュー、将来のメニュー構成、説明文、実行単位の安定 ID。
- `KeyAction`: `keymap.ini` の Action 名、trigger 種別、既存 helper との互換。

既存 `keymap.ini` の Action 名は互換維持のため、急に `CommandId` 名へ移行しない。
新規に割り当て可能にする操作は `KeyAction` を追加し、(catalog 導入後は) `CommandSpec.key_action`
へ紐づける。

### 3.2 デフォルト未割り当てを明示的に扱う

現状の keymap 実装は「Action には既定 chord がある」前提が強い。これを緩め、以下を
区別する。

- `default_chords = [Ctrl+F]`: 既定割り当てあり。
- `default_chords = []`: デフォルト未割り当てだが、ユーザー割り当て可能。
- `binding_policy = Reserved`: 固定扱い。`keymap.ini` へは出さない、または固定理由を出すだけ。
- `binding_policy = NotBindable`: UI 操作やマウス操作など、キー割り当ての対象外。

生成される `keymap.ini.default` では、未割り当てコマンドは次のように見せる。

```ini
[Grid]
# GridToggleStackMode = none ; スタック表示を切り替え (標準では未割り当て)
```

コメントを外して `Ctrl+Shift+S` などを書けば有効になる。コメントのままなら挙動ゼロ変化。

#### `none` の意味を 1 つに固定する

既存実装 (key-customization-impl-plan.md §3) でも `Action = none` は既に「明示無効化」を
意味する。本 Phase で未割り当て表示にも `none` を流用するため、二義性を避けて以下に固定する。

- **コメント付き `# Action = none`**: パースされない。単なる**参照表示**で「標準では未割り当て」
  を示すだけ。`keymap.ini.default` (および初回生成 `keymap.ini`) に列挙される。
- **コメント解除 `Action = none`**: ユーザー override として**明示無効化**。effective chord を
  空にする。
- ただし**元から既定未割り当ての action** (`default_chords()` が空) では、override の有無に
  かかわらず最終的なバインドは空なので**挙動上は区別できない**。`none` が実際に意味を持つのは
  **既定 chord を持つ action を無効化するとき**だけ (例: `FsSlideshow = none` で S 既定を殺す)。
- 番号行との混在は従来どおり禁止 (`Action.1 = none` と `Action = none` は同義、他番号と併記しない)。

実装としては「既定未割り当て (空 `default_chords`)」と「override で `none`」は同じ
"effective = 空" に畳まれるため、パーサ側に新しい分岐は要らない。`keymap.ini.default` 生成器が
**既定が空の action を `# Action = none` 行として出せる**ようにするだけでよい。

### 3.3 固定入力は失敗ではなく分類する

ユーザー要望は「操作可能なすべてのものにキーアサインの余地を残す」ことだが、すべての
物理入力を最初から自由化すると、閉じられないダイアログや IME 誤発火を作りやすい。

そのため、初期フェーズでは以下を予約扱いにする。

- `Esc`: モード脱出・キャンセルの保険。将来も常に脱出できる fallback を残す。
- `Enter`: IME 確定、グリッド open、画像 close、動画 play/pause など文脈差が大きい。
- 矢印キー: グリッド移動、見開き、RTL、スタックジャンプ、動画 seek / 音量が絡む。
- text field / IME 中の編集キー。
- OS clipboard / D&D / 右クリックメニュー。

予約扱いの操作も catalog には載せられるが、初期段階では `BindingPolicy::Reserved` として
`keymap.ini` の自由割り当て対象から外す。

## 4. スコープと衝突判定

> **Phase 1 の実装対象外。** 本節は設計として残すが、`CommandScope` enum や衝突判定は
> Phase 1 では一切実装しない。Phase 2 初期実装後も、競合は**拒否せず警告ログのみ**とし、
> dispatch は現行の「先勝ち」(key-customization-impl-plan.md §0) のまま維持する。
> scope 導入はこの警告を出すための整理であり、dispatch 方針自体はまだ変えない。
> 下記の scope 一覧・active scope 表・衝突レベルは**机上の初期案**であり、§4.1 自身が注記する
> とおり実 dispatch と綺麗に一致しない可能性がある。そのため **Phase 2 の最初に、実コードの
> active scope 判定箇所と `consume_key` / `key_pressed` / native VK の消費順を読み出してから**
> enum の variant と「同時 active になり得る scope の隣接関係」を確定する (§6 Phase 2 参照)。
> 隣接関係はどこか 1 箇所の表に declare し、dispatch とのズレをテストで検知できる形にする。

キー衝突は全アプリで一律に禁止しない。同時に有効になり得るスコープだけを見る。

### 4.1 スコープ案

```rust
pub enum CommandScope {
    Global,
    Grid,
    FsCommon,
    FsImage,
    FsVideo,
    Erase,
    Conceal,
    Crop,
    Text,
    LocalAdjust,
    DialogLocal,
    UiOnly,
}
```

代表的な active scope は以下。

| 状態 | Active scope |
| --- | --- |
| グリッド | `Global`, `Grid` |
| 画像フルスクリーン | `Global`, `FsCommon`, `FsImage` |
| 動画フルスクリーン | `Global`, `FsCommon`, `FsVideo` |
| 消しゴム | `Global`, `FsCommon`, `Erase` |
| 隠蔽加工 | `Global`, `FsCommon`, `Conceal` |
| 切り取り | `Global`, `FsCommon`, `Crop` |
| テキスト注釈 | `Global`, `FsCommon`, `Text` |
| 補正レイヤー | `Global`, `FsCommon`, `LocalAdjust` |

実装上、既存コードが `FsImage` の一部処理を編集モード中にも通している場合は、現行挙動を
優先して active scope を調整する。catalog 側の理想形で dispatch 順を変えない。

### 4.2 衝突レベル

| レベル | 条件 | 初期フェーズの扱い |
| --- | --- | --- |
| Hard | 同一 scope / 同一 trigger / 同一 chord | 警告。将来 GUI ではエラー候補 |
| Active overlap | 同時 active になり得る scope 同士の同一 chord | 警告 |
| Disjoint | 同時 active にならない scope 同士の同一 chord | 許可 |
| Reserved | `Esc` / `Enter` / 矢印など予約キーとの衝突 | 警告または割り当て無視 |
| Trigger mismatch | Press と KeyHold / ModifierHold の同一 chord | 個別警告。現行 dispatch を優先 |

初期フェーズでは **衝突を禁止せず、警告だけ** にする。既存の簡易 keymap は競合しても
dispatch 側の先勝ちで運用されていたため、いきなり起動失敗や設定無効化を増やさない。

### 4.3 優先度 resolver は後続フェーズ

`Shift+↑↓` は現在、画像・動画・スタックで意味が変わる。

- 画像通常: `↑↓` と同系統の前後移動。
- スタックのフラット読書中: 前 / 次スタックへジャンプ。
- 動画: 音量調整。

この種のキーは、catalog に載せるだけでは安全に自由化できない。Phase 4 初期実装では、
`ActiveScopes` と優先順位を明示した resolver の足場を置き、スタックジャンプ部分だけを
`FsStackJumpPrev/Next` として移した。

plain 矢印、`Esc`、`Enter` と、非スタック時の `Shift+↑↓` エイリアスは固定または予約として扱う。

## 5. BindingPolicy

割り当て可否は bool ではなく段階を持たせる。

```rust
pub enum BindingPolicy {
    /// Ctrl/Shift/Alt + 通常キーを許可する通常ショートカット。
    FullChord,

    /// ツール名の "(B)" 表示など、単独キーだけを想定する操作。
    /// 初期実装では強制せず、compact 表示の判断だけに使ってもよい。
    SingleKeyPreferred,

    /// デフォルトでは未割り当てだが、ユーザーが割り当て可能。
    DefaultUnassigned,

    /// 安全上・互換上、当面固定。catalog には載せるが keymap 対象にしない。
    Reserved,

    /// マウス、D&D、IME、OS clipboard など keyboard binding の範囲外。
    NotBindable,
}
```

`SingleKeyPreferred` は、消しゴム / 隠蔽加工のツール切替ラベルに関係する。最初から
修飾キーを禁止すると既存カスタムとの互換に影響するため、初期実装では次の扱いが安全。

- 実際の `keymap.ini` では FullChord と同様に受理する。
- 将来 UI の `ツール名 (キー)` 表示では、単独キーのときだけ `(B)` のように表示する。
- `Ctrl+B` のような修飾付きなら、compact 表示は省略し、必要なら tooltip にフル表記を出す。

## 6. 段階リリース計画

数日おきのリリースに合わせ、各段階が単独で出せるようにする。

### Phase 0: 設計固定とレビュー

成果物:

- 本書。
- ClaudeCode / Codex review 用の観点整理。
- 既存固定入力の棚卸し方針。

完了条件:

- `CommandId` と `KeyAction` の境界に合意する。**→ 合意済み (2026-06-24)**: Phase 1 では
  `CommandId` を導入せず `KeyAction` 拡張に絞り、catalog 型は Phase 2 以降で実コードと
  突き合わせてから入れる。
- 初回リリースで触る範囲を `GridToggleStackMode` など少数に絞る。**→ 合意済み**。
- Esc / Enter / 矢印を初期予約扱いにすることを確認する。**→ 合意済み**。

### Phase 1: KeyAction 拡張 + デフォルト未割り当て (CommandId は導入しない)

次回リリース候補。UI 表示変更は行わない。**`CommandId` / `CommandSpec` / `CommandScope` /
`BindingPolicy` は導入しない** (§3.1 参照)。既存 `KeyAction` を小さく拡張するだけに絞る。

実装内容:

- `KeyAction::default_chords()` が**空の `ChordList`** を返せるようにする (= 既定未割り当て)。
  現状 keymap.rs の `all_actions_have_unique_names_and_parse_back` が非空を強制しているので、
  このテストを「空を許す」形へ緩める。
- `keymap.ini.default` 生成器が、既定が空の action を `# Action = none` 行として出せるようにする。
- `Keymap::effective_chords(action)`、`first_chord_label(action)` 相当の表示用 API を追加する。
  初回では UI から使わなくてもよい (生成・テスト用)。
- `GridToggleStackMode` を**通常の `KeyAction`** として追加し、既定 chord を空にして
  `keymap.ini` から割り当て可能にする。
- スタック表示トグルの実行は既存 `toggle_stack_mode()` を呼ぶ。
  `toggle_stack_mode()` 自身が `stack_mode_available()` を確認するため、ボタン動作と同じ可否判定になる。

リスク:

- `KeyAction` は既定 chord あり前提のテストを持っているため、テスト修正が必要。
- `none` の意味が「明示無効化」と「デフォルト未割り当て」の両方に見えるため、§3.2 で固定した
  仕様 (コメント付き = 参照表示、コメント解除 = 明示無効化、既定空 action では区別不能) どおりに
  生成器コメントと docs を書く。

完了条件:

- 既存 `keymap.ini` を編集していない環境では挙動が変わらない。
- `GridToggleStackMode = Ctrl+Shift+S` のように書くとスタック表示が切り替わる。
- `GridToggleStackMode = none` と書くと明示的に未割り当てになる (既定が空なので無設定と同挙動)。
- 生成された `keymap.ini.default` が自己パースできる。
- `CommandId` 等の新しい型を増やしていない (= ドリフト検知対象は `KeyAction` 系のまま)。

### Phase 2: 衝突警告と固定入力棚卸し (catalog 型はここで導入)

`CommandScope` / 衝突判定 / (必要なら) `CommandId` / `CommandSpec` / `BindingPolicy` を
**実際に行使するのはこの Phase から**。Phase 1 では一切作らない。

実装内容:

- **先に実コードを読む**: active scope の判定箇所と `consume_key` / `key_pressed` / native VK の
  消費順を棚卸しし、§4.1 の scope 案 / active scope 表 / 隣接関係を実態に合わせて確定してから
  enum を切る (机上案のまま固定しない)。
- scope / chord / trigger を集め、衝突分類を行う純粋関数を追加する。
- 初期は警告ログのみ。起動失敗や設定拒否にはしない。
- 固定入力を `Reserved` / `NotBindable` として整理する。
- scope の隣接関係 (同時 active になり得る組) を 1 箇所の表に declare し、dispatch とのズレを
  テストで検知できるようにする。
- `keymap.ini.default` 先頭コメントに、衝突判定の範囲と予約キーの扱いを書く。

完了条件:

- 同一 scope 内で同じ chord を割り当てた場合に警告が出る。
- `Grid` と `Erase` のような disjoint scope の同一 chord は警告しない、または低優先度警告に留める。
- scope enum / 隣接関係が、実コードの active scope 判定と矛盾しない (テストで担保)。

### Phase 3: 安全な固定キーの keymap 化

実装内容:

- 優先度が複雑でない固定キーから `KeyAction` 化する。
- 候補:
  - **グリッド側の F7-F10 マスク一括適用 / Shift+F7-F10 削除系**。
    **→ Phase 3 初期実装済み**。`GridApplyErase1/2`、`GridApplyConceal1/2`、
    `GridDeleteEraseMask`、`GridDeleteConcealMask` として `KeyAction` 化し、
    フルスクリーン側の `FsApplyErase*` / `FsApplyConceal*` と既定キーを揃えた。
  - toolbar/menu 操作に対応する既存 App メソッド呼び出し。
  - その他、既定未割り当てにできる便利操作。
- Esc / Enter / plain 矢印はまだ予約。`Shift+↑↓` は Phase 4 初期実装でスタックジャンプ部分だけ
  `KeyAction` 化し、非スタック時のエイリアスは固定のまま。

完了条件:

- 追加した操作がすべて `CommandSpec` に載っている。
- keymap-spec / keymap.ini.default が同期している。
- 既存既定キーは変わらない。

### Phase 4: Context resolver

実装内容:

- `ActiveScopes` と priority を使って、同一キーをどの command が受けるかを純粋ロジックで判定する。
- 既存 dispatch をすべて置き換えず、まず `Shift+↑↓` のような局所的な箇所から試す。
- 動画 native presenter の転送 whitelist と App 側 resolver を同期させる。
- **初期実装済み**: `FsStackJumpPrev/Next` を追加し、egui フルスクリーンのスタックジャンプと
  動画の `VideoPrevFile/NextFile` を keymap 経由へ寄せた。native 側は既存の
  `matches_vk_action(VideoPrevFile/NextFile)` 経路を維持する。

完了条件:

- スタックフラット表示中、動画中、通常画像中で `Shift+↑↓` の既存挙動を保てる。
- resolver の優先順位が unit test で固定されている。

### Phase 5: UI 表示の動的化

実装内容:

- メニューの `(Ctrl+F)` などを `first_chord_label()` 由来にする。
- 消しゴム / 隠蔽加工の `筆 [B]` などを `compact_single_key_label()` 由来にする。
- フルスクリーンホバーバーの `[R]` / `[Shift+Z]` / `[I / Tab]` などを実 keymap 由来にする。
- native 動画 overlay へ必要な shortcut label snapshot を渡す。
- **初期スライス実装済み**: 消しゴム / 隠蔽加工パネルの描画・消去ボタンとツールボタンを
  `compact_action_label()` 由来にした。
- **メニュー表示スライス実装済み**: 検索 / タグビュー系のトップメニュー項目と、一部 toolbar /
  search bar hover text を `first_chord_action_label()` 由来にした。
- **フルスクリーンホバーバー表示スライス実装済み**: 静止画フルスクリーンの上部ホバーバー
  tooltip と、表示モード / ズーム・フィット popup の shortcut 表記を実 keymap 由来にした。
  `Esc` / `F11` は固定扱い。
- **native 動画 overlay 表示スライス実装済み**: App 側で `NativeOverlayShortcutLabels` を作り、
  top bar / bottom HUD / jump panel / seek hover thumbnail の shortcut 表記を実 keymap 由来にした。
  `Esc` / `F11`、Ctrl+Shift+←/→、Ctrl+ホイールなど固定扱いの入力は従来どおり。
- **レーティング tooltip 表示スライス実装済み**: ★フィルタとフォルダバー右側のコンテナ★
  tooltip に表示する `F1〜F6` / `Shift+F1〜F6` を、`RatingItem*` / `RatingContainer*`
  の effective chord 由来にした。

完了条件:

- keymap 変更後、主要メニュー / ツール名表示が実割り当てに追従する。
- 複数 chord の場合は 1 つ目だけ表示する。
- 修飾付き chord は compact 表示で無理に詰め込まない。
- 初期スライスでは、修飾付きまたは未割り当てのツール shortcut はボタン上に表示しない。

### Phase 6: メニュー構成カスタマイズ

実装内容:

- **基盤スライス実装済み**: `MenuCommandId` / `MenuCommandSpec` / `TopMenuId` を追加し、
  既に KeyAction と対応済みの top menu 項目だけを catalog 化する。`render_menubar()` は
  既存と同じラベルを `Keymap::menu_command_label()` 経由で取得するだけで、挙動は変えない。
- **ファイルメニュー静的項目スライス実装済み**: `MenuCommandId::ALL` と drift テストを追加し、
  ファイルメニューの静的項目を catalog 化する。`フォルダを開く…` は `GlobalOpenFolder` と
  紐付け、`Ctrl+O` などの shortcut 表示が keymap 変更に追従する。
- **お気に入りメニュー静的項目スライス実装済み**: お気に入りメニューの静的項目
  (`このフォルダを追加…` / `編集`) を catalog 化する。登録済みお気に入りの動的一覧は対象外。
- **タグメニュー静的項目スライス実装済み**: タグメニューの固定ラベル項目
  (`ピン留めタグの管理…`) を catalog 化する。件数付き項目やピン留めタグの動的一覧は対象外。
- **UI-only 静的メニュー項目まとめスライス実装済み**: `TopMenuId::Books` / `Video` /
  `Settings` / `Help` と、製本 / 動画 / 設定 / ヘルプメニューの固定ラベル leaf 項目を catalog 化する。
  件数付き項目、動的一覧、サブメニュー本体、状態でラベルが変わる更新確認ボタンは対象外。
- **menu catalog drift hardening スライス実装済み**: `TopMenuId::ALL`、parent 別 catalog iterator、
  `TopMenuId` / `MenuCommandId` の enum ⇄ `ALL` drift テストを追加する。
- **menu layout settings model スライス実装済み**: stable name 文字列ベースの `MenuLayoutSettings` と
  resolver を追加し、未知 ID の読み飛ばし・既定補完・非表示 command の解決を純関数で固定する。
- **menu layout Settings 永続化スライス実装済み**: `Settings.menu_layout` を追加し、欠落時 default と
  `settings.db` roundtrip をテストする。描画接続・編集 UI は後続。
- **menu layout visibility render スライス実装済み**: `render_menubar()` が resolver 出力を読み、
  catalog 化済み固定 leaf 項目と空 top menu の表示 ON/OFF を反映する。
- **menu layout top menu order render スライス実装済み**: top menu の描画順を
  `ResolvedMenuLayout.menus` に合わせる。
- **menu layout command order render スライス実装済み**: catalog 化済み固定 leaf 項目の
  メニュー内描画順を `ResolvedTopMenu.commands` に合わせる。動的ブロックは既存アンカーに残す。
- **menu layout editor スライス実装済み**: 環境設定「表示 → メニュー構成」で top menu と固定 leaf
  項目の表示 / 非表示、上下移動、既定化を編集できるようにする。
- メニュー構成を `CommandId` のツリーとして扱う。
- ユーザー設定には `CommandId` の並びだけを保存し、処理本体を複製しない。
- toolbar customization と同じく、表示順・表示 ON/OFF を段階的に扱う。

以降のスライスで、UI-only / keymap 対象外のメニュー項目も段階的に `MenuCommandSpec` へ
追加し、構成保存・編集 UI は catalog が十分安定してから着手する。

### Phase 7: コンテキスト別ショートカットヘルプ (`?`)

実装内容:

- **基盤スライス実装済み**: `CommandDisplayRow` と
  `Keymap::command_display_rows_for_active_scopes()` で、active scope に属する
  `CommandSpec` と effective chord の表示ラベルを取り出せるようにする。UI はまだ作らず、
  dispatch も変えない。
- **グリッドヘルプ初期スライス実装済み**: 当時は `?` を固定ヘルプキーとして扱い、グリッド文脈だけ
  「ショートカット」ダイアログを開く。keymap 化済み操作は実割り当てから表示し、Enter /
  Backspace / 矢印など予約・固定扱いの操作は補助行で表示する。
- **グリッドヘルプ未設定表示スライス実装済み**: キー未設定 / 明示無効化中の操作を別枠で表示し、
  既定キーなしの割り当て可能操作を見つけられるようにする。
- **画像フルスクリーンヘルプスライス実装済み**: 通常の画像フルスクリーンで `?` を押したとき、
  `FsCommon` / `FsImage` / `Rating` の実割り当てと固定キーを表示する。専用 viewport では
  同じ viewport 内にダイアログを描く。
- **消しゴム / 隠蔽加工ヘルプスライス実装済み**: 消しゴム / 隠蔽加工モード中に `?` を押したとき、
  それぞれ `Erase` / `Conceal` scope の実割り当てと固定キーを表示する。
- **切り取りヘルプスライス実装済み**: 切り取りモード中に `?` を押したとき、`Crop` scope の
  実割り当てと固定キーを表示する。
- **補正レイヤーヘルプスライス実装済み**: 補正レイヤーパネル中に `?` を押したとき、
  `LocalAdjust` scope の実割り当てと固定キーを表示する。パネル内のテキスト入力や
  数値入力が keyboard focus を持つ場合は `?` を奪わない。
- **egui 動画ヘルプ初期スライス実装済み**: egui 経路の動画フルスクリーンで `?` を押したとき、
  `FsVideo` scope と動画で有効な一部 `FsCommon` / `Rating` の実割り当て、固定シーク操作を
  表示する。
- **native 動画 overlay ヘルプスライス実装済み**: Windows native presenter 上でも `?` を押したとき、
  egui 動画ヘルプと同じ対象行を中央モーダルで表示する。snapshot は App 側で作成し、
  presenter は所有済み文字列を描画する。ヘルプ表示中は Esc で閉じ、背面動画操作へキーを
  漏らさない。実機補正として、ヘルプ表示中は右メタデータパネル / 左ジャンプパネルの hover
  表示も抑止し、右上の閉じるボタンにパネルが重ならないようにする。
- **テキスト注釈ヘルプスライス実装済み**: テキスト注釈モード中に `?` を押したとき、`Text`
  scope の実割り当てと固定キーを表示する。本文や検索欄などが keyboard focus を持つ場合は
  `?` を奪わない。
- **ヘルプキー KeyAction 化スライス実装済み**: `HelpShowContextShortcuts` を `Global` scope の
  `KeyAction` として追加し、既定を `?` (内部的には `Shift+/`) にする。egui / native 動画の
  dispatch と、ヘルプ内の固定キー欄はこの Action の effective chord に追従する。
- 既定 `?` のヘルプキーで、現在のコンテキスト (グリッド / 画像フルスクリーン / 動画フルスクリーン /
  消しゴム / 隠蔽加工 / 補正レイヤー / テキスト注釈など) で有効なショートカット一覧を表示する。
- 表示内容は固定表ではなく、現在読み込まれている `keymap.ini` の effective chords から作る。
- 同じ `CommandSpec` / `KeyAction` の scope・description・binding policy を使い、未割り当てや
  予約扱いの操作は表示方針を明示する。
- 複数 chord がある操作は全て表示するか、主 chord + 詳細展開にするかを UI 設計で決める。
- `?` 自体は `HelpShowContextShortcuts` として KeyAction 化する。Esc / Enter / 矢印は引き続き
  予約・固定扱いとして残す。

完了条件:

- keymap 変更後、ヘルプ表示のキー一覧が実割り当てに追従する。
- 現在コンテキストで発火しない操作を混ぜず、Global / FsCommon / 編集モード固有操作の重なりを
  active scope から説明できる。
- テキスト入力・IME 変換・ダイアログ操作中にヘルプキーが誤発火しない。
- ヘルプ UI はリリース中の通常操作を阻害せず、Esc / 閉じるボタン / ウィンドウの × で閉じられる。

## 7. 初回リリースの詳細タスク

次回リリースまでに進める最初のステップは Phase 1 に絞る。

### 7.1 実装タスク

1. `KeyAction::default_chords()` が空の `ChordList` を返せるようにする。
   (`src/command_catalog.rs` のような新モジュールや catalog 型は**作らない**。`src/keymap.rs`
   内で完結させる。)
2. keymap.rs の `all_actions_have_unique_names_and_parse_back` テストの「非空強制」を、空を許す形に緩める。
3. `keymap.ini` / `keymap.ini.default` 生成で、既定が空の action を `# Action = none` 行として出す。
4. `Keymap::effective_chords()` と表示用 label helper (`first_chord_label` 相当) を追加する。
5. `GridToggleStackMode` を `KeyAction` に追加する (`ini_name` / `description` / `context` /
   `trigger` / `default_chords`=空 / `ALL_ACTIONS` を揃える)。
6. グリッド側のキーハンドラで、既存ガードを保ったまま `GridToggleStackMode` を見る。
7. 成立時は、自己ガード付きの `toggle_stack_mode()` を呼ぶ。
8. `docs/keymap.ini.default` と keymap 関連 docs を更新する。

### 7.2 初回では触らないもの

- メニュー表示の shortcut label。
- 消しゴム / 隠蔽加工の tool label。
- `Shift+↑↓` スタックジャンプ。
- plain 矢印、Esc、Enter。
- native 動画 overlay 表示 (shortcut label は Phase 5、`?` ヘルプは Phase 7 native スライスで対応)。
- GUI 設定画面。

### 7.3 手動確認

- keymap.ini 未編集で既存挙動が変わらない。
- スタック対応フォルダで、アドレスバーの「スタック」ボタンが従来通り動く。
- `GridToggleStackMode = Ctrl+Shift+S` などを設定して起動し、同じトグルが動く。
- スタック非対応ビューでは no-op または既存と同じ toast / disabled 相当になる。
- 検索バー、アドレス入力、ダイアログ、IME 入力中に新しいキーが誤発火しない。

## 8. 自動テスト計画

UI 全体の自動化より、まず純粋ロジックを厚くする。

> **テストターゲット注意**: keymap の純粋ロジックは `#[cfg(test)]` で keymap.rs 内にあるが、
> `App` を組む App-level テストは `--lib` には出ない。**`cargo test --bin mimageviewer-core`**
> を使う (MEMORY: reference_test_target_app_tests / key-customization-impl-plan.md §8)。

### 8.1 keymap unit test

**Phase 1 で書くもの (KeyAction ベース、catalog 不要)**:

- default unassigned action (空 `default_chords`) が `# Action = none` として生成される。
- 既定が空の action でも `ALL_ACTIONS` / `parse_ini_name` の往復・一意性が成立する
  (`all_actions_have_unique_names_and_parse_back` を空許容に緩めた後も他前提を壊さない)。
- 生成した `keymap.ini.default` が warnings なし、または期待 warnings だけでパースできる。
- `none`、空 override、複数 chord、全置換セマンティクスを検証する。
- `first_chord_label()` が override / default / none を正しく返す。
- `compact_single_key_label()` が単独キーだけを返し、修飾付きでは `None` を返す。

**Phase 2 以降で書くもの (catalog 型を導入したら)**:

- `CommandId` が重複しない。
- `CommandSpec` が同じ `ini_name` を複数持たない。
- 既存 `KeyAction::ALL_ACTIONS` が catalog に登録されている (`CommandId ⇄ KeyAction` ドリフト検知)。

### 8.2 衝突判定 unit test

Phase 2 以降。

- 同一 scope の同一 chord は Hard。
- `Grid` と `Erase` の同一 chord は Disjoint。
- `FsCommon` と `FsImage` の同一 chord は Active overlap。
- `FsImage` と `FsVideo` の同一 chord は通常 Disjoint。
- `Global` と各 scope の同一 chord は Active overlap。
- Reserved chord との衝突が検出される。

### 8.3 App-level test

可能なら以下を追加する。

- `Keymap::from_ini_str("[Grid]\nGridToggleStackMode = Ctrl+Shift+S\n")` を注入した App で、
  該当キー入力が stack toggle intent を立てる (実 API 名は `from_ini_str`)。
- keymap 未設定では該当キーが no-op。
- text focus / dialog guard 中は発火しない。

実際の egui 入力イベント注入が重い場合は、key handler から小さな純粋関数を切り出してテストする。

## 9. リスクと対策

| リスク | 内容 | 対策 |
| --- | --- | --- |
| 入力消費順の変化 | `consume_key` の順番が変わると複数機能の優先度が変わる | Phase 1 では dispatch 置換を最小化する |
| Esc / Enter 退避不能 | 自由化でモードを閉じられなくなる | 初期は Reserved。将来も fallback を残す |
| 矢印キーの文脈衝突 | グリッド、画像、動画、スタック、RTL、見開きが絡む | resolver までは固定 |
| `none` の混乱 | デフォルト未割り当てとユーザー明示無効化が似ている | コメントとパース仕様を明確化する |
| UI 表示と実割り当ての不一致 | Phase 1 では表示はまだ固定 | リリースノートで「表示追従は後続」と明記する |
| native 動画転送漏れ | App 側だけ keymap 化しても native overlay から届かない | Phase 4/5 で whitelist と同時に扱う |
| テスト困難 | egui 入力は完全 E2E が重い | catalog / resolver / parser を純粋関数化する |

## 10. ClaudeCode レビュー依頼観点

レビュー時は、実装前に以下を確認してもらう。

1. Phase 1 で `CommandId` / `CommandSpec` を**導入しない**判断 (既存 `KeyAction` 拡張に絞る) が
   妥当か。後続 Phase で catalog 型を入れ直すときの手戻りが許容範囲か。
2. 初回リリースで `GridToggleStackMode` だけを割り当て可能にする範囲が小さすぎないか。
3. `default_chords = []` (空) と `none` の扱い (§3.2 で固定した二義性解消) が既存 keymap
   セマンティクスを壊さないか。`all_actions_have_unique_names_and_parse_back` の非空強制を
   緩める変更が他のテスト前提を崩さないか。
4. `Esc` / `Enter` / 矢印 / `Shift+↑↓` を予約扱いにする線引きが妥当か。
5. scope 衝突判定で `FsCommon`、`FsImage`、`FsVideo`、編集モードの active scope が実コードと矛盾しないか。
6. native 動画 presenter への転送 whitelist を後続 Phase に回しても、Phase 1 の変更範囲として安全か。
7. 自動テストを純粋ロジック中心にする方針で、回帰検出として足りない観点がないか。

## 11. 実装時のドキュメント更新

Phase 1 を実装するときは、少なくとも以下を更新する。

- [key-customization-impl-plan.md](key-customization-impl-plan.md):
  default unassigned (空 `default_chords`) の実装結果を追記する。Phase 1 では command catalog
  (`CommandId` 等) は導入しない旨も残す。
- [keymap-spec.md](keymap-spec.md):
  スタック表示トグルが keymap 対象になったこと、Esc / Enter / 矢印を予約扱いにすることを明記する。
- [keymap.ini.default](keymap.ini.default):
  `GridToggleStackMode = none` を追加する。
- [architecture-overview.md](architecture-overview.md):
  Phase 1 は新モジュールを追加しない (keymap.rs 内で完結) ため、原則更新不要。catalog モジュールを
  追加する Phase 2 以降でモジュール表に追記する。
- ユーザー向け manual / spec:
  実際にユーザー visible な変更として出すリリースでは、未割り当て操作も ini で割り当て可能になったことを短く追記する。

UI 表示の動的化をまだ入れない場合は、リリースノートで「メニューやツール上のキー表示の追従は後続」と明記する。
