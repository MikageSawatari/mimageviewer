# stage-passive-gesture-3 — 再生中の動画ウィンドウでも右ドラッグを受理し、ガイドを出す
# (backlog §1.100 の続き)

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) と §6-3 (live-park の仕様) を読むこと。**
前段 1 (認識・アクティブ化・実行): [stage-passive-gesture](detached-rework-stage-passive-gesture.md)。
前段 2 (ガイド描画): [stage-passive-gesture-guide](detached-rework-stage-passive-gesture-guide.md)。
どちらも実装済み・検収合格・実機確認済み。

ブランチ: `detached-rework`。コミットメッセージに `(detached-rework R2)` を含める。

---

## 1. 直すこと

静止画の別ウィンドウでは、非アクティブでも右ドラッグが認識され、ガイドが出て、成立すると
ウィンドウがアクティブ化されてコマンドが実行される (実機確認済み)。
**再生中の動画ウィンドウ (`ParkedLive`) では右クリックが一切反応しない。**
左クリックで一度アクティブ化すると動く。

**利用者決定 (2026-08-21)**: **静止画と揃える。ガイドも出す。**

## 2. なぜ効かないか (実機ログで確定)

これは今回の作業による退行ではなく、**live-park の既存仕様**である。

実機ログ (2026-08-21):

```
[native-video] parked-live passive event ignored: idx=59 window_id=9
  event=Window(MouseMove(NativeVideoMouseEvent { x: 1388, y: 479, ... }))
```

- 動画ウィンドウの映像は **native presenter という別 HWND** が描いており、
  マウス入力はそちらが受け取る。egui のビューポートはその**裏**にいて入力を見ない。
- `handle_native_video_output_event` ([native_video.rs:3583](../src/app/native_video.rs:3583)) は
  `native_video_parked_live_input_window_id` が `Some` のとき、
  - **左ボタン**だけを「クリックで復帰」に変換し
    ([native_video.rs:3584](../src/app/native_video.rs:3584))、
  - HUD コマンドもアクティブ化に変換し ([:3603](../src/app/native_video.rs:3603))、
  - **それ以外の利用者入力をすべて捨てる**
    ([:3617](../src/app/native_video.rs:3617)、判定は
    [`native_video_output_event_allowed_while_parked_live`](../src/app/native_video.rs:3448) の
    allow-list = `PlacementSwitched` / `PlacementSwitchFailed` / `GeometryChanged` /
    `DpiChanged` / `RequestRaiseHud` / `MouseLeave` のみ)。
- これはプラン §6-3 の「非アクティブ中は映像と音声のみ継続。**操作は最初のクリックで復帰のみ**」
  という決定そのもの。**本ステージはこの仕様を右ドラッグに限って変更する。**

**動画→音声モード**は presenter が hidden で egui の音楽ビューが前面なので、
前段 1 / 2 の egui 経路が既に効く。**二重に描かないこと** (下の §3.3)。

## 3. やること

### 3.1 右ドラッグ入力だけ live-park のフィルタを通す

`native_video_output_event_allowed_while_parked_live`
([native_video.rs:3448](../src/app/native_video.rs:3448)) に、右ドラッグに必要な入力だけを足す。

- **右ボタンの down / up**
- **右ボタンが押されている間の MouseMove** (軌跡の座標列。ジェスチャの reducer に必須)

**足してはいけないもの** (現状どおり捨てる):

- キー入力、ホイール
- 左ボタン (既存のクリック復帰のまま)
- HUD コマンド (既存のアクティブ化変換のまま)
- シーク、音量、再生速度など presenter の操作系

⚠ **右ボタンが押されていない MouseMove は通さない。** 通すと HUD の hover 追従など
「復帰前に操作できてしまう」挙動が復活し、§6-3 の決定を必要以上に壊す。
「右ドラッグが進行中か」は**その窓を所有者とするジェスチャ / リング状態の有無**で判定する
(時間窓ではない、憲法 5)。

所有者の解決は既に入っている: `handle_native_video_mouse_button`
([native_video.rs:9573](../src/app/native_video.rs:9573)) と
`handle_native_video_window_event` ([native_video.rs:4047](../src/app/native_video.rs:4047)) は
`native_video_parked_live_input_window_id` から
`RightDragOwner::DetachedWindow(id)` を組み立てている。**フィルタを通せば正しい所有者で reducer に入る。**

### 3.2 右ドラッグが無効のときは従来どおり

`RightDragMode::Disabled` のときは**フィルタを緩めない**。
非アクティブな動画ウィンドウ上の右クリックで、ウィンドウが前面化したり
フルスクリーンが閉じたりしてはいけない (静止画側の決定と一致)。

### 3.3 ガイドは native presenter のオーバーレイへ出す

⚠ **前段 2 で作った egui のガイド描画は、動画ウィンドウでは presenter に隠れて見えない。**
動画ウィンドウの映像は別 HWND が描いているため。

アクティブな動画には既に専用経路がある:

| 関数 | 位置 |
| --- | --- |
| `sync_native_video_mouse_gesture_overlay` | [gamepad_input.rs:3966](../src/app/gamepad_input.rs:3966) |
| `native_video_mouse_gesture_overlay` (組み立て) | [gamepad_input.rs:3980](../src/app/gamepad_input.rs:3980) |
| ring 版の組み立て | [gamepad_input.rs:4166](../src/app/gamepad_input.rs:4166) |
| presenter への設定 | `set_native_video_ring_picker_overlay` [native_video.rs:8806](../src/app/native_video.rs:8806) |

