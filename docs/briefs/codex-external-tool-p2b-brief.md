# 実装ブリーフ: 外部ツール P2b — ツールバー / キースロット / ピッカー / コンテナー対象

正本は [docs/external-tool-launch-plan.md](../external-tool-launch-plan.md)。
特に **§4.2 (対象の解決)**、**§4.9 (どこから起動するか)**、**§4.10 (設定 UI とキー割り当て)** を先に読む。
仕様が本ブリーフと食い違って見えたら**正本を優先し、着手前に指摘する**。

## 前提 (P2a で入っている)

- 対象集合の解決は純関数 `external_tool::resolve_external_targets` に集約済み。
  `ExternalTargetSource` に `GridContext` / `GridKey` / `Viewer` / `Playback` / `Container` がある。
- **`Container` は解決関数だけあって入口が無い。** それを付けるのが本段の仕事の 1 つ。
- `SelectionPolicy` (`Single` / `Each` / `Batch`) は起動時に効く。
- 混在選択 (仮想ページを含む) は起動前に全体拒否する。

## やること

### 1. キースロット (正本 §4.10)

**固定スロット方式**。動的な `KeyAction` は作らない
([keymap-spec.md](../keymap-spec.md) の identity モデルを壊さないため)。

| アクション | 内容 |
| --- | --- |
| `ExternalToolPicker` | 登録済みツールの選択メニューを出す |
| `ExternalTool1` .. `ExternalTool10` | N 番目のツールを直接起動 |
| `ExternalToolForContainer` | 現在のフォルダー / 本を渡す (ピッカー) |

**ピン留めタグ (`GridTogglePinnedTag1..20`) がそのまま雛形になる。** 同じ形で実装する。

- 配列は `PINNED_TAG_ACTIONS_ARRAY` ([keymap.rs:3042](../../src/keymap.rs:3042)) と同型
- スロット番号の逆引きは `pinned_tag_slot_number` ([keymap.rs:3231](../../src/keymap.rs:3231)) と同型
- `ini_name()` は `pinned_tag_slot_number` の分岐 ([keymap.rs:3337](../../src/keymap.rs:3337)) と同型

CLAUDE.md の要求どおり、**`ini_name()` / `context()` / `trigger()` / `default_chords()` /
`ALL_ACTIONS` / 呼び出し側 helper / [docs/keymap.ini.default](../keymap.ini.default) を揃える**。
`GridTogglePinnedTag1` を grep すると触るべき箇所が全部出る (186 箇所)。

- **既定キーは割り当てない** (空)。10 個も既定を占有すると既存の割り当てとぶつかる。
  利用者が操作カスタマイズで割り当てる。この判断の理由をコード側にも残すこと。
- スロット番号とツールの対応は並べ替えで変わる。**設定 UI にスロット番号を明示する**
  (正本 §4.10)。

### 2. ピッカー UI

`ExternalToolPicker` / `ExternalToolForContainer` から出す選択メニュー。

- 登録済みツールを一覧し、選ぶと起動する。
- **対象が無い / 渡せないときは、選ばせる前に理由を出す。** P2a と同じ拒否文言を再利用する
  (`GridItem::file_operation_refusal` / `checked_virtual_selection_message`)。
  **同じ意味を 2 か所で別々に綴らない。**
- モーダルにするなら **`modal_dialog_block_reason` への登録を忘れない** (登録しないと背面
  グリッドへ入力が漏れる。P1 で実際に指摘された)。

### 3. コンテナー対象の起動 (正本 §4.2 最終行)

現在のフォルダー / ZIP / PDF 本体 **1 件**を渡す。NeeView の「ブックを外部アプリで開く」相当。

**同じツール定義を「ページに対して」「本に対して」の 2 通りで起動できるようにし、
別ツールとして登録させない** (正本 §4.2)。

