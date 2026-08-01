# ブリーフ: native video window の health detection (§7.3)

実装 = Codex Sol / レビュー = ClaudeCode。v2.9.1 出荷前。

正本は [native-video-window-thread-plan.md](native-video-window-thread-plan.md) の **§7.3
「再発の検知」**。本ブリーフはその前倒し実装の範囲と理由をまとめたもので、設計を上書きしない。
着手前に同計画書の §4.2 (thread boundary)、Stage 4 実装記録、§7.3 を読むこと。

## 1. なぜ今やるか

Stage 4 で P1 の ownership invariant は production で成立済み (2026-08-01 に実機で再確認:
render を意図的に停止させた状態で親 HWND 破棄が 2.5ms、pump ping 0.04ms)。しかし
**Stage 5 / 6 / 7 は未着手**で、次版では動画経路に新機能 (別 worktree で進行中の Web 配信、
コンテナ詰め替え) が乗る。

そこへ機能を足す前に、**再発時に原因 thread を切り分けられる状態**にしておく。現状は
[src/lib.rs](../src/lib.rs) の汎用 `UI THREAD HANG suspected` しか無く、pump が止まったのか
render が driver 内で止まったのかが区別できない。

**この作業は振る舞いを変えない**ので、Stage 5 / 6 本体より先に入れる。回帰リスクが低く、
その後の実機確認すべての一次資料になる。

実際に 2026-08-01 の §7.2 実機確認でカーソル不具合 (backlog §1.28) が出たが、
**カーソル関連の計装が無いため事後確認できなかった**。同じことを繰り返さない。

## 2. 実装する範囲

§7.3 の全項目。要点だけ再掲する (詳細は正本を読むこと):

- lock-free / latest-value の `NativeWindowHealth` を追加する
- 記録する: pump thread id、presenter / HUD の HWND と epoch、最後に dispatch した message の
  sequence と時刻、pump command の last received / completed request id と時刻、render の
  last started / completed operation (`Attach` / `AcquireSync` / `FenceWait` / `Present` /
  `DCompCommit` / `Detach`) と epoch / 時刻、placement、source generation、visibility state
- **path や media metadata は記録しない**
- watchdog は pump へ generation / sequence 付きの posted ping を送り、ack sequence を atomic に
  観測する。**watchdog thread 自身が `SendMessage` や renderer lock を使わない**こと
  (使うと監視対象と同じ環に入る)
- 判定と出力は §7.3 の 1〜5 のとおり:
  `NATIVE VIDEO WINDOW PUMP STALL` / `NATIVE VIDEO RENDER STALL` / `UI THREAD HANG` 行への
  pump ack age・render operation age・epoch の併記 / transition edge + 10 秒 rate limit /
  debug・test build での `GetWindowThreadProcessId` assertion

### 2-1. §7.3 への追加 (今回のブリーフで足す分)

backlog §1.28 の事後確認を可能にするため、observation に次を含める:

- `cursor_hidden` (render core 側の auto-hide 状態)
- `cursor_within_client` (`NativeWindowObservation` の値)
- `cursor_last_activity` からの経過時間
- 直近の placement 遷移と、その時刻

§1.28 の修正自体は Stage 5 で行う。ここで入れるのは**観測だけ**で、カーソルの挙動を変えない。

## 3. 制約

**3-1. busy polling とログ洪水を作らない。** §7.3 の 4 のとおり transition edge と 10 秒
rate limit で記録する。アイドル時の消費電力に効くので、
[idle-health-check.md](idle-health-check.md) のゲートを通ること。

**3-2. pump → render の逆向き wait を作らない。** §10 の 4 に該当する。観測のために
render 側の lock を取ったり、pump から render の完了を待ったりしない。latest-value slot と
atomic だけで組む。

**3-3. 振る舞いを変えない。** 既存の close / placement / input の semantics に触れない。
このスライスで挙動が変わったら、それは設計ミスとして差し戻す。

**3-4. 記録内容にユーザーデータを入れない。** ファイルパス、メディアのタイトル、
メタデータを含めない (§7.3 明記)。

## 4. テストで縛ること

- watchdog の判定が純関数として単体テストできること (ack age / operation age / 閾値 →
  `PumpStall` / `RenderStall` / 正常 の分類)
- rate limit と transition edge が、同一状態の継続でログを増やさないこと
- `NativeWindowHealth` の書き込みが lock を取らないこと (型 / API 境界で縛る)
- 既存の ignored watchdog test
  `production_parent_destroy_remains_bounded_during_render_stall` が引き続き通ること
  (この health 追加が pump join を遅らせていないことの確認)

## 5. 検証

```
cargo fmt --all -- --check
cargo test -p mimageviewer --lib
cargo test -p mimageviewer --lib -- --ignored --nocapture production_parent_destroy_remains_bounded_during_render_stall
python scripts/check_ui_glyphs.py
```

アイドル時の影響はレビュー側が `scripts/check-idle-health.ps1` の 3 シナリオで確認する。

## 6. スコープ外

- Stage 5 (VST owner handoff、backlog §1.28 のカーソル所有の修正)
- Stage 6 (hidden / source / EOF / placement failure の lifecycle hardening)
- Stage 7 の残り (legacy path 再流入の source / type gate、最終実機 gate)
