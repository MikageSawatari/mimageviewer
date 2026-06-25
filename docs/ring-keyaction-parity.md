# RingActionId ↔ KeyAction パリティ棚卸し

> ステータス: **棚卸し済み / 漏れ修正済み。⚠️ 13 件も triage 済みで、KeyAction 化 4 件・固定 7 件・ring 専用 3 件に分類済み** (2026-06-25)。
> 目的: キー操作コマンドカタログ化 (`docs/key-command-catalog-plan.md`) の中で、
> 「リング/パッド/ジェスチャ (`RingActionId`) には登録したが、キーボード (`KeyAction`) には
> 登録し忘れた」操作を洗い出し、再発を防ぐための対応表。

## 背景

`RingActionId` (106 variant, `src/ring_shortcut.rs`) と `KeyAction` (318 variant, `src/keymap.rs`)
を意味対応で突き合わせたところ、一部の操作が **ring/パッドには割り当てられるのにキーボードには
割り当てられない**状態だった。キー側はハードコードの生キー処理で取り残されており、

- コマンド一覧 (操作カスタマイズ) に出ない
- キー再割り当てできない
- **競合検出が効かない** (内蔵動作と二重発火しても警告が出ない)

という不具合になる。ユーザー報告の Backspace (親フォルダ) / F11 (最大化・ウィンドウ切替) が該当。

## 凡例

- ✅ 対応する `KeyAction` あり (キーボード割当・競合検出 OK)
- ❌ **漏れ**。キー側ハードコードで割当不可・競合検出不可 → `KeyAction` 化する
- ⚠️ `KeyAction` 無し。**要判断** (キー割当を出すか、固定/クリップボード/ring 専用として docs 明記か)
- ⚪ 設計上 ring/パッド専用 (場所ジャンプ等。キー化不要)
- 固定: キーボード入力としては予約・OS/Shell 連携・マウス専用などの理由で keymap 対象外

## 集計 (actionable 104 / `None`・`Unknown` 除く)

| 判定 | 件数 |
|---|---|
| ✅ 対応済み | 94 |
| 固定 (理由付き) | 7 |
| ⚪ ring 専用 (意図的) | 3 |
| ❌ / ⚠️ 未処理 | 0 |

## ✅ 漏れ修正済み

| RingActionId | 追加する KeyAction | 既定キー | 現状ハードコード |
|---|---|---|---|
| `ToggleWindowMode` | `FsToggleWindowMode` | F11 | `src/ui_fullscreen.rs` (`toggle_still_window_mode`) + `src/app/native_video.rs:5210` (VK 0x7A) |
| `ToggleMaximize` | `GridToggleMaximize` | F11 | `src/app.rs:18571` (`consume_key(F11)` → `toggle_main_window_maximized`) |
| `GridParentFolder` | `GridParentFolder` | Backspace | `src/app.rs:18492` (`key_pressed(Backspace)`) |

native 動画 presenter の F11 は別スレッドで VK 直読みのため、`HelpShowContextShortcuts` と同じ
global chord snapshot 方式 (`install_global_native_video_shortcuts` /
`native_video_context_shortcuts_help_key_down` の同型) で effective chord を公開済み。

## ✅ / 固定 / ⚪ triage 済み

| RingActionId | 現状のキー処理 | 推奨 | 判断 |
|---|---|---|---|
| `TreeFolderPrev` / `TreeFolderNext` | Ctrl+↑↓ 生処理 (`src/app.rs`) | **KeyAction 化**推奨 (Backspace/F11 と同じ問題) | ✅ `GridTreeFolderPrev` / `GridTreeFolderNext` として KeyAction 化。既定 Ctrl+↑ / Ctrl+↓ |
| `SiblingFolderPrev` / `SiblingFolderNext` | グリッド側 Ctrl+PageUp/PageDown 生処理。FS は `FsSiblingPrev/Next` ✅ | KeyAction 化 or 固定明記 | ✅ グリッド側を `GridSiblingFolderPrev` / `GridSiblingFolderNext` として KeyAction 化。既定 Ctrl+PageUp / Ctrl+PageDown |
| `GridHistoryBack` / `GridHistoryForward` | マウス戻る/進むボタンと Alt+←/→ 固定入力 | キー希望なら KeyAction、不要なら固定明記 | 固定。マウス戻る/進む・ブラウザ戻る/進む hook と Alt+←/→ を同じ履歴経路へ畳む OS/マウス入力なので keymap 対象外 |
| `ImageHome` / `ImageEnd` | Home/End 生処理 | 固定として `keymap-spec.md` 明記が妥当 | 固定。先頭/末尾移動は予約ナビゲーション扱いで、連結表示や編集サブモードとの優先関係を保つ |
| `ImageCopyToClipboard` | ring/マウス/パッドから clipboard copy を実行 | 固定明記が妥当 | 固定。OS クリップボード経路で、通常の Ctrl+C / Shell menu / context menu と同じ固定入力レイヤーに置く |
| `ImageCopyPath` / `ImageCopyFileName` | ring/マウス/パッドからパス / ファイル名を clipboard へコピー | 固定明記 or KeyAction | 固定。OS クリップボード / Shell 連携として keymap 対象外 |
| `ImageOpenFolder` | ring/マウス/パッドから Explorer で表示フォルダを開く。対応するキー操作なし | KeyAction 化 or ring 専用明記 | ⚪ ring 専用。画像フルスクリーン中の補助操作で、既存キーボード操作は無い |
| `GridToggleSnapshotLock` | ring/マウス/パッドから ★固定を切替。対応するキー操作なし | キー操作があれば KeyAction、無ければ ring 専用明記 | ⚪ ring 専用。フォルダバー UI とリング用の一発操作として扱う |

## ✅ 対応済み (参考)

ウィンドウ/本: `ToggleDetachedViewer`→`ToggleDetachedViewerMode`、`AddToBook`→`Grid/Fs/VideoAddToActiveBook`、
`PinRepresentativeThumb`→`GridPin`。
グリッド: `GridToggleDetails`→`GridToggleDetailsView`、`GridToggleCheck`、`GridSelectAll`、`GridColumnCount1..10`。
画像 FS: `ImageRotateLeft/Right`→`FsRotateCcw/FsRotateCw`、`ImageCapture`→`FsCapture`、
`ImageToggleMetadata`→`FsToggleMetadata`、`ImageSlideshow`→`FsSlideshow`、`ImagePixelGrid`→`FsPixelGrid`、
`ImageBackgroundCycle`→`FsBgCycle`、`ImageComparePin`→`FsCompareToggle`(比較系・近似)。
動画 FS: `VideoCapture/Mute/Loop/Bookmark/MarkerPrev/MarkerNext/TileMode/ExternalPlayer` は同名 `KeyAction` あり。

## ⚪ ring/パッド専用

`CycleFavorite` / `GridToggleSnapshotLock` / `ImageOpenFolder`。

`OpenFavorite1..20` / `OpenDriveC..Z` /
`OpenLocationDriveList・ReadingHistory・Rating1..5・BooksRoot・Desktop・Pictures・Downloads` は
当初 ring/パッド専用候補だったが、ユーザー要望により Grid 文脈の KeyAction として追加済み。

## 恒久チェックの提案

この棚卸しを腐らせないため、`RingActionId` と `KeyAction` の対応をテスト化する
(既存の `enum_variant_names_from_source` と同じ作法)。
「ring にあってキー側で扱う想定なのに `KeyAction` が無い」を検出し、⚪/固定は allowlist で除外する。
これでジェスチャ/パッド側だけ足してキー側を忘れる再発を機械的に防げる。
