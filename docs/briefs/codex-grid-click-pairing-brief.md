# グリッドのクリック対を、egui のクリック回数から自前の状態へ移す

対象: 利用者の実機確認 (2026-08-19)。**v3.1.2 に入れる。**

症状: **ダブルクリックで PDF を開き、Esc で閉じ、すぐ同じ PDF を 1 回クリックすると開く。**
(§2.5 の「選択済み項目のクリックで開く」設定は OFF。settings.db で確認済み)

## 0. 経緯と、なぜ継ぎ足しをやめるか

このクリック判定には既に 3 段の継ぎ足しが乗っている。

1. egui のクリック回数 (1/2/3)
2. 「triple も開く」(`d493180b`) — 3 回目が triple にされて開かない問題への対処
3. 「同じセルのときだけ」(`43439bb8`) — 別セルが対になる問題への対処

今回さらに 4 段目 (「開いたら区切る」) を足すところだが、**そこで止めて 1 つに集約する**。
CLAUDE.md「バグ修正の一般原則」の、相互排他的な状態を bool で足していかず単一の
state owner へ集約する、に当たる。

**egui 本体は触らない** (利用者判断 2026-08-19)。egui の triple クリックは
`text_selection/text_cursor_state.rs:73` が行選択に使っており、mIV のテキスト入力でも
効いている。framework 側で消すとそれを道連れにする。**「そのクリックが何を意味するか」は
ウィジェット側の判断**であり、テキスト欄は triple を使い、ファイル一覧は使わない。

## 1. 実測 (推測ではない)

perf log の `cell_signal` (同一セル idx 9、`grid_open_selected_item_on_click = false`、
`grid_click_selection_mode = "Check"`):

```
81.875 idx=10 dbl=False prev=169ms prev2=891ms  -> accepted   (triple 窓 1000ms の内側)
82.347 idx=9  dbl=False prev=472ms prev2=641ms  -> selection_only (別セル。同セル条件が効いている)
82.858 idx=9  dbl=False prev=511ms prev2=983ms  -> accepted   (前回から 511ms = 窓の外なのに開く)
```

`double_clicked` が false で開いている = **`triple_clicked()` 経由**。利用者から見れば
単発のクリックで開いている。

## 2. やること

グリッドのセルの起動判定を、**自前のペアリング状態 1 つ**にする。

```
クリック(idx, now):
    対になる = 直前が Some(prev) かつ prev.idx == idx かつ (now - prev.at) < delay
    開く      = 対になる
    直前      = 対になる ? None : Some(idx, now)     // 開いたら連なりを終える
```

- `delay` は `ctx.options(|o| o.input_options.max_double_click_delay)`。**§2.9 で OS から
  取った値をそのまま使う。新しい閾値を作らない。**
- `now` は `ctx.input(|i| i.time)`。
- 起動判定から **`response.double_clicked()` と `response.triple_clicked()` を外す**。
  グリッドは egui のクリック回数を読まない。
- **`response.clicked()` は引き続きトリガーに使う。** 移動量 (`max_click_dist`) と押下時間
  (`max_click_duration`) の判定は egui のままにする。置き換えるのは**対にするかどうかだけ**。
- 既存の `last_primary_clicked_grid_idx` ([app.rs:9184](../../src/app.rs:9184)) は
  **このペアリング状態へ吸収して削除する**。同セル条件は構造に内包される。
- セル以外へのクリック (空白等) は対を切る。既存の
  `non_cell_click_breaks_grid_double_click_pair` が守っている挙動を維持する。
- `items_generation` が変わったら対を切る (別の一覧の同じ idx と対にしない)。
- §2.5 の再クリック open の経路は**そのまま**。あちらは「選択済み項目のクリック」で、
  対の話ではない。

## 3. これで何が起きるか

| 場面 | 結果 |
| --- | --- |
| 開く → Esc → 単発クリック | **開かない** (今回の症状) |
| クリック、クリック (遅い)、クリック | 3 回目で**開く** (`d493180b` の意図を維持) |
| 開く → Esc → ダブルクリック | 2 回目で**開く** |
| セル A → セル B を素早く | **開かない** (`43439bb8` の意図を維持) |
| 通常のダブルクリック | 開く |
| タッチのダブルタップ | 従来どおり開く |

## 4. やらないこと

- egui を vendor しない / パッチしない。
- 新しい時間閾値を作らない。使うのは OS 由来の既存値だけ。
- グリッド以外の `double_clicked()` 消費箇所を変えない (§2.10 で別途)。
- §2.5 の設定と条件を変えない。
- 「開いた直後は N ms 無視」のような時間窓を足さない。区切りは**状態**であって時間ではない。

## 5. テスト

**既存 15 本 (`grid_reclick_open_tests`) が、意図の変わらないものは無修正で通ることが
挙動を変えていない証拠になる。** 通らなくなったものは、なぜ意図が変わったかを報告する。

追加:

1. 開く → 同じセルを単発クリック → **開かない**。
2. 開く → 同じセルをダブルクリック → **開く**。
3. クリック 3 回 (2 回目は窓の外、3 回目は窓の内) → 3 回目で開く。
4. セル A → セル B (窓の内) → 開かない。
5. `items_generation` が変わったら対が切れる。
6. mutation: 「開いたら None にする」を外すと 1 が落ちる。「idx 一致」を外すと 4 が落ちる。
   両方を報告する。

## 6. ドキュメント

- [next-release-backlog.md](../next-release-backlog.md) §2.6 / §2.9 の末尾に追記する
  (**閉じた記録は消さない**)。egui のクリック回数を読むのをやめた理由、egui を触らない
  判断とその根拠 (テキスト行選択) を書く。
- [docs/spec.md](../spec.md) に、一覧のダブルクリック判定が OS の時間で同一セルの対に
  なることを書く。

## 7. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。
