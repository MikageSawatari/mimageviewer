# ブリーフ: FS 生キーの permit 経由化 と IME 状態の viewport 分離

実装 = Codex Sol / レビュー = ClaudeCode。v2.9.1 出荷前。

正本は [keyboard-input-ownership-plan.md](keyboard-input-ownership-plan.md)。本ブリーフはその
S4 / S5 のうち **今回着手する範囲だけ**を切り出したもので、設計判断を上書きするものではない。
着手前に同計画書の §2 (設計) と §4 (テストで縛る不変条件) を読むこと。

## 1. 直す対象

### 1-A. 静止画フルスクリーンの Esc / 矢印が入力所有権を迂回している

`handle_fs_key_input` は [src/ui_fullscreen.rs](../src/ui_fullscreen.rs) の冒頭で
`self.keyboard_owner_for_pass(ctx)` を呼ぶが、結果を `let _owner` で捨てている。その後の

- Esc: `ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))`
- 矢印: `arrow_right` / `arrow_left` / `arrow_down` / `arrow_up` の生 `consume_key`

は**無条件に消費される**。音楽ビューには `ctx.wants_keyboard_input()` による早期 return が
あるが、静止画側には無い。

結果、左パネルのブックマーク名 `TextEdit` (`book_bookmark_title_edit`、描画は
[src/ui_adjustment_panel.rs](../src/ui_adjustment_panel.rs)) を編集中に

- Esc を押すと入力欄ではなくフルスクリーンが閉じる
- 矢印でカーソル移動ではなく前後ページへ移動する

同一フレーム内で `handle_fs_key_input` がパネル描画より先に走るため、`TextEdit` がイベントを
見る前に消費されている。`common_modal_dialog_open` にもこのフィールドは含まれていない。

計画書 §4 が既に不変条件として挙げている
「editor が focused なら、T / I / Tab / Space / Enter / Escape / 矢印が FS へ漏れない」
の未達分である。

### 1-B. `App::ime_composing` が全 viewport 共有で、回復手段が無い

[src/app.rs](../src/app.rs) の `update_ime_state` にある 4 箇所だけが writer で、egui の
`ImeEvent` に完全に依存している。**viewport の破棄 / focus 喪失 / タイムアウトによる clear が
存在しない**。`Enabled` を受けた viewport が `Disabled` / `Commit` を返す前に消えると
(フルスクリーン終了、detached 破棄、IME 切替) `true` に張り付く。

張り付くと `ime_input_active()` が真を返し続け、`keyboard_ownership_snapshot` が
`TextInputPhase::ImeGrace` を返し、`blocks_legacy_keymap_shortcuts` / `blocks_legacy_main_shortcuts`
の両方が真になって**アプリのショートカットが全滅する**。`e8b55b2d` が直したのは入口
(helper の IME サンプリング順序) と native 側の消費者であって、**増幅器は残っている**。

対称性も壊れている: [src/ime_focus.rs](../src/ime_focus.rs) 側は per-viewport の `data_temp` に
載っているので egui の GC で自然回復するが、App 側だけ回復しない。

計画書 §4 の「IME 状態が viewport 間で上書きされない」に対応する。

## 2. 制約 (これを外すと退行する)

**2-1. ブランケットな `ctx.wants_keyboard_input()` ガードを入れてはならない。**
[src/ui_fullscreen.rs](../src/ui_fullscreen.rs) の該当箇所のコメントにあるとおり、Esc / 矢印を
`input_mut` で先に消費しているのは **補正パネル内のスライダー等に矢印を奪われないため**である。
`wants_keyboard_input()` はスライダーへの focus でも真になるので、これでゲートすると
「スライダーを触った後は矢印でページ送りできない」という退行になる。

したがって **ブロック条件は `TextInput` 系の phase だけ**にする:

- ブロックする: `TextInputPhase::Focused` / `FocusRecovery` / `ImeGrace` / `PendingFocus`
- ブロックしない: `KeyboardOwner::FocusedUi` (= テキストでない widget の focus)

`blocks_legacy_keymap_shortcuts` は `FocusedUi` も含むので、**それをそのまま流用しないこと**。
生キー用の述語を別に用意するか、既存述語を phase 別に分ける。どちらにするかは実装者判断だが、
「なぜ `FocusedUi` を通すのか」をコメントに残すこと。

**2-2. 症状パッチにしない。** `book_bookmark_title_edit.is_some()` を見て早期 return する、
`common_modal_dialog_open` にフィールドを足す、といった個別対応をしない。`_owner` を捨てている
のが原因なので、**捨てずに使う**のが構造的修正である。他に `TextEdit` が増えても自動的に効く形
にすること。

**2-3. 1-B は timeout clear で済ませない。** 「300ms 経ったら false に戻す」は症状パッチであり、
既存の 300ms グレース (`ime_input_active` の後段) と意味が混ざる。**IME 状態を viewport ごとに
持つ**のが構造的修正。`ime_focus.rs` 側が既に per-viewport の `data_temp` を使っているので、
所有をそちらへ寄せるか、同じ粒度の型を App 側に持たせる。どちらを選んだかと理由を
[keyboard-input-ownership-plan.md](keyboard-input-ownership-plan.md) へ追記すること。

**2-4. detached 凍結ルール。** detached 述語 / viewport 経路に触れる場合は CLAUDE.md
「Detached viewer リワーク中のルール」に従い、着手前に
[detached-rework-plan.md](detached-rework-plan.md) §2 を読むこと。

## 3. テストで縛ること

計画書 §4 の該当項目を実テストにする。**§4 末尾の警告を必ず読むこと** —
「前 pass でフォーカス済みにしてから handler を直接呼ぶだけのテスト」は revert 済みの失敗
パターンで、実機経路を保証しない。**実際の描画順で handler・編集開始・`request_focus`・
IME event を走らせる**こと。

1. ブックマーク名 `TextEdit` に focus がある状態で、Esc がフルスクリーンを閉じない
2. 同状態で ← → ↑ ↓ がページ送り / ファイル移動を起こさない
3. `PendingFocus` の最初の 1 pass でも Esc / 矢印が FS へ漏れない
4. **スライダー等の非テキスト widget に focus がある状態では、矢印が従来どおりページ送りする**
   (2-1 の退行ガード。これが無いと次の実装者が同じ罠を踏む)
5. `book_bookmark_title_edit` が `Some` でも focus / pending / IME が無ければ FS ショートカットは動く
6. 1-B: viewport A で `Ime::Enabled` を受けた後 A が破棄されても、viewport B のショートカットが
   死なない
7. 1-B: `Enabled` の後 `Disabled` / `Commit` が来ないまま viewport が消えるケースで、
   `ime_input_active()` が回復する

## 4. 検証

```
cargo fmt --all -- --check
cargo test -p mimageviewer --lib
cargo test -p mimageviewer --test ui_snapshot
python scripts/check_ui_glyphs.py
```

実機確認はレビュー側 (ClaudeCode) が
[keyboard-input-ownership-plan.md](keyboard-input-ownership-plan.md) §5 の項目で依頼する。
最低限、**フルスクリーン左パネルのブックマーク名を編集中に Esc / 上下左右 / Tab / Enter /
T / I** を押して、入力欄側が正しく受け取ることを確認する。

## 5. スコープ外

- S5 のソース監査テスト (router 外の生 `consume_key` 禁止) — 今回は FS の Esc / 矢印のみ。
  監査テストは列挙漏れの再発防止として次版で入れる
- S6 の単行入力共通部品への一括移行
- backlog §1.28 のカーソル auto-hide (native window 側、Stage 5 で対応)
