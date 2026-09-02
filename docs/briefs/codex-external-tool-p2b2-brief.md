# 実装ブリーフ: 外部ツール P2b-2 — ツールバー / メニューバー / OsDefault の但し書き

P2b-1 (キースロット / ピッカー / コンテナー対象) が入っている前提。
正本は [docs/external-tool-launch-plan.md](../external-tool-launch-plan.md) の §4.9 / §4.10。
仕様が本ブリーフと食い違って見えたら**正本を優先し、着手前に指摘する**。

## 範囲の歯止め (**2026-09-01 追加。前回の実行がここを越えて 2 時間で打ち切られた**)

**次のファイル / 領域には触らない。**

- `src/app/native_video.rs`、`src/ui_fullscreen.rs` の入力処理、`src/ui_folder_pane.rs`
- detached (別ウィンドウ) の述語 / viewport 経路
- **`docs/detached-rework-plan.md` には一切書かない**
- 既存の予約キー (Esc / 矢印 / Enter) の優先順位そのものの変更

**新しい KeyAction を足すと予約キーとの優先順位が気になるはずだが、そこは本段の仕事ではない。**
既定キーを割り当てないので衝突は起きない。優先順位の調整が必要だと判断したら、
**着手せずに報告すること**。

detached は CLAUDE.md で凍結中で、触るには「ClaudeCode と Codex の双方が症状パッチでない
ことに合意する」手順が要る。前回はその手順を踏まずに、合意があったことにする記録が
正本へ書かれた。**合意していない合意を書かない。**

## 分量について

前回は 23 ファイル / +2,618 行に膨らんで 2 時間で切られた。本段は上記の歯止めの内側なら
その半分以下で収まるはず。**大きくなってきたら、範囲を広げる前に止まって報告する。**

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

### 6. `OsDefault` + `Batch` の但し書き (**正本 §4.5 が P2b と名指ししている**)

`OsDefault` は「まとめて渡す」を選んでも実際には `Each` と同じ動きになる (N 件まとめて
既定アプリへ渡す API が無いため)。**設定 UI にその旨を出す** (正本 §4.5 の表)。

選べるのに黙って別の動きをするのは、この機能でこれまで繰り返し直してきた形そのもの。
外部ツールの設定で `OsDefault` かつ `Batch` を選んだときに、その場で分かるようにする。
文言は実装時に決めてよい。

---

## 正本の P2b 該当箇所 (2026-09-01 に全部洗った)

着手前に `P2b` で grep した結果、本ブリーフが負う義務は上の 1〜6 で全部。
他の `P2` 言及はすべて「P2 ではやらないこと」の注記 (見開き / 混在選択 / フルスクリーン
1 件) で、本段の作業ではない。

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
