# backlog §1.92 — `VideoToggleAudioMode` に 1 つの意味の持ち主を与える

対象: backlog §1.92「別ウィンドウの動画再生中に外部アプリから戻ると <kbd>Z</kbd> だけ効かない」。
観測は完了済み ([native-video-key] 計装、commit `b9448ca8`)。**今回は原因修正**。

detached リワーク凍結下の作業。**Codex Sol と「症状パッチではなく構造的修正である」ことの合意
取得済み** (2026-08-18、条件付き同意)。条件は本 brief の §2 に取り込んである。
実装後、判断理由を [detached-rework-plan.md](../detached-rework-plan.md) §11 に記録すること。

## 0. 原因 (実測で確定)

`VideoToggleAudioMode` は **2 方向あるのに、経路ごとに別々の場所が片方ずつ持っている**。

| 遷移 | 現在の持ち主 |
| --- | --- |
| 通常動画 → 音声モード | `handle_native_video_key_event` (native 経路のみ) |
| 音声モード → 動画 | `handle_fs_key_input` の音楽ビュー分岐 (egui 経路のみ) |
| 音声 VST 表示 → 音楽ビュー | `handle_native_video_key_event` の VST gate (native 経路のみ) |

detached では presenter が `WS_CHILD` としてホストの内側にいるため、キーが **egui 経路**へ届く
ことがある。そのとき「通常動画 → 音声モード」に持ち主が居ないので <kbd>Z</kbd> が無反応になる。
P やカーソルが効くのは、それらが egui の `handle_video_input` に mapping を持つため。

実測: 効かなかった Z は `[fs-key] source=fullscreen keys=Z:down` として観測され、
`[native-video-key]` はセッション全体で 1 行 (終了時の Escape) だけだった。

## 1. やること

**`VideoToggleAudioMode` の意味を 1 つの helper に集約し、両経路から呼ぶ。**

```
fn toggle_video_audio_mode(&mut self, ctx, fs_idx, source: VideoAudioEnterSource) -> <typed outcome>
```

現在の状態から遷移先を決める。分岐は 3 つで、**新しい状態も述語も作らない**:

1. 音声 VST 表示中 (`video_audio_vst_active_for`) → `exit_video_audio_vst`
2. 音声モード中 (`video_audio_mode == Some(fs_idx)`) → `exit_video_audio_mode`
3. それ以外 (通常動画) → `enter_video_audio_mode`

`enter_video_audio_mode` は既に presentation (Fullscreen / MainWindow / DetachedWindow) と
placement / source-swap / host 準備状態を**正本として検査している**。**caller 側でこれらを複製しない。**

### 1.1 native 経路

現在の Z の 2 箇所 (VST gate の toggle、match arm の enter) を helper 経由にする。
**挙動は変えない** — 同じ状態で同じ遷移になることをテストで固定する。

### 1.2 egui 経路 (これが今回の追加)

**`handle_video_input`** (egui の動画 action owner。P / seek / 再生などが既にここ) に mapping を足す。
音楽ビュー分岐の隣に新しい分岐を作らない (動画 action の dispatch がさらに分散するため)。

- `consume_action_no_repeat` を使う (native 側の `!key.repeat` と対にする)。
- guard は既存の動画 action と揃える (context menu / IME / `wants_keyboard_input` / modal)。
- **presentation で分岐しない**。detached 限定にもしない。egui にキーが届くのは detached だけで
  なく、in-window、focus handoff、コンテキストメニュー終了直後にもある。

### 1.3 音楽ビュー分岐の扱い

既存の音声→動画分岐は helper 呼び出しへ寄せてよいが、**guard と consume の条件は変えない**
(`no_repeat` の理由が Codex P2 としてコメントに残っている)。

## 2. Codex の反証を取り込む (必須)

**音声 VST 表示中は `fs_music_view_active` が意図的に false になる。**
そのため「`!fs_music_view_active` なら enter」と書くと、VST 表示中の <kbd>Z</kbd> が
`enter_video_audio_mode` に入り `AlreadyActive` で拒否され、**VST が閉じなくなる**。
これは提案そのものへの具体的な反証であり、§1 の 3 分岐はこれを避けるための構造。

`enter_video_audio_mode` の `AlreadyActive` gate を排他の代用にしない。

## 3. やらないこと

- FsVideo 全 action の dispatcher 集約は**別作業**。今回は `VideoToggleAudioMode` 1 つだけ。
  (Codex 見積り: 4〜7 ファイル / 数百〜1,000 行の中規模改修。今回に必須ではない)
- focus、ウィンドウ順序、z-order、detached 述語、presenter の生成/破棄に触れない。
- 時間窓 (debounce / grace / settle) を足さない。
- `enter_video_audio_mode` の**拒否条件を変えない**。

## 4. テスト (mutation を通すこと)

3 状態 × 2 経路を handler レベルで固定する。

- **T1** 通常動画 + egui 経路の Z → enter が要求される (**修正前に落ちること**を確認して報告)
- **T2** 通常動画 + native 経路の Z → 従来どおり enter (退行が無いこと)
- **T3** 音声モード + egui 経路の Z → exit
- **T4** 音声 VST 表示 + native 経路の Z → `exit_video_audio_vst` (**`AlreadyActive` に落ちない**こと)
- **T5** egui 経路で repeat の Z は遷移を起こさない
- **T6** §4.2 の所有権テストが通ったまま (画像側が Video / Audio の Z を消費しない)

各テストについて、対応する分岐を削除 / 反転して**実際に落ちること**を確認し、結果を報告に含める。

## 5. 記録

- [detached-rework-plan.md](../detached-rework-plan.md) §11: 触れた範囲と、構造的修正と判断した
  理由 (= 欠けていた action→既存遷移の mapping を補うものであり、新しい状態・時間窓・guard を
  作らない) を記録する。
- [next-release-backlog.md](../next-release-backlog.md) §1.92: **エントリ末尾に追記**して閉じる。
- 併せて **`VideoAdjustSlot1..10` も native dispatcher にしか見当たらない** (Codex 指摘)。
  同型の parity 欠落の可能性があるので、**確認して backlog に新規エントリとして起票**する
  (修正は別作業)。

## 6. 機械チェック

```
cargo fmt --all && cargo fmt --all -- --check
cargo check -p mimageviewer --bin mimageviewer-core
.\scripts\test-full.ps1
.\scripts\build-dev.ps1
```

commit / stage はしない。ブランチは `master`。**別セッションが同じ作業ツリーで動いている**ので、
自分が触っていないファイルの変更を戻したり stage したりしない。
