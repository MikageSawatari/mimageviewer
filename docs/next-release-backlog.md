# 次リリース検討バックログ

このファイルは、まだ着手していない作業候補だけを置く恒久バックログ。
完了した項目はコミット履歴・リリースノート・個別設計メモに任せ、このファイルからは削除する。

運用ルール:

- 着手前に `docs/README.md` から該当領域の設計ドキュメントを読む。
- 着手中のものだけ `対応中` と明記してよい。完了したらこのファイルから削除する。
- 判断保留・見送りの理由は、次に再判断する人が困らない最小限だけ残す。
- 依存ライブラリ更新は `CLAUDE.md` のリリース手順チェックリスト Phase 2 と整合させる。

---

## 1. 優先候補

### 1.1 フルスクリーン / 別ウィンドウ / 連結読みの追跡調査

- 背景: 5ch レス 792。v1.7.0 時点で実用上は問題ないが、フルスクリーン・F12 別ウィンドウ・連結読み周辺で複数の気づきが報告された。
- 2026-06-17 にコード調査を実施し、各項目の現状・真因・方針を以下に詳細化した。
  関連コード位置の行番号は調査時点 (2026-06-17) のもの。着手時に再確認すること。

#### 1.1.1 仮想デスクトップにフルスクリーンが付いてくる

- 調査結果: backlog 当初の「topmost が原因」仮説は静的解析では裏付けられなかった。
  - 専用 FS ウィンドウは `build_fullscreen_viewport_builder_with_transparency`
    (`src/ui_fullscreen.rs:4891`) で `with_decorations(false)` + モニタ論理矩形
    (`monitor::get_monitor_logical_rect_at` `src/monitor.rs:95`) の `with_position` +
    `with_inner_size` として生成され、`WS_EX_TOPMOST` も winit の `Fullscreen::Borderless`
    も付いていない。タスクバー上に出るのは `HWND_TOP` (topmost ではない) への前面化
    (`src/dwm_transitions.rs:67`) による。
  - topmost を実際に付けているのは詳細サムネのホバープレビューと動画 HUD オーバーレイ
    (`src/video/native_presenter/hud_window.rs:185`) だけ。後者は動画再生時のみ追従の一因に
    なりうる。
- 最有力の真因 (優先順):
  1. `MonitorFromPoint` 失敗時のフォールバック `with_fullscreen(true)` (`src/ui_fullscreen.rs:4909`)。
     ここだけが winit 真ボーダーレス → `MarkFullscreenWindow(TRUE)` を呼び、Win11 が現デスクトップに
     ピン留めする。
  2. Win11 が「モニタ全面のフォアグラウンド窓」をフルスクリーン扱いする OS 側ヒューリスティクス。
  3. 動画の場合は topmost HUD。
- 方針: まず再現確認。フォールバック分岐と HWND の `GWL_EXSTYLE & WS_EX_TOPMOST` にログを仕込み、
  ユーザー環境で再現してもらう (Trivial)。真因が確定するまでコード修正は当て推量になる。確定後の
  修正自体は Small 見込み (フォールバックを踏ませない頑健化 / 必要なら `IVirtualDesktopManager` 判定)。
- 規模 / リスク: Medium (真因未確定) / 中。実機確認: Win+Ctrl+←/→ を 静止画 FS / 動画 FS /
  マルチモニタで。

#### 1.1.2 F12 別ウィンドウをフルスクリーン化できない 【方針確定 (2026-06-17): 仮想フルスクリーン方式 / VST GUI は段階分離】

- 方針確定: 真フルスクリーン (winit `Fullscreen`) ではなく、**仮想フルスクリーン
  (ボーダーレスウィンドウ) 方式**で実装する。ゲームの「ボーダーレスウィンドウ」モード相当で、
  画面全体を覆う装飾なしウィンドウにする。VST3 音声処理は従来どおり維持するが、
  VST3 GUI / VST3 パネル対応は本タスクから分ける。
- 採用理由: この方式はメイン FS が採用している作りに近く (装飾オフ + モニタ矩形被覆、
  winit `Fullscreen` 不使用)、流用できる実装と実績がある。
