# Codex レビュー依頼: VST3 プラグイン GUI 非表示状態の永続化 (`user_hidden`)

## 概要

ユーザーが個別に GUI × で閉じたプラグインの状態を **再起動後も維持する** 機能の追加。
`docs/vst3-todo.md` の P1 タスク「プラグイン GUI 非表示状態の永続化」を実装。

## レビュー基準コミット

```bash
git diff a475770..HEAD
```

(= `a475770 fix(vst3): Codex P2 review 反映 (active filter + timeout 延長)` 以降の差分)

## 背景

- 既存実装: `PluginSlot.user_hidden: bool` (= ランタイムフィールド) のみ存在。
  「GUI ×」(プラグインウィンドウの × / 管理パネルの「GUI ×」ボタン) でこれが
  true になり、`set_all_guis_visible(true)` (= VST ボタンで一斉表示) が skip 対象にする。
- 課題: 再起動すると `user_hidden=true` 情報が失われる → 一斉表示で再表示される。
- ユーザー要望 (2026-04): 「× で閉じた状態を再起動後も維持してほしい」。

## 実装概要

### 1. `Vst3PluginEntry` (= settings.json) に `user_hidden: bool` を追加

```rust
// src/settings.rs
pub struct Vst3PluginEntry {
    pub path: String,
    #[serde(default)] pub bypass: bool,
    #[serde(default)] pub state: Option<String>,
    #[serde(default)] pub user_hidden: bool,  // ← 新規
}
```

`#[serde(default)]` により旧 settings.json も読み込み互換 (= false で復元)。

### 2. `DspBridge::add_plugin` に `user_hidden: bool` を引数追加

`src/video/dsp/mod.rs`:

```rust
pub fn add_plugin(
    &self, plugin_path: &str, sample_rate: u32, block_size: u32,
    bypass: bool, user_hidden: bool,  // ← 新規
) -> Result<usize, String>
```

slot 構築時にこの値を `PluginSlot.user_hidden` に格納する。
`kick_off_vst3_startup` (= app.rs) と `kick_off_vst3_chain_rebuild` (= vst3_actions.rs)
の両呼出側で `entry.user_hidden` を渡す。

### 3. `show_slot_gui` 新規ウィンドウ作成 (Path B) で `user_hidden=false` を解除

```rust
// src/video/dsp/mod.rs Step 4 (新規ウィンドウ作成成功時)
if let Some(slot) = inner.slots.get_mut(idx) {
    slot.gui_hwnd = hwnd;
    slot.gui_visible = true;
    slot.user_hidden = false;  // ← 新規 (既存 hwnd!=0 の早期 return path と同様)
    ...
}
```

起動後に `user_hidden=true` で復元された状態で、ユーザーが「GUI」ボタンで明示
要求した時に GUI が初めて作成される (= Path B) → user_hidden 解除する必要がある。
既存 Path A (= hwnd!=0 の高速トグル) では既に解除していた。

### 4. `pump_gui_signals` を `() → Vec<usize>` に変更

```rust
// src/video/dsp/mod.rs
pub fn pump_gui_signals(&self) -> Vec<usize> {
    ...
    for &idx in &close_targets {
        self.user_hide_slot_gui(idx);
    }
    ...
    close_targets  // user_hidden 化された slot idx を返す
}
```

App 側 wrapper (= vst3_actions.rs `vst3_pump_gui_signals`) でこれを受け取り、
`settings.vst3_plugins[idx].user_hidden = true` を更新して `save()`。

### 5. パネル「GUI / GUI ×」ボタンハンドラ (vst3_manager.rs)

show / hide 両方で settings 側を同期。show 経路は **idempotent** (= 既に
正しい状態なら save しない) になるよう、`vst3_gui_visible` と `user_hidden`
の両方の change-detection を AND 結合。hide 経路は元から idempotent だった。

## 触ったファイル

