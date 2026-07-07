# Stage R2b 検収所見 #2: ParkedLive 窓がクリックで復帰できない (fix1 の入力抑止が復帰クリックまで飲み込む)

正本プラン: [detached-rework-plan.md](detached-rework-plan.md) /
前回所見: [findings-1](detached-rework-stage-r2b-findings-1.md)

## 実機ログの事実 (2026-07-06 08:2x、Fable 解析)

```
33.758 state_transition id=4 Active→Closing→ParkedLive reason=park_legacy_live_media  ← park 成功
40.29〜 passive_event id=4 focused=true focused_prev=true ... が毎フレーム続くのみ
       (passive_activate_queued は一度も出ない)
41.914 state_transition id=4 ParkedLive→Closing reason=passive_close                 ← 閉じられるまで
                                                                                        ParkedLive のまま
```

ユーザーは動画窓をクリックして「復帰した」と認識した (窓が前面化し映像も再生中の
ため見分けが付かない) が、実際には **ParkedLive→Resuming→Active の遷移が一度も
起きていない**。ホイールが効かないのは「復帰後の不具合」ではなく、**ParkedLive の
まま**なので fix1 の入力抑止が仕様どおり効いているだけ。

## 根因 (F5)

detached 動画の窓は native presenter child が全面を覆うため、クリックは
**native 入力経路**に入る。fix1 の `native_video_parked_live_input_suppressed` /
`native_video_output_event_allowed_while_parked_live` は ParkedLive 中の native
入力を**クリックも含めて全部 no-op** にしたため、「クリック = 復帰」の唯一の
入力が死んだ。egui 側の `passive_activate via=pointer` は presenter child に
覆われていて到達しない (focused=true になるだけで pointer_activation は立たない)。

## 修正要件

1. ParkedLive 中の native 入力フィルタで、**左クリック (button down→up) だけは
   「窓 window_id の復帰要求」に変換**して App へ通す。それ以外 (ホイール・キー・
   HUD ヒットテスト・右/中クリック・ダブルクリック) は引き続き no-op。
   - 復帰要求は既存の ParkedLive 専用復帰経路 (fix1 で実装済み) に接続する。
   - シーク位置へのクリックスルーはしない (クリックは復帰のみ。復帰後の 2 回目の
     クリックから通常動作)。
2. 復帰完了後は通常の native 入力が全て戻ることをテストで固定
   (「復帰後にホイール nav が効く」= 今回ユーザーが踏んだケースの回帰テスト)。
3. 回帰テスト (cfg(test) 経路で):
   - ParkedLive 中: native click → activation 要求が queue される / wheel・key → no-op
   - 復帰後: native wheel が通常処理に到達する
4. 副次確認: 静止画の Parked 窓は egui 経路 (`passive_activate via=pointer`) で
   従来どおり動いていることを既存テストで確認 (今回の変更が触らないこと)。

## 完了条件

- [ ] 上記テストが存在して緑、既存 parked_live / still_window テスト緑
- [ ] `cargo fmt --check` / `cargo test --bin mimageviewer-core` / `cargo test` 緑
- [ ] コミットメッセージに `R2b fix2` を含める
- [ ] `.\scripts\build-release.ps1` で実機バイナリ再準備

実機再検証シナリオ (ユーザー): 動画 detached 再生 → 他窓クリックで live-park →
ParkedLive 窓上でホイール (**何も起きない**) → **クリック 1 回で復帰** →
ホイールで次/前の動画へ切替できる → シーク・音量操作可能 → もう一度他窓 →
live-park → クリック復帰、の 2 往復。
