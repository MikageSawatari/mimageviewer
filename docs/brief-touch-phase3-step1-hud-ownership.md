# ブリーフ: Phase 3 Step 1 — HUD HWND の `PT_TOUCH` 所有

対象: v2.13.0 タッチ対応 Phase 3。実装 = Codex Sol / レビュー・検収 = ClaudeCode /
実機確認 = 利用者。

正本: [docs/touch-support-plan.md](touch-support-plan.md) §5.9 (案 C) / §5.10 / §5.14。

前提 (完了・コミット済み・実機確認済み、`14a02194` まで):
Phase 1 一式 (presenter HWND の `PT_TOUCH` 所有を含む)、Phase 2、Step 3b、Step 3d。

---

## 1. なぜ今これをやるのか

Phase 1 Step 4 で所有したのは **presenter HWND だけ**で、**HUD HWND は意図的に対象外**に
した (plan §5.9「Phase 1 Step 4 実装結果」)。その結果として残っている穴:

1. **HUD 上の長押しが右クリックに化ける**。presenter 側は所有によって構造的に解消したが、
   HUD は未所有なので Windows の既定ジェスチャハンドラが `WM_RBUTTONDOWN` を合成し続ける
   (plan §5.9「案 D」の表)。plan §6-2 の残件
2. HUD 上のダブルタップ・drag scroll・ターゲットサイズ調整は、**stream を所有していないと
   実装できない**。promoted mouse では接点の寿命も本数も分からない

したがって **Phase 3 の他の作業はすべてこの Step に依存する**。ここを先に構造として
片付ける。

## 2. この Step の範囲 — 入力 transport だけ

**入れるもの**: HUD HWND の `PT_TOUCH` whole-stream 所有と、overlay への pointer emulation。

**入れないもの** (後続の Step でやる。ここで一緒にやらないこと):

- 左右ダブルタップの相対シーク (±5 秒、plan §5.14-9)
- 動画 / 音楽の初回オーバーレイヘルプ
- **HUD ボタンのターゲットサイズ調整** — 実機を見てから決める。今回は寸法を 1px も変えない
- native パネル (ジャンプ / ブックマーク / 補正 / ★ / タグ) の `ScrollArea` drag

範囲を絞るのは、**この変更が「今まで動いていた HUD のタッチ操作」を壊し得る**ため。
見た目が変わらなければ、実機で「ボタンが今までどおり押せるか」だけを見れば検収できる。

---

## 3. 現状の構造

| | presenter HWND | HUD HWND |
| --- | --- | --- |
| wndproc | `src/video/native_window.rs:1374` `wnd_proc` | `src/video/native_window_host/hud_window.rs:662` `hud_wnd_proc` |
| `WM_POINTER*` | **所有済み** (`handle_presenter_pointer_message` / `handle_owned_pointer_followup`) | **未処理** = `DefWindowProcW` → promoted mouse |
| promoted mouse filter | 全 mouse arm に `should_discard_promoted_touch_mouse` (`native_window.rs:1360`) | **無し** (= だから今も HUD ボタンが指で押せている) |
| 所有状態 | `WindowState.touch_ownership: NativeTouchOwnership` (`native_window.rs:459`) | 無し |
| activation | 通常の activate | `WS_EX_NOACTIVATE` + `WM_MOUSEACTIVATE` → `MA_NOACTIVATE` |

出荷ゲートの実機ログでは **HUD HWND に `PT_TOUCH` が 35 件届いている** (plan §6-1)。
`WM_NCHITTEST` が region 外で `HTTRANSPARENT` を返すので、**HUD の当たり判定の外に落ちた
接点は presenter に届く**。この分岐は OS がやるので、こちらで再現しないこと (下の §4.2)。

座標系は既に統一されている。HUD HWND は presenter の geometry を mirror しており
(`native_window.rs:1666` 付近)、既存の mouse 経路は presenter / HUD どちらの `MouseMove`
も同じ `native_pos(x, y)` へ流している (`render_core.rs:4151`)。
**touch も同じ変換に合流させること。新しい座標変換を作らない。**

---

## 4. 実装方針

### 4.1 所有規約は presenter と同じにする

`native_window.rs` の `handle_presenter_pointer_message` / `handle_owned_pointer_followup`
が既に正しい形をしている。**同じ規約を HUD にも適用する**:

- HUD の `WindowState` (`hud_window.rs:386`) に `NativeTouchOwnership` を持たせる
- **`WM_POINTERDOWN` でのみ所有に入る**。`GetPointerType` が `PT_TOUCH` でない / 問い合わせ
  失敗 / kill switch のときは所有せず `DefWindowProcW` へ流す (fail-open)
