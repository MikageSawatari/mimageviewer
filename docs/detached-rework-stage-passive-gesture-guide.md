# stage-passive-gesture-2 — 非アクティブな別ウィンドウにもジェスチャガイドを描く
# (backlog §1.100 の続き)

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)。
**着手前に同書 §2 (憲法) を読むこと。**
前段 (認識・アクティブ化・実行) の指示書: [stage-passive-gesture](detached-rework-stage-passive-gesture.md)。
そちらは実装済み・検収合格・実機確認済み。

ブランチ: `detached-rework`。コミットメッセージに `(detached-rework R2)` を含める。

---

## 1. 直すこと

前段で、非アクティブな別ウィンドウ上の右ドラッグは**認識され、成立するとウィンドウが
アクティブ化されてコマンドが実行される**ようになった (実機確認済み 2026-08-21)。
しかし**ドラッグ中のガイド表示が非アクティブ窓には出ない**。前段で意図的にスコープ外にした部分。

**利用者要望 (2026-08-21)**: 「非アクティブでも UI のガイド表示はそのまま出すようにしたい」。

満たすべきこと: **アクティブ窓で出るのと同じガイドが、同じ条件で、非アクティブ窓にも出る。**
「同じ条件」には表示設定・抑止条件・出現遅延を含む (下の §3.1)。

## 2. なぜ出ていないか (確認済み)

ガイドを描く 2 つの関数は、どちらも**所有者が `Root` のときだけ描く**。

| 関数 | 位置 | 所有者判定 |
| --- | --- | --- |
| `draw_mouse_gesture_overlay` | [gamepad_input.rs:447](../src/app/gamepad_input.rs:447) | [:467](../src/app/gamepad_input.rs:466) で `gesture.owner != RightDragOwner::Root` なら return |
| `draw_mouse_ring_flick_overlay` | [gamepad_input.rs:393](../src/app/gamepad_input.rs:393) | [:411](../src/app/gamepad_input.rs:411) で `flick.owner != RightDragOwner::Root` なら return |

⚠ **`owner != RightDragOwner::Root` の判定はコード全体で 6 箇所ある。変えてよいのは上の 2 箇所だけ。**
残り 4 箇所は Root 限定が正しいので**触らない**:

| 位置 | 何のため | Root 限定が正しい理由 |
| --- | --- | --- |
| [gamepad_input.rs:1175](../src/app/gamepad_input.rs:1175) / [:1195](../src/app/gamepad_input.rs:1195) | `mouse_ring_context_menu_suppressed` | 右クリックメニューは root pass の関心事。非アクティブ窓はメニューを出さない |
| [gamepad_input.rs:3776](../src/app/gamepad_input.rs:3776) | `native_video_mouse_gesture_overlay` | アクティブな native video HUD 用 |
| [gamepad_input.rs:3962](../src/app/gamepad_input.rs:3962) | native ring overlay の組み立て | 同上 |

加えて、**非アクティブな静止画窓は `show_viewport_deferred` で描かれ、コールバックは `self` を
借りられない** ([ui_fullscreen.rs:11622](../src/ui_fullscreen.rs:11622))。
描けるのは `DeferredDetachedImageWindowView` (DTO) が持っている情報だけ。
上の 2 関数は `&self` から `settings` / `mouse_gesture` / `mouse_ring_flick` を読むので、
そのままでは呼べない。

`ParkedLive` の動画 / 音声窓は `show_viewport_immediate` なので `self` を借りられる
([ui_fullscreen.rs:11745](../src/ui_fullscreen.rs:11745))。こちらは DTO を経由しなくても描ける。

**先例**: native video presenter は egui ですらない別ウィンドウにガイドを出すため、
**同じ内容を既に DTO 化している** (`NativeOverlayRingPicker { rows: [{label, value}] }`、
組み立ては [gamepad_input.rs:3772](../src/app/gamepad_input.rs:3772))。
ガイドが serialize 可能なことはこれで実証済み。

## 3. やること

### 3.1 「描く内容」を所有者から組み立てる純関数に分ける

今は「判定 + 組み立て + 描画」が 1 関数に混ざっている。次の 3 段に分ける。

