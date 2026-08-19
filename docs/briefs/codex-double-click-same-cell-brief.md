# §2.9 追補 — ダブルクリックの「同じ場所」条件が抜けている

対象: 利用者報告 (2026-08-19、§2.9 を実機確認中)。
**v3.1.2 に入れる。§2.9 が入って初めて実害が出た。**

症状: **チェック方式でスペースでチェックした後、別のファイルを素早くクリックすると
そのファイルが開いてしまう。**

## 0. 原因 (egui のソースで確定)

`egui-0.33.3` の `input_state/mod.rs` は、**時間だけでダブルクリックを決める**。

```rust
// 1215
let double_click = (time - self.last_click_time) < self.options.max_double_click_delay;
```

- 保持しているのは `last_click_time` / `last_last_click_time` だけで、
  **`last_click_pos` に相当するものが無い** (1104-1140 のフィールド定義を確認済み)。
- `could_any_button_be_click()` (1519) が見ているのは
  **その 1 回の press→release の中での移動量**であって、前のクリックからの距離ではない。

つまり **egui では、窓の中に入った 2 回のクリックは、どこを押していてもダブルクリックになる。**
1 回目がセル A、2 回目がセル B でも、B が `double_clicked()` になる。

**Windows は時間と距離の両方を条件にする** (`SM_CXDOUBLECLK` / `SM_CYDOUBLECLK`、既定 4x4px)。
§2.9 で時間だけを OS に合わせた結果、**窓が 300ms → 500ms へ広がり、条件の片方だけが緩んだ**。
だから今回になって出た。

**§2.9 を戻すのではなく、抜けている条件を足すのが正しい。** 時間を OS に合わせたのは正しく、
足りないのは Windows が対にしている「同じ場所」の側である。

## 1. やること

グリッドのセルで、`response.double_clicked()` を**直前のクリックが同じセルだったときだけ**
起動として扱う。

- 直前の primary click の **idx** を App が持ち、`response.clicked()` のときに更新する。
- **更新は判定の後**。`selected_before_click` と同じ順序の問題で、先に更新すると常に
  「同じセル」になって条件が効かない。
- 起動の述語は現在
  `activate = response.double_clicked() || (response.clicked() && grid_reclick_open_allowed(...))`
  ([ui_main.rs:12780](../../src/ui_main.rs:12780) 付近)。前半に同セル条件を足す。
- **ピクセル距離までは見ない。** セル内で少し動いた 2 回目は従来どおり開く。ここを
  Windows と同じ 4px にすると、**今まで開けていた操作が開かなくなる**方向の変更になる。
  グリッドで意味のある単位はセルなので、セル一致で止める。

### 1.1 適用範囲

**今回はグリッドのセルだけ。** 同じ穴は他の `double_clicked()` 消費箇所
(ナビゲータ / シークバー / ダイアログ等) にもあるが、報告された実害はグリッドで、
セルが密に並んでいるぶん誤爆しやすい。他の箇所は backlog へ別項として起票し、
**この修正では触らない**。

## 2. やらないこと

- §2.9 を戻さない (時間を OS に合わせたのは正しい)。
- egui 側を patch しない。
- 時間窓を独自に足さない。使うのは既に OS から取った値だけで、**新しい閾値を導入しない**
  (憲法 §2 規則 5)。
- ピクセル距離の判定を足さない (§1)。

## 3. テスト

1. セル A をクリック → 窓の内側でセル B をクリック → **B は開かない** (今回の報告)。
2. セル A をクリック → 窓の内側でセル A を再びクリック → **開く** (通常のダブルクリック)。
3. 窓の外側でセル A を 2 回 → 開かない (設定 OFF のとき)。
4. §2.5 の設定 ON で、窓の外側の再クリックが従来どおり開く。
5. 空白 → セルの順など、直前のクリックがセルでない場合に開かない。
6. 既存の double-click テスト
   (`double_click_still_opens_in_check_mode_exactly_once` ほか) が無修正で通る。
   **赤くなったら報告して止まる。**
7. mutation: 同セル条件を外すと 1 が落ちることを確認して報告する。

## 4. ドキュメント

- [next-release-backlog.md](../next-release-backlog.md) §2.9 の末尾に追記する
  (**閉じた記録は消さない**)。egui が位置を見ないこと、Windows は見ること、
  グリッド以外は別項にしたことを書く。
- グリッド以外の `double_clicked()` 消費箇所について**新規エントリを起票する**
  (番号は採番して報告)。

## 5. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。
