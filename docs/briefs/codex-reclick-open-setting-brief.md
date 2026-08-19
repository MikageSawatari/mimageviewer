# backlog §2.5 追補 — 「選択済み項目のクリックで開く」を設定にし、既定を OFF にする

対象: [next-release-backlog.md](../next-release-backlog.md) §2.5 (専用スレ >>246)。
本体は実装・コミット済み (`dd833a10`)。**利用者判断 (2026-08-19) で設定化する。**

## 0. 決まったこと

- **チェックボックス 1 つ。既定 OFF (従来互換)。**
- 置き場所は**環境設定 → サムネイル**の既存節「一覧のクリック選択」
  ([pages.rs:635](../../src/ui_dialogs/preferences/pages.rs:635) の
  `anchored(ui, state, "thumbnail/click-selection", ...)`)。選択方式の ComboBox と
  その説明文の直後に置く。利用者が探しに行った場所がここだった。
- **「フォルダ / アーカイブだけ開く」案は不採用。** mIV には画像のみのフォルダを本として開く
  設定があり (`should_auto_fullscreen_grid_container`)、「開くと何が起きるか」で分けると
  **同じフォルダという種別なのに中身と設定次第でクリックの意味が変わる**。予測できない挙動に
  なるので採らない (利用者判断)。

## 1. やること

### 1.1 設定

`Settings` に `grid_open_selected_item_on_click: bool` を足す。**既定 `false`**。
`#[serde(default)]` で、この版より前の設定を読んだときも `false` になること。

### 1.2 UI

「一覧のクリック選択」節の説明文の直後にチェックボックスを 1 つ。隣の
`grid_cursor_wrap` ([pages.rs:655](../../src/ui_dialogs/preferences/pages.rs:655)) と同じく
`ui.checkbox` + `ui.small()` の説明の形に揃える。

説明には**動く条件**を書く: 修飾キーなしのクリックだけ、複数選択中は 1 件へ戻すだけで
開かない、タッチは対象外。エクスプローラー方式でのみ働くことも書く。

### 1.3 判定へ足す

`grid_reclick_open_allowed` ([ui_main.rs:4231](../../src/ui_main.rs:4231)) は現在 5 条件。
**6 つ目としてこの設定を足すだけ**。他の条件は変えない。

## 2. やらないこと

- 既存 5 条件の意味を変えない。
- 種別 (コンテナ / 画像) による分岐を入れない (§0)。
- ダブルクリックと Enter の経路を変えない。
- 設定を操作カスタマイズ側へ置かない。

## 3. テスト

1. 既定が `false` で、この field が無い旧設定を読んでも `false`。
2. **OFF のとき、選択済み項目をクリックしても開かない** (= v3.1.1 と同じ挙動)。
3. ON のとき、既存の §2.5 テスト群が従来どおり通る。
   **既に入っている「開く」側のテストは設定 ON を明示する形へ直す** (既定が OFF になったため)。
4. 純関数テストに新しい条件の分岐を足す。
5. mutation: 設定条件を削ると 2 が落ちることを確認して報告する。

## 4. ドキュメント

- [htdocs/mimageviewer/manual/grid.html](../../htdocs/mimageviewer/manual/grid.html):
  一覧操作表の 147 行目付近が現在**無条件に「もう一度クリックすると開く」**と書いてある。
  設定で有効にしたときの動作である旨へ直す。154 行目の補足も設定名に合わせる。
  バージョン番号・内部語を書かない。
- [docs/spec.md](../spec.md) の設定項目に追記する。
- [next-release-backlog.md](../next-release-backlog.md) §2.5 の末尾に追記する
  (**既に閉じた記録は消さず**、設定化と既定 OFF の判断、およびその理由を足す)。

## 5. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
python scripts/check_ui_glyphs.py
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

環境設定の見た目が変わるので snapshot が赤くなったら
[ui-snapshot-policy.md](../ui-snapshot-policy.md) の手順で更新し、更新した旨を報告する。

commit / stage はしない。ブランチは `master`。
