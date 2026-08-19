# §2.6 追補 — ダブルクリックが数えられなかった理由を記録する

対象: 利用者の実機再現 (2026-08-19)。**perf log で症状を捕まえた。**

## 0. ログが示したこと (推測ではない)

PDF を Esc で閉じた直後、左下にアイドル高画質化が出ている状態で、同じ PDF タイル (idx 9) を
3 回クリックした記録:

| 押下 | 離す | `first_click` | `double_clicked` | 直前の離すからの間隔 |
| --- | --- | --- | --- | --- |
| 165.637 | 165.735 | true | **false** | — |
| 166.295 | 166.384 | true | **false** | 649ms |
| 166.489 | 166.612 | true | **false** | **228ms** |

同じ session の成功例 (idx 12): 162.945 押下 → 163.057 離す、`double_clicked: true`、
直前の離すからの間隔 **400ms**。

**400ms は成立し、228ms は成立していない。** したがって**閾値の問題ではない**。
実測でこの機の `GetDoubleClickTime()` は 500ms で、§2.9 の前の egui 既定 300ms と比べても
228ms はどちらの窓の内側にある。

egui の判定は `(time - last_click_time) < max_double_click_delay` の 1 行だけなので、
228ms で false になるには **`last_click_time` が直前のクリックで更新されていない**ことになる。
更新は `InputState` が「クリックである」と判定したときだけ起きる。

一方 `first_click` は `response.clicked()` で、これは
`FAKE_PRIMARY_CLICKED || clicked_by(Primary)` である
(`egui-0.33.3/src/response.rs:157`)。**pointer 由来でないクリック (fake) でも true になる。**

**ここから先はログに材料が無い。** 推測で直さないこと。

## 1. やること — 既存の `cell_signal` に 4 つ足す

新しいイベントを作らない。発火条件も変えない (cell signal が起きたときだけ出る)。

| 属性 | 取得元 | 何が分かるか |
| --- | --- | --- |
| `time_since_last_click` | `ctx.input(\|i\| i.pointer.time_since_last_click())` | **egui 自身が測っている間隔**。228ms なのか、もっと大きいのか |
| `max_double_click_delay` | `ctx.options(\|o\| o.input_options.max_double_click_delay)` | §2.9 が本当に効いているか |
| `clicked_by_primary` | `response.clicked_by(egui::PointerButton::Primary)` | `first_click` が fake 由来かどうか |
| `double_clicked_by_primary` | `response.double_clicked_by(egui::PointerButton::Primary)` | 上と対にして読む |

`GridCellSignal` ([grid_input_diagnostics.rs:144](../../src/grid_input_diagnostics.rs:144)) に
field を足し、`report_grid_cell_signal` の emit へ載せる。**呼び出し側 3 箇所すべて**で埋めること
([ui_main.rs:12841](../../src/ui_main.rs:12841) ほか、badge / 通常 / 既存の 2 箇所)。

## 2. 読み方 (backlog に書くこと)

- `time_since_last_click` が実測間隔 (0.228) と一致し、`max_double_click_delay` がそれより
  大きいのに `double_clicked_by_primary` が false → **egui の判定そのものに矛盾**。
  pass 構成や widget id を疑う。
- `time_since_last_click` が実測間隔より**大きい** → 直前のクリックが `last_click_time` を
  更新していない。**`clicked_by_primary` が false で `first_click` が true なら fake 由来**で、
  pointer のクリックとして成立していない。そこが本命になる。
- `max_double_click_delay` が 0.3 のまま → §2.9 がその Context へ届いていない。

## 3. やらないこと

- **症状を直さない。** guard / retry / 閾値の調整を入れない。原因が確定していない。
- 発火条件を変えない。**抑制条件を調査対象の値に依存させない** (`double_clicked` が
  false のときだけ出す、等にしない。成功例と比べられなくなる)。
- §2.9 と同セル条件を戻さない。どちらもこの症状の原因ではないことがログで分かっている。

## 4. テスト

1. 4 属性が `cell_signal` に**常に**出る (成功時も失敗時も)。
2. `clicked_by_primary` と `first_click` が食い違うケースを作れるなら固定する。
3. 既存の grid diagnostics テストが無修正で通る。

## 5. ドキュメント

- [next-release-backlog.md](../next-release-backlog.md) §2.6 に、今回のログで**閾値と位置が
  原因でないと確定した**こと、§2 の読み方を追記する。**エントリは閉じない。**

## 6. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。
