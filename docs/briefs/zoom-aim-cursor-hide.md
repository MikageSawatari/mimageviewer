# §1.95 Z 照準中はカーソルを隠す

正本: [docs/next-release-backlog.md](../next-release-backlog.md) §1.95。
着手前に [CLAUDE.md](../../CLAUDE.md) の「バグ修正の一般原則」と
[docs/display-pipeline.md](../display-pipeline.md) を読むこと。

## 1. 何を直すのか

利用者報告 (pattier、2026-08-19): <kbd>Z</kbd> を押している間に出る照準枠が、
**カーソルの真下に来ない**。画面の端へ寄るほどずれが大きい。

原因は特定済みで、**マッピングは仕様どおり**である。
`displayed_image_transform.rs` の `z_cursor_image_px` はカーソルを `pan_band` 内の
**割合**で画像座標へ写す。`pan_band` は上部バー / 下部シークバーの反応領域を避けるため
上下だけ内側に詰めてあるので、狭い帯を画像全体へ引き伸ばす分だけ縦が敏感になる。
レターボックス時は左右の余白ぶんも写り込む。

**利用者の判断 (2026-08-19): 照準中はマウスカーソルを隠す。マッピングは変えない。**

理由: 照準枠 `z_aim_frame_rect` と確定後の可視範囲は同じ `cursor_image` と同じ
`z_visible_source` を通っているので、「枠に見えている範囲＝離した後に出る範囲」は
既に一致している。知覚されている問題は「枠がカーソルの真下にない」の一点だけなので、
**基準であるカーソルを消せば解消する**。マッピングを変えないので、Z を離した瞬間の
ジャンプも発生しない。

## 2. 現状のカーソル所有構造 (先に把握すること)

OS カーソルの表示 / 非表示は、フルスクリーンのアイドル自動非表示が単独で持っている。

- 状態: `App::cursor_last_activity: Option<Instant>` と `App::cursor_hidden: bool`
  ([src/app.rs](../../src/app.rs) の宣言、`open_fullscreen` で毎回リセット)。
- 活動検出 ([src/ui_fullscreen.rs](../../src/ui_fullscreen.rs) 13377 付近):
  pointer delta / press / click / wheel があれば `cursor_last_activity` を今にして
  **`cursor_hidden = false` + `CursorVisible(true)` を送る**。
- 隠す判定 (同 14814 付近): `fs_ui_is_clean` かつ
  `idle >= fullscreen_cursor_hide_delay_secs`、**または既に `cursor_hidden`** なら
  `CursorVisible(false)` を 1 回 + `set_cursor_icon(CursorIcon::None)` を毎フレーム。
  `|| self.cursor_hidden` があるので **idle 由来の非表示はラッチする**。
- 保存 / 復元: `fullscreen_cursor_state()` / `restore_fullscreen_cursor_state()` が
  `FullscreenCursorState { last_activity, hidden }` を運ぶ。呼び出し元は
  [src/app/native_video.rs](../../src/app/native_video.rs) の placement 切替と
  [src/app.rs](../../src/app.rs)、`ui_fullscreen.rs` の数箇所。
- `cursor_hidden` の読み手が 6 箇所ある (`still_passive_side_panel_hover_enabled`、
  `passive_hover_enabled`、上部バー hover 判定 2 箇所、edge hover 2 箇所)。いずれも
  **「OS カーソルが見えていないので、stale な hover 位置で chrome を出してはいけない」**
  の意味で読んでいる。

### ここが罠

照準中、利用者は**マウスを動かしている**。上の活動検出が毎フレーム走るので、
「照準中は `cursor_hidden = true` を立てる」だけの実装は活動検出と取り合いになり、
表示 / 非表示が交互に送られる。逆に活動検出を照準中だけ黙らせると、今度は
idle ラッチ側が「隠したまま」を引き継いで、**Z を離した後もカーソルが戻らない**。

