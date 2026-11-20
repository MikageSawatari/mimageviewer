# 全画面の「…」から F12 の別ウィンドウ表示を切り替える (§1.830)

状態: 設計メモ (2026-11-20、設計担当 ClaudeCode Opus 5.5)。実装前の独立レビュー待ち。
正本のバックログ: [next-release-backlog.md §1.830](next-release-backlog.md)。

## 1. 目的と範囲

F12 (`KeyAction::ToggleDetachedViewerMode`) には画面上の入口が無く、キーを知らない利用者は
一覧と全画面を交互にしか行けないと思っている (利用者 2026-11-20)。F11 のような上部バーの
ボタンは窓が小さいときに並びきらないので増やさず、既存の「…」(その他の機能) の中に 1 行足す。
利用者決定: 静止画と動画の両方に入れる。

- 静止画: 全画面の「その他の機能」パネル (`FsOverflowPanelState::Root`) に「別ウィンドウ表示」行。
  F12 の別ウィンドウ (detached viewport) の中のパネルにも同じ行が出て、そこから OFF に戻せる。
- 動画: native presenter の上部バーの「…」 (`draw_native_more_popup`、今はデインターレースだけ) に
  同じ行。別ウィンドウの中の child native presenter でも同じ。
- 行の表記: `別ウィンドウ表示 [F12]` + 右側に `ON` / `OFF`。キー表記は keymap の
  `ToggleDetachedViewerMode` の先頭 chord (`first_chord_bracket_label` / `first_chord_label`)。
  割り当てを外していれば括弧を出さない (他の行と同じ)。
- 新しい KeyAction・既定キー・設定項目・永続データは足さない。`FullscreenOverflowItemId` は
  並び固定で設定に保存しない (settings.rs のコメント、2026-08-13 利用者判断) ので移行は不要。

範囲外: 一覧 (グリッド) 側への入口、リングメニュー、ヘルプの文言、F11 ボタンの配置変更。

## 2. 入口は 1 か所 (新しい分岐を足さない)

行の操作はどちらも既存の `App::toggle_detached_viewer_mode()` (src/app.rs) を呼ぶだけ。
F12 キー (静止画 `handle_fs_key_input` / 動画 `handle_video_input` / native `NativeVideoKeyEvent` /
main window / ページ一覧) とリング (`RingActionId::ToggleDetachedViewer`) が既に合流している 1 か所で、
remote 制御中・音声モード中 (VST サブモード含む)・source-swap 中・「毎回新しいウィンドウで開く」の
拒否と通知、メディア別窓の一時 migration、設定の反転と保存、トーストはすべてそこで行われる。
行の側では拒否条件を判定し直さない。

## 3. ON / OFF の表示の正本

`toggle_detached_viewer_mode` の振る舞いは 3 通りある:

| 経路 | 条件 (既存の述語) | F12 がすること | 行に出す状態 |
| --- | --- | --- | --- |
| 無効 | `detached_toggle_disabled_by_always_new_images()` (毎回新しい窓 + 非メディア) | 警告だけ | 行を無効 (理由はトーストと同じ文言) |
| メディア別窓 | `always_new_media_f12_target_presentation()` が `Some(target)` | 現在のメディアを main ⇔ 別窓へ一時 migration (設定は変えない) | 現在の表示意図が `DetachedWindow` なら ON |
| 通常 | 上のどちらでもない | `settings.detached_viewer_enabled` を反転・保存し、現在の表示先を合わせる | `settings.detached_viewer_enabled` |

表示側でこの順序を書き写すと、述語の順序が 2 か所に分かれる (One Owner Per Spelling)。
そこで toggle の先頭の経路選択を、副作用の無い分類関数へ切り出して両方から使う:

```rust
#[cfg(windows)]
enum DetachedToggleRoute {
    DisabledByAlwaysNewImages,
    MediaWindow { target: ViewerPresentation },
    Preference,
}
fn detached_toggle_route(&self) -> DetachedToggleRoute; // 既存 2 述語をこの順で読むだけ
fn detached_toggle_row_state(&self) -> DetachedToggleRowState { on: bool, disabled_reason: Option<&'static str> }
```