- followup (`WM_POINTERUPDATE` / `WM_POINTERUP` / `WM_POINTERCAPTURECHANGED` /
  `WM_POINTERENTER` / `WM_POINTERLEAVE`) は **未登録 pointer id を掴まない**。
  所有していない id はそのまま `DefWindowProcW` へ
- `WM_POINTERUP` / canceled flag / `WM_POINTERCAPTURECHANGED` / `WM_NCDESTROY` で解放する。
  HUD には `WM_CAPTURECHANGED` / `WM_CANCELMODE` / `WM_DESTROY` の
  `emit_synthetic_button_cleanup` (`hud_window.rs:577`) があるので、**touch の終端も
  同じ場所に対応付ける**こと。「mouse は掃除されるが touch は残る」を作らない
- 所有できる pointer は上限付き (`MAX_OWNED_TOUCH_POINTERS`)

**ロジックの重複を作らないこと。** presenter 用に書いた純粋部分
(`NativeTouchOwnership`、`native_touch_followup_phase`、`native_touch_mouse_discard_decision`、
`native_client_pixels_to_points`) はそのまま共有する。wndproc 固有の Win32 呼び出しだけが
HUD 側に増える形にする。両 wndproc に同じ判断が 2 か所書かれる状態になるなら、
**共通の helper へ括り出す**こと。

### 4.2 ⚠ OS の hit-test をやり直さないこと

**HUD HWND に届いた接点は、定義上 HUD のコントロール上にある。**
`WM_NCHITTEST` が既に `HTCLIENT` を返したからその HWND に来ている。

現在の presenter 側アダプタは `TapZoneGeometry.excluded` に
`compute_hud_regions()` の矩形を入れて widget passthrough を判定している
(`render_core.rs:4000` `native_touch_geometry`)。だがこの矩形は doc comment のとおり
**HUD の input-claim 近似であって、描画された egui の response rect ではない**。

したがって:

- **HUD 由来の stream は、幾何に関係なく常に widget passthrough とすること。**
  `classify_tap` に再分類させない
- そのために `NativeVideoTouchEvent` へ**発生元を表す typed field** を足す
  (`bool` ではなく `NativeVideoWindowSource` などの型)。`bool from_hud` は将来
  3 つ目の HWND が出たときに壊れる
- presenter 由来の stream は今までどおり `native_touch_geometry()` の分類を使う
  (中央 / 左右 / excluded)

これで「近似矩形がずれていたせいで HUD ボタンを押したつもりがクロームが toggle した」
という事故が原理的に起きなくなる。

### 4.3 pointer emulation は既存アダプタへ合流させる

overlay の egui context は 1 つなので、**アダプタも 1 つのまま**にする
(`render_core.rs` の `self.native_touch`)。HUD 由来の Touch event も
`push_native_touch_event` に流し、同じ adapter に食わせる。

- **primary を駆動するのは先頭接点だけ**という既存規約を維持する。
  presenter に 1 本、HUD に 1 本という状態でも、primary emulation の owner は 1 つ
- widget passthrough の stream では **press と release を両方通す** (既存の
  `widget_passthrough_keeps_primary_press_and_release` テストと同じ挙動)
- Cancel / capture loss / 抑止境界では、既存の
  「click 距離を十分超える `PointerMoved` → primary release → `PointerGone`」で
  egui の down を確実に解除する (`cancel_primary_press`)。
  **`PointerGone` だけでは egui の `down[]` は消えない** (plan §6-3)

### 4.4 promoted mouse を捨てる

所有した以上、同じ接点から promoted mouse が来ると二重発火になる。
`should_discard_promoted_touch_mouse` を **HUD の全 mouse arm** にも適用する。

- 判定は `GetCurrentInputMessageSource()` が `IMDT_TOUCH` と**確定した場合だけ捨てる**
  fail-open のまま (`native_touch_mouse_discard_decision`)。問い合わせ失敗 /
  `IMDT_UNAVAILABLE` は捨てない
- `MIV_DISABLE_TOUCH_GESTURES=1` では **所有もフィルタもしない**。
  = 今日と完全に同じ promoted-mouse 経路に戻ること

### 4.5 capture と focus

- **`SetCapture` を touch 経路で呼ばないこと。** `WM_POINTER` には暗黙のキャプチャが
  あり、mouse capture と混ぜると解放責任が二重になる。HUD の mouse down が
  `SetCapture` しているのは mouse 経路の都合なので、そこへ相乗りしない
- **focus 引き渡しは新設しない。** HUD の mouse button-down が
  `NativeVideoWindowEvent::RequestFocusClaim` (`native_window.rs:134`) を送っているなら、
  touch DOWN でも**同じ条件で同じ event を送る**。条件を足さない