- 仕組み (egui 0.33.3 / egui-winit 0.33.3 で検証済み):
  - detached viewport 中の F11 トグルで、同じ viewport id へ `ViewportCommand::Decorations(false)`
    + `OuterPosition(rect.min)` + `InnerSize(rect.size)` を送る。egui-winit はこれらを既存ウィンドウの
    `set_decorations` / `set_outer_position` / `set_inner_size` で適用する
    (`egui-winit-0.33.3/src/lib.rs:1457,1509` ほか) ため **ウィンドウ再生成が起きない**。
  - rect は在モニタの論理矩形 (`monitor::get_monitor_logical_rect_at` にウィンドウ中心を渡す)。
    前面維持はメイン FS の `HWND_TOP` 化 (`src/dwm_transitions.rs:67`) を流用。topmost にはしない
    (alt-tab は通常どおり、ゲームのボーダーレスと同じ挙動)。
    ただし `with_taskbar(false)` 相当の runtime `ViewportCommand` は無いため、タスクバー被覆は
    実機確認で判断する。必要なら viewport 再生成または Win32 側 style 調整を別案として検討する。
  - 退場時は `Decorations(true)` + 退避した位置 / サイズを復元。
- なぜ安全か (バグ混入を抑えられる根拠):
  - 同一 viewport id への window-state コマンドなので `viewer_presentation` を変えず、再生成経路
    `hide_current_fullscreen_viewport_for_recreate` (`src/ui_fullscreen.rs:3057` / `:4731`) を
    通らない → 過去 P1 (commit `b03ef5c7` の host migration teardown) に触れない。`Maximized` と
    同じ安全度。
  - winit `set_fullscreen` / `Borderless` / `MarkFullscreenWindow` を通らない → 1.1.1 の最悪
    パターン (デスクトップピン留め) を回避。
- 方式比較 (3 案):
  - `Maximized` トグル: 最小実装・OS が復元矩形を管理。ただしタイトルバー残・タスクバー被覆なし。
  - 仮想 FS (採用): 全面被覆・装飾なし・再生成なし。`Maximized` より「復元矩形を自前で退避 / 復元」
    する分だけ作業増。
  - 真 FS (`Fullscreen(true)`): コードは 1 行だが `MarkFullscreenWindow` で 1.1.1 を悪化させ、
    多 viewport 内部状態とも絡みやすい → 不採用。
- 唯一の追加作業 = 復元矩形の自前管理: placement 保存クロージャ (`src/ui_fullscreen.rs:3132`、
  現状 `outer_rect` + `maximized` を保存) が、仮想 FS 中の全面矩形を復元位置として保存しないよう
  ガードを追加する (minimized 中スキップと同じ要領)。
- 現状の detached 中 F11 はトースト表示のみ (`src/ui_fullscreen.rs:5772`)。ここを上記トグルに
  差し替える。
- VST3 GUI / パネルを分ける理由:
  - 現行コードは VST3 GUI を `ViewerPresentation::Fullscreen` 専用として扱う
    (`sync_native_video_vst3_available` `src/app/native_video.rs:3653`)。
  - F12 動画は `NativeVideoPlacement::DetachedViewerChild` で、detached egui viewport の child window
    として表示する。WS_CHILD を VST editor の owner にすると z-order / focus が壊れやすいため、
    現行実装はあえて VST owner / HUD 登録を解除している。
  - 将来 F12 仮想 FS でも VST3 GUI を使うなら、`DetachedBorderlessFullscreen` 相当の
    「VST owner として安全な top-level presenter」扱いを追加し、VST availability / HUD overlay /
    owner 登録 / Alt+Tab 復帰をまとめて検証する別サブタスクにする。見積りは +1.5〜3日、
    リスクは中〜中高。
- 初期スコープ: 主対象は画像 (および ZIP / PDF ページ) の detached ビューア。通常 (VST GUI なし)
  動画の detached 仮想 FS を対象に含めるかは実装着手時に判断する。
- 規模 / リスク: Small / 低〜中 (再生成なし = teardown 回避、`MarkFullscreenWindow` 回避、
  メイン FS 実装の一部流用、新規依存なし、`src/ui_fullscreen.rs` 中心)。タスクバー被覆や動画を
  含める場合は中へ寄る。
- 実機確認: マルチモニタで在モニタに全面化 / 復元で元の窓サイズに戻る / タスクバー被覆と alt-tab
  挙動 / detached 静止画・ZIP・PDF ページ。
- ドキュメント更新: `docs/detached-viewer-implementation-plan.md` §6 (現状「detached 中の F11 は
  無効」)、`docs/keymap-spec.md`、ユーザーマニュアル。