`toggle_detached_viewer_mode` は remote / 音声モード / source-swap の拒否の後、今の
`if detached_toggle_disabled_by_always_new_images { .. }` と `if let Some(target) = always_new_media_f12_target_presentation() { .. }`
の 2 つの if を `match self.detached_toggle_route()` に置き換えるだけで、各腕の中身は変えない
(挙動不変のリファクタ)。無効時の警告文は定数にして、行の無効理由と共用する。
remote / 音声モード / source-swap の拒否は一時的な状態で、行は有効のままにし、押したときに
toggle の既存の拒否と通知に任せる (表示側で同じ判定を書かない)。

メディア別窓経路の「現在の表示意図」は `always_new_media_f12_target_presentation()` の中で
既に計算している (`video_presentation_transition` 中は target、そうでなければ `viewer_presentation`)。
返り値の target が `DetachedWindow` でないこと = 現在が別窓、なので新しい述語は要らない。

## 4. 静止画パネル

- `FullscreenOverflowItemId::DetachedViewer` を `fixed_order()` の**末尾**に足す
  (既存の行の位置を動かさない。1093×614 では Root は既に 11 行でスクロールする高さであり、
  末尾は ScrollArea の中にある)。`label()` は `"別ウィンドウ表示"`。
  `fs_overflow_panel_body_row_count(Root)` は `fixed_order().len() + 1` なので自動で 1 増える。
- `fullscreen_overflow_panel_view` の腕: label = `first_chord_bracket_label(label, ToggleDetachedViewerMode)`、
  status = ON/OFF、enabled / disabled_reason = `detached_toggle_row_state()`。
- `apply_fs_overflow_panel_action` の `Activate(DetachedViewer)`:
  `self.dismiss_fs_overflow_panel(); self.toggle_detached_viewer_mode();`
  - パネルを先に閉じる理由: パネルの状態は viewer context が所有し (`fs_overflow_panel_state`)、
    F12 で同じ context の表示先が main 全画面 ⇔ 別ウィンドウへ移る。閉じないと、押した窓とは別の窓に
    パネルが開いたまま現れる。F12 キーの経路でも「パネルが開いている間の次のキー押下で閉じる」
    (`handle_fs_key_input` 冒頭) ので、キーと同じ結果になる。拒否された場合も閉じるのはキーと同じ。
  - ページ一覧 (`PageOverview` 行) と同じ「閉じてから既存の操作を呼ぶ」形で、新しい状態は足さない。
- 押した pointer stream: クリックは release で確定し、パネルの Area が押下から所有していた入力は
  その frame で終わる。toggle 後に表示先が変わっても、押下中の stream を別 viewport へ持ち越す
  状態は無い (`fs_front_pointer` / `fs_primary_suppression` は既存の切替経路の扱いのまま)。
  ここは実装前レビューで確認してもらう論点 (§7)。
- 独立静止画窓 (毎回新しい窓): 行は無効で、理由はトーストと同じ文言。keymap-spec の
  「独立静止画窓の F12 は含めない」と一致する。

## 5. 動画 (native presenter)

- `NativeOverlayCommand::ToggleDetachedViewerMode` → presenter の command 変換で
  `NativeVideoOutputEvent::ToggleDetachedViewerMode` (既存の `ToggleWindowMode` と同じ形、source epoch 付きの
  所有 context の event 経路)。App 側 `handle_native_video_output_event` の腕で
  `self.toggle_detached_viewer_mode()` を呼ぶだけ。
- ParkedLive 中の HUD 操作は「ボタンの機能は働かず、クリックが活性化要求になる」既存の一覧
  (`native_video_output_event_is_user_command` 相当の match) に新しい variant を足す。
- VST サブモード中の early-match (`Ev::ToggleWindowMode` 等で VST を抜ける分岐) には足さない。
  音声モード中は toggle 側が既存の警告で拒否する (§2)。
- metadata: `NativeOverlayShortcutLabels.detached_viewer: Option<String>`
  (`first_chord_label(ToggleDetachedViewerMode)`) と `NativeOverlayMetadata.detached_viewer_on: bool`
  (`detached_toggle_row_state().on`)。メディアは「無効」経路に入らないので disabled は持たせない。
