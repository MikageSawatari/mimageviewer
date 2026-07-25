# Detached rework stage-folder-nav — 独立静止画窓の物理フォルダ移動

作成: 2026-07-24 / v2.8.0

状態: コード・自動テスト実装済み、Windows 実機 smoke 待ち

## 1. 目的

複数ウィンドウモードの active independent 静止画窓で、Ctrl+↑↓ と
Ctrl+PageUp/PageDown（同じ `KeyAction` へ割り当てたマウス進む・戻るを含む）から、
現在の detached context を次の通常画像フォルダ、ZIP/CBZ、PDF へ移動できるようにする。

ガードだけを外して App-global の非同期結果を main context へ適用してはならない。
フォルダ移動の request / pending / 連打量 / result は `ViewerContextBundle` 所有とし、
要求元 context を mount している間だけ poll / apply する。

## 2. 仕様

- independent detached の静止画窓は **物理スコープ固定**。
  - Ctrl+↑↓: `effective_folder()` を起点に現行 `folder_tree` の DFS 順。
  - Ctrl+PageUp/PageDown: 同じ親直下の前後兄弟。
  - 並び順、隠しファイル、同名 ZIP 優先などの一般設定は現行どおり使う。
- main のローカル絞り込み、Ctrl+G / Ctrl+S 検索、★固定、スマートフォルダ、
  サブフォルダ展開、レーティング / facet / 色フィルタを参照・変更しない。
  移動先の先頭ページ選定も main の表示フィルタに左右されない。
- Ctrl+G / タグ / レーティング / ★固定など複数の実親を含む仮想一覧から開いた場合も、
  detached 側で開いた通常画像の実親、または ZIP / PDF コンテナを非同期に列挙し直す。
  plain 前後移動、見開き、シークバー、スライドショーが参照する `items` /
  `visible_indices` は検索結果の部分集合や固有順ではなく、その物理ソースの完全な一覧とする。
- 初期スコープは通常画像フォルダ、ZIP/CBZ、PDF。変換確認が必要な
  RAR/7z/LZH、動画 / 音声への昇格、検索固有順、スライドショー NextFolder は含めない。
- passive 化、別窓 activate、close、context drop、新しい load で in-flight request を
  cancel し、遅延結果を main または sibling context へ移さない。
- folder-nav reopen は既存 detached window ID、位置、サイズを維持し、main へ focus を奪わない。

## 3. 触ってよい範囲

- `src/app.rs`
- `src/ui_fullscreen.rs`
- `src/app/tests.rs`
- 本ステージと関連仕様の文書、ユーザーマニュアル、バックログ

新しい HWND 捕捉、placement 保存先、時間窓、detached 用 bool / `Option` は追加しない。
物理スコープは enum、非同期状態は既存 `ViewerContextBundle` で表現する。

## 4. 完了条件

1. `folder_nav_pending`、通常画像の `folder_pane_open_pending`、
   `pending_folder_nav_steps`、`pending_folder_nav_mode` が
   `ViewerContextBundle` の swap / pause / Drop 対象であり、初回 scan 中は
   `folder_pane_open_pending` 自体が active context の生存条件になる。
2. `update_active_detached_viewer_context` が、active bundle を mount 中にだけ
   folder-nav を poll / apply / chain する。
3. independent still の両ハンドラが main の検索・絞り込み分岐より先に
   `Fullscreen` / `SiblingFullscreen` の物理移動を開始する。
4. independent media と linked viewer の既存仕様は変えない。
5. main、active A、passive B の ownership、close / activate 中の cancel、
   ZIP/PDF deferred reopen、連打を自動テストで固定する。
6. `cargo fmt --check`、関連テスト、`cargo test`、`cargo build` が通る。
7. Windows 実機確認用 release build を作成し、通常インストール版は agent が起動しない。

## 5. 実機 smoke

1. 同じ親に `1巻.zip` / `2巻.zip` / `3巻.pdf` を置き、複数ウィンドウモードで
   `1巻.zip` を開く。Ctrl+PageDown と割り当て済みマウス進むで 2巻、3巻へ移動する。
2. main 側で Ctrl+F、レーティング / facet 絞り込み、Ctrl+G、★固定を有効にしても、
   detached 窓は物理順で移動し、main の一覧・選択・スクロール・条件が変わらない。
3. 窓 A の移動直後に窓 B をクリック、または A を閉じる。A の遅延結果が B / main に出ない。
4. ZIP↔PDF 移動で同じ OS 窓の位置・サイズが保たれ、新窓生成・main focus steal・白フラッシュがない。

## 6. 実装結果

- `ViewerNavigationScope::{Main, DetachedPhysical}` を `ViewerContextBundle` 所有にした。
- folder-nav の pending / 連打量 / mode を bundle swap、pause、Drop の対象にした。
- folder-nav の境界ヒントも bundle 所有にし、active 窓の結果を main / sibling へ残さない。
- active detached 更新は bundle mount 中にだけ folder-nav を poll / apply / chain する。
- detached physical load は main の folder pane pending、snapshot lock、検索 pending、
  folder history、rating / facet / color filter、main history persistence を参照・変更しない。
