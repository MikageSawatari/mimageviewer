# キーボード入力所有権の一元化 (案A) — 実装計画

> ステータス: **設計確定 / S1・S2 完了・S3 以降未着手** (2026-07-29)。実装 = Codex Sol、レビュー = ClaudeCode。
> 発端: フルスクリーンのパネル内 `TextEdit` へ入力するとショートカットが発動する / 逆に
> ショートカットが効かなくなる問題の**度重なる再発**。直近の修正試行 `deb62b92` は
> `52f2fff9` で revert 済み。
> 案D (viewport / HWND を含む入力ルーティング) は本計画の対象外。バックログへ積む。

## 1. なぜ再発するのか

キーボード入力の所有権を判断する場所が **8 つに分散**している。

| 判断材料 | 使っている場所の例 |
|---|---|
| App のフォーカス bool (`*_has_focus`) | グリッド `handle_keyboard` |
| `ctx.wants_keyboard_input()` | Keymap の Win32 経路、音楽ビュー、注釈モードの一部 |
| `ime_input_active()` (300ms グレース) | 各所に散在 |
| `*_dialog_open` 系の述語 | モーダル判定 |
| `Option<EditState>` の存在 | 音楽ブックマーク改名、(誤用) 画像ブックマーク |
| `Response::has_focus` / `lost_focus` | 各 `TextEdit` の自衛 |
| native overlay の `text_input_active` | 動画 native presenter |
| root ctx と fullscreen ctx の別 | root handler と FS handler |

このため **`TextEdit` を 1 つ足すたびに複数のハンドラへ条件を追記**する必要があり、列挙漏れが
構造的に起きる。「音楽ビューには有る / 画像には無い」という非対称がそのまま実害になった。

確認済みの具体的な穴:

1. **Keymap の egui fallback が `wants_keyboard_input()` を見ない** (`src/keymap.rs` ≈6495)。
   Win32 frame が非アクティブ、または当該フレームに key down が無いと、TextEdit にフォーカスが
   あってもショートカットが egui event を消費して発火する。`docs/keymap-spec.md` ≈79 の
   「フォーカス中 UI が所有している間は消費しない」という仕様と食い違っている
2. **ショートカット消費がウィジェット描画より前**。編集開始 → `request_focus` 予定 → 次 pass 冒頭で
   まだ TextEdit が描かれていない → ショートカットがキーを消費、という窓がある
3. **draft / session state を live ownership として使った** (`book_bookmark_title_edit.is_some()`)。
   保存・キャンセル以外でもフォーカスは失われ得るため、`is_some()` は「今キーを所有している」を
   表さない。加えて当該 state はパネル描画中に `take()` されるため、フレーム内で `None` になる
4. **単行 `TextEdit` の既定 `return_key` が Enter** (egui 0.33.3)。IME Commit と raw Enter が同じ
   pass に来ると確定 → Enter を終了キーとして処理 → `surrender_focus` になる。ショートカット層とは
   無関係のウィジェット層の問題

## 2. 設計 (案A)

### 2.1 所有者の型

viewport pass ごとに **一度だけ** 所有者を決定する。

```text
KeyboardOwner =
    Modal
  | TextInput { viewport, widget_id, phase }
  | FocusedUi { viewport, widget_id }
  | ApplicationShortcut { scope }
  | Unclaimed

TextInputPhase = PendingFocus | Focused | ImeGrace
```

- `PendingFocus` は「編集を開始したが最初の `request_focus` がまだ効いていない」1 pass ぶんの
  一時所有。**編集 state 全体を所有権にしない**ための区別 (穴 3 の再発防止)
- `ImeGrace` は既存の 300ms グレース相当。変換確定直後の取りこぼしを吸収する
- `Modal` は既存のモーダル述語を集約する

### 2.2 消費の許可

ショートカット側は、ゲートが発行した **`ShortcutPermit` を持つときだけ** 消費できる API にする。

- `Keymap::consume_action` / `consume_action_no_repeat` / `pressed_action` / `key_held_action` /
  `consume_first_action` / `consume_rating_action` など**すべての consume 系**が permit を要求する
- 生の `ctx.input_mut(|i| i.consume_key(..))` も同じ permit を要求する経路へ寄せる
- **permit を持たない限りキーを消費できない**ことを型で保証するのが本案の主眼

### 2.3 ゲートの位置

ゲートは **ズーム (Z ホールド) / F12 / help / 編集モード / 通常ナビより前**に置く。
キーを発火させない housekeeping (マウス pending の破棄など) だけはゲート前を許可する。

### 2.4 Keymap の両経路

Win32 `KeyEdge` 経路と egui event fallback の **双方**で同じ ownership 判定を必須にする (穴 1)。

### 2.5 単行入力の共通部品

`return_key(None)` / IME 時のフォーカス維持 / 非 IME Enter の明示 submit / 安定した widget ID を
まとめた共通部品を作り、各 `TextEdit` をそれへ寄せる (穴 4)。既存の最も堅い実装は
`src/ui_metadata_panel.rs` ≈1164 (静止画タグ入力) なので、これを土台にする。