**⚠ 前段 2 の指示書はこの 2 箇所 (`:3980` / `:4166`) を「Root 限定が正しいので触るな」と
書いた。本ステージではその指示を明示的に更新する: 所有者を引数で受け取れるようにする。**
右クリックメニュー抑止の 2 箇所
([gamepad_input.rs:1379](../src/app/gamepad_input.rs:1379) /
[:1399](../src/app/gamepad_input.rs:1399)) は**引き続き Root 限定のまま**。

**組み立てと push を行う場所が重要**: `set_native_video_ring_picker_overlay` は
`self.fullscreen_idx` と `self.fs_cache` から player を引く
([native_video.rs:8810](../src/app/native_video.rs:8810))。
つまり**その窓の bundle がマウントされている間にしか正しい presenter を指せない**。

- `apply_passive_detached_right_drag_event` は root pass の中で走り、
  **メインの context がマウントされている**。ここから push してはいけない (別 player を指す)。
- 正しい場所は `poll_parked_live_detached_windows`
  ([app.rs:39516](../src/app.rs:39516))。ここは既に
  **その窓の bundle を mount し** ([app.rs:39539](../src/app.rs:39539))、
  `native_video_parked_live_input_window_id = Some(id)` を立てている
  ([app.rs:39541](../src/app.rs:39541))。
  **このマウント区間で、所有者 `DetachedWindow(id)` のガイドを組み立てて push する。**
- その窓を所有者とするジェスチャ / リングが無いときは、**同じ区間で overlay を `None` に戻す**
  (出しっぱなしにしない)。

前段 2 で作った `right_drag_guide_for_owner` が既に所有者を引数に取るので、
**判定と内容はそこから再利用する**。native 用の行データ (`NativeOverlayRingPickerRow`) へ
変換するだけにし、**表示条件をもう一度書き直さない** (二重管理を作らない)。

### 3.4 egui 側と二重に描かない

音声モード (presenter hidden) では egui のガイドが正しい。動画 (presenter 表示中) では
native のガイドが正しい。**どちらか一方だけが出ることを保証する。**
判定は既存の `fs_music_view_active` / presenter の可視性を使う
(`sync_native_video_mouse_gesture_overlay` が既に music_view を見て分岐している
[gamepad_input.rs:3970](../src/app/gamepad_input.rs:3970))。

## 4. スコープ外

- ガイドの見た目の変更 (アクティブ動画のものと同一にする)
- live-park のその他の入力規則 (キー / ホイール / シーク / HUD は**クリックで復帰のまま**)
- 右クリックメニュー抑止の Root 限定 2 箇所
- viewer context registry (R2e)、純粋 reducer (R2f)
- 前段 1 / 2 で入れた静止画の経路 (**実機確認済み。触らない**)

## 5. 触ってはいけないもの

- `find_visible_thread_window_matching_rect*` (憲法 1)
- geometry 由来の recreate (憲法 2)
- App への新しい detached 用 bool / Option (憲法 3)
- placement の新しい保存先 (憲法 4)
- 時間窓での競合吸収 (憲法 5)
- 既存テストの削除・弱体化 (憲法 8)
- **presenter のライフサイクル、可視性、z-order、hit-test region** — 入力フィルタと
  オーバーレイ内容だけを変える

## 6. 完了条件

1. `cargo check -p mimageviewer --bin mimageviewer-core` と
   `cargo test -p mimageviewer --lib` が緑 (`vendor\ffmpeg\bin` を PATH に入れる)。
2. 新規テスト (最低これだけ):
   - `native_video_output_event_allowed_while_parked_live` が、右ボタン down / up と
     **右ドラッグ進行中の** MouseMove を通し、**進行中でない** MouseMove・キー・ホイール・
     シーク・音量・HUD コマンドを**通さない**
   - 右ドラッグが `Disabled` のときは右ボタンも通さない
   - parked-live の右ドラッグが `RightDragOwner::DetachedWindow(id)` として reducer に入る
   - ガイドの組み立てが `right_drag_guide_for_owner` と**同じ表示条件**で動く
     (条件を二重に持っていないことを示す)
   - 所有者のジェスチャが無いとき overlay が `None` に戻る
   - 左ボタンのクリック復帰、キー / ホイール / HUD の既存挙動が変わらない (退行防止)
3. `cargo fmt` 済み。`python scripts/check_ui_glyphs.py` が 0 件。
4. 完了報告に、allow-list へ足した event の一覧と、**足さなかったもの**を file:line で書く。

## 7. 実機 smoke (利用者が実施)

1. 動画を別ウィンドウで再生し、メインウィンドウをアクティブにして live-park させる
2. その動画ウィンドウ上で**アクティブ化せずに**右ドラッグを始めて止める
   → 400ms 後にガイドが出る (アクティブ動画のものと同じ見た目)
3. パターンを描く → ハイライトが追従する
4. 離す → ガイドが消え、ウィンドウがアクティブ化されてコマンドが実行される
5. リングショートカットモードでも 2〜4
6. 右ドラッグを「無効」にすると、非アクティブな動画ウィンドウ上の右クリックで
   前面化もフルスクリーン終了も起きない
7. live-park 中に**左クリック 1 回**で復帰する既存動作が変わらない
8. live-park 中にキー / ホイール / HUD が**効かない**ままであること (§6-3 の維持)
9. 動画→音声モードでガイドが**二重に出ない**
10. アクティブな動画のジェスチャ / リングが従来どおり (退行確認)