```
src/settings.rs                  Vst3PluginEntry に user_hidden 追加 + migration init
src/video/dsp/mod.rs             add_plugin 引数追加 / show_slot_gui Path B / pump_gui_signals 戻り値
src/app.rs                       kick_off_vst3_startup の add_plugin 呼出
src/ui_dialogs/vst3_actions.rs   vst3_pump_gui_signals が settings 同期 / chain-rebuild
src/ui_dialogs/vst3_manager.rs   show/hide ボタン handler が settings 同期
src/ui_dialogs/preferences.rs    page_vst3 の Vst3PluginEntry 構築
docs/vst3-todo.md                完了マーク
```

## 設計判断ポイント (= 確認したい点)

### A. `user_hidden` の二重管理 (slot + settings) は妥当か

- **slot 側 (= ランタイム)**: `set_all_guis_visible(true)` の skip 判定で参照。
  毎フレーム可能性のあるパスなので Mutex 越しに読む。
- **settings 側 (= 永続化)**: 起動時に slot へコピー。GUI 操作時に逆方向に更新。
- DspBridge は settings への参照を持たない設計 (= レイヤ分離)。よって両側を
  別々に保持し、App 層が同期役を担う構造。
- 代替案: settings から直接読む / 直接書く流儀にする → DspBridge が settings
  に依存することになり、レイヤ違反になる。今の双方向同期で良いか?

### B. `vst3_pump_gui_signals` の早期 return + 線形スキャン

`pump_gui_signals` は毎フレーム呼ばれるが、戻り値 `Vec<usize>` が空の場合は
即 return (= `if user_hidden_indices.is_empty() { return; }`)。close 通知が
ある時のみ settings の get_mut + save を走らせる。これで hot-path への影響を
排除できているか?

### C. show ボタン経路の change-detection

```rust
let mut changed = !self.settings.vst3_gui_visible;  // 元から true なら no-op
self.settings.vst3_gui_visible = true;
if let Some(entry) = self.settings.vst3_plugins.get_mut(idx) {
    if entry.user_hidden { entry.user_hidden = false; changed = true; }
}
if changed { self.settings.save(); }
```

これで idempotent (= 連続クリックで同一状態の場合は disk write しない)。
hide 経路も同様に idempotent。問題ないか?

### D. `pump_gui_signals` の `for &idx in &close_targets` + 末尾で `close_targets` を
return する選択 (元々の clone() を回避)

```rust
for &idx in &close_targets {
    self.user_hide_slot_gui(idx);
}
...
close_targets  // ownership move
```

clone を避けつつ末尾で move return。可読性 / 効率の観点で問題ないか?

### E. `show_slot_gui` Path A / Path B の `user_hidden = false` 二重書き

両方の path で書く。早期 return (Path A) と Step 4 (Path B) の構造的分離が
あるため、関数頭で 1 回書く統一は危険 (= ロード未完了エラーで return する
パスがあり、その手前で書くと「ロード失敗なのに user_hidden=false にされる」
不整合を生む)。現状の二重書きで OK か?

## 検証

```bash
cargo check --bin mimageviewer-core    # OK
```

実機検証 (= 手作業):
1. プラグインを 2 つ追加し、1 つを GUI × で閉じる
2. mIV を再起動 → settings.json から `user_hidden=true` で復元される
3. VST ボタンで一斉表示 → × で閉じた方は表示されない (= skip される)
4. 管理パネルで「GUI」ボタンをそのスロットに対して押す → 表示されて
   `user_hidden=false` に永続化される

## 期待するレビューフォーカス

1. **[P1] バグ**: 上記設計判断 A-E のいずれかに見落としや race があるか?
2. **[P2] レイヤ違反**: DspBridge ⇔ settings の同期パスは健全か?
3. **[P2] レビュー観点**: 起動時に `user_hidden=true` 復元 → 後で
   `set_all_guis_visible(true)` 経由で skip される流れに、見えない
   タイミング依存が無いか?
4. **[P3] 改善案**: `Vst3PluginEntry` の field 列挙 (path / bypass / state /
   user_hidden) を `add_plugin` に渡す形に再構成するべきか?
   (= 引数 sprawl 緩和)

返答は P1/P2/P3 サマリ形式で。