## 3. 求める構造

**「今フレーム OS カーソルを隠しているか、その理由は何か」を単一の typed state に集約する。**

- 例: `enum FsCursorHide { Idle, ZoomAiming }` を持ち、`cursor_hidden: bool` を
  `cursor_hide_reason: Option<FsCursorHide>` へ置き換える。既存の 6 読み手向けに
  `fn cursor_hidden(&self) -> bool` を派生させてよい (意味は「OS カーソルが見えていない」
  のままなので、照準中に true になるのは正しい)。
- `ZoomAiming` は **`fs_zoom_aiming` から毎フレーム導出する**。ラッチしない。
  こうすると照準の終わり方 (下記) を個別に扱わなくてよい。
- `Idle` は現在のラッチ挙動を維持する。活動検出が解除できるのは `Idle` だけで、
  `ZoomAiming` を解除してはいけない。
- `CursorVisible` を送る場所と `set_cursor_icon(CursorIcon::None)` を呼ぶ場所は
  **1 箇所のまま**にする。理由が変わった frame だけコマンドを送り、隠している間は
  毎フレーム icon を適用する (egui は frame 跨ぎで sticky にならない)。
- `FullscreenCursorState` は理由まで運ぶか、`hidden` のままにして復元後 1 フレームで
  再導出させるかを選び、**選んだ理由をコード近傍のコメントに残す**。native_video 側の
  placement 切替の挙動は変えない。

## 4. 照準が終わる経路 (全部通ること)

`fs_zoom_aiming` が false になる経路は現状 5 つある。導出型にすればどれも自動で戻るが、
**テストで固定すること**。

1. Z の falling edge → `fs_zoom_active = true` (通常の確定)
2. `fs_zoom_reset()` — ズーム解除、コンテキスト外への移動、`toggle_fs_zoom_mode_action`
3. `fs_zoom_reset_transient()` — フォーカス喪失 / モーダル表示直前
4. `update_fs_zoom_mode_keys` の `fs_video_key_context_active` 分岐 (Z ホールド中に動画へ移動)
5. `level_permit` が None (フォーカス喪失後に routed edge だけ届いた場合)

## 5. やってはいけないこと

- `pan_band`、`z_cursor_image_px`、`z_visible_source` の**マッピングを変えない**。
  縦のゲイン非対称は既知で、カーソルという基準が消えれば残らない見込み。実機で
  なお気になれば次段階として別途扱う。
- **時間窓で吸収しない**。「離してから N ms は隠したまま」等は入れない。
- `CursorVisible` の送信者を増やさない。照準用に 2 つ目の sticky bool を足さない。
- `SetCursorPos` でカーソルを動かさない (mIV に経路が無く、DPI 換算と合成
  `WM_MOUSEMOVE` が付いてくる)。
- 照準中だけ 1:1 マッピングにする / HUD を抑止する案は**採らない** (離した瞬間に
  帯マッピングへ戻って枠と着地がずれる)。

## 6. テスト

`cargo test -p mimageviewer --lib` で走る handler-level テストを追加する。

- 照準開始で hide 理由が `ZoomAiming` になる。
- 照準中にマウスを動かし続けても `ZoomAiming` が維持される (活動検出に負けない)。
- 上記 §4 の 5 経路それぞれで hide 理由が `ZoomAiming` から外れ、idle 条件を
  満たしていなければ `None` に戻る。
- 照準前から idle で隠れていた場合、照準終了後も `Idle` のまま隠れている
  (照準が既存のラッチを壊さない)。
- `CursorVisible` コマンドが理由の変化した frame だけ送られる (照準中に毎フレーム
  送っていない)。

## 7. 完了後

- [docs/next-release-backlog.md](../next-release-backlog.md) の §1.95 を削除する
  (完了項目はファイルから消す運用)。
- 実機確認は利用者が行う。`.\scripts\build-dev.ps1` までをこちらで用意する。