入口は 2 つ:
- `ExternalToolForContainer` キー
- 右クリックメニュー (フォルダー背景 / コンテナー項目)。**どこに置くのが自然かは
  [context_menu_model.rs](../../src/context_menu_model.rs) の既存構成を見て決め、判断理由を書く。**

### 4. ツールバー (**2026-09-01 訂正**。前版のブリーフは正本より狭かった)

正本 §4.9 は「任意ツールをボタンとして置ける (**お気に入り / タグと同じ動的項目パターン**)」。
実装を見ると、そのパターンは **`ToolbarSectionId` のセクション + `ToolbarSectionDisplay` の
表示モード**を指す ([settings.rs:1644](../../src/settings.rs:1644))。

| モード | 見え方 |
| --- | --- |
| `Buttons` | 登録ツールが**それぞれボタン**として並ぶ |
| `Collapsible` | 折り畳める |
| `Dropdown` | 1 つのプルダウンから選ぶ |

つまり「ツールごとの直接起動ボタン」と「単一ピッカー」は**排他ではなく、同じセクションの
表示モードの違い**。お気に入りと同じ形で両方作る。

- `ToolbarSectionId::ExternalTools` を足す。ラベル / 表示フラグの対応表は
  [ui_main.rs:3362](../../src/ui_main.rs:3362) と [ui_main.rs:3499](../../src/ui_main.rs:3499) にある。
- 描画は `TS::Favorites` の腕 ([ui_main.rs:8354](../../src/ui_main.rs:8354)) をそのまま雛形にする
  (`toolbar_label` → `finish_toolbar_section_lead` → `toolbar_section_fold_toggle` →
  モード別描画、未登録なら `(未登録)`)。
- 設定は `show_toolbar_external_tools` / `toolbar_external_tools_display` /
  `toolbar_external_tools_collapsed` の 3 つ。お気に入りと同じ命名に揃える。
- ツールバーカスタマイズ ([ui_dialogs/toolbar_settings.rs](../../src/ui_dialogs/toolbar_settings.rs)) にも並べる。
- **既定は OFF。** 既存のツールバーを黙って変えない。
- `ToolbarSectionId` / `ToolbarSectionDisplay` はどちらも `#[serde(other)]` の `Unknown` を
  持つので、variant 追加は旧バイナリで settings を全損させない。**その仕組みを壊さない。**

### 5. メニューバー (**前版のブリーフが落としていた**)

正本 §4.9 の表より: **「ファイル」配下に「外部ツール ▸」**。
**全ツールを出す** (`show_in_context_menu` の設定に関係なく)。右クリックは「出すと決めた
ものだけ」だが、メニューバーは全部から選べる場所という役割分担。

## やらないこと (P3 / P4)

- 一時実体化 (`PayloadPolicy`)。仮想ページは引き続き渡せない
- `SpreadPolicy` (3 値まとめて P4)
- 動画の現在フレーム、`{container}` / `{entry}` / `{page}` / `{time}`

## 品質

- **`cargo fmt` を通す** (pre-commit フックが `--check` で弾く)。
- UI 文言を足したら `python scripts/check_ui_glyphs.py` を通す。
- **keymap は既存テストが厳しい。** `cargo test -p mimageviewer --lib keymap` で
  identity / 網羅性のテストが通ることを確認する。
- 見た目を変えたら `cargo test -p mimageviewer --test ui_snapshot`。意図した変化なら
  `UPDATE_SNAPSHOTS=1` で更新し、**PNG を目視してから**コミットする
  ([ui-snapshot-policy.md](../ui-snapshot-policy.md))。
- スロット解決 (番号 → ツール) とコンテナー対象解決は**純関数にして単体テスト**を書く。
  特に「登録が 3 件のときに `ExternalTool5` を押したら何も起きず理由が出る」ことを固定する。

## 報告

変更点の要約と、判断が要った箇所を挙げる。正本と食い違う判断をした場合は**必ず明示**する。
