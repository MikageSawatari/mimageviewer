# 実装ブリーフ: 外部ツール P2c — 導線の整理と件数の扱い

正本は [docs/external-tool-launch-plan.md](../external-tool-launch-plan.md) の **§4.5 / §4.9 / §4.10**。
2026-09-01 の決定を反映済みなので、**正本を読めば仕様は全部そこにある**。
食い違って見えたら**正本を優先し、着手前に指摘する**。

## 範囲の歯止め

**次のファイル / 領域には触らない。**

- `src/app/native_video.rs`、`src/ui_fullscreen.rs` の入力処理、`src/ui_folder_pane.rs`
- detached の述語 / viewport 経路。**`docs/detached-rework-plan.md` には一切書かない**
- 既存の予約キー (Esc / 矢印 / Enter) の優先順位そのものの変更

必要になったと判断したら**着手せずに報告する**。

## A. 導線を 2 つに絞る (撤去)

正本 §4.9 の決定どおり、**右クリックとキースロットだけ**にする。

1. **ツールバーのセクションを撤去** — `ToolbarSectionId::ExternalTools`、
   `show_toolbar_external_tools` / `toolbar_external_tools_display` /
   `toolbar_external_tools_collapsed`、`ui_main.rs` の描画、ツールバーカスタマイズの行。
   直前のコミット `791ed6ed` で入れたもの。
2. **メニューバーの「ファイル ▸ 外部ツール」を撤去** — 同コミット。
3. **`show_in_context_menu` を設定ごと削除** — `ExternalTool` の field、
   `settings.db` の列、設定 UI のチェックボックス、右クリック構築時のフィルタ、
   「ON の総数が 10 を超えたら OFF で追加」の既定ロジック
   (`show_in_context_menu_by_default`)。**登録したツールは常に右クリックへ出す。**

`external_tools` テーブルは**未出荷**なので、列の削除にマイグレーションは要らない
(CLAUDE.md「永続データ・スキーマ変更時の判断」)。

## B. 件数の扱い (正本 §4.5 の「件数の扱い」)

1. **`Single` は対象が 2 件以上なら起動しない。** 理由をトーストで出す。
   先頭 1 件だけ渡す現在の動作を置き換える。
2. **既定を `Each` にする** (`SelectionPolicy` の `#[default]` を移す)。
3. **確認 (既定 5) と上限 (既定 10) をツールごとの設定にする。**
   `ExternalTool` に 2 field と `settings.db` に 2 列を足す。
   - 対象件数 N > 上限 → **起動しない**。件数と設定場所が分かるトーストを出す
   - N > 確認 → 既存の確認ダイアログ (件数を出す)
   - 数えるのは**対象件数 N**。`Each` / `Batch` / `OsDefault` + `Batch` すべてに効かせる
   - 既存の `EACH_LAUNCH_CONFIRM_THRESHOLD` (20 固定) はこの設定に置き換える
4. **`Single` のときは確認 / 上限の数値を設定 UI に出さない。** 代わりに
   「1 件だけ渡します。2 件以上選ばれているときは起動しません」と 1 行で説明する。
   効かない数値を並べない。`Each` / `Batch` のときだけ数値を出す。
5. **`Executable` + `Batch` は起動前にコマンドライン長を検査する。**
   `CreateProcess` の上限は 32,767 文字。組み立てた引数列がこれを超えるなら起動せず、
   ファイル数が多すぎる旨を出す。**OS のエラーをそのまま見せない。**
   `Association` の `Batch` は `IDataObject` で渡すのでこの検査は不要。

## C. 11 件目以降のスロット表示

設定 UI の説明文には「11 件目以降は固定キーの対象外」と既に書いてある。
**一覧の行を見ても分かるか**を確認し、スロット番号が空欄 / `—` になっていなければ直す。
既に分かる形なら何もしない (**その判断を報告に書く**)。

## 品質

- **`cargo fmt` を通す** (pre-commit フックが `--check` で弾く)。
- UI 文言を足したら `python scripts/check_ui_glyphs.py`。
- **件数の判定は純関数にして単体テスト**を書く。`Single` の 2 件拒否、上限超過の拒否、
  確認の発火、`OsDefault` + `Batch` にも上限が効くこと、コマンドライン長の検査。
- `cargo test -p mimageviewer --lib`。見た目を変えたら `--test ui_snapshot`。
- 撤去でテストやドキュメントが古くなるので、**関連する記述も一緒に直す**
  (`docs/spec.md`、`docs/toolbar-customization-plan.md`、マニュアル、製品ページ)。

## 報告

変更点と、判断が要った箇所を挙げる。正本と食い違う判断をしたら**必ず明示**する。