- HUD は `WS_EX_NOACTIVATE` なので presenter のような activation tap の概念は無い。
  `suppress_widget_primary` を HUD 由来 stream に立てないこと
  (立てると HUD ボタンが押せなくなる)

### 4.6 長押し

所有した stream を `DefWindowProcW` へ渡さないので、**右クリックは合成されなくなる**。
これが今回の主目的の 1 つ。**長押しに独自の意味を新しく割り当てないこと** (今回は無反応)。

---

## 5. ⚠ 先に読むこと — plan §5.10 の「実機で 3 回踏んだ同型バグ」

新しい抑止・新しい cross-frame 状態を足すときは、**着手前に plan §5.10 の表を読むこと**。
要点:

- replay される値は**問い合わせ (query)** であって**出来事 (edge)** ではない
- replay される値から cross-frame の状態を arm しない。arm するならその状態は
  **自分で Idle へ戻れる**必要がある
- 相互排他の状態を bool で持たず **typed owner** にする

今回は「入力 transport だけ」なので新しい状態はほとんど要らないはずである。
**もし新しい bool を足したくなったら、それは設計を疑う合図**。

---

## 6. マウス無影響 (§5.15)

- **マウスのみの入力列で HUD の挙動が一切変わらないこと。** hover による HUD 表示 /
  非表示、ボタンの click、drag (シークバー / 音量)、ホイール、右クリック、
  `WM_MOUSELEAVE` の cursor ownership、すべて不変
- ペンとマウスは `PT_TOUCH` ではないので所有しない = 今までどおり
- キーボード操作も不変
- `MIV_DISABLE_TOUCH_GESTURES=1` で現行挙動へ戻ること

---

## 7. テスト

Win32 の wndproc 自体は unit test できないので、**純粋部分に押し出してから固定する**。

- HUD 由来 stream が**幾何に関係なく** widget passthrough になること
  (presenter の excluded 矩形と一致しない座標でも press / release が通る)
- presenter 由来 stream の分類が**今までどおり**であること (回帰)
- presenter と HUD の stream が同時に生きているとき、
  **primary emulation の owner が 1 つだけ**であること
- HUD stream の Cancel / capture changed で primary down が解除されること
  (`PointerGone` 単独で終わらないこと)
- 未登録 pointer id の followup を掴まないこと (HWND をまたいだ id の取り違えを含む)
- kill switch で所有もフィルタもしないこと
- promoted mouse filter が fail-open であること (問い合わせ失敗で捨てない)

`MIV_TOUCH_DEBUG=1` の診断ログに **HUD 由来であることが出る**ようにすること
(実機ログで presenter / HUD の切り分けができないと検収できない)。

---

## 8. 完了条件

- `cargo fmt` (引数なし)
- `cargo test -p mimageviewer --lib` が**全件**通ること (現在 4957 件)
- `cargo test -p mimageviewer --test ui_snapshot` が通ること
- `cargo check -p mimageviewer --bin mimageviewer-core`
- `python scripts/check_ui_glyphs.py` が 0 件
- **非 Windows を壊さないこと** (CI の ubuntu `cargo check` が番人)
- **[docs/touch-support-plan.md](touch-support-plan.md) を更新**すること:
  - §5.9 の「Phase 1 Step 4 実装結果」に続けて **Phase 3 Step 1 の実装記録**を書く。
    特に「OS の hit-test をやり直さない」判断を残す
  - §6-2 (長押し→右クリック) の HUD 分を解消済みへ更新し、実機確認待ちであることを書く
  - §5.9 の「HUD HWND は意図的に対象外」の記述を現状に合わせる

## 9. 制約

- **アプリを起動しないこと。** 検証ビルドは ClaudeCode が用意する
- **ブランチ操作・コミットをしないこと。** master の作業ツリーで作業する
- detached-rework 凍結ルールは有効。detached / viewport 述語に触れる必要が出たら、
  症状パッチを入れずに報告すること
- **範囲を広げないこと** (§2 の「入れないもの」)。HUD のレイアウト・寸法・
  ボタン構成・auto-hide の挙動には手を入れない

---

完了したら次を報告すること:

1. presenter と HUD で共有した部分 / HUD 固有になった部分の切り分け
2. 「HUD 由来 = widget passthrough」をどこで決めているか (型と 1 か所性)
3. touch の終端 (UP / Cancel / capture / destroy) を mouse の cleanup とどう対応付けたか
4. テスト結果
5. **実機で確認してほしいこと** (番号付きで、期待する結果つき)
