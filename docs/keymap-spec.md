# キー / マウス操作仕様 (静止画 vs 動画)

mimageviewer のフルスクリーン操作におけるキー / マウス アサインの整理。
画像 (静止画 / アニメーション GIF / PDF / ZIP 内画像) と動画でアサインが
異なる項目を一覧化し、整合性 / 不整合を明らかにする。

フルスクリーン境界ヒント、Ctrl+S/F/G 検索スコープ、動画タイルモードまで含めた
横断仕様と現状差分は [fullscreen-navigation-consistency.md](fullscreen-navigation-consistency.md)
を参照すること。

## グリッドビュー (フルスクリーン外) 共通

| キー | 動作 |
|---|---|
| <kbd>Backspace</kbd> | 親フォルダへ |
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> | 前のフォルダへ (DFS pre-order、画像なしフォルダは skip_limit までスキップ) |
| <kbd>Ctrl</kbd>+<kbd>↓</kbd> | 次のフォルダへ (DFS pre-order) |
| <kbd>F1</kbd>〜<kbd>F5</kbd> | レーティング 1〜5 |
| <kbd>F6</kbd> | レーティング解除 |

## フルスクリーン共通 (画像 / 動画とも)

| 入力 | 動作 |
|---|---|
| <kbd>Esc</kbd> | フルスクリーン解除 |
| <kbd>I</kbd> / <kbd>Tab</kbd> | メタデータパネル固定表示トグル (右パネル) |
| <kbd>Space</kbd> | 選択 (チェック) トグル — スライドショー再生中なら停止 |
| <kbd>Backspace</kbd> | 親フォルダへ → グリッドビュー |
| マウスホイール | 前 / 次のファイル |
| マウス左クリック | (画像) ページめくり / (動画) 再生・一時停止トグル |
| <kbd>Shift</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 見開きの場合の左右進行 (動画では下記参照) |

## 画像 フルスクリーン

| キー | 動作 |
|---|---|
| <kbd>←</kbd> / <kbd>→</kbd> | 前 / 次のファイル |
| <kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次のファイル (= 一般慣例で左右と同義) |
| <kbd>R</kbd> / <kbd>L</kbd> | 右 / 左 90° 回転 |
| <kbd>Z</kbd> | 画像分析モード |
| <kbd>S</kbd> | スライドショー 再生 / 停止 |
| <kbd>M</kbd> | ルーペ トグル |
| <kbd>Shift</kbd> (押しっぱ) | ルーペ |
| <kbd>B</kbd> | 透過背景色サイクル |
| <kbd>E</kbd> | 消しゴムモード開始 / 確定 |
| <kbd>U</kbd> / <kbd>Shift</kbd>+<kbd>U</kbd> / <kbd>Alt</kbd>+<kbd>U</kbd> | AI モデル 次 / 前 / リセット |
| <kbd>Enter</kbd> | (動画ボタンが映る場合のみ) 外部プレイヤー |

## 動画 フルスクリーン (Phase 7.H 適用後)

| キー / 入力 | 動作 | 備考 |
|---|---|---|
| <kbd>Enter</kbd> | 再生 / 一時停止トグル | |
| <kbd>Shift</kbd>+<kbd>Enter</kbd> | 外部プレイヤー起動 | |
| <kbd>←</kbd> / <kbd>→</kbd> | 5 秒シーク (デフォルト) | |
| <kbd>Shift</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 1 秒シーク (細かい) | Phase 7.H |
| <kbd>Ctrl</kbd>+<kbd>←</kbd> / <kbd>→</kbd> | 30 秒シーク (大きい) | Phase 7.H |
| <kbd>↑</kbd> / <kbd>↓</kbd> | **前 / 次のファイル** (画像と同じ、マウスホイールと同じ) | Phase 7.H |
| <kbd>Shift</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 音量 ±20% | |
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 現在コンテキストの前 / 次フォルダまたは検索結果へ移動 | native presenter 経路でも有効 |
| <kbd>M</kbd> | ミュート トグル | |
| <kbd>L</kbd> | ループ再生 トグル | |
| <kbd>B</kbd> | ブックマーク追加 (現在位置 🔖) | |
| <kbd>S</kbd> | タイルモード ON/OFF | |
| <kbd>Esc</kbd> (タイル中) | タイルモード解除 | |
| マウス左クリック | 再生 / 一時停止トグル (HUD/パネル除く) | |
| マウスホイール | 前 / 次のファイル | 画像と同じ |
| <kbd>Ctrl</kbd>+ホイール (タイル中) | 列数切替 (6/10/16/20/26/30) | 上部バーの 3x3 / 5x5 アイコンボタンでも同じ操作可 |

## 不整合の解消 (Phase 7.H 適用後)

| 入力 | 画像モード | 動画モード | 状態 |
|---|---|---|---|
| <kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次ファイル | 前 / 次ファイル | ✅ 揃った |
| マウスホイール | 前 / 次ファイル | 前 / 次ファイル | ✅ 揃った |
| <kbd>Ctrl</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次フォルダまたは検索結果 | 前 / 次フォルダまたは検索結果 | ✅ 揃った |
| <kbd>Shift</kbd>+<kbd>↑</kbd> / <kbd>↓</kbd> | 前 / 次ファイル (= ↑↓ と同義) | 音量 ±20% | ⚠ 残った差異 (許容、動画プレイヤー慣例) |
| <kbd>←</kbd> / <kbd>→</kbd> | 前 / 次ファイル | 5 秒シーク | ⚠ 動画プレイヤー慣例 (mpv/VLC/YouTube) で許容 |
| マウス左クリック | ページめくり | 再生 / 一時停止 | ⚠ 動画プレイヤー慣例で許容 |

## 設計メモ

- 動画モードで ↑↓ をファイル移動に再アサインする方針は、旧 egui 経路では
  `handle_video_input` がプレーン ArrowUp/ArrowDown を consume せず後段へ流すことで
  実現していた。現行 Windows native presenter 経路では
  `app/native_video.rs::handle_native_video_key_event` が plain ↑↓ を直接
  `navigate_native_video_fullscreen` に流している。
- Ctrl+↑↓ も同じ思想で native key handler から
  `handle_fullscreen_ctrl_nav_context` へ流し、フォルダ / Ctrl+S / Ctrl+G の
  スコープ解決を画像系と共有する。
- 5/1/30 秒シークの粒度は動画プレイヤー一般の慣例 (mpv: ←→=5s, Shift+←→=1s,
  ←/→ alone in YouTube=5s, J/L=10s) を踏襲しつつ、modifier で粒度切替できる
  ようにした。
- 音量は HUD 下部のスライダーをマウスでドラッグすれば 1% 粒度で調整可能。
  キーボードでは Shift+↑↓ の 20% 粒度のみとし、5% 粒度の plain ↑↓ アサインは
  廃止 (= プレーン ↑↓ をファイル移動に譲るため)。