#### 1.1.4 縦 / 横連結で見えてきたページに AI アップスケールが遅れて適用される

- 現状: final AI スケジューラはアンカーページ `fs_idx` 中心のみで viewport 非対応。先読みは
  前 +2 / 後 -1 の逐次窓 (`src/settings.rs:2682`) + 描画時 1 ページ / frame。連結は最大 16 ページ
  同時表示なので追いつかず、見えたページに遅れて適用される。
- 方針 (最小から):
  - Step1 (Small・最有効): 連結時は既存の `continuous_keep_set` (可視 + pad、`src/app.rs:34244`) を
    先読みターゲットへ流す。隣の `maybe_native_rerender_pdf_for_ai` は既に keep_set を使っているのに
    `prefetch_final_ai` (`src/app.rs:34297`) だけ未対応。
  - Step2 / 3 (Medium): 同時 1 ジョブ制約 (2026-06「完了直前 kill → 再 spawn」対策で導入) の緩和・
    VRAM / 並行数チューニング。Step1 を計測してから判断。
- 注意: AI は worker 化済みで UI スレッドは守られるが、ターゲット拡大時の
  `assemble_edit_result_pixels` の UI スレッド texture upload に注意 (1 フレーム予算を保つ)。
- 規模 / リスク: Step1 Small / 全体 Medium。中 (並行数増で 2026-06 の respawn バグを再発させない
  こと、retained LRU eviction の thrash 確認)。実機確認: RTX 4090 で連結スクロール + `--perf-log` /
  `analyze_perf.py scroll`,`hitches`。

- 優先度: 1.1.2 (方針確定) / 1.1.1 / 1.1.4 は P2/P3。フルスクリーン周辺は egui
  multi-viewport と Win32 の境界があり、項目ごとに分けて扱う。

## 2. アーカイブ / 仮想フォルダ

現時点ではなし。

---

## 3. フォルダツリーペイン

### 3.1 folder pane scan worker の thread 構成判断

- 背景: `scan_real_subfolders` はノードごとに短命 thread を spawn する。
- 現状: `folder_pane/scan_subfolders` perf event で ms / entry 数 / dir 数 / cancel / error を記録済み。
  cancel 付きで thread leak は見えていない。
- 方針:
  - 低速共有や大量ノード展開で遅い scan / concurrent scan が見えた場合だけ、dispatcher / pool 方式へ寄せる。
- 優先度: P3。

## 4. 補正 / AI

### 4.1 local-adjust layers の入場時同期 DB 読み

- 背景: フルスクリーン入場初回フレームで `LocalAdjustDb::get_layers` を同期実行する。
- 現状: フォルダ open 一括読みを避けるための意図的 tradeoff。
- 方針:
  - 数十 MB 級ページで hitch が報告 / 計測された場合に worker 化する。
  - read-only 経路の not-loaded は現状どおり None 返しを維持する。
- 優先度: P3 monitor。

### 4.4 表示トリム / 余白カット

- 背景: 5ch レス 792-⑥。要望文は「crop 後フィット」だったが、既存の crop は投稿 / 書き出し用の
  「切り取り」で、漫画ビューア用途の「読みながらサクッと余白を詰める」機能とは目的が違う。
- 実装済み (2026-06-17):
  - 左ホバーの補正パネルを「画像補正 / 表示トリム」のタブ式にし、選択タブを設定保存する。
    上部バー右側の表示トリムボタン / 画像補正アイコンは削除した。
  - 表示トリムタブで、トリムなし / 自動余白カット / 本全体の設定を適用、を
    ラジオで切り替えられる。このページ個別設定は現在ページだけのチェックで適用し、
    前後ページへ移動するとチェックは外れる。
  - 本全体 / このページの手動設定では、単ページ / 見開き連動 (上・下・中央側・外側) /
    見開き左右別を 0〜20% のスライダーで調整できる。
  - 自動余白カットは表示中ページごとに検出する。自動検出ボタンは現在ページ / 見開きの
    単色余白を本全体 / このページの手動スライダーへ反映する。
  - `draw_fs_image` / `draw_fs_spread` の content bbox 経路に統合し、ページ全体 / 横幅 / 縦幅 /
    100% 原寸でも表示トリム後の矩形を fit 基準にする。
  - bbox 外は描画せず背景色に落とす。中央側のトリムは見開きの見える端が gap に合うよう再配置する。
  - スライダー操作時は、対象の手動設定モードを適用する。
  - 見開き連動 / 左右別の切替時は値を移行し、左右別→連動では平均値にする。
  - 基本適用モード / 本全体設定は本キー、ページ個別設定値は page_path_key で
    `view_trim.db` に保存する。ページ個別チェック状態は保存せず、自動余白カットは
    モードだけ保存し、検出 bbox は保存しない。
  - 出力用 crop / 保存 / Ctrl+E / 補正 / AI キャッシュには影響しない。