- 動画のみのフォルダを探索対象から外し、動画・静止画混在時も先頭の静止画へ着地する。
- 通常画像を grid から always-new detached で開く経路も、ZIP/PDF page と同じ
  `ViewerContextDescriptor` 経路へ統合した。空の独立 active bundle で親フォルダの
  worker scan を開始し、対象画像 path も typed scan request に保持する。
- scan 完了後だけ detached bundle を mount して `load_folder_with_scan` を適用し、
  対象 path を完全な物理 `items` 上の index へ解決して開く。仮想一覧の backing
  `items` / `visible_indices` と検索固有順は detached へ複製しない。
- 初回 scan が複数フレームにまたがっても active context / session / window runtime を
  保持する。明示した画像 path は Windows の filesystem identity で厳密に解決し、
  削除・hidden 設定などで完全一覧に存在しない場合は先頭画像へ fallback せず、
  失敗した detached session と runtime を正常終了する。
- 画像のみ通常フォルダのコンテナ open は Enter / ダブルクリック / gamepad 共通の
  typed open planを通す。Folderはmain-owned worker scanで「未分類候補」として走査し、
  画像本と確定したcompleted scanだけをdetached bundleへ一度だけ移譲する。混在Folderは
  session/runtimeを作らず通常main navigationへ戻す。これにより、候補段階でdetachedを作成し、
  混在判明後に開く先を失う回帰を構造的に防ぐ。
- ZIP/PDF page は従来どおり container enumerate worker の完全な結果から対象 entry/page を
  解決する。detached の明示 target は `Required` とし、entry 削除・archive 差し替え・
  PDF ページ数減少で消失した場合は保存ページや先頭ページへ fallback せず session を閉じる。
  通常画像 scan と ZIP/PDF enumerate の pending / cancel / result はいずれも
  `ViewerContextBundle` 所有で、main の検索条件・選択・スクロールを保持する。
- protected PDF のパスワード要求、再試行用 session password、成功後の保存予約も
  `ViewerContextBundle` 所有にした。ダイアログ確定／キャンセル時は request owner を mount
  して処理し、列挙再開が main context へ脱落しない。パスワード待ちも active context の
  生存条件とする。
- viewport 作成前に空一覧・列挙失敗・必須 target 消失へ到達した場合も、通常の window close
  と同じ共通終端で active session を finish し、`DetachedWindowManager` の runtime を削除する。
- 実フォルダ scan は `Result<ScannedDir, io::Error>` を維持する。`read_dir` 失敗を空一覧として
  `load_folder_with_scan` へ渡さず、detached の read-only open や Ctrl+↑↓ が catalog の
  `delete_missing()` を誤って実行しない。
- `src/app/tests.rs` で request ownership、main filter preservation、pause / Drop cancel を固定した。
- ディスク上に `a/b/c`、仮想一覧に `c/a` だけがある状態から通常画像を開き、detached の
  非同期完了後が物理順 `a/b/c`、`a` の次が検索で欠落していた `b` になる回帰テストを追加した。
  同時に main の `c/a` 順、検索条件、選択、スクロール、既存 folder-nav state が変わらず、
  Ctrl+↓ request が detached bundle にだけ入ることを固定した。
- 同テストは scan 完了まで production の `update_active_detached_viewer_context` を通し、
  未完了フレームで context / session / runtime が生存することも固定する。さらに対象消失時の
  session / runtime 終了と、大文字小文字だけ異なる Windows path の正しい解決を固定する。
- 追加レビューの回帰として、detached PDF password request の owner 内再開、viewport 未生成の
  terminal close、ZIP/PDF 必須 target 消失、Folder container open の main 無変更、
  scan error 時の catalog row 保持を固定する。
- Folderの回帰テストは、画像だけなら分類完了後に初めてdetached contextを生成し、画像+動画の
  混在ならmain一覧へ遷移してdetached session/runtimeを生成しないことを固定する。
  複数ウィンドウON/OFF × 画像フォルダ本設定ON/OFFの4象限も同じtyped入口で固定する。

## 7. 検証結果

- `cargo fmt --check`: 成功
- `python scripts/check_ui_glyphs.py`: 成功
- `cargo check -p mimageviewer --bin mimageviewer-core`: 成功
- `cargo test -p mimageviewer --lib still_window_mode_key_tests::detached_`: 84 passed
- `cargo test -p mimageviewer --lib image_folder_container_open_classifies_before_creating_detached_context`: 成功
- `cargo test -p mimageviewer --lib mixed_folder_candidate_falls_back_to_main_navigation_without_detached_session`: 成功
- `scripts/test-full.ps1`: PASS（workspace / integration / doctest を含む）
- `scripts/build-dev.ps1`: 成功。`target/dev-runtime/mimageviewer-core.exe` を生成し、
  agent は起動していない。
- `scripts/build-release.ps1`: 今回の追修正では未実施（通常確認用 `build-dev.ps1` を使用）。
- Windows 実機 smoke: 未実施