- `draw_native_more_popup`: ヘッダ「その他の機能」の下、デインターレース見出しの上に 1 行
  (`native_label_with_shortcut("別ウィンドウ表示", ..)` + 右端に ON/OFF)。押したら
  command を積んで popup を閉じる (デインターレース選択と同じ)。popup の高さは 1 行分増える。
  `native_more_popup_row_rect` の行番号はデインターレース 3 行のままずらす。
- presenter は F12 の placement 切替で作り直されるので、popup の open 状態は新しい presenter へ持ち越されない。

## 6. 状態の組み合わせを減らす検討 (CLAUDE.md 2026-09-28)

- 行を押したらパネル / popup を必ず閉じる。開いたまま表示先が変わる組み合わせを作らない (§4)。
- ON/OFF の表示は毎 frame の既存状態から導出し、行専用の状態・保留・再試行を持たない。
- 切替中 (動画の presentation transition 中) にもう一度押した場合は、F12 キーの連打と同じく
  toggle の既存 reducer が扱う。行の側では抑止しない。

まれな失敗は割り切り (ログ + 通知)。この機能は新しい書き込み経路を持たない
(設定の保存は既存 toggle の `settings.save()`)。

## 7. 実装前レビューで見てほしい論点

1. `detached_toggle_route` の切り出しが toggle の挙動を変えないこと (腕の中身・順序)。
2. 静止画: 描画 pass の中 (`draw_fs_overflow_panel_foreground` の Area の中) から toggle を呼んでよいか。
   既存の F12 キーは key 入力処理、page overview の command も入力段階から呼ぶ。描画中に
   `apply_egui_viewer_presentation_for_current_item` が viewport / binding を変えることで、
   同じ frame の残りの描画や別窓 (detached viewport の closure) に不整合が出ないか。
   出るなら、既存の「frame 境界で適用する」経路 (あれば) に乗せる。新しい pending は足さない方針。
3. 別ウィンドウの中のパネルから押したとき (mount 中の context = detached の context) に、
   キーの F12 と同じ context に作用すること。
4. パネルを開いたまま切り替えた後、押した窓・新しい窓のどちらでも、パネルの状態・クリック・
   pointer の抑止が残らないこと。
5. native の新 event が ParkedLive・source epoch の既存規則に正しく乗ること。

## 8. テスト (ブランチでは関連だけ、run-heavy.ps1 経由)

- 静止画: Root の行一覧に末尾で出ること / label のキー表記が keymap 割当に従う (割当解除で括弧なし) /
  ON/OFF が `settings.detached_viewer_enabled` に従う / 毎回新しい窓の非メディアで無効 + 理由。
- 状態遷移: 行を押すと `fs_overflow_panel_state == Closed` かつ `toggle_detached_viewer_mode` と同じ結果
  (設定反転・表示先)。別ウィンドウの context から押すと OFF に戻る。兄弟 context の状態は不変。
- 実描画: パネルを開いて行をクリックした後、旧窓・新窓の次のクリックが抑止されず、別窓にパネルが出ない。
- 音声モード中は既存の警告で拒否 (設定不変)。
- native: popup の行クリックで `ToggleDetachedViewerMode` command が出て popup が閉じる / metadata の
  ON/OFF・キー表記 / App の event 処理で toggle が呼ばれる / ParkedLive では活性化要求だけ。
- 1093×614: Root パネルのヘッダと閉じるボタンが ScrollArea の外で見え、末尾の行までスクロールで届く
  (既存の `tests/support/scroll_layout.rs` 系の検査に従う)。native popup が 614 の高さに収まる。
- 見た目が変わる ui_snapshot があれば更新して目視。UI 文言は `python scripts/check_ui_glyphs.py`。

## 9. 文書

- マニュアル `htdocs/mimageviewer/manual/fullscreen.html` の「別ウィンドウ表示（F12）」と、動画の上部バーの「…」の説明。
- `docs/keymap-spec.md` の「その他の個別操作」に行の説明、`docs/video-architecture.md` の HUD overlay 節。
- `docs/detached-rework-plan.md` §11 に触れた範囲と、構造的修正である判断の記録。
