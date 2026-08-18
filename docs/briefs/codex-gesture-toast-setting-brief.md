# backlog §4.4 — マウスジェスチャ実行後の通知を消せるようにする

対象: [next-release-backlog.md](../next-release-backlog.md) §4.4 (専用スレ >>246)。
v3.1.2 で対応する、と利用者へ回答済み。

要望は「ジェスチャを実行するたび右上に出る `[Gesture: ...]` を出さないようにしたい」。
**右ドラッグ中に出る操作ガイドとは別機能**で、ガイドの既存設定は変えない。

## 0. 現状 (source inspection、2026-08-19)

通知は 1 箇所からしか出ていない。
[gamepad_input.rs:936](../../src/app/gamepad_input.rs:936) の
`trigger_mouse_gesture_action` に 2 つ:

```rust
if matches!(action, RingActionId::None) {
    self.show_feedback_toast(format!("[Gesture: {pattern_label} なし]"));
    return None;
}
self.show_feedback_toast(format!(
    "[Gesture: {pattern_label} {}]",
    action.label_for_context(action_context)
));
self.apply_ring_action(ctx, action_context, action, "mouse-gesture")
```

`show_feedback_toast_with_duration` ([app.rs:58591](../../src/app.rs:58591)) は egui の
toast と **native video overlay の両方**へ出すので、**この 1 箇所を抑えれば
grid / フルスクリーン / detached / native 動画のすべてで消える**。文脈ごとの分岐は要らない。

## 1. やること

### 1.1 設定を足す

[ring_shortcut.rs:1938](../../src/ring_shortcut.rs:1938) の既存 2 つと同じ形で:

```rust
#[serde(default = "default_true")]
pub mouse_gesture_result_toast_visible: bool,
```

`Default` impl ([ring_shortcut.rs:2170](../../src/ring_shortcut.rs:2170)) に `true`、
既定値テスト ([ring_shortcut.rs:2649](../../src/ring_shortcut.rs:2649)) に assert を足す。

**既定 ON は必須**。リリース済みの挙動なので、既存利用者の見え方を変えない。
`#[serde(default = "default_true")]` により、この版より前の設定を読んでも ON になる。

### 1.2 UI

[pages.rs:886](../../src/ui_dialogs/preferences/pages.rs:886) の `page_operation_behavior` に
3 つ目の checkbox を足す。文言は既存 2 つと揃え、**ガイドではなく実行後の通知**だと
分かるようにする。

先頭の `ui.small(...)` は現在「右ドラッグ操作中に表示するガイド」しか説明していないので、
実行後の通知も含む説明に直す。

### 1.3 抑制

上の 2 箇所の `show_feedback_toast` だけを設定で抑える。

**`apply_ring_action` が出す feedback は消さない。** アクション自身のエラーや結果表示は
別の用途で、これを消すと「実行したのに何も起きていないように見える」ことになる。
抑制はこの 2 行に限定する。

## 2. やらないこと

- ガイド (`mouse_ring_help_visible` / `mouse_gesture_help_visible`) の挙動を変えない。
- ジェスチャの実行そのものを止めない。**通知を切っても動作は必ず実行される。**
- 文脈ごと (Grid / Image / Video / Edit) に別設定を作らない。要望は 1 つの ON/OFF。
- リングショートカット側の通知には手を出さない (要望はジェスチャのみ)。
- 時間窓・遅延で「出す/出さない」を決めない。設定だけで決まる。

## 3. テスト

`trigger_mouse_gesture_action` を直接呼べる形でテストする (既存の gesture テストの
置き場所に合わせる)。

1. 設定 ON: 割り当て済みジェスチャで toast が出る。
2. 設定 ON: 未割り当て (`RingActionId::None`) で「なし」の toast が出る。
3. 設定 OFF: 1 と 2 の**どちらでも toast が出ない**。
4. **設定 OFF でもアクションは実行される** (`apply_ring_action` が呼ばれた結果が観測できる形で)。
   ここが一番大事なので必ず書く。
5. 既定値が true で、旧設定 (この field が無い JSON) を読んでも true になる。
6. mutation: 抑制条件を削る / 反転すると 3 と 4 が落ちることを確認して報告する。

## 4. ドキュメント

- [htdocs/mimageviewer/manual/](../../htdocs/mimageviewer/manual/) のマウス操作 /
  操作カスタマイズのページに設定を追記する。**バージョン番号を書かない**
  (CLAUDE.md「マニュアル・製品ページの記述方針」)。内部語も使わない。
- [docs/spec.md](../spec.md) の設定項目に追記する。
- [next-release-backlog.md](../next-release-backlog.md) §4.4 に結果を追記して閉じる
  (エントリ末尾に追記。冒頭の記述を消さない)。

## 5. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
python scripts/check_ui_glyphs.py
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

UI 文言を足すので `check_ui_glyphs.py` を必ず通すこと (0 件で exit 0)。
環境設定の見た目が変わるので、snapshot テストが赤くなったら
[ui-snapshot-policy.md](../ui-snapshot-policy.md) の手順で更新し、**更新した旨を報告する**。

commit / stage はしない。ブランチは `master`。
報告には変更ファイル一覧、追加テスト、mutation 結果、snapshot 更新の有無を含める。
