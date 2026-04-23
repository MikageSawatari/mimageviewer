# マルチモニター DPI 問題 調査メモ

## 環境

| 項目 | 内容 |
|------|------|
| OS | Windows 11 Pro |
| GPU | NVIDIA RTX 4090 |
| eframe | 0.33 (wgpu バックエンド) |
| egui | 0.33 |
| winit | 0.30.13（eframe 0.31 当時の調査、eframe 0.33 で winit も更新済み） |
| モニター構成 | 3台（うち少なくとも2台が DPI 150%、1台が DPI 100%） |

## 再現手順

1. アプリをプライマリモニター以外（DPI 150% のセカンドモニター）に移動する
2. `Win+Shift+→` でモニターを順番に切り替える
3. 3回押して元のモニターに戻ったとき、ウィンドウの位置がずれている

## 観測された挙動

デバッグログで `outer_rect` の変化を追跡したところ、以下のパターンが確認された。

```
[11.483s] pos=(130,0)      ppp=1.50   ← Win+Shift+→ 1回目（モニター移動）
[11.989s] pos=(2016,-699)  ppp=1.00   ← DPI 変化が発生、OS が再配置
[12.025s] pos=(1344,-466)  ppp=1.50   ← 再び DPI 変化
[12.644s] pos=(2522,-699)  ppp=1.00   ← 振動継続
[12.685s] pos=(1681,-466)  ppp=1.50
```

- `ppp`（pixels per point = DPI スケール）が 1.50 と 1.00 の間で複数回振動する
- ウィンドウが複数のモニター間を短時間で往復するような挙動が見られる
- 最終的には元のモニターに近い位置に収まるが、ずれが生じる

## 原因の仮説

Windows は DPI の異なるモニター間をウィンドウが移動したとき、
`WM_DPICHANGED` メッセージをアプリに送信し、推奨ウィンドウ矩形を通知する。

winit はこのメッセージを受け取り、推奨矩形に従ってウィンドウを自動的にリサイズ・再配置する。
3台のモニターで DPI が交互に異なる場合、この `WM_DPICHANGED` 処理が連鎖して
ウィンドウ位置の振動が発生すると考えられる。

## アプリ側コードの確認

`app.rs` 内で `ctx.send_viewport_cmd()` を呼び出しているのは以下の2箇所のみ：

```rust
ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));  // タイトルバー更新
ctx.send_viewport_cmd(egui::ViewportCommand::Close);         // 終了ボタン
```

ウィンドウの位置・サイズを変更するコマンドは一切送っていない。
**問題はアプリのコードではなく eframe/winit のレイヤーにある。**

## 調査すべき点

### 1. winit の WM_DPICHANGED ハンドリング

- winit 0.30.x で `WM_DPICHANGED` 受信時にウィンドウを自動リサイズする処理があるか
- 該当 issue / PR:
  - winit リポジトリ: https://github.com/rust-windowing/winit
  - 検索キーワード: `WM_DPICHANGED`, `dpi_changed`, `scale_factor_changed`, multi-monitor

### 2. eframe の DPI 変化への対応

- eframe 0.31 が `ScaleFactorChanged` イベントを受けてウィンドウを操作するか
- 該当リポジトリ: https://github.com/emilk/egui (eframe は egui の一部)
- 検索キーワード: `scale_factor`, `dpi`, `WM_DPICHANGED`

### 3. 回避策の候補

#### 案 A: DPI_AWARENESS を下げる
- `SetProcessDpiAwareness(PROCESS_SYSTEM_DPI_AWARE)` または
  マニフェストで `dpiAware=true/pm` の代わりに `system` を使う
- **トレードオフ**: HiDPI モニターで UI がぼやける

#### 案 B: winit のバージョンを変える
- winit 0.29.x や 0.31.x では挙動が異なる可能性
- eframe 0.31 が依存する winit のバージョン制約に注意