### 2.6 draft state の扱い

`BookBookmarkTitleEdit` などの draft state は **draft のまま残し、ゲートから参照させない**。
初回のフォーカス待ちだけを `PendingFocus` claim として別に発行する。

## 3. 実装スライス

| # | 内容 | 備考 |
|---|---|---|
| S1 | Keymap の ownership 判定を両経路へ (穴 1) | **完了** `70e19f20`。`consume_action` / `pressed_action` / `key_held_action` / `take_key_hold_edges` / `modifier_held_action` を同じ境界へ統一 |
| S2 | ブックマークタイトル `TextEdit` をタグ入力と同構造へ (穴 4 の個別対応) | **完了** `6d4aa372`。明示 widget ID / `return_key(None)` / 非 IME Enter の明示 submit / focus 復元 |
| S3 | `KeyboardOwner` / `TextInputPhase` / `ShortcutPermit` の型と、pass 単位の決定関数を追加 | 純粋な状態遷移として単体テスト可能にする |
| S4 | `Keymap` の全 consume 系を permit 必須へ | 呼び出し側を一斉に移行 |
| S5 | 生 `consume_key` の呼び出しを permit 経由へ移行 + **ソース監査テスト**で router 外の生 consume を禁止 | 列挙漏れの機械的な再発防止 |
| S6 | 単行入力の共通部品を作り、各ダイアログ / パネルの `TextEdit` を移行 | 潜在バグの一括解消 |
| S7 | ドキュメント更新 | `docs/keymap-spec.md`、`CLAUDE.md` の IME 節、本計画書 |

## 4. テストで縛る不変条件

Codex の設計レビューで挙がったもの。**リリース前の回帰テストで検出する運用**にするため、
可能な限り自動テストへ落とす。

### 所有権

- `TextInput` 所有中は、どの `KeyAction` も発火せず、Win32 KeySlot と egui Event の**どちらも
  消費されない**
- `PendingFocus` 中の最初の 1 pass でも、文字キーが FS 層へ漏れない
- `BookBookmarkTitleEdit` が `Some` でも、focus / pending / IME が無ければ FS ショートカットは動く
  (revert した退行の再発防止)
- 逆に editor が focused なら、T / I / Tab / Space / Enter / Escape / 矢印が FS へ漏れない
- Keymap の `frame_had_key_down == false` fallback でも、`wants_keyboard_input == true` なら
  consume しない
- 生 `consume_key` を使う固定キーも、`KeyAction` と同じ permit を要求する
- `Event::Text` / `Event::Ime` をアプリショートカットの正本として使わない

### IME / フォーカス

- `Ime::Commit("…") + Key::Enter` が同一 pass に来た後も、対象 `TextEdit` がフォーカスを保持する
- その次の pass で入力した `T` が文字列へ追加され、FS 状態を変更しない
- `return_key(None)` の `TextEdit` では IME Enter でフォーカスを失わず、非 IME Enter の明示 submit
  だけが 1 回発火する
- widget ID がブックマーク行の追加 / 削除 / 並べ替え / サムネイル状態変化で変わらない

### viewport (案D の前哨。今回は検出のみ)

- 1 回の物理押下を root と fullscreen が二重処理しない
- child `TextEdit` 所有中の child 由来キーを root handler が処理しない
- IME 状態が viewport 間で上書きされない

⚠️ **revert したテストの失敗パターンを繰り返さない**: 前 pass でフォーカス済みにしてから handler を
直接呼ぶだけのテストは、実機経路 (編集開始 → `request_focus` → viewport 転送 → Win32 fallback) を
保証しない。**実際の描画順で handler・編集開始・`request_focus`・IME event を走らせる**こと。

## 5. 実機確認 (リリース前)

自動テストで確信が持てない項目。

- MS-IME で長めに変換し、Enter 確定後すぐ続きの文字を入力できる
- 変換候補選択中の矢印 / Space / Enter / Escape が FS へ漏れない
- 確定後 300ms をまたぐケース、Commit と Enter が同一 / 別フレームで届くケース
- F11 / F12、VST editor からの focus handoff、window / fullscreen 切替
- Hover / ClickToShow 両パネルモード、ブックマーク行の追加・削除・並べ替え後
- 音楽ブックマーク改名で、ショートカットが止まるだけでなく IME Enter 後も入力が継続する
- native 動画のタグ / ブックマーク改名との挙動差
- JIS 配列の文字キー、通常 Enter、テンキー Enter
- 複数モニター、fullscreen viewport 再生成後

実機ログには最低限 `viewport_id` / 送信元 HWND / focused widget ID / `wants_keyboard_input` /
IME 状態 / 選択された `KeyboardOwner` / KeySlot と egui のどちらから消費したか を**同一行**で残す。

## 6. 対象外 (バックログへ)

**案D: viewport / HWND を含む入力ルーティング**。`KeyEdge` に送信元 HWND / viewport を持たせ、
root / fullscreen / detached が自分由来の edge だけを消費する。IME 状態も `ViewportId -> ImeState`
へ分離する。案A の完了後に検討する。
