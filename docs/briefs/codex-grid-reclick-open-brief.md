# backlog §2.5 — 選択済み項目を、修飾なしの再クリックで開く

対象: [next-release-backlog.md](../next-release-backlog.md) §2.5 (専用スレ >>246)。
v3.1.2 で対応する、と利用者へ回答済み。

要望は「一覧で既に選択されている項目をもう一度シングルクリックしたら、Enter /
ダブルクリックと同じように開いてほしい」。

## 0. 現状 (source inspection、2026-08-19)

グリッドセルの入力は [ui_main.rs:12471](../../src/ui_main.rs:12471) 付近の 1 関数にある。
順序は次のとおり:

1. タグバッジの hit-test (当たれば `open_tag_view_for_tag` して `return`)
2. `response.clicked()` → `apply_grid_click_selection(...)` で**選択状態を更新**
3. `grid_open_from_click_allowed()` が false なら `return` (dialog / context menu 等)
4. 開く経路は `response.double_clicked()` の **2 箇所だけ**
   ([ui_main.rs:12530](../../src/ui_main.rs:12530) の bookmark view、
   [ui_main.rs:12536](../../src/ui_main.rs:12536) の本体。後者が Folder / ZIP / PDF /
   画像 / 動画などの item 種別分岐を全部持っている)

つまり **`double_clicked()` の 2 箇所を「起動する」述語に置き換えれば足りる**。
item 種別ごとの open 分岐を複製してはならない。

## 1. やること

### 1.1 クリック前の選択状態を snapshot する

`apply_grid_click_selection` は**選択を書き換える**ので、その**前**に
「このセルは既に選択されていたか」を控える。1 回目のクリックで選択された直後に、
同じクリックで開いてはいけない。

### 1.2 判定を純関数にする

egui 抜きで unit test できる形にする (`ui_main.rs` の既存の純関数群の隣)。入力は
少なくとも次を取り、**すべてが揃ったときだけ true**:

| 条件 | 理由 |
| --- | --- |
| クリック前に選択済みだった | 1 回目のクリックで開かない |
| `GridClickSelectionMode::Explorer` | チェック方式のクリックは選択操作のまま (仕様) |
| Ctrl / Shift なし | 範囲・追加選択を open に変えない |
| touch 由来の pointer でない | [touch-support-plan.md](../touch-support-plan.md) §5.8「再タップ open は入れない」を維持 |

`has_touch_derived_pointer_activity()`
([touch_correlation.rs:78](../../src/touch_correlation.rs:78)) が既にあり、
[ui_main.rs:717](../../src/ui_main.rs:717) の `should_sync_grid_scrollbar` が同じ用途で
使っている。**新しい touch 判定を作らない。**

### 1.3 起動述語へ合流させる

`double_clicked()` の 2 箇所を `activate` (= `double_clicked() || 再クリック open`) に
置き換える。

**同じ pointer release が両方成立しても open は 1 回**であること。1 つの述語に
まとめれば構造的にそうなるので、2 つの分岐を並べない。

右クリック、タグバッジ、drag 開始、`grid_open_from_click_allowed()` が false の状態では
発火しない。これらは既に上流で `return` しているので、**新しい guard を足す必要はない**。
足したくなったら、それは配置位置が違う。

## 2. やらないこと

- item 種別ごとの open 分岐を複製しない。既存の activation 境界へ合流させる。
- ダブルクリック経路の挙動を変えない。**§2.6 (ダブルクリックが時々無反応) は別件で、
  本項で症状が見えなくなっても解決扱いにしない。**
- チェック方式 (`GridClickSelectionMode::Check`) の挙動を変えない。
- touch の再タップ open を入れない。
- 設定項目を作らない (要望は挙動そのもの)。**必要という判断が出たら報告して止まる。**
- 時間窓 (2 回目までの猶予 ms 等) を使わない。判定は「クリック前に選択済みだったか」
  という状態であって、経過時間ではない (憲法 §2 規則 5)。

## 3. テスト

### 3.1 純関数 (必須)

条件 4 つそれぞれを 1 つずつ崩して false になること、全部揃って true になること。

### 3.2 handler level

`setup_app` 系のテストで、grid セルのクリック応答を通して:

1. 未選択セルの 1 回目クリック → 選択されるだけで**開かない**。
2. 続けて同じセルをクリック → 開く。
3. Ctrl / Shift 付きのクリックでは開かない (選択操作のまま)。
4. チェック方式では開かない。
5. ダブルクリックは従来どおり開く (回帰)。
6. `grid_open_from_click_allowed()` が false のとき (ダイアログ表示中) は開かない。
7. mutation: 1.1 の snapshot を「クリック後の選択状態」に変えると 1 が落ちること
   (= 1 回目で開いてしまう) を確認して報告する。**これが一番重要な回帰。**

### 3.3 回帰確認 (自動テストで届く範囲は自動で、残りは手順を報告に書く)

Folder / ZIP / PDF / 直読み RAR / 変換対象アーカイブ / Image / Video、Ctrl・Shift 選択、
native D&D、タグバッジ、touch tap。

## 4. ドキュメント

- [htdocs/mimageviewer/manual/](../../htdocs/mimageviewer/manual/) の一覧操作 /
  マウス操作のページに追記する。バージョン番号・内部語を書かない。
- [docs/spec.md](../spec.md) に挙動を追記する。
- [next-release-backlog.md](../next-release-backlog.md) §2.5 に結果を追記して閉じる
  (エントリ末尾に追記。冒頭の記述を消さない)。

## 5. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。
報告には変更ファイル一覧、追加テスト、3.2-7 の mutation 結果、手動確認が要る項目を含める。
