# v2.7.0 その他不具合・要望セッション向け 第3回レビューフィードバック

作成: 2026-07-22
比較基準: `v2.6.0` (`0d504f6d`) .. `61218736`
今回の主な再確認コミット: `61218736`

## 結論

前回指摘した「最大192行の sample best-fit」は、全行を1 frame最大192行ずつ測る job に修正され、
可視範囲外の最長値へ exact に収束する基本設計とテストを確認しました。ただし exact の入力世代と
状態列の固定サンプルに **P2 が2件**残っています。

## [P2] タグ・遅延メタの内容変更を検知せず、異なる時点の幅を合成する

- 場所: `src/ui_main.rs:11088-11105`, `src/ui_main.rs:11244-11265`,
  `src/app.rs:36537-36549`, `src/tag_ops.rs:711-728`
- シナリオ: 193行以上のタグ列または遅延列で best-fit を開始する。1 batch目の行を空タグ/`...`で
  測った後、その行の tag cache や page count/created/dimensions/video codec が到着する。job key は
  `items_generation`、表示順、font/DPI 等しか持たないため stale にならず、後続 batchと混在した
  最大幅を確定する。1 batch目に届いた長い値は再測定されず、列幅が不足する。
- 破れている不変条件: exact job が適用する値は、単一の内容世代に属する全行の最大幅である。
- 根本原因: 「一覧 identity/order の世代」と「セル表示内容の世代」を同一視している。
  `details_lazy_meta.apply_patch` と tag cache 更新は通常 `details_order_revision` を進めない。

### 必要な修正

1. tag cache と遅延メタ結果に、best-fit が監視できる内容 revision を設ける。
2. 対象列の内容 revision が変わった job は結果を適用せず、再開始または明示的に再走査する。
3. 全列共通 revision で無関係な更新により starvation しないよう、少なくとも tags / lazy metadataを
   列依存で扱う。連続 load 中の再開始方針も state transition として固定する。
4. job 完了後に内容が変わった場合の仕様（幅を自動再計算するか、次の user best-fit まで固定か）は
   文書化する。少なくとも job 実行中に世代が混ざった結果は適用しない。

### 必須回帰テスト

- 1 batch目を測定後、その行へ長い tag を到着させると旧 job が破棄/再走査され、最終幅が収まる。
- page count、image dimensions、video codec のいずれかでも同じ遷移を通す。
- 無関係な列の cache 更新では対象 job を不必要に破棄しない。
- load が完了した安定世代では全行を1回ずつ測って確定する。

## [P2] 状態列を固定サンプルだけで確定し、実際の行を一度も測らない

- 場所: `src/ui_main.rs:11120-11140`, `src/ui_main.rs:11951-11961`,
  `src/bookmark_browser.rs:340-352`
- シナリオ: ブックマーク一覧で missing な本ページを表示すると、状態列は
  `フォルダー / 12345 ページ / 見つかりません` のような動的文字列になる。閲覧履歴もページ数に
  応じて `12345 / 123456` になり得る。しかし best-fit は `補 レ 消 隠 文 回 ピ`、
  `9999 / 9999`、`未読` の3サンプルだけを測り、`needs_dynamic_rows=false` で即時確定する。
- 破れている不変条件: 状態列も現在一覧全体の実表示文字列を収める。固定サンプル shortcut は
  表示値の上限を数学的に包含する場合だけ使える。
- 根本原因: 通常グリッドの有限 badge vocabulary と、bookmark/reading-history view で状態列を
  転用する動的文字列を同じ固定サンプル扱いにした。

### 必要な修正とテスト

- bookmark view / reading-history view の State は他の動的列と同じ bounded all-row scan を行う。
  通常グリッドだけ固定サンプルを使うなら view kind を job key に含める。
- missing suffix を持つブックマーク、5桁以上の page hint、5桁以上の既読位置を用意し、実文字列の
  幅へ収束するテストを追加する。
- job 中に通常/ブックマーク/閲覧履歴 view が切り替わった場合は stale になることを検証する。

## 解消を確認した前回指摘

- 全行を1 frame最大192行で分割測定し、193行目以降の最長値も測る。
- width は全行完了後にだけ適用する。
- items generation、sort/filter order、列、font、DPI の変更で旧 job を破棄する。
- 全 job 共通で1 frame 1 batch の budget を持つ。
- font import の source-open/copy/parse failure cleanup は維持されている。

## 検証結果

- `cargo test -p mimageviewer --bin mimageviewer-core details_best_fit`: 9 passed
- `cargo test --lib copy_font_source_open_failure_does_not_leave_empty_target`: 1 passed
- `cargo fmt --all -- --check`: passed
- `cargo test --workspace`: passed（失敗0）

注: `cargo test --lib details_best_fit` は該当テストが `src/main.rs` 側にあるため0件になります。
上記 `--bin mimageviewer-core` のコマンドを使用してください。

## 完了条件

上記2件を修正し、「1 frame上限」「identity/order revision」「cell-content revision」「view kind」、
「exact幅を適用できる条件」を対応表にしてください。