```rust
/// ガイドとして描く内容。所有者を問わず同じ形。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum RightDragGuide {
    Gesture {
        /// 入力中パターンの表示文字列 (未入力なら "-")
        current_pattern: String,
        rows: Vec<RightDragGuideRow>,      // label = パターン, value = アクション名
        selected_row: Option<usize>,
    },
    Ring {
        center: egui::Pos2,
        selected: Option<RingDirection>,
        slot_labels: Vec<String>,          // RING_SHORTCUT_SLOT_COUNT 個
    },
}
```

1. **組み立て**: `App::right_drag_guide_for_owner(owner: RightDragOwner, surface_context) -> Option<RightDragGuide>`
   - 現在の 2 関数が持っている**抑止条件をすべてここへ移す**。取りこぼすと退行になる:
     - `right_drag_mode(surface_context)` が該当モードであること
     - `mouse_gesture_help_visible` / `mouse_ring_enabled` + `mouse_ring_help_visible`
     - `self.ring_picker.is_some()` なら描かない
     - `gesture.context` / `flick.context` が `surface_context` と一致すること
     - `guide_visible()` が true であること (`armed` または `mouse_flick_menu_delay()` 経過)
   - 違うのは `owner` の比較対象だけ。**`Root` 決め打ちをやめて引数の owner と比較する。**
2. **描画**: 純関数 `draw_right_drag_guide(painter, full_rect, &RightDragGuide)`。
   `&self` を取らない。ドーナツは既存の `draw_ring_guide_donut`
   ([gamepad_input.rs:6566](../src/app/gamepad_input.rs:6566)) をそのまま使う
   (既に painter + ラベルクロージャだけの自由関数)。
3. **既存の 2 関数は上の 2 段を呼ぶ薄いラッパにする。**
   呼び出し元 ([ui_fullscreen.rs:15084](../src/ui_fullscreen.rs:15084)、
   [ui_main.rs:13641](../src/ui_main.rs:13641) / [:15251](../src/ui_main.rs:15251) /
   [:15315](../src/ui_main.rs:15315) / [:15924](../src/ui_main.rs:15924)) は変更しない。

⚠ **描画の実装を 2 本に増やさない。** アクティブ窓と非アクティブ窓で見た目がずれると、
どちらが正しいか分からなくなる。1 本の描画関数を 3 つの呼び出し元が使う形にする。

### 3.2 deferred (静止画) 窓へ DTO で渡す

- `DeferredDetachedImageWindowView` ([app.rs:450](../src/app.rs:450)) に
  `right_drag_guide: Option<RightDragGuide>` を足す。
- root pass が毎フレーム `deferred_detached_image_window_shared(view)`
  ([app.rs:39660](../src/app.rs:39660) 付近) で view を差し替えているので、
  **その view を組み立てるところで `right_drag_guide_for_owner(DetachedWindow(id), ...)` を呼ぶ。**
- deferred コールバック ([ui_fullscreen.rs:11622](../src/ui_fullscreen.rs:11622)) の
  `CentralPanel` 内、`draw_detached_image_window_bar` の**後**に
  `draw_right_drag_guide` を呼ぶ (バーより手前に出す)。

`surface_context` は、その窓の内容から決まる既存の
`DetachedImageWindowSnapshot::right_drag_context` を使う (前段で追加済み、
[app.rs:694](../src/app.rs:694) 付近)。

### 3.3 ガイドが出る・更新されるように repaint を出す

DTO は root pass が更新し、実際に描くのは子 viewport なので、**子が repaint しないと
ガイドが出ない / 古いまま**になる。

- **非アクティブ窓の右ドラッグが生きている間、root はその窓の viewport へ repaint を要求する**
  (`ctx.request_repaint_of(viewport_id)`)。
- `guide_visible()` は `mouse_flick_menu_delay()` (400ms) 経過でも true になるので、
  **ポインタが止まっていてもその時刻に一度は描き直す**必要がある。
  既存の `request_ring_overlay_repaint_after` ([gamepad_input.rs:62](../src/app/gamepad_input.rs:62))
  と同じ考え方で、対象 viewport に対して同じことをする。
