# レビュー依頼: 外部ツール連携 + 焼き込み段階の統一 (前半)

Codex Sol への依頼書。**新しいセッションで出す前提。**

## 依頼の趣旨

複雑な領域を短期間で広く触ったので、**第三者の目で欠陥を洗ってほしい**。
同意ではなく、**見つけた穴**が欲しい。合意点の要約は不要。

対象リポジトリ: `C:\home\mimageviewer-extlaunch`
対象ブランチ: `context-menu-owner-hwnd` (master へは未マージ)

```
git -C C:\home\mimageviewer-extlaunch diff master...HEAD
```

**要約を信じず、コードを読んで判断すること。** 以下は「何を意図したか」であって
「何が正しいか」ではない。

## 範囲 A: 外部ツール連携 (master へマージ済み)

`git log --oneline` で `external_tool` / `materializer` 関連を辿れる。正本は
[docs/external-tool-launch-plan.md](../external-tool-launch-plan.md)。

主なファイル: `src/external_tool.rs`, `src/materializer.rs`,
`src/native_context_menu.rs`, `src/ui_dialogs/preferences/pages.rs`

### 特に見てほしい設計判断

1. **`PayloadPolicy` は 3 値** (`TempEdited` / `TempOriginal` / `OriginalFile`)。
   「一時ファイルか実ファイルか」を値の名前で区別し、**出力の形は編集の有無で変えない**
   (`TempEdited` は常に PNG、`TempOriginal` は常に元バイト列)。
   → 受け取る側から見て予測可能にする意図。**穴はないか。**
2. **`decide_materialization` から編集の有無の引数を落とした。** 編集は「合成段を通るか」
   だけを決め、出力の形は決めない。
3. **launch ACK の frame 境界。** 進捗 modal の描画と `authorize_..._after_ui` の
   単一所有 (`external_tool_launch_ui_frame` に frame 番号を持つ)。embedded フルスクリーンでは
   main update の tail が飛ぶため、fs body 側からも呼ぶ。**専用 viewport では両方通る**ので
   先に到達した方が描く。→ **frame 番号で足りているか。他に tail が飛ぶ経路はないか。**
4. **入力ブロックの導出**を描画述語 (`external_tool_materialize_progress_visible`) から
   行うようにした。以前は pending の非空から導いており、supersede 済み要求が
   「ダイアログが無いのに入力だけ止まる」状態を作っていた。
5. **`MaterializeSource::VideoFrame`** — 提示済みフレームの PTS をミリ秒整数で持ち、
   cache key に入れる。時刻は `screenshot_target_secs` (`{time}` の
   `position_secs` とは**別の値**)。
6. **`PlaceholderFacts`** — 一時ファイルのパスから復元できない値 (書庫本体 / エントリ名 /
   ページ番号 / 再生位置) を UI スレッドで確定させ、request と 1 つの struct で持ち回る。
7. **見開き展開** (`external_tool_spread_expansion`) は stack 展開の隣・件数判定より前。
   `Merged` は <kbd>Ctrl+E</kbd> と同じ経路で合成。`BothPages` は**読み順** (ページ番号昇順)。
8. **未リリーステーブルの形の版** (`external_tools_unreleased_shape`)。列の有無ではなく
   **値の綴りまで**含む。綴りを変えると読み込みが `Incompatible` になり設定全体が保存
   されなくなるため。

### 直したバグ (再発していないかも見てほしい)

- ツールチップを `main_hwnd` の owned window として作っていたため、F12 の別窓で右クリック
  するとメインが Z 順で引き上げられていた → owner を外した
- `TTM_ADDTOOLW` が `cbSize` で拒否されていた (common controls v6 の長さを送っていたが、
  マニフェスト無しなので v5 が動いている) → v1 の長さを送る

## 範囲 B: 焼き込み段階の統一 (前半のみ、未マージ)

正本は [docs/bake-stage-unification-plan.md](../bake-stage-unification-plan.md)。
**既定は全部現状維持なので、この 6 コミットでは誰の挙動も変わらないはず。**
→ **本当に変わらないかを確かめてほしい。**

主なファイル: `src/bake_stage.rs` (新規), `src/books.rs`, `src/materializer.rs`,
`src/settings.rs`, `src/ui_dialogs/preferences/pages.rs`

### 特に見てほしい点

1. **段の挿入位置。** 表示用補正と AI は `apply_adjustments_fast` の**後**・注釈の**前**に
   入れた。表示側では注釈が最終合成の上に載る (`ensure_comic_composite_texture` の base が
   `ensure_final_composite_pixels`) ため。回転と切り取りは幾何なので後段のまま。
   → **この順序が表示側と本当に一致しているか。** 特に切り取り・回転との相対順序。
2. **`final_composite` の再利用。** 自前で鎖を組まず、`build_final_composite_plan_after_ai` +
   `execute_final_composite` を通す。カラー化の適用可否 (近モノクロ判定) も含めて 1 か所。
   `adjust_before_effect` は None に潰している (色調補正は手前で済んでいるため)。
   → **潰し方が正しいか。plan の他のフィールドで焼き込み側に合わないものはないか。**
3. **`BookAiSnapshot` は「実際にアップスケールしたか」を返す** (要求ではなく結果)。
   スマートシャープの固定規則 (`effective_smart_sharpen`) の入力になるため。
4. **指紋に段を含めた** (`edit_fingerprint`)。含めないと段違いの一時ファイルが再利用される。
   → **他に段で変わるのに鍵へ入っていないものはないか。**
5. **設定は機能ごとに 4 つ**、UI は 1 つの表。既定は
   製本 = `Edits` / 画像 Ctrl+E = `DisplayAdjust` / バッチ Ctrl+E = `DisplayAdjust` /
   外部ツール = `Edits`。**ただしバッチはまだ配線しておらず `Edits` のまま動く**
   (段取り 5 が未着手)。→ **設定値と実挙動の食い違いが利用者に見えないか。**
6. `book_baked_edit_snapshot` は製本とバッチ Ctrl+E が共有するので `stage` を引数にした。
   → **呼び出し側の割り当てが正しいか** (製本経路が batch の設定を読んでいないか、逆も)。

### 既知の未完 (指摘不要)

- AI のモデル決定を表示側から切り出していないので、`ai` は常に `None`。
  **深い段を選んでも AI は焼かれない。** 段取り 4 で解消予定。
- 画像 Ctrl+E はまだ表示画素経路のまま。
- マニュアル / 製品ページ未更新。

## 特に疑ってほしいこと

私はこの作業中に**自分で 5 回、実測や第二意見で前提を覆されている**。

- 「動画のフレームを作る経路が無い」→ あった (`video::screenshot::capture_frame`)
- 「メニューのオーナー HWND がアクティブ化させている」→ 前面窓は動いていなかった
- 「注釈は寸法変更に追従する」→ しない (絶対ピクセル固定)
- 「カラー化は AI 推論」→ 純 CPU
- 「一時ファイルの無効化は既存の追い出し手順の再利用で済む」→ 全然足りなかった

**同じ形の思い込みが残っている前提を探してほしい。** 特に「既存の仕組みがあるから大丈夫」
と書いている箇所は、実際にその仕組みが**この経路でも働くか**を確かめてほしい。

## 回答の形

file:line 付きの具体的な指摘で。重大度順。
「全体としては妥当」のような総評は不要。**穴が無ければ「無い」と言い切ってよい。**
