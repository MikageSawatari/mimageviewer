# egui 0.37.0 のローカル変更（段2a-1）

原本とchecksum・VCSの記録は [upstream-provenance.json](../upstream-provenance.json) を参照。
0.37.0原本を基準に、旧`vendor/egui`の既存パッチだけを付け直す。
rootの依存切替・製品操作差の受入れはこの段階に含めない。
README、Cargo.toml.orig、同梱Cargo.lockは原本を保持する。

## 差分台帳

分類は [更新計画 §4.8](../../docs/egui-0.37-update-plan.md#48-vendorパッチ台帳) に従う。

| 項目 | 分類 | 変更と保証 |
| --- | --- | --- |
| `PointerState::released_buttons_with_click_counts` | (b) 保持 | `src/input_state/mod.rs`。本家が判定した全button releaseを順に値へ射影し、非clickは`None`で残す。判定やイベントの生成・消費は変更しない |
| 旧クリック分類fixture | (c) 新分類へ移行 | 旧位置列`(20,20)→(30,30)→(40,40)→(50,50)`は全てcount 1。別の近接位置列で1/2/3/3を検査し、全button・非click・focus区間・順序・discard後の非再生を維持する |
| `InputState::discard_pending_scroll` | (b) 新ownerへ適応 | `src/input_state/mod.rs`。0.37の`WheelState::unprocessed_wheel_delta`だけをゼロにし、scrollとwheel zoomの未来passへの残量を終端する |
| repaint delay witness | (b) 保持 | `src/context.rs`とCargo.tomlの`miv-repaint-witness` feature。現在viewportのschedulerを読み、outstandingがあれば0、有限delayならその値、要求なしなら`None`。状態を変更しない |
| 本家click分類 | (a) 差分不要 | 0.37の距離・時間条件をそのまま使用する。旧分類を再実装せず、本家の事実を別回帰で固定する |
| license本文 | (b) 保持 | 旧vendorのMIT / Apache本文をbyte変更なしで同梱する |

IME中断要求のviewport所有・pass出力集約と、非描画viewport退役入口は後続段2a-2／2a-3の範囲であり、本段では変更しない。

## Wheel終端の境界

前面popup等の既存入力ownerが終了するpassの終端に呼ぶ。現在passのpublic deltaと
wheel eventの消費は呼出側が行う。新WheelStateでは旧scroll／zoomの二つのqueueが一つに
統合されているため、未配送deltaだけを破棄する。現在delta、pointer、focus、touch、
gestureのstatus・source・時刻・modifier latchは保持し、次passの新規wheelは通常どおり処理する。
開いている間の平滑化を毎pass消す用途や、汎用の非表示入力終端には使わない。

## 回帰と検証の扱い

- 射影は旧2回帰を移植し、実`Context::run_ui`でdiscardを通す。検査後のFullOutputは
  `drop_without_applying_deltas`でheadless明示破棄する。
- 分類は近接／離れたclick、6 logical points未満・ちょうど・超過（倍率1／2）、
  doubleの厳密な時間境界、tripleの直近二区間条件を検査する。
- wheelはscroll／zoom双方の未配送残量、現passと所有状態の保持、後続idleでの非再生、
  新規wheelの受理を検査する。終端しない比較経路では残量が再生されることも確認する。
- witnessは要求なし／遅延／即時／outstandingを読み、schedulerが変わらないことを検査する。

準備graph・実行件数・command・結果は親担当の段2a検証台帳に記録する。
本書は旧版の成功結果を0.37へ流用せず、未実行の回帰を成功済みとは扱わない。

## ライセンス

packageの`MIT OR Apache-2.0`宣言は維持する。原本crateには本文がないため、
旧`vendor/egui`の同名本文をコピーする。

| コピー元 | SHA-256 |
| --- | --- |
| `../../vendor/egui/LICENSE-MIT` | `3fceb7b317f3c70451942b0e123723a2e250c7eaf970ce4b490a57a94c5588ff` |
| `../../vendor/egui/LICENSE-APACHE` | `1ac50a5abd5cff5331d59928d83369b66b0099a88f0111dc2175526b3c666dec` |