- ⚠ これは**憲法 5 が禁じる「時間窓で競合を吸収する」ではない**。
  `mouse_flick_menu_delay()` は既存のガイド出現遅延 (UX 仕様) で、
  ここでやるのは「その時刻に描き直す」だけ。判定条件を時間で変えない。
- 右ドラッグが終わった / cancel されたフレームで **ガイドを消すための repaint も 1 回出す**
  (出しっぱなしにしない)。

### 3.4 `ParkedLive` (動画 / 音声) 窓

`show_viewport_immediate` の中なので `self` を借りられる。DTO を経由せず、
`right_drag_guide_for_owner(DetachedWindow(id), ...)` → `draw_right_drag_guide` を
その場で呼ぶ ([ui_fullscreen.rs:11981](../src/ui_fullscreen.rs:11981) の `CentralPanel` 内、
`draw_parked_live_music_window` / `draw_detached_image_window_bar` の後)。

⚠ **native presenter の HUD 側には触らない。** `ParkedLive` の動画窓は native presenter が
映像を出しているが、右ドラッグのガイドは egui 側の overlay で足りる
(`sync_native_video_mouse_gesture_overlay` は**アクティブな** native video 用で、
所有者が `Root` のときだけ動く。ここは変更しない)。

## 4. スコープ外

- ガイドの見た目・レイアウト・配色の変更 (**アクティブ窓と同一にする。差を作らない**)
- native presenter HUD の overlay 経路
- ring picker (`ring_picker`) 本体 = X ボタンの選択 UI。非アクティブ窓では従来どおり出さない
- viewer context registry (R2e)、純粋 reducer (R2f)
- 前段で入れた認識・アクティブ化・実行の経路そのもの (**動作確認済み。触らない**)

## 5. 触ってはいけないもの

- `find_visible_thread_window_matching_rect*` (憲法 1)
- geometry 由来の recreate (憲法 2)
- App への新しい detached 用 bool / Option (憲法 3)。ガイドは既存の
  `mouse_gesture` / `mouse_ring_flick` から**導出**するもので、新しい state ではない
- placement の新しい保存先 (憲法 4)
- **判定条件を時間窓に変えること** (憲法 5)。repaint の予約は可、判定は不可
- 既存テストの削除・弱体化 (憲法 8)

## 6. 完了条件

1. `cargo check -p mimageviewer --bin mimageviewer-core` と
   `cargo test -p mimageviewer --lib` が緑 (テストは `vendor\ffmpeg\bin` を PATH に入れる)。
2. 新規テスト (最低これだけ):
   - `right_drag_guide_for_owner` が、アクティブ窓 (`Root`) と非アクティブ窓
     (`DetachedWindow(id)`) で**同じ入力に対して同じ内容**を返す
   - 抑止条件がすべて効く: モード不一致 / `*_help_visible` が false / `ring_picker` 表示中 /
     `context` 不一致 / `guide_visible()` が false → いずれも `None`
   - 所有者が違う窓の DTO には guide が入らない (窓 A のドラッグ中に窓 B の view が
     guide を持たない)
   - 右ドラッグ終了・cancel 後に guide が `None` になる
3. `cargo fmt` 済み。`python scripts/check_ui_glyphs.py` が 0 件。
4. 完了報告に、**組み立て関数へ移した抑止条件の一覧**と、既存 2 関数がそれを
   1 つも失っていないことの根拠を file:line で書く。

## 7. 実機 smoke (利用者が実施)

1. 複数ウィンドウモードで独立ウィンドウを 3 枚開く
2. 非アクティブなウィンドウ上で右ドラッグを始めて**そのまま止める**
   → 400ms 後にガイドが出る (アクティブ窓と同じ見た目)
3. そのままパターンを描く → 該当行のハイライトがアクティブ窓と同じように追従する
4. 離す → ガイドが消え、ウィンドウがアクティブ化されてコマンドが実行される
5. リングショートカットモードでも 2〜4 (ドーナツが押下位置に出る)
6. 環境設定でガイド表示を OFF にすると、非アクティブ窓にも出ない
7. 再生中の動画ウィンドウ (`ParkedLive`) でも 2〜4
8. アクティブ窓・メイングリッド・native 動画のガイドが従来どおり (退行確認)
