# 実装ブリーフ: 外部ツール P2a — 対象解決の共通化と `SelectionPolicy`

正本は [docs/external-tool-launch-plan.md](../external-tool-launch-plan.md)。
特に **§4.2 (対象の解決)**、**§4.4 (プレースホルダ)**、**§4.5 (複数選択と見開き)** を先に読む。
仕様が本ブリーフと食い違って見えたら**正本を優先し、着手前に指摘する**。

## 現状

`LaunchTarget` は **1 件しか持てない** ([external_tool.rs:172](../../src/external_tool.rs:172))。
右クリックした 1 件だけを見て、`checked` を無視する
([item-kind-capability-matrix.md](../item-kind-capability-matrix.md) §6-14)。
`SelectionPolicy` は型と保存だけあり、**起動時に一切参照されていない**
(`Single` / `Each` / `Batch` すべて同じ動作)。

## やること

### 1. 対象集合の解決を 1 か所に集約する

**純関数として切り出し、単体テストできる形にする。** 起動元ごとの分岐を各呼び出し側に散らさない。

| 起動元 | 対象 |
| --- | --- |
| グリッド右クリック | `checked` が空でなければ `checked`、空なら右クリックした項目 |
| グリッドのキー操作 | `checked` 優先、無ければ `selected` |
| フルスクリーン / 連結読み | 現在ページ。見開き中は `SpreadPolicy` に従う (P2a では `MainPageOnly` 相当で可、後述) |
| 動画・音声再生中 | 再生中の 1 本 |
| コンテナー対象 | 現在のフォルダー / ZIP / PDF 本体 1 件 |

- `checked` 優先は**既存の一括レーティングと同じ規則**に揃える ([app.rs](../../src/app.rs) の
  `selection_target_indices` 周辺)。**新しい規則を作らない。**
- 順序は「クリック / 現在項目を先頭、以降は一覧の表示順」。`checked` は `HashSet` なので
  **index 昇順で安定化**する (`decide_drag_payload` が同じことをしている、
  [ui_main.rs](../../src/ui_main.rs))。
- **コンテナー対象の起動は P2b (キー割り当て) で入口が付く。** P2a では解決関数だけ用意し、
  呼び出し側は後段でよい。関数が未使用になるなら `#[allow(dead_code)]` ではなく、
  **P2b まで出さない**か、テストから呼んで生かす。

### 2. 混在選択は実行しない (正本 §4.5、2026-08-31 決定)

対象に仮想ページ (`ZipImage` / `PdfPage` / `Stack` / `ZipDir`) が **1 件でも含まれていたら
起動せずトーストで断る**。飛ばして部分実行しない。

- ファイル操作 (コピー / 削除 / D&D) が既に同じ判断をしている。文言と判断の形を揃える
  (`checked_virtual_selection_message` / `classify_checked_file_operation_selection` が
  [ui_dialogs/context_menu.rs](../../src/ui_dialogs/context_menu.rs) にある)。**同じ意味を 2 か所で
  別々に綴らない** — 使えるなら再利用し、できないなら理由をコメントに残す。
- **これは P3 までの暫定である旨をコードに残す。** P3 で実体化が入ればページも渡せるようになり、
  この制限は消える。

### 3. `SelectionPolicy` を実際に効かせる

| 値 | 動作 |
| --- | --- |
| `Single` (既定) | 対象が複数でも**先頭 1 件だけ**渡す |
| `Each` | 対象 1 件につき 1 プロセス起動 |
| `Batch` | 全件を 1 プロセスへ渡す (`{files}`) |

- **`Each` の起動数上限は既定 20。** 超える場合は**確認ダイアログ**を出す。
  既存の起動確認 (`show_external_tool_launch_confirmation`) と同じ枠組みに載せられるか検討し、
  載せられないなら理由を書く。**`modal_dialog_block_reason` への登録を忘れない**
  (登録しないと背面グリッドへ入力が漏れる。P1 で実際に指摘された)。
- **`Batch` には `{files}` が要る。** 現状 `expand_arguments` は 1 トークン → 1 引数だが、
  `{files}` は 1 トークン → N 引数へ展開する。**トークン展開の戻り値を `Vec<OsString>` へ
  変えるなど、構造で解く。** `{files}` を空白連結した 1 文字列にしない (§4.4 の分割規約に反する)。
- `{files}` を含まない `Batch` テンプレートの扱いを決めて書く (`{file}` 自動付与の既定規則と
  衝突しないか。§4.4 の「キーワードを 1 つも含まない引数テンプレートには `{file}` を自動追加」)。
- **起動が複数になる経路では、結果の受け取りも複数になる。** 現行の pending は
  `Option` 1 枠で、P1 レビューで「先行の起動結果を落とす」と指摘済み。**`Each` で N 件同時に
  走ることを前提に見直す。**

### 4. 失敗の通知

- 起動に失敗した件数を通知する。**黙って減らさない。**
- `Each` で一部だけ失敗したときも、何件成功して何件失敗したかを出す。

## やらないこと (P3 以降)

- 一時実体化 (`PayloadPolicy`)。仮想ページは引き続き渡せない
- `SpreadPolicy::Merged` の合成 (P4)。P2a では見開き中も**主ページ 1 件**でよい
- `{container}` / `{entry}` / `{page}` / `{time}` (P4)
- ツールバー / キースロット / ピッカー UI (P2b)

## 品質

- **`cargo fmt` を通す** (pre-commit フックが `--check` で弾く)。
- **対象解決と `SelectionPolicy` は純関数にして単体テストを書く。** 特に
  `checked` 優先 / 順序の安定化 / 混在拒否 / `Each` の上限 / `{files}` の展開。
- UI 文言を足したら `python scripts/check_ui_glyphs.py` を通す。
- テストは `cargo test -p mimageviewer --lib` で確認する。

## 報告

実装後、**変更点の要約と、判断が要った箇所**を挙げる。正本と食い違う判断をした場合は
**必ず明示**する (黙って仕様を変えない)。