#### 案 C: WM_DPICHANGED を無視する
- eframe の初期化時に `WM_DPICHANGED` を無視するフックを設定する
- winit の `EventLoop` にアクセスできるか確認が必要

#### 案 D: eframe の最新版にアップデート
- eframe 0.33 に更新済み。問題が継続するか要再確認
- 2026-04: eframe 0.34.1 へのアップデートを試みたが、wgpu 29 の依存グラフで
  `gpu-allocator 0.28` が `windows 0.61` を引く一方 `wgpu-hal 29` は内部で
  `windows 0.62` を期待しており、`ID3D12Heap` 型の衝突で ビルド不能だった。
  `gpu-allocator 0.29` または `wgpu 30` 待ち。上流の修正が出たら再チャレンジ。

#### 案 E: ウィンドウを最大化して使う
- ユーザー側の回避策として、最大化状態では `Win+Shift+Arrow` の位置ずれが出にくい可能性

#### 案 F: 初期サイズを初回フレームで再適用（採用済み）
- `ViewportBuilder::with_inner_size` 段階では マルチモニタ DPI 混在時に
  論理/物理ピクセルの取り違えで異常サイズ（縦に極端に長い等）のウィンドウが
  生成される既知バグがある（[egui#4918](https://github.com/emilk/egui/issues/4918) /
  [winit#923](https://github.com/rust-windowing/winit/issues/923)）。
- 対策: `App::pending_initial_size` に意図したサイズを保持し、
  初回 `update()` 呼び出し（= DPI 確定後）で
  `ctx.send_viewport_cmd(ViewportCommand::InnerSize(..))` により再適用する。
- 実装: `src/main.rs` → `src/app.rs` の `initialized` ブロック。
- 通常経路でも無害な no-op になるため、副作用なし。
- Win+Shift+Arrow による位置ずれ（移動時のバグ）は別問題で、この対策では解消しない。

#### 案 G: outer ではなく inner_size を保存する（採用済み, v0.9）
- 旧挙動は `last_outer_rect` の `width()/height()` を `settings.window_size` に保存し、
  次回起動時に `ViewportBuilder::with_inner_size(size)` で適用していた。タイトルバー +
  ボーダー分だけ毎回サイズが縮む症状の原因だった。
- 対策: `App::last_inner_size` を追加し、`track_window_rect` で `inner_rect` のサイズを
  別途保存する。`on_exit` と `hide_to_tray` の両方で、これを `settings.window_size` に
  書き戻す（`inner_size` が取れない稀なケースだけ outer にフォールバック）。

#### 案 H: トレイ復帰時に WINDOWPLACEMENT で完全復元（採用済み, v0.9）
- タスクトレイ常駐からの復帰時、`ShowWindow(SW_SHOW)` だけでは DPI 再計算が
  走って微妙にサイズが変わる問題があった。
- 対策: hide 直前に Win32 `GetWindowPlacement(hwnd, &mut wp)` で矩形を丸ごと保存し、
  復帰直後に `SetWindowPlacement(hwnd, &wp)` で完全復元する。`SetWindowPlacement`
  は eframe/winit を経由しないため DPI 丸めを完全にバイパスできる。
- 実装: `src/tray_integration.rs` の `SavedWindowPlacement` / `capture_window_placement` /
  `restore_window_placement`。`App::saved_placement` フィールドで hide → show 間を保持。

## ログ全文

デバッグログ（`mimageviewer.log`）は `target/debug/` ディレクトリに生成される。
ログには `[viewport] rect updated` のプレフィックスで位置変化が記録されている。

## 関連コード箇所

| ファイル | 内容 |
|----------|------|
| `src/main.rs` | ウィンドウ初期サイズ・位置の設定、モニター境界チェック |
| `src/monitor.rs` | `MonitorFromPoint` / `GetMonitorInfoW` / `GetDpiForMonitor` を使ったモニター情報取得 |
| `src/app.rs` | `last_outer_rect` / `last_pixels_per_point` の追跡、`on_exit` でのウィンドウ状態保存 |