- 残:
  - 実機で使用感確認後、枠ドラッグ操作を追加するか判断。
- 優先度: P3。

---

## 5. 入力カスタマイズ / マウス / ゲームパッド

### 5.1 Shift / Alt + ホイールのカスタマイズ再設計

- 背景: v1.7.0 のリングショートカット / マウスボタン実装中に、Shift / Alt + ホイールのペアバインドを
  追加候補にしたが、実機確認で動画まわりの退行リスクが高いと判断した。
- 方針:
  - v1.7.0 では公開 UI / 入力経路から外し、通常ホイール、Ctrl+ホイール、中ボタンドラッグの既存挙動を維持する。
  - 将来再開する場合は、グリッド / 画像フルスクリーン / 動画フルスクリーンを別々に設計する。
  - native video overlay の consumed wheel、modifier 転送、動画タイルの Ctrl+ホイール、編集パネル / スクロールパネルとの
    優先順位を先に決める。
- 実装メモ: `ring_shortcuts.shift_wheel_pair` / `alt_wheel_pair` は互換読み込み用フィールドとして残すが、
  現行 UI / 入力経路からは参照しない。
- 規模 / リスク: Medium / 中。動画系の手動確認を含めて別タスクで扱う。

---

## 6. リリース前確認 / 依存更新

### 6.1 ネイティブ依存

| 対象 | 現状 / 次の確認 | 注意点 |
| --- | --- | --- |
| PDFium | vendor 更新後の PDF 表示手動確認が必要 | PDF 開封、ページ列挙、サムネ、フルスクリーン、パスワード PDF |
| FFmpeg LGPL shared | 動画再生の手動確認と LGPL ソース tarball 配置更新 | DLL 名が変わる更新では `setup-ffmpeg.sh` / loader / `build.rs` を揃える |
| ONNX Runtime | `ort-sys` 要求 DLL と setup script の VERSION を確認 | C API バージョン一致、`+crt-static` + `load-dynamic` 維持 |
| VST3 SDK / bridge | C++ ソース変更がなければ再ビルド不要 | 更新時は商用プラグインで実機確認 |

### 6.2 Rust クレート

- 通常の `cargo update` は互換範囲でまとめて実施する。
- メジャー / rc 脱出は個別判断:
  - `ort`
  - `pdfium-render`
  - `ffmpeg-the-third`
  - `image`
  - `zip`
  - `sevenz-rust2`
  - `delharc`
  - `unrar`
  - `turbojpeg`
- 更新後に確認するもの:
  - `cargo test`
  - 検索 bench 回帰
  - perf smoke
  - `dumpbin /dependents` で不要な VC runtime DLL が復活していないこと

---

## 7. 着手時に読み直す関連ドキュメント

| 領域 | ドキュメント |
| --- | --- |
| UI 同期 I/O / worker 化 | `docs/ui-responsiveness.md`, `docs/async-architecture.md` |
| ZIP / PDF / 変換アーカイブ | `docs/virtual-folders.md`, `docs/shell-file-operations-context-menu-plan.md` |
| フォルダ移動 / Ctrl+↑↓ | `docs/fullscreen-navigation-consistency.md`, `docs/keymap-spec.md` |
| 入力カスタマイズ / マウス / ゲームパッド | `docs/keymap-spec.md`, `docs/key-customization-impl-plan.md`, `docs/ring-shortcut-plan.md` |
| フルスクリーン / F12 別ウィンドウ / 連結読み | `docs/display-pipeline.md`, `docs/detached-viewer-implementation-plan.md`, `docs/fullscreen-navigation-consistency.md` |
| 表示 / AI / 補正 | `docs/display-pipeline.md`, `docs/preset-and-adjustment.md` |
| リリース / 依存更新 | `CLAUDE.md` のリリース手順、各 native 依存管理節 |
