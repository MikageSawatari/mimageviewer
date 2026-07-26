# Detached viewer 画像編集制限レビュー依頼 (2026-06-30)

## 背景

detached viewer の複数窓 / ピン留め / active-passive 切替で、消しゴム・補正レイヤーなどの
編集状態を保持・確定しながら bundle 間で移すと、再び複雑なライフサイクル問題を作るリスクが高い。
ユーザー判断として、連動なし detached window では画像編集機能を制限し、表示系操作だけを残す方針にする。

## 確定仕様

- 通常 F12 の **Active・連動 (linked)** detached viewer では、従来どおり画像編集機能を使える。
- **Active・連動なし** detached viewer では、以下を起動できない。
  - 消しゴム
  - 補正レイヤー
  - 隠蔽加工
  - テキスト注釈
  - 切り取り
  - フルスクリーン表示中のマスクスロット適用 / 削除
- Active・連動なし detached viewer でも、以下の表示系操作は使える。
  - 全体の色調補正
  - ポストフィルタ
  - AI 表示設定
  - パノラマ (V)
  - 分析 (Shift+Z)
  - ページ送り / ズーム / 回転などの閲覧操作
- 編集モード中はピンボタンを無効化し、tooltip で「確定またはキャンセルしてから切り離す」旨を案内する。
- 設定「画像を開くとき、毎回新しいウィンドウで開く」の説明に、この設定で開いた別ウィンドウでは画像編集機能を利用できない旨を表示する。

## 実装概要

- `App::detached_viewer_image_edit_tools_disabled_reason()` を追加。
  - always-new detached session 中は編集機能を無効化。
  - pinned / independent still session 中は編集機能を無効化。
  - active detached context が外側に存在する defensive case でも無効化。
- `App::detached_viewer_pin_disabled_reason()` を追加。
  - `is_overlay_edit_mode_active()` または `view_trim_mode` 中はピンを無効化。
- `ui_fullscreen.rs`
  - フルスクリーンの F7-F10 / マスク削除ショートカットを編集制限時は実行せず toast。
  - E / Ctrl+M / Ctrl+T の編集モード開始を編集制限時は実行せず toast。
  - ピンボタンは表示しつつ disabled 風にし、tooltip で理由を表示。
- `ui_adjustment_panel.rs`
  - 左パネル「画像補正」ヘッダーの編集入口 (消しゴム / 補正レイヤー / 隠蔽加工 / 切り取り / テキスト注釈) を編集制限時は disabled。
  - エクスポートは編集状態を開始しないため従来どおり。
- `ui_dialogs/preferences/pages.rs`
  - always-new 設定の直下に画像編集機能制限の注意書きを追加。
- docs / manual を更新。

## 重点レビュー依頼

1. **制限条件は広すぎないか**
   - 通常 F12 の linked detached viewer で消しゴム等が使えるままか。
   - always-new / pinned / independent viewer だけが制限されているか。
2. **制限条件は狭すぎないか**
   - キー起動 (E / Ctrl+M / Ctrl+T / F7-F10 / マスク削除) と補正パネルの編集ボタンの両方が止まるか。
   - 消しゴム以外の補正レイヤー / 隠蔽 / テキスト / crop が漏れていないか。
3. **表示系操作を誤って潰していないか**
   - 全体補正 / ポストフィルタ / AI 表示設定 / V パノラマ / Shift+Z 分析が連動なし窓でも動くか。
4. **ピン無効化の UX**
   - 編集中にピンボタンが誤って昇格しないか。
   - tooltip と toast の文言が妥当か。
5. **ドキュメント整合**
   - `docs/detached-viewer-implementation-plan.md` の §3.0 と `docs/spec.md` / `docs/keymap-spec.md` / manual が同じ仕様を述べているか。

## 実機 smoke 推奨

- 通常 F12 linked:
  - 消しゴム開始 -> 確定/キャンセル -> ピン可能に戻る。
  - 編集中はピンが無効。
- pinned:
  - V / Shift+Z は動く。
  - E / Ctrl+M / Ctrl+T / F7-F10 は実行されず案内が出る。
- always-new:
  - 複数窓で V / Shift+Z は動く。
  - 編集入口は無効。
  - 設定画面の注意書きが表示される。

## 確認済み自動テスト

以下を実行済み。

- `cargo check --bin mimageviewer-core`
- `cargo test still_window_mode_key_tests --bin mimageviewer-core`
- `cargo fmt --check`
