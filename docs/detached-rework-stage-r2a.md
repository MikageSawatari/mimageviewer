# Stage R2a 指示書: DetachedWindowRuntime の導入 (状態集約の器、挙動不変)

正本プラン: [detached-rework-plan.md](detached-rework-plan.md)
**着手前に必ずプラン §2 (憲法) を読むこと。**

- 位置付け: R2 (状態集約) の第 1 サブステージ。**挙動不変**の機械的な集約で、
  R2b (reducer + メディア live-park) の土台を作る。
- 実装: Codex / 検収: Fable / 実機 smoke: **なし** (R2b 完了後にまとめて実施)

## 1. 目的

detached 窓ごとの状態を 1 つの構造体に集約し、bool/Option の散在 (BA-7) を
解消する器を作る。R2a では器と遷移の**記録**まで。遷移が挙動を**駆動**するのは R2b。

## 2. 実装内容

### 2.1 型の導入

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetachedWindowState {
    Opening,    // OS 窓生成 → hwnd 確定待ち
    Active,     // live content (active context または legacy linked)
    Parked,     // frozen texture のみ (現行の passive)
    ParkedLive, // メディア再生継続中の非アクティブ窓 (R2b で使用開始、R2a では定義のみ)
    Resuming,   // Parked → Active 復帰中 (bundle swap-in)
    Closing,    // Close 送信 → 破棄確認待ち
}

pub(crate) struct DetachedWindowRuntime {
    pub window_id: u64,
    pub state: DetachedWindowState,
    pub hwnd: u64,      // 0 = 未確定。DetachedWindowHwndRegistry を吸収
    pub pinned: bool,
    pub linked: bool,   // メイン選択との連動
}

// App: detached_window_runtimes: IndexMap<u64, DetachedWindowRuntime> 相当
```

### 2.2 hwnd registry の吸収

- R1 の `DetachedWindowHwndRegistry` (HashMap) を runtime map に吸収する。
  既存の registry API (`hwnd_alive` / `set` / `clear` / `select_created_hwnd` /
  `select_unclaimed_hwnd`) は runtime map の上の同名メソッドとして維持し、
  呼び出し側の diff を最小化する。cfg(test) の synthetic 注入も維持。
- **hwnd の SSoT は runtime に一本化** (registry と runtime の二重管理はしない)。

### 2.3 遷移の記録 (shadow state)

- `fn transition_detached_window_state(&mut self, window_id, new_state, reason: &'static str)`
  を 1 つだけ作り、既存の遷移点から呼ぶ:
  - 窓の新規 open (Opening)、hwnd 確定 + 表示 (Active)
  - `pause_current_active_detached_viewer_context` 成功 (Parked)
  - `activate_detached_image_window_snapshot` / resume 開始 (Resuming) → 完了 (Active)
  - close 要求 (Closing) → 破棄確認 (runtime から削除)
- ログ形式: `state_transition window_id=N from=X to=Y reason=Z`。
  想定外遷移 (例: Parked → Closing 以外からの削除) は `state_transition_unexpected`
  で警告ログ (panic はしない)。
- **R2a では state は診断ログ専用** (読んで挙動を変えるのは R2b)。ただし書き込みは
  この関数 1 本に限る。既存の散在フラグ (`detached_viewer_pin_active` 等) は
  R2a では**触らない** (削除は R2b で対応するフラグから順に)。

### 2.4 pinned / linked の初期値同期

- runtime 生成時に既存フラグ (pin_active / independent) から写し、以後の
  pin 変更点で runtime も更新する (書き込み 2 箇所化は R2b までの過渡と
  コメントで明記)。

## 3. やらないこと (スコープ外)

- 挙動の変更 (park/close/activate の分岐、placement、focus)
- 散在フラグの削除 (R2b)
- placement の runtime への移設 (R2c)
- メディア live-park の実装 (R2b。ParkedLive は enum 定義のみ)

## 4. 完了条件

- [ ] `DetachedWindowHwndRegistry` 構造体が消え、runtime map に一本化
      (grep: `DetachedWindowHwndRegistry` 0 件)
- [ ] R1/R1b の hwnd テスト群が runtime 版として全て緑 (テスト名は維持または
      改名一覧を完了報告に)
- [ ] 全遷移点で `state_transition` ログが出る (実装した遷移点の一覧を完了報告に)
- [ ] `cargo fmt --check` / `cargo test --bin mimageviewer-core` / `cargo test` 緑
- [ ] 挙動不変の確認: 既存 detached テスト全緑 + 新規フラグ追加なし

完了報告後、Fable 検収。実機 smoke は R2b とまとめて行う。
